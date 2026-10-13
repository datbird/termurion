//! The Python API's variables: the store of user variables set through the API on sessions,
//! tabs, windows and the app (a session's SetUserVar values live in its shell state), and
//! the last values seen of each scope, whose differences are VARIABLE_CHANGE notifications
//! (`Language/iTermVariables.m`, `H:1866-1877`); VariableRequest's answer (`H:3106-3267`);
//! and the VARIABLE_CHANGE subscriptions (`H:1804-1864`, `H:2025-2051`, `H:893-925`).

use std::collections::{BTreeMap, BTreeSet};

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

/// A scope's values as the expression language reads them (a path is a key).
impl crate::expr::Scope for Values {
    fn value(&self, path: &str) -> Option<serde_json::Value> {
        self.get(path).cloned()
    }
}

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

    /// Takes `now` as the object's baseline unless it has one, so a change made right after
    /// a subscription is still seen at the next observation.
    pub fn baseline(&mut self, scope: Scope, id: &str, now: Values) {
        self.seen.entry((scope, id.to_string())).or_insert(now);
    }

    /// Keeps the last observed values only of the objects still watched: one no longer
    /// watched starts from a new baseline when it is watched again.
    pub fn keep_seen(&mut self, watched: &[(Scope, String)]) {
        self.seen.retain(|k, _| watched.contains(k));
    }

    /// Drops an object that closed: its user variables and its last observed values.
    pub fn forget(&mut self, scope: Scope, id: &str) {
        let key = (scope, id.to_string());
        self.user.remove(&key);
        self.seen.remove(&key);
    }
}

/// Names whose values are numbers in iTerm2's scopes.
pub const NUMERIC: &[&str] = &["columns", "rows", "pid", "effective_root_pid", "jobPid", "number", "selectionLength", "showingAlternateScreen", "isLocalhost", "tmuxWindow", "sshIntegrationLevel"];

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

/// Whether a VariableRequest may set `name`: "user." then no further dot (`H:3108-3111`).
pub fn legal_set_name(name: &str) -> bool {
    name.strip_prefix("user.").is_some_and(|n| !n.contains('.'))
}

/// Puts `vars` into `out` as `prefix` + name, each typed as iTerm2 types it.
fn put_typed<K: AsRef<str>>(out: &mut Values, prefix: &str, vars: &BTreeMap<K, String>) {
    for (k, v) in vars {
        out.insert(format!("{prefix}{}", k.as_ref()), typed(k.as_ref(), v));
    }
}

/// Puts user variables into `out` as `prefix` + name, as the text `Vars` carries.
fn put_user(out: &mut Values, prefix: &str, vars: &BTreeMap<String, String>) {
    for (k, v) in vars {
        out.insert(format!("{prefix}{k}"), serde_json::Value::String(v.clone()));
    }
}

/// Every name a scope resolves, with its value, from the variables of the session that
/// stands for it: a session sees its own names, `user.`, `tab.` (with `tab.window.`) and
/// `iterm2.` (`iTermVariableScope+Session.m:14-19`); a tab its own, `user.` and `window.`
/// (`+Tab.m:15-19`); a window its own, `user.`, `iterm2.` and its current tab as
/// `currentTab.` (`+Window.m:15-23`); the app its own and `user.`.
pub fn scope_values(v: &crate::prompted::Vars, scope: Scope) -> Values {
    let mut out = Values::new();
    let window = |out: &mut Values, prefix: &str| {
        put_typed(out, prefix, &v.window);
        put_user(out, &format!("{prefix}user."), &v.window_user);
    };
    let tab = |out: &mut Values, prefix: &str| {
        put_typed(out, prefix, &v.tab);
        put_user(out, &format!("{prefix}user."), &v.tab_user);
        window(out, &format!("{prefix}window."));
    };
    let app = |out: &mut Values, prefix: &str| {
        put_typed(out, prefix, &v.app);
        put_user(out, &format!("{prefix}user."), &v.app_user);
    };
    match scope {
        Scope::Session => {
            put_typed(&mut out, "", &v.session);
            put_user(&mut out, "user.", &v.user);
            tab(&mut out, "tab.");
            app(&mut out, "iterm2.");
        }
        Scope::Tab => tab(&mut out, ""),
        Scope::Window => {
            window(&mut out, "");
            app(&mut out, "iterm2.");
            put_typed(&mut out, "currentTab.", &v.tab);
            put_user(&mut out, "currentTab.user.", &v.tab_user);
        }
        Scope::App => app(&mut out, ""),
    }
    out
}

/// Puts back the JSON types of the API's user values over the text `Vars` carries: the
/// object's own (`id` in `scope`) and those of the tab, window and app it reaches, whose
/// ids come from `v`.
pub fn typed_users(store: &Store, v: &crate::prompted::Vars, scope: Scope, id: &str, out: &mut Values) {
    let mut put = |prefix: &str, s: Scope, id: Option<&str>| {
        let Some(id) = id else { return };
        for (k, val) in store.user(s, id) {
            out.insert(format!("{prefix}user.{k}"), val);
        }
    };
    let tab = v.tab.get("id").map(String::as_str);
    let window = v.window.get("id").map(String::as_str);
    match scope {
        Scope::Session => {
            put("", Scope::Session, Some(id));
            put("tab.", Scope::Tab, tab);
            put("tab.window.", Scope::Window, window);
            put("iterm2.", Scope::App, Some(""));
        }
        Scope::Tab => {
            put("", Scope::Tab, Some(id));
            put("window.", Scope::Window, window);
        }
        Scope::Window => {
            put("", Scope::Window, Some(id));
            put("iterm2.", Scope::App, Some(""));
            put("currentTab.", Scope::Tab, tab);
        }
        Scope::App => put("", Scope::App, Some("")),
    }
}

/// VariableRequest's gets (`H:3242-3253`): "*" is every value as a string in one JSON
/// dictionary; any other name the JSON of its value, "null" when unset.
/// Whether a session's VariableRequest gets need its selection (a copy of the whole
/// selection, so it is read only then): `*`, `selection` or `selectionLength`.
pub fn wants_selection(names: &[String]) -> bool {
    names.iter().any(|n| n == "*" || crate::prompted::names_selection(n))
}

pub fn gets(names: &[String], values: &Values) -> Vec<String> {
    names
        .iter()
        .map(|name| {
            if name == "*" {
                let strings: BTreeMap<&str, String> = values.iter().map(|(k, v)| (k.as_str(), v.as_str().map_or_else(|| v.to_string(), str::to_string))).collect();
                serde_json::to_string(&strings).unwrap_or_else(|_| "{}".to_string())
            } else {
                values.get(name).map_or_else(|| "null".to_string(), |v| v.to_string())
            }
        })
        .collect()
}

/// A set's value (`H:3230-3235`): "null", and JSON that does not parse, are null, which
/// unsets.
pub fn set_value(json: &str) -> serde_json::Value {
    if json == "null" {
        return serde_json::Value::Null;
    }
    serde_json::from_str(json).unwrap_or(serde_json::Value::Null)
}

/// The sessions, tabs and windows ids name, and their values.
pub trait Objects {
    /// The object `id` names in `scope`, as the store keys it (a session's pane id, a tab's
    /// id, a window's `pty-` id, "" for the app, which every id names), or None.
    fn id(&self, scope: Scope, id: &str) -> Option<String>;
    /// Every object of `scope` (iTerm2's `allSessions`, `allTabs`, `terminals`).
    fn all(&self, scope: Scope) -> Vec<String>;
    /// The values of the object the store keys as `id` (`scope_values`).
    fn values(&self, scope: Scope, id: &str) -> Values;

    fn exists(&self, scope: Scope, id: &str) -> bool {
        self.id(scope, id).is_some()
    }
}

/// A set to apply: the scope, the object's id, the name and the value (null unsets).
pub type Set = (Scope, String, String, serde_json::Value);

/// VariableRequest (`H:3106-3267`): every set name legal or INVALID_NAME, then the scope
/// (MISSING_SCOPE); tab and session "all" set on every object and refuse gets
/// (MULTI_GET_DISALLOWED); window "all" is looked up like any id, as iTerm2 tests
/// `session_id` for it (kept); an unknown object is TAB_, WINDOW_ or SESSION_NOT_FOUND;
/// sets then gets, the status left unset except for the app (OK). Tab and session "all"
/// answer OK explicitly too (`H:3255-3267`), with no values.
pub fn answer(r: &pb::VariableRequest, o: &dyn Objects) -> (pb::VariableResponse, Vec<Set>) {
    use pb::variable_request::Scope as S;
    use pb::variable_response::Status;
    let refuse = |s: Status| (pb::VariableResponse { status: Some(s as i32), values: Vec::new() }, Vec::new());
    if !r.set.iter().all(|s| legal_set_name(s.name())) {
        return refuse(Status::InvalidName);
    }
    let (scope, id, missing) = match &r.scope {
        None => return refuse(Status::MissingScope),
        Some(S::App(_)) => (Scope::App, "", Status::Ok),
        Some(S::TabId(id)) => (Scope::Tab, id.as_str(), Status::TabNotFound),
        Some(S::WindowId(id)) => (Scope::Window, id.as_str(), Status::WindowNotFound),
        Some(S::SessionId(id)) => (Scope::Session, id.as_str(), Status::SessionNotFound),
    };
    let sets_on = |id: &str| -> Vec<Set> { r.set.iter().map(|s| (scope, id.to_string(), s.name().to_string(), set_value(s.value()))).collect() };
    if id == crate::api_session::ALL && matches!(scope, Scope::Tab | Scope::Session) {
        if !r.get.is_empty() {
            return refuse(Status::MultiGetDisallowed);
        }
        let sets = o.all(scope).iter().flat_map(|id| sets_on(id)).collect();
        return (pb::VariableResponse { status: Some(Status::Ok as i32), values: Vec::new() }, sets);
    }
    let Some(id) = o.id(scope, id) else { return refuse(missing) };
    let sets = sets_on(&id);
    let mut values = o.values(scope, &id);
    for (_, _, name, value) in &sets {
        if value.is_null() {
            values.remove(name);
        } else {
            values.insert(name.clone(), value.clone());
        }
    }
    let status = (scope == Scope::App).then_some(Status::Ok as i32);
    (pb::VariableResponse { status, values: gets(&r.get, &values) }, sets)
}

/// The VARIABLE_CHANGE subscriptions: each connection's for one object (with the id the
/// request's identifier named, fixed when it subscribed, as iTerm2 keeps the object's
/// scope), and those for every session or window, present and future (`H:893-925`).
#[derive(Clone, Debug, Default)]
pub struct Subs {
    one: BTreeMap<u64, Vec<(pb::VariableMonitorRequest, String)>>,
    all_sessions: Vec<(u64, pb::VariableMonitorRequest)>,
    all_windows: Vec<(u64, pb::VariableMonitorRequest)>,
}

impl Subs {
    /// iTerm2's answer to a VARIABLE_CHANGE (un)subscribe (`H:1804-1864`): no `subscribe`
    /// REQUEST_MALFORMED; "all" for sessions and windows always subscribes (a repeat too,
    /// `H:2025-2051`) and unsubscribes every match or NOT_SUBSCRIBED; "all" for the app and
    /// tabs INVALID_IDENTIFIER; otherwise a repeat ALREADY_SUBSCRIBED, an object not found
    /// INVALID_IDENTIFIER, an unsubscribe without a match NOT_SUBSCRIBED.
    pub fn request(&mut self, conn: u64, r: &pb::NotificationRequest, o: &dyn Objects) -> pb::notification_response::Status {
        use pb::notification_response::Status;
        let Some(subscribe) = r.subscribe else { return Status::RequestMalformed };
        let m = match &r.arguments {
            Some(pb::notification_request::Arguments::VariableMonitorRequest(m)) => m.clone(),
            _ => pb::VariableMonitorRequest::default(),
        };
        let scope = scope_of(&m);
        if m.identifier() == crate::api_session::ALL {
            let list = match scope {
                Scope::Session => &mut self.all_sessions,
                Scope::Window => &mut self.all_windows,
                Scope::App | Scope::Tab => return Status::InvalidIdentifier,
            };
            if subscribe {
                list.push((conn, m));
                return Status::Ok;
            }
            let before = list.len();
            list.retain(|(c, x)| !(*c == conn && *x == m));
            return if list.len() == before { Status::NotSubscribed } else { Status::Ok };
        }
        let mine = self.one.entry(conn).or_default();
        let at = mine.iter().position(|(x, _)| *x == m);
        let status = match (subscribe, at) {
            (true, Some(_)) => Status::AlreadySubscribed,
            (true, None) => match o.id(scope, m.identifier()) {
                Some(id) => {
                    mine.push((m, id));
                    Status::Ok
                }
                None => Status::InvalidIdentifier,
            },
            (false, None) => Status::NotSubscribed,
            (false, Some(i)) => {
                mine.remove(i);
                Status::Ok
            }
        };
        if mine.is_empty() {
            self.one.remove(&conn);
        }
        status
    }

    /// Any subscription exists.
    pub fn any(&self) -> bool {
        !self.one.is_empty() || !self.all_sessions.is_empty() || !self.all_windows.is_empty()
    }

    /// The objects to observe, each once, by scope then id: those subscribed one by one that
    /// still exist, and every session or window while "all" of them is watched.
    pub fn watched(&self, o: &dyn Objects) -> Vec<(Scope, String)> {
        let mut out = BTreeSet::new();
        for (m, id) in self.one.values().flatten() {
            let scope = scope_of(m);
            if o.exists(scope, id) {
                out.insert((scope, id.clone()));
            }
        }
        for (scope, list) in [(Scope::Session, &self.all_sessions), (Scope::Window, &self.all_windows)] {
            if !list.is_empty() {
                out.extend(o.all(scope).into_iter().map(|id| (scope, id)));
            }
        }
        out.into_iter().collect()
    }

    /// Who hears a change of `name` on the object `id` of `scope`: each connection with the
    /// identifier to report, the request's for one object, the object's own for "all"
    /// (`H:1866-1877`); one entry per subscription.
    pub fn subscribers(&self, scope: Scope, id: &str, name: &str) -> Vec<(u64, String)> {
        let mut out: Vec<(u64, String)> = self
            .one
            .iter()
            .flat_map(|(conn, subs)| subs.iter().filter(|(m, x)| scope_of(m) == scope && x == id && m.name() == name).map(|(m, _)| (*conn, m.identifier().to_string())))
            .collect();
        let all = match scope {
            Scope::Session => &self.all_sessions[..],
            Scope::Window => &self.all_windows[..],
            Scope::Tab | Scope::App => &[],
        };
        out.extend(all.iter().filter(|(_, m)| m.name() == name).map(|(conn, _)| (*conn, id.to_string())));
        out
    }

    /// A closed connection's subscriptions go (`H:2154-2192`).
    pub fn drop_conn(&mut self, conn: u64) {
        self.one.remove(&conn);
        self.all_sessions.retain(|(c, _)| *c != conn);
        self.all_windows.retain(|(c, _)| *c != conn);
    }
}

/// The scope of a monitor request (SESSION when unset, the proto2 default).
fn scope_of(m: &pb::VariableMonitorRequest) -> Scope {
    match m.scope() {
        pb::VariableScope::Session => Scope::Session,
        pb::VariableScope::Tab => Scope::Tab,
        pb::VariableScope::Window => Scope::Window,
        pb::VariableScope::App => Scope::App,
    }
}

/// The VARIABLE_CHANGE notification (`H:1866-1877`): the scope, the identifier, the name and
/// the new value's JSON ("null" when unset).
pub fn change_notification(scope: Scope, identifier: &str, name: &str, json: &str) -> pb::Notification {
    pb::Notification {
        variable_changed_notification: Some(pb::VariableChangedNotification {
            scope: Some(scope.proto()),
            identifier: Some(identifier.to_string()),
            name: Some(name.to_string()),
            json_new_value: Some(json.to_string()),
        }),
        ..Default::default()
    }
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

    /// Sessions "1" and "2", tab "5", window "pty-A" and the app; session "1" holds columns
    /// 80 and title "t".
    struct Fixture;

    impl Objects for Fixture {
        fn id(&self, scope: Scope, id: &str) -> Option<String> {
            let known = match scope {
                Scope::Session => ["1", "2"].contains(&id),
                Scope::Tab => id == "5",
                Scope::Window => id == "pty-A",
                Scope::App => return Some(String::new()),
            };
            known.then(|| id.to_string())
        }

        fn all(&self, scope: Scope) -> Vec<String> {
            let ids: &[&str] = match scope {
                Scope::Session => &["1", "2"],
                Scope::Tab => &["5"],
                Scope::Window => &["pty-A"],
                Scope::App => &[""],
            };
            ids.iter().map(|s| s.to_string()).collect()
        }

        fn values(&self, scope: Scope, id: &str) -> Values {
            if (scope, id) == (Scope::Session, "1") {
                values(&[("columns", json!(80)), ("title", json!("t"))])
            } else {
                Values::new()
            }
        }
    }

    use pb::variable_request::Scope as VS;
    use pb::variable_response::Status as VStatus;

    fn vreq(scope: Option<VS>, sets: &[(&str, &str)], gets: &[&str]) -> pb::VariableRequest {
        pb::VariableRequest {
            scope,
            set: sets.iter().map(|(n, v)| pb::variable_request::Set { name: Some(n.to_string()), value: Some(v.to_string()) }).collect(),
            get: gets.iter().map(|g| g.to_string()).collect(),
        }
    }

    fn status(r: &pb::VariableResponse) -> Option<VStatus> {
        r.status.map(|s| VStatus::try_from(s).unwrap())
    }

    #[test]
    fn variable_request_follows_iterm2() {
        assert!(legal_set_name("user.x"));
        assert!(!legal_set_name("user.a.b"));
        assert!(!legal_set_name("x"));
        assert!(legal_set_name("user."));

        let o = Fixture;
        let session = |id: &str| Some(VS::SessionId(id.into()));
        // Names first, even with no scope (`H:3106-3118`).
        for scope in [session("1"), None] {
            let (r, sets) = answer(&vreq(scope, &[("title", "\"x\"")], &[]), &o);
            assert_eq!((status(&r), sets.len()), (Some(VStatus::InvalidName), 0));
        }
        let (r, _) = answer(&vreq(None, &[], &["columns"]), &o);
        assert_eq!(status(&r), Some(VStatus::MissingScope));
        assert_eq!(status(&answer(&vreq(session("99"), &[], &["columns"]), &o).0), Some(VStatus::SessionNotFound));
        assert_eq!(status(&answer(&vreq(Some(VS::TabId("77".into())), &[], &[]), &o).0), Some(VStatus::TabNotFound));
        assert_eq!(status(&answer(&vreq(Some(VS::WindowId("pty-Z".into())), &[], &[]), &o).0), Some(VStatus::WindowNotFound));
        // Kept: iTerm2 tests `session_id` for window "all" (`H:3176-3185`).
        assert_eq!(status(&answer(&vreq(Some(VS::WindowId("all".into())), &[("user.x", "1")], &[]), &o).0), Some(VStatus::WindowNotFound));
        let (r, sets) = answer(&vreq(session("all"), &[("user.x", "1")], &["columns"]), &o);
        assert_eq!((status(&r), sets.len()), (Some(VStatus::MultiGetDisallowed), 0));
        let (r, sets) = answer(&vreq(session("all"), &[("user.x", "1")], &[]), &o);
        assert_eq!(status(&r), Some(VStatus::Ok));
        assert_eq!(sets, vec![(Scope::Session, "1".into(), "user.x".into(), json!(1)), (Scope::Session, "2".into(), "user.x".into(), json!(1))]);

        let (r, sets) = answer(&vreq(session("1"), &[], &["columns", "nope", "*"]), &o);
        assert_eq!(r.values, vec!["80", "null", "{\"columns\":\"80\",\"title\":\"t\"}"]);
        assert_eq!(status(&r), None, "a session answer leaves the status unset");
        assert!(sets.is_empty());
        // Sets come before gets, a null unsets.
        let (r, sets) = answer(&vreq(session("1"), &[("user.y", "[1]")], &["user.y"]), &o);
        assert_eq!(r.values, vec!["[1]"]);
        assert_eq!(sets, vec![(Scope::Session, "1".into(), "user.y".into(), json!([1]))]);
        let (r, _) = answer(&vreq(Some(VS::TabId("5".into())), &[("user.z", "null")], &["user.z"]), &o);
        assert_eq!((status(&r), r.values.clone()), (None, vec!["null".to_string()]));
        let (r, sets) = answer(&vreq(Some(VS::App(true)), &[("user.a", "\"v\"")], &["user.a"]), &o);
        assert_eq!((status(&r), r.values.clone()), (Some(VStatus::Ok), vec!["\"v\"".to_string()]));
        assert_eq!(sets, vec![(Scope::App, String::new(), "user.a".into(), json!("v"))]);
        assert_eq!(status(&answer(&vreq(Some(VS::TabId("all".into())), &[], &[]), &o).0), Some(VStatus::Ok));

        assert_eq!(set_value("null"), serde_json::Value::Null);
        assert_eq!(set_value("{bad"), serde_json::Value::Null);
        assert_eq!(set_value("\"s\""), json!("s"));
    }

    #[test]
    fn api_selection_follows_iterm2() {
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        assert!(wants_selection(&names(&["selection"])));
        assert!(wants_selection(&names(&["columns", "selectionLength"])));
        assert!(wants_selection(&names(&["*"])));
        assert!(!wants_selection(&names(&["columns", "user.selection"])));
        // A session's selection is an ordinary session variable to the API.
        let mut v = crate::prompted::Vars::default();
        v.session.insert("selection", "abc".into());
        v.session.insert("selectionLength", "3".into());
        let s = scope_values(&v, Scope::Session);
        assert_eq!(gets(&names(&["selection", "selectionLength"]), &s), ["\"abc\"", "3"]);
    }

    #[test]
    fn scope_values_follow_iterm2() {
        let mut v = crate::prompted::Vars::default();
        v.session.insert("columns", "80".into());
        v.user.insert("a".into(), "1".into());
        v.tab.insert("id", "5".into());
        v.tab_user.insert("b".into(), "2".into());
        v.window.insert("id", "pty-A".into());
        v.app.insert("pid", "9".into());
        let s = scope_values(&v, Scope::Session);
        assert_eq!(s.get("columns"), Some(&json!(80)));
        assert_eq!(s.get("user.a"), Some(&json!("1")));
        assert_eq!(s.get("tab.id"), Some(&json!("5")));
        assert_eq!(s.get("tab.user.b"), Some(&json!("2")));
        assert_eq!(s.get("tab.window.id"), Some(&json!("pty-A")));
        assert_eq!(s.get("iterm2.pid"), Some(&json!(9)));
        let t = scope_values(&v, Scope::Tab);
        assert_eq!((t.get("id"), t.get("user.b"), t.get("window.id")), (Some(&json!("5")), Some(&json!("2")), Some(&json!("pty-A"))));
        assert_eq!(t.get("columns"), None);
        let w = scope_values(&v, Scope::Window);
        assert_eq!(w.get("id"), Some(&json!("pty-A")));
        assert_eq!(w.get("iterm2.pid"), Some(&json!(9)));
        assert_eq!(w.get("currentTab.id"), Some(&json!("5")));
        let a = scope_values(&v, Scope::App);
        assert_eq!(a.get("pid"), Some(&json!(9)));
        assert_eq!(a.get("id"), None);

        // API user values keep their JSON types over the text `Vars` carries.
        let mut store = Store::default();
        store.set_user(Scope::Session, "1", "user.a", json!(1)).unwrap();
        store.set_user(Scope::Tab, "5", "user.b", json!([2])).unwrap();
        store.set_user(Scope::App, "", "user.c", json!(true)).unwrap();
        let mut s = scope_values(&v, Scope::Session);
        typed_users(&store, &v, Scope::Session, "1", &mut s);
        assert_eq!((s.get("user.a"), s.get("tab.user.b"), s.get("iterm2.user.c")), (Some(&json!(1)), Some(&json!([2])), Some(&json!(true))));
        let mut t = scope_values(&v, Scope::Tab);
        typed_users(&store, &v, Scope::Tab, "5", &mut t);
        assert_eq!(t.get("user.b"), Some(&json!([2])));
    }

    fn nreq(scope: Scope, id: Option<&str>, name: &str, subscribe: Option<bool>) -> pb::NotificationRequest {
        pb::NotificationRequest {
            session: None,
            subscribe,
            notification_type: Some(pb::NotificationType::NotifyOnVariableChange as i32),
            arguments: Some(pb::notification_request::Arguments::VariableMonitorRequest(pb::VariableMonitorRequest {
                name: Some(name.into()),
                scope: Some(scope.proto()),
                identifier: id.map(str::to_string),
            })),
        }
    }

    /// No objects at all (no window open).
    struct Empty;

    impl Objects for Empty {
        fn id(&self, _: Scope, _: &str) -> Option<String> {
            None
        }

        fn all(&self, _: Scope) -> Vec<String> {
            Vec::new()
        }

        fn values(&self, _: Scope, _: &str) -> Values {
            Values::new()
        }
    }

    #[test]
    fn all_unsubscribe_with_no_sessions_is_ok() {
        use pb::notification_response::Status as N;
        // Termurion keeps "all" as one request, not per-object entries: made while no session
        // exists, its unsubscribe finds it and answers OK. iTerm2 answers NOT_SUBSCRIBED here,
        // as it looks for per-session entries (m11b Task 4 ruling; only with no window open).
        let mut s = Subs::default();
        assert_eq!(s.request(1, &nreq(Scope::Session, Some("all"), "rows", Some(true)), &Empty), N::Ok);
        assert_eq!(s.request(1, &nreq(Scope::Session, Some("all"), "rows", Some(false)), &Empty), N::Ok);
        assert_eq!(s.request(1, &nreq(Scope::Session, Some("all"), "rows", Some(false)), &Empty), N::NotSubscribed);
        assert!(!s.any());
    }

    #[test]
    fn variable_subscriptions_follow_iterm2() {
        use pb::notification_response::Status as N;
        let o = Fixture;
        let mut s = Subs::default();
        assert!(!s.any());
        assert_eq!(s.request(1, &nreq(Scope::Session, Some("1"), "columns", None), &o), N::RequestMalformed);
        assert_eq!(s.request(1, &nreq(Scope::Session, Some("1"), "columns", Some(true)), &o), N::Ok);
        assert!(s.any());
        assert_eq!(s.request(1, &nreq(Scope::Session, Some("1"), "columns", Some(true)), &o), N::AlreadySubscribed);
        assert_eq!(s.request(2, &nreq(Scope::Session, Some("1"), "columns", Some(true)), &o), N::Ok, "another connection");
        assert_eq!(s.request(1, &nreq(Scope::Session, Some("99"), "columns", Some(true)), &o), N::InvalidIdentifier);
        assert_eq!(s.request(1, &nreq(Scope::App, Some("all"), "pid", Some(true)), &o), N::InvalidIdentifier);
        assert_eq!(s.request(1, &nreq(Scope::Tab, Some("all"), "id", Some(true)), &o), N::InvalidIdentifier);
        // iTerm2 never refuses a repeated "all" (`H:2025-2051`).
        assert_eq!(s.request(1, &nreq(Scope::Session, Some("all"), "rows", Some(true)), &o), N::Ok);
        assert_eq!(s.request(1, &nreq(Scope::Session, Some("all"), "rows", Some(true)), &o), N::Ok);
        assert_eq!(s.subscribers(Scope::Session, "2", "rows"), vec![(1, "2".to_string()), (1, "2".to_string())], "kept: a repeat posts twice");
        assert_eq!(s.subscribers(Scope::Session, "1", "columns"), vec![(1, "1".to_string()), (2, "1".to_string())]);
        assert!(s.subscribers(Scope::Session, "2", "columns").is_empty());
        assert_eq!(s.watched(&o), vec![(Scope::Session, "1".to_string()), (Scope::Session, "2".to_string())]);
        assert_eq!(s.request(1, &nreq(Scope::Session, Some("all"), "rows", Some(false)), &o), N::Ok);
        assert_eq!(s.request(1, &nreq(Scope::Session, Some("all"), "rows", Some(false)), &o), N::NotSubscribed);
        assert!(s.subscribers(Scope::Session, "2", "rows").is_empty());
        assert_eq!(s.request(1, &nreq(Scope::Session, Some("1"), "rows", Some(false)), &o), N::NotSubscribed);
        // Windows take "all" too.
        assert_eq!(s.request(1, &nreq(Scope::Window, Some("all"), "title", Some(true)), &o), N::Ok);
        assert_eq!(s.subscribers(Scope::Window, "pty-A", "title"), vec![(1, "pty-A".to_string())]);
        // The app, with the request's identifier.
        assert_eq!(s.request(1, &nreq(Scope::App, Some(""), "pid", Some(true)), &o), N::Ok);
        assert_eq!(s.subscribers(Scope::App, "", "pid"), vec![(1, String::new())]);
        assert_eq!(s.request(1, &nreq(Scope::Tab, Some("5"), "title", Some(true)), &o), N::Ok);
        assert_eq!(
            s.watched(&o),
            vec![(Scope::Session, "1".to_string()), (Scope::Tab, "5".to_string()), (Scope::Window, "pty-A".to_string()), (Scope::App, String::new())]
        );
        assert_eq!(s.request(1, &nreq(Scope::Session, Some("1"), "columns", Some(false)), &o), N::Ok);
        assert_eq!(s.subscribers(Scope::Session, "1", "columns"), vec![(2, "1".to_string())]);
        s.drop_conn(1);
        s.drop_conn(2);
        assert!(!s.any());
        assert!(s.watched(&o).is_empty());
    }

    #[test]
    fn variable_changes_are_posted() {
        let n = change_notification(Scope::Session, "1", "columns", "81");
        assert_eq!(
            n.variable_changed_notification,
            Some(pb::VariableChangedNotification { scope: Some(Scope::Session.proto()), identifier: Some("1".into()), name: Some("columns".into()), json_new_value: Some("81".into()) })
        );
        let mut store = Store::default();
        store.observe(Scope::Session, "1", values(&[("columns", json!(80))]));
        let c = store.observe(Scope::Session, "1", Values::new());
        let n = change_notification(c[0].scope, &c[0].id, &c[0].name, &c[0].json);
        assert_eq!(n.variable_changed_notification.and_then(|v| v.json_new_value).as_deref(), Some("null"), "a removed value");

        // A baseline taken at a subscription is kept by the next observation.
        store.baseline(Scope::Tab, "5", values(&[("title", json!("a"))]));
        store.baseline(Scope::Tab, "5", values(&[("title", json!("b"))]));
        assert_eq!(store.observe(Scope::Tab, "5", values(&[("title", json!("c"))])).len(), 1);
        // Objects no longer watched start over.
        store.keep_seen(&[(Scope::Session, "1".to_string())]);
        assert!(store.observe(Scope::Tab, "5", values(&[("title", json!("d"))])).is_empty());
        assert_eq!(store.observe(Scope::Session, "1", values(&[("columns", json!(1))])).len(), 1);
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
