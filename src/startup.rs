//! What Tandem opens at launch: iTerm2's Startup settings.


/// The window restoration policy: iTerm2's tags 0 to 2.
pub const SYSTEM: i64 = 0;
pub const DEFAULT_ARRANGEMENT: i64 = 1;
pub const HOTKEY_ONLY: i64 = 2;

/// The settings page's note under Always open (GeneralPreferencesViewController).
pub const HOTKEY_NOTE: &str = "The existence of hotkey windows disables opening a window at startup.";

#[derive(Clone, Copy, Debug)]
pub struct StartupPrefs {
    /// OpenBookmark.
    pub open_profiles_window: bool,
    pub policy: i64,
    /// AlwaysOpenWindowAtStartup.
    pub always_open: bool,
    /// openNewWindowAtStartup (advanced).
    pub open_new_window: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Actions {
    pub profiles_window: bool,
    pub arrangement: bool,
    pub window: bool,
    pub restore: bool,
}

/// iTerm2's launch order: the Profiles window if set; the default arrangement for policy
/// 1; otherwise a window for policy 0 unless a hotkey window exists (Always open overrides
/// that). Nothing else: a Tandem with no window runs resident with its notification area icon (R3).
/// `plan` with saved windows (session restoration): policy 0 restores them, and they count
/// as the startup window.
pub fn plan_with(p: StartupPrefs, has_default_arrangement: bool, has_hotkey_windows: bool, has_saved_windows: bool) -> Actions {
    let base = plan(p, has_default_arrangement, has_hotkey_windows);
    let restore = p.policy == SYSTEM && has_saved_windows;
    Actions { restore, window: base.window && !restore, ..base }
}

pub fn plan(p: StartupPrefs, has_default_arrangement: bool, has_hotkey_windows: bool) -> Actions {
    let arrangement = p.policy == DEFAULT_ARRANGEMENT && has_default_arrangement;
    let iterm = !arrangement && (p.policy == SYSTEM || p.policy == DEFAULT_ARRANGEMENT) && p.open_new_window && (p.always_open || !has_hotkey_windows);
    Actions { profiles_window: p.open_profiles_window, arrangement, window: iterm, restore: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(policy: i64) -> StartupPrefs {
        StartupPrefs { open_profiles_window: false, policy, always_open: false, open_new_window: true }
    }

    #[test]
    fn policy_zero_restores() {
        let a = plan_with(prefs(SYSTEM), false, false, true);
        assert!(a.restore && !a.window, "restored windows count as the startup window");
        assert!(plan_with(prefs(SYSTEM), false, false, false).window, "nothing saved: a window");
        assert!(!plan_with(prefs(DEFAULT_ARRANGEMENT), true, false, true).restore);
        assert!(!plan_with(prefs(HOTKEY_ONLY), false, false, true).restore);
    }

    #[test]
    fn startup_follows_iterm2() {
        assert_eq!(plan(prefs(SYSTEM), false, false), Actions { profiles_window: false, arrangement: false, window: true, restore: false });
        assert_eq!(plan(prefs(SYSTEM), false, true), Actions { profiles_window: false, arrangement: false, window: false, restore: false }, "a hotkey window stops the startup window");
        assert!(plan(StartupPrefs { always_open: true, ..prefs(SYSTEM) }, false, true).window, "Always open at least one terminal window");
        assert_eq!(plan(prefs(DEFAULT_ARRANGEMENT), true, false), Actions { profiles_window: false, arrangement: true, window: false, restore: false });
        assert!(plan(prefs(DEFAULT_ARRANGEMENT), false, false).window, "no default arrangement: a window");
        assert!(!plan(prefs(HOTKEY_ONLY), false, true).window, "Only Restore Hotkey Window");
        // R3 (13v1 Task 7): the Windows-only "a window opens when nothing else would show" rule is gone;
        // Tandem starts resident with its icon instead, as iTerm2 runs with no window.
        assert!(!plan(prefs(HOTKEY_ONLY), false, false).window, "R3: Only Restore Hotkey Window with no hotkey window opens nothing");
        assert!(plan(StartupPrefs { open_profiles_window: true, ..prefs(SYSTEM) }, false, false).profiles_window);
        assert!(!plan(StartupPrefs { open_new_window: false, ..prefs(SYSTEM) }, false, false).window, "R3: openNewWindowAtStartup off opens nothing, as iTerm2");
    }
}
