//! The Tandem window: tabs of split panes, drawn with wgpu.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
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
use crate::margins;
use crate::scrollbar;
use crate::power;
use crate::input::{self, Clicks};
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
use crate::settings_ui::{Edit, Mode, SettingsWindow};

const FONT: &str = "Cascadia Mono";
/// 12 pt, iTerm2's default size, at 96 dpi.
const FONT_PX: f32 = 16.0;
/// The side tab bar's own margin, in pixels at 100% (not the terminal margin).
const SIDE_TAB_MARGIN: f32 = 4.0;
/// How often a fading tab bar draws while it fades (60 frames a second).
const FLASH_FRAME: Duration = Duration::from_millis(16);
/// How far below the pointer a tab tooltip starts, in points (the arrow pointer's height).
const TIP_POINTER_PT: f32 = 20.0;
/// Lines scrolled per wheel notch.
const WHEEL_LINES: f32 = 3.0;
/// How long after the last settings edit the file is written.
const SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

mod actions;
mod gpu_capture;
mod paste_ui;
mod edit_session_ui;
mod history_ui;
mod shell_history_ui;
mod snippets_ui;
mod clippings_ui;
mod inline_chat_ui;
mod completion_ui;
mod pathnav_ui;
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
mod cursor_ui;
mod fonts_ui;
mod frames_ui;
mod ime_ui;
mod files_ui;
mod find_ui;
mod infobar_ui;
mod keyproto_ui;
mod keys_ui;
mod fold_ui;
mod labels_ui;
mod screenshot_ui;
mod semantic_ui;
mod url_handler_ui;
mod cmdinfo_app;
mod cmdbuttons_ui;
mod session_end_ui;
mod log_ui;
mod archive_ui;
#[cfg(windows)]
mod logout_ui;
mod session_ui;
mod seltools_ui;
mod selection_ui;
mod view_ui;
mod marks_ui;
mod offscreen_ui;
mod progress_ui;
mod sleep_ui;
mod menu;
mod porthole_ui;
mod prompted_ui;
mod triggers_ui;
mod trigger_file_ui;
mod requests_ui;
mod emu_ui;
mod jobs_ui;
mod close_ui;
mod coprocess_ui;
mod dlog_ui;
mod aps_ui;
mod windows_ui;
#[cfg(windows)]
mod chrome_ui;
mod desktops_ui;
mod launch_ui;
mod jumplist_ui;
mod default_term_ui;
mod open_file_ui;
mod presence_ui;
mod resident_ui;
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
mod session_hotkey_ui;
mod broadcast_ui;
mod tmux_ui;
mod tmux_tabs_ui;
mod tmux_dashboard_app;
mod bury_ui;
mod statusbar_ui;
mod pane_title_ui;
mod pane_move_ui;
mod pane_lock_ui;
mod statusbar_content;
mod metrics_ui;
mod git_ui;
mod toolbelt_ui;
mod tool_web_ui;
mod browser_app;
mod browser_find_app;
mod browser_copymode_app;
mod adblock_app;
mod browser_extensions_app;
mod browser_db_app;
mod bookmark_tags_app;
mod browser_pages_app;
mod browser_perms_app;
mod browser_reader_app;
mod devtools_read_app;
mod view_source_app;
mod page_saver_app;
mod browser_test_seam;
mod browser_scripts_app;
mod browser_ctx_app;
mod browser_marks_app;
mod browser_autofill_app;
mod browser_basic_auth_app;
mod browser_download_app;
mod browser_password_app;
mod browser_triggers_app;
mod browser_smart_app;
mod browser_overlay_app;
mod browser_input_app;
mod browser_frames_app;
mod browser_restore_app;
mod web_popover_app;
mod web_replay_app;
mod toolbelt_tools_ui;
mod toolbelt_more_ui;
mod pane_nav_ui;
mod open_quickly_app;
mod presets_ui;
mod profiles_open_ui;
mod bindings_ui;
mod bonjour_ui;
mod replay_ui;
mod pwm_app;
mod api_ui;
mod api_requests;
mod script_ui;
mod script_runtime_ui;
mod claude_code_app;
mod scripts_alerts;
mod tips_app;
mod update_app;
mod dynamic_profiles_app;
mod access_app;
mod webhost_app;
mod locale_ui;
mod warnings_app;
mod special_exceptions_app;
mod suppressed_app;
mod host_offers_ui;
mod host_state_app;
mod frustration_ui;
mod mouse_ui;
mod shell_version_ui;
mod prefs_ui;
mod data_files_ui;
mod settings_export_ui;
mod ai_app;
mod ai_runs;
mod ai_engage_app;
mod codecierge_app;
mod ai_explain_app;
mod chat_app;
mod chat_window_app;
mod cockpit_app;
mod orch_app;
mod orch_dispatch;
mod orch_watch_app;
mod chat_session_app;
mod transparency_ui;
mod chat_engine;
mod ai_tools_app;
mod ai_browser_app;
mod ai_exec_app;
mod ai_safety_app;
mod scripts_app;
mod script_console_app;
mod script_deps_app;
mod builtin_ui;
mod ssh_app;
mod ssh_framer_app;
mod ssh_files_app;
mod ssh_page_app;
mod ssh_panel_app;
mod ssh_url_app;
mod workgroup_app;
mod workgroup_git_app;
mod workgroup_restore_app;
mod code_review_app;
mod workgroup_toolbar_ui;
mod session_params_app;

pub enum UserEvent {
    /// A pane has output, or its shell exited.
    Wake,
    /// A Win chord from the keyboard hook.
    Chord(crate::menus::Chord),
    /// The debug log was written on its worker thread: the file's path or the reason it failed,
    /// and whether to say it stopped (`dlog_ui`).
    DebugLogSaved(Result<PathBuf, String>, bool),
    /// A menu item was clicked (its id).
    Menu(String),
    /// A notification was clicked.
    NotificationClicked,
    /// The monitors or a work area changed.
    DisplayChanged,
    /// The notification area icon was clicked (true for a right click).
    Tray(bool),
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
    /// An ssh: URL came from outside (the URL pipe): `ssh_url_app::receive_url`.
    OpenUrl(String),
    /// Another `tandem.exe` handed over a request (the launch pipe, `launch_ui`).
    Launch(crate::launch::Request),
    /// A console handed over by the Windows default-terminal handoff (`handoff_win`, `default_term_ui`).
    Handoff(crate::handoff_win::Handoff),
    /// Windows is logging off: flush the logs and archives and end the hosted shells, then set
    /// the flag the window procedure is waiting on (`logout_ui`).
    Logoff(Arc<std::sync::atomic::AtomicBool>),
    /// A screen reader asked to select text or scroll, or attached (`uia_win::Request`).
    Uia(crate::uia_win::Request),
    /// Windows asks whether the session may end (WM_QUERYENDSESSION on window `hwnd`): the app sets
    /// `end_now` (true = nothing to ask) and then `done` (`logout_ui`, `resident_ui::shutdown_decide`).
    ShutdownQuery { done: Arc<std::sync::atomic::AtomicBool>, end_now: Arc<std::sync::atomic::AtomicBool>, hwnd: isize },
    /// The shutdown was held for the quit question: ask it now, after the window procedure returned.
    ShutdownAsk,
    /// The quit question's answer (true: OK), from the thread that showed it.
    ShutdownAnswer(bool),
    /// The Save Page As worker finished a file operation for pane `pane`'s save `id` (`page_saver_app`).
    PageSaveFile { pane: PaneId, id: u64, done: page_saver_app::FileDone },
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
    /// A tmux tab's window layout as tmux last sent it: what the tab draws at when it is smaller
    /// than the area (the filler fills the rest, 13u Task 10).
    tmux_node: Option<crate::tmux_layout::Node>,
}

impl Tab {
    /// A tab with no title, not pinned and in no group.
    pub(super) fn new(id: u64, layout: Layout, focused: PaneId) -> Tab {
        Tab { id, layout, focused, zoomed: None, title: None, pinned: false, group: None, tmux_node: None }
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
    /// `anchor` is Some for a character drag, whose press the selection holds as a visual
    /// position (`Selection::set_visual_anchor`, moved with scrolling output). `command` is Some
    /// for a plain single click, whether its press removed a selection: the release may select a
    /// command (13s Task 9).
    Select { pane: PaneId, anchor: Option<(i32, usize, bool)>, scroll: Option<selection_ui::SelScroll>, scroll_off: bool, command: Option<bool> },
    /// A press on the selection waiting to become a drag of its text out of the window.
    Out(selection_ui::DragOut),
    Divider { path: Vec<bool>, axis: Axis, parent: Rect },
    /// The thumb of a pane's scrollbar, held this far below its top (pixels).
    Scrollbar { pane: PaneId, grab: f32 },
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
    /// A left press on a pane's title bar (`pane_title_ui`): the pane and where the press was
    /// (pixels). Moving far enough drags the pane (`pane_move_ui`).
    Title { pane: PaneId, from: (f32, f32) },
}

/// One wgpu instance with its adapter, device and queue.
#[derive(Clone)]
struct GpuHandles {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
}

/// The second instance (DX12 with DirectComposition swapchains) that serves only windows that need alpha.
type AlphaGpu = GpuHandles;

struct Gpu {
    surface: wgpu::Surface<'static>,
    /// The primary instance, adapter, device and queue (the first window's): every egui window, prompt
    /// and new opaque window takes its handles from here, whatever kind this window's own surface is.
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    instance: wgpu::Instance,
    /// Set on a window that needs alpha: the handles its surface and renderer were made on.
    alpha: Option<AlphaGpu>,
    /// The pointer shape last set on the window (`refresh_pointer_shape` writes it only on change).
    shape: Option<crate::pointer_shape::Shape>,
    // Dropped last, after the surface.
    window: Arc<Window>,
}

impl Gpu {
    /// The instance this window's surface was made on.
    fn render_instance(&self) -> &wgpu::Instance {
        self.alpha.as_ref().map_or(&self.instance, |a| &a.instance)
    }

    /// The device its renderer draws on.
    fn render_device(&self) -> &wgpu::Device {
        self.alpha.as_ref().map_or(&self.device, |a| &a.device)
    }

    /// The queue that presents its surface.
    /// The adapter its surface was made against.
    fn render_adapter(&self) -> &wgpu::Adapter {
        self.alpha.as_ref().map_or(&self.adapter, |a| &a.adapter)
    }

    fn render_queue(&self) -> &wgpu::Queue {
        self.alpha.as_ref().map_or(&self.queue, |a| &a.queue)
    }
}

pub struct App {
    notify: Notify,
    /// The foreground jobs of the panes' shells.
    jobs: crate::jobwatch::JobWatch,
    proxy: Arc<Mutex<EventLoopProxy<UserEvent>>>,
    menu_bar: Option<menu::MenuBar>,
    /// Keys > Remap Modifiers, read by the keyboard hook's thread.
    remap_tags: Arc<std::sync::RwLock<crate::remap::Tags>>,
    /// What the keyboard hook needs to decide which Win chords Tandem takes.
    route_snapshot: Arc<std::sync::RwLock<keyroute::Snapshot>>,
    /// Global bindings to Do Not Remap Modifiers, for the hook.
    exempt_map: Arc<std::sync::RwLock<toml::Table>>,
    globals: Globals,
    /// The workgroups (13y1): the definitions, their latches and where each came from. Read by the
    /// editor, the menu and the entry code of the later tasks.
    workgroup_model: crate::workgroup::Model,
    /// The live workgroup instances, app-wide (13y1): every member pane keeps `Pane.workgroup`.
    /// Not per window.
    workgroups: workgroup_app::Instances,
    /// `NAME=value` variables the next pane gets after the REPL's (`ITERM_WORKGROUP_ID`). Set by
    /// one workgroup spawn, read once by `session_env` and emptied; a launch override it is not
    /// (the REPL's `start_env_override` suppresses Initial Text and injection, this does not).
    start_extra_env: Vec<String>,
    /// The Leader was pressed; the next key carries the Leader flag.
    leader_on: bool,
    mru: Mru,
    next_tab_id: u64,
    /// The modifiers of a Cycle Tabs chord; letting go of them ends the cycle.
    mru_mods: Option<Mods>,
    alt_side: AltSide,
    /// Which Alt, Control and Win keys are down, left and right apart (kitty's Hyper, Meta and Super
    /// mapping), as the key readers see them: remapped in the app when Remap modifiers globally is off.
    sides: crate::keyproto::Sides,
    /// The modifiers as the keys are physically held, before an in-app remap.
    phys_mods: Mods,
    phys_sides: crate::keyproto::Sides,
    /// Force keyboard: app-wide (nothing here is per window).
    locale: crate::input_locale::Forcer,
    /// The layout last seen (or forced), to notice the person changing it.
    locale_seen: String,
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
    /// The alternate-scroll inferrer of each pane the wheel has scrolled (app-wide; the person's input only).
    alt_infer: HashMap<PaneId, crate::alt_infer::Inferrer>,
    /// iTerm2's once-per-process OSC 7 conflict flag (app-wide, never per window).
    osc7_latch: crate::emu::Osc7Latch,
    /// Each pane's escape-sequence print guard (app-wide, not swapped per window).
    print_guards: HashMap<PaneId, crate::emu::PrintGuard>,
    /// The pane that had the keyboard at the last check (a move resets its inferrer).
    alt_infer_focus: Option<PaneId>,
    /// Mark navigation: the mark each pane is at.
    mark_pos: HashMap<PaneId, u32>,
    /// Each pane's find bar and matches.
    finds: HashMap<PaneId, find_ui::FindState>,
    /// The query every find bar shares with `SynchronizeQueryWithFindPasteboard` on (iTerm2's find
    /// pasteboard; Windows has none, so it lives in Tandem). App-wide, not swapped per window.
    find_pasteboard: String,
    /// The pane that had the keyboard when the shared query was last loaded.
    find_focus_seen: Option<PaneId>,
    /// Where the focused pane's find bar and its mode label were last drawn.
    find_bar_rect: Option<Rect>,
    find_mode_rect: Option<Rect>,
    /// The old find bar's previous, next and close rectangles (`UseOldStyleDropDownViews`).
    find_old_buttons: Option<scene::OldFind>,
    /// The title bars drawn last frame (pane, geometry), for clicks (per window).
    title_bars: Vec<(PaneId, crate::pane_title::Geometry)>,
    /// A left press on a title bar button, waiting for its release.
    title_press: Option<pane_title_ui::TitlePress>,
    /// Move Session to Split Pane or Swap With Session is waiting for a pane to be picked. App-wide.
    pane_move: Option<pane_move_ui::PaneMove>,
    /// The Win-click target under the pointer while Win is held.
    hover: Option<(PaneId, crate::find::Match)>,
    /// A Win-click target is under the pointer (the hand shows even with the underline off).
    hover_hand: bool,
    /// What `update_hover` last computed for: pane, cell and the two hover keys (P2).
    hover_key: Option<(PaneId, i32, usize, bool, bool)>,
    command_click: Option<(Instant, PaneId, usize)>,
    command_info: Option<selection_ui::CommandInfo>,
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
    /// Find Globally's terminal hits for the current query, and the browser panes' hits that have
    /// arrived (13x3 Task 6); `global_generation` counts the searches, so an answer to an older one is dropped.
    global_hits: Vec<crate::global_find_ui::Hit>,
    global_web: Vec<crate::global_find_ui::Hit>,
    global_error: Option<String>,
    global_generation: u64,
    /// The Set Named Mark window, and the pane it names a mark in.
    /// The Set Named Mark prompt, its pane, and the mark it renames (the toolbelt's Edit).
    prompt: Option<(crate::prompt_ui::PromptWindow, PaneId, crate::app::marks_ui::PromptFor)>,
    /// A browser mark window asked for and not yet open (13x5): `about_to_wait` opens it in the shared
    /// `prompt` slot. `mark_prompt_focused`: the open window it waits for was already brought forward.
    pending_mark_prompt: Option<(PaneId, crate::app::browser_marks_app::MarkPrompt)>,
    mark_prompt_focused: bool,
    /// The delete confirm of a browser mark (a `ConfirmWindow`), its pane, mark and navigation epoch;
    /// and the input guard of the browser mark windows (`prompt_ui::InputGuard`, the one guard every
    /// window a page can raise uses: 1 s after it opens and again after it regains the focus).
    mark_confirm: Option<(crate::prompt_ui::ConfirmWindow, PaneId, String, Option<u64>)>,
    mark_guard: Option<crate::prompt_ui::InputGuard>,
    /// The browser's named marks, newest first, one cache per store (normal, Dev Null); app-wide.
    browser_marks: HashMap<bool, crate::browser_marks::Cache>,
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
    /// The last press was the click that activated its window, so its release, drags and hover
    /// are not reported either (`PTYMouseHandler.m:1511`); the next press clears it (13r and 13s read it).
    pub(super) first_mouse_down: bool,
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
    /// Which kind of surface this window was made with (decided at creation).
    win_alpha_surface: crate::transparency::SurfaceKind,
    /// The current window's Use Transparency toggle.
    win_transparency: crate::transparency::WinTransparency,
    /// The current window failed to become an alpha window.
    win_alpha_failed: bool,
    /// The current window follows the current desktop (Space is All Desktops).
    win_follow_desktop: bool,
    /// The monitor (primary first) the current window's profile anchors it to (`placement::anchor`).
    win_anchor: Option<usize>,
    /// DWM was last told this window's backdrop is blurred (the frame is extended); `set_blur` runs on a change.
    blur_applied: bool,
    /// The look `presence_tick` last applied to the current window.
    look_seen: Option<crate::presence::Look>,
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
    /// The Dynamic Profiles folder's watch (app-wide, 13v2 Task 6); None where there is none.
    dynamic_scan: Option<crate::dynamic_profiles_scan::Scanner>,
    /// Errors of a burst that came while an alert for the first still waited (`_pendingErrors`).
    dynamic_errors_pending: usize,
    /// Each dynamic profile's file as read (its text and what it looked like), for a write-back.
    dynamic_files: HashMap<String, dynamic_profiles_app::Source>,
    /// Rewritable dynamic profiles edited since the last write-back, and when it falls due.
    dynamic_write: BTreeSet<String>,
    dynamic_write_due: Option<Instant>,
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
    /// Sessions waiting for their `$$NAME$$` answers (13y2 Task 9e), oldest first.
    param_prompts: Vec<session_params_app::Pending>,
    /// The profile with the answers in it, set only while the answered session starts.
    param_profile: Option<Profile>,
    /// The answers each parameterised session started with, kept for its restarts (13y2 Task 9e I1).
    param_answers: HashMap<PaneId, Vec<(String, String)>>,
    /// A session is being rebuilt from an arrangement: it never prompts.
    restoring_session: bool,
    /// The hops the next pane reconnects through (Duplicate Session), moved into the new pane
    /// by `start_pane_as`. Set and cleared around one start, so it is not per-window state.
    start_jumps_override: Option<Vec<crate::ssh_jumps::Reconnection>>,
    /// Variables the next pane gets over its session variables (the REPL's cookie and key; a
    /// secret, never logged). Set and cleared around one `open_window`.
    start_env_override: Option<Vec<String>>,
    /// The tab the next pane goes into once started (a split), for its `ITERM_SESSION_ID`;
    /// None is a new tab. Set and cleared around one start, so it is not per-window state.
    start_tab: Option<usize>,
    /// What the next session is for, which picks its Advanced working directory (`start_dir`):
    /// a Tab unless `split_as`, `api_split` or `open_window_with` set it around one start.
    start_object: crate::start_dir::Object,
    /// The pane a split comes from (its folder and its variables feed the new session's
    /// directory); None is the current tab's focused pane. Set around one start with
    /// `start_object` Pane.
    start_from: Option<PaneId>,
    /// A new window's first session comes from the previously current window's focused
    /// pane: its folder and the pane itself (R3), taken before the empty window swaps in.
    window_previous_dir: Option<PathBuf>,
    window_previous_pane: Option<PaneId>,
    /// Tandem's terminfo folder, written once per run on the first WSL session that needs it
    /// (`terminfo::ensure`); app-wide.
    terminfo_dir: Option<PathBuf>,
    /// Sessions opened for a git menu command, by pane (app-wide).
    single_use: HashMap<PaneId, git_ui::SingleUse>,
    /// Undo Close: closed things waiting, and the close being collected now.
    undo: crate::undo_close::Stack<undo_ui::Undone>,
    undo_capture: Option<undo_ui::Capture>,
    /// What the archive writers' threads report (13q Task 8), collected by `archive_tick`.
    archive_results: archive_ui::Results,
    /// The Edit Window Title or Edit Tab Title window, and what it names.
    title_prompt: Option<(crate::prompt_ui::PromptWindow, windows_ui::TitleEdit)>,
    /// The New Tab Group or Rename Tab Group window, and what it names (app-wide, like the
    /// title window).
    group_prompt: Option<(crate::prompt_ui::PromptWindow, tab_groups_ui::GroupPrompt)>,
    /// Windows to open at the next turn of the event loop (new windows need it).
    pending_windows: Vec<windows_ui::NewWindow>,
    /// Tabs (by id) whose new window opens behind the active one (`TmuxWindowsOpenInBackground`).
    background_windows: std::collections::HashSet<u64>,
    /// ssh: URLs that arrived before the first window (app-wide, not in `swap_live`; m14e).
    pending_urls_at_start: Vec<String>,
    /// The launch pipe this Tandem serves, when it owns it (an idle `-Embedding` start stops it
    /// before it ends; otherwise it ends with the process).
    url_listener: Option<crate::url_launch::Serving>,
    /// The flood cap on URLs from outside (P13).
    url_flood: crate::url_launch::Flood,
    /// Requests of a second `tandem.exe` that wait for the first window, and the flood cap on
    /// them (app-wide, not in `swap_live`; m13v1).
    launch_queue: Vec<crate::launch::Request>,
    launch_flood: crate::url_launch::Flood,
    /// Tandem runs with no window (m13v1 Task 7, R3): the last window is hidden, empty and off the
    /// taskbar, the notification area icon is its way back. App-wide, not in `swap_live`.
    resident: bool,
    /// The Downloads folder chosen for the run (final review item 17). App-wide.
    downloads_cache: files_ui::DownloadsCache,
    /// Where in `pending_windows` the resident wake's one window waits (final review item 3): later
    /// tabs asked for before it opens join it. Cleared when the window is awake. App-wide.
    pending_wake: Option<usize>,
    /// When this Tandem started, and whether the person has done anything yet (a key in a
    /// session, Close, a window they closed): iTerm2's grace period for quitting (`AD:915-942`).
    launched_at: Instant,
    user_interacted: bool,
    /// The recent profiles behind the Jump List's Recents (Task 6); app-wide.
    recents: crate::jumplist::Recents,
    /// The Jump List needs a rebuild (the profiles, the arrangements or the recents changed).
    jump_list_due: bool,
    jump_sync: crate::jumplist::Sync,
    /// The default-terminal handoff class, registered while Tandem is the default (m13v1 Task 5).
    handoff: Option<crate::handoff_win::Server>,
    /// The handed-off console the next started pane takes (`spawn_pane`), and the ones that
    /// arrived before the first window.
    start_handoff: Option<crate::handoff_win::Handoff>,
    handoff_queue: Vec<crate::handoff_win::Handoff>,
    /// When an `-Embedding` start began: it makes no window until its handoff arrives.
    embedding: Option<Instant>,
    handoffs_seen: usize,
    /// Files and folders opened with Tandem: the batch gathering, the workers that check them and
    /// the scripts the person said OK to run (app-wide, not in `swap_live`; m13v1 Task 2).
    open_batch: crate::open_file::Batch,
    open_ignored: crate::open_file::IgnoredCount,
    open_work: open_file_ui::OpenWork,
    open_runs: Vec<crate::open_file::Launch>,
    /// The profile chooser for an opened `.it2triggers` file, and the files waiting for it.
    trigger_import: Option<trigger_file_ui::ImportWindow>,
    trigger_imports: std::collections::VecDeque<Vec<toml::Table>>,
    open_with_done: bool,
    /// Automatic Profile Switching: each session's stack, and this computer's name.
    aps: HashMap<PaneId, crate::aps::Switcher>,
    local_host: String,
    /// A coprocess's stderr waiting for its banner's answer: (command, text).
    coprocess_errors: HashMap<PaneId, (String, String)>,
    /// Files dropped on the window, handled together once the drop ends.
    dropped: Vec<(PaneId, String)>,
    /// Files dropped on a pane whose program registered for drops (OSC 72), collected until the
    /// event loop turn ends so one drop goes to the program whole (13o Task 9).
    dnd_dropped: Vec<(PaneId, std::path::PathBuf, (f32, f32))>,
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
    /// Session hotkeys: the ones the person set or allowed (the only ones that register) and those
    /// waiting for an answer; the sessions of each registered group, by id from
    /// `SESSION_HOTKEY_BASE`; and a pass is due to register them again. App-wide.
    session_hotkeys: crate::session_hotkeys::Book,
    session_hotkey_ids: Vec<Vec<PaneId>>,
    session_hotkeys_dirty: bool,
    /// Panes whose own session hotkey value was looked at once (`session_hotkeys_adopt`).
    session_hotkey_seen: std::collections::HashSet<PaneId>,
    /// A Tandem window was in front at the last check (Reopen on activation).
    app_active: bool,
    /// Focus follows mouse: the pointer's screen point when Tandem lost the foreground, then
    /// the point activation refuses to move focus from until the pointer moves. App-wide.
    ffm_saved: Option<(f64, f64)>,
    /// The Ctrl+wheel pinch's running total. App-wide.
    magnifier: crate::wheel::Magnifier,
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
    /// Each pane's host stack (m14e Task 4), per pane and not in `swap_live`.
    host_stacks: HashMap<PaneId, crate::host_state::Stack>,
    /// A title a pane may get back when its ssh session ended, waiting on the offer bar.
    pending_title_restores: HashMap<PaneId, Option<String>>,
    /// RequestAttention: pane flashes and fireworks (start time, and the cursor cell).
    flashes: HashMap<PaneId, Instant>,
    /// The centered bell indicator's start per pane, and whether it fades (it does not when
    /// Windows animations are off, P13).
    bell_indicators: HashMap<PaneId, (Instant, bool)>,
    /// The floors under every beep, flash and taskbar flash (B6). App-wide on purpose: three
    /// flashes a second is a limit of the person's eyes, not of one pane.
    bell_limiter: crate::bell::Limiter,
    /// The taskbar button's count: bells rung and sessions waiting while no Tandem window is
    /// active. App-wide (one count for every window's button).
    bell_badge: crate::bell_badge::Badge,
    /// The label last queued for the buttons and when; None makes the next pass queue it again.
    badge_pushed: Option<(String, Instant)>,
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
    /// The scrollbar style Windows asks for ("Always show scrollbars"), read at start and
    /// again when Windows broadcasts a settings change. App-wide: one setting, every window.
    scroll_style: scrollbar::Style,
    /// Per pane: the display offset last seen, and until when an overlay scrollbar shows after
    /// the view moved (iTerm2's overlay scroller shows for a second after a scroll).
    scroll_seen: HashMap<PaneId, (usize, Instant)>,
    /// The pane whose scrollbar the pointer is over.
    scroll_hover: Option<PaneId>,
    /// Per pane: ShowLocationsInScrollbar and ShowMarksInScrollbar as they were when it started.
    scroll_ticks: HashMap<PaneId, (bool, bool)>,
    /// Per pane: the scrollbar's ticks and the key they were built for, so the buffer is not
    /// walked every frame (`scrollbar_series`). A RefCell: drawing reads the app by `&self`.
    scroll_series: std::cell::RefCell<HashMap<PaneId, scrollbar::TickCache>>,
    /// Whether the key window showed blinking text at the last `tick_cursor` (one scan per pass).
    text_blinking: bool,
    /// The block under the pointer (pane, number, first and last line), and the drawn pill
    /// buttons.
    block_hover: Option<(PaneId, u32, i32, i32)>,
    block_buttons: Vec<blocks_ui::BlockButton>,
    /// Porthole layouts by pane and id, and the portholes drawn last frame.
    porthole_views: HashMap<(PaneId, u32), porthole_ui::PView>,
    porthole_hits: Vec<porthole_ui::PortholeHit>,
    /// The info bars, completion lists, composers and popups, toast, banner and Move Pane
    /// pick as the last frame DREW them: the draw code records each rectangle, nothing
    /// recomputes it, and the mouse holds exactly these back from the program (B5).
    overlay_drawn: Vec<(mouse::Overlay, Rect)>,
    /// The drawn I-beam cursors, built once in `resumed` and shared by every window.
    cursors: Option<crate::pointer_shape::Cursors>,
    /// The pointer entered a window and has not yet reached a terminal pane there.
    ffm_entered: bool,
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
    /// URL handler commands that finished (`url_handler_ui`), drained with the triggers' tick.
    handler_inbox: url_handler_ui::HandlerInbox,
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
    /// Window > Notify on Status Change for the current window (per window: `WinSlot`).
    status_armed: bool,
    /// The IME's composition for the current window (per window: `WinSlot`).
    ime: crate::ime::State,
    /// When the current window may draw after pane output (per window: `WinSlot`).
    pacer: crate::cadence::Pacer,
    /// The FPS meter's moving average of frame periods (per window: `WinSlot`).
    fps: crate::perf_stats::MovingAverage,
    /// Frames drawn while the FPS meter is on (per window: `WinSlot`).
    frame_number: u64,
    /// Saved renderer logs for Copy Performance Stats (13l Task 5; app-wide).
    perf: crate::perf_stats::Logs,
    /// Each window's frame counters, by window number (app-wide).
    perf_frames: std::collections::BTreeMap<usize, crate::perf_stats::FrameStats>,
    /// The drawing statistics `LogDrawingPerformance` logs (app-wide, every window).
    draw_timers: crate::perf::Timers,
    /// The last key press in any window (iTerm2's `_lastKeystrokeTime`, `UC:90-94`).
    last_keystroke: Option<Instant>,
    /// The power line (13l Task 3): app-wide, sampled when the output path asks, at most once a second.
    power: crate::power::Poll,
    last_status: HashMap<PaneId, Option<String>>,
    /// The bells on sessions (Session Status', the Cockpit's): each armed session and its status
    /// when armed; the next different status shows the alert, then it disarms
    /// (`NotifyOnStatusChangeController`).
    status_watches: std::collections::BTreeMap<PaneId, Option<String>>,
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
    /// App-wide last-use sequence; each focus change takes two numbers (`activityCounter`).
    activity_seq: u64,
    /// The pane each tab was focused on when `sync_pane_activity` last looked, by tab id (a tab
    /// keeps its id across windows, so this is not per window).
    pane_seen: HashMap<u64, PaneId>,
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
    /// The Command Info window of a Win-clicked command line; one at a time, app-wide.
    cmdinfo: Option<crate::cmdinfo_ui::CmdInfoWindow>,
    /// The prompt path navigator's workers (app-wide), the click waiting out the double-click
    /// time (when, pane, index of the path mark) and the path mark a press landed on.
    pathnav: pathnav_ui::PathNav,
    pathnav_due: Option<(Instant, PaneId, usize)>,
    pathnav_press: Option<(PaneId, usize)>,
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
    /// This window's dynamic tool web views by tool name, and what each was made for (13x Task 2;
    /// per window: `swap_live`).
    tool_views: HashMap<String, crate::webhost_win::WebView>,
    tool_loaded: HashMap<String, tool_web_ui::Loaded>,
    /// What Shell > ssh > Disconnect last showed in this window (per window: `swap_live`).
    ssh_menu_shown: Option<Option<String>>,
    /// What Shell > ssh > Download Files last showed in this window, None until set (per window:
    /// `swap_live`, P21).
    download_files_shown: Option<bool>,
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
    /// Literal writes waiting for a pane's pastes to drain (13y1 Task 7: the auto-send's Return).
    paste_after: Vec<paste_ui::LiteralAfter>,
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
    /// The Edit Bookmark window (app-wide, never swapped: one at a time), a request for it that waits
    /// for the next pass of the event loop, and whether its Delete question is open.
    bookmark_editor: Option<crate::bookmark_tag_ui::TagEditor>,
    bookmark_editor_req: Option<(PaneId, String, Option<String>)>,
    bookmark_delete_asking: bool,
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
    /// `UseOldStyleDropDownViews` as it was when this window opened (a change affects new
    /// windows only, `AS:526`).
    dropdown_views: crate::dropdown_views::Opened,
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
    /// The logoff watch is on (`logout_ui`): one setting for every window.
    logout_armed: bool,
    /// The end-of-session watch has been armed once (`logout_ui::tick_logout`).
    logout_set: bool,
    /// The shutdown query has been armed once.
    query_set: bool,
    /// Session restoration: when to save the windows, whether saving is over (quit, or the
    /// last window closed), and a focus loss waiting to be saved.
    state_debounce: crate::restore_state::Debounce,
    state_frozen: bool,
    focus_lost: bool,
    /// The shell's virtual desktop manager, made on first use (13g); `desktops_tried` is set
    /// once the attempt was made, so a machine without one is asked only once.
    desktops: Option<crate::desktops::win::Manager>,
    desktops_tried: bool,
    /// The missing-desktop notice was shown this run (iTerm2's `dispatch_once`).
    space_complained: bool,
    /// When following windows are next checked against the current desktop.
    desktop_due: Option<Instant>,
    /// A window activated on its numbered desktop, checked once `SWITCH_RECHECK` later: its HWND, the Space, when.
    switch_recheck: Option<(isize, i64, Instant)>,
    /// The failed switch was logged this run.
    switch_logged: bool,
    /// When the windows refit after a display change (`UpdateScreenParamsDelay` after the last message).
    display_due: Option<Instant>,
    /// The setting is confirmed (the warning answered); the warning shows only while it is not.
    ui_element_confirmed: bool,
    /// Tandem is acting as a UI element now (`presence::is_ui_element`).
    ui_element_now: bool,
    /// The notification area icon is added (app-wide).
    tray_on: bool,
    /// The font settings the renderer last loaded.
    font_sig: String,
    /// The faces of this window's panes (`fonts_ui::PaneFaces`; per window like the renderer).
    pane_faces: fonts_ui::PaneFaces,
    /// The blink phase (iTerm2's `blinkingItemsVisible`), shared by the cursor and blinking
    /// text; app-wide, so not in `swap_live`.
    blink: crate::cursor_anim::Blink,
    /// The next time the cursor clock needs the loop awake, set by `tick_cursor`.
    cursor_due: Option<Instant>,
    /// When the running cursor slide or smear next asks for a frame.
    anim_frame: Option<Instant>,
    /// When the next OSC 9;4 bar frame is due (`tick_progress`); panes are shared by every
    /// window, so app-wide and not in `swap_live`.
    progress_frame: Option<Instant>,
    /// Prevent Sleep (13p): the one request, what the last look saw, and the Session tab's line.
    sleep: crate::sleep_guard::Guard,
    sleep_tracker: crate::sleep_guard::Tracker,
    sleep_status: String,
    /// The focused pane's smooth-blink fade, advanced by `tick_cursor`.
    blink_fade: crate::cursor_anim::Fade,
    /// The smooth blink's alpha for the key window's focused pane, stored by `tick_cursor`
    /// (`Fade::alpha` needs `&mut`, drawing is `&self`); None when smooth blink is off.
    cursor_alpha: Option<f32>,
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
    /// Remote files being fetched for browser views, by the transfer engine's job (app-wide, 13x8).
    ssh_page_jobs: HashMap<u64, crate::ssh_page::PageJob>,
    /// The `iterm2-ssh:` URL the new browser pane loads first: set only by the person's View answer
    /// and taken by `start_browser_pane` in the same call.
    ssh_view_url: Option<String>,
    /// What waits for each file request, by pane and the id Tandem sent it with (app-wide,
    /// not in `swap_live`; 14d).
    ssh_waiters: HashMap<(PaneId, u64), ssh_files_app::Waiter>,
    /// The id of the next file request.
    ssh_next_file_id: u64,
    /// The Composer's remote suggestion answers, by pane (app-wide, not in `swap_live`; 14d).
    ssh_suggest_cache: HashMap<PaneId, crate::ssh_suggest::Cache>,
    /// When the cache is purged next (at most every 10 s).
    ssh_purge_at: Option<Instant>,
    /// The panel's running search: its pane, the id the remote gave it (empty until it
    /// answers) and the panel's generation (app-wide; 14d).
    ssh_search: Option<(PaneId, String, u64)>,
    /// The ids of the searches Tandem started lately, by pane: results of a replaced search are
    /// still acknowledged (P18), and no other id is.
    ssh_search_issued: Vec<(PaneId, String)>,
    /// The search acknowledgements still out, by pane: a result past 16 is dropped (P18).
    ssh_search_acks: HashMap<PaneId, crate::ssh_suggest::AckWindow>,
    /// The transfer engine's jobs (app-wide, not in `swap_live`; 14d).
    ssh_transfers: crate::ssh_transfer::Transfers,
    /// The local side of each transfer job: its temp file and what its banner shows (Task 4).
    ssh_locals: HashMap<u64, crate::ssh_local::Local>,
    /// Folder unzips running on worker threads, each answering (job, result) once.
    ssh_unzips: Vec<crate::ssh_transfer::UnzipWait>,
    /// Download Files items waiting for an earlier item of their name to land (app-wide, P12).
    ssh_landings: ssh_panel_app::Landings,
    /// The remote file panel (Shell > ssh > Download Files) and its model; one at a time (app-wide,
    /// not in `swap_live`; 14d).
    ssh_panel: Option<(crate::ssh_panel_ui::SshPanelWindow, crate::ssh_panel::Panel)>,
    /// Download Files' batches still out, by id (app-wide; 14d).
    ssh_batches: HashMap<u64, ssh_panel_app::Batch>,
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
    /// The connections that are cc-status hooks, each with the cookie it presented (ruling P14):
    /// they may set a status only on the pane that cookie belongs to.
    api_hooks: HashMap<u64, Option<String>>,
    /// Running invocations by id (app-wide, as every script field is), the next run id and
    /// call token, and the run each started call's token belongs to.
    script_runs: BTreeMap<u64, script_ui::Run>,
    script_next: u64,
    /// Expression bindings per pane and bound key (`bindings_ui`); app-wide, so no `swap_live`.
    bindings: HashMap<(PaneId, String), bindings_ui::Bound>,
    /// Bonjour hosts as profiles (`bonjour_ui`): the browser while `EnableRendezvous` is on, the
    /// hosts live now, and the state last synced; app-wide, so no `swap_live`.
    bonjour: Option<crate::bonjour::Browser>,
    bonjour_live: Vec<crate::bonjour::Live>,
    bonjour_wanted: Option<bool>,
    /// The active pane `follow_edit_session` last saw; app-wide, so no `swap_live`.
    edit_follow_seen: Option<PaneId>,
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
    /// Open site-permission questions and the tick's requests (app-wide, 13x6).
    perm_asks: browser_perms_app::Asks,
    /// The web notification on the tray balloon, and when each granted origin last showed one (13x6).
    web_note: crate::browser_perms::NoteScreen,
    /// The pane and origin the open toolbar menu's site items were built for (`representedObject`, `BT:397`).
    perm_menu: Option<(PaneId, String)>,
    web_note_limits: crate::triggers::RateLimit<String>,
    /// The one ordered queue of profile permission writes of each browser store (`[Browser,
    /// BrowserDevNull]`, 13x6 Task 5 fix): every mirror, sync, revoke and reset goes through it.
    perm_queues: [crate::perm_queue::ProfileQueue; 2],
    scripts_alert: Option<(crate::prompt_ui::ConfirmWindow, scripts_alerts::Ask)>,
    /// Checking for updates (M13v2): the worker, the window, the next scheduled check, whether the
    /// check in flight is the person's, and the view waiting to show. App-wide, never per window.
    update_checker: Option<crate::update_fetch_win::Checker>,
    update_window: Option<crate::update_ui::UpdateWindow>,
    update_due: Option<Instant>,
    update_manual: bool,
    update_show: Option<crate::update_ui::View>,
    /// The waiting view comes from a manual check (it opens while resident too).
    update_show_manual: bool,
    /// The Tip of the Day (M13v2): the card, the one deadline for the next try, and whether the
    /// launch started the schedule. App-wide, never per window.
    tip_window: Option<crate::tip_ui::TipWindow>,
    tip_due: Option<Instant>,
    tips_started: bool,
    /// Screen readers (M13v2 Task 10): the UI Automation host of each window (by registry key),
    /// what is kept of each pane while a client listens, the one settle deadline, the window
    /// and focus last published, and whether an Attach is already posted. App-wide.
    uia_hosts: HashMap<u64, crate::uia_win::Host>,
    access_trackers: HashMap<PaneId, access_app::AccessPane>,
    access_due: Option<Instant>,
    /// When the snapshot was last built, and the deadline a held-back build waits for (one build a frame).
    access_built: Option<Instant>,
    access_hold: Option<Instant>,
    access_window: Option<access_app::AccessWindow>,
    access_focus_sent: Option<(u64, PaneId)>,
    access_attach: Arc<std::sync::atomic::AtomicBool>,
    /// Locale questions waiting for the picker: the start-time prompts, and Settings' Change
    /// Locale requests (`locale_ui`).
    locale_asks: VecDeque<locale_ui::Ask>,
    locale_window: Option<locale_ui::PickerWindow>,
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
    /// The Manage Special Exceptions window (app-wide, one at a time).
    special_exceptions: Option<special_exceptions_app::Open>,
    suppressed_retitle_due: Option<Instant>,
    /// The AI alert queue (ruling 12c-7), the alert showing, and the Settings > General > AI
    /// sub-tab an answer asked to reveal.
    ai_asks: VecDeque<crate::ai_gate::Alert>,
    ai_alert: Option<(crate::prompt_ui::ConfirmWindow, crate::ai_gate::Alert)>,
    ai_reveal: Option<crate::settings_ai::SubTab>,
    /// The one-vendor key prompts waiting to show (ruling 12e-1), and the one showing.
    ai_key_asks: VecDeque<(crate::ai_settings::Vendor, ai_app::KeyThen)>,
    autofill: browser_autofill_app::AutofillState,
    /// HTTP authentication state per browser pane (app-wide, keyed by pane; not swapped).
    basic_auth: HashMap<PaneId, browser_basic_auth_app::AuthPane>,
    /// Browser downloads by id (app-wide, never swapped) and the last id.
    browser_downloads: HashMap<u64, crate::browser_download::Download>,
    download_seq: u64,
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
    /// The browser's history and bookmarks (app-wide, never swapped): the persistent database,
    /// opened on first use; whether opening it failed (then it is not tried again and history
    /// lives in `browser_db_mem` for the run); Dev Null Mode's in-memory database.
    browser_db: Option<crate::browser_db::Db>,
    browser_db_failed: bool,
    browser_db_mem: Option<crate::browser_db::Db>,
    /// The cleanup of an earlier run's chat links failed: no chat binds to a pane this run.
    chat_links_off: bool,
    chat_broker: crate::chat_broker::Broker,
    /// The AI Chat window (app-wide, never swapped: one per app), the broker subscription it
    /// reads every chat through, the chat it shows, and its toolbar as last computed (rebuilt
    /// only when `chat_toolbar_dirty`, so a streamed chunk never re-reads the providers).
    chat_window: Option<crate::chat_window_ui::ChatWindow>,
    /// The Cockpit (app-wide, never swapped: one per app), what it keeps between redraws
    /// (kept when the window closes), and when its frame or split is next written out.
    cockpit: Option<crate::cockpit_ui::CockpitWindow>,
    cockpit_view: crate::cockpit::View,
    cockpit_save_due: Option<Instant>,
    /// The Claude Code setup window (app-wide, one at a time), whether this launch's health
    /// check ran, whether the announcement was put off for this run ("Ask Later"), and a
    /// setup wanted from an answer that has no event loop (the next pass opens it).
    cc_onboarding: Option<crate::cc_onboarding_ui::OnboardingWindow>,
    claude_health_done: bool,
    claude_watcher_off: bool,
    claude_install_due: bool,
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
    /// The selected command's buttons from the last frame (this window's; `swap_live`).
    cmd_button_hits: Vec<(PaneId, Rect, crate::cmdbuttons::Button)>,
    /// The pane, mark and button whose menu is open (the pick arrives later).
    cmd_button_menu: Option<(PaneId, u32, crate::cmdbuttons::Button)>,
    /// The offscreen command lines this window drew: pane, band, line to reveal (13p; in `WinSlot`).
    offscreen_hits: Vec<(PaneId, Rect, i32)>,
    /// Each visible pane's prompt facts for this frame (`prepare_offscreen`); scratch, not swapped.
    offscreen_prepared: HashMap<PaneId, Vec<crate::offscreen::PromptFacts>>,
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
    /// The browser tool calls waiting by token (13x AI browser tools), app-wide.
    ai_browser: ai_browser_app::Calls,
    /// What a model said about the session it started (provenance, 12f Task 9), by pane.
    orch_provenance: HashMap<PaneId, String>,
    /// Orchestration requests parked on the person: request id to (chat id, call id).
    orch_parks: crate::orch::Parks,
    /// What each parked orchestration request runs on its answer (12f Task 10), by request id.
    orch_waits: HashMap<String, orch_dispatch::Wait>,
    /// Scopes the person refused per chat (asked again only once claimed), in memory.
    orch_denied: HashMap<String, Vec<String>>,
    /// (chat id, call id) of `session_*` mirror calls running through `ai_run_tool`, with their
    /// pane and start; `ai_tool_finished` hands their results to `orch_resume`.
    orch_mirrors: HashMap<(String, String), (PaneId, Instant)>,
    /// CRs due 75 ms after an orchestration bracketed paste.
    orch_pending_cr: Vec<orch_dispatch::PendingCr>,
    /// Running orchestration safety checks.
    orch_checks: Vec<orch_dispatch::Check>,
    /// Allowed start_session calls waiting for the event loop.
    orch_spawns: Vec<orch_dispatch::Spawn>,
    /// The sessions each chat's agent started, and when (ruling P15's cap).
    orch_spawned: HashMap<String, Vec<(Instant, PaneId)>>,
    /// Each chat's watchers and timers (Task 11); in memory only, a launch starts empty (ruling P2).
    orch_watchers: HashMap<String, Vec<crate::orch_watch::Watcher>>,
    /// What the agent submitted into each pane since its last prompt, so a split command is
    /// classified whole (Task 10 fix round 1).
    orch_held: HashMap<PaneId, crate::orch_gate::Held>,
    /// Panes whose Restart end action restarts them at the given time (13q); app-wide, a pane
    /// closed meanwhile is dropped in `close_pane`.
    pending_restarts: HashMap<PaneId, Instant>,
    /// When each pane was last restarted by its end action (the bound of `session_end::restart_allowed`).
    restart_history: HashMap<PaneId, Vec<Instant>>,
    /// A screen watch's poll, by watcher id.
    orch_polls: HashMap<String, crate::orch_watch::Poll>,
    /// The events each chat is about to receive, held for the gate and the rate.
    orch_outbox: HashMap<String, crate::orch_watch::Outbox>,
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
    /// The last notification was "Debug logging turned on": a click on it turns logging off.
    last_notified_debug: Option<u64>,
    /// The pane the debug log last named as focused (app-wide: a window switch changes it too).
    focus_logged: Option<PaneId>,
    /// The automatic-start notification shows once per launch (`AD:1309-1324`).
    debug_notice_shown: bool,
    /// Status bar strings with function calls, by session and string: their shown values
    /// and running evaluations (app-wide; a pane's entries go with the pane).
    script_templates: HashMap<(PaneId, String), script_ui::Template>,
    /// The last bar layout pass that asked for status bar strings (`begin_script_pass`).
    script_pass: u64,
    /// Each pane's latest Initial Text generation (`bump_initial_text`): a late answer of an older start is dropped.
    initial_text_gen: HashMap<PaneId, u64>,
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
    /// The HTML popover or Quick Look open now (app-wide, one at a time) and the session whose scope its bridge runs in (13x Task 3).
    web_popover: Option<(Option<PaneId>, crate::web_popover::WebPopover)>,
    /// The hovered link or toast above a browser page (13x2 Task 5): one small popup, app-wide.
    browser_tip: Option<crate::browser_tip::TipWindow>,
    /// The newest hover or toast waiting for the next `web_tick` (it needs the event loop).
    browser_tip_next: Option<browser_overlay_app::TipAsk>,
    /// The tip window's life: made once, hidden and reused, rate-limited.
    browser_tip_life: crate::browser_tip::Lifecycle,
    /// The browser Instant Replay playback window (one at a time, app-wide like the popovers) and the
    /// movie save in flight.
    web_replay: Option<crate::web_replay_ui::PlaybackWindow>,
    /// The browser pane whose frames the playback window shows (Clear closes it).
    web_replay_pane: Option<PaneId>,
    web_replay_save: Option<web_replay_app::Saving>,
    /// Chat icon generations by chat id (app-wide, 13x Task 4): coalescing state, and the candidates being rendered.
    icon_requests: HashMap<String, crate::svg_icon::Request>,
    icon_jobs: HashMap<String, chat_window_app::IconJob>,
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
    /// The second wgpu instance for see-through windows, made on first need (app-wide).
    alpha_gpu: Option<AlphaGpu>,
    /// No DX12 adapter answered: every window draws opaque (a banner says so once).
    transparency_unavailable: bool,
    /// Windows whose surface kind no longer fits their profiles, recreated on the next pass.
    alpha_pending: Vec<WindowId>,
    /// The unavailable banner was shown.
    transparency_banner: bool,
    /// The frame just built has a see-through pane (the surface wants a premultiplied mode).
    frame_alpha: bool,
    /// A present failed during a hotkey window's fade: fades are off for the rest of the run (app-wide).
    fade_failed: bool,
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
    /// The WebView2 host (13x Task 1): the environments and the runtime check, and what the views
    /// post. App-wide, never per window.
    web_envs: webhost_app::Envs,
    web_events: std::sync::mpsc::Receiver<crate::webhost::Posted>,
    /// The workers that read `iterm2-file:` pages' files (13x8 Task 3), started on the first request.
    /// App-wide, never per window.
    file_pool: Option<crate::file_serve::Pool>,
    /// The window whose keyboard focus sits in one of its own web views (13x Task 1a): the parent saw
    /// `Focused(false)` but is still the foreground window. App-wide, never per window.
    web_focus_held: Option<WindowId>,
    /// The last web view that reported gaining the keyboard (any kind), until it reports losing it.
    /// App-wide, never per window.
    web_focus_view: Option<crate::webhost::ViewId>,
    /// The tool views of every window, by view id, with whether the view failed (13x Task 2).
    /// App-wide: an event names a view, not a window.
    tool_view_ids: HashMap<crate::webhost::ViewId, bool>,
    /// Each dynamic tool's remake budget after a failed view (app-wide, by tool name; final review I5).
    tool_restarts: HashMap<String, crate::webhost::Restarts>,
    /// The web view of every browser pane (13x Task 5). App-wide: keyed by pane, and a view is
    /// re-parented when its pane changes windows.
    browser_views: HashMap<PaneId, browser_app::BrowserView>,
    /// The Save Page As job of each browser pane (13x4 Task 10). App-wide: pane ids are.
    page_saves: HashMap<PaneId, page_saver_app::Saving>,
    /// The last save's number, so a cancelled save's worker events are ignored.
    page_save_seq: u64,
    /// Commands a person confirmed from a page's Smart Selection action, each run in a single-use
    /// window by the next tick (the answer has no event loop). App-wide: pane ids are.
    browser_runs: Vec<(PaneId, String)>,
    /// What a page pointer press or its answer asks that needs the event loop (opens, Smart Selection
    /// actions, the session's own pointer actions), run by the next tick. App-wide.
    browser_next: Vec<browser_input_app::PageNext>,
    /// The ad blocker: rules, the list download and its schedule (13x Task 8; app-wide).
    adblock: adblock_app::Adblock,
    /// Browser extensions: this run's folder map, the sync and the approval questions (13x8 Task 10; app-wide).
    browser_ext: browser_extensions_app::Extensions,
    /// The pane that was focused at the last placement, to tell a pane change from none.
    focus_seen: crate::browser::FocusSeen<WindowId>,
    /// The active pane at the last focus-report tick, per window (13y2 Task 9d).
    report_seen: crate::browser::FocusSeen<WindowId>,
    /// iTerm2's `_focused` per pane: the last focus state each pane was told (app-wide).
    focus_reported: crate::webhost::FocusLedger,
    /// The URL the next browser pane starts at (an arrangement's, or a page's new tab).
    browser_url_override: Option<String>,
    /// The launch is applying the saved windows: a browser pane makes its view but loads its page
    /// only when this ends (`browser_restoration_done`). Not per window: no `swap_live` entry.
    restoring_windows: bool,
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

/// What ends with the process, on EVERY way out of it (Quit, Erase All, a logoff, a panic on the
/// main thread): the keyboard hook and its hotkeys come off (a held remapped key is released),
/// the person's keyboard layout comes back, and the notification area icon goes. A crash or kill
/// needs nothing of it that Windows does not do (the hook, hotkeys and the per-thread layout end
/// with the process; the icon lingers until hovered). It runs on the UI thread, and twice is safe.
pub(crate) fn teardown_process() {
    crate::dlog::stop_at_exit();
    crate::winkeys::uninstall();
    crate::input_locale::restore_after_panic();
    crate::notify::remove(0);
    crate::handoff_win::begin_closing();
    // A running update check ends with Tandem (its curl.exe), not after it.
    #[cfg(windows)]
    crate::update_fetch_win::cancel_running();
    // The Dynamic Profiles folder's watch stops with Tandem.
    #[cfg(windows)]
    crate::dynamic_profiles_scan::stop_all();
    // Every screen reader provider is disconnected, so no client keeps a dead one (P14).
    #[cfg(windows)]
    crate::uia_win::disconnect_all();
    // The overlay icons come off the taskbar buttons (the badge is the app's, not the shell's).
    #[cfg(windows)]
    crate::taskbar_win::clear_all();
    // Every web view closes (its browser process lets go of its folder), then this run's Dev Null
    // browser folder goes with it (file removal).
    #[cfg(windows)]
    let _ = crate::webhost_win::close_all();
    #[cfg(windows)]
    crate::webhost_win::cleanup_files();
}

pub fn run() {
    crate::dlog::mark_main_thread();
    // A cookie in Tandem's own environment (Tandem started from a Tandem shell) must reach no
    // session and no child: only a cookie Tandem mints for a session is ever set.
    for name in crate::session_env::NEVER_INHERITED {
        // SAFETY: the first thing Tandem does, before any thread (the event loop, tandem-host,
        // the API server) exists, so nothing reads the environment meanwhile.
        unsafe { std::env::remove_var(name) };
    }
    // An ssh: URL from a protocol handler (m14e): a running Tandem of this user takes it and this
    // process ends; a refused command line opens nothing. Before any window or thread exists.
    let mut pending_urls_at_start = Vec::new();
    match crate::url_launch::url_argument(&std::env::args_os().collect::<Vec<_>>()) {
        crate::url_launch::UrlArg::None => {}
        crate::url_launch::UrlArg::Refused(why) => return crate::host::log(&format!("url: refused {why}")),
        // Only when no Tandem of this user runs does this one start (fix r1): a busy or failed
        // hand-over never opens a second Tandem beside the first.
        crate::url_launch::UrlArg::Url(url) => match crate::url_launch::hand_to_running(&crate::launch::Request::Url { url: url.clone() }) {
            crate::url_launch::AfterHand::Exit => return,
            crate::url_launch::AfterHand::Start => pending_urls_at_start.push(url),
            crate::url_launch::AfterHand::Refuse(why) => return crate::host::log(&format!("url: refused: {why}")),
        },
    }
    // Started again after Import All Settings and Data: the old Tandem ends first (`IE:898-911`).
    let wait_pid = std::env::var(settings_export_ui::WAIT_PID_VAR).ok().and_then(|v| v.parse::<u32>().ok());
    // SAFETY: as above, before any thread exists; no session or child inherits the variable.
    unsafe { std::env::remove_var(settings_export_ui::WAIT_PID_VAR) };
    if let Some(pid) = wait_pid {
        settings_export_ui::wait_for_pid(pid);
    }
    // The rest of the command line (m13v1): a running Tandem of this user takes the request over
    // the same pipe and this process ends (after the wait above, so the relaunch after an import
    // never hands its start to the Tandem it replaced). With none running, this one starts and
    // runs the request after its first window.
    let mut launch_queue = Vec::new();
    let mut embedding = false;
    let request = match std::env::args_os().skip(1).map(|a| a.into_string()).collect::<Result<Vec<String>, _>>() {
        Ok(args) => crate::launch::parse(&args, &std::env::current_dir().unwrap_or_default()),
        Err(_) => Err(crate::launch::USAGE.to_string()),
    };
    match request {
        Err(usage) => return message_box(&usage),
        // Handled above (its hand-over and its queue are the URL's own).
        Ok(crate::launch::Request::Url { .. }) => {}
        // COM starts `tandem.exe -Embedding` for one handoff; any local program can too. It runs
        // only while Tandem is the default terminal with its pack and no Tandem answers the
        // launch pipe; it forwards nothing and makes no window until a handoff arrives (B9).
        Ok(crate::launch::Request::Embedding) => {
            let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
            let pack = conpty::load(&Source::Shipped(exe)).is_ok_and(|a| a.pack.is_some());
            let tandem = crate::default_term::is_tandem(&crate::default_term::current(&crate::reg::HkcuRegistry));
            match crate::default_term::embedding_start(tandem, pack, &crate::url_launch::probe_running()) {
                crate::default_term::EmbeddingStart::Run => embedding = true,
                crate::default_term::EmbeddingStart::Exit(why) => return crate::host::log(&format!("embedding: not started: {why}")),
            }
        }
        Ok(request) => {
            if crate::launch::forwardable_start(&request, wait_pid.is_some()) {
                match crate::url_launch::hand_to_running(&request) {
                    crate::url_launch::AfterHand::Exit => return,
                    crate::url_launch::AfterHand::Start => {}
                    crate::url_launch::AfterHand::Refuse(why) => return crate::host::log(&format!("launch: refused: {why}")),
                }
            }
            // A bare start is the startup plan's own window: it has nothing more to do.
            if request != crate::launch::Request::Reopen {
                launch_queue.push(request);
            }
        }
    }
    // A panic on the main thread (it ends Tandem) takes the keyboard hook down first, so no remap
    // outlives it; a worker thread's panic leaves the hook alone.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("main") {
            teardown_process();
        }
        previous_hook(info);
    }));
    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(l) => l,
        Err(e) => return message_box(&format!("Could not start the event loop: {e}")),
    };
    let proxy = Arc::new(Mutex::new(event_loop.create_proxy()));
    let clicks = proxy.clone();
    crate::notify::on_click(Box::new(move || {
        let _ = clicks.lock().map(|p| p.send_event(UserEvent::NotificationClicked));
    }));
    let trays = proxy.clone();
    crate::notify::on_tray(Box::new(move |right| {
        let _ = trays.lock().map(|p| p.send_event(UserEvent::Tray(right)));
    }));
    let displays = proxy.clone();
    crate::display_watch::on_change(Box::new(move || {
        let _ = displays.lock().map(|p| p.send_event(UserEvent::DisplayChanged));
    }));
    let job_wake = proxy.clone();
    let jobs = crate::jobwatch::JobWatch::start(Arc::new(move || {
        let _ = job_wake.lock().map(|p| p.send_event(UserEvent::JobsChanged));
    }));
    // This Tandem takes the requests sent to it. One that cannot bind is a second Tandem of the
    // same user (another started at the same moment, or something holds the name): it hands its
    // own request over once more and ends, so it never installs a second keyboard hook, hotkeys
    // or tray icon beside the first (13u S4, R14de-6).
    let mut pipe_problem: Option<String> = None;
    let url_listener = match crate::url_launch::listen(launch_events(proxy.clone())) {
        Ok(h) => Some(h),
        Err(e) => {
            crate::host::log(&format!("launch: not listening: {e}"));
            if embedding {
                return;
            }
            let mut again = pending_urls_at_start.iter().map(|u| crate::launch::Request::Url { url: u.clone() }).chain(launch_queue.iter().cloned()).collect::<Vec<_>>();
            if again.is_empty() {
                again.push(crate::launch::Request::Reopen);
            }
            let after: Vec<_> = again.iter().map(crate::url_launch::hand_to_running).collect();
            // A holder that is not this Tandem (another user, a lower integrity level, another
            // program) is sent nothing and does not stop Tandem from starting: this one runs
            // without a listener and says so (review r1, Important 1). Only a holder that may
            // be Tandem but did not answer ends the launch.
            match crate::url_launch::lost_pipe(&after, false) {
                crate::url_launch::LostPipe::Handed => return,
                crate::url_launch::LostPipe::Refused => {
                    message_box(&format!("Tandem could not take its launch pipe: {e}"));
                    return;
                }
                crate::url_launch::LostPipe::RunWithout => {}
            }
            pipe_problem = Some("Another program holds Tandem's launch pipe, so files and links opened with Tandem cannot reach this window.".to_string());
            None
        }
    };
    let wake = proxy.clone();
    let notify: Notify = Arc::new(move || {
        let _ = wake.lock().unwrap().send_event(UserEvent::Wake);
    });

    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
    let mut banners = Banners::default();
    if let Some(text) = pipe_problem {
        banners.set("launch-pipe", text);
    }
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
    // Read once at launch (its help says a restart is needed).
    let (prefs_saved, prefs_startup) = prefs_ui::load_at_start(&mut globals, &mut store, &prefs_ui::curl_fetch);
    let trigger_pool_width = globals.int("MaximumNumberOfTriggerCommands");
    // iTerm2 drops bindings whose profile is gone when it loads them (P11).
    if crate::url_schemes::prune_globals(&mut globals, &store) {
        if let Err(e) = globals.save() {
            banners.set("globals", format!("Could not save settings: {e}"));
        }
    }
    // Tandem's own scheme registrations follow the exe when it moved (only schemes the person bound).
    crate::url_schemes::repair_at_start(&globals);
    // A crash mid-download leaves a hidden `.Tandem.*` temp: older ones are swept with the
    // launch's other housekeeping (P24), off the launch path.
    let downloads_setting = globals.string("DownloadsDirectory");
    std::thread::spawn(move || {
        let now = std::time::SystemTime::now();
        if let Some(d) = files_ui::downloads(&downloads_setting).dir {
            crate::ssh_local::sweep_stale(&d, now);
        }
        crate::ssh_local::sweep_stale(&std::env::temp_dir(), now);
    });
    // So does the default-terminal handoff class (P11), under the same rule.
    crate::default_term::repair_at_start();
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
    // The workgroups, with their one-time migrations; written back only when something changed.
    let (mut workgroup_model, workgroups_changed) = crate::workgroup::Model::load(&globals);
    if workgroups_changed {
        workgroup_model.save(&mut globals);
        if let Err(e) = globals.save() {
            banners.set("globals", format!("Could not save settings: {e}"));
        }
    }
    // The Script Console starts with the Automatic Profile Switching entry while its logging
    // is on (`SH:340-351`).
    let aps_logging = globals.bool(aps_ui::LOG_KEY);
    // The first window's tab bar takes SelectsTabsOnMouseDown now; later windows at opening.
    let tab_selects_on_down = globals.bool("SelectsTabsOnMouseDown");
    let mut dropdown_views = crate::dropdown_views::Opened::default();
    dropdown_views.open(globals.bool("UseOldStyleDropDownViews"));
    jobs.set_fast_updates(globals.bool("FastForegroundJobUpdates"));
    jobs.set_ancestry_diagnostics(globals.bool("LogForegroundJobAncestryDiagnostics"));
    let side_bar_preferred = side_bar_default(&globals);
    let min_tab_drag = globals.int("MinimumTabDragDistance") as f32;
    // Session restoration: shells run in tandem-host so they outlive Tandem (read once:
    // a change needs a restart, as in iTerm2).
    // A second Tandem (the host serves the first) neither restores nor saves windows: the
    // saved state belongs to the first.
    let mut second_copy = false;
    let (host, host_welcome) = if globals.bool("RunJobsInServers") {
        let wait = std::time::Duration::from_secs_f64(globals.get("TimeoutForDaemonAttachment").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(10.0).clamp(1.0, 120.0));
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
    let open_work = open_file_ui::OpenWork::new(notify.clone());
    let pathnav = pathnav_ui::PathNav::new(notify.clone());
    let composer_worker = crate::composer_worker::Worker::new(notify.clone(), Arc::new(crate::composer_worker::RealFs));
    let ui_element_confirmed = globals.bool(crate::presence::UI_ELEMENT_KEY);
    let cockpit_view = cockpit_app::initial_view(&globals);
    let (web_envs, web_events) = webhost_app::Envs::seeded(adblock_app::proxy_args_of(&globals), globals.bool("WebKitAdblockEnabled"));
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
        session_hotkeys: Default::default(),
        session_hotkey_ids: Vec::new(),
        session_hotkeys_dirty: false,
        session_hotkey_seen: Default::default(),
        app_active: false,
        ffm_saved: None,
        magnifier: Default::default(),
        app_hidden: Vec::new(),
        host_welcome,
        notify,
        jobs,
        proxy,
        menu_bar: None,
        remap_tags: Arc::new(std::sync::RwLock::new(crate::remap::Tags::from_globals(&globals))),
        route_snapshot: Arc::new(std::sync::RwLock::new(keyroute::Snapshot::default())),
        exempt_map: Arc::new(std::sync::RwLock::new(toml::Table::new())),
        globals,
        workgroup_model,
        workgroups: Default::default(),
        start_extra_env: Vec::new(),
        leader_on: false,
        mru: Mru::default(),
        next_tab_id: 1,
        mru_mods: None,
        alt_side: AltSide::Left,
        sides: crate::keyproto::Sides::default(),
        phys_mods: Mods::default(),
        phys_sides: crate::keyproto::Sides::default(),
        locale: Default::default(),
        locale_seen: String::new(),
        pointer_clicks: pointer::Clicks::default(),
        pointer_last: None,
        pointer_down: None,
        mouse_override: HashMap::new(),
        alt_click: None,
        key_click: None,
        alt_infer: HashMap::new(),
        osc7_latch: crate::emu::Osc7Latch::default(),
        print_guards: HashMap::new(),
        alt_infer_focus: None,
        mark_pos: HashMap::new(),
        finds: HashMap::new(),
        find_pasteboard: String::new(),
        find_focus_seen: None,
        find_bar_rect: None,
        find_mode_rect: None,
        find_old_buttons: None,
        title_bars: Vec::new(),
        title_press: None,
        pane_move: None,
        hover: None,
        hover_hand: false,
        hover_key: None,
        command_click: None,
        command_info: None,
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
        pending_mark_prompt: None,
        mark_prompt_focused: false,
        mark_confirm: None,
        mark_guard: None,
        browser_marks: HashMap::new(),
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
        first_mouse_down: false,
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
        belt: Default::default(),
        tool_gen: 0,
        belt_keys: Default::default(),
        composer_keys: Default::default(),
        composers: HashMap::new(),
        composer_worker,
        scripts_scan: None,
        dynamic_scan: None,
        dynamic_errors_pending: 0,
        dynamic_files: HashMap::new(),
        dynamic_write: BTreeSet::new(),
        dynamic_write_due: None,
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
        param_prompts: Vec::new(),
        param_profile: None,
        param_answers: HashMap::new(),
        restoring_session: false,
        start_jumps_override: None,
        start_env_override: None,
        start_tab: None,
        start_object: crate::start_dir::Object::Tab,
        start_from: None,
        window_previous_dir: None,
        window_previous_pane: None,
        terminfo_dir: None,
        single_use: HashMap::new(),
        undo: Default::default(),
        undo_capture: None,
        archive_results: Default::default(),
        pending_windows: Vec::new(),
        background_windows: Default::default(),
        pending_urls_at_start,
        url_listener,
        url_flood: Default::default(),
        launch_queue,
        launch_flood: Default::default(),
        resident: false,
        downloads_cache: Default::default(),
        pending_wake: None,
        launched_at: Instant::now(),
        user_interacted: false,
        recents: Default::default(),
        jump_list_due: false,
        jump_sync: Default::default(),
        handoff: None,
        start_handoff: None,
        handoff_queue: Vec::new(),
        embedding: embedding.then(Instant::now),
        handoffs_seen: 0,
        open_batch: Default::default(),
        open_ignored: Default::default(),
        open_work,
        open_runs: Vec::new(),
        trigger_import: None,
        trigger_imports: std::collections::VecDeque::new(),
        open_with_done: false,
        local_host: aps_ui::local_host(),
        dropped: Vec::new(),
        dnd_dropped: Vec::new(),
        coprocess_errors: HashMap::new(),
        global_find: None,
        global_hits: Vec::new(),
        global_web: Vec::new(),
        global_error: None,
        global_generation: 1,
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
        host_stacks: HashMap::new(),
        pending_title_restores: HashMap::new(),
        pending_reads: HashMap::new(),
        pending_properties: HashMap::new(),
        pending_urls: HashMap::new(),
        print_setup: crate::winprint::Setup::default(),
        shot: None,
        replay_window: None,
        replay_seen: None,
        pwm_window: None,
        tool_views: HashMap::new(),
        tool_loaded: HashMap::new(),
        ssh_menu_shown: None,
        download_files_shown: None,
        pwm: Default::default(),
        replay_keys: Default::default(),
        recording_warnings: Vec::new(),
        recording_replay: None,
        block_hover: None,
        block_buttons: Vec::new(),
        porthole_views: HashMap::new(),
        porthole_hits: Vec::new(),
        overlay_drawn: Vec::new(),
        cursors: None,
        ffm_entered: false,
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
        trigger_pool: crate::triggers::CommandPool::new(trigger_pool_width),
        trigger_children: Vec::new(),
        handler_inbox: Default::default(),
        bells: HashMap::new(),
        bell_icons: std::collections::HashSet::new(),
        activity: HashMap::new(),
        badges: HashMap::new(),
        last_notified: None,
        alert_limits: crate::triggers::RateLimit::default(),
        status_armed: false,
        ime: Default::default(),
        pacer: Default::default(),
        fps: crate::perf_stats::MovingAverage::new(crate::perf_stats::FPS_ALPHA),
        frame_number: 0,
        perf: Default::default(),
        perf_frames: Default::default(),
        draw_timers: crate::perf::Timers::new(&["frame", "scene", "draw", "present"]),
        last_keystroke: None,
        power: crate::power::Poll::default(),
        last_status: HashMap::new(),
        status_watches: Default::default(),
        status_popover: None,
        status_popover_closed: None,
        open_quickly: None,
        quickly_query: String::new(),
        focus_order: HashMap::new(),
        activity_seq: 0,
        pane_seen: HashMap::new(),
        focus_seq: 0,
        status_prompt: None,
        global_note: crate::notes::GlobalNote::new(toolbelt_more_ui::notes_path(&std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir))),
        job_watches: Vec::new(),
        jobs_wanted: HashMap::new(),
        jobs_wanting: Default::default(),
        jobs_tree_seen: 0,
        jobs_watch_at: None,
        cmdinfo: None,
        pathnav,
        pathnav_due: None,
        pathnav_press: None,
        jobs_popover: None,
        jobs_popover_closed: None,
        jobs_confirm: None,
        signal_waits: Vec::new(),
        idle_sent: HashMap::new(),
        started: Instant::now(),
        uploads: HashMap::new(),
        pastes: HashMap::new(),
        paste_after: Vec::new(),
        paste_asks: HashMap::new(),
        paste_history: None,
        snippets: snippets_ui::load_snippets(),
        clips: HashMap::new(),
        inline: HashMap::new(),
        add_clip_win: None,
        bookmark_editor: None,
        bookmark_editor_req: None,
        bookmark_delete_asking: false,
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
        scroll_style: scrollbar::current_style(),
        scroll_seen: HashMap::new(),
        scroll_hover: None,
        scroll_ticks: HashMap::new(),
        scroll_series: Default::default(),
        text_blinking: false,
        flashes: HashMap::new(),
        bell_indicators: HashMap::new(),
        bell_limiter: crate::bell::Limiter::default(),
        bell_badge: crate::bell_badge::Badge::default(),
        badge_pushed: None,
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
        focused: true,
        tab_bar: tab_layout::Bar::default(),
        tab_scroll: 0.0,
        tab_hover: None,
        tab_keep_selected: true,
        tab_seen: None,
        tab_press: None,
        tab_selects_on_down,
        dropdown_views,
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
        logout_armed: false,
        logout_set: false,
        query_set: false,
        state_debounce: Default::default(),
        state_frozen: second_copy,
        focus_lost: false,
        desktops: None,
        desktops_tried: false,
        space_complained: false,
        desktop_due: None,
        switch_recheck: None,
        switch_logged: false,
        display_due: None,
        ui_element_confirmed,
        ui_element_now: false,
        tray_on: false,
        font_sig: String::new(),
        pane_faces: Default::default(),
        profiles_win: None,
        blink: crate::cursor_anim::Blink::new(Instant::now()),
        cursor_due: None,
        anim_frame: None,
        progress_frame: None,
        sleep: Default::default(),
        sleep_tracker: Default::default(),
        sleep_status: crate::sleep_guard::status_text(0, 0),
        blink_fade: crate::cursor_anim::Fade::default(),
        cursor_alpha: None,
        api_server: None,
        api_jar: Default::default(),
        ssh_secrets: Default::default(),
        ssh_registry: Default::default(),
        ssh_page_jobs: HashMap::new(),
        ssh_view_url: None,
        ssh_waiters: HashMap::new(),
        ssh_next_file_id: 1,
        ssh_suggest_cache: HashMap::new(),
        ssh_purge_at: None,
        ssh_search: None,
        ssh_search_issued: Vec::new(),
        ssh_search_acks: HashMap::new(),
        ssh_transfers: crate::ssh_transfer::Transfers::new(),
        ssh_locals: HashMap::new(),
        ssh_unzips: Vec::new(),
        ssh_landings: ssh_panel_app::Landings::default(),
        ssh_panel: None,
        ssh_batches: HashMap::new(),
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
        api_hooks: HashMap::new(),
        script_runs: BTreeMap::new(),
        script_next: 1,
        bindings: HashMap::new(),
        bonjour: None,
        bonjour_live: Vec::new(),
        bonjour_wanted: None,
        edit_follow_seen: None,
        script_tokens: HashMap::new(),
        script_alerts: VecDeque::new(),
        script_templates: HashMap::new(),
        script_pass: 0,
        initial_text_gen: HashMap::new(),
        script_alert: None,
        provisioner: None,
        provision_after: HashMap::new(),
        provision_needs: HashMap::new(),
        provision_targets: HashMap::new(),
        provision_waiting: Vec::new(),
        backend_probes: Vec::new(),
        uv_installed: crate::script_runtime::uv_exe(&script_runtime_ui::uv_dir()).is_file(),
        scripts_alerts: VecDeque::new(),
        perm_asks: Default::default(),
        web_note: Default::default(),
        perm_menu: None,
        web_note_limits: Default::default(),
        perm_queues: Default::default(),
        scripts_alert: None,
        update_checker: None,
        update_window: None,
        update_due: None,
        update_manual: false,
        update_show: None,
        update_show_manual: false,
        tip_window: None,
        tip_due: None,
        tips_started: false,
        uia_hosts: HashMap::new(),
        access_trackers: HashMap::new(),
        access_due: None,
        access_built: None,
        access_hold: None,
        access_window: None,
        access_focus_sent: None,
        access_attach: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        locale_asks: VecDeque::new(),
        locale_window: None,
        suppressed,
        show_remembered: false,
        catalog_due: None,
        inline_asking: false,
        suppressed_window: None,
        special_exceptions: None,
        suppressed_retitle_due: None,
        ai_asks: VecDeque::new(),
        ai_alert: None,
        ai_reveal: None,
        ai_key_asks: VecDeque::new(),
        autofill: Default::default(),
        basic_auth: HashMap::new(),
        browser_downloads: HashMap::new(),
        download_seq: 0,
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
        browser_db: None,
        browser_db_failed: false,
        browser_db_mem: None,
        chat_links_off: false,
        chat_broker: crate::chat_broker::Broker::new(),
        chat_window: None,
        cockpit: None,
        cockpit_view,
        cockpit_save_due: None,
        cc_onboarding: None,
        claude_health_done: false,
        claude_watcher_off: false,
        claude_install_due: false,
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
        cmd_button_hits: Vec::new(),
        cmd_button_menu: None,
        offscreen_hits: Vec::new(),
        offscreen_prepared: HashMap::new(),
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
        ai_browser: ai_browser_app::Calls::default(),
        orch_provenance: HashMap::new(),
        orch_parks: HashMap::new(),
        orch_waits: HashMap::new(),
        orch_denied: HashMap::new(),
        orch_mirrors: HashMap::new(),
        orch_pending_cr: Vec::new(),
        orch_checks: Vec::new(),
        orch_spawns: Vec::new(),
        orch_spawned: HashMap::new(),
        orch_watchers: HashMap::new(),
        orch_held: HashMap::new(),
        pending_restarts: HashMap::new(),
        restart_history: HashMap::new(),
        orch_polls: HashMap::new(),
        orch_outbox: HashMap::new(),
        #[cfg(windows)]
        ai_wire_log: Arc::new(Mutex::new(crate::ai_wire_log::WireLog::new())),
        script_procs: HashMap::new(),
        script_jobs_left: Vec::new(),
        script_after_enable: Vec::new(),
        repl_after_enable: None,
        repl_pending: None,
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
        last_notified_debug: None,
        focus_logged: None,
        debug_notice_shown: false,
        unread_counts: HashMap::new(),
        unread_session_counts: HashMap::new(),
        script_components: HashMap::new(),
        script_parsed: HashMap::new(),
        script_titles: HashMap::new(),
        text_popover: None,
        web_popover: None,
        browser_tip: None,
        browser_tip_next: None,
        browser_tip_life: Default::default(),
        web_replay: None,
        web_replay_pane: None,
        web_replay_save: None,
        icon_requests: HashMap::new(),
        icon_jobs: HashMap::new(),
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
        alpha_gpu: None,
        transparency_unavailable: false,
        alpha_pending: Vec::new(),
        transparency_banner: false,
        frame_alpha: false,
        fade_failed: false,
        prefs_launch_folder,
        relaunch_on_exit: false,
        relaunch_due: false,
        restored_scripts,
        restored_dir: None,
        web_envs,
        web_events,
        file_pool: None,
        web_focus_held: None,
        web_focus_view: None,
        tool_view_ids: HashMap::new(),
        tool_restarts: HashMap::new(),
        browser_views: HashMap::new(),
        page_saves: HashMap::new(),
        page_save_seq: 0,
        browser_runs: Vec::new(),
        browser_next: Vec::new(),
        adblock: adblock_app::Adblock::default(),
        browser_ext: Default::default(),
        focus_seen: Default::default(),
        report_seen: Default::default(),
        focus_reported: Default::default(),
        browser_url_override: None,
        restoring_windows: false,
    };
    // Debug logging at launch starts before the first window (`AD:1356-1358`).
    app.start_debug_logging_automatically();
    // The handoff class is registered before the first window, so COM's wait for it is short.
    app.sync_handoff_server();
    if let Err(e) = event_loop.run_app(&mut app) {
        message_box(&format!("Tandem stopped: {e}"));
    }
    // Stop browsing: the callbacks end before the process does.
    app.bonjour = None;
    teardown_process();
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

    /// The font's own cell, without the spacings; (0, 0) before the renderer exists, which
    /// the cursor reads as "the same as the cell".
    fn base_cell(&self) -> (f32, f32) {
        self.gpu.as_ref().map_or((0.0, 0.0), |g| g.renderer.base_cell())
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
        // A selected tab's sessions are no longer waiting (`tabWasSelected`).
        let sessions: Vec<String> = self.tabs[i].layout.panes().iter().map(|p| p.to_string()).collect();
        self.bell_badge.tab_selected(&sessions);
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
        (tab_position::side_width(self.side_bar_preferred, w / scale, SIDE_TAB_MARGIN, self.side_bar_minimum()) * scale).round()
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
                let (tabs, windows) = (self.tabs.len(), self.pending_tabs());
                self.new_tab(guid.as_deref());
                // Nothing opened: a copy made for it goes.
                if let Some(g) = guid.filter(|_| self.tabs.len() == tabs && self.pending_tabs() == windows) {
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
        let preferred = tab_position::side_width(start_width + dx, w, SIDE_TAB_MARGIN, self.side_bar_minimum()).round();
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
        self.refresh_pointer_shape();
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

    /// The side and the top and bottom margins in pixels (`TerminalMargin`, `TerminalVMargin`,
    /// iTerm2's 5 and 2 points by default), read on every call.
    fn margins(&self) -> (f32, f32) {
        let scale = self.scale();
        (margins::px(self.globals.int("TerminalMargin"), scale), margins::px(self.globals.int("TerminalVMargin"), scale))
    }

    /// The one place pixels become columns and rows, so tmux, resizes, snaps, screenshots and
    /// hit tests agree: `rect` less the margins and `room` pixels at its right edge, at `cell`.
    fn grid_size_in(&self, rect: Rect, cell: (f32, f32), room: f32) -> (u16, u16) {
        margins::pane_grid((rect.w, rect.h), cell, self.margins(), room)
    }

    /// The profile with this Guid, or the default profile.
    fn profile(&self, guid: Option<&str>) -> Profile {
        if let Some(answered) = &self.param_profile {
            return answered.clone();
        }
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
        use crate::tab_theme::{Inputs, MinimalKnobs, dimming};
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
        if let Some(amount) = self.dimmed_pane_touches_bar() {
            let a = amount * 0.375;
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
            dimming: dimming(g.bool("DimBackgroundWindows"), focused, self.dim_base(), g.bool("DimOnlyText")),
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
        // The invisible divider needs the other panes' backgrounds to dim away: Dim inactive split
        // panes on and Dim only text off (P12), on top of 13e's prominence 0 gate.
        let invisible = (self.globals.bool("DimInactiveSplitPanes") && !self.globals.bool("DimOnlyText")).then(|| crate::tab_theme::single_other_background(&panes, current)).flatten();
        let theme = crate::tab_theme::divider_color(self.tab_style(), self.resolved_style(), decoration_bg, prominence, invisible);
        pane_nav_ui::divider_rgb(&self.globals.string("SplitPaneColor"), theme)
    }

    /// `SplitPaneDimmingAmount`, the base of every dimming amount.
    pub(super) fn dim_base(&self) -> f32 {
        self.globals.number("SplitPaneDimmingAmount").map_or(0.4, |v| v as f32)
    }

    /// How a pane of `tab` dims (the model of `crate::dim`): the one place the settings become a
    /// `Dim`, read by the render loop, the title bars, the tab bar's outline and the tab palette.
    /// Broadcast inputs use the focused pane as the sender (`PT:12402-12422`); the window being key
    /// is `self.focused`.
    fn pane_dim(&self, tab: &Tab, id: PaneId, bview: &crate::broadcast::View) -> crate::dim::Dim {
        use crate::dim;
        let (receives, current_receives) = dim::broadcast_inputs(&self.broadcast, bview, id, tab.focused);
        let split = dim::inactive_split(self.globals.bool("DimInactiveSplitPanes"), id == tab.focused, receives, current_receives);
        let background = dim::background(self.globals.bool("DimBackgroundWindows"), self.focused);
        dim::Dim { amount: dim::amount(split, background, self.dim_base()), only_text: self.globals.bool("DimOnlyText") }
    }

    /// The largest dimming amount of a dimmed pane lying against the tab bar's edge
    /// (`shouldTweakMinimalTabOutlineAlpha`), when there is one: not in a background window that
    /// dims as a whole, whose dimming looks fine under the outline.
    fn dimmed_pane_touches_bar(&self) -> Option<f32> {
        use tab_position::TabPosition::{Bottom, Left, Right, Top};
        if self.tab_style() != crate::tab_theme::TabStyle::Minimal || (self.globals.bool("DimBackgroundWindows") && !self.focused) {
            return None;
        }
        let tab = self.tabs.get(self.active)?;
        let area = self.content_area_for(tab);
        let bview = self.broadcast_view();
        let position = self.tab_position();
        self.visible(tab)
            .into_iter()
            .filter(|(_, r)| match position {
                Top => r.y <= area.y + 1.0,
                Bottom => r.y + r.h >= area.y + area.h - 1.0,
                Left => r.x <= area.x + 1.0,
                Right => r.x + r.w >= area.x + area.w - 1.0,
            })
            .filter_map(|(id, _)| crate::dim::outline_amount(self.pane_dim(tab, id, &bview)))
            .max_by(f32::total_cmp)
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

    /// Restart Session (`restartSession`, `PS:3846-3872`): a new shell in the same pane, with the
    /// same profile, and the pane keeps its contents (R3). A running shell is reset and gets
    /// "Session Restarted" written under its text, then ends; an ended pane is reset only. The
    /// pane is in the current window (in any tab, shown or not) or buried; a buried one starts at
    /// the content area's size and takes its tab's size when it comes back. The menu always
    /// passes the focused, shown pane (its rect); the tab and buried fallbacks are for the API's
    /// RestartSession, which may name a background tab's or a buried session. A running shell ends before the
    /// new one starts (`PS:3863-3871`), so a shell that fails to start leaves an ended pane with
    /// "Session failed to start.".
    fn restart_pane(&mut self, id: PaneId) {
        // The safety net: no caller ends the programs an ended shell left without the question.
        if self.panes.get(&id).is_some_and(|p| p.exited.is_some() && !p.live_survivors().is_empty()) {
            return;
        }
        self.restart_launching(id);
    }

    /// A restart asked for by `via` (13q B7): what it does first follows `restart_step`. A running
    /// shell's question is the menu's only (the API never asks); programs an ended shell left ask
    /// on every route, and the timer shows the offer instead. Returns what it did first, so a caller
    /// can tell a restart from a question that is waiting for the person.
    pub(super) fn restart_via(&mut self, id: PaneId, via: crate::session_end::Via) -> crate::session_end::RestartStep {
        use crate::session_end::{restart_first, RestartStep};
        let Some(p) = self.panes.get(&id) else { return RestartStep::Offer };
        let left = if p.exited.is_some() { p.live_survivors() } else { Vec::new() };
        // A single-use window (git, Run Command in Window) and the other panes with nothing of
        // their own to start are refused before any question.
        let step = restart_first(self.restartable(id), via, p.exited.is_some(), left.len());
        match step {
            RestartStep::Now => self.restart_pane(id),
            RestartStep::AskRunningJobs => {
                if !self.alert_waits(|then| matches!(then, scripts_alerts::Then::Restart(p, _) if *p == id)) {
                    let about = self.panes.get(&id).map(|p| p.restart_subject()).unwrap_or_default();
                    self.ask_scripts(scripts_alerts::Ask::new(menu::restart_spec(), scripts_alerts::Then::Restart(id, about)));
                }
            }
            RestartStep::AskSurvivors => {
                if !self.alert_waits(|then| matches!(then, scripts_alerts::Then::Restart(p, _) if *p == id)) {
                    let about = crate::session_end::AskedAbout { generation: self.panes.get(&id).map_or(0, |p| p.shell_gen), survivors: left };
                    self.ask_scripts(scripts_alerts::Ask::new(crate::session_end::restart_survivors_spec(&about.survivors), scripts_alerts::Then::Restart(id, about)));
                }
            }
            RestartStep::Offer => self.offer_restart(id),
            RestartStep::Refused => {}
        }
        step
    }

    /// The restart itself, after whatever question it needed was answered.
    fn restart_launching(&mut self, id: PaneId) {
        // A workgroup member restarts with its own command and its instance's environment.
        let launch = self.workgroup_launch(id);
        if matches!(launch, workgroup_app::MemberLaunch::Refused) {
            return;
        }
        if let workgroup_app::MemberLaunch::Run { command, env } = &launch {
            self.start_command_override = command.clone();
            self.start_extra_env = env.clone();
        }
        self.restart_confirmed(id);
        if matches!(launch, workgroup_app::MemberLaunch::Run { .. }) {
            self.start_command_override = None;
            self.start_extra_env.clear();
        }
    }

    fn restart_confirmed(&mut self, id: PaneId) {
        // One still waiting for its answers starts only from the prompt (13y2 Task 9e I1).
        if !self.restartable(id) || self.param_waiting(id) {
            return;
        }
        let in_tab = || {
            let tab = self.tabs.iter().find(|t| t.layout.panes().contains(&id))?;
            tab.layout.rects(self.pane_area_for(tab), self.gap()).into_iter().find(|(p, _)| *p == id).map(|(_, r)| r)
        };
        // A hidden workgroup peer (in no layout) starts at the content area's size and takes its slot's when shown.
        let hidden_peer = || self.panes.get(&id).is_some_and(|p| p.workgroup.is_some()) && self.pane_window_id(id).is_none();
        let Some(rect) = self.pane_rect(id).or_else(in_tab).or_else(|| self.buried.contains(id).then(|| self.content_area())).or_else(|| hidden_peer().then(|| self.content_area())) else { return };
        // A timer for this pane is stale now (`maybeReplaceTerminatedShellWithNewInstance` checks `_exited`).
        self.pending_restarts.remove(&id);
        // Instant Replay closes first; the new shell starts live.
        self.end_replays(&[id]);
        let guid = self.panes.get(&id).map(|p| p.profile.clone());
        // A parameterised session reruns its answered program (`PS:4889-4899`).
        let answered = self.param_answers_of(id).map(<[_]>::to_vec);
        // The new shell never saw what a model typed into the old one.
        self.orch_held.remove(&id);
        // A slow Initial Text of the old shell must not type into the new one (13p); the new
        // shell sends its own once it is installed (P14).
        self.bump_initial_text(id);
        // A live shell ends first, as iTerm2's does (`PS:3863-3871`), with the words under its
        // text; a start that then fails leaves an ended pane.
        if self.panes.get(&id).is_some_and(|p| p.exited.is_none() && p.shell_pid().is_some()) {
            let custom = self.globals.string("SessionEndMessageText");
            let text = crate::session_end::restarted_text(&custom, &self.globals.string("SessionRestartedMessageText"));
            let divider = self.globals.string("SessionEndMessageDividerCharacter");
            if let Some(pid) = self.panes.get(&id).and_then(|p| p.shell_pid()) {
                self.jobs.forget(pid);
            }
            if let Some(mut old) = self.panes.remove(&id) {
                self.ssh_pane_dropped(&old);
                self.forget_cookie(&old);
                old.own_mut().append_end_message(&text, &divider);
                // A start that then fails leaves a whole ended pane: its log written out, its
                // program's state (progress bar, drops, held output) gone. Decision: the failure
                // fires no SessionEnded; the person asked for the restart.
                old.stop_log();
                old.own_mut().program_ended();
                old.end_live_shell();
                self.panes.insert(id, old);
            }
        }
        let start_dir = self.panes.get(&id).and_then(|p| p.start_dir.clone());
        // No folder recorded (a pane from before 13q's start) reads as a gone folder: the profile's rule.
        let set_here = answered.is_some() && self.param_profile.is_none();
        if set_here {
            self.param_profile = session_params_app::restart_profile(&self.profile(guid.as_deref()), answered.as_deref());
        }
        let spawned = self.spawn_pane(id, rect, guid.as_deref(), Some(start_dir.as_deref().unwrap_or(std::path::Path::new(""))));
        if set_here {
            self.param_profile = None;
        }
        let (fresh, profile) = match spawned {
            Ok(started) => started,
            Err(e) => {
                self.restart_failed(id, &e);
                return;
            }
        };
        let Some(mut old) = self.panes.remove(&id) else { return };
        // The old log stops and is written out before the new shell's opens the same file (a
        // restart appends to it): two writers on one file would interleave.
        old.stop_log();
        crate::session_log::wait_for_writers(Duration::from_millis(500));
        // The offer and the tmux question are about the shell that is gone (`willRecycleSession`).
        if let Some(b) = self.bars.get_mut(&id) {
            b.retain(|x| ![crate::session_end::RESTART_BAR, crate::session_end::STORM_BAR, crate::session_end::TMUX_CLOSE_BAR, session_end_ui::SURVIVORS_BAR].contains(&x.id.as_str()));
        }
        // The pid is reusable once the shell is gone: the watcher forgets the one the pane has or had.
        if let Some(pid) = old.shell_pid().or(old.exit_pid()) {
            self.jobs.forget(pid);
        }
        self.ssh_pane_dropped(&old);
        // The old shell is gone for good; the new one has its own cookie.
        self.forget_cookie(&old);
        // The old pane's pty drops here, which ends its shell.
        let pane = Pane::relaunch(old, fresh);
        // Its inline files went with its screen.
        self.close_inline_questions_of(id);
        // The id lives on: the old screen's ticks must not meet the new screen.
        self.forget_scrollbar(id);
        self.install_pane(id, pane, &profile, true);
        // The restarted shell types its own Initial Text once, as iTerm2's start does
        // (`PS:3589-3600`, P14): the old start's answer was dropped above, an SSH profile still
        // waits for its conductor's login, and an override launch (none restarts) never types it.
        if self.start_command_override.is_none() && self.start_env_override.is_none() {
            self.send_initial_text(id, &profile);
        }
        // The new Screen has no chat watch: a chat streaming this session goes on (`CVC:144-165`).
        self.chat_refresh();
    }

    /// A restart whose shell did not start: the pane is an ended one (a live shell ended first). It writes the
    /// error as its end message (`taskDiedWithError`, `PS:4686-4688`) and both show "Session
    /// failed to start." with no buttons (`NC:318-327`). The "start" banner is not set.
    fn restart_failed(&mut self, id: PaneId, error: &str) {
        let ended = self.panes.get(&id).is_some_and(|p| p.exited.is_some());
        if ended {
            let divider = self.globals.string("SessionEndMessageDividerCharacter");
            if let Some(p) = self.panes.get_mut(&id) {
                p.own_mut().append_end_message(error, &divider);
            }
        }
        if let Some(b) = self.bars.get_mut(&id) {
            b.retain(|x| x.id != crate::session_end::RESTART_BAR && x.id != crate::session_end::STORM_BAR);
        }
        self.ask(id, crate::session_end::RESTART_BAR, crate::session_end::FAILED_TO_START, &[], true);
    }

    /// "Restart session?" answered OK (running jobs or left-over programs): the pane restarts in
    /// its own window (gone: nothing happens). Only the shell the question named restarts: if it
    /// exited and was restarted since, or left other programs, the answer is not for the pane as
    /// it is now, and the restart asks again about that (never ending a new shell unasked).
    pub(super) fn restart_answered(&mut self, id: PaneId, asked: &crate::session_end::AskedAbout) {
        let Some(now) = self.panes.get(&id).map(|p| p.restart_subject()) else { return };
        self.enter_pane_window(id);
        match crate::session_end::answer_step(asked, &now) {
            crate::session_end::AnswerStep::Restart => self.restart_launching(id),
            crate::session_end::AnswerStep::AskAgain => {
                self.restart_via(id, crate::session_end::Via::Menu);
            }
        }
    }

    /// The session variables for pane `id`, about to start with profile `p` (`PS:3246-3350`),
    /// and the cookie minted for it: a Session cookie in the jar, only while "Enable Python
    /// API" and `setCookie` are both on and the server runs. The numbers are those at launch:
    /// the window's number, the tab it goes into, and iTerm2's pane number (the tab's session
    /// count less one once it is in the tab, `TAB:7435-7443`). The result holds a secret:
    /// never log it. A pane started on SSH transport `t` other than ConPTY is told which
    /// (`TANDEM_SSH_TRANSPORT`, for it2ssh).
    fn session_env(&mut self, id: PaneId, p: &Profile, t: crate::ssh_transport::Transport, cmd: &str) -> (Vec<String>, Option<String>) {
        let window = self.window_number();
        let counts: Vec<usize> = self.tabs.iter().map(|t| t.layout.panes().len()).collect();
        let holding = self.tabs.iter().position(|t| t.layout.panes().contains(&id));
        let (tab, pane) = crate::session_env::numbers(&counts, holding, self.start_tab);
        // The REPL's variables (`start_env_override`) carry its own cookie, which then is the
        // pane's (it leaves the jar with the pane); no Session cookie is minted beside it.
        let mut extra = self.start_env_override.clone().unwrap_or_default();
        let given = extra.iter().find_map(|e| e.strip_prefix("ITERM2_COOKIE=")).map(str::to_string);
        // A launch's own entries (a workgroup member's, a git launch's removals) are applied again last.
        let launch = std::mem::take(&mut self.start_extra_env);
        extra.extend(launch.iter().cloned());
        let cookie = given.or_else(|| {
            (self.globals.bool(api_ui::ENABLE_KEY) && self.globals.bool(api_ui::SET_COOKIE_KEY) && self.api_server.is_some())
                .then(|| self.api_jar.lock().unwrap().mint(crate::api_auth::CookieUse::Session))
        });
        let name = p.name();
        let id = id.to_string();
        // The session starts with the profile's Unicode version, as its Capabilities reply does.
        let version = crate::osc1337::profile_unicode(p.values.get("Unicode Version").and_then(|v| v.as_integer()));
        let term_features = crate::osc1337::capabilities(crate::osc1337::session_features(self.globals.bool("AllowClipboardAccess"), 0, version, p.bool("Ambiguous Double Width")));
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        let path = self
            .utilities
            .as_ref()
            .filter(|_| self.globals.bool("AddUtilitiesToPATH"))
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
            lc_terminal: self.globals.bool("ShouldSetLCTerminal"),
            path: path.as_deref(),
            ssh_path: ssh_path.as_deref(),
            ssh_transport: transport,
        };
        let mut env = crate::session_env::overlay(crate::session_env::vars(&facts), &extra);
        // TERM is the profile's Terminal Type, only when nothing else names it (`PS:3253-3255`).
        // Decided before the POSIX variables (which never name TERM) so a WSL session's
        // `WSLENV` can list it (P17); the entry still goes last.
        let term = crate::session_env::term_var(&env, std::env::var_os("TERM").is_some(), &p.terminal_type());
        // The POSIX variables follow, never interleaved (`PS:3284-3366`); `merge` puts them over
        // the inherited block.
        let posix = self.posix(p, cmd, term.is_some());
        env.extend(crate::session_env::posix_vars(&posix));
        env.extend(term);
        (crate::session_env::launch_last(env, &launch), cookie)
    }

    /// The POSIX variables a session on `cmd` gets: `COLORFGBG` from the profile's palette, the
    /// locale (`locale_guess::locale_vars`), and `TERMINFO_DIRS` for WSL (R6).
    fn posix(&mut self, p: &Profile, cmd: &str, term_set: bool) -> crate::session_env::Posix {
        let palette = crate::palette::Palette::from_profile(p, self.dark());
        let colorfgbg = crate::session_env::colorfgbg(&palette.ansi, palette.fg, palette.bg, self.globals.bool("UseColorfgbgFallback"));
        let locale = self.locale_pairs(p);
        // Only a WSL session wants it, and only a local `%APPDATA%` is written (P12).
        let terminfo = (self.globals.bool("ShouldSetTerminfoDirs") && crate::session_env::is_wsl(cmd)).then(|| self.terminfo_folder()).flatten();
        posix_for(cmd, terminfo.as_deref().map(|d| d.to_string_lossy()).as_deref(), colorfgbg, locale, std::env::var("WSLENV").ok(), term_set)
    }

    /// The locale variables for the profile (`PS:3284-3331`). The start-time prompt and the
    /// wrong-encoding warning do not hold the shell (Decisions "Locale on Windows"): a suspect
    /// custom locale is used as written and the warning is asked, a failed guess falls to
    /// `LC_CTYPE` (iTerm2's path after "Use Minimal POSIX Locale") and the prompt is queued. An
    /// override launch (a git window, a script, an orchestrated command) never asks.
    #[cfg(windows)]
    fn locale_pairs(&mut self, p: &Profile) -> Vec<(String, String)> {
        use crate::locale_guess::{self, Outcome};
        let encoding = p.int("Character Encoding");
        let facts = crate::locale_win::facts(encoding, &self.globals.string("FallbackLCCType"));
        let name = locale_guess::encoding_name(encoding);
        let supported = |c: &str| crate::locale_win::supported(c, name);
        let mode = locale_guess::Mode::from_value(p.int("Set Local Environment Vars"));
        let asks = self.start_command_override.is_none() && self.start_env_override.is_none();
        match locale_guess::locale_vars(mode, &p.string("Custom Locale"), &facts, encoding == 4, &supported) {
            Outcome::Vars(v) => v,
            Outcome::None => Vec::new(),
            Outcome::SuspectCustom(l) => {
                if asks {
                    let guid = self.store.shared_guid(&p.guid());
                    self.ask_locale_mismatch(guid, l.clone(), false);
                }
                vec![("LANG".to_string(), l)]
            }
            Outcome::NeedsPrompt => {
                if asks {
                    let guid = self.store.shared_guid(&p.guid());
                    self.queue_locale_prompt(guid, p.name(), !p.bool("Is Dynamic Profile"));
                }
                locale_guess::lc_ctype(&facts, &supported).map(|v| vec![("LC_CTYPE".to_string(), v)]).unwrap_or_default()
            }
        }
    }

    #[cfg(not(windows))]
    fn locale_pairs(&mut self, _p: &Profile) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Tandem's terminfo folder, written on first use and kept for the run. A UNC `%APPDATA%`
    /// is never written (none, silently); a write error leaves it out with a banner.
    #[cfg(windows)]
    fn terminfo_folder(&mut self) -> Option<PathBuf> {
        if self.terminfo_dir.is_none() {
            let appdata = crate::session_env::appdata_local()?;
            match crate::terminfo::ensure(&appdata) {
                Ok(dir) => {
                    self.banners.clear("terminfo");
                    self.terminfo_dir = Some(dir);
                }
                Err(e) => self.banners.set("terminfo", format!("Could not write Tandem's terminfo files: {e}")),
            }
        }
        self.terminfo_dir.clone()
    }

    #[cfg(not(windows))]
    fn terminfo_folder(&mut self) -> Option<PathBuf> {
        None
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
        self.focus_reported.forget(id);
        self.report_seen.forget(id);
        self.page_save_cancel(id);
        self.browser_extensions_pane_gone(id);
        self.api_session_subs.forget_session(id);
        // The shell a model's held text was typed into is gone (Task 10 fix round 2).
        self.orch_held.remove(&id);
        self.unread_session_counts.retain(|(p, _), _| *p != id);
        self.script_templates.retain(|(p, _), _| *p != id);
        self.bindings.retain(|(p, _), _| *p != id);
        if let Some(s) = self.settings.as_mut() {
            s.edit_session = crate::settings_ui::edit_session_after_close(s.edit_session, id);
            // The Session Hotkey row (13u) goes with the session it showed.
            if s.edit_session.is_none() {
                s.session_hotkey = None;
                s.session_hotkey_waiting = false;
                s.session_hotkey_refused = None;
            }
        }
        if self.edit_follow_seen == Some(id) {
            self.edit_follow_seen = None;
        }
        self.script_components.retain(|(p, _), _| *p != id);
        self.script_titles.remove(&id);
        self.host_stacks.remove(&id);
        self.alt_infer.remove(&id);
        self.print_guards.remove(&id);
        self.pending_title_restores.remove(&id);
        if self.text_popover.as_ref().is_some_and(|(p, _)| *p == id) {
            self.text_popover = None;
        }
        if self.web_popover.as_ref().is_some_and(|(p, _)| *p == Some(id)) {
            self.web_popover = None;
        }
        self.browser_tip_drop(id);
        self.marks_pane_gone(id);
        self.autofill_pane_gone(id);
        self.browser_pw_pane_gone(id);
        self.browser_downloads_view_gone(id);
        self.ssh_page_view_gone(id);
        // Waiting browser tools answer a failure before the view closes.
        self.ai_browser_pane_gone(id);
        self.close_browser_view(id);
    }

    /// A cookie that will never be used takes itself out of the jar.
    fn forget_cookie_text(&self, cookie: Option<&str>) {
        if let Some(c) = cookie {
            self.api_jar.lock().unwrap().remove(c);
        }
    }

    fn start_pane_as(&mut self, id: PaneId, rect: Rect, guid: Option<&str>) -> bool {
        // A Browser profile starts a web page, not a shell (`GW:98-111`); with BrowserProfiles off
        // it is a terminal. No handed-over console, no command override is ever a browser.
        let p = self.profile(guid);
        if self.start_handoff.is_none() && self.start_command_override.is_none() && self.is_browser_profile(&p) {
            return self.start_browser_pane(id, rect, &p);
        }
        self.browser_url_override = None;
        // A `$$NAME$$` left in the command, name or folder asks first (13y2 Task 9e).
        let answers = match self.param_gate(id, rect, &p) {
            session_params_app::Gate::Deferred => return true,
            session_params_app::Gate::Refused(why) => {
                self.banners.set("start", why);
                return false;
            }
            session_params_app::Gate::Start(answers) => answers,
        };
        self.param_profile = session_params_app::restart_profile(&p, answers.as_deref());
        let base = p;
        let handed = self.start_handoff.is_some();
        let spawned = self.spawn_pane(id, rect, guid, None);
        self.param_profile = None;
        match spawned {
            Ok((pane, p)) => {
                self.install_pane(id, pane, &p, false);
                if let Some(answers) = answers {
                    self.param_settle(id, &base, answers);
                }
                // iTerm2 blanks Initial Text for single-use and URL launches (`iTermController.m:1872`,
                // `:2188`): a git window, an orchestrated or scripted command and an ssh: URL's
                // copy never type the profile's text (B2). The API's CreateTab with custom
                // properties starts the base profile's own shell with the text, as iTerm2's does
                // (the API is authenticated; its command is a profile property, not an override).
                if self.start_command_override.is_none() && self.start_env_override.is_none() && !handed {
                    self.send_initial_text(id, &p);
                }
                true
            }
            Err(e) => {
                self.banners.set("start", e);
                false
            }
        }
    }

    /// Starts the shell of pane `id` at `rect` with profile `guid`: the pane, not yet installed,
    /// and its profile; or why it did not start (the caller says so: a new pane sets the "start"
    /// banner, a restart shows a bar).
    /// `restart_dir` is a restarted pane's original folder (P13): used while it is still a local
    /// folder, else the profile's rule without the focused pane.
    fn spawn_pane(&mut self, id: PaneId, rect: Rect, guid: Option<&str>, restart_dir: Option<&std::path::Path>) -> Result<(Pane, Profile), String> {
        // The new pane's grid comes from its own profile's face, measured now, less its room.
        let p = self.profile(guid);
        let cell = self.profile_cell(&p);
        let (cols, rows) = self.grid_size_in(rect, cell, self.pane_room_for(id, &p));
        // A console handed over by Windows is the pane; no shell is started (m13v1 Task 5).
        if let Some(h) = self.start_handoff.take() {
            return self.pane_from_handoff(id, p, h, cols, rows);
        }
        let (cmd, transport) = match self.start_command_override.clone() {
            // The API's create command never takes the SSH path.
            Some(cmd) => (cmd, crate::ssh_transport::Transport::Conpty),
            None => self.profile_command(&p),
        };
        // A restored arrangement names each session's folder.
        let (cwd, warning) = match &self.start_dir_override {
            Some(crate::arrangement::Folder::Forced(d)) if crate::start_dir::probe_ok(std::path::Path::new(d), &|d| crate::localfs::is_local_dir(d)) => (PathBuf::from(d), None),
            // A git command's window runs only in its folder, never the profile's.
            Some(crate::arrangement::Folder::Forced(d)) if self.start_command_override.is_some() => {
                return Err(format!("The folder {d} no longer exists, so git did not run."));
            }
            Some(crate::arrangement::Folder::Forced(d)) => {
                let (c, _) = profile::start_dir_with(&p, None);
                (c, Some(format!("The folder {d} from the arrangement does not exist; the profile's folder is used.")))
            }
            Some(crate::arrangement::Folder::Previous(d)) => profile::start_dir_with(&p, Some(std::path::Path::new(d))),
            // A restart reuses the folder the shell first started in, never OSC 7 or the focused
            // pane (`PS:4890-4896`, P13).
            _ if restart_dir.is_some() => crate::session_end::restart_dir(restart_dir, &p, &|d| crate::localfs::is_local_dir(d)),
            _ => self.profile_start_dir(&p),
        };
        match warning {
            Some(w) => self.banners.set("directory", w),
            None => self.banners.clear("directory"),
        }
        if self.host.as_ref().is_some_and(|h| h.is_lost()) {
            self.host = None;
            self.banners.set("host", "The session host stopped. New shells run inside Tandem, so they end with it.");
        }
        let (env, cookie) = self.session_env(id, &p, transport, &cmd);
        // Only the profile's own shell is injected; `cmd` stays the profile's line for the pane,
        // its banner and its title (B6).
        let injected = self.injected_command(&p, &cmd, transport);
        let launch = injected.as_deref().unwrap_or(&cmd);
        let started = match &self.host {
            Some(h) => match Pane::start_hosted_transport(id, h, transport, launch, Some(&cwd), &env, cols, rows, self.notify.clone()) {
                // A host below protocol 4 cannot run this transport: the session runs inside
                // Tandem instead.
                Err(e) if e == crate::host_client::HOST_TOO_OLD => {
                    self.banners.set("host", "The session host is too old for this SSH session, so it runs inside Tandem.");
                    Pane::start_transport(id, &self.api, transport, launch, Some(&cwd), &env, cols, rows, self.notify.clone())
                }
                other => other,
            },
            None => Pane::start_transport(id, &self.api, transport, launch, Some(&cwd), &env, cols, rows, self.notify.clone()),
        };
        match started {
            Ok(mut pane) => {
                // Never the answered profile's name or program (13y2 Task 9e I4): the shared profile's
                // name, and for a parameterised start the program before the answers.
                let base = guid.and_then(|g| self.store.get(g)).unwrap_or_else(|| self.store.default_profile());
                let shared = base.get(crate::session_profile::ORIGINAL_GUID).and_then(|v| v.as_str().map(str::to_string)).and_then(|g| self.store.get(&g)).unwrap_or(base);
                let unanswered = self.param_profile.is_some().then(|| profile::command_line(base, &self.shell, None));
                crate::rlog!("session started: profile {} program {}", crate::dlog::name(&shared.name()), crate::dlog::program(session_params_app::start_log_command(&cmd, unanswered.as_deref())));
                pane.cookie = cookie;
                pane.command = cmd;
                pane.start_dir = Some(cwd);
                if self.start_command_override.is_none() {
                    // Not an ssh session for F27 yet: its hook, vouched by the origin token,
                    // makes it one.
                    pane.start_as_profile(profile::is_ssh(&p));
                    pane.ssh.initial_directory = profile::ssh_initial_directory(&p);
                }
                if let Some(jumps) = self.start_jumps_override.take() {
                    pane.pending_jumps = jumps;
                }
                Ok((pane, p))
            }
            Err(e) => {
                self.forget_cookie_text(cookie.as_deref());
                Err(format!("Could not start {cmd}: {e}"))
            }
        }
    }

    /// The command line with the PowerShell shell integration loaded, when the profile asks
    /// ("Load Shell Integration Automatically"), the launch is the profile's own shell on
    /// ConPTY (never a git, orchestrated, script or API command, never an SSH profile), the
    /// shell is PowerShell and the script is written (`PS:3633-3653`). None leaves the line as
    /// it is.
    fn injected_command(&mut self, p: &Profile, cmd: &str, transport: crate::ssh_transport::Transport) -> Option<String> {
        let own = self.start_command_override.is_none() && self.start_env_override.is_none();
        if !own || transport != crate::ssh_transport::Transport::Conpty || profile::is_ssh(p) || !p.bool("Load Shell Integration Automatically") {
            return None;
        }
        crate::shell_inject::shell_of(cmd)?;
        let script = self.ensure_pwsh_script()?;
        crate::shell_inject::inject(cmd, &script)
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
    /// A restarted pane (`relaunch`) keeps its Instant Replay recording and its scroll accumulators.
    fn install_pane(&mut self, id: PaneId, mut pane: Pane, p: &Profile, relaunch: bool) {
        pane.restyle(Palette::from_profile(p, self.dark()), Look::from_profile(p));
        pane.set_bidi_rules(crate::bidi::Rules::from_globals(&self.globals));
        pane.set_copy_options(crate::seltext::Options::from_globals(&self.globals));
        screen_follows_profile(&mut pane.screen, p);
        pane.screen.set_path_probe(self.path_probe());
        pane.profile = p.guid();
        pane.set_buffering(p.bool("Buffer Input by Default"));
        if !relaunch {
            // `IRMemory` is read once per session (`S:121-122`).
            pane.dvr = crate::dvr::Dvr::new(crate::pane::capacity_for(self.globals.int("IRMemory")));
            // Read once per session ("Changes to this setting only affect new sessions").
            pane.wheel = crate::wheel::PaneWheel::new(self.globals.number("HorizontalScrollingSensitivity").unwrap_or(0.1));
        }
        if let Some(pid) = pane.shell_pid() {
            self.jobs.watch(pid);
            if let Some(paused) = pane.console_pause_flag() {
                self.jobs.watch_console(pid, paused);
            }
        }
        // Read once, as iTerm2 does ("will not affect existing sessions until you restart").
        self.scroll_ticks.insert(id, (self.globals.bool("ShowLocationsInScrollbar"), self.globals.bool("ShowMarksInScrollbar")));
        self.panes.insert(id, pane);
        self.apply_emu_rules(id);
        // "Default Pane Locked" is read when a session first loads its profile, not at a restart.
        if !relaunch {
            self.apply_new_pane_defaults(id);
        }
        // Idle time and the other per-session start reads: a restarted shell is a new session (S4, P21).
        self.apply_session_start(id);
        // Automatic logging at the start, the restart and a restore (`PS:3549-3560`).
        self.start_auto_log(id);
        // A session of a saved profile is a recent (`SF:393-397`).
        if !relaunch {
            let saved = self.store.shared_guid(&p.guid());
            if let Some((guid, name)) = self.store.profiles.iter().find(|q| q.guid() == saved).map(|q| (q.guid(), q.name())) {
                self.note_recent(&guid, &name);
            }
        }
    }

    /// Instant Replay: every pane, background tabs and tmux panes included, records a frame
    /// of its own screen when it changed, at iTerm2's update cadence for it
    /// (`cadence::frame_gap`; iTerm2 records at each refresh of every session).
    pub(super) fn record_replay_frames(&mut self) {
        let now = Instant::now();
        let now_us = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as i64);
        let visible = self.replay_visible_panes();
        let prefs = self.cadence_prefs();
        for (id, p) in self.panes.iter_mut() {
            // No gap is shorter than 1 ms (`interval`'s floor): a pane that captured within it
            // cannot record yet, so its gap is not worked out.
            if p.replay_at.is_some_and(|t| now.saturating_duration_since(t) < Duration::from_millis(1)) {
                continue;
            }
            let gap = p.replay_gap(now, visible.contains(id), self.activity.get(id).and_then(|a| a.last_output()), &prefs, false);
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
        let prefs = self.cadence_prefs();
        let mut due = None;
        for (id, p) in self.panes.iter_mut() {
            let Some(at) = p.replay_at.filter(|_| !p.gateway) else { continue };
            let t = at + p.replay_gap(now, visible.contains(id), self.activity.get(id).and_then(|a| a.last_output()), &prefs, false);
            if t > now && due.is_none_or(|d| t < d) {
                due = Some(t);
            }
        }
        due
    }

    /// The folder and the variables' pane a new session's profile directory comes from. A new
    /// window reads the previously current window's focused pane (captured before the empty
    /// window swapped in, R3); a split reads the pane it splits; a tab the focused pane.
    fn start_source(&self) -> (Option<PathBuf>, Option<PaneId>) {
        use crate::start_dir::Object;
        let object = self.start_object;
        let scope = match object {
            Object::Window => self.window_previous_pane,
            Object::Pane => self.start_from.or_else(|| self.focused_id()),
            Object::Tab => self.focused_id(),
        };
        let here = match object {
            Object::Window => None,
            _ => scope.and_then(|id| self.pane_dir(id)),
        };
        (crate::start_dir::previous_for(object, here, self.window_previous_dir.clone()), scope)
    }

    /// The directory a profile starts a session in (`start_dir::resolve`). The custom text is
    /// interpolated in the source pane's scope only when that pane is local: a remote pane's
    /// variables are set by its program, so they are never an expansion scope (B4).
    fn profile_start_dir(&self, p: &Profile) -> (PathBuf, Option<String>) {
        let (previous, scope) = self.start_source();
        let local = scope.filter(|id| self.panes.contains_key(id) && !self.remote_text(*id));
        let expand = |text: &str| match local {
            Some(id) => self.evaluate_for(id, text).unwrap_or_else(|_| text.to_string()),
            None => text.to_string(),
        };
        let mode = crate::start_dir::mode(p, self.start_object);
        crate::start_dir::resolve(&mode, previous.as_deref(), &profile::home(), &expand, &|d| crate::localfs::is_local_dir(d))
    }

    /// The focused session's directory, from shell integration, when it is on this machine
    /// (iTerm2 prefers the local directory for a new session).
    fn previous_dir(&self) -> Option<std::path::PathBuf> {
        // Through the guard: a framing pane's reported folder is remote text (P10).
        self.pane_dir(self.tabs.get(self.active)?.focused)
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
            s.edit_session = None;
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
                s.sleep_status = self.sleep_status.clone();
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
        // The one gate (B2): a preview only redraws; any other edit first puts a stale preview back,
        // in either window, so the previewed colors are never saved.
        let preview = matches!(edit, Edit::Preview);
        let (store, settings, profiles_win) = (&mut self.store, &mut self.settings, &mut self.profiles_win);
        let go = crate::settings_ui::gate_edit(&edit, &mut || {
            for w in [settings.as_mut(), profiles_win.as_mut()].into_iter().flatten() {
                w.restore_preview(store);
            }
        });
        if !go {
            if preview {
                self.refresh_panes();
                self.redraw();
            }
            return;
        }
        // A rewritable dynamic profile's edit goes back to its file once it settles.
        if let Edit::Changed(guid) = &edit {
            self.dynamic_edited(guid);
            // A session's own copy edited in Edit Session: what it overrides is worked out again.
            self.store.refresh_overridden(guid);
        }
        match edit {
            Edit::None | Edit::Preview => return,
            Edit::SessionHotkey(pane, s) => self.set_session_hotkey(pane, s),
            Edit::SessionEdit(pane, values) => self.session_edit(pane, values),
            Edit::OpenMany(guids, how) => self.open_selected_profiles(guids, how, crate::settings_ui::Opener::ProfilesWindow),
            // Taken by the window's redraw handler, which has the event loop.
            Edit::EditProfiles(_) => return,
            Edit::Error(e) => self.banners.set("import", e),
            Edit::Alert { heading, text } => {
                let id = crate::trigger_file::identifier_for(&heading, &text);
                self.ask_scripts(scripts_alerts::ask(&heading, &text, &[crate::api_auth::OK], Some(id), crate::warnings::Kind::Persistent, scripts_alerts::Then::Acknowledged));
            }
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
                // A deleted profile's ssh: binding goes with it, and an import's iTerm2 Guids that
                // match nothing are dropped (P11).
                let pruned = crate::url_schemes::prune_globals(&mut self.globals, &self.store);
                if matches!(edit, Edit::Imported(_)) || pruned {
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
        // The workgroups editor wrote "Workgroups" and its marks: the running model follows.
        self.workgroup_model = crate::workgroup::Model::load(&self.globals).0;
        // Job updates wait on a job's exit, or only poll (Advanced > Experimental Features).
        self.jobs.set_fast_updates(self.globals.bool("FastForegroundJobUpdates"));
        self.jobs.set_ancestry_diagnostics(self.globals.bool("LogForegroundJobAncestryDiagnostics"));
        if api_now {
            self.api_settings_changed();
        }
        // The Actions tool shows the actions.
        self.tools_changed();
        self.settle_action_undo();
        self.menus_dirty = true;
        self.sync_checks();
        self.remap_setting_changed();
        self.locale_update();
        self.register_hotkeys();
        self.scripts_folder_changed();
        // WebUserAgent or BrowserPluginPathHint may have changed.
        self.web_settings_changed();
        // The Theme may have changed: every window follows the forced mode (or Windows' again).
        self.apply_theme_everywhere();
        // The border settings, or the Theme, may have changed.
        self.apply_border_everywhere();
        // An emulation knob (Advanced > Terminal) may have changed.
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        // PathsToIgnore or EnableSemanticHistoryOnNetworkMounts may have changed.
        let probe = self.path_probe();
        for id in ids {
            self.apply_emu_rules(id);
            if let Some(p) = self.panes.get_mut(&id) {
                p.screen.set_path_probe(probe.clone());
            }
        }
        self.tmux_resend_colors();
        // AllowLiveResize may have changed.
        self.apply_resizable_everywhere();
        // Title bars, the status bar's height or place, or SplitPaneDividerWidth may have
        // changed (13h P13, P21; 13i S11): panes gain or lose room in every window, parked
        // ones and their tmux reports too.
        self.relayout_changed_windows();
        // Disable Transparency or a profile value may have changed the blur.
        self.sync_blur();
        self.refresh_scroll_style();
        self.presence_globals_changed();
        self.bonjour_sync();
    }

    /// Starts watching and scanning the Scripts folder the settings name (`FM:236-252`).
    fn scripts_scan_start(&mut self) {
        let keys = (self.globals.bool("UseCustomScriptsFolder"), self.globals.string("CustomScriptsFolder"));
        let folder = self.scripts_folder();
        // Reached through a link to a network path: the scanner refuses it (an empty tree,
        // nothing made, watched or polled), and the app-wide entry says why once.
        if !crate::scripts::ancestors_stay_local(&folder, &crate::netpath::read_reparse) {
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
        let bidi = crate::bidi::Rules::from_globals(&self.globals);
        let copy = crate::seltext::Options::from_globals(&self.globals);
        for pane in self.panes.values_mut() {
            let p = self.store.get(&pane.profile).unwrap_or_else(|| self.store.default_profile());
            pane.restyle(Palette::from_profile(p, dark), Look::from_profile(p));
            pane.set_bidi_rules(bidi);
            pane.set_copy_options(copy);
            screen_follows_profile(pane.own_mut(), p);
        }
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        for id in ids {
            self.apply_emu_rules(id);
        }
        // A browser pane's trigger list follows its profile (13x5 Task 9).
        self.browser_triggers_refresh();
        self.tmux_resend_colors();
        self.tmux_save_tab_colors();
        // "Status bar enabled" may have changed for a tmux tab.
        self.tmux_watch_status_all();
        // A profile edit may have changed a pane's fonts: its face is measured now.
        self.ensure_faces();
        // A profile edit may make a window need an alpha surface.
        self.check_alpha_surfaces();
        self.sync_blur();
        // A pane's alpha may have changed: the border rule reads it.
        self.apply_border_everywhere();
        self.redraw();
    }

    fn save_now(&mut self) {
        self.save_due = None;
        self.tools_changed();
        match self.save_store() {
            Ok(()) => self.banners.clear("save"),
            Err(e) => self.banners.set("save", format!("Could not save settings: {e}")),
        }
        self.prefs_changed();
    }

    /// The one save of the profile store: a color preview still live in either settings window is
    /// put back first (the panes show the profile again), so nothing writes previewed colors (B2).
    pub(super) fn save_store(&mut self) -> Result<(), String> {
        let (settings, profiles_win) = (&mut self.settings, &mut self.profiles_win);
        let (restored, saved) = crate::settings_ui::save_restored(&mut self.store, &mut |store| {
            let mut any = false;
            for w in [settings.as_mut(), profiles_win.as_mut()].into_iter().flatten() {
                any |= w.restore_preview(store);
            }
            any
        });
        if restored {
            self.refresh_panes();
            self.redraw();
        }
        saved
    }

    /// Takes the renderer's fonts from the default profile. A font that is not
    /// installed is named in the banner.
    fn apply_fonts(&mut self) {
        let p = self.store.default_profile().clone();
        let mut normal = p.font("Normal Font");
        let mut alt = p.bool("Use Non-ASCII Font").then(|| p.font("Non Ascii Font").family);
        // Re-measuring the font re-shapes all text, so only do it when a font setting changed.
        // The settings are compared before the font names are looked up (every frame asks).
        let sig = format!("{normal:?} {alt:?} {} {} {} {} {} {} {}", p.float("Horizontal Spacing"), p.float("Vertical Spacing"), self.scale(), p.font("Badge Font").family, self.globals.string("FontsForGenerousRounding"), self.globals.int("MinimumWeightDifferenceForBoldFont"), self.globals.bool("EnableContextualAlternates"));
        if sig == self.font_sig {
            return;
        }
        self.font_sig = sig;
        // A held slice (Reduce Flicker) was drawn at the old cells: it never outlives a font or scale change.
        for p in self.panes.values_mut() {
            p.held = None;
            p.held_until = None;
        }
        if let Some(g) = self.gpu.as_ref() {
            normal.family = g.renderer.resolve_family(&normal.family);
            alt = alt.map(|a| g.renderer.resolve_family(&a));
        }
        let generous = self.globals.string("FontsForGenerousRounding");
        let bold_difference = self.globals.int("MinimumWeightDifferenceForBoldFont") as i32;
        let calt = self.globals.bool("EnableContextualAlternates");
        let Some(g) = self.gpu.as_mut() else { return };
        let px = normal.size_pt * 4.0 / 3.0 * g.window.scale_factor() as f32;
        // The other panes' faces re-measure in place at a new scale (their ids stay valid).
        g.renderer.set_scale(g.window.scale_factor() as f32);
        g.renderer.set_generous_list(&generous);
        g.renderer.set_bold_difference(bold_difference);
        g.renderer.set_contextual_alternates(calt);
        g.renderer.set_fonts(&normal.family, alt.as_deref(), px, p.float("Horizontal Spacing") as f32, p.float("Vertical Spacing") as f32);
        g.renderer.set_badge_family(&badge_ui::badge_family(badge_ui::profile_font(&p), &self.globals.string("BadgeFont")));
        g.renderer.set_badge_bold(self.globals.bool("BadgeFontIsBold"));
        // Read with the window's other fonts, not on every change (iTerm2: "Changing this
        // setting does not affect existing windows").
        g.renderer.set_toolbelt_family(&self.globals.string("ToolbeltFont"));
        g.renderer.set_toolbelt_size(self.globals.get("ToolbeltFontSize").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(0.0) as f32);
        let missing: Vec<String> = std::iter::once(normal.family).chain(alt).filter(|f| !g.renderer.has_family(f)).collect();
        if missing.is_empty() {
            self.banners.clear("font");
        } else {
            self.banners.set("font", format!("Font not installed: {}. Text uses a fallback font.", missing.join(", ")));
        }
        self.ensure_faces();
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
        // A browser profile's window is its Width and Height in points, one point to the cell
        // (`TWSH:170-176`).
        let browser = self.is_browser_profile(&p);
        let (cw, ch) = if browser { (self.scale(), self.scale()) } else { self.cell() };
        let (side, vert) = self.margins();
        // A legacy scrollbar's strip is part of the window's room around the grid, so a new
        // window opens with its columns.
        let (padx, pady) = (2.0 * side + self.scroll_room(), 2.0 * vert);
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
        let (columns, rows) = if browser {
            let (c, r) = crate::browser::window_cells(p.int("Width in Points"), p.int("Height in Points"));
            (i64::from(c), i64::from(r))
        } else {
            (columns, rows)
        };
        let (mut w, mut h) = (columns as f32 * cw + padx, rows as f32 * ch + pady + line);
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
        self.new_tab_as(guid, true);
    }

    /// `by_person` false is a script's or the API's tab, which leaves a pick on (P15).
    fn new_tab_as(&mut self, guid: Option<&str>, by_person: bool) {
        if !self.tabs.is_empty() && !self.layout_allowed(crate::winmodel::LayoutOp::AddTab) {
            return;
        }
        self.pane_move_event(pane_move_ui::PickEvent::NewSession { by_person });
        self.add_tab(guid);
    }

    /// A new tab in the current window without the Lock Layout check: the API's CreateTab
    /// (iTerm2 guards user tab creation through menu validation, `PT:13832`, and its API
    /// handler has no lock check, `H:2465-2588`).
    fn add_tab(&mut self, guid: Option<&str>) {
        if self.wake_from_resident(resident_ui::Wake::Window(guid)) {
            return;
        }
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
            self.check_alpha_surfaces();
            self.relayout();
        }
    }

    fn split(&mut self, axis: Axis, guid: Option<&str>) {
        self.split_as(axis, guid, true);
    }

    /// `by_person` false is a script's or the API's split, which leaves a pick on (P15).
    fn split_as(&mut self, axis: Axis, guid: Option<&str>, by_person: bool) {
        if !self.layout_allowed(crate::winmodel::LayoutOp::Split) {
            return;
        }
        // No split while Instant Replay shows (`PT:11124-11129`).
        if crate::replay::on_event(self.in_instant_replay(), crate::replay::ReplayEvent::Split) == crate::replay::Close::Refuse {
            return;
        }
        let Some(target) = self.tabs.get(self.active).map(|t| t.focused) else { return };
        self.pane_move_event(pane_move_ui::PickEvent::NewSession { by_person });
        if self.tmux_split(target, axis == Axis::Row) {
            return;
        }
        let tab = &self.tabs[self.active];
        let target_was_active = target == tab.focused;
        let area = self.content_area();
        let Some((_, rect)) = tab.layout.rects(area, self.gap()).into_iter().find(|(id, _)| *id == target) else { return };
        let half = match axis {
            Axis::Row => Rect { w: rect.w / 2.0, ..rect },
            Axis::Column => Rect { h: rect.h / 2.0, ..rect },
        };
        self.start_tab = Some(self.active);
        (self.start_object, self.start_from) = (crate::start_dir::Object::Pane, Some(target));
        let started = self.start_pane(half, guid);
        (self.start_object, self.start_from) = (crate::start_dir::Object::Tab, None);
        self.start_tab = None;
        if let Some(id) = started {
            let tab = &mut self.tabs[self.active];
            tab.layout.split(target, axis, id);
            if pane_nav_ui::focus_new_split(self.globals.bool("FocusFollowsMouse"), self.globals.bool("FocusNewSplitPaneWithFocusFollowsMouse"), target_was_active) {
                self.tabs[self.active].focused = id;
            }
            self.tabs[self.active].zoomed = None;
            self.check_alpha_surfaces();
            self.apply_width_pins();
            self.relayout();
        }
    }

    /// Closes a pane. The last pane closes its tab, and the last tab exits.
    fn close_pane(&mut self, id: PaneId, event_loop: &ActiveEventLoop) {
        // Any member of a workgroup ending takes the instance down (`WI:257-296`).
        self.workgroup_member_ended(id);
        // The pane's own window is current for all that follows: an archive of a pane closed in
        // a background window is built from that window's frame, style and title.
        self.enter_pane_window(id);
        // Archive On Closure writes first, while the pane still has its text (`PS:4057-4065`).
        self.auto_archive(id);
        // An ended session closed too soon warns first (`terminate`, `PS:3887`).
        self.warn_short_lived(id);
        // The source ending cancels the pick (`PS:3940-3942`).
        self.pane_move_event(pane_move_ui::PickEvent::Ended(id));
        // Waiting browser tools answer "the browser session was closed" first; the unlink below
        // would otherwise answer them "page changed".
        self.ai_browser_pane_gone(id);
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
        // Its Find Globally rows go with it.
        self.global_pane_gone(id);
        // A git command's window closed any way ends its command, as does Manage
        // Dependencies' pip command.
        self.single_use_ended(id);
        self.deps_command_ended(id);
        // Its Command Info window goes with it.
        self.cmd_button_hits.retain(|(p, ..)| *p != id);
        if self.pathnav_due.is_some_and(|(_, p, _)| p == id) {
            self.pathnav_due = None;
        }
        if self.command_click.is_some_and(|(_, p, _)| p == id) {
            self.command_click = None;
        }
        if self.command_info.is_some_and(|i| i.pane == id) {
            self.command_info = None;
        }
        self.pathnav_press = self.pathnav_press.filter(|(p, _)| *p != id);
        if self.cmdinfo.as_ref().is_some_and(|w| w.pane == id) {
            self.cmdinfo = None;
        }
        // A timed restart of a pane that is gone must not fire.
        self.pending_restarts.remove(&id);
        self.restart_history.remove(&id);
        self.param_pane_gone(id);
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
        self.session_hotkey_pane_gone(id);
        self.aps.remove(&id);
        self.focus_order.remove(&id);
        self.forget_scrollbar(id);
        if let Some(mut p) = self.panes.remove(&id) {
            // Nothing is logged after the person closed the pane (`PS:4728`); a pane kept for Undo
            // Close below logs nothing either while it is paused, and keeps its log (P15).
            if self.undo_capture.is_none() || p.exited.is_some() || p.tmux_io().is_some() || p.dvr.read_only {
                p.stop_log();
            }
            // Its file requests will never be answered: they fail now, and its waiters go.
            self.ssh_files_pane_gone(id);
            if let Some(pid) = p.shell_pid().or(p.exit_pid()) {
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
        self.bell_indicators.remove(&id);
        self.bell_badge.forget(&id.to_string());
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
        self.bookmark_editor_close_for(id);
        if let Some(t) = self.tabs.iter().position(|t| t.layout.panes().contains(&id)) {
            let tab = &mut self.tabs[t];
            if tab.layout.remove(id) {
                if tab.focused == id {
                    tab.focused = tab.layout.panes()[0];
                }
            } else {
                let gone = self.tabs.remove(t);
                self.mru.forget(gone.id);
                self.pane_seen.remove(&gone.id);
                self.api_vars.forget(crate::api_vars::Scope::Tab, &gone.id.to_string());
                self.active = crate::close_rules::active_after_close(self.active, t, self.tabs.len(), self.globals.bool("MoveLeftAfterClosingTab"));
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
            // iTerm2's rule for the last window (`AD:915-942`, R3): Tandem stays resident unless
            // "Quit when all windows are closed" says otherwise (and the grace period has passed).
            if self.windows.parked().next().is_none() && self.after_last_window_now() == crate::resident::AfterLast::StayResident {
                self.prefs_gpu = None;
                return self.go_resident_after_last();
            }
            if !self.drop_current_window() {
                self.save_pending_settings();
                self.save_empty_state();
                self.flush_notes();
                // The last window closed: its logs are written out, its shells end (iTerm2), and the
                // host hears it.
                // Panes held for Undo Close (the stack, and this close's own capture) release their
                // logs too: nothing else would wait for their writers.
                self.clear_undo();
                if let Some(c) = self.undo_capture.as_mut() {
                    for p in &mut c.panes {
                        p.stop_log();
                    }
                }
                self.finish_archives(false, std::time::Duration::from_secs(5));
                self.close_all_logs();
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
        self.paced_redraw(outputs, now);
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
        // Each pane's grid comes from its own cell: the faces are measured first.
        self.ensure_faces();
        let sizes: Vec<(PaneId, (u16, u16), (f32, f32))> = self
            .tabs
            .iter()
            .flat_map(|t| self.visible(t))
            .map(|(id, rect)| (id, self.grid_size_for(id, rect), self.pane_cell(id)))
            .collect();
        self.pane_faces.mark_laid();
        self.tmux_report_sizes();
        let mut reframe = Vec::new();
        let mut resize_reports = Vec::new();
        for (id, (cols, rows), (cw, ch)) in sizes {
            if let Some(p) = self.panes.get_mut(&id) {
                if p.resize(cols, rows) {
                    crate::dlog!("pane {} resized to {} columns {} rows", id, cols, rows);
                    self.activity.entry(id).or_default().resized(Instant::now());
                    if p.replay.is_some() {
                        reframe.push(id);
                    }
                    // DEC mode 2048 (13o Task 6): the new grid and the text area it covers in
                    // physical pixels from the pane's own cell (13n), written after the pty
                    // was resized.
                    // The grid the pane really has (it keeps 2x1 when squeezed), not the request.
                    let (cols, rows) = p.clamped_size(cols, rows);
                    if let Some(report) = p.resize_report_if_on(rows, cols, resize_report_px(rows, cols, (cw, ch))) {
                        resize_reports.push((id, report));
                    }
                }
                p.set_cell_px(cw as u16, ch as u16);
            }
        }
        for (id, report) in resize_reports {
            self.write_resize_report(id, &report);
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
        let inner = self.text_rect(id, rect);
        let term = self.panes[&id].screen.term();
        let (col, row, side) = fonts_ui::pointer_cell(self.pane_cell(id), (inner.x, inner.y), x, y, term.columns() as u32, term.screen_lines() as u32, self.neighbor_fraction());
        // A right-to-left row: the pointer's visual cell is a different logical cell, and its
        // halves turn over (the left half of the cell is the character's end). Mouse reports
        // do not use this column: iTerm2 reports the visual one (`report_cell`).
        let right = side == Side::Right;
        let (logical, rtl) = self.panes[&id].screen.logical_col(row as usize, col as usize);
        let side = if right != rtl { Side::Right } else { Side::Left };
        (logical as u32, row, side)
    }

    /// The cell a mouse report names: the VISUAL column under the pointer, as iTerm2 reports it
    /// (`PTYMouseHandler.m:1404`, `mouseHandlerCoordForPointInView`: plain pixel division, no
    /// bidi conversion). Clamped to the grid like `cell_at`.
    fn report_cell(&self, id: PaneId, rect: Rect, x: f32, y: f32) -> (u32, u32) {
        let (cw, ch) = self.pane_cell(id);
        let inner = self.text_rect(id, rect);
        let term = self.panes[&id].screen.term();
        mouse::cell_at((x - inner.x) as f64, (y - inner.y) as f64, cw as f64, ch as f64, term.columns() as u32, term.screen_lines() as u32)
    }

    /// The pointer on screen as a visual selection sees it: grid line, visual column, and
    /// whether it is in the right half of that cell.
    fn visual_pos(&self, id: PaneId, rect: Rect, x: f32, y: f32) -> (i32, usize, bool) {
        let (col, row) = self.report_cell(id, rect, x, y);
        let (cw, _) = self.pane_cell(id);
        let right = crate::seltext::side_for(((x - self.text_rect(id, rect).x) - col as f32 * cw) / cw, self.neighbor_fraction()) == Side::Right;
        let offset = self.panes[&id].screen.term().grid().display_offset() as i32;
        (row as i32 - offset, col as usize, right)
    }

    fn grid_point(&self, id: PaneId, col: u32, row: u32) -> Point {
        let offset = self.panes[&id].screen.term().grid().display_offset();
        viewport_to_point(offset, Point::new(row as usize, Column(col as usize)))
    }

    /// Copy as the person asked for it (Edit > Copy, a key, copy mode): the selection's text
    /// goes to the clipboard, except an empty one with `DisallowCopyEmptyString` (`TV:4089-4150`).
    fn copy_selection(&mut self, id: PaneId) {
        self.copy_selection_with(id, None);
    }

    /// `copy_selection` with the text already extracted, when the caller has it.
    fn copy_selection_with(&mut self, id: PaneId, text: Option<&str>) {
        if self.copy_porthole_selection(id, porthole_ui::CopyAs::Plain) {
            return;
        }
        let Some(text) = text.map(str::to_string).or_else(|| self.selected_text(id)) else { return };
        if !crate::seltext::copy_decision(&text, self.globals.bool("DisallowCopyEmptyString")) {
            return;
        }
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
                self.step_pane(action == Action::NextPane);
            }
            Action::Focus(dir) => self.select_pane_dir(dir),
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
    fn refresh_route(&mut self) {
        let mut snapshot = self.with_route(keyroute::Snapshot::new);
        snapshot.composer = self.composer_has_keys();
        snapshot.respect_system = self.globals.bool("AllowSymbolicHotKeys");
        snapshot.profile_shortcuts = keyroute::claimed_shortcuts(self.store.profiles.iter().map(|p| crate::profile_menu::stored_key(&p.string("Shortcut"))));
        (snapshot.peer_digits, snapshot.peer_keys) = self.peer_route_keys();
        snapshot.peer_keys.extend(self.toolbar_route_keys());
        *self.route_snapshot.write().unwrap() = snapshot;
        let exempt: toml::Table = self
            .globals
            .table("GlobalKeyMap")
            .into_iter()
            .filter(|(_, v)| crate::keyaction::Binding::from_value(v).is_some_and(|b| b.action == 22))
            .collect();
        *self.exempt_map.write().unwrap() = exempt;
        *self.remap_tags.write().unwrap() = crate::remap::Tags::from_globals(&self.globals);
        // Every focus, tab, window and profile change comes through here (B6): Edit Session
        // follows the active session.
        self.follow_edit_session();
        self.browser_gates_refresh();
    }

    /// Saves a custom scripts folder typed in the open Settings window and not yet applied,
    /// before Tandem ends, so nothing typed is lost.
    fn save_pending_settings(&mut self) {
        // A color preset still previewed goes back before anything is saved (B2).
        for slot in [&mut self.settings, &mut self.profiles_win] {
            if let Some(w) = slot.as_mut() {
                w.restore_preview(&mut self.store);
            }
        }
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

    /// The pane's mouse modes, or none when mouse reporting is off for it (or restricted to the
    /// alternate screen and the pane is not in it).
    fn modes(&self, id: PaneId) -> mouse::Modes {
        let Some(p) = self.panes.get(&id) else { return mouse::Modes::default() };
        let restrict = self.store.get(&p.profile).is_some_and(|prof| prof.bool("Restrict Mouse Reporting to Alternate Screen Mode"));
        if mouse::reporting_on(self.mouse_allowed(id), p.screen.term().mode().contains(TermMode::ALT_SCREEN), restrict) {
            p.screen.mouse_modes()
        } else {
            mouse::Modes::default()
        }
    }

    fn pointer_prefs(&self) -> pointer::Prefs {
        pointer::Prefs { pass_on_control_click: self.globals.bool("PassOnControlClick"), report_right_click: self.globals.bool("ReportRightClick") }
    }

    /// The multi-click count a press of `button` here would be, 1 to 4, without recording it.
    fn next_os_clicks(&self, button: u8) -> u8 {
        let now = Instant::now();
        let (x, y) = self.cursor;
        let near = 4.0 * self.scale();
        match self.pointer_last {
            Some((t, b, (px, py), n)) if b == button && now.duration_since(t) <= input::MULTI_CLICK && (x - px).abs() <= near && (y - py).abs() <= near => (n + 1).min(4),
            _ => 1,
        }
    }

    /// The multi-click count for a press of `button` here: 1 to 4.
    fn os_clicks(&mut self, button: u8) -> u8 {
        let count = self.next_os_clicks(button);
        self.pointer_last = Some((Instant::now(), button, self.cursor, count));
        count
    }

    /// What a key sends to the terminal, with the focused profile's Keys options.
    fn key_bytes(&self, key: &Key, base: Option<&Key>, text: Option<&str>, keypad: bool) -> Option<Vec<u8>> {
        let t = self.tabs.get(self.active)?;
        let pane = self.panes.get(&t.focused)?;
        let mode = *pane.own().term().mode();
        let opts = self.store.get(&pane.profile).map_or(KeyOptions { left_option: 2, right_option: 2, option_is_alt: true, app_keypad_allowed: false, left_changeable: true, right_changeable: false, right_unset: false }, KeyOptions::from_profile);
        // DECSET 1036 (Advanced SupportDecsetMetaSendsEscape): a program may switch a changeable side to Esc+.
        let support = self.globals.bool("SupportDecsetMetaSendsEscape");
        let meta = pane.own().meta_sends_escape();
        let opts = KeyOptions { left_option: opts.effective(AltSide::Left, meta, support), right_option: opts.effective(AltSide::Right, meta, support), ..opts };
        keys::press_bytes_with(key, base, text, self.mods, self.alt_side, mode.contains(TermMode::APP_CURSOR), mode.contains(TermMode::APP_KEYPAD), keypad, &opts)
    }

    fn key(&mut self, ev: &KeyEvent, event_loop: &ActiveEventLoop) {
        // A key in a session ends the quit grace period (`PTYTextView.m:7505-7509`).
        if ev.state == ElementState::Pressed {
            self.note_interaction();
        }
        // Remap modifiers globally off: this key is read with the remapped modifiers.
        let remapped = self.remapped_key(ev);
        let ev = remapped.as_ref().unwrap_or(ev);
        // iTerm2's `didHandleKeystroke`: an echo within 0.1 s of it draws at once.
        if ev.state == ElementState::Pressed {
            self.last_keystroke = Some(Instant::now());
        }
        // A key press is new input: a cancel stops waiting for the stray commit of the cancelled
        // composition (before the hold below, so the Esc that cancels sets the wait again).
        if ev.state == ElementState::Pressed {
            self.ime.input_started();
        }
        // While the focused pane has a composition its keys are the IME's, not the session's;
        // Esc cancels the composition first.
        if self.ime_holds_key(ev) {
            return;
        }
        // Esc ends a title bar drag (the drag session cancels, nothing moves).
        if ev.state == ElementState::Pressed && ev.logical_key == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape) && self.title_drag_lost(pane_move_ui::DragLost::Escape) {
            return;
        }
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
        // A workgroup's base field with the keyboard takes its keys, releases too.
        if self.base_field_key(ev) {
            return;
        }
        // A review member's prompt field with the keyboard takes its keys, releases too.
        if self.review_overlay_key(ev) {
            return;
        }
        // A browser pane's address field takes its keys, releases too; Ctrl+L and F6 focus it.
        if self.browser_key(ev) {
            return;
        }
        // The API's key up and flags changed notifications.
        self.api_key_early(ev);
        if self.protocol_key_event(ev) || ev.state != ElementState::Pressed {
            return;
        }
        // A Win chord made by an in-app remap never passed the hook, which takes real Win chords
        // for the menu commands: it takes the same road.
        if self.remap_in_app_on() && self.mods.cmd && !self.phys_mods.cmd {
            if let Some(chord) = crate::winkeys::vk_to_key(crate::winkeys::vk_of(ev.physical_key)).map(|key| crate::menus::Chord { key, cmd: true, alt: self.mods.alt, shift: self.mods.shift, ctrl: self.mods.ctrl }) {
                return self.run_chord(&chord, event_loop);
            }
        }
        if self.bar_key(ev, event_loop) || self.upload_key(ev) {
            return;
        }
        self.alt_infer_key(ev);
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
                let pick = self.tabs.get(self.active).and_then(|t| self.pane_by_number(t, d));
                if let Some(id) = pick {
                    self.focus_quietly(self.active, id);
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
                // After the global bindings (the route above): a workgroup's peer shortcuts, then the
                // toolbar's (Task 5) and the menu's.
                if self.peer_key(&press.stroke, chord) {
                    self.relayout();
                    return;
                }
                if let Some(pane) = self.focused_id().filter(|p| self.workgroup_toolbar_member(*p)) {
                    if self.workgroup_toolbar_shortcut(pane, &press.stroke) {
                        self.relayout();
                        return;
                    }
                }
                if let Some(c) = chord {
                    // A menu chord, then a profile's Ctrl+Win shortcut (`keyroute::terminal_chord`);
                    // neither sends bytes to the session, so neither clears a selection (S10).
                    let shortcuts = self.route_snapshot.read().unwrap().profile_shortcuts.clone();
                    match keyroute::terminal_chord(&shortcuts, c) {
                        keyroute::ChordAction::Menu(item) => self.run_command(item, event_loop),
                        keyroute::ChordAction::Profile { key, window } => {
                            if let Some(guid) = self.profile_for_shortcut(key) {
                                self.open_profile(&guid, window);
                            }
                        }
                        keyroute::ChordAction::Nothing => {}
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

    /// A pane showing a frozen screenshot copy takes no keys until its panel closes, nor
    /// a replaying pane (the replay view has no shell).
    pub(super) fn terminal_refuses_keys(&self) -> bool {
        self.shot.as_ref().is_some_and(|s| Some(s.pane) == self.focused_id()) || self.focused_id().and_then(|id| self.panes.get(&id)).is_some_and(|p| p.replay.is_some())
    }

    /// Filter and Zoom views are read-only: typing ends the view. True when one ended.
    pub(super) fn end_read_only_view(&mut self) -> bool {
        let Some(id) = self.focused_id().filter(|id| self.panes.get(id).is_some_and(|p| p.live.is_some())) else { return false };
        self.end_view(id);
        true
    }

    /// A key with no binding: the Windows extras, movement-key scrolling, else the pane.
    fn to_terminal(&mut self, key: &Key, base: Option<&Key>, bytes: Option<Vec<u8>>, event_loop: &ActiveEventLoop) {
        if let Some(action) = base.and_then(|b| keys::shortcut(b, self.mods)) {
            return self.action(action, event_loop);
        }
        if self.terminal_refuses_keys() {
            return;
        }
        if self.end_read_only_view() && matches!(key, Key::Escape) {
            return;
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
            let sender = pane.id;
            pane.scroll(Scroll::Bottom);
            // A key for the session removes a finished selection, not one being dragged (R3).
            // Only a key with bytes: a bare Win press for a Win-drag keeps it (B4).
            let live = matches!(&self.drag, Drag::Select { pane: dragged, .. } if *dragged == sender);
            if selection_ui::typing_clears(live, self.globals.bool("TypingClearsSelection")) {
                if let Some(p) = self.panes.get_mut(&sender) {
                    p.clear_selection();
                }
            }
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
        // The subclass's flag is taken for every press (a click on the tab bar or a status bar
        // that activated the window must not leave it for the next terminal click), and the
        // last click's hold ends; the terminal part applies it after every UI hit test (B6).
        let first = self.first_mouse();
        self.first_mouse_down = false;
        // Any press ends a navigator click waiting out the double-click time: a second click
        // cancels it (`TV:7924-7933`).
        self.pathnav_due = None;
        self.pathnav_press = None;
        // A second click cancels a command click waiting out the double-click time
        // (`mouseHandlerCancelSingleClick:`), and a press ends the first-selection notice.
        self.command_click = None;
        if self.command_info_press(button) {
            return;
        }
        self.hide_tab_tip();
        // A press starts over on the tab bar too: a bar press whose release never came (focus
        // went mid-press) must not take this press's release; a press on the bar stores itself.
        self.tab_press = None;
        self.title_press = None;
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
        // While a pick is on, a left press is a drop or a cancel, title bars and bars included.
        if self.pane_move_press(button, x, y, event_loop) {
            return;
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
        if button == MouseButton::Left && self.navigator_press(x, y) {
            return;
        }
        if button == MouseButton::Left && (self.info_bar_press(x, y, event_loop) || self.porthole_press(x, y) || self.block_press(x, y) || self.cmd_button_press(x, y, event_loop)) {
            return;
        }
        // A divider's grab zone is Tandem's for every button: the left drags it, the others
        // do nothing (a program never sees a press meant for a divider).
        if let Some(d) = self.divider_at(x, y) {
            if button == MouseButton::Left {
                self.tmux_drag_start(&d.path, d.axis);
                self.drag = Drag::Divider { path: d.path, axis: d.axis, parent: d.parent };
            }
            return;
        }
        // A Diff member's waiting panel covers its terminal: every press on it is its own.
        if self.review_overlay_press(button, x, y) {
            return;
        }
        if self.diff_overlay_press(button, x, y) {
            return;
        }
        // A workgroup member's session toolbar, before a browser pane's (they never overlap).
        if let Some(pane) = self.workgroup_toolbar_at(x, y) {
            if button == MouseButton::Left && self.workgroup_toolbar_press(pane, x, y, event_loop) {
                return;
            }
            if button != MouseButton::Left {
                return;
            }
        }
        // A browser pane's toolbar, and the suggestions list under its address field.
        if let Some(pane) = self.browser_bar_at(x, y) {
            if button == MouseButton::Left && self.browser_toolbar_press(pane, x, y, event_loop) {
                return;
            }
            if button != MouseButton::Left {
                return;
            }
        }
        // After the divider grab (P7), so a lower pane's title bar does not take the strip.
        if self.title_hit_at(x, y).is_some() {
            let clicks = self.os_clicks(button_number(button));
            if self.title_press(button, clicks, x, y, event_loop) {
                return;
            }
        }
        if button == MouseButton::Left && self.find_press(x, y) {
            return;
        }
        // Any button on one of Tandem's overlays inside a pane is Tandem's, never the program's (B5).
        if self.tandem_ui_at(x, y) {
            return;
        }
        // The offscreen command line scrolls to its command (`PTYTextView.m:7885-7896`): after
        // every Tandem surface that can sit over its strip, a single click that did not activate
        // the window (`clickCount == 1`); the band covers the pane's scrollbar, so it comes first.
        if button == MouseButton::Left && offscreen_ui::click_allowed(first, self.next_os_clicks(0)) && self.offscreen_click(x, y) {
            self.os_clicks(0);
            return;
        }
        // The scrollbar is the pane's own, below Tandem's overlays and before any report.
        // The click that activated the window only activates it, on the bar too.
        if self.bar_press_only_activates(first, x, y) {
            return;
        }
        if self.scroll_press(button, x, y) {
            return;
        }
        let Some((id, rect)) = self.pane_at(x, y) else { return };
        self.alt_infer_non_scroll(id);
        let b = button_number(button);
        // The click that activated the window (`mouseDownImpl:`): only now, so a divider, title
        // bar or status bar press is never swallowed and never counts as first mouse.
        // Captured before the press focuses the pane (`MH:278-295`).
        let was_focused = self.focused_id() == Some(id);
        let (kind, counts) = mouse::first_click(first, self.mods.cmd, self.globals.bool("AlwaysAcceptFirstMouse"), self.globals.bool("FocusFollowsMouse"), self.focused_id() == Some(id));
        self.first_mouse_down = counts;
        match kind {
            mouse::FirstClick::ActivateOnly => return,
            mouse::FirstClick::FocusOnly => {
                self.focus_stamped(id);
                return self.redraw();
            }
            mouse::FirstClick::Normal => {}
        }
        // A left click always focuses its pane; right and middle only when the setting says so.
        if b == 0 || self.globals.bool("FocusOnRightOrMiddleClick") {
            self.focus_stamped(id);
        }
        // The press makes the terminal first responder (`notifyActive`): the focused pane's
        // Auto Composer takes the keyboard back; the press still selects in the output.
        if self.focused_id() == Some(id) {
            self.composer_take_back(true);
        }
        let (col, row, side) = self.cell_at(id, rect, x, y);
        let (rcol, rrow) = self.report_cell(id, rect, x, y);
        let mods = self.mods;
        let modes = self.modes(id);
        let mb = match b {
            0 => Some(Button::Left),
            1 => Some(Button::Right),
            2 => Some(Button::Middle),
            _ => None,
        };
        let reportable = mb.is_some() && self.report_gate(id, mouse::Kind::Click);
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
        // A click that only brought the window forward is a plain click (`MH:278-295`).
        let acts = crate::semantic::win_click_acts(self.first_mouse_down, was_focused, self.globals.bool("CmdClickWhenInactiveInvokesSemanticHistory"));
        // Shift+Win-click on a link that opens in Tandem splits the pane (`PTYTextView.m:7987-8043`).
        if b == 0 && mods.cmd && mods.shift && !mods.ctrl && acts && self.globals.bool("CommandSelection") {
            let point = self.grid_point(id, col, row);
            if self.open_link_split(id, point) {
                #[cfg(windows)]
                crate::winkeys::mask_start_menu();
                return self.redraw();
            }
        }
        if b == 0 && mods.cmd && !mods.ctrl && acts && self.globals.bool("CommandSelection") {
            #[cfg(windows)]
            crate::winkeys::mask_start_menu();
            let point = self.grid_point(id, col, row);
            if self.open_target(id, point, false, event_loop) {
                return self.redraw();
            }
        }
        if let (true, Some(mb)) = (reportable, mb) {
            let ev = mouse::Event { button: mb, action: mouse::Action::Press, mods, col: rcol, row: rrow };
            if let Some(bytes) = mouse::encode(modes, ev) {
                self.write_mouse_report(id, &bytes);
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
                // The path mark under a plain single click: the release decides (no drag).
                self.pathnav_press = if count == 1 && plain { self.path_mark_index(id, point).map(|i| (id, i)) } else { None };
                // Win without Alt adds a selection (`MH:360-431`); the earlier ones are held while the new
                // one is made and put back beside it. Win with Ctrl did not reach the open above.
                let append = mods.cmd && !mods.alt;
                // The Win release must not open the Start menu, whether or not CommandSelection
                // masked it above (it is off: a Win-drag still appends).
                #[cfg(windows)]
                if mods.cmd {
                    crate::winkeys::mask_start_menu();
                }
                // A press on the selection waits for a drag of its text (R3, `MH:379-391`).
                let on_selection = count == 1 && !mods.shift && !self.first_mouse_down && self.cell_is_selected(id, point);
                if selection_ui::drag_decision(on_selection, self.globals.bool("RequireCmdForDraggingText"), mods.cmd) == selection_ui::PressKind::MaybeDrag {
                    self.drag = Drag::Out(selection_ui::DragOut { pane: id, from: (x, y), win: mods.cmd, command: plain && !self.first_mouse_down });
                    return self.redraw();
                }
                // Whether this press removes a selection: a single click off it (`MH:337-350`).
                let removed_selection = count == 1 && !mods.shift && !append && self.panes.get(&id).is_some_and(|p| !p.screen.subs().is_empty());
                let carried = self.commit_live_selection(id, append, count > 1);
                let pick = input::click_pick(count, mods, self.globals.bool("DoubleClickPerformsSmartSelection"));
                let smart = pick == input::Pick::Smart && self.smart_select(id, point, false).is_some();
                // A character drag on a bidi row is visual: remember where it began on screen.
                let anchor = (!smart && pick == input::Pick::Type(input::SelectionType::Simple)).then(|| self.visual_pos(id, rect, x, y));
                if !smart {
                    match pick {
                        input::Pick::Smart | input::Pick::Word => self.select_word_at(id, point),
                        input::Pick::Line => self.select_line_at(id, point),
                        input::Pick::Type(ty) => {
                            let mut sel = Selection::new(ty, point, side);
                            if let Some(a) = anchor {
                                sel.set_visual_anchor(a);
                            }
                            let mode = if ty == input::SelectionType::Block { crate::seltext::Mode::Box } else { crate::seltext::Mode::Character };
                            self.panes.get_mut(&id).unwrap().set_selection(Some(sel), mode);
                        }
                    }
                } else {
                    self.smart_window(id, point);
                }
                self.carry_subs(id, carried);
                let alt_only = mods.alt && !mods.shift && !mods.ctrl && !mods.cmd;
                let follow = mouse::press_follow_up(self.first_mouse_down, smart, count, alt_only);
                if follow.copy_now {
                    // iTerm2 copies a selection as soon as it is made.
                    self.copy_on_selection(id);
                }
                // A terminal selection ends any porthole's.
                self.clear_porthole_selections();
                // A Smart Selection is whole; moving the pointer does not reshape it. The
                // click that activated the window never extends or copies one (`MH:778-782`).
                self.drag = if follow.drag_selects { Drag::Select { pane: id, anchor, scroll: None, scroll_off: false, command: (count == 1 && plain && !self.first_mouse_down).then_some(removed_selection) } } else { Drag::None };
                self.alt_click = follow.alt_click.then_some((id, (col, row)));
                self.key_click = (count == 1 && !mods.shift).then_some(id);
            }
            // With no binding and no reporting app, a right click opens the menu (iTerm2's native behavior).
            1 => {
                let point = self.grid_point(id, col, row);
                self.show_context_menu_at(Some((id, Some(point))));
            }
            _ => {}
        }
        self.redraw();
    }

    /// Alt-click moves the cursor (Pointer > Alt-click moves cursor), unless the app
    /// takes clicks or the view is scrolled back.
    fn alt_click_moves_cursor(&mut self, pane: PaneId, target: (u32, u32)) {
        // iTerm2 (`PTYMouseHandler.m:595-607`) asks the terminal's raw mouse mode, not the gates.
        let raw_off = self.panes.get(&pane).is_some_and(|p| mouse::alt_click_may_move_cursor(p.screen.mouse_modes()));
        if !self.globals.bool("OptionClickMovesCursor") || !raw_off {
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
        if self.title_release(button, event_loop) {
            return;
        }
        if self.browser_toolbar_release(button, event_loop) {
            return;
        }
        let b = button_number(button);
        let nav_press = self.pathnav_press.take();
        if let Some(pd) = self.pointer_down.take() {
            if pd.button == b {
                let modes = self.modes(pd.pane);
                let reportable = b <= 2 && self.report_gate(pd.pane, mouse::Kind::Drag);
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
            Drag::Title { pane, .. } => {
                self.title_drag_release(pane, event_loop);
            }
            // The width becomes every new window's (iTerm2 writes it at each move, `RTV:2264-2277`).
            Drag::SideBar { .. } => {
                self.globals.set("LeftTabBarWidth", toml::Value::Float(self.side_bar_preferred as f64));
                self.save_globals();
                self.update_side_handle_hover();
            }
            // Its press went (the tab moved away under it): the drag ends with nothing dropped.
            Drag::Tab { .. } | Drag::Chip { .. } | Drag::AddButton { .. } | Drag::Window { .. } => self.end_tab_drag(),
            Drag::Select { pane, command, .. } => {
                // The text is extracted once (an autoscroll drag over a big buffer is a big extraction)
                // and only when a selection exists; the live one and the extras decide that cheaply.
                let any = self.panes.get(&pane).is_some_and(|p| !p.screen.subs().is_empty());
                let text = if any { self.selected_text(pane).filter(|s| !s.is_empty()) } else { None };
                let empty = text.is_none();
                // A Win-click with no drag (B11): the selection it held comes back as the live one.
                let kept = self.panes.get(&pane).filter(|p| p.screen.live_is_empty()).map(|p| p.screen.extra_subs()).filter(|k| !k.is_empty());
                if let Some(kept) = kept {
                    self.set_subs(pane, kept);
                } else if empty {
                    if let Some(p) = self.panes.get_mut(&pane) {
                        p.clear_selection();
                    }
                    if let Some((p, cell)) = alt_click.filter(|(p, _)| *p == pane) {
                        self.alt_click_moves_cursor(p, cell);
                    }
                    // A click with no drag on a path in the prompt opens the navigator after the
                    // double-click time, ahead of the command selection (`TV:7897-7934`).
                    if let Some((p, i)) = nav_press.filter(|(p, _)| *p == pane) {
                        self.start_navigator_click(p, i);
                    }
                    // A click (no drag) that comes up on the key cursor opens the password
                    // manager.
                    if key_click == Some(pane) && self.released_on_key_cursor(pane) {
                        self.open_password_manager(None, Some(pane));
                    } else if let Some(removed) = command {
                        // Last (`TV:7940-7962`): no fold, note, key cursor or path mark took the click.
                        self.click_after_release(pane, removed);
                    }
                } else if let Some(text) = text {
                    self.selection_ended_with(pane, &text);
                    // iTerm2 copies a selection as soon as it is made.
                    self.copy_on_selection_with(pane, Some(&text));
                    self.chat_offer_selection_with(pane, &text);
                }
            }
            Drag::App { pane, button, clicks, at } => {
                // macOS counts a release as no click once the multi-click time has passed.
                let reported = self.report_gate(pane, mouse::Kind::Drag);
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
                    let (col, row) = self.report_cell(pane, rect, x, y);
                    let modes = self.modes(pane);
                    let ev = mouse::Event { button, action: mouse::Action::Release, mods: self.mods, col, row };
                    if let Some(bytes) = mouse::encode(modes, ev) {
                        self.write_mouse_report(pane, &bytes);
                    }
                }
            }
            // A click on the selection with no drag clears it (R3, `MH:692-700`); a drag that
            // started cleared `Drag::Out` before it ran, so its release never gets here.
            Drag::Out(d) => {
                if selection_ui::release_clears_selection(selection_ui::PressKind::MaybeDrag, false) {
                    if let Some(p) = self.panes.get_mut(&d.pane) {
                        p.clear_selection();
                    }
                }
                // The press did not remove a selection (it was on one), so the click selects the command too.
                if d.command {
                    self.click_after_release(d.pane, false);
                }
            }
            // A scrollbar drag ends with the press; the view stays where the thumb left it.
            Drag::Scrollbar { .. } => {}
            _ => {}
        }
        self.redraw();
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        self.pane_move_hover(x, y);
        self.update_tab_hover();
        self.update_side_handle_hover();
        self.toolbelt_move(x, y);
        self.composer_move(x, y);
        self.clip_move(x, y);
        self.inline_move(x, y);
        self.update_hover();
        self.update_block_hover();
        // An overlay scrollbar widens under the pointer.
        let over_bar = self.scrollbar_at(x, y).map(|(id, _, _)| id);
        if over_bar != self.scroll_hover {
            self.scroll_hover = over_bar;
            self.redraw();
        }
        // Hover timestamps show while the pointer is at a pane's right edge.
        let near = self.near_stamp_edge();
        if near != self.stamp_near {
            self.stamp_near = near;
            self.redraw();
        }
        // A title bar drag whose release was lost (capture taken away) ends here.
        if matches!(self.drag, Drag::Title { .. }) && self.title_drag_lost(pane_move_ui::DragLost::Moved { left_down: { let (swapped, l, r) = crate::winkeys::mouse_buttons(); pane_move_ui::primary_down(swapped, l, r) } }) {
            return;
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
            Drag::Title { pane, from } => {
                let (pane, from) = (*pane, *from);
                self.title_drag_move(pane, from, x, y);
            }
            Drag::Scrollbar { pane, grab } => {
                let (pane, grab) = (*pane, *grab);
                self.scroll_drag_to(pane, grab, y);
            }
            Drag::Select { pane, .. } => {
                let pane = *pane;
                self.sel_scroll_update(pane, x, y);
                self.select_drag_to(pane, x, y);
            }
            Drag::Out(d) => {
                let d = *d;
                self.drag_out_move(d, x, y);
            }
            Drag::Divider { path, axis, parent } => {
                let ratio = match axis {
                    Axis::Row => (x - parent.x) / parent.w,
                    Axis::Column => (y - parent.y) / parent.h,
                };
                let path = path.clone();
                // A hidden resident window has no tabs and no drag (Task 7).
                let Some(tab) = self.tabs.get_mut(self.active) else { return };
                tab.layout.set_ratio(&path, ratio);
                self.relayout();
            }
            Drag::App { pane, button, .. } => {
                let (pane, button) = (*pane, *button);
                // A drag counts as reported where the program asked for any clicks (iTerm2's
                // test for drag reports), even when only motion with a button is sent.
                let reported = self.report_gate(pane, mouse::Kind::Drag);
                if button == Button::Left {
                    self.frustration(pane, |d| d.mouse_dragged(reported));
                } else {
                    self.frustration(pane, |d| d.other_mouse());
                }
                self.send_motion(pane, Some(button), x, y);
            }
            Drag::None => {
                if let Some((id, _)) = self.pane_at(x, y) {
                    // The pointer reached a terminal pane (not chrome, not an overlay) after
                    // entering the window: the one moment focus follows mouse decides.
                    if self.ffm_entered && !self.tandem_ui_at(x, y) {
                        self.ffm_enter();
                    }
                    self.ffm_pane_focus(id, (x, y));
                    if self.modes(id).tracking == Tracking::Any && self.report_gate(id, mouse::Kind::Hover) {
                        self.send_motion(id, None, x, y);
                    }
                }
            }
        }
    }

    fn send_motion(&mut self, pane: PaneId, button: Option<Button>, x: f32, y: f32) {
        let Some(rect) = self.pane_rect(pane) else { return };
        let (col, row) = self.report_cell(pane, rect, x, y);
        let modes = self.modes(pane);
        let kind = if button.is_some() { mouse::Kind::Drag } else { mouse::Kind::Hover };
        if !self.report_gate(pane, kind) {
            return;
        }
        let ev = mouse::Event { button: button.unwrap_or(Button::None), action: mouse::Action::Motion, mods: self.mods, col, row };
        if let Some(bytes) = mouse::encode(modes, ev) {
            self.write_mouse_report(pane, &bytes);
        }
    }

    fn wheel(&mut self, delta: MouseScrollDelta) {
        // A line of the wheel is a cell of the pane under the pointer.
        let (cw, ch) = self.pointer_pane_cell();
        let scale = self.scale() as f64;
        let cell_pt = (cw as f64 / scale, ch as f64 / scale);
        let ev = match delta {
            MouseScrollDelta::LineDelta(x, y) => crate::wheel::event_from(x, y, None, WHEEL_LINES as f64, cell_pt),
            MouseScrollDelta::PixelDelta(p) => crate::wheel::event_from(0.0, 0.0, Some((p.x / scale, p.y / scale)), WHEEL_LINES as f64, cell_pt),
        };
        let (dx, dy) = (ev.dx as f32, ev.dy as f32);
        if self.tab_bar_wheel(delta) || self.composer_wheel(dy) || self.toolbelt_wheel(dy) || self.inline_wheel(dy) || self.clip_wheel(dy) || self.porthole_wheel(dx, dy) {
            return;
        }
        let (x, y) = self.cursor;
        // A wheel over a Tandem overlay is not the program's: no report and no arrows.
        if self.tandem_ui_at(x, y) {
            return;
        }
        // Over a scrollbar the wheel scrolls the pane, whatever the program asked for.
        if self.scroll_wheel(dy, ev.precise) {
            return;
        }
        let Some((id, rect)) = self.pane_at(x, y) else { return };
        // Ctrl+wheel is the pinch gesture: the font size, over a pane only.
        if self.pinch(ev) {
            return;
        }
        let (col, row) = self.report_cell(id, rect, x, y);
        self.route_wheel(id, col, row, ev);
    }

    /// How underlines and strikethrough draw, for the live frame and a screenshot alike: the
    /// Advanced switches, the renderer's face metrics and the window's scale.
    pub(super) fn draw_prefs(&mut self) -> scene::DrawPrefs {
        let metrics = self.gpu.as_mut().map(|g| g.renderer.underline_metrics()).unwrap_or(scene::DrawPrefs::default().metrics);
        scene::DrawPrefs::from_globals(&self.globals, metrics, self.scale())
    }

    /// This window draws see-through now (as `build_frame` decides it): an alpha surface, with a pane
    /// whose profile has Transparency.
    pub(super) fn see_through_now(&self) -> bool {
        let alphas = self.tabs.get(self.active).map(|t| self.pane_alphas(t)).unwrap_or_default();
        self.see_through_with(&alphas)
    }

    /// `see_through_now` for the active tab's pane alphas already read (`build_frame` and the
    /// fade gate share it, so they cannot drift apart).
    fn see_through_with(&self, alphas: &[(PaneId, f32)]) -> bool {
        self.win_alpha_surface == crate::transparency::SurfaceKind::Alpha && !self.transparency_unavailable && crate::transparency::needs_alpha(&alphas.iter().map(|(_, a)| *a).collect::<Vec<_>>())
    }

    fn build_frame(&mut self) -> Frame {
        let mut attention: Option<Instant> = None;
        // Every pane's face is measured before the frame (`pane_cell` only reads).
        self.ensure_faces();
        let cell = self.cell();
        let draw = self.draw_prefs();
        let palette_bg = self.panes.values().next().map_or(crate::palette::Palette::default().bg, |p| p.screen.palette.bg);
        let mut frame = Frame { clear: rgba(palette_bg, 1.0), gap_underlines: !draw.solid_underlines, ..Default::default() };
        // Only a window made for alpha draws see-through; its panes' alphas come from their profiles.
        let alphas = self.tabs.get(self.active).map(|t| self.pane_alphas(t)).unwrap_or_default();
        let see_through = self.see_through_with(&alphas);
        self.frame_alpha = see_through;
        self.warn_transparency_unavailable(&alphas);
        if see_through {
            frame.clear = [0.0; 4];
            // A hotkey window fading on the alpha surface: the renderer scales the whole target.
            frame.fade = self.hotkey.as_ref().filter(|h| h.fading()).map_or(1.0, |h| h.fade);
        }
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
        // The bars' clocks follow what programs reported, before the tab bar's looks read them.
        self.update_progress_anims(Instant::now());
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
        // A tmux window smaller than its tab leaves the rest to the filler (`PTYTab.m:1189-1208`).
        if let Some(tab) = self.tabs.get(self.active) {
            let strips = self.tmux_filler(tab);
            if strips.iter().any(Option::is_some) {
                let bg = self.panes.get(&tab.focused).map_or(palette_bg, |p| p.screen.palette.bg);
                let black = self.full_screen() && self.globals.bool("UseBlackFillerColorForTmuxInFullScreen");
                let color = scene::filler_color(bg, scene::filler_dark(bg), black);
                let alpha = if see_through { alphas.iter().find(|(a, _)| *a == tab.focused).map_or(1.0, |(_, a)| *a) } else { 1.0 };
                let color = if see_through { crate::transparency::premultiplied([color.r, color.g, color.b], alpha) } else { scene::rgba(color, 1.0) };
                for rect in strips.into_iter().flatten() {
                    frame.quads.push(scene::Quad { rect, color });
                }
            }
        }
        let mut block_buttons = Vec::new();
        let mut fps_rect: Option<Rect> = None;
        let mut picker_hits = Vec::new();
        let mut cmd_button_hits = Vec::new();
        let mut offscreen_hits: Vec<(PaneId, Rect, i32)> = Vec::new();
        let mut band_jobs: Vec<(PaneId, Rect, (f32, f32))> = Vec::new();
        let mut indicator_hits = Vec::new();
        let mut inline_hits = Vec::new();
        self.prepare_portholes();
        self.prepare_cursors();
        self.prepare_badges();
        self.prepare_offscreen();
        self.prepare_inline_chats();
        let mut porthole_hits = Vec::new();
        let mut overlay_drawn: Vec<(mouse::Overlay, Rect)> = Vec::new();
        let mut held_updates: Vec<(PaneId, Option<crate::flicker::Held>, Option<Instant>)> = Vec::new();
        // The focused browser pane's find bar and its mode label, when drawn above the page.
        let mut browser_find_drawn: Option<(Rect, Rect, Option<scene::OldFind>)> = None;
        let bview = self.broadcast_view();
        let stripes = self.globals.bool("Use background pattern indicator").then(|| self.stripe_tile());
        if let Some(tab) = self.tabs.get(self.active) {
            for (id, rect) in self.visible(tab) {
                // A browser pane's view covers its rectangle: only its background is Tandem's.
                if self.is_browser_pane(id) {
                    if let Some(p) = self.panes.get(&id) {
                        frame.quads.push(scene::Quad { rect, color: scene::rgba(p.screen.palette.bg, 1.0) });
                    }
                    if let Some(bar) = self.browser_strip(tab, id) {
                        self.draw_browser_bar(&mut frame, id, bar, rect, self.cell());
                        picker_hits.extend(self.draw_browser_picker(&mut frame, id, bar, self.cell()));
                    }
                    // An announcement bar covers the top of the page (the web view gives way below it).
                    overlay_drawn.extend(self.draw_info_bar(&mut frame, id, rect, self.pane_cell(id)).map(|r| (mouse::Overlay::InfoBar, r)));
                    // The find bar sits in its own row above the page, below the announcement bar: the
                    // one rectangle `browser_split` also gives the web view (13x3 B1).
                    if let (Some(row), Some(f)) = (self.browser_split(id, rect).0, self.finds.get(&id)) {
                        let swap = self.globals.bool("SwapFindNextPrevious");
                        let count = self.browser_views.get(&id).map_or_else(String::new, |b| crate::browser_find::label(b.find.total(), b.find.current(), swap));
                        let selection = self.panes.get(&id).map_or(crate::palette::Palette::default().selection, |p| p.screen.palette.selection);
                        let bar = scene::FindBar { query: &f.query, count: &count, mode: self.find_mode().label(), error: false, notice: None, progress: None, selection };
                        scene::find_strip(&mut frame, row);
                        let first_quad = frame.quads.len();
                        // The one find bar: `UseOldStyleDropDownViews` picks its layout on a browser pane too.
                        let (mode_rect, old) = if self.dropdown_views.old_style() {
                            let o = scene::find_bar_old(&mut frame, &bar, row, self.pane_cell(id), self.scale());
                            (o.mode, Some(o))
                        } else {
                            (scene::find_bar(&mut frame, &bar, row, self.pane_cell(id)), None)
                        };
                        if id == tab.focused {
                            browser_find_drawn = frame.quads.get(first_quad).map(|q| (q.rect, mode_rect, old));
                        }
                    }
                    continue;
                }
                // Everything a pane's iteration draws lays out in its own cell and shapes in its own
                // face; the window's chrome (after the loop) keeps the default cell and face.
                let cell = self.pane_cell(id);
                let face = self.pane_faces.face(id).unwrap_or(crate::render::DEFAULT_FACE);
                let draw = self.draw_prefs_for(face);
                let receives = self.broadcast.receives(&bview, id);
                let sends = self.broadcast.sends(&bview, id);
                if let Some(b) = backgrounds.iter().find(|b| b.pane == id) {
                    scene::background(&mut frame, b.rect, &b.pieces, &b.image, b.bg, b.blend);
                }
                // The selected command's rows (None in a view and in the alternate screen).
                let command_region = self.panes.get_mut(&id).and_then(|p| p.selected_region());
                self.draw_badge(&mut frame, id, rect);
                // The badge sizes itself in pixels against the default font and keeps the default
                // face; the rows from here on are laid out in the pane's cell and take its face.
                let rows_from = frame.rows.len();
                if let Some(p) = self.panes.get(&id) {
                    let shortcuts = self.shortcuts.as_ref().filter(|s| s.pane == id);
                    let labeled: Vec<crate::find::Match> = shortcuts.map(|s| s.items.iter().map(|(_, m, _)| *m).collect()).unwrap_or_default();
                    let labels: Vec<(crate::find::Pos, String)> = shortcuts.map(|s| s.items.iter().map(|(l, m, _)| (m.start, l.clone())).collect()).unwrap_or_default();
                    // Find highlights show only while the find bar is open.
                    let matches = if labeled.is_empty() { self.finds.get(&id).filter(|f| f.open).map(|f| f.results.matches.as_slice()).unwrap_or_default() } else { labeled.as_slice() };
                    let hover = self.hover.filter(|(h, _)| *h == id).map(|(_, m)| m);
                    let notes: Vec<(crate::find::Match, Option<String>)> = self.note_cells(id).into_iter().map(|(m, _, t)| (m, t)).collect();
                    let bg_alpha = if see_through { alphas.iter().find(|(a, _)| *a == id).map_or(1.0, |(_, a)| *a) } else { 1.0 };
                    let opaque_colors = see_through && self.store.get(&p.profile).is_some_and(|pr| pr.bool("Only The Default BG Color Uses Transparency"));
                    let dim = self.pane_dim(tab, id, &bview);
                    let selections = p.screen.extra_subs();
                    let hl = scene::Highlights { matches, color: p.screen.palette.match_bg, hover, link: p.screen.palette.link, labels: &labels, notes: &notes, dim, key_cursor: self.focused && id == tab.focused && p.at_password_prompt(), bg_alpha, opaque_colors, text_blink_on: self.blink.visible || !self.focused, selections: &selections, command_region, shade: command_region.map(|_| crate::cmdsel::shade_color(p.screen.palette.bg, self.globals.number("AlphaForDeselectedCommandShade").map_or(0.25, |a| (a as f32).clamp(0.0, 1.0)))), outline: crate::cmdsel::outline_colors(p.screen.palette.selection, p.screen.palette.bg) };
                    // The margins, one layer per pixel and the only margin painter: the pane's
                    // color, or an alternate screen's edge colors (`ExtendBackgroundColorIntoMargins`).
                    {
                        let inner = self.text_rect(id, rect);
                        let (cols, lines) = (p.screen.term().columns(), p.screen.term().screen_lines());
                        let grid = Rect { x: inner.x, y: inner.y, w: cols as f32 * cell.0, h: lines as f32 * cell.1 };
                        let bg = p.screen.palette.bg;
                        let has_image = backgrounds.iter().any(|b| b.pane == id);
                        let (alt, extend) = (p.screen.alt_screen(), self.globals.bool("ExtendBackgroundColorIntoMargins"));
                        let edges = if alt && extend { p.screen.edge_backgrounds() } else { Vec::new() };
                        let fill = margins::fill(&edges, bg, extend, alt);
                        // The strip and the stamp column are not margin: the painter takes the
                        // pane without them, and the column gets the pane color itself.
                        let outer = self.text_outer(id, rect);
                        if see_through {
                            scene::margin_fill_alpha(&mut frame, outer, grid, cell.1, &fill, bg, bg_alpha, opaque_colors, has_image);
                        } else {
                            scene::margin_fill(&mut frame, outer, grid, cell.1, &fill, has_image.then_some(bg));
                        }
                        if let Some(column) = self.stamp_column(id, rect).filter(|_| !has_image) {
                            let color = if see_through { crate::transparency::premultiplied([bg.r, bg.g, bg.b], bg_alpha) } else { scene::rgba(bg, 1.0) };
                            if see_through || color != frame.clear {
                                frame.quads.push(scene::Quad { rect: column, color });
                            }
                        }
                    }
                    // An Auto Composer hides the terminal cursor and the rows it covers.
                    let cover = self.auto_composer_cover(id, rect);
                    let marks = crate::flicker::Marks::of(&frame);
                    let (overlay_from, underline_from, strike_from, glyph_from) = (frame.overlay.len(), frame.underlines.len(), frame.strikes.len(), frame.glyphs.len());
                    // Reduce Flicker: while the program has the cursor hidden, the pane's last
                    // live picture stands in for the one it is drawing.
                    let reduce = self.store.get(&p.profile).is_some_and(|pr| pr.bool("Reduce Flicker"));
                    let hold = crate::flicker::holding(reduce, p.screen.cursor_hidden_at, Instant::now());
                    let alt = p.screen.alt_screen();
                    match p.held.as_ref().filter(|h| hold && h.usable(rect, alt, face, cell, p.screen.cursor_hidden_at)) {
                        Some(h) => {
                            h.replay(&mut frame);
                            if let Some(c) = cover {
                                composer_ui::skip_covered(&mut frame.overlay, overlay_from, &mut frame.underlines, underline_from, &mut frame.strikes, strike_from, &mut frame.glyphs, glyph_from, c);
                            }
                            held_updates.push((id, None, p.screen.cursor_hidden_at.map(|t| t + crate::flicker::HOLD)));
                        }
                        None => {
                            scene::pane_with(&mut frame, &p.screen, self.text_rect(id, rect), cell, self.focused && id == tab.focused, &self.cursor_look_for(id, self.focused && id == tab.focused, cover.is_none()), &draw, Some(&hl), face);
                            if let Some(c) = cover {
                                composer_ui::skip_covered(&mut frame.overlay, overlay_from, &mut frame.underlines, underline_from, &mut frame.strikes, strike_from, &mut frame.glyphs, glyph_from, c);
                            }
                            // Kept for the next hold; dropped when the switch is off.
                            let held = reduce.then(|| crate::flicker::Held::slice(&frame, &marks, Instant::now(), rect, alt, face, cell));
                            held_updates.push((id, held, None));
                        }
                    }
                    // The gray quad over the whole pane (margins included); Dim Only Text dimmed the cells itself.
                    if dim.amount > 0.0 && !dim.only_text {
                        frame.overlay.push(scene::Quad { rect, color: scene::rgba(crate::palette::rgb(128, 128, 128), dim.amount) });
                    }
                    self.draw_active_border(&mut frame, tab, id, rect);
                    // Broadcast stripes: over the cell backgrounds, under the text.
                    if let (Some((n, px)), true) = (&stripes, receives) {
                        let r = self.text_rect(id, rect);
                        frame.images.push(scene::ImageQuad { rect: r, uv: [0.0, 0.0, r.w / *n as f32, r.h / *n as f32], key: (0x5354_5249, *n as usize), pixels: px.clone(), size: (*n, *n), repeat: true });
                    }
                    // The offscreen command line is drawn last, below (`band_jobs`).
                    band_jobs.push((id, rect, cell));
                    porthole_hits.extend(self.draw_portholes(&mut frame, id, rect, cell));
                }
                self.draw_view_extras(&mut frame, id, rect, cell);
                self.draw_scrollbar(&mut frame, id, rect, cell);
                if let Some((row, col, selecting)) = self.copy_cursor_on_screen(id) {
                    // Drawn where the row's display order puts the cell.
                    let col = self.panes.get(&id).map_or(col, |p| p.screen.visual_col(row, col));
                    scene::copy_cursor(&mut frame, self.text_rect(id, rect), cell, row, col, selecting);
                }
                if self.shot.as_ref().is_some_and(|s| s.pane == id) {
                    scene::camera_sign(&mut frame, self.text_rect(id, rect), cell);
                }
                if self.panes.get(&id).is_some_and(|p| p.buffer.on) {
                    scene::buffering_sign(&mut frame, self.text_rect(id, rect), cell);
                }
                indicator_hits.extend(self.draw_coprocess_indicator(&mut frame, id, self.text_rect(id, rect), cell, (sends, receives)));
                picker_hits.extend(self.draw_session_picker(&mut frame, id, self.text_rect(id, rect), cell));
                cmd_button_hits.extend(self.draw_command_buttons(&mut frame, id, self.text_rect(id, rect), cell));
                overlay_drawn.extend(self.draw_info_bar(&mut frame, id, rect, cell).map(|r| (mouse::Overlay::InfoBar, r)));
                if let Some(t) = self.draw_attention(&mut frame, id, self.text_rect(id, rect), cell) {
                    attention = Some(attention.map_or(t, |a: Instant| a.min(t)));
                }
                let show = self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).is_none_or(|p| p.bool("Show Mark Indicators"));
                if let Some(p) = self.panes.get(&id) {
                    // Block bars take a row's gutter over mark bars.
                    let block_rows = self.block_view_rows(id);
                    let theme = self.store.get(&p.profile).is_some_and(|pr| pr.bool("Use Theme Mark Colors"));
                    let g = p.screen.term().grid();
                    let off = g.display_offset() as i32;
                    let folded = |row: usize| g[alacritty_terminal::index::Line(row as i32 - off)].fold() != 0;
                    let (cols, inner) = (g.columns(), self.text_rect(id, rect));
                    // The selected command's rows on screen: a line-style mark inside it is not drawn (13m hand-over).
                    let top = g.history_size().saturating_sub(g.display_offset());
                    let view_region = command_region.filter(|(_, last)| *last >= top).map(|(first, last)| (first.saturating_sub(top), last - top));
                    let pad = self.margins().0;
                    let mut bars: Vec<(usize, crate::marks::MarkColor)> = Vec::new();
                    for (row, m) in p.screen.visible_marks() {
                        if block_rows.contains(&row) || !(show || m.name.is_some()) {
                            continue;
                        }
                        let color = crate::marks::color(&m, self.globals.bool("ShowYellowMarkForJobStoppedBySignal"));
                        // A line-style mark draws a line, and no bar unless its row is folded.
                        if m.line_style {
                            if !scene::line_mark_hidden(row, view_region) {
                                let line = scene::line_mark_color(scene::mark_color(color, &p.screen.palette, theme), p.screen.palette.bg);
                                scene::line_mark(&mut frame, inner, row, cell, line, !m.command.is_empty(), cols);
                            }
                            if !folded(row) {
                                continue;
                            }
                        }
                        bars.push((row, color));
                    }
                    scene::mark_bars(&mut frame, &bars, inner, cell.1, pad, &p.screen.palette, theme);
                    if self.globals.bool("DebugShowPromptMarkRangesInLegacyRenderer") {
                        let spans = p.screen.prompt_range_spans();
                        scene::prompt_range_outlines(&mut frame, &scene::prompt_span_rects(&spans, inner, cell, cols, g.screen_lines()));
                    }
                    block_buttons.extend(self.draw_blocks(&mut frame, id, rect, cell));
                    self.draw_inline_buttons(&mut frame, id, rect, cell);
                }
                // The composer over the pane, then the completion list last, so everything under
                // them is cut out.
                overlay_drawn.extend(self.draw_composer(&mut frame, id, rect, cell));
                overlay_drawn.extend(self.draw_completion(&mut frame, id, rect, cell).map(|r| (mouse::Overlay::Completion, r)));
                if id == tab.focused {
                    if let Some(text) = self.fps_overlay() {
                        fps_rect = scene::fps_meter(&mut frame, &text, self.text_rect(id, rect), cell);
                    }
                }
                self.draw_inline_progress(&mut frame, id, rect);
                for row in frame.rows[rows_from..].iter_mut().filter(|r| r.face == crate::render::DEFAULT_FACE) {
                    row.face = face;
                }
            }
            self.draw_clippings(&mut frame, cell);
            inline_hits = self.draw_inline_chats(&mut frame, cell);
            if tab.zoomed.is_none() {
                scene::dividers(&mut frame, &tab.layout.dividers(area, self.gap()), self.divider_color());
            }
            self.find_bar_rect = browser_find_drawn.map(|d| d.0);
            self.find_mode_rect = browser_find_drawn.map(|d| d.1);
            self.find_old_buttons = browser_find_drawn.and_then(|d| d.2);
            // A pane whose status bar shows its find (a Search or Filter Tool) has no find bar; a
            // browser pane's was drawn above its page.
            let bar_pane = self.visible(tab).into_iter().find(|(id, _)| *id == tab.focused).filter(|_| self.bar_find_host(tab.focused).is_none() && !self.is_browser_pane(tab.focused));
            if let (Some((_, rect)), Some(f)) = (bar_pane, self.finds.get(&tab.focused).filter(|f| f.open)) {
                let count = if f.filter { "Filter".to_string() } else { f.results.label(self.globals.bool("SwapFindNextPrevious")) };
                let mode = self.find_mode();
                let restricted = self.panes.get_mut(&tab.focused).is_some_and(|p| find_ui::find_restricted(p.search_region().is_some(), f.filter, p.live.is_some()));
                let palette_selection = self.panes.get(&tab.focused).map_or(crate::palette::Palette::default().selection, |p| p.screen.palette.selection);
                let bar = scene::FindBar { query: &f.query, count: &count, mode: mode.label(), error: f.error.is_some(), notice: restricted.then_some(crate::cmdsel::NOTICE), progress: f.progress, selection: palette_selection };
                let first_quad = frame.quads.len();
                let (mode_rect, old) = if self.dropdown_views.old_style() {
                    let o = scene::find_bar_old(&mut frame, &bar, rect, cell, self.scale());
                    (o.mode, Some(o))
                } else {
                    (scene::find_bar(&mut frame, &bar, rect, cell), None)
                };
                self.find_bar_rect = frame.quads.get(first_quad).map(|q| q.rect);
                self.find_mode_rect = Some(mode_rect);
                self.find_old_buttons = old;
            }
        }
        for (id, held, until) in held_updates {
            if let Some(p) = self.panes.get_mut(&id) {
                match until {
                    Some(t) => p.held_until = Some(t),
                    None => (p.held, p.held_until) = (held, None),
                }
            }
        }
        if let Some(r) = self.tab_bar_rect().filter(|_| flash_over) {
            scene::lay_over(&mut frame, bar_layer, r, self.tab_flash.alpha(Instant::now()), palette_bg, cell.1);
        }
        self.draw_toolbelt(&mut frame, cell);
        self.draw_title_bars(&mut frame, cell);
        self.draw_workgroup_toolbars(&mut frame, cell);
        self.draw_diff_overlays(&mut frame, cell);
        self.draw_review_overlays(&mut frame, cell);
        self.draw_status_bars(&mut frame, cell);
        // After the bars, so the overlay cuts their text out (P22).
        overlay_drawn.extend(self.draw_pane_move(&mut frame, cell).into_iter().map(|r| (mouse::Overlay::PaneMove, r)));
        let (ww, wh) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
        let mut tip_rect: Option<Rect> = None;
        let line = self.division_height();
        if line > 0.0 {
            scene::division_line(&mut frame, ww, line, crate::chrome::division_color(self.resolved_style(), self.focused));
        }
        self.draw_find_cursor(&mut frame, Rect { x: 0.0, y: 0.0, w: ww, h: wh }, cell);
        if let Some((text, _)) = self.toast.as_ref().filter(|(_, until)| Instant::now() < *until) {
            overlay_drawn.push((mouse::Overlay::Toast, scene::toast(&mut frame, text, Rect { x: 0.0, y: 0.0, w: ww, h: wh }, cell)));
        }
        // The first-selection notice of a clicked command (13s Task 9), over the panes and the toast.
        overlay_drawn.extend(self.draw_command_info(&mut frame, Rect { x: 0.0, y: 0.0, w: ww, h: wh }, cell).map(|r| (mouse::Overlay::CommandInfo, r)));
        // The tab tooltip over everything but the banner, under the pointer (a pointer's
        // height below it).
        if let Some(text) = self.tab_tip_shown().and_then(|i| self.tab_tip_text(i)) {
            let (x, y) = self.cursor;
            tip_rect = Some(scene::tab_tooltip(&mut frame, &text, (x, y + TIP_POINTER_PT * self.scale()), Rect { x: 0.0, y: 0.0, w: ww, h: wh }, cell));
        }
        // A pinned hotkey window's pin shows its hover text (the tab tip wins while it shows).
        if tip_rect.is_none() {
            let (x, y) = self.cursor;
            if let Some(text) = scene::indicator_tip(&indicator_hits, x, y) {
                tip_rect = Some(scene::tab_tooltip(&mut frame, text, (x, y + TIP_POINTER_PT * self.scale()), Rect { x: 0.0, y: 0.0, w: ww, h: wh }, cell));
            }
        }
        if see_through {
            let (ww, wh) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
            let chrome = rgba(palette_bg, 1.0);
            let mut quads = crate::transparency::backdrop(Rect { x: 0.0, y: 0.0, w: ww, h: wh }, area, chrome, &[]);
            quads.append(&mut frame.quads);
            frame.quads = quads;
        }
        self.attention_due = attention;
        self.block_buttons = block_buttons;
        self.porthole_hits = porthole_hits;
        self.picker_hits = picker_hits;
        self.cmd_button_hits = cmd_button_hits;
        self.inline_hits = inline_hits;
        self.chat_indicator_hits = indicator_hits;
        let error = self.banners.text().or_else(|| self.panes.values().find_map(|p| p.error.clone()));
        if let Some(text) = error {
            let (w, h) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
            overlay_drawn.push((mouse::Overlay::Banner, scene::banner(&mut frame, &text, Rect { x: 0.0, y: 0.0, w, h }, cell)));
        }
        // The offscreen command lines, last: over the pane's rows and strokes (they are cut out)
        // but around every Tandem surface drawn over a pane's top, which the one overlay list
        // names (`ui_rects`): info bars, the composer and completion, portholes, pills,
        // pickers, indicators, the find bar, the toast and the banner (Task 6 review 2).
        let mut holes: Vec<Rect> = overlay_drawn.iter().map(|(_, r)| *r).collect();
        holes.extend(self.porthole_hits.iter().map(|h| h.3));
        holes.extend(self.block_buttons.iter().map(|b| b.0));
        holes.extend(self.picker_hits.iter().map(|h| h.1));
        holes.extend(self.cmd_button_hits.iter().map(|h| h.1));
        holes.extend(self.chat_indicator_hits.iter().map(|h| h.1));
        holes.extend(self.find_bar_rect.into_iter().chain(self.find_mode_rect));
        // The tab tooltip, the fps meter, the clippings panels and preview and the inline chat
        // panels are drawn over a pane too (Task 6 review, r1).
        holes.extend(tip_rect.into_iter().chain(fps_rect));
        holes.extend(self.clip_holes());
        holes.extend(self.inline_hits.iter().filter_map(|h| if let inline_chat_ui::PanelHit::Body(_, r) = h { Some(*r) } else { None }));
        for (id, rect, cell) in band_jobs {
            offscreen_hits.extend(self.draw_offscreen(&mut frame, id, rect, cell, &holes).into_iter().map(|(r, line)| (id, r, line)));
        }
        self.offscreen_hits = offscreen_hits;
        self.overlay_drawn = overlay_drawn;
        frame
    }

    fn render(&mut self) {
        // Before the timed phases (13w S12): each browser view is placed for this frame, so the
        // COM calls a changed placement makes are not counted as drawing.
        self.place_browser_views();
        // Nothing is measured while LogDrawingPerformance is off.
        let timing = self.globals.bool("LogDrawingPerformance");
        let started = timing.then(Instant::now);
        self.render_frame(timing);
        if let Some(started) = started {
            self.draw_timers.stats[0].add(started.elapsed().as_secs_f64(), 1);
            if self.draw_timers.due(Instant::now(), crate::perf::DRAW_EVERY) {
                // iTerm2's console (`PTM:176-192`) and the debug log: the numeric table only.
                let table = crate::perf::log_table("render", &mut self.draw_timers.stats);
                crate::dlog::debug_output(table.as_str());
                crate::dlog!("{}", table);
            }
        }
    }

    fn render_frame(&mut self, timing: bool) {
        // Focus moves between windows and panes, as the debug log reports them.
        if crate::dlog::is_on() {
            let focus = self.focused_id();
            if focus != self.focus_logged {
                self.focus_logged = focus;
                if let Some(pane) = focus {
                    crate::dlog!("pane focus changed to {}", pane);
                }
            }
        }
        // Recorded first: a frame that finds no surface still ends the wait for one.
        let now = Instant::now();
        let before = self.pacer.last();
        self.pacer.drew(now);
        self.note_frame(before, now);
        // A status bar field whose bar went away gives the keyboard back before drawing.
        self.check_composer_fields();
        self.sync_menu_hidden();
        // The next edge drag snaps to what this frame shows.
        self.publish_snap();
        let scene_start = timing.then(Instant::now);
        let frame = self.build_frame();
        if let Some(t) = scene_start {
            self.draw_timers.stats[1].add(t.elapsed().as_secs_f64(), 1);
        }
        // The caption's hit tests answer from what this frame drew.
        self.publish_caption();
        let (title, menu_text) = self.caption_text();
        if self.window_menu_text != menu_text {
            self.window_menu_text = menu_text;
            self.refresh_window_list();
        }
        // iTerm2 shows the Leader as sparkles on the cursor; Tandem says so in the title.
        let title = if self.leader_on { format!("[Leader] {title}") } else { title };
        // A mode change reconfigures the surface once; this frame draws with it.
        let transparent = self.frame_alpha;
        self.sync_surface_alpha(transparent);
        // Tab switches, focus, restore from minimized: the backdrop follows what this frame shows.
        self.sync_blur();
        // Window numbers show from 1 in the saved logs (P20).
        let window_n = self.window_number() + 1;
        let Some(gpu) = self.gpu.as_mut() else { return };
        gpu.window.set_title(&title);
        let wait_start = Instant::now();
        let acquired = gpu.surface.get_current_texture();
        // The renderer's timings for Copy Performance Stats: counters and milliseconds only.
        let dropped = matches!(acquired, wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded);
        let start_gap = before.map(|b| now.saturating_duration_since(b).as_secs_f64() * 1000.0);
        let wait = wait_start.elapsed().as_secs_f64() * 1000.0;
        self.perf_frames.entry(window_n).or_default().frame(start_gap, Some(wait), dropped, &mut self.perf, window_n);
        let texture = match acquired {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                gpu.surface.configure(gpu.render_device(), &gpu.config);
                gpu.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                // A fade running now stops, and fades stay off for the run.
                if self.hotkey.as_ref().is_some_and(|h| h.fading()) {
                    self.fade_failed = true;
                }
                match gpu.render_instance().create_surface(gpu.window.clone()) {
                    Ok(s) => gpu.surface = s,
                    Err(e) => self.banners.set("gpu", format!("Lost the window surface: {e}")),
                }
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.surface.configure(gpu.render_device(), &gpu.config);
                    gpu.window.request_redraw();
                }
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                if self.hotkey.as_ref().is_some_and(|h| h.fading()) {
                    self.fade_failed = true;
                }
                self.banners.set("gpu", "GPU validation error");
                return;
            }
        };
        let view = texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let (w, h) = (gpu.config.width, gpu.config.height);
        // The non-sRGB view a fade needs on an sRGB target; no fade, no view.
        let raw = gpu.config.view_formats.first().filter(|_| frame.fade < 1.0).map(|f| texture.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(*f), ..Default::default() }));
        #[cfg(debug_assertions)]
        let capturing = gpu_capture::begin(gpu.render_device());
        let draw_start = timing.then(Instant::now);
        let drawn = gpu.renderer.draw_with(&frame, &view, raw.as_ref(), w, h);
        if let Some(t) = draw_start {
            self.draw_timers.stats[2].add(t.elapsed().as_secs_f64(), 1);
        }
        if let Err(e) = drawn {
            if self.hotkey.as_ref().is_some_and(|h| h.fading()) {
                self.fade_failed = true;
            }
            self.banners.set("gpu", e);
        }
        let present_start = timing.then(Instant::now);
        if let Some(gpu) = self.gpu.as_ref() {
            gpu.render_queue().present(texture);
            #[cfg(debug_assertions)]
            if capturing {
                gpu_capture::end(gpu.render_device());
            }
        }
        if let Some(t) = present_start {
            self.draw_timers.stats[3].add(t.elapsed().as_secs_f64(), 1);
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


/// What the launch pipe hands the event loop: a link as `OpenUrl`, any other request as `Launch`.
fn launch_events(proxy: Arc<Mutex<EventLoopProxy<UserEvent>>>) -> Box<dyn Fn(crate::launch::Request) + Send> {
    Box::new(move |request| {
        let _ = proxy.lock().map(|p| {
            p.send_event(match request {
                crate::launch::Request::Url { url } => UserEvent::OpenUrl(url),
                other => UserEvent::Launch(other),
            })
        });
    })
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        // An `-Embedding` start makes its window when the handoff arrives (`handoff_arrived`).
        if self.embedding.is_some() && self.start_handoff.is_none() {
            return;
        }
        // Disable App Nap: opt out of EcoQoS once at start (effective after a restart).
        #[cfg(windows)]
        if self.globals.bool("DisableAppNap") {
            if let Err(e) = power::opt_out_of_power_throttling() {
                eprintln!("power throttling opt-out unavailable: {e}");
            }
        }
        if self.snippets.broken {
            self.banners.set("snippets", format!("Could not read the snippets in {}. The file stays as it is until a snippet is added.", snippets_ui::snippets_path().display()));
        }
        // The Dynamic Profiles folder is read before the first window, so a startup arrangement
        // can name a dynamic profile.
        self.dynamic_profiles_start();
        let arrangement_policy = self.globals.bool("OpenArrangementAtStartup");
        let hotkey_only = !arrangement_policy && self.globals.bool("OpenNoWindowsAtStartup");
        let hotkey_profiles: Vec<String> = self.hotkey_prefs().into_iter().filter(|(_, p)| p.active()).map(|(g, _)| g).collect();
        let system_policy = !arrangement_policy && !hotkey_only;
        // `state_frozen` at launch: a second copy of Tandem, which leaves the state to the first.
        let mut saved = if (system_policy || hotkey_only) && !self.state_frozen && self.start_handoff.is_none() { self.saved_windows() } else { None };
        let regular_saved = saved.as_ref().is_some_and(|(w, _)| w.iter().any(|w| crate::arrangement::hotkey_guid(w).is_none()));
        let g = &self.globals;
        let policy = if arrangement_policy { crate::startup::DEFAULT_ARRANGEMENT } else if hotkey_only { crate::startup::HOTKEY_ONLY } else { crate::startup::SYSTEM };
        let startup_prefs = crate::startup::StartupPrefs { open_profiles_window: g.bool("OpenBookmark"), policy, always_open: g.bool("AlwaysOpenWindowAtStartup"), open_new_window: g.bool("OpenNewWindowAtStartup") };
        let early = crate::startup::plan_with(startup_prefs, false, !hotkey_profiles.is_empty(), system_policy && regular_saved);
        // Nothing to show at launch (Only Restore Hotkey Window, or policy 0 with a hotkey
        // window and nothing restored): the first window is a hotkey window, made hidden.
        let quiet = hotkey_only || (system_policy && !early.window && !early.restore);
        let startup_hotkey = if quiet && !self.state_frozen && self.start_handoff.is_none() { hotkey_profiles.first().cloned() } else { None };
        // R3: when the plan opens no window (and no hotkey window comes up), Tandem starts resident, as
        // iTerm2 runs with no window: the first window is made hidden and empty, the icon is the way in.
        // The icon-click key never decides a launch window (`AD:2463-2470`): only the plan does.
        let resident_start = self.start_handoff.is_none() && startup_hotkey.is_none() && matches!(policy, crate::startup::SYSTEM | crate::startup::HOTKEY_ONLY) && hotkey_profiles.is_empty() && !early.restore && !early.window;
        let mut attrs = Window::default_attributes().with_title("Tandem").with_inner_size(LogicalSize::new(1000.0, 650.0));
        if startup_hotkey.is_some() || resident_start {
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
        // Every terminal window allows the IME (winit leaves it off otherwise).
        window.set_ime_allowed(true);
        let prefer = power::chosen_preference(self.globals.bool("preferIntegratedGPU"));
        match init_gpu(window, event_loop, prefer) {
            Ok(gpu) => {
                let k = windows_ui::key(gpu.window.id());
                self.last_window = Some(gpu.window.id());
                self.gpu = Some(gpu);
                self.load_cursors(event_loop);
                self.windows.start(k);
                // A resident start has no window in the list until a tab makes it one.
                if resident_start {
                    self.resident = true;
                } else {
                    self.registry.open(k);
                }
                // Edge drags snap to the cell grid, and follow AllowLiveResize.
                self.install_sizing();
                // A forced Theme's title bar survives system setting changes.
                self.guard_theme();
                if let Some(h) = self.hwnd() {
                    crate::display_watch::watch(h);
                    #[cfg(windows)]
                    {
                        crate::firstmouse_win::watch(h);
                        self.taskbar_watch(h);
                        logout_ui::watch(h);
                    }
                }
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
            .then_some(())
            .filter(|_| self.start_handoff.is_none())
            .map(|_| {
                let store = self.arrangements();
                store.default_name().and_then(|n| store.get(n).cloned().map(|w| (Some(n.to_string()), w)))
            })
            .flatten()
            .filter(|(_, w)| !w.is_empty());
        let has_arrangement = default_arrangement.is_some();
        // The saved state's windows are the launch's own restoration (`launch` true); a default
        // arrangement is an arrangement like any other.
        let startup_arrangement = if startup_hotkey.is_some() { None } else { saved.map(|w| (None, w, true)).or(default_arrangement.map(|(n, w)| (n, w, false))) };
        // The launch's own restoration (saved windows, hotkey windows) holds browser loads back.
        self.restoring_windows = matches!(&startup_arrangement, Some((_, _, true))) || !hotkey_saved.is_empty();
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
                None if resident_start => {}
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
        if !arranged {
            // The launch window starts from its first profile's Initial Use Transparency (`PT:864`).
            let first = self.tabs.first().and_then(|t| t.layout.panes().first().copied()).and_then(|id| self.panes.get(&id)).and_then(|p| self.store.get(&p.profile));
            self.win_transparency = crate::transparency::WinTransparency::new(crate::transparency::initial_use(first, self.store.default_profile()));
        }
        self.install_menus();
        // A window the default arrangement filled keeps its frame, style, and locks.
        let steps = crate::winmodel::open_steps(arranged);
        if resident_start {
            // Hidden and empty: nothing to place or style until a window opens.
        } else if steps.place && startup_hotkey.is_none() {
            self.place_window();
        } else {
            self.claim_slot();
            self.anchor_from_profile(startup_hotkey.as_deref());
        }
        if steps.profile_window && startup_hotkey.is_none() && !resident_start {
            self.apply_window_profile(None);
        }
        // The Profiles window follows Open profiles window.
        let g = &self.globals;
        let policy = if g.bool("OpenArrangementAtStartup") { crate::startup::DEFAULT_ARRANGEMENT } else if g.bool("OpenNoWindowsAtStartup") { crate::startup::HOTKEY_ONLY } else { crate::startup::SYSTEM };
        let prefs = crate::startup::StartupPrefs { open_profiles_window: g.bool("OpenBookmark"), policy, always_open: g.bool("AlwaysOpenWindowAtStartup"), open_new_window: g.bool("OpenNewWindowAtStartup") };
        if crate::startup::plan_with(prefs, has_arrangement, !hotkey_profiles.is_empty(), has_saved).profiles_window {
            self.open_settings(event_loop, Mode::Profiles);
        }
        let style = startup_hotkey.as_deref().and_then(|g| self.store.get(g)).unwrap_or_else(|| self.store.default_profile()).int("Window Type");
        if steps.profile_style && style != crate::frames::NORMAL && !resident_start {
            self.apply_window_style(style, false);
        }
        // The first window is made on the primary instance; one whose profiles want alpha is recreated now.
        self.check_alpha_surfaces();
        self.alpha_recreate_tick(event_loop);
        // URLs that came with the start or before the window open now, after the first window.
        self.open_pending_urls();
        self.open_launch_queue(event_loop);
        self.open_handoff_queue(event_loop);
        self.register_open_with();
        // After the first window and the launch's own requests: the second launch's question.
        self.update_start();
        self.tips_launch();
        // Nothing queued: the restoration ended with the first window.
        if self.restoring_windows && self.pending_windows.is_empty() {
            self.restoring_windows = false;
            self.browser_restoration_done();
        }
    }

    /// Runs the cursor blink when the focused pane's profile asks for it.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // An `-Embedding` start waits for its handoff with no window. Work that reached it (a launch,
        // a link, files) makes it the main instance; with none it stops serving the pipe after
        // `IDLE_EXIT` and ends on the next pass, once what the pipe took has arrived (review I3).
        if self.gpu.is_none() {
            if let Some(t0) = self.embedding {
                use crate::default_term::Idle;
                let serving = self.url_listener.as_ref().is_some_and(|s| !s.stopped());
                match crate::default_term::embedding_next(t0, Instant::now(), self.handoffs_seen, self.embedding_waiting_work(), serving) {
                    Idle::Wait(until) => return event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(until)),
                    Idle::StopServing => {
                        self.embedding_stop_serving();
                        return event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
                    }
                    Idle::Exit => return event_loop.exit(),
                    Idle::BecomeMain => {
                        if !self.embedding_becomes_main(event_loop) {
                            return;
                        }
                    }
                }
            }
        }
        self.handoff_banner_tick();
        // Timers (blink, bells, activity, marks, animations) act for the window the person
        // uses; the window that drew last may be another one.
        if let Some(w) = self.last_window {
            self.enter(w);
        }
        self.sync_pane_activity();
        self.pane_focus_tick();
        self.session_hotkeys_flush();
        self.open_pending_mark_prompt(event_loop);
        self.flush_dropped();
        self.ssh_suggest_purge();
        // The queue is taken whole: a wake's index into it means nothing after this.
        self.pending_wake = None;
        for w in std::mem::take(&mut self.pending_windows) {
            self.open_pending(event_loop, w);
            self.tmux_apply_geometry();
        }
        // After the last saved window: the restored pages load.
        if self.restoring_windows {
            self.restoring_windows = false;
            self.browser_restoration_done();
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
        self.cockpit_follows_app();
        self.claude_tick(event_loop);
        let cockpit_due = self.cockpit_tick();
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
        let edge = active != self.app_active;
        if active && !self.app_active {
            // Tandem is in front: the waiting sessions clear and the bells reset (`PT:4999`).
            self.bell_badge.became_active();
            self.bell_badge.reset_bells();
            self.hotkey_reactivated();
            self.ffm_became_active();
            self.tips_became_active();
        } else if !active && self.app_active {
            self.ffm_became_inactive();
        }
        self.locale_focus(self.app_active, active);
        self.locale_tick();
        self.force_for_session();
        self.app_active = active;
        // The Win-hover underline follows the window's key state (`TVA:466-472`).
        if edge {
            self.update_hover();
        }
        // Again after 13j's edge (S9): a pane focused by aggressive focus follows mouse stamps in
        // this pass, and the inferrer's focus check runs inside it (P20).
        self.sync_pane_activity();
        let hotkey_due = self.tick_hotkeys();
        let find_due = self.run_due_searches();
        let spotlight = self.tick_find_cursor();
        let animation = self.next_animation_frame();
        let sync = self.flush_due_syncs();
        let toast = self.toast_due();
        let scrollbars = self.tick_scrollbars();
        let rotation = self.tick_rotations();
        let upload = self.feed_uploads();
        let paste_due = self.paste_tick().into_iter().chain(self.auto_command_tick()).min();
        let triggers = self.tick_triggers();
        self.tick_bell();
        let badge_due = self.tick_badge();
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
        self.alpha_recreate_tick(event_loop);
        self.open_composer_windows(event_loop);
        self.open_jobs_confirm(event_loop);
        self.open_recording_windows(event_loop);
        self.open_git_log(event_loop);
        let pwm_due = [self.tick_pwm(), self.basic_auth_tick(event_loop)].into_iter().flatten().min();
        #[cfg(windows)]
        self.tick_logout();
        self.open_pwm_windows(event_loop);
        self.open_autofill_windows(event_loop);
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
        self.ssh_unzip_tick();
        self.scripts_scan_tick();
        self.update_tick(event_loop);
        self.tips_tick(event_loop);
        let dynamic_due = self.dynamic_profiles_tick();
        self.access_tick();
        // What WebView2 views posted (before the wake times below are worked out).
        self.web_tick(event_loop);
        self.icon_tick();
        let icon_due = self.icon_due();
        let browser_due = self.browser_tick();
        // OK in "Importing Settings and Data": Tandem quits and starts again.
        if std::mem::take(&mut self.relaunch_due) {
            self.relaunch(event_loop);
        }
        self.prefs_tick(event_loop);
        self.catalog_tick();
        self.suppressed_tick();
        self.open_script_alerts(event_loop);
        self.open_scripts_alerts(event_loop);
        self.bookmark_editor_tick(event_loop);
        self.browser_runs_tick(event_loop);
        self.browser_next_tick(event_loop);
        self.open_locale_asks(event_loop);
        self.open_ai_alert(event_loop);
        self.open_ai_key_window(event_loop);
        self.open_ai_reveal(event_loop);
        self.ai_discovery_tick();
        self.ai_runs_tick();
        self.open_ai_engage_windows(event_loop);
        let ai_due = self.ai_discovery_due();
        let chat_due = self.chat_tick();
        // Orchestration's checks, mirror limits, delayed CRs and spawns (12f Task 10).
        self.orch_check_tick();
        self.orch_spawn_tick(event_loop);
        let orch_due = [self.orch_due(), self.orch_watch_tick()].into_iter().flatten().min();
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
        if self.display_due.is_some_and(|t| Instant::now() >= t) {
            self.display_due = None;
            self.refit_all_windows();
        }
        let desktop_due = self.desktop_tick();
        let ui_element = self.presence_tick();
        self.tray_tick(ui_element);
        if self.settings.as_mut().is_some_and(|s| s.take_space_warning()) {
            self.ask_scripts(scripts_alerts::ask(crate::desktops::SPACES_WARNING_HEADING, crate::desktops::SPACES_WARNING, &["OK"], Some(crate::desktops::SPACES_WARNING_ID), crate::warnings::Kind::Permanent, scripts_alerts::Then::Nothing));
        }
        let tip_due = self.tick_tab_tip();
        let flash_due = self.tick_tab_flash();
        let tmux_due = [self.tick_tmux(), self.tmux_pause_tick(event_loop), self.tmux_dashboard_due()].into_iter().flatten().min();
        let cursor_due = self.tick_cursor();
        let sel_scroll_due = self.sel_scroll_tick();
        let frame_due = self.frame_wake();
        let flicker_due = self.flicker_tick(Instant::now());
        let ended_due = self.tick_ended(event_loop);
        self.param_prompt_next(event_loop);
        let workgroup_due = self.tick_workgroups(event_loop);
        let restart_due = self.tick_restarts();
        self.tick_progress();
        let progress_due = self.progress_wake();
        self.log_tick();
        self.archive_tick();
        let sleep_due = self.recompute_sleep(Instant::now());
        let bindings_due = self.tick_bindings();
        self.bonjour_tick();
        let cmdinfo_due = self.cmdinfo_tick();
        let pathnav_due = self.pathnav_tick();
        let open_due = self.open_tick(event_loop);
        let jump_due = self.jump_list_tick();
        let command_due = self.command_click_tick();
        // Update and Tip of the Day deadlines wake no resident Tandem (the 13v1 merge).
        let (update_wake, tip_of_day_wake) = (update_app::idle_due(self.update_due, self.resident), update_app::idle_due(self.tip_due, self.resident));
        let wake = [workgroup_due, jump_due, bindings_due, cmdinfo_due, pathnav_due, command_due, open_due, ended_due, restart_due, self.save_due, self.prefs_save_due, find_due, spotlight, animation, sync, toast, scrollbars, self.attention_due, badge_due, rotation, upload, paste_due, triggers, activity, idle_codes, undo_due, hotkey_due, tmux_due, ssh_due, status_bars, notes_due, jobs_due, quickly_due, composer_due, replay_due, pwm_due, script_due, provision_due, console_due, import_due, self.catalog_due, self.suppressed_retitle_due, update_wake, tip_of_day_wake, self.access_due, dynamic_due, icon_due, browser_due, tip_due, flash_due, ai_due, chat_due, orch_due, chat_window_due, cockpit_due, desktop_due, self.display_due, sel_scroll_due, cursor_due, frame_due, flicker_due, progress_due, sleep_due].into_iter().flatten().min();
        match wake {
            Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Hotkey(ids) => self.hotkey_pressed(&ids, event_loop),
            UserEvent::Uia(r) => self.uia_request(r),
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
            UserEvent::Tray(right) => return self.tray_clicked(right, event_loop),
            UserEvent::JobsChanged => return self.jobs_changed(),
            UserEvent::DebugLogSaved(result, announce) => return self.debug_log_saved(result, announce),
            UserEvent::PageSaveFile { pane, id, done } => return self.page_save_file_done(pane, id, done),
            UserEvent::DisplayChanged => {
                let delay = crate::placement::display_delay(self.globals.number("UpdateScreenParamsDelay").unwrap_or(1.0));
                self.display_due = Some(Instant::now() + delay);
                return;
            }
            UserEvent::ThemeReset => return self.reapply_forced_theme(),
            UserEvent::Caption(hwnd, e) => return self.caption_event(hwnd, e, event_loop),
            UserEvent::OpenUrl(url) => return self.receive_url(url),
            UserEvent::Launch(request) => return self.receive_launch(request, event_loop),
            UserEvent::Handoff(h) => return self.handoff_arrived(h, event_loop),
            UserEvent::Logoff(done) => {
                #[cfg(windows)]
                self.logoff_now(&done);
                #[cfg(not(windows))]
                done.store(true, std::sync::atomic::Ordering::Release);
                return;
            }
            UserEvent::ShutdownQuery { done, end_now, hwnd } => {
                #[cfg(windows)]
                self.shutdown_decide(hwnd, &end_now);
                #[cfg(not(windows))]
                let _ = (hwnd, end_now);
                done.store(true, std::sync::atomic::Ordering::Release);
                return;
            }
            UserEvent::ShutdownAsk => return self.shutdown_query(event_loop),
            UserEvent::ShutdownAnswer(ok) => return self.shutdown_answered(ok, event_loop),
            UserEvent::Wake => {}
        }
        let mut changed = false;
        let mut outputs = Vec::new();
        let now = Instant::now();
        self.sync_output_policy();
        self.sync_host_policy();
        // The prompt state machine follows View > Auto Composer before it reads output (the
        // Task 3 contract: every pane, new ones included).
        let auto_composer = self.globals.bool("AutoComposer");
        let smart_logging = self.globals.bool("SmartLoggingWithAutoComposer");
        let always_line_marks = self.globals.bool("AlwaysUseLineStyleMarks");
        for (id, p) in self.panes.iter_mut() {
            p.own_mut().prompt_state.enabled = auto_composer;
            p.log_compact = (smart_logging, auto_composer);
            // New prompt marks are line-style with Auto Composer or AlwaysUseLineStyleMarks
            // (`S:18934`); marks already made keep their style.
            p.screen.line_style_marks = auto_composer || always_line_marks;
            let got = p.pump();
            if got {
                outputs.push(*id);
            }
            changed |= got;
        }
        changed |= self.ssh_pump();
        changed |= self.tmux_pump(event_loop);
        // Panes a tmux or ssh pump fed got output too (`feed_output`); they draw through the pacer.
        for (id, p) in self.panes.iter() {
            if p.got_output && !outputs.contains(id) {
                outputs.push(*id);
            }
        }
        self.stuck_tick();
        self.api_output(&outputs);
        self.api_session_events();
        self.record_replay_frames();
        // A pane whose shell exited stays and follows its end action, once its output drained.
        let (exited_now, _) = self.handle_ended(event_loop);
        changed |= !exited_now.is_empty();
        self.check_coprocesses();
        if changed {
            // An exit shows in the tab bar: a held pane's end draws through the pacer too.
            for id in exited_now {
                if !outputs.contains(&id) {
                    outputs.push(id);
                }
            }
            self.after_output(&outputs, now);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.prompt_event(id, &event) || self.param_prompt_event(id, &event) || self.mark_confirm_event(id, &event) || self.coprocess_prompt_event(id, &event) || self.tmux_prompt_event(id, &event) || self.tmux_dashboard_event(id, &event, event_loop) || self.ssh_panel_event(id, &event) || self.chat_window_event(id, &event, event_loop) || self.cockpit_event(event_loop, id, &event) || self.cc_onboarding_event(id, &event) || self.chat_prompt_event(id, &event) || self.title_prompt_event(id, &event) || self.group_prompt_event(id, &event) || self.arrange_prompt_event(id, &event) || self.global_find_event(id, &event) || self.paste_history_event(id, &event) || self.list_popup_event(id, &event) || self.advanced_paste_event(id, &event) || self.note_event(id, &event) || self.add_clip_event(id, &event) || self.review_window_event(id, &event) || self.locale_event(id, &event) || self.shot_event(id, &event) || self.status_setup_event(id, &event) || self.git_log_event(id, &event) || self.status_popover_event(event_loop, id, &event) || self.jobs_window_event(id, &event) || self.composer_window_event(id, &event) || self.open_quickly_event(event_loop, id, &event) || self.replay_window_event(event_loop, id, &event) || self.recording_warning_event(id, &event) || self.pwm_window_event(id, &event) || self.pwm_confirm_event(id, &event) || self.api_alert_event(event_loop, id, &event) || self.script_alert_event(id, &event) || self.scripts_alert_event(id, &event) || self.bookmark_editor_event(id, &event) || self.update_window_event(id, &event) || self.tip_window_event(id, &event) || self.ai_alert_event(id, &event) || self.ai_key_event(id, &event) || self.autofill_card_event(id, &event) || self.basic_auth_window_event(id, &event) || self.ai_engage_event(id, &event) || self.script_picker_event(id, &event) || self.script_export_event(id, &event) || self.script_deps_event(id, &event, event_loop) || self.script_console_event(id, &event, event_loop) || self.script_inspector_event(id, &event) || self.script_dialog_event(id, &event) || self.browser_tip_event(id, &event) || self.web_popover_event(id, &event) || self.web_replay_event(id, &event) || self.text_popover_event(id, &event) || self.suppressed_event(id, &event) || self.special_exceptions_event(id, &event) || self.trigger_import_event(id, &event) || self.cmdinfo_event(id, &event) {
            return;
        }
        let window_exists = self.current_window().is_some() && !self.resident;
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
                    // A color preset still previewed goes back before the window does (B2).
                    let previewed = slot.as_mut().is_some_and(|s| s.restore_preview(&mut self.store));
                    *slot = None;
                    crate::winkeys::set_recording(false);
                    if previewed {
                        self.apply_edit(Edit::Preview);
                    }
                    if pending {
                        self.apply_edit(Edit::GlobalsChanged);
                    }
                }
                WindowEvent::RedrawRequested => {
                    let win = slot.as_mut().unwrap();
                    win.show_remembered = self.show_remembered;
                    win.window_exists = window_exists;
                    let edit = win.redraw(&mut self.store, &mut self.globals, &mut self.snippets);
                    let answered = std::mem::take(&mut win.answered);
                    // Profiles > Session's "Configure Status Bar" opens the setup window.
                    let setup = slot.as_mut().and_then(|s| s.configure_status_bar.take());
                    // Configure SSH's help button opens its page in the browser.
                    let url = slot.as_mut().and_then(|s| s.open_url.take());
                    // An exported color preset is shown in Explorer.
                    let reveal = slot.as_mut().and_then(|s| s.take_reveal());
                    // Profiles > General's URL schemes menu asks before Tandem becomes the handler.
                    let scheme_ask = slot.as_mut().and_then(|s| s.scheme_ask.take());
                    // Profiles > Text's "Manage Special Exceptions" opens its window.
                    let special = slot.as_mut().and_then(|s| s.special_exceptions.take());
                    // Profiles > Terminal's "Change Locale" opens the picker.
                    let locale = slot.as_mut().and_then(|s| s.locale_pick.take());
                    // Edit Session's Copy Current Settings to Selected Profile, after its question.
                    let copy = slot.as_mut().and_then(|s| s.copy_request.take());
                    // The dynamic profile warning's Mark as Rewritable and Reveal in Explorer.
                    let dynamic = slot.as_mut().and_then(|s| s.dynamic_request.take());
                    // "Edit Profiles\u{2026}" opens the Settings window, which needs the event loop.
                    let edit = match edit {
                        Edit::EditProfiles(guid) => {
                            self.edit_profiles(event_loop, guid);
                            Edit::None
                        }
                        other => other,
                    };
                    self.apply_edit(edit);
                    self.settings_warnings_answered(answered);
                    if let Some(guid) = setup {
                        self.open_status_setup(statusbar_ui::SetupTarget::Shared(guid), None, false, event_loop);
                    }
                    if let Some(a) = scheme_ask {
                        self.ask_scheme(a);
                    }
                    if let Some(guid) = special {
                        self.open_special_exceptions(guid, event_loop);
                    }
                    if let Some(guid) = locale {
                        self.open_locale_settings(guid);
                    }
                    if let Some((pane, dest)) = copy {
                        self.copy_to_profile(pane, &dest);
                    }
                    match dynamic {
                        Some(crate::settings_ui::DynamicRequest::Mark(guid)) => self.mark_rewritable(&guid),
                        Some(crate::settings_ui::DynamicRequest::Reveal(guid)) => self.reveal_dynamic(&guid),
                        None => {}
                    }
                    if let Some(path) = reveal {
                        self.reveal_in_explorer(&path);
                    }
                    if let Some(url) = url {
                        if let Err(e) = semantic_ui::shell_open(&url, false) {
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
                self.refresh_scroll_style();
                self.refresh_panes();
                // A light or dark window may gain or lose its border (`BordersOnlyInLightMode`).
                self.apply_border();
            }
            // A drag over a pane whose program registered for drops (OSC 72) is reported to it.
            WindowEvent::HoveredFile(_) => self.dnd_hover(),
            WindowEvent::HoveredFileCancelled => self.dnd_cancel(),
            WindowEvent::DroppedFile(path) => {
                // The pointer as the system reports it now: the last pointer move is stale during an
                // OLE drag, and a file must never reach a pane the person did not drop on.
                let real = self.pointer_now();
                // Only the pane under the pointer, and only when its program asked: any other
                // drop keeps the file-name paste (a background pane never gets a file).
                if let Some((pt, p)) = real.and_then(|pt| self.pane_at(pt.0, pt.1).map(|(p, _)| (pt, p))).filter(|(_, p)| self.dnd_accepts(*p)) {
                    self.dnd_dropped.push((p, path, pt));
                    return;
                }
                // The drag ends here: a pane told of it (trust lost since) must not stay hovered.
                self.dnd_forget_hover();
                let (x, y) = real.unwrap_or(self.cursor);
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
                    g.surface.configure(g.render_device(), &g.config);
                }
                // Locked split pane widths hold through the resize (`TAB:6596-6640`).
                self.apply_width_pins();
                self.relayout();
                self.sync_blur();
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
            WindowEvent::Focused(f) => self.window_focus(id, f),
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                self.phys_mods = Mods { shift: s.shift_key(), alt: s.alt_key(), ctrl: s.control_key(), cmd: s.super_key() };
                self.phys_sides = crate::keyproto::Sides { lalt: m.lalt_state() == ModifiersKeyState::Pressed, ralt: m.ralt_state() == ModifiersKeyState::Pressed, lctrl: m.lcontrol_state() == ModifiersKeyState::Pressed, rctrl: m.rcontrol_state() == ModifiersKeyState::Pressed, lwin: m.lsuper_state() == ModifiersKeyState::Pressed, rwin: m.rsuper_state() == ModifiersKeyState::Pressed };
                // What every key reader sees: remapped here when Remap modifiers globally is off.
                self.apply_remapped_mods();
                // A held Win key keeps a flashing tab bar up (`PT flagsChanged:`, `PT:13278`).
                let was = self.tab_flash.state();
                self.tab_flash.set_win_held(self.mods.cmd, Instant::now());
                if was != self.tab_flash.state() {
                    self.redraw();
                }
                self.update_hover();
                // Cycle Tabs ends when the chord's modifiers come up.
                if let Some(c) = self.mru_mods {
                    if !((c.ctrl && self.mods.ctrl) || (c.alt && self.mods.alt) || (c.cmd && self.mods.cmd)) {
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
            // The pointer entered this window: focus follows mouse decides when it first
            // reaches a terminal pane (`mouse_move`), not over the chrome.
            WindowEvent::CursorEntered { .. } => self.ffm_entered = true,
            WindowEvent::CursorLeft { .. } => {
                // The pointer left the window: no Help tooltip stays up.
                self.ffm_entered = false;
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
                // A pane whose profile changed its font has a new cell and needs its grid again.
                self.ensure_faces();
                if sig != self.font_sig || self.pane_faces.stale() || self.laid_out.as_ref() != Some(&self.room()) {
                    self.relayout();
                }
                self.render();
                // The IME's candidate window follows the caret of the frame just drawn.
                self.ime_area();
            }
            // The IME's composition and commits; the Settings and tool windows (egui) keep
            // egui-winit's own handling.
            WindowEvent::Ime(ime) => self.ime_event(ime, event_loop),
            _ => {}
        }
    }
}

/// Lists the adapters once per run (name, backend, device type; P19) and records whether both
/// kinds of GPU exist for the settings sheet.
fn log_adapters_once(instance: &wgpu::Instance) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        for a in &adapters {
            let i = a.get_info();
            eprintln!("adapter available: backend={:?} name={} device_type={:?}", i.backend, i.name, i.device_type);
        }
        let kinds: Vec<wgpu::DeviceType> = adapters.iter().map(|a| a.get_info().device_type).collect();
        power::set_both_gpu_kinds(power::both_gpu_kinds(&kinds));
    });
}

fn init_gpu(window: Arc<Window>, event_loop: &ActiveEventLoop, power_preference: wgpu::PowerPreference) -> Result<Gpu, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
        event_loop.owned_display_handle(),
    )));
    let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
    log_adapters_once(&instance);
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference,
        compatible_surface: Some(&surface),
        ..Default::default()
    }))
    .map_err(|e| e.to_string())?;
    let info = adapter.get_info();
    eprintln!("renderer adapter: backend={:?} name={} vendor={:#06x} device_type={:?}", info.backend, info.name, info.vendor, info.device_type);
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
        alpha_mode: crate::transparency::pick_alpha_mode(&caps.alpha_modes, false),
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
        color_space: wgpu::SurfaceColorSpace::Auto,
    };
    surface.configure(&device, &config);
    let renderer = Renderer::new(&device, &queue, format, FONT, FONT_PX * window.scale_factor() as f32);
    Ok(Gpu { surface, adapter, device, queue, config, renderer, instance, alpha: None, shape: None, window })
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
    screen.set_profile_unicode(crate::osc1337::profile_unicode(p.values.get("Unicode Version").and_then(|v| v.as_integer())));
    screen.set_scrollback_with_status_bar(p.bool("Scrollback With Status Bar"));
    screen.set_alternate_scroll_allowed(p.bool("Allow Alternate Mouse Scroll"));
    let (list, prefs) = triggers_ui::triggers_of(p);
    screen.set_triggers(list, prefs);
}

/// Whether a window's menu bar hides: in full screen with `HideMenuBarInFullscreen`
/// (`P:830`); leaving full screen brings it back.
fn menu_should_hide(full_screen: bool, hide_key: bool) -> bool {
    full_screen && hide_key
}

/// The session's POSIX variables for a program started from `cmdline`: `TERMINFO_DIRS` only for
/// a WSL session (R6: MSYS2, Git Bash, cmd and PowerShell never get it), the rest as given.
/// `term_set` says 13o's `term_var` gave the session a `TERM`; a WSL session then lists it in
/// `WSLENV` so the profile's Terminal Type reaches the distribution (P17).
pub(super) fn posix_for(cmdline: &str, terminfo_dir: Option<&str>, colorfgbg: Option<String>, locale: Vec<(String, String)>, inherited_wslenv: Option<String>, term_set: bool) -> crate::session_env::Posix {
    let wsl = crate::session_env::is_wsl(cmdline);
    crate::session_env::Posix {
        colorfgbg,
        locale,
        terminfo: terminfo_dir.filter(|_| wsl).map(str::to_string),
        inherited_wslenv,
        wsl_term: term_set && wsl,
    }
}

/// DEC mode 2048: the text area a `rows` x `cols` grid covers in physical pixels, from the
/// pane's own cell (13n: each pane has its own face), as (height, width).
fn resize_report_px(rows: u16, cols: u16, (cw, ch): (f32, f32)) -> (u32, u32) {
    ((f32::from(rows) * ch).round() as u32, (f32::from(cols) * cw).round() as u32)
}

/// What one window focus event does, by effect group (preflight B6): `apply_focus` runs ONLY what
/// this table names, so a web view taking keyboard focus (`IntoOwnChild`) can skip the window
/// losses and keep the cleanup that still applies. Every statement of the arm is classified in the
/// source test `focus_arm_statements_are_classified`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct FocusActions {
    /// `focused`, the border, the job poll rate (13i dimming reads the flag): None leaves them.
    pub flag: Option<bool>,
    /// `focus_lost`: the window save marker.
    pub save: bool,
    /// A hotkey window hides.
    pub hotkey_hide: bool,
    /// The API's focus-lost event.
    pub api_lost: bool,
    /// The DECSET 1004 report sent to the focused pane.
    pub report: Option<bool>,
    /// Key state that would wait for a release or a commit: belt, replay and composer keys, `sent_keys`, a half-typed composition.
    pub clear_keys: bool,
    /// A pending Leader.
    pub drop_leader: bool,
    /// A divider or title drag, a command click, a tab press and the tab tip: the pointer is elsewhere.
    pub mouse_cancel: bool,
    /// The tab bar's flash stops (the window resigned key).
    pub flash_stop: bool,
    /// UI Automation is told the focus (it decides its own target, B7).
    pub access: bool,
    /// A gain that was reported: last window, the stamp, the Window menu, a closed password manager's hold.
    pub gain_stamps: bool,
    /// The bell count resets: a window became key (13w, `PT:4717`, `:4999`). Never on the return
    /// from the window's own web view, whose loss was never reported (13w seam S2).
    pub bell_reset: bool,
}

pub(super) fn focus_actions(step: crate::webhost::FocusStep, gained: bool) -> FocusActions {
    use crate::webhost::FocusStep::*;
    match (step, gained) {
        // A loss that stays in the window (focus went to its own web view): the pointer is
        // elsewhere and keys go to the child, so those cleanups run; the window did not resign
        // key, so nothing else does (`PseudoTerminal.m:5421-5500` runs only on resignation).
        (IntoOwnChild, _) => FocusActions { mouse_cancel: true, clear_keys: true, access: true, ..FocusActions::default() },
        // The loss was never reported, so neither is this gain; only UI Automation follows.
        (ReturnFromChild, _) => FocusActions { access: true, ..FocusActions::default() },
        (Normal, false) => FocusActions {
            flag: Some(false),
            save: true,
            hotkey_hide: true,
            api_lost: true,
            report: Some(false),
            clear_keys: true,
            drop_leader: true,
            mouse_cancel: true,
            flash_stop: true,
            access: true,
            gain_stamps: false,
            bell_reset: false,
        },
        (Normal, true) => FocusActions { flag: Some(true), report: Some(true), access: true, gain_stamps: true, bell_reset: true, ..FocusActions::default() },
    }
}

impl App {
    /// `WindowEvent::Focused`. Keyboard focus moving into this window's own web view arrives as a
    /// loss, but the window is still the foreground window: `focus_step` tells the two apart and
    /// `apply_focus` runs only what `focus_actions` names. A held focus is re-checked by
    /// `web_focus_recheck`, because the real deactivation sends the parent nothing.
    fn window_focus(&mut self, id: WindowId, f: bool) {
        let held = self.web_focus_held == Some(id);
        let step = match self.hwnd() {
            Some(hwnd) => crate::webhost::focus_step(f, hwnd, crate::webhost_win::foreground_root(), held),
            None => crate::webhost::FocusStep::Normal,
        };
        match step {
            crate::webhost::FocusStep::IntoOwnChild => self.web_focus_held = Some(id),
            // A real event (either way) ends the hold: the loss or gain it carries is reported now.
            crate::webhost::FocusStep::ReturnFromChild | crate::webhost::FocusStep::Normal => {
                if held {
                    self.web_focus_held = None;
                }
            }
        }
        if !f && step == crate::webhost::FocusStep::Normal {
            // The app is no longer in front: no link tip stays over another program.
            self.browser_tip_clear_all();
        }
        self.apply_focus(step, f, id);
        self.sync_web_keyboard();
    }

    /// Runs one focus step in the current window: only what the table names (preflight B6).
    fn apply_focus(&mut self, step: crate::webhost::FocusStep, f: bool, id: WindowId) {
        let a = focus_actions(step, f);
        // 13w seam S2: a real focus change is logged as 13w logs it; a move into or out of the
        // window's own web view is one fixed line (no window lost or gained focus).
        if step == crate::webhost::FocusStep::Normal {
            crate::dlog!("window focus changed: {}", f);
        } else {
            crate::dlog!("focus moved between a window and its web view");
        }
        // A window becoming key resets the bell count (`PT:4717`, `:4999`).
        if a.bell_reset {
            self.bell_badge.reset_bells();
        }
        // A divider drag loses its release when focus goes, and so do keys.
        if a.mouse_cancel {
            self.tmux_drag_cancel();
            // A title bar drag ends with nothing moved.
            self.title_drag_lost(pane_move_ui::DragLost::FocusLost);
            // A command click waiting out the double-click time does not outlive the focus (B7).
            self.command_click = None;
            // So does a press on the tab bar (a held close button lets go).
            if self.tab_press.take().is_some() {
                self.redraw();
            }
            // And the tab tooltip goes, as a macOS tooltip does when the app deactivates.
            self.hide_tab_tip();
        }
        if a.flash_stop {
            // And the tab bar's flash stops (`PT windowDidResignKey:`, `PT:5470-5471`):
            // it fades, and in full screen a second stop ends it at once (`PT:5484`).
            self.stop_tab_flash();
            self.tab_flash.set_win_held(false, Instant::now());
            if self.full_screen() {
                self.stop_tab_flash();
            }
        }
        if a.clear_keys {
            self.belt_keys.clear();
            self.replay_keys.clear();
            self.composer_keys.clear();
            // A half-typed composition is dropped, not committed.
            self.ime_cancel();
            // Keys held while the window loses focus send no release to Tandem.
            self.sent_keys.clear();
        }
        if let Some(flag) = a.flag {
            self.focused = flag;
            // The border takes the focused or unfocused color.
            self.apply_border();
            self.jobs.set_fast(flag);
            // Tandem going to the background saves the windows (iTerm2 saves on resign
            // active); a switch between Tandem's own windows gains focus again at once.
            self.focus_lost = a.save;
        }
        if a.access {
            self.access_focus();
        }
        if a.hotkey_hide && self.hotkey.is_some() {
            self.hotkey_focus_lost();
        }
        // Focus reporting (DECSET 1004), before the password manager's hold ends below: tell the app the window gained or lost focus.
        if let Some(gained) = a.report {
            if let Some(pane) = self.focused_pane_id() {
                self.report_focus_to(pane, gained);
            }
        }
        if a.gain_stamps {
            self.last_window = Some(id);
            self.stamp_focus();
            // The Window menu checks the focused window.
            self.refresh_window_list();
            // A closed password manager's hold ends with the window's next gain, which is
            // not reported either (its loss was not).
            self.pwm_focus_gained();
        }
        if a.api_lost {
            self.api_focus_lost();
        }
        // iTerm2 drops a pending Leader when the app goes to the background.
        if a.drop_leader && self.leader_on {
            self.leader_on = false;
            self.refresh_route();
        }
        self.redraw();
    }

    /// A window's Win32 handle, the current one or a parked one.
    fn window_hwnd(&self, id: WindowId) -> Option<isize> {
        if self.current_window() == Some(id) {
            return self.hwnd();
        }
        self.windows.parked().find_map(|(_, s)| s.gpu.as_ref().filter(|g| g.window.id() == id).and_then(|g| menu::hwnd_of(&g.window)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of `name`'s body in `src` (to its closing brace at `indent`).
    fn fn_body<'a>(src: &'a str, name: &str, indent: &str) -> &'a str {
        let rest = src.split(name).nth(1).unwrap_or_else(|| panic!("{name}"));
        rest.split(&format!("\n{indent}}}\n")).next().unwrap()
    }

    /// Review 3: the trigger pool's width is read after "Load settings from a custom folder or
    /// URL" replaced the globals, so that location's MaximumNumberOfTriggerCommands counts.
    #[test]
    fn the_pool_width_is_read_after_the_custom_settings_load() {
        let src = include_str!("mod.rs");
        let load = src.find("prefs_ui::load_at_start(&mut globals").expect("the load");
        let read = src.find("let trigger_pool_width = globals.int(").expect("the read");
        assert!(read > load, "the pool width is read after load_at_start");
    }

    #[test]
    fn focus_into_web_view_keeps_the_window() {
        use crate::webhost::FocusStep::*;
        let a = focus_actions(IntoOwnChild, false);
        assert_eq!(a.flag, None, "the window stays focused for 13i's dimming");
        assert!(!a.save && !a.hotkey_hide && !a.api_lost && !a.drop_leader);
        assert_eq!(a.report, None, "no DECSET 1004 focus-out");
        assert!(a.clear_keys, "key releases go to the child, never to winit");
        let n = focus_actions(Normal, false);
        assert_eq!(n.flag, Some(false));
        assert!(n.save && n.hotkey_hide && n.api_lost && n.clear_keys && n.drop_leader && n.mouse_cancel && n.flash_stop && n.access);
        assert_eq!(n.report, Some(false));
        assert!(!n.gain_stamps);
        let r = focus_actions(ReturnFromChild, true);
        assert_eq!(r.flag, None);
        assert_eq!(r.report, None, "the loss was never reported, so no focus-in either");
        assert!(!r.gain_stamps, "the gain stamps belong to a reported gain");
        assert!(!r.bell_reset, "13w S2: the bell count resets only on a reported gain");
        let g = focus_actions(Normal, true);
        assert_eq!(g.flag, Some(true));
        assert_eq!(g.report, Some(true));
        assert!(g.gain_stamps && g.access && g.bell_reset);
        assert!(!g.save && !g.hotkey_hide && !g.api_lost && !g.clear_keys && !g.drop_leader && !g.mouse_cancel && !g.flash_stop);
    }

    #[test]
    fn into_own_child_runs_mouse_and_key_cleanup_only() {
        use crate::webhost::FocusStep::*;
        let a = focus_actions(IntoOwnChild, false);
        let expected = FocusActions { mouse_cancel: true, clear_keys: true, access: true, ..FocusActions::default() };
        assert_eq!(a, expected, "the pointer is elsewhere and keys go to the child; the tab flash and every window loss stay");
        assert!(!a.flash_stop, "the window stays key");
        // The gain that follows runs nothing but UI Automation.
        assert_eq!(focus_actions(ReturnFromChild, true), FocusActions { access: true, ..FocusActions::default() });
    }

    /// Every `self.` path the arm's body uses, with the field of `FocusActions` that guards it
    /// (`always` for the unguarded redraw). A statement added to the arm (13w's bell reset, a log
    /// line, a later plan's effect) that is not here fails `focus_arm_statements_are_classified`.
    const CLASSIFIED: &[(&str, &str)] = &[
        ("self.tmux_drag_cancel", "mouse_cancel"),
        ("self.title_drag_lost", "mouse_cancel"),
        ("self.command_click", "mouse_cancel"),
        ("self.tab_press.take", "mouse_cancel"),
        ("self.hide_tab_tip", "mouse_cancel"),
        ("self.stop_tab_flash", "flash_stop"),
        ("self.tab_flash.set_win_held", "flash_stop"),
        ("self.full_screen", "flash_stop"),
        ("self.belt_keys.clear", "clear_keys"),
        ("self.replay_keys.clear", "clear_keys"),
        ("self.composer_keys.clear", "clear_keys"),
        ("self.ime_cancel", "clear_keys"),
        ("self.sent_keys.clear", "clear_keys"),
        ("self.focused", "flag"),
        ("self.apply_border", "flag"),
        ("self.jobs.set_fast", "flag"),
        ("self.focus_lost", "save"),
        ("self.access_focus", "access"),
        ("self.hotkey.is_some", "hotkey_hide"),
        ("self.hotkey_focus_lost", "hotkey_hide"),
        ("self.last_window", "gain_stamps"),
        ("self.stamp_focus", "gain_stamps"),
        ("self.refresh_window_list", "gain_stamps"),
        ("self.pwm_focus_gained", "gain_stamps"),
        ("self.api_focus_lost", "api_lost"),
        ("self.leader_on", "drop_leader"),
        ("self.refresh_route", "drop_leader"),
        ("self.focused_pane_id", "report"),
        ("self.report_focus_to", "report"),
        ("self.bell_badge.reset_bells", "bell_reset"),
        ("self.redraw", "always"),
    ];

    /// The body of `fn apply_focus` and the struct's field names, from the source text.
    fn focus_source() -> (&'static str, Vec<&'static str>) {
        let src: &'static str = include_str!("mod.rs");
        let body = fn_body(src, "fn apply_focus(", "    ");
        let def = src.split("pub(super) struct FocusActions {").nth(1).unwrap().split("\n}\n").next().unwrap();
        let fields = def.lines().filter_map(|l| l.trim().strip_prefix("pub ")).map(|l| l.split(':').next().unwrap().trim()).collect();
        (body, fields)
    }

    /// Preflight B6/S2: the arm calls only `window_focus`, and everything `apply_focus` touches is classified.
    #[test]
    fn focus_arm_statements_are_classified() {
        let src = include_str!("mod.rs");
        let arm = src.split("WindowEvent::Focused(f) =>").nth(1).unwrap().lines().next().unwrap();
        assert_eq!(arm.trim(), "self.window_focus(id, f),", "the arm is one call; every effect goes through the table");
        let (body, fields) = focus_source();
        let mut seen = std::collections::BTreeSet::new();
        let mut rest = body;
        while let Some(i) = rest.find("self.") {
            let tail = &rest[i..];
            let end = tail.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')).unwrap_or(tail.len());
            seen.insert(tail[..end].trim_end_matches('.').to_string());
            rest = &tail[end..];
        }
        for path in &seen {
            assert!(CLASSIFIED.iter().any(|(p, _)| p == path), "apply_focus uses {path}, which no FocusActions field classifies (add the effect to the table)");
        }
        for (path, field) in CLASSIFIED {
            assert!(seen.contains(*path), "{path} is classified but apply_focus no longer uses it");
            if *field != "always" {
                assert!(fields.contains(field), "{field} is not a FocusActions field");
                assert!(body.contains(&format!("a.{field}")), "apply_focus never reads a.{field}");
            }
        }
    }

    /// 13x Task 1a fix: the DECSET 1004 report runs BEFORE the password manager's hold is released
    /// (`pwm_focus_gained`), as the old arm did, so a gain the hold swallows is never reported.
    #[test]
    fn the_focus_report_runs_before_the_password_hold_ends() {
        let (body, _) = focus_source();
        let report = body.find("self.report_focus_to(").expect("the report");
        let hold = body.find("self.pwm_focus_gained()").expect("the hold release");
        assert!(report < hold, "report at {report}, pwm_focus_gained at {hold}");
    }

    #[test]
    fn focus_actions_cover_the_arm() {
        let (_, fields) = focus_source();
        let classified: std::collections::BTreeSet<&str> = CLASSIFIED.iter().map(|(_, f)| *f).filter(|f| *f != "always").collect();
        let mut named: Vec<&str> = fields.clone();
        named.sort();
        assert_eq!(named.len(), classified.len(), "the table has one field per statement group");
        assert_eq!(named, classified.into_iter().collect::<Vec<_>>());
        // report is read through `a.report` too, and the web hold is the arm's own state.
        assert!(include_str!("mod.rs").contains("web_focus_held: Option<WindowId>"));
    }

    /// 13y2 Task 9d (`PTYTab.m:951-960`, `PTYSession.m:9856-9886`): the sequence of reports two
    /// terminal panes get through the app's ledger, and the wiring that makes it one path.
    #[test]
    fn terminal_panes_report_focus() {
        use crate::webhost::{pane_focus_reports, FocusFacts, FocusLedger};
        let on = FocusFacts { held: false, gateway: false, wants: true };
        let mut led = FocusLedger::default();
        let wrote = std::cell::RefCell::new(Vec::<(u64, bool)>::new());
        let tell = |led: &mut FocusLedger, pane: u64, gained: bool, f: FocusFacts| {
            if led.report(pane, gained, f) {
                wrote.borrow_mut().push((pane, gained));
            }
        };
        // The window gains focus with pane 1 active, then focus moves to pane 2.
        tell(&mut led, 1, true, on);
        for (p, g) in pane_focus_reports(Some((1, false)), Some((2, false))) {
            tell(&mut led, p, g, on);
        }
        assert_eq!(*wrote.borrow(), vec![(1, true), (1, false), (2, true)]);
        // The next tick with no change writes nothing (a repeated step is a no-op).
        wrote.borrow_mut().clear();
        tell(&mut led, 2, true, on);
        tell(&mut led, 1, false, on);
        assert!(wrote.borrow().is_empty(), "nothing twice");
        // Pane 2 without DECSET 1004 gets nothing while pane 1 still hears its focus-out.
        let off = FocusFacts { wants: false, ..on };
        tell(&mut led, 2, false, on);
        tell(&mut led, 1, true, on);
        wrote.borrow_mut().clear();
        for (p, g) in pane_focus_reports(Some((1, false)), Some((2, false))) {
            tell(&mut led, p, g, if p == 2 { off } else { on });
        }
        assert_eq!(*wrote.borrow(), vec![(1, false)]);
        // A held pane and a gateway pane get nothing.
        wrote.borrow_mut().clear();
        tell(&mut led, 7, true, FocusFacts { held: true, ..on });
        tell(&mut led, 8, true, FocusFacts { gateway: true, ..on });
        assert!(wrote.borrow().is_empty());
        // The window arm reported the loss; the pane change while unfocused then writes nothing
        // new, and the window's next gain reports to the pane focused then, once.
        tell(&mut led, 1, false, on);
        tell(&mut led, 2, false, on);
        wrote.borrow_mut().clear();
        tell(&mut led, 2, false, on);
        tell(&mut led, 1, false, on);
        assert!(wrote.borrow().is_empty());
        tell(&mut led, 1, true, on);
        tell(&mut led, 1, true, on);
        assert_eq!(*wrote.borrow(), vec![(1, true)]);
        // The tick's catch-up reports go through the same ledger (driven in
        // `webhost::focus_tick_catches_panes_up`: close of the active pane, a hold that ended).
        assert!(fn_body(include_str!("webhost_app.rs").split("#[cfg(test)]").next().unwrap(), "fn pane_focus_tick(", "    ").contains("crate::webhost::focus_tick("));
        // Wiring: one path, a tick per pass, state dropped with the pane, no second reporter.
        let live = include_str!("mod.rs").split("#[cfg(test)]\nmod tests").next().unwrap();
        assert!(live.contains("self.pane_focus_tick();"));
        assert!(fn_body(live, "pub(super) fn pane_gone(", "    ").contains("self.focus_reported.forget(id);"));
        assert!(!include_str!("browser_app.rs").split("#[cfg(test)]").next().unwrap().contains("browser_focus_moved"), "the tick owns the pane comparison");
        let w = include_str!("webhost_app.rs").split("#[cfg(test)]").next().unwrap();
        assert!(!w.contains("p.screen.term().mode().contains(alacritty_terminal::term::TermMode::FOCUS_IN_OUT)"));
        assert!(fn_body(w, "fn report_focus_to(", "    ").contains("self.focus_reported.report("));
    }

    /// 13w seam S12: the browser views are placed before the first draw timer starts, and only
    /// there (once per frame).
    #[test]
    fn browser_views_are_placed_before_the_draw_timers() {
        let src = include_str!("mod.rs");
        let render = fn_body(src, "fn render(&mut self)", "    ");
        assert!(render.find("self.place_browser_views();").unwrap() < render.find("let started = timing.then(Instant::now);").unwrap());
        let live = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        assert_eq!(live.matches("self.place_browser_views();").count(), 1);
    }

    #[test]
    fn per_frame_paths_do_no_idle_work() {
        // Adopting session hotkeys runs every pass: with every pane seen it returns before any Vec.
        let adopt = fn_body(include_str!("session_hotkey_ui.rs"), "fn session_hotkeys_adopt(", "    ");
        let early = adopt.find("return").expect("an early return");
        assert!(early < adopt.find("collect").expect("the new panes"), "the early return comes first");
        // The non-sRGB view of the swap-chain texture is made only while a fade runs.
        let render = include_str!("mod.rs");
        let raw = render.split("let raw = ").nth(1).and_then(|r| r.split(';').next()).expect("the raw view");
        assert!(raw.contains("frame.fade < 1.0"), "the raw view is made only for a fading frame: {raw}");
        // The fade gate and the frame read see-through from one place.
        let build = fn_body(render, "fn build_frame(", "    ");
        assert!(build.contains("self.see_through_with(&alphas)"), "build_frame uses the shared see-through test");
        assert!(!build.contains("needs_alpha("), "build_frame repeats no see-through expression");
    }

    /// DEC mode 2048 x 13n: the report's pixel size is the grid times the pane's OWN cell (its
    /// face, Make Text Bigger), the same cell its grid was sized from, never the window's.
    #[test]
    fn resize_report_uses_the_pane_cell() {
        assert_eq!(resize_report_px(24, 80, (10.0, 20.0)), (480, 800));
        assert_eq!(resize_report_px(12, 40, (20.5, 40.25)), (483, 820));
        let src = include_str!("mod.rs");
        let body = &src[src.find("    fn relayout(&mut self) {").unwrap()..];
        let body = &body[..body.find("\n    }\n").unwrap()];
        assert!(body.contains(".map(|(id, rect)| (id, self.grid_size_for(id, rect), self.pane_cell(id)))"), "sizes carry each pane's cell");
        assert!(body.contains("for (id, (cols, rows), (cw, ch)) in sizes {"));
        assert!(body.contains("p.resize_report_if_on(rows, cols, resize_report_px(rows, cols, (cw, ch)))"), "the report reads the tuple's cell");
        assert!(!body.contains("self.cell()"), "relayout never reads the window cell");
    }

    /// Character Encoding (13o Task 8): every path that hands a pane its emulation rules (install,
    /// refresh, settings change, tmux install) also hands it the profile's encoding, and a tmux
    /// pane or gateway never gets one.
    #[test]
    fn encoding_travels_with_the_emulation_rules() {
        let src = include_str!("emu_ui.rs");
        let body = &src[src.find("fn apply_emu_rules").unwrap()..];
        let body = &body[..body.find("\n    }\n").unwrap()];
        assert!(body.contains("\"Character Encoding\""));
        assert!(body.contains("p.tmux_io().is_none() && !p.gateway"));
        assert!(body.contains("p.set_encoding("));
    }

    /// Task 7 fix (P21): one "Allow Printing?" alert is pending per pane, and its answer checks
    /// the guard again, so an Allow queued before a Disable never prints.
    #[test]
    fn print_guard_answers_are_rechecked() {
        let src = include_str!("emu_ui.rs");
        let ask = &src[src.find("fn print_request").unwrap()..];
        let ask = &ask[..ask.find("\n    }\n").unwrap()];
        assert!(ask.contains("alert_waits(|t| matches!(t, Then::PrintGuard { pane: p, .. } if *p == pane))"), "a pane with a question pending asks no more");
        assert!(ask.find("alert_waits").unwrap() < ask.find(".check(").unwrap(), "a dropped attempt is not counted");
        let answer = &src[src.find("fn print_guard_answered").unwrap()..];
        let answer = &answer[..answer.find("\n    }\n").unwrap()];
        assert!(answer.contains("print_guards.get_mut(&pane)"), "no guard is made for a pane that closed");
        assert!(answer.contains(".refused("), "the answer re-checks the guard");
        assert!(!answer.contains("or_default"));
    }

    #[test]
    fn terminfo_is_for_wsl_only() {
        let dir = Some(r"C:\A\Tandem\terminfo");
        let at = |cmd: &str| posix_for(cmd, dir, Some("15;0".into()), vec![("LANG".into(), "en_US.UTF-8".into())], Some("X/p".into()), true);
        assert_eq!(at("wsl.exe -d Ubuntu").terminfo.as_deref(), dir);
        assert_eq!(at(r#""C:\Windows\System32\wsl.exe""#).terminfo.as_deref(), dir);
        for other in ["pwsh.exe", r"C:\msys64\usr\bin\bash.exe --login", "cmd.exe", r"C:\Program Files\Git\bin\bash.exe", "ubuntu.exe"] {
            let p = at(other);
            assert_eq!(p.terminfo, None, "{other}");
            // The rest is untouched for every program.
            assert_eq!(p.colorfgbg.as_deref(), Some("15;0"));
            assert_eq!(p.locale, vec![("LANG".to_string(), "en_US.UTF-8".to_string())]);
            assert_eq!(p.inherited_wslenv.as_deref(), Some("X/p"));
        }
        assert_eq!(posix_for("wsl.exe", None, None, Vec::new(), None, false).terminfo, None, "no folder, no variable");
        let line = crate::session_env::posix_vars(&at("pwsh.exe"));
        assert!(!line.iter().any(|v| v.starts_with("TERMINFO_DIRS=") || v.contains("TERMINFO_DIRS")), "{line:?}");
    }

    /// P17: 13o's `TERM` reaches a WSL session through `WSLENV`, only when `term_var` set it
    /// (a TERM in Tandem's own environment or a profile's own `TERM=` entry wins and is not
    /// listed), never for another program; the `TERM=` entry goes after the POSIX block.
    #[test]
    fn term_reaches_wsl_through_wslenv() {
        let at = |cmd: &str, set: bool| crate::session_env::posix_vars(&posix_for(cmd, None, None, Vec::new(), None, set));
        assert_eq!(at("wsl.exe -d Ubuntu", true), ["WSLENV=TERM"]);
        assert_eq!(at(r#""C:\Windows\System32\wsl.exe""#, true), ["WSLENV=TERM"]);
        assert!(at("wsl.exe", false).is_empty(), "no TERM set, nothing listed");
        for other in ["pwsh.exe", "cmd.exe", r"C:\msys64\usr\bin\bash.exe --login", "ubuntu.exe"] {
            assert!(at(other, true).is_empty(), "{other}");
        }
        let src = include_str!("mod.rs");
        let body = &src[src.find("    fn session_env(").unwrap()..];
        let body = &body[..body.find("\n    }\n").unwrap()];
        let term = body.find("let term = crate::session_env::term_var(").expect("TERM is decided first");
        assert!(term < body.find("self.posix(p, cmd, term.is_some())").unwrap());
        assert!(body.find("posix_vars(&posix)").unwrap() < body.find("env.extend(term)").unwrap(), "TERM= still goes last");
    }

    /// 13y1 Task 7 round 4: a launch's own entries (a git launch's `-NAME` removals) are applied after
    /// everything else the session sets, so they reach the child (`session_env::launch_last`).
    #[test]
    fn session_env_applies_the_launch_last() {
        let src = include_str!("mod.rs");
        let body = &src[src.find("    fn session_env(").unwrap()..];
        let body = &body[..body.find("\n    }\n").unwrap()];
        // Split so this test does not count as a second take (`workgroup_env_keeps_session_guards`).
        let take = body.find(concat!("let launch = std::mem::take(&mut self.", "start_extra_env);")).expect("the launch is taken once");
        let last = body.find("(crate::session_env::launch_last(env, &launch), cookie)").expect("the launch goes last");
        assert!(take < last && body.find("env.extend(term)").unwrap() < last, "after TERM");
        assert!(body.trim_end().ends_with("(crate::session_env::launch_last(env, &launch), cookie)"), "nothing follows it");
    }

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
    fn a_profile_edit_reaches_tmux_panes_alternate_scroll_gate() {
        // refresh_panes walks every pane (tmux ones too) through screen_follows_profile on its own screen.
        let mut pane = Pane::tmux(1, 0, 0, 10, 3);
        let mut p = Profile::new("x");
        p.values.insert("Allow Alternate Mouse Scroll".into(), toml::Value::Boolean(false));
        screen_follows_profile(pane.own_mut(), &p);
        pane.own_mut().feed(b"\x1b[?1007h");
        assert!(!pane.own_mut().alternate_scroll_mode());
        p.values.insert("Allow Alternate Mouse Scroll".into(), toml::Value::Boolean(true));
        screen_follows_profile(pane.own_mut(), &p);
        pane.own_mut().feed(b"\x1b[?1007h");
        assert!(pane.own_mut().alternate_scroll_mode());
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
