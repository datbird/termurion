# Tandem Milestone 13u: Keys, hotkey windows, tmux leftovers

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Scope (read first):** research `.superpowers/research/m13-facts.md` section 0 (the `keyaction.rs:379` stub bullet and the `03:417` stale note), section 4 item 6 (the tmux Custom title prefix; the facts file assigns it to 13y, the controller moved it here), section 5 (the 13u list), section 6 R3 and R7, and the 25 Appendix A rows whose Part is `13u`; `.superpowers/research/m13-rulings.md` (R2, R3, the parity-corrections line); `docs/superpowers/rulings/m11c-rulings.txt:157`; and the Resume notes below, and nothing else: DECSET 1036 and the Left/Right Option Key Changeable switches; per-side Control and Win remapping to Hyper, Meta and Super in the profile; Remap modifiers globally turned off; the forced keyboard layout (Force keyboard, once per session, the layout); Respect system shortcuts; the Undo key action (44); the pinned hotkey window indicator; session hotkeys and tmux's `@hotkeys`; the fading hotkey window; tmux's two attach popups, filler area, background opening, and the tmux title prefix on custom session titles. Move to Split Pane (62) and the Move Pane pointer action belong to 13h; Next/Previous Window and New Window With Profile pointer actions (`src/pointer.rs:61`) are pointer rows `02:477`, `02:478`, `02:487` in 13j.

**Goal:** a person gets iTerm2's keyboard behavior in every corner the Keys tab and the Advanced pane reach: programs that ask for Alt to send Esc get it when allowed, a Control or Win key can report as Hyper, Meta or Super to kitty-protocol apps, remapping can stay inside Tandem so Windows' own shortcuts see the real keys, Tandem can force a keyboard layout, Windows' own shortcuts win over key mappings by default, Undo does what Edit > Undo does, a session's own hotkey brings it forward (and tmux remembers it), hotkey windows fade in and out, and tmux windows open, fill and title themselves as in iTerm2.

**Architecture:** pure models first, each a plain `pub mod` tested on the Windows test machine. One vendored alacritty patch records DECSET 1036 (set, reset, report). `keys.rs`'s `KeyOptions` answers the effective Alt behavior with 1036; `keyproto.rs` cooks modifiers for the kitty protocol with the profile's bucky bits; `remap.rs` gains the in-app remap used when Remap modifiers globally is off; `input_locale.rs` is the forced-layout model with a `#[cfg(windows)]` Win32 side (`GetKeyboardLayoutList`, `ActivateKeyboardLayout`); `keyroute.rs`'s Win-chord gate respects Windows' shell chords; `session_hotkeys.rs` groups session hotkeys and encodes tmux's `@hotkeys`; `hotkey_motion.rs` gains the fade. The app side: `src/app/keys_ui.rs` (1036, bucky bits, forced layout, symbolic shortcuts), `src/app/session_hotkey_ui.rs`, `hotkey_ui.rs` (fade, pinned), `tmux_tabs_ui.rs`/`tmux_ui.rs` (filler, background, title prefix). Per-pane state (the forced-once flag) lives on the `Pane`; hotkey registrations and the fade are app-wide; the forced layout is app-wide; no new field is per window, so none goes in `swap_live`.

**Tech Stack:** as milestones 0 to 12 (winit 0.30, wgpu 30, egui 0.36, muda). No new crates: `windows-sys` `Win32_UI_Input_KeyboardAndMouse` (`GetKeyboardLayoutList`, `ActivateKeyboardLayout`, `GetKeyboardLayout`, `ToUnicodeEx`), `Win32_Globalization` (`LCIDToLocaleName`, `GetLocaleInfoEx`), `Win32_UI_WindowsAndMessaging` (`SetLayeredWindowAttributes`, `GetWindowLongPtrW`, `SetWindowLongPtrW`), all already enabled, and `Win32_System_Registry` (`RegOpenKeyExW`, `RegGetValueW` for the layout names; approved in the 13g rulings, added to `Cargo.toml` by whichever of 13g and 13u lands first).

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. iTerm2 facts: `.superpowers/research/m13-facts.md` Appendix A 13u rows and the Resume notes below (the source wins where they differ; see Corrections). Rows: `01-profile-settings.md` `01:41`, `01:276`, `01:277`, `01:283` to `01:286`, `01:297`; `02-global-settings.md` `02:163`, `02:164`, `02:312`, `02:329`, `02:337` to `02:340`, `02:406`, `02:698`, `02:944`, `02:953`, `02:1098` (`02:1116` is milestone 14e's, 13s-u rulings); `03-features-and-menus.md` `03:417`, `03:430`, `03:668`; rows found by title.

## Global Constraints

- Everything from milestones 0 to 12 still holds. Read `.superpowers/sdd-templates/implementer-rules.md` first; its invariants bind every task (per-window App fields go in `swap_live`; tmux panes keep tmux's size, so anything that takes room is subtracted before the size report). Precedents: `docs/superpowers/rulings/` (texts with a runtime placeholder are `pub fn`, others `pub const`).
- Ruling R3 binds: every default mismatch is fixed to iTerm2's default, each as its own step with its own test, and recorded in `m13u-rulings.txt`. In this plan: `02:340` (Windows' own shortcuts win over key mappings, default on, Task 6 Step 3a, under ruling R7 below). `02:1116` is not in this plan (milestone 14e owns it).
- Ruling R2 binds: a key iTerm2 defines but never reads is stored and shown, doing nothing. Every key in this plan is read in iTerm2 at `64791fc` (Resume notes "Every key is read").
- Ruling R7 (13s-u rulings, "Respect system shortcuts"): `AllowSymbolicHotKeys` on (the default, R3) means a Win chord that the Windows shell keeps for itself (`winkeys::shell_owned`) goes to Windows unless a Tandem menu command owns it; off, key mappings take it too. A profile Shortcut (13t Task 8, a Profiles menu item) counts as a Tandem menu command.
- Key names (13a ruling, R13xy-2): new Advanced keys use iTerm2's defaults key, the DEFINE name with its first letter capitalized: `ShowPinnedIndicator`, `UseBlackFillerColorForTmuxInFullScreen`, `TmuxWindowsOpenInBackground`, `SupportDecsetMetaSendsEscape` (none exists in `schema/global.toml` yet). Existing keys keep their names (`tmuxTitlePrefix`, `hotkeyTermAnimationDuration`).
- Build order: 13u builds after 13a (the key rule, the Advanced categories) and after 13t Task 8 (Task 8 here reads 13t's `SettingsWindow.edit_session: Option<PaneId>`; there is no edit-session mode in `SettingsWindow` today, `menu.rs:420` only selects a profile). Task 6's profile-shortcut test is written by whichever of 13t Task 8 and 13u Task 6 lands second. Task 9's fade probe follows the 13g renderer ruling; if 13g has landed, the probe also covers windows on 13g's second (composition) instance. 13y2 builds after 13u Task 10 (R13xy-1: `jobtitle::tmux_session_title` owns the custom-title prefix; 13y2 keeps only the built-in case). 13h flips key action 62 (`status(62)`) after this plan's Task 1.
- User text is iTerm2's verbatim with "iTerm2" replaced by "Tandem", "Option" by "Alt", "Command" and "\u{2318}" by "Win", "System Settings > Keyboard > Keyboard Shortcuts > Keyboard" by "Windows keyboard shortcuts" (Resume notes "Texts"). Advanced rows use iTerm2's help text: the label is its first line, the help the whole text (the 11d precedent).
- Other 13x plans add rows, `global_control` arms and vendored alacritty patches in parallel: add `Kind::String`, `Kind::Int` and `Kind::Float` arms to `settings_keys::global_control` only if another 13x task has not; take the next free number in `crates/alacritty_terminal/TANDEM-PATCHES.md` when writing the patch.
- Characters outside ASCII are `\u{...}` escapes in code and `\uXXXX` in TOML. Win is Cmd and Alt is Option. New pure code is a plain `pub mod`; Win32 code is `#[cfg(windows)]`. Tests run on the Windows test machine with `scripts/xt test --lib <filter>`. Do not launch the GUI.

## Review Focus

1. The settings rows: the Advanced keys with iTerm2's sections and help, the Keys tab's layout rows enabled only with Force keyboard, the tmux attach checkbox and popup driving one stored key, the profile's Option-changeable and Control/Win bucky rows no longer marked macOS-only. Test: Task 1 `keys_rows_follow_iterm2`, `tmux_attach_controls_follow_iterm2`.
2. DECSET 1036: set and reset by programs, reported by DECRQM, cleared by RIS; honored only with SupportDecsetMetaSendsEscape, only on a side marked changeable, never when either side already sends Esc+. Test: Task 2 `meta_sends_escape_mode_follows_xterm`, `effective_option_follows_iterm2`.
3. Bucky bits: a side mapped to Hyper, Meta or Super reports 16, 32 or 8 in kitty's modifier field instead of Control or nothing, the other side unchanged, legacy encodings untouched. Test: Task 3 `bucky_bits_follow_iterm2`.
4. Remapping off: the hook injects nothing; keys Tandem receives are read with the remapped modifiers; Windows' chords see the physical keys. Test: Task 4 `in_app_remap_follows_iterm2`.
5. Forced layout: forced while Tandem is active and restored when it is not; once per session forces only at a new session's first focus and never restores; the layouts listed by name. Test: Task 5 `forced_layout_follows_iterm2`, `layout_names_follow_windows`.
6. Respect system shortcuts (R3 under R7's reading): shell chords go to Windows unless a Tandem menu command owns them, or the option is off. Test: Task 6 `symbolic_shortcuts_follow_the_option`.
7. (Removed: `02:1116` host changes are milestone 14e's.)
8. Session hotkeys: one registration per chord, cycling through its sessions, preferring the active one in a background window; set from Edit Session; saved to and read from tmux's `@hotkeys`. Test: Task 8 `session_hotkey_cycle_follows_iterm2`, `hotkeys_option_round_trips`.
9. Fades: edge styles slide and fade, other styles fade, over hotkeyTermAnimationDuration; with animation off, at once; a probe first proves a layered window presents correctly with the wgpu swap chain, else fades are off and the row partial. Test: Task 9 `fade_follows_iterm2`, `fade_allowed_follows_the_probe`.
10. tmux: a smaller tmux window draws at tmux's size with the filler color (black in full screen with the key), new tmux tabs stay in the background with the key, a custom-titled tmux session shows the prefix. Test: Task 10 `filler_follows_iterm2`, `tmux_background_follows_iterm2`, `tmux_title_prefix_follows_iterm2`.

---

### Task 1: The keys and tmux settings rows, Undo, the pinned indicator

**Files:** Modify `schema/global.toml` (Advanced `ShowPinnedIndicator` in "Hotkey"; `UseBlackFillerColorForTmuxInFullScreen`, `TmuxWindowsOpenInBackground` in "Tmux Integration"; `SupportDecsetMetaSendsEscape` in "Experimental Features" (`restoreKeyModeAutomaticallyOnHostChange` is m14e's row, not added here); Keys > "Remap Modifiers" gains `ForceKeyboard`, `ForceKeyboardOncePerSession`, `KeyboardLocale`, `AllowSymbolicHotKeys` after `RemapModifiersGlobally` `:144`; `OpenTmuxWindowsIn` `:1286` is drawn by a custom row), `schema/profile.toml` ("Right Option Key Sends" `:2593` (label "Alt Key Sends (Right)", so the right side of Task 2's 1036 logic can be set from Settings; review advisory, Decision), "Left Option Key Changeable" `:2605`, "Right Option Key Changeable" `:2616`, "Left Command Key" `:2706`, "Right Command Key" `:2718`: `macos_only = false`, labels and help from Resume notes "Settings rows"), `src/settings_keys.rs` (`row_enabled` `:731`: `KeyboardLocale` and `ForceKeyboardOncePerSession` follow `ForceKeyboard`; `tmux_attach_rows` for `OpenTmuxWindowsIn`; `keys_page` `:620` shows the new rows), `src/keyaction.rs` (`status` `:379`: 44 is Done; 62 stays for 13h), `src/app/actions.rs` (`run_key_action` `:222`: arm 44), `src/scene.rs` (`indicator_glyphs` `:829` gains `pinned: bool` after coprocess; `PIN_GLYPH`), `src/app/coprocess_ui.rs` (`draw_coprocess_indicator` `:184` passes it).

**Interfaces:**
- Consumes: `commands::DISABLED_IN_SESSION` (`src/commands.rs:18`), `App::toolbelt_chord` (Win+Z undo in a tool, `src/app/menu.rs:333`), the field `hotkey::Prefs::auto_hides` (`src/hotkey.rs:87-92`; a field, not a function), `App::hotkey`.
- Produces in `settings_keys`: `pub fn tmux_attach_rows(ui: &mut egui::Ui, g: &mut Globals) -> bool` with pure helpers `pub fn attach_checkbox(mode: i64) -> bool` (mode 2), `pub fn attach_popup(mode: i64) -> i64` (2 shows as 1), `pub fn attach_set(checked: bool, popup: i64) -> i64` (`GP:2325-2361`).
- Produces in `scene`: `pub fn indicator_glyphs(sends: bool, receives: bool, coprocess: bool, pinned: bool) -> Vec<char>`; `pub const PIN_GLYPH: char = '\u{E718}'`.
- Produces in `App`: `undo_key_action(&mut self)` (Edit > Undo's path: a toolbelt text field with the keyboard undoes, else the item is disabled in a session and the action beeps, `PS:12166-12168`, `PS:5148-5153`); `in_pinned_hotkey_window(&self) -> bool` (`ShowPinnedIndicator` and the window's hotkey profile does not auto-hide, `PS:14295-14304`).

- [ ] **Step 1: failing tests:**
  - `settings_keys.rs` `keys_rows_follow_iterm2` (Review Focus 1): each row of Resume notes "Settings rows" has that tab, section, kind, default, label and help (`SupportDecsetMetaSendsEscape` false, the release build's `YES_IF_BETA_ELSE_NO`; no `restoreKeyModeAutomaticallyOnHostChange` row (m14e's); `AllowSymbolicHotKeys` true; `ForceKeyboard` false; `KeyboardLocale` ""); `row_enabled(g, "KeyboardLocale")` follows `ForceKeyboard`; the five profile rows (with "Right Option Key Sends") are not macOS-only.
  - `tmux_attach_controls_follow_iterm2`: `attach_checkbox(2)` true, `(0)` false; `attach_popup(2)` 1, `(0)` 0; `attach_set(true, 0)` 2, `(false, 1)` 1, `(false, 0)` 0; the popup is disabled while checked.
  - `keyaction.rs` `undo_action_is_done`: `status(44)` Done, `status(62)` Later(7) until 13h lands (13h's test flips it).
  - `scene.rs` `indicators_keep_iterm2_order` extended: `indicator_glyphs(true, false, true, true)` is broadcast, coprocess, pin.
- [ ] **Step 2:** run `scripts/xt test --lib settings_keys`, `keyaction`, `scene`; expect failures (record the RED output).
- [ ] **Step 3:** implement. `tmux_attach_rows` draws "Open tmux windows as tabs in the attaching window" and "When attaching, open unrecognized windows in" with "Windows" and "Tabs in a new window", both over `OpenTmuxWindowsIn` (the Phony keys are never stored). Key action 44 calls `undo_key_action`. The pin shows on every pane of a pinned hotkey window when the key is on.
- [ ] **Step 4:** run the filters, the full `--lib` and `scripts/xt build --release`; all pass.
- [ ] **Step 5:** commit `keys: settings rows, tmux attach controls, Undo key action, pinned indicator`.

### Task 2: DECSET 1036 (metaSendsEscape)

**Files:** Modify `crates/alacritty_terminal/src/term/mod.rs` (`TermMode::META_SENDS_ESCAPE` as the next free bit after `REPORT_ASSOCIATED_TEXT` `:79`; `set_private_mode` `:2041` and `unset_private_mode` set and clear it for `PrivateMode::Unknown(1036)`; `report_private_mode` `:2153` answers it; `reset_state` clears it), `crates/alacritty_terminal/TANDEM-PATCHES.md` (the next entry), `src/keys.rs` (`KeyOptions` `:161`: `pub fn effective(&self, side: AltSide, meta_sends_escape: bool, support: bool) -> i64`; `from_profile` reads the changeable switches), `src/app/mod.rs` (`key` `:2006` passes the pane's mode and the Advanced key).

**Interfaces:**
- Consumes: alacritty `TermMode`, `PrivateMode::Unknown`, `ModeState`.
- Produces in `keys`: `KeyOptions { .., pub left_changeable: bool, pub right_changeable: bool }`; `effective` (`PS:12940-12972`: with support on, neither side set to Esc+ (2), the mode on and that side changeable, Esc+; else the side's own setting, the right one following the left when unset).
- Produces in alacritty: `TermMode::META_SENDS_ESCAPE`.

- [ ] **Step 1: failing tests:**
  - `src/screen.rs` `meta_sends_escape_mode_follows_xterm` (Review Focus 2): a fresh screen reports `\x1b[?1036$p` as `\x1b[?1036;2$y`; after `\x1b[?1036h` the mode is set and reports `;1$y`; `\x1b[?1036l` clears it; `\x1bc` (RIS) clears it.
  - `keys.rs` `effective_option_follows_iterm2`: left Normal (0), right unset, left changeable: `effective(Left, true, true)` 2; `(Left, true, false)` 0 (support off, the release default); `(Left, false, true)` 0; right not changeable: `effective(Right, true, true)` 0; left set to Esc+ already: 2 either way; right set to Esc+: the left side is not changed by the mode (`shouldRespectTerminalMetaSendsEscape` is false) and stays 0.
- [ ] **Step 2:** run `scripts/xt test --lib screen` and `keys`; expect failures.
- [ ] **Step 3:** implement. The patch's lines are commented `Tandem patch:`; upstream behavior is unchanged for every other mode. `key` uses `effective` wherever it read `left_option`/`right_option` (`src/keys.rs:173-175`).
- [ ] **Step 4:** run the filters, the full `--lib` and the release build; all pass.
- [ ] **Step 5:** commit `keys: DECSET 1036 with the Option-changeable switches`.

### Task 3: Control and Win as Hyper, Meta and Super

**Files:** Modify `src/keyproto.rs` (`KeyEvent` `:18` gains `pub sides: Sides`; `kitty` `:237` and `kitty_modifier` `:337` cook the modifier field; `pub struct Bucky { pub left_ctrl: u8, pub right_ctrl: u8, pub left_win: u8, pub right_win: u8 }` with `from_profile`), `src/app/mod.rs` (`key` and the modifier events fill `sides` from winit's `ModifiersState` left/right states, as `alt_side` is now `:3143`), `src/screen.rs` or `src/pane.rs` (the pane's `Bucky` from its profile).

**Interfaces:**
- Consumes: `winit::event::Modifiers::{lcontrol_state, rcontrol_state, lsuper_state, rsuper_state}`.
- Produces in `keyproto`: `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub struct Sides { pub lctrl: bool, pub rctrl: bool, pub lwin: bool, pub rwin: bool }`; `pub fn cook(mods: Mods, sides: Sides, b: Bucky) -> u32` (the kitty modifier bits: shift 1, alt 2, ctrl 4, super 8, hyper 16, meta 32; a side mapped to 1 Hyper, 2 Meta, 3 Super adds that bit instead of its own; Win alone adds nothing, `iTermModernKeyMapper.swift:1717-1860`).

- [ ] **Step 1: failing tests** in `keyproto.rs`:
  - `bucky_bits_follow_iterm2` (Review Focus 3): default Bucky: Ctrl+a with left ctrl in kitty mode is `\x1b[97;5u`; left ctrl mapped to Hyper: `\x1b[97;17u`; right ctrl down with the left mapped: `\x1b[97;5u` (per side); left Win mapped to Super: Win+a is `\x1b[97;9u` (Win alone was never reported); Meta: 33; the modifier key itself (kitty flags 8) reports LeftCtrl with the Hyper bit; legacy mode (no kitty flags) still sends `\x01` for Ctrl+a whatever the mapping (`updateKeyMapper`: bucky bits only reach the modern mapper, `PS:6471-6476`).
- [ ] **Step 2:** run `scripts/xt test --lib keyproto`; expect failures.
- [ ] **Step 3:** implement. `param` (`:82`) stays for the legacy encoders; the kitty encoders use `cook`.
- [ ] **Step 4:** run the filter, the full `--lib` and the release build; all pass.
- [ ] **Step 5:** commit `keys: Control and Win as Hyper, Meta and Super for the kitty protocol`.

### Task 4: Remap modifiers globally off

**Files:** Modify `src/remap.rs` (`pub fn remap_in_app(tags: &Tags, physical: Mods, sides: Sides) -> (Mods, Sides)`; `Remapper::on_key` `:97` takes `global: bool`), `src/winkeys.rs` (`remap` `:241` injects only when global; `Hook` reads a shared `global` flag), `src/app/mod.rs` (`key`: with the key off, the event's modifiers and text go through `remap_in_app` and `winkeys::text_for`), `src/winkeys.rs` (`pub fn text_for(vk: u16, scan: u32, mods: Mods) -> Option<String>` through `ToUnicodeEx` with a keyboard state built from `mods`), `src/app/menu.rs` (`install` `:178` passes the flag; a GlobalsChanged updates it).

**Interfaces:**
- Consumes: `remap::{Tags, targets}` (`src/remap.rs:21-75`), `keyproto::Sides` (Task 3).
- Produces in `remap`: `remap_in_app`.
- Produces in `winkeys`: `pub fn set_remap_global(on: bool)`, `text_for` (the current layout) over `pub fn text_for_layout(hkl: isize, vk: u16, scan: u32, mods: Mods) -> Option<String>`.

- [ ] **Step 1: failing tests:**
  - `remap.rs` `in_app_remap_follows_iterm2` (Review Focus 4): tags with left Alt as Control: physical left Alt gives Ctrl on (Alt off); right Alt untouched; tags identity: unchanged; a Remapper with `global` false returns `Verdict::Pass` for every key (the hook injects nothing, `iTermModifierRemapper.m:591-594`) and with true keeps today's `Replace` behavior.
  - `winkeys.rs` (windows) `text_follows_remapped_mods`: through `text_for_layout(hkl, vk, scan, mods)` with the US layout's HKL taken from `GetKeyboardLayoutList` (id "04090409"); when the machine has no US layout the test prints "US keyboard layout not installed; skipped" and passes (it never loads a layout into the user's session). VK 'C': no mods "c", Ctrl "\u{3}", Shift "C".
- [ ] **Step 2:** run `scripts/xt test --lib remap`, `winkeys`; expect failures.
- [ ] **Step 3:** implement. Off, a physical chord reaches Windows as typed (Alt+Tab switches windows even with Alt remapped), and Tandem reads its own keys as remapped (`iTermApplication.m:252-285`, the in-app path). The row's help becomes iTerm2's (Resume notes "Settings rows") and the Tandem-only note goes.
- [ ] **Step 4:** run the filters, the full `--lib` and the release build; all pass.
- [ ] **Step 5:** commit `keys: remap modifiers inside Tandem when not global`.

### Task 5: The forced keyboard layout

**Files:** Create `src/input_locale.rs` (pure model with a `#[cfg(windows)]` Win32 side); add to `src/lib.rs`. Modify `src/settings_keys.rs` (the `KeyboardLocale` row is a combo of `input_locale::layouts()`), `src/app/mod.rs` (`WindowEvent::Focused` `:3080` updates an app-wide "any Tandem window focused" state and calls `locale_focus` only when that state changes, so focus moving between two Tandem windows neither restores nor forces the layout; iTerm2 keys on app activation, `InputSourceForcer.swift`; field `locale: crate::input_locale::Forcer` app-wide; `apply_edit`'s GlobalsChanged calls `locale_update`), `src/pane.rs` (`pub needs_forced_layout: bool`, true for a new session), `src/app/keys_ui.rs` (create; mod line in `src/app/mod.rs`; `locale_focus`, `locale_update`, `force_for_session`).

**Interfaces:**
- Consumes: `windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayoutList, GetKeyboardLayout, ActivateKeyboardLayout, HKL}`, `windows_sys::Win32::System::Registry::{RegOpenKeyExW, RegGetValueW}` (read-only: `HKLM\SYSTEM\CurrentControlSet\Control\Keyboard Layouts\<KLID>\Layout Text`), `windows_sys::Win32::Globalization::{LCIDToLocaleName, GetLocaleInfoEx, LOCALE_SLOCALIZEDDISPLAYNAME}` (only when the registry has no Layout Text).
- Produces in `input_locale`: `pub struct Prefs { pub force: bool, pub once: bool, pub layout: String }` with `from_globals`; `#[derive(Clone, Debug, PartialEq, Eq)] pub enum Step { Activate(String), Nothing }`; `pub struct Forcer { system: Option<String>, active: bool, changed_since_active: bool }` with `pub fn became_active(&mut self, prefs: &Prefs, current: &str) -> Step` (remember the system layout, then force unless once-per-session), `pub fn resigned(&mut self, prefs: &Prefs) -> Step` (restore the system layout unless once-per-session), `pub fn layout_changed(&mut self, prefs: &Prefs) -> Step` (force again once after becoming active, `InputSourceForcer.swift:107-118`), `pub fn prefs_changed(&mut self, prefs: &Prefs) -> Step`, `pub fn session_focused(&mut self, prefs: &Prefs, needs: bool) -> (Step, bool)` (once-per-session: force and clear the flag); `pub fn layout_id(hkl: isize) -> String` (the KLID-style 8 hex digits of the layout's low and high words); `#[cfg(windows)] pub fn layouts() -> Vec<(String, String)>` (display name, id; sorted by name; the name is the layout's own "Layout Text", so US and US-International differ; a name that still repeats gets its id appended in parentheses); `pub fn display_names(raw: &[(String, String)]) -> Vec<(String, String)>` (the pure duplicate rule); `#[cfg(windows)] pub fn current() -> String`; `#[cfg(windows)] pub fn activate(id: &str) -> bool`.

- [ ] **Step 1: failing tests** in `input_locale.rs`:
  - `forced_layout_follows_iterm2` (Review Focus 5): force on, layout "00000407": `became_active(.., "00000409")` activates "00000407"; `resigned` activates "00000409"; with once-per-session both are `Nothing`, and `session_focused(.., true)` activates once and clears the flag, `(.., false)` nothing; force off: every step `Nothing`; an empty layout with force on falls back to the system layout (`desiredLocale ?? systemLocale`); `layout_changed` forces once, then `Nothing` until the next activation.
  - `layout_names_follow_windows`: `display_names(&[("US", "04090409"), ("US", "F0010409"), ("German", "04070407")])` is German, "US (04090409)", "US (F0010409)" sorted by name; unique names are unchanged; the app-activation rule: two `Focused` events moving between Tandem windows produce no `became_active`/`resigned` step (a pure `activation_change(was_any: bool, now_any: bool) -> Option<bool>`); `layout_id(0x0409_0409)` "04090409"; `layout_id(0xF020_0409)` "F0200409" (Decision: the id is the HKL's two words, which round-trips through `GetKeyboardLayoutList`).
- [ ] **Step 2:** run `scripts/xt test --lib input_locale`; expect failures.
- [ ] **Step 3:** implement. `activate` finds the HKL with that id in `GetKeyboardLayoutList` and calls `ActivateKeyboardLayout(hkl, 0)` on the UI thread (Windows' default input setting is one layout for all apps, so restoring on deactivation matters); an id not installed logs and does nothing (`InputSourceForcer.swift:180-199`). The layout combo lists installed layouts; a stored id no longer installed shows empty.
- [ ] **Step 4:** run the filter, the full `--lib` and the release build; all pass.
- [ ] **Step 5:** commit `keys: force a keyboard layout`.

### Task 6: Respect Windows' own shortcuts (one R3 fix)

**Files:** Modify `src/keyroute.rs` (`Snapshot` `:130` gains `respect_system: bool`; `win_chord_bound` `:159` refuses a key-mapping-only hit on a shell chord when it is set), `src/winkeys.rs` (`pub fn shell_owned_vk(vk: u16, alt: bool, shift: bool, ctrl: bool) -> bool` over `shell_owned` `:480`, mapping the VK to its scan code), `src/app/mod.rs` (`refresh_route` builds the snapshot with `AllowSymbolicHotKeys`).

**Interfaces:**
- Consumes: `winkeys::shell_owned`, `commands::command_for_chord`, `MapVirtualKeyW(MAPVK_VK_TO_VSC_EX)`.
- Produces in `keyroute`: `pub fn win_chord_bound(s: &Snapshot, chord: &Chord) -> bool` (same signature); `pub fn yields_to_system(respect: bool, shell_owned: bool, menu_command: bool) -> bool` (`respect && shell_owned && !menu_command`); `menu_command` is true for a chord `commands::command_for_chord` knows and for a Ctrl+Win chord in 13t Task 8's `Snapshot.profile_shortcuts` (a profile Shortcut is a Profiles menu item, R7).

- [ ] **Step 1: failing tests** in `keyroute.rs`:
  - `symbolic_shortcuts_follow_the_option` (Review Focus 6): `yields_to_system(true, true, false)` true; `(true, true, true)` false (a Tandem menu command still wins, as iTerm2's menu still sees a key equivalent the text view passes on, `TV:690-705`); `(false, true, false)` false; `(true, false, false)` false. A snapshot with a global key mapping on Win+E (shell-owned) and `respect_system` true: `win_chord_bound` false; with false: true; Win+T (New Tab, a menu command, shell-owned) true either way; a snapshot whose `profile_shortcuts` holds 'D' claims Ctrl+Win+D with `respect_system` true (when 13t Task 8 has landed; else 13t Task 8 adds this line).
- [ ] **Step 2:** run `scripts/xt test --lib keyroute`, `winkeys`; expect failures.
- [ ] **Step 3a (R3, `02:340`):** with the default on, a key mapping on a chord Windows keeps (Win+E, Win+D, Win+Ctrl+Left...) no longer takes it from Windows. Record: "Tandem's hook took shell chords for key mappings; iTerm2's default (Respect shortcuts from System Settings) lets the system's shortcuts win. Fixed (R3) under ruling R7."
- [ ] **Step 3b:** the row's label is "Respect shortcuts from Windows keyboard shortcuts" and its help Tandem's one-line explanation of the Windows mapping (Resume notes "Settings rows").
- [ ] **Step 4:** run the filters, the full `--lib` and the release build; all pass.
- [ ] **Step 5:** commit `keys: respect Windows' own shortcuts over key mappings`.

### Task 7: Removed (milestone 14 owns `02:1116`)

`02:1116` restoreKeyModeAutomaticallyOnHostChange is owned by milestone 14 (`.superpowers/research/m14-rulings.md` item 6; 13s-u rulings: 13u Task 7 REMOVED). `docs/superpowers/plans/2026-10-14-m14e-ssh-urls.md` Task 4 builds it (`src/host_state.rs`, `src/app/host_state_app.rs`, `Request::HostChanged`, the schema row). This plan creates no `host_state.rs`, adds no schema row for the key and closes no parity row for it. The task number is kept so the other task numbers (13t, 13y2 and the rulings cite 13u Tasks 6, 8 and 10) stay valid.

### Task 8: Session hotkeys and tmux's @hotkeys

**Files:** Create `src/session_hotkeys.rs` (pure); add to `src/lib.rs`. Create `src/app/session_hotkey_ui.rs` (mod line in `src/app/mod.rs`). Modify `src/app/hotkey_ui.rs` (`register_hotkeys` `:76` adds session chords with ids from `SESSION_HOTKEY_BASE`; `hotkey_pressed` `:176` routes them), `src/settings_ui.rs` (the "Session Hotkey" row shows only while 13t Task 8's `SettingsWindow.edit_session` is Some, as a shortcut recorder writing that pane's session value), `src/tmux_controller.rs` (add a new `Req::Hotkeys` variant and put it in the 9th slot of the attach request list, which is `Req::Ignore` today (`src/tmux_controller.rs:1063`, the `@hotkeys` reply); its reply is parsed into `pub hotkeys: HashMap<u32, hotkey::Shortcut>`; pane opens carry their hotkey), `src/tmux_persist.rs` (`pub fn hotkeys_string(map: &[(u32, hotkey::Shortcut)], live: &BTreeSet<u32>) -> String`, `pub fn parse_hotkeys(value: &str) -> HashMap<u32, hotkey::Shortcut>`), `src/app/tmux_tabs_ui.rs` (a tmux pane's session value "Session Hotkey" from the map; a change sends `option_command(s, "@hotkeys", ..)`).

**Interfaces:**
- Consumes: 13t Task 8 `SettingsWindow.edit_session: Option<PaneId>` (build 13t Task 8 first); `hotkey::{Shortcut, chord}` (`src/hotkey.rs:57-150`), `winkeys::set_hotkeys`, `App::{set_session_values, session_values}`, `tmux_persist::option_command` (`src/tmux_persist.rs:218`, `@hotkeys` prefix "h_" already listed), `tmux_windows::decode_option`.
- Produces in `session_hotkeys`: `pub const SESSION_HOTKEY_BASE: u32 = 0x1000`; `pub fn groups(sessions: &[(PaneId, hotkey::Shortcut)]) -> Vec<(hotkey::Chord, Vec<PaneId>)>` (one registration per chord, sessions in creation order); `pub fn pick(group: &[PaneId], first_responder: Option<PaneId>, active_in_background: &[PaneId]) -> Option<PaneId>` (`iTermSessionHotkeyController.m:68-94`: the one after the first responder, else one active in a non-key window, else the first); `pub fn short_string(s: &hotkey::Shortcut) -> String` and `pub fn from_short_string(t: &str) -> Option<hotkey::Shortcut>` (`keyCode,modifiers,characters,charactersIgnoringModifiers` with `\0` `\1` `\2` escaping "\\", ",", " ", `iTermShortcut.m:26-86`).
- Produces in `App`: `session_hotkey_pressed(&mut self, id: u32)`; `set_session_hotkey(&mut self, pane: PaneId, s: Option<hotkey::Shortcut>)`.

- [ ] **Step 1: failing tests:**
  - `session_hotkeys.rs` `session_hotkey_cycle_follows_iterm2` (Review Focus 8): two sessions on Ctrl+Alt+1 and one on Ctrl+Alt+2 give two groups; `pick(&[a, b], Some(a), &[])` b; `(Some(b))` a; `(None, &[b])` b; `(None, &[])` a.
  - `hotkeys_option_round_trips`: `short_string` of keyCode 18, modifiers 1048576, characters "1", ignoring "1" is "18,1048576,1,1"; characters "a,b" encode as "a\\1b"; `from_short_string` inverts; fewer than four parts is None; `tmux_persist::hotkeys_string(&[(3, s)], &{3})` is "3=18,1048576,1,1" and pane 4 not live is dropped; `parse_hotkeys` of `option_command`'s encoded value gives it back.
  - `settings_ui.rs` `session_hotkey_row_is_edit_session_only`: `SettingsWindow` with `edit_session: Some(pane)` shows the row and writes pane's session value; with `edit_session: None` it does not show.
- [ ] **Step 2:** run `scripts/xt test --lib session_hotkeys`, `tmux_persist`, `settings_ui`; expect failures.
- [ ] **Step 3:** implement. A session hotkey is a session value (never the saved profile); pressing it reveals the picked session (window forward, tab and pane focused, `PS:21843-21845`); a pane that ends leaves its group. tmux: `@hotkeys` is read at attach (`TmuxController.m:3068-3085`) and each opening pane gets its hotkey; setting one on a tmux pane writes `@hotkeys` for the live panes (`TmuxController.m:2204-2270`). The `03:417` row loses its stale "8h remains".
- [ ] **Step 4:** run the filters, the full `--lib` and the release build; all pass.
- [ ] **Step 5:** commit `keys: session hotkeys and tmux's @hotkeys`.

### Task 9: Fading hotkey windows

**Files:** Modify `src/hotkey_motion.rs` (`Motion` `:57` gains `pub alpha: (f32, f32)`; `at` returns the alpha; `pub fn fades(style: i64) -> bool`; the left styles slide with a zero distance and fade, `iTermProfileHotKey.m:467-469`), `src/app/hotkey_ui.rs` (`show_hotkey_window` `:238` and the hide path `:308` start a motion for every animated style; the tick sets the window's alpha), `src/app/hotkey_ui.rs` (`fn set_window_alpha(hwnd: isize, alpha: f32)`, `#[cfg(windows)]`: `WS_EX_LAYERED` added for the animation, `SetLayeredWindowAttributes(LWA_ALPHA)`, the style removed at the end).

**Interfaces:**
- Consumes: the field `hotkey::Prefs::animates` (`src/hotkey.rs:91`; a field, not a function), `hotkey_duration` (`src/app/hotkey_ui.rs:352`), `Roll`.
- Produces in `hotkey_motion`: `Motion { from, to, duration, alpha }`, `pub fn at(&self, elapsed: Duration) -> ((f32, f32), f32, bool)`; `pub fn fades(style: i64) -> bool` (every style but full screen); `pub fn curve(style: i64) -> fn(f32) -> f32` (edges ease in, others linear; Decision).

- [ ] **Step 0 (probe, 13s-u ruling; the 13g renderer rule for any swap-chain change):** layered windows and wgpu's DXGI flip-model (or Vulkan) presentation are known not to combine reliably: `SetLayeredWindowAttributes` can succeed while the window draws blank or presents fail. Before any fade code, a probe on the Windows test machine (an `#[ignore]`d test whose window is created off screen with `WS_EX_TOOLWINDOW` and never brought in front of the user; if it cannot start over ssh it reports BLOCKED and the controller asks Robert before anything runs in his desktop session) renders a known color through today's wgpu path, adds `WS_EX_LAYERED` with `LWA_ALPHA` 128, presents three frames, reads the result back with `PrintWindow`/`BitBlt` from the window DC and checks the pixel is the half-blended color and `present` returned no error; it removes the style and checks the opaque color returns. Run it plus the existing render tests, and add a checklist line per GPU vendor available (the Windows test machine's NVIDIA; Intel/AMD when a machine is at hand). If the probe fails on any tested path, `FADES_WORK` is false: fades are off (every animated style shows and hides at once, slides still slide), `01:297` and `03:668` stay partial with the reason, and Steps 1 to 3 build only the slide half. Record the probe result in `m13u-rulings.txt`.
- [ ] **Step 1: failing tests** in `hotkey_motion.rs`:
  - `fade_allowed_follows_the_probe`: a pure `fade_allowed(probe_ok: bool, animates: bool, present_failed: bool) -> bool`: (true, true, false) true; (false, _, _) false; (true, false, _) false; (true, true, true) false (a present error during a fade stops fading for the rest of the run and shows the window at once).
  - `fade_follows_iterm2` (Review Focus 9): a top-style show over 0.25 s: alpha 0 at 0, 1 at 0.25 s, the origin moving with ease-in; a normal-style show: origin fixed, alpha linear (0.5 at 0.125 s); a hide runs alpha 1 to 0; a left-style show has `from == to` and fades (`hiddenOriginForScreen:` puts it at the screen's left, where it already is); `slides` keeps top, bottom, right; the existing slide tests still pass with the new return.
- [ ] **Step 2:** run `scripts/xt test --lib hotkey_motion`; expect failures.
- [ ] **Step 3:** implement. With "Animate hotkey window" off nothing changes (at once, alpha 1). A layered style is set only while a fade runs (so the swap chain keeps its normal path), and only when `fade_allowed` (Step 0's result, recorded in a const, and no present error seen); if `SetLayeredWindowAttributes` fails or a present during the fade returns an error or the surface is lost, the fade stops, the style is removed and the window shows at once, and fades stay off for the rest of the run; no banner is raised (Decision).
- [ ] **Step 4:** run the filter, the full `--lib` and the release build; all pass.
- [ ] **Step 5:** commit `hotkeys: fade hotkey windows in and out`.

### Task 10: tmux filler, background opening, the title prefix

**Files:** Modify `src/tmux_layout.rs` (`pub fn natural_size(node: &Node, cell: (f32, f32), gap: f32) -> (f32, f32)`), `src/app/menu.rs` (`visible` `:803`: a tmux tab lays out in its natural size at the top left of the content area), `src/scene.rs` (`pub fn filler_color(bg: Rgb, dark_bg: bool, black: bool) -> Rgb`), `src/app/mod.rs` (`build_frame` `:2495` fills the rest of a tmux tab with it), `src/app/tmux_tabs_ui.rs` (`:815-825`: with `TmuxWindowsOpenInBackground` a new tmux tab is inserted without becoming active and a new tmux window opens without activation), `src/app/jobs_ui.rs` (`session_title` `:44`: a tmux client pane with a custom title gets the prefix).

**Interfaces:**
- Consumes: `tmux_layout::Node` (`src/tmux_layout.rs`), `Tab::tmux` state, `App::{content_area_for, tmux_title}`, `title_provider_id` (`src/app/jobs_ui.rs:36`).
- Produces in `scene`: `filler_color` (`PTYTab.m:1189-1208`: black in full screen with the key; else the background mixed 10% toward white when it is dark, toward black when light).
- Produces in `App`: `tmux_window_name_of(&self, pane: PaneId) -> Option<String>`.

- [ ] **Step 1: failing tests:**
  - `scene.rs` `filler_follows_iterm2` (Review Focus 10): `filler_color(black, true, false)` is (26, 26, 26) (0.1 toward white); `(white, false, false)` (230, 230, 230); `(_, _, true)` black; `tmux_layout::natural_size` of an 80x24 single pane at cell (10, 20) is (800, 480), two side-by-side 40-column panes 810 (the one-cell divider).
  - `tmux_tabs_ui.rs` (pure `tmux_windows::open_active(background_key: bool) -> bool`) `tmux_background_follows_iterm2`: false with the key, true without.
  - `jobs_ui.rs` (pure `jobtitle::tmux_session_title(prefix: &str, provider: Option<Option<&str>>, base: &str, window_name: &str) -> Option<String>`) `tmux_title_prefix_follows_iterm2`: custom with a provider value "api" and prefix "\u{21a3} " gives "\u{21a3} api"; custom with no Title Function gives "\u{21a3} " plus the window name; not custom gives None (Tandem's tmux label paths keep their titles); a gateway is not handled here (its `[prefix base client]` exists, `tmux_windows::gateway_title`).
- [ ] **Step 2:** run `scripts/xt test --lib scene`, `tmux_layout`, `tmux_windows`, `jobtitle`; expect failures.
- [ ] **Step 3:** implement. A tmux tab whose content area is larger than tmux's window draws the panes at tmux's size and the rest in the filler color (`flexibleView_`); the size report still sends the whole area (tmux resizes to it, then the filler vanishes); a smaller area keeps today's layout. The custom-title rule is `iTermSessionNameController.m:368-387` for sessions with a tmux controller; it supersedes `m11c-rulings.txt:157` (recorded). This task owns the custom-title prefix (R13xy-1: `jobtitle::tmux_session_title` here; 13y2 keeps only the built-in title case).
- [ ] **Step 4:** run the filters, the full `--lib` and the release build; all pass.
- [ ] **Step 5:** commit `tmux: filler area, background opening, title prefix on custom titles`.

### Task 11: Parity, checklist, rulings

**Files:** Modify `docs/parity/01-profile-settings.md`, `docs/parity/02-global-settings.md`, `docs/parity/03-features-and-menus.md`, `docs/parity/00-roadmap.md` (the milestone 13 paragraph: 13u done, counts); create `docs/acceptance/m13u-checklist.md`, `docs/superpowers/rulings/m13u-rulings.txt`.

- [ ] **Step 1:** parity, rows found by title (line numbers from m13-facts may have drifted): 01 Session Hotkey `01:41` (have), Left and Right Option Key Changeable `01:276`, `01:277` (have; Windows note: Alt; remove "macOS-only"), Left and Right Control Key `01:283`, `01:284`, Left and Right Command Key `01:285`, `01:286` (have; Windows note: Win; kitty protocol only, as iTerm2's modern mapper), HotKey Window Animates `01:297` (have: slide and fade); 02 Open tmux windows as tabs in the attaching window `02:163`, Open unrecognized tmux windows in `02:164` (have: one key, two controls; remove the 8e note), Remap modifiers globally `02:312`, `02:329` (have), Force keyboard `02:337`, once per session `02:338`, Keyboard locale `02:339` (have; Windows note: installed keyboard layouts, ActivateKeyboardLayout), Respect shortcuts `02:340` (have; Windows note: Windows' own Win chords, ruling R7), Undo `02:406` (have; remove "Comes in milestone 7."), ShowPinnedIndicator `02:698`, UseBlackFillerColorForTmuxInFullScreen `02:944`, TmuxWindowsOpenInBackground `02:953`, SupportDecsetMetaSendsEscape `02:1098` (have); `02:1116` is not touched (milestone 14e); 03 tmux integration `03:417` (have; remove "8h remains"), tmux Window/pane hotkeys `03:430` (have), Hotkey windows Animated show/hide `03:668` (have).
- [ ] **Step 2:** `m13u-checklist.md` in the m10b form (copy `tandem.exe`, `tandem-host.exe`, `tandem-git.exe`, `conpty.dll`, `OpenConsole.exe` together): with SupportDecsetMetaSendsEscape on and Left Alt Normal, `printf '\e[?1036h'` then Alt+b in bash moves back a word, `printf '\e[?1036l'` stops it, Right Alt not changeable ignores it; kitty protocol (`kitten show-key -m kitty`) reports Left Ctrl as Hyper with the profile set; Remap modifiers globally off with Left Alt as Ctrl: Alt+C copies in Tandem and Alt+Tab still switches windows; Force keyboard to German: typing in Tandem gives German keys, switching to Notepad gives the previous layout back, once per session forces only a new tab's first focus; with a global key mapping on Win+E and Respect shortcuts on, Win+E opens Explorer, off it runs the mapping, Win+T still makes a tab; the Undo key action in a Snippets tool field undoes, in the terminal it beeps; ShowPinnedIndicator shows a pin on a pinned hotkey window; Edit Session > Session hotkey Ctrl+Alt+1 on two sessions cycles them, from another app too; in tmux -CC a pane's session hotkey survives detach and attach; a normal-style hotkey window fades, a top-style one slides and fades, animation off shows at once (or, if Task 9's probe failed, every style shows at once and the rows are partial); tmux attach with a smaller client shows the filler, black in full screen with the key; TmuxWindowsOpenInBackground keeps focus on the attaching tab; a tmux pane with a custom title function shows the prefix; one line per kept iTerm2 quirk and ruling a person can see.
- [ ] **Step 3:** `m13u-rulings.txt` in the `Ruling: ... - cost if wrong: ...` form: the R3 fix (Task 6 Step 3a), ruling R7, the removal of Task 7 (milestone 14e owns `02:1116`), the Task 9 probe result, the `m11c-rulings.txt:157` supersession, every Decision below, then each task's rulings and deferred minors as they happen.
- [ ] **Step 4:** run `scripts/xt test --lib`; commit `docs: m13u parity, checklist, rulings`.

---

## Resume notes (facts gathered before Task 1)

- Nothing of 13u is written. Start the ledger at `.superpowers/sdd/2026-10-15-m13u-keys-hotkeys/progress.md` with `task-start PLAN 1` (create the folder). iTerm2 paths are under `~/gitrepos/iterm2-ref/sources/` (HEAD `64791fc`). `AS` `Settings/iTermAdvancedSettingsModel.m`, `PR` `Settings/iTermPreferences.m`, `GP` `Settings/GeneralPreferencesViewController.m`, `KP` `Settings/KeysPreferencesViewController.m`, `XIB` `Settings/Base.lproj/PreferencePanel.xib`, `PS` `PTYSession/PTYSession.m`, `PSA` `PTYSession/PTYSession+ARC.m`, `NC` `PTYSession/iTermNaggingController.m`, `SNC` `PTYSession/iTermSessionNameController.m`, `VT` `VT100/VT100Terminal.m`, `MKM` `Keyboard/iTermModernKeyMapper.swift`, `MR` `Keyboard/iTermModifierRemapper.m`, `APP` `AppKit/iTermApplication.m`, `ISF` `Hacks/InputSourceForcer.swift`, `SHK` `Hotkey/iTermSymbolicHotkeys.swift`, `SHC` `Hotkey/iTermSessionHotkeyController.m`, `SC` `Hotkey/iTermShortcut.m`, `PHK` `Hotkey/iTermProfileHotKey.m`, `TC` `tmux/TmuxController.m`, `TAB` `TerminalView/PTYTab.m`, `TV` `TerminalView/PTYTextView.m`, `IH` `Drawing/iTermIndicatorsHelper.m`, `PP` `Settings/iTermProfilePreferences.m`, `PT` `TerminalView/PseudoTerminal.m`.

### Tandem code these tasks build on (verified at `5269076`)

- Keys: `KeyOptions` (`src/keys.rs:161-176`, `left_option`, `right_option`, `option_is_alt`, `from_profile`), `AltSide` (`:152`); `App::key` (`src/app/mod.rs:2006`), `dispatch` (`:2068`), `refresh_route` (`:1940`), `to_terminal` (`:2112`), the modifiers event (`alt_side` `:3143`); `keyproto::{KeyEvent :18, Keyboard :41, encode :54, param :82 (Win never counted), csi_u :166, kitty :237, kitty_modifier :337}`; `Screen::{keyboard :632, set_key_mode :662}`.
- alacritty: `TermMode` bits (`crates/alacritty_terminal/src/term/mod.rs:57-80`, last `REPORT_ASSOCIATED_TEXT` `1 << 22`), `set_private_mode` (`:2041`, unknown modes ignored `:2044-2047`), `report_private_mode` (`:2153`), `reset_state`; 15 patches in `TANDEM-PATCHES.md`.
- Hook and remap: `winkeys::{Gate :92, remap :241, hook_proc :268, install :320, shell_owned :480, set_hotkeys :528, mask_start_menu :211}`; `remap::{Tags :21, Remapper :83, on_key :97}`; `keyroute::{Snapshot :130, win_chord_bound :159}`; `commands::command_for_chord`; the install call (`src/app/menu.rs:161-178`).
- Hotkeys: `hotkey::{Shortcut :57-72, Prefs :76-111 (animates :91), chord :137, Win, Act}`; `hotkey_ui::{register_hotkeys :76, hotkey_pressed :176, show_hotkey_window :238, slide_for :341, hotkey_duration :352, finish_hide :356}`; `hotkey_motion::{DURATION :11, slides :14, hidden_origin :20, slide_allowed :30, ease_in :36, Motion :57}`.
- Actions: `keyaction::status` (`src/keyaction.rs:377-383`), `run_key_action` (`src/app/actions.rs:222`), `App::later` (`:106`), `commands::DISABLED_IN_SESSION` (`src/commands.rs:18`), `run_chord`'s toolbelt Win+Z (`src/app/menu.rs:331-344`), `find_ui::beep` (`src/app/find_ui.rs:203`).
- Indicators: `scene::{indicator_glyphs :829, top_right_indicators :860}`, `draw_coprocess_indicator` (`src/app/coprocess_ui.rs:184`).
- Hosts: the RemoteHost arm (`src/marks.rs:889-897`, `shell.user`, `shell.host`).
- tmux: `tmux_windows::open_list` (`src/tmux_windows.rs:10-25`, `@hotkeys` asked `:21`), `tmux_persist::option_command` (`src/tmux_persist.rs:218`, `@hotkeys` "h_" `:226`), the controller's option replies (`src/tmux_controller.rs:1095-1110`), `tmux_layout::to_layout` (`src/tmux_layout.rs:196`), a new tmux tab made active (`src/app/tmux_tabs_ui.rs:818-824`), `tmux_title` (`:720`), `tmux_titles` (`src/app/tmux_ui.rs:264`), `session_title` and `title_provider_id` (`src/app/jobs_ui.rs:36-89`), `window_title` (`:92`), `OpenTmuxWindowsIn` (`schema/global.toml:1286-1293`); `App::visible` (`src/app/menu.rs:803`).
- Settings: `keys_page` (`src/settings_keys.rs:620`), `global_control` (`:581`), `row_enabled` (`:731`); profile rows "Session Hotkey" (`schema/profile.toml:406`, dict), "Option Key Sends" (`:2581`), "Right Option Key Sends" (`:2593`), the changeable switches (`:2605`, `:2616`, macOS-only today), "Left/Right Control Key" (`:2682`, `:2694`), "Left/Right Command Key" (`:2706`, `:2718`, macOS-only today), "HotKey Window Animates" (`:2831`).

### Every key is read (R2 check)

- `AS:475` ShowPinnedIndicator `PS:14296`; `AS:732` `TAB:1190`; `AS:741` TmuxWindowsOpenInBackground `PT:3992`; `AS:900` SupportDecsetMetaSendsEscape `PS:12941`; `AS:918` `PS:17937`; `AS:738` tmuxTitlePrefix `SNC:371-384`; `PR:169-173` ForceKeyboard, ForceKeyboardOncePerSession, AllowSymbolicHotKeys, KeyboardLocale, RemapModifiersGlobally read `ISF:145-160`, `TV:698-699`, `MR:490`, `MR:591`; `PR:82`, `:92-93` OpenTmuxWindowsIn and the two phony keys (`GP:2325-2361`); profile "Left/Right Option Key Changeable" `PS:12956`, `:12965`; "Left/Right Control Key", "Left/Right Command Key" `PS:12975-12989`, `MKM:1717-1735`; "Session Hotkey" `PS:4124-4126`, `:6294-6296`; "HotKey Window Animates" `PHK:787`, `:1286`.

### DECSET 1036 (VT, PS)

- `VT:899-900` sets `metaSendsEscape` for 1036; `VT:5708-5709` reports it; `VT:398`, `:1696` reset clears it. `PS:12940-12972`: `optionKey` answers Esc+ when supported, neither side sends Esc+, the mode is on and the left side is changeable (the right likewise with its switch; a missing right setting follows the left).

### Bucky bits (MKM, PS)

- `PS:6471-6476`: the modern mapper runs only with kitty flags. `MKM:1717-1735`: sides mapped away from Regular leave the event's Control/Command. `MKM:1754-1790`: shift 1, alt 2, control 4, super 8, hyper 16, meta 32; encoded is the sum plus one. `MKM:1797-1860`: each side's bucky bit replaces its flag.

### Remapping (MR, APP)

- `MR:586-640`: the event tap remaps only while iTerm2 is active and only with RemapModifiersGlobally; `APP:252-285`: with the tap off, events iTerm2 receives are remapped in-app ("Only things like cmd-tab will not be remapped in this case").

### Forced layout (ISF, KP)

- `ISF:43-200`: the system layout is remembered at activation; sustained mode forces while active and restores on resign; once-per-session forces at a new session's first focus and never restores; a user layout change after activation is overridden once. `KP:252-352`: the three controls, the layout popup sorted by name, enabled with Force keyboard. `PS:13552`, `iTermSessionFactory.m:384`: a new session needs forcing.

### Symbolic hotkeys (SHK, TV)

- `TV:697-703`: on macOS 15 and later, with the key on and the event matching an enabled system symbolic hotkey (`SHK:24-66`, exact modifiers), the text view returns NO so the key mapper does not take it; the menu still gets its key equivalents.

### Host state (PS, PSA, NC)

- `PS:17919-17942`: on a host name change not via SSH integration and not between two localhosts, with the key on, `pushOrPopHostState:`. `PS:18007-18033`: push the key mapping mode, host, icon and window names; on a return pop to it, restore the mode (key reporting flags are a TODO in iTerm2, kept), offer the titles. `PSA:244-253`, `NC:619-660`: the offer only with Allow Title Setting; remembered as `NoSyncRestoreIconAndWindowNameOnHostChange`.

### Session hotkeys (SHC, SC, TC, PS)

- `SHC:25-178`: one Carbon registration per shortcut, sessions in a list, the press picks as Task 8. `PS:21843-21859`: reveal; first responder and background-window tests. `SC:26-86`: the short string and its escaping. `TC:2204-2290`, `:3042-3085`: `@hotkeys` written for live panes, read at attach; `tmux/TmuxWindowOpener.m:664-667`: an opening pane's hotkey. `PS:8098-8106`: a tmux session's hotkey change writes it.

### Hotkey fades (PHK)

- `PHK:513-535`: edge styles move from the hidden origin and fade in together, ease-in, `hotkeyTermAnimationDuration`; `PHK:537-570`: out likewise; `PHK:583-617`: other styles fade in and out (NSAnimationContext's default timing); `PHK:726-768`, `:770-822`: which styles do which; animation off sets alpha at once. `PHK:461-470`: the left styles' hidden origin is the screen's left edge.

### tmux (TAB, SNC, AS)

- `TAB:1189-1208`: the flexible (filler) view's color. `SNC:368-387`: a gateway shows `[prefix base client]`; a session with a tmux controller shows `prefix base` when `sessionNameControllerUniqueIdentifier` is non-nil (the system title, or a custom title with a function, `PS:22680-22692`) and `prefix windowName` when custom with no function.

### Settings rows

| Key | Tab / Section | Type | Default | Label / Help |
|---|---|---|---|---|
| `ShowPinnedIndicator` | Advanced / Hotkey | bool | false | "Show indicator for pinned hotkey windows." |
| `UseBlackFillerColorForTmuxInFullScreen` | Advanced / Tmux Integration | bool | false | "Use black for filler area in tmux windows in full screen." |
| `TmuxWindowsOpenInBackground` | Advanced / Tmux Integration | bool | false | "Open tmux windows and tabs in the background?\nWhen enabled, attaching with tmux -CC (or when tmux creates a new window) will reveal the window or tab without moving keyboard focus to it. Focus stays on the attaching session." |
| `SupportDecsetMetaSendsEscape` | Advanced / Experimental Features | bool | false | "Support DECSET 1036?\nThis allows apps in the terminal to control whether the Alt key sends esc+ or acts like a regular Alt key." |
| `ForceKeyboard` | Keys / Remap Modifiers | bool | false | "Force keyboard" / "Forces Tandem to use the selected keyboard layout while it is active." |
| `ForceKeyboardOncePerSession` | Keys / Remap Modifiers | bool | false | "Only force once per session when it first receives keyboard focus" / "When \u{201c}Force keyboard\u{201d} is on, Tandem continuously forces the selected layout while it is active, so you cannot switch to another layout within a session. Turn this on to force the layout only once, the first time a newly created terminal receives keyboard focus, leaving you free to change it afterward." |
| `KeyboardLocale` | Keys / Remap Modifiers | string | "" | "Keyboard locale" / same |
| `AllowSymbolicHotKeys` | Keys / Remap Modifiers | bool | true | "Respect shortcuts from Windows keyboard shortcuts" / "Win shortcuts Windows keeps for itself go to Windows instead of a key mapping; Tandem's own menu shortcuts still work." |
| `RemapModifiersGlobally` (help only) | Keys / Remap Modifiers | bool | true | "Remap modifiers globally" / "When off, Tandem remaps modifiers only for keys it receives, so Windows' own shortcuts see the physical keys." |
| "Left Option Key Changeable" (profile) | Keys | bool | true | "Left Alt key changeable" / "Whether left Alt key mode can be toggled at runtime" |
| "Right Option Key Changeable" (profile) | Keys | bool | false | "Right Alt key changeable" / "Whether right Alt key mode can be toggled at runtime" |
| "Left Command Key", "Right Command Key" (profile) | Keys | enum | 0 | "Left Win Key", "Right Win Key" (Normal, Hyper, Meta, Super) |

- Sources: `AS:475` (`SECTION_HOTKEY` "Hotkey", `AS:468`), `AS:732`, `:741` (`SECTION_TMUX` "Tmux Integration", `AS:726`), `AS:900`, `:918` (`SECTION_EXPERIMENTAL`), `PR:169-173`, `:851`, `:863-866`, `XIB:10112-10190`, `PP:884-885` (profile descriptions), `PR:1237-1238`. "Option" becomes "Alt" and the macOS-only texts become Windows' (Texts rule); the Force keyboard help and the two Respect/Remap helps are Tandem's (iTerm2 has no tooltip or a macOS-specific one).

### Texts

- "Open tmux windows as tabs in the attaching window", "When attaching, open unrecognized windows in", "Windows", "Tabs in a new window" (`XIB:8600-8720`); (the host-change title offer texts are m14e's, Task 7 removed); "This Hotkey Window is pinned." (the pin's hover text, `IH:392`).

### Corrections to the research (the source wins)

- `02:1098` SupportDecsetMetaSendsEscape defaults to `YES_IF_BETA_ELSE_NO`, NO in a release build; Tandem's missing 1036 matches the release default, so no R3 fix applies.
- `02:340`: iTerm2's check applies to every enabled macOS symbolic hotkey, not a fixed list, and only on macOS 15 and later; Windows has no user-editable symbolic hotkey table, so the shell's own chords stand in (ruling R7).
- `02:1116` belongs to milestone 14 (m14-rulings item 6, m14e Task 4); the Host state notes below stay only as context.
- `01:283`-`01:286`: bucky bits reach only the kitty protocol's modern mapper; the facts point at the xterm parameter (`keyproto.rs:82`), which stays unchanged.
- `03:668`/`01:297`: iTerm2's edge styles fade while they slide, and the left styles fade in place (their hidden origin is where they show); Tandem's comment "left does not" slide is right about the motion, and the fade was missing for every style.
- Section 4 item 6: the prefix is also applied to the system title in iTerm2 (`sessionNameControllerUniqueIdentifier` is non-nil for it); Tandem's tmux sessions already show prefixed window names through their own label paths, so only the custom-title case is added (13s-u rulings: the custom-title prefix is 13u's, the built-in title prefix case goes to 13y; R13xy-1).
- `02:406`: Undo selects Edit > Undo, which is disabled while a terminal has focus; the action does something only in a text field (a toolbelt tool), else beeps.

### Decisions (each goes in `m13u-rulings.txt`)

- R7 (ruled): shell-owned Win chords (`winkeys::shell_owned`) are Windows' "symbolic hotkeys"; with the option on they go to Windows unless a Tandem menu command (including a profile Shortcut, a Profiles menu item) owns them. Cost: a user mapping on Win+E needs the option off.
- The forced layout's id is the HKL's two 16-bit words as 8 hex digits. Cost: the id differs from a registry KLID for some IMEs, but round-trips on the same machine.
- Remapping off rebuilds typed text with `ToUnicodeEx` and the remapped modifiers. Cost: dead keys pressed with a remapped modifier may compose differently.
- Fades use a layered window only while they run, only when Task 9's probe passed; a failure or present error shows the window at once and turns fades off for the run. Cost: no fade on systems where layered flip-model windows fail.
- "Right Option Key Sends" is shown on Windows as "Alt Key Sends (Right)". Cost: on AltGr layouts Windows reports Right Alt as Ctrl+Alt, so the setting only affects layouts where Right Alt is plain Alt.
- Non-edge fades are linear (NSAnimationContext's default); edge slides and their fades ease in. Cost: slightly different pacing if AppKit's default is not linear.
- Session hotkeys use ids from 0x1000 in the existing hotkey registration (no new hook). Cost: none.
- The tmux filler appears only while tmux's window is smaller than the tab; Tandem still reports the whole area. Cost: a brief filler until tmux resizes.
- The custom-title prefix supersedes `m11c-rulings.txt:157`. Cost: none (iTerm2's behavior).
- Undo's beep uses `find_ui::beep`. Cost: none.
- Parity rows are found by title.

### Pitfalls for the implementer

- The 1036 patch must not touch any other private mode; take the next free `TermMode` bit and keep `TermMode::ANY` definitions intact.
- `effective` must be used at every place that reads Option Key Sends (key encoding, key bindings' Alt handling), not just the byte encoder.
- `set_remap_global` changes a flag the hook thread reads: use an atomic, never a lock taken in the hook.
- `ActivateKeyboardLayout` affects the calling thread's input: call it on the UI thread (winit's), not the hook thread.
- Session hotkey ids must be rebuilt with `register_hotkeys` whenever a session's hotkey changes or a pane with one closes.
- tmux `@hotkeys` is written only for live panes (`list-panes` first, as `TC:2207-2211`), or the option grows forever.
- A tmux tab drawn at natural size must keep hit tests, find bars and title bars inside the drawn pane rects, not the whole area.
- Nothing here is per window; `swap_live` is untouched.
