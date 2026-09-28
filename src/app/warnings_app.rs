//! iTerm2's warnings on the app side: the preempt that answers a silenced warning at once and
//! lists it in the suppressed catalog, remembering an answer, and the catalog's persistence
//! (`iTermWarning.m:433-464,571-593`, `iTermSuppressedAlerts.swift:101-161`).

use std::time::{Duration, Instant};

use super::App;
use crate::warnings::{self, Catalog, Preempt, Spec, Store};

/// The catalog is written this long after a record (iTerm2's `flushDelay`,
/// `iTermSuppressedAlerts.swift:69`).
const FLUSH_DELAY: Duration = Duration::from_millis(500);

/// Now in Unix seconds, the clock temporary silences use (ruling m11c).
fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// `preempt:`: the preempt rule, and an automatic answer recorded in `catalog` with its label
/// and the silence's episode (`recordSuppression`). A shown warning is never recorded.
pub(super) fn preempt_and_record(spec: &Spec, store: &dyn Store, catalog: &mut Catalog, now: f64, show_remembered: bool) -> Preempt {
    let preempt = warnings::preempt(spec, store, now, show_remembered);
    if let (Preempt::Answer(index), Some(id)) = (&preempt, spec.id.as_deref()) {
        catalog.record(spec, &spec.labels[*index], warnings::episode(store, id, now), now);
    }
    preempt
}

impl App {
    /// Whether `spec` shows, shows in the remembered mode or is answered at once; an automatic
    /// answer is listed in Suppressed Alerts and the catalog's write is scheduled.
    pub(super) fn warning_preempt(&mut self, spec: &Spec) -> Preempt {
        let preempt = preempt_and_record(spec, &self.globals, &mut self.suppressed, unix_now(), self.show_remembered);
        // As `scheduleFlush`: one write per burst, due after the first record.
        if matches!(preempt, Preempt::Answer(_)) && self.catalog_due.is_none() {
            self.catalog_due = Some(Instant::now() + FLUSH_DELAY);
        }
        preempt
    }

    /// A shown warning answered with button `index`, the checkbox `checked`: the answer is
    /// remembered when the button may be.
    pub(super) fn warning_answered(&mut self, spec: &Spec, index: usize, checked: bool) {
        if checked && spec.id.is_some() && spec.kind != warnings::Kind::Persistent && spec.rememberable(index) {
            warnings::remember(spec, &mut self.globals, index, true, unix_now());
            self.save_globals();
        }
    }

    /// Each pass of the event loop: the catalog is written once its write is due.
    pub(super) fn catalog_tick(&mut self) {
        if self.catalog_due.is_some_and(|t| Instant::now() >= t) {
            self.catalog_due = None;
            self.catalog_write();
        }
    }

    /// At exit: a pending write happens now (`applicationWillTerminate`).
    pub(super) fn catalog_flush(&mut self) {
        if self.catalog_due.take().is_some() {
            self.catalog_write();
        }
    }

    /// `persist`: lapsed entries go, the rest are saved; then the menu item follows.
    fn catalog_write(&mut self) {
        self.suppressed.prune(&self.globals, unix_now());
        self.globals.set(warnings::CATALOG_KEY, self.suppressed.to_value());
        self.save_globals();
        self.suppressed_changed();
    }

    /// The catalog changed. Task 4 retitles the Suppressed Alerts menu item here.
    fn suppressed_changed(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use toml::Value;

    #[derive(Default)]
    struct MapStore(BTreeMap<String, Value>);

    impl Store for MapStore {
        fn get(&self, key: &str) -> Option<Value> {
            self.0.get(key).cloned()
        }
        fn set(&mut self, key: &str, v: Value) {
            self.0.insert(key.to_string(), v);
        }
        fn remove(&mut self, key: &str) {
            self.0.remove(key);
        }
    }

    const NOW: f64 = 1_000_000.0;

    #[test]
    fn preempt_records_follow_iterm2() {
        let spec = Spec {
            id: Some("NoSyncX".into()),
            kind: warnings::Kind::Permanent,
            heading: "Heading".into(),
            text: "Body".into(),
            labels: vec!["OK".into(), "Cancel".into()],
            cancel: Some(1),
            never_remember: vec![],
        };
        let mut s = MapStore::default();
        let mut catalog = Catalog::default();
        // Not silenced: shown, nothing listed.
        assert_eq!(preempt_and_record(&spec, &s, &mut catalog, NOW, false), Preempt::Show);
        assert!(catalog.entries.is_empty());
        s.set("NoSyncX", Value::Boolean(true));
        s.set("NoSyncX_selection", Value::Integer(0));
        // Shown in the remembered mode: nothing listed either.
        assert_eq!(preempt_and_record(&spec, &s, &mut catalog, NOW, true), Preempt::ShowRemembered { saved_label: "OK".into() });
        assert!(catalog.entries.is_empty());
        // Answered automatically: one entry with the answered label.
        assert_eq!(preempt_and_record(&spec, &s, &mut catalog, NOW, false), Preempt::Answer(0));
        assert_eq!(catalog.entries.len(), 1);
        let entry = &catalog.entries["NoSyncX"];
        assert_eq!((entry.label.as_str(), entry.title.as_str(), entry.count, entry.last), ("OK", "Body", 1, NOW));
        assert_eq!(entry.episode.as_deref(), Some("permanent"));
        assert_eq!(preempt_and_record(&spec, &s, &mut catalog, NOW + 1.0, false), Preempt::Answer(0));
        assert_eq!((catalog.entries.len(), catalog.entries["NoSyncX"].count), (1, 2));
    }
}
