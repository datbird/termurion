//! The toolbelt in the app: each window's panel at its right (drawn, clicked, dragged), View >
//! Toolbelt and the hamburger's tool list, Open Toolbelt, and the arrangement keys. The pure
//! parts are `toolbelt` (the registry, the width and growth rules, the arrangement keys) and
//! `toolbelt_layout` (the split view and the panel layout).

use std::collections::BTreeSet;

use super::statusbar_content::MenuAt;
use super::toolbelt_tools_ui::{Shown, ShownKey, ToolUndo};
use super::*;
use crate::tool_list;
use crate::toolbelt::{self, Arranged, ToolId};
use crate::toolbelt_layout::{self as tbl, BeltHit, Split};

/// Menu ids of the tool check items (View > Toolbelt and the hamburger).
pub(super) const TOOL_ID: &str = "toolbelt:tool:";
/// Every toolbelt menu id starts so.
const MENU_PREFIX: &str = "toolbelt:";
/// Wrapper titles: the default control font, 13 points (`TW:51-60,103`).
const TITLE_PT: f32 = 13.0;
/// Room above and below a 1 point divider that still grabs it, in points.
const DIVIDER_SLOP: f32 = 3.0;

/// A tool's check item id, by the tool's name (built-in or dynamic).
pub(super) fn tool_menu_id(name: &str) -> String {
    format!("{TOOL_ID}{name}")
}

/// The tool name a check item's id carries.
pub(super) fn tool_name_from_menu_id(id: &str) -> Option<&str> {
    id.strip_prefix(TOOL_ID)
}

/// `text` broken at spaces into lines no wider than `max_w` (a word wider than that gets a
/// line of its own).
pub(super) fn wrap_words(text: &str, max_w: f32, width: &dyn Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        let joined = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if line.is_empty() || width(&joined) <= max_w {
            line = joined;
        } else {
            lines.push(std::mem::replace(&mut line, word.to_string()));
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// One tool's state in one window: its search field, its selection, its scroll, its open
/// folders, its popup's pick and its undo stack (`toolbelt_tools_ui`).
#[derive(Clone, Debug)]
pub(super) struct ToolState {
    pub(super) field: tool_list::Field,
    pub(super) selection: tool_list::Selection,
    pub(super) scroll: f32,
    /// Snippets' open folders, by tag path.
    pub(super) expanded: BTreeSet<String>,
    /// Profiles' bottom popup: the index into `toolbelt_tools::PROFILE_OPENS` last picked.
    pub(super) popup: usize,
    /// Snippets' and Actions' undo.
    pub(super) undo: Vec<ToolUndo>,
    /// The row count last laid out: a list that changed scrolls to its end unless it has the
    /// keyboard.
    pub(super) seen: usize,
    /// Captured Output's Clear (or an OSC 1337 ClearCapturedOutput it saw), and the session
    /// it was for (mark ids are per session).
    pub(super) cleared: crate::captured::Cleared,
    pub(super) cleared_pane: Option<PaneId>,
    /// An OSC 1337 ClearCapturedOutput the view just saw: the list shows nothing while the
    /// mark's captures and the search stay as they were then (`captured_fingerprint`, and the
    /// query), and everything again after the next change (`CO:157-163`).
    pub(super) blanked: Option<(u64, String)>,
    /// Jobs' collapsed processes (every parent starts expanded, `JT:484-569`).
    pub(super) collapsed: BTreeSet<u32>,
    /// Jobs' signal: an index into `toolbelt_tools::SIGNALS`, KILL at first (`JT:52-60`).
    pub(super) signal: usize,
}

impl Default for ToolState {
    fn default() -> Self {
        ToolState {
            field: Default::default(),
            selection: Default::default(),
            scroll: 0.0,
            expanded: BTreeSet::new(),
            popup: 0,
            undo: Vec::new(),
            seen: 0,
            cleared: Default::default(),
            cleared_pane: None,
            blanked: None,
            collapsed: BTreeSet::new(),
            signal: crate::toolbelt_tools::SIGNALS.iter().position(|s| *s == crate::toolbelt_tools::DEFAULT_SIGNAL).unwrap_or(0),
        }
    }
}

/// One window's toolbelt. The tool list, the saved proportions and the default width are
/// globals; everything here is per window (`WinSlot`).
#[derive(Default)]
pub(super) struct Belt {
    pub(super) visible: bool,
    /// Points; clamped to the window when laid out (`toolbelt::clamp_width`).
    pub(super) width: f32,
    /// The window showed its toolbelt before (the default width is read on the first show).
    pub(super) have_shown: bool,
    /// Hiding shrinks the window again: showing grew it (`hidingToolbeltShouldResizeWindow_`).
    pub(super) hiding_resizes: bool,
    pub(super) split: Split,
    /// The tool, and its part, that has the keyboard.
    pub(super) focus: Option<(ToolId, tool_list::Focus)>,
    pub(super) tools: HashMap<ToolId, ToolState>,
    /// What each tool shows, and what it was built from (`App::tool_shown`).
    pub(super) shown: HashMap<ToolId, (ShownKey, std::rc::Rc<Shown>)>,
    /// The arrangement's "Toolbelt" dictionary, kept as read and written back.
    pub(super) states: Option<toml::Table>,
    /// Session Status' snoozed sessions, each with its `tab_status_gen` when snoozed: a
    /// snooze holds until that session's status changes (`TS`, `snoozedSessionIDs`).
    pub(super) snoozed: std::collections::BTreeMap<u64, u64>,
    /// Notes' mode (Global first, not saved, `TN:91-99`), its editor, the editor's scroll
    /// (pixels) and the note the editor holds now (a different one loads, clearing the undo).
    pub(super) notes_mode: crate::notes::Mode,
    pub(super) note_edit: crate::text_edit::TextEdit,
    /// The editor's wrapped lines, kept between frames, keys and drag moves.
    pub(super) note_wrap: crate::text_edit::WrapCache,
    pub(super) note_scroll: f32,
    pub(super) note_source: Option<super::toolbelt_more_ui::NoteSource>,
    /// Codecierge's two editors (the goal view's and the suggestion view's reply), their wrapped
    /// lines and scrolls, the session whose text they hold, the transcript's distance from its
    /// end (0 shows the end) and its laid-out markdown with the key it was laid out for.
    pub(super) cc_goal: crate::text_edit::TextEdit,
    pub(super) cc_goal_wrap: crate::text_edit::WrapCache,
    pub(super) cc_goal_scroll: f32,
    pub(super) cc_reply: crate::text_edit::TextEdit,
    pub(super) cc_reply_wrap: crate::text_edit::WrapCache,
    pub(super) cc_reply_scroll: f32,
    pub(super) cc_guid: Option<String>,
    pub(super) cc_scroll: f32,
    pub(super) cc_doc: Option<(u64, crate::porthole::render::Layout)>,
    /// The saved proportions were offered to this window's split (its first layout, or an
    /// arrangement's own).
    proportions_tried: bool,
}

/// A drag that began in the toolbelt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum BeltDrag {
    /// The handle: where the drag began and the width then (points).
    Width { start_x: f32, start_w: f32 },
    /// The divider below wrapper `n`.
    Divider(usize),
    /// A row of Snippets or Actions being reordered (started by `tool_press`): the pointer's
    /// y when it was pressed.
    Row { tool: ToolId, start_y: f32 },
    /// A selection being dragged out in the Notes editor.
    Note,
    /// A selection being dragged out in Codecierge's editor.
    Codecierge,
}

fn float_of(v: Option<toml::Value>) -> Option<f32> {
    v.and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).map(|f| f as f32)
}

/// The split view's total height in points: the panel's, less the dividers.
fn split_total(panel_h: f32, tools: usize, scale: f32) -> f32 {
    (panel_h / scale - tools.saturating_sub(1) as f32 * tbl::DIVIDER).max(0.0)
}

impl App {
    /// The toolbelt's rectangle, while it shows.
    pub(super) fn toolbelt_panel(&self) -> Option<Rect> {
        self.window_layout().2
    }

    /// `ToolbeltTools` as stored (names of tools 9j does not show included).
    fn toolbelt_names(&self) -> Vec<String> {
        toolbelt::stored_names(self.globals.get(toolbelt::TOOLS_KEY).as_ref())
    }

    /// `ToolbeltTools` without the names no tool is registered under (`configuredTools`), what
    /// a toggle or a registration writes back.
    fn configured_toolbelt_names(&self) -> Vec<String> {
        toolbelt::configured_names(&self.toolbelt_names(), &toolbelt::dynamic_tools(self.globals.get(toolbelt::DYNAMIC_TOOLS_KEY).as_ref()))
    }

    pub(super) fn toolbelt_tools(&self) -> Vec<ToolId> {
        toolbelt::shown_tools(&self.toolbelt_names(), &toolbelt::dynamic_tools(self.globals.get(toolbelt::DYNAMIC_TOOLS_KEY).as_ref()))
    }

    /// The split view follows the tool list and the panel's height (hidden too: its height is
    /// the window's). A window's first layout applies the saved proportions (`RTV:277`).
    pub(super) fn layout_toolbelt(&mut self) {
        let scale = self.scale();
        let tools = self.toolbelt_tools();
        let total = split_total(self.window_room().h, tools.len(), scale);
        let before: f32 = self.belt.split.heights.iter().sum();
        let mut split = tbl::reconcile(&self.belt.split, &tools, total);
        if !self.belt.proportions_tried && !tools.is_empty() {
            self.belt.proportions_tried = true;
            if let Some(s) = self.globals.get(toolbelt::PROPORTIONS_KEY).and_then(|v| tbl::apply_proportions(&split, &v, total)) {
                split = s;
            }
        }
        // After proportions too: a saved prefix leaves the heights short of the total.
        self.belt.split = tbl::resize(&split, before, total);
        self.belt_drop_hidden_focus();
    }

    /// A tool that left the split, or whose wrapper collapsed, gives the keyboard back.
    fn belt_drop_hidden_focus(&mut self) {
        if self.belt.focus.is_some_and(|(t, _)| !self.belt.split.expanded(t)) {
            self.belt.focus = None;
        }
    }

    pub(super) fn belt_layout(&self) -> Option<tbl::BeltLayout> {
        self.toolbelt_panel().map(|panel| tbl::layout(panel, &self.belt.split, self.scale()))
    }

    /// Whether showing the toolbelt may grow the window (`window_can_fit`).
    fn belt_can_grow(&self) -> bool {
        self.window_can_fit()
    }

    /// The window's inner width grows (or shrinks, negative) by `points`. True when the size
    /// changed, so a Resized event follows (Windows resizes at once, and a size it refuses or
    /// one equal to the current size sends none).
    fn grow_window_by(&self, points: f32) -> bool {
        let Some(g) = self.gpu.as_ref() else { return false };
        let inner = g.window.inner_size();
        let w = (inner.width as f32 + points * self.scale()).round().max(1.0) as u32;
        if w == inner.width {
            return false;
        }
        let _ = crate::platform::caption::request_inner_size(&g.window, winit::dpi::PhysicalSize::new(w, inner.height));
        g.window.inner_size() != inner
    }

    /// View > Toolbelt > Show Toolbelt (Win+Shift+B).
    pub(super) fn toggle_toolbelt(&mut self) {
        self.set_toolbelt_visible(!self.belt.visible, true);
    }

    /// Shows or hides this window's toolbelt. With `grow`, showing grows the window by the
    /// toolbelt's width where it can (`PT:1286-1315`) and hiding shrinks a window that grew
    /// (`PT:1317-1346`).
    pub(super) fn set_toolbelt_visible(&mut self, on: bool, grow: bool) {
        self.show_toolbelt_growing(on, grow);
    }

    /// `set_toolbelt_visible`, returning how far showing grew the window (points). When the
    /// window resized, its Resized event lays it out; laying out now, against the old size,
    /// would shrink the panes and tell tmux the narrower size first (iTerm2 repositions and
    /// notifies tmux only when the window did not resize, `PT:1338-1341`).
    fn show_toolbelt_growing(&mut self, on: bool, grow: bool) -> f32 {
        if on == self.belt.visible {
            return 0.0;
        }
        let scale = self.scale();
        let can_grow = self.belt_can_grow();
        let mut grown = 0.0;
        let mut resized = false;
        if on {
            self.belt_first_show();
            if grow {
                let placed = self.gpu.as_ref().map(|g| (Self::outer_rect(g), g.window.inner_size().width as f32 / scale));
                let work = self.monitor_rects().map(|(w, _)| w);
                if let (Some((outer, inner_w)), Some(work)) = (placed, work) {
                    let g = toolbelt::show_growth(self.belt.width, inner_w + self.belt.width, (outer.x + outer.w) / scale, (work.x + work.w) / scale, can_grow);
                    self.belt.width = g.width;
                    self.belt.hiding_resizes = g.resized;
                    if g.grow_by > 0.0 {
                        grown = g.grow_by;
                        resized = self.grow_window_by(g.grow_by);
                    }
                }
            }
            self.belt.visible = true;
        } else {
            if grow {
                // What the toolbelt takes now (the stored width may be over the clamp).
                let width = self.toolbelt_panel().map_or(self.belt.width, |p| p.w / scale);
                let shrink = toolbelt::hide_shrink(width, self.belt.hiding_resizes, can_grow);
                if shrink > 0.0 {
                    resized = self.grow_window_by(-shrink);
                }
            }
            self.belt.visible = false;
            self.belt.focus = None;
        }
        if resized {
            self.redraw();
        } else {
            self.relayout();
        }
        self.sync_checks();
        grown
    }

    /// Open Toolbelt on a new window's first tab (`PT:8200-8207`); a hotkey window too, never
    /// growing. The deferred `finishToolbeltInitialization` then sets the resize flag from the
    /// grown frame's distance to the screen edge (`PT:782-787`, `PT:1061-1076`).
    pub(super) fn open_toolbelt_on_first_tab(&mut self, guid: Option<&str>) {
        let open = self.profile(guid).bool("Open Toolbelt");
        if !toolbelt::opens_on_first_tab(0, self.tabs.len(), open, self.belt.visible) {
            return;
        }
        let scale = self.scale();
        let right = self.gpu.as_ref().map(|g| {
            let r = Self::outer_rect(g);
            (r.x + r.w) / scale
        });
        let work = self.monitor_rects().map(|(w, _)| (w.x + w.w) / scale);
        let grown = self.show_toolbelt_growing(true, true);
        if let (Some(right), Some(work)) = (right, work) {
            self.belt.hiding_resizes = toolbelt::new_window_hiding_resizes(right, grown, work);
        }
    }

    /// The default width is read when the window first shows its toolbelt (`RTV:1497-1507`).
    fn belt_first_show(&mut self) {
        if !self.belt.have_shown {
            self.belt.have_shown = true;
            self.belt.width = float_of(self.globals.get(toolbelt::DEFAULT_WIDTH_KEY)).unwrap_or(toolbelt::DEFAULT_WIDTH);
        }
    }

    /// Every window hides its toolbelt (the only tool's close button, `TBV:699-705`,
    /// `PT:817-820`, kept).
    pub(super) fn hide_toolbelt_everywhere(&mut self) {
        let back = self.current_window();
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_id_of_key(k)).collect();
        for id in ids {
            if self.enter(id) && self.belt.visible {
                self.set_toolbelt_visible(false, true);
            }
        }
        if let Some(b) = back {
            self.enter(b);
        }
    }

    /// View > Toolbelt > Set Default Width: the toolbelt's width now, in points; nothing while
    /// it is hidden (`PT:1281-1284`).
    pub(super) fn set_default_toolbelt_width(&mut self) {
        let Some(panel) = self.toolbelt_panel() else { return };
        self.globals.set(toolbelt::DEFAULT_WIDTH_KEY, toml::Value::Float(f64::from(panel.w / self.scale())));
        self.save_globals();
    }

    /// A tool switched on (at the bottom) or off in `ToolbeltTools`; every window follows
    /// (`TBV:256-271`).
    pub(super) fn toggle_toolbelt_tool(&mut self, tool: ToolId) {
        self.toggle_toolbelt_named(tool.name());
    }

    /// `toggle_toolbelt_tool` by name: a dynamic tool's check item toggles it so.
    fn toggle_toolbelt_named(&mut self, name: &str) {
        let names = toolbelt::toggle_named(&self.configured_toolbelt_names(), name);
        self.globals.set(toolbelt::TOOLS_KEY, toolbelt::names_value(&names));
        self.save_globals();
        self.toolbelt_names_changed(&names);
    }

    /// RegisterTool (`H:2216-2243`): iTerm2's status; on OK the tool is registered
    /// (`TBV:154-186`), what changed is saved, and every window's Toolbelt menu follows.
    pub(super) fn api_register_tool(&mut self, r: &crate::api_proto::pb::RegisterToolRequest) -> crate::api_proto::pb::register_tool_response::Status {
        use crate::api_proto::pb::register_tool_response::Status;
        let status = toolbelt::register_tool_status(r);
        if status != Status::Ok {
            return status;
        }
        let (name, identifier, url) = (r.name.as_deref().unwrap_or_default(), r.identifier.as_deref().unwrap_or_default(), r.url.as_deref().unwrap_or_default());
        let before = self.toolbelt_names();
        let reloads = toolbelt::registration_reloads(self.globals.get(toolbelt::DYNAMIC_TOOLS_KEY).as_ref(), identifier, name, url);
        let (registry, names) = toolbelt::register_dynamic(self.globals.get(toolbelt::DYNAMIC_TOOLS_KEY).as_ref(), &self.configured_toolbelt_names(), identifier, name, url, r.reveal_if_already_registered());
        let names_changed = names != before;
        if let Some(registry) = &registry {
            self.globals.set(toolbelt::DYNAMIC_TOOLS_KEY, registry.clone());
        }
        if names_changed {
            self.globals.set(toolbelt::TOOLS_KEY, toolbelt::names_value(&names));
        }
        if registry.is_some() || names_changed {
            self.save_globals();
            self.toolbelt_names_changed(&names);
        }
        // The same name and URL again: the shown view loads its page again (`TBV:160-168`).
        if reloads {
            self.reload_tool_view(identifier);
        }
        status
    }

    /// `ToolbeltTools` or the dynamic tools changed: every window drops focus from a hidden
    /// tool, lays out again and refreshes its Toolbelt menu.
    fn toolbelt_names_changed(&mut self, names: &[String]) {
        self.menus_dirty = true;
        let back = self.current_window();
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_id_of_key(k)).collect();
        for id in ids {
            if self.enter(id) {
                if self.belt.focus.is_some_and(|(t, _)| !names.iter().any(|n| n == t.name())) {
                    self.belt.focus = None;
                }
                self.relayout();
                self.sync_checks();
            }
        }
        if let Some(b) = back {
            self.enter(b);
        }
    }

    /// Every tool's name as the Toolbelt menu lists them: the built-in and dynamic tools
    /// sorted by name (`TBV:227-254`).
    fn toolbelt_menu_names(&self) -> Vec<String> {
        toolbelt::all_tool_names(&toolbelt::dynamic_tools(self.globals.get(toolbelt::DYNAMIC_TOOLS_KEY).as_ref()))
    }

    /// The tools as check items, sorted by name, checked when configured (`TBV:227-254`).
    fn toolbelt_menu_items(&self, all: &[String]) -> Vec<muda::CheckMenuItem> {
        let names = self.toolbelt_names();
        all.iter().map(|t| muda::CheckMenuItem::with_id(tool_menu_id(t), t.replace('&', "&&"), true, names.contains(t), None)).collect()
    }

    /// The hamburger's menu: the tool list.
    pub(super) fn show_toolbelt_menu(&mut self, at: MenuAt) {
        let menu = muda::Menu::new();
        if let Some(Err(e)) = self.toolbelt_menu_items(&self.toolbelt_menu_names()).iter().map(|i| menu.append(i)).find(Result::is_err) {
            self.banners.set("menu", format!("Toolbelt menu: {e}"));
        }
        self.show_menu_at(menu, at);
    }

    /// View > Toolbelt's tool items, after its separator. Returns them for `sync_checks`.
    pub(super) fn append_toolbelt_menu(&mut self, sub: &muda::Submenu) -> Vec<(String, muda::CheckMenuItem)> {
        let all = self.toolbelt_menu_names();
        let items = self.toolbelt_menu_items(&all);
        if let Some(Err(e)) = items.iter().map(|i| sub.append(i)).find(Result::is_err) {
            self.banners.set("menu", format!("View > Toolbelt: {e}"));
        }
        all.into_iter().zip(items).collect()
    }

    /// The tool items follow the registered tools (a dynamic tool added or renamed rebuilds
    /// them) and their check marks follow `ToolbeltTools`.
    pub(super) fn sync_toolbelt_checks(&mut self) {
        let all = self.toolbelt_menu_names();
        let listed = self.menu_bar.as_ref().is_none_or(|b| b.toolbelt_checks.iter().map(|(n, _)| n).eq(all.iter()));
        if !listed {
            let items = self.toolbelt_menu_items(&all);
            let Some(bar) = &mut self.menu_bar else { return };
            if let Some((sub, fixed)) = &bar.toolbelt {
                while sub.remove_at(*fixed).is_some() {}
                if let Some(Err(e)) = items.iter().map(|i| sub.append(i)).find(Result::is_err) {
                    self.banners.set("menu", format!("View > Toolbelt: {e}"));
                }
                bar.toolbelt_checks = all.into_iter().zip(items).collect();
            }
        }
        let Some(bar) = &self.menu_bar else { return };
        let names = self.toolbelt_names();
        for (t, c) in &bar.toolbelt_checks {
            c.set_checked(names.contains(t));
        }
    }

    /// A toolbelt menu pick. False when `id` is not one.
    pub(super) fn toolbelt_picked(&mut self, id: &str, event_loop: &ActiveEventLoop) -> bool {
        if !id.starts_with(MENU_PREFIX) {
            return false;
        }
        if self.tool_menu_picked(id, event_loop) {
            return true;
        }
        if let Some(name) = tool_name_from_menu_id(id) {
            self.toggle_toolbelt_named(name);
        }
        true
    }

    /// Draws this window's toolbelt: the wrappers (title, close) and the dividers; each
    /// tool's content is its own.
    pub(super) fn draw_toolbelt(&mut self, frame: &mut Frame, cell: (f32, f32)) {
        self.belt_tips.clear();
        let Some(l) = self.belt_layout() else {
            // A hidden toolbelt shows no Jobs: the watcher lets this window's tree go.
            self.hide_tool_views();
            return self.jobs_want(false);
        };
        let scale = self.scale();
        let dark = self.dark();
        let Some(g) = self.gpu.as_mut() else { return };
        let text_scale = TITLE_PT * 4.0 / 3.0 * scale / g.renderer.font_px().max(1.0);
        let renderer = std::cell::RefCell::new(&mut g.renderer);
        let width = |s: &str| renderer.borrow_mut().text_width(s, true, text_scale);
        let mut titles = Vec::with_capacity(l.wrappers.len());
        let mut widths = Vec::with_capacity(l.wrappers.len());
        for (w, t) in l.wrappers.iter().zip(&self.belt.split.tools) {
            let shown = tool_list::truncate_tail(t.name(), w.title.w, &width);
            widths.push(width(&shown));
            titles.push(shown);
        }
        let empty: Vec<(String, f32)> = l.empty_label.map(|r| wrap_words(toolbelt::NO_TOOLS, r.w, &width).into_iter().map(|line| {
                    let w = width(&line);
                    (line, w)
                })
                .collect()).unwrap_or_default();
        let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
        let d = scene::BeltDraw { layout: &l, titles: &refs, dark, title_widths: &widths, empty: &empty, text_scale };
        scene::toolbelt(frame, &d, cell, scale);
        // A dynamic tool's content is its web view's (a child window); nothing is drawn for it.
        let tools = self.belt.split.tools.clone();
        self.place_tool_views(&l, &tools);
        self.draw_tools(frame, cell);
    }

    /// A press. True when it was in the toolbelt (other buttons than the left do nothing
    /// there but in a tool's list: the terminal is not under it). A press anywhere else gives
    /// the keyboard back; a press in it takes it from the Clippings panels.
    pub(super) fn toolbelt_press(&mut self, button: MouseButton, x: f32, y: f32, event_loop: &ActiveEventLoop) -> bool {
        let Some(l) = self.belt_layout().filter(|l| l.panel.contains(x, y)) else {
            self.belt.focus = None;
            return false;
        };
        for c in self.clips.values_mut() {
            c.keys = false;
        }
        if button != MouseButton::Left {
            if let BeltHit::Content(i) = tbl::hit(&l, x, y)
                && let (Some(&tool), Some(area)) = (self.belt.split.tools.get(i), l.wrappers.get(i).and_then(|w| w.content)) {
                    self.tool_press(tool, area, button, x, y, event_loop);
                }
            return true;
        }
        let scale = self.scale();
        let slop = DIVIDER_SLOP * scale;
        let divider = l.dividers.iter().position(|d| Rect { y: d.y - slop, h: d.h + 2.0 * slop, ..*d }.contains(x, y));
        let hit = match divider {
            Some(i) if !l.handle.contains(x, y) => BeltHit::Divider(i),
            _ => tbl::hit(&l, x, y),
        };
        match hit {
            BeltHit::Handle => {
                let w = l.panel.w / scale;
                self.drag = Drag::Belt(BeltDrag::Width { start_x: x, start_w: w });
            }
            // On mouse-down (`TBV:400-406`), below the button.
            BeltHit::Hamburger => self.show_toolbelt_menu(MenuAt { x: l.hamburger.x, y: l.hamburger.y + l.hamburger.h, above: false }),
            BeltHit::Close(i) => {
                if let Some(&tool) = self.belt.split.tools.get(i) {
                    if self.belt.split.tools.len() == 1 {
                        self.hide_toolbelt_everywhere();
                    } else {
                        self.toggle_toolbelt_tool(tool);
                    }
                }
            }
            BeltHit::Divider(i) => self.drag = Drag::Belt(BeltDrag::Divider(i)),
            BeltHit::Content(i) => {
                if let (Some(&tool), Some(area)) = (self.belt.split.tools.get(i), l.wrappers.get(i).and_then(|w| w.content)) {
                    self.tool_press(tool, area, button, x, y, event_loop);
                }
            }
            BeltHit::Title(_) | BeltHit::Nothing => {}
        }
        self.redraw();
        true
    }

    /// Pointer motion during a toolbelt drag.
    pub(super) fn toolbelt_move(&mut self, x: f32, y: f32) {
        let Drag::Belt(d) = self.drag else { return };
        let scale = self.scale();
        match d {
            // Dragging left widens it (`TBV:965-970`); the panes follow live, the redraw laying
            // out the changed room (`growToolbeltBy:`).
            BeltDrag::Width { start_x, start_w } => {
                self.belt.width = toolbelt::clamp_width(start_w + (start_x - x) / scale, self.window_room().w / scale);
            }
            BeltDrag::Divider(i) => {
                let Some(panel) = self.toolbelt_panel() else { return };
                if i + 1 < self.belt.split.tools.len() {
                    self.belt.split = tbl::drag_divider(&self.belt.split, i, (y - panel.y) / scale);
                    self.belt_drop_hidden_focus();
                }
            }
            // The drop line follows the pointer.
            BeltDrag::Row { .. } => {}
            BeltDrag::Note => self.notes_drag(x, y),
            BeltDrag::Codecierge => self.codecierge_drag(x, y),
        }
        self.redraw();
    }

    pub(super) fn toolbelt_release(&mut self, d: BeltDrag) {
        match d {
            // The last move's redraw lays the panes out.
            BeltDrag::Width { .. } => {}
            // The heights become every new window's (`TBV:949-955`).
            BeltDrag::Divider(_) => {
                self.globals.set(toolbelt::PROPORTIONS_KEY, tbl::proportions_value(&self.belt.split));
                self.save_globals();
            }
            BeltDrag::Row { tool, start_y } => self.tool_drop(tool, start_y),
            BeltDrag::Note | BeltDrag::Codecierge => {}
        }
        self.redraw();
    }

    /// The wheel over the toolbelt stays there (its lists scroll; the terminal does not).
    pub(super) fn toolbelt_wheel(&mut self, dy: f32) -> bool {
        let (x, y) = self.cursor;
        if !self.toolbelt_panel().is_some_and(|p| p.contains(x, y)) {
            return false;
        }
        self.tool_wheel(dy);
        true
    }

    /// The arrangement keys for this window (`PT:4579-4589`).
    pub(super) fn toolbelt_arranged(&self) -> Arranged {
        Arranged {
            has: self.belt.visible,
            hiding_resizes: Some(self.belt.hiding_resizes),
            proportions: (!self.belt.split.tools.is_empty()).then(|| tbl::proportions_value(&self.belt.split)),
            states: self.belt.states.clone(),
        }
    }

    /// An arrangement's toolbelt (`PT:4196-4202`): shown or hidden without growing the window,
    /// its resize flag as saved (absent reads as false, as `boolValue` of nil does), its
    /// proportions, and its per-tool states kept for the next save.
    pub(super) fn restore_toolbelt(&mut self, a: &Arranged) {
        self.belt.states = a.states.clone();
        self.belt.hiding_resizes = a.hiding_resizes.unwrap_or(false);
        if a.has {
            self.belt_first_show();
        }
        self.belt.visible = a.has;
        let tools = self.toolbelt_tools();
        let total = split_total(self.window_room().h, tools.len(), self.scale());
        let before: f32 = self.belt.split.heights.iter().sum();
        let mut split = tbl::reconcile(&self.belt.split, &tools, total);
        if let Some(s) = a.proportions.as_ref().and_then(|p| tbl::apply_proportions(&split, p, total)) {
            split = s;
            self.belt.proportions_tried = true;
        }
        self.belt.split = tbl::resize(&split, before, total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbelt_menu_ids_round_trip() {
        assert_eq!(tool_menu_id(ToolId::PasteHistory.name()), "toolbelt:tool:Paste History");
        for tool in ToolId::ALL {
            assert_eq!(tool_name_from_menu_id(&tool_menu_id(tool.name())), Some(tool.name()));
        }
        assert_eq!(tool_name_from_menu_id("toolbelt:tool:My Web Tool"), Some("My Web Tool"));
        assert_eq!(tool_name_from_menu_id("Paste History"), None);
    }

    /// 13x Task 2: RegisterTool's reload and rename (`TBV:154-186`) as the views see them.
    #[test]
    fn tool_reload_follows_iterm2() {
        use super::tool_web_ui::{plan, Op, Wanted};
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<String>>();
        let (reg, names) = toolbelt::register_dynamic(None, &s(&["Profiles"]), "id1", "Web", "https://a", false);
        let reg = reg.unwrap();
        // The same name and URL again: the registry does not change, and the shown view reloads.
        assert_eq!(toolbelt::register_dynamic(Some(&reg), &names, "id1", "Web", "https://a", false).0, None);
        assert!(toolbelt::registration_reloads(Some(&reg), "id1", "Web", "https://a"));
        // The registry maps the identifier to the tool the view was made for.
        assert_eq!(toolbelt::dynamic_tools(Some(&reg)).iter().find(|d| d.identifier == "id1").map(|d| d.name.as_str()), Some("Web"));
        // A new name: the view of the old name closes and the new name's view is made.
        let (reg2, names2) = toolbelt::register_dynamic(Some(&reg), &names, "id1", "Web 2", "https://a", false);
        let d2 = toolbelt::dynamic_tools(reg2.as_ref());
        let content = Some(crate::layout::Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 });
        let have: std::collections::HashMap<String, String> = [("Web".to_string(), "https://a".to_string())].into();
        let wanted: Vec<Wanted> = toolbelt::shown_tools(&names2, &d2).into_iter().filter_map(|t| match t {
            ToolId::Dynamic(n) => d2.iter().find(|d| d.name == n).map(|d| Wanted { name: d.name.clone(), url: d.url.clone(), content }),
            _ => None,
        }).collect();
        let ops = plan(&wanted, &have, true);
        assert!(ops.contains(&Op::Close("Web".into())), "{ops:?}");
        assert!(ops.iter().any(|o| matches!(o, Op::Create { name, .. } if name == "Web 2")), "{ops:?}");
    }

    #[test]
    fn empty_label_wraps_at_spaces() {
        let w = |s: &str| s.chars().count() as f32;
        assert_eq!(wrap_words("No Tools enabled. Select them", 12.0, &w), vec!["No Tools", "enabled.", "Select them"]);
        assert_eq!(wrap_words("unbreakable", 4.0, &w), vec!["unbreakable"]);
        assert!(wrap_words("", 4.0, &w).is_empty());
    }
}
