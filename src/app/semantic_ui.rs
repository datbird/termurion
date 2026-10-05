//! Smart Selection and Semantic History in the window: what a click selects and what a
//! Win-click opens.

use alacritty_terminal::index::Point;

use super::*;
use super::find_ui::{reveal, to_pos};
use crate::find::{self, Logical, Match};
use crate::sh_context;
use crate::semantic::{self, Around, ClickContext, Prefs, Run, Target};
use crate::smartsel::{self, Escape};
use crate::ssh_files;

/// The cells to underline for a Win-hover target: none when the underline key is off (the hand
/// pointer still shows, `TVA:548-552`).
pub(super) fn hover_cells(target_cells: Match, underline: bool) -> Option<Match> {
    underline.then_some(target_cells)
}

/// The columns between soft boundaries (the vertical dividers apps draw) around a click, when
/// `restrict` is on and a divider stands in the click's row (`UAH:86`, `UAH:401-430`).
pub(super) fn window_for(rows: &[Vec<char>], row: usize, col: usize, width: usize, restrict: bool) -> Option<(usize, usize)> {
    if !restrict || width == 0 {
        return None;
    }
    let dividers = crate::seltools::divider_columns(rows, row);
    (!dividers.is_empty()).then(|| crate::seltools::clip_to_dividers(0, width - 1, col, &dividers))
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

/// The logical lines Smart Selection reads around `at` (`RADIUS` each way) and the index of the
/// one holding `at`, built from a bounded span of rows (`rows_each_way`) inside the column
/// window, never from whole logical lines: one huge wrapped line costs the same as a short one.
fn smart_lines(screen: &crate::screen::Screen, at: find::Pos, window: Option<(usize, usize)>, max: usize) -> (Vec<Logical>, usize) {
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
    let (lo, hi) = (clicked.saturating_sub(RADIUS), (clicked + RADIUS + 1).min(lines.len()));
    let kept: Vec<Logical> = lines.drain(lo..hi).collect();
    (kept, clicked - lo)
}

/// What `update_hover` compares to skip work: the pane, the cell and both hover keys.
pub(super) type HoverKey = (PaneId, i32, usize, bool, bool);

pub(super) fn hover_key(at: Option<(PaneId, Point)>, underline: bool, preview: bool) -> Option<HoverKey> {
    at.map(|(id, p)| (id, p.line.0, p.column.0, underline, preview))
}

/// Smart Selection looks this many logical lines each way from the click.
const RADIUS: usize = 2;

impl App {
    /// Selects the best Smart Selection match at `point`. Returns the rule and match, or
    /// None (and selects nothing) when no rule matches there.
    pub(super) fn smart_select(&mut self, id: PaneId, point: Point, ignore_newlines: bool) -> Option<smartsel::Found> {
        let rules = self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).map(smartsel::rules)?;
        let p = self.panes.get_mut(&id)?;
        let at = to_pos(&p.screen, point);
        let max = self.globals.int("MaxSemanticHistoryPrefixOrSuffix").max(0) as usize;
        let (lines, _) = smart_lines(&p.screen, at, None, max);
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
        let max = self.globals.int("MaxSemanticHistoryPrefixOrSuffix").max(0) as usize;
        // Detection stays between soft boundaries (`UAH:86`), Smart Selection included (P12),
        // over a bounded number of rows, never whole logical lines.
        let window = click_window(&p.screen, at, true);
        let (lines, i) = smart_lines(&p.screen, at, window, max);
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
                s.host.as_ref().is_none_or(|h| self.host_is_this_machine(h))
                    && s.cwd.as_deref().is_none_or(|c| crate::profile::local_path(c).is_some_and(|p| crate::netpath::may_probe(&p.to_string_lossy(), &probe)))
            });
        let chars = crate::url_guess::url(&self.globals.string("URLCharacterSet"), &self.globals.string("URLCharacterSetExclusions"));
        crate::url_guess::UrlPrefs {
            chars,
            trailing: self.globals.string("TrailingPunctuationMarks"),
            conservative: self.globals.bool("ConservativeURLGuessing"),
            // 13s's RequireSlashInURLGuess (default true); until its row exists the default holds.
            require_slash: self.globals.get("RequireSlashInURLGuess").and_then(|v| v.as_bool()).unwrap_or(true),
            default_scheme: self.globals.string("DefaultURLScheme"),
            cwd_is_local,
        }
    }

    /// What Semantic History may probe: `PathsToIgnore` and `EnableSemanticHistoryOnNetworkMounts`
    /// with the real drive types (`NFM:17-70`).
    pub(super) fn path_probe(&self) -> crate::netpath::Probe {
        crate::netpath::Probe::from_settings(&self.globals.string("PathsToIgnore"), self.globals.bool("EnableSemanticHistoryOnNetworkMounts"), crate::netpath::real_drives())
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
        // A bound ssh: link opens a Tandem tab, never a local run, and never from a remote pane (P4).
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
                self.execute(id, run, c.ctx.cwd.as_deref(), background);
                true
            }
            None => self.secure_copy_at(id, point),
        }
    }

    /// What a Smart Selection action runs. Copy runs here and returns None.
    fn smart_action_run(&mut self, id: PaneId, a: &smartsel::RuleAction, captures: &[String], cwd: &str) -> Option<Run> {
        let (host, user) = self.host_user(id)?;
        let text = |escape| smartsel::expand(&a.parameter, captures, cwd, &host, &user, escape);
        Some(match a.action.as_str() {
            "openFile:" => semantic::open_file_run(&text(Escape::None), (!cwd.is_empty()).then(|| std::path::Path::new(cwd)), has_code()),
            "openUrl:" => semantic::open_url_run(&text(Escape::None), has_code()),
            "runCommand:" | "runCommandInWindow:" | "runCoprocess:" if smartsel::cmd_unsafe_values(&a.parameter, captures, cwd, &host, &user) => {
                Run::Blocked("Tandem did not run this command: a value in it holds % or a quote that cmd.exe would read.".into())
            }
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
                    cmd.raw_arg(semantic::cmd_args(&line)).creation_flags(0x0800_0000);
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
            Run::Spawn { program, args } => self.spawn_direct(&program, &args, cwd),
        }
        self.redraw();
    }

    /// Starts a program directly (one `CreateProcessW`, no shell). A name without a folder is
    /// found on the PATH; a batch file is refused any argument cmd.exe would read as syntax.
    fn spawn_direct(&mut self, program: &std::path::Path, args: &[String], cwd: Option<&std::path::Path>) {
        let found = semantic::resolve_program(program, std::env::var_os("PATH").as_deref());
        let Some(exe) = found else {
            self.banners.set("action", format!("Could not start {}: not found.", program.display()));
            return;
        };
        if semantic::is_batch(&exe) && args.iter().any(|a| semantic::batch_unsafe(a)) {
            self.banners.set("action", "Tandem did not start this program: a batch file would read special characters in the file name.");
            return;
        }
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        if let Some(d) = cwd.filter(|d| !crate::scripts::is_network_path(&d.to_string_lossy()) && d.is_dir()) {
            cmd.current_dir(d);
        }
        if let Err(e) = cmd.spawn() {
            self.banners.set("action", format!("Could not start {}: {e}", exe.display()));
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
        self.smart_param(id, a, &f.captures, &cwd, Escape::None)
    }

    /// A Smart Selection action's parameter with its captures, the working directory and the
    /// pane's host and user filled in: the one expansion the hover preview and the run share.
    fn smart_param(&self, id: PaneId, a: &smartsel::RuleAction, captures: &[String], cwd: &str, escape: Escape) -> Option<String> {
        let (host, user) = self.host_user(id)?;
        Some(smartsel::expand(&a.parameter, captures, cwd, &host, &user, escape))
    }

    /// The pane's reported host and user, empty when it reports none.
    fn host_user(&self, id: PaneId) -> Option<(String, String)> {
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
        let want = self.mods.cmd && !self.mods.ctrl && self.globals.bool("CommandSelection") && matches!(self.drag, Drag::None);
        let at = want.then(|| self.pane_at(x, y)).flatten().map(|(id, rect)| {
            let (col, row, _) = self.cell_at(id, rect, x, y);
            (id, self.grid_point(id, col, row))
        });
        let key = hover_key(at, underline, preview);
        if key != self.hover_key {
            self.hover_key = key;
            let found = at.and_then(|(id, point)| self.target_at(id, point).map(|(c, target, cells)| (id, c, cells, target)));
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

    /// The pane menu for `at`: the pane and, for a right click, the point (Smart Selection
    /// matches' actions, Copy Link Address and Unfold come from the point). The title bar passes
    /// its pane and no point (`titleBarMenu` is `menuAtCoord:(-1,-1)`, `CMH:615-616`).
    pub(super) fn show_context_menu_at(&mut self, at: Option<(PaneId, Option<Point>)>) {
        let mut extra: Vec<(String, CtxItem)> = Vec::new();
        let mut pending = None;
        let mut fold = None;
        if let Some((id, Some(point))) = at {
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
                } else if semantic::url_like_in(&c.ctx.before, &c.ctx.after, &c.ctx.urls).is_some() {
                    extra.push(("Copy URL".into(), CtxItem::CopyLink));
                }
                let link = c.ctx.link.clone().or_else(|| semantic::url_like_in(&c.ctx.before, &c.ctx.after, &c.ctx.urls));
                let cwd = c.ctx.cwd.as_ref().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
                let actions = c.found.as_ref().map(|f| (c.rules[f.rule].actions.clone(), f.captures.clone())).unwrap_or_default();
                pending = Some(CtxPending { pane: id, link, actions: actions.0, captures: actions.1, cwd, fold, providers: Vec::new() });
            } else if fold.is_some() {
                pending = Some(CtxPending { fold, ..CtxPending::for_pane(id, Vec::new()) });
            }
        }
        // Stop Coprocess (only while one runs) and Toggle Broadcasting Input, for the pane the
        // menu is for. The pending item exists for the pane even with no other item, so the
        // providers fill (P16).
        if let Some(id) = menu_pane(at) {
            let running = self.panes.get(&id).is_some_and(|p| p.coprocess_command().is_some());
            for label in ctx_extras(running) {
                extra.push((label.into(), if label == "Stop Coprocess" { CtxItem::StopCoprocess } else { CtxItem::ToggleBroadcast }));
            }
            // Lock Pane and the tab-wide items follow Toggle Broadcasting Input (`CMH:671-713`).
            for (label, item) in self.lock_items_for(id) {
                extra.push((label.into(), item));
            }
            pending.get_or_insert_with(|| CtxPending::for_pane(id, Vec::new()));
        }
        // The script providers' items (`CMH:898-914`), for that session.
        let mut providers: Vec<(String, CtxItem)> = Vec::new();
        if let Some(p) = pending.as_mut() {
            p.providers = crate::script_fn::context_rows(&self.api_fns.context_menu_items());
            providers = p.providers.iter().map(|(title, id)| (title.clone(), CtxItem::Provider(id.clone()))).collect();
        }
        self.ctx_pending = pending;
        let many = self.tabs.get(self.active).is_some_and(|t| t.layout.panes().len() > 1);
        self.show_context_menu_with(&extra, &providers, many);
    }

    /// A pick from the right-click menu's own items. Returns false for other ids.
    pub(super) fn context_picked(&mut self, id: &str, event_loop: &ActiveEventLoop) -> bool {
        let Some(item) = id.strip_prefix(CTX_ID) else { return false };
        let Some(pend) = self.ctx_pending.take() else { return true };
        let focused = self.tabs.get(self.active).map(|t| t.focused);
        let in_tab = self.tabs.get(self.active).is_some_and(|t| t.layout.panes().contains(&pend.pane));
        match ctx_pick(id, pend.pane, focused, in_tab) {
            CtxPick::Gone => {}
            CtxPick::Swap(pane) => self.start_pane_move(pane, false, event_loop),
            CtxPick::Provider { pane, unique_id } => {
                let title = pend.providers.iter().find(|(_, id)| *id == unique_id).map(|(t, _)| t.clone()).unwrap_or_default();
                self.context_provider_picked(pane, &unique_id, &title);
            }
            CtxPick::Command { id, focus, restore } => {
                // The command acts on the menu's pane: it takes the focus for the command, and
                // the pane that had it gets it back (iTerm2 never moves it, `CMH:542-547`).
                if let (Some(pane), Some(t)) = (focus, self.tabs.get_mut(self.active)) {
                    t.focused = pane;
                    self.refresh_route();
                }
                if let Some(item) = crate::menus::by_id(&id) {
                    self.run_command(item, event_loop);
                }
                let back = self.tabs.get(self.active).and_then(|t| restore_focus(restore, pend.pane, t.layout.panes().contains(&pend.pane), t.focused, t.zoomed.is_some(), |p| t.layout.panes().contains(&p)));
                if let (Some(pane), Some(t)) = (back, self.tabs.get_mut(self.active)) {
                    t.focused = pane;
                    self.refresh_route();
                }
            }
            CtxPick::Other => self.context_other_picked(item, pend),
        }
        self.redraw();
        true
    }

    fn context_other_picked(&mut self, item: &str, pend: CtxPending) {
        if item == "stopcoprocess" {
            self.stop_coprocess(pend.pane);
        } else if item == "togglebroadcast" {
            self.toggle_broadcast_pane(pend.pane);
        } else if item == "lockpane" {
            self.toggle_pane_lock(pend.pane);
        } else if item == "lockall" || item == "unlockall" {
            self.lock_all_in_tab(pend.pane, item == "lockall");
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
        } else if let Some(a) = item.strip_prefix("action.").and_then(|n| n.parse::<usize>().ok()).and_then(|n| pend.actions.get(n).cloned()) {
            let cwd_path = (!pend.cwd.is_empty()).then(|| std::path::PathBuf::from(&pend.cwd));
            // Asked again at the pick: the pane may have started framing since the menu showed.
            if !ssh_files::remote_action_allowed(self.remote_text(pend.pane), &a.action) {
                self.banners.set("action", format!("{} does not run for text from a remote host.", a.title));
            } else if let Some(run) = self.smart_action_run(pend.pane, &a, &pend.captures, &pend.cwd) {
                self.execute(pend.pane, run, cwd_path.as_deref(), false);
            }
        }
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

    /// Copy Link Address, the pointer action: the link or URL at a point.
    pub(super) fn copy_link_at(&mut self, id: PaneId, point: Point) {
        let Some(c) = self.clicked(id, point) else { return };
        let Some(link) = c.ctx.link.clone().or_else(|| semantic::url_like_in(&c.ctx.before, &c.ctx.after, &c.ctx.urls)) else { return };
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
    /// Swap With Session...: the pick mode for a swap, from the pane the menu is for.
    SwapPane,
    /// Lock Pane (checked while the pane is locked), Lock All and Unlock All Panes in Tab.
    LockPane(bool),
    LockAll,
    UnlockAll,
    /// A script context menu provider's item, by its unique id.
    Provider(String),
    /// A menu command (by menu item id, and whether it is enabled) that acts on the menu's pane.
    Command(String, bool),
}

impl CtxItem {
    pub(super) fn enabled(&self) -> bool {
        !matches!(self, CtxItem::Action(_, false) | CtxItem::Command(_, false))
    }

    pub(super) fn id(&self) -> String {
        match self {
            CtxItem::Action(i, _) => format!("{CTX_ID}action.{i}"),
            CtxItem::CopyLink => format!("{CTX_ID}copylink"),
            CtxItem::Unfold => format!("{CTX_ID}unfold"),
            CtxItem::StopCoprocess => format!("{CTX_ID}stopcoprocess"),
            CtxItem::ToggleBroadcast => format!("{CTX_ID}togglebroadcast"),
            CtxItem::SwapPane => format!("{CTX_ID}swappane"),
            CtxItem::LockPane(_) => format!("{CTX_ID}lockpane"),
            CtxItem::LockAll => format!("{CTX_ID}lockall"),
            CtxItem::UnlockAll => format!("{CTX_ID}unlockall"),
            CtxItem::Provider(id) => format!("{CTX_ID}provider.{id}"),
            CtxItem::Command(id, _) => format!("{CTX_ID}cmd.{id}"),
        }
    }
}

/// The fixed items of the pane menu, by label: Stop Coprocess while one runs, then Toggle
/// Broadcasting Input. They need only the pane, never a click point.
pub(super) fn ctx_extras(running_coprocess: bool) -> Vec<&'static str> {
    let mut v = Vec::new();
    if running_coprocess {
        v.push("Stop Coprocess");
    }
    v.push("Toggle Broadcasting Input");
    v
}

/// What a pick from the pane menu does, decided from the menu's pane (`CtxPending::pane`) and
/// the focused pane, so nothing acts on the focused pane when the menu is for another.
#[derive(Debug, PartialEq)]
pub(super) enum CtxPick {
    Swap(PaneId),
    Provider { pane: PaneId, unique_id: String },
    /// A menu command: the pane to focus first (None when it already is) and the pane that had
    /// the focus, which gets it back after the command (`restore_focus`).
    Command { id: String, focus: Option<PaneId>, restore: Option<PaneId> },
    /// The menu's pane is gone: the pick does nothing, never falls back to the focused pane.
    Gone,
    Other,
}

/// The pane that takes the focus back after a menu command ran on the menu's pane, as a
/// clicked-pane pointer action leaves it (`run_pointer_action`): the command did not move the
/// focus off the menu's pane, the tab is not maximized on it, and the old pane is still there.
/// A command that took the menu's pane out of the tab (Move Session to Window) leaves the tab's
/// focus on its first pane; the old pane gets it back too.
pub(super) fn restore_focus(restore: Option<PaneId>, menu_pane: PaneId, menu_in_tab: bool, now_focused: PaneId, zoomed: bool, present: impl Fn(PaneId) -> bool) -> Option<PaneId> {
    let prev = restore?;
    ((now_focused == menu_pane || !menu_in_tab) && !zoomed && present(prev)).then_some(prev)
}

pub(super) fn ctx_pick(item: &str, pend_pane: PaneId, focused: Option<PaneId>, in_tab: bool) -> CtxPick {
    let Some(item) = item.strip_prefix(CTX_ID) else { return CtxPick::Other };
    if !in_tab && (item == "swappane" || item.starts_with("cmd.")) {
        CtxPick::Gone
    } else if item == "swappane" {
        CtxPick::Swap(pend_pane)
    } else if let Some(unique_id) = item.strip_prefix("provider.") {
        CtxPick::Provider { pane: pend_pane, unique_id: unique_id.to_string() }
    } else if let Some(id) = item.strip_prefix("cmd.") {
        let moves = focused != Some(pend_pane);
        CtxPick::Command { id: id.to_string(), focus: moves.then_some(pend_pane), restore: focused.filter(|_| moves) }
    } else {
        CtxPick::Other
    }
}

/// The pane a pane menu is for.
fn menu_pane(at: Option<(PaneId, Option<Point>)>) -> Option<PaneId> {
    at.map(|(id, _)| id)
}

/// The label of the menu's Swap slot (`CMH:605`, "Swap With Session...").
pub(super) fn swap_label() -> &'static str {
    "Swap With Session\u{2026}"
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

impl CtxPending {
    /// A pending menu for `pane` with no link, fold or Smart Selection action.
    fn for_pane(pane: PaneId, providers: Vec<(String, String)>) -> Self {
        CtxPending { pane, link: None, actions: Vec::new(), captures: Vec::new(), cwd: String::new(), fold: None, providers }
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
mod title_menu_tests {
    use super::*;

    #[test]
    fn picks_act_on_the_menus_pane() {
        let (a, b): (PaneId, PaneId) = (1, 2);
        // Menu for B while A is focused: a provider pick and a Swap reach B only.
        assert_eq!(ctx_pick("tandem.ctx.provider.u1", b, Some(a), true), CtxPick::Provider { pane: b, unique_id: "u1".into() });
        assert_eq!(ctx_pick("tandem.ctx.swappane", b, Some(a), true), CtxPick::Swap(b));
        // Every plain command of the menu (the move group among them) focuses B first.
        for id in ["Session > Move Session > Move Session to Split Pane", "Session > Move Session > Move Session to Tab", "Session > Move Session > Move Session to Window", "Edit > Copy"] {
            let item = format!("tandem.ctx.cmd.{id}");
            assert_eq!(ctx_pick(&item, b, Some(a), true), CtxPick::Command { id: id.into(), focus: Some(b), restore: Some(a) }, "{id}");
            // Already focused: nothing to move, nothing to give back.
            assert_eq!(ctx_pick(&item, b, Some(b), true), CtxPick::Command { id: id.into(), focus: None, restore: None });
        }
        assert_eq!(ctx_pick("tandem.ctx.stopcoprocess", b, Some(a), true), CtxPick::Other);
        assert_eq!(CtxItem::Command("Edit > Copy".into(), true).id(), "tandem.ctx.cmd.Edit > Copy");
    }

    #[test]
    fn a_gone_menu_pane_does_nothing() {
        let (a, b): (PaneId, PaneId) = (1, 2);
        // The menu's pane closed before the pick arrived: no command runs on the focused pane.
        assert_eq!(ctx_pick("tandem.ctx.cmd.Shell > Close", b, Some(a), false), CtxPick::Gone);
        assert_eq!(ctx_pick("tandem.ctx.cmd.Edit > Paste", b, Some(a), false), CtxPick::Gone);
        assert_eq!(ctx_pick("tandem.ctx.swappane", b, Some(a), false), CtxPick::Gone);
        // A provider still resolves by id (P16).
        assert_eq!(ctx_pick("tandem.ctx.provider.u1", b, Some(a), false), CtxPick::Provider { pane: b, unique_id: "u1".into() });
    }

    #[test]
    fn commands_leave_the_focus_where_it_was() {
        let present = |p: PaneId| p == 1 || p == 2 || p == 3;
        // Copy or Paste on B with A focused: the command reached B, then A has the focus again.
        assert_eq!(restore_focus(Some(1), 2, true, 2, false, present), Some(1));
        // A command that moved the focus on (Split focuses the new pane) keeps it there.
        assert_eq!(restore_focus(Some(1), 2, true, 3, false, present), None);
        // Maximize on B keeps B focused: it is the one showing.
        assert_eq!(restore_focus(Some(1), 2, true, 2, true, present), None);
        // The old pane is gone (Close of A?): nothing to give back.
        assert_eq!(restore_focus(Some(9), 2, true, 2, false, present), None);
        // Move Session to Window on B left the tab: its focus fell to the first pane (3), A gets it back.
        assert_eq!(restore_focus(Some(1), 2, false, 3, false, present), Some(1));
        // The menu's pane was already focused.
        assert_eq!(restore_focus(None, 2, true, 2, false, present), None);
    }

    #[test]
    fn title_menu_is_the_pane_menu() {
        // No point: no link, fold or Smart Selection item; Stop Coprocess only while one runs.
        assert_eq!(ctx_extras(false), vec!["Toggle Broadcasting Input"]);
        assert_eq!(ctx_extras(true), vec!["Stop Coprocess", "Toggle Broadcasting Input"]);
        // Swap With Session... is the menu's own slot.
        assert_eq!(swap_label(), "Swap With Session\u{2026}");
        assert_eq!(CtxItem::SwapPane.id(), "tandem.ctx.swappane");
        assert!(CtxItem::SwapPane.enabled());
        // The pending item is built for the menu's pane even with no other item, so the
        // providers fill (P16, `m11c-rulings.txt:161`); which pane a pick reaches is
        // `picks_act_on_the_menus_pane`.
        let pend = CtxPending::for_pane(2, vec![("Item".into(), "u1".into())]);
        assert!(pend.link.is_none() && pend.fold.is_none() && pend.actions.is_empty());
        assert_eq!(pend.providers, vec![("Item".to_string(), "u1".to_string())]);
    }
}

#[cfg(test)]
mod tests {
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
        ("app/semantic_ui.rs", "show_context_menu_at"),
        ("app/semantic_ui.rs", "context_other_picked"),
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

    /// Calls that open or run something on this machine: Tandem's own sinks and every direct
    /// process start (`Command::new`, `ShellExecuteW`, `ShellExecuteExW`, `CreateProcessW`).
    const SINKS: &[&str] = &[
        "self.execute(",
        "smart_action_run(",
        "semantic::plan(",
        "guard_open(",
        "shell_open(",
        "start_coprocess(",
        "reveal_in_explorer(",
        "Command::new(",
        "ShellExecuteW(",
        "ShellExecuteExW(",
        "CreateProcessW(",
    ];

    /// Callers of a sink whose input is not text a remote host wrote, and why.
    const NOT_PANE_TEXT: &[(&str, &str, &str)] = &[
        ("app/semantic_ui.rs", "execute", "the sink itself; its callers are checked"),
        ("app/semantic_ui.rs", "smart_action_run", "builds the Run; its callers are checked"),
        ("app/semantic_ui.rs", "shell_open", "the sink itself; its callers are checked"),
        ("app/semantic_ui.rs", "reveal_in_explorer", "the sink itself; its callers are checked"),
        ("app/semantic_ui.rs", "spawn_direct", "the sink itself; execute's callers are checked"),
        ("semantic.rs", "plan", "builds a Run and opens nothing; its callers are checked"),
        ("semantic.rs", "open_url_run", "builds a Run and opens nothing; smart_action_run's callers are checked"),
        ("coproc_win.rs", "spawn", "the coprocess start; start_coprocess's callers are checked"),
        ("app/actions.rs", "run_key_action", "the user's own key binding"),
        ("app/ai_tools_app.rs", "ai_reveal_file", "reveals the AI's own new file, runs nothing"),
        ("ai_actions.rs", "get_help", "PowerShell Get-Help on a name man_page_plan checked, runs no command"),
        ("app/api_ui.rs", "open_link", "a fixed More Info page"),
        ("app/claude_code_app.rs", "cc_onboarding_event", "the fixed Learn more page (claude_code::HELP_URL)"),
        ("app/api_ui.rs", "reveal_allow_all_file", "reveals Tandem's own allow-all file"),
        ("app/builtin_ui.rs", "session_method", "a local script's request"),
        ("app/chat_window_app.rs", "chat_open_url", "a link in the AI chat window, not a pane"),
        ("app/chat_window_app.rs", "chat_file", "an AI chat attachment, inert types only (F19)"),
        ("app/coprocess_ui.rs", "coprocess_prompt_event", "the command the user typed"),
        ("app/coprocess_ui.rs", "coprocess_bar", "Notepad on Tandem's own coprocess error file"),
        ("app/coprocess_ui.rs", "flush_dropped", "files the user dropped"),
        ("app/host_offers_ui.rs", "host_offer_bar", "a fixed help page"),
        ("app/menu.rs", "run_command", "the Help menu's fixed documentation page"),
        ("app/mod.rs", "window_event", "the Configure SSH help page"),
        ("app/prefs_ui.rs", "curl_fetch", "the settings URL the user typed in Settings"),
        ("app/screenshot_ui.rs", "shot_event", "reveals the screenshot Tandem just saved"),
        ("app/scripts_app.rs", "edit_new_script", "the user's new script, edited, never run"),
        ("app/scripts_app.rs", "reveal_path", "a script in the user's Scripts folder, revealed"),
        ("app/settings_export_ui.rs", "start_relaunched", "starts Tandem's own executable"),
        ("app/snippets_ui.rs", "run_applied", "the user's own key binding"),
        ("app/ssh_panel_app.rs", "reveal_all", "the Download Files batch folder Tandem made (P12)"),
        ("app/tmux_ui.rs", "tmux_bar", "a fixed help page"),
        ("app/triggers_ui.rs", "trigger_request", "the user's own trigger rule (ruling)"),
        ("app/triggers_ui.rs", "start_command", "Run Command for the user's own trigger rule (ruling)"),
        ("bin/tandem-git.rs", "run", "the git menu's wrapper: git with the menu's own command"),
        ("git_run_win.rs", "start", "git for the status bar, Tandem's own arguments"),
        ("host_client.rs", "start_host_process", "starts Tandem's own session host"),
        ("probe.rs", "wsl", "`wsl.exe` with Tandem's own arguments"),
        ("pty.rs", "start", "a pane's shell, from its profile"),
        ("pty.rs", "start_pipes", "a pane's shell, from its profile"),
        ("pwm_run.rs", "run", "the password manager CLI the user set up"),
        ("script_proc.rs", "start", "a local Python API script"),
        ("script_provision.rs", "run_watched", "the scripts' Python setup steps"),
        ("util_console.rs", "run_command", "tandem-utils' program runner, run by the user at a prompt"),
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
            } else if let Some((_, body)) = out.last_mut() {
                if !t.starts_with("//") {
                    body.push_str(line);
                    body.push('\n');
                }
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

    #[test]
    fn every_entry_point_is_gated() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let (mut seen, mut wrong) = (Vec::new(), Vec::new());
        let mut files = Vec::new();
        sources(&src, &src, &mut files);
        files.sort();
        for (file, path) in files {
            let text = std::fs::read_to_string(&path).unwrap();
            for (name, body) in functions(&text) {
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
        let (lines, i) = smart_lines(&s, at, None, 2000);
        let bytes: usize = lines.iter().map(|l| l.text.len()).sum();
        // 27 rows each way at 80 columns, not the 500 rows of the line.
        assert!(bytes <= 55 * 80, "{bytes}");
        assert!(lines[i].offset_of(at).is_some(), "the clicked cell is still in its line");
        // A window cuts the columns too, and the bound holds on the whole buffer's far end.
        let (win, _) = smart_lines(&s, at, Some((0, 9)), 2000);
        assert!(win.iter().map(|l| l.text.len()).sum::<usize>() <= 55 * 10);
        let (top, i) = smart_lines(&s, find::Pos { row: 0, col: 0 }, None, 2000);
        assert!(top[i].offset_of(find::Pos { row: 0, col: 0 }).is_some());
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
        let (lines, i) = smart_lines(&s, at, None, 2000);
        assert_eq!(lines[i].text, "two https://a.b/c");
        assert_eq!(lines[i - 1].text, "one");
        assert_eq!(lines[i + 1].text, "three");
    }
}
