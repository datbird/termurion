//! iTerm2's paste history (PasteboardHistory): copies and pastes, trimmed, deduplicated,
//! oldest first, at most `PasteHistoryMaxOptions` entries.

/// One remembered copy or paste; `time` is seconds since 1970.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub value: String,
    pub time: f64,
}

#[derive(Clone, Debug)]
pub struct History {
    pub entries: Vec<Entry>,
    pub max: usize,
}

impl Default for History {
    fn default() -> History {
        History { entries: Vec::new(), max: 20 }
    }
}

impl History {
    /// iTerm2's save rules. True when the history changed.
    pub fn save(&mut self, text: &str, now: f64) -> bool {
        let value = text.trim();
        if value.is_empty() {
            return false;
        }
        if let Some(i) = self.entries.iter().position(|e| e.value == value) {
            self.entries.remove(i);
        }
        if self.entries.last().is_some_and(|e| value.starts_with(&e.value)) {
            self.entries.pop();
        }
        self.entries.push(Entry { value: value.to_string(), time: now });
        if self.entries.len() > self.max {
            self.entries.remove(0);
        }
        true
    }

    pub fn newest_first(&self) -> Vec<&Entry> {
        self.entries.iter().rev().collect()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// `{"Entries": [{"Value": ..., "Timestamp": ...}]}` (iTerm2's plist keys).
    pub fn to_json(&self) -> String {
        let entries: Vec<serde_json::Value> = self.entries.iter().map(|e| serde_json::json!({ "Value": e.value, "Timestamp": e.time })).collect();
        serde_json::json!({ "Entries": entries }).to_string()
    }

    pub fn from_json(text: &str, max: usize) -> History {
        let v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
        let mut entries: Vec<Entry> = v
            .get("Entries")
            .and_then(|e| e.as_array())
            .map(|a| a.iter().filter_map(|e| Some(Entry { value: e.get("Value")?.as_str()?.to_string(), time: e.get("Timestamp")?.as_f64()? })).collect())
            .unwrap_or_default();
        let extra = entries.len().saturating_sub(max);
        entries.drain(..extra);
        History { entries, max }
    }
}

/// The popup's second column: "3 lines, 12 bytes, 5 minutes ago" (lines counted in the
/// first 50 KiB, with "+" when there is more).
pub fn time_text(value: &str, time: f64, now: f64) -> String {
    const LIMIT: usize = 50 * 1024;
    let mut cut = value.len().min(LIMIT);
    while !value.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = &value[..cut];
    let lines = head.matches('\n').count() + usize::from(!head.is_empty() && !head.ends_with('\n'));
    let plus = if cut < value.len() { "+" } else { "" };
    let noun = if lines == 1 && plus.is_empty() { "line" } else { "lines" };
    let age = (now - time).max(0.0) as u64;
    let ago = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    let when = match age {
        0..60 => "just now".to_string(),
        60..3600 => ago(age / 60, "minute"),
        3600..86400 => ago(age / 3600, "hour"),
        _ => ago(age / 86400, "day"),
    };
    format!("{lines}{plus} {noun}, {}, {when}", crate::paste::format_bytes(value.len() as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_saves_like_iterm2() {
        let mut h = History::default();
        assert!(h.save("  abc\n", 1.0));
        assert!(h.save("abc", 2.0));
        assert_eq!(h.entries, vec![Entry { value: "abc".into(), time: 2.0 }], "trimmed; an equal entry moves to newest");
        h.save("abcd", 3.0);
        assert_eq!(h.entries.len(), 1, "a last entry that is a prefix goes (find-bar completions)");
        assert!(!h.save(" \n ", 4.0), "empty after trimming");
        for i in 0..25 {
            h.save(&format!("x{i}y"), 10.0 + i as f64);
        }
        assert_eq!(h.entries.len(), 20);
        assert_eq!(h.entries[0].value, "x5y", "the oldest go");
        assert_eq!(h.newest_first()[0].value, "x24y");
    }

    #[test]
    fn history_files_round_trip() {
        let mut h = History::default();
        h.save("one", 100.5);
        h.save("two\nlines", 200.0);
        let back = History::from_json(&h.to_json(), 20);
        assert_eq!(back.entries, h.entries);
        assert!(History::from_json("junk", 20).entries.is_empty());
        assert_eq!(History::from_json(&h.to_json(), 1).entries.len(), 1, "a smaller maximum keeps the newest");
        assert_eq!(time_text("a\nb\nc", 0.0, 30.0), "3 lines, 5 bytes, just now");
        assert_eq!(time_text("a", 0.0, 7200.0), "1 line, 1 byte, 2 hours ago");
        let long = "x\n".repeat(30_000);
        assert!(time_text(&long, 0.0, 0.0).starts_with("25600+ lines"), "{}", time_text(&long, 0.0, 0.0));
    }
}
