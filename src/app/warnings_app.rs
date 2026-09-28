//! iTerm2's warnings on the app side: the preempt that answers a silenced warning at once and
//! lists it in the suppressed catalog, remembering an answer, and the catalog's persistence
//! (`iTermWarning.m:433-464,571-593`, `iTermSuppressedAlerts.swift:101-161`).

use std::time::{Duration, Instant};

use winit::event_loop::ActiveEventLoop;

use super::App;
use crate::prompt_ui::ConfirmWindow;
use crate::warnings::{self, Catalog, Preempt, Spec, Store};

/// The catalog is written this long after a record (iTerm2's `flushDelay`,
/// `iTermSuppressedAlerts.swift:69`).
const FLUSH_DELAY: Duration = Duration::from_millis(500);

/// Now in Unix seconds, the clock temporary silences use (ruling m11c).
pub(super) fn unix_now() -> f64 {
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

/// `App::open_warning_window` with the GPU handles and "AlertsIndicateShortcuts" (`underline`)
/// given, for a caller that holds part of the app borrowed.
pub(super) fn open_warning(event_loop: &ActiveEventLoop, gpu: (&wgpu::Instance, &wgpu::Adapter, &wgpu::Device, &wgpu::Queue), spec: &Spec, remembered: Option<&str>, underline: bool) -> Result<ConfirmWindow, String> {
    let (instance, adapter, device, queue) = gpu;
    let labels: Vec<&str> = spec.labels.iter().map(String::as_str).collect();
    let heading = if spec.heading.is_empty() { warnings::DEFAULT_HEADING } else { spec.heading.as_str() };
    let text = remembered.map_or_else(|| spec.text.clone(), |saved| warnings::remembered_text(&spec.text, saved));
    ConfirmWindow::actions(event_loop, instance, adapter, device, queue, heading, &text, &labels, warnings::checkbox_label(spec), &warnings::shortcuts(&spec.labels), underline, remembered.is_some())
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

    /// Opens `spec` in a window of its own, for the sites that keep theirs (the API forms, the
    /// function call error, Send Signal, the password manager notice): the buttons, iTerm2's
    /// checkbox label and shortcut letters, and with `remembered` (the saved label) the
    /// explanation and "Permanently Forget Saved Selection" after the buttons.
    pub(super) fn open_warning_window(&self, event_loop: &ActiveEventLoop, spec: &Spec, remembered: Option<&str>) -> Result<ConfirmWindow, String> {
        let g = self.gpu.as_ref().ok_or_else(|| "no window".to_string())?;
        open_warning(event_loop, (&g.instance, &g.adapter, &g.device, &g.queue), spec, remembered, self.globals.bool("AlertsIndicateShortcuts"))
    }

    /// A warning shown in its own window answered with button `index`: "Permanently Forget
    /// Saved Selection" (only in the remembered mode) forgets the saved answer and gives true,
    /// the caller showing the warning again without the remembered mode
    /// (`iTermWarning.m:526-541`); any other button is remembered per the checkbox.
    pub(super) fn warning_window_answered(&mut self, spec: &Spec, remembered: bool, index: usize, checked: bool) -> bool {
        if remembered && index == spec.labels.len() {
            if let Some(id) = spec.id.as_deref() {
                warnings::clear_saved(&mut self.globals, id);
                self.save_globals();
            }
            return true;
        }
        self.warning_answered(spec, index, checked);
        false
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

    /// Each site's identifier and kind (a later edit that drops a site's identifier fails).
    /// The hotkey profile warning (`NoSyncSuppressAddAnotherHotkeyProfileWarning`) gets its spec
    /// with Task 6's `sheet_warning`; the Capture Output keys are announcements, not warnings,
    /// so they have none.
    #[test]
    fn sites_use_their_identifiers() {
        use crate::paste::Warning;
        use crate::warnings::Kind::{Permanent, Persistent, Temporary};
        let paste = |w: Warning| w.spec(w.heading(), &w.text());
        let failed = super::super::script_ui::Alert::Failed { heading: "h".into(), invocation: "f()".into(), text: "t".into(), reveal: None };
        let archive = super::super::scripts_alerts::ask("h", "t", &["OK", "Cancel", "Move to Trash"], Some(crate::scripts::ARCHIVE_ID), Temporary, super::super::scripts_alerts::Then::Archive("a.zip".into()));
        let sites: Vec<(Spec, &str, warnings::Kind)> = vec![
            (paste(Warning::Tabs), "AboutToPasteTabsWithCancel", Permanent),
            (paste(Warning::Lines { n: 2, at_prompt: false }), "NoSyncDoNotWarnBeforeMultilinePaste", Permanent),
            (paste(Warning::OneLine { at_prompt: true }), "NoSyncDoNotWarnBeforePastingOneLineEndingInNewlineAtShellPrompt", Permanent),
            (paste(Warning::Size(9)), "NoSyncPasteOverCharacterLimitWarning", Persistent),
            (super::super::tmux_tabs_ui::tmux_close_tab_ask(1, 2, 3).spec, "ClosingTmuxTabKillsTmuxWindows", Permanent),
            (super::super::tmux_tabs_ui::tmux_close_window_ask(1, 0, true, 2).unwrap().spec, "ClosingTmuxWindowKillsTmuxWindows", Permanent),
            (failed.spec().unwrap(), "NoSyncFunctionCallError", Temporary),
            (super::super::toolbelt_more_ui::signal_spec(2, false), "NoSyncSuppressSendSignal", Permanent),
            (super::super::broadcast_ui::broadcast_input_ask("t", "enableSendInputToAllTabs:").spec, "NoSyncSuppressBroadcastInputWarning", Permanent),
            (archive.spec, "NoSyncInstallScriptArchive", Temporary),
            (super::super::pwm_app::adapter_warning_spec("op", "t"), "NoSyncAdapterListWarning_op", Permanent),
            (super::super::api_ui::reset_spec(), "NoSyncAppSupportMoved", Permanent),
        ];
        for (spec, id, kind) in &sites {
            assert_eq!((spec.id.as_deref(), spec.kind), (Some(*id), *kind), "{id}");
        }
        assert_eq!((crate::script_launch::MALFORMED_ID, crate::script_launch::REBUILD_ID), ("NoSyncScriptMalformed", "NoSyncRebuildMissingUvEnv"));
        // Only OK of a found archive may be remembered; a malformed script's Reveal never is.
        assert_eq!(archive_never(), (vec![1, 2], vec![1]));
        // A remembered Reveal Preference is never replayed.
        let reset = super::super::api_ui::reset_spec();
        assert_eq!(reset.never_remember, [1]);
        let mut s = MapStore::default();
        s.set("NoSyncAppSupportMoved", Value::Boolean(true));
        s.set("NoSyncAppSupportMoved_selection", Value::Integer(1));
        assert_eq!(warnings::preempt(&reset, &s, NOW, false), Preempt::Show);
        s.set("NoSyncAppSupportMoved_selection", Value::Integer(0));
        assert_eq!(warnings::preempt(&reset, &s, NOW, false), Preempt::Answer(0));
    }

    /// `persistent_is_never_silenced`: a stored flag still shows a persistent warning.
    #[test]
    fn persistent_is_never_silenced() {
        let rebuild = Spec {
            id: Some(crate::script_launch::REBUILD_ID.into()),
            kind: warnings::Kind::Persistent,
            heading: "Rebuild Python Environment?".into(),
            text: "t".into(),
            labels: vec!["Rebuild".into(), "Cancel".into()],
            cancel: Some(1),
            never_remember: vec![],
        };
        let mut s = MapStore::default();
        s.set(crate::script_launch::REBUILD_ID, Value::Boolean(true));
        assert_eq!(warnings::preempt(&rebuild, &s, NOW, false), Preempt::Show);
        let size = crate::paste::Warning::Size(9);
        let size = size.spec(size.heading(), &size.text());
        s.set("NoSyncPasteOverCharacterLimitWarning", Value::Boolean(true));
        assert_eq!(warnings::preempt(&size, &s, NOW, false), Preempt::Show);
    }

    fn archive_never() -> (Vec<usize>, Vec<usize>) {
        use super::super::scripts_alerts::Then;
        (Then::Archive("a.zip".into()).never_remember(), Then::Malformed("m".into()).never_remember())
    }

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
