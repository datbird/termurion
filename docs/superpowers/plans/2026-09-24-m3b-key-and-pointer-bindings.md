# Tandem Milestone 3b: Key and Pointer Bindings

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's key bindings (global and per profile, all 77 action codes, the Leader key, presets), its pointer bindings (all 32 actions and the Pointer settings), modifier remapping, navigation number shortcuts, and the profile Keys options, stored in iTerm2's own formats so an imported iTerm2 setup works as is.

**Architecture:** A global settings store (`globals.rs`, `%APPDATA%\Tandem\settings.toml`) joins the profile store. Its schema is `schema/global.toml`. Its dictionary defaults and the key presets come from iTerm2's plists through `scripts/gen-keymaps.py` into `schema/keymaps.toml`. Pure modules do every decision: `keystroke.rs` (iTerm2's keystroke keys), `keyaction.rs` (action codes, text escaping, status), `pointer.rs` (pointer keys and the click rules), `remap.rs` (modifier remapping), `keyroute.rs` (what one key press does). The app runs the decisions; the Win-key hook remaps modifiers with `SendInput`. The settings window gets Keys and Pointer pages and a Key Mappings editor on the profile Keys tab.

**Tech Stack:** as milestone 3a. No new crates.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md` (section 3.4: Cmd is Win, Option is Alt). Inventory: `docs/parity/02-global-settings.md` (Keys, Pointer), `docs/parity/01-profile-settings.md` (Keys). iTerm2 facts: the research notes in this plan's ledger setup lines, with source paths.

## Global Constraints

- Everything from milestones 0 to 3a still holds.
- Storage uses iTerm2's own keys and value shapes: `GlobalKeyMap`, `Keyboard Map`, `Leader`, `PointerActions`, `LeftControl`..`FunctionKey`, `SwitchTabModifier`, `SwitchPaneModifier`, `SwitchWindowModifier`, `UseVirtualKeyCodesForDetectingDigits`, `LanguageAgnosticKeyBindings`, `RemapModifiersGlobally`, `CommandSelection`, `PassOnControlClick`, `ReportRightClick`, `OptionClickMovesCursor`, `ThreeFingerEmulates`, `FocusFollowsMouse`, `FocusOnRightOrMiddleClick`, `ReportHorizontalScrollEvents`. A binding value is a table with `Action` (integer for keys, string for pointer), `Text` or `Argument`, and optional `Label`, `Version`, `Escaping`, `Apply Mode`.
- Modifier flags use macOS values: Shift 0x20000, Control 0x40000, Option 0x80000, Command 0x100000, NumericPad 0x200000, Leader 0x1000000. Cmd is Win, Option is Alt.
- Defaults come from iTerm2's files, never typed by hand: `DefaultGlobalKeyMap.plist`, `DefaultPointerActions.plist`, `PresetKeyMappings.plist`, `iTermPreferences.m` defaults.
- The profile map wins over the global map. A key mapping wins over a menu chord.
- An action owned by a later milestone can be saved and shown, marked "(milestone N)". Pressing it does nothing but show a banner that names the milestone. It never half-works.
- Remapping works only while a Tandem window is in front (iTerm2 does the same). A remapped key that is down when focus leaves is still released.

## Review Focus

1. A remapped modifier held while focus moves to another app, then released. Expect: no stuck key in the other app. Test: Task 5 `a_remapped_key_is_released_even_after_focus_leaves`.
2. AltGr on a European layout while Left Control is remapped. Expect: AltGr still types its character (Windows sends a fake Left Control with scan code 0x21D). Test: Task 5 `altgr_fake_control_is_never_remapped`.
3. An imported binding stored in the 3-part form (`0x61-0x100000-0x0`) or the 2-part form without `0x`. Expect: it matches the same key press. Test: Task 2 `a_three_part_key_matches_a_legacy_press` and `parses_the_short_legacy_form`.
4. A pointer binding on the right button while a mouse-reporting app runs (vim with `mouse=a`). Expect: the binding still runs unless "Right click reported to apps" is on. Test: Task 4 `right_click_binding_beats_reporting_unless_bypassed`.
5. A Win chord that a profile maps but no menu item uses (Win+J, for example). Expect: the hook takes it and the mapping runs; an unmapped Win+J still goes to Windows. Test: Task 6 `a_mapped_win_chord_is_bound_for_the_hook`.

---

### Task 1: Global settings store, schema, and key map data

**Files:**
- Create: `scripts/gen-keymaps.py`, `schema/keymaps.toml` (generated), `schema/global.toml`, `src/globals.rs`
- Modify: `src/schema.rs` (load `schema/global.toml` too), `src/import.rs` (global keys from the iTerm2 plist), `src/lib.rs`

**Interfaces:**
- Produces:
  - `schema::global() -> &'static [Setting]`, `schema::find_global(key) -> Option<&'static Setting>`
  - `globals::Globals { values: BTreeMap<String, Value> }` with `load(path) -> Result<Globals, String>`, `save(&self) -> Result<(), String>`, `get(&self, key) -> Option<Value>` (stored value, else schema default), `set(&mut self, key, Value)` (a value equal to the default is removed), `bool/int(&self, key)`, `table(&self, key) -> toml::Table`, `default_path() -> PathBuf` (`%APPDATA%\Tandem\settings.toml`)
  - `keymaps::factory_global() -> &'static toml::Table`, `keymaps::factory_pointer() -> &'static toml::Table`, `keymaps::presets() -> &'static [(String, toml::Table)]` (in `src/globals.rs` as `pub mod keymaps` or a sibling module; one file is fine)
  - `import::globals_from_file(path) -> Result<Vec<(String, Value)>, String>` (only keys in the global schema)

- [ ] **Step 1: Generator.** `scripts/gen-keymaps.py [ref]` reads `plists/DefaultGlobalKeyMap.plist`, `plists/DefaultPointerActions.plist`, `plists/PresetKeyMappings.plist` with `plistlib` and writes `schema/keymaps.toml`:

```toml
# Generated by scripts/gen-keymaps.py from iTerm2's plists. Do not edit by hand.
[global."0x19-0x60000"]
Action = 39
Text = ""
[pointer."Button,1,1,,"]
Action = "kContextMenuPointerAction"
[preset."Natural Text Editing"."0x7f-0x100000"]
Action = 11
Text = "0x15"
```

Run: `python3 scripts/gen-keymaps.py && grep -c '^\[' schema/keymaps.toml`. Expected: 14 global + 6 pointer + 48 preset tables = 68.

- [ ] **Step 2: Global schema.** `schema/global.toml` uses the profile schema's `[[setting]]` shape. Tabs `Keys` and `Pointer`. Every default is copied from `iTermPreferences.m` lines 848-888 (research notes). Rows:
  - Keys / Key Bindings: `GlobalKeyMap` dict (default from keymaps.toml at runtime, schema default `{}` with `default_note`), `Leader` string "", `LanguageAgnosticKeyBindings` bool false.
  - Keys / Navigation Shortcuts: `SwitchTabModifier` enum 4, `SwitchPaneModifier` enum 9, `SwitchWindowModifier` enum 6; choices `9 No Shortcut, 5 Alt+Number, 4 Win+Number, 6 Win+Alt+Number, 1 Ctrl+Number`. `UseVirtualKeyCodesForDetectingDigits` bool false, label "Emulate U.S. Keyboard".
  - Keys / Remap Modifiers: `LeftControl` 11, `RightControl` 12, `LeftOption` 2, `RightOption` 3, `LeftCommand` 7, `RightCommand` 8, `FunctionKey` 10 (`macos_only = true`), `RemapModifiersGlobally` bool true. Remap choices: `11 Left Control, 12 Right Control, 2 Left Alt, 3 Right Alt, 7 Left Win, 8 Right Win, 9 None`. Labels say Alt and Win for Option and Command.
  - Pointer / General: `CommandSelection` true (label "Win-click opens filename/URL"), `PassOnControlClick` false, `ReportRightClick` false, `OptionClickMovesCursor` true (label "Alt-click moves cursor"), `ThreeFingerEmulates` false (`macos_only`), `FocusFollowsMouse` false, `FocusOnRightOrMiddleClick` false, `ReportHorizontalScrollEvents` true.
  - Pointer / Bindings: `PointerActions` dict.

- [ ] **Step 3: Failing tests** in `src/globals.rs`:

```rust
#[test]
fn defaults_come_from_iterm2() {
    let g = Globals::empty(&scratch("defaults"));
    assert_eq!(g.int("SwitchTabModifier"), 4);
    assert_eq!(g.int("SwitchWindowModifier"), 6);
    assert!(g.bool("RemapModifiersGlobally") && g.bool("ReportHorizontalScrollEvents"));
    assert_eq!(g.table("GlobalKeyMap")["0x9-0x40000"]["Action"].as_integer(), Some(32));
    assert_eq!(g.table("PointerActions")["Button,2,1,,"]["Action"].as_str(), Some("kPasteFromClipboardPointerAction"));
}

#[test]
fn saves_only_changes_and_round_trips() {
    let path = scratch("round");
    let mut g = Globals::empty(&path);
    g.set("FocusFollowsMouse", Value::Boolean(true));
    g.set("SwitchTabModifier", Value::Integer(4));
    g.save().unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("FocusFollowsMouse") && !text.contains("SwitchTabModifier"), "{text}");
    assert!(Globals::load(&path).unwrap().bool("FocusFollowsMouse"));
}

#[test]
fn a_corrupt_file_is_an_error_not_a_reset() {
    let path = scratch("corrupt");
    std::fs::write(&path, "not = [toml").unwrap();
    assert!(Globals::load(&path).is_err());
}

#[test]
fn presets_are_the_three_iterm2_ships() {
    let names: Vec<&str> = keymaps::presets().iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["Natural Text Editing", "Simulate Numlock", "Terminal.app Compatibility"]);
    assert_eq!(keymaps::presets()[0].1.len(), 8);
}
```

And in `src/import.rs`: `imports_global_keys_only` builds a plist in a temp file with `GlobalKeyMap`, `LeftCommand`, and an unrelated `NSWindow Frame` key, and expects exactly the first two back.

Run: `scripts/xt test --lib globals:: import::`. Expected: FAIL (module missing).

- [ ] **Step 4: Implement.** `Globals` mirrors `Store`: TOML file with one `[settings]` table, `NotFound` gives an empty store, a parse error is an `Err`. `get` falls back to `keymaps::factory_global()` for `GlobalKeyMap` and `factory_pointer()` for `PointerActions`, else the schema default. `schema.rs` gains a second `OnceLock` for `schema/global.toml` (`include_str!`).

- [ ] **Step 5: Run.** Expected: PASS. Commit: `Add the global settings store and iTerm2's key map data`.

---

### Task 2: Keystrokes

**Files:** Create `src/keystroke.rs`; modify `src/lib.rs`.

**Interfaces:**
- Produces (in `tandem::keystroke`):
  - flag consts `SHIFT, CONTROL, OPTION, COMMAND, NUMPAD, LEADER` (u64)
  - `#[derive(Clone, Debug, PartialEq, Eq)] pub struct Keystroke { pub ch: u32, pub flags: u64, pub keycode: Option<u16>, pub modified: u32, pub char_is_modified: bool }`
  - `Keystroke::new(ch, flags, keycode, modified) -> Keystroke` (masks flags to the six kept bits, adds NUMPAD for 0xF700..=0xF703)
  - `parse(s: &str) -> Option<Keystroke>`, `serialized(&self) -> String`, `legacy(&self) -> String`, `modified_key(&self) -> String`, `portable(&self) -> String`
  - `find<'a>(&self, map: &'a toml::Table, language_agnostic: bool) -> Option<&'a str>` (iTerm2's lookup order, research section 1)
  - `same_key(&self, other) -> bool` (character and flags only, iTerm2 `isEqual`)
  - `label(&self) -> String` ("Win+Shift+Left", "Ctrl+Tab", "Leader, Win+A", "Keypad 5")
  - `mac_char(key: &crate::keys::Key, shift: bool, keypad_enter: bool) -> u32` (Enter 0x0D, keypad Enter 0x03, Tab 0x09, Shift+Tab 0x19, Backspace 0x7F, Escape 0x1B, Up..Right 0xF700..0xF703, F(n) 0xF703+n, Insert 0xF727, Delete 0xF728, Home 0xF729, End 0xF72B, PageUp 0xF72C, PageDown 0xF72D, text: its first char)
  - `mac_keycode(code: winit::keyboard::KeyCode) -> Option<u16>` (kVK table: ANSI letters and digits, punctuation, keypad, arrows, F1-F20, Return 0x24, Tab 0x30, Space 0x31, Delete 0x33, Escape 0x35, ForwardDelete 0x75, Home 0x73, End 0x77, PageUp 0x74, PageDown 0x79, Help/Insert 0x72)
  - `from_mods(m: crate::mouse::Mods) -> u64`

- [ ] **Step 1: Failing tests:**

```rust
#[test]
fn parses_every_stored_form() {
    let k = parse("0xf700-0x260000").unwrap();
    assert_eq!((k.ch, k.flags, k.keycode), (0xF700, 0x260000, None));
    let k = parse("0x61-0x100000-0x0").unwrap();
    assert_eq!((k.ch, k.flags, k.keycode), (0x61, COMMAND, Some(0)));
    let k = parse(":0x21:0x40000").unwrap();
    assert!(k.char_is_modified && k.ch == 0x21 && k.flags == CONTROL);
}

#[test]
fn parses_the_short_legacy_form() {
    // Natural Text Editing stores "f702-0x280000" with no 0x on the character.
    let k = parse("f702-0x280000").unwrap();
    assert_eq!((k.ch, k.flags), (0xF702, OPTION | NUMPAD));
}

#[test]
fn arrows_always_carry_numpad() {
    let k = Keystroke::new(0xF702, OPTION, None, 0xF702);
    assert_eq!(k.legacy(), "0xf702-0x280000");
}

#[test]
fn serializes_like_iterm2() {
    let k = Keystroke::new(0x61, COMMAND | SHIFT, Some(0), 0x41);
    assert_eq!(k.serialized(), "0x61-0x120000-0x0");
    assert_eq!(k.legacy(), "0x61-0x120000");
    assert_eq!(k.modified_key(), ":0x41:0x100000");
}

#[test]
fn a_three_part_key_matches_a_legacy_press() {
    let mut map = toml::Table::new();
    map.insert("0x61-0x100000-0x0".into(), toml::Value::Table(toml::Table::new()));
    let press = Keystroke::new(0x61, COMMAND, None, 0x61);
    assert_eq!(press.find(&map, false), Some("0x61-0x100000-0x0"));
}

#[test]
fn language_agnostic_matches_the_physical_key() {
    let mut map = toml::Table::new();
    map.insert("0x71-0x100000-0xc".into(), toml::Value::Table(toml::Table::new())); // Cmd+Q, kVK_ANSI_Q
    let azerty_a = Keystroke::new(0x61, COMMAND, Some(0x0C), 0x61); // A on the Q key
    assert_eq!(azerty_a.find(&map, true), Some("0x71-0x100000-0xc"));
    assert_eq!(azerty_a.find(&map, false), None);
}

#[test]
fn labels_use_windows_names() {
    assert_eq!(parse("0xf702-0x320000").unwrap().label(), "Win+Shift+Left");
    assert_eq!(parse("0x9-0x40000").unwrap().label(), "Ctrl+Tab");
    assert_eq!(parse("0x61-0x1100000").unwrap().label(), "Leader, Win+A");
}

#[test]
fn shift_tab_is_backtab() {
    assert_eq!(mac_char(&crate::keys::Key::Tab, true, false), 0x19);
}
```

Run: `scripts/xt test --lib keystroke::`. Expected: FAIL.

- [ ] **Step 2: Implement** per the research notes (`iTermKeystroke.m` 231-368). `parse`: a leading `:` is the modified form; otherwise split on `-` into 2 or 3 hex parts, each with an optional `0x`. `find` without language-agnostic: serialized, legacy, modified, then any key whose parsed legacy form equals ours. With it and a keycode: modified, then the first key with the same portable form, then legacy.

- [ ] **Step 3: Run.** Expected: PASS. Commit: `Add iTerm2 keystrokes: parse, serialize, match, and label`.

---

### Task 3: Key actions

**Files:** Create `src/keyaction.rs`; modify `scripts/gen-keymaps.py` (action names), `schema/keymaps.toml`, `src/lib.rs`.

**Interfaces:**
- Produces (in `tandem::keyaction`):
  - `pub struct Binding { pub action: i64, pub text: String, pub label: String, pub version: i64, pub escaping: Option<i64> }` with `from_value(&toml::Value) -> Option<Binding>`, `to_value(&self) -> toml::Value`
  - `#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum Escaping { None, Compatibility, Common, Vim, VimAndCompatibility }`, `Binding::escaping(&self) -> Escaping` (version 0 or missing: Compatibility; 1: Common; 2: the `Escaping` field)
  - `expand(text: &str, e: Escaping) -> String`, `hex_bytes(text: &str) -> Vec<u8>`
  - `pub struct ActionInfo { pub code: i64, pub name: &'static str, pub arg: Arg }`, `pub enum Arg { None, Text, Hex, Escape, Profile, MenuItem, Sequence, Setting, Other }`, `pub fn actions() -> &'static [ActionInfo]` (77 entries, -1 to 76, iTerm2's base names from `displayName`, generated into keymaps.toml `[action_names]`)
  - `pub fn status(code: i64) -> crate::commands::Status`
  - `sequence(text: &str) -> Vec<Binding>` (JSON array of `{"action", "parameter", ...}` dictionaries, research section 2 row 66; read the exact field names from `iTermKeyBindingAction.m:712-731`)

- [ ] **Step 1: Failing tests:**

```rust
#[test]
fn every_action_code_has_a_name() {
    assert_eq!(actions().len(), 78); // -1 through 76
    assert_eq!(actions().iter().find(|a| a.code == 32).unwrap().name, "Cycle Tabs Forward");
}

#[test]
fn escaping_follows_the_version() {
    let b = |v: i64, e: Option<i64>| Binding { action: 12, text: String::new(), label: String::new(), version: v, escaping: e };
    assert_eq!(b(0, None).escaping(), Escaping::Compatibility);
    assert_eq!(b(1, None).escaping(), Escaping::Common);
    assert_eq!(b(2, Some(3)).escaping(), Escaping::Vim);
}

#[test]
fn expands_each_escaping_mode() {
    assert_eq!(expand(r"a\nb\x41", Escaping::Common), "a\nbA");
    assert_eq!(expand(r"a\x41", Escaping::Compatibility), r"a\x41");
    assert_eq!(expand(r"\e\t", Escaping::Compatibility), "\x1b\t");
    assert_eq!(expand(r"\<C-a>\101", Escaping::Vim), "\x01A");
    assert_eq!(expand(r"\n", Escaping::None), r"\n");
}

#[test]
fn hex_codes_become_bytes() {
    assert_eq!(hex_bytes("0x1b 0x7f"), [0x1b, 0x7f]);
    assert_eq!(hex_bytes("15"), [0x15]);
}

#[test]
fn statuses_split_now_from_later() {
    use crate::commands::Status;
    for code in [0, 2, 4, 9, 10, 11, 12, 13, 16, 17, 18, 21, 23, 25, 27, 28, 29, 30, 31, 32, 33, 34, 37, 38, 39, 49, 52, 53, 56, 59, 61, 66, 67, 68, 69, 70, 74, 75, 76] {
        assert_eq!(status(code), Status::Done, "{code}");
    }
    assert_eq!(status(15), Status::Later(10)); // Instant Replay
    assert_eq!(status(65), Status::Later(8)); // tmux
    assert_eq!(status(1), Status::Later(7)); // windows
}

#[test]
fn reads_a_stored_binding() {
    let v: toml::Value = toml::from_str("Action = 11\nText = \"0x15\"").unwrap();
    let b = Binding::from_value(&v).unwrap();
    assert_eq!((b.action, b.text.as_str(), b.version), (11, "0x15", 0));
}
```

Owners of the later actions (a ledger ruling records the list): 1, 3, 26, 43, 44, 62 -> 7; 15 -> 10; 35, 71, 72 -> 6; 36, 45-48, 57, 58, 73 -> 4; 40 -> 13 (color presets); 41, 42, 63, 64 -> 9; 60 -> 11; 65 -> 8. Codes 14 (deprecated, does nothing), 22, 24 (remap markers) are Done. Code -1 is Done (does nothing).

Run: `scripts/xt test --lib keyaction::`. Expected: FAIL.

- [ ] **Step 2: Implement.** Port `stringByExpandingVimSpecialCharacters` (NSStringITerm.m 1420-1456), the Common escapes (754-763, 690-725), and the Compatibility set (`\n \e \a \t`). Names: the generator reads `iTermKeyBindingAction.m` `displayName`'s default strings; for a format string (`Send Text: \"%@\"`) it keeps the text before the colon.

- [ ] **Step 3: Run.** Expected: PASS. Commit: `Add iTerm2's key binding actions, text escaping, and their status`.

---

### Task 4: Pointer bindings

**Files:** Create `src/pointer.rs`; modify `src/lib.rs`, `src/mouse.rs` (horizontal wheel buttons).

**Interfaces:**
- Produces (in `tandem::pointer`):
  - `pub fn key(button: u8, clicks: u8, m: Mods) -> String` ("Button,1,1,,", mods string in the order c o s ^)
  - `pub struct PointerBinding { pub action: String, pub argument: String, pub version: i64 }` with `from_value`, `to_value`
  - `pub struct Prefs { pub pass_on_control_click: bool, pub report_right_click: bool }`
  - `pub fn emulates_right_click(button: u8, clicks: u8, m: Mods, reportable: bool, p: &Prefs) -> bool`
  - `pub fn lookup(map: &toml::Table, button: u8, clicks: u8, m: Mods, reportable: bool, p: &Prefs) -> Option<PointerBinding>` (`PointerController.m actionForEvent`)
  - `pub enum Down { Bound, Report, Native }`, `pub enum Up { Perform(PointerBinding), Report, Native }`
  - `pub fn on_down(button: u8, clicks: u8, m: Mods, reportable: bool, map, p) -> Down`, `pub fn on_up(...) -> Up` (the PTYMouseHandler rules below)
  - `pub struct Clicks` (per-button count: resets when the button changes, capped by the OS double-click count)
  - `pub const ACTIONS: [(&str, &str, Arg); 32]` (constant, display name, argument kind) and `pub fn status(action: &str) -> commands::Status`
- Mouse flow (from `PTYMouseHandler.m`), which `on_down` and `on_up` encode:
  - Left: only a Ctrl-only single click that emulates a right click uses a binding; it fires on release and starts no selection.
  - Right down: a binding exists -> Bound; else reportable -> Report; else Native (the context menu). Right up: unless `report_right_click && reportable`, a binding -> Perform; else reportable -> Report; else Native.
  - Middle and other buttons: down -> Report if reportable (a binding is only noted). Up: reportable -> Report and stop; else a binding -> Perform.
  - Button numbers: 0 left, 1 right, 2 middle, 3+ others.
- Pointer action owners: OpenTarget, OpenTargetInBackground, SmartSelection, SmartSelectionIgnoringNewlines, CopyLinkAddress, QuickLook -> 4; NextWindow, PrevWindow, NewWindowWithProfile, MovePane -> 7; InvokeScriptFunction -> 11; both paste actions carry a paste-special config -> Partial("plain paste until milestone 9") when the argument is not empty, else Done. The rest are Done.

- [ ] **Step 1: Failing tests:**

```rust
fn defaults() -> toml::Table { crate::globals::keymaps::factory_pointer().clone() }
const NONE: Mods = Mods { shift: false, alt: false, ctrl: false, cmd: false };
const CTRL: Mods = Mods { ctrl: true, ..NONE };
const P: Prefs = Prefs { pass_on_control_click: false, report_right_click: false };

#[test]
fn keys_match_iterm2() {
    assert_eq!(key(1, 1, NONE), "Button,1,1,,");
    assert_eq!(key(0, 2, Mods { cmd: true, shift: true, ..NONE }), "Button,0,2,cs,");
}

#[test]
fn ctrl_click_emulates_a_right_click() {
    assert!(emulates_right_click(0, 1, CTRL, false, &P));
    assert!(!emulates_right_click(0, 1, Mods { shift: true, ..CTRL }, false, &P));
    let bypass = Prefs { pass_on_control_click: true, ..P };
    assert!(!emulates_right_click(0, 1, CTRL, true, &bypass));
    assert_eq!(lookup(&defaults(), 0, 1, CTRL, false, &P).unwrap().action, "kContextMenuPointerAction");
}

#[test]
fn right_click_binding_beats_reporting_unless_bypassed() {
    assert!(matches!(on_down(1, 1, NONE, true, &defaults(), &P), Down::Bound));
    assert!(matches!(on_up(1, 1, NONE, true, &defaults(), &P), Up::Perform(_)));
    let bypass = Prefs { report_right_click: true, ..P };
    assert!(matches!(on_up(1, 1, NONE, true, &defaults(), &bypass), Up::Report));
}

#[test]
fn middle_click_goes_to_a_tracking_app_first() {
    assert!(matches!(on_up(2, 1, NONE, true, &defaults(), &P), Up::Report));
    assert!(matches!(on_up(2, 1, NONE, false, &defaults(), &P), Up::Perform(b) if b.action == "kPasteFromClipboardPointerAction"));
}

#[test]
fn clicks_reset_when_the_button_changes() {
    let mut c = Clicks::default();
    assert_eq!(c.press(0, 1), 1);
    assert_eq!(c.press(0, 2), 2);
    assert_eq!(c.press(1, 3), 1);
}
```

Run: `scripts/xt test --lib pointer::`. Expected: FAIL.

- [ ] **Step 2: Implement.** Add `Button::WheelLeft` and `Button::WheelRight` to `mouse.rs` with codes 66 and 67 (and a test `horizontal_wheel_reports_66_and_67`).

- [ ] **Step 3: Run.** Expected: PASS. Commit: `Add iTerm2 pointer bindings and its click rules`.

---

### Task 5: Modifier remapping

**Files:** Create `src/remap.rs`; modify `src/winkeys.rs`, `src/lib.rs`.

**Interfaces:**
- Produces (in `tandem::remap`, pure):
  - `pub struct Tags { pub left_control: i64, pub right_control: i64, pub left_option: i64, pub right_option: i64, pub left_command: i64, pub right_command: i64 }` with `from_globals(&Globals)`, `identity()`
  - `pub fn targets(tag: i64) -> Vec<u16>` (virtual keys: 11 -> [0xA2], 12 -> [0xA3], 1 -> [0xA3], 2 -> [0xA4], 3 -> [0xA5], 5 -> [0xA4], 7 -> [0x5B], 4 -> [0x5B], 8 -> [0x5C], 6 -> [0x5B, 0xA4], 9 and 10 -> [])
  - `pub enum Verdict { Pass, Replace(Vec<(u16, bool)>) }` (keys to inject, key and down)
  - `#[derive(Default)] pub struct Remapper` with `on_key(&mut self, vk: u16, scan: u32, down: bool, focused: bool, tags: &Tags) -> Verdict`
- Rules:
  - Only the six side-specific modifier keys (0xA2 0xA3 0xA4 0xA5 0x5B 0x5C) are remapped, and only when their tag differs from their own.
  - A key down while not focused passes. A key up for a key the remapper replaced always releases its targets, focused or not.
  - A Left Control with scan code 0x21D is AltGr's fake Control: always Pass.
  - Injected events carry `winkeys::MAGIC` and the hook passes them through, so the remapper never sees its own output.
  - `DO_NOT_REMAP` (action 22) in the global map: when a non-modifier key goes down while a remap is active and the physical chord maps to 22, the hook injects: target ups, physical modifier downs, the key down; and on that key's up: key up, physical modifier ups, target downs. `Remapper::exempt_down/exempt_up` return these lists.
- `winkeys::install` gains a `tags: Arc<RwLock<Tags>>` and a `exempt: Box<dyn Fn(u16, Mods) -> bool>`. The hook runs the remapper before the Win gate. `Replace` means `SendInput` the list with `dwExtraInfo = MAGIC` and swallow the event.

- [ ] **Step 1: Failing tests:**

```rust
fn swapped() -> Tags { Tags { left_control: 7, left_command: 11, ..Tags::identity() } }

#[test]
fn an_identity_map_passes_everything() {
    let mut r = Remapper::default();
    assert_eq!(r.on_key(0xA2, 0x1D, true, true, &Tags::identity()), Verdict::Pass);
}

#[test]
fn left_control_becomes_left_win() {
    let mut r = Remapper::default();
    assert_eq!(r.on_key(0xA2, 0x1D, true, true, &swapped()), Verdict::Replace(vec![(0x5B, true)]));
    assert_eq!(r.on_key(0xA2, 0x1D, false, true, &swapped()), Verdict::Replace(vec![(0x5B, false)]));
}

#[test]
fn a_remapped_key_is_released_even_after_focus_leaves() {
    let mut r = Remapper::default();
    r.on_key(0x5B, 0x5B, true, true, &swapped());
    assert_eq!(r.on_key(0x5B, 0x5B, false, false, &swapped()), Verdict::Replace(vec![(0xA2, false)]));
}

#[test]
fn nothing_is_remapped_while_another_app_is_in_front() {
    let mut r = Remapper::default();
    assert_eq!(r.on_key(0xA2, 0x1D, true, false, &swapped()), Verdict::Pass);
    assert_eq!(r.on_key(0xA2, 0x1D, false, false, &swapped()), Verdict::Pass);
}

#[test]
fn altgr_fake_control_is_never_remapped() {
    let mut r = Remapper::default();
    assert_eq!(r.on_key(0xA2, 0x21D, true, true, &swapped()), Verdict::Pass);
}

#[test]
fn none_swallows_the_key() {
    let mut r = Remapper::default();
    let t = Tags { right_option: 9, ..Tags::identity() };
    assert_eq!(r.on_key(0xA5, 0x38, true, true, &t), Verdict::Replace(vec![]));
}

#[test]
fn win_and_alt_target_presses_both() {
    let mut r = Remapper::default();
    let t = Tags { right_control: 6, ..Tags::identity() };
    assert_eq!(r.on_key(0xA3, 0x1D, true, true, &t), Verdict::Replace(vec![(0x5B, true), (0xA4, true)]));
}
```

Run: `scripts/xt test --lib remap::`. Expected: FAIL.

- [ ] **Step 2: Implement** `remap.rs`, then the hook wiring. Swallowed Win-down events never reach the Win gate, so no Start menu mask is needed for them.

- [ ] **Step 3: Run** `scripts/xt test --lib remap:: winkeys::`. Expected: PASS. Commit: `Remap modifiers while Tandem is in front, as iTerm2 does`.

---

### Task 6: Key routing

**Files:** Create `src/keyroute.rs`; modify `src/lib.rs`.

**Interfaces:**
- Consumes: `Keystroke`, `Binding`, `Globals`, `Profile`, `commands::command_for_chord`.
- Produces (in `tandem::keyroute`):
  - `pub struct Context<'a> { pub profile_map: &'a toml::Table, pub global_map: &'a toml::Table, pub language_agnostic: bool, pub leader: Option<Keystroke>, pub leader_on: bool, pub switch_tab: i64, pub switch_pane: i64, pub switch_window: i64, pub us_digits: bool }`
  - `pub struct Press { pub stroke: Keystroke, pub digit: Option<u8> }` (`digit` is the typed digit, or the physical digit key when `us_digits`)
  - `pub enum Route { LeaderOn, LeaderOff, SelectWindow(u8), SelectPane(u8), SelectTab(u8), Binding(Binding), Terminal }`
  - `pub fn route(ctx: &Context, p: &Press) -> Route`: iTerm2's order. 1: the leader chord toggles (`LeaderOn` or `LeaderOff`). 2: with the leader on, add `LEADER` to the flags for the rest. 3: window, pane, then tab number shortcuts (exact Shift/Ctrl/Alt/Win match to the tag's mask; digit 9 is "last"). 4: the profile map, then the global map. 5: `Terminal`.
  - `pub fn tag_mask(tag: i64) -> Option<u64>` (1 -> CONTROL, 4 -> COMMAND, 5 -> OPTION, 6 -> COMMAND|OPTION, 9 -> None, 10 -> None, other -> COMMAND|OPTION)
  - `pub struct Snapshot { ... }` built from the same inputs, and `pub fn win_chord_bound(s: &Snapshot, chord: &Chord) -> bool` for the hook: true when the leader is on, when the chord is a number shortcut, when either map has the chord (legacy form, Win in the flags), or when `command_for_chord` finds a working command.

- [ ] **Step 1: Failing tests:**

```rust
fn ctx<'a>(p: &'a toml::Table, g: &'a toml::Table) -> Context<'a> {
    Context { profile_map: p, global_map: g, language_agnostic: false, leader: None, leader_on: false,
              switch_tab: 4, switch_pane: 9, switch_window: 6, us_digits: false }
}
fn press(ch: u32, flags: u64) -> Press { Press { stroke: Keystroke::new(ch, flags, None, ch), digit: char::from_u32(ch).and_then(|c| c.to_digit(10)).map(|d| d as u8) } }
fn map(key: &str, action: i64) -> toml::Table { toml::from_str(&format!("\"{key}\" = {{ Action = {action}, Text = \"\" }}")).unwrap() }

#[test]
fn the_profile_map_wins_over_the_global_map() {
    let (p, g) = (map("0x61-0x100000", 13), map("0x61-0x100000", 0));
    assert!(matches!(route(&ctx(&p, &g), &press(0x61, COMMAND)), Route::Binding(b) if b.action == 13));
}

#[test]
fn win_number_selects_a_tab() {
    let e = toml::Table::new();
    assert!(matches!(route(&ctx(&e, &e), &press(0x33, COMMAND)), Route::SelectTab(3)));
    assert!(matches!(route(&ctx(&e, &e), &press(0x33, COMMAND | SHIFT)), Route::Terminal));
    assert!(matches!(route(&ctx(&e, &e), &press(0x33, COMMAND | OPTION)), Route::SelectWindow(3)));
}

#[test]
fn the_leader_marks_the_next_key() {
    let e = toml::Table::new();
    let g = map("0x61-0x1000000", 13);
    let mut c = ctx(&e, &g);
    c.leader = Some(Keystroke::new(0x62, CONTROL, None, 0x62));
    assert!(matches!(route(&c, &press(0x62, CONTROL)), Route::LeaderOn));
    c.leader_on = true;
    assert!(matches!(route(&c, &press(0x61, 0)), Route::Binding(b) if b.action == 13));
}

#[test]
fn a_mapped_win_chord_is_bound_for_the_hook() {
    let e = toml::Table::new();
    let j = map("0x6a-0x100000", 13);
    let chord = Chord { key: ChordKey::Char('j'), cmd: true, alt: false, shift: false, ctrl: false };
    assert!(win_chord_bound(&Snapshot::new(&ctx(&j, &e)), &chord));
    assert!(!win_chord_bound(&Snapshot::new(&ctx(&e, &e)), &chord));
}
```

Run: `scripts/xt test --lib keyroute::`. Expected: FAIL.

- [ ] **Step 2: Implement.** A Win chord from the hook becomes a `Press` with `COMMAND` set and the chord's key as the character (`Char(c)`: c, arrows and others through `mac_char`).

- [ ] **Step 3: Run.** Expected: PASS. Commit: `Route key presses the way iTerm2 does: leader, number shortcuts, mappings`.

---

### Task 7: Profile Keys options in the encoder

**Files:** Modify `src/keys.rs`.

**Interfaces:**
- Produces: `pub struct KeyOptions { pub left_option: i64, pub right_option: i64, pub option_is_alt: bool, pub app_keypad_allowed: bool }` with `from_profile(&Profile)` (Right Option missing -> Left's value; `Treat Option as Alt` default true), and `press_bytes_with(key, base, text, mods, alt_side: AltSide, app_cursor, app_keypad, keypad: bool, opts: &KeyOptions) -> Option<Vec<u8>>`. `pub enum AltSide { Left, Right }`.
- `pub fn movement_scroll(key: &Key, mods: Mods, alt_screen: bool, exited: bool) -> Option<Scroll>` (research section 7).
- Rules: Option Normal (0): a text key sends its text with no ESC. Meta (1): ORs 0x80 into the first byte. Esc+ (2): ESC first (the old behavior). AltGr text (Ctrl+Alt with different text) always wins. Function and cursor keys keep the xterm modifier parameter, with Alt counted as 2 when `option_is_alt`, else 8. With `app_keypad_allowed` and the terminal in application keypad mode, keypad keys send SS3 codes (0-9 -> `ESC O p`..`ESC O y`, `.` -> `ESC O n`, Enter -> `ESC O M`, `+` -> `ESC O k`, `-` -> `ESC O m`, `*` -> `ESC O j`, `/` -> `ESC O o`); read `VT100Output.m` keypad code first and follow it where it differs.

- [ ] **Step 1: Failing tests:**

```rust
const OPTS: KeyOptions = KeyOptions { left_option: 2, right_option: 2, option_is_alt: true, app_keypad_allowed: false };

#[test]
fn option_normal_sends_plain_text() {
    let o = KeyOptions { left_option: 0, ..OPTS };
    assert_eq!(press_bytes_with(&t("b"), Some(&t("b")), Some("b"), ALT, AltSide::Left, false, false, false, &o), Some(b"b".to_vec()));
}

#[test]
fn option_meta_sets_the_high_bit() {
    let o = KeyOptions { left_option: 1, ..OPTS };
    assert_eq!(press_bytes_with(&t("b"), Some(&t("b")), Some("b"), ALT, AltSide::Left, false, false, false, &o), Some(vec![b'b' | 0x80]));
}

#[test]
fn each_side_uses_its_own_setting() {
    let o = KeyOptions { left_option: 0, right_option: 2, ..OPTS };
    assert_eq!(press_bytes_with(&t("b"), Some(&t("b")), Some("b"), ALT, AltSide::Right, false, false, false, &o), Some(b"\x1bb".to_vec()));
}

#[test]
fn meta_style_option_changes_the_modifier_parameter() {
    let o = KeyOptions { option_is_alt: false, ..OPTS };
    assert_eq!(press_bytes_with(&Key::F(1), None, None, ALT, AltSide::Left, false, false, false, &o), Some(b"\x1b[1;9P".to_vec()));
    assert_eq!(press_bytes_with(&Key::F(1), None, None, ALT, AltSide::Left, false, false, false, &OPTS), Some(b"\x1b[1;3P".to_vec()));
}

#[test]
fn keypad_needs_permission_for_application_mode() {
    let allowed = KeyOptions { app_keypad_allowed: true, ..OPTS };
    assert_eq!(press_bytes_with(&t("5"), Some(&t("5")), Some("5"), NONE, AltSide::Left, false, true, true, &allowed), Some(b"\x1bOu".to_vec()));
    assert_eq!(press_bytes_with(&t("5"), Some(&t("5")), Some("5"), NONE, AltSide::Left, false, true, true, &OPTS), Some(b"5".to_vec()));
}

#[test]
fn movement_keys_scroll_only_outside_apps() {
    assert_eq!(movement_scroll(&Key::PageUp, NONE, false, false), Some(Scroll::PageUp));
    assert_eq!(movement_scroll(&Key::PageUp, NONE, true, false), None);
    assert_eq!(movement_scroll(&Key::Up, NONE, false, false), None);
    assert_eq!(movement_scroll(&Key::Up, NONE, false, true), Some(Scroll::Delta(1)));
}
```

Run: `scripts/xt test --lib keys::`. Expected: FAIL.

- [ ] **Step 2: Implement.** `press_bytes` stays as a wrapper with Esc+ on both sides, so existing tests keep passing. Fix the stale doc comment at the top of `keys.rs` ("Cmd is Ctrl+Shift") in the same commit.

- [ ] **Step 3: Run.** Expected: PASS. Commit: `Apply the profile Keys options: Option sends, Alt vs Meta, keypad, movement keys`.

---

### Task 8: Pane and tab operations

**Files:** Modify `src/layout.rs`; create `src/mru.rs`; modify `src/lib.rs`.

**Interfaces:**
- Produces:
  - `Layout::swap(&mut self, a: PaneId, b: PaneId) -> bool`
  - `Layout::grow(&mut self, pane: PaneId, axis: Axis, by: f32) -> bool` (positive grows the pane along `axis`; negative shrinks it; moves the nearest divider on the side that has one)
  - `Layout::next(&self, from: PaneId, forward: bool) -> Option<PaneId>` (in `panes()` order, wrapping)
  - `mru::Mru` with `touch(id: u64)`, `step(&mut self, forward: bool) -> Option<u64>` (while cycling, each step goes one deeper; the first step goes to the second entry), `commit(&mut self)` (the chosen tab moves to the front), `forget(id)`

- [ ] **Step 1: Failing tests** (`layout.rs`):

```rust
#[test]
fn swapping_panes_keeps_the_shape() {
    let mut l = Layout::new(1);
    l.split(1, Axis::Row, 2);
    let area = Rect { x: 0.0, y: 0.0, w: 100.0, h: 50.0 };
    let before: Vec<Rect> = l.rects(area, 0.0).into_iter().map(|(_, r)| r).collect();
    assert!(l.swap(1, 2));
    assert_eq!(l.panes(), [2, 1]);
    assert_eq!(l.rects(area, 0.0).into_iter().map(|(_, r)| r).collect::<Vec<_>>(), before);
}

#[test]
fn grow_moves_the_divider_toward_the_neighbor() {
    let mut l = Layout::new(1);
    l.split(1, Axis::Row, 2);
    let area = Rect { x: 0.0, y: 0.0, w: 100.0, h: 50.0 };
    assert!(l.grow(2, Axis::Row, 0.1));
    let w2 = l.rect_of(2, area, 0.0).unwrap().w;
    assert!((w2 - 60.0).abs() < 0.01, "{w2}");
    assert!(!l.grow(2, Axis::Column, 0.1));
}
```

`mru.rs`:

```rust
#[test]
fn cycling_walks_deeper_then_commits() {
    let mut m = Mru::default();
    for id in [1, 2, 3] { m.touch(id); } // 3 is most recent
    assert_eq!(m.step(true), Some(2));
    assert_eq!(m.step(true), Some(1));
    m.commit();
    assert_eq!(m.step(true), Some(3));
}
```

Run: `scripts/xt test --lib layout:: mru::`. Expected: FAIL.

- [ ] **Step 2: Implement.** Step 3: run, PASS. Commit: `Add pane swapping and growing, and most-recently-used tab order`.

---

### Task 9: Wire keys and pointer into the app

**Files:** Modify `src/app/mod.rs`, `src/app/menu.rs`; create `src/app/actions.rs`.

**Interfaces:**
- Consumes: Tasks 1-8.
- Produces: `App` holds `globals: Globals`, `remap_tags: Arc<RwLock<Tags>>`, `route_snapshot: Arc<RwLock<Snapshot>>`, `leader_on: bool`, `mru: Mru`, `Tab.id: u64`, `Pane.mouse_reporting_off: bool`, `pointer_clicks: pointer::Clicks`. `app/actions.rs` has `run_key_action(&mut self, b: &Binding, stroke: &Keystroke, fallback: Option<Vec<u8>>, event_loop)` and `run_pointer_action(&mut self, b: &PointerBinding, pane: PaneId, cell: (u32, u32), event_loop)`.
- Behavior:
  - `key()`: build the `Keystroke` (character from `mac_char` or the logical text, flags from the live modifiers, keycode from `mac_keycode(physical)`, `NUMPAD` for keypad keys), run `keyroute::route`, and act: leader on/off (a "Leader" banner slot shows while on), number shortcuts (window 1 is this window; other numbers do nothing until milestone 7), a binding through `run_key_action`, else movement-key scroll, else the encoder with `KeyOptions`.
  - `run_chord()` (Win chords from the hook): route first; a `Terminal` result falls through to the menu command.
  - `ModifiersChanged`: track the Alt side (winit `lalt_state`/`ralt_state`); when no modifier of an MRU cycle is still down, `mru.commit()`.
  - Each Done key action runs its iTerm2 behavior. Send actions scroll to the bottom and write to the pane. Copy or Send and Paste or Send use `fallback` (the key's own bytes). Bypass runs the menu command for the chord, if any, and sends nothing. Toggle Setting flips the named bool in `Globals` or the focused pane's profile and saves. Set Profile applies the palette, look, and font of the new profile to the pane. Sequence runs each item in order. Select Menu Item finds the item by identifier, else by title. A Later action shows the banner "<name> comes in milestone N".
  - Mouse: `mouse_press` and `mouse_release` use `pointer::on_down` / `on_up` for right, middle, and Ctrl-left. Right and middle clicks focus the pane only when `FocusOnRightOrMiddleClick` is on. `FocusFollowsMouse` focuses the pane under the pointer on motion. `OptionClickMovesCursor`: an Alt single left click with no reporting sends arrow keys to move the cursor to the clicked cell (horizontal only unless the alternate screen is on, as iTerm2 does without shell integration). Horizontal wheel movement goes to a tracking app as buttons 66 and 67 when `ReportHorizontalScrollEvents` is on.
  - The hook: `bound` reads `route_snapshot`; `tags` reads `remap_tags`; `exempt` checks the global map for action 22. The app rebuilds both snapshots after a settings change, a pane focus change, and a profile change.
  - Toggle Mouse Reporting (59) flips `Pane.mouse_reporting_off`; `mouse::app_owns_mouse` gets it as an argument.
- Pure helpers get tests here: `option_click_arrows(cursor: (u32, u32), target: (u32, u32), cols: u32, vertical_ok: bool, app_cursor: bool) -> Vec<u8>` (`option_click_moves_along_the_line`, `vertical_moves_only_in_the_alternate_screen`).

- [ ] **Step 1: Failing tests** for `option_click_arrows`:

```rust
#[test]
fn option_click_moves_along_the_line() {
    assert_eq!(option_click_arrows((2, 5), (5, 5), 80, false, false), b"\x1b[C\x1b[C\x1b[C");
    assert_eq!(option_click_arrows((5, 5), (3, 5), 80, false, true), b"\x1bOD\x1bOD");
}

#[test]
fn vertical_moves_only_in_the_alternate_screen() {
    // Without vertical moves, the cursor walks the wrapped text: 80 - 5 + 2 steps right.
    assert_eq!(option_click_arrows((5, 1), (2, 2), 80, false, false).len(), 77 * 3);
    assert_eq!(option_click_arrows((5, 1), (2, 2), 80, true, false), b"\x1b[D\x1b[D\x1b[D\x1b[B");
}
```

Run: `scripts/xt test --lib app::`. Expected: FAIL.

- [ ] **Step 2: Implement** the helpers, then the wiring. Step 3: run the whole suite `scripts/xt test`. Expected: all pass, no warnings. Build `scripts/xt build --release`. Commit: `Run key and pointer bindings, remapping, and number shortcuts in the app`.

---

### Task 10: Settings pages

**Files:** Create `src/settings_keys.rs` (binding editors); modify `src/settings_ui.rs`, `src/app/mod.rs` (recording hook flag, global save), `src/winkeys.rs` (take every Win chord while recording).

**Interfaces:**
- Produces: `SettingsWindow` gets a page row: Profiles, Keys, Pointer (working), General, Appearance, Arrangements, Advanced (disabled, hover text names the milestone). `Mode::Settings` opens on the page used last. `redraw(&mut self, store: &mut Store, globals: &mut Globals) -> Edit`, with `Edit::GlobalsChanged`.
- Keys page (iTerm2's Keys pane): the Key Bindings table (Keystroke | Action, sorted by label), +, -, Edit, and "Presets..." (Factory Defaults replaces the map); a Leader recorder; "Interpret key bindings based on physical key"; the three number-shortcut menus and Emulate U.S. Keyboard; the Remap Modifiers menus (Fn disabled, `macos_only`), "Remap modifiers globally", and "Reset to Defaults".
- Pointer page: the General checkboxes (Three-finger disabled, `macos_only`; Win-click disabled with "milestone 4"), and the Bindings table with +, -, Edit. The pointer editor has Button (Left, Right, Middle, Button 4 to 8), Clicks (single to quad), Win, Alt, Shift, Ctrl checkboxes, gestures listed but disabled, Action, and Argument.
- Profile Keys tab: after the generic rows, the Key Mappings table with +, -, Edit, a Presets menu (the three iTerm2 presets, Replace or Merge), and "Delete sends ^H", which adds or removes the binding `0x7f-0x0-0x33` = action 16.
- The key binding editor (shared by both tables): a "Click to record" button that captures the next key press (`SettingsWindow::event` takes the `KeyboardInput` while recording; Win chords arrive from the hook, which takes every Win chord while `RECORDING` is set); the Action menu grouped by iTerm2's categories, Later actions labelled "(milestone N)"; and the parameter editor for the action's `Arg`: text with an Escaping menu (Send Text), hex text, escape text, a profile menu, a menu item menu, a boolean setting menu (Toggle Setting), and a list editor for Run Sequence (add, remove, reorder; no nested sequences). New bindings save with `Version = 2` and `Escaping = 2`, as iTerm2's editor does.
- Pure helpers with tests (`settings_keys.rs`): `rows(map: &toml::Table) -> Vec<(String, String, String)>` (key, keystroke label, action label; sorted by label), `apply_preset(map, preset, replace: bool) -> toml::Table`, `set_delete_sends_ctrl_h(map, on: bool)`, `delete_sends_ctrl_h(map) -> bool`.

- [ ] **Step 1: Failing tests:**

```rust
#[test]
fn a_preset_can_replace_or_merge() {
    let mut map: toml::Table = toml::from_str("\"0x61-0x100000\" = { Action = 13 }").unwrap();
    let nat = &crate::globals::keymaps::presets()[0].1;
    assert_eq!(apply_preset(&map, nat, true).len(), 8);
    map = apply_preset(&map, nat, false);
    assert_eq!(map.len(), 9);
}

#[test]
fn delete_sends_ctrl_h_is_a_binding() {
    let mut map = toml::Table::new();
    set_delete_sends_ctrl_h(&mut map, true);
    assert_eq!(map["0x7f-0x0-0x33"]["Action"].as_integer(), Some(16));
    assert!(delete_sends_ctrl_h(&map));
    set_delete_sends_ctrl_h(&mut map, false);
    assert!(map.is_empty());
}

#[test]
fn rows_show_labels() {
    let map: toml::Table = toml::from_str("\"0x9-0x40000\" = { Action = 32, Text = \"\" }").unwrap();
    assert_eq!(rows(&map), [("0x9-0x40000".to_string(), "Ctrl+Tab".to_string(), "Cycle Tabs Forward".to_string())]);
}
```

Run: `scripts/xt test --lib settings_keys::`. Expected: FAIL.

- [ ] **Step 2: Implement** the helpers, then the pages. Step 3: run the whole suite. Expected: all pass. Commit: `Add the Keys and Pointer settings pages and the Key Mappings editor`.

---

### Task 11: Parity and acceptance

**Files:** Modify `docs/parity/02-global-settings.md` (Keys, Pointer), `docs/parity/01-profile-settings.md` (Keys), `docs/parity/00-roadmap.md` (counts, milestone 3 done, the stale "Cmd maps to Ctrl+Shift" line); create `docs/acceptance/m3b-checklist.md`.

- [ ] **Step 1:** Update each row this milestone closes to `have`, or to `partial` / `missing` with the milestone that owns the rest. Fix the stale key names in the inventory (the remap keys are `LeftControl`..`FunctionKey`; the pointer keys are `CommandSelection`, `PassOnControlClick`, `ReportRightClick`). Recount the table in `00-roadmap.md` with a script, not by hand.
- [ ] **Step 2:** Write the checklist: about 15 rows, each a thing Robert does on EXTREME and what he should see (import his iTerm2 settings and see his key bindings; Win+1..9 tab switching; Ctrl+Tab MRU cycling; Natural Text Editing preset with Alt+Left in zsh; Delete sends ^H; a Leader binding; a Left Control to Win remap with Ctrl+T opening a tab and AltGr still typing; right-click menu, middle-click paste, and both with vim `mouse=a`; Alt-click moves the cursor; Focus follows mouse; Send Text with `\n`; Run Sequence; Toggle Mouse Reporting).
- [ ] **Step 3:** Commit: `Update parity for key and pointer bindings and add the milestone 3b checklist`.
