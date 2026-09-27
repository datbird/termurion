//! iTerm2's actions (iTermActionsModel): a titled key action with its parameter, escaping,
//! and apply mode, kept in the `Actions` setting and listed in Edit > Actions; and the
//! apply modes every key action has (which sessions it runs in).

use serde_json::{Value as Json, json};
use toml::Value;

use crate::keyaction::{Binding, Escaping};

pub const VERSION: i64 = 2;

/// iTerm2's apply modes with the key action editor's names.
pub const APPLY_MODES: [(i64, &str); 6] = [
    (0, "Apply to current session"),
    (1, "Apply to all sessions"),
    (2, "Apply to all sessions except current"),
    (3, "Apply to all sessions in window"),
    (4, "Apply to all sessions in tab"),
    (5, "Apply to broadcasted-to sessions"),
];

#[derive(Clone, Debug, PartialEq)]
pub struct UserAction {
    pub action: i64,
    pub title: String,
    pub parameter: String,
    pub escaping: Escaping,
    pub apply_mode: i64,
    pub version: i64,
}

impl UserAction {
    pub fn from_json(v: &Json) -> Option<UserAction> {
        let o = v.as_object()?;
        let int = |k: &str| o.get(k).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))).unwrap_or(0);
        let text = |k: &str| o.get(k).and_then(Json::as_str).unwrap_or_default().to_string();
        let version = int("version");
        let escaping = match version {
            0 => Escaping::Compatibility,
            1 => Escaping::Common,
            _ => Escaping::from_code(int("escaping")),
        };
        Some(UserAction { action: int("action"), title: text("title"), parameter: text("parameter"), escaping, apply_mode: int("applyMode"), version })
    }

    pub fn to_json(&self) -> Json {
        json!({"action": self.action, "title": self.title, "parameter": self.parameter, "version": self.version, "escaping": self.escaping.code(), "applyMode": self.apply_mode})
    }

    pub fn from_value(v: &Value) -> Option<UserAction> {
        UserAction::from_json(&serde_json::to_value(v).ok()?)
    }

    pub fn to_value(&self) -> Value {
        let mut t = toml::Table::new();
        t.insert("action".into(), Value::Integer(self.action));
        t.insert("title".into(), Value::String(self.title.clone()));
        t.insert("parameter".into(), Value::String(self.parameter.clone()));
        t.insert("version".into(), Value::Integer(self.version));
        t.insert("escaping".into(), Value::Integer(self.escaping.code()));
        t.insert("applyMode".into(), Value::Integer(self.apply_mode));
        Value::Table(t)
    }

    /// The key binding that performs it.
    pub fn binding(&self) -> Binding {
        Binding { action: self.action, text: self.parameter.clone(), label: self.title.clone(), version: VERSION, escaping: Some(self.escaping.code()), apply_mode: self.apply_mode }
    }
}

/// The `Actions` setting.
pub fn load(v: Option<&Value>) -> Vec<UserAction> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(UserAction::from_value).collect()).unwrap_or_default()
}

pub fn store(actions: &[UserAction]) -> Value {
    Value::Array(actions.iter().map(UserAction::to_value).collect())
}

/// An `.it2actions` file: a JSON array of action dictionaries.
pub fn import(text: &str) -> Result<Vec<UserAction>, String> {
    let v: Json = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let list = v.as_array().ok_or_else(|| "Malformed file".to_string())?;
    list.iter().map(|a| UserAction::from_json(a).ok_or_else(|| "Malformed file".to_string())).collect()
}

pub fn export(actions: &[&UserAction]) -> String {
    let list: Vec<Json> = actions.iter().map(|a| a.to_json()).collect();
    serde_json::to_string_pretty(&list).unwrap_or_default()
}

/// Actions whose editor offers an apply mode (iTerm2's applyHidden = NO).
pub fn shows_apply_mode(action: i64) -> bool {
    // Send Snippet, hex codes, the Send Text kinds, coprocesses, escape sequences, Set
    // Profile, color presets, Find Regular Expression, Copy Mode commands, script
    // functions, and the two Paste Special actions.
    matches!(action, 63 | 11 | 38 | 76 | 12 | 35 | 10 | 37 | 40 | 36 | 73 | 60 | 41 | 42)
}

/// Actions Tandem runs once per target session.
pub fn fans_out(action: i64) -> bool {
    matches!(action, 10 | 11 | 12 | 35 | 36 | 37 | 38 | 41 | 42 | 60 | 63 | 73 | 76)
}

/// iTerm2's sessionsForActionApplyMode. An empty `broadcast` means broadcasting is off.
pub fn targets<T: Copy + Eq>(mode: i64, current: T, all: &[T], window: &[T], tab: &[T], broadcast: &[T]) -> Vec<T> {
    match mode {
        0 => vec![current],
        1 => all.to_vec(),
        2 => all.iter().copied().filter(|&t| t != current).collect(),
        3 => window.to_vec(),
        4 => tab.to_vec(),
        5 if broadcast.is_empty() => vec![current],
        5 => broadcast.to_vec(),
        _ => Vec::new(),
    }
}

/// An action's name with iTerm2's apply-mode prefix ("In all sessions, ...").
pub fn with_apply_mode(name: &str, mode: i64) -> String {
    match mode {
        1 => format!("In all sessions, {name}"),
        2 => format!("In unfocused sessions, {name}"),
        3 => format!("In all sessions in the window, {name}"),
        4 => format!("In all sessions in the tab, {name}"),
        5 => format!("In all broadcasted-to sessions, {name}"),
        _ => name.to_string(),
    }
}

/// Send Snippet's name: the snippet's title, or that it was deleted.
pub fn snippet_action_name(title: Option<&str>) -> String {
    match title {
        Some(t) => format!("Send Snippet \u{201c}{t}\u{201d}"),
        None => "Send Deleted Snippet (no action)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_decode_like_iterm2() {
        let old = UserAction::from_json(&json!({"action": 12, "title": "hi", "parameter": "echo\\n"})).unwrap();
        assert_eq!((old.escaping, old.apply_mode, old.version), (Escaping::Compatibility, 0, 0));
        let v1 = UserAction::from_json(&json!({"action": 12, "title": "hi", "parameter": "x", "version": 1, "escaping": 0})).unwrap();
        assert_eq!(v1.escaping, Escaping::Common);
        let v2 = UserAction::from_json(&json!({"action": 63, "title": "s", "parameter": "{\"guid\":\"G\"}", "version": 2, "escaping": 3, "applyMode": 4})).unwrap();
        assert_eq!((v2.action, v2.escaping, v2.apply_mode), (63, Escaping::Vim, 4));
        assert_eq!(v2.to_json(), json!({"action": 63, "title": "s", "parameter": "{\"guid\":\"G\"}", "version": 2, "escaping": 3, "applyMode": 4}));
        assert_eq!(UserAction::from_json(&json!(5)), None);
        // The setting round trips; a missing setting is no actions.
        assert_eq!(load(None), Vec::<UserAction>::new());
        let list = vec![old.clone(), v2.clone()];
        assert_eq!(load(Some(&store(&list))), list);
        assert_eq!(UserAction::from_value(&v2.to_value()), Some(v2.clone()));
        // Files.
        assert_eq!(import(&export(&[&old, &v2])).unwrap(), list);
        assert!(import("{}").unwrap_err().starts_with("Malformed file"));
        assert!(import("[").is_err());
        // The binding it runs.
        let b = v2.binding();
        assert_eq!((b.action, b.text.as_str(), b.version, b.escaping, b.apply_mode), (63, "{\"guid\":\"G\"}", 2, Some(3), 4));
        // iTerm2's key action editor: the popup shows for exactly these.
        let shown: Vec<i64> = (-1..=76).filter(|&c| shows_apply_mode(c)).collect();
        assert_eq!(shown, vec![10, 11, 12, 35, 36, 37, 38, 40, 41, 42, 60, 63, 73, 76]);
    }

    #[test]
    fn every_apply_mode_action_fans_out() {
        // A mode the person chose never runs only in the current session.
        for code in (-1..=76).filter(|&c| shows_apply_mode(c)) {
            assert!(fans_out(code) || matches!(crate::keyaction::status(code), crate::commands::Status::Later(_)), "{code}");
        }
    }

    #[test]
    fn apply_modes_pick_targets() {
        let all = [1, 2, 3, 4, 5];
        let window = [1, 2, 3];
        let tab = [2, 3];
        assert_eq!(targets(0, 2, &all, &window, &tab, &[]), vec![2]);
        assert_eq!(targets(1, 2, &all, &window, &tab, &[]), vec![1, 2, 3, 4, 5]);
        assert_eq!(targets(2, 2, &all, &window, &tab, &[]), vec![1, 3, 4, 5]);
        assert_eq!(targets(3, 2, &all, &window, &tab, &[]), vec![1, 2, 3]);
        assert_eq!(targets(4, 2, &all, &window, &tab, &[]), vec![2, 3]);
        assert_eq!(targets(5, 2, &all, &window, &tab, &[]), vec![2]);
        assert_eq!(targets(5, 2, &all, &window, &tab, &[2, 4]), vec![2, 4]);
        assert_eq!(targets(9, 2, &all, &window, &tab, &[]), Vec::<i32>::new());
    }

    #[test]
    fn descriptions_follow_iterm2() {
        assert_eq!(with_apply_mode("Send Text: ls", 0), "Send Text: ls");
        assert_eq!(with_apply_mode("Next Tab", 1), "In all sessions, Next Tab");
        assert_eq!(with_apply_mode("X", 2), "In unfocused sessions, X");
        assert_eq!(with_apply_mode("X", 3), "In all sessions in the window, X");
        assert_eq!(with_apply_mode("X", 4), "In all sessions in the tab, X");
        assert_eq!(with_apply_mode("X", 5), "In all broadcasted-to sessions, X");
        assert_eq!(snippet_action_name(Some("deploy")), "Send Snippet \u{201c}deploy\u{201d}");
        assert_eq!(snippet_action_name(None), "Send Deleted Snippet (no action)");
    }
}
