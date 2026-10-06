//! Staying resident with no window (m13v1 Task 7): what iTerm2 decides when its last window
//! closes (`applicationShouldTerminateAfterLastWindowClosed:`, `AD:915-942`) and when the icon is
//! clicked with no window (`applicationShouldHandleReopen:`, `applicationOpenUntitledFile:`,
//! `AD:944-951`, `AD:1132-1153`). Pure; the window work is `app::resident_ui`.

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

/// Whether the notification area icon is there: 13g's rule, or Tandem is resident (otherwise a
/// Tandem with no window could not be reached).
pub fn icon_wanted(tray_13g: bool, resident: bool) -> bool {
    tray_13g || resident
}

/// `AD:1132-1153`: a click on the icon (or a second bare start) opens a window only when none
/// is on screen and `OpenUntitledFile` is on.
pub fn reopen_opens_window(open_untitled: bool, visible_windows: usize) -> bool {
    open_untitled && visible_windows == 0
}

/// What the log says when the grace period kept Tandem running.
pub fn grace_note() -> &'static str {
    "Not quitting Tandem because it ran very briefly and had no user interaction. Set the MinRunningTime float preference to 0 to turn this feature off."
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
        assert!(grace_note().starts_with("Not quitting Tandem because it ran very briefly"));
    }

    #[test]
    fn reopen_follows_iterm2() {
        assert!(reopen_opens_window(true, 0));
        assert!(!reopen_opens_window(false, 0), "OpenUntitledFile off");
        assert!(!reopen_opens_window(true, 1), "a window is on screen");
    }
}
