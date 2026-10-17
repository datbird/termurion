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

/// What the keyboard hook needs to decide, on its own thread, whether Termurion takes a
/// Win chord.
#[derive(Clone, Default)]
pub struct Snapshot {
    /// Legacy and typed-character forms of every mapping with Win in it.
    keys: HashSet<String>,
    leader: Option<Keystroke>,
    leader_on: bool,
    switch: [i64; 3],
    /// A composer has the keyboard: its own Cmd chords (`composer::chord`) are Termurion's even
    /// when nothing else binds them (Cmd+Y).
    pub composer: bool,
    /// The keys (A-Z, 0-9) some saved profile's Shortcut claims for Cmd+Ctrl+<key>
    /// (`claimed_shortcuts`), rebuilt with the Profiles menu.
    pub profile_shortcuts: Vec<char>,
    /// A workgroup has a peer group: Cmd+Option+Shift+1..9 are Termurion's (13y1 Task 4).
    pub peer_digits: bool,
    /// The forms of the peers' custom shortcuts that have Cmd in them.
    pub peer_keys: HashSet<String>,
    /// The forms of the person's own mappings with no Cmd in them (a literal Ctrl+Shift+X).
    literal: HashSet<String>,
}

impl Snapshot {
    pub fn new(ctx: &Context) -> Snapshot {
        let mut keys = HashSet::new();
        let mut literal = HashSet::new();
        let stock = crate::globals::keymaps::factory_global();
        for (is_global, map) in [(false, ctx.profile_map), (true, ctx.global_map)] {
            for (k, v) in map {
                let (Some(stroke), Some(b)) = (keystroke::parse(k), Binding::from_value(v)) else { continue };
                let is_stock = if is_global { stock.get(k) == Some(v) } else { crate::globals::keymaps::presets().iter().any(|(_, t)| t.get(k) == Some(v)) };
                if stroke.flags & COMMAND == 0 && !is_stock {
                    literal.extend([stroke.legacy(), stroke.modified_key()]);
                }
                if stroke.flags & COMMAND != 0 && !matches!(b.action, 22 | 24) {
                    keys.extend([stroke.legacy(), stroke.modified_key()]);
                }
            }
        }
        Snapshot { keys, leader: ctx.leader.clone(), leader_on: ctx.leader_on, switch: [ctx.switch_window, ctx.switch_pane, ctx.switch_tab], composer: false, profile_shortcuts: Vec::new(), peer_digits: false, peer_keys: HashSet::new(), literal }
    }

    /// Whether the person wrote a mapping for exactly these keys as pressed (Ctrl+Shift+X, no Cmd).
    /// It runs before a menu command, as iTerm2 runs key mappings before the menu (`iTermApplication
    /// sendEvent:`), so the Cmd reading of the same press yields to it.
    pub fn literal_wins(&self, pressed: &Keystroke) -> bool {
        pressed.flags & COMMAND == 0 && (self.literal.contains(&pressed.legacy()) || self.literal.contains(&pressed.modified_key()))
    }
}

/// True when Termurion has something for this Cmd chord (pressed as the shortcut key's own
/// modifiers, or the moved key of `shortcut_key`): the Leader, a composer chord, a number
/// shortcut, a key mapping, or a working menu command.
pub fn chord_bound(s: &Snapshot, chord: &Chord) -> bool {
    if s.leader_on || (s.composer && crate::composer::chord_of(chord).is_some()) {
        return true;
    }
    let p = press_from_chord(chord);
    if s.leader.as_ref().is_some_and(|l| l.same_key(&p.stroke)) || s.switch.iter().any(|t| number(*t, &p).is_some()) {
        return true;
    }
    let mut typed = p.stroke.clone();
    typed.char_is_modified = true;
    let (a, b) = (p.stroke.legacy(), typed.modified_key());
    // A workgroup's peer shortcuts are Termurion's, after a mapping and before a menu command.
    if (s.peer_digits && crate::workgroup_peers::chord_digit(chord).is_some()) || s.peer_keys.contains(&a) || s.peer_keys.contains(&b) {
        return true;
    }
    let menu = crate::commands::command_for_chord(chord).is_some();
    // 13t: a profile shortcut claims Cmd+Ctrl+<key> and counts as a menu command.
    let profile = profile_chord(chord).is_some_and(|(k, _)| s.profile_shortcuts.contains(&k));
    s.keys.contains(&a) || s.keys.contains(&b) || menu || profile
}

/// The key of a profile shortcut chord: Cmd+Ctrl+<A-Z or 0-9>, with Option for the alternate
/// (a new window). Never a bare key, and never with Shift.
pub fn profile_chord(chord: &Chord) -> Option<(char, bool)> {
    match chord.key {
        ChordKey::Char(c) if chord.cmd && chord.ctrl && !chord.shift && c.is_ascii_alphanumeric() => Some((c.to_ascii_uppercase(), chord.alt)),
        _ => None,
    }
}

/// The keys the profiles' shortcuts claim, once each (the first profile wins, `P8`).
pub fn claimed_shortcuts(keys: impl IntoIterator<Item = Option<char>>) -> Vec<char> {
    let mut out: Vec<char> = Vec::new();
    for k in keys.into_iter().flatten() {
        let k = k.to_ascii_uppercase();
        if k.is_ascii_alphanumeric() && !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

/// What a Cmd chord does once no key mapping and no number shortcut took it. One place for the
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

    /// TWEAKS 9 review I4: a literal Ctrl+Shift+X mapping beats the Cmd+X menu command the same
    /// press would be; a mapping with Cmd in it, a stock mapping and other keys do not count.
    #[test]
    fn a_literal_mapping_beats_the_cmd_reading_of_the_same_press() {
        let e = toml::Table::new();
        let pressed = Keystroke::new(0x58, CONTROL | SHIFT, None, 0x58);
        let literal = map("0x58-0x60000", 13);
        assert!(Snapshot::new(&ctx(&e, &literal)).literal_wins(&pressed), "the person's own Ctrl+Shift+X");
        let other = map("0x59-0x60000", 13);
        assert!(!Snapshot::new(&ctx(&e, &other)).literal_wins(&pressed), "another key");
        let cmd = map("0x58-0x100000", 13);
        assert!(!Snapshot::new(&ctx(&e, &cmd)).literal_wins(&pressed), "a Cmd mapping is the chord's own");
        assert!(!Snapshot::new(&ctx(&e, &e)).literal_wins(&pressed), "nothing mapped");
    }

    /// TWEAKS 9: a Cmd chord pressed with the shortcut key's own keys is the same stroke a saved
    /// binding, an import or an export names with iTerm2's Cmd.
    #[test]
    fn a_shortcut_key_chord_is_bound_like_its_cmd_stroke() {
        let e = toml::Table::new();
        let g = map("0x62-0x100000", 13); // Cmd+B
        let snap = Snapshot::new(&ctx(&e, &g));
        let b = win(ChordKey::Char('b'));
        assert!(chord_bound(&snap, &b), "Ctrl+Shift+B runs the binding");
        assert_eq!(press_from_chord(&b).stroke.flags & COMMAND, COMMAND, "the stroke keeps iTerm2's Cmd");
        assert!(chord_bound(&Snapshot::new(&ctx(&e, &e)), &win(ChordKey::Char('t'))), "a menu command");
        assert!(!chord_bound(&Snapshot::new(&ctx(&e, &e)), &win(ChordKey::F(13))), "nothing owns Cmd+F13: it goes to the terminal");
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
        assert!(chord_bound(&Snapshot::new(&ctx(&j, &e)), &chord));
        assert!(!chord_bound(&Snapshot::new(&ctx(&e, &e)), &chord));
        // Win+T has a menu command; Win+3 is a tab shortcut.
        assert!(chord_bound(&Snapshot::new(&ctx(&e, &e)), &win(ChordKey::Char('t'))));
        assert!(chord_bound(&Snapshot::new(&ctx(&e, &e)), &win(ChordKey::Char('3'))));
    }

    #[test]
    fn shifted_win_chords_match_their_stored_character() {
        let e = toml::Table::new();
        let a = map("0x41-0x120000", 13); // Win+Shift+A, stored with the capital
        let bang = map("0x21-0x120000", 13); // Win+Shift+1, stored as "!"
        assert!(chord_bound(&Snapshot::new(&ctx(&a, &e)), &Chord { shift: true, ..win(ChordKey::Char('a')) }));
        assert!(chord_bound(&Snapshot::new(&ctx(&bang, &e)), &Chord { shift: true, ..win(ChordKey::Char('1')) }));
        assert!(matches!(route(&ctx(&bang, &e), &press_from_chord(&Chord { shift: true, ..win(ChordKey::Char('1')) })), Route::Binding(_)));
    }

    #[test]
    fn stock_global_win_arrows_stay_termurions() {
        let e = toml::Table::new();
        let stock = crate::globals::keymaps::factory_global().clone();
        let s = Snapshot::new(&ctx(&e, &stock));
        for c in [win(ChordKey::Left), win(ChordKey::Right), win(ChordKey::Up), win(ChordKey::Down), win(ChordKey::Home), Chord { shift: true, ..win(ChordKey::Left) }, Chord { shift: true, ..win(ChordKey::Right) }] {
            assert!(chord_bound(&s, &c), "{c:?}");
        }
    }

    #[test]
    fn natural_text_editing_win_arrows_stay_termurions() {
        let e = toml::Table::new();
        // The shipped preset, picked in a profile's Keys, is entries verbatim: Termurion's own.
        let natural = &crate::globals::keymaps::presets().iter().find(|(n, _)| n == "Natural Text Editing").expect("the preset").1;
        let s = Snapshot::new(&ctx(natural, &e));
        assert!(chord_bound(&s, &win(ChordKey::Left)));
        assert!(chord_bound(&s, &win(ChordKey::Right)));
    }

    #[test]
    fn with_the_leader_on_every_win_chord_is_bound() {
        let e = toml::Table::new();
        let mut c = ctx(&e, &e);
        c.leader_on = true;
        assert!(chord_bound(&Snapshot::new(&c), &win(ChordKey::Char('j'))));
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
    fn claimed_shortcuts_skip_repeats_and_other_keys() {
        // No key is Windows': a profile's Cmd+Ctrl+key is Ctrl+Alt+key or Alt+Shift+key.
        let keys = [Some('K'), None, Some('k'), Some('D'), Some('0'), Some('F'), Some('#'), Some('K'), Some('B'), Some('C'), Some('X')];
        assert_eq!(claimed_shortcuts(keys), vec!['K', 'D', '0', 'F', 'B', 'C', 'X']);
    }

    #[test]
    fn profile_shortcut_is_claimed() {
        let e = toml::Table::new();
        let mut snap = Snapshot::new(&ctx(&e, &e));
        assert!(!chord_bound(&snap, &ctrl_win('q')));
        snap.profile_shortcuts = vec!['Q', 'X', 'B'];
        assert!(chord_bound(&snap, &ctrl_win('q')));
        assert!(chord_bound(&snap, &ctrl_win('x')));
        assert!(chord_bound(&snap, &Chord { alt: true, ..ctrl_win('q') }), "Alt opens a window");
        assert!(!chord_bound(&snap, &ctrl_win('w')), "an unbound key stays with Windows");
        assert!(!chord_bound(&snap, &win(ChordKey::Char('b'))), "Win+B alone is not a profile shortcut");
    }

    /// S2: the whole chain for a Cmd+Ctrl chord, in the order Termurion asks. The hook: a registered
    /// global hotkey (hotkey windows, the app hotkey, session hotkeys) first, then a modifier
    /// remap; it takes no chord for Termurion. In the app (`chord_bound`): a key mapping, then a
    /// menu command, then a profile shortcut; anything else goes to the terminal. In `route`, then
    /// `dispatch`: the mapping runs first, then `terminal_chord` (menu, then profile).
    #[test]
    fn ctrl_win_chord_precedence_chain() {
        let hook = include_str!("winkeys.rs");
        let body = hook.split("unsafe fn hook_body(").nth(1).expect("hook_body");
        let at = |needle: &str| body.find(needle).unwrap_or_else(|| panic!("hook_body calls {needle}"));
        assert!(at("global_hotkeys(") < at("remap(h, kb, down)"), "a registered hotkey before a remap");
        assert!(!body.contains("(h.bound)"), "the hook takes no chord for Termurion");
        let gate = include_str!("keyroute.rs").split("pub fn chord_bound(").nth(1).and_then(|r| r.split("\n}").next()).expect("chord_bound");
        assert!(gate.find("s.keys.contains").unwrap() < gate.find("menu || profile").unwrap(), "a key mapping before a menu command or profile shortcut");
        let e = toml::Table::new();
        let x = Chord { ctrl: true, ..win(ChordKey::Char('x')) };
        // A mapping and a profile shortcut on the same Cmd+Ctrl chord: Termurion takes it, and `route` runs
        // the mapping (13t P8: a mapping beats a profile shortcut).
        let mapped = map("0x78-0x140000", 13);
        let mut s = Snapshot::new(&ctx(&e, &mapped));
        s.profile_shortcuts = vec!['X'];
        assert!(chord_bound(&s, &x));
        let terminal = |c: &Chord| crate::keyroute::route(&ctx(&e, &mapped), &press_from_chord(c));
        assert!(matches!(terminal(&x), Route::Binding(_)), "the mapping runs, not the profile");
        // A menu chord beats a profile shortcut in the app.
        assert!(matches!(terminal_chord(&['T'], &win(ChordKey::Char('t'))), ChordAction::Menu(_)));
    }

    /// Cmd+Ctrl+key opens the profile in a tab, Option in a window; a menu chord wins over it,
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
