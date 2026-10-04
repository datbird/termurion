//! Smart Selection and Semantic History in the window: what a click selects and what a
//! Win-click opens.

use alacritty_terminal::index::Point;

use super::*;
use super::find_ui::{reveal, to_pos};
use crate::find::{self, Logical, Match};
use crate::semantic::{self, Around, ClickContext, Prefs, Run, Target};
use crate::smartsel::{self, Escape};
use crate::ssh_files;

/// Smart Selection looks this many logical lines each way from the click.
const RADIUS: usize = 2;

impl App {
    /// Selects the best Smart Selection match at `point`. Returns the rule and match, or
    /// None (and selects nothing) when no rule matches there.
    pub(super) fn smart_select(&mut self, id: PaneId, point: Point, ignore_newlines: bool) -> Option<smartsel::Found> {
        let rules = self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).map(smartsel::rules)?;
        let p = self.panes.get_mut(&id)?;
        let at = to_pos(&p.screen, point);
        let (lines, _) = find::logical_lines_around(&p.screen, at.row, RADIUS);
        let joined = find::Logical::join(&lines, if ignore_newlines { "" } else { "\n" });
        let click = joined.offset_of(at)?;
        let found = smartsel::select_at(&joined.text, click, &rules)?;
        let m = Match { start: joined.pos_before(found.range.start), end: joined.pos_before(found.range.end - 1) };
        reveal(p, &m);
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
        let (lines, i) = find::logical_lines_around(&p.screen, at.row, RADIUS);
        let line = lines.get(i)?.clone_line();
        let o = line.offset_of(at)?;
        let joined = Logical::join(&lines, "\n");
        let found = joined.offset_of(at).and_then(|click| smartsel::select_at(&joined.text, click, &rules));
        let smart = found.as_ref().filter(|f| !rules[f.rule].actions.is_empty()).map(|f| (f.rule, 0, joined.text[f.range.clone()].to_string()));
        // At most iTerm2's reach each way, cut on character boundaries.
        let mut base = o.saturating_sub(semantic::MAX_CONTEXT);
        while !line.text.is_char_boundary(base) {
            base += 1;
        }
        let mut end = (o + semantic::MAX_CONTEXT).min(line.text.len());
        while !line.text.is_char_boundary(end) {
            end -= 1;
        }
        let link = p.screen.term().grid()[point].hyperlink();
        let ctx = ClickContext {
            before: line.text[base..o].to_string(),
            after: line.text[o..end].to_string(),
            link: link.as_ref().map(|h| h.uri().to_string()),
            cwd: self.pane_dir(id),
            smart,
            raw_command: prefs.action == "raw command",
        };
        Some(Clicked { ctx, line, base, joined, found, rules, prefs, link_id: link.map(|h| h.id().to_string()) })
    }

    /// Whether a reported host name is this computer (`localhost`, its name or its DNS name; P17).
    pub(super) fn host_is_this_machine(&self, host: &str) -> bool {
        crate::statusbar_text::is_local_host(host, super::statusbar_content::local_names())
    }

    /// P10's one guard: text in this pane may have been written by a remote host, so it never
    /// becomes a local open, run or coprocess. True whenever a conductor frames the pane,
    /// whatever host it reports: the remote writes that report (OSC 1337 RemoteHost, OSC 7).
    pub(super) fn remote_text(&self, id: PaneId) -> bool {
        self.framing(id).is_some()
    }

    /// The pane's working directory from shell integration, when it is on this machine.
    pub(super) fn pane_dir(&self, id: PaneId) -> Option<std::path::PathBuf> {
        if self.remote_text(id) {
            return None;
        }
        let shell = &self.panes.get(&id)?.own().shell;
        if shell.host.as_ref().is_some_and(|h| !self.host_is_this_machine(h)) {
            return None;
        }
        crate::profile::local_path(shell.cwd.as_deref()?)
    }

    /// The Win-click target at a point and the cells it covers.
    pub(super) fn target_at(&self, id: PaneId, point: Point) -> Option<(Clicked, Target, Match)> {
        let c = self.clicked(id, point)?;
        let (target, range) = semantic::resolve_range(&c.ctx)?;
        let cells = match (&target, range) {
            (_, Some(r)) => Match { start: c.line.pos_before(c.base + r.start), end: c.line.pos_before(c.base + r.end - 1) },
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
    pub(super) fn open_target(&mut self, id: PaneId, point: Point, background: bool) -> bool {
        let found = self.target_at(id, point);
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
        let cwd = c.ctx.cwd.as_ref().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
        let run = match &target {
            Target::Action { rule, action, .. } => {
                let (Some(a), Some(f)) = (c.rules.get(*rule).and_then(|r| r.actions.get(*action)), c.found.as_ref()) else { return self.secure_copy_at(id, point) };
                self.smart_action_run(id, a, &f.captures, &cwd)
            }
            _ => {
                let around = Around { prefix: &c.ctx.before, suffix: &c.ctx.after, cwd: &cwd };
                semantic::plan(&target, &c.prefs, &around, has_code())
            }
        };
        match run {
            Some(run) => {
                self.execute(id, run, c.ctx.cwd.as_deref(), background);
                true
            }
            None => self.secure_copy_at(id, point),
        }
    }

    /// What a Smart Selection action runs. Copy runs here and returns None.
    fn smart_action_run(&mut self, id: PaneId, a: &smartsel::RuleAction, captures: &[String], cwd: &str) -> Option<Run> {
        let shell = &self.panes.get(&id)?.own().shell;
        let (host, user) = (shell.host.clone().unwrap_or_default(), shell.user.clone().unwrap_or_default());
        let text = |escape| smartsel::expand(&a.parameter, captures, cwd, &host, &user, escape);
        Some(match a.action.as_str() {
            "openFile:" | "openUrl:" => Run::Open(text(Escape::None)),
            "runCommand:" | "runCommandInWindow:" => Run::Command(text(Escape::Cmd)),
            "runCoprocess:" => Run::Coprocess(text(Escape::Cmd)),
            "sendText:" => Run::Send(text(Escape::None)),
            "copy:" => {
                let t = text(Escape::None);
                if let Some(cb) = &mut self.clipboard {
                    if let Err(e) = cb.set_text(t) {
                        self.banners.set("clipboard", format!("Copy failed: {e}"));
                    }
                }
                return None;
            }
            other => {
                self.banners.set("action", format!("Unknown Smart Selection action {other:?}."));
                return None;
            }
        })
    }

    pub(super) fn execute(&mut self, id: PaneId, run: Run, cwd: Option<&std::path::Path>, background: bool) {
        match run {
            Run::Open(target) => {
                if let Err(e) = shell_open(&target, cwd, background) {
                    self.banners.set("action", format!("Could not open {target}: {e}"));
                }
            }
            Run::Command(line) => {
                let mut cmd = std::process::Command::new("cmd.exe");
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    // CREATE_NO_WINDOW: the command's own windows still show.
                    cmd.raw_arg(format!("/c {line}")).creation_flags(0x0800_0000);
                }
                if let Some(d) = cwd.filter(|d| d.is_dir()) {
                    cmd.current_dir(d);
                }
                if let Err(e) = cmd.spawn() {
                    self.banners.set("action", format!("Could not run {line}: {e}"));
                }
            }
            Run::Send(text) => {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.scroll(Scroll::Bottom);
                    p.input(text.as_bytes());
                }
            }
            Run::Later(name, m) => self.banners.set("action", format!("{name} comes in milestone {m}.")),
            Run::Coprocess(command) => self.start_coprocess(id, &command, false, crate::coprocess::Entry::SemanticHistory),
            Run::Blocked(why) => self.banners.set("action", why),
            Run::Reveal(path) => self.reveal_in_explorer(&path),
        }
        self.redraw();
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
        let mut cmd = std::process::Command::new(program);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.raw_arg(format!("/select,{}", semantic::cmd_quote(path)));
        }
        if let Err(e) = cmd.spawn() {
            self.banners.set("action", format!("Could not show {path}: {e}"));
        }
    }

    /// Win held over a pane: underline the Win-click target under the pointer and show
    /// a hand. Called when the pointer moves and when the modifiers change.
    pub(super) fn update_hover(&mut self) {
        let (x, y) = self.cursor;
        let want = self.mods.cmd && !self.mods.ctrl && self.globals.bool("CommandSelection") && matches!(self.drag, Drag::None);
        let hover = want
            .then(|| self.pane_at(x, y))
            .flatten()
            .and_then(|(id, rect)| {
                let (col, row, _) = self.cell_at(id, rect, x, y);
                let point = self.grid_point(id, col, row);
                self.target_at(id, point).map(|(_, target, cells)| (id, cells, target))
            });
        // A link's text can differ from where it goes, so the target shows while hovering.
        match hover.as_ref().map(|(_, _, t)| t) {
            Some(Target::Link(u) | Target::Url(u)) => self.banners.set("hover", format!("Win-click opens {u}")),
            _ => self.banners.clear("hover"),
        }
        let hover = hover.map(|(id, cells, _)| (id, cells));
        if hover != self.hover {
            self.hover = hover;
            if let Some(g) = &self.gpu {
                g.window.set_cursor(if hover.is_some() { winit::window::CursorIcon::Pointer } else { winit::window::CursorIcon::Default });
            }
            self.redraw();
        }
    }

    /// The right-click menu, with the clicked Smart Selection match's actions and Copy
    /// Link Address on a link at the top.
    pub(super) fn show_context_menu_at(&mut self, at: Option<(PaneId, Point)>) {
        let mut extra: Vec<(String, CtxItem)> = Vec::new();
        let mut pending = None;
        let mut fold = None;
        if let Some((id, point)) = at {
            if let Some(f) = self.fold_at_point(id, point) {
                extra.push(("Unfold".into(), CtxItem::Unfold));
                fold = Some(f);
            }
            if let Some(c) = self.clicked(id, point) {
                // In a remote pane the actions that open or run here are greyed (P10).
                let remote = self.remote_text(id);
                if let Some(f) = c.found.as_ref() {
                    for (i, a) in c.rules[f.rule].actions.iter().enumerate() {
                        extra.push((a.title.clone(), CtxItem::Action(i, ssh_files::remote_action_allowed(remote, &a.action))));
                    }
                }
                if c.ctx.link.is_some() {
                    extra.push(("Copy Link Address".into(), CtxItem::CopyLink));
                } else if semantic::url_like(&c.ctx.before, &c.ctx.after).is_some() {
                    extra.push(("Copy URL".into(), CtxItem::CopyLink));
                }
                let link = c.ctx.link.clone().or_else(|| semantic::url_like(&c.ctx.before, &c.ctx.after));
                let cwd = c.ctx.cwd.as_ref().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
                let actions = c.found.as_ref().map(|f| (c.rules[f.rule].actions.clone(), f.captures.clone())).unwrap_or_default();
                pending = Some(CtxPending { pane: id, link, actions: actions.0, captures: actions.1, cwd, fold, providers: Vec::new() });
            } else if fold.is_some() {
                pending = Some(CtxPending { pane: id, link: None, actions: Vec::new(), captures: Vec::new(), cwd: String::new(), fold, providers: Vec::new() });
            }
        }
        // Stop Coprocess shows only while one runs (iTerm2's context menu).
        if let Some((id, _)) = at.filter(|(id, _)| self.panes.get(id).is_some_and(|p| p.coprocess_command().is_some())) {
            extra.push(("Stop Coprocess".into(), CtxItem::StopCoprocess));
            if pending.is_none() {
                pending = Some(CtxPending { pane: id, link: None, actions: Vec::new(), captures: Vec::new(), cwd: String::new(), fold: None, providers: Vec::new() });
            }
        }
        // Toggle Broadcasting Input, for the pane that was clicked (always offered).
        if let Some((id, _)) = at {
            extra.push(("Toggle Broadcasting Input".into(), CtxItem::ToggleBroadcast));
            if pending.is_none() {
                pending = Some(CtxPending { pane: id, link: None, actions: Vec::new(), captures: Vec::new(), cwd: String::new(), fold: None, providers: Vec::new() });
            }
        }
        // The script providers' items (`CMH:898-914`), for the clicked session.
        let mut providers: Vec<(String, CtxItem)> = Vec::new();
        if let Some(p) = pending.as_mut() {
            p.providers = crate::script_fn::context_rows(&self.api_fns.context_menu_items());
            providers = p.providers.iter().map(|(title, id)| (title.clone(), CtxItem::Provider(id.clone()))).collect();
        }
        self.ctx_pending = pending;
        self.show_context_menu_with(&extra, &providers);
    }

    /// A pick from the right-click menu's own items. Returns false for other ids.
    pub(super) fn context_picked(&mut self, id: &str) -> bool {
        let Some(item) = id.strip_prefix(CTX_ID) else { return false };
        let Some(pend) = self.ctx_pending.take() else { return true };
        if item == "stopcoprocess" {
            self.stop_coprocess(pend.pane);
        } else if item == "togglebroadcast" {
            self.toggle_broadcast_pane(pend.pane);
        } else if item == "unfold" {
            if let Some(f) = pend.fold {
                self.unfold(pend.pane, f);
            }
        } else if item == "copylink" {
            if let (Some(link), Some(cb)) = (pend.link, &mut self.clipboard) {
                if let Err(e) = cb.set_text(link) {
                    self.banners.set("clipboard", format!("Copy failed: {e}"));
                }
            }
        } else if let Some(unique_id) = item.strip_prefix("provider.") {
            let title = pend.providers.iter().find(|(_, id)| id == unique_id).map(|(t, _)| t.clone()).unwrap_or_default();
            self.context_provider_picked(pend.pane, unique_id, &title);
        } else if let Some(a) = item.strip_prefix("action.").and_then(|n| n.parse::<usize>().ok()).and_then(|n| pend.actions.get(n).cloned()) {
            let cwd_path = (!pend.cwd.is_empty()).then(|| std::path::PathBuf::from(&pend.cwd));
            // Asked again at the pick: the pane may have started framing since the menu showed.
            if !ssh_files::remote_action_allowed(self.remote_text(pend.pane), &a.action) {
                self.banners.set("action", format!("{} does not run for text from a remote host.", a.title));
            } else if let Some(run) = self.smart_action_run(pend.pane, &a, &pend.captures, &pend.cwd) {
                self.execute(pend.pane, run, cwd_path.as_deref(), false);
            }
        }
        self.redraw();
        true
    }

    /// Opens text picked from a label: an existing file (with its line) through Semantic
    /// History, else the text as a URL. In a remote pane only a web or mail link opens; other
    /// text may only download (P10).
    pub(super) fn open_text(&mut self, id: PaneId, text: &str) {
        if self.remote_text(id) {
            let link = semantic::url_like("", text);
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
        let target = match semantic::find_path("", text, cwd.as_deref()) {
            Some((path, line, column)) => Target::File { path, line, column },
            None => Target::Url(semantic::url_like("", text).unwrap_or_else(|| text.to_string())),
        };
        let cwd_s = cwd.as_ref().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
        let around = Around { prefix: "", suffix: text, cwd: &cwd_s };
        if let Some(run) = semantic::plan(&target, &prefs, &around, has_code()) {
            self.execute(id, run, cwd.as_deref(), false);
        }
    }

    /// Copy Link Address, the pointer action: the link or URL at a point.
    pub(super) fn copy_link_at(&mut self, id: PaneId, point: Point) {
        let Some(c) = self.clicked(id, point) else { return };
        let Some(link) = c.ctx.link.clone().or_else(|| semantic::url_like(&c.ctx.before, &c.ctx.after)) else { return };
        if let Some(cb) = &mut self.clipboard {
            if let Err(e) = cb.set_text(link) {
                self.banners.set("clipboard", format!("Copy failed: {e}"));
            }
        }
    }
}

/// Menu item ids of the right-click menu's own items.
pub(super) const CTX_ID: &str = "tandem.ctx.";

pub(super) enum CtxItem {
    /// A Smart Selection action by index, and whether it is enabled (not in a remote pane, P10).
    Action(usize, bool),
    CopyLink,
    Unfold,
    StopCoprocess,
    ToggleBroadcast,
    /// A script context menu provider's item, by its unique id.
    Provider(String),
}

impl CtxItem {
    pub(super) fn enabled(&self) -> bool {
        !matches!(self, CtxItem::Action(_, false))
    }

    pub(super) fn id(&self) -> String {
        match self {
            CtxItem::Action(i, _) => format!("{CTX_ID}action.{i}"),
            CtxItem::CopyLink => format!("{CTX_ID}copylink"),
            CtxItem::Unfold => format!("{CTX_ID}unfold"),
            CtxItem::StopCoprocess => format!("{CTX_ID}stopcoprocess"),
            CtxItem::ToggleBroadcast => format!("{CTX_ID}togglebroadcast"),
            CtxItem::Provider(id) => format!("{CTX_ID}provider.{id}"),
        }
    }
}

/// What the right-click menu's own items act on.
pub(super) struct CtxPending {
    pane: PaneId,
    link: Option<String>,
    actions: Vec<smartsel::RuleAction>,
    captures: Vec<String>,
    cwd: String,
    fold: Option<u32>,
    /// The script providers' items shown (title, unique id).
    providers: Vec<(String, String)>,
}

pub(super) struct Clicked {
    pub(super) ctx: ClickContext,
    /// The clicked logical line, and where `ctx.before` starts in it.
    line: Logical,
    base: usize,
    /// The lines Smart Selection looked at, and its match.
    pub(super) joined: Logical,
    pub(super) found: Option<smartsel::Found>,
    rules: Vec<smartsel::Rule>,
    prefs: Prefs,
    link_id: Option<String>,
}

/// VS Code is on the PATH, so the best editor can open a file at a line.
fn has_code() -> bool {
    static HAS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HAS.get_or_init(|| {
        std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).any(|d| ["code.cmd", "code.exe"].iter().any(|f| d.join(f).is_file()))
        })
    })
}

/// Opens a file, folder, or URL with its default app. `background` asks the app not to
/// take the focus.
#[cfg(windows)]
pub(super) fn shell_open(target: &str, cwd: Option<&std::path::Path>, background: bool) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, SW_SHOWNORMAL};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, file) = (wide("open"), wide(target));
    let dir = cwd.map(|d| wide(&d.to_string_lossy()));
    let show = if background { SW_SHOWNOACTIVATE } else { SW_SHOWNORMAL };
    let r = unsafe {
        ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()), show)
    };
    // Values of 32 or less are errors (SE_ERR_*).
    if r as isize > 32 { Ok(()) } else { Err(format!("ShellExecute error {}", r as isize)) }
}

#[cfg(not(windows))]
pub(super) fn shell_open(target: &str, _cwd: Option<&std::path::Path>, _background: bool) -> Result<(), String> {
    Err(format!("opening {target} needs Windows"))
}

#[cfg(test)]
mod tests {
    /// The functions that turn text in a pane into a local open, run or coprocess: each asks the
    /// one guard, `remote_text` (P10). Win-click and the Open Target pointer actions reach
    /// `open_target`; the right-click menu and its pointer action `show_context_menu_at` and
    /// `context_picked`; Open Selection and a label pick `open_text`; OSC 1337 OpenURL `open_url`.
    const GATED: &[(&str, &str)] = &[
        ("semantic_ui.rs", "open_target"),
        ("semantic_ui.rs", "show_context_menu_at"),
        ("semantic_ui.rs", "context_picked"),
        ("semantic_ui.rs", "open_text"),
        ("semantic_ui.rs", "pane_dir"),
        ("prompted_ui.rs", "open_url"),
    ];

    /// Calls that open or run something on this machine.
    const SINKS: &[&str] = &["self.execute(", "smart_action_run(", "semantic::plan(", "guard_open(", "shell_open(", "start_coprocess(", "reveal_in_explorer("];

    /// Callers of a sink whose input is not text a remote host wrote, and why.
    const NOT_PANE_TEXT: &[(&str, &str, &str)] = &[
        ("semantic_ui.rs", "execute", "the sink itself; its callers are checked"),
        ("semantic_ui.rs", "smart_action_run", "builds the Run; its callers are checked"),
        ("actions.rs", "run_key_action", "the user's own key binding"),
        ("ai_tools_app.rs", "ai_reveal_file", "reveals the AI's own new file, runs nothing"),
        ("api_ui.rs", "open_link", "a fixed More Info page"),
        ("builtin_ui.rs", "session_method", "a local script's request"),
        ("chat_window_app.rs", "chat_open_url", "a link in the AI chat window, not a pane"),
        ("chat_window_app.rs", "chat_file", "an AI chat attachment, inert types only (F19)"),
        ("coprocess_ui.rs", "coprocess_prompt_event", "the command the user typed"),
        ("coprocess_ui.rs", "flush_dropped", "files the user dropped"),
        ("host_offers_ui.rs", "host_offer_bar", "a fixed help page"),
        ("mod.rs", "window_event", "the Configure SSH help page"),
        ("porthole_ui.rs", "open_link_target", "a Porthole document link, asked per link"),
        ("snippets_ui.rs", "run_applied", "the user's own key binding"),
        ("tmux_ui.rs", "tmux_bar", "a fixed help page"),
        ("triggers_ui.rs", "trigger_request", "the user's own trigger rule"),
    ];

    /// Each `fn` in a source file with its body, up to the test module.
    fn functions(src: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for line in src.lines() {
            if line.trim_start().starts_with("#[cfg(test)]") {
                break;
            }
            let t = line.trim_start();
            let decl = ["pub(super) fn ", "pub(crate) fn ", "pub fn ", "fn "].iter().find_map(|k| t.strip_prefix(k));
            if let Some(rest) = decl {
                let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                out.push((name, String::new()));
            } else if let Some((_, body)) = out.last_mut() {
                if !t.starts_with("//") {
                    body.push_str(line);
                    body.push('\n');
                }
            }
        }
        out
    }

    #[test]
    fn every_entry_point_is_gated() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("app");
        let (mut seen, mut wrong) = (Vec::new(), Vec::new());
        let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "rs")).collect();
        files.sort();
        for path in files {
            let file = path.file_name().unwrap().to_string_lossy().to_string();
            let src = std::fs::read_to_string(&path).unwrap();
            for (name, body) in functions(&src) {
                let gated = GATED.contains(&(file.as_str(), name.as_str()));
                if gated {
                    if !body.contains("remote_text(") {
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
        for (f, n) in GATED {
            if !seen.iter().any(|(a, b)| a == f && b == n) {
                wrong.push(format!("{f} {n} is gone: update GATED"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
