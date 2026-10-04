//! Leaving the taskbar and Alt+Tab (13g): when Tandem acts as iTerm2's UI element, and what a
//! window's styles are. Pure; `app::presence_ui` applies it.

pub const UI_ELEMENT_KEY: &str = "HideFromDockAndAppSwitcher";
pub const REQUIRES_HOTKEYS_KEY: &str = "UIElementRequiresHotkeys";
pub const EXCLUDED_FROM_CYCLING_KEY: &str = "HotkeyWindowsExcludedFromCycling";

/// The persistent warning before the setting turns on (`APV:251-262`, Mac words replaced).
pub const EXCLUDE_HEADING: &str = "Warning";
pub const EXCLUDE_WARNING: &str = "When Tandem is excluded from the taskbar, you can always get back to Settings using the notification area icon. Look for a Tandem icon on the right side of your taskbar.";
pub const EXCLUDE_ACTION: &str = "Exclude From Taskbar and Alt+Tab Switcher";
pub const CANCEL: &str = "Cancel";

/// Whether Tandem is a UI element now (`AD:3756-3765`): the setting, narrowed by the second
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
pub const TRAY_TIP: &str = "Tandem";

/// The icon's menu (`AD:1846-1880`): label, and the selector of the menu item it runs or the
/// kind of submenu ("buried", "main").
pub const TRAY_ITEMS: [(&str, &str); 6] = [
    ("Settings", "showPrefWindow:"),
    ("Bring All Windows to Front", "arrangeInFront:"),
    ("Buried Sessions", "buried"),
    ("Check For Updates", "checkForUpdatesFromMenu:"),
    ("Main Menu", "main"),
    ("Quit Tandem", "terminate:"),
];

/// "HotKey Window Dock Click Action" values.
pub const DOCK_DO_NOT_SHOW: i64 = 0;
pub const DOCK_ALWAYS_SHOW: i64 = 1;
pub const DOCK_SHOW_IF_NONE: i64 = 2;

/// Whether the notification area icon is there: always while Tandem is a UI element, whatever
/// the Advanced setting says. iTerm2 gates it on `StatusBarIcon` (`APP:979-993`), but Tandem's
/// windows leave the taskbar and Alt+Tab too, so the icon is the only way back (coordinator
/// ruling).
pub fn tray_wanted(ui_element_now: bool, _status_bar_icon: bool) -> bool {
    ui_element_now
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
        assert_eq!(EXCLUDE_WARNING, "When Tandem is excluded from the taskbar, you can always get back to Settings using the notification area icon. Look for a Tandem icon on the right side of your taskbar.");
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
            [("Settings", "showPrefWindow:"), ("Bring All Windows to Front", "arrangeInFront:"), ("Buried Sessions", "buried"), ("Check For Updates", "checkForUpdatesFromMenu:"), ("Main Menu", "main"), ("Quit Tandem", "terminate:")]
        );
        for (_, sel) in TRAY_ITEMS.iter().filter(|(_, s)| s.ends_with(':')) {
            assert!(crate::menus::items().iter().any(|i| i.selector == *sel), "{sel}");
        }
        assert_eq!(STATUS_BAR_ICON_KEY, "StatusBarIcon");
        assert_eq!(TRAY_TIP, "Tandem");
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

    /// `APP:979-993`.
    #[test]
    fn tray_wanted_follows_iterm2() {
        assert!(tray_wanted(true, true));
        // The icon is the only way back, so the Advanced setting cannot remove it.
        assert!(tray_wanted(true, false));
        assert!(!tray_wanted(false, true));
        assert!(!tray_wanted(false, false));
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
