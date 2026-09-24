//! iTerm2's menus, from schema/menus.toml (generated from MainMenu.xib), and the
//! Windows chord for each item. Cmd is the Win key and Option is Alt (spec 3.4).

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

/// Every command with this chord, in menu order, alternates last.
pub fn items_for(chord: &Chord) -> Vec<&'static Item> {
    let commands = items().iter().filter(|i| !i.separator && !i.submenu && !i.selector.is_empty());
    let (normal, alternates): (Vec<_>, Vec<_>) = commands.partition(|i| !i.alternate);
    normal.into_iter().chain(alternates).filter(|i| i.chord().as_ref() == Some(chord)).collect()
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
        let top: Vec<&str> = items().iter().filter(|i| !i.separator && i.path.len() == 1).map(|i| i.label()).collect();
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
