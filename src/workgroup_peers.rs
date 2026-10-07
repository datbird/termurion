//! Peers (milestone 13y1, Task 4): the pure side of a workgroup's peer group. Who the members are,
//! which one a digit or an offset picks, and what a segment's label says. The app side (swapping the
//! visible member into the pane) is `app/workgroup_app.rs`.
//!
//! Citations: `WPP` `iTermWorkgroupPeerPort.swift`, `WMS` `WorkgroupModeSwitcherItem.swift` under
//! `~/gitrepos/iterm2-ref/sources/Workgroups`.

use crate::keystroke;
use crate::menus::{Chord, ChordKey};
use crate::workgroup::{Kind, Shortcut, Workgroup};

/// A member's label when its display name is empty (`WPP:94`).
pub const EMPTY_LABEL: &str = "Peer";

/// The built-in shortcut's words for position `n` (Windows names for iTerm2's option, shift, command).
const DIGIT_PREFIX: &str = "Alt+Shift+Win+";

/// The peer group of `w`: the root then its peers, each (config id, label, custom shortcut)
/// (`WPP:86-96`: the port's members are the peers' configs, the leader's first).
pub fn members(w: &Workgroup) -> Vec<(String, String, Option<Shortcut>)> {
    let Some(root) = w.root() else { return Vec::new() };
    let label = |s: &crate::workgroup::SessionConfig| if s.display_name.is_empty() { EMPTY_LABEL.to_string() } else { s.display_name.clone() };
    let mut out = vec![(root.unique_identifier.clone(), label(root), root.peer_switch_shortcut.clone())];
    for s in w.children(&root.unique_identifier).into_iter().filter(|s| s.kind == Kind::Peer) {
        out.push((s.unique_identifier.clone(), label(s), s.peer_switch_shortcut.clone()));
    }
    out
}

/// The member index Alt+Shift+Win+`digit` picks among `count`: 1 to 8 directly when in range, 9
/// the last (`WPP:461-485`).
pub fn by_digit(count: usize, digit: u32) -> Option<usize> {
    if count == 0 {
        return None;
    }
    match digit {
        1..=8 if digit as usize <= count => Some(digit as usize - 1),
        9 => Some(count - 1),
        _ => None,
    }
}

/// The member `offset` places from `active`, skipping members that are not `alive`, wrapping at
/// both ends; None with one member or when every other member is dead (`WPP:543-575`). An unknown
/// `active` counts from the first.
pub fn by_offset(ids: &[String], active: &str, offset: i32, alive: &dyn Fn(&str) -> bool) -> Option<String> {
    let count = ids.len() as i64;
    if count < 2 {
        return None;
    }
    let base = ids.iter().position(|i| i == active).unwrap_or(0) as i64;
    for step in 1..count {
        let at = (base + offset as i64 * step).rem_euclid(count) as usize;
        if alive(&ids[at]) {
            return Some(ids[at].clone());
        }
    }
    None
}

/// The built-in shortcut's text for member `position` (1-based) when it is one of the first
/// eight. The ninth-key rule for the last member needs the count (`segment_label`).
pub fn digit_label(position: usize) -> Option<String> {
    (1..=8).contains(&position).then(|| format!("{DIGIT_PREFIX}{position}"))
}

/// A segment's text: the label, an em space, then the custom shortcut or the built-in one
/// (`WMS:34-57`, `:88-110`). The last of nine or more members shows the 9 shortcut.
pub fn segment_label(label: &str, custom: Option<&str>, position: usize, count: usize) -> String {
    let suffix = match custom {
        Some(c) => Some(c.to_string()),
        None => digit_label(position).or_else(|| (position == count && position > 8).then(|| format!("{DIGIT_PREFIX}9"))),
    };
    match suffix.filter(|s| !s.is_empty()) {
        Some(s) => format!("{label}\u{2003}{s}"),
        None => label.to_string(),
    }
}

/// The digit of an Alt+Shift+Win+digit chord (1 to 9); Alt+Win+digit selects a window and takes no part.
pub fn chord_digit(c: &Chord) -> Option<u32> {
    if !(c.alt && c.shift && c.cmd && !c.ctrl) {
        return None;
    }
    match c.key {
        ChordKey::Char(d) => d.to_digit(10).filter(|d| (1..=9).contains(d)),
        _ => None,
    }
}

/// Whether `key` is the stored custom shortcut (iTerm2's keystroke equality: character and flags).
pub fn shortcut_matches(s: &Shortcut, key: &keystroke::Keystroke) -> bool {
    keystroke::parse(&s.serialized).is_some_and(|k| k.same_key(key))
}

/// The text of a custom shortcut for a segment, None when it does not parse.
pub fn custom_label(s: &Shortcut) -> Option<String> {
    keystroke::parse(&s.serialized).map(|k| k.label())
}

/// Previous Peer and Next Peer are enabled with more than one peer (`WPP:486-490`).
pub fn menu_enabled(peer_count: usize) -> bool {
    peer_count > 1
}

/// The pane whose clippings `pane` uses: a member of a peer group shares the shown member's, any other
/// member (a split, a tab) its own.
pub fn clip_home(peer_member: bool, shown: Option<crate::layout::PaneId>, pane: crate::layout::PaneId) -> crate::layout::PaneId {
    if peer_member { shown.unwrap_or(pane) } else { pane }
}

/// Whether a member shows its spinner: its agent reported Working.
pub fn busy(state: crate::session_state::State) -> bool {
    state == crate::session_state::State::Working
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menus::{Chord, ChordKey};
    use crate::workgroup::claude_code::{config, DIFF, MAIN, REVIEW};
    use crate::workgroup::Shortcut;

    /// A member is busy while its agent reports Working (12f's status); the toolbar's segment shows the spinner.
    #[test]
    fn busy_flags_follow_the_reported_status() {
        use crate::session_state::State;
        assert!(busy(State::Working));
        for s in [State::Idle, State::Waiting, State::Unknown] {
            assert!(!busy(s), "{s:?}");
        }
        assert!(include_str!("app/workgroup_toolbar_ui.rs").contains("crate::workgroup_peers::busy(session_state::reported_state("));
    }

    /// Clippings are the peer group's (`PSPP:30-55`): a split or tab member has its own.
    #[test]
    fn clippings_are_shared_within_a_peer_group_only() {
        assert_eq!(clip_home(true, Some(7), 3), 7, "a peer-group member uses the shown member's");
        assert_eq!(clip_home(false, Some(7), 3), 3, "a split or tab member keeps its own");
        assert_eq!(clip_home(true, None, 3), 3);
        let app = include_str!("app/workgroup_app.rs");
        let f = &app[app.find("fn clip_home").unwrap()..];
        assert!(f[..f.find("\n    }\n").unwrap()].contains("config_of(pane).is_some()"), "keyed by the peer group");
    }

    fn ids() -> Vec<String> {
        members(&config()).into_iter().map(|m| m.0).collect()
    }

    #[test]
    fn peer_activation_follows_iterm2() {
        let m = members(&config());
        assert_eq!(m, vec![(MAIN.to_string(), "Chat".to_string(), None), (DIFF.to_string(), "Diff".to_string(), None), (REVIEW.to_string(), "Code Review".to_string(), None)]);
        let ids = ids();
        let all = |_: &str| true;
        assert_eq!(by_offset(&ids, MAIN, 1, &all).as_deref(), Some(DIFF));
        assert_eq!(by_offset(&ids, MAIN, -1, &all).as_deref(), Some(REVIEW), "wraps");
        assert_eq!(by_offset(&ids, REVIEW, 1, &all).as_deref(), Some(MAIN), "wraps forward");
        assert_eq!(by_offset(&ids, MAIN, 1, &|id: &str| id != DIFF).as_deref(), Some(REVIEW), "a dead member is skipped");
        assert_eq!(by_offset(&ids, MAIN, 4, &all).as_deref(), Some(DIFF), "an offset past the end wraps");
        assert_eq!(by_offset(&ids[..1], MAIN, 1, &all), None, "one member");
        assert_eq!(by_offset(&ids, MAIN, 1, &|id: &str| id == MAIN), None, "every other member dead");
        assert_eq!(by_offset(&ids, "gone", 1, &all).as_deref(), Some(DIFF), "an unknown active id counts from the first");
        assert_eq!(by_offset(&[], MAIN, 1, &all), None);
    }

    #[test]
    fn an_empty_label_reads_peer() {
        let mut w = config();
        for s in &mut w.sessions {
            s.display_name.clear();
        }
        assert!(members(&w).iter().all(|m| m.1 == "Peer"));
    }

    #[test]
    fn members_are_the_root_then_its_peers_only() {
        let mut w = config();
        let split = crate::workgroup::presets::coding_agent_plus_diff().sessions[0].clone();
        let mut extra = split;
        extra.unique_identifier = "x".into();
        extra.parent_id = Some(MAIN.into());
        extra.kind = crate::workgroup::Kind::Tab;
        w.sessions.push(extra);
        assert_eq!(members(&w).len(), 3, "a tab is not a peer");
    }

    #[test]
    fn peer_shortcuts_follow_iterm2() {
        assert_eq!(by_digit(3, 2), Some(1));
        assert_eq!(by_digit(3, 4), None);
        assert_eq!(by_digit(3, 9), Some(2));
        assert_eq!(by_digit(3, 0), None);
        assert_eq!(by_digit(0, 9), None);
        assert_eq!(by_digit(12, 8), Some(7));
        assert_eq!(by_digit(12, 9), Some(11));
        assert_eq!(by_digit(8, 8), Some(7));
        assert_eq!(digit_label(1).as_deref(), Some("Alt+Shift+Win+1"));
        assert_eq!(digit_label(8).as_deref(), Some("Alt+Shift+Win+8"));
        assert_eq!(digit_label(9), None, "the 9 rule needs the count");
        assert_eq!(digit_label(0), None);
        assert_eq!(segment_label("Diff", None, 2, 3), "Diff\u{2003}Alt+Shift+Win+2");
        assert_eq!(segment_label("Last", None, 12, 12), "Last\u{2003}Alt+Shift+Win+9", "the last of nine or more");
        assert_eq!(segment_label("Mid", None, 10, 12), "Mid", "no shortcut for it");
        assert_eq!(segment_label("Diff", Some("Ctrl+D"), 2, 3), "Diff\u{2003}Ctrl+D", "a custom shortcut replaces the digit");
        assert_eq!(segment_label("Diff", Some(""), 2, 3), "Diff");
    }

    fn chord(digit: char, alt: bool, shift: bool, cmd: bool, ctrl: bool) -> Chord {
        Chord { key: ChordKey::Char(digit), cmd, alt, shift, ctrl }
    }

    #[test]
    fn the_digit_chord_is_alt_shift_win_only() {
        assert_eq!(chord_digit(&chord('3', true, true, true, false)), Some(3));
        assert_eq!(chord_digit(&chord('9', true, true, true, false)), Some(9));
        assert_eq!(chord_digit(&chord('0', true, true, true, false)), None);
        assert_eq!(chord_digit(&chord('3', true, false, true, false)), None, "Alt+Win+3 is Select Window");
        assert_eq!(chord_digit(&chord('3', true, true, true, true)), None);
        assert_eq!(chord_digit(&chord('a', true, true, true, false)), None);
        assert_eq!(chord_digit(&chord('3', false, true, true, false)), None);
    }

    #[test]
    fn the_digit_chords_collide_with_no_menu_item() {
        for d in '1'..='9' {
            assert!(crate::commands::command_for_chord(&chord(d, true, true, true, false)).is_none(), "{d}");
        }
    }

    #[test]
    fn a_custom_shortcut_matches_its_key() {
        let k = crate::keystroke::Keystroke::new(0x64, crate::keystroke::OPTION | crate::keystroke::CONTROL, None, 0x64);
        let sc = Shortcut { serialized: k.serialized() };
        assert!(shortcut_matches(&sc, &k));
        let other = crate::keystroke::Keystroke::new(0x65, crate::keystroke::OPTION | crate::keystroke::CONTROL, None, 0x65);
        assert!(!shortcut_matches(&sc, &other));
        assert!(!shortcut_matches(&Shortcut { serialized: "garbage".into() }, &k));
        assert_eq!(custom_label(&sc).as_deref(), Some("Ctrl+Alt+D"));
        assert_eq!(custom_label(&Shortcut { serialized: "garbage".into() }), None);
    }

    #[test]
    fn the_menu_needs_more_than_one_peer() {
        assert!(!menu_enabled(0));
        assert!(!menu_enabled(1));
        assert!(menu_enabled(2));
        assert!(menu_enabled(3));
    }
}
