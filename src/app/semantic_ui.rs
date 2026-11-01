//! Smart Selection and Semantic History in the window: what a click selects and what a
//! Win-click opens.

use alacritty_terminal::index::Point;

use super::*;
use super::find_ui::{reveal_as, to_pos};
use crate::find::{self, Logical, Match};
use crate::sh_context;
use crate::semantic::{self, Around, ClickContext, Prefs, Run, Target};
use crate::smartsel;
use crate::ssh_files;

/// `SmartSelectionRadius` (iTerm2's 2): the logical lines Smart Selection looks at each way from
/// the click. Clamped to 0..=50 (P6), so a negative value is 0 and a hand-edited huge one cannot
/// make every click read thousands of lines.
pub(super) fn radius(g: &crate::globals::Globals) -> usize {
    let n = g.number("SmartSelectionRadius").filter(|v| !v.is_nan()).unwrap_or(2.0);
    n.clamp(0.0, 50.0) as usize
}
/// How a plan is run beyond the plan itself.
#[derive(Clone, Copy)]
pub(super) struct Exec<'a> {
    /// An Alt-click background open (`background` in the URL handler's template).
    pub(super) background: bool,
    /// The plan came from a clicked file: never handed to the URL handler.
    pub(super) from_file: bool,
    /// Present where a window may open (Run Command in Window).
    pub(super) event_loop: Option<&'a ActiveEventLoop>,
}

/// The cells to underline for a Win-hover target: none when the underline key is off (the hand
/// pointer still shows, `TVA:548-552`).
pub(super) fn hover_cells(target_cells: Match, underline: bool) -> Option<Match> {
    underline.then_some(target_cells)
}

/// The columns between soft boundaries (the vertical dividers apps draw) around a click, when
/// `restrict` is on and a divider stands in the click's row (`UAH:86`, `UAH:401-430`).
pub(super) fn window_for(rows: &[Vec<char>], row: usize, col: usize, width: usize, restrict: bool) -> Option<(usize, usize)> {
    // One rule with word selection's soft boundaries (S2).
    restrict.then(|| crate::wordsel::window_at(rows, row, col, width)).flatten()
}

/// The columns between soft boundaries around `at`, when `restrict` is on.
fn click_window(screen: &crate::screen::Screen, at: find::Pos, restrict: bool) -> Option<(usize, usize)> {
    use alacritty_terminal::grid::Dimensions;
    let grid = screen.term().grid();
    let line = at.row as i32 - grid.history_size() as i32;
    let reach = crate::seltools::DIVIDER_RUN as i32 - 1;
    let rows = find::cell_rows(screen, line - reach, line + reach);
    let row = (line - (line - reach).max(-(grid.history_size() as i32))) as usize;
    window_for(&rows, row, at.col, grid.columns(), restrict)
}

/// The text around `at` through `sh_context`: rows enough for the byte limit, in the window.
fn around(screen: &crate::screen::Screen, at: find::Pos, window: Option<(usize, usize)>, respect: bool, max: usize) -> sh_context::Context {
    use alacritty_terminal::grid::Dimensions;
    let grid = screen.term().grid();
    let top = -(grid.history_size() as i32);
    let line = at.row as i32 + top;
    // Each cell holds at least one byte, so this many rows hold `max` bytes.
    let n = rows_each_way(grid.columns(), max);
    let first = (line - n).max(top);
    let rows = find::rows_text(screen, first, line + n, window);
    sh_context::build(&rows, (line - first) as usize, at.col, respect, max)
}

/// The rows each way from a click that hold `max` bytes, as `around` reads them.
fn rows_each_way(columns: usize, max: usize) -> i32 {
    (sh_context::ROW_CLAMP.max(max / columns.max(1) + 2)).min(200) as i32
}

/// The logical lines Smart Selection reads around `at` (`radius` each way, `SmartSelectionRadius`) and the index of the
/// one holding `at`, built from a bounded span of rows (`rows_each_way`) inside the column
/// window, never from whole logical lines: one huge wrapped line costs the same as a short one.
fn smart_lines(screen: &crate::screen::Screen, at: find::Pos, window: Option<(usize, usize)>, max: usize, radius: usize) -> (Vec<Logical>, usize) {
    use alacritty_terminal::grid::Dimensions;
    let grid = screen.term().grid();
    let top = -(grid.history_size() as i32);
    let line = at.row as i32 + top;
    let n = rows_each_way(grid.columns(), max);
    let first = (line - n).max(top);
    let rows = find::rows_text(screen, first, line + n, window);
    let click_row = (line - first).max(0) as usize;
    let mut lines: Vec<Logical> = Vec::new();
    let mut clicked = 0;
    let mut cur = Logical { text: String::new(), cells: Vec::new() };
    for (i, r) in rows.iter().enumerate() {
        let base = cur.text.len();
        cur.cells.extend(r.cells.iter().map(|(o, p)| (o + base, *p)));
        cur.text.push_str(&r.text);
        if i == click_row {
            clicked = lines.len();
        }
        if !r.wraps || i + 1 == rows.len() {
            lines.push(std::mem::replace(&mut cur, Logical { text: String::new(), cells: Vec::new() }));
        }
    }
    let (lo, hi) = (clicked.saturating_sub(radius), (clicked + radius + 1).min(lines.len()));
    let kept: Vec<Logical> = lines.drain(lo..hi).collect();
    (kept, clicked - lo)
}

/// What `update_hover` compares to skip work: the pane, the cell and both hover keys.
pub(super) type HoverKey = (PaneId, i32, usize, bool, bool);

pub(super) fn hover_key(at: Option<(PaneId, Point)>, underline: bool, preview: bool) -> Option<HoverKey> {
    at.map(|(id, p)| (id, p.line.0, p.column.0, underline, preview))
}

impl App {
    /// Selects the best Smart Selection match at `point`. Returns the rule and match, or
    /// None (and selects nothing) when no rule matches there.
    pub(super) fn smart_select(&mut self, id: PaneId, point: Point, ignore_newlines: bool) -> Option<smartsel::Found> {
        let rules = self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).map(smartsel::rules)?;
        let p = self.panes.get_mut(&id)?;
        let at = to_pos(&p.screen, point);
        let max = self.globals.int("MaxSemanticHistoryPrefixOrSuffix").max(0) as usize;
        let (lines, _) = smart_lines(&p.screen, at, None, max, radius(&self.globals));
        let joined = find::Logical::join(&lines, if ignore_newlines { "" } else { "\n" });
        let click = joined.offset_of(at)?;
        let found = smartsel::select_at(&joined.text, click, &rules)?;
        let m = Match { start: joined.pos_before(found.range.start), end: joined.pos_before(found.range.end - 1) };
        reveal_as(p, &m, crate::seltext::Mode::Smart);
        Some(found)
    }

    /// What is around a click: the context Semantic History resolves, the clicked line
    /// (for ranges), and the Smart Selection match there.
    pub(super) fn clicked(&self, id: PaneId, point: Point) -> Option<Clicked> {
        let p = self.panes.get(&id)?;
        let profile = self.store.get(&p.profile)?;
        let rules = smartsel::rules(profile);
        let prefs = Prefs::from_value(profile.get("Semantic History").as_ref());
        let at = to_pos(&p.screen, point);
        let max = self.globals.int("MaxSemanticHistoryPrefixOrSuffix").max(0) as usize;
        // Detection stays between soft boundaries (`UAH:86`), Smart Selection included (P12),
        // over a bounded number of rows, never whole logical lines.
        let window = click_window(&p.screen, at, true);
        let (lines, i) = smart_lines(&p.screen, at, window, max, radius(&self.globals));
        let line = lines.get(i)?.clone_line();
        line.offset_of(at)?;
        let joined = Logical::join(&lines, "\n");
        let found = joined.offset_of(at).and_then(|click| smartsel::select_at(&joined.text, click, &rules));
        let smart = found.as_ref().filter(|f| !rules[f.rule].actions.is_empty()).map(|f| (f.rule, 0, joined.text[f.range.clone()].to_string()));
        let ignore = self.globals.bool("IgnoreHardNewlinesInURLs");
        let respect = sh_context::respect_hard(ignore, p.screen.alt_screen(), window.is_some());
        let text = around(&p.screen, at, window, respect, max);
        // The file phase's second pass, when the first did not respect hard newlines and a
        // window did not rule that out (`UAF:291-297`).
        let hard = (!respect && window.is_none()).then(|| around(&p.screen, at, None, true, max));
        let link = p.screen.term().grid()[point].hyperlink();
        // A click on a prompt's command line shows Command Info, ahead of every link (`UAF:275-286`).
        let prompt = self.globals.bool("EnableCmdClickPromptForShowCommandInfo").then(|| {
            let (mark, (a, b)) = p.screen.command_hit(point)?;
            Some((mark, text.range_between(to_pos(&p.screen, a), to_pos(&p.screen, b))?))
        });
        let prompt = prompt.flatten();
        let urls = self.url_prefs(id);
        let filename_set = crate::url_guess::filename(&urls.chars, &self.globals.string("FilenameCharacterSet"));
        let ctx = ClickContext {
            before: text.before.clone(),
            after: text.after.clone(),
            link: link.as_ref().map(|h| h.uri().to_string()),
            cwd: self.pane_dir(id),
            smart,
            raw_command: prefs.action == "raw command",
            urls,
            no_probe: self.remote_text(id),
            windowed: window.is_some(),
            respecting: hard.as_ref().map(|h| (h.before.clone(), h.after.clone())),
            filename_set: Some(filename_set),
            probe: self.path_probe(),
            order: semantic::order_from(self.globals.bool("PrioritizeSmartSelectionActions"), self.globals.bool("DisableSmartSelectionActionsOnClick")),
            prompt,
        };
        Some(Clicked { ctx, line, text, hard, joined, found, rules, prefs, link_id: link.map(|h| h.id().to_string()) })
    }

    /// The text for `\3` and `\4`: restricted to the soft-boundary window only with
    /// `RestrictSemanticHistoryPrefixAndSuffixToLogicalWindow` (R3), blank cells as spaces.
    fn action_context(&self, id: PaneId, point: Point) -> Option<(String, String)> {
        let p = self.panes.get(&id)?;
        let at = to_pos(&p.screen, point);
        let window = click_window(&p.screen, at, self.globals.bool("RestrictSemanticHistoryPrefixAndSuffixToLogicalWindow"));
        let respect = sh_context::respect_hard(self.globals.bool("IgnoreHardNewlinesInURLs"), p.screen.alt_screen(), window.is_some());
        let max = self.globals.int("MaxSemanticHistoryPrefixOrSuffix").max(0) as usize;
        let c = around(&p.screen, at, window, respect, max);
        (!c.cells.is_empty()).then_some((c.before, c.after))
    }

    /// The URL guessing settings for a pane: the Advanced keys plus whether its working
    /// directory is local (P3): false for a framed pane, a host that is not this machine, or a
    /// network path; true when the pane reports no directory.
    pub(super) fn url_prefs(&self, id: PaneId) -> crate::url_guess::UrlPrefs {
        let shell = self.panes.get(&id).map(|p| &p.own().shell);
        let probe = self.path_probe();
        let cwd_is_local = !self.remote_text(id)
            && shell.is_none_or(|s| {
                super::statusbar_content::is_this_machine(s.host.as_deref(), s.machine_id.as_deref())
                    && s.cwd.as_deref().is_none_or(|c| crate::profile::local_path(c).is_some_and(|p| crate::netpath::may_probe(&p.to_string_lossy(), &probe)))
            });
        let chars = crate::url_guess::url(&self.globals.string("URLCharacterSet"), &self.globals.string("URLCharacterSetExclusions"));
        crate::url_guess::UrlPrefs {
            chars,
            trailing: self.globals.string("TrailingPunctuationMarks"),
            conservative: self.globals.bool("ConservativeURLGuessing"),
            require_slash: self.globals.bool("RequireSlashInURLGuess"),
            default_scheme: self.globals.string("DefaultURLScheme"),
            cwd_is_local,
        }
    }

    /// What Semantic History may probe: `PathsToIgnore` and `EnableSemanticHistoryOnNetworkMounts`
    /// with the real drive types (`NFM:17-70`).
    pub(super) fn path_probe(&self) -> crate::netpath::Probe {
        crate::netpath::Probe::from_settings(&self.globals.string("PathsToIgnore"), self.globals.bool("EnableSemanticHistoryOnNetworkMounts"), crate::netpath::real_drives())
    }

    /// P10's one guard: text in this pane may have been written by a remote host, so it never
    /// becomes a local open, run or coprocess. True whenever a conductor frames the pane,
    /// whatever host it reports: the remote writes that report (OSC 1337 RemoteHost, OSC 7).
    pub(super) fn remote_text(&self, id: PaneId) -> bool {
        self.framing(id).is_some()
    }

    /// The pane's working directory from shell integration, when it is on this machine.
    pub(super) fn pane_dir(&self, id: PaneId) -> Option<std::path::PathBuf> {
        // A browser pane has no shell and so no directory (13w seam S14), whatever its screen holds.
        if self.remote_text(id) || self.is_browser_pane(id) {
            return None;
        }
        let shell = &self.panes.get(&id)?.own().shell;
        if !super::statusbar_content::is_this_machine(shell.host.as_deref(), shell.machine_id.as_deref()) {
            return None;
        }
        crate::profile::local_path(shell.cwd.as_deref()?)
    }

    /// The Win-click target at a point and the cells it covers.
    pub(super) fn target_at(&self, id: PaneId, point: Point) -> Option<(Clicked, Target, Match)> {
        let c = self.clicked(id, point)?;
        let (target, range, pass) = semantic::resolve_in(&c.ctx)?;
        let cells = match (&target, range) {
            (_, Some(r)) => {
                let t = if pass == semantic::Pass::Respecting { c.hard.as_ref()? } else { &c.text };
                Match { start: t.pos_at(r.start)?, end: t.pos_at(r.end - 1)? }
            }
            (Target::Action { .. }, None) => {
                let f = c.found.as_ref()?;
                Match { start: c.joined.pos_before(f.range.start), end: c.joined.pos_before(f.range.end - 1) }
            }
            // A link covers the cells around the click that carry the same link.
            _ => {
                let p = self.panes.get(&id)?;
                let at = to_pos(&p.screen, point);
                let same = |pos: find::Pos| {
                    let pt = super::find_ui::to_point(&p.screen, pos);
                    p.screen.term().grid()[pt].hyperlink().map(|h| h.id().to_string()) == c.link_id
                };
                let i = c.line.cells.iter().position(|(_, q)| *q == at)?;
                let mut a = i;
                while a > 0 && same(c.line.cells[a - 1].1) {
                    a -= 1;
                }
                let mut b = i;
                while b + 1 < c.line.cells.len() && same(c.line.cells[b + 1].1) {
                    b += 1;
                }
                Match { start: c.line.cells[a].1, end: c.line.cells[b].1 }
            }
        };
        Some((c, target, cells))
    }

    /// Win-click (and the Open URL/Semantic History pointer actions): opens what is at
    /// the click. Returns false when nothing there opens.
    pub(super) fn open_target(&mut self, id: PaneId, point: Point, background: bool, event_loop: &ActiveEventLoop) -> bool {
        let found = self.target_at(id, point);
        // Command Info runs nothing and reads only the pane's own marks, so it opens in a pane whose
        // text a remote host wrote too (B5); everything it types goes through the guard.
        if let Some((_, Target::CommandInfo(mark), _)) = &found {
            let mark = *mark;
            self.show_command_info(id, mark, (self.cursor.0, self.cursor.1), event_loop);
            return true;
        }
        // Text a remote host wrote never builds a local run (P10): only a web or mail link
        // opens, anything else may only download.
        let link = found.as_ref().and_then(|(_, t, _)| match t {
            Target::Link(u) | Target::Url(u) => Some(u.as_str()),
            _ => None,
        });
        if ssh_files::click_route(self.remote_text(id), link) == ssh_files::ClickRoute::SecureCopyOnly {
            return self.secure_copy_at(id, point);
        }
        let Some((c, target, _)) = found else { return self.secure_copy_at(id, point) };
        // A bound ssh: link opens a Termurion tab, never a local run, and never from a remote pane (P4).
        if let Target::Link(u) | Target::Url(u) = &target {
            let bound = self.bound_ssh_profile();
            if let Some(guid) = super::ssh_url_app::click_plan(self.remote_text(id), u, bound.as_deref()) {
                let url = u.clone();
                return self.open_ssh_url(&url, Some(guid));
            }
        }
        let cwd = c.ctx.cwd.as_ref().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
        let run = match &target {
            Target::Action { rule, action, .. } => {
                let (Some(a), Some(f)) = (c.rules.get(*rule).and_then(|r| r.actions.get(*action)), c.found.as_ref()) else { return self.secure_copy_at(id, point) };
                self.smart_action_run(id, a, &f.captures, &cwd)
            }
            _ => {
                // \3 and \4 come from a fresh context (`UAH:401-430`).
                let (prefix, suffix) = self.action_context(id, point).unwrap_or_else(|| (c.ctx.before.clone(), c.ctx.after.clone()));
                let around = Around { prefix: &prefix, suffix: &suffix, cwd: &cwd };
                semantic::plan(&target, &c.prefs, &around, has_code())
            }
        };
        match run {
            Some(run) => {
                // A file is never handed to the URL handler (`handler_applies`).
                let from_file = matches!(target, Target::File { .. });
                self.execute_with(id, run, c.ctx.cwd.as_deref(), Exec { background, from_file, event_loop: Some(event_loop) });
                true
            }
            None => self.secure_copy_at(id, point),
        }
    }

    /// What a Smart Selection action runs. Copy runs here and returns None. The parameter is
    /// iTerm2's interpolated string with `matches` encoded per action when the profile says so
    /// (`CAP:161-225`), else the `\0`..`\8` form; a command action's line is built by the one
    /// cmd.exe builder either way, and a value it refuses runs nothing (B3).
    pub(super) fn smart_action_run(&mut self, id: PaneId, a: &smartsel::RuleAction, captures: &[String], cwd: &str) -> Option<Run> {
        // An evaluation error or a refused value runs nothing and says why.
        let param = match self.smart_param(id, a, captures, cwd)? {
            Ok(p) => p,
            Err(why) => return Some(Run::Blocked(if why.is_empty() { "The action's parameter could not be evaluated.".into() } else { why })),
        };
        Some(match a.action.as_str() {
            "openFile:" => semantic::open_file_run(&param, (!cwd.is_empty()).then(|| std::path::Path::new(cwd)), has_code()),
            "openUrl:" => semantic::open_url_run(&param, has_code()),
            "runCommand:" => Run::Command(param),
            "runCommandInWindow:" => semantic::run_in_window(&param),
            "runCoprocess:" => Run::Coprocess(param),
            "sendText:" => Run::Send(param),
            "copy:" => {
                if let Some(cb) = &mut self.clipboard
                    && let Err(e) = cb.set_text(param) {
                        self.banners.set("clipboard", format!("Copy failed: {e}"));
                    }
                return None;
            }
            other => {
                self.banners.set("action", format!("Unknown Smart Selection action {other:?}."));
                return None;
            }
        })
    }

    /// Runs a Win-click's plan where no window can open and no file was clicked: a link opened by a
    /// program's request, a label, a menu pick. See `execute_with`.
    pub(super) fn execute(&mut self, id: PaneId, run: Run, cwd: Option<&std::path::Path>, background: bool) {
        self.execute_with(id, run, cwd, Exec { background, from_file: false, event_loop: None });
    }

    /// Runs a plan. An http, https, mailto or ftp URL goes to `UrlHandlerCommand` when one is
    /// set (never a file); `Run::Window` opens a single-use window and needs the event loop.
    pub(super) fn execute_with(&mut self, id: PaneId, run: Run, cwd: Option<&std::path::Path>, how: Exec) {
        let Exec { background, from_file, event_loop } = how;
        // Send Text and Run Coprocess need the pane's shell; an ended session says so (P11).
        if semantic::needs_live_session(&run) && self.panes.get(&id).is_some_and(|p| p.exited.is_some() || p.ended) {
            self.banners.set("action", "The session has ended.");
            return self.redraw();
        }
        match run {
            Run::Open(target) => {
                if semantic::handler_applies(&Run::Open(target.clone()), from_file) && self.url_handler(id, &target, cwd, background) {
                    // The handler took it, or said why not.
                } else if !from_file && crate::browser::web_link(&target).is_some() {
                    // A web link a person asked for: a browser pane, the ask, or the system (13x Task 9).
                    self.open_link(&target, crate::browser::Style::Tab, background, Some(id));
                } else if crate::semantic::shell_url(&target).is_some() {
                    if let Err(e) = shell_open(&target, background) {
                        self.banners.set("action", format!("Could not open {target}: {e}"));
                    }
                } else {
                    // A file Semantic History found: judged again here with its own rule (the ignore
                    // list and the network-mounts setting), and opened only as that LocalPath.
                    let dir = cwd.and_then(crate::localfs::LocalPath::judge);
                    match crate::localfs::LocalPath::semantic(&target, &self.path_probe()) {
                        Some(file) => {
                            if let Err(e) = crate::localfs::open_with_shell(&file, dir.as_ref(), background) {
                                self.banners.set("action", format!("Could not open {target}: {e}"));
                            }
                        }
                        None => self.banners.set("action", crate::localfs::refused().to_string()),
                    }
                }
            }
            Run::Command(line) => {
                let mut cmd = std::process::Command::new("cmd.exe");
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    // CREATE_NO_WINDOW: the command's own windows still show.
                    cmd.raw_arg(semantic::cmd_args(&line)).creation_flags(0x0800_0000);
                }
                if let Some(d) = cwd.and_then(crate::localfs::LocalPath::judge).filter(crate::localfs::is_dir) {
                    crate::localfs::set_cwd(&mut cmd, &d);
                }
                if let Err(e) = cmd.spawn() {
                    self.banners.set("action", format!("Could not run {line}: {e}"));
                }
            }
            Run::Send(text) => {
                if let Some(p) = self.panes.get_mut(&id) {
                    // A password read is never fed program text (B5).
                    if p.at_password_prompt() {
                        self.banners.set("action", super::cmdinfo_app::PASSWORD_PROMPT);
                    } else {
                        p.scroll(Scroll::Bottom);
                        p.input(text.as_bytes());
                    }
                }
            }
            Run::Later(name, m) => self.banners.set("action", format!("{name} comes in milestone {m}.")),
            Run::Coprocess(command) => self.start_coprocess(id, &command, false, crate::coprocess::Entry::SemanticHistory),
            Run::Blocked(why) => self.banners.set("action", why),
            Run::Reveal(path) => self.reveal_in_explorer(&path),
            Run::OpenScheme(url) => self.open_scheme_link(&url, background),
            Run::OpenLocal { path, has_code } => self.open_local(id, path, has_code, background),
            Run::Spawn { program, args } => self.spawn_direct(&program, &args, cwd),
            Run::Window(command) => match event_loop {
                Some(event_loop) => self.run_in_new_window(id, command, cwd, event_loop),
                None => self.banners.set("action", "Termurion could not open a window for this command."),
            },
        }
        self.redraw();
    }

    /// Starts a program directly (one `CreateProcessW`, no shell). A name without a folder is
    /// found on the PATH; a batch file is refused any argument cmd.exe would read as syntax.
    fn spawn_direct(&mut self, program: &std::path::Path, args: &[String], cwd: Option<&std::path::Path>) {
        let found = semantic::resolve_on_path(program, std::env::var_os("PATH").as_deref());
        let Some(exe) = found else {
            self.banners.set("action", format!("Could not start {}: not found.", program.display()));
            return;
        };
        if semantic::is_batch(&exe) && args.iter().any(|a| semantic::batch_unsafe(a)) {
            self.banners.set("action", "Termurion did not start this program: a batch file would read special characters in the file name.");
            return;
        }
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        if let Some(d) = cwd.and_then(crate::localfs::LocalPath::judge).filter(crate::localfs::is_dir) {
            crate::localfs::set_cwd(&mut cmd, &d);
        }
        if let Err(e) = cmd.spawn() {
            self.banners.set("action", format!("Could not start {}: {e}", exe.display()));
        }
    }

    /// Run Command in Window: a new single-use window of the default profile runs `command`
    /// (`IC:2139-2200`), in the pane's folder when that is a local folder. The pane is
    /// registered as single-use so a session end or close clears its record; it is not
    /// short-lived, its end follows the default profile's end action and no restart route
    /// restarts it (P20, `run_window_single_use`).
    fn run_in_new_window(&mut self, owner: PaneId, command: String, cwd: Option<&std::path::Path>, event_loop: &ActiveEventLoop) {
        let probe = self.path_probe();
        let dir = cwd
            .map(|d| d.to_string_lossy().to_string())
            .filter(|d| d.as_bytes().get(1) == Some(&b':') && crate::localfs::LocalPath::semantic(d, &probe).is_some_and(|d| crate::localfs::is_dir(&d)));
        match self.open_single_use_window(event_loop, command, dir) {
            Some(new) => {
                let end = self.store.default_profile().int(crate::session_end::END_ACTION_KEY);
                self.single_use.insert(new, super::git_ui::run_window_single_use(owner, end));
            }
            None => self.banners.set("action", "Termurion could not open a window for this command."),
        }
    }

    /// Shows `path` selected in Explorer; nothing runs it. Used by semantic history and the AI's
    /// `create_file`.
    pub(super) fn reveal_in_explorer(&mut self, path: &str) {
        #[cfg(windows)]
        let program = crate::script_provision::system_directory().and_then(|s| semantic::explorer_in(&s));
        #[cfg(not(windows))]
        let program: Option<std::path::PathBuf> = None;
        let Some(program) = program else {
            self.banners.set("action", format!("Could not show {path}: the Windows folder is unknown."));
            return;
        };
        let Some(arg) = semantic::explorer_select_arg(path) else {
            self.banners.set("action", format!("Termurion does not show this path in Explorer: {path:?}"));
            return;
        };
        let mut cmd = std::process::Command::new(program);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.raw_arg(arg);
        }
        #[cfg(not(windows))]
        let _ = arg;
        if let Err(e) = cmd.spawn() {
            self.banners.set("action", format!("Could not show {path}: {e}"));
        }
    }

    /// What Win-hover shows: the underline (`EnableUnderlineSemanticHistoryOnCmdHover`) and the
    /// URL preview (`ShowURLPreviewForSemanticHistory`).
    pub(super) fn hover_wanted(&self) -> (bool, bool) {
        (self.globals.bool("EnableUnderlineSemanticHistoryOnCmdHover"), self.globals.bool("ShowURLPreviewForSemanticHistory"))
    }

    /// The Open URL a Smart Selection action at the click computes, for the hover preview.
    fn smart_open_url(&self, id: PaneId, c: &Clicked, target: &Target) -> Option<String> {
        let Target::Action { rule, action, .. } = target else { return None };
        let (a, f) = (c.rules.get(*rule)?.actions.get(*action)?, c.found.as_ref()?);
        if a.action != "openUrl:" {
            return None;
        }
        let cwd = c.ctx.cwd.as_ref().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
        self.smart_param(id, a, &f.captures, &cwd)?.ok()
    }

    /// A Smart Selection action's parameter with its captures, the working directory and the
    /// pane's host and user filled in: the one computation the hover preview and the run share.
    fn smart_param(&self, id: PaneId, a: &smartsel::RuleAction, captures: &[String], cwd: &str) -> Option<Result<String, String>> {
        let (host, user) = self.host_user(id)?;
        Some(smartsel::parameter(a, captures, self.smart_interpolated(id), &|t, m, cmd| self.evaluate_matches_with(id, t, m, cmd), cwd, &host, &user))
    }

    /// The pane's profile says Smart Selection parameters are interpolated strings.
    fn smart_interpolated(&self, id: PaneId) -> bool {
        self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).is_some_and(|p| p.bool("Smart Selection Actions Use Interpolated Strings"))
    }

    /// The pane's reported host and user, empty when it reports none.
    pub(super) fn host_user(&self, id: PaneId) -> Option<(String, String)> {
        let shell = &self.panes.get(&id)?.own().shell;
        Some((shell.host.clone().unwrap_or_default(), shell.user.clone().unwrap_or_default()))
    }

    /// Win held over a pane: underline the Win-click target under the pointer (with its key),
    /// show a hand, and preview its URL (with its key). Called when the pointer moves and when
    /// the modifiers change. Resolving probes the disk, so it runs only when the pane, the cell,
    /// Win or a hover key changed since the last call (P2).
    pub(super) fn update_hover(&mut self) {
        let (x, y) = self.cursor;
        let (underline, preview) = self.hover_wanted();
        let window_key = semantic::hover_allowed(self.focused && self.app_active, self.globals.bool("CmdClickWhenInactiveInvokesSemanticHistory"));
        let want = window_key && self.pointer_mods().cmd && !self.pointer_mods().ctrl && self.globals.bool("CommandSelection") && matches!(self.drag, Drag::None);
        let at = want.then(|| self.pane_at(x, y)).flatten().map(|(id, rect)| {
            let (col, row, _) = self.cell_at(id, rect, x, y);
            (id, self.grid_point(id, col, row))
        });
        let key = hover_key(at, underline, preview);
        if key != self.hover_key {
            self.hover_key = key;
            let found = at.and_then(|(id, point)| self.target_at(id, point).map(|(c, target, cells)| (id, c, cells, target)));
            // An ssh: link the click would refuse shows no underline and no hand.
            let found = found.filter(|(id, _, _, t)| match t {
                Target::Link(u) | Target::Url(u) if super::ssh_url_app::is_ssh(u) => {
                    super::ssh_url_app::hover_open(self.remote_text(*id), u, self.bound_ssh_profile().as_deref())
                }
                _ => true,
            });
            // A link's text can differ from where it goes, so the URL shows while hovering.
            let text = found.as_ref().and_then(|(id, c, _, t)| {
                semantic::preview_text(t, self.smart_open_url(*id, c, t).as_deref(), preview)
            });
            match text {
                Some(t) => self.banners.set("hover", t),
                None => self.banners.clear("hover"),
            }
            self.hover_hand = found.is_some();
            let hover = found.and_then(|(id, _, cells, _)| hover_cells(cells, underline).map(|m| (id, m)));
            if hover != self.hover {
                self.hover = hover;
                self.redraw();
            }
        }
        self.refresh_pointer_shape();
    }

    /// Opens text picked from a label: an existing file (with its line) through Semantic
    /// History, else the text as a URL. In a remote pane only a web or mail link opens; other
    /// text may only download (P10).
    pub(super) fn open_text(&mut self, id: PaneId, text: &str) {
        let urls = self.url_prefs(id);
        if self.remote_text(id) {
            let link = semantic::url_like_in("", text, &urls);
            match link.filter(|u| ssh_files::click_route(true, Some(u.as_str())) == ssh_files::ClickRoute::Local) {
                Some(u) => self.execute(id, semantic::guard_open(&u, has_code()), None, false),
                None => {
                    self.secure_copy_text(id, Some(text));
                }
            }
            return;
        }
        let cwd = self.pane_dir(id);
        let prefs = self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).map(|p| Prefs::from_value(p.get("Semantic History").as_ref())).unwrap_or_default();
        let target = match semantic::find_path("", text, cwd.as_deref(), &self.path_probe()) {
            Some((path, line, column)) => Target::File { path, line, column },
            None => Target::Url(semantic::url_like_in("", text, &urls).unwrap_or_else(|| text.to_string())),
        };
        let cwd_s = cwd.as_ref().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
        let around = Around { prefix: "", suffix: text, cwd: &cwd_s };
        if let Some(run) = semantic::plan(&target, &prefs, &around, has_code()) {
            self.execute(id, run, cwd.as_deref(), false);
        }
    }

    /// Open Selection as URL and the split items (`CMH:1151-1167`, `findUrlInString`): the URL in the
    /// selected text opens as a tab or beside the pane. Text a remote host wrote opens only a web or
    /// mail link (P10), and every scheme goes through `guard_open`.
    pub(super) fn open_selection_url(&mut self, id: PaneId, text: &str, style: crate::browser::Style) {
        let urls = self.url_prefs(id);
        let Some(u) = semantic::url_like_in("", text, &urls) else { return };
        if ssh_files::click_route(self.remote_text(id), Some(&u)) != ssh_files::ClickRoute::Local {
            return;
        }
        match semantic::guard_open(&u, has_code()) {
            Run::Open(t) if style != crate::browser::Style::Tab => {
                // The URL handler first, as a link's open does, then the pane beside this one.
                if !self.url_handler(id, &t, None, false) && crate::browser::web_link(&t).is_some() {
                    self.open_link(&t, style, false, Some(id));
                }
            }
            run => self.execute(id, run, None, false),
        }
    }

    /// Copy Link Address, the pointer action: the link or URL at a point.
    pub(super) fn copy_link_at(&mut self, id: PaneId, point: Point) {
        let Some(c) = self.clicked(id, point) else { return };
        let Some(link) = c.ctx.link.clone().or_else(|| semantic::url_like_in(&c.ctx.before, &c.ctx.after, &c.ctx.urls)) else { return };
        if let Some(cb) = &mut self.clipboard
            && let Err(e) = cb.set_text(link) {
                self.banners.set("clipboard", format!("Copy failed: {e}"));
            }
    }
}

pub(super) struct Clicked {
    pub(super) ctx: ClickContext,
    /// The clicked logical line.
    line: Logical,
    /// The cells of `ctx.before + ctx.after` and of `ctx.respecting`.
    text: sh_context::Context,
    hard: Option<sh_context::Context>,
    /// The lines Smart Selection looked at, and its match.
    pub(super) joined: Logical,
    pub(super) found: Option<smartsel::Found>,
    rules: Vec<smartsel::Rule>,
    prefs: Prefs,
    link_id: Option<String>,
}

/// VS Code is on the PATH, so the best editor can open a file at a line.
pub(super) fn has_code() -> bool {
    static HAS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HAS.get_or_init(|| {
        std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).any(|d| ["code.cmd", "code.exe"].iter().any(|f| crate::localfs::is_local_file(&d.join(f))))
        })
    })
}

/// Opens a URL with its default app. Only a URL (`semantic::shell_url`): a path, a drive path or a
/// file link is refused here, so no caller hands the shell an unjudged file; a file opens through
/// `localfs::open_with_shell` with a judged `LocalPath`. `background` asks the app not to take the focus.
pub(super) fn shell_open(target: &str, background: bool) -> Result<(), String> {
    crate::platform::shell_open::open_with(target, background)
}

/// Hands a link of another app's scheme to the app Windows registered for it: the URL is judged again
/// (`link_scheme::screen`), percent-encoded so no quote or space can split what the app's command
/// receives, and passed to ShellExecuteW as the one file argument, never as a shell string.
#[cfg(windows)]
pub(super) fn shell_open_scheme(url: &str, background: bool) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, SW_SHOWNORMAL};
    let scheme = crate::link_scheme::scheme_of(url).ok_or_else(|| "Termurion opens only links here, not paths.".to_string())?;
    crate::link_scheme::screen(&scheme, url)?;
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, file) = (wide("open"), wide(&crate::semantic::encode_url_chars(url)));
    let show = if background { SW_SHOWNOACTIVATE } else { SW_SHOWNORMAL };
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), show) };
    if r as isize > 32 { Ok(()) } else { Err(format!("ShellExecute error {}", r as isize)) }
}

#[cfg(not(windows))]
pub(super) fn shell_open_scheme(_url: &str, _background: bool) -> Result<(), String> {
    Err("opening a link needs Windows".to_string())
}

#[cfg(test)]
mod title_menu_tests {
    use super::*;

    #[test]
    fn smart_selection_radius_follows_the_key() {
        let mut g = crate::globals::Globals::empty(std::path::Path::new("radius-test.toml"));
        assert_eq!(radius(&g), 2);
        g.set("SmartSelectionRadius", toml::Value::Integer(0));
        assert_eq!(radius(&g), 0);
        g.set("SmartSelectionRadius", toml::Value::Integer(5));
        assert_eq!(radius(&g), 5);
        g.set("SmartSelectionRadius", toml::Value::Integer(-4));
        assert_eq!(radius(&g), 0);
        g.set("SmartSelectionRadius", toml::Value::Integer(100_000));
        assert_eq!(radius(&g), 50);
    }
}

#[cfg(test)]
mod tests {
    /// The 13w merge (S14 and the one locality answer): a browser pane has no directory, and no
    /// browser code makes its own "is this on this computer" check: a session's locality comes
    /// only from `statusbar_content::is_this_machine` (the token-aware one).
    #[test]
    fn browser_panes_have_no_dir_and_no_own_locality_check() {
        let me = include_str!("semantic_ui.rs");
        let body = &me[me.find("pub(super) fn pane_dir(").unwrap()..];
        let body = &body[..body.find("\n    }\n").unwrap()];
        assert!(body.find("self.is_browser_pane(id)").unwrap() < body.find("is_this_machine(").unwrap(), "the browser pane answers None before any locality read");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for f in ["browser.rs", "webhost.rs", "webhost_win.rs", "web_popover.rs", "platform/windows/web_popover.rs", "app/browser_app.rs", "app/web_popover_app.rs", "app/webhost_app.rs", "app/tool_web_ui.rs"] {
            let text = std::fs::read_to_string(root.join(f)).unwrap();
            let live = text.split("#[cfg(test)]\nmod tests").next().unwrap();
            for own in ["local_names(", "is_local_session(", "host_identity::", "GetComputerName", "machine_id"] {
                assert!(!live.contains(own), "{f}: an own locality check ({own}); use statusbar_content::is_this_machine");
            }
        }
    }
    /// Final wave minor 9: Send Text and Smart Selection sendText: do not type at a password prompt.
    #[test]
    fn send_text_refuses_a_password_prompt() {
        let src = include_str!("semantic_ui.rs");
        let arm = &src[src.find("Run::Send(text) => {").unwrap()..];
        let arm = &arm[..arm.find("Run::Later").unwrap()];
        assert!(arm.find("at_password_prompt()").unwrap() < arm.find("p.input(").unwrap(), "{arm}");
    }

    /// The functions that turn text in a pane into a local open, run or coprocess, or a pane's
    /// reported folder into a local shell's folder or an Explorer target: each asks the one
    /// guard, `remote_text` (P10), or `pane_dir`, which asks it. Win-click and the Open Target
    /// pointer actions reach `open_target`; the right-click menu and its pointer action
    /// `show_context_menu_at` and `context_other_picked`; Open Selection and a label pick `open_text`;
    /// OSC 1337 OpenURL `open_url`; the status bar's Current Directory menu `dir_menu_picked`; a
    /// Porthole document link `open_porthole_link` and `open_link_target`; a new session's
    /// folder `previous_dir`. Paths are under src/.
    const GATED: &[(&str, &str)] = &[
        ("app/semantic_ui.rs", "open_target"),
        ("app/semantic_ui.rs", "open_selection_url"),
        ("app/browser_app.rs", "open_link_split"),
        ("app/ctx_menu_app.rs", "show_context_menu_at"),
        ("app/ctx_menu_app.rs", "context_other_picked"),
        ("app/ctx_menu_app.rs", "open_selection_mail"),
        ("app/semantic_ui.rs", "open_text"),
        ("app/semantic_ui.rs", "pane_dir"),
        ("app/prompted_ui.rs", "open_url"),
        ("app/statusbar_content.rs", "dir_menu_picked"),
        ("app/porthole_ui.rs", "open_porthole_link"),
        ("app/porthole_ui.rs", "open_link_target"),
        ("app/mod.rs", "previous_dir"),
    ];

    /// The guard calls a GATED function may make.
    const GUARDS: &[&str] = &["remote_text(", "self.pane_dir("];

    /// Calls that open or run something on this machine: Termurion's own sinks and every direct
    /// process start (`Command::new`, `ShellExecuteW`, `ShellExecuteExW`, `CreateProcessW`).
    const SINKS: &[&str] = &[
        "self.execute(",
        "self.execute_with(",
        // Every way into the browser pane or the system from a link (Task 9 review I2).
        "self.open_link(",
        "self.open_link_system(",
        "self.open_url_in_browser(",
        // The one funnel for a URL a page supplied (13x2): its callers are audited too.
        "self.browser_page_open(",
        "self.open_link_answered(",
        "self.upsell_answered(",
        "smart_action_run(",
        "semantic::plan(",
        "guard_open(",
        "shell_open(",
        // The platform funnel: any app file calling it is audited like shell_open itself.
        "shell_open::open",
        // The platform spawn funnel: any caller is audited like a spawn itself.
        "start_detached(",
        "imp::open_url(",
        "open_with_shell(",
        "shell_verb(",
        "start_coprocess(",
        "start_coprocess_titled(",
        "reveal_in_explorer(",
        "Command::new(",
        "ShellExecuteW(",
        "ShellExecuteExW(",
        "CreateProcessW(",
    ];

    /// Callers of a sink whose input is not text a remote host wrote, and why.
    const NOT_PANE_TEXT: &[(&str, &str, &str)] = &[
        ("settings_effects.rs", "open_folder", "shows Termurion's own RenderPlugins folder beside its settings (render_fx::user_dir) on the person's click, never pane text; nothing is run"),
        ("app/browser_extensions_app.rs", "browser_extensions_reveal", "only SELECTS the extensions folder in Explorer: a local drive path (root_ok, judged) the profile or the default names, never pane text; nothing is opened or run"),
        ("app/transfers_ui.rs", "transfer_picked", "Open is the person's click on a file Termurion itself saved from a transfer: a program or script type was saved as `name.ext.download`, a type the shell reads on listing was refused (inert_files), and the path comes from the transfer list, never pane text"),
        ("regex_railroad.rs", "open", "opens the page this module wrote itself in the temp folder (a judged file whose name is the process id), never pane text; the regular expression is only drawn in it"),
        ("app/ai_app.rs", "ai_key_event", "opens a vendor page from Termurion's own text table (settings_ai::registration_message), never pane text"),
        ("app/alerts_ui.rs", "reveal_notified", "opens Termurion's own help page (links::HELP_COPY_MODE) after its own notification was clicked, never pane text"),
        ("app/panels_app.rs", "panel_act", "opens links Termurion's own windows carry (the issue page, What's New pages), never pane text"),
        ("app/triggers_ui.rs", "slow_offer_answered", "opens Termurion's own help page (links::HELP_TRIGGERS), never pane text"),
        ("app/triggers_ui.rs", "show_trigger_stats", "opens a text file Termurion just wrote itself in the temp folder, never pane text"),
        ("app/semantic_ui.rs", "execute", "the sink itself; its callers are checked"),
        ("app/semantic_ui.rs", "execute_with", "the sink itself; its callers are checked"),
        ("app/url_handler_ui.rs", "url_handler", "starts the person's own handler program directly; reached only from execute_with, the URL is one argument (handler_plan)"),
        ("app/semantic_ui.rs", "smart_action_run", "builds the Run; its callers are checked"),
        ("app/pathnav_ui.rs", "pathnav_tick", "runs what open_local settled: a Run that came through guard_open or open_file_run, whose callers are checked"),
        ("app/semantic_ui.rs", "shell_open", "the sink itself; its callers are checked"),
        ("platform/shell_open.rs", "open_with", "the platform sink itself: it refuses anything but a URL (semantic::shell_url) first; its callers are checked through the shell_open::open token"),
        ("platform/windows/shell_open.rs", "open_url", "the backend of platform::shell_open::open_with, which judges the target as a URL (semantic::shell_url) before calling it; only that function reaches it"),
        ("platform/unix/shell_open.rs", "open_url", "the backend of platform::shell_open::open_with, which judges the target as a URL (semantic::shell_url) before calling it; only that function reaches it"),
        ("app/semantic_ui.rs", "shell_open_scheme", "the sink itself; its only caller is link_scheme_app::launch_scheme, after link_scheme::decide and the person's answer"),
        ("app/browser_download_app.rs", "browser_downloads_message", "Show in folder only SELECTS the file Termurion itself finished in the Downloads folder (the id names its own record, never the page's text); nothing is opened or run"),
        ("app/page_saver_app.rs", "page_save_acts", "selects the index.html the saver itself wrote in the folder the person picked in Explorer; nothing is opened or run, and the path is not pane text"),
        ("app/workgroup_toolbar_ui.rs", "usage_report_answered", "opens the fixed issue form; the person's click on Report Bug, the query holds only the report Termurion built"),
        ("app/workgroup_toolbar_ui.rs", "run_usage", "runs `claude.exe` from the path or the item's own command line (the person's settings, never pane text), with no substitution"),
        ("app/browser_app.rs", "open_link", "takes a Run::Open from guard_open or a web_link; its callers (execute_with, open_selection_url, open_link_split) are checked"),
        ("app/browser_app.rs", "open_link_answered", "the answer to the ask open_link raised for a link it had settled"),
        ("app/browser_app.rs", "upsell_answered", "the answer to the upsell open_link raised; opens the fixed plugin page or the link it settled"),
        ("app/browser_ctx_app.rs", "browser_ctx_picked", "Search with opens a URL built from the person's own selection through browser_page_open (nav_gate first); no other item opens anything"),
        ("app/browser_smart_app.rs", "browser_smart_action", "a page's Smart Selection action, from the person's own menu pick: the one parameter builder (smartsel::parameter, the 13p-r refusals), a URL only through browser_page_open, a file and a command only after the ask that shows the whole text"),
        ("app/browser_input_app.rs", "browser_next_tick", "opens a page-supplied URL only through browser_page_open (queued only when nav_gate said Navigate), a Smart Selection action only through browser_smart_action (its ask), and the session's own pointer actions"),
        ("app/browser_smart_app.rs", "browser_open_file_confirmed", "the answer to the Open file? ask: open_file_run's plan (a program is revealed, never run) through the sink"),
        ("app/browser_smart_app.rs", "browser_runs_tick", "runs the commands the person confirmed, each built by the one cmd.exe builder before the ask, in a single-use window"),
        ("app/browser_scripts_app.rs", "browser_page_open", "the one gate for URLs a page, page text or an arrangement supplied: nav_gate, then guard_open, never a path"),
        ("browser_js.rs", "nav_gate", "builds a NavGate and opens nothing; its caller is checked"),
        ("app/web_popover_app.rs", "quick_look_url", "only an http or https URL with a host (web_popover::quick_look_target) is shown or opened, the one kind a remote pane's link may open (P10)"),
        ("app/web_popover_app.rs", "web_popover_event", "opens the page the popover shows, only when its scheme is http or https (quick_look_scheme_ok)"),
        ("app/web_popover_app.rs", "web_popover_view_event", "an external protocol a popover's page asked for goes through external_uri_run (guard_open), only for a person's gesture"),
        ("app/browser_app.rs", "open_link_system", "opens a link execute or the menu already settled (guard_open, the old road for every non-web scheme); only shell_url's kinds open"),
        ("app/browser_app.rs", "upsell_answered", "opens the constant Microsoft WebView2 page (browser::PLUGIN_URL) through guard_open, or the link a person clicked, which open_link already cleaned"),
        ("app/browser_app.rs", "run_external", "an external protocol a browser page asked for or a person typed goes through external_uri_run (guard_open), only for a person's gesture"),
        ("app/web_popover_app.rs", "external_uri_run", "builds a decision and opens nothing; its caller is checked"),
        ("localfs.rs", "open_with_shell", "the file sink itself: it takes a judged LocalPath; its callers are checked"),
        ("localfs.rs", "shell_verb", "the file sink itself: it takes a judged LocalPath; its callers are checked"),
        ("app/scripts_alerts.rs", "bind_scheme_confirmed", "opens the constant Default apps page (url_schemes::DEFAULT_APPS_URI), never text from a pane or a link"),
        ("app/semantic_ui.rs", "reveal_in_explorer", "the sink itself; its callers are checked"),
        ("app/ctx_menu_app.rs", "context_image_picked", "opens a PNG it encoded itself from the picture's pixels, in Termurion's own folder, under a name made of the process id and the image number; no pane text is a path or a command"),
        ("app/dynamic_profiles_app.rs", "reveal_dynamic", "selects a Dynamic Profiles file below the folder (a plain-name path the network judge passed); nothing runs it"),
        ("app/scripts_alerts.rs", "scripts_answered", "the Dynamic Profiles error's Reveal in Explorer selects the file the loader named; nothing runs it"),
        ("app/semantic_ui.rs", "spawn_direct", "the sink itself; execute's callers are checked"),
        ("semantic.rs", "plan", "builds a Run and opens nothing; its callers are checked"),
        ("semantic.rs", "open_url_run", "builds a Run and opens nothing; smart_action_run's callers are checked"),
        ("coproc_win.rs", "spawn", "the coprocess start; start_coprocess's callers are checked"),
        ("app/actions.rs", "run_key_action", "the user's own key binding"),
        ("app/ai_tools_app.rs", "ai_reveal_file", "reveals the AI's own new file, runs nothing"),
        ("ai_actions.rs", "get_help", "PowerShell Get-Help on a name man_page_plan checked, runs no command"),
        ("app/api_ui.rs", "open_info_link", "a fixed More Info page"),
        ("app/claude_code_app.rs", "cc_onboarding_event", "the fixed Learn more page (claude_code::HELP_URL)"),
        ("app/api_ui.rs", "reveal_allow_all_file", "reveals Termurion's own allow-all file"),
        ("app/web_replay_app.rs", "web_replay_saved", "selects Termurion's own saved movie in Explorer (a path it made itself)"),
        ("app/builtin_ui.rs", "session_method", "a local script's request"),
        ("app/chat_window_app.rs", "chat_open_url", "a link in the AI chat window, not a pane"),
        ("app/chat_window_app.rs", "chat_file", "an AI chat attachment, inert types only (F19)"),
        ("app/coprocess_ui.rs", "coprocess_prompt_event", "the command the user typed"),
        ("app/coprocess_ui.rs", "coprocess_bar", "Notepad on Termurion's own coprocess error file"),
        ("app/coprocess_ui.rs", "flush_dropped", "files the user dropped, through the one cmd.exe builder and its refusal (drop_command)"),
        ("app/coprocess_ui.rs", "start_coprocess", "the sink itself; its callers are checked"),
        ("app/toolbelt_more_ui.rs", "run_captured", "a Captured Output command the user's own Capture Output trigger built through the one cmd.exe builder and its refusal"),
        ("app/host_offers_ui.rs", "host_offer_bar", "a fixed help page"),
        ("app/menu.rs", "run_command", "the Help menu's fixed documentation page"),
        ("app/menu.rs", "open_help_page", "fixed help pages from the Help menu (the repository, Copy Mode shortcuts), never pane text"),
        ("app/mod.rs", "window_event", "the Configure SSH help page, the color gallery page, an exported color preset revealed"),
        ("app/prefs_ui.rs", "curl_fetch", "the settings URL the user typed in Settings"),
        ("app/screenshot_ui.rs", "shot_event", "reveals the screenshot Termurion just saved"),
        ("app/scripts_app.rs", "edit_new_script", "the user's new script, edited, never run"),
        ("app/scripts_app.rs", "reveal_path", "a script in the user's Scripts folder, revealed"),
        ("app/settings_export_ui.rs", "start_relaunched", "starts Termurion's own executable"),
        ("app/snippets_ui.rs", "run_applied", "the user's own key binding"),
        ("app/ssh_panel_app.rs", "reveal_all", "the Download Files batch folder Termurion made (P12)"),
        ("app/tmux_ui.rs", "tmux_bar", "a fixed help page"),
        ("app/triggers_ui.rs", "trigger_request", "the user's own trigger rule (ruling)"),
        ("app/triggers_ui.rs", "start_command", "Run Command for the user's own trigger rule (ruling)"),
        ("bin/termurion-git.rs", "run", "the git menu's wrapper: git with the menu's own command"),
        ("git_run_win.rs", "start", "git for the status bar, Termurion's own arguments"),
        ("host_client.rs", "start_host_process", "starts Termurion's own session host by its fixed file name"),
        ("platform/windows/proc.rs", "start_detached", "the spawn funnel itself: it runs the path it is given; its callers are checked through the start_detached( token"),
        ("platform/unix/proc.rs", "start_detached", "the Unix backend: it starts nothing and reports Unsupported"),
        ("host.rs", "reopen_app", "restarts Termurion's own exe, found beside the host's own exe"),
        ("probe.rs", "wsl", "`wsl.exe` with Termurion's own arguments"),
        ("platform/windows/pty.rs", "start", "a pane's shell, from its profile"),
        ("platform/windows/pty.rs", "start_pipes", "a pane's shell, from its profile"),
        ("pwm_run.rs", "run", "the password manager CLI the user set up"),
        ("script_proc.rs", "start", "a local Python API script"),
        ("script_provision.rs", "run_watched", "the scripts' Python setup steps"),
        ("update_fetch_win.rs", "fetch", "curl.exe from System32 with Termurion's own fixed arguments (update::fetch_args), https only"),
        ("app/tips_app.rs", "tip_learn_more", "a URL from the static tips, a fixed https page, through guard_open"),
        ("app/update_app.rs", "learn_more_target", "the pinned releases page (update::release_url) through guard_open"),
        ("app/update_app.rs", "learn_more", "opens what learn_more_target settled, the pinned releases page"),
        ("util_console.rs", "run_command", "termurion-utils' program runner, run by the user at a prompt"),
    ];

    /// What may stand before `fn` in a declaration.
    const QUALIFIERS: &[&str] = &["pub(super) ", "pub(crate) ", "pub ", "unsafe ", "const ", "async ", "extern \"system\" ", "extern \"C\" "];

    fn unqualified(mut t: &str) -> &str {
        while let Some(rest) = QUALIFIERS.iter().find_map(|q| t.strip_prefix(q)) {
            t = rest;
        }
        t
    }

    /// Each `fn` in a source file with its body, up to its test module (`#[cfg(test)]` then
    /// `mod x {`). A `#[cfg(test)]` fn is still read; `unsafe extern "system" fn` is a fn.
    fn functions(src: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        let mut cfg_test = false;
        for line in src.lines() {
            let t = line.trim_start();
            if t.starts_with("#[cfg(test)]") {
                cfg_test = true;
                continue;
            }
            if t.starts_with("#[") {
                continue;
            }
            let bare = unqualified(t);
            if cfg_test && bare.starts_with("mod ") && t.trim_end().ends_with('{') {
                break;
            }
            if !t.is_empty() {
                cfg_test = false;
            }
            if let Some(rest) = bare.strip_prefix("fn ") {
                let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                out.push((name, String::new()));
            } else if let Some((_, body)) = out.last_mut()
                && !t.starts_with("//") {
                    body.push_str(line);
                    body.push('\n');
                }
        }
        out
    }

    /// Every `.rs` file under `dir`, as (path under src/ with `/`, path).
    fn sources(src: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, std::path::PathBuf)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let path = e.unwrap().path();
            if path.is_dir() {
                sources(src, &path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path.strip_prefix(src).unwrap().to_string_lossy().replace('\\', "/");
                out.push((rel, path));
            }
        }
    }

    #[test]
    fn the_scan_reads_every_kind_of_fn() {
        let src = "pub fn a() {\n    x();\n}\nunsafe extern \"system\" fn b() {\n    Command::new(1);\n}\n#[cfg(test)]\nfn c() {\n    y();\n}\n#[cfg(test)]\nmod tests {\n    fn d() {}\n}\n";
        let names: Vec<String> = functions(src).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert!(functions(src)[1].1.contains("Command::new("), "a spawn in an extern fn is its own");
    }

    /// What the scan says is wrong with `files` (path under src/, text): a GATED function that does not
    /// ask the guard, and a caller of a sink that is neither GATED nor listed with a reason.
    fn gate_violations(files: &[(String, String)]) -> (Vec<String>, Vec<(String, String)>) {
        let (mut seen, mut wrong) = (Vec::new(), Vec::new());
        for (file, text) in files {
            for (name, body) in functions(text) {
                let gated = GATED.contains(&(file.as_str(), name.as_str()));
                if gated {
                    if !GUARDS.iter().any(|g| body.contains(g)) {
                        wrong.push(format!("{file} {name} must ask remote_text (P10)"));
                    }
                    seen.push((file.clone(), name.clone()));
                }
                if let Some(sink) = SINKS.iter().find(|k| body.contains(*k)) {
                    let exempt = NOT_PANE_TEXT.iter().any(|(f, n, _)| *f == file && *n == name);
                    if !gated && !exempt {
                        wrong.push(format!("{file} {name} calls {sink}: route it through remote_text (P10) or list why its input is not pane text"));
                    }
                }
            }
        }
        (wrong, seen)
    }

    fn real_sources() -> Vec<(String, String)> {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        sources(&src, &src, &mut files);
        files.sort();
        files.into_iter().map(|(file, path)| (file, std::fs::read_to_string(&path).unwrap())).collect()
    }

    #[test]
    fn every_entry_point_is_gated() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let (mut wrong, seen) = gate_violations(&real_sources());
        for (f, n) in GATED {
            if !seen.iter().any(|(a, b)| a == f && b == n) {
                wrong.push(format!("{f} {n} is gone: update GATED"));
            }
        }
        for (f, n, _) in NOT_PANE_TEXT {
            if !std::fs::read_to_string(src.join(f)).is_ok_and(|t| functions(&t).iter().any(|(m, _)| m == n)) {
                wrong.push(format!("{f} {n} is gone: update NOT_PANE_TEXT"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    /// The guard is proved by planting: a function that hands pane text to any opening call, with no
    /// `remote_text` and no listing, is reported; the same call in a GATED function is not.
    #[test]
    fn a_planted_ungated_opening_call_fails_the_gate() {
        for call in ["self.open_link(text, Style::Tab, false, None)", "self.open_link_system(text, false)", "self.open_url_in_browser(text, Style::Tab, None)", "self.open_link_answered(text, Style::Tab, false, None, false, 0)", "self.upsell_answered(text, 1)", "self.execute_with(id, run, None, exec)", "self.browser_page_open(pane, text, PageOpen::Tab, el)", "crate::platform::shell_open::open(text)", "crate::platform::shell_open::open_with(text, false)", "crate::platform::proc::start_detached(text)"] {
            let mut files = real_sources();
            files.push(("app/planted.rs".to_string(), format!("impl App {{\n    fn evil(&mut self, text: &str) {{\n        {call};\n    }}\n}}\n")));
            let (wrong, _) = gate_violations(&files);
            assert!(wrong.iter().any(|w| w.starts_with("app/planted.rs evil calls")), "{call}: {wrong:?}");
        }
        // A GATED name that asks the guard passes.
        let mut files = real_sources();
        files.push(("app/semantic_ui.rs".to_string(), "fn open_text(&mut self) {\n    if self.remote_text(id) {}\n    self.open_link(t);\n}\n".to_string()));
        let (wrong, _) = gate_violations(&files);
        assert!(!wrong.iter().any(|w| w.contains("open_text")), "{wrong:?}");
    }
}

#[cfg(test)]
mod hover_tests {
    use super::*;
    use crate::find::Pos;

    fn cells() -> Match {
        Match { start: Pos { row: 3, col: 2 }, end: Pos { row: 3, col: 9 } }
    }

    #[test]
    fn hover_underline_follows_the_key() {
        assert_eq!(hover_cells(cells(), true), Some(cells()));
        assert_eq!(hover_cells(cells(), false), None);
    }

    #[test]
    fn hover_key_follows_cell_and_keys() {
        let id: PaneId = 1;
        let at = |line: i32, col: usize| Some((id, Point::new(alacritty_terminal::index::Line(line), alacritty_terminal::index::Column(col))));
        let base = hover_key(at(1, 2), true, true);
        assert_eq!(base, hover_key(at(1, 2), true, true), "same cell, same keys: no recompute");
        assert_ne!(base, hover_key(at(1, 3), true, true));
        assert_ne!(base, hover_key(at(2, 2), true, true));
        assert_ne!(base, hover_key(at(1, 2), false, true));
        assert_ne!(base, hover_key(at(1, 2), true, false));
        assert_ne!(base, hover_key(None, true, true));
    }

    #[test]
    fn context_size_follows_the_key() {
        let row = |t: &str| crate::find::RowText { text: t.into(), cells: t.char_indices().enumerate().map(|(c, (o, _))| (o, find::Pos { row: 0, col: c })).collect(), wraps: false };
        let rows = [row(&"x".repeat(5000))];
        let c = sh_context::build(&rows, 0, 2500, true, 2000);
        assert_eq!((c.before.len(), c.after.len()), (2000, 2000));
        let c = sh_context::build(&rows, 0, 5, true, 10);
        assert_eq!((c.before.len(), c.after.len()), (5, 10), "clamped at the start");
        let c = sh_context::build(&rows, 0, 50, true, 0);
        assert_eq!((c.before.len(), c.after.len()), (0, 0));
    }

    #[test]
    fn prefix_suffix_respect_soft_boundaries() {
        let rows: Vec<Vec<char>> = (0..crate::seltools::DIVIDER_RUN).map(|_| "abc\u{2502}def".chars().collect()).collect();
        assert_eq!(window_for(&rows, 3, 5, 7, true), Some((4, 6)));
        assert_eq!(window_for(&rows, 3, 5, 7, false), None);
        assert_eq!(window_for(&rows, 3, 1, 7, true), Some((0, 2)));
        let plain: Vec<Vec<char>> = (0..crate::seltools::DIVIDER_RUN).map(|_| "abcdefg".chars().collect()).collect();
        assert_eq!(window_for(&plain, 3, 5, 7, true), None);
        assert_eq!(window_for(&plain, 3, 5, 7, false), None);
        let short: Vec<Vec<char>> = rows[..crate::seltools::DIVIDER_RUN - 1].to_vec();
        assert_eq!(window_for(&short, 3, 5, 7, true), None, "a divider needs its run");
    }
}

#[cfg(test)]
mod smart_lines_tests {
    use super::*;

    /// A screen holding one logical line of `n` characters wrapped over many rows.
    fn long_line(n: usize) -> crate::screen::Screen {
        let mut s = crate::screen::Screen::with_scrollback(80, 24, 5000);
        s.feed("x".repeat(n).as_bytes());
        s.feed(b"\r\ntail\r\n");
        s
    }

    #[test]
    fn smart_text_is_bounded_on_a_huge_logical_line() {
        let s = long_line(40_000);
        let at = find::Pos { row: 100, col: 5 };
        let (lines, i) = smart_lines(&s, at, None, 2000, 2);
        let bytes: usize = lines.iter().map(|l| l.text.len()).sum();
        // 27 rows each way at 80 columns, not the 500 rows of the line.
        assert!(bytes <= 55 * 80, "{bytes}");
        assert!(lines[i].offset_of(at).is_some(), "the clicked cell is still in its line");
        // A window cuts the columns too, and the bound holds on the whole buffer's far end.
        let (win, _) = smart_lines(&s, at, Some((0, 9)), 2000, 2);
        assert!(win.iter().map(|l| l.text.len()).sum::<usize>() <= 55 * 10);
        let (top, i) = smart_lines(&s, find::Pos { row: 0, col: 0 }, None, 2000, 2);
        assert!(top[i].offset_of(find::Pos { row: 0, col: 0 }).is_some());
    }

    /// Merge seam (S3): `smart_lines` reads `SmartSelectionRadius` lines each way, not 13r's
    /// fixed 2.
    #[test]
    fn smart_lines_take_the_radius() {
        let mut s = crate::screen::Screen::with_scrollback(80, 24, 100);
        s.feed(b"a\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng\r\n");
        let history = {
            use alacritty_terminal::grid::Dimensions;
            s.term().grid().history_size()
        };
        let at = find::Pos { row: history + 3, col: 0 };
        let texts = |r: usize| smart_lines(&s, at, None, 2000, r).0.into_iter().map(|l| l.text).collect::<Vec<_>>();
        assert_eq!(texts(0), ["d"]);
        assert_eq!(texts(1), ["c", "d", "e"]);
        assert_eq!(texts(3), ["a", "b", "c", "d", "e", "f", "g"]);
        let code = include_str!("semantic_ui.rs");
        let code = &code[..code.find("#[cfg(test)]").unwrap()];
        assert!(!code.contains("const RADIUS"));
        assert_eq!(code.matches(", radius(&self.globals))").count(), 2, "both Smart Selection call sites");
        assert!(code.contains("require_slash: self.globals.bool(\"RequireSlashInURLGuess\")"), "url_prefs reads the row (S1)");
    }

    /// Merge seam (S2): one soft-boundary window rule, word selection's.
    #[test]
    fn semantic_windows_are_word_selection_windows() {
        let rows: Vec<Vec<char>> = (0..crate::seltools::DIVIDER_RUN).map(|_| "abc \u{2502} def \u{2502} ghi".chars().collect()).collect();
        assert_eq!(window_for(&rows, 1, 7, 15, true), Some((5, 9)), "between the dividers");
        for col in [0, 2, 7, 13] {
            assert_eq!(window_for(&rows, 1, col, 15, true), crate::wordsel::window_at(&rows, 1, col, 15), "{col}");
        }
        assert_eq!(window_for(&rows, 1, 7, 15, false), None);
        let plain: Vec<Vec<char>> = vec!["no dividers".chars().collect()];
        assert_eq!(window_for(&plain, 0, 3, 11, true), None);
    }

    #[test]
    fn smart_text_keeps_short_lines_whole() {
        let mut s = crate::screen::Screen::with_scrollback(80, 24, 100);
        s.feed(b"one\r\ntwo https://a.b/c\r\nthree\r\n");
        let history = {
            use alacritty_terminal::grid::Dimensions;
            s.term().grid().history_size()
        };
        let at = find::Pos { row: history + 1, col: 4 };
        let (lines, i) = smart_lines(&s, at, None, 2000, 2);
        assert_eq!(lines[i].text, "two https://a.b/c");
        assert_eq!(lines[i - 1].text, "one");
        assert_eq!(lines[i + 1].text, "three");
    }
}
