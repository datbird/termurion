//! OSC 21337, iTerm2's tab status: `key=value;key=value` with `\;` and `\\` escapes. The
//! keys are `indicator` (a colored dot), `status` (text), `status-color`, and `detail`. A
//! tab shows the focused pane's status, or else the one that matches the highest priority.

use alacritty_terminal::vte::ansi::Rgb;

/// The fields of a payload, unescaped. A field without `=` has an empty value.
pub fn parse(payload: &str) -> Vec<(String, String)> {
    let mut fields = vec![String::new()];
    let mut chars = payload.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(';') => fields.last_mut().unwrap().push(';'),
                Some('\\') => fields.last_mut().unwrap().push('\\'),
                Some(other) => {
                    let f = fields.last_mut().unwrap();
                    f.push('\\');
                    f.push(other);
                }
                None => fields.last_mut().unwrap().push('\\'),
            },
            ';' => fields.push(String::new()),
            c => fields.last_mut().unwrap().push(c),
        }
    }
    fields
        .into_iter()
        .filter(|f| !f.is_empty())
        .map(|f| match f.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (f, String::new()),
        })
        .collect()
}

/// An xterm color: `rgb:r/g/b` (1 to 4 hex digits each) or `#rgb`, `#rrggbb`, `#rrrgggbbb`,
/// `#rrrrggggbbbb`.
pub fn xterm_color(s: &str) -> Option<Rgb> {
    // One component of 1 to 4 hex digits, scaled to 8 bits.
    let part = |h: &str| -> Option<u8> {
        if h.is_empty() || h.len() > 4 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        let max = (1u32 << (4 * h.len())) - 1;
        Some(((v * 255 + max / 2) / max) as u8)
    };
    if let Some(rest) = s.strip_prefix("rgb:") {
        let p: Vec<&str> = rest.split('/').collect();
        return match p[..] {
            [r, g, b] => Some(Rgb { r: part(r)?, g: part(g)?, b: part(b)? }),
            _ => None,
        };
    }
    let hex = s.strip_prefix('#')?;
    if hex.is_empty() || hex.len() % 3 != 0 || hex.len() > 12 {
        return None;
    }
    let n = hex.len() / 3;
    // `#` forms take the leading digits of each component.
    let lead = |i: usize| -> Option<u8> {
        let t = &hex[i * n..(i + 1) * n];
        if n == 1 { part(t) } else { u8::from_str_radix(&t[..2], 16).ok() }
    };
    Some(Rgb { r: lead(0)?, g: lead(1)?, b: lead(2)? })
}

/// A pane's status. Each field is absent until set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub indicator: Option<String>,
    pub status: Option<String>,
    pub status_color: Option<String>,
    pub detail: Option<String>,
}

impl Status {
    /// Applies fields: a missing key leaves its field, an empty value clears it, a value sets
    /// it; unknown keys are ignored. True when something changed.
    pub fn apply(&mut self, pairs: &[(String, String)]) -> bool {
        let before = self.clone();
        for (k, v) in pairs {
            let field = match k.as_str() {
                "indicator" => &mut self.indicator,
                "status" => &mut self.status,
                "status-color" => &mut self.status_color,
                "detail" => &mut self.detail,
                _ => continue,
            };
            *field = (!v.is_empty()).then(|| v.clone());
        }
        *self != before
    }

    pub fn is_empty(&self) -> bool {
        *self == Status::default()
    }
}

/// iTerm2's default priority patterns, highest first.
pub const PRIORITIES: [&str; 3] = ["wait", "work", "idle"];

/// The status a tab shows: the focused pane's if it has one, else the other panes' status
/// whose text matches the earliest priority pattern (case-insensitive substring).
pub fn aggregate<'a>(focused: Option<&'a Status>, others: &[&'a Status], priorities: &[&str]) -> Option<&'a Status> {
    if let Some(f) = focused.filter(|f| !f.is_empty()) {
        return Some(f);
    }
    let rank = |s: &Status| {
        let text = s.status.as_deref().unwrap_or("").to_lowercase();
        priorities.iter().position(|p| text.contains(&p.to_lowercase())).unwrap_or(priorities.len())
    };
    others.iter().copied().filter(|s| !s.is_empty()).enumerate().min_by_key(|(i, s)| (rank(s), *i)).map(|(_, s)| s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::Screen;

    fn kv(k: &str, v: &str) -> (String, String) {
        (k.to_string(), v.to_string())
    }

    #[test]
    fn tab_status_parses_escapes() {
        assert_eq!(parse(r"status=a\;b;detail=c\\d"), vec![kv("status", "a;b"), kv("detail", r"c\d")]);
        assert_eq!(parse(r"status=x\ny;indicator"), vec![kv("status", r"x\ny"), kv("indicator", "")]);
        assert_eq!(parse("status=a=b"), vec![kv("status", "a=b")], "splits at the first =");
        assert_eq!(xterm_color("rgb:ff/80/00"), Some(Rgb { r: 255, g: 128, b: 0 }));
        assert_eq!(xterm_color("rgb:f/8/0"), Some(Rgb { r: 255, g: 136, b: 0 }));
        assert_eq!(xterm_color("rgb:ffff/0000/8080"), Some(Rgb { r: 255, g: 0, b: 128 }));
        assert_eq!(xterm_color("#0f0"), Some(Rgb { r: 0, g: 255, b: 0 }));
        assert_eq!(xterm_color("#102030"), Some(Rgb { r: 0x10, g: 0x20, b: 0x30 }));
        assert_eq!(xterm_color("red"), None);
        let mut st = Status::default();
        assert!(!st.apply(&[kv("bogus", "1")]), "unknown keys change nothing");
    }

    #[test]
    fn fields_are_tri_state() {
        let mut st = Status::default();
        assert!(st.apply(&[kv("status", "working"), kv("indicator", "#00ff00")]));
        assert!(st.apply(&[kv("detail", "x")]));
        assert_eq!(st.status.as_deref(), Some("working"), "absent keys stay");
        assert!(st.apply(&[kv("status", "")]));
        assert_eq!(st.status, None, "empty clears");
        assert!(!st.apply(&[kv("detail", "x")]), "same value: no change");
    }

    #[test]
    fn focused_pane_wins_then_priority() {
        let s = |t: &str| Status { status: Some(t.to_string()), ..Status::default() };
        let (idle, waiting, working) = (s("Idle"), s("Waiting for input"), s("working hard"));
        let empty = Status::default();
        assert_eq!(aggregate(Some(&idle), &[&waiting], &PRIORITIES), Some(&idle));
        assert_eq!(aggregate(Some(&empty), &[&idle, &working, &waiting], &PRIORITIES), Some(&waiting));
        assert_eq!(aggregate(None, &[&idle, &working], &PRIORITIES), Some(&working));
        let other = s("compiling");
        assert_eq!(aggregate(None, &[&other, &idle], &PRIORITIES), Some(&idle), "a match beats no match");
        assert_eq!(aggregate(None, &[&other], &PRIORITIES), Some(&other));
        assert_eq!(aggregate(None, &[&empty], &PRIORITIES), None);
    }

    #[test]
    fn prompt_start_clears_status() {
        let mut sc = Screen::new(20, 4);
        sc.feed(b"\x1b]21337;status=busy;indicator=#ff0000\x07");
        assert_eq!(sc.tab_status.status.as_deref(), Some("busy"));
        sc.feed(b"\x1b]133;A\x07");
        assert!(sc.tab_status.is_empty());
    }
}
