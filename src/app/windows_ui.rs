//! Many terminal windows. The current window's state lives in `App`'s own fields; every
//! other window's state waits in a `WinSlot`, and `enter` swaps a window in before its
//! events run or its frame is drawn.

use super::*;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Fullscreen;

/// What belongs to one window.
pub(super) struct WinSlot {
    pub(super) gpu: Option<Gpu>,
    pub(super) tabs: Vec<Tab>,
    active: usize,
    menu_bar: Option<menu::MenuBar>,
    cursor: (f32, f32),
    drag: Drag,
    pointer_down: Option<PointerDown>,
    hover: Option<(PaneId, crate::find::Match)>,
    find_bar_rect: Option<Rect>,
    find_mode_rect: Option<Rect>,
    /// The title bars drawn last frame (pane, geometry), for clicks.
    title_bars: Vec<(PaneId, crate::pane_title::Geometry)>,
    title_press: Option<super::pane_title_ui::TitlePress>,
    /// The tab bar as last laid out, in points, not scrolled (`build_frame`).
    tab_bar: crate::tab_layout::Bar,
    /// The tab bar's scroll offset in points.
    tab_scroll: f32,
    /// The tab under the pointer, and whether the pointer is on its close button.
    tab_hover: Option<(usize, bool)>,
    /// Every layout brings the selected tab into view, until the bar is scrolled by hand.
    tab_keep_selected: bool,
    /// The selected tab's id and the tab count at the last layout: a change selects anew.
    tab_seen: Option<(u64, usize)>,
    /// The press on the tab bar: what it hit, its click count, Alt, and the tab's id.
    tab_press: Option<super::TabPress>,
    /// `SelectsTabsOnMouseDown` as it was when the window opened.
    tab_selects_on_down: bool,
    /// A side tab bar's preferred width in points.
    side_bar_preferred: f32,
    /// A side tab bar's scroll offset in points.
    side_bar_scroll: f32,
    /// The tab bar's flash in full screen.
    pub(super) tab_flash: crate::tab_flash::Flash,
    /// The window kept its size when the tab bar showed, to stay on screen (`PT _excursionPrevented`).
    excursion_prevented: bool,
    /// The tab bar took room from the sessions at the last layout, full screen aside.
    bar_was_visible: bool,
    /// How much the window grew for the tab bar when it last showed (pixels).
    bar_grown: (f32, f32),
    /// A side tab bar's group chips as last drawn (pixels, window coordinates) and their ids.
    side_chips: Vec<(Rect, String)>,
    /// The slot a dragged tab would drop at in this window's bar (the drop marker).
    tab_drop_marker: Option<usize>,
    /// The tab bar and each tab's rect as last drawn (pixels, window coordinates): where a
    /// tab dragged from another window drops (`window_bar_hit`).
    pub(super) tab_bar_drawn: Option<(Rect, Vec<Rect>, Vec<crate::tab_drag::Pill>)>,
    /// The pointer is on a side bar's drag handle (the resize pointer shows).
    side_handle_hover: bool,
    /// The tab whose tooltip the pointer is waiting on or showing, and when it settled there.
    tab_tip: Option<(usize, Instant)>,
    block_buttons: Vec<blocks_ui::BlockButton>,
    block_hover: Option<(PaneId, u32, i32, i32)>,
    porthole_hits: Vec<porthole_ui::PortholeHit>,
    /// The session picker's buttons this window drew (`chat_session_app`).
    picker_hits: Vec<(PaneId, Rect)>,
    /// The inline chat panels' rectangles this window drew (`inline_chat_ui`).
    inline_hits: Vec<inline_chat_ui::PanelHit>,
    /// The AI indicators' rectangles this window drew, for a press on them (`chat_session_app`).
    chat_indicator_hits: Vec<(PaneId, Rect, crate::scene::IndicatorKind)>,
    focused: bool,
    font_sig: String,
    pub(super) win_style: i64,
    win_slot: Option<(String, u32)>,
    win_locks: crate::winmodel::Locks,
    pub(super) win_title: Option<String>,
    window_menu_text: (String, String),
    border_applied: Option<u32>,
    theme_applied: Option<bool>,
    menu_hidden: bool,
    caption_hover: Option<crate::caption::Button>,
    caption_pressed: Option<crate::caption::Button>,
    caption_label_w: f32,
    caption_name_w: f32,
    zoom_restore: Option<Rect>,
    /// Which kind of surface the window was made with (decided at creation, `transparency::SurfaceKind`).
    pub(super) win_alpha_surface: crate::transparency::SurfaceKind,
    /// The window's Use Transparency toggle and its full screen bookkeeping.
    pub(super) win_transparency: crate::transparency::WinTransparency,
    /// Making this window an alpha window failed once: it stays opaque and is not tried again.
    pub(super) win_alpha_failed: bool,
    /// The window follows the current desktop (its profile's Space is All Desktops).
    pub(super) win_follow_desktop: bool,
    /// The monitor this window's profile anchors it to (`placement::anchor`).
    pub(super) win_anchor: Option<usize>,
    /// DWM was last told this window's backdrop is blurred (the frame is extended).
    pub(super) blur_applied: bool,
    /// The taskbar and Alt+Tab look `presence_tick` last applied (winit's skip is set on a change only).
    pub(super) look_seen: Option<crate::presence::Look>,
    mru: Mru,
    mru_mods: Option<Mods>,
    laid_out: Option<crate::statusbar_layout::Room>,
    /// Set on a profile's hotkey window.
    pub(super) hotkey: Option<super::hotkey_ui::HotkeyWin>,
    broadcast: crate::broadcast::Broadcast,
    broadcast_tabs: std::collections::BTreeSet<u64>,
    belt: super::toolbelt_ui::Belt,
    /// Instant Replay's panel, and the tab and pane it last saw in front.
    replay_window: Option<crate::replay_window::ReplayWindow>,
    replay_seen: Option<(u64, PaneId)>,
    /// The password manager open over this window.
    pwm_window: Option<super::pwm_app::PwmWin>,
    /// What Shell > ssh > Disconnect last showed: the focused session's identity, None for none
    /// (the outer None: not shown yet). Per window, since each window has its own menu bar.
    ssh_menu_shown: Option<Option<String>>,
    /// What Shell > ssh > Download Files last showed in this window (P21).
    download_files_shown: Option<bool>,
    /// Window > Notify on Status Change is armed for this window (`PseudoTerminal.m:1350`).
    pub(super) status_armed: bool,
}

impl WinSlot {
    /// Its Lock Size and Lock Layout.
    pub(super) fn locks(&self) -> crate::winmodel::Locks {
        self.win_locks
    }

    /// The tab it has in front.
    pub(super) fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub(super) fn new(gpu: Gpu) -> WinSlot {
        WinSlot {
            gpu: Some(gpu),
            tabs: Vec::new(),
            active: 0,
            menu_bar: None,
            cursor: (0.0, 0.0),
            drag: Drag::None,
            pointer_down: None,
            hover: None,
            find_bar_rect: None,
            find_mode_rect: None,
            title_bars: Vec::new(),
            title_press: None,
            tab_bar: crate::tab_layout::Bar::default(),
            tab_scroll: 0.0,
            tab_hover: None,
            tab_keep_selected: true,
            tab_seen: None,
            tab_press: None,
            tab_selects_on_down: true,
            side_bar_preferred: crate::tab_position::DEFAULT_SIDE_WIDTH,
            side_bar_scroll: 0.0,
            tab_flash: crate::tab_flash::Flash::new(),
            excursion_prevented: false,
            bar_was_visible: false,
            bar_grown: (0.0, 0.0),
            side_chips: Vec::new(),
            tab_drop_marker: None,
            tab_bar_drawn: None,
            side_handle_hover: false,
            tab_tip: None,
            block_buttons: Vec::new(),
            block_hover: None,
            porthole_hits: Vec::new(),
            picker_hits: Vec::new(),
            inline_hits: Vec::new(),
            chat_indicator_hits: Vec::new(),
            focused: false,
            font_sig: String::new(),
            win_style: 0,
            win_slot: None,
            win_locks: Default::default(),
            win_title: None,
            window_menu_text: Default::default(),
            border_applied: None,
            theme_applied: None,
            menu_hidden: false,
            caption_hover: None,
            caption_pressed: None,
            caption_label_w: 0.0,
            caption_name_w: 0.0,
            zoom_restore: None,
            win_alpha_surface: Default::default(),
            win_transparency: crate::transparency::WinTransparency::new(true),
            win_alpha_failed: false,
            win_follow_desktop: false,
            win_anchor: None,
            blur_applied: false,
            look_seen: None,
            mru: Mru::default(),
            mru_mods: None,
            laid_out: None,
            hotkey: None,
            broadcast: Default::default(),
            broadcast_tabs: Default::default(),
            belt: Default::default(),
            replay_window: None,
            replay_seen: None,
            pwm_window: None,
            ssh_menu_shown: None,
            download_files_shown: None,
            status_armed: false,
        }
    }
}

/// The windows whose Notify on Status Change is armed: the current one (its key and arm) and
/// each parked one (`status_changes`).
pub(super) fn armed_windows(current: Option<(u64, bool)>, parked: &[(u64, bool)]) -> Vec<u64> {
    current.into_iter().chain(parked.iter().copied()).filter(|(_, on)| *on).map(|(k, _)| k).collect()
}

/// The window id as the registry keys it.
pub(super) fn key(id: WindowId) -> u64 {
    u64::from(id)
}

/// A tab that leaves a window for another: its id leaves the window's broadcasting tabs and
/// its panes the window's broadcast domains and sources (ReorderTabs across windows, Move Tab
/// to New Window, Merge All Windows).
fn leave_broadcast(broadcast: &mut crate::broadcast::Broadcast, broadcast_tabs: &mut std::collections::BTreeSet<u64>, tab: &Tab) {
    broadcast_tabs.remove(&tab.id);
    broadcast.forget(&tab.layout.panes());
}

impl App {
    /// Takes the tab at `at` out of the current window to move it to another: its replays
    /// end, the window's MRU forgets it, and it leaves the window's broadcast settings. The
    /// caller chooses the new active tab.
    pub(super) fn take_tab(&mut self, at: usize) -> Tab {
        let panes = self.tabs[at].layout.panes();
        self.end_replays(&panes);
        let tab = self.tabs.remove(at);
        self.mru.forget(tab.id);
        leave_broadcast(&mut self.broadcast, &mut self.broadcast_tabs, &tab);
        tab
    }

    /// Swaps a slot's fields with the live ones.
    fn swap_live(&mut self, s: &mut WinSlot) {
        use std::mem::swap;
        swap(&mut self.gpu, &mut s.gpu);
        swap(&mut self.tabs, &mut s.tabs);
        swap(&mut self.active, &mut s.active);
        swap(&mut self.menu_bar, &mut s.menu_bar);
        swap(&mut self.cursor, &mut s.cursor);
        swap(&mut self.drag, &mut s.drag);
        swap(&mut self.pointer_down, &mut s.pointer_down);
        swap(&mut self.hover, &mut s.hover);
        swap(&mut self.find_bar_rect, &mut s.find_bar_rect);
        swap(&mut self.find_mode_rect, &mut s.find_mode_rect);
        swap(&mut self.title_bars, &mut s.title_bars);
        swap(&mut self.title_press, &mut s.title_press);
        swap(&mut self.tab_bar, &mut s.tab_bar);
        swap(&mut self.tab_scroll, &mut s.tab_scroll);
        swap(&mut self.tab_hover, &mut s.tab_hover);
        swap(&mut self.tab_keep_selected, &mut s.tab_keep_selected);
        swap(&mut self.tab_seen, &mut s.tab_seen);
        swap(&mut self.tab_press, &mut s.tab_press);
        swap(&mut self.tab_selects_on_down, &mut s.tab_selects_on_down);
        swap(&mut self.side_bar_preferred, &mut s.side_bar_preferred);
        swap(&mut self.side_bar_scroll, &mut s.side_bar_scroll);
        swap(&mut self.tab_flash, &mut s.tab_flash);
        swap(&mut self.excursion_prevented, &mut s.excursion_prevented);
        swap(&mut self.bar_was_visible, &mut s.bar_was_visible);
        swap(&mut self.bar_grown, &mut s.bar_grown);
        swap(&mut self.side_chips, &mut s.side_chips);
        swap(&mut self.tab_drop_marker, &mut s.tab_drop_marker);
        swap(&mut self.tab_bar_drawn, &mut s.tab_bar_drawn);
        swap(&mut self.side_handle_hover, &mut s.side_handle_hover);
        swap(&mut self.tab_tip, &mut s.tab_tip);
        swap(&mut self.block_buttons, &mut s.block_buttons);
        swap(&mut self.block_hover, &mut s.block_hover);
        swap(&mut self.porthole_hits, &mut s.porthole_hits);
        swap(&mut self.picker_hits, &mut s.picker_hits);
        swap(&mut self.inline_hits, &mut s.inline_hits);
        swap(&mut self.chat_indicator_hits, &mut s.chat_indicator_hits);
        swap(&mut self.focused, &mut s.focused);
        swap(&mut self.font_sig, &mut s.font_sig);
        swap(&mut self.win_style, &mut s.win_style);
        swap(&mut self.win_slot, &mut s.win_slot);
        swap(&mut self.win_locks, &mut s.win_locks);
        swap(&mut self.win_title, &mut s.win_title);
        swap(&mut self.window_menu_text, &mut s.window_menu_text);
        swap(&mut self.border_applied, &mut s.border_applied);
        swap(&mut self.theme_applied, &mut s.theme_applied);
        swap(&mut self.menu_hidden, &mut s.menu_hidden);
        swap(&mut self.caption_hover, &mut s.caption_hover);
        swap(&mut self.caption_pressed, &mut s.caption_pressed);
        swap(&mut self.caption_label_w, &mut s.caption_label_w);
        swap(&mut self.caption_name_w, &mut s.caption_name_w);
        swap(&mut self.zoom_restore, &mut s.zoom_restore);
        swap(&mut self.win_alpha_surface, &mut s.win_alpha_surface);
        swap(&mut self.win_transparency, &mut s.win_transparency);
        swap(&mut self.win_alpha_failed, &mut s.win_alpha_failed);
        swap(&mut self.win_follow_desktop, &mut s.win_follow_desktop);
        swap(&mut self.win_anchor, &mut s.win_anchor);
        swap(&mut self.blur_applied, &mut s.blur_applied);
        swap(&mut self.look_seen, &mut s.look_seen);
        swap(&mut self.mru, &mut s.mru);
        swap(&mut self.mru_mods, &mut s.mru_mods);
        swap(&mut self.laid_out, &mut s.laid_out);
        swap(&mut self.hotkey, &mut s.hotkey);
        swap(&mut self.broadcast, &mut s.broadcast);
        swap(&mut self.broadcast_tabs, &mut s.broadcast_tabs);
        swap(&mut self.belt, &mut s.belt);
        swap(&mut self.replay_window, &mut s.replay_window);
        swap(&mut self.replay_seen, &mut s.replay_seen);
        swap(&mut self.pwm_window, &mut s.pwm_window);
        swap(&mut self.ssh_menu_shown, &mut s.ssh_menu_shown);
        swap(&mut self.download_files_shown, &mut s.download_files_shown);
        swap(&mut self.status_armed, &mut s.status_armed);
    }

    /// The terminal window whose password manager is `id`, when it is parked.
    pub(super) fn window_of_pwm(&self, id: WindowId) -> Option<WindowId> {
        self.windows.parked().find(|(_, s)| s.pwm_window.as_ref().is_some_and(|w| w.id() == id)).and_then(|(_, s)| s.gpu.as_ref().map(|g| g.window.id()))
    }

    /// Every terminal window with a password manager open, the current one first.
    pub(super) fn pwm_owners(&self) -> Vec<WindowId> {
        let current = self.pwm_window.as_ref().and(self.current_window());
        let parked = self.windows.parked().filter(|(_, s)| s.pwm_window.is_some()).filter_map(|(_, s)| s.gpu.as_ref().map(|g| g.window.id()));
        current.into_iter().chain(parked).collect()
    }

    /// Every open password manager, the current window's first.
    pub(super) fn pwm_windows(&self) -> Vec<&super::pwm_app::PwmWin> {
        self.pwm_window.iter().chain(self.windows.parked().filter_map(|(_, s)| s.pwm_window.as_ref())).collect()
    }

    /// The password manager window whose Win32 handle is `hwnd`, and its terminal window.
    pub(super) fn pwm_of_hwnd(&self, hwnd: isize) -> Option<WindowId> {
        if self.pwm_window.as_ref().is_some_and(|w| w.hwnd() == Some(hwnd)) {
            return self.current_window();
        }
        self.windows.parked().find(|(_, s)| s.pwm_window.as_ref().is_some_and(|w| w.hwnd() == Some(hwnd))).and_then(|(_, s)| s.gpu.as_ref().map(|g| g.window.id()))
    }

    /// The terminal window whose Instant Replay panel is `panel`, when it is parked.
    pub(super) fn window_of_replay_panel(&self, panel: WindowId) -> Option<WindowId> {
        self.windows.parked().find(|(_, s)| s.replay_window.as_ref().is_some_and(|w| w.id() == panel)).and_then(|(_, s)| s.gpu.as_ref().map(|g| g.window.id()))
    }

    /// Every window's Instant Replay panel hides while Tandem is not the active app
    /// (`hidesOnDeactivate`, `IRX:23`).
    pub(super) fn replay_panels_follow_app(&mut self) {
        let fg = super::hotkey_ui::foreground();
        let active = fg != 0 && super::hotkey_ui::ours(fg);
        if let Some(w) = self.replay_window.as_mut() {
            w.set_shown(active);
        }
        for (_, s) in self.windows.parked_mut() {
            if let Some(w) = s.replay_window.as_mut() {
                w.set_shown(active);
            }
        }
    }

    /// Makes a terminal window current. False for a window that is not one.
    pub(super) fn enter(&mut self, id: WindowId) -> bool {
        let k = key(id);
        if self.windows.current() == Some(k) {
            return true;
        }
        // The live fields go into a slot, `Parked::enter` swaps the window in, and the slot's
        // fields come back live.
        let mut live = WinSlot::new_empty();
        self.swap_live(&mut live);
        let entered = self.windows.enter(k, &mut live);
        self.swap_live(&mut live);
        entered
    }

    /// Leaves no window current (after walking the windows from a state with none current):
    /// the current one's state is parked and the live fields are empty again.
    pub(super) fn leave_windows(&mut self) {
        if self.windows.current().is_none() {
            return;
        }
        let mut live = WinSlot::new_empty();
        self.swap_live(&mut live);
        self.windows.leave(live);
    }

    /// The id of the window that shows `pane`, the current one or a parked one.
    pub(super) fn pane_window_id(&self, pane: PaneId) -> Option<WindowId> {
        if self.tabs.iter().any(|t| t.layout.panes().contains(&pane)) {
            return self.current_window();
        }
        self.windows.parked().find(|(_, s)| s.tabs.iter().any(|t| t.layout.panes().contains(&pane))).and_then(|(_, s)| s.gpu.as_ref().map(|g| g.window.id()))
    }

    /// Lock Layout of the window that shows `pane` forbids `op` (false for a pane no window shows).
    pub(super) fn pane_window_forbids(&self, pane: PaneId, op: crate::winmodel::LayoutOp) -> bool {
        if self.tabs.iter().any(|t| t.layout.panes().contains(&pane)) {
            return self.win_locks.forbids(op);
        }
        self.windows.parked().find(|(_, s)| s.tabs.iter().any(|t| t.layout.panes().contains(&pane))).is_some_and(|(_, s)| s.win_locks.forbids(op))
    }

    /// Makes the window that shows `pane` current. False when no window shows it.
    pub(super) fn enter_pane_window(&mut self, pane: PaneId) -> bool {
        if self.tabs.iter().any(|t| t.layout.panes().contains(&pane)) {
            return true;
        }
        let found = self.windows.parked().find(|(_, s)| s.tabs.iter().any(|t| t.layout.panes().contains(&pane))).map(|(k, _)| *k);
        let Some(k) = found else { return false };
        let id = self.windows.parked().find(|(kk, _)| **kk == k).and_then(|(_, s)| s.gpu.as_ref().map(|g| g.window.id()));
        id.is_some_and(|id| self.enter(id))
    }

    /// Every window's menu bar: the current window's and each parked window's.
    pub(super) fn menu_bars(&self) -> impl Iterator<Item = &menu::MenuBar> {
        self.menu_bar.iter().chain(self.windows.parked().filter_map(|(_, s)| s.menu_bar.as_ref()))
    }

    pub(super) fn menu_bars_mut(&mut self) -> impl Iterator<Item = &mut menu::MenuBar> {
        self.menu_bar.iter_mut().chain(self.windows.parked_mut().filter_map(|(_, s)| s.menu_bar.as_mut()))
    }

    /// Every terminal window for the Window menu: (registry key, number, its title as the OS
    /// shows it without the label, its title without the number), by number. A window not
    /// drawn yet is "Untitled".
    pub(super) fn window_menu_entries(&self) -> Vec<(u64, usize, String, String)> {
        let current = self.windows.current();
        self.registry
            .order()
            .into_iter()
            .filter_map(|k| {
                let texts = if current == Some(k) { Some(&self.window_menu_text) } else { self.windows.parked().find(|(kk, _)| **kk == k).map(|(_, s)| &s.window_menu_text) }?;
                let (text, title) = if texts.0.is_empty() { (crate::wintitle::UNTITLED.to_string(), crate::wintitle::UNTITLED.to_string()) } else { texts.clone() };
                Some((k, self.registry.number(k)?, text, title))
            })
            .collect()
    }

    /// The current window's id.
    pub(super) fn current_window(&self) -> Option<WindowId> {
        self.gpu.as_ref().map(|g| g.window.id())
    }

    /// The tab each window other than the current one has in front, for windows on screen
    /// (not minimized, not a hidden hotkey window).
    pub(super) fn parked_front_tabs(&self) -> Vec<&Tab> {
        self.parked_fronts().into_iter().map(|(_, t)| t).collect()
    }

    /// Those windows, each with the tab it has in front.
    pub(super) fn parked_fronts(&self) -> Vec<(&Window, &Tab)> {
        let on_screen = |w: &Window| w.is_visible() != Some(false) && w.is_minimized() != Some(true);
        self.windows.parked().filter_map(|(_, s)| s.gpu.as_ref().filter(|g| on_screen(&g.window)).and_then(|g| s.tabs.get(s.active).map(|t| (&*g.window, t)))).collect()
    }

    /// The tab each window other than the current one has in front, minimized and hidden
    /// windows too.
    pub(super) fn parked_front_tabs_all(&self) -> Vec<&Tab> {
        self.windows.parked().filter_map(|(_, s)| s.tabs.get(s.active)).collect()
    }

    /// Asks every window to draw.
    pub(super) fn redraw_all(&self) {
        for (_, s) in self.windows.parked() {
            if let Some(g) = &s.gpu {
                g.window.request_redraw();
            }
        }
    }

    /// Opens a terminal window with one tab of `guid` (the default profile when None).
    pub(super) fn open_window(&mut self, event_loop: &ActiveEventLoop, guid: Option<&str>) -> Result<WindowId, String> {
        self.open_window_with(event_loop, NewWindow::Profile(guid.map(str::to_string)))
    }

    /// The current window's title from its first tab's profile (iTerm2 keeps the initial
    /// profile's custom window title, `PT:486-491`, `:887-890`): that session's profile, else
    /// `guid`'s, else the default profile.
    pub(super) fn apply_custom_window_title(&mut self, guid: Option<&str>) {
        let session = self.tabs.first().and_then(|t| self.panes.get(&t.focused)).and_then(|p| self.store.get(&p.profile));
        let profile = session.or_else(|| guid.and_then(|g| self.store.get(g))).unwrap_or_else(|| self.store.default_profile());
        self.win_title = crate::wintitle::custom_window_format(profile.bool("Use Custom Window Title"), &profile.string("Custom Window Title"));
    }

    fn open_window_with(&mut self, event_loop: &ActiveEventLoop, first: NewWindow) -> Result<WindowId, String> {
        let hotkey = matches!(first, NewWindow::Hotkey(..) | NewWindow::HotkeyRestored(..));
        let mut attrs = Window::default_attributes().with_title("Tandem").with_inner_size(LogicalSize::new(1000.0, 650.0));
        if hotkey {
            // A hotkey window starts hidden and has no taskbar button (a Quake-style window).
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs = attrs.with_visible(false).with_skip_taskbar(true);
        }
        // A window whose profiles want transparency is made for alpha (its surface cannot change later).
        let mut kind = self.surface_kind_for(&first);
        let made = loop {
            let mut a = attrs.clone();
            if kind == crate::transparency::SurfaceKind::Alpha {
                use winit::platform::windows::WindowAttributesExtWindows;
                a = a.with_no_redirection_bitmap(true);
            }
            let made = event_loop.create_window(a).map_err(|e| e.to_string()).and_then(|w| self.gpu_for(event_loop, Arc::new(w), kind));
            // No second instance on this machine: the window is made again, opaque.
            if made.is_err() && kind == crate::transparency::SurfaceKind::Alpha {
                self.transparency_unavailable = true;
                kind = crate::transparency::SurfaceKind::Opaque;
                continue;
            }
            break made;
        };
        let gpu = match made {
            Ok(g) => g,
            Err(e) => {
                // A tab on its way to the new window stays where it was.
                match first {
                    NewWindow::Tab(tab) | NewWindow::TornOff(tab, _) => self.tabs.push(tab),
                    NewWindow::Tabs(tabs) => self.tabs.extend(tabs),
                    NewWindow::Profile(_) | NewWindow::Arrangement(..) | NewWindow::Restored(_) | NewWindow::Orphans(_) | NewWindow::Hotkey(..) | NewWindow::HotkeyRestored(..) => {}
                }
                // Pinned or grouped tabs coming back keep the order's rules, and tmux hears
                // a changed order.
                if self.settle_tabs() {
                    self.tmux_reorder();
                }
                self.relayout();
                return Err(e);
            }
        };
        let previous = self.current_window();
        let id = gpu.window.id();
        let mut slot = WinSlot::new(gpu);
        slot.win_alpha_surface = kind;
        self.swap_live(&mut slot);
        // Edge drags snap to the cell grid, and follow AllowLiveResize.
        self.install_sizing();
        // A forced Theme's title bar survives system setting changes.
        self.guard_theme();
        if let Some(h) = self.hwnd() {
            crate::display_watch::watch(h);
        }
        // iTerm2 reads it once per tab bar it makes: a change affects new windows only.
        self.tab_selects_on_down = self.globals.bool("SelectsTabsOnMouseDown");
        // A new window's side bar starts at `LeftTabBarWidth`.
        self.side_bar_preferred = super::side_bar_default(&self.globals);
        if let Some(old) = self.windows.current() {
            self.windows.park(old, slot);
        }
        self.windows.start(key(id));
        self.registry.open(key(id));
        let first_guid = match &first {
            NewWindow::Profile(g) => g.clone(),
            NewWindow::Tab(t) | NewWindow::TornOff(t, _) => self.panes.get(&t.focused).map(|p| self.store.shared_guid(&p.profile)),
            NewWindow::Tabs(ts) => ts.first().and_then(|t| self.panes.get(&t.focused)).map(|p| self.store.shared_guid(&p.profile)),
            NewWindow::Arrangement(w, _) | NewWindow::Restored(w) => self.first_arrangement_guid(w),
            NewWindow::Orphans(_) => None,
            NewWindow::Hotkey(g, _) | NewWindow::HotkeyRestored(g, _) => Some(g.clone()),
        };
        // A new window's Use Transparency is the first profile's Initial Use Transparency (`PT:864`).
        let initial = crate::transparency::initial_use(first_guid.as_deref().and_then(|g| self.store.get(g)), self.store.default_profile());
        self.win_transparency = crate::transparency::WinTransparency::new(initial);
        self.apply_fonts();
        self.size_window(first_guid.as_deref());
        let style = first_guid.as_deref().and_then(|g| self.store.get(g)).unwrap_or_else(|| self.store.default_profile()).int("Window Type");
        // A git command's single-use window is never full screen (`iTermController.m`
        // `openSingleUseWindowWithCommand:` uses the default window type instead).
        let style = if self.start_command_override.is_some() && matches!(style, crate::frames::FULL_SCREEN | crate::frames::LION_FULL_SCREEN) { crate::frames::NORMAL } else { style };
        let applies_space = matches!(first, NewWindow::Profile(_) | NewWindow::Hotkey(..));
        let arranged = matches!(first, NewWindow::Arrangement(..) | NewWindow::Restored(_) | NewWindow::HotkeyRestored(..));
        let torn_at = match &first {
            NewWindow::TornOff(_, at) => Some(*at),
            _ => None,
        };
        match first {
            NewWindow::Profile(guid) => self.new_tab(guid.as_deref()),
            NewWindow::Tab(tab) | NewWindow::TornOff(tab, _) => {
                self.tabs.push(tab);
                self.active = 0;
                self.relayout();
            }
            NewWindow::Tabs(tabs) => {
                self.tabs.extend(tabs);
                self.active = 0;
                self.settle_tabs();
                self.relayout();
            }
            NewWindow::Arrangement(w, name) => {
                if let Err(e) = self.apply_arrangement_window(&w, name.as_deref(), false) {
                    self.banners.set("arrangement", e);
                }
            }
            NewWindow::Restored(w) => {
                if let Err(e) = self.apply_arrangement_window(&w, None, true) {
                    self.banners.set("arrangement", e);
                }
            }
            NewWindow::Orphans(sessions) => self.adopt_orphans(&sessions),
            NewWindow::Hotkey(guid, previous) => {
                self.new_tab(Some(&guid));
                let mut h = super::hotkey_ui::HotkeyWin::new(guid);
                h.previous = previous;
                self.hotkey = Some(h);
            }
            NewWindow::HotkeyRestored(guid, w) => {
                if let Err(e) = self.apply_arrangement_window(&w, None, true) {
                    self.banners.set("arrangement", e);
                }
                self.hotkey = Some(super::hotkey_ui::HotkeyWin::new(guid));
            }
        }
        // The first tab's profile may give the window a title (`PT:887-890`); an arrangement's
        // window keeps its own.
        if !arranged {
            self.apply_custom_window_title(first_guid.as_deref());
        }
        self.install_menus();
        // An arrangement's window has its own frame, style, and locks. A hotkey window's
        // frame is its style's (it is not placed like a new window).
        let restored_hotkey = hotkey && arranged;
        let steps = crate::winmodel::open_steps(arranged || restored_hotkey);
        if let Some((x, y)) = torn_at {
            // A torn-off tab's window has its top-left corner where the tab was dropped.
            self.claim_slot();
            if let Some(g) = &self.gpu {
                g.window.set_outer_position(PhysicalPosition::new(x, y));
            }
        } else if steps.place && !hotkey {
            self.place_window();
        } else {
            self.claim_slot();
        }
        if torn_at.is_some() || !(steps.place && !hotkey) {
            self.anchor_from_profile(first_guid.as_deref());
        }
        // The profile's Space puts a new window on a desktop (P8: a profile launch or a hotkey
        // window's first opening, never an arrangement, restoration, tab or orphan window).
        if applies_space {
            let space = first_guid.as_deref().and_then(|g| self.store.get(g)).unwrap_or_else(|| self.store.default_profile()).int("Space");
            self.apply_space(space);
        }
        if steps.profile_style && style != crate::frames::NORMAL {
            self.apply_window_style(style, false);
        }
        // After the style, so a Maximized or Full Screen style does not restore it again.
        if steps.profile_window && !hotkey {
            self.apply_window_profile(first_guid.as_deref());
        }
        // A restored window (hotkey ones too) keeps its arrangement's "Has Toolbelt".
        if !arranged && !self.tabs.is_empty() {
            self.open_toolbelt_on_first_tab(first_guid.as_deref());
        }
        if self.tabs.is_empty() {
            // The shell did not start: the window closes again (keeping no frame for its
            // slot), the one before comes back, and the banner says why.
            let why = self.banners.text().unwrap_or_default();
            if let Some((guid, slot)) = self.win_slot.take() {
                self.slots.give(&guid, slot);
            }
            self.drop_current_window();
            if let Some(p) = previous {
                self.enter(p);
            }
            return Err(why);
        }
        // Square corners are set once, when the window is made (iTerm2 needs a restart).
        self.apply_corners();
        self.apply_border();
        // A hotkey window made by its hotkey shows now; a restored one waits for the hotkey.
        if hotkey && !arranged {
            self.roll_in();
        }
        Ok(id)
    }

    /// The current window is gone (its last tab closed): another window becomes current.
    /// False when it was the last one.
    pub(super) fn drop_current_window(&mut self) -> bool {
        let Some(k) = self.windows.current() else { return false };
        self.save_window_frame();
        self.forget_window(k);
        if let Some(h) = self.hwnd() {
            crate::winkeys::remove_window(h);
        }
        let mut live = WinSlot::new_empty();
        self.swap_live(&mut live);
        // `close_current` drops the closed window (which closes the OS window) and swaps the
        // next one into `live`.
        let next = self.windows.close_current(&mut live);
        self.swap_live(&mut live);
        // A hotkey window, even hidden, keeps Tandem running (iTerm2).
        let hotkeys = self.hotkey_windows().len();
        let regular = self.registry.order().len().saturating_sub(hotkeys);
        if next.is_none() || !crate::hotkey::keeps_running(regular, hotkeys) {
            return false;
        }
        self.relayout();
        self.redraw();
        true
    }

    /// Closes one window and its sessions (Cmd+Shift+W, the close box). The last window
    /// closing ends Tandem.
    pub(super) fn close_window(&mut self, id: WindowId, event_loop: &ActiveEventLoop) {
        // Tandem's own confirmation first, then tmux's question (iTerm2's order).
        if !self.enter(id) || !self.confirm_close(close_ui::Closing::Window) || self.tmux_close_window() {
            return;
        }
        self.close_window_now(event_loop);
    }

    /// Closes the current window's sessions as one undoable close, asking nothing.
    pub(super) fn close_window_now(&mut self, event_loop: &ActiveEventLoop) {
        let focused = self.focused_id();
        self.replay_event(crate::replay::ReplayEvent::WindowClosed, focused, event_loop);
        let panes: Vec<PaneId> = self.tabs.iter().flat_map(|t| t.layout.panes()).collect();
        self.begin_undo(crate::undo_close::Kind::Window, &panes);
        for p in panes {
            self.close_pane(p, event_loop);
        }
        self.end_undo();
    }

    /// Close Tab for the tab `index` (the menu's Close Tab, a tab's close button, a middle
    /// click): a tmux tab asks tmux; any other asks the close confirmation, then closes.
    pub(super) fn close_tab_at(&mut self, index: usize, event_loop: &ActiveEventLoop) {
        if index >= self.tabs.len() || !self.layout_allowed(crate::winmodel::LayoutOp::CloseTab) || self.tmux_close_tab(index) {
            return;
        }
        if self.confirm_close(close_ui::Closing::Tab(index)) {
            self.close_tab_now(index, event_loop);
        }
    }

    /// Closes the current window's tab `index` and its sessions as one undoable close, asking
    /// nothing.
    pub(super) fn close_tab_now(&mut self, index: usize, event_loop: &ActiveEventLoop) {
        let Some(panes) = self.tabs.get(index).map(|t| t.layout.panes()) else { return };
        self.begin_undo(crate::undo_close::Kind::Tab, &panes);
        for id in panes {
            self.close_pane(id, event_loop);
        }
        self.end_undo();
    }

    /// A session the person closes: alone in its tab it closes as the tab.
    pub(super) fn close_session_undoable(&mut self, id: PaneId, event_loop: &ActiveEventLoop) {
        if self.tmux_kill_pane(id) {
            return;
        }
        let panes = self.tabs.iter().find(|t| t.layout.panes().contains(&id)).map(|t| t.layout.panes()).unwrap_or_default();
        let kind = if panes.len() > 1 { crate::undo_close::Kind::Session } else { crate::undo_close::Kind::Tab };
        self.begin_undo(kind, if kind == crate::undo_close::Kind::Session { std::slice::from_ref(&id) } else { &panes });
        self.close_pane(id, event_loop);
        self.end_undo();
    }

    pub(super) fn window_number(&self) -> usize {
        self.windows.current().and_then(|k| self.registry.number(k)).unwrap_or(0)
    }

    /// The current window's title before its number: the title rules, then the tmux client's
    /// name when the number shows (`PT:3105-3122`).
    pub(super) fn caption_base(&self) -> String {
        let focused = self.tabs.get(self.active).map(|t| t.focused);
        // The gate is the FORMAT being non-empty, not its evaluated text (`PT:13636-13639`).
        let custom = self.win_title.as_deref().filter(|f| !f.is_empty()).map(|f| self.window_title_text(f, focused));
        let title = crate::wintitle::window_title(
            custom.as_deref(),
            self.tab_bar_visible(),
            self.globals.bool("ShowWindowTitleWhenTabBarInvisible"),
            focused.map(|f| self.window_title(f)).as_deref(),
            focused.map(|f| self.session_title(f)).as_deref(),
        );
        let client = focused.and_then(|f| self.tmux_client_of(f));
        crate::tmux_windows::window_title(&title, self.globals.bool("WindowNumber"), self.globals.bool("tmuxIncludeClientNameInWindowTitle"), client.as_deref())
    }

    /// Whether the current window has a title bar to carry the number label: decorated, not
    /// full screen, and not a Compact or Minimal window (whose own caption draws the label,
    /// `compact_caption`).
    fn window_titled(&self) -> bool {
        let decorated = self.gpu.as_ref().is_some_and(|g| g.window.is_decorated());
        crate::wintitle::has_title_bar(decorated, self.full_screen(), self.compact_window())
    }

    /// Whether the current window is a Compact or Minimal window: its type as the Theme
    /// themes it (`frames::themed`) is one of the compact forms, and it is not full screen. Worked
    /// out here, never stored: `win_style` stays the unthemed type (arrangements, hotkey slides
    /// and snapping read it), so a Theme change needs no new type.
    pub(super) fn compact_window(&self) -> bool {
        !self.full_screen() && self.compact_type()
    }

    /// Whether the window's themed type is a compact form, full screen or not: the caption
    /// subclass follows this, not `compact_window` (in full screen winit's style has no title bar
    /// and the subclass leaves the frame alone, so a full screen round trip changes nothing).
    pub(super) fn compact_type(&self) -> bool {
        use crate::frames;
        frames::is_compact(frames::themed(self.win_style, crate::tab_theme::compact_theme(self.tab_style())))
    }

    /// The current window's OS caption (`PT setWindowTitle:subtitle:`): `caption_base` with the
    /// "N. " prefix or the shortcut label (R11). Also its Window menu texts: the caption as
    /// iTerm2's window title (the prefix, no label) and the title without the number.
    pub(super) fn caption_text(&self) -> (String, (String, String)) {
        use crate::wintitle::{caption, number_prefix};
        let base = self.caption_base();
        let show = self.globals.bool("WindowNumber");
        let accessory = self.globals.bool("UseShortcutAccessoryViewController");
        let titled = self.window_titled();
        let number = self.window_number();
        let shown = caption(&base, show, accessory, titled, self.globals.int("SwitchWindowModifier"), number);
        let text = format!("{}{base}", number_prefix(show, accessory, titled, number).unwrap_or_default());
        (shown, (text, base))
    }

    /// What a Compact or Minimal window's caption draws: its title, and the number label
    /// right-aligned (R11).
    pub(super) fn compact_caption(&self) -> (String, Option<String>) {
        let show = self.globals.bool("WindowNumber");
        let accessory = self.globals.bool("UseShortcutAccessoryViewController");
        crate::wintitle::compact_caption(&self.caption_base(), show, accessory, self.globals.int("SwitchWindowModifier"), self.window_number())
    }

    /// A window closed: its number and id go, and so do its API variables.
    fn forget_window(&mut self, k: u64) {
        if let Some(g) = self.registry.guid(k) {
            self.api_vars.forget(crate::api_vars::Scope::Window, g);
        }
        self.registry.close(k);
        self.refresh_window_list();
    }

    /// The window that shows `pane`, the current one or a parked one.
    pub(super) fn pane_window(&self, pane: PaneId) -> Option<PaneWindow<'_>> {
        let holding = |tabs: &[Tab]| tabs.iter().position(|t| t.layout.panes().contains(&pane));
        if let Some(tab) = holding(&self.tabs) {
            let key = self.windows.current()?;
            return Some(PaneWindow { key, tabs: &self.tabs, tab, front: self.tabs.get(self.active), title: self.win_title.as_deref(), hotkey: self.hotkey.is_some() });
        }
        self.windows
            .parked()
            .find_map(|(k, s)| holding(&s.tabs).map(|tab| PaneWindow { key: *k, tabs: &s.tabs, tab, front: s.active_tab(), title: s.win_title.as_deref(), hotkey: s.hotkey.is_some() }))
    }
}

/// What the variables need of a window: its registry key, its tabs, the index of the one
/// holding the pane, the one in front, its Edit Window Title text, and whether it is a hotkey
/// window.
pub(super) struct PaneWindow<'a> {
    pub(super) key: u64,
    pub(super) tabs: &'a [Tab],
    pub(super) tab: usize,
    pub(super) front: Option<&'a Tab>,
    pub(super) title: Option<&'a str>,
    pub(super) hotkey: bool,
}

impl WinSlot {
    pub(super) fn new_empty() -> WinSlot {
        WinSlot {
            gpu: None,
            tabs: Vec::new(),
            active: 0,
            menu_bar: None,
            cursor: (0.0, 0.0),
            drag: Drag::None,
            pointer_down: None,
            hover: None,
            find_bar_rect: None,
            find_mode_rect: None,
            title_bars: Vec::new(),
            title_press: None,
            tab_bar: crate::tab_layout::Bar::default(),
            tab_scroll: 0.0,
            tab_hover: None,
            tab_keep_selected: true,
            tab_seen: None,
            tab_press: None,
            tab_selects_on_down: true,
            side_bar_preferred: crate::tab_position::DEFAULT_SIDE_WIDTH,
            side_bar_scroll: 0.0,
            tab_flash: crate::tab_flash::Flash::new(),
            excursion_prevented: false,
            bar_was_visible: false,
            bar_grown: (0.0, 0.0),
            side_chips: Vec::new(),
            tab_drop_marker: None,
            tab_bar_drawn: None,
            side_handle_hover: false,
            tab_tip: None,
            block_buttons: Vec::new(),
            block_hover: None,
            porthole_hits: Vec::new(),
            picker_hits: Vec::new(),
            inline_hits: Vec::new(),
            chat_indicator_hits: Vec::new(),
            focused: false,
            font_sig: String::new(),
            win_style: 0,
            win_slot: None,
            win_locks: Default::default(),
            win_title: None,
            window_menu_text: Default::default(),
            border_applied: None,
            theme_applied: None,
            menu_hidden: false,
            caption_hover: None,
            caption_pressed: None,
            caption_label_w: 0.0,
            caption_name_w: 0.0,
            zoom_restore: None,
            win_alpha_surface: Default::default(),
            win_transparency: crate::transparency::WinTransparency::new(true),
            win_alpha_failed: false,
            win_follow_desktop: false,
            win_anchor: None,
            blur_applied: false,
            look_seen: None,
            mru: Mru::default(),
            mru_mods: None,
            laid_out: None,
            hotkey: None,
            broadcast: Default::default(),
            broadcast_tabs: Default::default(),
            belt: Default::default(),
            replay_window: None,
            replay_seen: None,
            pwm_window: None,
            ssh_menu_shown: None,
            download_files_shown: None,
            status_armed: false,
        }
    }
}

/// A window's surface and renderer on the first window's device.
pub(super) fn init_gpu_shared(window: Arc<Window>, from: &Gpu) -> Result<Gpu, String> {
    let surface = from.instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
    let caps = surface.get_capabilities(&from.adapter);
    let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
    let size = window.inner_size();
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: crate::transparency::pick_alpha_mode(&caps.alpha_modes, false),
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
        color_space: wgpu::SurfaceColorSpace::Auto,
    };
    surface.configure(&from.device, &config);
    let renderer = Renderer::new(&from.device, &from.queue, format, FONT, FONT_PX * window.scale_factor() as f32);
    Ok(Gpu { surface, adapter: from.adapter.clone(), device: from.device.clone(), queue: from.queue.clone(), config, renderer, instance: from.instance.clone(), alpha: None, window })
}

impl App {
    /// Window > New Window and New Window with Current Profile.
    pub(super) fn new_window(&mut self, event_loop: &ActiveEventLoop, guid: Option<&str>) {
        if let Err(e) = self.open_window(event_loop, guid) {
            self.banners.set("window", format!("Could not open a window: {e}"));
            self.redraw();
        }
    }
}

impl App {
    /// The Profiles menu lists every profile after its fixed items.
    pub(super) fn refresh_profiles_menu(&mut self) {
        let items: Vec<(String, String)> = self.store.profiles.iter().map(|p| (p.guid(), p.name())).collect();
        let Some(bar) = &mut self.menu_bar else { return };
        if bar.profile_items == items {
            return;
        }
        let Some((sub, fixed)) = &bar.profiles else { return };
        while sub.remove_at(*fixed).is_some() {}
        for (guid, name) in &items {
            let entry = muda::MenuItem::with_id(format!("{}{guid}", menu::PROFILE_ID), name.replace('&', "&&"), true, None);
            if let Err(e) = sub.append(&entry) {
                self.banners.set("menu", format!("Profiles menu: {e}"));
            }
        }
        bar.profile_items = items;
    }

    /// A profile picked in the Profiles menu: a tab, or a window with Alt held (swapped by
    /// openProfilesInNewWindow; a profile that refuses tabs always gets a window).
    pub(super) fn open_profile_from_menu(&mut self, guid: &str) {
        use crate::winmodel::{OpenRequest, Target, open_target};
        let option = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) } < 0;
        let prevent = self.store.get(guid).is_some_and(|p| p.bool("Prevent Opening in a Tab"));
        match open_target(OpenRequest::ProfileMenu { option }, prevent, self.globals.bool("openProfilesInNewWindow")) {
            Target::Tab => self.new_tab(Some(guid)),
            Target::Window => {
                self.pending_windows.push(NewWindow::Profile(Some(guid.to_string())));
                self.redraw();
            }
        }
    }
}

/// What a new window starts with.
pub(super) enum NewWindow {
    /// One tab of a profile (the default one when None).
    Profile(Option<String>),
    /// A tab moved from another window.
    Tab(Tab),
    /// A tab dragged out of every tab bar, and the screen point it was dropped at (the new
    /// window's top-left corner).
    TornOff(Tab, (i32, i32)),
    /// Tabs brought back by Undo Close.
    Tabs(Vec<Tab>),
    /// A window table from an arrangement, and the saved arrangement's name (None for
    /// window restoration, a file, or Duplicate Window).
    Arrangement(crate::arrangement::Value, Option<String>),
    /// A window table of the saved state, restored at launch: the only window whose panes may
    /// adopt the chat references they saved (`arrangement::reference_for`).
    Restored(crate::arrangement::Value),
    /// Host sessions nobody claimed at launch: (host session id, pid), a tab each.
    Orphans(Vec<(u64, u32)>),
    /// A profile's hotkey window, made on its first press (and the window that was in
    /// front before the press).
    Hotkey(String, Option<isize>),
    /// A hotkey window from the saved state: made hidden, shown by its hotkey.
    HotkeyRestored(String, crate::arrangement::Value),
}

impl App {
    pub(super) fn open_pending(&mut self, event_loop: &ActiveEventLoop, w: NewWindow) {
        if let Err(e) = self.open_window_with(event_loop, w) {
            self.banners.set("window", format!("Could not open a window: {e}"));
            self.redraw();
        }
    }

    pub(super) fn window_id_of_key(&self, k: u64) -> Option<WindowId> {
        self.window_of_key(k)
    }

    /// The window id of a registry key, current or parked.
    fn window_of_key(&self, k: u64) -> Option<WindowId> {
        if self.windows.current() == Some(k) {
            return self.current_window();
        }
        self.windows.parked().find(|(kk, _)| **kk == k).and_then(|(_, s)| s.gpu.as_ref().map(|g| g.window.id()))
    }

    /// Brings a window to the front and makes it current.
    pub(super) fn focus_terminal_window(&mut self, id: WindowId) {
        if !self.enter(id) {
            return;
        }
        self.bring_forward();
        self.last_window = Some(id);
    }

    /// Alt+Win+1 to 9.
    pub(super) fn select_window_number(&mut self, digit: u8) {
        let Some(k) = self.registry.switch_target(digit) else { return };
        let hidden = self.hidden_hotkey_key(k);
        if let Some(id) = self.window_of_key(k) {
            // A hidden hotkey window comes back the way its hotkey shows it (iTerm2).
            if hidden && self.enter(id) {
                self.roll_in();
            } else {
                self.focus_terminal_window(id);
            }
        }
    }

    /// Next Window and Previous Window: by number, wrapping.
    pub(super) fn step_window(&mut self, forward: bool) {
        let Some(k) = self.windows.current() else { return };
        // Hidden hotkey windows take no part, nor open ones when excluded from cycling.
        let mut at = k;
        let next = loop {
            let n = if forward { self.registry.next(at) } else { self.registry.previous(at) };
            match n {
                Some(n) if n != k && self.cycling_skips_key(n) => at = n,
                other => break other,
            }
        };
        if let Some(id) = next.filter(|n| *n != k).and_then(|n| self.window_of_key(n)) {
            self.focus_terminal_window(id);
        }
    }

    /// Window > Move Tab to New Window (not the window's only tab).
    pub(super) fn move_tab_to_new_window(&mut self) {
        self.move_tab_at_to_new_window(self.active);
    }

    /// Moves the tab at `at` to a new window (`PT it_moveTabToNewWindow:`, `PT:14593-14620`):
    /// not the window's only tab, nor one holding a loaded recording; it leaves its group, as
    /// a torn-off tab does (`ungroup:YES`). The selected tab stays selected, or its neighbor
    /// is, its group expanded.
    pub(super) fn move_tab_at_to_new_window(&mut self, at: usize) {
        if self.tabs.len() < 2 || self.tabs.get(at).is_none_or(|t| self.holds_recording(t)) {
            return;
        }
        let selected = self.tabs.get(self.active).map(|t| t.id);
        let mut tab = self.take_tab(at);
        tab.group = None;
        self.active = selected.and_then(|s| self.tabs.iter().position(|t| t.id == s)).unwrap_or(self.active.min(self.tabs.len() - 1));
        self.expand_selected_group();
        self.pending_windows.push(NewWindow::Tab(tab));
        self.relayout();
        self.redraw();
    }

    /// Session > Move Session > Move Session to Window: the focused pane leaves its tab
    /// for a new window (a one-pane tab moves whole).
    pub(super) fn move_session_to_window(&mut self) {
        let Some(pane) = self.tabs.get(self.active).map(|t| t.focused) else { return };
        // A loaded recording is not moved.
        if self.panes.get(&pane).is_some_and(|p| p.dvr.read_only) || self.tmux_break(pane, false, None, None) {
            return;
        }
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        if t.layout.panes().len() < 2 {
            return self.move_tab_to_new_window();
        }
        self.end_replays(&[pane]);
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        if t.layout.remove(pane) {
            t.focused = t.layout.panes()[0];
            t.zoomed = None;
        }
        let tab = Tab::new(self.next_tab_id, Layout::new(pane), pane);
        self.next_tab_id += 1;
        self.pending_windows.push(NewWindow::Tab(tab));
        self.relayout();
        self.redraw();
    }

    /// Session > Move Session > Move Session to Tab: the focused pane leaves its tab for a
    /// new tab of this window (only when the tab has other panes).
    pub(super) fn move_session_to_tab(&mut self) {
        let Some(pane) = self.tabs.get(self.active).map(|t| t.focused) else { return };
        if self.tmux_break(pane, true, None, None) {
            return;
        }
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        if t.layout.panes().len() < 2 || !t.layout.remove(pane) {
            return;
        }
        t.focused = t.layout.panes()[0];
        t.zoomed = None;
        let tab = Tab::new(self.next_tab_id, Layout::new(pane), pane);
        self.next_tab_id += 1;
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        self.relayout();
        self.redraw();
    }

    /// A pane dragged by its title bar onto a window's tab bar (`window`, slot `index`): it leaves
    /// its tab (which keeps its other panes) and becomes a tab of that window
    /// (`PseudoTerminal.m:8648-8677`, `MovePaneController.m:113-124`). The slot keeps the pinned
    /// zone and a group's run (P12). A window under Lock Layout takes no tab; a loaded recording
    /// never moves. A tmux pane breaks out through tmux into the destination window, next to the
    /// tab the slot names (`PseudoTerminal.m:8651-8659`); a window with no tab of its session
    /// refuses the drop.
    pub(super) fn move_session_to_new_tab(&mut self, pane: PaneId, window: WindowId, index: usize, event_loop: &ActiveEventLoop) {
        let _ = event_loop;
        let Some(source) = self.current_window() else { return };
        let Some(from) = self.tabs.iter().position(|t| t.layout.panes().contains(&pane)) else { return };
        if self.tabs[from].layout.panes().len() < 2 || self.panes.get(&pane).is_some_and(|p| p.dvr.read_only) {
            return;
        }
        if window != source && self.windows.parked().any(|(_, s)| s.gpu.as_ref().is_some_and(|g| g.window.id() == window) && s.locks().forbids(crate::winmodel::LayoutOp::AddTab)) {
            return;
        }
        if self.tmux_of(pane).is_some() {
            // The tab goes into the DESTINATION window, next to the tab the drop slot names
            // (`PseudoTerminal.m:8651-8659`); a window with no tab of this tmux session has no
            // place to put it, so the drop is refused.
            let Some((gw, ..)) = self.tmux_of(pane) else { return };
            let ids: Vec<Option<(PaneId, u32)>> = if window == source {
                self.tabs.iter().map(|t| self.tmux_of(t.focused).map(|(g, _, w)| (g, w))).collect()
            } else {
                self.windows.parked().find(|(_, s)| s.gpu.as_ref().is_some_and(|g| g.window.id() == window)).map(|(_, s)| s.tabs.iter().map(|t| self.tmux_of(t.focused).map(|(g, _, w)| (g, w))).collect()).unwrap_or_default()
            };
            let Some(beside) = super::pane_move_ui::tmux_drop_sibling(&ids, index, gw) else {
                super::find_ui::beep();
                return;
            };
            self.tmux_break(pane, true, None, Some(beside));
            return;
        }
        self.end_replays(&[pane]);
        let super::pane_move_ui::Detached::Kept = super::pane_move_ui::detach(&mut self.tabs, pane) else { return };
        self.broadcast.forget(&super::pane_move_ui::tab_drop_left(pane, window == source));
        let tab = Tab::new(self.next_tab_id, Layout::new(pane), pane);
        self.next_tab_id += 1;
        let id = tab.id;
        if window != source {
            self.relayout();
            self.redraw();
            if !self.enter(window) {
                // The window went: the session keeps a tab of its own here.
                self.tabs.push(tab);
                self.select_tab(self.tabs.len() - 1);
                self.relayout();
                return;
            }
        }
        let groups: Vec<Option<&str>> = self.tabs.iter().map(|t| t.group.as_ref().map(|g| g.id.as_str())).collect();
        let pinned: Vec<bool> = self.tabs.iter().map(|t| t.pinned).collect();
        let slot = super::pane_move_ui::new_tab_slot(&groups, &pinned, index);
        self.tabs.insert(slot, tab);
        self.settle_tabs();
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
            self.select_tab(i);
        }
        self.tmux_reorder();
        self.relayout();
        if window != source {
            self.focus_terminal_window(window);
        }
        self.redraw();
    }

    /// A pane dragged by its title bar outside every window (`MovePaneController.m:234-237`): it
    /// leaves its tab for a new window whose top left corner is `origin` (13d's torn-off
    /// placement). A loaded recording never moves; a tmux pane breaks out through tmux with that
    /// point (`MovePaneController.m:136-140`).
    pub(super) fn move_session_to_window_at(&mut self, pane: PaneId, origin: (i32, i32), event_loop: &ActiveEventLoop) {
        let Some(source) = self.current_window() else { return };
        let Some(from) = self.tabs.iter().position(|t| t.layout.panes().contains(&pane)) else { return };
        if self.tabs[from].layout.panes().len() < 2 || self.panes.get(&pane).is_some_and(|p| p.dvr.read_only) {
            return;
        }
        if self.tmux_break(pane, false, Some(origin), None) {
            return;
        }
        self.end_replays(&[pane]);
        let super::pane_move_ui::Detached::Kept = super::pane_move_ui::detach(&mut self.tabs, pane) else { return };
        self.broadcast.forget(&[pane]);
        let tab = Tab::new(self.next_tab_id, Layout::new(pane), pane);
        self.next_tab_id += 1;
        // Opened now: a failed open puts the tab back here.
        self.open_pending(event_loop, NewWindow::TornOff(tab, origin));
        let Some(new) = self.current_window().filter(|w| *w != source) else {
            self.relayout();
            self.redraw();
            return;
        };
        if self.enter(source) {
            self.relayout();
            self.redraw();
        }
        self.focus_terminal_window(new);
    }

    /// The tab holds a loaded recording's pane, which is never moved to another window.
    pub(super) fn holds_recording(&self, t: &Tab) -> bool {
        t.layout.panes().iter().any(|id| self.panes.get(id).is_some_and(|p| p.dvr.read_only))
    }

    /// Window > Arrange Windows > Merge All Windows: every other window's tabs come here,
    /// in window-number order, and those windows close. A tab holding a loaded recording
    /// stays where it is, and so does its window.
    pub(super) fn merge_all_windows(&mut self) {
        let Some(k) = self.windows.current() else { return };
        for other in self.registry.merge_order(k) {
            let Some(mut slot) = self.windows.remove(other) else { continue };
            let tabs = std::mem::take(&mut slot.tabs);
            let (mut moved, mut kept, active) = split_for_merge(tabs, slot.active, |t| self.holds_recording(t));
            for t in &moved {
                leave_broadcast(&mut slot.broadcast, &mut slot.broadcast_tabs, t);
            }
            // The replays of the panes that move end; a panel stays only with its tab.
            let panes: Vec<PaneId> = moved.iter().flat_map(|t| t.layout.panes()).collect();
            for id in panes {
                if let Some(p) = self.panes.get_mut(&id).filter(|p| p.replay.is_some()) {
                    p.replay = None;
                    p.end_view();
                }
            }
            self.tabs.append(&mut moved);
            if !kept.is_empty() {
                // Its active tab moved: its panel goes, and `replay_seen` still names the
                // moved tab, so a replaying recording now in front opens its panel when
                // the window is next looked at (`replay_follow_focus`).
                if active.is_none() {
                    slot.replay_window = None;
                }
                slot.active = active.unwrap_or(0);
                // A new selection there may be hidden in a collapsed group.
                super::tab_groups_ui::expand_selected(&mut kept, slot.active);
                slot.tabs = kept;
                if let Some(g) = &slot.gpu {
                    g.window.request_redraw();
                }
                self.windows.park(other, slot);
                continue;
            }
            if let Some((guid, n)) = slot.win_slot.take() {
                self.slots.give(&guid, n);
            }
            self.forget_window(other);
            if let Some(g) = &slot.gpu {
                if let Ok(RawWindowHandle::Win32(h)) = g.window.window_handle().map(|h| h.as_raw()) {
                    crate::winkeys::remove_window(h.hwnd.get());
                }
            }
            drop(slot);
        }
        // Merged pinned tabs go to the front, and groups stay together.
        if self.settle_tabs() {
            self.tmux_reorder();
        }
        self.relayout();
        self.redraw();
    }

    /// Shell > Duplicate Window: the window saved as an arrangement (without contents) and
    /// opened again, with its splits, profiles, and folders.
    pub(super) fn duplicate_window(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        // A window of loaded recordings only has nothing to duplicate.
        let Some(snap) = self.snapshot_window(super::arrange_ui::Contents::None) else { return };
        self.pending_windows.push(NewWindow::Arrangement(snap, None));
        self.redraw();
    }
}

impl App {
    /// The current window's monitor: its work area and whole area.
    pub(super) fn monitor_rects(&self) -> Option<(Rect, Rect)> {
        monitor_rects_of(self.hwnd()?)
    }

    /// Applies an iTerm2 window style to the current window. The Window Style menu makes
    /// the edge styles span the whole edge (100 percent), as iTerm2's menu does.
    pub(super) fn apply_window_style(&mut self, style: i64, from_menu: bool) {
        use crate::frames::{self, Input};
        let Some((work, screen)) = self.monitor_rects() else {
            return self.banners.set("window", "Could not read this window's monitor.");
        };
        let profile = self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)).and_then(|p| self.store.get(&p.profile)).unwrap_or_else(|| self.store.default_profile()).clone();
        // `measurements(profile:)`: an eligible type's explicit percentages, else cells.
        let percents = {
            let (w, h) = frames::profile_percents(&profile);
            frames::percents(style, w, h)
        };
        let mut percent = (percents.0.unwrap_or(-1.0), percents.1.unwrap_or(-1.0));
        let s = frames::unthemed(style);
        if from_menu {
            match s {
                frames::TOP_PERCENT | frames::BOTTOM_PERCENT => percent.0 = 100.0,
                frames::LEFT_PERCENT | frames::RIGHT_PERCENT => percent.1 = 100.0,
                _ => {}
            }
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let (outer, inner) = (g.window.outer_size(), g.window.inner_size());
        let pos = g.window.outer_position().unwrap_or_default();
        let (cw, ch) = self.cell();
        let area = self.content_area();
        let pad = 2.0 * PAD * self.scale();
        let chrome = (pad, inner.height as f32 - area.h + pad);
        let input = Input {
            style: s,
            percent,
            // A percentage is whole cells of that share of the work area (`WSH:223-233`).
            cells: (
                percents.0.map_or(profile.int("Columns").clamp(1, 1000) as u16, |p| frames::percent_cells(p, cw, work.w)),
                percents.1.map_or(profile.int("Rows").clamp(1, 1000) as u16, |p| frames::percent_cells(p, ch, work.h)),
            ),
            cell: (cw, ch),
            chrome,
            initial: Rect { x: pos.x as f32, y: pos.y as f32, w: outer.width as f32, h: outer.height as f32 },
            work,
            screen,
        };
        let f = frames::frame(&input);
        let w = &g.window;
        // Entering or leaving full screen runs the Use Transparency state machine (`PTWS:587-600`).
        if f.fullscreen != w.fullscreen().is_some() {
            self.win_transparency.before_fullscreen_toggle(w.fullscreen().is_some(), self.globals.bool(crate::transparency::DISABLE_FULLSCREEN_KEY));
        }
        if f.fullscreen {
            w.set_fullscreen(Some(Fullscreen::Borderless(w.current_monitor())));
        } else {
            w.set_fullscreen(None);
            w.set_decorations(f.decorations);
            w.set_maximized(f.maximized);
            if !f.maximized && !matches!(s, frames::NORMAL | frames::NO_TITLE_BAR | frames::ACCESSORY) {
                // The frame is the outer frame; the inner size leaves the borders out.
                let (dw, dh) = if f.decorations { (outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height)) } else { (0, 0) };
                w.set_outer_position(PhysicalPosition::new(f.rect.x.round() as i32, f.rect.y.round() as i32));
                let _ = super::caption_ui::request_inner_size(&w, PhysicalSize::new((f.rect.w as u32).saturating_sub(dw).max(1), (f.rect.h as u32).saturating_sub(dh).max(1)));
            }
        }
        // Accessory (iTerm2's floating panel) stays above other windows without a taskbar
        // button; leaving it gives both back. A hotkey window keeps its own.
        if self.hotkey.is_none() && (s == frames::ACCESSORY) != (self.win_style == frames::ACCESSORY) {
            use winit::platform::windows::WindowExtWindows;
            let accessory = s == frames::ACCESSORY;
            w.set_window_level(if accessory { winit::window::WindowLevel::AlwaysOnTop } else { winit::window::WindowLevel::Normal });
            w.set_skip_taskbar(accessory);
        }
        self.win_style = s;
        // The style may enter or leave full screen: the border follows at once.
        self.apply_border();
        self.redraw();
    }
}

const SAVED_FRAMES: &str = "SavedWindowFrames";

fn to_rect(t: (f64, f64, f64, f64)) -> Rect {
    Rect { x: t.0 as f32, y: t.1 as f32, w: t.2 as f32, h: t.3 as f32 }
}

impl App {
    pub(super) fn outer_rect(g: &Gpu) -> Rect {
        let p = g.window.outer_position().unwrap_or_default();
        let s = g.window.outer_size();
        Rect { x: p.x as f32, y: p.y as f32, w: s.width as f32, h: s.height as f32 }
    }

    /// Where a new window goes: New window placement, the profile's saved slot, and its
    /// Screen setting.
    /// Takes the window's profile slot (its frame is kept there when it closes).
    pub(super) fn claim_slot(&mut self) -> (String, u32) {
        let guid = self.tabs.first().and_then(|t| self.panes.get(&t.focused)).map(|p| self.store.shared_guid(&p.profile)).unwrap_or_else(|| self.store.default_guid.clone());
        let slot = self.slots.take(&guid);
        self.win_slot = Some((guid.clone(), slot));
        (guid, slot)
    }

    pub(super) fn place_window(&mut self) {
        use crate::placement::{self, Rect as R};
        let (guid, slot) = self.claim_slot();
        let monitors: Vec<R> = placement::primary_first(super::requests_ui::work_areas().into_iter().map(to_rect).collect());
        let Some(g) = self.gpu.as_ref() else { return };
        let current = Self::outer_rect(g);
        let others: Vec<R> = self.windows.parked().filter_map(|(_, s)| s.gpu.as_ref().map(Self::outer_rect)).collect();
        let inside = |r: &R, x: f32, y: f32| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
        let window_monitor = monitors.iter().position(|m| inside(m, current.x + current.w / 2.0, current.y + current.h / 2.0)).unwrap_or(0);
        let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        let have_cursor = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) } != 0;
        // Without the pointer's position, "Screen with Cursor" uses the window's monitor.
        let cursor_monitor = if have_cursor { monitors.iter().position(|m| inside(m, pt.x as f32, pt.y as f32)).unwrap_or(window_monitor) } else { window_monitor };
        let profile = self.store.get(&guid).unwrap_or_else(|| self.store.default_profile());
        let pref = profile.get("Screen").and_then(|v| v.as_integer()).unwrap_or(-1);
        let work = placement::screen_for(pref, &monitors, window_monitor, cursor_monitor);
        self.win_anchor = placement::anchor(pref, monitors.len(), cursor_monitor);
        let saved = self.globals.table(SAVED_FRAMES).get(&placement::slot_key(&guid, slot)).and_then(|v| v.as_array()).and_then(|a| {
            let n: Vec<f32> = a.iter().filter_map(|x| x.as_float().or_else(|| x.as_integer().map(|i| i as f64))).map(|f| f as f32).collect();
            (n.len() == 4).then(|| R { x: n[0], y: n[1], w: n[2], h: n[3] })
        });
        let policy = self.globals.get("WindowPlacement").and_then(|v| v.as_integer()).unwrap_or(placement::POSITION);
        let count = self.registry.order().len();
        let target = placement::place(policy, saved, count, work, &others, current, &monitors).or_else(|| {
            // A Screen preference moves even a window Windows placed.
            (pref != -1).then(|| placement::move_to(current, monitors.get(window_monitor).copied().unwrap_or(work), work))
        });
        let Some(r) = target else { return };
        let w = &g.window;
        w.set_outer_position(PhysicalPosition::new(r.x.round() as i32, r.y.round() as i32));
        if (r.w, r.h) != (current.w, current.h) {
            let (outer, inner) = (w.outer_size(), w.inner_size());
            let (dw, dh) = (outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height));
            let _ = super::caption_ui::request_inner_size(&w, PhysicalSize::new((r.w as u32).saturating_sub(dw).max(1), (r.h as u32).saturating_sub(dh).max(1)));
        }
    }

    /// The anchor of a window `place_window` did not place (hotkey, arrangement, torn-off): the
    /// monitor its profile's Screen names.
    pub(super) fn anchor_from_profile(&mut self, guid: Option<&str>) {
        use crate::placement::{self, Rect as R};
        let monitors: Vec<R> = placement::primary_first(super::requests_ui::work_areas().into_iter().map(to_rect).collect());
        let Some(g) = self.gpu.as_ref() else { return };
        let current = Self::outer_rect(g);
        let inside = |r: &R, x: f32, y: f32| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
        let window_monitor = monitors.iter().position(|m| inside(m, current.x + current.w / 2.0, current.y + current.h / 2.0)).unwrap_or(0);
        let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        let have_cursor = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) } != 0;
        let cursor_monitor = if have_cursor { monitors.iter().position(|m| inside(m, pt.x as f32, pt.y as f32)).unwrap_or(window_monitor) } else { window_monitor };
        let pref = guid.and_then(|g| self.store.get(g)).unwrap_or_else(|| self.store.default_profile()).get("Screen").and_then(|v| v.as_integer()).unwrap_or(-1);
        self.win_anchor = placement::anchor(pref, monitors.len(), cursor_monitor);
    }

    /// After a display change (`applicationDidChangeScreenParameters`, `AD:954-961`): every
    /// window refits by its style (`canonicalizeWindowFrame`, `PT:5126-5215`), then, with
    /// `ChaseAnchoredScreen`, an anchored window on another monitor goes back to its anchor.
    pub(super) fn refit_all_windows(&mut self) {
        use crate::frames::{self, Refit};
        use crate::placement::{self, Rect as R};
        let back = self.current_window();
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_id_of_key(k)).collect();
        for id in ids {
            if !self.enter(id) {
                continue;
            }
            let Some(g) = self.gpu.as_ref() else { continue };
            if g.window.is_minimized() == Some(true) {
                continue;
            }
            if let Some(h) = self.hotkey.as_ref() {
                // A hotkey window is framed by its own style on its profile's screen.
                let guid = h.guid.clone();
                let moved = self.move_to_preferred_screen(&guid);
                // Staying on its screen, its screen may still have changed size.
                let style = self.win_style;
                if placement::reframe_hotkey(style, moved) {
                    self.apply_window_style(style, false);
                }
            } else {
                match frames::refit(self.win_style) {
                    Refit::Keep => {}
                    Refit::Recenter => {
                        if let Some((work, _)) = self.monitor_rects() {
                            let w = g.window.clone();
                            let r = frames::recenter(Self::outer_rect(g), work);
                            super::chrome_ui::set_outer_frame(&w, r);
                        }
                    }
                    Refit::Fit => {
                        let style = self.win_style;
                        self.apply_window_style(style, false);
                    }
                }
            }
            // The frame the old screens held no longer exists.
            self.zoom_restore = None;
            if self.globals.bool("ChaseAnchoredScreen") {
                let monitors: Vec<R> = placement::primary_first(super::requests_ui::work_areas().into_iter().map(to_rect).collect());
                if let Some(g) = self.gpu.as_ref() {
                    let outer = Self::outer_rect(g);
                    let inside = |r: &R, x: f32, y: f32| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
                    let at = monitors.iter().position(|m| inside(m, outer.x + outer.w / 2.0, outer.y + outer.h / 2.0)).unwrap_or(0);
                    if let (Some(i), Some(from)) = (placement::chase(self.win_anchor, monitors.len(), at), monitors.get(at).copied()) {
                        let chased = placement::chase_frame(self.win_style, outer, from, monitors[i]);
                        let w = g.window.clone();
                        // A maximized window is not moved by position: restore it first, and
                        // maximize it again on the new monitor unless its style frames it.
                        let (restore, remaximize) = placement::chase_maximize(chased.refit, w.is_maximized());
                        if restore {
                            w.set_maximized(false);
                        }
                        w.set_outer_position(PhysicalPosition::new(chased.frame.x.round() as i32, chased.frame.y.round() as i32));
                        if remaximize {
                            w.set_maximized(true);
                        }
                        // The move canonicalizes again (`PT:5128`): the style frames it on the new screen.
                        if chased.refit {
                            let style = self.win_style;
                            self.apply_window_style(style, false);
                        }
                    }
                }
            }
        }
        if let Some(b) = back {
            self.enter(b);
        }
    }

    /// Keeps the current window's frame for its profile slot (when it closes, or at quit).
    pub(super) fn save_window_frame(&mut self) {
        let Some((guid, slot)) = self.win_slot.take() else { return };
        self.slots.give(&guid, slot);
        let Some(g) = self.gpu.as_ref() else { return };
        if g.window.is_minimized() == Some(true) || g.window.fullscreen().is_some() || g.window.is_maximized() {
            return;
        }
        let r = Self::outer_rect(g);
        let mut t = self.globals.table(SAVED_FRAMES);
        t.insert(crate::placement::slot_key(&guid, slot), toml::Value::Array([r.x, r.y, r.w, r.h].iter().map(|v| toml::Value::Float(*v as f64)).collect()));
        self.globals.set(SAVED_FRAMES, toml::Value::Table(t));
        self.save_globals();
    }

    /// Quit: every window keeps its frame.
    pub(super) fn save_all_window_frames(&mut self) {
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_of_key(k)).collect();
        for id in ids {
            if self.enter(id) {
                self.save_window_frame();
            }
        }
    }
}

/// What Edit Window Title or Edit Tab Title names.
pub(super) enum TitleEdit {
    Window(WindowId),
    Tab(WindowId, u64),
}

impl App {
    /// The window settings a profile applies when its window opens: Hide After Opening and
    /// Lock Window Size Automatically.
    pub(super) fn apply_window_profile(&mut self, guid: Option<&str>) {
        let p = guid.and_then(|g| self.store.get(g)).unwrap_or_else(|| self.store.default_profile()).clone();
        if p.bool("Lock Window Size Automatically") {
            self.win_locks.size = true;
            self.win_locks.layout = false;
        }
        self.apply_resizable();
        if let Some(g) = self.gpu.as_ref().filter(|_| p.bool("Hide After Opening")) {
            g.window.set_minimized(true);
        }
    }

    /// False (with a beep) when Lock Layout forbids the change.
    pub(super) fn layout_allowed(&self, op: crate::winmodel::LayoutOp) -> bool {
        if !self.layout_allowed_quiet(op) {
            unsafe { windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(windows_sys::Win32::UI::WindowsAndMessaging::MB_OK) };
            return false;
        }
        true
    }

    /// False, without a beep, when Lock Layout forbids the change (a tab drag that never
    /// begins, `PT:7610-7616`).
    pub(super) fn layout_allowed_quiet(&self, op: crate::winmodel::LayoutOp) -> bool {
        !self.win_locks.forbids(op)
    }

    pub(super) fn toggle_lock(&mut self, layout: bool) {
        if layout {
            self.win_locks.toggle_layout();
        } else {
            self.win_locks.toggle_size();
        }
        self.apply_resizable();
    }

    /// Window > Resize Window: one cell narrower, wider, shorter, or taller.
    pub(super) fn resize_window_by_cell(&mut self, r: crate::winmodel::Resize) {
        if self.win_locks.size {
            return;
        }
        let cell = self.cell();
        let Some(g) = &self.gpu else { return };
        let inner = g.window.inner_size();
        let (w, h) = crate::winmodel::step_size((inner.width, inner.height), cell, r);
        let _ = super::caption_ui::request_inner_size(&g.window, PhysicalSize::new(w, h));
    }

    /// Edit Window Title and Edit Tab Title: a one-line window; an empty title clears it.
    pub(super) fn open_title_prompt(&mut self, event_loop: &ActiveEventLoop, tab: bool) {
        if let Some((w, _)) = &self.title_prompt {
            return w.focus();
        }
        let Some(win) = self.current_window() else { return };
        let (label, target, now) = if tab {
            let Some(t) = self.tabs.get(self.active) else { return };
            ("Tab title:", TitleEdit::Tab(win, t.id), t.title.clone().unwrap_or_default())
        } else {
            ("Window title:", TitleEdit::Window(win), self.win_title.clone().unwrap_or_default())
        };
        let Some(g) = self.gpu.as_ref() else { return };
        let title = if tab { "Edit Tab Title" } else { "Edit Window Title" };
        match crate::prompt_ui::PromptWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, title, label, &now) {
            Ok(w) => self.title_prompt = Some((w, target)),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Edit Tab Title's text for a tab of the current window (None clears it): a tmux tab's
    /// names its tmux window.
    pub(super) fn apply_tab_title(&mut self, tab: u64, text: Option<String>) {
        let first = self.tabs.iter().find(|t| t.id == tab).map(|t| t.focused);
        let renamed = match (first, text.as_deref()) {
            (Some(f), Some(name)) => self.tmux_rename(f, name),
            _ => false,
        };
        if !renamed {
            if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) {
                t.title = text;
            }
        }
    }

    /// Events for the title window. False when `window` is not it.
    pub(super) fn title_prompt_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        use crate::prompt_ui::PromptResult;
        let Some((w, _)) = self.title_prompt.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        match event {
            WindowEvent::CloseRequested => self.title_prompt = None,
            WindowEvent::RedrawRequested => match w.redraw() {
                PromptResult::Open | PromptResult::Extra(_) => {}
                PromptResult::Cancel => self.title_prompt = None,
                PromptResult::Ok(text) => {
                    let Some((_, target)) = self.title_prompt.take() else { return true };
                    let text = Some(text.trim().to_string()).filter(|t| !t.is_empty());
                    match target {
                        TitleEdit::Window(id) => {
                            if self.enter(id) {
                                self.win_title = text;
                            }
                        }
                        TitleEdit::Tab(id, tab) => {
                            if self.enter(id) {
                                self.apply_tab_title(tab, text);
                            }
                        }
                    }
                    self.redraw();
                }
            },
            other => {
                w.event(other);
            }
        }
        true
    }
}

impl App {
    /// Window > Arrange Windows > Arrange Windows Horizontally, on the current window's
    /// monitor.
    pub(super) fn arrange_windows_horizontally(&mut self) {
        let Some((work, _)) = self.monitor_rects() else { return };
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_of_key(k)).collect();
        let mut frames = Vec::new();
        for id in &ids {
            if self.enter(*id) {
                // Only this monitor's windows that are showing (iTerm2 arranges per screen).
                let here = self.monitor_rects().is_some_and(|(w, _)| w == work);
                if let Some(g) = self.gpu.as_ref().filter(|g| here && g.window.is_minimized() != Some(true)) {
                    frames.push((*id, Self::outer_rect(g)));
                }
            }
        }
        let placed = crate::placement::arrange_horizontally(work, &frames.iter().map(|f| f.1).collect::<Vec<_>>());
        for ((id, _), r) in frames.iter().zip(placed) {
            if self.enter(*id) {
                if let Some(g) = &self.gpu {
                    let (outer, inner) = (g.window.outer_size(), g.window.inner_size());
                    let (dw, dh) = (outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height));
                    g.window.set_outer_position(PhysicalPosition::new(r.x as i32, r.y as i32));
                    let _ = super::caption_ui::request_inner_size(&g.window, PhysicalSize::new((r.w as u32).saturating_sub(dw).max(1), (r.h as u32).saturating_sub(dh).max(1)));
                }
            }
        }
    }
}

impl App {
    /// A window's tabs and the index of its selected tab, current or parked.
    pub(super) fn window_tabs(&self, k: u64) -> Option<(&[Tab], usize)> {
        if self.windows.current() == Some(k) {
            return Some((&self.tabs, self.active));
        }
        self.windows.parked().find(|(kk, _)| **kk == k).map(|(_, s)| (&s.tabs[..], s.active))
    }

    /// Every window's tabs, by window number: (window number, tab index, the tab's panes).
    pub(super) fn all_tab_panes(&self) -> Vec<(usize, usize, Vec<PaneId>)> {
        let mut out = Vec::new();
        for k in self.registry.order() {
            let n = self.registry.number(k).unwrap_or(0);
            let tabs: Vec<Vec<PaneId>> = if self.windows.current() == Some(k) {
                self.tabs.iter().map(|t| t.layout.panes()).collect()
            } else {
                self.windows.parked().find(|(kk, _)| **kk == k).map(|(_, s)| s.tabs.iter().map(|t| t.layout.panes()).collect()).unwrap_or_default()
            };
            out.extend(tabs.into_iter().enumerate().map(|(i, p)| (n, i, p)));
        }
        out
    }
}

/// The work area and the whole of the monitor a window is on (the nearest one), in pixels.
pub(super) fn monitor_rects_of(hwnd: isize) -> Option<(Rect, Rect)> {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    let m = unsafe { MonitorFromWindow(hwnd as _, MONITOR_DEFAULTTONEAREST) };
    if unsafe { GetMonitorInfoW(m, &mut info) } == 0 {
        return None;
    }
    let r = |r: windows_sys::Win32::Foundation::RECT| Rect { x: r.left as f32, y: r.top as f32, w: (r.right - r.left) as f32, h: (r.bottom - r.top) as f32 };
    Some((r(info.rcWork), r(info.rcMonitor)))
}

/// A merge's split of a window's tabs: the ones that move, the ones `stays` keeps, and the
/// index among the kept ones of the window's active tab (None when it moved).
fn split_for_merge(tabs: Vec<Tab>, active: usize, stays: impl Fn(&Tab) -> bool) -> (Vec<Tab>, Vec<Tab>, Option<usize>) {
    let (mut moved, mut kept, mut now) = (Vec::new(), Vec::new(), None);
    for (i, t) in tabs.into_iter().enumerate() {
        if stays(&t) {
            if i == active {
                now = Some(kept.len());
            }
            kept.push(t);
        } else {
            moved.push(t);
        }
    }
    (moved, kept, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_arm_is_per_window() {
        // Window 1 arms; entering window 2 swaps its slot in, and its own arm is off.
        assert_eq!(armed_windows(Some((1, true)), &[(2, false)]), vec![1]);
        assert_eq!(armed_windows(Some((2, false)), &[(1, true)]), vec![1]);
        // Entering window 1 again brings its arm back; the menu check reads the current slot.
        assert_eq!(armed_windows(Some((1, true)), &[(2, false)]), vec![1]);
        assert_eq!(armed_windows(Some((2, true)), &[(1, true), (3, false)]), vec![2, 1]);
        assert_eq!(armed_windows(None, &[(1, false)]), Vec::<u64>::new());
        // A new window starts unarmed.
        assert!(!WinSlot::new_empty().status_armed);
    }

    fn tab(id: u64, pane: PaneId) -> Tab {
        Tab::new(id, Layout::new(pane), pane)
    }

    #[test]
    fn a_merge_leaves_loaded_recordings_behind() {
        // Tabs 1 and 3 hold loaded recordings (panes 10 and 30) and stay; the active tab 2
        // moves, so the window's active tab becomes its first kept one.
        let stays = |t: &Tab| t.layout.panes().iter().any(|p| *p == 10 || *p == 30);
        let (moved, kept, active) = split_for_merge(vec![tab(1, 10), tab(2, 20), tab(3, 30)], 1, stays);
        assert_eq!(moved.iter().map(|t| t.id).collect::<Vec<_>>(), vec![2]);
        assert_eq!(kept.iter().map(|t| t.id).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(active, None, "the active tab moved");
        let (_, kept, active) = split_for_merge(vec![tab(1, 10), tab(2, 20), tab(3, 30)], 2, stays);
        assert_eq!((kept.len(), active), (2, Some(1)), "tab 3 stays active, now at index 1");
    }
}
