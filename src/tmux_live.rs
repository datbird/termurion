//! The live tmux suite: a real tmux server (psmux on Windows, tmux in WSL, or tmux on another
//! machine over ssh) behind the shipped ConPTY, driven by the same gateway, handshake, and
//! controller the app uses.
//! Nothing here opens a window. Every test is ignored and uses its own server name
//! (`-L termurion-test-...`), so it never touches a running multiplexer.
//!
//! Run by hand: `scripts/xt test --release --lib tmux_live -- --ignored --nocapture --test-threads=1`.
//! The tests print one line per fact (`LIVE ...`); docs/probe/2026-10-14-psmux-wsl-tmux-results.md
//! is made from them.
//!
//! The ssh tests need a host: `TERMURION_TMUX_SSH_HOST` (an ssh alias or address; the user's
//! ssh config applies), optionally `TERMURION_TMUX_SSH_USER` and `TERMURION_TMUX_SSH_PORT`. The
//! login must work without a prompt (keys or an agent) and the host needs tmux 3.2 or later.
//! With the host unset those tests print `LIVE ssh SKIP` and pass.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use crate::conpty::{self, Source, dev_vendor_dir};
    use crate::pty::{Pty, spawn};
    use crate::tmux_controller::{Action, Controller};
    use crate::tmux_gateway::{Event, Flags, Gateway, Tag};
    use crate::tmux_handshake::{self, Handshake, Step};
    use crate::tmux_hook::{Hook, Out};
    use crate::tmux_proto::Note;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Flavor {
        Psmux,
        Wsl,
        /// tmux on another machine, reached with ssh.
        Ssh,
    }

    /// The ssh options for the test host, from the environment (None: no host named).
    fn ssh_args() -> Option<Vec<String>> {
        let host = std::env::var("TERMURION_TMUX_SSH_HOST").ok().filter(|h| !h.trim().is_empty())?;
        let mut a: Vec<String> = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=15"].iter().map(|s| s.to_string()).collect();
        if let Some(port) = std::env::var("TERMURION_TMUX_SSH_PORT").ok().filter(|p| !p.trim().is_empty()) {
            a.extend(["-p".into(), port]);
        }
        if let Some(user) = std::env::var("TERMURION_TMUX_SSH_USER").ok().filter(|u| !u.trim().is_empty()) {
            a.extend(["-l".into(), user]);
        }
        a.push(host);
        Some(a)
    }

    /// A short random suffix for a remote server's name, so two runs never meet.
    fn random_name(prefix: &str) -> String {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        format!("{prefix}{:06x}", (t ^ (std::process::id() as u128) << 20) & 0xff_ffff)
    }

    /// One tmux call on the ssh host. Windows' ssh.exe does not leave when its output goes to a
    /// pipe (the remote command is long done), so the output goes to a file and the call is
    /// capped; stdin is closed.
    fn ssh_cli(ssh: &[String], server: &str, args: &[&str]) -> String {
        use std::process::{Command, Stdio};
        let file = std::env::temp_dir().join(format!("termurion-live-ssh-{}-{server}.txt", std::process::id()));
        let Ok(out) = std::fs::File::create(&file) else { return "no temp file".into() };
        let Ok(err) = out.try_clone() else { return "no temp file".into() };
        let child = Command::new("ssh.exe").arg("-n").args(ssh).args(["tmux", "-L", server]).args(args).stdin(Stdio::null()).stdout(out).stderr(err).spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => return format!("spawn error {e}"),
        };
        let end = Instant::now() + Duration::from_secs(30);
        let finished = loop {
            match child.try_wait() {
                Ok(Some(_)) => break true,
                Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(100)),
                _ => break false,
            }
        };
        if !finished {
            let _ = child.kill();
            let _ = child.wait();
        }
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let _ = std::fs::remove_file(&file);
        text.trim().to_string()
    }

    /// One tmux CLI call against a test server, whatever the flavor: stdout and stderr, trimmed.
    fn server_cli(flavor: Flavor, server: &str, args: &[&str]) -> String {
        use std::process::Command;
        let out = match flavor {
            Flavor::Psmux => Command::new("psmux").args(["-L", server]).args(args).output(),
            Flavor::Wsl => Command::new("wsl.exe").args(["-e", "tmux", "-L", server]).args(args).output(),
            Flavor::Ssh => {
                let Some(a) = ssh_args() else { return "no ssh host".into() };
                return ssh_cli(&a, server, args);
            }
        };
        match out {
            Ok(o) => format!("{} {}", String::from_utf8_lossy(&o.stdout).trim(), String::from_utf8_lossy(&o.stderr).trim()),
            Err(e) => format!("spawn error {e}"),
        }
    }

    impl Flavor {
        fn label(self) -> &'static str {
            match self {
                Flavor::Psmux => "psmux",
                Flavor::Wsl => "wsl",
                Flavor::Ssh => "ssh",
            }
        }

        fn start(self, server: &str, session: &str) -> String {
            match self {
                Flavor::Psmux => format!("psmux.exe -L {server} -CC new-session -s {session}"),
                Flavor::Wsl => format!("wsl.exe -e tmux -L {server} -f /dev/null -CC new-session -s {session}"),
                // A pty on the far side (-tt), as typing `ssh host tmux -CC` does.
                Flavor::Ssh => format!("ssh.exe -tt {} tmux -L {server} -f /dev/null -CC new-session -s {session}", ssh_args().expect("TERMURION_TMUX_SSH_HOST").join(" ")),
            }
        }

        /// Ends the test's own server, by its own `-L` name.
        fn stop(self, server: &str, session: &str) {
            use std::process::Command;
            match self {
                Flavor::Psmux => {
                    let _ = Command::new("psmux").args(["-L", server, "kill-session", "-t", session]).output();
                    let _ = Command::new("psmux").args(["-L", server, "kill-server"]).output();
                }
                Flavor::Wsl => {
                    let _ = Command::new("wsl.exe").args(["-e", "tmux", "-L", server, "kill-server"]).output();
                }
                Flavor::Ssh => {
                    server_cli(self, server, &["kill-server"]);
                }
            }
        }
    }

    /// One command and how tmux answered it.
    #[derive(Clone, Debug)]
    pub struct Done {
        pub cmd: String,
        pub ok: bool,
        pub text: String,
    }

    const TYPED: Tag = 1 << 40;
    /// The size Termurion's tab would have: unlike the ConPTY (80x24) and psmux's first layout (120x30).
    pub const TAB: (u32, u32) = (100, 28);

    pub struct Live {
        pub flavor: Flavor,
        server: String,
        session: String,
        pub p: Pty,
        hook: Hook,
        pub gw: Gateway,
        pub hs: Handshake,
        pub ctl: Controller,
        t0: Instant,
        last_tick: f64,
        pub done: Vec<Done>,
        pub aborts: Vec<String>,
        pub notes: Vec<Note>,
        pub actions: Vec<Action>,
        pub exited: bool,
        /// psmux killed the attached session and sent no `%exit` (`Event::Gone`).
        pub gone: bool,
        /// psmux: Termurion detached by dropping the client.
        pub left: bool,
        pub entered: bool,
        pub out: HashMap<u32, Vec<u8>>,
        replies: HashMap<Tag, (bool, String)>,
        next_typed: Tag,
        pub sizes: HashMap<u32, (u32, u32)>,
        pub lines: Vec<String>,
        pub screen: Vec<u8>,
        /// The handshake's replies by question, raw.
        pub asks: Vec<(String, String)>,
        /// The server is a process on this machine (the app's `set_server_local`).
        pub server_local: bool,
        /// A local server's pane pids, as the controller reported them.
        pub pane_pids: HashMap<u32, u32>,
        /// The text of each paste buffer the controller fetched for the clipboard.
        pub clipboard: Vec<String>,
    }

    fn tag_text(s: &str) -> Option<(String, bool, String)> {
        // The gateway's log lines for a normal reply and for an error reply, which name the command.
        if let Some(rest) = s.strip_prefix("[Normal response to \u{201c}") {
            return Some((rest.trim_end_matches("\u{201d}]").to_string(), true, String::new()));
        }
        if let Some(rest) = s.strip_prefix("[Error \u{201c}") {
            let (text, cmd) = rest.split_once("\u{201d} in response to \u{201c}")?;
            return Some((cmd.trim_end_matches("\u{201d}]").to_string(), false, text.to_string()));
        }
        None
    }

    impl Live {
        pub fn start(flavor: Flavor, name: &str) -> Live {
            let server = format!("termurion-test-{name}");
            let api = conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap();
            let p = spawn(&api, &flavor.start(&server, &server), 80, 24).unwrap();
            let mut gw = Gateway::new("\r");
            gw.logging = true;
            let mut ctl = Controller::new(tmux_handshake::FIRST_FREE_TAG);
            ctl.client = (TAB.0, TAB.1);
            Live { flavor, session: server.clone(), server, p, hook: Hook::default(), gw, hs: Handshake::default(), ctl, t0: Instant::now(), last_tick: 0.0, done: Vec::new(), aborts: Vec::new(), notes: Vec::new(), actions: Vec::new(), exited: false, gone: false, left: false, entered: false, out: HashMap::new(), replies: HashMap::new(), next_typed: TYPED, sizes: HashMap::new(), lines: Vec::new(), screen: Vec::new(), asks: Vec::new(), server_local: false, pane_pids: HashMap::new(), clipboard: Vec::new() }
        }

        /// Attaches and waits for the first windows to open.
        pub fn attach(flavor: Flavor, name: &str) -> Live {
            let mut l = Live::start(flavor, name);
            let ok = l.pump_until(Duration::from_secs(40), |l| l.actions.iter().any(|a| matches!(a, Action::InitialWindowsOpened)));
            l.say(&format!("attach {}", if ok { "ok" } else { "TIMEOUT" }));
            l.settle(Duration::from_millis(1500));
            l
        }

        pub fn say(&self, text: &str) {
            println!("LIVE {} {text}", self.flavor.label());
        }

        fn clock(&self) -> f64 {
            self.t0.elapsed().as_secs_f64()
        }

        fn flush(&mut self) {
            let w = self.gw.take_writes();
            if !w.is_empty() {
                let _ = self.p.write(&w);
            }
            // A psmux detach: the app lets go and ends the client.
            if self.gw.take_left() {
                self.left = true;
                self.exited = true;
                self.p.terminate();
            }
        }

        fn feed(&mut self, bytes: &[u8]) {
            let now = self.clock();
            self.ctl.set_clock(now);
            for o in self.hook.feed(bytes) {
                match o {
                    Out::Screen(b) => self.screen.extend(b),
                    Out::Enter => self.entered = true,
                    Out::Line(l) | Out::Exit(l) => {
                        if self.lines.len() < 20000 {
                            self.lines.push(String::from_utf8_lossy(&l).chars().take(200).collect());
                        }
                        let ev = self.gw.line(&l);
                        self.events(ev);
                    }
                }
            }
            self.flush();
        }

        fn events(&mut self, events: Vec<Event>) {
            for e in events {
                match e {
                    Event::Print(s) => {
                        if let Some((cmd, ok, text)) = tag_text(&s) {
                            self.done.push(Done { cmd, ok, text });
                        }
                    }
                    Event::Reply { tag, ok, text, data } => {
                        if tag >= TYPED {
                            self.replies.insert(tag, (ok, text));
                        } else if let Some(ask) = tmux_handshake::ask_of(tag) {
                            self.asks.push((format!("{ask:?}"), if ok { text.clone() } else { "ERROR".into() }));
                            for step in self.hs.reply(ask, ok.then_some(text.as_str()), &mut self.gw) {
                                match step {
                                    Step::Abort(t, m) => self.aborts.push(format!("{t}: {m}")),
                                    Step::VersionKnown => {
                                        self.ctl.psmux = self.hs.psmux;
                                        self.ctl.version_known(self.hs.min, self.hs.tab_bug, self.hs.status_bar, &mut self.gw)
                                    }
                                }
                            }
                            if ask == tmux_handshake::Ask::ServerPid {
                                self.server_local = self.hs.server_local(&tmux_handshake::windows_process_name);
                                self.ctl.set_server_local(self.server_local, &mut self.gw);
                            }
                            if ask == tmux_handshake::Ask::SetTitles {
                                self.ctl.set_titles_known(self.hs.set_titles, &mut self.gw);
                            }
                        } else if let Some(a) = self.ctl.reply(tag, ok, &text, &data, &mut self.gw) {
                            self.actions(a);
                        }
                    }
                    Event::Initialized => self.hs.kick_off(&mut self.gw, Instant::now()),
                    Event::Note(n) => {
                        if matches!(n, Note::SessionChanged { .. }) {
                            self.hs.kick_off(&mut self.gw, Instant::now());
                        }
                        if let Note::Output { pane, data } = &n {
                            self.out.entry(*pane).or_default().extend(data);
                        }
                        self.notes.push(n.clone());
                        let a = self.ctl.note(n, &mut self.gw);
                        self.actions(a);
                    }
                    Event::Abort(m) => self.aborts.push(m),
                    Event::InitialFailed(m) => self.aborts.push(format!("initial failed: {m}")),
                    Event::Exit => self.exited = true,
                    Event::Gone => {
                        // What the app does: let go of the gateway and end the client.
                        self.gone = true;
                        self.exited = true;
                        self.p.terminate();
                    }
                }
            }
        }

        /// What the app does for the controller's actions: watch the panes it shows and tell
        /// the controller the tab sizes.
        fn actions(&mut self, actions: Vec<Action>) {
            let mut resized = false;
            for a in &actions {
                match a {
                    Action::Open { window, panes, .. } => {
                        self.sizes.insert(window.id, TAB);
                        resized = true;
                        for p in panes {
                            self.ctl.watch_pane(p.pane, true, &mut self.gw);
                        }
                    }
                    Action::Relayout { added, removed, .. } => {
                        resized = true;
                        for p in added {
                            self.ctl.watch_pane(p.pane, true, &mut self.gw);
                        }
                        for p in removed {
                            self.ctl.forget_pane(*p);
                            self.ctl.unwatch_pane(*p, &mut self.gw);
                        }
                    }
                    Action::Output(pane, data) => self.out.entry(*pane).or_default().extend(data),
                    Action::PanePid { pane, pid } => {
                        self.pane_pids.insert(*pane, *pid);
                    }
                    // What the app does with TmuxSyncClipboard on.
                    Action::PasteBuffer(name) => self.ctl.fetch_buffer(name, &mut self.gw),
                    Action::Clipboard(text) => self.clipboard.push(text.clone()),
                    Action::Close(w) => {
                        self.sizes.remove(w);
                    }
                    _ => {}
                }
            }
            self.actions.extend(actions);
            if resized {
                let sizes: Vec<(u32, (u32, u32))> = self.sizes.iter().filter(|(w, _)| self.ctl.windows.contains_key(w)).map(|(w, s)| (*w, *s)).collect();
                self.ctl.tabs_resized(&sizes, &mut self.gw);
            }
        }

        /// Reads for up to `wait` of quiet, or until `cond`. False on a timeout.
        pub fn pump_until(&mut self, wait: Duration, cond: impl Fn(&Live) -> bool) -> bool {
            let end = Instant::now() + wait;
            loop {
                if cond(self) {
                    return true;
                }
                if Instant::now() >= end {
                    return false;
                }
                if self.exited && self.p.exit_code().is_some() {
                    return cond(self);
                }
                let now = self.clock();
                if now - self.last_tick >= 0.25 {
                    self.last_tick = now;
                    self.ctl.poll(now, &mut self.gw);
                    let a = self.ctl.tick(now, &mut self.gw);
                    self.actions(a);
                    self.flush();
                }
                if let Some(b) = self.p.read_timeout(Duration::from_millis(50)) {
                    self.feed(&b);
                }
            }
        }

        /// Reads until nothing arrives for `quiet`.
        pub fn settle(&mut self, quiet: Duration) {
            let mut last = Instant::now();
            loop {
                let now = self.clock();
                if now - self.last_tick >= 0.25 {
                    self.last_tick = now;
                    self.ctl.poll(now, &mut self.gw);
                    let a = self.ctl.tick(now, &mut self.gw);
                    self.actions(a);
                    self.flush();
                }
                match self.p.read_timeout(Duration::from_millis(100)) {
                    Some(b) => {
                        last = Instant::now();
                        self.feed(&b);
                    }
                    None if last.elapsed() >= quiet => return,
                    None => {}
                }
            }
        }

        /// Sends one command with a tag and waits for its reply: (ok, text); text is "timeout" if none came.
        pub fn run(&mut self, cmd: &str) -> (bool, String) {
            let tag = self.next_typed;
            self.next_typed += 1;
            self.gw.send(cmd, Some(tag), Flags { tolerate: true, ..Default::default() });
            self.flush();
            if !self.pump_until(Duration::from_secs(10), |l| l.replies.contains_key(&tag)) {
                return (false, "timeout".into());
            }
            self.replies.remove(&tag).unwrap()
        }

        /// `run`, printed as one audit line.
        pub fn audit(&mut self, cmd: &str) -> bool {
            let (ok, text) = self.run(cmd);
            let one: String = text.replace('\n', " | ").chars().take(160).collect();
            self.say(&format!("CMD {} {cmd} => {one}", if ok { "ok" } else { "ERR" }));
            ok
        }

        pub fn count(&self, f: impl Fn(&Note) -> bool) -> usize {
            self.notes.iter().filter(|n| f(n)).count()
        }

        pub fn finish(mut self) {
            self.settle(Duration::from_millis(300));
        }
    }

    /// The test's own server always goes, also when the test panics.
    impl Drop for Live {
        fn drop(&mut self) {
            self.flavor.stop(&self.server, &self.session);
        }
    }

    fn attach_report(flavor: Flavor, name: &str) {
        let mut l = Live::attach(flavor, name);
        l.say(&format!("entered={} version_min={:?} max={:?} known={} tab_bug={} v3_2={}", l.entered, l.hs.min, l.hs.max, l.hs.known, l.hs.tab_bug, l.gw.v3_2));
        l.say(&format!("status_bar={} default_terminal={:?} server_pid={:?} set_titles={}", l.hs.status_bar, l.hs.default_terminal, l.hs.server_pid, l.hs.set_titles));
        l.say(&format!("windows={:?} session={:?}", l.ctl.windows, l.ctl.session));
        l.say(&format!("aborts={:?}", l.aborts));
        let d: Vec<Done> = l.done.clone();
        for x in &d {
            let one: String = x.text.replace('\n', " | ").chars().take(160).collect();
            l.say(&format!("ATTACH {} {} => {one}", if x.ok { "ok" } else { "ERR" }, x.cmd));
        }
        let mut kinds: Vec<String> = l.notes.iter().map(|n| format!("{n:?}").split([' ', '{', '(']).next().unwrap_or("").to_string()).collect();
        kinds.sort();
        kinds.dedup();
        l.say(&format!("notes={kinds:?}"));
        // The size the controller asked for, and what tmux has.
        let layout = l.run("list-windows -F \"#{window_id} #{window_width}x#{window_height} #{window_layout}\"");
        l.say(&format!("layout after attach: {layout:?} (tab size {TAB:?})"));
        // Session end.
        let session = l.session.clone();
        let t = Instant::now();
        let (ok, text) = l.run(&format!("kill-session -t {session}"));
        l.say(&format!("kill-session ok={ok} text={text:?}"));
        let ended = l.pump_until(Duration::from_secs(12), |l| l.exited || l.p.exit_code().is_some());
        l.say(&format!("after kill-session: ended={ended} exited_event={} client_exit={:?} after {:?}", l.exited, l.p.exit_code(), t.elapsed()));
        let tail: Vec<&String> = l.lines.iter().rev().take(6).collect();
        l.say(&format!("last lines: {tail:?}"));
        l.finish();
    }

    #[test]
    #[ignore]
    fn explore_attach_psmux() {
        attach_report(Flavor::Psmux, "xa");
    }

    #[test]
    #[ignore]
    fn explore_attach_wsl() {
        attach_report(Flavor::Wsl, "xa");
    }

    use crate::tmux_edits::{self, Dir};

    fn first_pane(l: &Live) -> (u32, u32) {
        let (w, ps) = l.ctl.windows.iter().next().expect("a window");
        (*w, ps[0])
    }

    fn session_id(l: &Live) -> u32 {
        l.ctl.session.as_ref().map(|s| s.0).unwrap_or(0)
    }

    /// Prints every command tmux answered since `from`, once each, with its result.
    fn print_done(l: &Live, from: usize) {
        let mut seen = std::collections::HashSet::new();
        for d in &l.done[from..] {
            if !seen.insert(format!("{}|{}|{}", d.cmd, d.ok, d.text)) {
                continue;
            }
            let one: String = d.text.replace('\n', " | ").chars().take(140).collect();
            l.say(&format!("AUDIT {} {} => {one}", if d.ok { "ok" } else { "ERR" }, d.cmd));
        }
    }

    /// Every command Termurion's controller sends, from the attach on, and what the server said.
    fn audit(flavor: Flavor, name: &str) {
        let mut l = Live::attach(flavor, name);
        let (w, pane) = first_pane(&l);
        let s = session_id(&l);
        l.say(&format!("version min={:?} max={:?} windows={:?} session={s}", l.hs.min, l.hs.max, l.ctl.windows));
        print_done(&l, 0);
        let from = l.done.len();
        // Monitors and status.
        l.ctl.watch_status(w, true, &mut l.gw);
        let mut vars = std::collections::BTreeMap::new();
        vars.insert("user.a".to_string(), "b".to_string());
        l.ctl.sync_user_vars(pane, &vars, &mut l.gw);
        l.ctl.color_reports(pane, (200, 200, 200), (10, 10, 10), true, &mut l.gw);
        l.settle(Duration::from_secs(2));
        // Editing.
        l.audit(&tmux_edits::split_window(pane, true, &Dir::Home, true));
        l.audit(&tmux_edits::split_window(pane, false, &Dir::Recycle, true));
        l.settle(Duration::from_millis(800));
        let panes = l.ctl.windows.get(&w).cloned().unwrap_or_default();
        l.say(&format!("panes after two splits: {panes:?}"));
        if panes.len() >= 3 {
            l.ctl.break_pane(panes[2], None, None, &mut l.gw);
            l.settle(Duration::from_secs(1));
        }
        l.ctl.resize_pane(pane, 3, true, &mut l.gw);
        l.ctl.zoom(pane, &mut l.gw);
        l.settle(Duration::from_millis(800));
        l.ctl.zoom(pane, &mut l.gw);
        if panes.len() >= 2 {
            l.ctl.swap(panes[0], panes[1], &mut l.gw);
            l.ctl.move_pane(panes[0], panes[1], true, false, &mut l.gw);
        }
        l.settle(Duration::from_millis(800));
        let _ = l.ctl.new_window(None, &Dir::Home, Some(TAB), &mut l.gw);
        l.ctl.new_window(Some(w), &Dir::Recycle, None, &mut l.gw);
        l.settle(Duration::from_secs(2));
        let wins: Vec<u32> = l.ctl.windows.keys().copied().collect();
        l.say(&format!("windows now {wins:?}"));
        l.audit(&tmux_edits::rename_window(w, Some(s), "renamed"));
        l.audit(&tmux_edits::rename_window(w, None, "renamed2"));
        l.audit(&tmux_edits::rename_pane(pane, "title", l.hs.min).unwrap_or_else(|| "select-pane -T x".into()));
        l.audit(&format!("select-pane -t \"%{pane}\""));
        l.audit("select-layout -t @1 even-horizontal");
        l.audit(&format!("list-panes -t %{pane} -F '#{{pane_id}}'"));
        for cmd in tmux_keys_commands(&l, pane) {
            l.audit(&cmd);
        }
        // Session and window state options.
        for (n, v) in [("@iterm2_size", "100,28"), ("@hidden", ""), ("@buried_indexes", ""), ("@affinities", "1"), ("@per_window_settings", ""), ("@per_tab_settings", ""), ("@origins", ""), ("@hotkeys", ""), ("@tab_colors", "")] {
            l.audit(&crate::tmux_persist::option_command(s, n, v));
        }
        l.audit(&crate::tmux_persist::user_vars_command(pane, "abc"));
        l.ctl.hide(w, &mut l.gw);
        l.ctl.reorder(l.ctl.windows.keys().copied().rev().collect(), &mut l.gw);
        l.ctl.save_window_state(&[crate::tmux_controller::GroupState { terminal: "pty-1".into(), windows: wins.clone(), origin: (0, 0), size: None, tab_titles: vec![(w, "t".into())] }], &mut l.gw);
        l.ctl.save_tab_colors(&[(pane, Some((1, 2, 3)))], &mut l.gw);
        l.ctl.save_affinities(&[crate::tmux_persist::Group { windows: wins.clone(), terminal: "pty-1".into(), full_screen: false }], &mut l.gw);
        l.ctl.open_windows(&[w], true, &mut l.gw);
        l.settle(Duration::from_secs(2));
        // Dashboard.
        l.ctl.list_sessions(&mut l.gw);
        l.ctl.dashboard_windows(s, 0.0);
        let now = l.clock() + 2.0;
        l.ctl.poll(now, &mut l.gw);
        l.settle(Duration::from_secs(2));
        l.audit(&crate::tmux_dashboard::list_windows(s, &crate::tmux_windows::detailed_format(l.hs.min.unwrap_or(3.4))));
        l.audit(crate::tmux_dashboard::list_sessions());
        l.audit(&crate::tmux_dashboard::rename_session(s, "renamed-session"));
        l.audit(&crate::tmux_dashboard::attach_session(s));
        l.audit("list-clients -F \"#{client_name}\"");
        l.audit("display-message -p '#{client_name}'");
        l.audit("show-buffer -b buffer0");
        // Pause mode.
        l.ctl.pause_panes(&[pane], &mut l.gw);
        l.settle(Duration::from_secs(1));
        l.ctl.unpause(&[pane], &mut l.gw);
        l.audit(&crate::tmux_pause::enable_command(120.0));
        l.audit(&crate::tmux_pause::pause_command(&[pane]));
        l.audit(&format!("refresh-client -A '%{pane}:continue'"));
        l.audit("refresh-client -C 100,28");
        l.audit(&format!("refresh-client -C @{w}:100x28"));
        l.audit(&format!("resize-window -t @{w} -x 100 -y 28"));
        l.audit("refresh-client -B 'it2_x:%0:#{pane_title}'");
        l.audit("refresh-client -B 'it2_x'");
        l.settle(Duration::from_secs(1));
        print_done(&l, from);
        let from = l.done.len();
        l.say(&format!("aborts={:?}", l.aborts));
        // A new session attaches the control client to it; the Dashboard lists after it.
        l.ctl.command_then_list(&crate::tmux_dashboard::new_session("extra"), false, &mut l.gw);
        l.settle(Duration::from_secs(2));
        print_done(&l, from);
        l.audit("kill-session -t extra");
        l.finish();
    }

    fn tmux_keys_commands(l: &Live, pane: u32) -> Vec<String> {
        let mut out = Vec::new();
        for text in [&b"echo hi"[..], b"\x01\x1b[A", "\u{e9}\u{4e2d}".as_bytes(), b"a;b"] {
            if let Ok(cmds) = crate::tmux_keys::encode(text, pane, l.hs.min, l.hs.max) {
                out.extend(cmds);
            }
        }
        out.push(crate::tmux_keys::key_name_command(pane, "C-Left"));
        out
    }

    #[test]
    #[ignore]
    fn audit_psmux() {
        audit(Flavor::Psmux, "au");
    }

    #[test]
    #[ignore]
    fn audit_wsl() {
        audit(Flavor::Wsl, "au");
    }

    fn result(l: &Live, feature: &str, ok: bool, note: &str) {
        l.say(&format!("RESULT | {feature} | {} | {note}", if ok { "pass" } else { "FAIL" }));
        // The server still goes: `Live` ends it when the panic unwinds.
        assert!(ok, "{} {feature}: {note}", l.flavor.label());
    }

    /// The features of the plan's live suite, through the real gateway and controller.
    fn suite(flavor: Flavor, name: &str) {
        let mut l = Live::attach(flavor, name);
        let (w, pane) = first_pane(&l);
        let s = session_id(&l);
        result(&l, "attach", !l.ctl.windows.is_empty() && l.aborts.is_empty() && l.actions.iter().any(|a| matches!(a, Action::InitialWindowsOpened)), &format!("version {:?}, aborts {:?}", l.hs.min, l.aborts));
        // The client size: the window takes the tab's size.
        let sized = l.pump_until(Duration::from_secs(5), |l| l.notes.iter().any(|n| matches!(n, Note::LayoutChange { layout, .. } if layout.contains(&format!("{}x{}", TAB.0, TAB.1)))));
        let (_, layout) = l.run("list-windows -F \"#{window_id} #{window_layout}\"");
        result(&l, "size to the tab", sized, &format!("layout {}", layout.replace('\n', " | ")));
        // New window.
        let n = l.ctl.windows.len();
        l.ctl.new_window(None, &Dir::Home, Some(TAB), &mut l.gw);
        l.flush();
        let ok = l.pump_until(Duration::from_secs(10), |l| l.ctl.windows.len() > n);
        result(&l, "new window", ok, &format!("windows {:?}", l.ctl.windows));
        // Splits.
        let panes = |l: &Live| l.ctl.windows.get(&w).map_or(0, |p| p.len());
        l.audit(&tmux_edits::split_window(pane, true, &Dir::Home, true));
        let ok = l.pump_until(Duration::from_secs(10), |l| panes(l) == 2);
        result(&l, "split -h", ok, &format!("panes {:?}", l.ctl.windows.get(&w)));
        let target = l.ctl.windows.get(&w).and_then(|p| p.last().copied()).unwrap_or(pane);
        l.audit(&tmux_edits::split_window(target, false, &Dir::Home, true));
        let ok = l.pump_until(Duration::from_secs(10), |l| panes(l) == 3);
        result(&l, "split -v", ok, &format!("panes {:?}", l.ctl.windows.get(&w)));
        // Resize a divider.
        let layout_of = |l: &mut Live| l.run(&format!("list-windows -t @{w} -F \"#{{window_layout}}\"")).1;
        let before = layout_of(&mut l);
        l.ctl.resize_pane(pane, 4, true, &mut l.gw);
        l.flush();
        l.settle(Duration::from_secs(1));
        let after = layout_of(&mut l);
        result(&l, "resize pane", before != after, &format!("{before} -> {after}"));
        // select-layout.
        let before = layout_of(&mut l);
        let ok = l.audit(&format!("select-layout -t @{w} even-vertical"));
        l.settle(Duration::from_secs(1));
        let after = layout_of(&mut l);
        result(&l, "select-layout", ok && before != after, &format!("{before} -> {after}"));
        // Client resize to a new tab size.
        l.sizes.insert(w, (90, 26));
        let sizes: Vec<(u32, (u32, u32))> = l.sizes.iter().map(|(a, b)| (*a, *b)).collect();
        l.ctl.tabs_resized(&sizes, &mut l.gw);
        l.flush();
        l.settle(Duration::from_secs(1));
        let after = layout_of(&mut l);
        result(&l, "resize to a new tab size", after.contains("90x26"), &after);
        // Rename.
        l.audit(&tmux_edits::rename_window(w, Some(s), "renamed"));
        let ok = l.pump_until(Duration::from_secs(5), |l| l.actions.iter().any(|a| matches!(a, Action::Rename(x, n) if *x == w && n == "renamed")));
        result(&l, "rename-window", ok, "Rename action");
        // Keys round trip.
        let marker = match flavor {
            Flavor::Psmux => "echo ('ROUND'+'TRIP')\r",
            Flavor::Wsl | Flavor::Ssh => "echo ROUND''TRIP\r",
        };
        let focus = l.ctl.windows.get(&w).and_then(|p| p.first().copied()).unwrap_or(pane);
        l.out.clear();
        for cmd in crate::tmux_keys::encode(marker.as_bytes(), focus, l.hs.min, l.hs.max).unwrap() {
            l.audit(&cmd);
        }
        let ok = l.pump_until(Duration::from_secs(10), |l| l.out.get(&focus).is_some_and(|o| o.windows(9).any(|x| x == b"ROUNDTRIP")));
        result(&l, "send-keys round trip", ok, &format!("output {:?}", String::from_utf8_lossy(l.out.get(&focus).map_or(&[][..], |o| &o[..o.len().min(120)]))));
        // Pause.
        if l.ctl.pause_mode() {
            l.ctl.pause_panes(&[focus], &mut l.gw);
            l.flush();
            let ok = l.pump_until(Duration::from_secs(5), |l| l.ctl.is_paused(focus));
            l.ctl.unpause(&[focus], &mut l.gw);
            l.flush();
            l.settle(Duration::from_secs(1));
            result(&l, "pause and unpause", ok && !l.ctl.is_paused(focus), &format!("{:?}", l.actions.iter().filter(|a| matches!(a, Action::Paused { .. } | Action::Unpaused(_))).count()));
        } else {
            result(&l, "pause and unpause", false, "pause mode is off for this server");
        }
        // Kill a pane.
        let victim = l.ctl.windows.get(&w).and_then(|p| p.last().copied()).unwrap();
        l.audit(&tmux_edits::kill_pane(victim));
        let ok = l.pump_until(Duration::from_secs(10), |l| panes(l) == 2);
        result(&l, "kill-pane", ok, &format!("panes {:?}", l.ctl.windows.get(&w)));
        // Detach.
        let sent = l.gw.request_detach();
        l.flush();
        let t = Instant::now();
        let ok = l.pump_until(Duration::from_secs(12), |l| l.exited);
        let ended = l.pump_until(Duration::from_secs(5), |l| l.p.exit_code().is_some());
        result(&l, "detach", sent && ok && ended, &format!("gateway ended {ok} (client dropped {}), client exit {:?} after {:?}", l.left, l.p.exit_code(), t.elapsed()));
        // Detach leaves the session running on the server.
        let listing = server_cli(flavor, &l.server, &["list-sessions"]);
        result(&l, "session survives the detach", listing.contains(&l.session), &listing.chars().take(80).collect::<String>());
        // The test's own server goes, and nothing of it is left.
        l.flavor.stop(&l.server, &l.session);
        l.settle(Duration::from_millis(500));
        let left = server_cli(flavor, &l.server, &["list-sessions"]);
        let gone = left.contains("no server running") || left.contains("error connecting") || !left.contains(&format!("{}:", l.session));
        result(&l, "kill-server cleanup", gone, &left.chars().take(80).collect::<String>());
        l.finish();
    }

    #[test]
    #[ignore]
    fn suite_psmux() {
        suite(Flavor::Psmux, "su");
    }

    #[test]
    #[ignore]
    fn suite_wsl() {
        suite(Flavor::Wsl, "su");
    }

    /// The ssh tests run only with a host named in the environment.
    fn ssh_ready() -> bool {
        if ssh_args().is_some() {
            return true;
        }
        println!("LIVE ssh SKIP TERMURION_TMUX_SSH_HOST is not set");
        false
    }

    #[test]
    #[ignore]
    fn suite_ssh() {
        if ssh_ready() {
            suite(Flavor::Ssh, &random_name("su"));
        }
    }

    /// A shell command on the host that prints to the test's output (own `-L` name only).
    fn psmux_cli(server: &str, args: &[&str]) -> String {
        let mut full = vec!["-L", server];
        full.extend_from_slice(args);
        match std::process::Command::new("psmux").args(&full).output() {
            Ok(o) => format!("{} {}", String::from_utf8_lossy(&o.stdout).trim(), String::from_utf8_lossy(&o.stderr).trim()),
            Err(e) => format!("spawn error {e}"),
        }
    }

    /// What psmux says that tmux does not: version text, command list, and how a session ends.
    #[test]
    #[ignore]
    fn explore_psmux_more() {
        let mut l = Live::attach(Flavor::Psmux, "mo");
        let asks = l.asks.clone();
        for (k, v) in asks.iter().filter(|(k, _)| matches!(k.as_str(), "Version" | "Phony" | "ServerPid" | "Probe22" | "Probe21" | "Probe23" | "Probe18" | "Status" | "DefaultTerminal" | "Utf8")) {
            l.say(&format!("ASK {k} => {:?}", v.chars().take(100).collect::<String>()));
        }
        for cmd in ["list-commands", "server-info", "list-sessions", "display-message -p '#{version}'", "display-message -p '#{session_id} #{session_name} #{socket_path}'", "has-session -t nosuch", "show-options -g status-interval"] {
            let (ok, text) = l.run(cmd);
            l.say(&format!("MORE {} {cmd} => {:?}", if ok { "ok" } else { "ERR" }, text.chars().take(600).collect::<String>()));
        }
        // Latency of a send and of a plain command.
        let (_, pane) = first_pane(&l);
        for cmd in [format!("send -lt %{pane} x"), format!("send -t %{pane} 'C-Left'"), "display-message -p ok".to_string(), format!("send -lt %{pane} y")] {
            let t = Instant::now();
            let (ok, _) = l.run(&cmd);
            l.say(&format!("LATENCY {cmd} ok={ok} {:?}", t.elapsed()));
        }
        // Option storage round trip.
        let s = session_id(&l);
        l.run(&format!("set -t ${s} @probe \"abc\""));
        let (ok, text) = l.run(&format!("show -v -q -t ${s} @probe"));
        l.say(&format!("MORE option round trip ok={ok} {text:?}"));
        // A second session in the same namespace, then switch-client and run-command.
        let ns = "termurion-test-mo";
        l.say(&format!("MORE cli new-session: {}", psmux_cli(ns, &["new-session", "-d", "-s", "other"])));
        l.settle(Duration::from_secs(2));
        for cmd in ["list-sessions -F \"#{session_id} #{session_name}\"", "attach-session -t other", "switch-client -t other", "run-command new-session -d -s viarun", "new-session -d -s viacc"] {
            let (ok, text) = l.run(cmd);
            l.say(&format!("MORE {} {cmd} => {:?}", if ok { "ok" } else { "ERR" }, text.chars().take(300).collect::<String>()));
            l.settle(Duration::from_secs(2));
        }
        l.say(&format!("MORE sessions now {:?} ctl.session {:?}", l.ctl.sessions, l.ctl.session));
        l.say(&format!("MORE cli list: {}", psmux_cli(ns, &["list-sessions"])));
        let tail: Vec<&String> = l.lines.iter().rev().take(12).collect();
        l.say(&format!("MORE last lines {tail:?}"));
        let kinds: Vec<String> = l.notes.iter().rev().take(12).map(|n| format!("{n:?}").chars().take(100).collect()).collect();
        l.say(&format!("MORE last notes {kinds:?}"));
        // The client's end: detach, then a kill of the attached session.
        for cmd in ["detach", "detach-client", "kill-session -t other"] {
            let t = Instant::now();
            let (ok, text) = l.run(cmd);
            l.say(&format!("END {} {cmd} => {text:?} after {:?}", if ok { "ok" } else { "ERR" }, t.elapsed()));
            l.settle(Duration::from_secs(1));
            l.say(&format!("END exited={} client_exit={:?}", l.exited, l.p.exit_code()));
        }
        let session = l.session.clone();
        let (ok, text) = l.run(&format!("kill-session -t {session}"));
        l.say(&format!("END kill own session ok={ok} {text:?}"));
        l.settle(Duration::from_secs(4));
        l.say(&format!("END after kill: exited={} client_exit={:?} notes tail {:?}", l.exited, l.p.exit_code(), l.notes.iter().rev().take(4).collect::<Vec<_>>()));
        for bytes in [&b"\r"[..], b"\x04", b"\x03"] {
            let _ = l.p.write(bytes);
            l.settle(Duration::from_secs(3));
            l.say(&format!("END after {bytes:?}: exited={} client_exit={:?} lines tail {:?}", l.exited, l.p.exit_code(), l.lines.iter().rev().take(3).collect::<Vec<_>>()));
        }
        l.say(&format!("MORE cli list at end: {}", psmux_cli(ns, &["list-sessions"])));
        psmux_cli(ns, &["kill-server"]);
        l.finish();
    }

    /// The suite's kill-pane step again, with the lines tmux and psmux send for it.
    fn kill_pane_lines(flavor: Flavor) {
        let mut l = Live::attach(flavor, "kp");
        let (w, pane) = first_pane(&l);
        l.audit(&tmux_edits::split_window(pane, true, &Dir::Home, true));
        l.pump_until(Duration::from_secs(5), |l| l.ctl.windows.get(&w).is_some_and(|p| p.len() == 2));
        let n = l.lines.len();
        let victim = l.ctl.windows.get(&w).and_then(|p| p.last().copied()).unwrap();
        l.audit(&tmux_edits::kill_pane(victim));
        l.settle(Duration::from_secs(2));
        for line in &l.lines[n..] {
            l.say(&format!("KP {}", line.chars().take(120).collect::<String>()));
        }
        l.say(&format!("KP windows {:?}", l.ctl.windows));
        l.finish();
    }

    #[test]
    #[ignore]
    fn explore_kill_pane_psmux() {
        kill_pane_lines(Flavor::Psmux);
    }

    #[test]
    #[ignore]
    fn explore_kill_pane_wsl() {
        kill_pane_lines(Flavor::Wsl);
    }

    /// The attached session killed from inside: tmux says `%exit`; psmux says nothing, and the
    /// gateway lets go by itself (`Event::Gone`), after which the client process is ended.
    fn session_end(flavor: Flavor, name: &str) {
        let mut l = Live::attach(flavor, name);
        let session = l.session.clone();
        let t = Instant::now();
        let ok = l.audit(&format!("kill-session -t {session}"));
        let ended = l.pump_until(Duration::from_secs(12), |l| l.exited && l.p.exit_code().is_some());
        result(&l, "kill-session of the attached session", ok && ended, &format!("gateway ended {} (gone event {}) client exit {:?} after {:?}", l.exited, l.gone, l.p.exit_code(), t.elapsed()));
        l.finish();
    }

    #[test]
    #[ignore]
    fn session_end_psmux() {
        session_end(Flavor::Psmux, "se");
    }

    #[test]
    #[ignore]
    fn session_end_wsl() {
        session_end(Flavor::Wsl, "se");
    }

    #[test]
    #[ignore]
    fn session_end_ssh() {
        if ssh_ready() {
            session_end(Flavor::Ssh, &random_name("se"));
        }
    }

    /// tmux's paste buffer reaches the local clipboard: `%paste-buffer-changed`, `show-buffer`.
    fn clipboard(flavor: Flavor, name: &str) {
        let mut l = Live::attach(flavor, name);
        let ok = l.audit("set-buffer \"CLIPTEXT 123\"");
        let got = l.pump_until(Duration::from_secs(if flavor == Flavor::Psmux { 4 } else { 10 }), |l| l.clipboard.iter().any(|c| c.contains("CLIPTEXT 123")));
        let notices = l.count(|n| matches!(n, Note::PasteBufferChanged(_)));
        if flavor == Flavor::Psmux && !got {
            // psmux refuses `set-buffer` in control mode and sends no %paste-buffer-changed, so its
            // paste buffer never reaches the clipboard. Recorded, not asserted.
            let (shown, text) = l.run("show-buffer");
            l.say(&format!("NOTE | clipboard | not available | set-buffer ok {ok}, buffer notices {notices}, clipboard {:?}, show-buffer ok {shown} {text:?}", l.clipboard));
        } else {
            result(&l, "clipboard", ok && got, &format!("buffer notices {notices} clipboard {:?}", l.clipboard));
        }
        l.finish();
    }

    #[test]
    #[ignore]
    fn clipboard_psmux() {
        clipboard(Flavor::Psmux, "cb");
    }

    #[test]
    #[ignore]
    fn clipboard_wsl() {
        clipboard(Flavor::Wsl, "cb");
    }

    #[test]
    #[ignore]
    fn clipboard_ssh() {
        if ssh_ready() {
            clipboard(Flavor::Ssh, &random_name("cb"));
        }
    }

    /// Which servers count as local (iTerm2's serverIsLocal): psmux only; its panes' pids are
    /// read and name real processes of this machine. A WSL or ssh server is not local and is
    /// never asked for a pane pid.
    fn locality(flavor: Flavor, name: &str) {
        let mut l = Live::attach(flavor, name);
        l.settle(Duration::from_secs(2));
        let (_, pane) = first_pane(&l);
        let want_local = flavor == Flavor::Psmux;
        result(&l, "server local", l.server_local == want_local, &format!("server pid {:?}, local {}", l.hs.server_pid, l.server_local));
        if want_local {
            let server = l.hs.server_pid.and_then(tmux_handshake::windows_process_name);
            let pid = l.pane_pids.get(&pane).copied();
            let name = pid.and_then(tmux_handshake::windows_process_name);
            let tree = pid.and_then(|p| crate::jobwatch::descendants_now(p).ok()).map_or(0, |t| t.len());
            result(&l, "server process is psmux", server.is_some(), &format!("{server:?}"));
            result(&l, "pane pid is a process here", pid.is_some() && name.is_some(), &format!("pid {pid:?} {name:?} ({tree} below it)"));
            // A program started in the pane shows up under that pid.
            let focus = pane;
            for cmd in crate::tmux_keys::encode(b"ping -n 20 127.0.0.1\r", focus, l.hs.min, l.hs.max).unwrap() {
                l.run(&cmd);
            }
            let found = pid.is_some_and(|p| {
                let end = Instant::now() + Duration::from_secs(15);
                loop {
                    let procs = crate::jobwatch::descendants_now(p).unwrap_or_default();
                    if procs.iter().any(|q| q.name.to_ascii_lowercase().starts_with("ping")) {
                        break true;
                    }
                    if Instant::now() > end {
                        break false;
                    }
                    l.settle(Duration::from_millis(300));
                }
            });
            result(&l, "job read from the pane's process tree", found, "ping below the pane's shell");
        } else {
            result(&l, "no pane pid asked", l.pane_pids.is_empty(), &format!("{:?}", l.pane_pids));
        }
        l.finish();
    }

    #[test]
    #[ignore]
    fn locality_psmux() {
        locality(Flavor::Psmux, "lo");
    }

    #[test]
    #[ignore]
    fn locality_wsl() {
        locality(Flavor::Wsl, "lo");
    }

    #[test]
    #[ignore]
    fn locality_ssh() {
        if ssh_ready() {
            locality(Flavor::Ssh, &random_name("lo"));
        }
    }

    /// More of what psmux does, for the notes: kill-pane's window, join-pane, the command list.
    #[test]
    #[ignore]
    fn explore_psmux_round2() {
        let mut l = Live::attach(Flavor::Psmux, "r2");
        let (w, pane) = first_pane(&l);
        let s = session_id(&l);
        let (_, cmds) = l.run("list-commands");
        l.say(&format!("R2 commands {}", cmds.replace('\n', ",")));
        l.audit(&tmux_edits::split_window(pane, true, &Dir::Home, true));
        l.pump_until(Duration::from_secs(5), |l| l.ctl.windows.get(&w).is_some_and(|p| p.len() == 2));
        l.audit("list-windows -F \"#{window_id} #{window_panes} #{window_name}\"");
        let victim = l.ctl.windows.get(&w).and_then(|p| p.last().copied()).unwrap();
        l.audit(&format!("kill-pane -t %{victim}"));
        l.settle(Duration::from_secs(2));
        l.audit("list-windows -F \"#{window_id} #{window_panes} #{window_name}\"");
        l.audit("list-panes -a -F \"#{window_id} #{pane_id}\"");
        l.say(&format!("R2 windows after kill-pane {:?}", l.ctl.windows));
        l.audit(&format!("new-window -PF '#{{window_id}}' -a -t \"${s}:+\""));
        l.settle(Duration::from_secs(1));
        let ws: Vec<u32> = l.ctl.windows.keys().copied().collect();
        l.say(&format!("R2 windows {ws:?}"));
        if let Some(&a) = ws.last() {
            l.audit(&format!("select-window -t @{a}"));
            l.audit(&format!("unlink-window -k -t @{a}"));
            l.audit(&format!("kill-window -t @{a}"));
        }
        l.settle(Duration::from_secs(1));
        l.say(&format!("R2 windows end {:?}", l.ctl.windows));
        l.audit(&crate::tmux_dashboard::rename_session(s, "renamed-session"));
        l.settle(Duration::from_secs(1));
        l.say(&format!("R2 session after rename {:?}", l.ctl.session));
        l.finish();
    }

    /// The raw bytes psmux sends for `detach`, and how long its client stays.
    #[test]
    #[ignore]
    fn explore_psmux_detach_raw() {
        let mut l = Live::attach(Flavor::Psmux, "dr");
        let _ = l.p.write(b"detach\r");
        let t = Instant::now();
        let mut all = Vec::new();
        while t.elapsed() < Duration::from_secs(8) {
            if let Some(b) = l.p.read_timeout(Duration::from_millis(100)) {
                l.say(&format!("DRAW {:?} +{:?} {:?}", t.elapsed(), b.len(), String::from_utf8_lossy(&b[b.len().saturating_sub(120)..])));
                all.extend(b);
            }
            if let Some(c) = l.p.exit_code() {
                l.say(&format!("DRAW client exit {c} after {:?}", t.elapsed()));
                break;
            }
        }
        l.say(&format!("DRAW tail {:?} exit {:?}", String::from_utf8_lossy(&all[all.len().saturating_sub(160)..]), l.p.exit_code()));
        l.finish();
    }
}
