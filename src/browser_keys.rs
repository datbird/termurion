//! Key bindings in a browser pane (milestone 13x2, Task 6): which of iTerm2's key actions a page
//! can run (`KBP:12-171`), the editing rule (`IA:527-537`), the scripts that scroll, send text and
//! extend the selection (`WVA`), and the gate that decides, inside WebView2's accelerator handler,
//! whether Tandem takes a key before the page sees it. Keys come only from the person; page script
//! events never reach this module. Pure: the App side is `app::browser_input_app`.

use crate::browser_js::{fill_template, resource};
use crate::infobar::{Bar, shortcut_mods};
use crate::keyaction::{self, Binding};
use crate::keyroute::{self, Route};
use crate::keystroke::Keystroke;
use crate::menus::{Chord, ChordKey};

/// The `mods` byte of `Event::Key` (`webhost_win::modifier_bits`).
pub const SHIFT: u8 = 1;
pub const CTRL: u8 = 2;
pub const ALT: u8 = 4;
pub const WIN: u8 = 8;

/// iTerm2's `ScrollMovement` (`BAP:8-15`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    End,
    Home,
    Down,
    Up,
    PageDown,
    PageUp,
}

/// `performScroll` (`WVA:19-38`); run in the page world.
pub fn scroll_script(s: Scroll) -> &'static str {
    match s {
        Scroll::End => "window.scrollTo(0, document.body.scrollHeight);",
        Scroll::Home => "window.scrollTo(0, 0);",
        Scroll::Down => "window.scrollBy(0, 40);",
        Scroll::Up => "window.scrollBy(0, -40);",
        Scroll::PageDown => "window.scrollBy(0, window.innerHeight);",
        Scroll::PageUp => "window.scrollBy(0, -window.innerHeight);",
    }
}

/// What a key binding does in a browser pane (`BAP:12-171`).
#[derive(Clone, Debug, PartialEq)]
pub enum KeyAct {
    /// Nothing more: ignored, meaningless in a browser, or not built (iTerm2 returns true).
    Handled,
    /// iTerm2's PTYSession runs it: `run_key_action` with the pane as the session.
    Session,
    CopyMode(String),
    SetProfile(String),
    Scroll(Scroll),
    PasteSpecial { config: String, from_selection: bool },
    Send { text: String, broadcast: bool },
    /// The snippet key; the app looks it up.
    Snippet(String),
    Extend { start: bool, forward: bool, unit: i64 },
    CopyOrSend,
    PasteOrSend,
    ToggleSetting(String),
}

/// `perform(keyBindingAction:)` (`BAP:15-167`, codes `KBH:38-116`).
pub fn classify(b: &Binding) -> KeyAct {
    use KeyAct::*;
    match b.action {
        22 | 24 | -1 | 74 | 13 => Handled,
        0 | 1 | 2 | 3 | 18..=21 | 23 | 25..=34 | 39 | 43 | 44 | 49..=58 | 60 | 61 | 62 | 64..=68 | 71 | 72 => Session,
        73 => CopyMode(b.text.clone()),
        37 => SetProfile(b.text.clone()),
        4 => Scroll(self::Scroll::End),
        5 => Scroll(self::Scroll::Home),
        6 => Scroll(self::Scroll::Down),
        7 => Scroll(self::Scroll::Up),
        8 => Scroll(self::Scroll::PageDown),
        9 => Scroll(self::Scroll::PageUp),
        // Meaningless in a browser (`BAP:63-68`), and not yet implemented (`BAP:70-73`).
        10 | 14 | 15 | 16 | 17 | 35 | 59 | 40 | 36 => Handled,
        41 => PasteSpecial { config: b.text.clone(), from_selection: false },
        42 => PasteSpecial { config: b.text.clone(), from_selection: true },
        11 => Send { text: String::from_utf8_lossy(&keyaction::hex_bytes(&b.text)).into_owned(), broadcast: true },
        12 => Send { text: keyaction::expand(&b.text, b.escaping()), broadcast: true },
        38 => Send { text: keyaction::expand(&b.text, b.vim_escaping()), broadcast: true },
        76 => Send { text: keyaction::expand(&b.text, b.vim_escaping()), broadcast: false },
        63 => Snippet(b.text.clone()),
        45..=48 => {
            let unit = b.text.trim().parse::<i64>().unwrap_or(0);
            Extend { start: b.action >= 47, forward: b.action % 2 == 0, unit }
        }
        69 => CopyOrSend,
        70 => PasteOrSend,
        75 => ToggleSetting(b.text.clone()),
        _ => Handled,
    }
}

/// Arrows, Insert, Delete, Home, End (`IA:230-250`; Windows has no Insert chord key).
pub fn is_navigation(key: &ChordKey) -> bool {
    matches!(key, ChordKey::Up | ChordKey::Down | ChordKey::Left | ChordKey::Right | ChordKey::Home | ChordKey::End | ChordKey::Delete)
}

/// While a text field has the page's focus, a binding that sends text (and Ignore and Bypass,
/// `PS:5003-5010`) or a navigation key is not remapped (`IA:527-537`): the page gets the key.
pub fn editing_blocks(b: &Binding, key: &ChordKey, editing: bool) -> bool {
    editing && (crate::composer::binding_sends_text(b) || b.action == 13 || b.action == 74 || is_navigation(key))
}

/// `sendText`'s escaping (`WVA:42-46`): backslash, quote, newline, return; nothing else.
pub fn insert_text_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r")
}

/// insert-text.js with `{{TEXT}}` filled.
pub fn insert_script(text: &str) -> String {
    fill_template(resource("insert-text.js").unwrap_or_default(), &[("TEXT", &insert_text_escape(text))])
}

/// The script of a selection extension unit (`WVA:379-433`; 4 is a mark, skipped).
pub fn extend_script_name(unit: i64) -> Option<&'static str> {
    Some(match unit {
        0 => "extend-selection-character",
        1 => "extend-selection-word",
        2 => "extend-selection-bigword",
        3 => "extend-selection-line",
        _ => return None,
    })
}

pub fn extend_script(start: bool, forward: bool, unit: i64) -> Option<String> {
    let file = format!("{}.js", extend_script_name(unit)?);
    let (s, f) = (if start { "true" } else { "false" }, if forward { "true" } else { "false" });
    let text = resource(&file)?;
    if unit == 1 {
        return type_forward_extension(text).ok().map(|t| fill_template(&t, &[("START", s), ("FORWARD", f)]));
    }
    Some(fill_template(text, &[("START", s), ("FORWARD", f)]))
}

/// Chromium's Windows editing behavior extends a selection forward by word to the START of the next
/// word, so the extension takes the trailing space ("hello " for "hello"; macOS, iTerm2's platform,
/// stops at the end of the word). The assembled script trims whitespace the extension ended on,
/// never past the selection's anchor (a selection that is only whitespace is left alone). The copied
/// file is not edited; the target must be found exactly once, or there is no word extension.
const FORWARD_EXTEND_RAW: &str = "            newOffset = modifiedRange.endOffset;\n";
const FORWARD_EXTEND_TYPED: &str = "            newOffset = modifiedRange.endOffset;\n            // Tandem (Windows): Chromium's Windows editing behavior extends forward by word to the start\n            // of the next word, past the space; macOS stops at the end of the word. Trim whitespace the\n            // extension ended on, never back to the anchor.\n            if (newContainer.nodeType === 3) {\n                var floor = newContainer === modifiedRange.startContainer ? modifiedRange.startOffset : 0;\n                var trimmed = newOffset;\n                while (trimmed > floor && /\\s/.test(newContainer.textContent.charAt(trimmed - 1))) trimmed--;\n                if (trimmed > floor) newOffset = trimmed;\n            }\n";

fn type_forward_extension(script: &str) -> Result<String, String> {
    match script.matches(FORWARD_EXTEND_RAW).count() {
        1 => Ok(script.replacen(FORWARD_EXTEND_RAW, FORWARD_EXTEND_TYPED, 1)),
        n => Err(format!("the word extension script's forward text is there {n} times, not once")),
    }
}

/// The chord of a key the gate saw.
pub fn chord_of(vk: u32, mods: u8) -> Option<Chord> {
    let vk = u16::try_from(vk).ok()?;
    #[cfg(windows)]
    let key = crate::winkeys::vk_to_key(vk)?;
    #[cfg(not(windows))]
    let key = match vk {
        0x41..=0x5A => ChordKey::Char((vk as u8 + 32) as char),
        0x25 => ChordKey::Left,
        _ => return None,
    };
    Some(Chord { key, cmd: mods & WIN != 0, alt: mods & ALT != 0, shift: mods & SHIFT != 0, ctrl: mods & CTRL != 0 })
}

/// The text a key types in the person's keyboard layout under its modifiers (`winkeys::text_for`):
/// Shift+1 is "!" on a US layout and Ctrl+C is U+0003, as an NSEvent's `characters`. None off
/// Windows, for a key that types nothing, and for a dead key.
pub fn layout_text(vk: u32, mods: u8) -> Option<String> {
    #[cfg(windows)]
    {
        let vk = u16::try_from(vk).ok()?;
        crate::winkeys::text_for(vk, 0, crate::mouse::Mods { shift: mods & SHIFT != 0, alt: mods & ALT != 0, ctrl: mods & CTRL != 0, cmd: false })
    }
    #[cfg(not(windows))]
    {
        let _ = (vk, mods);
        None
    }
}

/// The character a key types, which Copy or Send and Paste or Send send when they do not copy or
/// paste (`BAP:171`, the event's characters): the layout's text when the key types one
/// (`layout_text`), else the chord's own character, a Ctrl letter (and `[`, `\`, `]`) as its
/// control character (deferred minor 207).
pub fn own_char(c: &Chord, layout: Option<&str>) -> Option<String> {
    if let Some(t) = layout.filter(|t| !t.is_empty()) {
        return Some(t.to_string());
    }
    match c.key {
        ChordKey::Char(ch) if c.ctrl && (ch.is_ascii_alphabetic() || matches!(ch, '[' | '\\' | ']')) => Some(char::from(ch.to_ascii_uppercase() as u8 & 0x1f).to_string()),
        ChordKey::Char(ch) => Some(if c.shift { ch.to_ascii_uppercase() } else { ch }.to_string()),
        ChordKey::Space => Some(" ".into()),
        ChordKey::Enter => Some("\r".into()),
        ChordKey::Tab => Some("\t".into()),
        _ => None,
    }
}

// --- The gate ---

/// Who owns a key the page raised (the shared order; 13y1 adds its peer and toolbar owners).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The front announcement bar's button.
    Bar(usize),
    /// Ctrl+L, Alt+D, F6: the address field.
    Address,
    /// A profile or global key mapping.
    Binding,
    /// Copy mode on a browser pane: every other key is copy mode's (13x3 Task 8).
    CopyMode,
    /// A window, pane or tab number shortcut.
    Number,
    Leader,
    /// A workgroup member's peer shortcut or session toolbar shortcut (13y1, seam S3): after the
    /// global bindings, before the menu's keys (`workgroup_toolbar::key_owner`, P13).
    Workgroup,
    Page,
}

impl Owner {
    pub fn taken(self) -> bool {
        !matches!(self, Owner::Page)
    }
}

/// The one order (iTerm2's, 13y1 merge review ruling a): a bar's button, then a binding the person
/// made (it beats the default address accelerators, as a mapping beats a menu key in iTerm2,
/// `IA:527-548`), then a workgroup member's own keys for a key no mapping routes (13y1: after the
/// global bindings, before the menu, as in a terminal member), then the address accelerators, then
/// copy mode while it is on (every remaining key, so `h` and `j` never reach the page), then the rest
/// of what the key maps say.
pub fn key_owner(bar: Option<usize>, address: bool, workgroup: bool, copy_mode: bool, mapped: Owner) -> Owner {
    match bar {
        Some(i) => Owner::Bar(i),
        None if mapped == Owner::Binding => Owner::Binding,
        None if workgroup && mapped == Owner::Page => Owner::Workgroup,
        None if address => Owner::Address,
        None if copy_mode => Owner::CopyMode,
        None => mapped,
    }
}

/// A key that is only a modifier (or Caps Lock): copy mode never takes it.
pub fn modifier_vk(vk: u32) -> bool {
    matches!(vk, 0x10..=0x12 | 0x14 | 0x5B | 0x5C | 0xA0..=0xA5)
}

/// Alt+F4 and F10 (and Shift+F10): the system's close and menu keys. Copy mode never takes them, so
/// the host window keeps them as the terminal's copy mode does (13x3 final wave, Task 8 minor 1).
pub fn system_key(vk: u32, mods: u8) -> bool {
    (vk == 0x73 && mods & ALT != 0) || vk == 0x79
}

/// Ctrl+L, Alt+D and F6 (13x's `browser_page_key`).
pub fn address_accelerator(vk: u32, mods: u8) -> bool {
    let (ctrl, alt, win) = (mods & CTRL != 0, mods & ALT != 0, mods & WIN != 0);
    (ctrl && !alt && !win && vk == 0x4C) || (alt && !ctrl && !win && vk == 0x44) || vk == 0x75
}

/// What the gate's closure holds: copies of the maps and settings `keyroute::route` reads.
#[derive(Clone, Default)]
pub struct GateState {
    pub profile_map: toml::Table,
    pub global_map: toml::Table,
    pub language_agnostic: bool,
    pub leader: Option<Keystroke>,
    pub leader_on: bool,
    /// SwitchTabModifier, SwitchPaneModifier, SwitchWindowModifier.
    pub switch: [i64; 3],
    pub bar: Option<Bar>,
    pub editing: bool,
    /// A workgroup member's peer and toolbar shortcuts (13y1), as stored; empty for other panes.
    pub workgroup_keys: Vec<crate::workgroup::Shortcut>,
    /// Copy mode is on in this browser pane (13x3 Task 8): it owns the keys nothing else claims.
    pub copy_mode: bool,
}

impl GateState {
    /// `keyroute::route` of the chord with this state's maps (the gate and the app side both ask it).
    pub fn route(&self, chord: &Chord) -> Route {
        let ctx = keyroute::Context {
            profile_map: &self.profile_map,
            global_map: &self.global_map,
            language_agnostic: self.language_agnostic,
            leader: self.leader.clone(),
            leader_on: self.leader_on,
            switch_tab: self.switch[0],
            switch_pane: self.switch[1],
            switch_window: self.switch[2],
        };
        keyroute::route(&ctx, &keyroute::press_from_chord(chord))
    }

    pub fn owner(&self, vk: u32, mods: u8) -> Owner {
        // A Win chord is the keyboard hook's and `browser_page_key`'s.
        if mods & WIN != 0 {
            return Owner::Page;
        }
        let copy = self.copy_mode && !modifier_vk(vk) && !system_key(vk, mods);
        let address = address_accelerator(vk, mods);
        let Some(chord) = chord_of(vk, mods) else { return key_owner(None, address, false, copy, Owner::Page) };
        let bar = self.bar.as_ref().filter(|_| shortcut_mods(chord.alt, chord.ctrl, chord.shift, chord.cmd)).and_then(|b| match chord.key {
            ChordKey::Char(c) => b.shortcut(c),
            _ => None,
        });
        let route = self.route(&chord);
        // Only a key no mapping routes reaches a member's keys, as `route_press` does (Route::Terminal).
        let workgroup = matches!(route, Route::Terminal) && !self.workgroup_keys.is_empty() && {
            let stroke = keyroute::press_from_chord(&chord).stroke;
            self.workgroup_keys.iter().any(|s| crate::workgroup_peers::shortcut_matches(s, &stroke))
        };
        let mapped = match route {
            // Bypass is not actionable (`KBA:617-622`): its key goes on to the page.
            Route::Binding(b) if b.action == 74 => Owner::Page,
            Route::Binding(b) if editing_blocks(&b, &chord.key, self.editing) => Owner::Page,
            Route::Binding(_) => Owner::Binding,
            Route::SelectWindow(_) | Route::SelectPane(_) | Route::SelectTab(_) => Owner::Number,
            Route::LeaderOn => Owner::Leader,
            Route::Terminal => Owner::Page,
        };
        key_owner(bar, address, workgroup, copy, mapped)
    }
}

// --- What a classified key does ---

#[derive(Clone, Copy, Debug, Default)]
pub struct Facts {
    pub has_selection: bool,
    pub clipboard_text: bool,
}

/// One effect of a key; the app runs each (page scripts in the page world, world scripts in
/// Tandem's isolated world).
#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    PageScript(String),
    WorldScript(String),
    Insert(String),
    Copy,
    Paste,
    Session,
    CopyMode(String),
    Toggle(String),
    Snippet(String),
    PasteSpecial { config: String, from_selection: bool },
    SetProfile(String),
}

/// `perform` with the facts it asks the page (`BAP:139-160`); `own` is the key's own character.
pub fn plan(act: &KeyAct, own: &str, f: &Facts) -> Vec<Do> {
    let send_own = || if own.is_empty() { Vec::new() } else { vec![Do::Insert(own.to_string())] };
    match act {
        KeyAct::Handled => Vec::new(),
        KeyAct::Session => vec![Do::Session],
        KeyAct::CopyMode(k) => vec![Do::CopyMode(k.clone())],
        KeyAct::SetProfile(g) => vec![Do::SetProfile(g.clone())],
        KeyAct::Scroll(s) => vec![Do::PageScript(scroll_script(*s).to_string())],
        KeyAct::PasteSpecial { config, from_selection } => vec![Do::PasteSpecial { config: config.clone(), from_selection: *from_selection }],
        // `broadcast` is ignored, as iTerm2's `actionPerformingSend` ignores it (`BVC:1545-1551`).
        KeyAct::Send { text, .. } => vec![Do::Insert(text.clone())],
        KeyAct::Snippet(k) => vec![Do::Snippet(k.clone())],
        KeyAct::Extend { start, forward, unit } => extend_script(*start, *forward, *unit).map(Do::WorldScript).into_iter().collect(),
        KeyAct::CopyOrSend if f.has_selection => vec![Do::Copy],
        KeyAct::CopyOrSend => send_own(),
        KeyAct::PasteOrSend if f.clipboard_text => vec![Do::Paste],
        KeyAct::PasteOrSend => send_own(),
        KeyAct::ToggleSetting(j) => vec![Do::Toggle(j.clone())],
    }
}

// --- Broadcast between browser panes (Task 8) ---

/// The handler of Tandem's broadcast script (`broadcast-input.js`, Client world).
pub const BROADCAST_HANDLER: &str = "tandemBroadcastInput";

/// The longest text one broadcast event carries (a page's own paste limit).
pub const MAX_BROADCAST_TEXT: usize = 1_048_576;

/// What iTerm2 sends between browser sessions: `insert(text:)` and `doCommandBySelector`
/// (`BWV:1033-1047`); here an inserted text, or one of the editing keys.
#[derive(Clone, Debug, PartialEq)]
pub enum BroadcastEvent {
    Insert(String),
    Key { key: String, code: String, key_code: i64, mods: u8 },
}

/// The editing keys that are broadcast, with their virtual key codes (Decision: iTerm2's selectors
/// are these editing commands; shortcut chords are not broadcast).
const BROADCAST_KEYS: &[(&str, i64)] = &[
    ("Backspace", 8),
    ("Tab", 9),
    ("Enter", 13),
    ("Escape", 27),
    ("PageUp", 33),
    ("PageDown", 34),
    ("End", 35),
    ("Home", 36),
    ("ArrowLeft", 37),
    ("ArrowUp", 38),
    ("ArrowRight", 39),
    ("ArrowDown", 40),
    ("Delete", 46),
];

/// A message of the broadcast script as an event, or None for any other shape. The message comes
/// from the isolated world only, and is still held to the set the script sends.
pub fn broadcast_of(body: &serde_json::Value) -> Option<BroadcastEvent> {
    match body.get("t")?.as_str()? {
        "insert" => {
            let text = body.get("text")?.as_str()?;
            (!text.is_empty() && text.len() <= MAX_BROADCAST_TEXT).then(|| BroadcastEvent::Insert(text.to_string()))
        }
        "key" => {
            let key = body.get("key")?.as_str()?;
            let code = body.get("code")?.as_str()?;
            let key_code = body.get("keyCode")?.as_i64()?;
            let mods = body.get("mods")?.as_u64().filter(|m| *m <= 15)? as u8;
            let known = BROADCAST_KEYS.iter().any(|(k, c)| *k == key && *c == key_code);
            let code_ok = !code.is_empty() && code.len() <= 32 && code.chars().all(|c| c.is_ascii_alphanumeric());
            (known && code_ok).then(|| BroadcastEvent::Key { key: key.to_string(), code: code.to_string(), key_code, mods })
        }
        _ => None,
    }
}

/// DevTools' `modifiers` for Tandem's key bits (Alt 1, Ctrl 2, Meta 4, Shift 8).
fn devtools_modifiers(mods: u8) -> u8 {
    (if mods & ALT != 0 { 1 } else { 0 }) | (if mods & CTRL != 0 { 2 } else { 0 }) | (if mods & WIN != 0 { 4 } else { 0 }) | (if mods & SHIFT != 0 { 8 } else { 0 })
}

/// The DevTools calls that replay `e` in a target page, in order.
pub fn replay_calls(e: &BroadcastEvent) -> Vec<(&'static str, serde_json::Value)> {
    use serde_json::json;
    match e {
        BroadcastEvent::Insert(text) => vec![("Input.insertText", json!({ "text": text }))],
        BroadcastEvent::Key { key, code, key_code, mods } => {
            let event = |kind: &str| {
                json!({ "type": kind, "key": key, "code": code, "windowsVirtualKeyCode": key_code, "nativeVirtualKeyCode": key_code, "modifiers": devtools_modifiers(*mods) })
            };
            // Enter is a keyDown with text "\r" (Blink makes the char that submits a form or breaks a
            // line); the other editing keys are raw keys, as before.
            let mut down = event(if *key == "Enter" { "keyDown" } else { "rawKeyDown" });
            if *key == "Enter" {
                down["text"] = json!("\r");
                down["unmodifiedText"] = json!("\r");
            }
            vec![("Input.dispatchKeyEvent", down), ("Input.dispatchKeyEvent", event("keyUp"))]
        }
    }
}

/// What the app knows of a pane that could receive a broadcast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    pub browser: bool,
    /// The pane has a WebView2 view (a buried peer has none until it is shown).
    pub live: bool,
    pub shown: bool,
    pub dev_null: bool,
}

/// The panes of the sender's broadcast domain that get its browser input: browser panes other than
/// the sender (iTerm2 does not broadcast browser input to terminals yet, `PSB:400`), with a live,
/// shown view, in the sender's privacy class (P18: private text never lands in a persistent profile).
pub fn broadcast_peers(domain: &[crate::layout::PaneId], sender: crate::layout::PaneId, facts: impl Fn(crate::layout::PaneId) -> Option<Peer>) -> Vec<crate::layout::PaneId> {
    let Some(from) = facts(sender) else { return Vec::new() };
    domain
        .iter()
        .copied()
        .filter(|p| *p != sender)
        .filter(|p| facts(*p).is_some_and(|f| f.browser && f.live && f.shown && f.dev_null == from.dev_null))
        .collect()
}

/// How long a pane's own messages are ignored after the last replay to it ended: the replay's
/// completion and the page's report of it travel different roads, so no ordering is relied on.
pub const HOLD_OFF: std::time::Duration = std::time::Duration::from_millis(250);

/// Whether a broadcast message of a pane is acted on: the pane is a broadcast source now, no replay
/// is in flight to it (`receivingBroadcast`, `BM:882-899`) and the last one ended at least `HOLD_OFF`
/// ago (`quiet_for` None: none ever).
pub fn broadcast_accepts(sends: bool, receiving: u32, quiet_for: Option<std::time::Duration>) -> bool {
    sends && receiving == 0 && quiet_for.is_none_or(|d| d >= HOLD_OFF)
}

/// Whether a pane's broadcast message is acted on: not the echo of an input it was sent, and
/// `broadcast_accepts`. The echo is consumed FIRST (final review minor 5): an echo that arrives in the
/// hold-off is used up there, so the person's own identical input right after is never dropped.
pub fn broadcast_verdict(guard: &mut EchoGuard, e: &BroadcastEvent, now: std::time::Instant, sends: bool, receiving: u32, quiet_for: Option<std::time::Duration>) -> bool {
    let echo = guard.is_echo(e, now);
    !echo && broadcast_accepts(sends, receiving, quiet_for)
}

/// How long a replayed input waits for its echo before it is forgotten.
pub const ECHO_TTL: std::time::Duration = std::time::Duration::from_secs(2);
/// At most this many replayed inputs are remembered per pane.
pub const ECHO_MAX: usize = 16;

/// The inputs a pane was sent (a per-event origin tag kept natively, since a replayed input is
/// trusted in the page): the pane's own report of one is dropped once, so a receiver never
/// re-broadcasts what it received, whatever the order of events.
#[derive(Default)]
pub struct EchoGuard {
    sent: Vec<(BroadcastEvent, std::time::Instant)>,
}

impl EchoGuard {
    /// `e` is about to be replayed in this pane.
    pub fn note(&mut self, e: &BroadcastEvent, now: std::time::Instant) {
        self.sent.retain(|(_, t)| now.saturating_duration_since(*t) < ECHO_TTL);
        if self.sent.len() >= ECHO_MAX {
            self.sent.remove(0);
        }
        self.sent.push((e.clone(), now));
    }

    /// Whether the pane's own report of `e` is the echo of a replay (consumed: true once).
    pub fn is_echo(&mut self, e: &BroadcastEvent, now: std::time::Instant) -> bool {
        self.sent.retain(|(_, t)| now.saturating_duration_since(*t) < ECHO_TTL);
        match self.sent.iter().position(|(s, _)| s == e) {
            Some(i) => {
                self.sent.remove(i);
                true
            }
            None => false,
        }
    }

    /// `e` was noted but no input was made (the target's guard refused, or a step failed): no echo
    /// will come, so the note goes (the newest one for `e`).
    pub fn forget(&mut self, e: &BroadcastEvent) {
        if let Some(i) = self.sent.iter().rposition(|(s, _)| s == e) {
            self.sent.remove(i);
        }
    }

    pub fn len(&self) -> usize {
        self.sent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sent.is_empty()
    }
}

/// Whether a replay runs its next call: only while calls are left and the target is still in the
/// sender's broadcast set (deferred minor 221). A replay that stops because its target left forgets
/// the echo it noted (final residual 2), as a refused guard or a failed step does; one that ran all
/// its calls keeps the note, since its echo comes.
pub fn replay_continues(guard: &mut EchoGuard, e: &BroadcastEvent, calls_left: bool, still_target: bool) -> bool {
    if !calls_left {
        return false;
    }
    if !still_target {
        guard.forget(e);
        return false;
    }
    true
}

/// The count of replays in flight to a view goes up before the first call.
pub fn receiving_raise(n: u32) -> u32 {
    n.saturating_add(1)
}

/// And down when the last call answers.
pub fn receiving_lower(n: u32) -> u32 {
    n.saturating_sub(1)
}

/// How long a replay may be in flight before the count is not believed (a call that never answers
/// must not silence the pane for good).
pub const RECEIVING_STALE_SECS: u64 = 5;

/// The receiving side's own check, run in Tandem's world before anything is replayed: true when the
/// focused field holds a secret (a password, a one-time code, a card), when the focus is in a frame
/// (this world cannot see inside it), or when anything fails. Only a plain `false` lets a replay go.
pub fn guard_body() -> String {
    let sel = serde_json::to_string(&crate::web_replay::sensitive_selector()).unwrap_or_default();
    format!("try {{ const SEL = {sel}; {FIELD_CHECK_JS} return tandemSecretField(tandemDeepest(), SEL); }} catch (e) {{ return true; }}")
}

/// What the secret check knows of one element (the Rust statement of `FIELD_CHECK_JS`, for tests).
pub struct FieldFacts<'a> {
    pub tag: &'a str,
    pub input_type: &'a str,
    pub autocomplete: &'a str,
    /// The computed `-webkit-text-security`.
    pub text_security: &'a str,
    /// An INPUT, a TEXTAREA or content-editable.
    pub editable: bool,
}

/// The rule `FIELD_CHECK_JS` states: a focus that is nothing (BODY, HTML) is not secret; a frame, a
/// password input, a masked field, a secret autocomplete token, and anything that is not itself a known
/// editable (the host of a closed shadow root, a button) is secret (fail closed).
pub fn field_is_secret(f: &FieldFacts) -> bool {
    if matches!(f.tag, "BODY" | "HTML") {
        return false;
    }
    if matches!(f.tag, "IFRAME" | "FRAME") || f.input_type.eq_ignore_ascii_case("password") {
        return true;
    }
    if !f.text_security.is_empty() && f.text_security != "none" {
        return true;
    }
    let token = |t: &str| ["current-password", "new-password", "one-time-code"].contains(&t) || t.starts_with("cc-");
    if f.autocomplete.split_ascii_whitespace().any(|t| token(&t.to_ascii_lowercase())) {
        return true;
    }
    !f.editable
}

/// The check both sides run in Tandem's isolated world (a page cannot replace its builtins): the
/// element focused through open shadow roots, judged by `tandemSecretField` (the selector the caller
/// holds as `SEL`, a masked computed style, the autocomplete tokens, the field type, and fail closed
/// for anything not itself an editable).
pub const FIELD_CHECK_JS: &str = "function tandemDeepest() { let a = document.activeElement; \
for (let i = 0; i < 32 && a; i++) { if (a.shadowRoot && a.shadowRoot.activeElement) { a = a.shadowRoot.activeElement; } else { break; } } return a; } \
function tandemSecretField(a, SEL) { \
if (!a || a.nodeType !== 1) { return true; } \
const tag = a.tagName; \
if (tag === 'BODY' || tag === 'HTML') { return false; } \
if (tag === 'IFRAME' || tag === 'FRAME') { return true; } \
if (a.matches(SEL)) { return true; } \
const cs = getComputedStyle(a); \
const ts = cs.webkitTextSecurity || cs.getPropertyValue('-webkit-text-security'); \
if (ts && ts !== 'none') { return true; } \
const ac = (a.getAttribute('autocomplete') || '').toLowerCase().split(/\\s+/); \
for (const t of ac) { if (t === 'current-password' || t === 'new-password' || t === 'one-time-code' || t.indexOf('cc-') === 0) { return true; } } \
if (tag === 'INPUT' && String(a.type).toLowerCase() === 'password') { return true; } \
return !(tag === 'INPUT' || tag === 'TEXTAREA' || a.isContentEditable === true); }";

/// The broadcast script as a Client-world user script (main frame; typing in a frame is not broadcast).
pub fn user_script(secret: &str) -> crate::browser_js::UserScript {
    let selector = serde_json::to_string(&crate::web_replay::sensitive_selector()).unwrap_or_default();
    crate::browser_js::UserScript {
        id: "broadcast-input",
        code: fill_template(resource("broadcast-input.js").unwrap_or_default(), &[("SECRET", secret), ("SELECTOR", &selector), ("FIELD_CHECK", FIELD_CHECK_JS)]),
        inject: crate::browser_js::Inject::DocumentStart,
        main_frame_only: true,
        world: crate::browser_js::World::Client,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyaction::{Binding, Escaping, expand};
    use crate::menus::{Chord, ChordKey};

    fn b(action: i64, text: &str) -> Binding {
        Binding::new(action, text)
    }

    #[test]
    fn key_actions_follow_iterm2() {
        for a in [22, 24, -1, 74, 13, 10, 14, 15, 16, 17, 35, 59, 40, 36] {
            assert_eq!(classify(&b(a, "x")), KeyAct::Handled, "{a}");
        }
        let session = [0, 1, 2, 3, 18, 19, 20, 21, 23, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 39, 43, 44, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 60, 61, 62, 64, 65, 66, 67, 68, 71, 72];
        for a in session {
            assert_eq!(classify(&b(a, "x")), KeyAct::Session, "{a}");
        }
        assert_eq!(classify(&b(73, "vv")), KeyAct::CopyMode("vv".into()));
        assert_eq!(classify(&b(37, "guid")), KeyAct::SetProfile("guid".into()));
        let scrolls = [Scroll::End, Scroll::Home, Scroll::Down, Scroll::Up, Scroll::PageDown, Scroll::PageUp];
        for (i, s) in scrolls.into_iter().enumerate() {
            assert_eq!(classify(&b(4 + i as i64, "")), KeyAct::Scroll(s));
        }
        assert_eq!(classify(&b(42, "cfg")), KeyAct::PasteSpecial { config: "cfg".into(), from_selection: true });
        assert_eq!(classify(&b(41, "cfg")), KeyAct::PasteSpecial { config: "cfg".into(), from_selection: false });
        assert_eq!(classify(&b(11, "0x41 0x42")), KeyAct::Send { text: "AB".into(), broadcast: true });
        let mut c = b(12, "a\\n");
        c.escaping = Some(1);
        assert_eq!(classify(&c), KeyAct::Send { text: expand("a\\n", Escaping::Compatibility), broadcast: true });
        let v = b(38, "a\\n");
        assert_eq!(classify(&v), KeyAct::Send { text: expand("a\\n", v.vim_escaping()), broadcast: true });
        assert_eq!(classify(&b(76, "a\\n")), KeyAct::Send { text: expand("a\\n", b(76, "").vim_escaping()), broadcast: false });
        assert_eq!(classify(&b(63, "k")), KeyAct::Snippet("k".into()));
        for (a, start, forward) in [(45, false, false), (46, false, true), (47, true, false), (48, true, true)] {
            assert_eq!(classify(&b(a, "1")), KeyAct::Extend { start, forward, unit: 1 });
        }
        assert_eq!(classify(&b(69, "")), KeyAct::CopyOrSend);
        assert_eq!(classify(&b(70, "")), KeyAct::PasteOrSend);
        assert_eq!(classify(&b(75, "{\"k\":1}")), KeyAct::ToggleSetting("{\"k\":1}".into()));
    }

    #[test]
    fn editing_rule_follows_iterm2() {
        let ctrl_j = ChordKey::Char('j');
        assert!(editing_blocks(&b(12, "x"), &ctrl_j, true), "send text goes to the page");
        assert!(!editing_blocks(&b(0, ""), &ctrl_j, true), "next tab still runs");
        assert!(editing_blocks(&b(0, ""), &ChordKey::Left, true), "a navigation key goes to the page");
        assert!(editing_blocks(&b(13, ""), &ctrl_j, true) && editing_blocks(&b(74, ""), &ctrl_j, true));
        // Deferred minor 206: a Sequence sends text when any of its steps does (iTerm2's sendsText
        // recurses into KEY_ACTION_SEQUENCE).
        let seq = |steps: &[crate::keyaction::Binding]| b(66, &crate::keyaction::sequence_text(steps));
        assert!(editing_blocks(&seq(&[b(0, ""), b(12, "x")]), &ctrl_j, true), "a sequence with send text goes to the page");
        assert!(!editing_blocks(&seq(&[b(0, ""), b(1, "")]), &ctrl_j, true), "a sequence of tab moves still runs");
        assert!(!editing_blocks(&seq(&[b(12, "x")]), &ctrl_j, false));
        assert!(!editing_blocks(&b(66, "not json"), &ctrl_j, true));
        for a in [12, 0, 13, 74] {
            assert!(!editing_blocks(&b(a, ""), &ChordKey::Left, false) && !editing_blocks(&b(a, ""), &ctrl_j, false));
        }
        for k in [ChordKey::Up, ChordKey::Down, ChordKey::Left, ChordKey::Right, ChordKey::Home, ChordKey::End, ChordKey::Delete] {
            assert!(is_navigation(&k));
        }
        for k in [ChordKey::Char('a'), ChordKey::F(1), ChordKey::Enter] {
            assert!(!is_navigation(&k));
        }
    }

    #[test]
    fn scroll_scripts_follow_iterm2() {
        assert_eq!(scroll_script(Scroll::End), "window.scrollTo(0, document.body.scrollHeight);");
        assert_eq!(scroll_script(Scroll::Home), "window.scrollTo(0, 0);");
        assert_eq!(scroll_script(Scroll::Down), "window.scrollBy(0, 40);");
        assert_eq!(scroll_script(Scroll::Up), "window.scrollBy(0, -40);");
        assert_eq!(scroll_script(Scroll::PageDown), "window.scrollBy(0, window.innerHeight);");
        assert_eq!(scroll_script(Scroll::PageUp), "window.scrollBy(0, -window.innerHeight);");
    }

    #[test]
    fn insert_text_escapes_follow_iterm2() {
        assert_eq!(insert_text_escape("a\\b\"c\nd\re"), "a\\\\b\\\"c\\nd\\re");
        assert_eq!(insert_text_escape("tab\t'x'"), "tab\t'x'", "nothing else is touched");
        for (u, n) in [(0, "extend-selection-character"), (1, "extend-selection-word"), (2, "extend-selection-bigword"), (3, "extend-selection-line")] {
            assert_eq!(extend_script_name(u), Some(n));
        }
        assert_eq!(extend_script_name(4), None);
        assert_eq!(extend_script_name(-1), None);
        // The templates fill: nothing of `{{...}}` is left.
        let s = insert_script("hi\n");
        assert!(s.contains("\"hi\\n\"") && !s.contains("{{"));
        let e = extend_script(true, false, 1).unwrap();
        assert!(e.contains("var isStart = true;") && e.contains("var isForward = false;") && !e.contains("{{"));
        assert!(extend_script(true, true, 4).is_none());
    }

    /// Main bug found in the 13x3 probe review: Chromium's Windows editing behavior extends forward by
    /// word to the next word's start, so "hello" selected as "hello ". The assembled script trims it.
    #[test]
    fn forward_word_extension_stops_before_the_space_on_windows() {
        let f = extend_script(false, true, 1).expect("the script assembles");
        assert_eq!(f.matches("// Tandem (Windows): Chromium's Windows editing behavior extends forward by word").count(), 1);
        assert!(f.contains("trimmed--") && !f.contains("{{"));
        assert_eq!(f.matches(FORWARD_EXTEND_TYPED).count(), 1);
        assert_eq!(resource("extend-selection-word.js").unwrap().matches(FORWARD_EXTEND_RAW).count(), 1, "the copy is unchanged");
        assert!(type_forward_extension("no target here").is_err());
        assert!(type_forward_extension(&format!("{FORWARD_EXTEND_RAW}{FORWARD_EXTEND_RAW}")).is_err());
        assert_eq!(type_forward_extension(&format!("a{FORWARD_EXTEND_RAW}b")), Ok(format!("a{FORWARD_EXTEND_TYPED}b")));
        let b = extend_script(true, false, 1).unwrap();
        assert!(b.contains("var isForward = false;"), "backward is the same patched file, guarded by isForward");
        assert!(!extend_script(true, true, 0).unwrap().contains("Tandem (Windows)"), "other units are untouched");
    }

    #[test]
    fn chords_come_from_the_keys_the_gate_sees() {
        assert_eq!(chord_of(0x41, CTRL), Some(Chord { key: ChordKey::Char('a'), cmd: false, alt: false, shift: false, ctrl: true }));
        assert_eq!(chord_of(0x25, 0).map(|c| c.key), Some(ChordKey::Left));
        let all = chord_of(0x41, SHIFT | CTRL | ALT | WIN).unwrap();
        assert!(all.shift && all.ctrl && all.alt && all.cmd);
        assert_eq!(chord_of(0x10, 0), None, "a bare modifier is no chord");
    }

    fn table(entries: &[(&str, i64)]) -> toml::Table {
        let mut t = toml::Table::new();
        for (k, a) in entries {
            t.insert((*k).into(), toml::from_str::<toml::Value>(&format!("Action = {a}\nText = \"t\"")).unwrap());
        }
        t
    }

    #[test]
    fn the_gate_takes_what_iterm2_remaps() {
        // Ctrl+J = 0x6a-0x40000; Ctrl+K next to it.
        let mut g = GateState { profile_map: table(&[("0x6a-0x40000", 0), ("0x6b-0x40000", 12)]), switch: [4, 9, 6], ..GateState::default() };
        assert_eq!(g.owner(0x4A, CTRL), Owner::Binding);
        assert_eq!(g.owner(0x4B, CTRL), Owner::Binding, "text is sent when no field has the page's focus");
        assert_eq!(g.owner(0x4C, CTRL), Owner::Address, "Ctrl+L is Tandem's address field");
        assert_eq!(g.owner(0x44, ALT), Owner::Address);
        assert_eq!(g.owner(0x75, 0), Owner::Address);
        assert_eq!(g.owner(0x4D, CTRL), Owner::Page, "an unbound key is the page's");
        g.editing = true;
        assert_eq!(g.owner(0x4B, CTRL), Owner::Page, "a text binding goes to the field");
        assert_eq!(g.owner(0x4A, CTRL), Owner::Binding, "next tab still runs");
        // A Win chord belongs to the hook and `browser_page_key`, never the gate.
        assert_eq!(g.owner(0x4A, WIN | CTRL), Owner::Page);
        // Number shortcuts: Alt+Win is the window tag 6 (Cmd+Opt); Ctrl+digit is the pane tag when set to 1.
        g.switch = [4, 1, 6];
        assert_eq!(g.owner(0x32, CTRL), Owner::Number);
    }

    #[test]
    fn a_bar_button_beats_every_binding() {
        let bar = crate::infobar::Bar { id: "x".into(), text: "t".into(), buttons: vec!["_Allow".into(), "_Deny".into()], important: true };
        let g = GateState { bar: Some(bar), global_map: table(&[("0x64-0x80000", 0)]), ..GateState::default() };
        assert_eq!(g.owner(0x44, ALT), Owner::Bar(1), "Alt+D presses Deny, not the address field");
        assert_eq!(g.owner(0x41, ALT), Owner::Bar(0));
        assert_eq!(g.owner(0x41, ALT | CTRL), Owner::Page, "AltGr types a character");
        assert_eq!(g.owner(0x5A, ALT), Owner::Page);
    }

    /// Review I2 and m2 (`KBA:617-622`, `IA:527-548`): a Bypass binding is not actionable, so its key
    /// goes on to the page; a person's own binding on an address key runs.
    #[test]
    fn a_bypass_binding_is_the_pages_and_a_persons_binding_beats_the_address_keys() {
        let g = GateState { profile_map: table(&[("0x6a-0x40000", 74), ("0x6c-0x40000", 0)]), ..GateState::default() };
        assert_eq!(g.owner(0x4A, CTRL), Owner::Page, "Bypass passes the key on");
        assert_eq!(g.owner(0x4C, CTRL), Owner::Binding, "Ctrl+L is the person's binding here");
        assert_eq!(g.owner(0x44, ALT), Owner::Address, "unbound address keys stay Tandem's");
        assert_eq!(g.owner(0x75, 0), Owner::Address);
    }

    #[test]
    fn the_owner_order_is_one_function() {
        assert_eq!(key_owner(Some(2), true, false, false, Owner::Binding), Owner::Bar(2));
        assert_eq!(key_owner(None, true, false, false, Owner::Binding), Owner::Binding, "a person's own binding beats the default address keys");
        assert_eq!(key_owner(None, true, false, false, Owner::Page), Owner::Address);
        assert_eq!(key_owner(None, false, false, false, Owner::Binding), Owner::Binding);
        // 13y1 (S3): a member's own key after the bindings and the bar, before the address keys.
        assert_eq!(key_owner(None, true, true, false, Owner::Page), Owner::Workgroup);
        assert_eq!(key_owner(None, false, true, false, Owner::Binding), Owner::Binding, "a global binding wins (P13)");
        assert_eq!(key_owner(Some(1), false, true, false, Owner::Page), Owner::Bar(1));
        assert_eq!(key_owner(None, false, true, false, Owner::Number), Owner::Number, "a number route never reaches the member's keys");
        assert!(Owner::Binding.taken() && Owner::Bar(0).taken() && Owner::Address.taken() && Owner::Number.taken() && Owner::Leader.taken() && Owner::Workgroup.taken() && Owner::CopyMode.taken());
        assert!(!Owner::Page.taken());
    }

    /// 13x3 Task 8 (B5): copy mode is one position in the one order, after the address keys and before
    /// the number shortcuts and the leader, and a binding or a bar button beats it.
    #[test]
    fn copy_mode_has_one_place_in_the_owner_order() {
        assert_eq!(key_owner(Some(0), false, false, true, Owner::Page), Owner::Bar(0), "the bar first");
        assert_eq!(key_owner(None, false, false, true, Owner::Binding), Owner::Binding, "a person's binding beats copy mode");
        assert_eq!(key_owner(None, true, false, true, Owner::Page), Owner::Address, "the address keys beat it");
        assert_eq!(key_owner(None, false, false, true, Owner::Page), Owner::CopyMode);
        assert_eq!(key_owner(None, false, false, true, Owner::Number), Owner::CopyMode, "and it beats the number shortcuts");
        assert_eq!(key_owner(None, false, false, true, Owner::Leader), Owner::CopyMode);
        assert_eq!(key_owner(None, false, false, false, Owner::Page), Owner::Page);
        assert!(Owner::CopyMode.taken());
        // Through the gate: plain keys, with copy mode on; a binding on Ctrl+B still wins; Win never; a
        // modifier alone is never taken.
        let on = GateState { copy_mode: true, ..GateState::default() };
        assert_eq!(on.owner(0x4A, 0), Owner::CopyMode, "j");
        assert_eq!(on.owner(0x1B, 0), Owner::CopyMode, "Esc");
        assert_eq!(on.owner(0x4A, WIN), Owner::Page, "a Win chord is not captured");
        assert_eq!(on.owner(0x10, SHIFT), Owner::Page, "Shift alone");
        assert_eq!(on.owner(0x4C, CTRL), Owner::Address, "Ctrl+L");
        let off = GateState::default();
        assert_eq!(off.owner(0x4A, 0), Owner::Page);
        let bound = GateState { copy_mode: true, profile_map: table(&[("0x62-0x40000", 0)]), ..GateState::default() };
        assert_eq!(bound.owner(0x42, CTRL), Owner::Binding, "Ctrl+B bound by the person");
    }

    /// 13x3 final wave (Task 8 minor 1): the system keys are not copy mode's. Alt+F4 closes the
    /// window and F10 opens the system menu bar even while copy mode is on, as the terminal's does.
    #[test]
    fn system_keys_are_never_copy_modes() {
        let on = GateState { copy_mode: true, ..GateState::default() };
        assert_eq!(on.owner(0x73, ALT), Owner::Page, "Alt+F4");
        assert_eq!(on.owner(0x79, 0), Owner::Page, "F10");
        assert_eq!(on.owner(0x79, SHIFT), Owner::Page, "Shift+F10");
        assert_eq!(on.owner(0x73, 0), Owner::CopyMode, "F4 alone is an ordinary key");
        assert_eq!(on.owner(0x73, CTRL), Owner::CopyMode, "Ctrl+F4 is not the system close");
        assert_eq!(on.owner(0x75, 0), Owner::Address, "F6 stays the address key in every branch");
    }

    /// The ruled order, whole (13y1 and 13x3 agree, 13y1 merge review ruling a): Bar, the person's
    /// Binding, Workgroup, Address, CopyMode, then the rest. Every flag on at once, each one dropped in
    /// turn, gives the next owner down; and through the gate a member key beats copy mode.
    #[test]
    fn the_full_owner_order_after_the_13x3_merge() {
        // (bar, address, workgroup, copy_mode, mapped)
        assert_eq!(key_owner(Some(3), true, true, true, Owner::Binding), Owner::Bar(3));
        assert_eq!(key_owner(None, true, true, true, Owner::Binding), Owner::Binding);
        assert_eq!(key_owner(None, true, true, true, Owner::Page), Owner::Workgroup);
        assert_eq!(key_owner(None, true, false, true, Owner::Page), Owner::Address);
        assert_eq!(key_owner(None, false, false, true, Owner::Page), Owner::CopyMode);
        assert_eq!(key_owner(None, false, false, false, Owner::Page), Owner::Page);
        // The rest (Number, Leader) only after copy mode; a member key never claims a routed key.
        assert_eq!(key_owner(None, false, true, true, Owner::Number), Owner::CopyMode);
        assert_eq!(key_owner(None, false, true, false, Owner::Leader), Owner::Leader);
        // Through the one gate.
        let ctrl_m = keyroute::press_from_chord(&chord_of(0x4D, CTRL).unwrap()).stroke.serialized();
        let g = GateState { copy_mode: true, workgroup_keys: vec![crate::workgroup::Shortcut { serialized: ctrl_m }], profile_map: table(&[("0x62-0x40000", 0)]), ..GateState::default() };
        assert_eq!(g.owner(0x4D, CTRL), Owner::Workgroup, "a member's key beats copy mode");
        assert_eq!(g.owner(0x42, CTRL), Owner::Binding, "the person's binding beats both");
        assert_eq!(g.owner(0x4C, CTRL), Owner::Address, "Ctrl+L beats copy mode");
        assert_eq!(g.owner(0x4A, 0), Owner::CopyMode, "j");
        assert_eq!(g.owner(0x73, ALT), Owner::Page, "Alt+F4 is never copy mode's");
    }

    #[test]
    fn the_gate_offers_a_members_own_keys() {
        // 13y1 seam S3: a member's peer or toolbar shortcut is Tandem's, not the page's.
        let ctrl_m = keyroute::press_from_chord(&chord_of(0x4D, CTRL).unwrap()).stroke.serialized();
        let peer = crate::workgroup::Shortcut { serialized: ctrl_m };
        let mut g = GateState { profile_map: table(&[("0x6a-0x40000", 0)]), ..GateState::default() };
        assert_eq!(g.owner(0x4D, CTRL), Owner::Page, "not a member: the page's");
        g.workgroup_keys = vec![peer];
        assert_eq!(g.owner(0x4D, CTRL), Owner::Workgroup);
        assert_eq!(g.owner(0x4E, CTRL), Owner::Page, "another key stays the page's");
        assert_eq!(g.owner(0x4A, CTRL), Owner::Binding, "a mapping still wins");
    }

    #[test]
    fn keys_run_as_iterm2() {
        let none = Facts::default();
        assert_eq!(plan(&KeyAct::Scroll(Scroll::Down), "", &none), vec![Do::PageScript("window.scrollBy(0, 40);".into())]);
        assert_eq!(plan(&KeyAct::Send { text: "AB".into(), broadcast: true }, "", &none), vec![Do::Insert("AB".into())]);
        let sel = Facts { has_selection: true, clipboard_text: true };
        assert_eq!(plan(&KeyAct::CopyOrSend, "c", &sel), vec![Do::Copy]);
        assert_eq!(plan(&KeyAct::CopyOrSend, "c", &none), vec![Do::Insert("c".into())]);
        assert_eq!(plan(&KeyAct::PasteOrSend, "v", &sel), vec![Do::Paste]);
        assert_eq!(plan(&KeyAct::PasteOrSend, "v", &none), vec![Do::Insert("v".into())]);
        assert_eq!(plan(&KeyAct::Session, "", &none), vec![Do::Session]);
        assert_eq!(plan(&KeyAct::Handled, "", &none), Vec::<Do>::new());
        assert_eq!(plan(&KeyAct::Extend { start: true, forward: true, unit: 3 }, "", &none), vec![Do::WorldScript(extend_script(true, true, 3).unwrap())]);
        assert_eq!(plan(&KeyAct::Extend { start: true, forward: true, unit: 4 }, "", &none), Vec::<Do>::new(), "mark is skipped");
        assert_eq!(plan(&KeyAct::CopyMode("vv".into()), "", &none), vec![Do::CopyMode("vv".into())]);
        assert_eq!(plan(&KeyAct::ToggleSetting("j".into()), "", &none), vec![Do::Toggle("j".into())]);
        assert_eq!(plan(&KeyAct::Snippet("k".into()), "", &none), vec![Do::Snippet("k".into())]);
        assert_eq!(plan(&KeyAct::PasteSpecial { config: "c".into(), from_selection: true }, "", &none), vec![Do::PasteSpecial { config: "c".into(), from_selection: true }]);
        // The character a key sends is the event's characters (`BAP:171`): with no layout text, the
        // chord's own letter, upper case with Shift, and a Ctrl letter its control character
        // (deferred minor 207: Copy or Send on Ctrl+C sends U+0003, as iTerm2).
        let ch = |key, shift, ctrl| Chord { key, cmd: false, alt: false, shift, ctrl };
        assert_eq!(own_char(&ch(ChordKey::Char('c'), false, true), None), Some("\u{3}".into()));
        assert_eq!(own_char(&ch(ChordKey::Char('v'), false, true), None), Some("\u{16}".into()));
        assert_eq!(own_char(&ch(ChordKey::Char('['), false, true), None), Some("\u{1b}".into()));
        assert_eq!(own_char(&ch(ChordKey::Char('c'), true, false), None), Some("C".into()));
        assert_eq!(own_char(&ch(ChordKey::Left, false, false), None), None);
        assert_eq!(own_char(&ch(ChordKey::Space, false, false), None), Some(" ".into()));
        assert_eq!(own_char(&ch(ChordKey::Enter, false, false), None), Some("\r".into()));
        // The layout's text wins when the key types one: Shift+1 is "!" on a US layout, not "1".
        assert_eq!(own_char(&ch(ChordKey::Char('1'), true, false), Some("!")), Some("!".into()));
        assert_eq!(own_char(&ch(ChordKey::Char('c'), false, true), Some("\u{3}")), Some("\u{3}".into()));
        assert_eq!(own_char(&ch(ChordKey::Char('1'), true, false), Some("")), Some("1".into()), "an empty layout text is no text");
    }

    #[test]
    fn broadcast_follows_iterm2() {
        use serde_json::json;
        assert_eq!(broadcast_of(&json!({"t": "insert", "text": "ab"})), Some(BroadcastEvent::Insert("ab".into())));
        assert_eq!(
            broadcast_of(&json!({"t": "key", "key": "Backspace", "code": "Backspace", "keyCode": 8, "mods": 0})),
            Some(BroadcastEvent::Key { key: "Backspace".into(), code: "Backspace".into(), key_code: 8, mods: 0 })
        );
        // Other shapes are nothing: an unknown kind, an empty or non-string text, a key outside the
        // editing set, a code with odd characters, a key code or mods out of range.
        for bad in [
            json!({}),
            json!({"t": "other"}),
            json!({"t": "insert"}),
            json!({"t": "insert", "text": ""}),
            json!({"t": "insert", "text": 5}),
            json!({"t": "key", "key": "a", "code": "KeyA", "keyCode": 65, "mods": 0}),
            json!({"t": "key", "key": "Enter", "code": "Ent er", "keyCode": 13, "mods": 0}),
            json!({"t": "key", "key": "Enter", "code": "Enter", "keyCode": 999, "mods": 0}),
            json!({"t": "key", "key": "Enter", "code": "Enter", "keyCode": 13, "mods": 16}),
            json!({"t": "key", "key": "Enter", "code": "Enter", "keyCode": 13}),
        ] {
            assert_eq!(broadcast_of(&bad), None, "{bad}");
        }
        assert_eq!(broadcast_of(&json!({"t": "insert", "text": "x".repeat(MAX_BROADCAST_TEXT + 1)})), None, "bounded");
        // The replay: text through Input.insertText, a key as rawKeyDown then keyUp.
        assert_eq!(replay_calls(&BroadcastEvent::Insert("ab".into())), vec![("Input.insertText", json!({"text": "ab"}))]);
        let enter = BroadcastEvent::Key { key: "Enter".into(), code: "Enter".into(), key_code: 13, mods: 0 };
        let calls = replay_calls(&enter);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "Input.dispatchKeyEvent");
        // Review I2 (iTerm2's `insertNewline:` submits forms): Enter is a keyDown with text "\r", never a
        // raw key (Blink drives implicit submission and the line break from the char it makes).
        assert_eq!(calls[0].1["type"], "keyDown");
        assert_eq!(calls[0].1["text"], "\r");
        assert_eq!(calls[0].1["unmodifiedText"], "\r");
        assert_eq!(calls[0].1["windowsVirtualKeyCode"], 13);
        let tab = BroadcastEvent::Key { key: "Tab".into(), code: "Tab".into(), key_code: 9, mods: 0 };
        assert_eq!(replay_calls(&tab)[0].1["type"], "rawKeyDown", "the other keys stay raw keys");
        assert!(replay_calls(&tab)[0].1.get("text").is_none());
        assert_eq!(calls[0].1["key"], "Enter");
        assert_eq!(calls[0].1["code"], "Enter");
        assert_eq!(calls[0].1["modifiers"], 0);
        assert_eq!(calls[1].1["type"], "keyUp");
        assert_eq!(calls[1].1["windowsVirtualKeyCode"], 13);
        // Tandem's bits (Shift 1, Ctrl 2, Alt 4, Win 8) become DevTools' (Alt 1, Ctrl 2, Meta 4, Shift 8).
        let all = BroadcastEvent::Key { key: "Tab".into(), code: "Tab".into(), key_code: 9, mods: SHIFT | CTRL | ALT | WIN };
        assert_eq!(replay_calls(&all)[0].1["modifiers"], 15);
        for (mods, want) in [(SHIFT, 8), (CTRL, 2), (ALT, 1), (WIN, 4), (SHIFT | ALT, 9), (0, 0)] {
            let e = BroadcastEvent::Key { key: "Tab".into(), code: "Tab".into(), key_code: 9, mods };
            assert_eq!(replay_calls(&e)[1].1["modifiers"], want, "{mods}");
        }
        assert_eq!(BROADCAST_HANDLER, "tandemBroadcastInput");
    }

    fn peer(browser: bool, dev_null: bool) -> Option<Peer> {
        Some(Peer { browser, live: true, shown: true, dev_null })
    }

    #[test]
    fn broadcast_reaches_other_browsers() {
        use crate::broadcast::{Broadcast, Mode, TabView, View};
        use std::collections::BTreeSet;
        // Browser A (1), browser B (2), terminal C (3) in one tab, all in one domain (All Panes).
        let v = View { tabs: vec![TabView { broadcasting: true, panes: vec![1, 2, 3] }], current_tab: 0, current: 1, exited: BTreeSet::new() };
        let b = Broadcast { raw: Mode::AllPanes, ..Default::default() };
        let kinds = |p: u64| match p {
            1 | 2 => peer(true, false),
            3 => peer(false, false),
            _ => None,
        };
        let domain = b.targets(&v, 1);
        assert!(domain.contains(&3), "the model counts the terminal; the browser filter removes it");
        assert_eq!(broadcast_peers(&domain, 1, kinds), vec![2], "browsers other than the sender; not the terminal (PSB:400)");
        // Only the domain's panes: a browser outside it gets nothing.
        let outside = View { tabs: vec![TabView { broadcasting: true, panes: vec![1, 3] }, TabView { broadcasting: false, panes: vec![2] }], current_tab: 0, current: 1, exited: BTreeSet::new() };
        assert_eq!(broadcast_peers(&b.targets(&outside, 1), 1, kinds), Vec::<u64>::new());
        // Broadcast off: the sender alone, so nobody.
        assert_eq!(broadcast_peers(&Broadcast::default().targets(&v, 1), 1, kinds), Vec::<u64>::new());
        // P18: another privacy class, a view that is not live or not shown are skipped.
        let privacy = |p: u64| match p {
            1 => peer(true, false),
            2 => peer(true, true),
            4 => Some(Peer { browser: true, live: false, shown: true, dev_null: false }),
            5 => Some(Peer { browser: true, live: true, shown: false, dev_null: false }),
            6 => peer(true, false),
            _ => None,
        };
        assert_eq!(broadcast_peers(&[1, 2, 4, 5, 6], 1, privacy), vec![6], "Dev Null never crosses to a persistent profile (nor back), buried and hidden views are skipped");
        let dev = |p: u64| peer(true, p == 1 || p == 2);
        assert_eq!(broadcast_peers(&[1, 2, 3], 1, dev), vec![2], "Dev Null panes broadcast among themselves");
        // The sender takes a message only when it sends and no replay is in flight to it.
        assert!(broadcast_accepts(true, 0, None));
        assert!(!broadcast_accepts(true, 1, None), "a replay in flight: its own events are dropped, nothing ping-pongs");
        assert!(!broadcast_accepts(false, 0, None), "not a broadcast source");
        // The receiving count is a counter: raised before the first call, lowered when the last answers.
        assert_eq!(receiving_raise(0), 1);
        assert_eq!(receiving_raise(u32::MAX), u32::MAX);
        assert_eq!(receiving_lower(2), 1);
        assert_eq!(receiving_lower(0), 0);
    }

    #[test]
    fn broadcast_script_follows_the_rules() {
        let js = resource("broadcast-input.js").unwrap();
        assert!(js.contains("Tandem's own script"));
        assert!(js.contains("tandemBroadcastInput") && js.contains("{{SECRET}}") && js.contains("{{SELECTOR}}"));
        // Every listener checks isTrusted first (P26), and a field that holds a secret sends nothing (P18).
        let mut at = 0;
        let mut listeners = 0;
        while let Some(i) = js[at..].find("addEventListener(") {
            let tail = &js[at + i..];
            assert!(tail[..tail.len().min(160)].contains("if (!e.isTrusted)"), "a listener without the isTrusted check");
            listeners += 1;
            at += i + 1;
        }
        assert_eq!(listeners, 2, "beforeinput and keydown");
        assert!(js.contains("tandemSecretField(t, SENSITIVE)") && js.contains("composedPath"));
        assert!(!js.contains("insertLineBreak"), "Enter travels as its key only: a line break in a textarea is made by the replayed Enter, never a second time by an insert");
        for t in ["insertText", "insertFromPaste"] {
            assert!(js.contains(t), "{t}");
        }
        for k in ["Backspace", "Delete", "Enter", "Tab", "Escape", "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"] {
            assert!(js.contains(k), "{k}");
        }
        let made = user_script("S1");
        assert_eq!(made.world, crate::browser_js::World::Client);
        assert!(made.main_frame_only && made.code.contains("S1") && !made.code.contains("{{"));
        assert!(made.code.contains("input[type=password i]"), "the selector is filled in");
        // The receiving side asks before it types: a focused sensitive field, or a frame, gets nothing.
        let g = guard_body();
        assert!(g.contains("activeElement") && g.contains("shadowRoot") && g.contains("IFRAME") && g.contains("catch (e) { return true; }"));
        assert!(g.contains("input[type=password i]") && g.is_ascii());
    }

    /// Review I3 (password leak): the secret check runs in the isolated world on both sides and covers
    /// masked text fields, every autocomplete secret token, password inputs and any focus that is not
    /// itself a known editable (a closed shadow root's host, a frame, a button).
    #[test]
    fn masked_and_closed_fields_are_secret() {
        let f = |tag, kind, ac, ts, editable| field_is_secret(&FieldFacts { tag, input_type: kind, autocomplete: ac, text_security: ts, editable });
        assert!(f("INPUT", "password", "", "none", true), "a password input");
        assert!(f("INPUT", "PassWord", "", "", true));
        assert!(f("INPUT", "text", "", "disc", true), "a masked text field (PIN, bank login)");
        assert!(f("INPUT", "text", "", "circle", true) && f("TEXTAREA", "", "", "square", true));
        for ac in ["current-password", "new-password", "one-time-code", "cc-number", "CC-CSC", "username current-password"] {
            assert!(f("INPUT", "text", ac, "none", true), "{ac}");
            assert!(f("TEXTAREA", "", ac, "none", true), "{ac} on a textarea");
        }
        assert!(f("DIV", "", "", "none", false), "the host of a closed shadow root is not an editable: fail closed");
        assert!(f("IFRAME", "", "", "none", false) && f("FRAME", "", "", "none", false));
        assert!(f("BUTTON", "", "", "none", false));
        assert!(!f("INPUT", "text", "", "none", true) && !f("TEXTAREA", "", "off", "none", true) && !f("DIV", "", "", "none", true), "a plain editable is not secret");
        assert!(!f("BODY", "", "", "none", false) && !f("HTML", "", "", "none", false), "nothing focused");
        // The script text states the same rule, in both places.
        let js = FIELD_CHECK_JS;
        for t in ["webkitTextSecurity", "current-password", "new-password", "one-time-code", "'cc-'", "isContentEditable", "'IFRAME'", "'password'", "shadowRoot"] {
            assert!(js.contains(t), "{t}");
        }
        assert!(js.is_ascii());
        let made = user_script("S1").code;
        assert!(made.contains("webkitTextSecurity") && made.contains("tandemSecretField(") && made.contains("tandemDeepest()") && !made.contains("{{"));
        let g = guard_body();
        assert!(g.contains("webkitTextSecurity") && g.contains("tandemSecretField(tandemDeepest(), SEL)") && g.contains("catch (e) { return true; }"));
        let src = resource("broadcast-input.js").unwrap();
        assert!(src.contains("{{FIELD_CHECK}}"));
    }

    /// Review I1: suppression never depends on the order of the replay's completion and the page's
    /// own message: a hold-off after the last replay, and an echo guard that drops the very input a
    /// pane was sent.
    #[test]
    fn two_source_panes_never_bounce() {
        use std::time::{Duration, Instant};
        // The hold-off: nothing is taken for 250 ms after the last replay ended, count or no count.
        assert_eq!(HOLD_OFF, Duration::from_millis(250));
        assert!(!broadcast_accepts(true, 0, Some(Duration::from_millis(100))), "inside the hold-off");
        assert!(broadcast_accepts(true, 0, Some(Duration::from_millis(250))));
        assert!(!broadcast_accepts(true, 1, Some(Duration::from_secs(1))), "a replay still in flight");
        // The echo guard: what a pane was sent is dropped once when its own page reports it, even after
        // the hold-off, and a later identical input of the person is not lost.
        let t0 = Instant::now();
        let a = BroadcastEvent::Insert("a".into());
        let enter = BroadcastEvent::Key { key: "Enter".into(), code: "Enter".into(), key_code: 13, mods: 0 };
        let mut b = EchoGuard::default();
        b.note(&a, t0);
        b.note(&enter, t0);
        assert!(!b.is_echo(&BroadcastEvent::Insert("b".into()), t0), "another text is the person's");
        assert!(b.is_echo(&a, t0 + Duration::from_millis(400)), "the echo, after the hold-off");
        assert!(!b.is_echo(&a, t0 + Duration::from_millis(500)), "once: the next 'a' is the person's");
        assert!(b.is_echo(&enter, t0 + Duration::from_secs(1)));
        b.note(&a, t0);
        assert!(!b.is_echo(&a, t0 + ECHO_TTL + Duration::from_millis(1)), "an echo that never came expires");
        // Bounded.
        for i in 0..100 {
            b.note(&BroadcastEvent::Insert(i.to_string()), t0);
        }
        assert!(b.len() <= ECHO_MAX);
        // Two panes that both send: A is typed in, B is sent it and reports it, A is then sent nothing.
        let mut pane_b = EchoGuard::default();
        pane_b.note(&a, t0);
        assert!(pane_b.is_echo(&a, t0 + Duration::from_millis(5)), "B's report of the replayed input is dropped, so A never gets it back");
    }

    /// Final review minor 5: an echo that arrives inside the hold-off is consumed there, so the
    /// person's next identical key in that pane is not dropped; and a noted input that was never made
    /// (the target's guard refused, a step failed) is forgotten.
    #[test]
    fn the_hold_off_never_drops_a_persons_key() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let a = BroadcastEvent::Insert("a".into());
        let mut g = EchoGuard::default();
        g.note(&a, t0);
        assert!(!broadcast_verdict(&mut g, &a, t0 + Duration::from_millis(60), true, 0, Some(Duration::from_millis(50))), "the echo, inside the hold-off");
        assert!(g.is_empty(), "consumed there");
        assert!(broadcast_verdict(&mut g, &a, t0 + Duration::from_millis(600), true, 0, Some(Duration::from_millis(550))), "the person's own 'a' a moment later is sent");
        // A replay the guard refused (or that failed) made no input: no echo comes, nothing is dropped.
        g.note(&a, t0);
        g.forget(&a);
        assert!(g.is_empty());
        assert!(broadcast_verdict(&mut g, &a, t0 + Duration::from_millis(600), true, 0, Some(Duration::from_millis(550))));
        // forget takes one note only.
        g.note(&a, t0);
        g.note(&a, t0);
        g.forget(&a);
        assert_eq!(g.len(), 1);
    }

    /// Final residual 2 (deferred minor 221): a replay stopped because its target left the
    /// broadcast set forgets the echo it noted, so the person's identical key in that pane after it
    /// rejoins is not dropped; a replay that ran all its calls keeps the note for its echo.
    #[test]
    fn a_replay_whose_target_left_forgets_its_echo() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let a = BroadcastEvent::Insert("a".into());
        let mut g = EchoGuard::default();
        g.note(&a, t0);
        assert!(replay_continues(&mut g, &a, true, true), "still a target, calls left: the next call runs");
        assert_eq!(g.len(), 1, "the echo is still expected");
        assert!(!replay_continues(&mut g, &a, true, false), "the target left: the replay stops");
        assert!(g.is_empty(), "and forgets the echo no input will make");
        // The pane rejoins within ECHO_TTL: the person's own 'a' there is sent, not taken for an echo.
        assert!(broadcast_verdict(&mut g, &a, t0 + Duration::from_millis(600), true, 0, Some(Duration::from_millis(550))));
        // A replay that ran every call made its input: the note stays for the echo.
        g.note(&a, t0);
        assert!(!replay_continues(&mut g, &a, false, true));
        assert!(!replay_continues(&mut g, &a, false, false), "done, even if the target left after the last call");
        assert_eq!(g.len(), 1);
        assert!(!broadcast_verdict(&mut g, &a, t0 + Duration::from_millis(600), true, 0, Some(Duration::from_millis(550))), "its echo is dropped once");
    }
}
