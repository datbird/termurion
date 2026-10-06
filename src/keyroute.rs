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
    /// The keys (A-Z, 0-9) some saved profile's Shortcut claims for Ctrl+Win+<key>
    /// (`claimed_shortcuts`), rebuilt with the Profiles menu.
    pub profile_shortcuts: Vec<char>,
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
        Snapshot { keys, leader: ctx.leader.clone(), leader_on: ctx.leader_on, switch: [ctx.switch_window, ctx.switch_pane, ctx.switch_tab], composer: false, profile_shortcuts: Vec::new() }
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
    s.keys.contains(&p.stroke.legacy()) || s.keys.contains(&typed.modified_key()) || crate::commands::command_for_chord(chord).is_some() || profile_chord(chord).is_some_and(|(k, _)| s.profile_shortcuts.contains(&k))
}

/// The key of a profile shortcut chord: Ctrl+Win+<A-Z or 0-9>, with Alt for the alternate
/// (a new window). Never a bare key, and never with Shift.
pub fn profile_chord(chord: &Chord) -> Option<(char, bool)> {
    match chord.key {
        ChordKey::Char(c) if chord.cmd && chord.ctrl && !chord.shift && c.is_ascii_alphanumeric() => Some((c.to_ascii_uppercase(), chord.alt)),
        _ => None,
    }
}

/// The keys the profiles' shortcuts claim, once each (the first profile wins, `P8`). A key whose
/// Ctrl+Win chord Windows keeps for itself (`winkeys::shell_owned`) is never claimed: a profile
/// shortcut can come from an imported file and must not take a Windows chord.
pub fn claimed_shortcuts(keys: impl IntoIterator<Item = Option<char>>) -> Vec<char> {
    let mut out: Vec<char> = Vec::new();
    for k in keys.into_iter().flatten() {
        let k = k.to_ascii_uppercase();
        if k.is_ascii_alphanumeric() && !windows_keeps(k) && !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

/// Whether Windows keeps Ctrl+Win+`key` (the key on its U.S. position).
fn windows_keeps(key: char) -> bool {
    #[cfg(windows)]
    {
        use crate::hotkey::{Chord as HChord, ChordKey as HKey, MOD_CONTROL, MOD_WIN};
        const ROWS: [(&str, u32); 4] = [("1234567890", 0x02), ("QWERTYUIOP", 0x10), ("ASDFGHJKL", 0x1E), ("ZXCVBNM", 0x2C)];
        let Some(scan) = ROWS.iter().find_map(|(row, first)| row.find(key).map(|i| first + i as u32)) else { return false };
        crate::winkeys::shell_owned(HChord { key: HKey::Scan(scan), mods: MOD_WIN | MOD_CONTROL })
    }
    #[cfg(not(windows))]
    {
        let _ = key;
        false
    }
}

/// What a Win chord does once no key mapping and no number shortcut took it. One place for the
/// order (menu chords, then a profile shortcut) so a later milestone extends it here.
pub enum ChordAction {
    Menu(&'static crate::menus::Item),
    /// Open the profile bound to `key`, in a new window when `window`.
    Profile { key: char, window: bool },
    Nothing,
}

pub fn terminal_chord(shortcuts: &[char], chord: &Chord) -> ChordAction {
    if let Some(item) = crate::commands::command_for_chord(chord) {
        return ChordAction::Menu(item);
    }
    match profile_chord(chord) {
        Some((key, window)) if shortcuts.contains(&key) => ChordAction::Profile { key, window },
        _ => ChordAction::Nothing,
    }
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
        // Win+B has no menu command (Win+Y is Engage AI's since milestone 12e).
        let j = map("0x62-0x100000", 13);
        let chord = win(ChordKey::Char('b'));
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

    fn ctrl_win(c: char) -> Chord {
        Chord { ctrl: true, ..win(ChordKey::Char(c)) }
    }

    #[test]
    fn profile_shortcut_chords_are_bounded() {
        assert_eq!(profile_chord(&ctrl_win('q')), Some(('Q', false)));
        assert_eq!(profile_chord(&Chord { alt: true, ..ctrl_win('7') }), Some(('7', true)));
        assert_eq!(profile_chord(&win(ChordKey::Char('q'))), None, "Ctrl is part of the chord");
        assert_eq!(profile_chord(&Chord { cmd: false, ..ctrl_win('q') }), None, "never a bare key");
        assert_eq!(profile_chord(&Chord { shift: true, ..ctrl_win('q') }), None);
        assert_eq!(profile_chord(&ctrl_win('-')), None);
        assert_eq!(profile_chord(&Chord { key: ChordKey::F(5), ..ctrl_win('q') }), None);
    }

    #[test]
    fn claimed_shortcuts_skip_windows_chords_and_repeats() {
        // Ctrl+Win+D, F, B and C belong to Windows (virtual desktops, Feedback, ...).
        let keys = [Some('Q'), None, Some('q'), Some('D'), Some('7'), Some('F'), Some('#'), Some('Q'), Some('B'), Some('C'), Some('X')];
        assert_eq!(claimed_shortcuts(keys), vec!['Q', '7', 'X']);
    }

    #[test]
    fn profile_shortcut_is_claimed() {
        let e = toml::Table::new();
        let mut snap = Snapshot::new(&ctx(&e, &e));
        assert!(!win_chord_bound(&snap, &ctrl_win('q')));
        snap.profile_shortcuts = vec!['Q', 'X', 'B'];
        assert!(win_chord_bound(&snap, &ctrl_win('q')));
        assert!(win_chord_bound(&snap, &ctrl_win('x')));
        assert!(win_chord_bound(&snap, &Chord { alt: true, ..ctrl_win('q') }), "Alt opens a window");
        assert!(!win_chord_bound(&snap, &ctrl_win('w')), "an unbound key stays with Windows");
        assert!(!win_chord_bound(&snap, &win(ChordKey::Char('b'))), "Win+B alone is not a profile shortcut");
    }

    /// The Ctrl+Win+key opens the profile in a tab, Alt in a window; a menu chord wins over it,
    /// and the chord never takes the typed-bytes path (so it cannot clear a selection, S10).
    #[test]
    fn profile_shortcut_follows_iterm2() {
        let shortcuts = ['Q', 'T'];
        assert!(matches!(terminal_chord(&shortcuts, &ctrl_win('q')), ChordAction::Profile { key: 'Q', window: false }));
        assert!(matches!(terminal_chord(&shortcuts, &Chord { alt: true, ..ctrl_win('q') }), ChordAction::Profile { key: 'Q', window: true }));
        assert!(matches!(terminal_chord(&shortcuts, &ctrl_win('w')), ChordAction::Nothing));
        assert!(matches!(terminal_chord(&[], &ctrl_win('q')), ChordAction::Nothing));
        // Win+T is a menu command: a menu chord comes before a profile shortcut.
        assert!(matches!(terminal_chord(&shortcuts, &win(ChordKey::Char('t'))), ChordAction::Menu(_)));
    }

    #[test]
    fn profile_shortcut_does_not_clear_selection() {
        let src = include_str!("app/mod.rs");
        let arm = src.find("terminal_chord(").expect("the chord arm asks keyroute::terminal_chord");
        let typed = src[arm..].find("self.to_terminal(").expect("the typed arm follows");
        let between = &src[arm..arm + typed];
        assert!(between.contains("} else if let Some((key, base)) = typed"), "a chord and typed bytes are separate arms");
    }
}
