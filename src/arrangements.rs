//! Saved arrangements by name, in iTerm2's two settings: `Window Arrangements` (a name to
//! its windows) and `Default Arrangement Name`.


use std::collections::BTreeMap;

pub use toml::{Table, Value};

pub const ARRANGEMENTS_KEY: &str = "Window Arrangements";
pub const DEFAULT_KEY: &str = "Default Arrangement Name";

/// Named arrangements and the default one.
#[derive(Clone, Debug, Default)]
pub struct Store {
    all: BTreeMap<String, Vec<Value>>,
    default: Option<String>,
}

impl Store {
    pub fn from_values(arrangements: Option<&Value>, default: Option<&Value>) -> Store {
        let all = arrangements
            .and_then(|v| v.as_table())
            .map(|t| t.iter().filter_map(|(k, v)| v.as_array().map(|a| (k.clone(), a.clone()))).collect())
            .unwrap_or_default();
        let default = default.and_then(|v| v.as_str()).map(str::to_string);
        Store { all, default }
    }

    /// The two settings' values (None to remove a setting).
    pub fn to_values(&self) -> (Option<Value>, Option<Value>) {
        let t: Table = self.all.iter().map(|(k, v)| (k.clone(), Value::Array(v.clone()))).collect();
        ((!t.is_empty()).then_some(Value::Table(t)), self.default.clone().map(Value::String))
    }

    /// Names, sorted.
    pub fn names(&self) -> Vec<&str> {
        self.all.keys().map(String::as_str).collect()
    }

    pub fn get(&self, name: &str) -> Option<&Vec<Value>> {
        self.all.get(name)
    }

    pub fn default_name(&self) -> Option<&str> {
        self.default.as_deref().filter(|d| self.all.contains_key(*d))
    }

    /// Saves (or replaces) an arrangement; the first one saved becomes the default.
    pub fn save(&mut self, name: &str, windows: Vec<Value>) {
        self.all.insert(name.to_string(), windows);
        if self.default_name().is_none() {
            self.default = Some(name.to_string());
        }
    }

    /// Deletes one; deleting the default makes a neighbor the default.
    pub fn delete(&mut self, name: &str) {
        let names: Vec<String> = self.all.keys().cloned().collect();
        let Some(i) = names.iter().position(|n| n == name) else { return };
        self.all.remove(name);
        if self.default.as_deref() == Some(name) {
            let rest: Vec<&String> = self.all.keys().collect();
            self.default = rest.get(i).or_else(|| rest.get(i.saturating_sub(1))).map(|s| s.to_string());
        }
    }

    /// False when `to` is taken or `from` is missing.
    pub fn rename(&mut self, from: &str, to: &str) -> bool {
        let to = clean_name(to);
        if to.is_empty() || self.all.contains_key(&to) {
            return false;
        }
        let Some(v) = self.all.remove(from) else { return false };
        self.all.insert(to.clone(), v);
        if self.default.as_deref() == Some(from) {
            self.default = Some(to);
        }
        true
    }

    pub fn set_default(&mut self, name: &str) {
        if self.all.contains_key(name) {
            self.default = Some(name.to_string());
        }
    }

    pub fn replace(&mut self, name: &str, windows: Vec<Value>) {
        if let Some(v) = self.all.get_mut(name) {
            *v = windows;
        }
    }
}

/// Newlines become spaces; ends are trimmed.
pub fn clean_name(s: &str) -> String {
    s.replace("\r\n", " ").replace(['\n', '\r'], " ").trim().to_string()
}

/// The save dialog's name: "Arrangement (<date>)", with the time, then the seconds, then a
/// number added until it is free.
pub fn unique_name(date: &str, time: &str, time_seconds: &str, taken: &[String]) -> String {
    let free = |n: &String| !taken.contains(n);
    for c in [format!("Arrangement ({date})"), format!("Arrangement ({date} {time})"), format!("Arrangement ({date} {time_seconds})")] {
        if free(&c) {
            return c;
        }
    }
    (2..).map(|i| format!("Arrangement ({date} {time_seconds}) {i}")).find(free).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w() -> Vec<Value> {
        vec![Value::Table(Table::new())]
    }

    #[test]
    fn default_rules() {
        let mut s = Store::default();
        s.save("b", w());
        assert_eq!(s.default_name(), Some("b"), "the first saved becomes the default");
        s.save("a", w());
        s.save("c", w());
        assert_eq!(s.names(), vec!["a", "b", "c"], "sorted");
        assert_eq!(s.default_name(), Some("b"));
        s.delete("b");
        assert!(matches!(s.default_name(), Some("a") | Some("c")), "a neighbor becomes the default: {:?}", s.default_name());
        s.set_default("c");
        s.delete("a");
        s.delete("c");
        assert_eq!(s.default_name(), None);
        assert!(s.names().is_empty());
    }

    #[test]
    fn unique_names() {
        let taken = vec!["Arrangement (Sep 27, 2026)".to_string()];
        let n = unique_name("Sep 27, 2026", "10:05 AM", "10:05:33 AM", &taken);
        assert_eq!(n, "Arrangement (Sep 27, 2026 10:05 AM)");
        let taken2 = vec![taken[0].clone(), n.clone()];
        assert_eq!(unique_name("Sep 27, 2026", "10:05 AM", "10:05:33 AM", &taken2), "Arrangement (Sep 27, 2026 10:05:33 AM)");
        assert_eq!(unique_name("Sep 27, 2026", "10:05 AM", "10:05:33 AM", &[]), "Arrangement (Sep 27, 2026)");
        assert_eq!(clean_name("two\nlines\r\n"), "two lines");
    }

    #[test]
    fn rename_keeps_the_default() {
        let mut s = Store::default();
        s.save("old", w());
        assert!(s.rename("old", "new"));
        assert_eq!((s.names(), s.default_name()), (vec!["new"], Some("new")));
        s.save("x", w());
        assert!(!s.rename("x", "new"), "a taken name is refused");
        let (t, d) = s.to_values();
        let back = Store::from_values(t.as_ref(), d.as_ref());
        assert_eq!((back.names(), back.default_name()), (vec!["new", "x"], Some("new")));
    }
}
