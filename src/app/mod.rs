//! The Tandem window: tabs of split panes, drawn with wgpu.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::Selection;
use alacritty_terminal::term::{TermMode, viewport_to_point};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, KeyLocation, ModifiersKeyState, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Window, WindowId};

use crate::conpty::{self, Api, Source};
use crate::globals::{self, Globals};
use crate::input::{self, Clicks, Wheel};
use crate::keyroute::{self, Route};
use crate::keys::{self, Action, AltSide, Key, KeyOptions};
use crate::keystroke;
use crate::tab_drag;
use crate::tab_flash;
use crate::tab_layout;
use crate::tab_position::{self, TabPosition};
use crate::mru::Mru;
use crate::pointer::{self, Down, Up};
use crate::layout::{Axis, Layout, PaneId, Rect};
use crate::mouse::{self, Button, Mods, Tracking};
use crate::pane::Pane;
use crate::pty::Notify;
use crate::palette::Palette;
use crate::profile::{self, Profile};
use crate::render::Renderer;
use crate::screen::Look;
use crate::store::{self, Store};
use crate::scene::{self, Frame, rgba};
use crate::settings_ui::{Edit, How, Mode, SettingsWindow};

const FONT: &str = "Cascadia Mono";
/// 12 pt, iTerm2's default size, at 96 dpi.
const FONT_PX: f32 = 16.0;
/// Space between a pane's edge and its text, and the divider width, in pixels at 100%.
const PAD: f32 = 4.0;
const GAP: f32 = 1.0;
/// How often a fading tab bar draws while it fades (60 frames a second).
const FLASH_FRAME: Duration = Duration::from_millis(16);
/// How far below the pointer a tab tooltip starts, in points (the arrow pointer's height).
const TIP_POINTER_PT: f32 = 20.0;
/// Lines scrolled per wheel notch.
const WHEEL_LINES: f32 = 3.0;
/// Half a cursor blink cycle.
const BLINK: std::time::Duration = std::time::Duration::from_millis(530);
/// How long after the last settings edit the file is written.
const SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

mod actions;
mod paste_ui;
mod history_ui;
mod shell_history_ui;
mod snippets_ui;
mod clippings_ui;
mod inline_chat_ui;
mod completion_ui;
mod composer_ui;
mod annotations_ui;
mod activity_ui;
mod alerts_ui;
mod badge_ui;
mod bell_ui;
mod bgimage_ui;
mod blocks_ui;
mod clipboard_ui;
mod copymode_ui;
mod files_ui;
mod find_ui;
mod infobar_ui;
mod keyproto_ui;
mod fold_ui;
mod labels_ui;
mod screenshot_ui;
mod semantic_ui;
mod session_ui;
mod seltools_ui;
mod view_ui;
mod marks_ui;
mod menu;
mod porthole_ui;
mod prompted_ui;
mod triggers_ui;
mod requests_ui;
mod jobs_ui;
mod close_ui;
mod coprocess_ui;
mod aps_ui;
mod windows_ui;
#[cfg(windows)]
mod chrome_ui;
#[cfg(windows)]
mod sizing_ui;
#[cfg(windows)]
mod caption_ui;
mod tab_drag_ui;
mod tab_groups_ui;
mod undo_ui;
mod arrange_ui;
mod restore_ui;
mod hotkey_ui;
mod broadcast_ui;
mod tmux_ui;
mod tmux_tabs_ui;
mod tmux_dashboard_app;
mod bury_ui;
mod statusbar_ui;
mod statusbar_content;
mod metrics_ui;
mod git_ui;
mod toolbelt_ui;
mod toolbelt_tools_ui;
mod toolbelt_more_ui;
mod open_quickly_app;
mod replay_ui;
mod pwm_app;
mod api_ui;
mod api_requests;
mod script_ui;
mod script_runtime_ui;
mod scripts_alerts;
mod warnings_app;
mod suppressed_app;
mod host_offers_ui;
mod frustration_ui;
mod shell_version_ui;
mod prefs_ui;
mod settings_export_ui;
mod ai_app;
mod ai_runs;
mod ai_engage_app;
mod codecierge_app;
mod ai_explain_app;
mod chat_app;
mod chat_window_app;
mod chat_session_app;
mod chat_engine;
mod ai_tools_app;
mod ai_exec_app;
mod ai_safety_app;
mod scripts_app;
mod script_console_app;
mod script_deps_app;
mod builtin_ui;
mod ssh_app;
mod ssh_framer_app;

pub enum UserEvent {
    /// A pane has output, or its shell exited.
    Wake,
    /// A Win chord from the keyboard hook.
    Chord(crate::menus::Chord),
    /// A menu item was clicked (its id).
    Menu(String),
    /// A notification was clicked.
    NotificationClicked,
    /// A shell's foreground job changed.
    JobsChanged,
    /// A system-wide hotkey was pressed (the ids of every hotkey it matched).
    Hotkey(Vec<u32>),
    /// A system setting changed under a forced Theme: winit reset the title bars to the
    /// system mode, so the windows take the forced mode again.
    ThemeReset,
    /// A window button of a Minimal or Compact window's caption was hovered, pressed or released:
    /// the window (its HWND) and what happened (`caption_ui`).
    Caption(isize, crate::caption::Event),
}

#[derive(Clone)]
struct Tab {
    /// Stable across moves, for the most-recently-used order.
    id: u64,
    layout: Layout,
    focused: PaneId,
    /// Maximize Active Pane: this pane fills the tab.
    zoomed: Option<PaneId>,
    /// Edit Tab Title's text, instead of the session title.
    title: Option<String>,
    /// Pinned tabs stay at the front of the bar (`PTS:355-416`).
    pinned: bool,
    /// The tab's group; its definition rides every member (`TABH:99-115`).
    group: Option<crate::tab_groups::TabGroup>,
}

impl Tab {
    /// A tab with no title, not pinned and in no group.
    pub(super) fn new(id: u64, layout: Layout, focused: PaneId) -> Tab {
        Tab { id, layout, focused, zoomed: None, title: None, pinned: false, group: None }
    }
}

/// A press whose pointer binding runs when the button comes up.
struct PointerDown {
    button: u8,
    clicks: u8,
    pane: PaneId,
    cell: (u32, u32),
}

enum Drag {
    None,
    Select { pane: PaneId },
    Divider { path: Vec<bool>, axis: Axis, parent: Rect },
    /// A press the program took as a mouse report: its click count and when it came, for
    /// the frustration detector's release.
    App { pane: PaneId, button: Button, clicks: u8, at: Instant },
    /// Selecting text in a porthole (its frame when the drag began).
    Porthole { pane: PaneId, id: u32, frame: Rect },
    /// A Clippings panel's width or one of its rows.
    Clip { pane: PaneId, kind: clippings_ui::ClipDrag },
    /// An inline chat panel's width: where the drag began (pixels) and the width then (logical).
    Inline { pane: PaneId, start_x: f32, start_w: f32 },
    /// The toolbelt's width, a divider between its tools, or a tool's row.
    Belt(toolbelt_ui::BeltDrag),
    /// A composer's text selection, drag handle, or border.
    Composer { pane: PaneId, kind: composer_ui::ComposerDrag },
    /// A side tab bar's drag handle: where the press was (pixels) and the bar's width then
    /// (points).
    SideBar { start_x: f32, start_width: f32 },
    /// A left press on a tab (`tab_drag_ui`): the tab's index, where the press was (pixels),
    /// whether the pointer has moved far enough to drag it, and the window's selected tab
    /// before the press selected this one (`preDragSelectedTabViewItem`, recorded in
    /// `PSMTabBarControl mouseDown:` before `tabClick:`, `PSMTabBarControl.m:3683-3703`).
    Tab { from: usize, start: (f32, f32), began: bool, before: Option<u64> },
    /// A left press on a group chip (`tab_drag_ui`): the group, where the press was (pixels),
    /// and whether the pointer has moved far enough to drag the whole group
    /// (`PSM:3763-3786`).
    Chip { group: String, start: (f32, f32), began: bool },
    /// A left press on the "+" button, where it was (pixels): moving far enough drags the
    /// window.
    AddButton { start: (f32, f32) },
    /// A left press on the bar that only a drag converts, to a window drag (`tab_drag::Arm::Window`:
    /// the empty bar, a close button, a tab Lock Layout keeps in place), where it was (pixels).
    /// It never selects or reorders; the press clicks as usual.
    Window { start: (f32, f32) },
}

struct Gpu {
    surface: wgpu::Surface<'static>,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    instance: wgpu::Instance,
    // Dropped last, after the surface.
    window: Arc<Window>,
}

pub struct App {
    notify: Notify,
    /// The foreground jobs of the panes' shells.
    jobs: crate::jobwatch::JobWatch,
    proxy: Arc<Mutex<EventLoopProxy<UserEvent>>>,
    menu_bar: Option<menu::MenuBar>,
    /// Make Text Bigger and Smaller, in points, on top of the profile font size.
    font_offset: f32,
    /// Keys > Remap Modifiers, read by the keyboard hook's thread.
    remap_tags: Arc<std::sync::RwLock<crate::remap::Tags>>,
    /// What the keyboard hook needs to decide which Win chords Tandem takes.
    route_snapshot: Arc<std::sync::RwLock<keyroute::Snapshot>>,
    /// Global bindings to Do Not Remap Modifiers, for the hook.
    exempt_map: Arc<std::sync::RwLock<toml::Table>>,
    globals: Globals,
    /// The Leader was pressed; the next key carries the Leader flag.
    leader_on: bool,
    mru: Mru,
    next_tab_id: u64,
    /// The modifiers of a Cycle Tabs chord; letting go of them ends the cycle.
    mru_mods: Option<Mods>,
    alt_side: AltSide,
    pointer_clicks: pointer::Clicks,
    /// The last press for the pointer multi-click count: when, button, where, count.
    pointer_last: Option<(Instant, u8, (f32, f32), u8)>,
    /// A press that runs a pointer binding on release.
    pointer_down: Option<PointerDown>,
    /// Toggle Mouse Reporting: a pane's own choice over its profile's.
    mouse_override: HashMap<PaneId, bool>,
    /// An Alt single click in a pane at a cell, which may move the cursor on release.
    alt_click: Option<(PaneId, (u32, u32))>,
    /// A plain single click began in this pane: its release may open the password manager
    /// (`PTYMouseHandler.m:644-659` looks at the mouse-up).
    key_click: Option<PaneId>,
    wheel_acc_x: f32,
    /// Mark navigation: the mark each pane is at.
    mark_pos: HashMap<PaneId, u32>,
    /// Each pane's find bar and matches.
    finds: HashMap<PaneId, find_ui::FindState>,
    /// Where the focused pane's find bar and its mode label were last drawn.
    find_bar_rect: Option<Rect>,
    find_mode_rect: Option<Rect>,
    /// The Win-click target under the pointer while Win is held.
    hover: Option<(PaneId, crate::find::Match)>,
    /// The annotation or session note window: its pane and note (None for the session note).
    note_win: Option<(crate::note_ui::NoteWindow, PaneId, Option<u32>)>,
    /// Annotation navigation: the row each pane is at.
    note_pos: HashMap<PaneId, i32>,
    /// View > Show Timestamps and Show Cursor Guide, per pane, over the profile's choice.
    show_stamps: HashMap<PaneId, bool>,
    show_guide: HashMap<PaneId, bool>,
    /// Find Cursor is up until then (longer while Win is held).
    find_cursor_until: Option<Instant>,
    /// The pointer is at the right edge of a pane with Hover timestamps.
    stamp_near: bool,
    /// When an animated image next changes frame.
    anim_due: Option<Instant>,
    /// Copy Mode, per pane.
    copy_modes: HashMap<PaneId, crate::copymode::State>,
    /// Labels on results (Find URLs and the like), waiting for a key.
    shortcuts: Option<labels_ui::Shortcuts>,
    /// The last pop-up menu, kept until its pick arrives.
    popup_menu: Option<muda::Menu>,
    /// The open Tab Color picker: what it colors and the swatches it was built from, kept
    /// until its pick arrives. App-wide: the popup is modal, one at a time.
    color_pick: Option<(menu::ColorTarget, Vec<crate::palette::Rgb>)>,
    /// What the right-click menu's own items act on.
    ctx_pending: Option<semantic_ui::CtxPending>,
    /// The Find Globally window.
    global_find: Option<crate::global_find_ui::GlobalFindWindow>,
    /// The Set Named Mark window, and the pane it names a mark in.
    /// The Set Named Mark prompt, its pane, and the mark it renames (the toolbelt's Edit).
    prompt: Option<(crate::prompt_ui::PromptWindow, PaneId, Option<u32>)>,
    /// Session > Run Coprocess's window.
    co_prompt: Option<(crate::prompt_ui::PromptWindow, PaneId)>,
    /// A tmux close question's answer, waiting for the event loop: gateway, actions, and
    /// whether the whole window closes.
    tmux_pending_close: Vec<(PaneId, Vec<crate::tmux_controller::Action>, bool)>,
    /// A Close Tabs answer (group close commands), waiting for the event loop: the window it
    /// was asked from and the tabs to close.
    pending_tab_closes: Vec<(WindowId, Vec<u64>)>,
    /// A divider drag in a tmux tab: window key, gateway, pane before the divider,
    /// horizontal, start.
    tmux_drag: Option<(u64, PaneId, u32, bool, f32)>,
    /// Buried sessions (Session > Bury Session).
    buried: crate::bury::Burial,
    /// The tmux Dashboard and the gateway it shows.
    tmux_dash: Option<crate::tmux_dashboard_ui::TmuxDashboardWindow>,
    tmux_dash_gw: Option<PaneId>,
    /// tmux lines a cancelled drag released, run on the next pass.
    tmux_held_events: Vec<(PaneId, Vec<crate::tmux_gateway::Event>)>,
    /// tmux gateways, by the pane that runs `tmux -CC`.
    tmux: HashMap<PaneId, tmux_ui::TmuxGw>,
    /// Each watched tmux window's status lines, by gateway and window: (left, right).
    tmux_status: HashMap<(PaneId, u32), (String, String)>,
    /// A gateway's C key waits for the event loop to open its window.
    tmux_ask_command: Option<PaneId>,
    /// C's window, "Enter command to send tmux:".
    tmux_prompt: Option<(crate::prompt_ui::PromptWindow, PaneId)>,
    /// The terminal windows that are not current, their numbers, and the last one focused.
    windows: crate::winmodel::Parked<windows_ui::WinSlot>,
    registry: crate::winmodel::Registry,
    last_window: Option<WindowId>,
    /// The current window's style (iTerm2's window type id).
    win_style: i64,
    /// The current window's saved-frame slot: (profile guid, slot number).
    win_slot: Option<(String, u32)>,
    slots: crate::placement::Slots,
    /// The current window's Lock Size and Lock Layout, and its Edit Window Title text.
    win_locks: crate::winmodel::Locks,
    win_title: Option<String>,
    /// The current window's Window menu texts as last drawn: its title as the OS shows it
    /// without the label, and its title without the number (`caption_text`).
    window_menu_text: (String, String),
    /// The border color last given to the current window's DWM frame (`apply_border`).
    border_applied: Option<u32>,
    /// The mode (dark or not) the current window last followed (`apply_theme`), so a change of
    /// it is known without asking winit, which does not track a forced theme.
    theme_applied: Option<bool>,
    /// The current window's menu bar is hidden (full screen, `HideMenuBarInFullscreen`).
    menu_hidden: bool,
    /// The caption button under the pointer and the one held down in the current Minimal or
    /// Compact window (`caption_ui`).
    caption_hover: Option<crate::caption::Button>,
    caption_pressed: Option<crate::caption::Button>,
    /// The width in pixels of the window switch label's slot in the caption (its text measured
    /// plus padding), 0 without one; kept by `sync_caption_label` since layout cannot measure.
    caption_label_w: f32,
    /// The window name's text measured in pixels (its natural width), 0 without a name; kept by
    /// `sync_caption_label`.
    caption_name_w: f32,
    /// The current window's frame before a vertical Window > Zoom, which the next Zoom
    /// restores (a full Zoom maximizes, and Windows keeps that frame itself).
    zoom_restore: Option<Rect>,
    /// The current window's toolbelt.
    belt: toolbelt_ui::Belt,
    /// Bumped when something a toolbelt tool shows changes (`tools_changed`).
    tool_gen: u64,
    /// The keys whose press a focused toolbelt tool took: their releases are its too.
    belt_keys: toolbelt_tools_ui::TakenKeys<PhysicalKey>,
    /// The keys whose press a focused composer took: their releases are its too.
    composer_keys: toolbelt_tools_ui::TakenKeys<PhysicalKey>,
    /// Each pane's composer view (drawing, highlighting, suggestions; app-wide, by pane), its
    /// worker thread, the windows it opens, and the last multi-caret copy's groups.
    composers: HashMap<PaneId, composer_ui::View>,
    composer_worker: crate::composer_worker::Worker,
    /// Watches and scans the Scripts folder (started with the first window).
    scripts_scan: Option<crate::scripts_scan::Scanner>,
    /// The Scripts menu's tree from the last scan.
    scripts_tree: Vec<crate::scripts::Node>,
    /// Every script's relative name ("folder/name.py"), iTerm2's `allScripts`.
    scripts_all: Vec<String>,
    /// "Custom Python API Scripts Folder" and its text when the scanner last started: a change
    /// to either starts it again.
    scripts_keys: (bool, String),
    /// AutoLaunch was tried (`_ranAutoLaunchScript`): it runs once per Tandem run.
    autolaunch_ran: bool,
    composer_windows: composer_ui::Windows,
    composer_clip: Option<(String, Vec<usize>)>,
    /// The pane whose dropdown's Help button is under the pointer (its tooltip shows).
    composer_hover: Option<PaneId>,
    /// The tab and pane the composer's keyboard was last settled for (`composer_focus_follows`).
    composer_focus_key: Option<(usize, Option<PaneId>)>,
    /// The Actions as they were when the toolbelt opened the Edit Action sheet, and its window:
    /// the sheet's OK pushes them on that window's Actions undo (`iTermToolActions.m:267-276`).
    action_edit_undo: Option<(WindowId, toml::Value)>,
    /// What the current window's terminals were laid out for: the content area and every
    /// pane's terminal rectangle, per-pane bars included.
    laid_out: Option<crate::statusbar_layout::Room>,
    /// The save-arrangement window and what it saves, and menus to rebuild (arrangements
    /// changed).
    arrange_prompt: Option<(crate::prompt_ui::PromptWindow, arrange_ui::SaveWhat)>,
    menus_dirty: bool,
    /// The folder the next pane starts in, for a restored arrangement's session.
    start_dir_override: Option<crate::arrangement::Folder>,
    /// The command the next pane runs instead of its profile's (a git menu command's
    /// single-use window, the REPL's interpreter).
    start_command_override: Option<String>,
    /// The hops the next pane reconnects through (Duplicate Session), moved into the new pane
    /// by `start_pane_as`. Set and cleared around one start, so it is not per-window state.
    start_jumps_override: Option<Vec<crate::ssh_jumps::Reconnection>>,
    /// Variables the next pane gets over its session variables (the REPL's cookie and key; a
    /// secret, never logged). Set and cleared around one `open_window`.
    start_env_override: Option<Vec<String>>,
    /// The tab the next pane goes into once started (a split), for its `ITERM_SESSION_ID`;
    /// None is a new tab. Set and cleared around one start, so it is not per-window state.
    start_tab: Option<usize>,
    /// Sessions opened for a git menu command, by pane (app-wide).
    single_use: HashMap<PaneId, git_ui::SingleUse>,
    /// Undo Close: closed things waiting, and the close being collected now.
    undo: crate::undo_close::Stack<undo_ui::Undone>,
    undo_capture: Option<undo_ui::Capture>,
    /// The Edit Window Title or Edit Tab Title window, and what it names.
    title_prompt: Option<(crate::prompt_ui::PromptWindow, windows_ui::TitleEdit)>,
    /// The New Tab Group or Rename Tab Group window, and what it names (app-wide, like the
    /// title window).
    group_prompt: Option<(crate::prompt_ui::PromptWindow, tab_groups_ui::GroupPrompt)>,
    /// Windows to open at the next turn of the event loop (new windows need it).
    pending_windows: Vec<windows_ui::NewWindow>,
    /// Automatic Profile Switching: each session's stack, and this computer's name.
    aps: HashMap<PaneId, crate::aps::Switcher>,
    local_host: String,
    /// A coprocess's stderr waiting for its banner's answer: (command, text).
    coprocess_errors: HashMap<PaneId, (String, String)>,
    /// Files dropped on the window, handled together once the drop ends.
    dropped: Vec<(PaneId, String)>,
    api: Api,
    /// The session host this Tandem runs its shells in, and what it said at the handshake.
    host: Option<crate::host_client::Client>,
    /// The current window's hotkey state (swapped per window).
    hotkey: Option<hotkey_ui::HotkeyWin>,
    /// The current window's broadcast input state and its broadcasting tabs (swapped per
    /// window).
    broadcast: crate::broadcast::Broadcast,
    broadcast_tabs: std::collections::BTreeSet<u64>,
    /// The window, tab, and pane the menu checks were last synced for.
    checks_key: Option<(Option<WindowId>, usize, Option<PaneId>)>,
    /// The broadcast stripe tile (its size and pixels).
    stripes: Option<(u32, std::sync::Arc<Vec<u8>>)>,
    /// Profile hotkey ids (index + 1) to profile guids; failed registrations by guid.
    hotkey_ids: Vec<String>,
    hotkey_errors: std::collections::HashMap<String, String>,
    hotkeys_registered: bool,
    /// A Tandem window was in front at the last check (Reopen on activation).
    app_active: bool,
    /// Windows the app hotkey minimized, to bring back on the next press.
    app_hidden: Vec<WindowId>,
    host_welcome: Option<crate::host_client::Welcome>,
    shell: String,
    gpu: Option<Gpu>,
    tabs: Vec<Tab>,
    active: usize,
    panes: HashMap<PaneId, Pane>,
    next_id: PaneId,
    mods: Mods,
    /// Announcement bars waiting in each pane.
    bars: HashMap<PaneId, crate::infobar::Bars>,
    /// RequestAttention: pane flashes and fireworks (start time, and the cursor cell).
    flashes: HashMap<PaneId, Instant>,
    fireworks: HashMap<PaneId, (Instant, (usize, usize))>,
    /// SetColors `tab`: the tab color a pane's program chose.
    tab_colors: HashMap<PaneId, alacritty_terminal::vte::ansi::Rgb>,
    /// tmux windows opening in new Tandem windows: their saved origin and size, by tab.
    tmux_geometry: HashMap<u64, (Option<(i32, i32)>, Option<crate::tmux_persist::WinSize>)>,
    /// Each Tandem window's last place and size while not minimized (by window key).
    tmux_last_geometry: HashMap<u64, ((i32, i32), Option<crate::tmux_persist::WinSize>)>,
    /// Background images by path (None for a file that does not load), and folder
    /// rotations by folder and interval.
    bg_images: HashMap<PathBuf, Option<Arc<crate::bgimage::Loaded>>>,
    rotators: HashMap<(PathBuf, u64), bgimage_ui::Rotator>,
    /// When the last StealFocus question closed.
    steal_asked: Option<Instant>,
    /// When a pane flash or fireworks needs the next frame.
    attention_due: Option<Instant>,
    /// A short message at the bottom of the window, and when it goes away.
    toast: Option<(String, Instant)>,
    /// The block under the pointer (pane, number, first and last line), and the drawn pill
    /// buttons.
    block_hover: Option<(PaneId, u32, i32, i32)>,
    block_buttons: Vec<blocks_ui::BlockButton>,
    /// Porthole layouts by pane and id, and the portholes drawn last frame.
    porthole_views: HashMap<(PaneId, u32), porthole_ui::PView>,
    porthole_hits: Vec<porthole_ui::PortholeHit>,
    /// A link under a porthole press (opened when the button comes up without a drag), the
    /// links waiting on a bar, and the language list a popup shows.
    porthole_link: Option<(PaneId, u32, String)>,
    porthole_link_asks: HashMap<PaneId, crate::porthole::LinkTarget>,
    porthole_languages: Option<(PaneId, u32, Vec<crate::porthole::detect::Language>)>,
    /// The status bar menu's target, until its pick arrives.
    status_menu: Option<statusbar_ui::StatusMenu>,
    /// The status bar setup window (one at a time, like iTerm2's sheet).
    status_setup: Option<statusbar_ui::StatusSetup>,
    /// The next whole second a status bar Clock redraws at.
    status_clock_due: Option<Instant>,
    /// The status bar graphs' meters and their Windows samplers (app-wide: every window's
    /// graphs show the same machine), and the slide animation's next frame.
    meters: crate::statusbar_metrics::Meters,
    samplers: crate::metrics_win::WinSamplers,
    graph_frame_due: Option<Instant>,
    /// The git components' pollers by pane (a pane's first git component), the worker
    /// that runs git off this thread, and the panes whose bar placed its git component at
    /// the last draw (app-wide: keyed by pane, like `activity`).
    git: HashMap<PaneId, git_ui::GitPane>,
    git_worker: git_ui::GitWorker,
    git_placed: std::collections::HashSet<PaneId>,
    /// The git Log popover, and a log that arrived for one (the pane it is for).
    git_log: Option<crate::git_log_ui::GitLogWindow>,
    git_log_ready: Option<(PaneId, String)>,
    /// Triggers: the Show Alert limit, messages already shown, the Alert bar's trigger, and
    /// Run Command's pool and processes.
    trigger_limits: crate::triggers::RateLimit<(PaneId, usize)>,
    trigger_told: std::collections::HashSet<(PaneId, String)>,
    trigger_alerts: HashMap<PaneId, usize>,
    trigger_pool: crate::triggers::CommandPool,
    trigger_children: Vec<(PaneId, String, std::process::Child)>,
    /// Each pane's bell state, and the panes whose tab shows a bell icon.
    bells: HashMap<PaneId, crate::bell::Bell>,
    bell_icons: std::collections::HashSet<PaneId>,
    /// Each pane's output and idle state.
    activity: HashMap<PaneId, crate::activity::Activity>,
    /// Each pane's badge layout, with what it was made for.
    badges: HashMap<PaneId, (String, Option<crate::badge::BadgeLayout>)>,
    /// Notifications: the session of the last one, the escape-sequence limit, Notify on
    /// Status Change, and the idle codes sent.
    last_notified: Option<PaneId>,
    alert_limits: crate::triggers::RateLimit<PaneId>,
    notify_status_armed: bool,
    last_status: HashMap<PaneId, Option<String>>,
    /// Session Status' bell on one session: it and its status when armed; the next change
    /// from that shows the alert, then it disarms (`NotifyOnStatusChangeController`).
    status_watch: Option<(PaneId, Option<String>)>,
    /// Session Status' settings popover (one at a time: it closes when it loses focus), when
    /// it last closed (a press on its gear that took the focus away closes it, not reopens
    /// it), and its + prompt.
    status_popover: Option<crate::status_popover_ui::StatusPopover>,
    status_popover_closed: Option<Instant>,
    /// View > Open Quickly: the palette and its preview while open, the query kept for the
    /// next open, and each pane's focus ordinal (bumped when it becomes the focused pane, the
    /// palette's recency tiebreaker; `lastActivityOrdinal`, kept for this run only).
    open_quickly: Option<open_quickly_app::Quickly>,
    quickly_query: String,
    focus_order: HashMap<PaneId, u64>,
    focus_seq: u64,
    status_prompt: Option<crate::prompt_ui::PromptWindow>,
    /// The Notes tool's global note (one for every window) and its file.
    global_note: crate::notes::GlobalNote,
    /// Jobs' Notify on Termination: each watched process's pid, creation time and name.
    job_watches: Vec<(u32, u64, String)>,
    /// The shell each window's Jobs shows, the trees the watcher keeps for them, the tree
    /// generation last seen, and when the termination watches look next.
    jobs_wanted: HashMap<WindowId, u32>,
    jobs_wanting: std::collections::HashSet<u32>,
    jobs_tree_seen: u64,
    jobs_watch_at: Option<Instant>,
    /// Jobs' Inspect popover (and when it last closed), and the signal question with what it
    /// will send (the window opens on the next loop turn) and, in the remembered mode, the
    /// saved answer.
    jobs_popover: Option<crate::status_popover_ui::InfoPopover>,
    jobs_popover_closed: Option<Instant>,
    jobs_confirm: Option<(Option<crate::prompt_ui::ConfirmWindow>, toolbelt_more_ui::SignalRequest, Option<String>)>,
    /// Send Signal's console events on their way from a local pane (app-wide): each result is
    /// collected when its thread ends, so a close in progress never holds the window up.
    signal_waits: Vec<std::thread::JoinHandle<Result<(), String>>>,
    idle_sent: HashMap<PaneId, Instant>,
    started: Instant,
    /// An open Make Screenshot panel.
    shot: Option<screenshot_ui::Shot>,
    /// This window's Instant Replay Control panel (per window: `swap_live`).
    replay_window: Option<crate::replay_window::ReplayWindow>,
    /// The tab and pane in front when last looked, for what ends Instant Replay (per window).
    replay_seen: Option<(u64, PaneId)>,
    /// This window's password manager (per window: `swap_live`).
    pwm_window: Option<pwm_app::PwmWin>,
    /// What Shell > ssh > Disconnect last showed in this window (per window: `swap_live`).
    ssh_menu_shown: Option<Option<String>>,
    /// The password manager's app-wide state: authentication, the source worker, the echo
    /// probes' passwords, broadcast rounds and questions.
    pwm: pwm_app::PwmState,
    /// Keys whose press Instant Replay took, so their releases are taken too.
    replay_keys: toolbelt_tools_ui::TakenKeys<PhysicalKey>,
    /// Import and Export Recording's warnings, each opened on the next pass (it needs the
    /// event loop); app level, as iTerm2's are (`window:nil`).
    recording_warnings: Vec<(Option<crate::prompt_ui::ConfirmWindow>, &'static str, String)>,
    /// A loaded recording's pane, entering replay on the next pass (`RC:124-126`).
    recording_replay: Option<PaneId>,
    /// Page Setup's printer, paper, and margins.
    print_setup: crate::winprint::Setup,
    /// OpenURL requests waiting for an answer, by pane and host; uploads being sent.
    pending_urls: HashMap<(PaneId, String), String>,
    uploads: HashMap<PaneId, prompted_ui::Upload>,
    /// Each pane's pastes being written (the queue, how they are written, and whether the
    /// progress bar shows), and pastes waiting on a question.
    pastes: HashMap<PaneId, (crate::paste::Queue, crate::broadcast::Write, bool)>,
    paste_asks: HashMap<PaneId, paste_ui::Asking>,
    /// Paste history (read on first use), its popup, Advanced Paste (the pane it pastes
    /// into; whether it came from the menu), and an "Advanced..." waiting for the loop.
    paste_history: Option<crate::paste_history::History>,
    paste_history_win: Option<(crate::list_popup_ui::ListPopup, PaneId)>,
    advanced_paste_win: Option<(crate::advanced_paste_ui::AdvancedPasteWindow, PaneId, bool)>,
    advanced_paste_request: Option<(PaneId, String)>,
    /// The shell history (read on first use), what each pane fed it, and the list popup
    /// (Command History, Recent Directories) with its pane, purpose, and values.
    shell_history: Option<crate::shell_history::Store>,
    /// The snippets file (Edit > Snippets, Send Snippet, Settings > Shortcuts).
    snippets: crate::snippets::Store,
    /// Each session's clippings and panel, the New Clipping sheet (for its pane), and the
    /// panel button under the pointer.
    clips: HashMap<PaneId, clippings_ui::ClipState>,
    /// Each pane's inline chat (its binding and panel state), app-wide as `clips` is.
    inline: HashMap<PaneId, inline_chat_ui::InlineState>,
    add_clip_win: Option<(crate::clip_add_ui::AddClipWindow, PaneId)>,
    clip_hover: Option<(PaneId, crate::clippings::Button)>,
    shell_fed: HashMap<PaneId, shell_history_ui::Fed>,
    list_popup: Option<(crate::list_popup_ui::ListPopup, PaneId, shell_history_ui::PopupFor, Vec<String>)>,
    /// The completion list at a pane's cursor, and Auto Command Completion's typed text
    /// and when it may show.
    completion: Option<(PaneId, crate::completion::List)>,
    auto: (Option<PaneId>, crate::completion::Auto),
    upload_due: Option<Instant>,
    /// SetProfileProperty values and SetBackgroundImageFile paths waiting for an answer.
    pending_properties: HashMap<PaneId, Vec<(String, toml::Value)>>,
    pending_background: HashMap<PaneId, String>,
    /// OSC 52 reads waiting for the user's answer, with the selection parameter to echo.
    pending_reads: HashMap<PaneId, String>,
    /// Keys whose press went to a program, so their release may too.
    sent_keys: keyproto_ui::Held,
    /// The physical key of the press being routed now.
    typing: Option<PhysicalKey>,
    /// A key press (typed or a Win chord from the hook) is being routed now: iTerm2's
    /// `[NSApp currentEvent]` is a key-down, which Window > Zoom's Shift rule asks.
    key_down: bool,
    cursor: (f32, f32),
    drag: Drag,
    clicks: Clicks,
    wheel_acc: f32,
    focused: bool,
    /// The tab bar as last laid out, in points, not scrolled (`build_frame`).
    tab_bar: tab_layout::Bar,
    /// The tab bar's scroll offset in points.
    tab_scroll: f32,
    /// The tab under the pointer, and whether the pointer is on its close button.
    tab_hover: Option<(usize, bool)>,
    /// Every layout brings the selected tab into view, until the bar is scrolled by hand.
    tab_keep_selected: bool,
    /// The selected tab's id and the tab count at the last layout: a change selects anew.
    tab_seen: Option<(u64, usize)>,
    /// The press on the tab bar: what it hit, its click count (0 for the middle button),
    /// whether Alt was held, and the id of the tab it hit.
    tab_press: Option<TabPress>,
    /// `SelectsTabsOnMouseDown` as it was when this window opened (iTerm2 copies it into each
    /// tab bar it makes, `iTermTabBarControlView.m:58`; a change affects new windows only).
    tab_selects_on_down: bool,
    /// `MinimumTabDragDistance` in pixels, read once at startup (iTerm2 reads it when it makes
    /// a tab bar, `iTermTabBarControlView.m:44`; its help asks for a restart).
    min_tab_drag: f32,
    /// The slot a dragged tab would drop at in this window's bar, while a tab drag is under
    /// way over it (the drop marker).
    tab_drop_marker: Option<usize>,
    /// The tab bar, each tab's rect and each collapsed group's chip as last drawn, pixels,
    /// window coordinates (None without a bar): where a tab dragged from another window drops
    /// (`window_bar_hit`, `window_bar_drop`).
    tab_bar_drawn: Option<(Rect, Vec<Rect>, Vec<crate::tab_drag::Pill>)>,
    /// The collapsed group a dragged tab would join if dropped now, and its window: that
    /// window's bar frames it in the accent color (`PSM collapsedTabGroupDropTargetIdentifier`).
    /// App-wide: one pointer drags.
    tab_join: Option<(WindowId, String)>,
    /// A side tab bar's preferred width in points: `LeftTabBarWidth` when the window opened,
    /// then the width its drag handle sets.
    side_bar_preferred: f32,
    /// A side tab bar's scroll offset in points (`PSM _scrollOffset` on the y axis).
    side_bar_scroll: f32,
    /// The tab bar's flash in full screen (`tab_flash`).
    tab_flash: crate::tab_flash::Flash,
    /// The window kept its size when the tab bar showed, to stay on screen, so it keeps it
    /// when the bar hides again (`PT _excursionPrevented`).
    excursion_prevented: bool,
    /// The tab bar took room from the sessions at the last layout, full screen aside: a
    /// change fits the window to the bar (`fit_window_to_bar`).
    bar_was_visible: bool,
    /// How much the window grew (pixels, width and height) when the tab bar last showed:
    /// the bar hiding shrinks it by exactly that (`tab_position::bar_growth`).
    bar_grown: (f32, f32),
    /// A side tab bar's group chips as last drawn (`scene::tab_bar_vertical`), pixels, window
    /// coordinates, with their group ids.
    side_chips: Vec<(Rect, String)>,
    /// The pointer is on a side bar's drag handle, so the resize pointer shows.
    side_handle_hover: bool,
    /// The tab icon maps, bundled with the user's copies merged over (`tab_icons::load_maps`),
    /// loaded at startup and by `reload_tab_icon_maps`.
    tab_icon_maps: crate::tab_icons::Maps,
    /// Each pane's tab icon and what it was worked out from (`tab_icon_for`), by pane id.
    tab_icon_cache: std::cell::RefCell<HashMap<PaneId, (tab_groups_ui::IconKey, Option<scene::TabImage>)>>,
    /// Decoded icon pictures: tinted bundled icons, the user's PNGs and custom icons, each
    /// loaded once (None: nothing could be read).
    tab_icon_images: std::cell::RefCell<HashMap<tab_groups_ui::ImageId, Option<scene::TabImage>>>,
    /// The tab whose tooltip the pointer is waiting on or showing, and when the pointer
    /// settled on it; the tip shows `TIP_DELAY` later.
    tab_tip: Option<(usize, Instant)>,
    /// When a tab tooltip is next due to show, in any window (there is one pointer).
    tab_tip_wake: Option<Instant>,
    /// The last left press on a tab bar (any window): when, on which tab (None off the tabs)
    /// and its click count.
    tab_clicks: (Instant, Option<usize>, u8),
    banners: Banners,
    /// The `utilities` folder beside tandem.exe, when `utilities::ensure` made it at start.
    utilities: Option<PathBuf>,
    clipboard: Option<arboard::Clipboard>,
    store: Store,
    settings: Option<SettingsWindow>,
    profiles_win: Option<SettingsWindow>,
    /// When unsaved settings edits are due to be written.
    save_due: Option<Instant>,
    /// Session restoration: when to save the windows, whether saving is over (quit, or the
    /// last window closed), and a focus loss waiting to be saved.
    state_debounce: crate::restore_state::Debounce,
    state_frozen: bool,
    focus_lost: bool,
    /// The font settings the renderer last loaded.
    font_sig: String,
    /// Cursor blink phase, and when it last flipped.
    blink_on: bool,
    last_blink: Instant,
    /// The Python API's server while it runs (app-wide, as every API field is: none goes in
    /// `swap_live`). Not `api`, which is the ConPTY API.
    api_server: Option<crate::api_server::Server>,
    /// The cookies handed to sessions, shared with the server's cookie check.
    api_jar: Arc<Mutex<crate::api_auth::Jar>>,
    /// The origin secrets handed to it2ssh (app-wide, not in `swap_live`), shared with the
    /// secret pipe's thread; 14b's conductor hook checks them.
    pub(super) ssh_secrets: std::sync::Arc<std::sync::Mutex<crate::ssh_secret::Secrets>>,
    /// The framed conductors of every window, by identity (app-wide, not in `swap_live`):
    /// 14d's Download Files reads it. Only `ssh_apply` and the pane drop paths change it.
    ssh_registry: crate::ssh_registry::Registry,
    /// The secret pipe's server, started with the first window; None when it could not start.
    ssh_secret_server: Option<crate::ssh_secret_win::Server>,
    /// The Script Console's entries: scripts, connections and the app-wide logs.
    script_history: crate::script_history::History,
    /// The API's alerts, shown one at a time in order; each window opens on a pass (it needs
    /// the event loop).
    api_alerts: Vec<(Option<crate::prompt_ui::ConfirmWindow>, api_ui::Alert)>,
    /// The server runs.
    api_on: bool,
    /// Connections waiting for the "Allow Python API Usage?" answer.
    api_pending: HashMap<u64, crate::api_server::Pending>,
    /// The corrupt allow-all file contents the reset warning was last shown for.
    api_reset_seen: Option<String>,
    /// "Enable Python API" and "Allow all apps to connect" as last acted on, so a change of
    /// another setting does not act on them again.
    api_settings_seen: (bool, bool),
    /// "Enable Python API?" was confirmed (its marker file exists): the server may start.
    api_consent: bool,
    /// The notification subscriptions of upgraded connections.
    api_subs: crate::api_dispatch::Subscriptions,
    /// The transaction, if one runs, and the requests held behind it.
    api_tx: crate::api_dispatch::Transactions,
    /// Connections to authorize and refusals that arrived while a transaction froze the UI
    /// thread; `api_tick` takes them first.
    api_deferred: VecDeque<crate::api_server::Event>,
    /// User variables set through the API, and each scope's last observed values.
    api_vars: crate::api_vars::Store,
    /// The VARIABLE_CHANGE subscriptions of upgraded connections.
    api_var_subs: crate::api_vars::Subs,
    /// The session notification subscriptions of upgraded connections (keystrokes and their
    /// filter, screen updates, prompts, custom escape sequences), and the "all" ones.
    api_session_subs: crate::api_notify::SessionSubs,
    /// Panes that got output (the output loop, tmux integration output, Inject) since screen
    /// updates were last posted, each once.
    api_dirty: Vec<PaneId>,
    /// The snapshot notifications were last computed from, kept only while a connection
    /// subscribes to an app-wide notification.
    api_last: Option<crate::api_model::Model>,
    /// The sessions (panes in windows) when "all" session requests were last applied, kept
    /// only while one is kept: a new one takes them.
    api_all_seen: Option<std::collections::BTreeSet<PaneId>>,
    /// The modifier keys held, from their presses and releases (`api_key_early`), for the
    /// FLAGS_CHANGED a focus loss implies.
    api_held: Mods,
    /// Tmux SendCommand requests waiting for their reply, by gateway and command tag: the
    /// connection and the request.
    api_tmux_commands: HashMap<(PaneId, crate::tmux_gateway::Tag), (u64, crate::api_proto::pb::ClientOriginatedMessage)>,
    /// Tmux CreateWindow requests waiting for the `new-window` reply, by gateway and tag.
    api_tmux_creates: HashMap<(PaneId, crate::tmux_gateway::Tag), (u64, crate::api_proto::pb::ClientOriginatedMessage)>,
    /// Tmux CreateWindow requests waiting for the new window's tab, by gateway and tmux window.
    api_tmux_windows: HashMap<(PaneId, u32), (u64, crate::api_proto::pb::ClientOriginatedMessage)>,
    /// Tmux CreateWindow requests whose tab (gateway, tab id) waits to become a window.
    api_tmux_tabs: Vec<(PaneId, u64, (u64, crate::api_proto::pb::ClientOriginatedMessage))>,
    /// The functions scripts registered and the calls waiting for them.
    api_fns: crate::script_fn::Registry,
    /// Finished script calls (a result, a failure, a timeout, a closed connection) by the
    /// caller's token, for the invocations waiting on them.
    api_rpc_done: VecDeque<(u64, Result<crate::expr::Value, crate::expr::Failure>)>,
    /// Each allowed connection's `x-iterm2-key` and display name, for its log lines.
    api_keys: HashMap<u64, (String, String)>,
    /// Running invocations by id (app-wide, as every script field is), the next run id and
    /// call token, and the run each started call's token belongs to.
    script_runs: BTreeMap<u64, script_ui::Run>,
    script_next: u64,
    script_tokens: HashMap<u64, u64>,
    /// "Function Call Failed" alerts (and results) waiting to show, and the one showing (true:
    /// in the remembered mode).
    script_alerts: VecDeque<script_ui::Alert>,
    script_alert: Option<(crate::prompt_ui::ConfirmWindow, script_ui::Alert, bool)>,
    /// The provisioning worker (started on first use), what each submitted job continues
    /// with, the install each job queued behind one needs (dependent id to install id), the
    /// installed-Python probes running off the UI thread, and whether Tandem's uv is installed
    /// (the runtime menu item's title). Also the environment each submitted job makes, and the
    /// requests waiting for an environment already being asked about or built.
    provisioner: Option<crate::script_provision::Provisioner>,
    provision_after: HashMap<u64, script_runtime_ui::After>,
    provision_needs: HashMap<u64, u64>,
    provision_targets: HashMap<u64, crate::script_provision::Target>,
    provision_waiting: Vec<script_runtime_ui::Waiting>,
    backend_probes: Vec<script_runtime_ui::Probe>,
    uv_installed: bool,
    /// The alerts (iTerm2's warnings shown as windows) waiting to show, and the one showing.
    scripts_alerts: VecDeque<scripts_alerts::Ask>,
    scripts_alert: Option<(crate::prompt_ui::ConfirmWindow, scripts_alerts::Ask)>,
    /// iTerm2's suppressed alerts (app-wide): the catalog of warnings answered automatically,
    /// "Always show alerts with remembered selections" (not saved, as iTerm2's process global),
    /// and when the catalog's write is due.
    suppressed: crate::warnings::Catalog,
    show_remembered: bool,
    catalog_due: Option<Instant>,
    /// `take_inline_asks` is asking: a released pane's held output is taken by its loop.
    inline_asking: bool,
    /// The Suppressed Alerts window (one per app) and when the menu title's "ago" is next
    /// refreshed (while something is suppressed).
    suppressed_window: Option<crate::suppressed_alerts_ui::SuppressedWindow>,
    suppressed_retitle_due: Option<Instant>,
    /// The AI alert queue (ruling 12c-7), the alert showing, and the Settings > General > AI
    /// sub-tab an answer asked to reveal.
    ai_asks: VecDeque<crate::ai_gate::Alert>,
    ai_alert: Option<(crate::prompt_ui::ConfirmWindow, crate::ai_gate::Alert)>,
    ai_reveal: Option<crate::settings_ai::SubTab>,
    /// The one-vendor key prompts waiting to show (ruling 12e-1), and the one showing.
    ai_key_asks: VecDeque<(crate::ai_settings::Vendor, ai_app::KeyThen)>,
    ai_key_window: Option<(crate::ai_key_ui::KeyWindow, crate::ai_settings::Vendor, ai_app::KeyThen)>,
    /// The discovered Ollama models (app-wide, never swapped), the transport AI requests go
    /// through, and the refresh the settings window waits on.
    ai_discovery: crate::ai_discovery::Cache,
    ai_transport: Arc<dyn crate::ai_transport::Transport>,
    ai_probe: Option<crate::ai_discovery::Ticket>,
    /// 12e's AI requests in flight and the next id (app-wide, never swapped; ruling R12e-1).
    ai_runs: HashMap<u64, ai_runs::Run>,
    ai_runs_next: u64,
    /// Composer suggestions: the run in flight for each pane (ruling P12).
    composer_ai: HashMap<PaneId, u64>,
    /// Codecierge per terminal session, keyed by its session guid (`CG:554-567`). App-wide, not
    /// in `swap_live`: a session is one pane whichever window shows it.
    codecierge: HashMap<String, crate::codecierge::Session>,
    /// What the Explain Output items show now (app-wide), so a selection change refreshes them.
    explain_shown: Option<bool>,
    /// Engage AI: the dialog showing (with the pane and what it asked), the one queued to show,
    /// the wait window (with its run), and what each Engage run was started from.
    ai_engage: Option<(crate::ai_engage_ui::EngageWindow, PaneId, crate::ai_engage::Ask)>,
    ai_engage_open: Option<(PaneId, crate::ai_engage::Ask)>,
    ai_wait: Option<(crate::ai_engage_ui::WaitWindow, u64)>,
    ai_engage_pending: HashMap<u64, ai_engage_app::EngageFrom>,
    /// Explain Output with AI (app-wide, never swapped): a request held while the key prompt
    /// shows, with its chat title, and each hidden annotation the processor placed (its id to the
    /// pane and note).
    ai_explain_waiting: Option<(crate::ai_explain::Request, String)>,
    ai_annotations: HashMap<String, (PaneId, u32)>,
    /// The AI chats (app-wide, never swapped): the chat database, opened on first use, whether
    /// opening it failed (then it is not tried again), and the broker.
    chat_db: Option<crate::chat_store::Store>,
    chat_db_failed: bool,
    /// The cleanup of an earlier run's chat links failed: no chat binds to a pane this run.
    chat_links_off: bool,
    chat_broker: crate::chat_broker::Broker,
    /// The AI Chat window (app-wide, never swapped: one per app), the broker subscription it
    /// reads every chat through, the chat it shows, and its toolbar as last computed (rebuilt
    /// only when `chat_toolbar_dirty`, so a streamed chunk never re-reads the providers).
    chat_window: Option<crate::chat_window_ui::ChatWindow>,
    chat_sub: Option<crate::chat_broker::SubId>,
    chat_shown: Option<String>,
    /// The shown chat's input (text, pending attachments), the drafts of the chats not shown,
    /// and the paths dropped on the chat window since the last pass (app-wide: the window is).
    chat_input: crate::chat_input::InputState,
    chat_drafts: crate::chat_input::Drafts,
    chat_drops: Vec<std::path::PathBuf>,
    /// The find bar of the chat window (app-wide, as the window is): the query the next bar
    /// opens with (Tandem has no shared find pasteboard), and the bar's state while it is open.
    chat_find_query: String,
    chat_find: chat_window_app::ChatFind,
    chat_toolbar: crate::chat_window_ui::ToolbarData,
    chat_toolbar_dirty: bool,
    /// The chat list: its rows as last built, whether they need building again, when they were
    /// built (ages refresh every 60 s, `CCV:66-70`), each chat's row snippet until a message
    /// of it changes, and the chats a right-click menu acts on (all app-wide).
    chat_rows: Vec<crate::chat_list::Row>,
    chat_rows_dirty: bool,
    chat_rows_built: Option<Instant>,
    chat_snippets: HashMap<String, String>,
    /// The chat window's search across chats: its query, debounce and streamed hits.
    chat_search: crate::chat_search::State,
    chat_menu_ids: Vec<String>,
    /// The shown chat's messages, kept current from the broker, and the conversation rows made
    /// from them (rebuilt only when `chat_convo_dirty`, never per frame).
    chat_msgs: Vec<crate::chat_message::Message>,
    chat_convo: std::sync::Arc<Vec<crate::chat_window_ui::RowData>>,
    chat_convo_dirty: bool,
    /// A pick of a session for a chat is open (app-wide; every terminal window shows the picker).
    chat_picker: Option<chat_session_app::Pick>,
    /// The picker buttons this window drew, per pane (this window's, in `WinSlot`).
    picker_hits: Vec<(PaneId, Rect)>,
    /// The inline chat panels' rectangles this window drew, for a press on them.
    inline_hits: Vec<inline_chat_ui::PanelHit>,
    /// The AI indicators this window drew (this window's, in `WinSlot`).
    chat_indicator_hits: Vec<(PaneId, Rect, scene::IndicatorKind)>,
    /// Chats streaming their linked session's finished commands (app-wide, keyed by chat).
    streaming_chats: std::collections::HashSet<String>,
    /// The indicator menu on screen: the pane and the routes its items run (`chat_session_app`).
    chat_indicator_menu: Option<(PaneId, Vec<crate::chat_routes::Route>)>,
    /// Every chat's linked session, read for the indicators at most once a second.
    chat_links: std::cell::RefCell<Option<(Instant, Vec<(String, String)>)>>,
    /// The subscription that hears every chat for as long as the store is open, so a minted
    /// title needs no window (ruling F14).
    chat_mint_sub: Option<crate::chat_broker::SubId>,
    /// The rename prompt and its chat.
    chat_prompt: Option<(crate::prompt_ui::PromptWindow, String)>,
    /// The minted-title runs in flight, each with the endpoint its gate is re-checked for every
    /// pass (ruling R12cd-1); app-wide.
    title_runs: HashMap<String, (crate::ai_turn::Runner, crate::ai_models::Endpoint)>,
    /// The chats' permission overrides (app-wide, never swapped); every chat's stored
    /// permissions are merged in when the database opens.
    ai_overrides: crate::ai_permissions::Overrides,
    /// The text an AI typed onto each pane's prompt line without submitting it (ruling
    /// R12cd-2), app-wide, the one accumulator 12f's orchestration typing shares.
    ai_staged: HashMap<PaneId, crate::ai_actions::Staged>,
    /// The "Share Terminal Contents Automatically?" question was asked this launch (once per
    /// launch, `didAskAutoProvideConsentThisSession`), app-wide.
    ai_consent_asked: bool,
    /// Each chat's last screen a request really carried (`lastAutoProvidedScreen`), keyed by
    /// chat id, app-wide.
    ai_watermarks: HashMap<String, crate::ai_context::Watermark>,
    /// The command the model is running on each pane; `ai_exec_tick` finds its end.
    ai_execs: HashMap<PaneId, crate::ai_exec::Running>,
    /// (chat, request id) of commands a newer command replaced, answered by `ai_exec_tick`.
    ai_exec_canceled: Vec<(String, String)>,
    /// Each pane's Get-Help running on a worker; `ai_help_tick` collects the answer.
    ai_helps: HashMap<PaneId, std::sync::mpsc::Receiver<String>>,
    /// The commands' safety checks by id (Task 10); `ai_check_tick` hands the finished ones on.
    ai_checks: HashMap<u64, ai_safety_app::Check>,
    ai_next_check: u64,
    /// The commands held behind the one-time safety question (ruling R12cd-3).
    ai_nag: crate::ai_classifier::NagGate,
    /// Each chat's agent by chat id (app-wide, never swapped): its turn, parks and vector-store
    /// flow (12d reads it by name).
    chat_agents: HashMap<String, crate::chat_agent::Agent>,
    /// (chat, request id) of the Get-Help running on each pane (minor m1), app-wide.
    ai_pending_tools: HashMap<PaneId, (String, String)>,
    /// 12b's wire log file for the chat turns' requests (app-wide).
    #[cfg(windows)]
    ai_wire_log: Arc<Mutex<crate::ai_wire_log::WireLog>>,
    /// Launched scripts' processes by their Script Console entry's id (their key), the
    /// launches waiting for "Enable Python API?" to be answered, and the script whose
    /// failure notification was posted last (a click reveals its entry; a session's alert
    /// takes the click over, as `last_notified`).
    script_procs: HashMap<String, crate::script_proc::ScriptProcess>,
    /// The jobs of scripts that ended by themselves while something they started still runs;
    /// dropped when that ends (closing a job ends what is in it, so Tandem's quit ends them).
    script_jobs_left: Vec<crate::script_provision::KillJob>,
    script_after_enable: Vec<crate::script_launch::Request>,
    /// Open Python REPL: the interpreter waiting for "Enable Python API?", and the one whose
    /// window opens on the next loop pass (which has the event loop).
    repl_after_enable: Option<PathBuf>,
    repl_pending: Option<PathBuf>,
    /// Panes that stay open with their output when their program ends (the REPL's), each
    /// with whether its end was already reported (`scripts_app::on_exit`).
    held_on_exit: HashMap<PaneId, bool>,
    /// The import under way (one at a time, iTerm2's `sInstallingScript`) and the files
    /// waiting behind it (iTerm2's serial install queue), each with whether it is an archive
    /// found in the Scripts folder (recycled once installed) and whether it is imported with
    /// no UI (Some: into AutoLaunch or not; `import_quietly`).
    importing: Option<scripts_app::Import>,
    import_queue: VecDeque<(PathBuf, bool, Option<bool>)>,
    /// Archives the scan found that were already asked about (`didFindScriptArchive:`'s
    /// `alreadyFound`).
    archives_asked: std::collections::HashSet<PathBuf>,
    /// Export's chooser (app-wide) and the exports running off the UI thread.
    script_export: Option<crate::script_export_ui::ExportWindow>,
    script_exports: Vec<std::sync::mpsc::Receiver<Result<PathBuf, String>>>,
    /// New Python Script's picker (app-wide), and the Python versions its popup is waiting
    /// for (a `Minors` job run off the UI thread).
    script_picker: Option<crate::script_new_ui::PickerWindow>,
    script_picker_minors: Option<std::sync::mpsc::Receiver<Vec<String>>>,
    /// Manage Dependencies (app-wide): its window, its state (kept while the window is
    /// closed, so a command still finishes), and the pip command whose window's end it
    /// waits for.
    script_deps: Option<crate::script_deps_ui::DepsWindow>,
    script_deps_state: script_deps_app::Deps,
    script_deps_pending: Option<(PaneId, script_deps_app::Pending)>,
    /// The Script Console and the Scripting Inspector (app-wide).
    script_console: Option<crate::script_console_ui::ConsoleWindow>,
    script_inspector: Option<crate::script_inspector_ui::InspectorWindow>,
    /// An entry to reveal in the console on the next loop pass (asked where no event loop
    /// was at hand).
    script_console_reveal: Option<String>,
    last_notified_script: Option<String>,
    /// Status bar strings with function calls, by session and string: their shown values
    /// and running evaluations (app-wide; a pane's entries go with the pane).
    script_templates: HashMap<(PaneId, String), script_ui::Template>,
    /// The last bar layout pass that asked for status bar strings (`begin_script_pass`).
    script_pass: u64,
    /// Status bar component unread counts set by scripts: by component identifier, and per
    /// session (which wins when set, `iTermStatusBarUnreadCountController.m:51-60`), drawn as
    /// a badge on the component.
    unread_counts: HashMap<String, i64>,
    unread_session_counts: HashMap<(PaneId, String), i64>,
    /// Script status bar components by session and unique id: what they show and their
    /// evaluations (app-wide; a pane's entries go with the pane).
    script_components: HashMap<(PaneId, String), script_ui::Component>,
    /// Session titles from a script's title provider (Title Components Custom with a Title
    /// Function), by session: the value shown and its evaluations (app-wide; a pane's entry
    /// goes with the pane).
    script_titles: HashMap<PaneId, script_ui::TitleState>,
    /// Script component configurations parsed, by their saved registration's text (emptied
    /// whenever the registrations change).
    script_parsed: HashMap<String, Option<std::rc::Rc<script_ui::Parsed>>>,
    /// The popover a StatusBarComponentRequest opened (one at a time) and its session.
    text_popover: Option<(PaneId, crate::status_popover_ui::TextPopover)>,
    /// The session title providers, as of the registry revision they were listed at.
    script_title_providers: Option<(u64, std::rc::Rc<Vec<crate::script_fn::TitleProvider>>)>,
    /// `alert` and `get_string` dialogs waiting to open, and the one showing.
    script_dialogs: VecDeque<builtin_ui::Dialog>,
    script_dialog: Option<builtin_ui::Open>,
    /// `run_tmux_command` calls waiting for tmux's reply, by (gateway, tag), with their token.
    script_tmux: HashMap<(PaneId, crate::tmux_gateway::Tag), u64>,
    /// `move_tab_to_window` calls waiting for their tab's new window: (tab id, token).
    script_moved: Vec<(u64, u64)>,
    /// "Load settings from a custom folder or URL" (app-wide): what the location held when it
    /// was loaded or last written (iTerm2's `savedRemotePrefs`), when the automatic save is due,
    /// a URL load running off the UI thread, whether Tandem ends once the quit question is
    /// answered, the launch's questions waiting for the first window, whether Settings opens at
    /// General on the next pass (Reveal in Settings), and the GPU the quit question opens with
    /// after the last window has gone.
    prefs_saved: Option<crate::remote_prefs::Snapshot>,
    prefs_save_due: Option<Instant>,
    prefs_fetch: Option<std::sync::mpsc::Receiver<Result<String, (i32, String)>>>,
    prefs_exit_pending: bool,
    prefs_startup: Vec<scripts_alerts::Ask>,
    prefs_reveal: bool,
    prefs_gpu: Option<(wgpu::Instance, wgpu::Adapter, wgpu::Device, wgpu::Queue)>,
    /// The custom folder's location as typed at launch; None once it was edited this session,
    /// which skips the quit step (`customFolderChanged`).
    prefs_launch_folder: Option<String>,
    /// Import All Settings and Data (app-wide): Tandem starts again once the event loop ends
    /// (`run()`), the quit that restart needs is due on the next loop pass, the script zips an
    /// import left for this run to install, and their folder, removed once they are done.
    relaunch_on_exit: bool,
    relaunch_due: bool,
    restored_scripts: Vec<PathBuf>,
    restored_dir: Option<PathBuf>,
}

/// pwsh.exe when it is on the PATH, else Windows PowerShell.
fn default_shell() -> String {
    let found = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("pwsh.exe").is_file()))
        .unwrap_or(false);
    if found { "pwsh.exe -NoLogo".into() } else { "powershell.exe -NoLogo".into() }
}

fn message_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Tandem").as_ptr(), MB_OK | MB_ICONERROR) };
}

pub fn run() {
    // A cookie in Tandem's own environment (Tandem started from a Tandem shell) must reach no
    // session and no child: only a cookie Tandem mints for a session is ever set.
    for name in crate::session_env::NEVER_INHERITED {
        // SAFETY: the first thing Tandem does, before any thread (the event loop, tandem-host,
        // the API server) exists, so nothing reads the environment meanwhile.
        unsafe { std::env::remove_var(name) };
    }
    // Started again after Import All Settings and Data: the old Tandem ends first (`IE:898-911`).
    let wait_pid = std::env::var(settings_export_ui::WAIT_PID_VAR).ok().and_then(|v| v.parse::<u32>().ok());
    // SAFETY: as above, before any thread exists; no session or child inherits the variable.
    unsafe { std::env::remove_var(settings_export_ui::WAIT_PID_VAR) };
    if let Some(pid) = wait_pid {
        settings_export_ui::wait_for_pid(pid);
    }
    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(l) => l,
        Err(e) => return message_box(&format!("Could not start the event loop: {e}")),
    };
    let proxy = Arc::new(Mutex::new(event_loop.create_proxy()));
    let clicks = proxy.clone();
    crate::notify::on_click(Box::new(move || {
        let _ = clicks.lock().map(|p| p.send_event(UserEvent::NotificationClicked));
    }));
    let job_wake = proxy.clone();
    let jobs = crate::jobwatch::JobWatch::start(Arc::new(move || {
        let _ = job_wake.lock().map(|p| p.send_event(UserEvent::JobsChanged));
    }));
    let wake = proxy.clone();
    let notify: Notify = Arc::new(move || {
        let _ = wake.lock().unwrap().send_event(UserEvent::Wake);
    });

    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
    let mut banners = Banners::default();
    let api = match conpty::load(&Source::Shipped(exe_dir.clone())) {
        Ok(api) => api,
        Err(e) => match conpty::load(&Source::Inbox) {
            Ok(api) => {
                banners.set("conpty", format!("Shipped ConPTY did not load ({e}). Using the Windows copy."));
                api
            }
            Err(e2) => return message_box(&format!("No ConPTY: {e}; {e2}")),
        },
    };
    let utilities = match crate::utilities::ensure(&exe_dir) {
        Ok(done) => {
            let entry = crate::utilities::path_entry(&done.folder.to_string_lossy());
            if let Some(text) = crate::utilities::refresh_banner(&done.failed) {
                crate::host::log(&format!("tandem: {text}"));
                banners.set("utilities", text);
            } else if entry.is_none() {
                banners.set("utilities", format!("The utilities folder {} has a \";\" in its path, so it is not put on PATH.", done.folder.display()));
            }
            Some(done.folder)
        }
        Err(e) => {
            banners.set("utilities", format!("The utilities folder could not be made: {e}"));
            None
        }
    };

    // An import staged by the last run is applied to the files before anything reads them.
    let restored_scripts = {
        let settings = globals::default_path();
        settings_export_ui::finish_import(settings.parent().unwrap_or(&settings))
    };
    let path = store::default_path();
    let mut store = match Store::load(&path) {
        Ok(s) => s,
        Err(e) => {
            // Never overwrite a settings file we could not read: work in a side file instead,
            // and keep the edits already made there in an earlier session.
            let side = path.with_extension("toml.recovered");
            let store = Store::load(&side).unwrap_or_else(|_| Store::fresh(&side));
            banners.set("settings", format!("Settings not loaded ({e}). Changes save to {}.", side.display()));
            store
        }
    };
    let gpath = globals::default_path();
    let mut globals = match Globals::load(&gpath) {
        Ok(g) => g,
        Err(e) => {
            let side = gpath.with_extension("toml.recovered");
            banners.set("globals", format!("Settings not loaded ({e}). Changes save to {}.", side.display()));
            Globals::load(&side).unwrap_or_else(|_| Globals::empty(&side))
        }
    };
    // "Load settings from a custom folder or URL": the location's settings replace the local
    // ones before anything reads them; its questions and failures show with the first window.
    let (prefs_saved, prefs_startup) = prefs_ui::load_at_start(&mut globals, &mut store, &prefs_ui::curl_fetch);
    let prefs_launch_folder = Some(globals.string(crate::remote_prefs::FOLDER_KEY));
    // The warnings answered automatically in earlier runs (iTerm2 reads its catalog at start).
    let suppressed = crate::warnings::Catalog::from_value(globals.get(crate::warnings::CATALOG_KEY).as_ref());
    if prefs_saved.is_some() {
        if let Err(e) = globals.save() {
            banners.set("globals", format!("Could not save settings: {e}"));
        }
        if let Err(e) = store.save() {
            banners.set("save", format!("Could not save settings: {e}"));
        }
    }
    // The legacy global AI headers move into each manual model once (`iTermMigrationHelper.m:458`).
    // After the custom settings folder or URL load, so headers arriving from there move too.
    if crate::ai_models::migrate_legacy_headers(&mut globals) {
        if let Err(e) = globals.save() {
            banners.set("globals", format!("Could not save settings: {e}"));
        }
    }
    // The Script Console starts with the Automatic Profile Switching entry while its logging
    // is on (`SH:340-351`).
    let aps_logging = globals.bool(aps_ui::LOG_KEY);
    // The first window's tab bar takes SelectsTabsOnMouseDown now; later windows at opening.
    let tab_selects_on_down = globals.bool("SelectsTabsOnMouseDown");
    let side_bar_preferred = side_bar_default(&globals);
    let min_tab_drag = globals.int("MinimumTabDragDistance") as f32;
    // Session restoration: shells run in tandem-host so they outlive Tandem (read once:
    // a change needs a restart, as in iTerm2).
    // A second Tandem (the host serves the first) neither restores nor saves windows: the
    // saved state belongs to the first.
    let mut second_copy = false;
    let (host, host_welcome) = if globals.bool("runJobsInServers") {
        let wait = std::time::Duration::from_secs_f64(globals.get("timeoutForDaemonAttachment").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(10.0).clamp(1.0, 120.0));
        match crate::host_client::connect_or_start(&exe_dir, wait) {
            Ok((c, w)) => (Some(c), Some(w)),
            Err(e) => {
                second_copy = matches!(e, crate::host_client::HostError::Busy(_));
                banners.set("host", format!("Shells run inside Tandem this time, so they end with it: {e}"));
                (None, None)
            }
        }
    } else {
        (None, None)
    };
    let git_worker = git_ui::GitWorker::new(Arc::new(crate::git_run_win::WinGitRun), notify.clone());
    // Discovered Ollama models from the last run resolve at once while a refresh runs.
    let ai_discovery = crate::ai_discovery::Cache::load(&crate::ai_discovery::default_path());
    let ai_transport: Arc<dyn crate::ai_transport::Transport> = Arc::new(crate::ai_winhttp::WinHttp);
    let composer_worker = crate::composer_worker::Worker::new(notify.clone(), Arc::new(crate::composer_worker::RealFs));
    let mut app = App {
        host,
        hotkey: None,
        broadcast: Default::default(),
        broadcast_tabs: Default::default(),
        stripes: None,
        checks_key: None,
        hotkey_ids: Vec::new(),
        hotkey_errors: Default::default(),
        hotkeys_registered: false,
        app_active: false,
        app_hidden: Vec::new(),
        host_welcome,
        notify,
        jobs,
        proxy,
        menu_bar: None,
        font_offset: 0.0,
        remap_tags: Arc::new(std::sync::RwLock::new(crate::remap::Tags::from_globals(&globals))),
        route_snapshot: Arc::new(std::sync::RwLock::new(keyroute::Snapshot::default())),
        exempt_map: Arc::new(std::sync::RwLock::new(toml::Table::new())),
        globals,
        leader_on: false,
        mru: Mru::default(),
        next_tab_id: 1,
        mru_mods: None,
        alt_side: AltSide::Left,
        pointer_clicks: pointer::Clicks::default(),
        pointer_last: None,
        pointer_down: None,
        mouse_override: HashMap::new(),
        alt_click: None,
        key_click: None,
        wheel_acc_x: 0.0,
        mark_pos: HashMap::new(),
        finds: HashMap::new(),
        find_bar_rect: None,
        find_mode_rect: None,
        hover: None,
        popup_menu: None,
        color_pick: None,
        shortcuts: None,
        copy_modes: HashMap::new(),
        show_stamps: HashMap::new(),
        show_guide: HashMap::new(),
        find_cursor_until: None,
        stamp_near: false,
        anim_due: None,
        note_win: None,
        note_pos: HashMap::new(),
        ctx_pending: None,
        prompt: None,
        co_prompt: None,
        tmux_drag: None,
        tmux_pending_close: Vec::new(),
        pending_tab_closes: Vec::new(),
        tmux_held_events: Vec::new(),
        tmux_dash: None,
        tmux_dash_gw: None,
        buried: Default::default(),
        tmux: HashMap::new(),
        tmux_status: HashMap::new(),
        tmux_ask_command: None,
        tmux_prompt: None,
        aps: HashMap::new(),
        windows: Default::default(),
        registry: Default::default(),
        last_window: None,
        win_style: 0,
        win_slot: None,
        slots: Default::default(),
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
        belt: Default::default(),
        tool_gen: 0,
        belt_keys: Default::default(),
        composer_keys: Default::default(),
        composers: HashMap::new(),
        composer_worker,
        scripts_scan: None,
        scripts_tree: Vec::new(),
        scripts_all: Vec::new(),
        scripts_keys: Default::default(),
        autolaunch_ran: false,
        composer_windows: Default::default(),
        composer_clip: None,
        composer_hover: None,
        composer_focus_key: None,
        action_edit_undo: None,
        laid_out: None,
        title_prompt: None,
        group_prompt: None,
        arrange_prompt: None,
        menus_dirty: false,
        start_dir_override: None,
        start_command_override: None,
        start_jumps_override: None,
        start_env_override: None,
        start_tab: None,
        single_use: HashMap::new(),
        undo: Default::default(),
        undo_capture: None,
        pending_windows: Vec::new(),
        local_host: aps_ui::local_host(),
        dropped: Vec::new(),
        coprocess_errors: HashMap::new(),
        global_find: None,
        api,
        shell: default_shell(),
        gpu: None,
        tabs: Vec::new(),
        active: 0,
        panes: HashMap::new(),
        next_id: 1,
        mods: Mods::default(),
        sent_keys: keyproto_ui::Held::default(),
        bars: HashMap::new(),
        pending_reads: HashMap::new(),
        pending_properties: HashMap::new(),
        pending_urls: HashMap::new(),
        print_setup: crate::winprint::Setup::default(),
        shot: None,
        replay_window: None,
        replay_seen: None,
        pwm_window: None,
        ssh_menu_shown: None,
        pwm: Default::default(),
        replay_keys: Default::default(),
        recording_warnings: Vec::new(),
        recording_replay: None,
        block_hover: None,
        block_buttons: Vec::new(),
        porthole_views: HashMap::new(),
        porthole_hits: Vec::new(),
        porthole_link: None,
        porthole_link_asks: HashMap::new(),
        porthole_languages: None,
        status_menu: None,
        status_setup: None,
        status_clock_due: None,
        meters: crate::statusbar_metrics::Meters::new(),
        samplers: crate::metrics_win::WinSamplers,
        graph_frame_due: None,
        git: HashMap::new(),
        git_worker,
        git_placed: std::collections::HashSet::new(),
        git_log: None,
        git_log_ready: None,
        trigger_limits: crate::triggers::RateLimit::default(),
        trigger_told: std::collections::HashSet::new(),
        trigger_alerts: HashMap::new(),
        trigger_pool: crate::triggers::CommandPool::new(16),
        trigger_children: Vec::new(),
        bells: HashMap::new(),
        bell_icons: std::collections::HashSet::new(),
        activity: HashMap::new(),
        badges: HashMap::new(),
        last_notified: None,
        alert_limits: crate::triggers::RateLimit::default(),
        notify_status_armed: false,
        last_status: HashMap::new(),
        status_watch: None,
        status_popover: None,
        status_popover_closed: None,
        open_quickly: None,
        quickly_query: String::new(),
        focus_order: HashMap::new(),
        focus_seq: 0,
        status_prompt: None,
        global_note: crate::notes::GlobalNote::new(toolbelt_more_ui::notes_path(&std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir))),
        job_watches: Vec::new(),
        jobs_wanted: HashMap::new(),
        jobs_wanting: Default::default(),
        jobs_tree_seen: 0,
        jobs_watch_at: None,
        jobs_popover: None,
        jobs_popover_closed: None,
        jobs_confirm: None,
        signal_waits: Vec::new(),
        idle_sent: HashMap::new(),
        started: Instant::now(),
        uploads: HashMap::new(),
        pastes: HashMap::new(),
        paste_asks: HashMap::new(),
        paste_history: None,
        snippets: snippets_ui::load_snippets(),
        clips: HashMap::new(),
        inline: HashMap::new(),
        add_clip_win: None,
        clip_hover: None,
        paste_history_win: None,
        advanced_paste_win: None,
        advanced_paste_request: None,
        shell_history: None,
        shell_fed: HashMap::new(),
        list_popup: None,
        completion: None,
        auto: (None, crate::completion::Auto::default()),
        upload_due: None,
        pending_background: HashMap::new(),
        toast: None,
        flashes: HashMap::new(),
        fireworks: HashMap::new(),
        tab_colors: HashMap::new(),
        tmux_geometry: HashMap::new(),
        tmux_last_geometry: HashMap::new(),
        attention_due: None,
        steal_asked: None,
        bg_images: HashMap::new(),
        rotators: HashMap::new(),
        typing: None,
        key_down: false,
        cursor: (0.0, 0.0),
        drag: Drag::None,
        clicks: Clicks::default(),
        wheel_acc: 0.0,
        focused: true,
        tab_bar: tab_layout::Bar::default(),
        tab_scroll: 0.0,
        tab_hover: None,
        tab_keep_selected: true,
        tab_seen: None,
        tab_press: None,
        tab_selects_on_down,
        min_tab_drag,
        tab_drop_marker: None,
        tab_bar_drawn: None,
        tab_join: None,
        side_bar_preferred,
        side_bar_scroll: 0.0,
        tab_flash: crate::tab_flash::Flash::new(),
        excursion_prevented: false,
        bar_was_visible: false,
        bar_grown: (0.0, 0.0),
        side_chips: Vec::new(),
        side_handle_hover: false,
        tab_icon_maps: tab_groups_ui::user_tab_icon_maps(None),
        tab_icon_cache: Default::default(),
        tab_icon_images: Default::default(),
        tab_tip: None,
        tab_tip_wake: None,
        tab_clicks: (Instant::now(), None, 0),
        banners,
        utilities,
        clipboard: arboard::Clipboard::new().ok(),
        store,
        settings: None,
        save_due: None,
        state_debounce: Default::default(),
        state_frozen: second_copy,
        focus_lost: false,
        font_sig: String::new(),
        profiles_win: None,
        blink_on: true,
        last_blink: Instant::now(),
        api_server: None,
        api_jar: Default::default(),
        ssh_secrets: Default::default(),
        ssh_registry: Default::default(),
        ssh_secret_server: None,
        script_history: crate::script_history::History::new(aps_logging, api_ui::unix_secs()),
        api_alerts: Vec::new(),
        api_on: false,
        api_pending: HashMap::new(),
        api_reset_seen: None,
        api_settings_seen: (false, false),
        api_consent: false,
        api_subs: Default::default(),
        api_tx: Default::default(),
        api_deferred: VecDeque::new(),
        api_vars: Default::default(),
        api_var_subs: Default::default(),
        api_session_subs: Default::default(),
        api_dirty: Vec::new(),
        api_last: None,
        api_all_seen: None,
        api_held: Mods::default(),
        api_tmux_commands: HashMap::new(),
        api_tmux_creates: HashMap::new(),
        api_tmux_windows: HashMap::new(),
        api_tmux_tabs: Vec::new(),
        api_fns: Default::default(),
        api_rpc_done: VecDeque::new(),
        api_keys: HashMap::new(),
        script_runs: BTreeMap::new(),
        script_next: 1,
        script_tokens: HashMap::new(),
        script_alerts: VecDeque::new(),
        script_templates: HashMap::new(),
        script_pass: 0,
        script_alert: None,
        provisioner: None,
        provision_after: HashMap::new(),
        provision_needs: HashMap::new(),
        provision_targets: HashMap::new(),
        provision_waiting: Vec::new(),
        backend_probes: Vec::new(),
        uv_installed: crate::script_runtime::uv_exe(&script_runtime_ui::uv_dir()).is_file(),
        scripts_alerts: VecDeque::new(),
        scripts_alert: None,
        suppressed,
        show_remembered: false,
        catalog_due: None,
        inline_asking: false,
        suppressed_window: None,
        suppressed_retitle_due: None,
        ai_asks: VecDeque::new(),
        ai_alert: None,
        ai_reveal: None,
        ai_key_asks: VecDeque::new(),
        ai_key_window: None,
        ai_discovery,
        ai_transport,
        ai_probe: None,
        ai_runs: HashMap::new(),
        ai_runs_next: 0,
        composer_ai: HashMap::new(),
        codecierge: HashMap::new(),
        explain_shown: None,
        ai_engage: None,
        ai_engage_open: None,
        ai_wait: None,
        ai_engage_pending: HashMap::new(),
        ai_explain_waiting: None,
        ai_annotations: HashMap::new(),
        chat_db: None,
        chat_db_failed: false,
        chat_links_off: false,
        chat_broker: crate::chat_broker::Broker::new(),
        chat_window: None,
        chat_sub: None,
        chat_shown: None,
        chat_input: Default::default(),
        chat_drafts: Default::default(),
        chat_drops: Vec::new(),
        chat_find_query: String::new(),
        chat_find: Default::default(),
        chat_toolbar: crate::chat_window_ui::ToolbarData::default(),
        chat_toolbar_dirty: false,
        chat_rows: Vec::new(),
        chat_rows_dirty: true,
        chat_rows_built: None,
        chat_snippets: HashMap::new(),
        chat_search: Default::default(),
        chat_menu_ids: Vec::new(),
        chat_msgs: Vec::new(),
        chat_convo: Default::default(),
        chat_convo_dirty: true,
        chat_picker: None,
        picker_hits: Vec::new(),
        inline_hits: Vec::new(),
        chat_indicator_hits: Vec::new(),
        streaming_chats: std::collections::HashSet::new(),
        chat_indicator_menu: None,
        chat_links: std::cell::RefCell::new(None),
        chat_mint_sub: None,
        chat_prompt: None,
        title_runs: HashMap::new(),
        ai_overrides: crate::ai_permissions::Overrides::default(),
        ai_staged: HashMap::new(),
        ai_consent_asked: false,
        ai_watermarks: HashMap::new(),
        ai_execs: HashMap::new(),
        ai_exec_canceled: Vec::new(),
        ai_helps: HashMap::new(),
        ai_checks: HashMap::new(),
        ai_next_check: 0,
        ai_nag: crate::ai_classifier::NagGate::default(),
        chat_agents: HashMap::new(),
        ai_pending_tools: HashMap::new(),
        #[cfg(windows)]
        ai_wire_log: Arc::new(Mutex::new(crate::ai_wire_log::WireLog::new())),
        script_procs: HashMap::new(),
        script_jobs_left: Vec::new(),
        script_after_enable: Vec::new(),
        repl_after_enable: None,
        repl_pending: None,
        held_on_exit: HashMap::new(),
        importing: None,
        import_queue: VecDeque::new(),
        archives_asked: std::collections::HashSet::new(),
        script_export: None,
        script_exports: Vec::new(),
        script_picker: None,
        script_picker_minors: None,
        script_deps: None,
        script_deps_state: script_deps_app::Deps::new(),
        script_deps_pending: None,
        script_console: None,
        script_inspector: None,
        script_console_reveal: None,
        last_notified_script: None,
        unread_counts: HashMap::new(),
        unread_session_counts: HashMap::new(),
        script_components: HashMap::new(),
        script_parsed: HashMap::new(),
        script_titles: HashMap::new(),
        text_popover: None,
        script_title_providers: None,
        script_dialogs: VecDeque::new(),
        script_dialog: None,
        script_tmux: HashMap::new(),
        script_moved: Vec::new(),
        prefs_saved,
        prefs_save_due: None,
        prefs_fetch: None,
        prefs_exit_pending: false,
        prefs_startup,
        prefs_reveal: false,
        prefs_gpu: None,
        prefs_launch_folder,
        relaunch_on_exit: false,
        relaunch_due: false,
        restored_scripts,
        restored_dir: None,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        message_box(&format!("Tandem stopped: {e}"));
    }
    // The restart after Import All Settings and Data: the new Tandem finishes the import.
    if app.relaunch_on_exit {
        drop(app);
        if let Err(e) = settings_export_ui::start_relaunched() {
            message_box(&format!("Could not start Tandem again: {e}. Start it to finish the import."));
        }
    }
}

impl App {
    fn scale(&self) -> f32 {
        self.gpu.as_ref().map_or(1.0, |g| g.window.scale_factor() as f32)
    }

    fn cell(&self) -> (f32, f32) {
        self.gpu.as_ref().map_or((8.0, 16.0), |g| g.renderer.cell)
    }

    /// The edge the tab bar sits on (`TabViewType`).
    pub(super) fn tab_position(&self) -> TabPosition {
        tab_position::from_value(self.globals.int("TabViewType"))
    }

    /// The tab bar shows (`tab_position::bar_visible`): while it flashes; in full screen only
    /// with Show tab bar in fullscreen; then always with `HideTab` off, else with more than
    /// one tab. The one place that decides it.
    pub(super) fn tab_bar_visible(&self) -> bool {
        self.bar_visible_with(self.tab_flash.flashing())
    }

    /// The tab bar takes room from the sessions: it shows, not only by flashing (a flashing
    /// bar lies over the sessions, `PT windowDecorationSize`, `PT:13214-13215`).
    fn tab_bar_takes_room(&self) -> bool {
        self.bar_visible_with(false)
    }

    /// Whether the window may change size to fit what it shows (the toolbelt, the tab bar):
    /// a Normal or No Title Bar window that is not maximized, snapped, full screen, minimized
    /// or size-locked. A hotkey window never changes (`winmodel::can_fit`).
    pub(super) fn window_can_fit(&self) -> bool {
        let Some(g) = self.gpu.as_ref() else { return false };
        let style = crate::frames::unthemed(self.win_style);
        crate::winmodel::can_fit(crate::winmodel::FitState {
            normal_style: style == crate::frames::NORMAL || style == crate::frames::NO_TITLE_BAR,
            maximized: g.window.is_maximized(),
            full_screen: g.window.fullscreen().is_some(),
            minimized: g.window.is_minimized() == Some(true),
            snapped: self.hwnd().is_some_and(window_arranged),
            size_locked: self.win_locks.size,
            hotkey: self.hotkey.is_some(),
        })
    }

    /// The tab bar showed (`showing`) or hid: the window grows by the bar's thickness `delta`
    /// (pixels, width and height) or shrinks by `delta` (what it grew when the bar showed), or
    /// keeps its size, as `tab_position::bar_change_fit` rules (R3, `PT:8070-8145`). What a
    /// show grew is kept in `bar_grown`. The new size is requested for the inner area (the fit
    /// is of the outer frame, less the borders) and its Resized event lays the window out, so
    /// tmux hears the final size once. True when the size changed (a size Windows refuses, or
    /// the same size, sends no Resized event).
    fn fit_window_to_bar(&mut self, showing: bool, delta: (f32, f32)) -> bool {
        self.bar_grown = (0.0, 0.0);
        if !self.window_can_fit() {
            return false;
        }
        let Some((work, _)) = self.monitor_rects() else { return false };
        let Some(g) = self.gpu.as_ref() else { return false };
        let outer = Self::outer_rect(g);
        let preserve = self.globals.bool("PreserveWindowSizeWhenTabBarVisibilityChanges");
        let only_if_off_screen = self.globals.bool("PreserveWindowSizeToKeepOnScreenWhenTabBarVisibilityChanges");
        match tab_position::bar_change_fit(outer, work, delta, showing, preserve, only_if_off_screen, self.excursion_prevented) {
            tab_position::Fit::Keep { excursion_prevented } => {
                self.excursion_prevented = excursion_prevented;
                false
            }
            tab_position::Fit::Resize(r) => {
                let (outer_size, inner) = (g.window.outer_size(), g.window.inner_size());
                let (dw, dh) = (outer_size.width.saturating_sub(inner.width), outer_size.height.saturating_sub(inner.height));
                let size = winit::dpi::PhysicalSize::new((r.w.round() as u32).saturating_sub(dw).max(1), (r.h.round() as u32).saturating_sub(dh).max(1));
                if size == inner {
                    return false;
                }
                let _ = caption_ui::request_inner_size(&g.window, size);
                // Windows resizes at once; what it granted is what a hide takes back.
                let now = g.window.inner_size();
                if showing {
                    self.bar_grown = tab_position::bar_growth((inner.width as f32, inner.height as f32), (now.width as f32, now.height as f32));
                }
                now != inner
            }
        }
    }

    /// When the tab bar starts or stops taking room (a tab opened or closed, `HideTab`
    /// changed), the window fits it (`fit_window_to_bar`): showing, by the bar's thickness
    /// then; hiding, by what the show grew. Full screen is left aside, so leaving it never
    /// refits, and a flashing bar takes no room. A window's first layout only notes the bar.
    /// True when the window resized.
    fn fit_bar_change(&mut self) -> bool {
        let visible = self.bar_visible_ignoring_full_screen();
        let first = self.laid_out.is_none() || self.gpu.is_none();
        if visible == self.bar_was_visible || first {
            self.bar_was_visible = visible;
            return false;
        }
        self.bar_was_visible = visible;
        let delta = match (visible, tab_position::vertical(self.tab_position())) {
            (false, _) => self.bar_grown,
            (true, true) => (self.side_bar_width().round(), 0.0),
            // A compact window's top bar sits in the caption strip, which is there anyway.
            (true, false) if self.caption_strip().is_some() && self.tab_position() == TabPosition::Top => (0.0, 0.0),
            (true, false) => (0.0, self.tab_height().round()),
        };
        self.fit_window_to_bar(visible, delta)
    }

    /// `tab_bar_takes_room` as if the window were not full screen.
    fn bar_visible_ignoring_full_screen(&self) -> bool {
        let always = tab_position::always_visible(self.globals.bool("HideTab"));
        tab_position::bar_visible(false, true, false, always, self.tabs.len())
    }

    fn bar_visible_with(&self, flashing: bool) -> bool {
        let always = tab_position::always_visible(self.globals.bool("HideTab"));
        tab_position::bar_visible(self.full_screen(), self.globals.bool("ShowFullScreenTabBar"), flashing, always, self.tabs.len())
    }

    /// The window is full screen (Toggle Full Screen: borderless).
    fn full_screen(&self) -> bool {
        self.gpu.as_ref().is_some_and(|g| g.window.fullscreen().is_some())
    }

    /// A flash may start in this window (`tab_flash::eligible`).
    fn tab_flash_eligible(&self) -> bool {
        tab_flash::eligible(self.full_screen(), self.globals.bool("ShowFullScreenTabBar"), self.globals.bool("FlashTabBarInFullscreen"), self.find_cursor_until.is_some())
    }

    /// `TabAutoShowHoldTime` and `TabFlashAnimationDuration`, each 0 to 60 seconds.
    fn tab_flash_times(&self) -> (Duration, Duration) {
        let secs = |key: &str, default: f64| Duration::from_secs_f64(self.globals.number(key).unwrap_or(default).clamp(0.0, 60.0));
        (secs("TabAutoShowHoldTime", 1.0), secs("TabFlashAnimationDuration", 0.25))
    }

    /// Flashes the tab bar (`PT` `[tabBarControl setFlashing:YES]`): a tab switch, a change in
    /// the number of tabs, entering full screen. Nothing happens unless the window may flash.
    pub(super) fn flash_tab_bar(&mut self) {
        let (hold, fade) = self.tab_flash_times();
        let eligible = self.tab_flash_eligible();
        let was = self.tab_flash.state();
        self.tab_flash.set_flashing(true, eligible, Instant::now(), hold, fade);
        if was != self.tab_flash.state() || self.tab_flash.flashing() {
            self.redraw();
        }
    }

    /// Stops the flash (`setFlashing:NO`): a hold fades out, a fade ends at once.
    fn stop_tab_flash(&mut self) {
        let (hold, fade) = self.tab_flash_times();
        let was = self.tab_flash.state();
        self.tab_flash.set_flashing(false, true, Instant::now(), hold, fade);
        if was != self.tab_flash.state() {
            self.redraw();
        }
    }

    /// Moves every window's flash along: in this window a flash it may no longer show fades
    /// out (`TBV updateFlashing`); a window whose flash changed, or is fading, draws. Answers
    /// when to look again.
    fn tick_tab_flash(&mut self) -> Option<Instant> {
        if matches!(self.tab_flash.state(), tab_flash::State::Holding | tab_flash::State::Extending) && !self.tab_flash_eligible() {
            self.stop_tab_flash();
        }
        let now = Instant::now();
        if tick_flash(&mut self.tab_flash, now) {
            self.redraw();
        }
        let mut wake = flash_wake(&self.tab_flash, now);
        for (_, s) in self.windows.parked_mut() {
            if tick_flash(&mut s.tab_flash, now) {
                if let Some(g) = &s.gpu {
                    g.window.request_redraw();
                }
            }
            wake = wake.into_iter().chain(flash_wake(&s.tab_flash, now)).min();
        }
        wake
    }

    /// Makes tab `i` the selected tab, as a click, a key or a command does: a collapsed group
    /// it is hidden in expands, the bar flashes in full screen and brings the tab into view
    /// (`PT tabView:didSelectTabViewItem:`).
    pub(super) fn select_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        let changed = i != self.active;
        self.active = i;
        self.expand_selected_group();
        if changed {
            self.flash_tab_bar();
        }
        self.scroll_active_tab_into_view();
    }

    /// View > Show Tabs in Fullscreen (`AD toggleFullScreenTabBar:`): flips
    /// `ShowFullScreenTabBar`, saves it, and every window lays out again.
    pub(super) fn toggle_full_screen_tab_bar(&mut self) {
        let on = !self.globals.bool("ShowFullScreenTabBar");
        self.globals.set("ShowFullScreenTabBar", toml::Value::Boolean(on));
        self.save_globals();
        self.relayout_changed_windows();
        self.redraw_all();
        self.sync_checks();
    }

    /// A horizontal tab bar's height in pixels (`DefaultTabBarHeight` points), whether or not
    /// the bar shows; `tab_bar_rect` says whether there is a bar.
    fn tab_height(&self) -> f32 {
        (self.tab_settings().height * self.scale()).round()
    }

    /// A side bar's narrowest width in points (`RTV minimumTabBarWidth`).
    pub(super) fn side_bar_minimum(&self) -> f32 {
        crate::tab_theme::side_minimum(self.tab_style())
    }

    /// A side tab bar's width in pixels: the window's preferred width (`LeftTabBarWidth` when
    /// it opened) clamped to the window (`tab_position::side_width`), with Tandem's side
    /// padding as the side margin.
    pub(super) fn side_bar_width(&self) -> f32 {
        let scale = self.scale();
        let w = self.gpu.as_ref().map_or(800.0, |g| g.config.width as f32);
        (tab_position::side_width(self.side_bar_preferred, w / scale, PAD, self.side_bar_minimum()) * scale).round()
    }

    /// The window shared out (`tab_position::window_split`): the tab bar's rect, the sessions'
    /// room and the toolbelt's rect, in pixels, window coordinates. The toolbelt takes the far
    /// right of the room above any banner first; the bar then takes its thickness
    /// (`tab_height` on the top or bottom, `side_bar_width` on a side) from its edge of the
    /// rest. No bar rect while the bar hides, no toolbelt rect while it hides.
    pub(super) fn window_layout(&self) -> (Option<Rect>, Rect, Option<Rect>) {
        let mut room = self.window_room();
        let scale = self.scale();
        // A Minimal or Compact window's caption strip takes the top of the room, across the whole
        // window, tabs or no tabs (`caption_strip`); a top bar shows in it, and never moves the
        // sessions, whether it takes room or flashes.
        let strip = self.caption_strip().map(|h| Rect { h, ..room });
        if let Some(s) = strip {
            room = Rect { y: room.y + s.h, h: (room.h - s.h).max(1.0), ..room };
        }
        let in_strip = strip.is_some() && self.tab_position() == TabPosition::Top;
        let belt = self.belt.visible.then(|| crate::toolbelt::clamp_width(self.belt.width, room.w / scale) * scale);
        let bar = self.tab_bar_visible().then(|| {
            let position = self.tab_position();
            (position, if tab_position::vertical(position) { self.side_bar_width() } else { self.tab_height() })
        });
        let (rect, sessions, belt_rect) = tab_position::window_split(room, belt, bar.filter(|_| !in_strip));
        if in_strip {
            return (strip.filter(|_| bar.is_some()), sessions, belt_rect);
        }
        if bar.is_some() && !self.tab_bar_takes_room() {
            // A flashing bar lies over the sessions: they keep the room they had.
            return (rect, tab_position::window_split(room, belt, None).1, belt_rect);
        }
        (rect, sessions, belt_rect)
    }

    /// The tab bar's rect in pixels, window coordinates; None while it hides. In a Minimal or
    /// Compact window's caption the top bar is the strip's tab area, clear of the window buttons.
    pub(super) fn tab_bar_rect(&self) -> Option<Rect> {
        let bar = self.window_layout().0?;
        match self.caption_layout().filter(|_| self.tab_position() == TabPosition::Top) {
            Some(l) => crate::caption::tab_bar_rect(&l, true).map(|t| Rect { x: t.x, w: t.w, ..bar }),
            None => Some(bar),
        }
    }

    /// The horizontal tab bar's rect: the bar's, on the top or bottom edge.
    fn horizontal_bar_rect(&self) -> Option<Rect> {
        self.tab_bar_rect().filter(|_| !tab_position::vertical(self.tab_position()))
    }

    /// A side tab bar's rect in pixels; None without a side bar.
    fn side_bar_rect(&self) -> Option<Rect> {
        self.tab_bar_rect().filter(|_| tab_position::vertical(self.tab_position()))
    }

    /// The shown bar's scroll offset in points: along a horizontal bar, down a side bar.
    fn bar_offset(&self) -> f32 {
        if self.side_bar_rect().is_some() {
            self.side_bar_scroll
        } else {
            self.tab_scroll
        }
    }

    /// `ScrollableSideTabBar`: rows that do not fit scroll (`PSM tabBarIsScrollable`).
    fn side_bar_scrollable(&self) -> bool {
        self.globals.bool("ScrollableSideTabBar")
    }

    /// A side bar's scroll offset brought within its rows as last laid out
    /// (`PSM clampScrollOffset`), in points.
    fn clamp_side_scroll(&mut self) {
        let Some(bar) = self.side_bar_rect() else { return };
        self.side_bar_scroll = tab_layout::clamp_scroll(self.side_bar_scroll, self.tab_bar.content, bar.h / self.scale());
    }

    /// A side bar's drag handle in pixels, window coordinates (`tab_position::drag_handle`,
    /// 3 points wide on the bar's inner edge); None without a side bar.
    fn side_handle(&self) -> Option<Rect> {
        let bar = self.tab_bar_rect()?;
        let scale = self.scale();
        let points = Rect { x: bar.x / scale, y: bar.y / scale, w: bar.w / scale, h: bar.h / scale };
        tab_position::drag_handle(points, self.tab_position()).map(|r| tab_layout::to_px(r, scale))
    }

    /// Each tab's rect in pixels, window coordinates, in `tabs` order: its cell (a side bar's
    /// row) scrolled and placed (`tab_position::rects_from_cells`). A tab not shown (in the
    /// overflow menu, collapsed, or with no bar) is zero-size. 13d's drags and 13h's pane drop
    /// read the tabs' places here.
    pub(super) fn bar_tab_rects(&self) -> Vec<Rect> {
        let n = self.tabs.len();
        let Some(bar) = self.tab_bar_rect() else { return vec![Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }; n] };
        let cells: Vec<(usize, Rect)> = self.tab_bar.cells.iter().map(|(i, _, r)| (*i, *r)).collect();
        tab_position::rects_from_cells(&cells, n, (bar.x, bar.y), self.bar_offset(), self.scale(), self.tab_bar.vertical)
    }

    /// Each collapsed group's chip as drawn, pixels, window coordinates, for a tab dropped on
    /// it (`tab_drag::pills`): the laid-out chips scrolled and placed as the tabs are.
    pub(super) fn bar_pills(&self, dragged: Option<usize>) -> Vec<crate::tab_drag::Pill> {
        let Some(bar) = self.tab_bar_rect() else { return Vec::new() };
        let cells: Vec<(usize, Rect)> = self.tab_bar.chips.iter().enumerate().map(|(k, (_, r))| (k, *r)).collect();
        let rects = tab_position::rects_from_cells(&cells, cells.len(), (bar.x, bar.y), self.bar_offset(), self.scale(), self.tab_bar.vertical);
        let chips: Vec<(Rect, String, std::ops::Range<usize>, bool)> =
            self.tab_bar.chips.iter().zip(&self.tab_bar.chip_texts).zip(rects).map(|(((id, _), t), r)| (r, id.clone(), t.run.clone(), t.collapsed)).collect();
        let hidden: Vec<bool> = self.tabs.iter().map(|t| t.group.as_ref().is_some_and(|g| g.collapsed)).collect();
        crate::tab_drag::pills(&chips, &hidden, dragged)
    }

    /// The group whose chip is under the window point (`x`, `y`) in pixels: on a horizontal
    /// bar the chip `tab_layout::hit` finds, on a side bar a chip as last drawn.
    pub(super) fn bar_chip_at(&self, x: f32, y: f32) -> Option<String> {
        if self.side_bar_rect().is_some() {
            return self.side_chips.iter().find(|(r, _)| r.contains(x, y)).map(|(_, id)| id.clone());
        }
        match self.tab_bar_hit(x, y)? {
            tab_layout::Hit::Chip(k) => self.tab_bar.chips.get(k).map(|(id, _)| id.clone()),
            _ => None,
        }
    }

    /// The tabs as the bar lays them out: each look, with its group's chip on the first tab of
    /// each run (a run's members counted for a collapsed chip) and a collapsed group's tabs
    /// hidden.
    fn tab_inputs(&self, looks: &[scene::TabLook]) -> Vec<tab_layout::TabInput> {
        let ids: Vec<Option<&str>> = self.tabs.iter().map(|t| t.group.as_ref().map(|g| g.id.as_str()).filter(|g| !g.is_empty())).collect();
        looks
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let group = self.tabs.get(i).and_then(|t| t.group.as_ref()).filter(|g| !g.id.is_empty()).map(|g| {
                    let first = i == 0 || ids[i - 1] != ids[i];
                    let count = ids[i..].iter().take_while(|x| **x == ids[i]).count() + ids[..i].iter().rev().take_while(|x| **x == ids[i]).count();
                    tab_layout::ChipInput { id: g.id.clone(), name: g.name.clone(), collapsed: g.collapsed, count, first }
                });
                tab_layout::TabInput {
                    title: l.title.clone(),
                    subtitle: l.subtitle.clone(),
                    icon: l.dot.is_some() || l.icon.is_some(),
                    processing: l.processing,
                    number: l.number,
                    pinned: l.pinned,
                    hidden: group.as_ref().is_some_and(|g| g.collapsed),
                    group,
                    graphic: l.icon_image.is_some(),
                    letter: match &l.icon_image {
                        Some(scene::TabImage::Letter { letter, .. }) => letter.clone(),
                        _ => String::new(),
                    },
                }
            })
            .collect()
    }

    /// Lays out the tab bar in `area` (pixels): a horizontal bar scrolled along by `offset`
    /// points, a side bar down, measuring labels in the prose font.
    fn layout_tab_bar(&mut self, looks: &[scene::TabLook], area: Rect, offset: f32) -> tab_layout::Bar {
        let s = self.tab_settings();
        let inputs = self.tab_inputs(looks);
        let vertical = tab_position::vertical(self.tab_position());
        let (scale, active, hovered) = (self.scale(), self.active, self.tab_hover.map(|(i, _)| i));
        let renderer = self.gpu.as_mut().map(|g| std::cell::RefCell::new(&mut g.renderer));
        // Points to pixels and back: a text of `pt` points is drawn at `pt * scale` pixels.
        let measure = |text: &str, pt: f32, bold: bool| match &renderer {
            Some(r) => {
                let mut r = r.borrow_mut();
                let row_scale = pt * scale / r.font_px().max(1.0);
                r.prose_width(text, bold, row_scale) / scale
            }
            None => text.chars().count() as f32 * pt * 0.5,
        };
        if vertical {
            tab_layout::build_vertical(&inputs, area.w / scale, area.h / scale, offset, hovered, &s, &measure)
        } else {
            tab_layout::build(&inputs, active, area.w / scale, offset, hovered, &s, &measure)
        }
    }

    /// The part of the tab bar the tabs scroll in, in points: left of the right margin, where
    /// the "+" is pinned (`PSM scrollViewportLength`).
    fn tab_viewport(&self) -> f32 {
        // The bar stops at the toolbelt.
        let w = self.horizontal_bar_rect().map_or(0.0, |r| r.w);
        (w / self.scale() - tab_layout::right_margin(false, !self.globals.bool("RemoveAddTabButton"))).max(0.0)
    }

    /// What the window point (`x`, `y`) in pixels is over on the tab bar (either bar, at its
    /// scroll); None off the bar.
    pub(super) fn tab_bar_hit(&self, x: f32, y: f32) -> Option<tab_layout::Hit> {
        let bar = self.tab_bar_rect().filter(|b| b.contains(x, y))?;
        let scale = self.scale();
        live_tab_hit(tab_layout::hit(&self.tab_bar, self.bar_offset(), (x - bar.x) / scale, (y - bar.y) / scale), self.tabs.len())
    }

    /// Brings the selected tab fully into view on a bar that scrolls, and keeps it there at
    /// every layout until the bar is scrolled by hand (`PSM scrollSelectedTabIntoView`).
    pub(super) fn scroll_active_tab_into_view(&mut self) {
        self.tab_keep_selected = true;
        let Some((_, _, r)) = self.tab_bar.cells.iter().find(|(i, _, _)| *i == self.active) else { return };
        if let Some(bar) = self.side_bar_rect() {
            self.side_bar_scroll = tab_layout::scroll_into_view(self.side_bar_scroll, r.y, r.h, bar.h / self.scale(), self.tab_bar.content);
            return;
        }
        self.tab_scroll = tab_layout::scroll_into_view(self.tab_scroll, r.x, r.w, self.tab_viewport(), self.tab_bar.content);
    }

    /// Every frame, shown bar or not: a newly selected tab, or a tab added or closed, is to be
    /// brought into view (`PSM tabView:didSelectTabViewItem:`,
    /// `tabViewDidChangeNumberOfTabViewItems:`), and a tab added or closed flashes the bar in
    /// full screen (`TabAutoShowHoldTime`'s help: "when the number of tabs changes or you
    /// switch tabs"; a switch flashes in `select_tab`). A window's first frame is no change.
    fn note_tab_changes(&mut self) {
        let seen = self.tabs.get(self.active).map(|t| (t.id, self.tabs.len()));
        if seen == self.tab_seen {
            return;
        }
        let count_changed = self.tab_seen.zip(seen).is_some_and(|((_, was), (_, now))| was != now);
        self.tab_seen = seen;
        self.tab_keep_selected = true;
        if count_changed {
            self.flash_tab_bar();
        }
    }

    /// At each layout: the selected tab is brought into view again while it is kept there
    /// (`note_tab_changes`), and the offset stays within the tabs.
    fn follow_active_tab(&mut self) {
        if self.tab_keep_selected {
            self.scroll_active_tab_into_view();
        }
        if self.side_bar_rect().is_some() {
            self.clamp_side_scroll();
        } else {
            self.tab_scroll = tab_layout::clamp_scroll(self.tab_scroll, self.tab_bar.content, self.tab_viewport());
        }
    }

    /// The held press is on a tab that is no longer at the pressed index.
    fn tab_press_moved(&self) -> bool {
        press_on_moved_tab(self.tab_press, |i| self.tabs.get(i).map(|t| t.id))
    }

    /// The "+" button in pixels, window coordinates; None while the bar hides or with
    /// `RemoveAddTabButton`. 13d drags the window by it.
    pub(super) fn new_tab_button_rect(&self) -> Option<Rect> {
        let bar = self.horizontal_bar_rect()?;
        tab_layout::add_rect_px(&self.tab_bar, self.tab_scroll, (bar.x, bar.y), self.scale())
    }

    /// The tab under the pointer follows it; a change redraws (the close button shows on hover).
    fn update_tab_hover(&mut self) {
        let (x, y) = self.cursor;
        let hover = match self.tab_bar_hit(x, y) {
            Some(tab_layout::Hit::Tab(i)) => Some((i, false)),
            Some(tab_layout::Hit::Close(i)) => Some((i, true)),
            _ => None,
        };
        if hover != self.tab_hover {
            self.tab_hover = hover;
            self.redraw();
        }
        self.update_tab_tip();
    }

    /// The tab tooltip follows the pointer: resting on a tab (not its close button, with no
    /// press on the bar) starts the wait again at each move until the tip shows; a shown tip
    /// stays while the pointer is on its tab, under the pointer. Anything else hides it.
    fn update_tab_tip(&mut self) {
        let now = Instant::now();
        let shown = self.tab_tip_shown().is_some();
        self.tab_tip = match self.tab_hover {
            Some((i, false)) if self.tab_press.is_none() => match self.tab_tip {
                Some((j, since)) if j == i && shown => Some((i, since)),
                _ => {
                    self.tab_tip_wake = Some(now + requests_ui::TIP_DELAY);
                    Some((i, now))
                }
            },
            _ => None,
        };
        // A shown tip moves with the pointer or goes.
        if shown {
            self.redraw();
        }
    }

    /// The tab whose tooltip shows now.
    fn tab_tip_shown(&self) -> Option<usize> {
        self.tab_tip.filter(|(_, since)| Instant::now() >= *since + requests_ui::TIP_DELAY).map(|(i, _)| i)
    }

    /// A press hides the tab tooltip until the pointer rests again.
    fn hide_tab_tip(&mut self) {
        if self.tab_tip_shown().is_some() {
            self.redraw();
        }
        self.tab_tip = None;
    }

    /// The windows draw a tab tooltip that came due; says when the next one is due.
    fn tick_tab_tip(&mut self) -> Option<Instant> {
        let at = self.tab_tip_wake?;
        if Instant::now() < at {
            return Some(at);
        }
        self.tab_tip_wake = None;
        self.redraw();
        None
    }

    /// A press on the tab bar (`PSM mouseDown:`, `otherMouseDown:`, `menuForEvent:`): the left
    /// button keeps the press and its click count, and selects the tab at once when
    /// `press_selects` says so (a press on a close button shows it pressed instead); the middle
    /// button keeps its hit for the release; the right button opens the tab's menu on a tab
    /// and the group's on a chip; other buttons do nothing on the bar.
    fn tab_bar_press(&mut self, button: MouseButton, hit: tab_layout::Hit) {
        use tab_layout::Hit;
        let alt = self.mods.alt;
        let id = match hit {
            Hit::Tab(i) | Hit::Close(i) => self.tabs.get(i).map(|t| t.id),
            _ => None,
        };
        match button {
            // The button draws pressed while its menu is open: the menu's modal loop draws
            // nothing, so the frame is drawn before it opens (`PSMOverflowPopUpButton mouseDown:`).
            MouseButton::Left if hit == Hit::Overflow => {
                self.tab_press = Some((hit, 1, alt, None));
                self.render();
                self.show_tab_overflow();
                self.tab_press = None;
                self.redraw();
            }
            MouseButton::Left => {
                let clicks = self.count_tab_click(hit);
                self.tab_press = Some((hit, clicks, alt, id));
                self.tab_drag_press(hit);
                if let Hit::Tab(i) = hit {
                    if tab_layout::press_selects(self.tab_selects_on_down, alt, clicks) {
                        self.select_tab(i);
                    }
                }
                self.redraw();
            }
            MouseButton::Middle => self.tab_press = Some((hit, 0, alt, id)),
            MouseButton::Right => match hit {
                Hit::Tab(_) | Hit::Close(_) => {
                    if let Some(tab) = id {
                        self.show_tab_menu(tab);
                    }
                }
                Hit::Chip(_) => {
                    let (x, y) = self.cursor;
                    if let Some(group) = self.bar_chip_at(x, y) {
                        self.show_group_menu(&group);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// The click count of a left press on the bar: one more than the last when it is on the
    /// same tab (or again off the tabs) within the double-click time, else 1. A press on the
    /// "+" counts 1 and starts the count over.
    fn count_tab_click(&mut self, hit: tab_layout::Hit) -> u8 {
        let (clicks, last) = tab_click_count(self.tab_clicks, Instant::now(), hit, clippings_ui::double_click_ms());
        self.tab_clicks = last;
        clicks
    }

    /// The release that ends a press on the tab bar (`PSM handleMouseUp:`, `otherMouseUp:`).
    /// False when no press on the bar waits for this button.
    fn tab_bar_release(&mut self, button: MouseButton, event_loop: &ActiveEventLoop) -> bool {
        use tab_layout::{Hit, Release};
        let Some((down, clicks, _, _)) = self.tab_press else { return false };
        let middle = clicks == 0;
        let ours = match button {
            MouseButton::Left => !middle,
            MouseButton::Middle => middle,
            _ => false,
        };
        if !ours {
            return false;
        }
        let moved = self.tab_press_moved();
        self.tab_press = None;
        let began = self.tab_drag_began();
        // The pressed tab is no longer where it was pressed: the release acts on no tab.
        if moved {
            self.end_tab_drag();
            self.redraw();
            return true;
        }
        let (x, y) = self.cursor;
        let up = self.tab_bar_hit(x, y);
        let live = |i: usize, n: usize| i < n;
        if middle {
            if let Some(i) = tab_layout::middle_release(Some(down), up, self.globals.bool("MiddleClickClosesTab")) {
                self.close_tab_at(i, event_loop);
            }
            self.redraw();
            return true;
        }
        // The "+" is a button of its own: it acts when released where it was pressed.
        if down == Hit::Add && up == Some(Hit::Add) {
            self.add_tab_button();
        }
        // A plain click on a group chip toggles its collapse (`PSM mouseUp:`, `PT:9731-9734`);
        // a chip dragged is no click.
        if let (Hit::Chip(k), Some(Hit::Chip(j)), 1, false) = (down, up, clicks, began) {
            let group = self.bar_chip_at(x, y);
            if k == j {
                if let Some(g) = group {
                    self.toggle_group_collapsed(&g);
                }
            }
        }
        let clicked = tab_layout::release(Some(down), up, clicks, self.globals.bool("DoubleClickTabToEdit"), self.globals.bool("TabCloseButtonsAlwaysVisible"));
        // A drag that began drops the tab; one that never began leaves the click.
        if began {
            self.tab_drag_release(event_loop);
        } else {
            self.end_tab_drag();
        }
        let n = self.tabs.len();
        match tab_drag::after_drag(began, clicked) {
            Release::Close(i) => self.close_tab_at(i, event_loop),
            Release::Select(i) if live(i, n) => self.select_tab(i),
            Release::EditTitle(i) if live(i, n) => {
                self.select_tab(i);
                self.open_title_prompt(event_loop, true);
            }
            // iTerm2's New Tab (`PT tabViewDoubleClickTabBar:`), a tmux tab when tmux makes it.
            Release::NewTab => {
                if self.tmux_new(false, true).is_none() {
                    self.new_tab(None);
                }
            }
            _ => {}
        }
        self.redraw();
        true
    }

    /// The "+" button (`PT tabViewDidClickAddTabButton:`): a tmux tab for a tmux session, else
    /// a tab with the default profile, or with `AddTabButtonUsesCurrentProfile` the current
    /// session's (a fresh private copy when the session has one).
    pub(super) fn add_tab_button(&mut self) {
        let pane = self.focused_id();
        let tmux = pane.is_some_and(|p| self.tmux_of(p).is_some());
        match add_tab_choice(tmux, self.globals.bool("AddTabButtonUsesCurrentProfile")) {
            AddTab::TmuxTab => {
                self.tmux_new(false, false);
            }
            AddTab::Default => self.new_tab(None),
            AddTab::CurrentProfile => {
                let guid = pane.and_then(|p| self.profile_for_new_session(p));
                let (tabs, windows) = (self.tabs.len(), self.pending_windows.len());
                self.new_tab(guid.as_deref());
                // Nothing opened: a copy made for it goes.
                if let Some(g) = guid.filter(|_| self.tabs.len() == tabs && self.pending_windows.len() == windows) {
                    self.forget_session_profile(&g);
                }
            }
        }
    }

    /// The wheel over a tab bar that scrolls: a horizontal wheel, or Shift with the vertical
    /// one, scrolls the tabs and stops keeping the selected tab in view (`PSM scrollWheel:`).
    /// False when the wheel is not the bar's.
    fn tab_bar_wheel(&mut self, delta: MouseScrollDelta) -> bool {
        let (x, y) = self.cursor;
        if self.tab_bar_hit(x, y).is_none() {
            return false;
        }
        if self.side_bar_rect().is_some() {
            self.side_bar_wheel(delta);
            return true;
        }
        let optimum = self.tab_settings().prefs.optimum;
        // A notched wheel moves in lines; precise deltas are already a distance (pixels here).
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x, y),
            MouseScrollDelta::PixelDelta(p) => tab_layout::pixel_lines(p.x, p.y, optimum, self.scale()),
        };
        let Some(to) = tab_layout::wheel_scroll(self.tab_scroll, dx, dy, self.mods.shift, optimum, self.tab_bar.content, self.tab_viewport()) else {
            return false;
        };
        if to != self.tab_scroll {
            self.tab_scroll = to;
            self.tab_keep_selected = false;
            self.redraw();
            // The tabs moved under a still pointer.
            self.update_tab_hover();
        }
        true
    }

    /// The wheel over a side bar (`PSM scrollWheel:`): with `ScrollableSideTabBar` and rows
    /// that do not fit, the vertical wheel scrolls the rows (a notched wheel a row a line),
    /// within the rows as last laid out, and stops keeping the selected tab in view. Otherwise
    /// nothing.
    fn side_bar_wheel(&mut self, delta: MouseScrollDelta) {
        let Some(bar) = self.side_bar_rect() else { return };
        let (scale, content) = (self.scale(), self.tab_bar.content);
        if !self.side_bar_scrollable() || content <= bar.h / scale {
            return;
        }
        let row = self.tab_settings().height;
        let dy = match delta {
            MouseScrollDelta::LineDelta(_, y) => y * row,
            MouseScrollDelta::PixelDelta(p) => p.y as f32 / scale,
        };
        let to = tab_layout::clamp_scroll(self.side_bar_scroll - dy, content, bar.h / scale);
        if to != self.side_bar_scroll {
            self.side_bar_scroll = to;
            self.tab_keep_selected = false;
            self.redraw();
            // The rows moved under a still pointer.
            self.update_tab_hover();
        }
    }

    /// Dragging a side bar's handle (`RTV:2264-2277`): the bar's width follows the pointer from
    /// `start_width` points (a right bar grows leftward), clamped by `side_width` and rounded;
    /// the sessions follow.
    fn side_bar_drag(&mut self, start_x: f32, start_width: f32, x: f32) {
        let scale = self.scale();
        let dx = (x - start_x) / scale;
        let dx = if self.tab_position() == TabPosition::Right { -dx } else { dx };
        let w = self.gpu.as_ref().map_or(800.0, |g| g.config.width as f32) / scale;
        let preferred = tab_position::side_width(start_width + dx, w, PAD, self.side_bar_minimum()).round();
        if preferred != self.side_bar_preferred {
            self.side_bar_preferred = preferred;
            self.relayout();
        }
    }

    /// The horizontal resize pointer shows while the pointer is on a side bar's drag handle or
    /// drags it (`RTV:1886`).
    fn update_side_handle_hover(&mut self) {
        let (x, y) = self.cursor;
        let on = matches!(self.drag, Drag::SideBar { .. }) || self.side_handle().is_some_and(|h| h.contains(x, y));
        if on == self.side_handle_hover {
            return;
        }
        self.side_handle_hover = on;
        if let Some(g) = &self.gpu {
            use winit::window::CursorIcon;
            g.window.set_cursor(if on {
                CursorIcon::EwResize
            } else if self.hover.is_some() {
                CursorIcon::Pointer
            } else {
                CursorIcon::Default
            });
        }
    }

    /// The window's room below the line under the title bar and above any banner, which the
    /// toolbelt, the tab bar and the sessions share (`window_layout`). The line's pixel is
    /// taken here, so panes and tmux's size report see it (`PT:13235-13237`).
    fn window_room(&self) -> Rect {
        let (w, h) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
        let bottom = if self.banners.text().is_some() { self.cell().1 + 8.0 } else { 0.0 };
        let top = self.division_height();
        Rect { x: 0.0, y: top, w, h: (h - bottom - top).max(1.0) }
    }

    /// The line under the title bar's height in the current window's pixels: one point, or
    /// none when it does not show (`chrome::division_height`).
    fn division_height(&self) -> f32 {
        crate::chrome::division_height(self.division_visible(), self.scale())
    }

    /// The line under the title bar shows in the current window (`chrome::division_visible`).
    fn division_visible(&self) -> bool {
        let decorated = self.gpu.as_ref().is_some_and(|g| g.window.is_decorated());
        let top_bar = self.tab_bar_visible() && self.tab_position() == TabPosition::Top;
        crate::chrome::division_visible(self.globals.bool("EnableDivisionView"), self.tab_style(), decorated, self.compact_window(), self.full_screen(), top_bar)
    }

    /// The window's room for tabs: beside the tab bar, above any banner, left of the toolbelt.
    /// Every pane rectangle and tmux's size report come from here, so tmux is told the size
    /// without the toolbelt and the bar.
    fn window_area(&self) -> Rect {
        self.window_layout().1
    }

    /// The area the active tab's panes share (less the window's status bar, when it shows).
    fn content_area(&self) -> Rect {
        match self.tabs.get(self.active) {
            Some(t) => self.content_area_for(t),
            None => self.window_area(),
        }
    }

    /// The text area inside a pane rectangle.
    fn inner(&self, rect: Rect) -> Rect {
        rect.inset(PAD * self.scale())
    }

    fn grid_size(&self, rect: Rect) -> (u16, u16) {
        let (cw, ch) = self.cell();
        let inner = self.inner(rect);
        ((inner.w / cw).floor().max(2.0) as u16, (inner.h / ch).floor().max(1.0) as u16)
    }

    /// The profile with this Guid, or the default profile.
    fn profile(&self, guid: Option<&str>) -> Profile {
        guid.and_then(|g| self.store.get(g)).unwrap_or_else(|| self.store.default_profile()).clone()
    }

    /// The appearance everything follows: a fixed Theme's own mode, else Windows' for this window.
    fn dark(&self) -> bool {
        crate::tab_theme::forced_dark(self.tab_style()).unwrap_or_else(|| self.system_dark())
    }

    /// The window's own winit theme (Windows' mode when nothing is forced), dark when unknown.
    pub(super) fn system_dark(&self) -> bool {
        self.gpu.as_ref().and_then(|g| g.window.theme()).is_none_or(|t| t == winit::window::Theme::Dark)
    }

    /// The tab bar's settings for this window: the globals, with the label size the resolved
    /// theme draws (`tab_theme::font_pt`) unless a custom size is on. The one place that builds
    /// them, so layout, hits and drags agree.
    pub(super) fn tab_settings(&self) -> tab_layout::Settings {
        let mut s = tab_layout::Settings::from_globals(&self.globals).with_theme_font(&self.globals, crate::tab_theme::font_pt(self.resolved_style()));
        s.height = self.theme_bar_height(self.compact_window(), self.tab_position(), s.height);
        s
    }

    /// A tab bar's thickness in points under the Theme (`tab_theme::bar_height`): Minimal's
    /// `CompactMinimalTabBarHeight` in a compact window's top bar and in a side bar, else `regular`.
    fn theme_bar_height(&self, compact_window: bool, position: TabPosition, regular: f32) -> f32 {
        let tall = self.globals.number("CompactMinimalTabBarHeight").map_or(38.0, |v| v as f32);
        crate::tab_theme::bar_height(self.tab_style(), compact_window, position, regular, tall)
    }

    /// A Minimal or Compact window's caption strip in pixels, at the bar's height whatever the bar's
    /// position or whether a bar shows (the panes never move when it hides); None in any other window.
    pub(super) fn caption_strip(&self) -> Option<f32> {
        if !self.compact_window() {
            return None;
        }
        let regular = tab_layout::Settings::from_globals(&self.globals).height;
        Some((self.theme_bar_height(true, TabPosition::Top, regular) * self.scale()).round())
    }

    /// The colors the tab bar draws with (`tab_theme::palette`), from the Theme, Windows' modes,
    /// the window being key, the three tab darkness keys and the focused pane's background.
    pub(super) fn tab_palette(&self) -> crate::tab_theme::TabPalette {
        use crate::tab_theme::{Inputs, MinimalKnobs, dimming, DIMMING_AMOUNT};
        let g = &self.globals;
        let session_bg = self
            .tabs
            .get(self.active)
            .and_then(|t| self.panes.get(&t.focused))
            .map_or(crate::palette::Palette::default().bg, |p| p.screen.palette.bg);
        let focused = self.focused && self.app_active;
        let float = |key: &str, default: f32| g.number(key).map_or(default, |v| v as f32);
        let base = MinimalKnobs::default();
        let mut minimal = MinimalKnobs {
            difference: float("MinimalTabStyleBackgroundColorDifference", base.difference),
            outline: float("MinimalTabStyleOutlineStrength", base.outline),
            legibility: float("MinimalTextLegibilityAdjustment", base.legibility),
            left_inset_is_first_tab: g.bool("MinimalTabStyleTreatLeftInsetAsPartOfFirstTab"),
            underline: float("MinimalSelectedTabUnderlineProminence", base.underline),
            deselected_colored_alpha: float("MinimalDeslectedColoredTabAlpha", base.deselected_colored_alpha),
        };
        // `shouldTweakMinimalTabOutlineAlpha` (`PT:10053-10107`): a dimmed inactive pane against the
        // bar raises the outline toward 1 by the dimming amount x 0.375.
        if self.dimmed_pane_touches_bar() {
            let a = DIMMING_AMOUNT * 0.375;
            minimal.outline = a + minimal.outline * (1.0 - a);
        }
        let inputs = Inputs {
            system_dark: self.system_dark(),
            system_high_contrast: crate::tab_theme::system_high_contrast(),
            focused,
            light_darkness: g.number("LightModeInactiveTabDarkness").map_or(0.07, |v| v as f32),
            dark_darkness: g.number("DarkModeInactiveTabDarkness").map_or(0.5, |v| v as f32),
            high_visibility: g.bool("HighVisibility"),
            session_bg,
            dimming: dimming(g.bool("DimBackgroundWindows"), focused, DIMMING_AMOUNT),
            minimal,
            colored_outline: g.number("ColoredSelectedTabOutlineStrength").map_or(0.5, |v| v as f32),
            colored_text_prominence: g.number("ColoredUnselectedTabTextProminence").map_or(0.5, |v| v as f32),
            tabs: self.tabs.len(),
        };
        let mut pal = crate::tab_theme::palette(self.tab_style(), &inputs);
        pal.attached = crate::tab_theme::attached(self.tab_style(), self.tab_position() == TabPosition::Top);
        pal
    }

    /// The split pane divider color (`tab_theme::divider_color`). Minimal starts from the focused
    /// pane's unprocessed background; at prominence 0 the divider takes the one background the
    /// window's other panes share (`PT:1911-1950`). Inactive panes keep their own background
    /// under the dimming overlay, and Dim only text has no key, so both conditions hold.
    pub(super) fn divider_color(&self) -> crate::palette::Rgb {
        let bg = |id: PaneId| self.panes.get(&id).map(|p| p.screen.palette.bg);
        let current = self.tabs.get(self.active).map(|t| t.focused);
        let decoration_bg = current.and_then(bg).unwrap_or_else(|| crate::palette::Palette::default().bg);
        let prominence = self.globals.number("MinimalSplitPaneDividerProminence").map_or(0.15, |v| v as f32);
        let panes: Vec<(PaneId, crate::palette::Rgb)> = self.tabs.iter().flat_map(|t| t.layout.panes()).filter_map(|id| bg(id).map(|c| (id, c))).collect();
        let invisible = crate::tab_theme::single_other_background(&panes, current);
        crate::tab_theme::divider_color(self.tab_style(), self.resolved_style(), decoration_bg, prominence, invisible)
    }

    /// Whether a pane is drawn undimmed: the focused pane of a window that is not key while Dim
    /// background windows is off, or (in the key window) a pane the broadcast input reaches.
    fn pane_undimmed(&self, id: PaneId, focused_pane: PaneId, receives: bool, bview: &crate::broadcast::View) -> bool {
        (!self.focused && id == focused_pane && !self.globals.bool("DimBackgroundWindows")) || (self.focused && receives && self.broadcast.should_broadcast_to(bview, focused_pane, Some(focused_pane)))
    }

    /// Whether a dimmed pane other than the tab's focused one lies against the tab bar's edge
    /// (`shouldTweakMinimalTabOutlineAlpha`): not in a background window that dims as a whole,
    /// whose dimming looks fine under the outline.
    fn dimmed_pane_touches_bar(&self) -> bool {
        use tab_position::TabPosition::{Bottom, Left, Right, Top};
        if self.tab_style() != crate::tab_theme::TabStyle::Minimal || (self.globals.bool("DimBackgroundWindows") && !self.focused) {
            return false;
        }
        let Some(tab) = self.tabs.get(self.active) else { return false };
        let area = self.content_area_for(tab);
        let bview = self.broadcast_view();
        let position = self.tab_position();
        self.visible(tab).into_iter().any(|(id, r)| {
            let touches = match position {
                Top => r.y <= area.y + 1.0,
                Bottom => r.y + r.h >= area.y + area.h - 1.0,
                Left => r.x <= area.x + 1.0,
                Right => r.x + r.w >= area.x + area.w - 1.0,
            };
            touches && id != tab.focused && !self.pane_undimmed(id, tab.focused, self.broadcast.receives(&bview, id), &bview)
        })
    }

    /// The Theme setting (`TabStyleWithAutomaticOption`), Automatic when unset.
    pub(super) fn tab_style(&self) -> crate::tab_theme::TabStyle {
        use crate::tab_theme::{KEY, TabStyle, from_value};
        self.globals.get(KEY).and_then(|v| v.as_integer()).map_or(TabStyle::Automatic, from_value)
    }

    /// The fixed style the current window draws (`crate::tab_theme::resolved`).
    pub(super) fn resolved_style(&self) -> crate::tab_theme::TabStyle {
        crate::tab_theme::resolved(self.tab_style(), self.system_dark(), crate::tab_theme::system_high_contrast())
    }

    fn start_pane(&mut self, rect: Rect, guid: Option<&str>) -> Option<PaneId> {
        let id = self.next_id;
        let started = self.start_pane_as(id, rect, guid);
        if started {
            self.next_id += 1;
        }
        started.then_some(id)
    }

    /// Restart Session: a new shell in the same pane, with the same profile. The pane is in
    /// the current window (in any tab, shown or not) or buried; a buried one starts at the
    /// content area's size and takes its tab's size when it comes back. The menu always
    /// passes the focused, shown pane (its rect); the tab and buried fallbacks are for the
    /// API's RestartSession, which may name a background tab's or a buried session.
    fn restart_pane(&mut self, id: PaneId) {
        let in_tab = || {
            let tab = self.tabs.iter().find(|t| t.layout.panes().contains(&id))?;
            tab.layout.rects(self.content_area_for(tab), GAP).into_iter().find(|(p, _)| *p == id).map(|(_, r)| r)
        };
        let Some(rect) = self.pane_rect(id).or_else(in_tab).or_else(|| self.buried.contains(id).then(|| self.content_area())) else { return };
        // Instant Replay closes first; the new shell starts live.
        self.end_replays(&[id]);
        let guid = self.panes.get(&id).map(|p| p.profile.clone());
        // The session keeps its chat reference across a restart (`PTYSession.m:2403-2420`).
        let reference = self.panes.get(&id).map(|p| p.session_guid.clone());
        if let Some(old) = self.panes.remove(&id) {
            if let Some(pid) = old.shell_pid() {
                self.jobs.forget(pid);
            }
            self.ssh_pane_dropped(&old);
            // The old shell is gone for good; the new one gets its own cookie.
            self.forget_cookie(&old);
        }
        // Its inline files went with its screen.
        self.close_inline_questions_of(id);
        if self.start_pane_as(id, rect, guid.as_deref()) {
            if let (Some(reference), Some(p)) = (reference.clone(), self.panes.get_mut(&id)) {
                p.keep_reference(&reference);
            }
            // The new Screen has no chat watch: a chat streaming this session goes on (`CVC:144-165`).
            self.chat_refresh();
        } else if let Some(reference) = reference {
            // The old pane is gone and no new one started: its chats unlink with the notice and
            // stop streaming, as if the pane had closed.
            self.chat_reference_gone(id, &reference);
        }
    }

    /// "Restart session?" answered OK: the pane restarts in its own window (gone: nothing
    /// happens).
    pub(super) fn restart_answered(&mut self, id: PaneId) {
        if !self.panes.contains_key(&id) {
            return;
        }
        self.enter_pane_window(id);
        self.restart_pane(id);
    }

    /// The session variables for pane `id`, about to start with profile `p` (`PS:3246-3350`),
    /// and the cookie minted for it: a Session cookie in the jar, only while "Enable Python
    /// API" and `setCookie` are both on and the server runs. The numbers are those at launch:
    /// the window's number, the tab it goes into, and iTerm2's pane number (the tab's session
    /// count less one once it is in the tab, `TAB:7435-7443`). The result holds a secret:
    /// never log it. A pane started on SSH transport `t` other than ConPTY is told which
    /// (`TANDEM_SSH_TRANSPORT`, for it2ssh).
    fn session_env(&mut self, id: PaneId, p: &Profile, t: crate::ssh_transport::Transport) -> (Vec<String>, Option<String>) {
        let window = self.window_number();
        let counts: Vec<usize> = self.tabs.iter().map(|t| t.layout.panes().len()).collect();
        let holding = self.tabs.iter().position(|t| t.layout.panes().contains(&id));
        let (tab, pane) = crate::session_env::numbers(&counts, holding, self.start_tab);
        // The REPL's variables (`start_env_override`) carry its own cookie, which then is the
        // pane's (it leaves the jar with the pane); no Session cookie is minted beside it.
        let extra = self.start_env_override.clone().unwrap_or_default();
        let given = extra.iter().find_map(|e| e.strip_prefix("ITERM2_COOKIE=")).map(str::to_string);
        let cookie = given.or_else(|| {
            (self.globals.bool(api_ui::ENABLE_KEY) && self.globals.bool(api_ui::SET_COOKIE_KEY) && self.api_server.is_some())
                .then(|| self.api_jar.lock().unwrap().mint(crate::api_auth::CookieUse::Session))
        });
        let name = p.name();
        let id = id.to_string();
        // The session starts with the profile's Unicode version, as its Capabilities reply does.
        let version = crate::osc1337::profile_unicode(p.values.get("Unicode Version").and_then(|v| v.as_integer()));
        let term_features = crate::osc1337::capabilities(crate::osc1337::session_features(self.globals.bool("AllowClipboardAccess"), 0, version));
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        let path = self
            .utilities
            .as_ref()
            .filter(|_| self.globals.bool("addUtilitiesToPATH"))
            .and_then(|folder| crate::utilities::path_entry(&folder.to_string_lossy()))
            .and_then(|entry| crate::utilities::session_path(std::env::var_os("PATH").as_deref(), &system_root, &entry));
        // Every profile's Path to SSH is passed (`PS:3247-3252`, `PS:3548`).
        let ssh_path = crate::ssh_config::from_profile(p).path_to_ssh;
        let transport = (t != crate::ssh_transport::Transport::Conpty).then(|| crate::ssh_transport::label(t));
        let facts = crate::session_env::Facts {
            window,
            tab,
            pane,
            id: &id,
            profile_name: &name,
            cookie: cookie.as_deref(),
            term_features: &term_features,
            lc_terminal: self.globals.bool("shouldSetLCTerminal"),
            path: path.as_deref(),
            ssh_path: ssh_path.as_deref(),
            ssh_transport: transport,
        };
        (crate::session_env::overlay(crate::session_env::vars(&facts), &extra), cookie)
    }

    /// A pane dropped for good takes its cookie out of the jar (a pane stashed for Undo Close
    /// keeps it).
    fn forget_cookie(&self, pane: &Pane) {
        self.forget_cookie_text(pane.cookie.as_deref());
    }

    /// A pane dropped for good (closed with no Undo Close, its Undo Close expired, or a
    /// failed arrangement's): its API subscriptions, unread counts and script caches go.
    /// Every drop path calls this, so an app-wide map keyed by pane cannot leak.
    pub(super) fn pane_gone(&mut self, id: PaneId) {
        self.api_session_subs.forget_session(id);
        self.held_on_exit.remove(&id);
        self.unread_session_counts.retain(|(p, _), _| *p != id);
        self.script_templates.retain(|(p, _), _| *p != id);
        self.script_components.retain(|(p, _), _| *p != id);
        self.script_titles.remove(&id);
        if self.text_popover.as_ref().is_some_and(|(p, _)| *p == id) {
            self.text_popover = None;
        }
    }

    /// A cookie that will never be used takes itself out of the jar.
    fn forget_cookie_text(&self, cookie: Option<&str>) {
        if let Some(c) = cookie {
            self.api_jar.lock().unwrap().remove(c);
        }
    }

    fn start_pane_as(&mut self, id: PaneId, rect: Rect, guid: Option<&str>) -> bool {
        let (cols, rows) = self.grid_size(rect);
        let p = self.profile(guid);
        let (cmd, transport) = match self.start_command_override.clone() {
            // The API's create command never takes the SSH path.
            Some(cmd) => (cmd, crate::ssh_transport::Transport::Conpty),
            None => self.profile_command(&p),
        };
        // A restored arrangement names each session's folder.
        let (cwd, warning) = match &self.start_dir_override {
            Some(crate::arrangement::Folder::Forced(d)) if std::path::Path::new(d).is_dir() => (PathBuf::from(d), None),
            // A git command's window runs only in its folder, never the profile's.
            Some(crate::arrangement::Folder::Forced(d)) if self.start_command_override.is_some() => {
                self.banners.set("start", format!("The folder {d} no longer exists, so git did not run."));
                return false;
            }
            Some(crate::arrangement::Folder::Forced(d)) => {
                let (c, _) = profile::start_dir_with(&p, None);
                (c, Some(format!("The folder {d} from the arrangement does not exist; the profile's folder is used.")))
            }
            Some(crate::arrangement::Folder::Previous(d)) => profile::start_dir_with(&p, Some(std::path::Path::new(d))),
            _ => profile::start_dir_with(&p, self.previous_dir().as_deref()),
        };
        match warning {
            Some(w) => self.banners.set("directory", w),
            None => self.banners.clear("directory"),
        }
        if self.host.as_ref().is_some_and(|h| h.is_lost()) {
            self.host = None;
            self.banners.set("host", "The session host stopped. New shells run inside Tandem, so they end with it.");
        }
        let (env, cookie) = self.session_env(id, &p, transport);
        let started = match &self.host {
            Some(h) => match Pane::start_hosted_transport(id, h, transport, &cmd, Some(&cwd), &env, cols, rows, self.notify.clone()) {
                // A host below protocol 4 cannot run this transport: the session runs inside
                // Tandem instead.
                Err(e) if e == crate::host_client::HOST_TOO_OLD => {
                    self.banners.set("host", "The session host is too old for this SSH session, so it runs inside Tandem.");
                    Pane::start_transport(id, &self.api, transport, &cmd, Some(&cwd), &env, cols, rows, self.notify.clone())
                }
                other => other,
            },
            None => Pane::start_transport(id, &self.api, transport, &cmd, Some(&cwd), &env, cols, rows, self.notify.clone()),
        };
        match started {
            Ok(mut pane) => {
                pane.cookie = cookie;
                pane.command = cmd;
                if self.start_command_override.is_none() {
                    // Not an ssh session for F27 yet: its hook, vouched by the origin token,
                    // makes it one.
                    pane.ssh.profile_ssh = profile::is_ssh(&p);
                    pane.ssh.initial_directory = profile::ssh_initial_directory(&p);
                }
                if let Some(jumps) = self.start_jumps_override.take() {
                    pane.pending_jumps = jumps;
                }
                self.install_pane(id, pane, &p);
                true
            }
            Err(e) => {
                self.forget_cookie_text(cookie.as_deref());
                self.banners.set("start", format!("Could not start {cmd}: {e}"));
                false
            }
        }
    }

    /// The command profile `p` starts and its transport. An SSH profile runs `it2ssh.exe` from
    /// the utilities folder `utilities::ensure` made, on the chosen SSH transport, only with SSH
    /// Integration on, the conductor hook in place and the file there; otherwise plain ssh on
    /// ConPTY, with the `ssh` banner saying why when SSH Integration is on.
    fn profile_command(&mut self, p: &Profile) -> (String, crate::ssh_transport::Transport) {
        use crate::ssh_session::{profile_banner, profile_it2ssh, CONDUCTOR_READY};
        let exe = profile_it2ssh(self.utilities.as_deref(), CONDUCTOR_READY, std::path::Path::is_file);
        let present = exe.is_some();
        let it2ssh = exe.as_deref();
        // A blank Command runs the shell, not ssh, so it gets no banner.
        let runs_ssh = profile::is_ssh(p) && !p.string("Command").trim().is_empty();
        match profile_banner(runs_ssh, crate::ssh_config::from_profile(p).ssh_integration, present) {
            Some(text) => self.banners.set("ssh", text),
            None => self.banners.clear("ssh"),
        }
        let transport = if profile::ssh_integrated(p, it2ssh) { crate::ssh_transport::CHOSEN } else { crate::ssh_transport::Transport::Conpty };
        (profile::command_line(p, &self.shell, it2ssh), transport)
    }

    /// A started pane takes its profile's look, triggers, and input settings.
    fn install_pane(&mut self, id: PaneId, mut pane: Pane, p: &Profile) {
        pane.restyle(Palette::from_profile(p, self.dark()), Look::from_profile(p));
        screen_follows_profile(&mut pane.screen, p);
        pane.profile = p.guid();
        pane.set_buffering(p.bool("Buffer Input by Default"));
        // `IRMemory` is read once per session (`S:121-122`).
        pane.dvr = crate::dvr::Dvr::new(crate::pane::capacity_for(self.globals.int("IRMemory")));
        if let Some(pid) = pane.shell_pid() {
            self.jobs.watch(pid);
            if let Some(paused) = pane.console_pause_flag() {
                self.jobs.watch_console(pid, paused);
            }
        }
        self.panes.insert(id, pane);
    }

    /// Instant Replay: every pane, background tabs and tmux panes included, records a frame
    /// of its own screen when it changed, at iTerm2's update cadence for it
    /// (`cadence::frame_gap`; iTerm2 records at each refresh of every session).
    pub(super) fn record_replay_frames(&mut self) {
        let now = Instant::now();
        let now_us = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as i64);
        let visible = self.replay_visible_panes();
        for (id, p) in self.panes.iter_mut() {
            // No gap is shorter than FAST_GAP: a pane that captured within it cannot record
            // yet, so its gap is not worked out.
            if p.replay_at.is_some_and(|t| now.saturating_duration_since(t) < crate::cadence::FAST_GAP) {
                continue;
            }
            let gap = p.replay_gap(now, visible.contains(id), self.activity.get(id).and_then(|a| a.last_output()));
            p.record_frame(now, now_us, gap);
        }
    }

    /// The panes of the tab in front of each window on screen (iTerm2's
    /// `sessionBelongsToVisibleTab` with a visible, not minimized window).
    fn replay_visible_panes(&self) -> std::collections::HashSet<PaneId> {
        let on_screen = self.gpu.as_ref().is_some_and(|g| g.window.is_minimized() != Some(true) && g.window.is_visible() != Some(false));
        let current = self.tabs.get(self.active).filter(|_| on_screen);
        current.into_iter().chain(self.parked_front_tabs()).flat_map(|t| t.layout.panes()).collect()
    }

    /// When a pane that captured a frame less than its gap ago may record again, so output
    /// that arrived in the gap is not left out of the recording until the next event.
    fn replay_frame_due(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let visible = self.replay_visible_panes();
        let mut due = None;
        for (id, p) in self.panes.iter_mut() {
            let Some(at) = p.replay_at.filter(|_| !p.gateway) else { continue };
            let t = at + p.replay_gap(now, visible.contains(id), self.activity.get(id).and_then(|a| a.last_output()));
            if t > now && due.is_none_or(|d| t < d) {
                due = Some(t);
            }
        }
        due
    }

    /// The focused session's directory, from shell integration, when it is on this machine
    /// (iTerm2 prefers the local directory for a new session).
    fn previous_dir(&self) -> Option<std::path::PathBuf> {
        let t = self.tabs.get(self.active)?;
        let shell = &self.panes.get(&t.focused)?.own().shell;
        let here = std::env::var("COMPUTERNAME").unwrap_or_default();
        if shell.host.as_ref().is_some_and(|h| !h.eq_ignore_ascii_case(&here) && !h.eq_ignore_ascii_case("localhost")) {
            return None;
        }
        profile::local_path(shell.cwd.as_deref()?)
    }

    /// The Settings window on one page.
    pub(super) fn open_settings_page(&mut self, event_loop: &ActiveEventLoop, page: crate::settings_ui::Page) {
        self.open_settings(event_loop, Mode::Settings);
        if self.settings.as_mut().is_some_and(|s| s.show_page(page)) {
            self.apply_edit(Edit::GlobalsChanged);
        }
    }

    fn open_settings(&mut self, event_loop: &ActiveEventLoop, mode: Mode) {
        // The Python API's authentication popup shows what the allow-all file says now.
        if mode == Mode::Settings {
            self.api_settings_opened();
        }
        let slot = match mode {
            Mode::Settings => &self.settings,
            Mode::Profiles => &self.profiles_win,
        };
        if let Some(s) = slot {
            s.focus();
        } else {
            self.make_settings(event_loop, mode);
        }
        // Opened any way but Edit Session from a tmux session (which sets it again after).
        let slot = match mode {
            Mode::Settings => &mut self.settings,
            Mode::Profiles => &mut self.profiles_win,
        };
        if let Some(s) = slot {
            s.set_tmux_session(false);
        }
    }

    fn make_settings(&mut self, event_loop: &ActiveEventLoop, mode: Mode) {
        let Some(g) = self.gpu.as_ref() else { return };
        match SettingsWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, mode) {
            Ok(mut s) => {
                s.follow_theme(crate::tab_theme::forced_dark(self.tab_style()));
                // Hotkeys that failed to register show in the sheet.
                s.hotkeys.errors = self.hotkey_errors.clone();
                s.title_providers = self.title_provider_items();
                s.ai.found = self.ai_discovery.all();
                match mode {
                    Mode::Settings => self.settings = Some(s),
                    Mode::Profiles => self.profiles_win = Some(s),
                }
            }
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Saves after an edit in the settings window and applies it to every open pane.
    fn apply_edit(&mut self, edit: Edit) {
        if let Some(w) = self.last_window {
            self.enter(w);
        }
        match edit {
            Edit::None => return,
            Edit::Open(guid, how) => match how {
                // A tab or split goes to the window the person used last.
                How::Tab => self.new_tab(Some(&guid)),
                How::Window => self.pending_windows.push(windows_ui::NewWindow::Profile(Some(guid))),
                How::SplitRight => self.split(Axis::Row, Some(&guid)),
                How::SplitDown => self.split(Axis::Column, Some(&guid)),
            },
            Edit::Error(e) => self.banners.set("import", e),
            Edit::SnippetsChanged => self.save_snippets(),
            Edit::GlobalsChanged => self.globals_changed(true),
            Edit::PrefsCopyNow => {
                self.globals_changed(true);
                self.prefs_offer_copy();
            }
            Edit::PrefsSaveNow => self.prefs_save_now(true),
            Edit::ExportAll => self.export_all(),
            Edit::ImportAll => self.import_all(),
            Edit::EraseAll => self.erase_all(),
            Edit::Changed(_) | Edit::Imported(_) => {
                // Rules may have changed: every session looks again (iTerm2's reloadAllProfiles).
                for s in self.aps.values_mut() {
                    s.mark_dirty();
                }
                if matches!(edit, Edit::Imported(_)) {
                    self.save_globals();
                }
                // Saving waits for a pause in editing, so a color drag does not write the file every frame.
                self.save_due = Some(Instant::now() + SAVE_DELAY);
                self.refresh_panes();
                self.apply_fonts();
                // A hotkey may have changed, and the Snippets Filter.
                self.register_hotkeys();
                self.refresh_snippet_menus();
            }
        }
        self.relayout();
        // A profile's bar or the bar settings changed: other windows' panes may have
        // gained or lost a bar (per pane, their room changes without the content area).
        self.relayout_changed_windows();
    }

    /// A global setting changed: saved and applied. With `api_now` false, "Enable Python API"
    /// and the authentication popup wait for `api_settings_changed` (the dispatch loop's
    /// settle, or the next pass), so a script's request that changed them is answered first.
    pub(super) fn globals_changed(&mut self, api_now: bool) {
        self.save_globals();
        if api_now {
            self.api_settings_changed();
        }
        // The Actions tool shows the actions.
        self.tools_changed();
        self.settle_action_undo();
        self.menus_dirty = true;
        self.sync_checks();
        self.register_hotkeys();
        self.scripts_folder_changed();
        // The Theme may have changed: every window follows the forced mode (or Windows' again).
        self.apply_theme_everywhere();
        // The border settings, or the Theme, may have changed.
        self.apply_border_everywhere();
        // AllowLiveResize may have changed.
        self.apply_resizable_everywhere();
    }

    /// Starts watching and scanning the Scripts folder the settings name (`FM:236-252`).
    fn scripts_scan_start(&mut self) {
        let keys = (self.globals.bool("UseCustomScriptsFolder"), self.globals.string("CustomScriptsFolder"));
        let folder = self.scripts_folder();
        // Reached through a link to a network path: the scanner refuses it (an empty tree,
        // nothing made, watched or polled), and the app-wide entry says why once.
        if !crate::scripts::ancestors_stay_local(&folder, &crate::scripts::read_link) {
            self.script_log_app(crate::scripts::scripts_folder_not_local(&folder));
        }
        // The old scanner's thread ends on its own; a scan it is in finishes first.
        let mut scanner = crate::scripts_scan::Scanner::start(folder, self.notify.clone());
        // A build under way keeps holding scans (its file events), as with the old scanner.
        scanner.hold(self.provisioner.as_ref().is_some_and(|p| p.busy()));
        self.scripts_scan = Some(scanner);
        self.scripts_keys = keys;
    }

    /// The Scripts folder the settings name now (`scriptsPath`, `FM:236-252`).
    pub(super) fn scripts_folder(&self) -> PathBuf {
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
        crate::scripts::scripts_folder(self.globals.bool("UseCustomScriptsFolder"), &self.globals.string("CustomScriptsFolder"), &appdata, &home, &crate::scripts::RealFs)
    }

    /// iTerm2's `scriptsFolderDidChange:` (`M:172-178`): either scripts folder setting changed
    /// (the text only when its editing ended), so the folder is watched and scanned afresh and
    /// a stopped scan is allowed again.
    fn scripts_folder_changed(&mut self) {
        if self.scripts_scan.is_none() {
            return;
        }
        if (self.globals.bool("UseCustomScriptsFolder"), self.globals.string("CustomScriptsFolder")) != self.scripts_keys {
            self.scripts_scan_start();
        }
    }

    /// Takes the scanner's results: the tree and the script names, and the slow-scan question
    /// ("Performance Issue", Stop or Continue, `M:489-509`). Each archive found is asked about
    /// once, whether or not the tree changed.
    fn scripts_scan_tick(&mut self) {
        while let Some((event, folder)) = self.scripts_scan.as_ref().and_then(|s| Some((s.try_event()?, s.folder().to_path_buf()))) {
            match event {
                crate::scripts_scan::ScanEvent::Slow => {
                    let text = crate::scripts::slow_text(&folder);
                    let buttons = [crate::scripts::STOP, crate::scripts::CONTINUE];
                    self.ask_scripts(scripts_alerts::ask(crate::scripts::SLOW_HEADING, &text, &buttons, None, crate::warnings::Kind::Persistent, scripts_alerts::Then::SlowScan));
                }
                // An unchanged tree is left alone, so nothing built from it is rebuilt.
                crate::scripts_scan::ScanEvent::Done(result) if result.tree != self.scripts_tree => {
                    self.archives_found(&result.archives);
                    self.scripts_all = crate::scripts::all_scripts(&result.tree);
                    self.scripts_tree = result.tree;
                    self.refresh_scripts_menu();
                    self.deps_tree_changed();
                }
                crate::scripts_scan::ScanEvent::Done(result) => self.archives_found(&result.archives),
            }
        }
    }

    /// Rebuilds every pane's colors and cursor from its profile and the system appearance.
    fn refresh_panes(&mut self) {
        // Session copies follow edits to their saved profiles first.
        self.store.merge_sessions();
        self.tools_changed();
        let dark = self.dark();
        for pane in self.panes.values_mut() {
            let p = self.store.get(&pane.profile).unwrap_or_else(|| self.store.default_profile());
            pane.restyle(Palette::from_profile(p, dark), Look::from_profile(p));
            screen_follows_profile(pane.own_mut(), p);
        }
        self.tmux_resend_colors();
        self.tmux_save_tab_colors();
        // "Status bar enabled" may have changed for a tmux tab.
        self.tmux_watch_status_all();
        self.redraw();
    }

    fn save_now(&mut self) {
        self.save_due = None;
        self.tools_changed();
        match self.store.save() {
            Ok(()) => self.banners.clear("save"),
            Err(e) => self.banners.set("save", format!("Could not save settings: {e}")),
        }
        self.prefs_changed();
    }

    /// Takes the renderer's fonts from the default profile. A font that is not
    /// installed is named in the banner.
    fn apply_fonts(&mut self) {
        let p = self.store.default_profile().clone();
        let mut normal = p.font("Normal Font");
        let mut alt = p.bool("Use Non-ASCII Font").then(|| p.font("Non Ascii Font").family);
        normal.size_pt = (normal.size_pt + self.font_offset).clamp(4.0, 128.0);
        // Re-measuring the font re-shapes all text, so only do it when a font setting changed.
        // The settings are compared before the font names are looked up (every frame asks).
        let sig = format!("{normal:?} {alt:?} {} {} {} {}", p.float("Horizontal Spacing"), p.float("Vertical Spacing"), self.scale(), p.font("Badge Font").family);
        if sig == self.font_sig {
            return;
        }
        self.font_sig = sig;
        if let Some(g) = self.gpu.as_ref() {
            normal.family = g.renderer.resolve_family(&normal.family);
            alt = alt.map(|a| g.renderer.resolve_family(&a));
        }
        let Some(g) = self.gpu.as_mut() else { return };
        let px = normal.size_pt * 4.0 / 3.0 * g.window.scale_factor() as f32;
        g.renderer.set_fonts(&normal.family, alt.as_deref(), px, p.float("Horizontal Spacing") as f32, p.float("Vertical Spacing") as f32);
        let badge = p.font("Badge Font").family;
        g.renderer.set_badge_family(if badge.is_empty() { "Helvetica" } else { &badge });
        // Read with the window's other fonts, not on every change (iTerm2: "Changing this
        // setting does not affect existing windows").
        g.renderer.set_toolbelt_family(&self.globals.string("toolbeltFont"));
        g.renderer.set_toolbelt_size(self.globals.get("toolbeltFontSize").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(0.0) as f32);
        let missing: Vec<String> = std::iter::once(normal.family).chain(alt).filter(|f| !g.renderer.has_family(f)).collect();
        if missing.is_empty() {
            self.banners.clear("font");
        } else {
            self.banners.set("font", format!("Font not installed: {}. Text uses a fallback font.", missing.join(", ")));
        }
    }

    /// Make Text Bigger, Make Text Smaller and Make Text Normal Size: the shared font offset
    /// becomes `offset`. With `AdjustWindowForFontSizeChange`, a window that may change size
    /// (`window_can_fit`: a Normal or No Title Bar window that is not full screen, maximized,
    /// minimized, snapped, size-locked or a hotkey window) then keeps its
    /// tab's grid at the new cell (`reallyChangeSessionFontSize:adjustWindow:`,
    /// `TAB:2189-2205`); else the panes refit the window as it is, and a tmux tab tells tmux
    /// through the size report. The caller lays out. 13n's `change_font_size` takes this over.
    fn set_font_offset(&mut self, offset: f32) {
        let grid = self.grid_size(self.content_area());
        self.font_offset = offset;
        let sig = self.font_sig.clone();
        self.apply_fonts();
        if sig != self.font_sig && self.globals.bool("AdjustWindowForFontSizeChange") && self.window_can_fit() {
            self.fit_window_to_grid(grid);
        }
    }

    /// Resizes the current window so its tab shows `grid` at the current cell, the top-left
    /// kept and the frame kept on the work area (`chrome::font_fit`). The decoration and the
    /// padding around the grid are the ones the edge snap measures (`snap_grid`); the Resized
    /// event that follows lays the window out at its new size.
    pub(super) fn fit_window_to_grid(&mut self, grid: (u16, u16)) {
        let Some(snap) = self.snap_grid() else { return };
        let Some((work, _)) = self.monitor_rects() else { return };
        let Some(g) = self.gpu.as_ref() else { return };
        let chrome = (snap.decoration.0 + snap.margins.0, snap.decoration.1 + snap.margins.1);
        let now = Self::outer_rect(g);
        let r = crate::chrome::font_fit(grid, snap.cell, chrome, work, now);
        if (r.x, r.y) != (now.x, now.y) {
            g.window.set_outer_position(winit::dpi::PhysicalPosition::new(r.x.round() as i32, r.y.round() as i32));
        }
        // The frame is the outer frame; the inner size leaves the borders out.
        let (outer, inner) = (g.window.outer_size(), g.window.inner_size());
        let (dw, dh) = (outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height));
        let size = winit::dpi::PhysicalSize::new((r.w.round() as u32).saturating_sub(dw).max(1), (r.h.round() as u32).saturating_sub(dh).max(1));
        if size != inner {
            let _ = caption_ui::request_inner_size(&g.window, size);
        }
    }

    /// Sizes the window to the profile's Columns and Rows, or to its percentages of the work area.
    fn size_window(&mut self, guid: Option<&str>) {
        let p = guid.and_then(|g| self.store.get(g)).unwrap_or_else(|| self.store.default_profile()).clone();
        let (cw, ch) = self.cell();
        let pad = 2.0 * PAD * self.scale();
        // The line under the title bar is part of the window's decoration (`PT:13235-13237`).
        let line = self.division_height() + self.caption_strip().unwrap_or(0.0);
        let Some(g) = self.gpu.as_ref() else { return };
        // A percent-eligible type sizes by its explicit percentages of the work area, in whole
        // cells (`TerminalWindowSizeHelper.measurements`, `measurementToCells`).
        let work = self.monitor_rects().map(|(work, _)| work);
        let (pw, ph) = {
            let (w, h) = crate::frames::profile_percents(&p);
            crate::frames::percents(p.int("Window Type"), w, h)
        };
        let columns = match (pw, work) {
            (Some(pct), Some(work)) => i64::from(crate::frames::percent_cells(pct, cw, work.w)),
            _ => p.int("Columns").max(10),
        };
        let rows = match (ph, work) {
            (Some(pct), Some(work)) => i64::from(crate::frames::percent_cells(pct, ch, work.h)),
            _ => p.int("Rows").max(4),
        };
        let (mut w, mut h) = (columns as f32 * cw + pad, rows as f32 * ch + pad + line);
        if let Some(work) = work.filter(|_| pw.is_some() || ph.is_some()) {
            // The whole window stays on the work area.
            let (outer, inner) = (g.window.outer_size(), g.window.inner_size());
            w = w.min(work.w - outer.width.saturating_sub(inner.width) as f32);
            h = h.min(work.h - outer.height.saturating_sub(inner.height) as f32);
        }
        let _ = caption_ui::request_inner_size(&g.window, winit::dpi::PhysicalSize::new(w.max(1.0).ceil() as u32, h.max(1.0).ceil() as u32));
    }

    /// "Always open in a new window, never in a tab": a new tab of `guid` opens as a window
    /// instead (a new window's first tab is the one exception).
    fn new_tab_opens_window(&self, guid: Option<&str>) -> bool {
        let prevent = guid.and_then(|g| self.store.get(g)).unwrap_or_else(|| self.store.default_profile()).bool("Prevent Opening in a Tab");
        !self.tabs.is_empty() && crate::winmodel::open_target(crate::winmodel::OpenRequest::NewTab, prevent, false) == crate::winmodel::Target::Window
    }

    fn new_tab(&mut self, guid: Option<&str>) {
        if !self.tabs.is_empty() && !self.layout_allowed(crate::winmodel::LayoutOp::AddTab) {
            return;
        }
        self.add_tab(guid);
    }

    /// A new tab in the current window without the Lock Layout check: the API's CreateTab
    /// (iTerm2 guards user tab creation through menu validation, `PT:13832`, and its API
    /// handler has no lock check, `H:2465-2588`).
    fn add_tab(&mut self, guid: Option<&str>) {
        if self.new_tab_opens_window(guid) {
            self.pending_windows.push(windows_ui::NewWindow::Profile(guid.map(str::to_string)));
            return self.redraw();
        }
        let area = self.content_area();
        if let Some(id) = self.start_pane(area, guid) {
            // The new tab's profile may give it a title (`PT:15265-15269`).
            let title = self.panes.get(&id).and_then(|p| {
                let profile = self.store.get(&p.profile)?;
                crate::wintitle::custom_tab_format(profile.bool("Use Custom Tab Title"), &profile.string("Custom Tab Title"), p.tmux_io().is_some())
            });
            self.tabs.push(Tab { title, ..Tab::new(self.next_tab_id, Layout::new(id), id) });
            self.next_tab_id += 1;
            self.active = self.tabs.len() - 1;
            self.relayout();
        }
    }

    fn split(&mut self, axis: Axis, guid: Option<&str>) {
        if !self.layout_allowed(crate::winmodel::LayoutOp::Split) {
            return;
        }
        // No split while Instant Replay shows (`PT:11124-11129`).
        if crate::replay::on_event(self.in_instant_replay(), crate::replay::ReplayEvent::Split) == crate::replay::Close::Refuse {
            return;
        }
        let Some(target) = self.tabs.get(self.active).map(|t| t.focused) else { return };
        if self.tmux_split(target, axis == Axis::Row) {
            return;
        }
        let tab = &self.tabs[self.active];
        let area = self.content_area();
        let Some((_, rect)) = tab.layout.rects(area, GAP).into_iter().find(|(id, _)| *id == target) else { return };
        let half = match axis {
            Axis::Row => Rect { w: rect.w / 2.0, ..rect },
            Axis::Column => Rect { h: rect.h / 2.0, ..rect },
        };
        self.start_tab = Some(self.active);
        let started = self.start_pane(half, guid);
        self.start_tab = None;
        if let Some(id) = started {
            let tab = &mut self.tabs[self.active];
            tab.layout.split(target, axis, id);
            tab.focused = id;
            tab.zoomed = None;
            self.relayout();
        }
    }

    /// Closes a pane. The last pane closes its tab, and the last tab exits.
    fn close_pane(&mut self, id: PaneId, event_loop: &ActiveEventLoop) {
        // Every chat linked to this session unlinks while its reference still names the pane.
        self.chat_pane_gone(id);
        // AI requests that belong to the pane end with it.
        self.composer_ai_cancel(id);
        self.ai_runs_pane_gone(id);
        // Codecierge's session for the pane stops for real (its run, its pending call, its queue)
        // and goes with it (`CG:36-40`, ruling R12e-3).
        if let Some(guid) = self.panes.get(&id).map(|p| p.session_guid.clone()) {
            self.codecierge_stop(&guid, crate::codecierge::Stage::Onboarding);
            self.codecierge.remove(&guid);
        }
        self.ai_annotations.retain(|_, (pane, _)| *pane != id);
        // A git command's window closed any way ends its command, as does Manage
        // Dependencies' pip command.
        self.single_use_ended(id);
        self.deps_command_ended(id);
        // A buried session that ends leaves the Buried Sessions menu.
        if self.buried.take(id).is_some() {
            self.menus_dirty = true;
        }
        self.enter_pane_window(id);
        // Instant Replay closes before the pane (and its recorder) goes (`PT:2766-2770`); the
        // panel of the pane itself closes with it.
        if self.in_instant_replay() {
            if self.focused_id() == Some(id) {
                self.replay_window = None;
            } else {
                let focused = self.focused_id();
                self.replay_event(crate::replay::ReplayEvent::PaneClosed, focused, event_loop);
            }
        }
        if let Some(p) = self.panes.get_mut(&id) {
            if p.replay.take().is_some() {
                p.end_view();
            }
        }
        self.tmux_pane_closed(id);
        self.aps.remove(&id);
        self.focus_order.remove(&id);
        if let Some(p) = self.panes.remove(&id) {
            if let Some(pid) = p.shell_pid() {
                self.jobs.forget(pid);
            }
            // A close the person made keeps a running shell for Undo Close.
            match self.undo_capture.as_mut() {
                // iTerm2 never offers Undo Close for a tmux pane, nor for a loaded
                // recording's (a synthetic session, `PS:3931`).
                Some(c) if p.exited.is_none() && p.tmux_io().is_none() && !p.dvr.read_only => {
                    let mut p = p;
                    p.set_paused(true);
                    c.panes.push(p);
                }
                _ => {
                    self.ssh_pane_dropped(&p);
                    self.forget_cookie(&p);
                    self.forget_session_profile(&p.profile);
                    // Gone for good (a pane kept for Undo Close keeps its state).
                    self.pane_gone(id);
                }
            }
        }
        if self.shot.as_ref().is_some_and(|s| s.pane == id) {
            self.shot = None;
        }
        self.finds.remove(&id);
        self.composers.remove(&id);
        self.copy_modes.remove(&id);
        self.note_pos.remove(&id);
        self.show_stamps.remove(&id);
        self.show_guide.remove(&id);
        self.bars.remove(&id);
        self.pending_reads.remove(&id);
        self.pending_properties.remove(&id);
        self.pending_urls.retain(|(p, _), _| *p != id);
        self.uploads.remove(&id);
        self.pending_background.remove(&id);
        self.flashes.remove(&id);
        self.fireworks.remove(&id);
        self.tab_colors.remove(&id);
        if self.note_win.as_ref().is_some_and(|(_, p, _)| *p == id) {
            self.note_win = None;
        }
        self.clips.remove(&id);
        self.inline_drop(id);
        if self.add_clip_win.as_ref().is_some_and(|(_, p)| *p == id) {
            self.add_clip_win = None;
        }
        if let Some(t) = self.tabs.iter().position(|t| t.layout.panes().contains(&id)) {
            let tab = &mut self.tabs[t];
            if tab.layout.remove(id) {
                if tab.focused == id {
                    tab.focused = tab.layout.panes()[0];
                }
            } else {
                let gone = self.tabs.remove(t);
                self.mru.forget(gone.id);
                self.api_vars.forget(crate::api_vars::Scope::Tab, &gone.id.to_string());
                self.active = crate::close_rules::active_after_close(self.active, t, self.tabs.len(), self.globals.bool("moveLeftAfterClosingTab"));
                // The neighbor selected may be hidden in a collapsed group.
                self.expand_selected_group();
            }
        }
        if self.tabs.is_empty() {
            // The last window's last tab closed while sessions are buried: the newest comes
            // back here instead of Tandem ending with them.
            if self.windows.parked().next().is_none() && self.buried.newest().is_some() {
                self.disinter_here();
                return;
            }
            // The window's last tab closed: the window goes; the last window ends Tandem. The
            // settings folder's quit question may open after it has gone, with its GPU.
            self.prefs_gpu = self.gpu.as_ref().map(|g| (g.instance.clone(), g.adapter.clone(), g.device.clone(), g.queue.clone()));
            if !self.drop_current_window() {
                self.save_pending_settings();
                self.save_empty_state();
                self.flush_notes();
                // The last window closed: its shells end (iTerm2), and the host hears it.
                self.leave_host(false);
                if self.save_due.is_some() {
                    self.save_now();
                }
                if self.prefs_before_exit() {
                    self.catalog_flush();
                    event_loop.exit();
                }
            } else {
                self.prefs_gpu = None;
            }
            return;
        }
        self.relayout();
    }

    /// What follows new output in `outputs` (the panes that got some): find bars and saved
    /// state notice it, then profile switching, downloads, requests, the composer, marks,
    /// menu checks and a redraw. The output loop and the API's Inject both come here.
    pub(super) fn after_output(&mut self, outputs: &[PaneId], now: Instant) {
        for id in outputs {
            if let Some(f) = self.finds.get_mut(id) {
                f.output_changed(now);
            }
        }
        if !outputs.is_empty() {
            self.state_debounce.mark(now);
        }
        self.run_automatic_profile_switching();
        self.take_downloads();
        self.take_requests();
        self.composer_output(outputs);
        self.check_marks();
        self.sync_checks();
        self.redraw();
    }

    fn relayout(&mut self) {
        self.mark_state_dirty();
        if self.fit_bar_change() {
            // The Resized event lays the window out at its new size.
            self.redraw();
            return;
        }
        self.sync_caption_label();
        self.layout_toolbelt();
        self.laid_out = Some(self.room());
        if let Some(t) = self.tabs.get(self.active) {
            self.mru.touch(t.id);
        }
        self.stamp_focus();
        self.refresh_route();
        let area = self.content_area();
        let (cw, ch) = self.cell();
        let _ = area;
        let sizes: Vec<(PaneId, (u16, u16))> = self
            .tabs
            .iter()
            .flat_map(|t| self.visible(t))
            .map(|(id, rect)| (id, self.grid_size(rect)))
            .collect();
        self.tmux_report_sizes();
        let mut reframe = Vec::new();
        for (id, (cols, rows)) in sizes {
            if let Some(p) = self.panes.get_mut(&id) {
                if p.resize(cols, rows) {
                    self.activity.entry(id).or_default().resized(Instant::now());
                    if p.replay.is_some() {
                        reframe.push(id);
                    }
                }
                p.set_cell_px(cw as u16, ch as u16);
            }
        }
        // A replaying pane's frame is fit to its new size again (the window is never
        // resized to the frame).
        for id in reframe {
            self.refit_frame(id);
        }
        self.redraw();
    }

    fn redraw(&self) {
        if let Some(g) = &self.gpu {
            g.window.request_redraw();
        }
        self.redraw_all();
        // The screenshot panel's buttons follow the pane's selection.
        if let Some(s) = &self.shot {
            s.window.request_redraw();
        }
    }

    fn focused_pane(&mut self) -> Option<&mut Pane> {
        let id = self.tabs.get(self.active)?.focused;
        self.panes.get_mut(&id)
    }

    fn pane_rect(&self, id: PaneId) -> Option<Rect> {
        let tab = self.tabs.get(self.active)?;
        self.visible(tab).into_iter().find(|(p, _)| *p == id).map(|(_, r)| r)
    }

    fn pane_at(&self, x: f32, y: f32) -> Option<(PaneId, Rect)> {
        let tab = self.tabs.get(self.active)?;
        self.visible(tab).into_iter().find(|(_, r)| r.contains(x, y))
    }

    /// The cell under (x, y) in pane `rect`, and which half of the cell it is in.
    fn cell_at(&self, id: PaneId, rect: Rect, x: f32, y: f32) -> (u32, u32, Side) {
        let (cw, ch) = self.cell();
        let inner = self.inner(rect);
        let term = self.panes[&id].screen.term();
        let (col, row) = mouse::cell_at((x - inner.x) as f64, (y - inner.y) as f64, cw as f64, ch as f64, term.columns() as u32, term.screen_lines() as u32);
        let side = if (x - inner.x) - col as f32 * cw > cw / 2.0 { Side::Right } else { Side::Left };
        (col, row, side)
    }

    fn grid_point(&self, id: PaneId, col: u32, row: u32) -> Point {
        let offset = self.panes[&id].screen.term().grid().display_offset();
        viewport_to_point(offset, Point::new(row as usize, Column(col as usize)))
    }

    fn copy_selection(&mut self, id: PaneId) {
        if self.copy_porthole_selection(id, porthole_ui::CopyAs::Plain) {
            return;
        }
        let Some(text) = self.panes.get(&id).and_then(|p| p.screen.term().selection_to_string()) else { return };
        self.remember_paste(&text);
        if let Some(cb) = &mut self.clipboard {
            if let Err(e) = cb.set_text(text) {
                self.banners.set("clipboard", format!("Copy failed: {e}"));
            }
        }
    }

    fn paste(&mut self) {
        let Some(text) = self.clipboard_text() else { return self.redraw() };
        if let Some(id) = self.focused_id() {
            self.user_paste(id, &text, crate::broadcast::Write::Paste);
        }
    }

    fn action(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        let n = self.tabs.len();
        match action {
            Action::NewTab => {
                if self.tmux_new(false, true).is_none() {
                    self.new_tab(None);
                }
            }
            Action::ClosePane => {
                if let Some(id) = self.tabs.get(self.active).map(|t| t.focused).filter(|_| self.layout_allowed(crate::winmodel::LayoutOp::ClosePane)).filter(|id| self.confirm_close(close_ui::Closing::Session(*id))) {
                    self.close_session_undoable(id, event_loop);
                }
            }
            Action::SplitRight => self.split(Axis::Row, None),
            Action::SplitDown => self.split(Axis::Column, None),
            Action::NextPane | Action::PrevPane => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    let panes = tab.layout.panes();
                    let i = panes.iter().position(|p| *p == tab.focused).unwrap_or(0);
                    let step = if action == Action::NextPane { 1 } else { panes.len() - 1 };
                    tab.focused = panes[(i + step) % panes.len()];
                }
            }
            Action::Focus(dir) => {
                let area = self.content_area();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if let Some(id) = tab.layout.neighbor(tab.focused, dir, area) {
                        tab.focused = id;
                    }
                }
            }
            Action::NextTab if n > 0 => self.select_tab((self.active + 1) % n),
            Action::PrevTab if n > 0 => self.select_tab((self.active + n - 1) % n),
            Action::SelectTab(i) if i < n => self.select_tab(i),
            Action::Copy => {
                if let Some(id) = self.tabs.get(self.active).map(|t| t.focused) {
                    self.copy_selection(id);
                    // Tandem's copy chord is iTerm2's Cmd-C to the frustration detector.
                    self.frustration_copy(id);
                }
            }
            Action::Paste => self.paste(),
            Action::ScrollPageUp => self.focused_pane().map_or((), |p| p.scroll(Scroll::PageUp)),
            Action::ScrollPageDown => self.focused_pane().map_or((), |p| p.scroll(Scroll::PageDown)),
            Action::ScrollTop => self.focused_pane().map_or((), |p| p.scroll(Scroll::Top)),
            Action::ScrollBottom => self.focused_pane().map_or((), |p| p.scroll(Scroll::Bottom)),
            Action::OpenSettings => self.open_settings(event_loop, Mode::Settings),
            Action::OpenProfiles => self.open_settings(event_loop, Mode::Profiles),
            _ => {}
        }
        self.relayout();
    }

    fn focused_profile(&self) -> Option<&Profile> {
        let t = self.tabs.get(self.active)?;
        self.panes.get(&t.focused).and_then(|p| self.store.get(&p.profile))
    }

    /// Runs `f` with the key routing inputs: the focused profile's map, the global map,
    /// the Leader, and the number shortcut settings.
    fn with_route<R>(&self, f: impl FnOnce(&keyroute::Context) -> R) -> R {
        let mut profile_map = self.focused_profile().and_then(|p| p.get("Keyboard Map")).and_then(|v| v.as_table().cloned()).unwrap_or_default();
        // A tmux pane: tmux's prefix bindings as Leader bindings over the profile's map.
        profile_map.extend(self.tmux_key_map());
        let global_map = self.globals.table("GlobalKeyMap");
        let ctx = keyroute::Context {
            profile_map: &profile_map,
            global_map: &global_map,
            language_agnostic: self.globals.bool("LanguageAgnosticKeyBindings"),
            leader: keystroke::parse(&self.globals.string("Leader")),
            leader_on: self.leader_on,
            switch_tab: self.globals.int("SwitchTabModifier"),
            switch_pane: self.globals.int("SwitchPaneModifier"),
            switch_window: self.globals.int("SwitchWindowModifier"),
        };
        f(&ctx)
    }

    /// Hands the keyboard hook what it needs after a focus, profile, or settings change.
    fn refresh_route(&self) {
        let mut snapshot = self.with_route(keyroute::Snapshot::new);
        snapshot.composer = self.composer_has_keys();
        *self.route_snapshot.write().unwrap() = snapshot;
        let exempt: toml::Table = self
            .globals
            .table("GlobalKeyMap")
            .into_iter()
            .filter(|(_, v)| crate::keyaction::Binding::from_value(v).is_some_and(|b| b.action == 22))
            .collect();
        *self.exempt_map.write().unwrap() = exempt;
        *self.remap_tags.write().unwrap() = crate::remap::Tags::from_globals(&self.globals);
    }

    /// Saves a custom scripts folder typed in the open Settings window and not yet applied,
    /// before Tandem ends, so nothing typed is lost.
    fn save_pending_settings(&mut self) {
        if self.settings.as_mut().is_some_and(|s| s.take_folder_pending()) {
            self.save_globals();
        }
    }

    fn save_globals(&mut self) {
        match self.globals.save() {
            Ok(()) => self.banners.clear("globals"),
            Err(e) => self.banners.set("globals", format!("Could not save settings: {e}")),
        }
        self.refresh_route();
        self.prefs_changed();
    }

    /// Mouse reporting is allowed for a pane: its Toggle Mouse Reporting choice, else its
    /// profile's "Mouse Reporting".
    fn mouse_allowed(&self, id: PaneId) -> bool {
        self.mouse_override.get(&id).copied().unwrap_or_else(|| {
            self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).is_none_or(|p| p.bool("Mouse Reporting"))
        })
    }

    /// The pane's mouse modes, or none when mouse reporting is off for it.
    fn modes(&self, id: PaneId) -> mouse::Modes {
        match self.panes.get(&id) {
            Some(p) if self.mouse_allowed(id) => p.screen.mouse_modes(),
            _ => mouse::Modes::default(),
        }
    }

    fn pointer_prefs(&self) -> pointer::Prefs {
        pointer::Prefs { pass_on_control_click: self.globals.bool("PassOnControlClick"), report_right_click: self.globals.bool("ReportRightClick") }
    }

    /// The multi-click count for a press of `button` here: 1 to 4.
    fn os_clicks(&mut self, button: u8) -> u8 {
        let now = Instant::now();
        let (x, y) = self.cursor;
        let near = 4.0 * self.scale();
        let count = match self.pointer_last {
            Some((t, b, (px, py), n)) if b == button && now.duration_since(t) <= input::MULTI_CLICK && (x - px).abs() <= near && (y - py).abs() <= near => (n + 1).min(4),
            _ => 1,
        };
        self.pointer_last = Some((now, button, (x, y), count));
        count
    }

    /// What a key sends to the terminal, with the focused profile's Keys options.
    fn key_bytes(&self, key: &Key, base: Option<&Key>, text: Option<&str>, keypad: bool) -> Option<Vec<u8>> {
        let t = self.tabs.get(self.active)?;
        let pane = self.panes.get(&t.focused)?;
        let mode = *pane.own().term().mode();
        let opts = self.store.get(&pane.profile).map_or(KeyOptions { left_option: 2, right_option: 2, option_is_alt: true, app_keypad_allowed: false }, KeyOptions::from_profile);
        keys::press_bytes_with(key, base, text, self.mods, self.alt_side, mode.contains(TermMode::APP_CURSOR), mode.contains(TermMode::APP_KEYPAD), keypad, &opts)
    }

    fn key(&mut self, ev: &KeyEvent, event_loop: &ActiveEventLoop) {
        // Instant Replay takes its keys before anything else (`PS:12795-12802`).
        if self.replay_key(ev, event_loop) {
            return;
        }
        // A tool with the keyboard takes its keys, releases too, before any reach the session.
        if self.toolbelt_key(ev, event_loop) {
            return;
        }
        // A composer with the keyboard takes its keys, releases too (Review Focus 5).
        if self.composer_key(ev) {
            return;
        }
        // The API's key up and flags changed notifications.
        self.api_key_early(ev);
        if self.protocol_key_event(ev) || ev.state != ElementState::Pressed {
            return;
        }
        if self.bar_key(ev, event_loop) || self.upload_key(ev) {
            return;
        }
        // Typing shows the cursor at once and restarts the blink.
        self.blink_on = true;
        self.last_blink = Instant::now();
        self.banners.clear("action");
        if self.picker_key(ev) || self.inline_key(ev) || self.clip_key(ev) || self.shortcut_key(ev) || self.find_key(ev) || self.copy_mode_key(ev) || self.completion_key(ev) {
            return;
        }
        let base = keystroke::key_of(&ev.key_without_modifiers(), None);
        let Some(key) = keystroke::key_of(&ev.logical_key, ev.text.as_deref()) else { return };
        let Some(stroke) = keystroke::from_key_event(ev, self.mods) else { return };
        let keypad = ev.location == KeyLocation::Numpad;
        let code = match ev.physical_key {
            PhysicalKey::Code(c) => Some(c),
            _ => None,
        };
        let digit = if self.globals.bool("UseVirtualKeyCodesForDetectingDigits") {
            code.and_then(us_digit)
        } else {
            match &key {
                Key::Text(s) => s.chars().next().and_then(|c| c.to_digit(10)).map(|d| d as u8),
                _ => None,
            }
        };
        let fallback = self.protocol_bytes(ev, &key, base.as_ref()).or_else(|| self.key_bytes(&key, base.as_ref(), ev.text.as_deref(), keypad));
        // The API's key down notification; a key a script filters goes no further.
        if self.api_key_down(&stroke) {
            return;
        }
        self.typing = Some(ev.physical_key);
        self.dispatch(keyroute::Press { stroke, digit }, fallback, Some((key, base)), None, event_loop);
        self.typing = None;
    }

    /// Runs a key press: the Leader, number shortcuts, key mappings, then the terminal
    /// (or, for a Win chord from the hook, its menu command).
    pub(super) fn dispatch(
        &mut self,
        press: keyroute::Press,
        fallback: Option<Vec<u8>>,
        typed: Option<(Key, Option<Key>)>,
        chord: Option<&crate::menus::Chord>,
        event_loop: &ActiveEventLoop,
    ) {
        let outer = std::mem::replace(&mut self.key_down, true);
        self.route_press(press, fallback, typed, chord, event_loop);
        self.key_down = outer;
    }

    fn route_press(
        &mut self,
        press: keyroute::Press,
        fallback: Option<Vec<u8>>,
        typed: Option<(Key, Option<Key>)>,
        chord: Option<&crate::menus::Chord>,
        event_loop: &ActiveEventLoop,
    ) {
        let was_leader = self.leader_on;
        let route = self.with_route(|c| keyroute::route(c, &press));
        self.leader_on = false;
        match route {
            Route::LeaderOn => self.leader_on = true,
            // One window until milestone 7: window 1 is this one.
            Route::SelectWindow(d) => self.select_window_number(d),
            Route::SelectPane(d) => {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    let panes = t.layout.panes();
                    let i = if d == 9 { panes.len() - 1 } else { d as usize - 1 };
                    if let Some(id) = panes.get(i) {
                        t.focused = *id;
                        t.zoomed = None;
                    }
                }
            }
            Route::SelectTab(d) => {
                let i = if d == 9 { self.tabs.len().saturating_sub(1) } else { d as usize - 1 };
                if i < self.tabs.len() {
                    self.select_tab(i);
                }
            }
            Route::Binding(b) => self.run_key_action(&b, fallback, chord, event_loop),
            Route::Terminal => {
                if let Some(c) = chord {
                    if let Some(item) = crate::commands::command_for_chord(c) {
                        self.run_command(item, event_loop);
                    }
                } else if let Some((key, base)) = typed {
                    self.to_terminal(&key, base.as_ref(), fallback, event_loop);
                    if was_leader {
                        self.refresh_route();
                    }
                    return;
                }
            }
        }
        self.relayout();
    }

    /// A key with no binding: the Windows extras, movement-key scrolling, else the pane.
    fn to_terminal(&mut self, key: &Key, base: Option<&Key>, bytes: Option<Vec<u8>>, event_loop: &ActiveEventLoop) {
        if let Some(action) = base.and_then(|b| keys::shortcut(b, self.mods)) {
            return self.action(action, event_loop);
        }
        // A pane showing a frozen screenshot copy takes no keys until its panel closes, nor
        // a replaying pane (the replay view has no shell).
        if self.shot.as_ref().is_some_and(|s| Some(s.pane) == self.focused_id()) || self.focused_id().and_then(|id| self.panes.get(&id)).is_some_and(|p| p.replay.is_some()) {
            return;
        }
        // Filter and Zoom views are read-only: a key ends the view (Esc only that).
        if let Some(id) = self.focused_id().filter(|id| self.panes.get(id).is_some_and(|p| p.live.is_some())) {
            self.end_view(id);
            if matches!(key, Key::Escape) {
                return;
            }
        }
        let mods = self.mods;
        let movement = self.focused_profile().is_some_and(|p| p.bool("Movement Keys Scroll Outside Interactive Apps"));
        let focused = self.focused_id();
        let Some(pane) = self.focused_pane() else { return };
        if movement {
            let alt_screen = pane.screen.term().mode().contains(TermMode::ALT_SCREEN);
            if let Some(scroll) = keys::movement_scroll(key, mods, alt_screen, pane.exited.is_some()) {
                pane.scroll(scroll);
                return self.redraw();
            }
        }
        if let Some(bytes) = bytes {
            pane.scroll(Scroll::Bottom);
            let sender = pane.id;
            if self.tmux_key_by_name(sender, key) {
                return;
            }
            self.user_write(sender, &bytes, crate::broadcast::Write::Key);
            if let (Some(k), Some(id)) = (self.typing.take(), focused) {
                self.sent_keys.press(k, id);
            }
        }
    }

    fn mouse_press(&mut self, button: MouseButton, event_loop: &ActiveEventLoop) {
        self.hide_tab_tip();
        // A press starts over on the tab bar too: a bar press whose release never came (focus
        // went mid-press) must not take this press's release; a press on the bar stores itself.
        self.tab_press = None;
        // A press starts over: a tmux drag whose release never came lets tmux go on.
        self.tmux_drag_cancel();
        let (x, y) = self.cursor;
        // A press anywhere but the toolbelt gives the keyboard back to the session (the tab
        // bar and the status bar too).
        if !self.toolbelt_panel().is_some_and(|p| p.contains(x, y)) {
            self.belt.focus = None;
        }
        // A press on a shown composer is its own; anywhere else takes the keyboard from it.
        if self.composer_press(button, x, y) {
            return;
        }
        // The caption's menu button pops the whole menu (the native bar cannot sit under the strip).
        if button == MouseButton::Left && self.caption_layout().and_then(|l| l.menu).is_some_and(|m| m.contains(x, y)) {
            return self.show_caption_menu(false);
        }
        // The side bar's drag handle, on its inner edge, before its rows (`RTV:1860-1895`).
        if button == MouseButton::Left && self.side_handle().is_some_and(|h| h.contains(x, y)) {
            self.drag = Drag::SideBar { start_x: x, start_width: self.side_bar_width() / self.scale() };
            return;
        }
        if let Some(hit) = self.tab_bar_hit(x, y) {
            return self.tab_bar_press(button, hit);
        }
        if self.status_press(button, x, y, event_loop) {
            return;
        }
        if self.toolbelt_press(button, x, y, event_loop) {
            return;
        }
        // The session picker's button on a pane (Task 5): a pick waits until one is pressed.
        if button == MouseButton::Left && self.picker_press(x, y) {
            return;
        }
        // A pane's AI indicator (linked or streaming) opens its menu.
        if button == MouseButton::Left && self.chat_indicator_press(x, y) {
            return;
        }
        if button == MouseButton::Left && self.clip_press(x, y, event_loop) {
            return;
        }
        if self.inline_press(button, x, y, event_loop) {
            return;
        }
        // Other buttons in a panel do nothing (the terminal is not under it).
        if self.clip_split_hit(x, y) || self.inline_split_hit(x, y) {
            return;
        }
        if button == MouseButton::Left && (self.info_bar_press(x, y, event_loop) || self.porthole_press(x, y) || self.block_press(x, y)) {
            return;
        }
        if button == MouseButton::Left {
            let area = self.content_area();
            let grab = 3.0 * self.scale();
            if let Some(d) = self.tabs.get(self.active).and_then(|t| {
                t.layout.dividers(area, GAP).into_iter().find(|d| {
                    Rect { x: d.rect.x - grab, y: d.rect.y - grab, w: d.rect.w + 2.0 * grab, h: d.rect.h + 2.0 * grab }.contains(x, y)
                })
            }) {
                self.tmux_drag_start(&d.path, d.axis);
                self.drag = Drag::Divider { path: d.path, axis: d.axis, parent: d.parent };
                return;
            }
        }
        if button == MouseButton::Left && self.find_press(x, y) {
            return;
        }
        let Some((id, rect)) = self.pane_at(x, y) else { return };
        let b = button_number(button);
        // A left click always focuses its pane; right and middle only when the setting says so.
        if b == 0 || self.globals.bool("FocusOnRightOrMiddleClick") {
            self.tabs[self.active].focused = id;
            self.refresh_route();
        }
        // The press makes the terminal first responder (`notifyActive`): the focused pane's
        // Auto Composer takes the keyboard back; the press still selects in the output.
        if self.focused_id() == Some(id) {
            self.composer_take_back(true);
        }
        let (col, row, side) = self.cell_at(id, rect, x, y);
        let mods = self.mods;
        let modes = self.modes(id);
        let mb = match b {
            0 => Some(Button::Left),
            1 => Some(Button::Right),
            2 => Some(Button::Middle),
            _ => None,
        };
        let reportable = mb.is_some() && mouse::app_owns_mouse(modes, mods);
        let app_tracking = modes.tracking != Tracking::Off;
        let os = self.os_clicks(b);
        let clicks = self.pointer_clicks.press(b, os);
        let map = self.globals.table("PointerActions");
        let down = pointer::on_down(b, clicks, mods, reportable, app_tracking, &map, &self.pointer_prefs());
        if down == Down::Bound {
            self.pointer_down = Some(PointerDown { button: b, clicks, pane: id, cell: (col, row) });
            return self.redraw();
        }
        // The frustration detector watches left presses; other buttons end its episode.
        if b == 0 {
            self.frustration(id, |d| d.mouse_down(clicks, reportable));
        } else {
            self.frustration(id, |d| d.other_mouse());
        }
        // Win-click opens the file or URL under the pointer (Semantic History). Windows
        // would open the Start menu when Win comes up, so that is masked.
        if b == 0 && mods.cmd && !mods.ctrl && self.globals.bool("CommandSelection") {
            #[cfg(windows)]
            crate::winkeys::mask_start_menu();
            let point = self.grid_point(id, col, row);
            if self.open_target(id, point, false) {
                return self.redraw();
            }
        }
        if let (true, Some(mb)) = (reportable, mb) {
            let ev = mouse::Event { button: mb, action: mouse::Action::Press, mods, col, row };
            if let Some(bytes) = mouse::encode(modes, ev) {
                self.panes.get_mut(&id).unwrap().input(&bytes);
            }
            self.drag = Drag::App { pane: id, button: mb, clicks, at: Instant::now() };
            return self.redraw();
        }
        match b {
            0 => {
                let count = self.clicks.press(Instant::now(), col, row);
                let point = self.grid_point(id, col, row);
                let plain = !mods.shift && !mods.alt && !mods.ctrl && !mods.cmd;
                if count == 1 && plain && (self.click_unfold(id, point) || self.click_note(id, point, event_loop)) {
                    return;
                }
                let pick = input::click_pick(count, mods, self.globals.bool("DoubleClickPerformsSmartSelection"));
                let smart = pick == input::Pick::Smart && self.smart_select(id, point, false).is_some();
                if !smart {
                    let ty = match pick {
                        input::Pick::Type(t) => t,
                        input::Pick::Smart => input::SelectionType::Semantic,
                    };
                    self.panes.get_mut(&id).unwrap().screen.term_mut().selection = Some(Selection::new(ty, point, side));
                }
                if smart || pick == input::Pick::Type(input::SelectionType::Semantic) {
                    self.clip_selection_to_dividers(id, point);
                }
                if smart {
                    // iTerm2 copies a selection as soon as it is made.
                    self.copy_selection(id);
                }
                // A terminal selection ends any porthole's.
                self.clear_porthole_selections();
                // A Smart Selection is whole; moving the pointer does not reshape it.
                self.drag = if smart { Drag::None } else { Drag::Select { pane: id } };
                let alt_only = mods.alt && !mods.shift && !mods.ctrl && !mods.cmd;
                self.alt_click = (count == 1 && alt_only).then_some((id, (col, row)));
                self.key_click = (count == 1 && !mods.shift).then_some(id);
            }
            // With no binding and no reporting app, a right click opens the menu (iTerm2's native behavior).
            1 => {
                let point = self.grid_point(id, col, row);
                self.show_context_menu_at(Some((id, point)));
            }
            _ => {}
        }
        self.redraw();
    }

    /// Alt-click moves the cursor (Pointer > Alt-click moves cursor), unless the app
    /// takes clicks or the view is scrolled back.
    fn alt_click_moves_cursor(&mut self, pane: PaneId, target: (u32, u32)) {
        if !self.globals.bool("OptionClickMovesCursor") || self.modes(pane).tracking != Tracking::Off {
            return;
        }
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let term = p.screen.term();
        if term.grid().display_offset() != 0 {
            return;
        }
        let c = term.grid().cursor.point;
        let mode = *term.mode();
        let bytes = actions::option_click_arrows((c.column.0 as u32, c.line.0.max(0) as u32), target, term.columns() as u32, mode.contains(TermMode::ALT_SCREEN), mode.contains(TermMode::APP_CURSOR));
        self.user_write(pane, &bytes, crate::broadcast::Write::AltClickArrows);
    }

    fn mouse_release(&mut self, button: MouseButton, event_loop: &ActiveEventLoop) {
        if self.tab_bar_release(button, event_loop) {
            return;
        }
        let b = button_number(button);
        if let Some(pd) = self.pointer_down.take() {
            if pd.button == b {
                let modes = self.modes(pd.pane);
                let reportable = b <= 2 && mouse::app_owns_mouse(modes, self.mods);
                let map = self.globals.table("PointerActions");
                if let Up::Perform(binding) = pointer::on_up(b, pd.clicks, self.mods, reportable, modes.tracking != Tracking::Off, &map, &self.pointer_prefs()) {
                    self.run_pointer_action(&binding, pd.pane, pd.cell, event_loop);
                }
                self.relayout();
                return;
            }
            self.pointer_down = Some(pd);
        }
        let alt_click = self.alt_click.take();
        let key_click = self.key_click.take();
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Porthole { pane, id, .. } => self.porthole_release(pane, id),
            Drag::Clip { pane, kind } => self.clip_release(pane, kind),
            Drag::Inline { .. } => self.inline_release(),
            Drag::Belt(d) => self.toolbelt_release(d),
            Drag::Composer { pane, kind } => self.composer_release(pane, kind),
            Drag::Divider { .. } => self.tmux_drag_end(event_loop),
            // The width becomes every new window's (iTerm2 writes it at each move, `RTV:2264-2277`).
            Drag::SideBar { .. } => {
                self.globals.set("LeftTabBarWidth", toml::Value::Float(self.side_bar_preferred as f64));
                self.save_globals();
                self.update_side_handle_hover();
            }
            // Its press went (the tab moved away under it): the drag ends with nothing dropped.
            Drag::Tab { .. } | Drag::Chip { .. } | Drag::AddButton { .. } | Drag::Window { .. } => self.end_tab_drag(),
            Drag::Select { pane } => {
                let empty = self.panes.get(&pane).and_then(|p| p.screen.term().selection_to_string()).map_or(true, |s| s.is_empty());
                if empty {
                    if let Some(p) = self.panes.get_mut(&pane) {
                        p.screen.term_mut().selection = None;
                    }
                    if let Some((p, cell)) = alt_click.filter(|(p, _)| *p == pane) {
                        self.alt_click_moves_cursor(p, cell);
                    }
                    // A click (no drag) that comes up on the key cursor opens the password
                    // manager.
                    if key_click == Some(pane) && self.released_on_key_cursor(pane) {
                        self.open_password_manager(None, Some(pane));
                    }
                } else {
                    // iTerm2 copies a selection as soon as it is made.
                    self.copy_selection(pane);
                    self.chat_offer_selection(pane);
                }
            }
            Drag::App { pane, button, clicks, at } => {
                // macOS counts a release as no click once the multi-click time has passed.
                let reported = mouse::app_owns_mouse(self.modes(pane), self.mods);
                if button == Button::Left {
                    let clicks = if at.elapsed() > input::MULTI_CLICK { 0 } else { clicks };
                    self.frustration(pane, |d| d.mouse_up(clicks, reported));
                } else {
                    self.frustration(pane, |d| d.other_mouse());
                }
                let (x, y) = self.cursor;
                // Look the pane up by id: the button may come up outside it, and the app
                // must still get the release (cell_at clamps to the pane's edge).
                if let Some(rect) = self.pane_rect(pane) {
                    let (col, row, _) = self.cell_at(pane, rect, x, y);
                    let modes = self.modes(pane);
                    let p = self.panes.get_mut(&pane).unwrap();
                    let ev = mouse::Event { button, action: mouse::Action::Release, mods: self.mods, col, row };
                    if let Some(bytes) = mouse::encode(modes, ev) {
                        p.input(&bytes);
                    }
                }
            }
            _ => {}
        }
        self.redraw();
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        self.update_tab_hover();
        self.update_side_handle_hover();
        self.toolbelt_move(x, y);
        self.composer_move(x, y);
        self.clip_move(x, y);
        self.inline_move(x, y);
        self.update_hover();
        self.update_block_hover();
        // Hover timestamps show while the pointer is at a pane's right edge.
        let near = self.near_stamp_edge();
        if near != self.stamp_near {
            self.stamp_near = near;
            self.redraw();
        }
        match &self.drag {
            Drag::Porthole { pane, id, frame } => {
                let (pane, id, frame) = (*pane, *id, *frame);
                self.porthole_drag(pane, id, frame, x, y);
            }
            Drag::Clip { .. } | Drag::Inline { .. } | Drag::Belt(_) | Drag::Composer { .. } => {}
            Drag::SideBar { start_x, start_width } => {
                let (start_x, start_width) = (*start_x, *start_width);
                self.side_bar_drag(start_x, start_width, x);
            }
            Drag::Tab { .. } | Drag::Chip { .. } | Drag::Window { .. } => self.tab_drag_move(x, y),
            Drag::AddButton { start } => {
                let start = *start;
                self.add_button_drag(start, x, y);
            }
            Drag::Select { pane } => {
                let pane = *pane;
                let Some(rect) = self.pane_rect(pane) else { return };
                let (col, row, side) = self.cell_at(pane, rect, x, y);
                let point = self.grid_point(pane, col, row);
                if let Some(sel) = self.panes.get_mut(&pane).and_then(|p| p.screen.term_mut().selection.as_mut()) {
                    sel.update(point, side);
                }
                self.redraw();
            }
            Drag::Divider { path, axis, parent } => {
                let ratio = match axis {
                    Axis::Row => (x - parent.x) / parent.w,
                    Axis::Column => (y - parent.y) / parent.h,
                };
                let path = path.clone();
                self.tabs[self.active].layout.set_ratio(&path, ratio);
                self.relayout();
            }
            Drag::App { pane, button, .. } => {
                let (pane, button) = (*pane, *button);
                // A drag counts as reported where the program asked for any clicks (iTerm2's
                // test for drag reports), even when only motion with a button is sent.
                let reported = mouse::app_owns_mouse(self.modes(pane), self.mods);
                if button == Button::Left {
                    self.frustration(pane, |d| d.mouse_dragged(reported));
                } else {
                    self.frustration(pane, |d| d.other_mouse());
                }
                self.send_motion(pane, Some(button), x, y);
            }
            Drag::None => {
                if let Some((id, _)) = self.pane_at(x, y) {
                    if self.globals.bool("FocusFollowsMouse") && self.tabs.get(self.active).is_some_and(|t| t.focused != id) {
                        self.tabs[self.active].focused = id;
                        self.refresh_route();
                        self.redraw();
                    }
                    if self.modes(id).tracking == Tracking::Any {
                        self.send_motion(id, None, x, y);
                    }
                }
            }
        }
    }

    fn send_motion(&mut self, pane: PaneId, button: Option<Button>, x: f32, y: f32) {
        let Some(rect) = self.pane_rect(pane) else { return };
        let (col, row, _) = self.cell_at(pane, rect, x, y);
        let modes = self.modes(pane);
        let p = self.panes.get_mut(&pane).unwrap();
        let ev = mouse::Event { button: button.unwrap_or(Button::None), action: mouse::Action::Motion, mods: self.mods, col, row };
        if let Some(bytes) = mouse::encode(modes, ev) {
            p.input(&bytes);
        }
    }

    fn wheel(&mut self, delta: MouseScrollDelta) {
        let (cw, ch) = self.cell();
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x * WHEEL_LINES, y * WHEEL_LINES),
            MouseScrollDelta::PixelDelta(p) => (p.x as f32 / cw, p.y as f32 / ch),
        };
        if self.tab_bar_wheel(delta) || self.composer_wheel(dy) || self.toolbelt_wheel(dy) || self.inline_wheel(dy) || self.clip_wheel(dy) || self.porthole_wheel(dx, dy) {
            return;
        }
        self.wheel_acc += dy;
        self.wheel_acc_x += dx;
        self.wheel_horizontal();
        let lines = self.wheel_acc.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.wheel_acc -= lines as f32;
        let (x, y) = self.cursor;
        let Some((id, rect)) = self.pane_at(x, y) else { return };
        let (col, row, _) = self.cell_at(id, rect, x, y);
        let mods = self.mods;
        let modes = self.modes(id);
        let p = self.panes.get_mut(&id).unwrap();
        let mode = *p.screen.term().mode();
        match input::wheel(modes, mods, mode.contains(TermMode::ALT_SCREEN), mode.contains(TermMode::APP_CURSOR), lines, col, row) {
            Wheel::Send(bytes) => p.input(&bytes),
            Wheel::Scroll(n) => p.scroll(Scroll::Delta(n)),
        }
        self.redraw();
    }

    /// Horizontal wheel movement goes to a mouse-reporting app as buttons 66 and 67 when
    /// "Mouse reporting supports horizontal scrolling" is on. Otherwise it does nothing.
    fn wheel_horizontal(&mut self) {
        let steps = self.wheel_acc_x.trunc() as i32;
        if steps == 0 {
            return;
        }
        self.wheel_acc_x -= steps as f32;
        if !self.globals.bool("ReportHorizontalScrollEvents") {
            return;
        }
        let (x, y) = self.cursor;
        let Some((id, rect)) = self.pane_at(x, y) else { return };
        let (col, row, _) = self.cell_at(id, rect, x, y);
        let modes = self.modes(id);
        if !mouse::app_owns_mouse(modes, self.mods) {
            return;
        }
        // Positive x moves the content right: the view scrolls left.
        let button = if steps > 0 { Button::WheelLeft } else { Button::WheelRight };
        let ev = mouse::Event { button, action: mouse::Action::Press, mods: self.mods, col, row };
        if let Some(one) = mouse::encode(modes, ev) {
            self.panes.get_mut(&id).unwrap().input(&one.repeat(steps.unsigned_abs() as usize));
        }
    }

    fn build_frame(&mut self) -> Frame {
        let mut attention: Option<Instant> = None;
        let cell = self.cell();
        let palette_bg = self.panes.values().next().map_or(crate::palette::Palette::default().bg, |p| p.screen.palette.bg);
        let mut frame = Frame { clear: rgba(palette_bg, 1.0), ..Default::default() };
        self.note_tab_changes();
        // A Minimal or Compact window's caption strip is under the tab bar and the sessions.
        self.sync_caption_label();
        self.draw_caption_bar(&mut frame);
        // A flashing bar is drawn apart, then laid over the sessions at its alpha.
        let flash_over = self.tab_flash.flashing() && !self.tab_bar_takes_room();
        let mut bar_layer = Frame::default();
        let bar_frame = if flash_over { &mut bar_layer } else { &mut frame };
        // A tab closed or moved under a held press: the press goes before the looks are made,
        // so no other tab draws its close button held for a frame.
        if self.tab_press_moved() {
            self.tab_press = None;
        }
        if let Some(area) = self.tab_bar_rect() {
            let mut looks: Vec<scene::TabLook> = self.tabs.iter().map(|t| self.tab_look(t)).collect();
            if let Some(i) = held_close(self.tab_press) {
                if let Some(l) = looks.get_mut(i) {
                    l.close_pressed = true;
                }
            }
            // Kept unscrolled for hits; the scroll offset applies at each use.
            self.tab_bar = self.layout_tab_bar(&looks, area, 0.0);
            let vertical = self.tab_bar.vertical;
            if vertical {
                self.tab_scroll = 0.0;
            }
            self.follow_active_tab();
            let offset = self.bar_offset();
            let scrolled = (offset > 0.0).then(|| self.layout_tab_bar(&looks, area, offset));
            let scale = self.scale();
            let font_px = self.gpu.as_ref().map_or(cell.1, |g| g.renderer.font_px()).max(1.0);
            // A dragged tab follows the pointer along the bar, drawn over the others.
            let dragged = self.dragged_bar(scrolled.as_ref().unwrap_or(&self.tab_bar), &mut looks, area);
            let drawn = dragged.as_ref().or(scrolled.as_ref()).unwrap_or(&self.tab_bar);
            // A dragged tab poised to join a collapsed group of this window frames it.
            let join = self.tab_join.as_ref().filter(|(w, _)| Some(*w) == self.current_window()).map(|(_, g)| g.clone());
            let joining = join.map(|join| tab_layout::Bar { join: Some(join), ..drawn.clone() });
            let drawn = joining.as_ref().unwrap_or(drawn);
            let (key, overflow_down) = (self.focused && self.app_active, matches!(self.tab_press, Some((tab_layout::Hit::Overflow, _, _, _))));
            let pal = self.tab_palette();
            let positioned = tab_layout::Bar { position: self.tab_position(), ..drawn.clone() };
            let drawn = &positioned;
            if vertical {
                let (_, chips) = scene::tab_bar_vertical(bar_frame, &looks, self.active, drawn, area, scale, scale / font_px, cell.1, key, overflow_down, &pal);
                self.side_chips = chips;
            } else {
                scene::tab_bar(bar_frame, &looks, self.active, drawn, area, scale, scale / font_px, cell.1, key, overflow_down, &pal);
                self.side_chips.clear();
            }
        } else {
            self.tab_bar = tab_layout::Bar::default();
            self.tab_scroll = 0.0;
            self.side_chips.clear();
            self.tab_hover = None;
            self.tab_tip = None;
        }
        self.tab_bar_drawn = self.tab_bar_rect().map(|bar| (bar, self.bar_tab_rects(), self.bar_pills(None)));
        self.draw_drop_marker(bar_frame);
        self.draw_caption_content(&mut frame);
        let area = self.content_area();
        let backgrounds = self.pane_backgrounds();
        let mut block_buttons = Vec::new();
        let mut picker_hits = Vec::new();
        let mut indicator_hits = Vec::new();
        let mut inline_hits = Vec::new();
        self.prepare_portholes();
        self.prepare_badges();
        self.prepare_inline_chats();
        let mut porthole_hits = Vec::new();
        let bview = self.broadcast_view();
        let stripes = self.globals.bool("Use background pattern indicator").then(|| self.stripe_tile());
        if let Some(tab) = self.tabs.get(self.active) {
            for (id, rect) in self.visible(tab) {
                let receives = self.broadcast.receives(&bview, id);
                let sends = self.broadcast.sends(&bview, id);
                if let Some(b) = backgrounds.iter().find(|b| b.pane == id) {
                    scene::background(&mut frame, b.rect, &b.pieces, &b.image, b.bg, b.blend);
                }
                self.draw_badge(&mut frame, id, rect);
                if let Some(p) = self.panes.get(&id) {
                    let shortcuts = self.shortcuts.as_ref().filter(|s| s.pane == id);
                    let labeled: Vec<crate::find::Match> = shortcuts.map(|s| s.items.iter().map(|(_, m, _)| *m).collect()).unwrap_or_default();
                    let labels: Vec<(crate::find::Pos, String)> = shortcuts.map(|s| s.items.iter().map(|(l, m, _)| (m.start, l.clone())).collect()).unwrap_or_default();
                    // Find highlights show only while the find bar is open.
                    let matches = if labeled.is_empty() { self.finds.get(&id).filter(|f| f.open).map(|f| f.results.matches.as_slice()).unwrap_or_default() } else { labeled.as_slice() };
                    let hover = self.hover.filter(|(h, _)| *h == id).map(|(_, m)| m);
                    let notes: Vec<(crate::find::Match, Option<String>)> = self.note_cells(id).into_iter().map(|(m, _, t)| (m, t)).collect();
                    let hl = scene::Highlights { matches, color: p.screen.palette.match_bg, hover, link: p.screen.palette.link, labels: &labels, notes: &notes, undimmed: self.pane_undimmed(id, tab.focused, receives, &bview), key_cursor: self.focused && id == tab.focused && p.at_password_prompt() };
                    // An Auto Composer hides the terminal cursor and the rows it covers.
                    let cover = self.auto_composer_cover(id, rect);
                    let overlay_from = frame.overlay.len();
                    scene::pane_with(&mut frame, &p.screen, self.inner(rect), cell, self.focused && id == tab.focused, self.blink_on && cover.is_none(), Some(&hl));
                    if let Some(c) = cover {
                        composer_ui::skip_covered(&mut frame.overlay, overlay_from, c);
                    }
                    // Broadcast stripes: over the cell backgrounds, under the text.
                    if let (Some((n, px)), true) = (&stripes, receives) {
                        let r = self.inner(rect);
                        frame.images.push(scene::ImageQuad { rect: r, uv: [0.0, 0.0, r.w / *n as f32, r.h / *n as f32], key: (0x5354_5249, *n as usize), pixels: px.clone(), size: (*n, *n), repeat: true });
                    }
                    porthole_hits.extend(self.draw_portholes(&mut frame, id, rect, cell));
                }
                self.draw_view_extras(&mut frame, id, self.inner(rect), cell);
                if let Some((row, col, selecting)) = self.copy_cursor_on_screen(id) {
                    scene::copy_cursor(&mut frame, self.inner(rect), cell, row, col, selecting);
                }
                if self.shot.as_ref().is_some_and(|s| s.pane == id) {
                    scene::camera_sign(&mut frame, self.inner(rect), cell);
                }
                if self.panes.get(&id).is_some_and(|p| p.buffer.on) {
                    scene::buffering_sign(&mut frame, self.inner(rect), cell);
                }
                indicator_hits.extend(self.draw_coprocess_indicator(&mut frame, id, self.inner(rect), cell, (sends, receives)));
                picker_hits.extend(self.draw_session_picker(&mut frame, id, self.inner(rect), cell));
                self.draw_info_bar(&mut frame, id, rect, cell);
                if let Some(t) = self.draw_attention(&mut frame, id, self.inner(rect), cell) {
                    attention = Some(attention.map_or(t, |a: Instant| a.min(t)));
                }
                let show = self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).is_none_or(|p| p.bool("Show Mark Indicators"));
                if let Some(p) = self.panes.get(&id) {
                    // Block bars take a row's gutter over mark bars.
                    let block_rows = self.block_view_rows(id);
                    let bars: Vec<(usize, crate::marks::MarkColor)> = p
                        .screen
                        .visible_marks()
                        .into_iter()
                        .filter(|(row, _)| !block_rows.contains(row))
                        .filter(|(_, m)| show || m.name.is_some())
                        .map(|(row, m)| (row, crate::marks::color(&m)))
                        .collect();
                    let pad = PAD * self.scale();
                    scene::mark_bars(&mut frame, &bars, self.inner(rect), cell.1, pad);
                    block_buttons.extend(self.draw_blocks(&mut frame, id, rect, cell));
                    self.draw_inline_buttons(&mut frame, id, rect, cell);
                }
                // The composer over the pane, then the completion list last, so everything under
                // them is cut out.
                self.draw_composer(&mut frame, id, rect, cell);
                self.draw_completion(&mut frame, id, self.inner(rect), cell);
            }
            self.draw_clippings(&mut frame, cell);
            inline_hits = self.draw_inline_chats(&mut frame, cell);
            if tab.zoomed.is_none() {
                scene::dividers(&mut frame, &tab.layout.dividers(area, GAP), self.divider_color());
            }
            self.find_bar_rect = None;
            self.find_mode_rect = None;
            // A pane whose status bar shows its find (a Search or Filter Tool) has no find bar.
            let bar_pane = self.visible(tab).into_iter().find(|(id, _)| *id == tab.focused).filter(|_| self.bar_find_host(tab.focused).is_none());
            if let (Some((_, rect)), Some(f)) = (bar_pane, self.finds.get(&tab.focused).filter(|f| f.open)) {
                let count = if f.filter { "Filter".to_string() } else { f.results.label() };
                let mode = self.find_mode();
                let bar = scene::FindBar { query: &f.query, count: &count, mode: mode.label(), error: f.error.is_some() };
                let mode_rect = scene::find_bar(&mut frame, &bar, rect, cell);
                self.find_bar_rect = frame.quads.last().map(|q| q.rect);
                self.find_mode_rect = Some(mode_rect);
            }
        }
        if let Some(r) = self.tab_bar_rect().filter(|_| flash_over) {
            scene::lay_over(&mut frame, bar_layer, r, self.tab_flash.alpha(Instant::now()), palette_bg, cell.1);
        }
        self.draw_toolbelt(&mut frame, cell);
        self.draw_status_bars(&mut frame, cell);
        let (ww, wh) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
        let line = self.division_height();
        if line > 0.0 {
            scene::division_line(&mut frame, ww, line, crate::chrome::division_color(self.resolved_style(), self.focused));
        }
        self.draw_find_cursor(&mut frame, Rect { x: 0.0, y: 0.0, w: ww, h: wh }, cell);
        if let Some((text, _)) = self.toast.as_ref().filter(|(_, until)| Instant::now() < *until) {
            scene::toast(&mut frame, text, Rect { x: 0.0, y: 0.0, w: ww, h: wh }, cell);
        }
        // The tab tooltip over everything but the banner, under the pointer (a pointer's
        // height below it).
        if let Some(text) = self.tab_tip_shown().and_then(|i| self.tab_tip_text(i)) {
            let (x, y) = self.cursor;
            scene::tab_tooltip(&mut frame, &text, (x, y + TIP_POINTER_PT * self.scale()), Rect { x: 0.0, y: 0.0, w: ww, h: wh }, cell);
        }
        self.attention_due = attention;
        self.block_buttons = block_buttons;
        self.porthole_hits = porthole_hits;
        self.picker_hits = picker_hits;
        self.inline_hits = inline_hits;
        self.chat_indicator_hits = indicator_hits;
        let error = self.banners.text().or_else(|| self.panes.values().find_map(|p| p.error.clone()));
        if let Some(text) = error {
            let (w, h) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
            scene::banner(&mut frame, &text, Rect { x: 0.0, y: 0.0, w, h }, cell);
        }
        frame
    }

    fn render(&mut self) {
        // A status bar field whose bar went away gives the keyboard back before drawing.
        self.check_composer_fields();
        self.sync_menu_hidden();
        // The next edge drag snaps to what this frame shows.
        self.publish_snap();
        let frame = self.build_frame();
        // The caption's hit tests answer from what this frame drew.
        self.publish_caption();
        let (title, menu_text) = self.caption_text();
        if self.window_menu_text != menu_text {
            self.window_menu_text = menu_text;
            self.refresh_window_list();
        }
        // iTerm2 shows the Leader as sparkles on the cursor; Tandem says so in the title.
        let title = if self.leader_on { format!("[Leader] {title}") } else { title };
        let Some(gpu) = self.gpu.as_mut() else { return };
        gpu.window.set_title(&title);
        let texture = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                gpu.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                match gpu.instance.create_surface(gpu.window.clone()) {
                    Ok(s) => gpu.surface = s,
                    Err(e) => self.banners.set("gpu", format!("Lost the window surface: {e}")),
                }
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.surface.configure(&gpu.device, &gpu.config);
                    gpu.window.request_redraw();
                }
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                self.banners.set("gpu", "GPU validation error");
                return;
            }
        };
        let view = texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let (w, h) = (gpu.config.width, gpu.config.height);
        if let Err(e) = gpu.renderer.draw(&frame, &view, w, h) {
            self.banners.set("gpu", e);
        }
        if let Some(gpu) = self.gpu.as_ref() {
            gpu.queue.present(texture);
        }
    }
}

/// iTerm2's button numbers: 0 left, 1 right, 2 middle, then the others.
fn button_number(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
        MouseButton::Other(n) => n.min(255) as u8,
    }
}

/// The digit a physical key has on a U.S. keyboard (Emulate U.S. Keyboard).
fn us_digit(code: KeyCode) -> Option<u8> {
    use KeyCode::*;
    Some(match code {
        Digit0 | Numpad0 => 0,
        Digit1 | Numpad1 => 1,
        Digit2 | Numpad2 => 2,
        Digit3 | Numpad3 => 3,
        Digit4 | Numpad4 => 4,
        Digit5 | Numpad5 => 5,
        Digit6 | Numpad6 => 6,
        Digit7 | Numpad7 => 7,
        Digit8 | Numpad8 => 8,
        Digit9 | Numpad9 => 9,
        _ => return None,
    })
}


impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        if self.snippets.broken {
            self.banners.set("snippets", format!("Could not read the snippets in {}. The file stays as it is until a snippet is added.", snippets_ui::snippets_path().display()));
        }
        let arrangement_policy = self.globals.bool("OpenArrangementAtStartup");
        let hotkey_only = !arrangement_policy && self.globals.bool("OpenNoWindowsAtStartup");
        let hotkey_profiles: Vec<String> = self.hotkey_prefs().into_iter().filter(|(_, p)| p.active()).map(|(g, _)| g).collect();
        let system_policy = !arrangement_policy && !hotkey_only;
        // `state_frozen` at launch: a second copy of Tandem, which leaves the state to the first.
        let mut saved = if (system_policy || hotkey_only) && !self.state_frozen { self.saved_windows() } else { None };
        let regular_saved = saved.as_ref().is_some_and(|(w, _)| w.iter().any(|w| crate::arrangement::hotkey_guid(w).is_none()));
        let g = &self.globals;
        let policy = if arrangement_policy { crate::startup::DEFAULT_ARRANGEMENT } else if hotkey_only { crate::startup::HOTKEY_ONLY } else { crate::startup::SYSTEM };
        let startup_prefs = crate::startup::StartupPrefs { open_profiles_window: g.bool("OpenBookmark"), policy, always_open: g.bool("AlwaysOpenWindowAtStartup"), open_new_window: g.bool("openNewWindowAtStartup") };
        let early = crate::startup::plan_with(startup_prefs, false, !hotkey_profiles.is_empty(), system_policy && regular_saved);
        // Nothing to show at launch (Only Restore Hotkey Window, or policy 0 with a hotkey
        // window and nothing restored): the first window is a hotkey window, made hidden.
        let quiet = hotkey_only || (system_policy && !early.window && !early.restore);
        let startup_hotkey = if quiet && !self.state_frozen { hotkey_profiles.first().cloned() } else { None };
        let mut attrs = Window::default_attributes().with_title("Tandem").with_inner_size(LogicalSize::new(1000.0, 650.0));
        if startup_hotkey.is_some() {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs = attrs.with_visible(false).with_skip_taskbar(true);
        }
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                message_box(&format!("Could not open a window: {e}"));
                return event_loop.exit();
            }
        };
        match init_gpu(window, event_loop) {
            Ok(gpu) => {
                let k = windows_ui::key(gpu.window.id());
                self.last_window = Some(gpu.window.id());
                self.gpu = Some(gpu);
                self.windows.start(k);
                self.registry.open(k);
                // Edge drags snap to the cell grid, and follow AllowLiveResize.
                self.install_sizing();
                // A forced Theme's title bar survives system setting changes.
                self.guard_theme();
                self.refresh_caption_frame();
                self.apply_corners();
                // The settings folder's questions and failures from the launch.
                for ask in std::mem::take(&mut self.prefs_startup) {
                    self.ask_scripts(ask);
                }
                // The Python API starts with the first window when it is on.
                self.api_launch();
                // Replaced scripts and installs an earlier run left are put right first.
                self.recover_imports();
                self.scripts_scan_start();
                // With the API up (or not) and the Scripts folder made: AutoLaunch, once.
                self.run_autolaunch();
                // Scripts from an Import All Settings and Data (installed for the next run's
                // AutoLaunch, as iTerm2 relaunches after installing them).
                self.import_restored_scripts();
                self.ssh_secrets_start();
            }
            Err(e) => {
                message_box(&format!("Could not start the GPU renderer: {e}"));
                return event_loop.exit();
            }
        }
        self.apply_fonts();
        self.size_window(None);
        // Use System Window Restoration Setting (policy 0): the windows saved at the last run.
        // Open default window arrangement (policy 1): that arrangement. Either way the first
        // window fills this one, and the others open as new windows.
        // Sessions still running in the host: the saved ones they belong to take them over,
        // and the rest open in one new window (iTerm2's orphan adoption).
        let orphans = match saved.as_mut() {
            Some((windows, instance)) => {
                let instance = instance.clone();
                self.plan_reattach(windows, instance.as_deref())
            }
            None => self.plan_reattach(&mut [], None),
        };
        // Hotkey windows come back hidden under both policies (iTerm2 restores them always);
        // the other windows only under policy 0.
        let (hotkey_saved, regular_saved): (Vec<_>, Vec<_>) = saved.map(|(w, _)| w).unwrap_or_default().into_iter().partition(|w| crate::arrangement::hotkey_guid(w).is_some());
        let mut hotkey_saved: Vec<(String, crate::arrangement::Value)> = hotkey_saved.into_iter().filter_map(|w| crate::arrangement::hotkey_guid(&w).filter(|g| hotkey_profiles.contains(g)).map(|g| (g, w))).collect();
        let saved = (system_policy && !regular_saved.is_empty()).then_some(regular_saved);
        let has_saved = saved.is_some() || !hotkey_saved.is_empty();
        // The default arrangement is restored by name (window restoration has none).
        let default_arrangement = self
            .globals
            .bool("OpenArrangementAtStartup")
            .then(|| {
                let store = self.arrangements();
                store.default_name().and_then(|n| store.get(n).cloned().map(|w| (Some(n.to_string()), w)))
            })
            .flatten()
            .filter(|(_, w)| !w.is_empty());
        let has_arrangement = default_arrangement.is_some();
        // The saved state's windows are the launch's own restoration (`launch` true); a default
        // arrangement is an arrangement like any other.
        let startup_arrangement = if startup_hotkey.is_some() { None } else { saved.map(|w| (None, w, true)).or(default_arrangement.map(|(n, w)| (n, w, false))) };
        let mut arranged = false;
        match startup_arrangement {
            Some((name, mut windows, launch)) => {
                let first = windows.remove(0);
                match self.apply_arrangement_window(&first, name.as_deref(), launch) {
                    Ok(()) => arranged = !self.tabs.is_empty(),
                    Err(e) => self.banners.set("arrangement", e),
                }
                if self.tabs.is_empty() {
                    self.new_tab(None);
                }
                for w in windows {
                    self.pending_windows.push(if launch { windows_ui::NewWindow::Restored(w) } else { windows_ui::NewWindow::Arrangement(w, name.clone()) });
                }
            }
            None => match &startup_hotkey {
                Some(guid) => {
                    if let Some(i) = hotkey_saved.iter().position(|(g, _)| g == guid) {
                        let (_, w) = hotkey_saved.remove(i);
                        match self.apply_arrangement_window(&w, None, true) {
                            Ok(()) => arranged = !self.tabs.is_empty(),
                            Err(e) => self.banners.set("arrangement", e),
                        }
                    }
                    if self.tabs.is_empty() {
                        self.new_tab(Some(guid));
                    }
                    self.hotkey = Some(hotkey_ui::HotkeyWin::new(guid.clone()));
                }
                None => self.new_tab(None),
            },
        }
        if !arranged {
            self.apply_custom_window_title(startup_hotkey.as_deref());
        }
        for (guid, w) in hotkey_saved {
            self.pending_windows.push(windows_ui::NewWindow::HotkeyRestored(guid, w));
        }
        if !orphans.is_empty() {
            self.pending_windows.push(windows_ui::NewWindow::Orphans(orphans));
        }
        self.install_menus();
        // A window the default arrangement filled keeps its frame, style, and locks.
        let steps = crate::winmodel::open_steps(arranged);
        if steps.place && startup_hotkey.is_none() {
            self.place_window();
        } else {
            self.claim_slot();
        }
        if steps.profile_window && startup_hotkey.is_none() {
            self.apply_window_profile(None);
        }
        // The Profiles window follows Open profiles window.
        let g = &self.globals;
        let policy = if g.bool("OpenArrangementAtStartup") { crate::startup::DEFAULT_ARRANGEMENT } else if g.bool("OpenNoWindowsAtStartup") { crate::startup::HOTKEY_ONLY } else { crate::startup::SYSTEM };
        let prefs = crate::startup::StartupPrefs { open_profiles_window: g.bool("OpenBookmark"), policy, always_open: g.bool("AlwaysOpenWindowAtStartup"), open_new_window: g.bool("openNewWindowAtStartup") };
        if crate::startup::plan_with(prefs, has_arrangement, !hotkey_profiles.is_empty(), has_saved).profiles_window {
            self.open_settings(event_loop, Mode::Profiles);
        }
        let style = startup_hotkey.as_deref().and_then(|g| self.store.get(g)).unwrap_or_else(|| self.store.default_profile()).int("Window Type");
        if steps.profile_style && style != crate::frames::NORMAL {
            self.apply_window_style(style, false);
        }
    }

    /// Runs the cursor blink when the focused pane's profile asks for it.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Timers (blink, bells, activity, marks, animations) act for the window the person
        // uses; the window that drew last may be another one.
        if let Some(w) = self.last_window {
            self.enter(w);
        }
        self.flush_dropped();
        for w in std::mem::take(&mut self.pending_windows) {
            self.open_pending(event_loop, w);
            self.tmux_apply_geometry();
        }
        self.api_tmux_tabs_opened();
        self.tmux_auto_bury();
        if self.save_due.is_some_and(|t| Instant::now() >= t) {
            self.save_now();
        }
        let lost = std::mem::take(&mut self.focus_lost);
        self.save_state(lost);
        self.replay_follow_focus(event_loop);
        self.replay_panels_follow_app();
        // Menu checks that follow the current tab and pane (broadcast input) refresh when
        // either changes.
        let key = (self.current_window(), self.active, self.tabs.get(self.active).map(|t| t.focused));
        // A tab or pane change inside the window (not a switch of windows) moves the
        // composer's keyboard.
        let focus_key = (key.1, key.2);
        if self.composer_focus_key != Some(focus_key) {
            self.composer_focus_key = Some(focus_key);
            self.composer_focus_follows();
        }
        if self.checks_key != Some(key) {
            self.checks_key = Some(key);
            self.sync_checks();
        }
        // A selection made or cleared at an idle prompt moves Explain Output's state.
        self.explain_follows();
        let active = hotkey_ui::tandem_in_front();
        if active && !self.app_active {
            self.hotkey_reactivated();
        }
        self.app_active = active;
        let hotkey_due = self.tick_hotkeys();
        let find_due = self.run_due_searches();
        let spotlight = self.tick_find_cursor();
        let animation = self.next_animation_frame();
        let sync = self.flush_due_syncs();
        let toast = self.toast_due();
        let rotation = self.tick_rotations();
        let upload = self.feed_uploads();
        let paste_due = self.paste_tick().into_iter().chain(self.auto_command_tick()).min();
        let triggers = self.tick_triggers();
        self.tick_bell();
        let activity = self.tick_activity();
        self.tick_status_change();
        let idle_codes = self.tick_idle_codes();
        let status_bars = self.tick_status_bars();
        let notes_due = self.tick_notes();
        let jobs_due = self.tick_jobs_tool();
        let quickly_due = self.tick_quickly();
        let composer_due = self.tick_composer();
        self.record_replay_frames();
        let replay_due = self.replay_frame_due();
        self.open_composer_windows(event_loop);
        self.open_jobs_confirm(event_loop);
        self.open_recording_windows(event_loop);
        self.open_git_log(event_loop);
        let pwm_due = self.tick_pwm();
        self.open_pwm_windows(event_loop);
        self.api_tick(event_loop);
        self.api_notify_tick();
        self.open_api_alerts(event_loop);
        self.tick_script_titles();
        let script_due = self.script_tick();
        let provision_due = self.provision_tick();
        self.scripts_tick();
        self.script_picker_tick();
        self.deps_tick();
        self.open_pending_deps_run(event_loop);
        let console_due = self.script_console_tick(event_loop);
        self.open_pending_repl(event_loop);
        let import_due = self.import_tick();
        self.script_export_tick();
        self.scripts_scan_tick();
        // OK in "Importing Settings and Data": Tandem quits and starts again.
        if std::mem::take(&mut self.relaunch_due) {
            self.relaunch(event_loop);
        }
        self.prefs_tick(event_loop);
        self.catalog_tick();
        self.suppressed_tick();
        self.open_script_alerts(event_loop);
        self.open_scripts_alerts(event_loop);
        self.open_ai_alert(event_loop);
        self.open_ai_key_window(event_loop);
        self.open_ai_reveal(event_loop);
        self.ai_discovery_tick();
        self.ai_runs_tick();
        self.open_ai_engage_windows(event_loop);
        let ai_due = self.ai_discovery_due();
        let chat_due = self.chat_tick();
        let chat_window_due = self.chat_window_tick();
        self.open_script_dialogs(event_loop);
        if self.attention_due.is_some_and(|t| Instant::now() >= t) {
            self.prune_attention();
            self.redraw();
        }
        let undo_due = self.expire_undo();
        self.open_tmux_command_prompt(event_loop);
        self.open_requested_advanced_paste(event_loop);
        self.feed_shell_history();
        // The conductors' command timers, before tmux_send_input: a timed-out conductor in a
        // tmux integration pane writes through the gateway.
        let ssh_due = self.tick_ssh();
        // Writes that did not come through user_write (mouse reports, Buffer Input).
        self.tmux_send_input();
        self.tmux_focus_tick();
        self.tmux_finish_close(event_loop);
        self.finish_tab_closes(event_loop);
        self.tmux_dashboard_tick();
        for (gw, events) in std::mem::take(&mut self.tmux_held_events) {
            self.tmux_events(gw, events, event_loop);
        }
        let tip_due = self.tick_tab_tip();
        let flash_due = self.tick_tab_flash();
        let tmux_due = [self.tick_tmux(), self.tmux_pause_tick(event_loop), self.tmux_dashboard_due()].into_iter().flatten().min();
        let wake = [self.save_due, self.prefs_save_due, find_due, spotlight, animation, sync, toast, self.attention_due, rotation, upload, paste_due, triggers, activity, idle_codes, undo_due, hotkey_due, tmux_due, ssh_due, status_bars, notes_due, jobs_due, quickly_due, composer_due, replay_due, pwm_due, script_due, provision_due, console_due, import_due, self.catalog_due, self.suppressed_retitle_due, tip_due, flash_due, ai_due, chat_due, chat_window_due].into_iter().flatten().min();
        let blinks = self
            .tabs
            .get(self.active)
            .and_then(|t| self.panes.get(&t.focused))
            .and_then(|p| self.store.get(&p.profile))
            .is_some_and(|p| p.bool("Blinking Cursor"));
        if !blinks || !self.focused {
            if !self.blink_on {
                self.blink_on = true;
                self.redraw();
            }
            match wake {
                Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
                None => event_loop.set_control_flow(ControlFlow::Wait),
            }
            return;
        }
        if Instant::now() >= self.last_blink + BLINK {
            self.blink_on = !self.blink_on;
            self.last_blink = Instant::now();
            self.redraw();
        }
        let next = wake.map_or(self.last_blink + BLINK, |t| t.min(self.last_blink + BLINK));
        event_loop.set_control_flow(ControlFlow::WaitUntil(next));
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Hotkey(ids) => self.hotkey_pressed(&ids, event_loop),
            UserEvent::Chord(c) => {
                // Win+B, Win+U and Win+C in a password manager window are its own keys.
                if self.pwm_take_chord(&c) {
                    return;
                }
                // Win+W, Shift+Win+Y and the find chords in the AI Chat window are its own keys.
                if self.chat_take_chord(&c, event_loop) {
                    return;
                }
                if let Some(w) = self.last_window {
                    self.enter(w);
                }
                if let Some(s) = self.settings.as_mut().filter(|s| s.recording()) {
                    return s.record_chord(&c);
                }
                return self.run_chord(&c, event_loop);
            }
            UserEvent::Menu(id) => {
                if let Some(w) = self.last_window {
                    self.enter(w);
                }
                return self.run_menu_id(&id, event_loop);
            }
            UserEvent::NotificationClicked => return self.reveal_notified(),
            UserEvent::JobsChanged => return self.jobs_changed(),
            UserEvent::ThemeReset => return self.reapply_forced_theme(),
            UserEvent::Caption(hwnd, e) => return self.caption_event(hwnd, e, event_loop),
            UserEvent::Wake => {}
        }
        let mut changed = false;
        let mut exited = Vec::new();
        let mut outputs = Vec::new();
        let now = Instant::now();
        self.sync_output_policy();
        self.sync_host_policy();
        // The prompt state machine follows View > Auto Composer before it reads output (the
        // Task 3 contract: every pane, new ones included).
        let auto_composer = self.globals.bool("AutoComposer");
        for (id, p) in self.panes.iter_mut() {
            p.own_mut().prompt_state.enabled = auto_composer;
            let got = p.pump();
            if got {
                outputs.push(*id);
            }
            changed |= got;
            // A held pane's end is reported once.
            if p.exited.is_some() && self.held_on_exit.get(id) != Some(&true) {
                exited.push(*id);
            }
        }
        changed |= self.ssh_pump();
        changed |= self.tmux_pump(event_loop);
        self.api_output(&outputs);
        self.api_session_events();
        self.record_replay_frames();
        for id in exited {
            // A git command's short-lived window ends without an alert.
            let short_lived = self.short_lived(id);
            self.single_use_ended(id);
            self.deps_command_ended(id);
            if !short_lived {
                self.session_ended(id);
            }
            if let Some(p) = self.panes.get_mut(&id) {
                p.own_mut().fire_event(crate::trigger_events::EventKind::SessionEnded, &Default::default(), &[]);
            }
            self.take_requests();
            match scripts_app::on_exit(self.held_on_exit.get(&id).copied()) {
                scripts_app::OnExit::Close => self.close_pane(id, event_loop),
                scripts_app::OnExit::Keep => {
                    self.held_on_exit.insert(id, true);
                    changed = true;
                }
                scripts_app::OnExit::Handled => {}
            }
        }
        self.check_coprocesses();
        if changed {
            self.after_output(&outputs, now);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.prompt_event(id, &event) || self.coprocess_prompt_event(id, &event) || self.tmux_prompt_event(id, &event) || self.tmux_dashboard_event(id, &event, event_loop) || self.chat_window_event(id, &event, event_loop) || self.chat_prompt_event(id, &event) || self.title_prompt_event(id, &event) || self.group_prompt_event(id, &event) || self.arrange_prompt_event(id, &event) || self.global_find_event(id, &event) || self.paste_history_event(id, &event) || self.list_popup_event(id, &event) || self.advanced_paste_event(id, &event) || self.note_event(id, &event) || self.add_clip_event(id, &event) || self.shot_event(id, &event) || self.status_setup_event(id, &event) || self.git_log_event(id, &event) || self.status_popover_event(event_loop, id, &event) || self.jobs_window_event(id, &event) || self.composer_window_event(id, &event) || self.open_quickly_event(event_loop, id, &event) || self.replay_window_event(event_loop, id, &event) || self.recording_warning_event(id, &event) || self.pwm_window_event(id, &event) || self.pwm_confirm_event(id, &event) || self.api_alert_event(event_loop, id, &event) || self.script_alert_event(id, &event) || self.scripts_alert_event(id, &event) || self.ai_alert_event(id, &event) || self.ai_key_event(id, &event) || self.ai_engage_event(id, &event) || self.script_picker_event(id, &event) || self.script_export_event(id, &event) || self.script_deps_event(id, &event, event_loop) || self.script_console_event(id, &event, event_loop) || self.script_inspector_event(id, &event) || self.script_dialog_event(id, &event) || self.text_popover_event(id, &event) || self.suppressed_event(id, &event) {
            return;
        }
        for profiles in [false, true] {
            let slot = if profiles { &mut self.profiles_win } else { &mut self.settings };
            if !slot.as_ref().is_some_and(|s| s.id() == id) {
                continue;
            }
            match event {
                WindowEvent::CloseRequested => {
                    // A custom scripts folder typed and not yet applied is applied as the
                    // window closes (iTerm2's `windowWillClose`, `GP:3428-3430`).
                    let pending = slot.as_mut().is_some_and(|s| s.take_folder_pending());
                    *slot = None;
                    crate::winkeys::set_recording(false);
                    if pending {
                        self.apply_edit(Edit::GlobalsChanged);
                    }
                }
                WindowEvent::RedrawRequested => {
                    let win = slot.as_mut().unwrap();
                    win.show_remembered = self.show_remembered;
                    let edit = win.redraw(&mut self.store, &mut self.globals, &mut self.snippets);
                    let answered = std::mem::take(&mut win.answered);
                    // Profiles > Session's "Configure Status Bar" opens the setup window.
                    let setup = slot.as_mut().and_then(|s| s.configure_status_bar.take());
                    // Configure SSH's help button opens its page in the browser.
                    let url = slot.as_mut().and_then(|s| s.open_url.take());
                    self.apply_edit(edit);
                    self.settings_warnings_answered(answered);
                    if let Some(guid) = setup {
                        self.open_status_setup(statusbar_ui::SetupTarget::Shared(guid), None, false, event_loop);
                    }
                    if let Some(url) = url {
                        if let Err(e) = semantic_ui::shell_open(&url, None, false) {
                            self.banners.set("window", format!("Could not open {url}: {e}"));
                        }
                    }
                    // Settings > General > AI's discovery requests (ruling R12ab-3).
                    self.drain_ai_requests(event_loop);
                }
                other => {
                    slot.as_mut().unwrap().event(&other);
                }
            }
            return;
        }
        // Tandem is ending once the settings folder's quit question is answered: the terminal
        // windows, whose sessions are gone, take nothing more.
        if self.prefs_exit_pending {
            return;
        }
        // Every other window is a terminal window: make it current first.
        if !self.enter(id) {
            return;
        }
        match event {
            WindowEvent::ThemeChanged(_) => {
                self.theme_applied = Some(self.dark());
                self.refresh_panes();
                // A light or dark window may gain or lose its border (`BordersOnlyInLightMode`).
                self.apply_border();
            }
            WindowEvent::DroppedFile(path) => {
                let (x, y) = self.cursor;
                let pane = self.pane_at(x, y).map(|(p, _)| p).or_else(|| self.focused_id());
                if let Some(p) = pane {
                    self.dropped.push((p, path.to_string_lossy().to_string()));
                }
            }
            WindowEvent::CloseRequested => self.close_window(id, event_loop),
            // tmux keeps where Tandem's windows are (@origins).
            WindowEvent::Moved(_) => self.tmux_save_all_windows(),
            WindowEvent::Resized(size) => {
                // Minimizing reports 0x0. Keep every pty at its size, or ConPTY reflows the screen.
                let Some((w, h)) = usable_size(size.width, size.height) else { return };
                if let Some(g) = self.gpu.as_mut() {
                    g.config.width = w;
                    g.config.height = h;
                    g.surface.configure(&g.device, &g.config);
                }
                self.relayout();
                self.tmux_save_all_windows();
                // Entering or leaving full screen resizes the window: the border goes or comes back.
                self.apply_border();
            }
            WindowEvent::ScaleFactorChanged { mut inner_size_writer, .. } => {
                // winit is about to size the window for a title bar the caption subclass removes:
                // the size it sets is corrected here, before it does (`caption_ui::dpi_size`).
                if let Some(size) = self.hwnd().and_then(caption_ui::dpi_size) {
                    let _ = inner_size_writer.request_inner_size(size);
                }
                self.apply_fonts();
                self.relayout();
            }
            WindowEvent::Focused(f) => {
                // A divider drag loses its release when focus goes, and so do keys.
                if !f {
                    self.tmux_drag_cancel();
                    // So does a press on the tab bar (a held close button lets go).
                    if self.tab_press.take().is_some() {
                        self.redraw();
                    }
                    // And the tab tooltip goes, as a macOS tooltip does when the app deactivates.
                    self.hide_tab_tip();
                    // And the tab bar's flash stops (`PT windowDidResignKey:`, `PT:5470-5471`):
                    // it fades, and in full screen a second stop ends it at once (`PT:5484`).
                    self.stop_tab_flash();
                    self.tab_flash.set_win_held(false, Instant::now());
                    if self.full_screen() {
                        self.stop_tab_flash();
                    }
                    self.belt_keys.clear();
                    self.replay_keys.clear();
                    self.composer_keys.clear();
                }
                self.focused = f;
                // The border takes the focused or unfocused color.
                self.apply_border();
                // Tandem going to the background saves the windows (iTerm2 saves on resign
                // active); a switch between Tandem's own windows gains focus again at once.
                self.focus_lost = !f;
                if !f && self.hotkey.is_some() {
                    self.hotkey_focus_lost();
                }
                if f {
                    self.last_window = Some(id);
                    self.stamp_focus();
                    // The Window menu checks the focused window.
                    self.refresh_window_list();
                }
                self.jobs.set_fast(f);
                // Keys held while the window loses focus send no release to Tandem.
                if !f {
                    self.sent_keys.clear();
                    self.api_focus_lost();
                }
                // iTerm2 drops a pending Leader when the app goes to the background.
                if !f && self.leader_on {
                    self.leader_on = false;
                    self.refresh_route();
                }
                // Focus reporting (DECSET 1004): tell the app the window gained or lost focus.
                if let Some(p) = self.focused_pane() {
                    if p.screen.term().mode().contains(TermMode::FOCUS_IN_OUT) {
                        p.focus_report(f);
                    }
                }
                // A closed password manager's hold ends with the window's next gain, which is
                // not reported either (its loss was not).
                if f {
                    self.pwm_focus_gained();
                }
                self.redraw();
            }
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                self.mods = Mods { shift: s.shift_key(), alt: s.alt_key(), ctrl: s.control_key(), cmd: s.super_key() };
                // A held Win key keeps a flashing tab bar up (`PT flagsChanged:`, `PT:13278`).
                let was = self.tab_flash.state();
                self.tab_flash.set_win_held(s.super_key(), Instant::now());
                if was != self.tab_flash.state() {
                    self.redraw();
                }
                self.update_hover();
                if s.alt_key() {
                    self.alt_side = if m.ralt_state() == ModifiersKeyState::Pressed { AltSide::Right } else { AltSide::Left };
                }
                // Cycle Tabs ends when the chord's modifiers come up.
                if let Some(c) = self.mru_mods {
                    if !((c.ctrl && s.control_key()) || (c.alt && s.alt_key()) || (c.cmd && s.super_key())) {
                        self.mru_mods = None;
                        self.mru.commit();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // A key that ends find typing, a tool's focus, Copy Mode, the completion list
                // or the labels gives the terminal the keyboard back (`notifyActive`).
                let elsewhere = self.focused_keys_elsewhere();
                let focused = self.focused_id();
                self.key(&event, event_loop);
                self.composer_take_back(elsewhere);
                // Any key ends a drag-then-Copy episode; Copy itself has already looked at it
                // (modifiers alone are no key press, as in iTerm2's keyDown:).
                if let Some(id) = focused.filter(|_| event.state == ElementState::Pressed && !labels_ui::is_modifier_key(&event.logical_key)) {
                    self.frustration(id, |d| d.key());
                }
            }
            // The pointer left the window: no Help tooltip stays up.
            WindowEvent::CursorLeft { .. } => {
                let composer = self.composer_hover.take().is_some();
                let tab = self.tab_hover.take().is_some();
                self.tab_tip = None;
                if composer || tab {
                    self.redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => self.mouse_move(position.x as f32, position.y as f32),
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => {
                // As for keys: a press that ends another consumer's keyboard (the find bar's
                // close button, a press away from a tool) gives the terminal it back.
                let elsewhere = self.focused_keys_elsewhere();
                self.mouse_press(button, event_loop);
                self.composer_take_back(elsewhere);
            }
            WindowEvent::MouseInput { state: ElementState::Released, button, .. } => self.mouse_release(button, event_loop),
            WindowEvent::MouseWheel { delta, .. } => self.wheel(delta),
            WindowEvent::RedrawRequested => {
                // A tab or pane change in this window ends or shows Instant Replay.
                self.replay_follow_focus(event_loop);
                // Closing a loaded recording's pane there can close this window and make
                // another current; then this window has nothing left to draw.
                if self.gpu.as_ref().is_none_or(|g| g.window.id() != id) {
                    return;
                }
                // A font change made while another window was current reaches this one now.
                let sig = self.font_sig.clone();
                self.apply_fonts();
                // A font change, a banner or a status bar (shared or per pane) that came,
                // went or changed height while another window was current reaches this one
                // now: the room its terminals were sized for is compared whole.
                if sig != self.font_sig || self.laid_out.as_ref() != Some(&self.room()) {
                    self.relayout();
                }
                self.render()
            }
            _ => {}
        }
    }
}

fn init_gpu(window: Arc<Window>, event_loop: &ActiveEventLoop) -> Result<Gpu, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
        event_loop.owned_display_handle(),
    )));
    let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: Some(&surface),
        ..Default::default()
    }))
    .map_err(|e| e.to_string())?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| e.to_string())?;
    let caps = surface.get_capabilities(&adapter);
    let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
    let size = window.inner_size();
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
        color_space: wgpu::SurfaceColorSpace::Auto,
    };
    surface.configure(&device, &config);
    let renderer = Renderer::new(&device, &queue, format, FONT, FONT_PX * window.scale_factor() as f32);
    Ok(Gpu { surface, adapter, device, queue, config, renderer, instance, window })
}

/// The surface size to use, or None while the window is minimized (0 wide or high).
fn usable_size(w: u32, h: u32) -> Option<(u32, u32)> {
    (w > 0 && h > 0).then_some((w, h))
}

/// A new window's side tab bar width in points: `LeftTabBarWidth`.
fn side_bar_default(globals: &Globals) -> f32 {
    globals.number("LeftTabBarWidth").map_or(tab_position::DEFAULT_SIDE_WIDTH, |v| v as f32)
}

/// Moves the tab `new_tab` just added (the last one) to just after `active`, and returns
/// its new index. None when the tab count did not grow past `before`: nothing opened.
fn move_new_tab<T>(tabs: &mut Vec<T>, before: usize, active: usize) -> Option<usize> {
    if tabs.len() <= before {
        return None;
    }
    let tab = tabs.pop()?;
    let at = (active + 1).min(tabs.len());
    tabs.insert(at, tab);
    Some(at)
}

/// What the "+" button opens (`PT tabViewDidClickAddTabButton:`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AddTab {
    /// A tmux tab: the current session is a tmux client.
    TmuxTab,
    /// A tab with the default profile.
    Default,
    /// A tab with the current session's profile (`AddTabButtonUsesCurrentProfile`).
    CurrentProfile,
}

/// Moves a window's flash along to `now`; true when the window should draw (its flash
/// changed state or is fading).
fn tick_flash(flash: &mut tab_flash::Flash, now: Instant) -> bool {
    let was = flash.state();
    flash.tick(now);
    flash.state() != was || flash.state() == tab_flash::State::FadingOut
}

/// When a window's flash next needs a look: its next change, and the next frame while it fades.
fn flash_wake(flash: &tab_flash::Flash, now: Instant) -> Option<Instant> {
    let next = flash.next_change();
    if flash.state() == tab_flash::State::FadingOut {
        return next.map(|t| t.min(now + FLASH_FRAME));
    }
    next
}

fn add_tab_choice(tmux_client: bool, use_current: bool) -> AddTab {
    if tmux_client {
        AddTab::TmuxTab
    } else if use_current {
        AddTab::CurrentProfile
    } else {
        AddTab::Default
    }
}

/// The window is snapped to a half or a quarter of the screen (`IsWindowArranged`, user32 on
/// Windows 10 and later; looked up at run time, so an older Windows says no).
fn window_arranged(hwnd: isize) -> bool {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    type Arranged = unsafe extern "system" fn(windows_sys::Win32::Foundation::HWND) -> i32;
    static ARRANGED: std::sync::OnceLock<Option<Arranged>> = std::sync::OnceLock::new();
    let f = ARRANGED.get_or_init(|| {
        let user32: Vec<u16> = "user32.dll".encode_utf16().chain(Some(0)).collect();
        let p = unsafe { GetProcAddress(GetModuleHandleW(user32.as_ptr()), c"IsWindowArranged".as_ptr().cast())? };
        Some(unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, Arranged>(p) })
    });
    f.is_some_and(|f| unsafe { f(hwnd as windows_sys::Win32::Foundation::HWND) } != 0)
}

/// A hit on the tab bar, unless it names a tab that no longer exists.
fn live_tab_hit(hit: Option<tab_layout::Hit>, tabs: usize) -> Option<tab_layout::Hit> {
    use tab_layout::Hit;
    hit.filter(|h| !matches!(h, Hit::Tab(i) | Hit::Close(i) if *i >= tabs))
}

/// A press on the tab bar: what it hit, its click count (0 for the middle button), whether Alt
/// was held, and the id of the tab it hit (None off the tabs).
pub(super) type TabPress = (tab_layout::Hit, u8, bool, Option<u64>);

/// The press was on a tab (or its close button) that `id_at` no longer finds at the pressed
/// index: a tab before it closed, or the tabs moved. iTerm2 holds the cell itself, so its
/// release can never act on another tab.
fn press_on_moved_tab(press: Option<TabPress>, id_at: impl Fn(usize) -> Option<u64>) -> bool {
    match press {
        Some((tab_layout::Hit::Tab(i) | tab_layout::Hit::Close(i), _, _, id)) => id_at(i) != id,
        _ => false,
    }
}

/// The close button a press holds down: a left press on it (the middle button's count is 0;
/// iTerm2 marks it pressed in `mouseDown:` only).
fn held_close(press: Option<TabPress>) -> Option<usize> {
    match press {
        Some((tab_layout::Hit::Close(i), clicks, _, _)) if clicks > 0 => Some(i),
        _ => None,
    }
}

/// The click count of a left press on the tab bar at `now`, and the new last press (when, on
/// which tab, its count): one more than the last when it is on the same tab (or again off the
/// tabs) within `double_ms`, else 1. A press on the "+" counts 1 and starts the count over.
fn tab_click_count(last: (Instant, Option<usize>, u8), now: Instant, hit: tab_layout::Hit, double_ms: u128) -> (u8, (Instant, Option<usize>, u8)) {
    use tab_layout::Hit;
    if hit == Hit::Add {
        return (1, (now, None, 0));
    }
    let on = match hit {
        Hit::Tab(i) | Hit::Close(i) => Some(i),
        _ => None,
    };
    let (then, prev, n) = last;
    let again = n > 0 && prev == on && now.duration_since(then).as_millis() <= double_ms;
    let clicks = if again { n.saturating_add(1) } else { 1 };
    (clicks, (now, on, clicks))
}

/// What a session's screen takes from its profile, at start (`install_pane`) and after every
/// profile edit (`refresh_panes`), as iTerm2 re-applies them (`PTYSession.m:7013-7016`): the
/// prompt column, the profile's Unicode version, scrollback with the status bar, the triggers,
/// whether programs may set the title (`PS:19364-19371`).
fn screen_follows_profile(screen: &mut crate::screen::Screen, p: &Profile) {
    screen.place_prompt_first_column = p.bool("Place Prompt at First Column");
    screen.allow_title = p.bool("Allow Title Setting");
    screen.profile_unicode = crate::osc1337::profile_unicode(p.values.get("Unicode Version").and_then(|v| v.as_integer()));
    screen.set_scrollback_with_status_bar(p.bool("Scrollback With Status Bar"));
    let (list, prefs) = triggers_ui::triggers_of(p);
    screen.set_triggers(list, prefs);
}

/// Whether a window's menu bar hides: in full screen with `HideMenuBarInFullscreen`
/// (`P:830`); leaving full screen brings it back.
fn menu_should_hide(full_screen: bool, hide_key: bool) -> bool {
    full_screen && hide_key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_bar_hides_in_full_screen() {
        assert!(menu_should_hide(true, true));
        assert!(!menu_should_hide(true, false));
        assert!(!menu_should_hide(false, true));
        assert!(!menu_should_hide(false, false));
    }

    #[test]
    fn add_button_follows_iterm2() {
        assert_eq!(add_tab_choice(true, false), AddTab::TmuxTab);
        assert_eq!(add_tab_choice(true, true), AddTab::TmuxTab);
        assert_eq!(add_tab_choice(false, false), AddTab::Default);
        assert_eq!(add_tab_choice(false, true), AddTab::CurrentProfile);
    }

    #[test]
    fn profile_edits_reach_the_screen() {
        // A profile edit re-applies the Unicode version, as a new pane takes it (PS:7013-7016).
        let mut s = crate::screen::Screen::new(10, 3);
        let mut p = Profile::new("x");
        p.values.insert("Unicode Version".into(), toml::Value::Integer(8));
        p.values.insert("Place Prompt at First Column".into(), toml::Value::Boolean(false));
        screen_follows_profile(&mut s, &p);
        assert_eq!(s.profile_unicode, 8);
        assert!(!s.place_prompt_first_column);
        s.clipboard_allowed = true;
        let reply = String::from_utf8(s.feed(b"\x1b]1337;Capabilities\x07")).unwrap();
        assert!(reply.contains("Uw8"), "{reply}");
        p.values.insert("Unicode Version".into(), toml::Value::Integer(9));
        screen_follows_profile(&mut s, &p);
        assert_eq!(s.profile_unicode, 9);
        let reply = String::from_utf8(s.feed(b"\x1b]1337;Capabilities\x07")).unwrap();
        assert!(reply.contains("Uw9"), "{reply}");
    }

    #[test]
    fn steal_focus_asks_once() {
        use crate::screen::Request;
        let now = Instant::now();
        let many: Vec<(PaneId, Request)> = (0..1000).map(|_| (1, Request::StealFocus)).chain([(2, Request::StealFocus), (1, Request::ReportCellSize)]).collect();
        let kept = clipboard_ui::thin_requests(many.clone(), None, now);
        assert_eq!(kept.iter().filter(|(p, r)| *p == 1 && *r == Request::StealFocus).count(), 1);
        assert_eq!(kept.iter().filter(|(p, r)| *p == 2 && *r == Request::StealFocus).count(), 1);
        assert!(kept.contains(&(1, Request::ReportCellSize)), "other requests stay");
        let quiet = clipboard_ui::thin_requests(many, Some(now), now + std::time::Duration::from_millis(500));
        assert!(!quiet.iter().any(|(_, r)| *r == Request::StealFocus), "none right after a question");
    }

    #[test]
    fn closing_an_earlier_tab_keeps_the_same_tab_active() {
        // On tab 2 of 3 (index 1), tab index 0 closes: the user stays on the same tab, now index 0.
        use crate::close_rules::active_after_close;
        assert_eq!(active_after_close(1, 0, 2, false), 0);
        // A later tab closing does not move the user.
        assert_eq!(active_after_close(0, 2, 2, false), 0);
        // The active tab itself closing moves to the one that took its place, or the last one.
        assert_eq!(active_after_close(2, 2, 2, false), 1);
        assert_eq!(active_after_close(0, 0, 0, false), 0);
    }

    #[test]
    fn a_new_tab_moves_next_to_the_active_one_only_if_it_opened() {
        let mut tabs = vec!["a", "b", "c", "new"];
        assert_eq!(move_new_tab(&mut tabs, 3, 1), Some(2));
        assert_eq!(tabs, ["a", "b", "new", "c"]);
        // The shell did not start: no tab was added, so nothing moves.
        let mut tabs = vec!["a", "b", "c"];
        assert_eq!(move_new_tab(&mut tabs, 3, 0), None);
        assert_eq!(tabs, ["a", "b", "c"]);
    }

    #[test]
    fn a_minimized_window_has_no_usable_size() {
        assert_eq!(usable_size(0, 0), None);
        assert_eq!(usable_size(800, 0), None);
        assert_eq!(usable_size(800, 600), Some((800, 600)));
    }

    #[test]
    fn only_a_left_press_holds_a_close_button() {
        use tab_layout::Hit;
        // iTerm2 sets `closeButtonPressed` in `mouseDown:` only, never for the middle button.
        assert_eq!(held_close(Some((Hit::Close(2), 1, false, Some(7)))), Some(2));
        assert_eq!(held_close(Some((Hit::Close(2), 2, true, Some(7)))), Some(2));
        assert_eq!(held_close(Some((Hit::Close(2), 0, false, Some(7)))), None, "a middle press");
        assert_eq!(held_close(Some((Hit::Tab(2), 1, false, Some(7)))), None);
        assert_eq!(held_close(None), None);
    }

    #[test]
    fn a_press_on_a_tab_that_moved_is_dropped() {
        use tab_layout::Hit;
        let ids = [10u64, 11, 12];
        let at = |i: usize| ids.get(i).copied();
        // The pressed tab is still where it was pressed.
        assert!(!press_on_moved_tab(Some((Hit::Close(1), 1, false, Some(11))), at));
        assert!(!press_on_moved_tab(Some((Hit::Tab(2), 0, false, Some(12))), at));
        // A tab before it closed: index 1 now holds another tab, which must not close.
        assert!(press_on_moved_tab(Some((Hit::Close(1), 1, false, Some(10))), at));
        assert!(press_on_moved_tab(Some((Hit::Tab(1), 0, false, Some(10))), at), "a middle press");
        // The pressed tab itself is gone off the end.
        assert!(press_on_moved_tab(Some((Hit::Tab(3), 1, false, Some(13))), at));
        // Presses off the tabs keep no tab.
        assert!(!press_on_moved_tab(Some((Hit::Add, 1, false, None)), at));
        assert!(!press_on_moved_tab(Some((Hit::Bar, 2, false, None)), at));
        assert!(!press_on_moved_tab(None, at));
    }

    #[test]
    fn tab_clicks_count_on_one_tab() {
        use tab_layout::Hit;
        let t0 = Instant::now();
        let soon = t0 + std::time::Duration::from_millis(100);
        let late = t0 + std::time::Duration::from_millis(900);
        let start = (t0, None, 0);
        // A first press counts 1; again on the same tab within the time 2; later 1 again.
        let (n, last) = tab_click_count(start, t0, Hit::Tab(1), 500);
        assert_eq!((n, last), (1, (t0, Some(1), 1)));
        assert_eq!(tab_click_count(last, soon, Hit::Tab(1), 500).0, 2);
        assert_eq!(tab_click_count(last, soon, Hit::Close(1), 500).0, 2, "the close button is on the tab");
        assert_eq!(tab_click_count(last, soon, Hit::Tab(2), 500).0, 1, "another tab");
        assert_eq!(tab_click_count(last, late, Hit::Tab(1), 500).0, 1, "too late");
        // Twice on the empty bar counts 2 (a double click opens a tab).
        let (_, bar) = tab_click_count(start, t0, Hit::Bar, 500);
        assert_eq!(tab_click_count(bar, soon, Hit::Bar, 500).0, 2);
        // The "+" counts 1, even right after a press on the empty bar, and starts the count over.
        let (n, after) = tab_click_count(bar, soon, Hit::Add, 500);
        assert_eq!(n, 1);
        assert_eq!(after.2, 0);
        assert_eq!(tab_click_count(after, soon, Hit::Bar, 500).0, 1);
    }

    #[test]
    fn a_stale_tab_is_not_a_hit() {
        use tab_layout::Hit;
        assert_eq!(live_tab_hit(Some(Hit::Tab(2)), 3), Some(Hit::Tab(2)));
        assert_eq!(live_tab_hit(Some(Hit::Tab(2)), 2), None);
        assert_eq!(live_tab_hit(Some(Hit::Close(2)), 2), None);
        assert_eq!(live_tab_hit(Some(Hit::Add), 0), Some(Hit::Add));
        assert_eq!(live_tab_hit(None, 3), None);
    }
}

/// Messages in the banner, one slot per source, so one message cannot hide another
/// and a fixed problem can clear its own slot.
#[derive(Default)]
struct Banners {
    slots: std::collections::BTreeMap<&'static str, String>,
}

impl Banners {
    fn set(&mut self, slot: &'static str, text: impl Into<String>) {
        self.slots.insert(slot, text.into());
    }

    fn clear(&mut self, slot: &'static str) {
        self.slots.remove(slot);
    }

    fn text(&self) -> Option<String> {
        (!self.slots.is_empty()).then(|| self.slots.values().cloned().collect::<Vec<_>>().join("  |  "))
    }
}

#[cfg(test)]
mod banner_tests {
    use super::*;

    #[test]
    fn a_later_message_does_not_hide_an_earlier_one() {
        let mut b = Banners::default();
        b.set("settings", "Settings not loaded");
        b.set("font", "Font not installed: Monaco");
        let t = b.text().unwrap();
        assert!(t.contains("Settings not loaded") && t.contains("Monaco"), "{t}");
    }

    #[test]
    fn a_fixed_problem_clears_its_slot() {
        let mut b = Banners::default();
        b.set("font", "Font not installed");
        b.clear("font");
        assert_eq!(b.text(), None);
    }
}
