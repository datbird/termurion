//! The terminal's right-click menu in the window: the facts of the pane and the click that
//! `ctx_menu::build` takes, the popup, and what each pick does. The items and their order are
//! `ctx_menu`'s; every feature behind an item is the one the main menu, a button or a click
//! already has.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Line, Point};

use super::files_ui::shown_name;
use super::find_ui::{to_point, to_pos};
use super::menu::info_box;
use super::semantic_ui::{has_code, Exec};
use super::windows_ui::NewWindow;
use super::*;
use crate::ctx_menu::{self, Ctx, Fold, HelperRow, ImageFacts, Link, CTX_ID};
use crate::{semantic, smartsel, ssh_files};

/// What the right-click menu's own items act on.
pub(super) struct CtxPending {
    pub(super) pane: PaneId,
    link: Option<String>,
    /// The Smart Selection actions shown, each with the captures of the match that offers it.
    actions: Vec<(smartsel::RuleAction, Vec<String>)>,
    cwd: String,
    fold: Option<u32>,
    /// The command mark on the clicked line (Command Info), and the one Fold folds.
    mark: Option<u32>,
    foldable: Option<u32>,
    /// The script providers' items shown (title, unique id).
    providers: Vec<(String, String)>,
    /// Quick Look Link's URL and the pointer (screen pixels) when the menu opened.
    quick_look: Option<(String, (f64, f64))>,
    /// The short selection the menu was made for (the URL items act on it).
    selection: Option<String>,
    /// The whole selection (Send Selection, Save Selection as Snippet, Send Email).
    full: Option<String>,
    /// The selection helpers shown, in the menu's numbering.
    helpers: Vec<crate::ctx_help::Helper>,
    /// The whole selection as a search text (`browser::search_query`).
    search: Option<String>,
    /// The text Download acts on.
    scp: Option<String>,
    /// The inline image the menu is for.
    image: Option<u32>,
    point: Option<Point>,
    /// The time the timestamp margin menu was opened on, in seconds.
    stamp: Option<f64>,
    /// Text the one Copy item copies (the workgroup toolbar's branch).
    copy: Option<String>,
}

impl CtxPending {
    /// The workgroup toolbar's branch menu: the text its Copy copies.
    pub(super) fn set_copy(&mut self, text: String) {
        self.copy = Some(text);
    }

    /// A timestamp margin menu: the time it was opened on.
    pub(super) fn set_stamp(&mut self, t: f64) {
        self.stamp = Some(t);
    }

    /// A pending menu for `pane` with nothing else.
    pub(super) fn for_pane(pane: PaneId, providers: Vec<(String, String)>) -> Self {
        CtxPending { pane, link: None, actions: Vec::new(), cwd: String::new(), fold: None, mark: None, foldable: None, providers, quick_look: None, selection: None, full: None, helpers: Vec::new(), search: None, scp: None, image: None, point: None, stamp: None, copy: None }
    }
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

/// A row's label made inert and cut: the selection is a program's text.
fn helper_row(h: &crate::ctx_help::Helper) -> HelperRow {
    HelperRow { label: crate::browser_ctx::inert_label(&h.title), enabled: h.copy.is_some(), swatch: h.color }
}

impl App {
    /// The inline image under `point` and what it offers (`imageInfoAtCoord:`).
    fn image_at(&self, id: PaneId, point: Point) -> Option<(u32, ImageFacts)> {
        let p = self.panes.get(&id)?;
        let tile = p.screen.term().grid()[point].image()?;
        let img = p.screen.images.by_id.get(&tile.image)?;
        Some((tile.image, ImageFacts { broken: img.broken, animated: img.frames.len() > 1, paused: img.paused.is_some() }))
    }

    /// A right click off the selection selects what is under it, as iTerm2's does (`CMH:177-195`): the
    /// Smart Selection match, else the word. All white space undoes it, and the old selection returns.
    fn select_for_menu(&mut self, id: PaneId, point: Point) {
        if self.cell_is_selected(id, point) {
            return;
        }
        let saved = self.panes.get(&id).map(|p| p.screen.subs()).unwrap_or_default();
        if self.smart_select(id, point, false).is_none() {
            self.select_word_at(id, point);
        }
        if self.selected_text(id).is_none_or(|t| t.trim().is_empty()) {
            if let Some(p) = self.panes.get_mut(&id) {
                if saved.is_empty() || !p.screen.set_subs(saved) {
                    p.clear_selection();
                }
            }
        }
    }

    /// The label a tab has in the Select submenu: its title and number (`PT:7222-7248`).
    fn select_label(&self, i: usize) -> String {
        let html = self.globals.bool("HTMLTabTitles");
        let Some(t) = self.tabs.get(i) else { return String::new() };
        let look = self.tab_look(t);
        let title = crate::tab_layout::plain_title(&crate::tab_layout::limit_title(&look.title), html);
        format!("{title} #{}", i + 1)
    }

    /// The pane menu for `at`: the pane and, for a right click, the point (the mark, the fold, the link
    /// and the selection come from the point). The title bar passes its pane and no point
    /// (`titleBarMenu`).
    pub(super) fn show_context_menu_at(&mut self, at: Option<(PaneId, Option<Point>)>) {
        let Some((id, point)) = at else { return };
        if !self.panes.contains_key(&id) {
            return;
        }
        if let Some(rows) = point.and_then(|_| self.timestamp_menu(id)) {
            return self.show_context_rows(&rows);
        }
        let mut c = Ctx { at_point: point.is_some(), ..Ctx::default() };
        let mut pend = CtxPending::for_pane(id, Vec::new());
        pend.point = point;
        if let Some((img, facts)) = point.and_then(|p| self.image_at(id, p)) {
            c.image = Some(facts);
            pend.image = Some(img);
        } else if let Some(p) = point {
            self.select_for_menu(id, p);
        }
        if let Some(p) = point {
            self.context_marks(id, p, &mut c, &mut pend);
        }
        // New Window, New Tab and Select (`PT:7200-7266`).
        let nodes = crate::profile_menu::tree(&crate::profile_menu::menu_rows(&self.store.profiles));
        c.new_window = ctx_menu::profile_rows(&nodes, "newwin");
        c.new_tab = ctx_menu::profile_rows(&nodes, "newtab");
        if self.tabs.len() > 1 {
            c.select = (0..self.tabs.len()).map(|i| ctx_menu::item(&format!("selecttab:{i}"), &self.select_label(i), true)).collect();
        }
        if c.image.is_none() {
            self.context_selection(id, &mut c, &mut pend);
            if let Some(p) = point {
                // Copy Link Address, or Copy URL for a detected one, and the Smart Selection actions
                // of the selected text (`CMH:571-577`), greyed in a remote pane (P10).
                let remote = self.remote_text(id);
                if let Some(click) = self.clicked(id, p) {
                    pend.cwd = click.ctx.cwd.as_ref().map(|d| d.to_string_lossy().to_string()).unwrap_or_default();
                    if click.ctx.link.is_some() {
                        c.link = Link::Address;
                    } else if semantic::url_like_in(&click.ctx.before, &click.ctx.after, &click.ctx.urls).is_some() {
                        c.link = Link::Url;
                    }
                    pend.link = click.ctx.link.clone().or_else(|| semantic::url_like_in(&click.ctx.before, &click.ctx.after, &click.ctx.urls));
                }
                if let Some(text) = pend.full.clone().filter(|t| t.chars().count() < crate::ctx_help::MAX_CUSTOM_ACTIONS_TEXT) {
                    let rules = self.panes.get(&id).and_then(|pane| self.store.get(&pane.profile)).map(smartsel::rules).unwrap_or_default();
                    let (host, user) = self.host_user(id).unwrap_or_default();
                    for (rule, captures) in smartsel::rules_matching_text(&text, &rules) {
                        for a in &rules[rule].actions {
                            let title = smartsel::expand(&a.title, &captures, if pend.cwd.is_empty() { "." } else { &pend.cwd }, &host, &user);
                            c.actions.push((crate::browser_ctx::inert_label(&title), ssh_files::remote_action_allowed(remote, &a.action)));
                            pend.actions.push((a.clone(), captures.clone()));
                        }
                    }
                }
            }
        }
        self.context_pane_facts(id, point, &mut c);
        // The script providers' items (`CMH:898-914`), for that session.
        pend.providers = crate::script_fn::context_rows(&self.api_fns.context_menu_items());
        c.providers = pend.providers.clone();
        self.ctx_pending = Some(pend);
        let rows = ctx_menu::build(&c);
        self.show_context_rows(&rows);
    }

    /// The marks on the clicked line (`CMH:208-267`): Command Info, a named mark's name, Fold or Unfold.
    fn context_marks(&self, id: PaneId, point: Point, c: &mut Ctx, pend: &mut CtxPending) {
        let Some(p) = self.panes.get(&id) else { return };
        let line = point.line.0;
        if let Some(f) = p.screen.fold_at(line) {
            c.fold = Fold::Unfold;
            pend.fold = Some(f);
        }
        let on_line = p.screen.term().grid()[Line(line)].mark();
        if on_line != 0 {
            if let Some(m) = p.screen.command_mark(on_line) {
                c.command_info = !m.command.trim().is_empty();
                c.mark_name = m.name.clone();
                pend.mark = Some(m.id);
            }
        }
        if c.fold == Fold::None {
            // The command at or above the line folds when it has output the fold can take (`CMH:255-266`).
            let cursor = p.screen.term().grid().cursor.point.line.0;
            let foldable = p.screen.prompt_at_or_before(line).filter(|&m| p.screen.command_record(m).is_some_and(|r| !r.command.trim().is_empty())).filter(|&m| {
                p.screen.output_range_of(m).is_some_and(|(a, b)| !(a.line.0..=b.line.0).contains(&cursor) && p.screen.fold_at(a.line.0).is_none())
            });
            if foldable.is_some() {
                c.fold = Fold::Fold;
                pend.foldable = foldable;
            }
        }
    }

    /// The selection's facts: its helpers, replacements, one-cell description, the Download title and the web items.
    fn context_selection(&mut self, id: PaneId, c: &mut Ctx, pend: &mut CtxPending) {
        let columns = self.panes.get(&id).map_or(0, |p| p.screen.term().columns());
        let sel = self.selected_text(id).filter(|t| !t.is_empty());
        let len = sel.as_ref().map_or(0, |t| t.chars().count());
        c.has_selection = sel.is_some();
        c.reasonable = sel.is_some() && len <= 1_000_000;
        let short = sel.clone().filter(|_| crate::web_popover::short_selection(len, columns));
        c.short = short.clone();
        pend.selection = short.clone();
        pend.full = sel.clone();
        // The helpers (`CMH:435-505`), made inert and bounded.
        let mut groups = crate::ctx_help::groups(short.as_deref(), c.reasonable.then(|| (sel.as_deref().unwrap_or(""), sel.as_deref().unwrap_or(""))));
        groups.retain(|h| !crate::browser_ctx::inert_label(&h.title).is_empty());
        c.helpers = groups.clone().map(|h| helper_row(&h));
        pend.helpers = groups.into_flat();
        if c.reasonable {
            c.replacements = self.replacement_kinds(id);
        }
        // One selected cell is described (`CMH:507-523`).
        if let (Some((a, b)), Some(p)) = (self.selection_pos(id), self.panes.get(&id)) {
            if a == b {
                c.char_description = ctx_menu::char_description(&p.screen.term().grid()[to_point(&p.screen, a)]);
            }
        }
        // Download (`CMH:528-548`): Termurion's file transfer is the SSH integration's, so the item
        // is enabled only when this pane's conductor can take the path.
        let integration = self.scp_path_for(id, "placeholder").is_some_and(|to| self.transfer_pane(id, &to));
        c.scp_title = if integration { "Download using SSH Integration" } else { "Download with scp" }.to_string();
        if let Some(text) = short.as_deref().map(str::trim).filter(|t| !t.is_empty() && !t.contains('\n')) {
            if let Some(path) = self.scp_path_for(id, text) {
                c.scp_title = format!("{} from {}", c.scp_title, shown_name(&path.hostname));
                c.scp_enabled = integration;
                pend.scp = Some(text.to_string());
            }
        }
        // The web items (`CMH:549-563`).
        if let Some(text) = &short {
            c.locally_openable = self.link_locally_openable(text);
            if let Some(url) = crate::web_popover::quick_look_target(text) {
                c.quick_look = true;
                pend.quick_look = Some((url, self.pointer_screen()));
            }
        }
        pend.search = sel.as_deref().and_then(crate::browser::search_query);
    }

    /// The pane's own facts: locks, moves, paste, annotations, coprocess, restart, bury, tmux and the terminal state.
    fn context_pane_facts(&mut self, id: PaneId, point: Option<Point>, c: &mut Ctx) {
        c.layout_locked = self.win_locks.layout;
        c.pane_locked = self.pane_locked(id);
        let (many, locked, all, any) = self.lock_facts_for(id);
        c.tab_has_many = many;
        c.all_locked = all;
        c.any_locked = any;
        c.alt = self.mods.alt;
        let may_move = !c.layout_locked && !locked;
        c.move_to_tab_ok = may_move && many;
        c.move_to_window_ok = may_move && self.tabs.iter().map(|t| t.layout.panes().len()).sum::<usize>() > 1;
        c.pasteable = self.clipboard_pasteable();
        c.has_coprocess = self.panes.get(&id).is_some_and(|p| p.coprocess_command().is_some());
        c.restartable = self.restartable(id);
        c.can_bury = !self.bury_refused(id);
        c.tmux_paused = self.tmux_of(id).and_then(|(gw, pane, _)| self.tmux.get(&gw).filter(|t| t.ctl.pause_mode()).map(|t| t.ctl.is_paused(pane)));
        c.terminal_state.on = self.terminal_state_on(id);
        c.annotation_hidden = point.is_some_and(|p| !self.hidden_notes_at(id, p).is_empty());
    }

    /// Whether Paste has something to paste: text, a file or a picture (`PTYTextView.m:479-495`).
    fn clipboard_pasteable(&mut self) -> bool {
        let Some(cb) = self.clipboard.as_mut() else { return false };
        cb.get_text().is_ok() || crate::winclip::first_file().is_some() || cb.get_image().is_ok()
    }

    /// The hidden annotations covering `point` (`hasOpenAnnotationInRange:`, `CMH:355-364`).
    fn hidden_notes_at(&self, id: PaneId, point: Point) -> Vec<u32> {
        let Some(p) = self.panes.get(&id) else { return Vec::new() };
        let at = to_pos(&p.screen, point);
        self.note_cells(id).into_iter().filter(|(m, nid, _)| m.start <= at && at <= m.end && p.screen.notes.by_id.get(nid).is_some_and(|n| n.hidden)).map(|(_, nid, _)| nid).collect()
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
            CtxPick::Other => self.context_other_picked(item, pend, event_loop),
        }
        self.redraw();
        true
    }

    fn context_other_picked(&mut self, item: &str, pend: CtxPending, event_loop: &ActiveEventLoop) {
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
        } else if item == "fold" {
            if let Some(m) = pend.foldable {
                self.cmd_fold(pend.pane, m);
            }
        } else if item == "cmdinfo" {
            if let Some(m) = pend.mark {
                let at = self.cursor;
                self.show_command_info(pend.pane, m, at, event_loop);
            }
        } else if item == "removemark" {
            if let (Some(m), Some(p)) = (pend.mark, self.panes.get_mut(&pend.pane)) {
                p.screen.rename_named_mark(m, None);
            }
        } else if let Some(i) = item.strip_prefix("selecttab:").and_then(|n| n.parse::<usize>().ok()) {
            if i < self.tabs.len() {
                self.select_tab(i);
            }
        } else if let Some(guid) = item.strip_prefix("newwin:") {
            self.pending_windows.push(NewWindow::Profile(Some(guid.to_string())));
        } else if let Some(guid) = item.strip_prefix("newtab:") {
            self.new_tab(Some(guid));
        } else if let Some(path) = item.strip_prefix("newwinall:") {
            self.open_all_from_menu(path, true);
        } else if let Some(path) = item.strip_prefix("newtaball:") {
            self.open_all_from_menu(path, false);
        } else if let Some(i) = item.strip_prefix("helper.").and_then(|n| n.parse::<usize>().ok()) {
            // A helper copies its text (bytes decoded as UTF-8 with what is not lost), and remembers it.
            if let Some(copy) = pend.helpers.get(i).and_then(|h| h.copy.as_ref()) {
                let text = crate::ctx_help::copy_text(copy);
                self.set_clipboard_text(text.clone());
                self.remember_paste(&text);
            }
        } else if item == "quicklook" {
            if let Some((url, at)) = pend.quick_look {
                self.quick_look_url(&url, at, event_loop);
            }
        } else if let Some(style) = match item {
            "openurl" => Some(crate::browser::Style::Tab),
            "openurlv" => Some(crate::browser::Style::Vertical),
            "openurlh" => Some(crate::browser::Style::Horizontal),
            _ => None,
        } {
            if let Some(text) = pend.selection {
                self.open_selection_url(pend.pane, &text, style);
            }
        } else if item == "mail" {
            if let Some(text) = pend.full {
                self.open_selection_mail(pend.pane, &text);
            }
        } else if item == "scp" {
            if let Some(text) = pend.scp {
                if !self.secure_copy_text(pend.pane, Some(&text)) {
                    self.banners.set("action", "Termurion could not download this file: the session cannot take transfers.");
                }
            }
        } else if item == "sendselection" {
            if let Some(text) = pend.full {
                self.cmdinfo_write(pend.pane, &text);
            }
        } else if item == "snippet" {
            if let Some(text) = pend.full {
                if self.cmdinfo_add_snippet(&text) {
                    self.show_toast(crate::cmdinfo::SNIPPET_ADDED);
                }
            }
        } else if item == "reveal" {
            if let Some(point) = pend.point {
                for n in self.hidden_notes_at(pend.pane, point) {
                    if let Some(entry) = self.panes.get_mut(&pend.pane).and_then(|p| p.screen.notes.by_id.get_mut(&n)) {
                        entry.hidden = false;
                    }
                }
            }
        } else if item == "searchweb" {
            // The search engine's page, built from the template and the escaped selection (`CMH:1179-1186`).
            if let Some(text) = pend.search {
                let url = crate::browser::search_url(&crate::browser::search_template(&self.globals.string("SearchCommand")), &text);
                self.execute(pend.pane, semantic::guard_open(&url, has_code()), None, false);
            }
        } else if item == "copylink" {
            if let (Some(link), Some(cb)) = (pend.link, &mut self.clipboard) {
                if let Err(e) = cb.set_text(link) {
                    self.banners.set("clipboard", format!("Copy failed: {e}"));
                }
            }
        } else if let Some((a, captures)) = item.strip_prefix("action.").and_then(|n| n.parse::<usize>().ok()).and_then(|n| pend.actions.get(n).cloned()) {
            let cwd_path = (!pend.cwd.is_empty()).then(|| std::path::PathBuf::from(&pend.cwd));
            // Asked again at the pick: the pane may have started framing since the menu showed.
            if !ssh_files::remote_action_allowed(self.remote_text(pend.pane), &a.action) {
                self.banners.set("action", format!("{} does not run for text from a remote host.", a.title));
            } else if let Some(run) = self.smart_action_run(pend.pane, &a, &captures, &pend.cwd) {
                self.execute_with(pend.pane, run, cwd_path.as_deref(), Exec { background: false, from_file: false, event_loop: Some(event_loop) });
            }
        } else if item.starts_with("img.") {
            self.context_image_picked(item, &pend);
        } else if item == "copytext" {
            if let (Some(text), Some(cb)) = (pend.copy, &mut self.clipboard) {
                if let Err(e) = cb.set_text(text) {
                    self.banners.set("clipboard", format!("Copy failed: {e}"));
                }
            }
        } else if item == "tsbaseline" || item == "tsdisable" {
            self.set_timestamp_baseline(pend.pane, if item == "tsdisable" { 0.0 } else { pend.stamp.unwrap_or(0.0) });
        }
    }

    /// Send Email to Selected Address (`CMH:1171-1181`): the selected text as a `mailto:` link, opened
    /// as any mail link is. Text a remote host wrote opens only a web or mail link (P10), and the link
    /// goes through `guard_open`.
    pub(super) fn open_selection_mail(&mut self, id: PaneId, text: &str) {
        let text = text.trim();
        if text.is_empty() || text.chars().any(char::is_control) {
            return self.banners.set("action", "Termurion did not open an email link: the selection is not an address.");
        }
        let url = if text.starts_with("mailto:") { text.to_string() } else { format!("mailto:{text}") };
        if ssh_files::click_route(self.remote_text(id), Some(&url)) != ssh_files::ClickRoute::Local {
            return;
        }
        self.execute(id, semantic::guard_open(&url, has_code()), None, false);
    }

    /// The first frame of an image as RGBA, with its size.
    fn image_pixels(&self, pane: PaneId, img: u32) -> Option<(u32, u32, Vec<u8>, Option<String>)> {
        let i = self.panes.get(&pane)?.screen.images.by_id.get(&img)?;
        let frame = i.frames.get(i.paused.unwrap_or(0).min(i.frames.len().saturating_sub(1)))?;
        Some((i.width_px, i.height_px, frame.rgba.as_ref().clone(), i.filename.clone()))
    }

    /// The image items (`CMH:1344-1366`, `PTYTextView+ARC.m:2090-2170`).
    fn context_image_picked(&mut self, item: &str, pend: &CtxPending) {
        let Some(img) = pend.image else { return };
        if item == "img.animation" {
            if let Some(i) = self.panes.get_mut(&pend.pane).and_then(|p| p.screen.images.by_id.get_mut(&img)) {
                i.paused = if i.paused.is_some() { None } else { Some(0) };
            }
            return;
        }
        let Some((w, h, rgba, name)) = self.image_pixels(pend.pane, img) else { return };
        match item {
            "img.inspect" => {
                let shown = name.as_deref().map_or_else(|| "Unnamed file".to_string(), shown_name);
                info_box(&format!("Filename: {shown}\nDimensions: {w} x {h}"));
            }
            "img.copy" => {
                if let Some(cb) = &mut self.clipboard {
                    if let Err(e) = cb.set_image(arboard::ImageData { width: w as usize, height: h as usize, bytes: std::borrow::Cow::Owned(rgba) }) {
                        self.banners.set("clipboard", format!("Copy failed: {e}"));
                    }
                }
            }
            "img.save" => {
                let base = name.as_deref().and_then(|n| std::path::Path::new(n).file_stem()).map_or_else(|| "image".to_string(), |s| s.to_string_lossy().into_owned());
                let dialog = rfd::FileDialog::new().set_title("Save Image As").set_file_name(format!("{base}.png")).add_filter("Image", &["png", "bmp", "gif", "jpg", "jpeg", "tiff"]);
                let Some(path) = crate::localfs::save_file(dialog) else { return };
                match encode_image(w, h, rgba, path.path()) {
                    Ok(bytes) => {
                        if let Err(e) = crate::localfs::write(&path, bytes) {
                            self.banners.set("action", format!("Could not save {}: {e}", path.path().display()));
                        }
                    }
                    Err(e) => self.banners.set("action", e),
                }
            }
            "img.open" => {
                // A temporary picture in Termurion's own folder, opened by its default app.
                let dir = std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, std::path::PathBuf::from).join("Termurion").join("images");
                let result = crate::localfs::judge_io(&dir).and_then(|d| {
                    crate::localfs::create_dir_all(&d)?;
                    let file = d.child(format!("image-{}-{img}.png", std::process::id())).ok_or_else(crate::localfs::refused)?;
                    let bytes = encode_image(w, h, rgba, std::path::Path::new("image.png")).map_err(std::io::Error::other)?;
                    crate::localfs::write(&file, bytes)?;
                    crate::localfs::open_with_shell(&file, None, false)
                });
                if let Err(e) = result {
                    self.banners.set("action", format!("Could not open the image: {e}"));
                }
            }
            _ => {}
        }
    }
}

/// The picture as the bytes of the file named by `path`'s extension (PNG when it has none).
fn encode_image(w: u32, h: u32, rgba: Vec<u8>, path: &std::path::Path) -> Result<Vec<u8>, String> {
    let buf = image::RgbaImage::from_raw(w, h, rgba).ok_or_else(|| "The picture is not complete.".to_string())?;
    let format = image::ImageFormat::from_path(path).unwrap_or(image::ImageFormat::Png);
    let mut dynamic = image::DynamicImage::ImageRgba8(buf);
    // JPEG and BMP take no alpha channel.
    if matches!(format, image::ImageFormat::Jpeg | image::ImageFormat::Bmp) {
        dynamic = image::DynamicImage::ImageRgb8(dynamic.to_rgb8());
    }
    let mut out = std::io::Cursor::new(Vec::new());
    dynamic.write_to(&mut out, format).map_err(|e| format!("Could not encode the picture: {e}"))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx_menu::Row;

    #[test]
    fn picks_act_on_the_menus_pane() {
        let (a, b): (PaneId, PaneId) = (1, 2);
        // Menu for B while A is focused: a provider pick and a Swap reach B only.
        assert_eq!(ctx_pick("termurion.ctx.provider.u1", b, Some(a), true), CtxPick::Provider { pane: b, unique_id: "u1".into() });
        assert_eq!(ctx_pick("termurion.ctx.swappane", b, Some(a), true), CtxPick::Swap(b));
        // Every plain command of the menu (the move group among them) focuses B first.
        for id in ["Session > Move Session > Move Session to Split Pane", "Session > Move Session > Move Session to Tab", "Session > Move Session > Move Session to Window", "Edit > Copy", "Session > Bury Session", "Session > Restart Session"] {
            let item = format!("termurion.ctx.cmd.{id}");
            assert_eq!(ctx_pick(&item, b, Some(a), true), CtxPick::Command { id: id.into(), focus: Some(b), restore: Some(a) }, "{id}");
            // Already focused: nothing to move, nothing to give back.
            assert_eq!(ctx_pick(&item, b, Some(b), true), CtxPick::Command { id: id.into(), focus: None, restore: None });
        }
        assert_eq!(ctx_pick("termurion.ctx.stopcoprocess", b, Some(a), true), CtxPick::Other);
        assert_eq!(ctx_menu::command("Edit > Copy", "Copy", true), ctx_menu::item("cmd.Edit > Copy", "Copy", true));
        assert!(matches!(ctx_menu::command("Edit > Copy", "Copy", true), Row::Item { id, .. } if id == "termurion.ctx.cmd.Edit > Copy"));
    }

    #[test]
    fn a_gone_menu_pane_does_nothing() {
        let (a, b): (PaneId, PaneId) = (1, 2);
        // The menu's pane closed before the pick arrived: no command runs on the focused pane.
        assert_eq!(ctx_pick("termurion.ctx.cmd.Shell > Close", b, Some(a), false), CtxPick::Gone);
        assert_eq!(ctx_pick("termurion.ctx.cmd.Edit > Paste", b, Some(a), false), CtxPick::Gone);
        assert_eq!(ctx_pick("termurion.ctx.swappane", b, Some(a), false), CtxPick::Gone);
        // A provider still resolves by id (P16).
        assert_eq!(ctx_pick("termurion.ctx.provider.u1", b, Some(a), false), CtxPick::Provider { pane: b, unique_id: "u1".into() });
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

    /// The pending item exists for the menu's pane even with nothing else, so the providers fill
    /// (P16, `m11c-rulings.txt:161`).
    #[test]
    fn the_pending_item_is_built_for_the_menus_pane() {
        let pend = CtxPending::for_pane(2, vec![("Item".into(), "u1".into())]);
        assert!(pend.link.is_none() && pend.fold.is_none() && pend.actions.is_empty());
        assert_eq!(pend.providers, vec![("Item".to_string(), "u1".to_string())]);
    }

    /// The helper rows are inert and bounded, and a color copies nothing (`CMU:336-363`).
    #[test]
    fn helper_rows_are_inert() {
        let g = crate::ctx_help::groups(Some("255"), Some(("255", "255")));
        assert!(g.synonyms.iter().any(|h| helper_row(h).label == "255 = 0xff" && helper_row(h).enabled));
        let color = crate::ctx_help::groups(Some("#ff8000"), None);
        assert_eq!(helper_row(color.color.as_ref().unwrap()).swatch, Some((255, 128, 0)));
        assert!(!helper_row(color.color.as_ref().unwrap()).enabled, "a color copies nothing");
        let evil = format!("{}\u{202e}\u{200b}", "9".repeat(150));
        for h in crate::ctx_help::groups(Some(&evil), Some((&evil, &evil))).into_flat() {
            let l = helper_row(&h).label;
            assert!(!l.contains('\u{202e}') && !l.contains('\u{200b}') && l.chars().count() <= 120, "{l:?}");
        }
        // Nothing selected: no rows.
        assert!(crate::ctx_help::groups(None, None).into_flat().is_empty());
        // A long selection offers the numeric rows and the Base64 copies but not the synonyms.
        let long = crate::ctx_help::groups(None, Some(("01 02 03 04", "01 02 03 04")));
        assert!(long.synonyms.is_empty() && !long.numeric.is_empty() && long.copy.len() >= 1);
    }

    /// The wiring: the builder numbers the helpers as the pick reads them, a pick copies and remembers,
    /// the menu asks the remote guard, and every item the menu builds has a pick.
    #[test]
    fn the_menu_is_wired() {
        let src = include_str!("ctx_menu_app.rs");
        let src = &src[..src.find("#[cfg(test)]").unwrap()];
        let build = &src[src.find("pub(super) fn show_context_menu_at").unwrap()..src.find("fn context_marks(").unwrap()];
        assert!(build.contains("into_flat()") || src.contains("pend.helpers = groups.into_flat();"));
        assert!(build.contains("remote_text(") && build.contains("ctx_menu::build(&c)"));
        let pick = &src[src.find("fn context_other_picked").unwrap()..src.find("pub(super) fn open_selection_mail").unwrap()];
        assert!(pick.contains("\"helper.\"") && pick.contains("remember_paste"));
        // Every own action the builder names is handled by a pick.
        let menu = include_str!("../ctx_menu.rs");
        let menu = &menu[..menu.find("#[cfg(test)]").unwrap()];
        for action in ["cmdinfo", "removemark", "unfold", "fold", "scp", "openurl", "openurlv", "openurlh", "quicklook", "searchweb", "mail", "swappane", "copylink", "sendselection", "snippet", "reveal", "togglebroadcast", "lockpane", "lockall", "unlockall", "stopcoprocess", "tsbaseline", "tsdisable"] {
            assert!(menu.contains(&format!("\"{action}\"")) || menu.contains(&format!("\"{action}.")), "the menu builds {action}");
            if !["swappane"].contains(&action) {
                assert!(pick.contains(&format!("\"{action}\"")) || src.contains(&format!("\"{action}\"")), "{action} has a pick");
            }
        }
        for prefix in ["selecttab:", "newwin:", "newtab:", "newwinall:", "newtaball:", "img.", "action.", "helper."] {
            assert!(pick.contains(&format!("\"{prefix}")), "{prefix} has a pick");
        }
    }

    /// Replacement items need whole lines: the check is the one Replace Selection makes.
    #[test]
    fn replacements_use_the_replace_selection_check() {
        let src = include_str!("seltools_ui.rs");
        let kinds = &src[src.find("pub(super) fn replacement_kinds").unwrap()..src.find("/// Replace Selection: the selected whole lines").unwrap()];
        assert!(kinds.contains("self.replace_source(id)"));
        let run = &src[src.find("fn replace_selection(").unwrap()..src.find("/// Clear to Start of Selection").unwrap()];
        assert!(run.contains("self.replace_source(id)"), "one check for the menu and the command");
    }

    /// An image is written out from its first frame, as the file named by its extension.
    #[test]
    fn an_image_encodes_by_its_extension() {
        let rgba = vec![200u8, 10, 10, 255, 10, 200, 10, 255];
        let png = encode_image(2, 1, rgba.clone(), std::path::Path::new("a.png")).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let jpg = encode_image(2, 1, rgba.clone(), std::path::Path::new("a.JPG")).unwrap();
        assert_eq!(&jpg[..2], [0xff, 0xd8], "JPEG takes no alpha but still encodes");
        assert_eq!(&encode_image(2, 1, rgba.clone(), std::path::Path::new("noext")).unwrap()[1..4], b"PNG");
        assert!(encode_image(3, 3, rgba, std::path::Path::new("a.png")).is_err(), "an incomplete picture is refused");
    }
}
