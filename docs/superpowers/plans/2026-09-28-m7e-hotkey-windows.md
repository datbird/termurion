# Tandem Milestone 7e: Hotkey Windows

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's hotkey windows: a profile's system-wide hotkey (a chord, a modifier double tap, or additional chords) shows and hides a dedicated window of that profile from any app, with iTerm2's toggle rules, siblings, slide animation for edge styles, auto-hide on focus loss, Pin Hotkey Window, reopen on reactivation, floating, focus return to the app that was in front, the Configure Hotkey Window sheet, Create a Dedicated Hotkey Window..., the app hotkey (Show/hide all windows), the "Only Restore Hotkey Window" startup policy, and hotkey windows restored hidden at launch.

**Architecture:** Pure models decide the rules: `hotkey.rs` (the profile's hotkey settings, Windows chords from iTerm2's stored key codes and flags, siblings and conflicts, the toggle decision, the dedicated profile clone), `doubletap.rs` (iTerm2's double-tap state machine fed by the low-level hook, with Windows' repeat and AltGr filtering), and `hotkey_motion.rs` (hidden origin per style, which styles slide, the ease-in curve, positions over time). `winkeys.rs` registers chords with `RegisterHotKey` on its hook thread and falls back to the hook for chords the shell owns and for double taps; presses reach the app as a user event. The window code keeps a hotkey window's state in its `WinSlot` (swapped in `swap_live`), hides it by DWM cloaking, slides it with timed `SetWindowPos`, and returns focus to the saved foreground window.

**Tech Stack:** as milestone 7d; `windows-sys` `RegisterHotKey`, `DwmSetWindowAttribute` (`Win32_Graphics_Dwm`), `SetWindowPos`, `GetForegroundWindow`/`SetForegroundWindow`.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m7e-hotkey-facts.md` (all sections) and `m7-iterm2-facts.md` section 3. Rows: `01-profile-settings.md` `:292-301`; `02-global-settings.md` `:307-308` and the Hotkey advanced rows; `03-features-and-menus.md` hotkey rows and Pin Hotkey Window.

## Global Constraints

- Everything from milestones 0 to 7d still holds.
- Stored keys are iTerm2's (research section 10): the profile's `Has Hotkey`, `HotKey Key Code` (macOS key code), `HotKey Modifier Flags` (NSEvent flags), `HotKey Characters`, `HotKey Characters Ignoring Modifiers`, `HotKey Activated By Modifier`, `HotKey Modifier Activation` (0 Control, 1 Shift, 2 Option, 3 Command), `HotKey Alternate Shortcuts`, `HotKey Window AutoHides` (on), `HotKey Window Reopens On Activation` (off), `HotKey Window Animates` (on), `HotKey Window Floats` (on), `HotKey Window Dock Click Action` (0); globals `Hotkey`, `HotkeyCode`, `HotkeyChar`, `HotkeyModifiers`. Command maps to Win, Option to Alt.
- Texts are iTerm2's (research section 1) with "iTerm2" read as "Tandem", Cmd as the Win key; the sheet's title is "HotKey Window Preferences".
- Toggle rule (research 3.2 to 3.5): siblings (the same chord or the same double-tap key) toggle together; a press hides when all siblings are open and one of them is focused (or it cannot switch to a pinned unfocused one), shows when any is hidden; a pinned window that is open but not focused is focused instead.
- Animation: 0.25 s (`hotkeyTermAnimationDuration`), ease-in cubic-bezier(0.42, 0, 1, 1); top, bottom, and right edge styles slide from outside their edge; every other style (left included, as in iTerm2) shows and hides at once (Tandem's stand-in for iTerm2's fade); a slide whose hidden frame would lie on another monitor shows at once.
- Auto-hide: focus moving to another app or to a Tandem terminal window that is not a sibling hides an auto-hiding hotkey window; Settings, the Profiles window, and Tandem's dialogs do not.
- A created hotkey window (even hidden) keeps Tandem running when the last other window closes.
- Double tap: each of the three gaps between 0.01 s and 0.3 s (`hotKeyDoubleTapMinDelay`, `hotKeyDoubleTapMaxDelay`); any other key or modifier resets.

## Review Focus

1. Two profiles bound to the same chord, one window shown and focused, the other hidden. Expect: a press shows the hidden one (not all open), a second press hides both. Test: Task 1 `toggle_follows_iterm2`.
2. Holding Ctrl (Windows repeats the key-down) and AltGr (a fake left Ctrl then right Alt) with a Control double tap set. Expect: neither fires; two real Ctrl taps do. Test: Task 2 `repeats_and_altgr_never_fire`.
3. A top-edge hotkey window on the second monitor whose hidden frame would overlap the first monitor. Expect: it shows at once, no slide across the other monitor. Test: Task 3 `slides_stay_on_their_monitor`.
4. The hotkey chord is Win+letter owned by the shell, or a chord another app registered. Expect: the shell chord is taken through the hook; the other app's chord shows "Another app uses this shortcut" in the sheet. Test: Task 1 `chords_map_to_windows_keys` (the mapping) plus the checklist.
5. The last regular window closes while a hidden hotkey window exists. Expect: Tandem keeps running; the hotkey shows the window. Test: Task 5 `a_hotkey_window_keeps_tandem_running` (the exit rule as a pure function).

---

### Task 1: The hotkey model

**Files:** Create `src/hotkey.rs`; modify `src/lib.rs`, `schema/profile.toml` (`HotKey Modifier Activation` is not macOS-only).

**Interfaces:** `Prefs::from_profile(&Profile) -> Prefs { primary: Option<Keystroke>, double_tap: Option<u8>, alternates: Vec<Keystroke>, auto_hides, reopens, animates, floats }` and `has_hotkey()`; `chord(&Keystroke) -> Option<Chord { vk: u32, mods: u32 }>`; `Group` siblings by chord and by double-tap key; `conflicts(guid, prefs, others) -> (bool chord, bool double_tap)`; `Win { open: bool, focused: bool, pinned: bool }` and `press(group: &[Win]) -> Vec<Act { Show, Hide, Focus, Nothing }>`; `dedicated_profile(default: &Profile, names: &[String], prefs) -> Profile` and `dedicated_name(names)`; `create_warning(names) -> (title, text, buttons)`.

- [ ] Failing tests `has_hotkey_counts_every_key`, `chords_map_to_windows_keys`, `siblings_share_a_chord`, `toggle_follows_iterm2`, `dedicated_profile_clone`, `create_warning_text`. Commit `Hotkey model`.

### Task 2: Double tap

**Files:** Create `src/doubletap.rs`; modify `src/lib.rs`.

**Interfaces:** `Machine::new(modifier, min, max)`, `Machine::event(Ev { vk, down, time, scan }) -> bool` (true on activation); the held set, repeat filtering, left/right folding, non-modifier reset, AltGr's fake Control ignored.

- [ ] Failing tests `double_tap_fires_on_second_release`, `slow_or_mixed_taps_reset`, `repeats_and_altgr_never_fire`. Commit `Double tap`.

### Task 3: Motion

**Files:** Create `src/hotkey_motion.rs`; modify `src/lib.rs`.

**Interfaces:** `slides(style) -> bool`; `hidden_origin(style, shown: Rect, work: Rect) -> (f32, f32)`; `slide_allowed(hidden: Rect, own: Rect, monitors: &[Rect]) -> bool`; `ease_in(t) -> f32` (cubic-bezier 0.42, 0, 1, 1); `Motion { from, to, start, duration }` with `at(now) -> ((f32, f32), bool done)`.

- [ ] Failing tests `hidden_origins_follow_iterm2`, `slides_stay_on_their_monitor`, `ease_in_matches_the_curve`, `motion_ends_at_the_target`. Commit `Hotkey window motion`.

### Task 4: Registration

**Files:** modify `src/winkeys.rs` (register and unregister on the hook thread through thread messages; WM_HOTKEY forwarded; a hook gate for shell-owned Win chords and for double taps, before `remap`, swallowing the key-down and its key-up, masking the Start menu; recording sends the chord to Settings instead), `src/app/menu.rs` (the user events), `src/app/mod.rs`.

**Interfaces:** `winkeys::set_hotkeys(Vec<Binding { id, chord, via_hook }>, taps: Vec<(id, modifier)>) -> Vec<(id, Result<(), String>)>`; `UserEvent::Hotkey(id)`.

- [ ] Failing test `registration_plan_splits_hook_and_system` (pure: which chords go to RegisterHotKey, which to the hook). Commit `Register hotkeys`.

### Task 5: Hotkey windows

**Files:** Create `src/app/hotkey_ui.rs`; modify `src/app/windows_ui.rs` (`NewWindow::Hotkey(guid)`, hidden creation, skip taskbar, the per-window hotkey state in `WinSlot` and `swap_live`), `src/app/mod.rs` (press handling, auto-hide in `Focused`, reopen on activation, the motion timer in `about_to_wait`, the last-window rule), `src/app/menu.rs` (Window > Pin Hotkey Window, Shift+Cmd+-), `src/app/actions.rs` (action 43), `src/app/restore_ui.rs` and `src/app/arrange_ui.rs` ("Is Hotkey Window", "Hotkey Profile GUID"; restored hidden), `src/startup.rs` use (`has_hotkey_windows`).

**Interfaces:** `App::hotkey_pressed(id)`, `roll_in(window)`, `roll_out(window, by_key)`, `auto_hide(new_foreground)`; `hotkey::keeps_running(regular_windows, hotkey_windows) -> bool`.

- [ ] Failing test `a_hotkey_window_keeps_tandem_running`. Commit `Hotkey windows`.

### Task 6: Settings

**Files:** Create `src/settings_hotkey.rs` (the Configure Hotkey Window sheet with every control and note, the Additional Hotkeys list, the recorder with "Click to Set", "Recording", the eraser, and "Confirm Shortcut"); modify `src/settings_ui.rs` (Profiles > Keys hotkey row; Keys > Hotkey tab: Create a Dedicated Hotkey Window..., App Hotkey), `schema/global.toml` (`Hotkey`, `HotkeyCode`, `HotkeyChar`, `HotkeyModifiers`, `hotkeyTermAnimationDuration`, `hotKeyDoubleTapMaxDelay`, `hotKeyDoubleTapMinDelay`, `showPinnedIndicator`, `hotkeyWindowsExcludedFromCycling`).

**Interfaces:** `hotkey::smells_accidental(&Keystroke) -> bool`; the sheet writes the profile keys on OK only.

- [ ] Failing test `accidental_keystrokes_ask`. Commit `Hotkey settings`.

### Task 7: Show/hide all windows

**Files:** modify `src/app/hotkey_ui.rs` (the app hotkey), `src/hotkey.rs` (`app_press(active, windows) -> AppAct { HideAll, ShowAll, OpenWindow }`).

- [ ] Failing test `app_hotkey_follows_iterm2`. Commit `Show and hide all windows`.

### Task 8: Parity and acceptance

- [ ] Update rows, recount, write `docs/acceptance/m7e-checklist.md`. Commit.
