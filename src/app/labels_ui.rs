//! iTerm2's content shortcuts: Find URLs, Find All Smart Selection Matches, and Pick
//! Result to Open put a label on each result on screen; typing a label opens it, and
//! Alt with the label copies it.

use super::*;
use crate::find::{self, Match};

/// Labels for `n` results: 1 to 9, then A to Z; past 35 results, two letters after the
/// digits, so no label is the start of another.
pub(super) fn labels(n: usize) -> Vec<String> {
    let letters = || ('A'..='Z').map(String::from);
    let rest: Box<dyn Iterator<Item = String>> = if n <= 9 + 26 {
        Box::new(letters())
    } else {
        Box::new(letters().flat_map(move |a| letters().map(move |b| format!("{a}{b}"))))
    };
    (1..=9).map(|d| d.to_string()).chain(rest).take(n).collect()
}

/// The result a typed label picks.
pub(super) fn pick(labels: &[String], typed: &str) -> Option<usize> {
    labels.iter().position(|l| l.eq_ignore_ascii_case(typed))
}

/// Some label starts with what was typed, so another key may finish it.
pub(super) fn partial(labels: &[String], typed: &str) -> bool {
    labels.iter().any(|l| l.len() > typed.len() && l.to_ascii_uppercase().starts_with(&typed.to_ascii_uppercase()))
}

/// A modifier key on its own, which must not put the labels away (Alt picks Copy).
pub(super) fn is_modifier_key(key: &winit::keyboard::Key) -> bool {
    use winit::keyboard::{Key, NamedKey};
    matches!(
        key,
        Key::Named(NamedKey::Alt | NamedKey::AltGraph | NamedKey::Shift | NamedKey::Control | NamedKey::Super | NamedKey::Meta | NamedKey::Hyper | NamedKey::CapsLock)
    )
}

/// The labeled results on screen in one pane.
pub(super) struct Shortcuts {
    pub pane: PaneId,
    pub items: Vec<(String, Match, String)>,
    /// Letters typed so far, for two-letter labels.
    pub typed: String,
}

/// iTerm2's URL pattern for Find URLs, less its trailing punctuation.
const URL_PATTERN: &str = r#"(?i)\b(?:(?:https?|ftp|file|ssh|telnet)://|mailto:|www\.)[^\s<>"'`]+"#;

/// Matches in `lines` that start on screen, each with its text.
fn on_screen(lines: &[find::Logical], hits: impl Iterator<Item = (usize, std::ops::Range<usize>)>, rows: &std::ops::RangeInclusive<usize>) -> Vec<(Match, String)> {
    hits.filter(|(_, r)| !r.is_empty())
        .map(|(i, r)| {
            let l = &lines[i];
            (Match { start: l.pos_before(r.start), end: l.pos_before(r.end - 1) }, l.text[r].to_string())
        })
        .filter(|(m, _)| rows.contains(&m.start.row))
        .collect()
}

impl App {
    /// Find URLs (`urls`), Find All Smart Selection Matches, or Pick Result to Open: labels
    /// on the focused pane's results on screen.
    pub(super) fn reveal_shortcuts(&mut self, kind: &str) {
        let Some(id) = self.focused_id() else { return };
        let Some(p) = self.panes.get(&id) else { return };
        let lines = find::visible_lines(&p.screen);
        let rows = find::visible_rows(&p.screen);
        let find_hits: Vec<(Match, String)> = if kind == "pick" {
            let matches = self.finds.get(&id).map(|f| f.results.matches.clone()).unwrap_or_default();
            matches
                .into_iter()
                .filter(|m| rows.contains(&m.start.row))
                .map(|m| {
                    let (a, b) = (super::find_ui::to_point(&p.screen, m.start), super::find_ui::to_point(&p.screen, m.end));
                    (m, p.screen.term().bounds_to_string(a, b))
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut items = if kind == "urls" {
            let re = regex::Regex::new(URL_PATTERN).expect("URL pattern");
            let hits = lines.iter().enumerate().flat_map(|(i, l)| {
                re.find_iter(&l.text)
                    .map(|m| {
                        let t = m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']']);
                        (i, m.start()..m.start() + t.len())
                    })
                    .collect::<Vec<_>>()
            });
            on_screen(&lines, hits, &rows)
        } else if !find_hits.is_empty() {
            find_hits
        } else {
            let rules = self.store.get(&p.profile).map(crate::smartsel::rules).unwrap_or_default();
            // Every rule of Normal precision or more, the longest match first where they overlap.
            let mut hits: Vec<(usize, std::ops::Range<usize>)> = Vec::new();
            for rule in rules.iter().filter(|r| r.precision >= 1.0) {
                let Ok(re) = fancy_regex::Regex::new(&crate::smartsel::to_rust_regex(&rule.regex)) else { continue };
                for (i, l) in lines.iter().enumerate() {
                    hits.extend(re.find_iter(&l.text).flatten().map(|m| (i, m.range())));
                }
            }
            hits.sort_by_key(|(i, r)| (*i, r.start, std::cmp::Reverse(r.end)));
            let mut kept: Vec<(usize, std::ops::Range<usize>)> = Vec::new();
            for (i, r) in hits {
                if kept.last().is_none_or(|(li, lr)| *li != i || r.start >= lr.end) {
                    kept.push((i, r));
                }
            }
            on_screen(&lines, kept.into_iter(), &rows)
        };
        items.sort_by_key(|(m, _)| m.start);
        if items.is_empty() {
            self.banners.set("action", "Nothing to pick on screen.");
            return self.redraw();
        }
        let names = labels(items.len());
        let items = names.into_iter().zip(items).map(|(l, (m, t))| (l, m, t)).collect();
        self.shortcuts = Some(Shortcuts { pane: id, items, typed: String::new() });
        self.belt_yield_keys();
        self.redraw();
    }

    /// A key while labels show. Returns true when the labels took it.
    pub(super) fn shortcut_key(&mut self, ev: &winit::event::KeyEvent) -> bool {
        let Some(sc) = self.shortcuts.as_mut() else { return false };
        if is_modifier_key(&ev.logical_key) {
            return true;
        }
        let text = ev.text.as_deref().unwrap_or_default();
        let is_label_key = text.len() == 1 && text.chars().all(|c| c.is_ascii_alphanumeric());
        if !is_label_key {
            // Esc, or any other key, puts the labels away.
            self.shortcuts = None;
            self.redraw();
            return true;
        }
        sc.typed.push_str(text);
        let names: Vec<String> = sc.items.iter().map(|(l, _, _)| l.clone()).collect();
        if let Some(i) = pick(&names, &sc.typed) {
            let (pane, text) = (sc.pane, sc.items[i].2.clone());
            self.shortcuts = None;
            if self.mods.alt {
                if let Some(cb) = &mut self.clipboard {
                    if let Err(e) = cb.set_text(text) {
                        self.banners.set("clipboard", format!("Copy failed: {e}"));
                    }
                }
                self.banners.set("action", "Copied.");
            } else {
                self.open_text(pane, &text);
            }
        } else if !partial(&names, &sc.typed) {
            self.shortcuts = None;
        }
        self.redraw();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_run_digits_then_letters() {
        let l = labels(35);
        assert_eq!(&l[..3], ["1", "2", "3"]);
        assert_eq!(l[8], "9");
        assert_eq!(l[9], "A");
        assert_eq!(l[34], "Z");
        let l = labels(40);
        assert_eq!(l[9], "AA");
        assert_eq!(l[10], "AB");
        assert_eq!(labels(0).len(), 0);
        assert_eq!(labels(9 + 26 * 26).last().map(String::as_str), Some("ZZ"));
    }

    #[test]
    fn a_typed_label_picks_its_result() {
        let l = labels(20);
        assert_eq!(pick(&l, "3"), Some(2));
        assert_eq!(pick(&l, "a"), Some(9));
        assert_eq!(pick(&l, "?"), None);
        let l = labels(40);
        assert_eq!(pick(&l, "A"), None);
        assert!(partial(&l, "a"));
        assert_eq!(pick(&l, "ab"), Some(10));
        assert!(!partial(&l, "Q"));
    }

    #[test]
    fn modifier_keys_keep_the_labels() {
        use winit::keyboard::{Key, NamedKey};
        assert!(is_modifier_key(&Key::Named(NamedKey::Alt)));
        assert!(is_modifier_key(&Key::Named(NamedKey::Shift)));
        assert!(is_modifier_key(&Key::Named(NamedKey::Control)));
        assert!(!is_modifier_key(&Key::Named(NamedKey::Escape)));
        assert!(!is_modifier_key(&Key::Character("a".into())));
    }
}
