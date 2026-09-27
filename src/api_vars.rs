//! The Python API's variable store: user variables set on tabs, windows and the app (a
//! session's own live in its shell state, as SetUserVar leaves them), and the last values
//! seen of each scope, whose differences are VARIABLE_CHANGE notifications
//! (`Language/iTermVariables.m`, `H:1866-1877`).

use std::collections::BTreeMap;

use crate::api_proto::pb;

/// iTerm2's variable scopes (`pb::VariableScope`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    Session,
    Tab,
    Window,
    App,
}

impl Scope {
    /// The scope's number in `pb::VariableScope` (1 to 4).
    pub fn proto(self) -> i32 {
        match self {
            Scope::Session => pb::VariableScope::Session as i32,
            Scope::Tab => pb::VariableScope::Tab as i32,
            Scope::Window => pb::VariableScope::Window as i32,
            Scope::App => pb::VariableScope::App as i32,
        }
    }
}

/// A scope's values by name.
pub type Values = BTreeMap<String, serde_json::Value>;

/// A value that changed since the scope was last observed: its JSON, "null" when gone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub scope: Scope,
    pub id: String,
    pub name: String,
    pub json: String,
}

/// Why a variable could not be set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetError {
    /// Only names starting "user." may be set.
    NotUser,
}

/// User variables and the last observed values, by scope and object id (a session's pane
/// id, a tab's id, a window's `pty-` id, "" for the app).
#[derive(Clone, Debug, Default)]
pub struct Store {
    user: BTreeMap<(Scope, String), Values>,
    seen: BTreeMap<(Scope, String), Values>,
}

impl Store {
    /// Sets a user variable (`name` starts "user."); JSON null removes it.
    pub fn set_user(&mut self, scope: Scope, id: &str, name: &str, value: serde_json::Value) -> Result<(), SetError> {
        let Some(n) = name.strip_prefix("user.") else { return Err(SetError::NotUser) };
        let key = (scope, id.to_string());
        if value.is_null() {
            if let Some(m) = self.user.get_mut(&key) {
                m.remove(n);
                if m.is_empty() {
                    self.user.remove(&key);
                }
            }
        } else {
            self.user.entry(key).or_default().insert(n.to_string(), value);
        }
        Ok(())
    }

    /// An object's user variables, by name without the "user." prefix.
    pub fn user(&self, scope: Scope, id: &str) -> Values {
        self.user.get(&(scope, id.to_string())).cloned().unwrap_or_default()
    }

    /// The object's values now against those last observed, sorted by name. The first
    /// observation of an object is the baseline and reports nothing.
    pub fn observe(&mut self, scope: Scope, id: &str, now: Values) -> Vec<Change> {
        let key = (scope, id.to_string());
        let Some(before) = self.seen.insert(key, now.clone()) else { return Vec::new() };
        let mut names: Vec<&String> = before.keys().chain(now.keys()).collect();
        names.sort();
        names.dedup();
        names
            .into_iter()
            .filter(|n| before.get(*n) != now.get(*n))
            .map(|n| Change {
                scope,
                id: id.to_string(),
                name: n.clone(),
                json: now.get(n).map_or_else(|| "null".to_string(), |v| v.to_string()),
            })
            .collect()
    }

    /// Drops an object that closed: its user variables and its last observed values.
    pub fn forget(&mut self, scope: Scope, id: &str) {
        let key = (scope, id.to_string());
        self.user.remove(&key);
        self.seen.remove(&key);
    }
}

/// Names whose values are numbers in iTerm2's scopes.
pub const NUMERIC: &[&str] = &["columns", "rows", "pid", "effective_root_pid", "jobPid", "number", "selectionLength", "showingAlternateScreen", "isLocalhost", "tmuxWindow"];

/// A variable's text as iTerm2 types it: a number for the numeric names when it reads as
/// one, a string otherwise.
pub fn typed(name: &str, text: &str) -> serde_json::Value {
    if NUMERIC.contains(&name) {
        if let Ok(n) = text.parse::<i64>() {
            return n.into();
        }
        if let Some(n) = text.parse::<f64>().ok().and_then(serde_json::Number::from_f64) {
            return serde_json::Value::Number(n);
        }
    }
    serde_json::Value::String(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn values(pairs: &[(&str, serde_json::Value)]) -> Values {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn variable_store_follows_iterm2() {
        let mut s = Store::default();
        assert_eq!(s.set_user(Scope::Tab, "3", "title", json!("x")), Err(SetError::NotUser));
        assert_eq!(s.set_user(Scope::Tab, "3", "user.x", json!(1)), Ok(()));
        assert_eq!(s.user(Scope::Tab, "3"), values(&[("x", json!(1))]));
        assert!(s.user(Scope::Tab, "4").is_empty(), "another object");
        assert!(s.user(Scope::Window, "3").is_empty(), "another scope");
        s.set_user(Scope::Tab, "3", "user.x", serde_json::Value::Null).unwrap();
        assert!(s.user(Scope::Tab, "3").is_empty(), "null removes it");

        assert_eq!(s.observe(Scope::Tab, "3", values(&[("title", json!("a"))])), vec![], "the first observation is the baseline");
        assert_eq!(s.observe(Scope::Tab, "3", values(&[("title", json!("b"))])), vec![Change { scope: Scope::Tab, id: "3".into(), name: "title".into(), json: "\"b\"".into() }]);
        assert_eq!(s.observe(Scope::Tab, "3", values(&[("title", json!("b"))])), vec![], "nothing changed");
        let got = s.observe(Scope::Tab, "3", values(&[("z", json!(2)), ("a", json!(1))]));
        let names: Vec<(&str, &str)> = got.iter().map(|c| (c.name.as_str(), c.json.as_str())).collect();
        assert_eq!(names, vec![("a", "1"), ("title", "null"), ("z", "2")], "sorted by name; a name gone is null");

        s.set_user(Scope::Tab, "3", "user.y", json!("v")).unwrap();
        s.forget(Scope::Tab, "3");
        assert!(s.user(Scope::Tab, "3").is_empty(), "forget drops user variables");
        assert_eq!(s.observe(Scope::Tab, "3", values(&[("title", json!("c"))])), vec![], "a baseline again after forget");

        assert_eq!((Scope::Session.proto(), Scope::Tab.proto(), Scope::Window.proto(), Scope::App.proto()), (1, 2, 3, 4));
    }

    #[test]
    fn typed_values_follow_iterm2() {
        assert_eq!(typed("columns", "80"), json!(80));
        assert_eq!(typed("pid", "12"), json!(12));
        assert_eq!(typed("path", "12"), json!("12"));
        assert_eq!(typed("columns", "x"), json!("x"));
        assert_eq!(typed("tmuxWindow", "-1"), json!(-1));
    }
}
