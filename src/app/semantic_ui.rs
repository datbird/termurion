//! Smart Selection and Semantic History in the window: what a click selects and what a
//! Win-click opens.

use alacritty_terminal::index::Point;

use super::*;
use super::find_ui::{reveal, to_pos};
use crate::find::{self, Logical, Match};
use crate::semantic::{self, Around, ClickContext, Prefs, Run, Target};
use crate::smartsel::{self, Escape};

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
    fn clicked(&self, id: PaneId, point: Point) -> Option<Clicked> {
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

    /// The pane's working directory from shell integration, when it is on this machine.
    pub(super) fn pane_dir(&self, id: PaneId) -> Option<std::path::PathBuf> {
        let shell = &self.panes.get(&id)?.own().shell;
        let here = std::env::var("COMPUTERNAME").unwrap_or_default();
        if shell.host.as_ref().is_some_and(|h| !h.eq_ignore_ascii_case(&here) && !h.eq_ignore_ascii_case("localhost")) {
            return None;
        }
        crate::profile::local_path(shell.cwd.as_deref()?)
    }

    /// The Win-click target at a point and the cells it covers.
    fn target_at(&self, id: PaneId, point: Point) -> Option<(Clicked, Target, Match)> {
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
        let Some((c, target, _)) = self.target_at(id, point) else { return false };
        let cwd = c.ctx.cwd.as_ref().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
        let run = match &target {
            Target::Action { rule, action, .. } => {
                let (Some(a), Some(f)) = (c.rules.get(*rule).and_then(|r| r.actions.get(*action)), c.found.as_ref()) else { return false };
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
            None => false,
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
            "runCoprocess:" => Run::Later("Run Coprocess", 6),
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
            Run::Blocked(why) => self.banners.set("action", why),
            Run::Reveal(path) => {
                // Explorer shows the file selected; nothing runs it.
                let mut cmd = std::process::Command::new("explorer.exe");
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.raw_arg(format!("/select,{}", semantic::cmd_quote(&path)));
                }
                if let Err(e) = cmd.spawn() {
                    self.banners.set("action", format!("Could not show {path}: {e}"));
                }
            }
        }
        self.redraw();
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
                if let Some(f) = c.found.as_ref() {
                    for (i, a) in c.rules[f.rule].actions.iter().enumerate() {
                        extra.push((a.title.clone(), CtxItem::Action(i)));
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
                pending = Some(CtxPending { pane: id, link, actions: actions.0, captures: actions.1, cwd, fold });
            } else if fold.is_some() {
                pending = Some(CtxPending { pane: id, link: None, actions: Vec::new(), captures: Vec::new(), cwd: String::new(), fold });
            }
        }
        self.ctx_pending = pending;
        self.show_context_menu_with(&extra);
    }

    /// A pick from the right-click menu's own items. Returns false for other ids.
    pub(super) fn context_picked(&mut self, id: &str) -> bool {
        let Some(item) = id.strip_prefix(CTX_ID) else { return false };
        let Some(pend) = self.ctx_pending.take() else { return true };
        if item == "unfold" {
            if let Some(f) = pend.fold {
                self.unfold(pend.pane, f);
            }
        } else if item == "copylink" {
            if let (Some(link), Some(cb)) = (pend.link, &mut self.clipboard) {
                if let Err(e) = cb.set_text(link) {
                    self.banners.set("clipboard", format!("Copy failed: {e}"));
                }
            }
        } else if let Some(a) = item.strip_prefix("action.").and_then(|n| n.parse::<usize>().ok()).and_then(|n| pend.actions.get(n).cloned()) {
            let cwd_path = (!pend.cwd.is_empty()).then(|| std::path::PathBuf::from(&pend.cwd));
            if let Some(run) = self.smart_action_run(pend.pane, &a, &pend.captures, &pend.cwd) {
                self.execute(pend.pane, run, cwd_path.as_deref(), false);
            }
        }
        self.redraw();
        true
    }

    /// Opens text picked from a label: an existing file (with its line) through Semantic
    /// History, else the text as a URL.
    pub(super) fn open_text(&mut self, id: PaneId, text: &str) {
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
    Action(usize),
    CopyLink,
    Unfold,
}

impl CtxItem {
    pub(super) fn id(&self) -> String {
        match self {
            CtxItem::Action(i) => format!("{CTX_ID}action.{i}"),
            CtxItem::CopyLink => format!("{CTX_ID}copylink"),
            CtxItem::Unfold => format!("{CTX_ID}unfold"),
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
}

struct Clicked {
    ctx: ClickContext,
    /// The clicked logical line, and where `ctx.before` starts in it.
    line: Logical,
    base: usize,
    /// The lines Smart Selection looked at, and its match.
    joined: Logical,
    found: Option<smartsel::Found>,
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
