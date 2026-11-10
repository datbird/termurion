//! Staying resident with no window (m13v1 Task 7): what iTerm2 decides when its last window
//! closes (`applicationShouldTerminateAfterLastWindowClosed:`, `AD:915-942`) and when the icon is
//! clicked with no window (`applicationShouldHandleReopen:`, `applicationOpenUntitledFile:`,
//! `AD:944-951`, `AD:1132-1153`), what a Windows shutdown does (`AD:993-1036`, Task 8) and which
//! folder downloads go to. Pure; the window work is `app::resident_ui`.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// What closing the last window does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterLast {
    Quit,
    StayResident,
}

/// `AD:915-942`: never quit while restoring or while any terminal (hotkey windows included)
/// exists; with no interaction yet and less than `min_running` seconds since launch, do not quit;
/// otherwise quit exactly when `QuitWhenAllWindowsClosed` is on.
pub fn after_last_window(restoring: bool, terminals_left: usize, interacted: bool, ran_for: Duration, min_running: f64, quit_pref: bool) -> AfterLast {
    if restoring || terminals_left > 0 {
        return AfterLast::StayResident;
    }
    if !interacted && ran_for.as_secs_f64() < min_running {
        return AfterLast::StayResident;
    }
    if quit_pref {
        AfterLast::Quit
    } else {
        AfterLast::StayResident
    }
}

/// Where the resident icon stands. Windows and macOS always say `Up`: their rules never change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// Added, and the panel took it.
    Up,
    /// Being added now.
    Adding,
    /// A panel is there, but no icon is up or on its way (none wanted yet, or a failed add waits).
    Idle,
    /// No panel shows tray icons (Linux: nothing owns StatusNotifierWatcher).
    Absent,
}

/// THE rule for a Termurion with no window: it may be left so only while its icon can bring it
/// back (up, on its way, or a panel there to take it), or a hotkey window can still be called.
pub fn may_be_windowless(icon: Icon, hotkey_windows: usize) -> bool {
    hotkey_windows > 0 || icon != Icon::Absent
}

/// The resident window is the last window hidden; where nothing could bring it back and no
/// terminal is left, Termurion quits instead.
pub fn after_last_window_where(may_be_windowless: bool, decision: AfterLast) -> AfterLast {
    if may_be_windowless { decision } else { AfterLast::Quit }
}

/// Whether a windowless Termurion can no longer be reached: no icon up or being added, no hotkey
/// window, no panel open and no window on its way.
pub fn stranded(windowless: bool, icon: Icon, hotkey_windows: usize, panels_open: bool, window_coming: bool) -> bool {
    windowless && !matches!(icon, Icon::Up | Icon::Adding) && hotkey_windows == 0 && !panels_open && !window_coming
}

/// What a stranded Termurion does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rescue {
    /// Tabs already wait in the resident slot: its window shows them.
    WakeWithTabs,
    /// The newest buried session comes back in a window (as when the last tab closes).
    Disinter,
    /// No session is left to keep: it quits.
    Quit,
}

pub fn rescue(tabs_waiting: bool, buried: bool) -> Rescue {
    if tabs_waiting {
        Rescue::WakeWithTabs
    } else if buried {
        Rescue::Disinter
    } else {
        Rescue::Quit
    }
}

/// How the resident window leaves the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Away {
    /// Hidden, kept with its GPU (Windows, macOS, X11).
    Hide,
    /// Destroyed (Wayland has no hide), then made again unmapped on its kept device, so the
    /// resident slot has a window as on the other systems.
    Destroy,
}

pub fn resident_away(can_hide: bool) -> Away {
    if can_hide { Away::Hide } else { Away::Destroy }
}

/// Whether the notification area icon is there: 13g's rule, or Termurion is resident (otherwise a
/// Termurion with no window could not be reached).
pub fn icon_wanted(tray_13g: bool, resident: bool) -> bool {
    tray_13g || resident
}

/// Whether the app keeps its Dock icon (macOS: Regular policy). It leaves the Dock when the menu
/// bar icon is all that is left (no terminal window, no panel such as Settings), and always when
/// hidden from the Dock.
pub fn dock_icon_wanted(ui_element_now: bool, resident: bool, panels_open: bool) -> bool {
    !ui_element_now && (!resident || panels_open)
}

/// `AD:1132-1153`: a click on the icon (or a second bare start) opens a window only when none
/// is on screen and `OpenUntitledFile` is on.
pub fn reopen_opens_window(open_untitled: bool, visible_windows: usize) -> bool {
    open_untitled && visible_windows == 0
}

/// What the log says when the grace period kept Termurion running.
pub fn grace_note() -> &'static str {
    "Not quitting Termurion because it ran very briefly and had no user interaction. Set the MinRunningTime float preference to 0 to turn this feature off."
}

/// What a Windows shutdown does about the quit question (`AD:1010-1036`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownAnswer {
    EndNow,
    AskFirst,
}

/// The reason Windows shows while the quit question is open.
pub const BLOCK_REASON: &str = "Termurion is asking whether to quit.";

/// `AD:1010-1036`: with `NeverBlockSystemShutdown` on no reason is collected, so nothing asks;
/// otherwise a shutdown asks exactly when a quit would (R3: Termurion used to end without asking).
pub fn on_shutdown(never_block: bool, has_reasons: bool) -> ShutdownAnswer {
    if !never_block && has_reasons {
        ShutdownAnswer::AskFirst
    } else {
        ShutdownAnswer::EndNow
    }
}

/// `AD:1010`: whether the quit reasons are collected at all.
pub fn reasons_collected(shutting_down: bool, never_block: bool) -> bool {
    !(shutting_down && never_block)
}

/// `NSFileManager+iTerm.m:297-313`: the override when it is set and writable, else the system
/// Downloads folder when that is writable, else None (the caller uses the profile folder).
pub fn downloads_dir(override_path: &str, known: Option<PathBuf>, writable: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    if !override_path.is_empty() && writable(Path::new(override_path)) {
        return Some(PathBuf::from(override_path));
    }
    known.filter(|k| writable(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: fn(u64) -> Duration = Duration::from_secs;

    #[test]
    fn quit_decision_follows_iterm2() {
        use AfterLast::*;
        assert_eq!(after_last_window(true, 0, true, S(60), 10.0, true), StayResident, "restoring");
        assert_eq!(after_last_window(false, 1, true, S(60), 10.0, true), StayResident, "a hotkey window is left");
        assert_eq!(after_last_window(false, 0, false, S(3), 10.0, true), StayResident, "the grace period");
        assert_eq!(after_last_window(false, 0, false, S(11), 10.0, true), Quit, "past the grace period");
        assert_eq!(after_last_window(false, 0, true, S(3), 10.0, true), Quit, "the person used it");
        assert_eq!(after_last_window(false, 0, false, S(3), 0.0, true), Quit, "no grace period");
        assert_eq!(after_last_window(false, 0, true, S(60), 10.0, false), StayResident, "the key off (R3)");
        assert_eq!(after_last_window(false, 0, false, S(60), 10.0, false), StayResident, "the key off, no interaction");
        assert!(icon_wanted(false, true) && icon_wanted(true, false) && icon_wanted(true, true));
        assert!(!icon_wanted(false, false));
        assert!(grace_note().starts_with("Not quitting Termurion because it ran very briefly"));
    }

    #[test]
    fn a_windowless_termurion_needs_a_way_back() {
        use AfterLast::*;
        use Icon::*;
        for icon in [Up, Adding, Idle] {
            assert!(may_be_windowless(icon, 0), "{icon:?}: the icon is up or can be");
        }
        assert!(!may_be_windowless(Absent, 0), "no panel, X11 or Wayland: nothing to wake it from");
        assert!(may_be_windowless(Absent, 1), "a hotkey window is left");
        assert_eq!(after_last_window_where(true, StayResident), StayResident, "with a tray: resident, as iTerm2");
        assert_eq!(after_last_window_where(true, Quit), Quit, "the quit setting still quits");
        assert_eq!(after_last_window_where(false, StayResident), Quit);
        assert_eq!(resident_away(true), Away::Hide);
        assert_eq!(resident_away(false), Away::Destroy, "Wayland cannot hide it");
    }

    /// Review C1/I1/I2: a windowless Termurion whose icon failed or went away makes a window again
    /// (or quits with no session left); one with a way back is left alone.
    #[test]
    fn a_stranded_termurion_comes_back_or_quits() {
        use Icon::*;
        assert!(!stranded(false, Absent, 0, false, false), "a window is there");
        assert!(!stranded(true, Up, 0, false, false));
        assert!(!stranded(true, Adding, 0, false, false), "the add is on its way");
        assert!(stranded(true, Idle, 0, false, false), "the add failed and waits for its retry");
        assert!(stranded(true, Absent, 0, false, false), "the watcher left");
        assert!(!stranded(true, Absent, 1, false, false), "a hotkey window can be called");
        assert!(!stranded(true, Absent, 0, true, false), "Settings is open");
        assert!(!stranded(true, Absent, 0, false, true), "a window is on its way");
        assert_eq!(rescue(true, true), Rescue::WakeWithTabs);
        assert_eq!(rescue(false, true), Rescue::Disinter);
        assert_eq!(rescue(false, false), Rescue::Quit);
    }

    #[test]
    fn reopen_follows_iterm2() {
        assert!(reopen_opens_window(true, 0));
        assert!(!reopen_opens_window(false, 0), "OpenUntitledFile off");
        assert!(!reopen_opens_window(true, 1), "a window is on screen");
    }

    #[test]
    fn dock_icon_follows_windows_and_hide_setting() {
        assert!(!dock_icon_wanted(false, true, false), "no windows, menu bar icon only: out of the Dock");
        assert!(dock_icon_wanted(false, false, false), "a window is open");
        assert!(dock_icon_wanted(false, true, true), "a panel (Settings) is open with no terminal");
        assert!(!dock_icon_wanted(true, false, false), "hidden from the Dock stays hidden");
        assert!(!dock_icon_wanted(true, true, true));
        assert!(!dock_icon_wanted(true, false, true));
    }

    #[test]
    fn shutdown_follows_iterm2() {
        use ShutdownAnswer::*;
        assert_eq!(on_shutdown(false, true), AskFirst, "a shutdown asks when a quit would");
        assert_eq!(on_shutdown(false, false), EndNow, "nothing to ask");
        assert_eq!(on_shutdown(true, true), EndNow, "NeverBlockSystemShutdown");
        assert_eq!(on_shutdown(true, false), EndNow);
        assert_eq!(BLOCK_REASON, "Termurion is asking whether to quit.");
        assert!(!reasons_collected(true, true), "AD:1010: no reason is collected");
        assert!(reasons_collected(true, false) && reasons_collected(false, true) && reasons_collected(false, false));
    }

    #[test]
    fn downloads_follow_iterm2() {
        use std::path::{Path, PathBuf};
        let known = || Some(PathBuf::from("C:\\U\\Downloads"));
        let all = |_: &Path| true;
        assert_eq!(downloads_dir("D:\\dl", known(), &all), Some(PathBuf::from("D:\\dl")));
        let only_known = |p: &Path| p == Path::new("C:\\U\\Downloads");
        assert_eq!(downloads_dir("D:\\dl", known(), &only_known), known(), "an override that is not writable falls back");
        assert_eq!(downloads_dir("", known(), &all), known(), "an empty override");
        assert_eq!(downloads_dir("", known(), &|_: &Path| false), None, "a known folder that is not writable (the caller uses the profile folder)");
        assert_eq!(downloads_dir("", None, &all), None, "no known folder");
    }
}
