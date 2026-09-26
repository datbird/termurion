//! What one key press does, in iTerm2's order (iTermApplication handleKeyDownEvent:):
//! the Leader, the window, pane, and tab number shortcuts, then the profile's key
//! mappings, then the global ones, and last the terminal.

use std::collections::HashSet;

use crate::keyaction::Binding;
use crate::keystroke::{self, COMMAND, CONTROL, LEADER, OPTION, SHIFT, Keystroke};
use crate::menus::{Chord, ChordKey};

pub struct Context<'a> {
    pub profile_map: &'a toml::Table,
    pub global_map: &'a toml::Table,
    pub language_agnostic: bool,
    pub leader: Option<Keystroke>,
    /// The Leader was pressed; this key gets the Leader flag.
    pub leader_on: bool,
    /// SwitchTabModifier, SwitchPaneModifier, SwitchWindowModifier tags.
    pub switch_tab: i64,
    pub switch_pane: i64,
    pub switch_window: i64,
}

pub struct Press {
    pub stroke: Keystroke,
    /// The number key, 0-9: the typed digit, or the U.S. digit key with Emulate U.S. Keyboard.
    pub digit: Option<u8>,
}

#[derive(Debug)]
pub enum Route {
    LeaderOn,
    /// Digit 1-9; 9 means the last one.
    SelectWindow(u8),
    SelectPane(u8),
    SelectTab(u8),
    Binding(Binding),
    Terminal,
}

/// The modifiers a navigation tag asks for (iTermPreferences maskForModifierTag:).
pub fn tag_mask(tag: i64) -> Option<u64> {
    match tag {
        1 => Some(CONTROL),
        4 => Some(COMMAND),
        5 => Some(OPTION),
        6 => Some(COMMAND | OPTION),
        // None, and Function, which Windows cannot see.
        9 | 10 => None,
        _ => Some(COMMAND | OPTION),
    }
}

const MODS: u64 = SHIFT | CONTROL | OPTION | COMMAND;

fn number(tag: i64, p: &Press) -> Option<u8> {
    let d = p.digit.filter(|d| (1..=9).contains(d))?;
    (tag_mask(tag)? == p.stroke.flags & MODS).then_some(d)
}

/// A mapping that runs as an action. Do Not Remap Modifiers and Remap Modifiers in
/// iTerm2 Only only guide the remapper.
fn mapping(map: &toml::Table, stroke: &Keystroke, agnostic: bool) -> Option<Binding> {
    let b = Binding::from_value(map.get(stroke.find(map, agnostic)?)?)?;
    (!matches!(b.action, 22 | 24)).then_some(b)
}

pub fn route(ctx: &Context, p: &Press) -> Route {
    let mut stroke = p.stroke.clone();
    if ctx.leader_on {
        stroke.flags |= LEADER;
    } else if ctx.leader.as_ref().is_some_and(|l| l.same_key(&stroke)) {
        return Route::LeaderOn;
    }
    if let Some(d) = number(ctx.switch_window, p) {
        return Route::SelectWindow(d);
    }
    if let Some(d) = number(ctx.switch_pane, p) {
        return Route::SelectPane(d);
    }
    if let Some(d) = number(ctx.switch_tab, p) {
        return Route::SelectTab(d);
    }
    mapping(ctx.profile_map, &stroke, ctx.language_agnostic)
        .or_else(|| mapping(ctx.global_map, &stroke, ctx.language_agnostic))
        .map_or(Route::Terminal, Route::Binding)
}

/// Shift's character on a U.S. keyboard. The hook knows only the key, so a Win+Shift
/// chord matches a binding stored with the shifted character through this table.
fn us_shifted(c: char) -> char {
    const PAIRS: &str = "1!2@3#4$5%6^7&8*9(0)-_=+[{]}\\|;:'\",<.>/?`~";
    let chars: Vec<char> = PAIRS.chars().collect();
    chars.chunks(2).find(|p| p[0] == c).map_or(c.to_ascii_uppercase(), |p| p[1])
}

/// The press a Win chord from the hook stands for.
pub fn press_from_chord(c: &Chord) -> Press {
    use crate::keys::Key;
    let key = match c.key {
        ChordKey::Char(ch) => Key::Text((if c.shift { us_shifted(ch) } else { ch }).to_string()),
        ChordKey::Up => Key::Up,
        ChordKey::Down => Key::Down,
        ChordKey::Left => Key::Left,
        ChordKey::Right => Key::Right,
        ChordKey::Home => Key::Home,
        ChordKey::End => Key::End,
        ChordKey::PageUp => Key::PageUp,
        ChordKey::PageDown => Key::PageDown,
        ChordKey::Delete => Key::Delete,
        ChordKey::F(n) => Key::F(n),
        ChordKey::Enter => Key::Enter,
        ChordKey::Tab => Key::Tab,
        ChordKey::Escape => Key::Escape,
        ChordKey::Backspace => Key::Backspace,
        ChordKey::Space => Key::Text(" ".into()),
    };
    let flags = keystroke::from_mods(crate::mouse::Mods { shift: c.shift, alt: c.alt, ctrl: c.ctrl, cmd: c.cmd });
    let ch = keystroke::mac_char(&key, c.shift, false);
    let digit = match c.key {
        ChordKey::Char(d) if !c.shift => d.to_digit(10).map(|d| d as u8),
        _ => None,
    };
    Press { stroke: Keystroke::new(ch, flags, None, ch), digit }
}

/// What the keyboard hook needs to decide, on its own thread, whether Tandem takes a
/// Win chord.
#[derive(Clone, Default)]
pub struct Snapshot {
    /// Legacy and typed-character forms of every mapping with Win in it.
    keys: HashSet<String>,
    leader: Option<Keystroke>,
    leader_on: bool,
    switch: [i64; 3],
    /// A composer has the keyboard: its own Win chords (`composer::chord`) are Tandem's even
    /// when nothing else binds them (Win+Y).
    pub composer: bool,
}

impl Snapshot {
    pub fn new(ctx: &Context) -> Snapshot {
        let mut keys = HashSet::new();
        for map in [ctx.profile_map, ctx.global_map] {
            for (k, v) in map {
                let (Some(stroke), Some(b)) = (keystroke::parse(k), Binding::from_value(v)) else { continue };
                if stroke.flags & COMMAND != 0 && !matches!(b.action, 22 | 24) {
                    keys.insert(stroke.legacy());
                    keys.insert(stroke.modified_key());
                }
            }
        }
        Snapshot { keys, leader: ctx.leader.clone(), leader_on: ctx.leader_on, switch: [ctx.switch_window, ctx.switch_pane, ctx.switch_tab], composer: false }
    }
}

/// True when Tandem has something for this Win chord: the Leader, a number shortcut,
/// a key mapping, or a working menu command.
pub fn win_chord_bound(s: &Snapshot, chord: &Chord) -> bool {
    if s.leader_on || (s.composer && crate::composer::chord_of(chord).is_some()) {
        return true;
    }
    let p = press_from_chord(chord);
    if s.leader.as_ref().is_some_and(|l| l.same_key(&p.stroke)) || s.switch.iter().any(|t| number(*t, &p).is_some()) {
        return true;
    }
    let mut typed = p.stroke.clone();
    typed.char_is_modified = true;
    s.keys.contains(&p.stroke.legacy()) || s.keys.contains(&typed.modified_key()) || crate::commands::command_for_chord(chord).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keystroke::{COMMAND, CONTROL, OPTION, SHIFT};
    use crate::menus::{Chord, ChordKey};

    fn ctx<'a>(p: &'a toml::Table, g: &'a toml::Table) -> Context<'a> {
        Context { profile_map: p, global_map: g, language_agnostic: false, leader: None, leader_on: false, switch_tab: 4, switch_pane: 9, switch_window: 6 }
    }
    fn press(ch: u32, flags: u64) -> Press {
        Press { stroke: Keystroke::new(ch, flags, None, ch), digit: char::from_u32(ch).and_then(|c| c.to_digit(10)).map(|d| d as u8) }
    }
    fn map(key: &str, action: i64) -> toml::Table {
        toml::from_str(&format!("\"{key}\" = {{ Action = {action}, Text = \"\" }}")).unwrap()
    }
    fn win(key: ChordKey) -> Chord {
        Chord { key, cmd: true, alt: false, shift: false, ctrl: false }
    }

    #[test]
    fn the_profile_map_wins_over_the_global_map() {
        let (p, g) = (map("0x61-0x100000", 13), map("0x61-0x100000", 0));
        assert!(matches!(route(&ctx(&p, &g), &press(0x61, COMMAND)), Route::Binding(b) if b.action == 13));
        let e = toml::Table::new();
        assert!(matches!(route(&ctx(&e, &g), &press(0x61, COMMAND)), Route::Binding(b) if b.action == 0));
        assert!(matches!(route(&ctx(&e, &e), &press(0x61, COMMAND)), Route::Terminal));
    }

    #[test]
    fn remap_markers_are_not_actions() {
        let e = toml::Table::new();
        let g = map("0x61-0x100000", 22);
        assert!(matches!(route(&ctx(&e, &g), &press(0x61, COMMAND)), Route::Terminal));
    }

    #[test]
    fn win_number_selects_a_tab() {
        let e = toml::Table::new();
        assert!(matches!(route(&ctx(&e, &e), &press(0x33, COMMAND)), Route::SelectTab(3)));
        assert!(matches!(route(&ctx(&e, &e), &press(0x33, COMMAND | SHIFT)), Route::Terminal));
        assert!(matches!(route(&ctx(&e, &e), &press(0x33, COMMAND | OPTION)), Route::SelectWindow(3)));
        assert!(matches!(route(&ctx(&e, &e), &press(0x30, COMMAND)), Route::Terminal));
    }

    #[test]
    fn number_shortcuts_beat_mappings() {
        let e = toml::Table::new();
        let g = map("0x33-0x100000", 13);
        assert!(matches!(route(&ctx(&e, &g), &press(0x33, COMMAND)), Route::SelectTab(3)));
    }

    #[test]
    fn a_pane_shortcut_needs_its_setting() {
        let e = toml::Table::new();
        let mut c = ctx(&e, &e);
        c.switch_pane = 1; // Ctrl+Number
        assert!(matches!(route(&c, &press(0x32, CONTROL)), Route::SelectPane(2)));
        c.switch_pane = 9;
        assert!(matches!(route(&c, &press(0x32, CONTROL)), Route::Terminal));
    }

    #[test]
    fn the_leader_marks_the_next_key() {
        let e = toml::Table::new();
        let g = map("0x61-0x1000000", 13);
        let mut c = ctx(&e, &g);
        c.leader = Some(Keystroke::new(0x62, CONTROL, None, 0x62));
        assert!(matches!(route(&c, &press(0x62, CONTROL)), Route::LeaderOn));
        assert!(matches!(route(&c, &press(0x61, 0)), Route::Terminal));
        c.leader_on = true;
        assert!(matches!(route(&c, &press(0x61, 0)), Route::Binding(b) if b.action == 13));
        // An unbound key after the leader goes to the terminal as usual.
        assert!(matches!(route(&c, &press(0x7a, 0)), Route::Terminal));
    }

    #[test]
    fn tag_masks_follow_iterm2() {
        assert_eq!(tag_mask(1), Some(CONTROL));
        assert_eq!(tag_mask(4), Some(COMMAND));
        assert_eq!(tag_mask(5), Some(OPTION));
        assert_eq!(tag_mask(6), Some(COMMAND | OPTION));
        assert_eq!(tag_mask(9), None);
        assert_eq!(tag_mask(2), Some(COMMAND | OPTION));
    }

    #[test]
    fn a_mapped_win_chord_is_bound_for_the_hook() {
        let e = toml::Table::new();
        let j = map("0x79-0x100000", 13);
        let chord = win(ChordKey::Char('y'));
        assert!(win_chord_bound(&Snapshot::new(&ctx(&j, &e)), &chord));
        assert!(!win_chord_bound(&Snapshot::new(&ctx(&e, &e)), &chord));
        // Win+T has a menu command; Win+3 is a tab shortcut.
        assert!(win_chord_bound(&Snapshot::new(&ctx(&e, &e)), &win(ChordKey::Char('t'))));
        assert!(win_chord_bound(&Snapshot::new(&ctx(&e, &e)), &win(ChordKey::Char('3'))));
    }

    #[test]
    fn shifted_win_chords_match_their_stored_character() {
        let e = toml::Table::new();
        let a = map("0x41-0x120000", 13); // Win+Shift+A, stored with the capital
        let bang = map("0x21-0x120000", 13); // Win+Shift+1, stored as "!"
        assert!(win_chord_bound(&Snapshot::new(&ctx(&a, &e)), &Chord { shift: true, ..win(ChordKey::Char('a')) }));
        assert!(win_chord_bound(&Snapshot::new(&ctx(&bang, &e)), &Chord { shift: true, ..win(ChordKey::Char('1')) }));
        assert!(matches!(route(&ctx(&bang, &e), &press_from_chord(&Chord { shift: true, ..win(ChordKey::Char('1')) })), Route::Binding(_)));
    }

    #[test]
    fn with_the_leader_on_every_win_chord_is_bound() {
        let e = toml::Table::new();
        let mut c = ctx(&e, &e);
        c.leader_on = true;
        assert!(win_chord_bound(&Snapshot::new(&c), &win(ChordKey::Char('j'))));
    }
}
