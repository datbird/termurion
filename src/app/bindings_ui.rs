//! Expression bindings per session (`PS:6319-6333`, `PS:22742-22771`): each live pane evaluates
//! the expressions its profile binds to color wells and checkboxes and writes the answers to its
//! own profile copy. A binding is evaluated again when its dependencies' values change. Nothing
//! here wakes the loop unless a bound key is waiting to run again.

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use super::script_ui::{Origin, ScopeSpec};
use super::App;
use crate::api_vars::Values;
use crate::bindings;
use crate::expr;
use crate::layout::PaneId;
use crate::script_eval::{self, Timeout};

/// The least time between two evaluations of one binding (a program that churns a variable
/// costs one evaluation per this long, not one per change).
const MIN_GAP: Duration = Duration::from_millis(250);
/// A bound expression's evaluation time limit, as a status bar component's.
const TIMEOUT_SECS: f64 = 5.0;

/// One bound key of one pane: what it evaluates, the values it read last, and where its
/// evaluation stands.
pub(super) struct Bound {
    expression: String,
    paths: BTreeSet<String>,
    deps: String,
    /// The number of the latest evaluation started; an answer of another number is stale.
    nth: u64,
    running: bool,
    /// The dependencies changed after the latest start.
    dirty: bool,
    last_start: Option<Instant>,
}

/// An evaluation to start.
#[derive(Debug, PartialEq)]
pub(super) struct Start {
    pub key: String,
    pub expression: String,
    pub nth: u64,
}

/// Brings `pane`'s entries to what its profile binds (`wanted`: key and expression) and says
/// which evaluations to start now and when the next waiting one may start. An entry whose
/// binding went is dropped; one whose expression changed starts over; one whose dependencies
/// changed runs again, once at a time and at least `MIN_GAP` after the last start.
pub(super) fn plan(
    map: &mut HashMap<(PaneId, String), Bound>,
    pane: PaneId,
    wanted: &[(String, String)],
    paths_of: &dyn Fn(&str) -> BTreeSet<String>,
    deps_of: &dyn Fn(&BTreeSet<String>) -> String,
    now: Instant,
) -> (Vec<Start>, Option<Instant>) {
    map.retain(|(p, k), _| *p != pane || wanted.iter().any(|(w, _)| w == k));
    let mut starts = Vec::new();
    let mut due: Option<Instant> = None;
    for (key, expression) in wanted {
        let id = (pane, key.clone());
        let b = map.entry(id).or_insert_with(|| Bound { expression: String::new(), paths: BTreeSet::new(), deps: String::new(), nth: 0, running: false, dirty: false, last_start: None });
        if b.expression != *expression {
            b.expression = expression.clone();
            b.paths = paths_of(expression);
            b.deps = deps_of(&b.paths);
            // Bumped and not running, so an older evaluation's answer is stale.
            b.nth += 1;
            b.running = false;
            b.dirty = true;
        } else {
            let deps = deps_of(&b.paths);
            if deps != b.deps {
                b.deps = deps;
                b.dirty = true;
            }
        }
        if b.dirty && !b.running {
            let ready = b.last_start.map_or(now, |t| t + MIN_GAP);
            if ready <= now {
                b.dirty = false;
                b.running = true;
                b.nth += 1;
                b.last_start = Some(now);
                starts.push(Start { key: key.clone(), expression: expression.clone(), nth: b.nth });
            } else {
                due = Some(due.map_or(ready, |d| d.min(ready)));
            }
        }
    }
    (starts, due)
}

/// Whether an answer of evaluation `nth` still belongs to the entry (the binding was not
/// removed or replaced meanwhile, `PS:22744-22752`).
pub(super) fn accepts(map: &HashMap<(PaneId, String), Bound>, pane: PaneId, key: &str, nth: u64) -> bool {
    map.get(&(pane, key.to_string())).is_some_and(|b| b.nth == nth && b.running)
}

/// An evaluation finished: the entry may run again.
pub(super) fn finished(map: &mut HashMap<(PaneId, String), Bound>, pane: PaneId, key: &str) {
    if let Some(b) = map.get_mut(&(pane, key.to_string())) {
        b.running = false;
    }
}

/// What an answer does to the session.
#[derive(Debug, PartialEq)]
pub(super) enum Answer {
    /// Set (or, with None, remove) the key on the session copy.
    Write(Option<toml::Value>),
    /// Add this line to the global console log.
    Log(String),
    Nothing,
}

/// `boundVariableDidChange:value:` and the key's conversion (`PP:512-561`): no value restores the
/// profile's own (`restore`), a value is converted by the key's kind, a failed evaluation is logged.
pub(super) fn answer(key: &str, value: &Result<serde_json::Value, String>, restore: Option<&toml::Value>) -> Answer {
    let value = match value {
        Err(text) => return Answer::Log(text.clone()),
        Ok(v) => v,
    };
    if value.is_null() {
        return Answer::Write(restore.cloned());
    }
    if bindings::is_color_key(key) {
        return match bindings::color_from(value, key) {
            Ok(c) => Answer::Write(Some(c)),
            Err(line) if line.is_empty() => Answer::Nothing,
            Err(line) => Answer::Log(line),
        };
    }
    match bindings::bool_from(value) {
        Some(b) => Answer::Write(Some(toml::Value::Boolean(b))),
        None => Answer::Nothing,
    }
}

impl App {
    /// Starts the evaluations the bound keys of every live pane are due for. The time the next
    /// waiting one may start, for the loop's wake; None when nothing waits.
    pub(super) fn tick_bindings(&mut self) -> Option<Instant> {
        let panes: Vec<PaneId> = self.panes.keys().copied().collect();
        let mut map = std::mem::take(&mut self.bindings);
        let mut due: Option<Instant> = None;
        let mut starts: Vec<(PaneId, Start)> = Vec::new();
        let now = Instant::now();
        map.retain(|(p, _), _| self.panes.contains_key(p));
        for pane in panes {
            let Some(p) = self.panes.get(&pane) else { continue };
            let live = !(p.archived || p.ended || self.buried.contains(pane));
            let wanted = match self.store.get(&p.profile) {
                Some(profile) if live && profile.values.contains_key(bindings::KEY) => bindings::bound(profile),
                _ => Vec::new(),
            };
            if wanted.is_empty() && !map.keys().any(|(p, _)| *p == pane) {
                continue;
            }
            // The scope's values with and without the selection, each built at most once.
            let cache: std::cell::RefCell<[Option<Values>; 2]> = Default::default();
            let deps_of = |paths: &BTreeSet<String>| {
                let sel = paths.iter().any(|x| crate::prompted::names_selection(x));
                let mut c = cache.borrow_mut();
                let values = c[usize::from(sel)].get_or_insert_with(|| self.session_values(pane, sel).unwrap_or_default());
                script_eval::deps_key(paths, values)
            };
            let paths_of = |text: &str| {
                let mut out = BTreeSet::new();
                if let Ok(e) = expr::parse_expression(text) {
                    script_eval::paths_in(&e, &mut out);
                }
                out
            };
            let (s, d) = plan(&mut map, pane, &wanted, &paths_of, &deps_of, now);
            starts.extend(s.into_iter().map(|s| (pane, s)));
            due = [due, d].into_iter().flatten().min();
        }
        self.bindings = map;
        for (pane, s) in starts {
            let scope = ScopeSpec::Session { pane, frame: Values::new() };
            self.invoke_expression(&s.expression, scope, Timeout::from_secs(TIMEOUT_SECS), Origin::Binding { pane, key: s.key, nth: s.nth });
        }
        due
    }

    /// An evaluation finished (or failed): it applies when it is still the entry's own.
    pub(super) fn binding_finished(&mut self, pane: PaneId, key: &str, nth: u64, r: Result<serde_json::Value, String>) {
        if !accepts(&self.bindings, pane, key, nth) {
            return;
        }
        finished(&mut self.bindings, pane, key);
        self.binding_answer(pane, key, r);
    }

    /// A bound key's new value (`boundVariableDidChange:value:`). Nothing when the profile no
    /// longer binds the key. The write does not drop a palette-owned binding: that is for the
    /// writers that override a color, and a binding must not delete itself.
    pub(super) fn binding_answer(&mut self, pane: PaneId, key: &str, value: Result<serde_json::Value, String>) {
        let Some(p) = self.panes.get(&pane) else { return };
        let guid = p.profile.clone();
        if !self.store.get(&guid).is_some_and(|profile| bindings::bound(profile).iter().any(|(k, _)| k == key)) {
            return;
        }
        let shared = self.store.shared_guid(&guid);
        let restore = self.store.profiles.iter().find(|q| q.guid() == shared).and_then(|q| q.values.get(key));
        match answer(key, &value, restore) {
            Answer::Write(v) => self.write_session_values(pane, &[(key.to_string(), v)]),
            Answer::Log(line) => self.script_log(None, line.trim_end_matches('\n').to_string()),
            Answer::Nothing => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wanted(key: &str, expr: &str) -> Vec<(String, String)> {
        vec![(key.to_string(), expr.to_string())]
    }

    fn paths(e: &str) -> BTreeSet<String> {
        std::iter::once(e.to_string()).collect()
    }

    /// A pane whose profile binds "Foreground Color" to "user.fg": an answer sets the session
    /// value, null restores the profile's own, a removed binding ends the writes.
    #[test]
    fn bindings_apply_to_sessions() {
        let mut store = crate::store::Store::fresh(std::path::Path::new("unused.toml"));
        let guid = store.default_profile().guid();
        let mut shared = store.default_profile().clone();
        shared.set("Foreground Color", crate::profile::color_value(crate::palette::rgb(10, 20, 30)));
        bindings::set(&mut shared, "Foreground Color", Some("user.fg"));
        store.profiles.retain(|p| p.guid() != guid);
        store.profiles.push(shared);
        let copy = store.divorce(&guid);

        let write = |store: &mut crate::store::Store, a: Answer| {
            if let Answer::Write(v) = a {
                store.set_session(&copy, &[("Foreground Color".to_string(), v)]);
            }
        };
        let own = store.profiles.iter().find(|p| p.guid() == guid).unwrap().values.get("Foreground Color").cloned();
        let a = answer("Foreground Color", &Ok(json!("#00ff00")), own.as_ref());
        assert!(matches!(a, Answer::Write(Some(_))));
        write(&mut store, a);
        assert_eq!(crate::settings_ui::rgb_of(store.get(&copy).unwrap().values.get("Foreground Color").unwrap()), [0, 255, 0]);
        // No value restores the profile's own.
        let a = answer("Foreground Color", &Ok(json!(null)), own.as_ref());
        write(&mut store, a);
        assert_eq!(store.get(&copy).unwrap().values.get("Foreground Color"), own.as_ref());
        // A color that is not a string logs iTerm2's line and writes nothing.
        assert!(matches!(answer("Foreground Color", &Ok(json!(5)), own.as_ref()), Answer::Log(l) if l.contains("is a color")));
        assert_eq!(answer("Foreground Color", &Ok(json!("nope")), own.as_ref()), Answer::Nothing);
        assert_eq!(answer("Blur", &Ok(json!(2)), None), Answer::Write(Some(toml::Value::Boolean(true))));
        assert_eq!(answer("Blur", &Err("boom".into()), None), Answer::Log("boom".into()));

        // Removing the binding stops further writes: the entry goes with it, and a late
        // answer is not accepted.
        let mut map = HashMap::new();
        let now = Instant::now();
        let (starts, _) = plan(&mut map, 1, &wanted("Foreground Color", "user.fg"), &paths, &|_| String::new(), now);
        assert_eq!(starts.len(), 1);
        assert!(accepts(&map, 1, "Foreground Color", starts[0].nth));
        plan(&mut map, 1, &[], &paths, &|_| String::new(), now);
        assert!(!accepts(&map, 1, "Foreground Color", starts[0].nth));
    }

    /// 100 dependency changes cost at most two evaluations: one in flight at a time, the rest
    /// folded into one more run after the answer.
    #[test]
    fn binding_flood_is_coalesced() {
        let mut map = HashMap::new();
        let t0 = Instant::now();
        let w = wanted("Blur", "user.x");
        let mut started = 0;
        for i in 0..100u64 {
            let deps = i.to_string();
            let (s, _) = plan(&mut map, 1, &w, &paths, &|_| deps.clone(), t0 + Duration::from_millis(i));
            started += s.len();
        }
        assert_eq!(started, 1, "one runs, the others wait for its answer");
        finished(&mut map, 1, "Blur");
        let late = t0 + Duration::from_millis(100);
        let (s, due) = plan(&mut map, 1, &w, &paths, &|_| "100".into(), late);
        assert!(s.is_empty(), "not before the gap");
        assert_eq!(due, Some(t0 + MIN_GAP));
        let (s, due) = plan(&mut map, 1, &w, &paths, &|_| "100".into(), t0 + MIN_GAP);
        assert_eq!(s.len(), 1);
        assert_eq!(due, None, "nothing wakes while one runs and nothing is dirty");
    }

    #[test]
    fn bindings_end_with_their_pane() {
        let mut map = HashMap::new();
        let now = Instant::now();
        plan(&mut map, 1, &wanted("Blur", "user.x"), &paths, &|_| String::new(), now);
        plan(&mut map, 2, &wanted("Blur", "user.x"), &paths, &|_| String::new(), now);
        map.retain(|(p, _), _| *p != 1);
        assert_eq!(map.len(), 1);
        let src = include_str!("mod.rs");
        assert!(src.contains("self.bindings.retain(|(p, _), _| *p != id);"), "pane_gone drops a pane's bindings");
    }

    /// A palette-owned binding's own answers leave it in place.
    #[test]
    fn palette_binding_survives_its_own_answer() {
        let src = include_str!("bindings_ui.rs");
        let body = &src[src.find("pub(super) fn binding_answer").unwrap()..];
        let body = &body[..body.find("\n    }\n").unwrap()];
        assert!(body.contains("self.write_session_values("));
        assert!(!body.contains("self.set_session_values("));
        let mut p = crate::profile::Profile::default();
        let mut t = toml::Table::new();
        let mut e = toml::Table::new();
        e.insert("expression".into(), "x".into());
        e.insert("owner".into(), "palette".into());
        t.insert("Foreground Color".into(), toml::Value::Table(e));
        p.set(bindings::KEY, toml::Value::Table(t));
        // A concrete write by Settings or an escape code drops it.
        assert!(bindings::drop_palette_in(&mut p, "Foreground Color"));
        assert!(bindings::get(&p).is_empty());
    }
}
