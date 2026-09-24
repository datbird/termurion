# Tandem Milestone 3a: Commands, Menus, and Win as Cmd

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's whole menu bar on Windows, with every command at its exact iTerm2 chord (Cmd = Win), about 60 commands working now, the rest shown disabled until their milestone, plus iTerm2's right-click menu.

**Architecture:** `scripts/gen-menus.py` turns iTerm2's `MainMenu.xib` into `schema/menus.toml` (431 items: 276 commands, 111 separators). `menus.rs` loads it and turns each key equivalent into a Windows `Chord`. `winkeys.rs` is a low-level keyboard hook that takes Win chords while Tandem has focus and sends them to the app. `commands.rs` says which commands work now and which milestone owns the rest. `app.rs` builds the native menu bar with `muda`, runs commands by selector, and shows the right-click menu.

**Tech Stack:** as milestone 2, plus `muda` 0.20.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md` (section 3.4: Cmd is the Win key). Inventory: `docs/parity/03-features-and-menus.md` section 1. Milestone 3b (key and pointer bindings, modifier remapping) gets its own plan.

## Global Constraints

- Everything from milestones 0 to 2 still holds.
- A command's id is its iTerm2 selector (for example `splitVertically:`). A few selectors cover several items (`print:`, `setWindowStyle:`, `terminalStateToggleApplicationKeypad:`, `returnToDefaultSize:`, `toggleMultiLinePasteWarning:`, `paste:`, `performFindPanelAction:`, `terminalStateSetEmulationLevel:`); for those, the item's label decides.
- Chords come from the xib, never typed by hand. Cmd is Win, Option is Alt, Ctrl is Ctrl.
- When two items share a chord, the first in menu order wins, alternates last (iTerm2's own rule).
- Win+L is never taken (Windows forbids it). The Start menu must not open after a Win chord.
- Commands owned by a later milestone show disabled. None of them may do something wrong or partial.

## Review Focus

1. A Win chord while another app has focus. Expect: untouched; Windows handles it. Test: Task 2 hook logic, `only_the_focused_tandem_window_takes_win_chords`.
2. Win pressed and released alone. Expect: the Start menu opens as usual. After a Win chord Tandem used, the Start menu does not open. Test: Task 2 `start_menu_is_masked_only_after_a_taken_chord`.
3. A chord that needs Shift on a US keyboard (`Cmd+?`, `Cmd++`). Expect: it matches the physical keys Win+Shift+/ and Win+Shift+=. Test: Task 1 `shifted_punctuation_matches_its_key`.
4. Two menu items with one chord (Select Pane Above and Decrease Height are both Opt+Cmd+Up). Expect: the first in menu order wins. Test: Task 1 `the_first_item_in_menu_order_wins_a_shared_chord`.
5. Maximize Active Pane, then close that pane. Expect: the tab shows its other panes again, no blank tab. Test: Task 4 `closing_the_zoomed_pane_unzooms`.

---

### Task 1: Menu schema and chords

**Files:**
- Create: `src/menus.rs`
- Modify: `src/lib.rs` (add `pub mod menus;`)

**Interfaces:**
- Produces (in `tandem::menus`):
  - `#[derive(Clone, Debug, Deserialize)] pub struct Item { pub path: Vec<String>, pub selector: String, pub key: String, pub mods: Vec<String>, pub separator: bool, pub submenu: bool, pub alternate: bool }` with `label(&self) -> &str`, `id(&self) -> String` (path joined with " > "), `chord(&self) -> Option<Chord>`
  - `#[derive(Clone, Debug, PartialEq, Eq, Hash)] pub enum ChordKey { Char(char), Up, Down, Left, Right, Home, End, PageUp, PageDown, Delete, F(u8), Enter, Tab, Escape, Backspace, Space }`
  - `#[derive(Clone, Debug, PartialEq, Eq, Hash)] pub struct Chord { pub key: ChordKey, pub cmd: bool, pub alt: bool, pub shift: bool, pub ctrl: bool }`
  - `pub fn items() -> &'static [Item]`, `pub fn by_id(id: &str) -> Option<&'static Item>`, `pub fn command_for(chord: &Chord) -> Option<&'static Item>`, `pub fn chord_label(c: &Chord) -> String`

- [ ] **Step 1: Write the failing tests**

`src/menus.rs`:

```rust
//! iTerm2's menus, from schema/menus.toml (generated from MainMenu.xib), and the
//! Windows chord for each item. Cmd is the Win key and Option is Alt (spec 3.4).

#[cfg(test)]
mod tests {
    use super::*;

    fn item(selector: &str) -> &'static Item {
        items().iter().find(|i| i.selector == selector).unwrap_or_else(|| panic!("{selector}"))
    }

    fn win(key: ChordKey) -> Chord {
        Chord { key, cmd: true, alt: false, shift: false, ctrl: false }
    }

    #[test]
    fn loads_the_whole_menu() {
        assert!(items().len() >= 431, "{}", items().len());
        let top: Vec<&str> = items().iter().filter(|i| i.path.len() == 1).map(|i| i.label()).collect();
        assert_eq!(top, ["iTerm2", "Shell", "Edit", "View", "Session", "Scripts", "Profiles", "Window", "Help"]);
    }

    #[test]
    fn split_chords_match_iterm2() {
        assert_eq!(item("splitVertically:").chord(), Some(win(ChordKey::Char('d'))));
        assert_eq!(item("splitHorizontally:").chord(), Some(Chord { shift: true, ..win(ChordKey::Char('d')) }));
        assert_eq!(chord_label(&item("splitHorizontally:").chord().unwrap()), "Win+Shift+D");
    }

    #[test]
    fn arrow_keys_decode() {
        let up = item("selectPaneUp:").chord().unwrap();
        assert_eq!(up, Chord { alt: true, ..win(ChordKey::Up) });
        assert_eq!(chord_label(&up), "Win+Alt+Up");
    }

    #[test]
    fn shifted_punctuation_matches_its_key() {
        assert_eq!(item("biggerFont:").chord(), Some(Chord { shift: true, ..win(ChordKey::Char('=')) }));
        assert_eq!(item("showHelp:").chord(), Some(Chord { shift: true, ..win(ChordKey::Char('/')) }));
    }

    #[test]
    fn the_first_item_in_menu_order_wins_a_shared_chord() {
        let chord = Chord { alt: true, ..win(ChordKey::Up) };
        assert_eq!(command_for(&chord).unwrap().selector, "selectPaneUp:");
    }

    #[test]
    fn commands_are_found_by_chord_and_id() {
        assert_eq!(command_for(&win(ChordKey::Char('t'))).unwrap().selector, "newSession:");
        assert_eq!(command_for(&win(ChordKey::Char(','))).unwrap().selector, "showPrefWindow:");
        let id = item("newSession:").id();
        assert_eq!(id, "Shell > New Tab");
        assert_eq!(by_id(&id).unwrap().selector, "newSession:");
    }

    #[test]
    fn item_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for i in items().iter().filter(|i| !i.separator) {
            assert!(seen.insert(i.id()), "duplicate id {}", i.id());
        }
    }
}
```

Add `pub mod menus;` to `src/lib.rs`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib menus`
Expected: compile errors (`items`, `Chord` not found).

- [ ] **Step 3: Write the module**

Put above the tests in `src/menus.rs`:

```rust
use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Item {
    pub path: Vec<String>,
    #[serde(default)]
    pub selector: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub mods: Vec<String>,
    #[serde(default)]
    pub separator: bool,
    #[serde(default)]
    pub submenu: bool,
    #[serde(default)]
    pub alternate: bool,
}

#[derive(Deserialize)]
struct File {
    item: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ChordKey {
    Char(char),
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    F(u8),
    Enter,
    Tab,
    Escape,
    Backspace,
    Space,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: ChordKey,
    /// The Win key, standing in for Cmd.
    pub cmd: bool,
    /// Alt, standing in for Option.
    pub alt: bool,
    pub shift: bool,
    pub ctrl: bool,
}

/// Characters that need Shift on a US keyboard, and the key that types them.
const SHIFTED: [(char, char); 21] = [
    ('~', '`'), ('!', '1'), ('@', '2'), ('#', '3'), ('$', '4'), ('%', '5'), ('^', '6'),
    ('&', '7'), ('*', '8'), ('(', '9'), (')', '0'), ('_', '-'), ('+', '='), ('{', '['),
    ('}', ']'), ('|', '\\'), (':', ';'), ('"', '\''), ('<', ','), ('>', '.'), ('?', '/'),
];

pub fn items() -> &'static [Item] {
    static ITEMS: OnceLock<Vec<Item>> = OnceLock::new();
    ITEMS.get_or_init(|| {
        toml::from_str::<File>(include_str!("../schema/menus.toml")).expect("schema/menus.toml must parse; the menus tests check it").item
    })
}

impl Item {
    pub fn label(&self) -> &str {
        self.path.last().map_or("", |s| s.as_str())
    }

    pub fn id(&self) -> String {
        self.path.join(" > ")
    }

    /// The Windows chord for this item's key equivalent, if it has one.
    pub fn chord(&self) -> Option<Chord> {
        let mut chars = self.key.chars();
        let c = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        let has = |m: &str| self.mods.iter().any(|x| x == m);
        let (key, implied_shift) = match c {
            '\u{f700}' => (ChordKey::Up, false),
            '\u{f701}' => (ChordKey::Down, false),
            '\u{f702}' => (ChordKey::Left, false),
            '\u{f703}' => (ChordKey::Right, false),
            '\u{f729}' => (ChordKey::Home, false),
            '\u{f72b}' => (ChordKey::End, false),
            '\u{f72c}' => (ChordKey::PageUp, false),
            '\u{f72d}' => (ChordKey::PageDown, false),
            '\u{f728}' | '\u{7f}' => (ChordKey::Delete, false),
            '\r' => (ChordKey::Enter, false),
            '\t' => (ChordKey::Tab, false),
            '\u{1b}' => (ChordKey::Escape, false),
            '\u{8}' => (ChordKey::Backspace, false),
            ' ' => (ChordKey::Space, false),
            c if ('\u{f704}'..='\u{f70f}').contains(&c) => (ChordKey::F((c as u32 - 0xf704 + 1) as u8), false),
            c => match SHIFTED.iter().find(|(s, _)| *s == c) {
                Some((_, base)) => (ChordKey::Char(*base), true),
                None => (ChordKey::Char(c.to_ascii_lowercase()), false),
            },
        };
        Some(Chord { key, cmd: has("cmd"), alt: has("opt"), shift: has("shift") || implied_shift, ctrl: has("ctrl") })
    }
}

pub fn by_id(id: &str) -> Option<&'static Item> {
    items().iter().find(|i| !i.separator && i.id() == id)
}

/// The command a chord runs. When items share a chord, the first in menu order wins,
/// and alternates (macOS Option variants) come after every normal item.
pub fn command_for(chord: &Chord) -> Option<&'static Item> {
    static TABLE: OnceLock<HashMap<Chord, &'static Item>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            let mut t = HashMap::new();
            let commands = items().iter().filter(|i| !i.separator && !i.submenu && !i.selector.is_empty());
            let (normal, alternates): (Vec<_>, Vec<_>) = commands.partition(|i| !i.alternate);
            for i in normal.into_iter().chain(alternates) {
                if let Some(c) = i.chord() {
                    t.entry(c).or_insert(i);
                }
            }
            t
        })
        .get(chord)
        .copied()
}

/// "Win+Shift+D": how Windows menus show a chord.
pub fn chord_label(c: &Chord) -> String {
    let mut parts: Vec<String> = Vec::new();
    if c.cmd {
        parts.push("Win".into());
    }
    if c.ctrl {
        parts.push("Ctrl".into());
    }
    if c.alt {
        parts.push("Alt".into());
    }
    if c.shift {
        parts.push("Shift".into());
    }
    parts.push(match &c.key {
        ChordKey::Char(ch) => ch.to_ascii_uppercase().to_string(),
        ChordKey::F(n) => format!("F{n}"),
        ChordKey::PageUp => "PgUp".into(),
        ChordKey::PageDown => "PgDn".into(),
        ChordKey::Delete => "Del".into(),
        ChordKey::Escape => "Esc".into(),
        other => format!("{other:?}"),
    });
    parts.join("+")
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib menus`
Expected: `7 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/menus.rs src/lib.rs
git commit -m "Load iTerm2's menus and map each key equivalent to a Windows chord"
git push
```

---

### Task 2: Win chords (the keyboard hook) and Cmd in Mods

**Files:**
- Create: `src/winkeys.rs`
- Modify: `src/mouse.rs` (`Mods` gains `pub cmd: bool`), every `Mods { .. }` literal (add `cmd: false`), `src/keys.rs` (drop the Ctrl+Shift stand-ins except the Windows extras), `Cargo.toml` (add `"Win32_UI_Input_KeyboardAndMouse"` to windows-sys features), `src/lib.rs`

**Interfaces:**
- Produces (in `tandem::winkeys`, Windows only):
  - `pub fn vk_to_key(vk: u16) -> Option<ChordKey>`
  - `pub struct Gate` (pure hook logic) with `on_key(&mut self, vk: u16, down: bool, foreground_is_tandem: bool, mods: (bool, bool, bool)) -> Verdict`
  - `pub enum Verdict { Pass, Take(Option<Chord>), PassAndMaskStart }`
  - `pub fn install(hwnd: isize, send: Box<dyn Fn(Chord) + Send + Sync>) -> Result<(), String>`
- `keys::shortcut` keeps only: Ctrl+Shift+C (Copy), Ctrl+Shift+V (Paste), Ctrl+Tab and Ctrl+Shift+Tab (next and previous tab), Shift+PageUp and Shift+PageDown (scroll a page). Everything else moves to the menu chords.

- [ ] **Step 1: Write the failing tests**

`src/winkeys.rs`:

```rust
//! Win chords for Tandem. Windows keeps many Win chords for itself (Win+D, Win+T), so
//! a low-level keyboard hook takes Win chords while a Tandem window has focus and sends
//! them to the app as Cmd chords. Win+L is never taken (Windows does not allow it), and
//! a harmless key is sent before Win comes up, so the Start menu does not open after a
//! chord Tandem used.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menus::ChordKey;

    const VK_LWIN: u16 = 0x5B;
    const NONE: (bool, bool, bool) = (false, false, false);

    #[test]
    fn keys_map_to_chord_keys() {
        assert_eq!(vk_to_key(0x44), Some(ChordKey::Char('d')));
        assert_eq!(vk_to_key(0x35), Some(ChordKey::Char('5')));
        assert_eq!(vk_to_key(0xDB), Some(ChordKey::Char('['))); // VK_OEM_4 on a US layout
        assert_eq!(vk_to_key(0x26), Some(ChordKey::Up));
        assert_eq!(vk_to_key(0x70), Some(ChordKey::F(1)));
        assert_eq!(vk_to_key(0x10), None); // Shift alone is a modifier, not a key
    }

    #[test]
    fn only_the_focused_tandem_window_takes_win_chords() {
        let mut g = Gate::default();
        assert_eq!(g.on_key(VK_LWIN, true, true, NONE), Verdict::Pass);
        assert!(matches!(g.on_key(0x44, true, true, NONE), Verdict::Take(Some(_))));
        assert_eq!(g.on_key(0x44, false, true, NONE), Verdict::Take(None));
        let mut other = Gate::default();
        other.on_key(VK_LWIN, true, false, NONE);
        assert_eq!(other.on_key(0x44, true, false, NONE), Verdict::Pass);
    }

    #[test]
    fn win_l_is_never_taken() {
        let mut g = Gate::default();
        g.on_key(VK_LWIN, true, true, NONE);
        assert_eq!(g.on_key(0x4C, true, true, NONE), Verdict::Pass);
    }

    #[test]
    fn start_menu_is_masked_only_after_a_taken_chord() {
        let mut g = Gate::default();
        g.on_key(VK_LWIN, true, true, NONE);
        assert_eq!(g.on_key(VK_LWIN, false, true, NONE), Verdict::Pass);
        g.on_key(VK_LWIN, true, true, NONE);
        g.on_key(0x44, true, true, NONE);
        assert_eq!(g.on_key(VK_LWIN, false, true, NONE), Verdict::PassAndMaskStart);
    }

    #[test]
    fn the_chord_carries_the_other_modifiers() {
        let mut g = Gate::default();
        g.on_key(VK_LWIN, true, true, NONE);
        let Verdict::Take(Some(c)) = g.on_key(0x44, true, true, (true, false, false)) else { panic!() };
        assert_eq!(c, crate::menus::Chord { key: ChordKey::Char('d'), cmd: true, alt: false, shift: true, ctrl: false });
    }
}
```

`mods` is `(shift, alt, ctrl)`. Add `#[cfg(windows)] pub mod winkeys;` to `src/lib.rs`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib winkeys`
Expected: compile errors (`vk_to_key`, `Gate`, `Verdict` not found).

- [ ] **Step 3: Write the hook**

Put above the tests in `src/winkeys.rs`:

```rust
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, MAPVK_VK_TO_CHAR, MapVirtualKeyW, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, SetWindowsHookExW, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_SYSKEYDOWN,
};

use crate::menus::{Chord, ChordKey};

const VK_LWIN: u16 = 0x5B;
const VK_RWIN: u16 = 0x5C;
/// An unassigned virtual key. Sending it while Win is down keeps the Start menu closed.
const VK_MASK: u16 = 0xE8;

pub fn vk_to_key(vk: u16) -> Option<ChordKey> {
    Some(match vk {
        0x41..=0x5A => ChordKey::Char((vk as u8 + 32) as char),
        0x30..=0x39 => ChordKey::Char(vk as u8 as char),
        0x26 => ChordKey::Up,
        0x28 => ChordKey::Down,
        0x25 => ChordKey::Left,
        0x27 => ChordKey::Right,
        0x24 => ChordKey::Home,
        0x23 => ChordKey::End,
        0x21 => ChordKey::PageUp,
        0x22 => ChordKey::PageDown,
        0x2E => ChordKey::Delete,
        0x0D => ChordKey::Enter,
        0x09 => ChordKey::Tab,
        0x1B => ChordKey::Escape,
        0x08 => ChordKey::Backspace,
        0x20 => ChordKey::Space,
        0x70..=0x7B => ChordKey::F((vk - 0x70 + 1) as u8),
        // Punctuation depends on the layout: ask Windows which character the key types.
        0xBA..=0xC0 | 0xDB..=0xDF => {
            let c = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_CHAR) } & 0x7FFF;
            ChordKey::Char(char::from_u32(c)?.to_ascii_lowercase())
        }
        _ => return None,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Let Windows have the key.
    Pass,
    /// Tandem takes the key; on a key down, this is the chord to run.
    Take(Option<Chord>),
    /// Let Win come up, but first send the mask key so the Start menu stays closed.
    PassAndMaskStart,
}

/// The hook's decisions, kept apart from Win32 so they can be tested.
#[derive(Default)]
pub struct Gate {
    win_down: bool,
    took_any: bool,
}

impl Gate {
    /// `mods` is (shift, alt, ctrl) as held now.
    pub fn on_key(&mut self, vk: u16, down: bool, foreground_is_tandem: bool, mods: (bool, bool, bool)) -> Verdict {
        if vk == VK_LWIN || vk == VK_RWIN {
            self.win_down = down;
            if !down && std::mem::take(&mut self.took_any) {
                return Verdict::PassAndMaskStart;
            }
            return Verdict::Pass;
        }
        // Win+L locks the workstation; Windows never lets an app take it.
        if !self.win_down || !foreground_is_tandem || vk == 0x4C {
            return Verdict::Pass;
        }
        let Some(key) = vk_to_key(vk) else { return Verdict::Pass };
        if !down {
            return Verdict::Take(None);
        }
        self.took_any = true;
        let (shift, alt, ctrl) = mods;
        Verdict::Take(Some(Chord { key, cmd: true, alt, shift, ctrl }))
    }
}

struct Hook {
    gate: Mutex<Gate>,
    send: Box<dyn Fn(Chord) + Send + Sync>,
}

static HOOK: OnceLock<Hook> = OnceLock::new();
static WINDOW: AtomicIsize = AtomicIsize::new(0);

fn held(vk: i32) -> bool {
    unsafe { GetAsyncKeyState(vk) as u16 & 0x8000 != 0 }
}

fn mask_start_menu() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_MASK, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    unsafe { SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some(h) = HOOK.get() {
            let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            // Our own mask key (and any other injected key) passes untouched.
            if kb.flags & LLKHF_INJECTED == 0 {
                let down = wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN;
                let focused = unsafe { GetForegroundWindow() } as isize == WINDOW.load(Ordering::Relaxed);
                let mods = (held(0x10), held(0x12), held(0x11));
                let verdict = h.gate.lock().unwrap().on_key(kb.vkCode as u16, down, focused, mods);
                match verdict {
                    Verdict::Pass => {}
                    Verdict::PassAndMaskStart => mask_start_menu(),
                    Verdict::Take(chord) => {
                        if let Some(c) = chord {
                            (h.send)(c);
                        }
                        return 1;
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// Installs the hook for Tandem's main window. Chords go to `send`, which must be quick:
/// Windows removes a low-level hook that is slow to answer.
pub fn install(hwnd: isize, send: Box<dyn Fn(Chord) + Send + Sync>) -> Result<(), String> {
    WINDOW.store(hwnd, Ordering::Relaxed);
    if HOOK.set(Hook { gate: Mutex::new(Gate::default()), send }).is_err() {
        return Ok(());
    }
    let h = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), std::ptr::null_mut(), 0) };
    if h.is_null() {
        return Err(format!("SetWindowsHookExW failed: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}
```

`src/mouse.rs`: add `pub cmd: bool` to `Mods` (after `ctrl`), documented as "the Win key, standing in for Cmd". Add `cmd: false` to every `Mods { .. }` literal in the crate (tests included). `app_owns_mouse` and `encode` ignore `cmd`.

`src/keys.rs`: replace `shortcut` with the reduced table (Copy, Paste, NextTab, PrevTab, ScrollPageUp, ScrollPageDown as above), delete the `Action` variants no longer produced (`NewTab`, `ClosePane`, `SplitRight`, `SplitDown`, `NextPane`, `PrevPane`, `Focus`, `SelectTab`, `ScrollTop`, `ScrollBottom`, `OpenSettings`, `OpenProfiles`), and delete their tests (`cmd_shortcuts`, `cmd_o_opens_profiles`, and `cmd_comma_opens_settings` in `settings_ui.rs`). Menu chords now cover them; Task 1 tests the chords.

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib`
Expected: all pass (winkeys adds 5; the removed shortcut tests are gone).

- [ ] **Step 5: Commit**

```bash
git add -A src Cargo.toml Cargo.lock
git commit -m "Take Win chords as Cmd while Tandem has focus"
git push
```

---

### Task 3: Command status

**Files:**
- Create: `src/commands.rs`
- Modify: `src/lib.rs` (add `pub mod commands;`)

**Interfaces:**
- Consumes: `menus::{items, Item}`.
- Produces (in `tandem::commands`):
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Status { Done, Partial(&'static str), Later(u8) }`
  - `pub fn status(item: &Item) -> Status`
  - `pub const DONE: &[&str]` (selectors that work now; for shared selectors, `"selector|Label"`)

- [ ] **Step 1: Write the failing tests**

`src/commands.rs`:

```rust
//! Which iTerm2 menu commands work in Tandem now, and which milestone brings the rest.
//! A command owned by a later milestone shows disabled; it never half-works.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menus::items;

    #[test]
    fn every_done_entry_names_a_real_menu_command() {
        for d in DONE {
            let (sel, label) = d.split_once('|').map_or((*d, None), |(s, l)| (s, Some(l)));
            assert!(
                items().iter().any(|i| i.selector == sel && label.is_none_or(|l| i.label() == l)),
                "{d} is not in the menu"
            );
        }
    }

    #[test]
    fn every_command_has_a_status() {
        for i in items().iter().filter(|i| !i.separator && !i.submenu && !i.selector.is_empty()) {
            let s = status(i);
            if let Status::Later(m) = s {
                assert!((4..=13).contains(&m), "{} -> milestone {m}", i.id());
            }
        }
    }

    #[test]
    fn known_statuses() {
        let find = |sel: &str, label: &str| items().iter().find(|i| i.selector == sel && i.label() == label).unwrap();
        assert_eq!(status(find("splitVertically:", "Split Vertically with Current Profile")), Status::Done);
        assert!(matches!(status(find("newWindow:", "New Window")), Status::Partial(_)));
        assert_eq!(status(find("performFindPanelAction:", "Find...")), Status::Later(4));
        assert_eq!(status(find("detachTmux:", "Detach")), Status::Later(8));
        assert_eq!(status(find("setWindowStyle:", "Maximized")), Status::Done);
        assert_eq!(status(find("setWindowStyle:", "Full-Width Top of Screen")), Status::Later(7));
    }
}
```

Add `pub mod commands;` to `src/lib.rs`.

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib commands`
Expected: compile errors (`DONE`, `status`, `Status` not found).

- [ ] **Step 3: Write the table**

Put above the tests in `src/commands.rs`:

```rust
use crate::menus::Item;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Works as in iTerm2.
    Done,
    /// Works, with the named difference until a later milestone.
    Partial(&'static str),
    /// Shown disabled until this milestone.
    Later(u8),
}

/// Commands that work now. "selector|Label" names one item of a shared selector.
pub const DONE: &[&str] = &[
    "showAbout:", "showPrefWindow:", "terminate:", "hide:", "newSession:", "newSessionAlternate:",
    "newSessionWithSameProfile:", "duplicateTab:", "splitHorizontally:", "splitVertically:",
    "openSplitHorizontallySheet:", "openSplitVerticallySheet:", "saveContents:", "saveDocumentAs:",
    "closeCurrentSession:", "closeTerminalWindow:", "closeCurrentTab:", "copy:", "paste:|Paste",
    "pasteSelection:", "selectAll:", "clearBuffer:", "clearScrollbackBuffer:", "toggleFullScreenMode:",
    "biggerFont:", "smallerFont:", "returnToDefaultSize:|Make Text Normal Size", "maximizePane:",
    "editCurrentSession:", "reset:", "resetCharset:", "terminalStateReset:",
    "terminalStateToggleAlternateScreen:", "terminalStateToggleFocusReporting:",
    "terminalStateToggleMouseReporting:", "terminalStateTogglePasteBracketing:",
    "terminalStateToggleApplicationCursor:", "terminalStateToggleApplicationKeypad:|Application Keypad",
    "showBookmarkWindow:", "performMiniaturize:", "performZoom:", "nextTab:", "previousTab:", "moveTabLeft:",
    "moveTabRight:", "selectPaneUp:", "selectPaneDown:", "selectPaneLeft:", "selectPaneRight:", "nextPane:",
    "previousPane:", "movePaneDividerUp:", "movePaneDividerDown:", "movePaneDividerLeft:",
    "movePaneDividerRight:", "arrangeSplitPanesEvenly:", "restartSession:", "duplicateSession:", "showHelp:",
    "arrangeInFront:", "setWindowStyle:|Normal", "setWindowStyle:|Full Screen", "setWindowStyle:|Maximized",
];

/// Commands that work with a known difference for now.
const PARTIAL: &[(&str, &str)] = &[
    ("newWindow:", "Opens a tab until separate windows arrive in milestone 7."),
    ("newWindowWithSameProfile:", "Opens a tab until separate windows arrive in milestone 7."),
    ("biggerFont:", "Changes every pane, since all panes share one font until per-pane fonts arrive."),
    ("smallerFont:", "Changes every pane, since all panes share one font until per-pane fonts arrive."),
];

fn in_list(list: &[&str], item: &Item) -> bool {
    list.iter().any(|d| match d.split_once('|') {
        Some((sel, label)) => item.selector == sel && item.label() == label,
        None => item.selector == *d,
    })
}

/// The milestone that brings a command, from the roadmap (docs/parity/00-roadmap.md).
fn owner(item: &Item) -> u8 {
    let s = item.selector.as_str();
    let path = item.id();
    let has = |needle: &str| path.contains(needle);
    let sel = |needle: &str| s.contains(needle);
    if has("tmux") || sel("Tmux") {
        8
    } else if has("Scripts") || sel("Script") || sel("REPL") || sel("PythonRuntime") || sel("Dependency") {
        11
    } else if sel("AI") || sel("NaturalLanguage") || sel("Cockpit") || sel("InlineChat") {
        12
    } else if sel("ir") && s.starts_with("ir") || sel("InstantReplay") || sel("PasswordManager") || sel("Recording") {
        10
    } else if has("Find") || has("Marks") || sel("Mark") || sel("Annotation") || sel("Note") || sel("fold") || sel("Filter")
        || sel("Selection") || sel("jumpTo") || sel("CurrentCommand") || sel("LastCommand") || sel("openSelection")
        || sel("clearTo") || sel("Timestamps") || sel("CursorGuide") || sel("findCursor") || sel("zoom")
    {
        4
    } else if sel("Trigger") || sel("Coprocess") || sel("Notify") || sel("Logging") && has("Log") {
        6
    } else if has("Broadcast") || has("Arrangements") || has("Window Style") || has("Arrange Windows") || has("Resize Window")
        || sel("Workgroup") || sel("TabGroup") || sel("Group") || sel("moveSession") || sel("movePane") || sel("bury")
        || sel("Hotkey") || sel("Archive") || sel("TabToNewWindow") || sel("Locked") || sel("duplicateWindow")
        || sel("undoClose") || sel("editWindowTitle") || sel("editTabTitle") || sel("Transparency") || sel("FullScreenTabBar")
        || sel("mergeAll") || sel("arrangeHorizontally") || sel("SizeChanges") || sel("LockSplitPaneWidth")
    {
        7
    } else if has("Toolbelt") || has("Paste Special") || sel("compose") || sel("Composer") || sel("openQuickly")
        || sel("History") || sel("Directories") || sel("Autocomplete") || sel("Clippings") || sel("Snippet")
    {
        9
    } else if has("Terminal State") || sel("copyWith") || sel("CopyMode") || sel("undo") || sel("redo") || sel("cut:")
        || sel("replaceSelection") || sel("renderSelection") || sel("print") || sel("PageLayout") || sel("ssh")
        || sel("downloadFiles") || sel("RemoteHost") || sel("Screenshot") || sel("BufferInput")
    {
        5
    } else {
        13
    }
}

pub fn status(item: &Item) -> Status {
    if in_list(DONE, item) {
        return Status::Done;
    }
    if let Some((_, note)) = PARTIAL.iter().find(|(sel, _)| item.selector == *sel) {
        return Status::Partial(note);
    }
    Status::Later(owner(item))
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib commands`
Expected: `3 passed`. When `known_statuses` disagrees with the owner rules, fix the rule order, not the test; ledger it.

- [ ] **Step 5: Commit**

```bash
git add src/commands.rs src/lib.rs
git commit -m "Say which menu commands work now and which milestone brings the rest"
git push
```

---

### Task 4: Layout and screen helpers the commands need

**Files:**
- Modify: `src/layout.rs`, `src/screen.rs`

**Interfaces:**
- Produces:
  - `Layout::even(&mut self)` (every split back to 0.5)
  - `Layout::nudge(&mut self, pane: PaneId, dir: Dir, fraction: f32) -> bool` (moves the nearest divider on `dir`'s axis next to `pane` by `fraction` of its split)
  - `pub struct Zoom` is not needed: the tab keeps `zoomed: Option<PaneId>`; `pub fn visible_rects(layout: &Layout, zoomed: Option<PaneId>, area: Rect, gap: f32) -> Vec<(PaneId, Rect)>` in `layout.rs`
  - `Screen::all_text(&self) -> String` (scrollback and screen, trailing spaces trimmed per line)
  - `Screen::clear(&mut self, scrollback_only: bool)`

- [ ] **Step 1: Write the failing tests**

Add to `src/layout.rs` tests:

```rust
    #[test]
    fn even_resets_every_split() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        l.set_ratio(&[], 0.2);
        l.even();
        assert_eq!(l.rects(AREA, 1.0)[0].1.w, 50.0);
    }

    #[test]
    fn nudge_moves_the_divider_next_to_the_pane() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        assert!(l.nudge(1, Dir::Right, 0.1));
        assert_eq!(l.rects(AREA, 1.0)[0].1.w, 60.0);
        assert!(l.nudge(2, Dir::Left, 0.2));
        assert_eq!(l.rects(AREA, 1.0)[0].1.w, 40.0);
        assert!(!l.nudge(1, Dir::Up, 0.1), "no horizontal divider to move");
    }

    #[test]
    fn closing_the_zoomed_pane_unzooms() {
        let mut l = Layout::new(1);
        l.split(1, Axis::Row, 2);
        assert_eq!(visible_rects(&l, Some(2), AREA, 1.0), vec![(2, AREA)]);
        l.remove(2);
        // The zoomed pane is gone: the tab shows what is left.
        assert_eq!(visible_rects(&l, Some(2), AREA, 1.0), vec![(1, AREA)]);
    }
```

Add to `src/screen.rs` tests:

```rust
    #[test]
    fn all_text_includes_scrollback() {
        let mut s = Screen::new(20, 3);
        for i in 0..6 {
            s.feed(format!("line{i}\r\n").as_bytes());
        }
        let t = s.all_text();
        assert!(t.starts_with("line0\nline1\n"), "{t:?}");
        assert!(t.contains("line5"));
    }

    #[test]
    fn clear_scrollback_keeps_the_screen() {
        let mut s = Screen::new(20, 3);
        for i in 0..6 {
            s.feed(format!("line{i}\r\n").as_bytes());
        }
        s.clear(true);
        let t = s.all_text();
        assert!(!t.contains("line0") && t.contains("line5"), "{t:?}");
        s.clear(false);
        assert!(!s.all_text().contains("line5"));
    }
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `scripts/xt test --lib`
Expected: compile errors (`even`, `nudge`, `visible_rects`, `all_text`, `clear` not found).

- [ ] **Step 3: Write the code**

`src/layout.rs`, in `impl Layout`:

```rust
    /// Puts every divider back in the middle of its split.
    pub fn even(&mut self) {
        fn go(n: &mut Node) {
            if let Node::Split { ratio, a, b, .. } = n {
                *ratio = 0.5;
                go(a);
                go(b);
            }
        }
        go(&mut self.root);
    }

    /// Moves the divider on `dir`'s side of `pane` by `fraction` of its split, toward `dir`.
    /// Returns false when `pane` has no divider on that side.
    pub fn nudge(&mut self, pane: PaneId, dir: Dir, fraction: f32) -> bool {
        let axis = match dir {
            Dir::Left | Dir::Right => Axis::Row,
            Dir::Up | Dir::Down => Axis::Column,
        };
        let toward_second = matches!(dir, Dir::Right | Dir::Down);
        // Walk down to the pane, remembering each split on the right axis where the pane
        // sits on the side that faces `dir`. The deepest one is the adjacent divider.
        fn go(n: &mut Node, pane: PaneId, axis: Axis, toward_second: bool, fraction: f32) -> Option<bool> {
            let Node::Split { axis: a, ratio, a: first, b: second } = n else {
                return if *n == Node::Leaf(pane) { Some(false) } else { None };
            };
            let in_first = go(first, pane, axis, toward_second, fraction);
            let (found, moved) = match in_first {
                Some(m) => (true, m),
                None => match go(second, pane, axis, toward_second, fraction) {
                    Some(m) => (false, m),
                    None => return None,
                },
            };
            if moved || *a != axis || found != toward_second {
                return Some(moved);
            }
            let step = if toward_second { fraction } else { -fraction };
            *ratio = (*ratio + step).clamp(0.1, 0.9);
            Some(true)
        }
        go(&mut self.root, pane, axis, toward_second, fraction).unwrap_or(false)
    }
```

And a free function:

```rust
/// The rectangles a tab shows: only the zoomed pane when one is zoomed and still here.
pub fn visible_rects(layout: &Layout, zoomed: Option<PaneId>, area: Rect, gap: f32) -> Vec<(PaneId, Rect)> {
    match zoomed.filter(|z| layout.panes().contains(z)) {
        Some(z) => vec![(z, area)],
        None => layout.rects(area, gap),
    }
}
```

Note on `nudge` with `Dir::Left` from the second pane: the pane is in `second`, so `found` is false and `toward_second` is false; they are equal, so the divider moves left (`-fraction`). With `Dir::Right` from the first pane: `found` is true and `toward_second` is true, so it moves right.

`src/screen.rs`, in `impl Screen` (add `use alacritty_terminal::vte::ansi::{ClearMode, Handler};` at the top):

```rust
    /// The whole buffer as text, oldest line first, trailing spaces trimmed.
    pub fn all_text(&self) -> String {
        let grid = self.term.grid();
        let top = -(grid.history_size() as i32);
        let mut out = String::new();
        for line in top..grid.screen_lines() as i32 {
            let row = &grid[Line(line)];
            let text: String = (0..grid.columns()).map(|c| row[Column(c)].c).collect();
            out.push_str(text.trim_end());
            out.push('\n');
        }
        let trimmed = out.trim_end_matches('\n').len();
        out.truncate(trimmed);
        out.push('\n');
        out
    }

    /// iTerm2's Clear Scrollback Buffer (`scrollback_only`) or Clear Buffer (everything).
    pub fn clear(&mut self, scrollback_only: bool) {
        self.term.clear_screen(ClearMode::Saved);
        if !scrollback_only {
            self.term.clear_screen(ClearMode::All);
            self.term.goto(Line(0), Column(0));
        }
    }
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `scripts/xt test --lib`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/layout.rs src/screen.rs
git commit -m "Add even splits, divider nudging, pane zoom, buffer text, and clearing"
git push
```

---

### Task 5: The menu bar, the right-click menu, and the commands

**Files:**
- Modify: `Cargo.toml` (add `muda = "0.20"`), `src/app.rs`, `src/settings_ui.rs` (a `select(guid)` for Edit Session)

**Interfaces:**
- Consumes: everything above.
- Produces: `UserEvent::{Chord(Chord), Menu(String)}`; `App::run_command(item: &Item, event_loop)`.

This task is glue between Windows and the tested modules. Its test is a clean build plus the tests above; Robert runs the checklist in Task 6.

- [ ] **Step 1: Write the code**

`src/app.rs`:

1. **Menu bar.** After the window exists, build a `muda::Menu` from `menus::items()`: a `Submenu` per path prefix, a `PredefinedMenuItem::separator()` per separator, and for each command a `MenuItem::with_id(item.id(), text, enabled, None)` where `text` is the label, plus `"\t" + chord_label(&chord)` when the item has a chord, and `enabled` is `status(item) != Later(_)`. Terminal State toggles use `CheckMenuItem` so their check marks can follow the focused pane. Attach with `unsafe { menu.init_for_hwnd(hwnd) }`. The macOS-only "iTerm2" menu keeps its title (it is iTerm2's application menu); rename nothing.
2. **Menu events.** `muda::MenuEvent::set_event_handler(Some(move |e| proxy.send_event(UserEvent::Menu(e.id.0))))`. In `user_event`, `Menu(id)` finds `menus::by_id(&id)` and runs it.
3. **Win chords.** After the window exists, `winkeys::install(hwnd, Box::new(move |c| proxy.send_event(UserEvent::Chord(c))))`. A failure goes to the banner. `Chord(c)` runs `menus::command_for(&c)` if it is enabled.
4. **Keyboard path.** `key()` keeps the reduced `keys::shortcut` extras, then sends everything else to the pane. It no longer calls any Cmd shortcuts itself.
5. **Right-click menu.** When the pane does not own the mouse, a right click opens a `muda::Menu` built from these menu ids, in this order: `Edit > Copy`, `Edit > Paste`, separator, `Shell > Split Vertically with Current Profile`, `Shell > Split Horizontally with Current Profile`, separator, `View > Maximize Active Pane`, `Edit > Select All`, `Edit > Clear Buffer`, separator, `Session > Edit Session...`, `Shell > Close`, shown with `show_context_menu_for_hwnd` at the cursor. Its clicks arrive as the same `UserEvent::Menu` ids.
6. **Commands.** `run_command(item)` matches `item.selector` (and `item.label()` for shared selectors) for every `Done` or `Partial` entry in `commands::DONE`/`PARTIAL`:
   - New tab (`newSession:`), next to the current tab (`newSessionAlternate:`, inserted at `active + 1`), with the focused pane's profile (`newSessionWithSameProfile:`, `duplicateTab:`, `duplicateSession:`); `newWindow:` and `newWindowWithSameProfile:` open a tab (Partial).
   - Splits with the focused pane's profile (`splitVertically:` is `Axis::Row`, `splitHorizontally:` is `Axis::Column`); the `...Sheet:` items open the Profiles window.
   - `closeCurrentSession:` closes the focused pane; `closeCurrentTab:` closes every pane in the tab; `closeTerminalWindow:` and `terminate:` save pending settings and exit.
   - `copy:`, `paste:|Paste`, `pasteSelection:` (pastes the selected text), `selectAll:` (a `Lines` selection from the top of history to the bottom).
   - `clearBuffer:` and `clearScrollbackBuffer:` call `Screen::clear`; `reset:`, `terminalStateReset:` call `term.reset_state()`; `resetCharset:` configures G0 as ASCII and makes it active.
   - Terminal State toggles feed the matching DEC private mode set or reset to the focused screen (`?1049`, `?1004`, `?1000`, `?2004`, `?1`, and `ESC =` / `ESC >` for the keypad) and flip the check mark from `TermMode` (`ALT_SCREEN`, `FOCUS_IN_OUT`, `MOUSE_MODE`, `BRACKETED_PASTE`, `APP_CURSOR`, `APP_KEYPAD`).
   - `biggerFont:` and `smallerFont:` change a session font offset of plus or minus 1 pt (added to the default profile's size in `apply_fonts`); `returnToDefaultSize:|Make Text Normal Size` sets it to 0.
   - `maximizePane:` toggles `Tab::zoomed`; layout and drawing use `layout::visible_rects`.
   - `nextTab:`, `previousTab:`, `moveTabLeft:`, `moveTabRight:` (swap in `tabs`), `selectPane*:` (`Layout::neighbor`), `nextPane:`, `previousPane:`, `movePaneDivider*:` (`Layout::nudge` by one cell's share of the split), `arrangeSplitPanesEvenly:` (`Layout::even`).
   - `restartSession:` drops the pane's pty and starts a new one with the same id, profile, and rectangle.
   - `saveContents:` and `saveDocumentAs:` ask for a file with `rfd::FileDialog::save_file()` and write `Screen::all_text()` or the selection.
   - `showPrefWindow:` opens Settings; `editCurrentSession:` opens Settings with the focused pane's profile selected; `showBookmarkWindow:` opens Profiles.
   - `performMiniaturize:` and `hide:` minimize; `performZoom:` toggles maximized; `toggleFullScreenMode:` and `setWindowStyle:|Full Screen` toggle borderless full screen; `setWindowStyle:|Normal` and `|Maximized` set those states; `arrangeInFront:` focuses the window.
   - `showHelp:` opens `https://iterm2.com/documentation.html` with the default browser (`ShellExecuteW` "open"); `showAbout:` shows a message box with Tandem's name and version.

- [ ] **Step 2: Build and test**

Run: `scripts/xt test --lib` and `scripts/xt build --release --bin tandem`
Expected: all pass, clean build. Ledger every API difference as a ruling.

- [ ] **Step 3: Commit**

```bash
git add -A src Cargo.toml Cargo.lock
git commit -m "Add iTerm2's menu bar and right-click menu, and run 60 of its commands"
git push
```

---

### Task 6: Parity status and the acceptance checklist

**Files:**
- Modify: `docs/parity/03-features-and-menus.md`, `docs/parity/02-global-settings.md` (Navigation Shortcuts rows the menu chords now cover), `docs/parity/00-roadmap.md`
- Create: `docs/acceptance/m3a-checklist.md`

- [ ] **Step 1: Update the inventory**

In section 1 of `03-features-and-menus.md`, set each menu row from `commands::status`: `Done` is `have`, `Partial` is `partial` (with its note), `Later(n)` stays `missing` with "milestone n" in a note. Recount every total.

- [ ] **Step 2: Write the checklist**

`docs/acceptance/m3a-checklist.md`: the menu bar shows iTerm2's nine menus; Win+T, Win+D, Win+Shift+D, Win+W, Win+Alt+arrows, Win+Shift+] and [, Win+], Win+[, Win+K, Win+, and Win+O do what the menu says; the Start menu does not open after them; Win+L still locks; Win chords in other apps still reach Windows; right-click shows the context menu; Maximize Active Pane toggles; Make Text Bigger and Smaller work; Save Contents writes the buffer; Terminal State check marks follow `vim` (Alternate Screen on while vim runs).

- [ ] **Step 3: Commit**

```bash
git add docs/parity docs/acceptance/m3a-checklist.md
git commit -m "Update parity for the menu bar and add the milestone 3a checklist"
git push
```
