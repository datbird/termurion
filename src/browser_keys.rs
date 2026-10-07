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
    editing && (crate::composer::sends_text(b.action) || b.action == 13 || b.action == 74 || is_navigation(key))
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
    Some(fill_template(resource(&file)?, &[("START", s), ("FORWARD", f)]))
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

/// The character a key types, which Copy or Send and Paste or Send send when they do not copy or
/// paste (`BAP:171`, the event's characters).
pub fn own_char(c: &Chord) -> Option<String> {
    match c.key {
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
    /// A window, pane or tab number shortcut.
    Number,
    Leader,
    Page,
}

impl Owner {
    pub fn taken(self) -> bool {
        !matches!(self, Owner::Page)
    }
}

/// The one order: a bar's button, then a binding the person made (it beats the default address
/// accelerators, as a mapping beats a menu key in iTerm2, `IA:527-548`), then the address
/// accelerators, then the rest of what the key maps say.
pub fn key_owner(bar: Option<usize>, address: bool, mapped: Owner) -> Owner {
    match bar {
        Some(i) => Owner::Bar(i),
        None if mapped == Owner::Binding => Owner::Binding,
        None if address => Owner::Address,
        None => mapped,
    }
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
        let Some(chord) = chord_of(vk, mods) else { return Owner::Page };
        let bar = self.bar.as_ref().filter(|_| shortcut_mods(chord.alt, chord.ctrl, chord.shift, chord.cmd)).and_then(|b| match chord.key {
            ChordKey::Char(c) => b.shortcut(c),
            _ => None,
        });
        let mapped = match self.route(&chord) {
            // Bypass is not actionable (`KBA:617-622`): its key goes on to the page.
            Route::Binding(b) if b.action == 74 => Owner::Page,
            Route::Binding(b) if editing_blocks(&b, &chord.key, self.editing) => Owner::Page,
            Route::Binding(_) => Owner::Binding,
            Route::SelectWindow(_) | Route::SelectPane(_) | Route::SelectTab(_) => Owner::Number,
            Route::LeaderOn => Owner::Leader,
            Route::Terminal => Owner::Page,
        };
        key_owner(bar, address_accelerator(vk, mods), mapped)
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
        assert_eq!(key_owner(Some(2), true, Owner::Binding), Owner::Bar(2));
        assert_eq!(key_owner(None, true, Owner::Binding), Owner::Binding, "a person's own binding beats the default address keys");
        assert_eq!(key_owner(None, true, Owner::Page), Owner::Address);
        assert_eq!(key_owner(None, false, Owner::Binding), Owner::Binding);
        assert!(Owner::Binding.taken() && Owner::Bar(0).taken() && Owner::Address.taken() && Owner::Number.taken() && Owner::Leader.taken());
        assert!(!Owner::Page.taken());
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
        // The character a key sends: the chord's own letter, upper case with Shift.
        assert_eq!(own_char(&Chord { key: ChordKey::Char('c'), cmd: false, alt: false, shift: false, ctrl: true }), Some("c".into()));
        assert_eq!(own_char(&Chord { key: ChordKey::Char('c'), cmd: false, alt: false, shift: true, ctrl: false }), Some("C".into()));
        assert_eq!(own_char(&Chord { key: ChordKey::Left, cmd: false, alt: false, shift: false, ctrl: false }), None);
        assert_eq!(own_char(&Chord { key: ChordKey::Space, cmd: false, alt: false, shift: false, ctrl: false }), Some(" ".into()));
        assert_eq!(own_char(&Chord { key: ChordKey::Enter, cmd: false, alt: false, shift: false, ctrl: false }), Some("\r".into()));
    }
}
