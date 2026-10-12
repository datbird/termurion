//! iTerm2's menus, from schema/menus.toml (generated from MainMenu.xib), and the
//! Windows chord for each item. Cmd is the Win key and Option is Alt (spec 3.4).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::api_proto::pb;

#[derive(Clone, Debug, Deserialize)]
pub struct Item {
    pub path: Vec<String>,
    #[serde(default)]
    pub selector: String,
    /// The XIB identifier, what the Python API's MenuItem request names; empty when the
    /// XIB gives none.
    #[serde(default)]
    pub identifier: String,
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

/// Items iTerm2 removes from the menu (`AD:402-419`): Capture GPU Frame in release builds, and
/// Check Settings Control Truncation unless built for development (Tandem's Settings wrap text,
/// so it is never built, R5). The one rule every menu copy and list of menu actions applies.
pub fn hidden_in_menu(selector: &str, debug_build: bool) -> bool {
    match selector {
        "captureNextMetalFrame:" => !debug_build,
        "debugCheckSettingsControlTruncation:" => true,
        // The browser toolbar's Debug Autofill Fields (`iTermBrowserToolbar.swift:370-378`, `#if DEBUG`).
        "debugAutofillMenuItemSelected:" => !debug_build,
        _ => false,
    }
}

/// The items of this build's menus: `items()` without what `hidden_in_menu` removes. Every menu
/// builder and list of menu actions reads this, never `items()`.
pub fn shown_items() -> impl Iterator<Item = &'static Item> {
    items().iter().filter(|i| !hidden_in_menu(&i.selector, cfg!(debug_assertions)))
}

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

/// A menu title as Tandem shows it (the menu bar, Open Quickly): iTerm2's with "Finder" as
/// "Explorer" (Decision). The schema keeps iTerm2's titles, which menu ids are made of.
pub fn shown_label(label: &str) -> String {
    label.replace("Finder", "Explorer")
}

pub fn by_id(id: &str) -> Option<&'static Item> {
    shown_items().find(|i| !i.separator && i.id() == id)
}

/// The leaf item (not a separator, not a submenu) with this XIB identifier, first in
/// menu order, as iTerm2 searches its main menu (`H:1526-1539`).
pub fn by_identifier(id: &str) -> Option<&'static Item> {
    shown_items().find(|i| !i.separator && !i.submenu && !i.identifier.is_empty() && i.identifier == id)
}

/// iTerm2's MenuItem answer (`H:3524-3547`) for the item found (None when no leaf has the
/// identifier), whether it is enabled and checked now, and whether the request only asks;
/// with whether to run the item's command.
pub fn menu_answer(item: Option<&Item>, enabled: bool, checked: bool, query_only: bool) -> (pb::MenuItemResponse, bool) {
    use pb::menu_item_response::Status;
    let mut r = pb::MenuItemResponse::default();
    if item.is_none() {
        r.status = Some(Status::BadIdentifier as i32);
        return (r, false);
    }
    if !enabled && !query_only {
        r.status = Some(Status::Disabled as i32);
        return (r, false);
    }
    r.checked = Some(checked);
    r.enabled = Some(enabled);
    r.status = Some(Status::Ok as i32);
    (r, !query_only)
}

/// The command a chord runs. When items share a chord, the first in menu order wins,
/// and alternates (macOS Option variants) come after every normal item.
pub fn command_for(chord: &Chord) -> Option<&'static Item> {
    static TABLE: OnceLock<HashMap<Chord, &'static Item>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            let mut t = HashMap::new();
            let commands = shown_items().filter(|i| !i.separator && !i.submenu && !i.selector.is_empty());
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
    let commands = shown_items().filter(|i| !i.separator && !i.submenu && !i.selector.is_empty());
    let (normal, alternates): (Vec<_>, Vec<_>) = commands.partition(|i| !i.alternate);
    normal.into_iter().chain(alternates).filter(|i| i.chord().as_ref() == Some(chord)).collect()
}

/// "Win+Shift+D": how Windows menus show a chord.
/// Windows keeps Win+L (Lock) with any other modifiers, so such a chord never reaches Tandem
/// and the menu does not show it.
pub fn reaches_tandem(c: &Chord) -> bool {
    !(c.cmd && c.key == ChordKey::Char('l'))
}

/// A chord key as Windows menus show it.
pub fn key_label(k: &ChordKey) -> String {
    match k {
        ChordKey::Char(ch) => ch.to_ascii_uppercase().to_string(),
        ChordKey::F(n) => format!("F{n}"),
        ChordKey::PageUp => "PgUp".into(),
        ChordKey::PageDown => "PgDn".into(),
        ChordKey::Delete => "Del".into(),
        ChordKey::Escape => "Esc".into(),
        other => format!("{other:?}"),
    }
}

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
    parts.push(key_label(&c.key));
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The class (Task 3 round 1): ONE filter hides Capture GPU Frame (release) and Check Settings
    /// Control Truncation (always) from every menu copy and every list of menu actions: the window
    /// menu, the tray's Main Menu, the key-binding targets, Open Quickly and the API's lookups.
    /// A list is built from `shown_items()`; `items()` is only searched by selector.
    #[test]
    fn every_menu_list_goes_through_the_one_filter() {
        assert!(hidden_in_menu("captureNextMetalFrame:", false));
        assert!(!hidden_in_menu("captureNextMetalFrame:", true));
        assert!(hidden_in_menu("debugCheckSettingsControlTruncation:", true));
        assert!(shown_items().all(|i| i.selector != "debugCheckSettingsControlTruncation:"));
        assert_eq!(shown_items().any(|i| i.selector == "captureNextMetalFrame:"), cfg!(debug_assertions));
        assert!(by_id(&item("debugCheckSettingsControlTruncation:").id()).is_none(), "the API cannot reach a hidden item by id");
        assert!(by_identifier("Check Settings Control Truncation").is_none());
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let raw = regex::Regex::new(r"menus::items\(\)").unwrap();
        let lookup = regex::Regex::new(r"menus::items\(\)\s*\.iter\(\)\s*\.find\(\|i\| i\.selector").unwrap();
        let (mut dirs, mut builders) = (vec![src], 0);
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("menus.rs") {
                    let t = std::fs::read_to_string(&p).unwrap();
                    let live = t.split("#[cfg(test)]").next().unwrap_or("");
                    let (all, finds) = (raw.find_iter(live).count(), lookup.find_iter(live).count());
                    assert_eq!(all, finds, "{p:?}: a list of menu items must come from menus::shown_items(), not menus::items()");
                    builders += live.matches("shown_items()").count();
                }
            }
        }
        assert!(builders >= 5, "the window menu, the tray menu, the binding targets and Open Quickly: {builders}");
        let own = include_str!("menus.rs");
        let own = own.split("#[cfg(test)]").next().unwrap();
        for f in ["pub fn by_id", "pub fn by_identifier", "pub fn items_for"] {
            let body = &own[own.find(f).unwrap()..];
            assert!(body[..body.find("\n}\n").unwrap()].contains("shown_items()"), "{f}");
        }
    }

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

    #[test]
    fn identifiers_follow_the_xib() {
        let label = |id: &str| by_identifier(id).unwrap_or_else(|| panic!("{id}")).label().to_string();
        assert_eq!(label("New Tab"), "New Tab");
        assert_eq!(label("trmux.Pause Pane"), "Pause Pane");
        assert_eq!(label("Paste Special.Paste Slowly"), "Paste Slowly");
        assert_eq!(label("Find.Find Globally..."), "Find Globally...");
        assert_eq!(label("Open Interactive Window"), "Open Python REPL");
        assert_eq!(label("Script Console"), "Console");
        assert_eq!(label("Run Coprocess\u{2026}"), "Run Coprocess...");
        assert!(by_identifier("Check For Updates\u{2026}").is_some());
        assert!(by_identifier("New Tab ").is_none());
        assert!(by_identifier("").is_none());
        // A submenu's identifier names nothing a MenuItem request can run: the XIB's own
        // submenu identifiers find no item unless a leaf shares the identifier.
        assert!(by_identifier("View").is_none(), "the View menu itself");
        assert!(by_identifier("Edit.Snippets").is_none(), "a submenu inside Edit");
        let submenus: Vec<&Item> = items().iter().filter(|i| i.submenu && !i.identifier.is_empty()).collect();
        assert!(submenus.len() > 5, "the XIB has named submenus");
        for i in submenus {
            let leaf = items().iter().any(|l| !l.submenu && !l.separator && l.identifier == i.identifier);
            assert_eq!(by_identifier(&i.identifier).is_some(), leaf, "{} is a submenu", i.identifier);
        }
        let mut seen = std::collections::HashSet::new();
        for i in items().iter().filter(|i| !i.identifier.is_empty()) {
            assert!(seen.insert(i.identifier.as_str()), "duplicate identifier {}", i.identifier);
        }
    }

    #[test]
    fn menu_answers_follow_iterm2() {
        use pb::menu_item_response::Status;
        let i = by_identifier("New Tab");
        let (r, run) = menu_answer(None, true, false, false);
        assert_eq!((r.status, r.checked, r.enabled, run), (Some(Status::BadIdentifier as i32), None, None, false));
        let (r, run) = menu_answer(i, false, false, false);
        assert_eq!((r.status, r.checked, r.enabled, run), (Some(Status::Disabled as i32), None, None, false));
        let (r, run) = menu_answer(i, false, true, true);
        assert_eq!((r.status, r.checked, r.enabled, run), (Some(Status::Ok as i32), Some(true), Some(false), false));
        let (r, run) = menu_answer(i, true, false, false);
        assert_eq!((r.status, r.checked, r.enabled, run), (Some(Status::Ok as i32), Some(false), Some(true), true));
    }

    #[test]
    fn script_labels_follow_windows() {
        assert_eq!(shown_label("Reveal Scripts in Finder"), "Reveal Scripts in Explorer");
        assert_eq!(shown_label("New Tab"), "New Tab");
    }

    #[test]
    fn win_l_chords_stay_with_windows() {
        let l = Chord { key: ChordKey::Char('l'), cmd: true, alt: false, shift: true, ctrl: false };
        assert!(!reaches_tandem(&l));
        assert!(reaches_tandem(&Chord { key: ChordKey::Char('k'), ..l }));
        assert!(reaches_tandem(&Chord { cmd: false, ..l }));
    }
}
