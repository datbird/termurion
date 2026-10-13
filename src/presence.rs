//! Leaving the taskbar and Alt+Tab (13g): when Termurion acts as iTerm2's UI element, and what a
//! window's styles are. Pure; `app::presence_ui` applies it.

pub const UI_ELEMENT_KEY: &str = "HideFromDockAndAppSwitcher";
pub const REQUIRES_HOTKEYS_KEY: &str = "UIElementRequiresHotkeys";
pub const EXCLUDED_FROM_CYCLING_KEY: &str = "HotkeyWindowsExcludedFromCycling";

/// The persistent warning before the setting turns on (`APV:251-262`, Mac words replaced).
pub const EXCLUDE_HEADING: &str = "Warning";
pub const EXCLUDE_WARNING: &str = "When Termurion is excluded from the taskbar, you can always get back to Settings using the notification area icon. Look for a Termurion icon on the right side of your taskbar.";
pub const EXCLUDE_ACTION: &str = "Exclude From Taskbar and Alt+Tab Switcher";
pub const CANCEL: &str = "Cancel";

/// Whether Termurion is a UI element now (`AD:3756-3765`): the setting, narrowed by the second
/// to "every terminal window is a hotkey window" (true on an empty list, as `allWithBlock`).
pub fn is_ui_element(ui_element: bool, requires_hotkeys: bool, windows: usize, hotkey_windows: usize) -> bool {
    ui_element && (!requires_hotkeys || hotkey_windows >= windows)
}

/// A window's two presence styles: `WS_EX_TOOLWINDOW` (no taskbar button, not in Alt+Tab) and
/// the taskbar skip alone (winit's `set_skip_taskbar`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub tool_window: bool,
    pub skip_taskbar: bool,
}

/// The styles of a window. `hotkey` is None for a regular window, Some(open) for a hotkey
/// window (`PT:1026-1047`). A hotkey window never shows a taskbar button; it is in Alt+Tab
/// only while open and not excluded from cycling. A hidden one is cloaked, which already keeps
/// it out of Alt+Tab, so it never carries the tool style (toggling it would hide and show the
/// window and trip the hotkey window's focus-loss hide).
pub fn window_look(ui_element_now: bool, hotkey: Option<bool>, excluded_from_cycling: bool) -> Look {
    let tool_window = ui_element_now || (hotkey == Some(true) && excluded_from_cycling);
    Look { tool_window, skip_taskbar: ui_element_now || hotkey.is_some() }
}

/// The window that owns the tray menu: the current terminal window, else the notification window
/// (a hidden top-level window), so the menu opens with no terminal window made yet. A zero handle is none.
pub fn tray_menu_owner(terminal: Option<isize>, notify: Option<isize>) -> Option<isize> {
    terminal.filter(|h| *h != 0).or(notify.filter(|h| *h != 0))
}

/// An Accessory window (`frames::ACCESSORY`) keeps its own taskbar skip.
pub fn with_accessory(look: Look, accessory: bool) -> Look {
    Look { tool_window: look.tool_window, skip_taskbar: look.skip_taskbar || accessory }
}

/// Whether a style change must hide and show the window for the taskbar to follow: a visible
/// regular window only. A hotkey window has no button, and hiding it would drop its focus.
pub fn restyle_needs_reshow(hotkey: bool, visible: bool) -> bool {
    visible && !hotkey
}

/// How a window is shown again after its style flipped (`ShowWindow`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reshow {
    /// SW_SHOWMINNOACTIVE: a minimized window stays minimized.
    MinimizedNoActivate,
    /// SW_SHOWMAXIMIZED: a maximized window stays maximized.
    Maximized,
    /// SW_SHOW: the focused window keeps its activation.
    Show,
    /// SW_SHOWNA: any other window, no focus change.
    ShowNoActivate,
}

/// The show command after the hide of a style flip. `maximized` is an OS-maximized window (not
/// full screen, which is sized by hand and shows as a normal window).
pub fn reshow_command(minimized: bool, maximized: bool, focused: bool) -> Reshow {
    if minimized {
        Reshow::MinimizedNoActivate
    } else if maximized {
        Reshow::Maximized
    } else if focused {
        Reshow::Show
    } else {
        Reshow::ShowNoActivate
    }
}

/// Whether Next and Previous Window pass a hotkey window by (`PT:1026-1047`).
pub fn cycling_skips(hotkey: Option<bool>, excluded_from_cycling: bool) -> bool {
    match hotkey {
        Some(open) => !open || excluded_from_cycling,
        None => false,
    }
}

pub const STATUS_BAR_ICON_KEY: &str = "StatusBarIcon";
/// The notification area icon's tip.
pub const TRAY_TIP: &str = "Termurion";

/// The icon's menu (`AD:1846-1880`): label, and the selector of the menu item it runs or the
/// kind of submenu ("buried", "main").
pub const TRAY_ITEMS: [(&str, &str); 7] = [
    ("New Window", "newWindow:"),
    ("Settings", "showPrefWindow:"),
    ("Bring All Windows to Front", "arrangeInFront:"),
    ("Buried Sessions", "buried"),
    ("Check For Updates", "checkForUpdatesFromMenu:"),
    ("Main Menu", "main"),
    ("Quit Termurion", "terminate:"),
];

/// "HotKey Window Dock Click Action" values.
pub const DOCK_DO_NOT_SHOW: i64 = 0;
pub const DOCK_ALWAYS_SHOW: i64 = 1;
pub const DOCK_SHOW_IF_NONE: i64 = 2;

/// Whether the notification area icon is there: iTerm2's rule (`APP:979-993`), the status item
/// exists only in UI-element mode and with `StatusBarIcon` on (read at start). The way back
/// without it is starting Termurion again, which acts as the dock click (`launch::Request::Reopen`).
pub fn tray_wanted(ui_element_now: bool, status_bar_icon: bool) -> bool {
    ui_element_now && status_bar_icon
}

/// The icon's state after a pass tried to reach `want`: a failed add or remove leaves it as it
/// was, so the next pass tries again (an Explorer that is not up yet, at logon).
pub fn tray_settle(want: bool, on: bool, done: bool) -> bool {
    if done {
        want
    } else {
        on
    }
}

/// Whether a pass has anything to do for the icon.
pub fn tray_changes(want: bool, on: bool) -> bool {
    want != on
}

/// How many failures in a row end the backoff: from here the retry stays at 30 s (and is logged once).
pub const TRAY_MAX_ATTEMPTS: u32 = 12;

/// The wait after `failures` failed adds in a row: 1 s, doubling, at most 30 s, for as long as it
/// keeps failing (never gives up); None for no failure at all.
pub fn tray_retry_delay(failures: u32) -> Option<std::time::Duration> {
    if failures == 0 {
        return None;
    }
    Some(std::time::Duration::from_secs((1u64 << (failures - 1).min(5)).min(30)))
}

/// Whether a pass tries the icon now: it differs from what is wanted and the last failure's delay
/// has passed. `_failures` no longer limits it: a resident Termurion keeps trying.
pub fn tray_attempt_due(want: bool, on: bool, _failures: u32, due: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    tray_changes(want, on) && due.is_none_or(|d| now >= d)
}

/// Whether this failure is the one that ends the backoff (logged once).
pub fn tray_enters_steady_retry(before: u32, after: u32) -> bool {
    before < TRAY_MAX_ATTEMPTS && after == TRAY_MAX_ATTEMPTS
}

/// A notification area operation for adding the icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayOp {
    Add,
    Modify,
}

/// The operations to try, in order, to get the icon up. When it is added, or an earlier add failed
/// (it may have timed out after succeeding), a modify goes first: success means it exists, and a
/// duplicate add is never made.
pub fn tray_add_order(added: bool, add_unsure: bool) -> &'static [TrayOp] {
    if added || add_unsure {
        &[TrayOp::Modify, TrayOp::Add]
    } else {
        &[TrayOp::Add]
    }
}

/// The failure count after an attempt: cleared by a success.
pub fn tray_failures_after(failures: u32, ok: bool) -> u32 {
    if ok {
        0
    } else {
        failures.saturating_add(1)
    }
}

/// Termurion's logo (docs/assets/tandem-logo.svg) as `size` x `size` RGBA pixels for the tray: the
/// blue card behind, the dark terminal card in front with its border, the prompt chevron and
/// cursor bar. 3x3 supersampled; clear outside the cards.
pub fn tray_icon_rgba(size: u32) -> Vec<u8> {
    fn round_rect(x: f64, y: f64, (rx, ry, w, h, r): (f64, f64, f64, f64, f64)) -> f64 {
        // Signed distance, negative inside.
        let (cx, cy) = (rx + w / 2.0, ry + h / 2.0);
        let (qx, qy) = ((x - cx).abs() - (w / 2.0 - r), (y - cy).abs() - (h / 2.0 - r));
        qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
    }
    fn segment(x: f64, y: f64, (ax, ay): (f64, f64), (bx, by): (f64, f64)) -> f64 {
        let (dx, dy) = (bx - ax, by - ay);
        let t = (((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        (x - (ax + t * dx)).hypot(y - (ay + t * dy))
    }
    let sample = |x: f64, y: f64| -> [f64; 4] {
        // Back to front, in the logo's 160 unit space.
        // Premultiplied colour, then alpha.
        let mut out = [0.0; 4];
        let mut put = |c: [f64; 3], a: f64| {
            for i in 0..3 {
                out[i] = c[i] * a + out[i] * (1.0 - a);
            }
            out[3] = a + out[3] * (1.0 - a);
        };
        let blue = [0x66 as f64, 0x78 as f64, 0xff as f64];
        if round_rect(x, y, (14.0, 14.0, 104.0, 84.0, 16.0)) <= 0.0 {
            put(blue, 0.9);
        }
        let front = (40.0, 52.0, 106.0, 94.0, 16.0);
        if round_rect(x, y, front) <= 0.0 {
            put(blue, 1.0);
            if round_rect(x, y, (44.0, 56.0, 98.0, 86.0, 12.0)) <= 0.0 {
                put([0x1d as f64, 0x24 as f64, 0x33 as f64], 1.0);
            }
        }
        let prompt = [0x8f as f64, 0xb3 as f64, 0xff as f64];
        if segment(x, y, (60.0, 94.0), (76.0, 106.0)).min(segment(x, y, (76.0, 106.0), (60.0, 118.0))) <= 5.0 || round_rect(x, y, (86.0, 113.0, 34.0, 7.0, 3.5)) <= 0.0 {
            put(prompt, 1.0);
        }
        out
    };
    let scale = 160.0 / f64::from(size);
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for j in 0..size {
        for i in 0..size {
            let mut acc = [0.0; 4];
            for sj in 0..3 {
                for si in 0..3 {
                    let s = sample((f64::from(i) + (f64::from(si) + 0.5) / 3.0) * scale, (f64::from(j) + (f64::from(sj) + 0.5) / 3.0) * scale);
                    for k in 0..3 {
                        acc[k] += s[k];
                    }
                    acc[3] += s[3];
                }
            }
            let a = acc[3] / 9.0;
            for k in 0..3 {
                px.push(if acc[3] > 0.0 { (acc[k] / acc[3]).round() as u8 } else { 0 });
            }
            px.push((a * 255.0).round() as u8);
        }
    }
    px
}

/// What the reopen (a dock click with no preference handling it) does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reopen<T> {
    Focus(T),
    NewWindow,
    Nothing,
}

/// The reopen's target among `states` (id, Some(shown) for a hotkey window, on screen): the last
/// window comes forward unless it is a hidden hotkey window (the preference says "Don't do
/// anything special"); with no window on screen a new one opens.
pub fn reopen_target<T: Copy + PartialEq>(last: Option<T>, states: &[(T, Option<bool>, bool)]) -> Reopen<T> {
    if let Some(w) = last {
        if states.iter().any(|(id, h, _)| *id == w && *h != Some(false)) {
            return Reopen::Focus(w);
        }
    }
    if states.iter().all(|(_, _, on)| !on) {
        Reopen::NewWindow
    } else {
        Reopen::Nothing
    }
}

const PANEL_PREFIX: &str = "tray-panel:";

/// The tray menu entries for windows outside the terminal list (Settings, chat, panels):
/// (menu id, title) from (window id, title).
pub fn tray_panel_items(panels: &[(u64, String)]) -> Vec<(String, String)> {
    panels.iter().map(|(id, t)| (format!("{PANEL_PREFIX}{id}"), if t.is_empty() { "Termurion".to_string() } else { t.clone() })).collect()
}

/// The window id of a tray panel menu id.
pub fn tray_panel_pick(id: &str) -> Option<u64> {
    id.strip_prefix(PANEL_PREFIX)?.parse().ok()
}

/// What a click on the icon does (`HKC:362-402`, `dockIconClicked`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DockClick {
    /// Every open hotkey window hides.
    HideHotkeyWindows,
    /// These profiles' hotkey windows show.
    Show(Vec<String>),
    /// Nothing happened: the platform's default reopen runs (the last window comes forward,
    /// or a new one opens).
    Unhandled,
}

/// The dock click decision: a visible hotkey window hides them all; otherwise each profile
/// (guid, its "HotKey Window Dock Click Action") may show its window.
pub fn dock_click(visible_hotkey_windows: usize, profiles: &[(String, i64)], visible_terminal_windows: usize) -> DockClick {
    if visible_hotkey_windows > 0 {
        return DockClick::HideHotkeyWindows;
    }
    let show: Vec<String> = profiles.iter().filter(|(_, a)| *a == DOCK_ALWAYS_SHOW || (*a == DOCK_SHOW_IF_NONE && visible_terminal_windows == 0)).map(|(g, _)| g.clone()).collect();
    if show.is_empty() {
        DockClick::Unhandled
    } else {
        DockClick::Show(show)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AD:3756-3765` (`updateProcessType`).
    #[test]
    fn ui_element_follows_iterm2() {
        assert!(!is_ui_element(false, false, 2, 0));
        assert!(!is_ui_element(false, true, 2, 2));
        assert!(is_ui_element(true, false, 2, 0));
        assert!(!is_ui_element(true, true, 2, 1));
        assert!(is_ui_element(true, true, 2, 2));
        // `allWithBlock` is true on an empty list.
        assert!(is_ui_element(true, true, 0, 0));
    }

    /// `PT:1026-1047`, with pre-flight B5 and B6.
    /// The tray menu is owned by the current terminal window, else by the notification window (a
    /// top-level window since 96f2a4a), so it opens with no window made yet (UI element mode).
    #[test]
    fn tray_menu_owner_falls_back_to_the_notify_window() {
        assert_eq!(tray_menu_owner(Some(7), Some(9)), Some(7));
        assert_eq!(tray_menu_owner(None, Some(9)), Some(9));
        assert_eq!(tray_menu_owner(None, None), None);
        assert_eq!(tray_menu_owner(Some(0), Some(9)), Some(9));
    }

    #[test]
    fn window_look_follows_iterm2() {
        let l = |tool_window, skip_taskbar| Look { tool_window, skip_taskbar };
        assert_eq!(window_look(true, None, false), l(true, true));
        assert_eq!(window_look(false, None, false), l(false, false));
        // An open hotkey window: no taskbar button, in Alt+Tab.
        assert_eq!(window_look(false, Some(true), false), l(false, true));
        assert_eq!(window_look(false, Some(true), true), l(true, true));
        // A hidden one is cloaked and out of Alt+Tab already: its style never toggles (B5).
        assert_eq!(window_look(false, Some(false), false), l(false, true));
        assert_eq!(window_look(false, Some(false), true), l(false, true));
        assert_eq!(window_look(true, Some(true), false), l(true, true));
        assert_eq!(window_look(true, Some(false), false), l(true, true));
    }

    /// An Accessory window never gets its taskbar button back (B6).
    #[test]
    fn accessory_keeps_its_skip() {
        assert_eq!(with_accessory(window_look(false, None, false), true), Look { tool_window: false, skip_taskbar: true });
        assert_eq!(with_accessory(window_look(false, None, false), false), Look { tool_window: false, skip_taskbar: false });
    }

    /// Only a window with a taskbar button flips its style with a hide and show (B5).
    #[test]
    fn restyle_hides_only_a_regular_visible_window() {
        assert!(restyle_needs_reshow(false, true));
        assert!(!restyle_needs_reshow(true, true));
        assert!(!restyle_needs_reshow(false, false));
    }

    /// Next Window skips an open hotkey window under the exclusion (P6).
    #[test]
    fn cycling_skips_follow_the_setting() {
        assert!(cycling_skips(Some(false), false));
        assert!(!cycling_skips(Some(true), false));
        assert!(cycling_skips(Some(true), true));
        assert!(!cycling_skips(None, true));
    }

    /// `APV:251-262`.
    #[test]
    fn exclude_warning_follows_iterm2() {
        assert_eq!(EXCLUDE_WARNING, "When Termurion is excluded from the taskbar, you can always get back to Settings using the notification area icon. Look for a Termurion icon on the right side of your taskbar.");
        assert_eq!(EXCLUDE_ACTION, "Exclude From Taskbar and Alt+Tab Switcher");
        assert_eq!(CANCEL, "Cancel");
        assert_eq!(EXCLUDE_HEADING, "Warning");
        assert_eq!(UI_ELEMENT_KEY, "HideFromDockAndAppSwitcher");
        assert_eq!(REQUIRES_HOTKEYS_KEY, "UIElementRequiresHotkeys");
        assert_eq!(EXCLUDED_FROM_CYCLING_KEY, "HotkeyWindowsExcludedFromCycling");
    }

    /// `AD:1846-1880`.
    #[test]
    fn tray_items_follow_iterm2() {
        assert_eq!(
            TRAY_ITEMS,
            [("New Window", "newWindow:"), ("Settings", "showPrefWindow:"), ("Bring All Windows to Front", "arrangeInFront:"), ("Buried Sessions", "buried"), ("Check For Updates", "checkForUpdatesFromMenu:"), ("Main Menu", "main"), ("Quit Termurion", "terminate:")]
        );
        for (_, sel) in TRAY_ITEMS.iter().filter(|(_, s)| s.ends_with(':')) {
            assert!(crate::menus::items().iter().any(|i| i.selector == *sel), "{sel}");
        }
        assert_eq!(STATUS_BAR_ICON_KEY, "StatusBarIcon");
        assert_eq!(TRAY_TIP, "Termurion");
    }

    /// `HKC:362-402`.
    #[test]
    fn dock_click_follows_iterm2() {
        let p = |g: &str, a| (g.to_string(), a);
        assert_eq!(dock_click(1, &[p("a", 1)], 3), DockClick::HideHotkeyWindows);
        let three = [p("a", DOCK_DO_NOT_SHOW), p("b", DOCK_ALWAYS_SHOW), p("c", DOCK_SHOW_IF_NONE)];
        assert_eq!(dock_click(0, &three, 1), DockClick::Show(vec!["b".into()]));
        assert_eq!(dock_click(0, &three, 0), DockClick::Show(vec!["b".into(), "c".into()]));
        assert_eq!(dock_click(0, &[p("a", 0)], 0), DockClick::Unhandled);
        assert_eq!(dock_click(0, &[], 0), DockClick::Unhandled);
    }

    /// `APP:979-993`: the icon exists only in UI-element mode AND with `StatusBarIcon` on.
    #[test]
    fn tray_wanted_follows_iterm2() {
        assert!(tray_wanted(true, true));
        assert!(!tray_wanted(true, false));
        assert!(!tray_wanted(false, true));
        assert!(!tray_wanted(false, false));
    }

    /// A failed add leaves the icon off, so the next pass tries again (task 9 review 1).
    #[test]
    fn tray_retries_a_failed_add() {
        assert!(!tray_settle(true, false, false), "failed add stays off");
        assert!(tray_settle(true, false, true));
        assert!(tray_settle(false, true, false), "failed remove stays on");
        assert!(!tray_settle(false, true, true));
        // A pass changes something only while the wanted and actual states differ.
        assert!(tray_changes(true, false));
        assert!(!tray_changes(true, true));
    }

    /// TWEAKS 8: while resident the icon is tried at once, retried on a growing delay a bounded
    /// number of times, and a TaskbarCreated starts over.
    #[test]
    fn tray_retry_is_bounded_and_backs_off() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        // Nothing to do when the icon is as wanted.
        assert!(!tray_attempt_due(true, true, 0, None, t0));
        // A first attempt needs no wait.
        assert!(tray_attempt_due(true, false, 0, None, t0));
        // After a failure the attempt waits for its due time.
        let due = t0 + tray_retry_delay(1).unwrap();
        assert!(!tray_attempt_due(true, false, 1, Some(due), t0));
        assert!(tray_attempt_due(true, false, 1, Some(due), due));
        // The delay grows and is capped; the retries end.
        assert!(tray_retry_delay(2).unwrap() > tray_retry_delay(1).unwrap());
        assert_eq!(tray_retry_delay(TRAY_MAX_ATTEMPTS - 1), Some(Duration::from_secs(30)));
        // TWEAKS 8 review I3: never give up; past the backoff it stays at 30 s.
        assert_eq!(tray_retry_delay(TRAY_MAX_ATTEMPTS), Some(Duration::from_secs(30)));
        assert_eq!(tray_retry_delay(500), Some(Duration::from_secs(30)));
        assert_eq!(tray_retry_delay(0), None);
        assert!(tray_attempt_due(true, false, TRAY_MAX_ATTEMPTS + 40, None, t0), "still retrying while resident with no icon");
        assert!(!tray_attempt_due(true, false, TRAY_MAX_ATTEMPTS + 40, Some(t0 + Duration::from_secs(30)), t0));
        // The steady retry is logged once, on the pass that enters it.
        assert!(!tray_enters_steady_retry(TRAY_MAX_ATTEMPTS - 2, TRAY_MAX_ATTEMPTS - 1));
        assert!(tray_enters_steady_retry(TRAY_MAX_ATTEMPTS - 1, TRAY_MAX_ATTEMPTS));
        assert!(!tray_enters_steady_retry(TRAY_MAX_ATTEMPTS, TRAY_MAX_ATTEMPTS + 1));
        assert!(!tray_enters_steady_retry(TRAY_MAX_ATTEMPTS, 0));
        // A result counts or clears the failures.
        assert_eq!(tray_failures_after(0, false), 1);
        assert_eq!(tray_failures_after(5, true), 0);
    }

    /// TWEAKS 8 review I2: an add that failed or timed out may have succeeded, so the next try is a
    /// modify first, then an add.
    #[test]
    fn tray_add_tries_modify_first_when_the_add_is_unsure() {
        assert_eq!(tray_add_order(false, false), &[TrayOp::Add]);
        assert_eq!(tray_add_order(false, true), &[TrayOp::Modify, TrayOp::Add], "after a failed add");
        assert_eq!(tray_add_order(true, false), &[TrayOp::Modify, TrayOp::Add], "already added");
    }

    /// TWEAKS 8: the icon drawn for the tray is Termurion's (the logo), the right size, not blank.
    #[test]
    fn tray_icon_is_the_logo() {
        let px = tray_icon_rgba(32);
        assert_eq!(px.len(), 32 * 32 * 4);
        assert_eq!(px[3], 0, "the corner is clear");
        let opaque = px.chunks(4).filter(|p| p[3] == 255).count();
        assert!(opaque > 300, "the terminal card is drawn");
        let light = px.chunks(4).filter(|p| p[3] == 255 && p[0] > 0x80 && p[1] > 0xa0).count();
        assert!(light > 5, "the prompt is drawn");
    }

    /// TWEAKS 8: the menu opens with a way to a window, and ends with Quit.
    #[test]
    fn tray_menu_offers_new_window_and_quit() {
        assert_eq!(TRAY_ITEMS[0], ("New Window", "newWindow:"));
        assert_eq!(TRAY_ITEMS[TRAY_ITEMS.len() - 1], ("Quit Termurion", "terminate:"));
    }

    /// iTerm2's reopen (`AD:946-952`): the last window comes forward, never a hidden hotkey
    /// window ("Don't do anything special"); with nothing on screen a window opens.
    #[test]
    fn reopen_never_shows_a_hidden_hotkey_window() {
        // (id, Some(shown) for a hotkey window, on screen)
        let hidden_hotkey = [(1u32, Some(false), false)];
        assert_eq!(reopen_target(Some(1), &hidden_hotkey), Reopen::NewWindow);
        assert_eq!(reopen_target(None, &hidden_hotkey), Reopen::NewWindow);
        let with_regular = [(1u32, Some(false), false), (2, None, true)];
        assert_eq!(reopen_target(Some(1), &with_regular), Reopen::Nothing, "a window is already on screen");
        assert_eq!(reopen_target(Some(2), &with_regular), Reopen::Focus(2));
        let minimized = [(3u32, None, false)];
        assert_eq!(reopen_target(Some(3), &minimized), Reopen::Focus(3));
        assert_eq!(reopen_target(Some(9), &minimized), Reopen::NewWindow);
        let shown_hotkey = [(4u32, Some(true), true)];
        assert_eq!(reopen_target(Some(4), &shown_hotkey), Reopen::Focus(4));
    }

    /// Windows that left the taskbar (Settings, chat, panels) are in the tray menu too.
    #[test]
    fn tray_lists_panel_windows() {
        let items = tray_panel_items(&[(7, "Settings".to_string()), (9, "Chat".to_string())]);
        assert_eq!(items.len(), 2);
        assert_eq!(tray_panel_pick(&items[1].0), Some(9));
        assert_eq!(tray_panel_pick("window-3"), None);
        assert_eq!(tray_panel_pick("tray-panel:x"), None);
    }

    /// A style flip keeps the window's state (Task 8 review): minimized stays minimized,
    /// maximized stays maximized, the focused window keeps its activation.
    #[test]
    fn reshow_keeps_the_window_state() {
        assert_eq!(reshow_command(true, false, false), Reshow::MinimizedNoActivate);
        assert_eq!(reshow_command(true, true, true), Reshow::MinimizedNoActivate);
        assert_eq!(reshow_command(false, true, false), Reshow::Maximized);
        assert_eq!(reshow_command(false, true, true), Reshow::Maximized);
        assert_eq!(reshow_command(false, false, true), Reshow::Show);
        assert_eq!(reshow_command(false, false, false), Reshow::ShowNoActivate);
    }
}
