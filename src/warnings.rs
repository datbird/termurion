//! iTerm2's warnings (`iTermWarning.m`) and its Suppressed Alerts catalog
//! (`iTermSuppressedAlerts.swift`) without UI: kinds, remembered keys, the preempt rule,
//! remembering an answer, the checkbox wording, button shortcut letters, the catalog of
//! automatically answered warnings and the relative-time texts (`NSDateFormatterExtras.m`).
//! Times are Unix seconds (iTerm2 uses NSDate reference seconds; m11c ruling).

use std::collections::BTreeMap;

use toml::{Table, Value};

/// How long a warning's answer may be remembered (`iTermWarningType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Always shown; never remembered.
    Persistent,
    /// Remembered for ten minutes.
    Temporary,
    /// Remembered for 30 days.
    OneMonth,
    /// Remembered until un-suppressed.
    Permanent,
}

/// `kTemporarySilenceTime` (`iTermWarning.m:13`).
pub const TEMPORARY_SECS: f64 = 600.0;
/// `kOneMonthTime` (`iTermWarning.m:14`).
pub const ONE_MONTH_SECS: f64 = 2_592_000.0;

/// Where the remembered button index lives (`selectionKeyForIdentifier:`).
pub fn selection_key(id: &str) -> String {
    format!("{id}_selection")
}

/// Where a temporary silence's end time lives (`temporarySilenceKeyForIdentifier:`).
pub fn until_key(id: &str) -> String {
    format!("{id}_SilenceUntil")
}

/// The settings a warning's remembered answer is kept in.
pub trait Store {
    fn get(&self, key: &str) -> Option<Value>;
    fn set(&mut self, key: &str, v: Value);
    fn remove(&mut self, key: &str);
}

impl Store for crate::globals::Globals {
    fn get(&self, key: &str) -> Option<Value> {
        crate::globals::Globals::get(self, key)
    }
    fn set(&mut self, key: &str, v: Value) {
        crate::globals::Globals::set(self, key, v)
    }
    fn remove(&mut self, key: &str) {
        self.unset(key)
    }
}

/// `boolForKey:`: a true bool or a non-zero number.
fn flag(s: &dyn Store, key: &str) -> bool {
    match s.get(key) {
        Some(Value::Boolean(b)) => b,
        Some(Value::Integer(i)) => i != 0,
        Some(Value::Float(f)) => f != 0.0,
        _ => false,
    }
}

/// `doubleForKey:`: a number, else 0.
fn double(s: &dyn Store, key: &str) -> f64 {
    match s.get(key) {
        Some(Value::Float(f)) => f,
        Some(Value::Integer(i)) => i as f64,
        _ => 0.0,
    }
}

/// Whether the identifier is silenced: the permanent flag is set or a temporary silence
/// has not ended (`identifierIsSilenced:`, `iTermWarning.m:622-639`).
pub fn is_silenced(s: &dyn Store, id: &str, now: f64) -> bool {
    flag(s, id) || double(s, &until_key(id)) > now
}

/// The remembered button index (`savedSelectionForIdentifier:`, `integerForKey:` reads a
/// missing key as 0). A negative stored index reads as out of range.
pub fn saved_selection(s: &dyn Store, id: &str) -> usize {
    let n = match s.get(&selection_key(id)) {
        Some(Value::Integer(i)) => i,
        Some(Value::Float(f)) => f as i64,
        Some(Value::Boolean(b)) => b as i64,
        _ => 0,
    };
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// The silence episode an automatic answer belongs to: "permanent", or "until:<end>" for
/// a live temporary silence, else None (`silenceEpisodeTokenForIdentifier:`,
/// `iTermWarning.m:641-656`).
pub fn episode(s: &dyn Store, id: &str, now: f64) -> Option<String> {
    if flag(s, id) {
        return Some("permanent".to_string());
    }
    let until = double(s, &until_key(id));
    (until > now).then(|| format!("until:{until}"))
}

/// One warning: its identifier (None: never remembered), kind, heading, text and button
/// labels. `cancel` is the index of the `cancelLabel` button and `never_remember` the
/// `doNotRememberLabels` buttons (`resolveActionRoles`, `iTermWarning.m:466-475`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub id: Option<String>,
    pub kind: Kind,
    pub heading: String,
    pub text: String,
    pub labels: Vec<String>,
    pub cancel: Option<usize>,
    pub never_remember: Vec<usize>,
}

impl Spec {
    /// Whether button `i` may be remembered: it exists and is neither the cancel button
    /// nor a do-not-remember one (`shouldRememberAction:`, `iTermWarning.m:517-524`).
    pub fn rememberable(&self, i: usize) -> bool {
        i < self.labels.len() && self.cancel != Some(i) && !self.never_remember.contains(&i)
    }

    /// The heading shown: its own, else `DEFAULT_HEADING` (`iTermWarning.m:328-331`).
    pub fn shown_heading(&self) -> &str {
        if self.heading.is_empty() { DEFAULT_HEADING } else { &self.heading }
    }

    /// The buttons that are not the cancel button (`numNonCancelActions`).
    pub fn non_cancel(&self) -> usize {
        self.labels.len() - self.cancel.filter(|&c| c < self.labels.len()).map_or(0, |_| 1)
    }
}

/// What to do before showing a warning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Preempt {
    /// Show it.
    Show,
    /// Show it with `remembered_text` and a `FORGET` button: it would have been answered,
    /// but "Always show alerts with remembered selections" is on.
    ShowRemembered { saved_label: String },
    /// Answer it with this button without showing it (the caller records it in the catalog).
    Answer(usize),
}

/// `preempt:` (`iTermWarning.m:433-464`).
pub fn preempt(spec: &Spec, s: &dyn Store, now: f64, show_remembered: bool) -> Preempt {
    let Some(id) = spec.id.as_deref() else { return Preempt::Show };
    if spec.kind == Kind::Persistent || !is_silenced(s, id, now) {
        return Preempt::Show;
    }
    let selection = saved_selection(s, id);
    if !spec.rememberable(selection) {
        return Preempt::Show;
    }
    if show_remembered {
        Preempt::ShowRemembered { saved_label: spec.labels[selection].clone() }
    } else {
        Preempt::Answer(selection)
    }
}

/// Remembers button `index` when the checkbox was `checked` (`handleResult:alert:`,
/// `iTermWarning.m:571-593`).
pub fn remember(spec: &Spec, s: &mut dyn Store, index: usize, checked: bool, now: f64) {
    let Some(id) = spec.id.as_deref() else { return };
    if !checked || !spec.rememberable(index) {
        return;
    }
    match spec.kind {
        Kind::Persistent => return,
        Kind::Temporary => s.set(&until_key(id), Value::Float(now + TEMPORARY_SECS)),
        Kind::OneMonth => s.set(&until_key(id), Value::Float(now + ONE_MONTH_SECS)),
        Kind::Permanent => s.set(id, Value::Boolean(true)),
    }
    s.set(&selection_key(id), Value::Integer(index as i64));
}

/// Forgets a remembered answer (`clearSavedSelectionForIdentifier:`, `iTermWarning.m:271-282`).
pub fn clear_saved(s: &mut dyn Store, id: &str) {
    s.remove(id);
    s.remove(&until_key(id));
    s.remove(&selection_key(id));
}

/// The remember checkbox's text, or None when there is no checkbox (`iTermWarning.m:373-406`).
pub fn checkbox_label(spec: &Spec) -> Option<&'static str> {
    let one = match spec.non_cancel() {
        0 => return None,
        n => n == 1,
    };
    match (spec.kind, one) {
        (Kind::Persistent, _) => None,
        (Kind::Temporary, true) => Some("Suppress this message for ten minutes"),
        (Kind::Temporary, false) => Some("Remember my choice for ten minutes"),
        (Kind::OneMonth, true) => Some("Suppress this message for 30 days"),
        (Kind::OneMonth, false) => Some("Remember my choice for 30 days"),
        (Kind::Permanent, true) => Some("Suppress this message permanently"),
        (Kind::Permanent, false) => Some("Remember my choice"),
    }
}

/// Each button's shortcut: the char index and lowercase letter of its label's first ASCII
/// letter (`assignKeyEquivalents`, `iTermWarning.m:296-326`). The first button (Enter) and
/// "Cancel" (Escape) already have a key and get none. iTerm2 never marks a letter as
/// taken, so two buttons can share one.
pub fn shortcuts(labels: &[String]) -> Vec<Option<(usize, char)>> {
    labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            if i == 0 || label == "Cancel" {
                return None;
            }
            label.chars().enumerate().find(|(_, c)| c.is_ascii_alphabetic()).map(|(at, c)| (at, c.to_ascii_lowercase()))
        })
        .collect()
}

/// The text of a warning shown because "Always show alerts with remembered selections" is
/// on (`iTermWarning.m:333-339`).
pub fn remembered_text(text: &str, saved_label: &str) -> String {
    format!(
        "{text}\n\nThis alert had a saved selection of \u{201c}{saved_label}\u{201d}. It is being shown because \u{201c}Always show alerts with remembered selections\u{201d} is turned on in Tandem > Suppressed Alerts."
    )
}

/// The extra button of a warning shown with its remembered selection (`iTermWarning.m:368-371`).
pub const FORGET: &str = "Permanently Forget Saved Selection";
/// The heading of a warning without one (`iTermWarning.m:328-331`).
pub const DEFAULT_HEADING: &str = "Warning";

/// Where the catalog is stored (`iTermSuppressedAlerts.swift:32`).
pub const CATALOG_KEY: &str = "NoSyncSuppressedAlertsCatalog";

/// One automatically answered warning (`iTermSuppressedAlert`).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    /// The warning's text.
    pub title: String,
    pub heading: Option<String>,
    /// The button answered on the person's behalf.
    pub label: String,
    /// When it was last answered, Unix seconds.
    pub last: f64,
    pub count: i64,
    /// The silence episode the count belongs to (`episode`).
    pub episode: Option<String>,
}

/// The warnings answered automatically, by identifier (`iTermSuppressedAlerts`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    pub entries: BTreeMap<String, Entry>,
}

impl Catalog {
    /// Reads the stored table of tables; anything else is empty (`SA` init).
    pub fn from_value(v: Option<&Value>) -> Catalog {
        let Some(table) = v.and_then(Value::as_table) else { return Catalog::default() };
        let entries = table
            .iter()
            .filter_map(|(id, v)| {
                let t = v.as_table()?;
                let text = |k: &str| t.get(k).and_then(Value::as_str).map(str::to_string);
                let last = match t.get("lastSuppressed") {
                    Some(Value::Float(f)) => *f,
                    Some(Value::Integer(i)) => *i as f64,
                    _ => 0.0,
                };
                let entry = Entry {
                    id: id.clone(),
                    title: text("title").unwrap_or_default(),
                    heading: text("heading"),
                    label: text("selectionLabel").unwrap_or_default(),
                    last,
                    count: t.get("count").and_then(Value::as_integer).unwrap_or(0),
                    episode: text("episode"),
                };
                Some((id.clone(), entry))
            })
            .collect();
        Catalog { entries }
    }

    /// The stored form: identifier to `title`, `heading` (when there is one),
    /// `selectionLabel`, `lastSuppressed`, `count`, `episode` (when there is one).
    pub fn to_value(&self) -> Value {
        let mut out = Table::new();
        for (id, e) in &self.entries {
            let mut t = Table::new();
            t.insert("title".into(), Value::String(e.title.clone()));
            if let Some(h) = &e.heading {
                t.insert("heading".into(), Value::String(h.clone()));
            }
            t.insert("selectionLabel".into(), Value::String(e.label.clone()));
            t.insert("lastSuppressed".into(), Value::Float(e.last));
            t.insert("count".into(), Value::Integer(e.count));
            if let Some(ep) = &e.episode {
                t.insert("episode".into(), Value::String(ep.clone()));
            }
            out.insert(id.clone(), Value::Table(t));
        }
        Value::Table(out)
    }

    /// Records an automatic answer with `label`; the count continues only within the same
    /// silence `episode` (`recordSuppression`, `SA:130-161`). An empty heading is iTerm2's
    /// nil heading.
    pub fn record(&mut self, spec: &Spec, label: &str, episode: Option<String>, now: f64) {
        let Some(id) = spec.id.as_deref() else { return };
        let previous = match (self.entries.get(id), &episode) {
            (Some(e), Some(ep)) if e.episode.as_ref() == Some(ep) => e.count,
            _ => 0,
        };
        let entry = Entry {
            id: id.to_string(),
            title: spec.text.clone(),
            heading: (!spec.heading.is_empty()).then(|| spec.heading.clone()),
            label: label.to_string(),
            last: now,
            count: previous + 1,
            episode,
        };
        self.entries.insert(id.to_string(), entry);
    }

    /// Drops entries whose silence has lapsed (`prune`, `SA:101-107`).
    pub fn prune(&mut self, s: &dyn Store, now: f64) {
        self.entries.retain(|id, _| is_silenced(s, id, now));
    }

    /// The silenced entries, newest first (`currentlySuppressedAlerts`, `SA:166-172`).
    pub fn current(&mut self, s: &dyn Store, now: f64) -> Vec<Entry> {
        self.prune(s, now);
        let mut out: Vec<Entry> = self.entries.values().cloned().collect();
        out.sort_by(|a, b| b.last.total_cmp(&a.last));
        out
    }

    /// Forgets one warning's answer so it is asked again (`unsuppressIdentifier:`, `SA:189-199`).
    pub fn unsuppress(&mut self, s: &mut dyn Store, id: &str) {
        clear_saved(s, id);
        self.entries.remove(id);
    }

    /// Forgets every listed warning's answer (`unsuppressAll`, `SA:201-211`).
    pub fn unsuppress_all(&mut self, s: &mut dyn Store) {
        for id in self.entries.keys() {
            clear_saved(s, id);
        }
        self.entries.clear();
    }
}

/// C `round` (half away from zero) as an int, as `NSDateFormatterExtras.m` computes.
fn rounded(x: f64) -> i64 {
    x.round() as i64
}

/// How long ago, `age` seconds, in words (`dateDifferenceStringFromDate:options:`,
/// `NSDateFormatterExtras.m:143-220`, English); `lowercase` for mid-sentence use.
pub fn date_difference(age: f64, lowercase: bool) -> String {
    let pick = |lower: &str, upper: &str| if lowercase { lower.to_string() } else { upper.to_string() };
    if age < 60.0 {
        pick("moments ago", "Moments ago")
    } else if age < 3600.0 {
        match rounded(age / 60.0) {
            1 => "1 minute ago".to_string(),
            n => format!("{n} minutes ago"),
        }
    } else if age < 86400.0 {
        match rounded(age / 60.0 / 60.0) {
            1 => "1 hour ago".to_string(),
            n => format!("{n} hours ago"),
        }
    } else if age < 604800.0 {
        match rounded(age / 60.0 / 60.0 / 24.0) {
            1 => pick("yesterday", "Yesterday"),
            7 => pick("one week ago", "One week ago"),
            n => format!("{n} days ago"),
        }
    } else {
        match rounded(age / 60.0 / 60.0 / 24.0 / 7.0) {
            1 => pick("last week", "Last week"),
            n => format!("{n} weeks ago"),
        }
    }
}

/// A bare duration, `age` seconds (`compactDateDifferenceStringFromTimeDelta:`,
/// `NSDateFormatterExtras.m:230-290`, English).
pub fn compact_date_difference(age: f64) -> String {
    if age < 60.0 {
        "< 1 min".to_string()
    } else if age < 3600.0 {
        match rounded(age / 60.0) {
            1 => "1 min".to_string(),
            n => format!("{n} min"),
        }
    } else if age < 86400.0 {
        match rounded(age / 60.0 / 60.0) {
            1 => "1 hour".to_string(),
            n => format!("{n} hrs"),
        }
    } else if age < 604800.0 {
        match rounded(age / 60.0 / 60.0 / 24.0) {
            1 => "1 day".to_string(),
            7 => "1 week".to_string(),
            n => format!("{n} days"),
        }
    } else {
        match rounded(age / 60.0 / 60.0 / 24.0 / 7.0) {
            1 => "1 week".to_string(),
            n => format!("{n} wks"),
        }
    }
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

    fn spec(kind: Kind, labels: &[&str], cancel: Option<usize>) -> Spec {
        Spec {
            id: Some("NoSyncX".into()),
            kind,
            heading: "Heading".into(),
            text: "Body".into(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            cancel,
            never_remember: vec![],
        }
    }

    fn permanent(s: &mut MapStore, selection: i64) {
        s.set("NoSyncX", Value::Boolean(true));
        s.set("NoSyncX_selection", Value::Integer(selection));
    }

    #[test]
    fn keys_follow_iterm2() {
        assert_eq!(selection_key("NoSyncX"), "NoSyncX_selection");
        assert_eq!(until_key("NoSyncX"), "NoSyncX_SilenceUntil");
        let mut s = MapStore::default();
        assert!(!is_silenced(&s, "NoSyncX", NOW));
        assert_eq!(episode(&s, "NoSyncX", NOW), None);
        s.set("NoSyncX", Value::Boolean(true));
        assert!(is_silenced(&s, "NoSyncX", NOW));
        assert_eq!(episode(&s, "NoSyncX", NOW).as_deref(), Some("permanent"));
        let mut s = MapStore::default();
        s.set("NoSyncX_SilenceUntil", Value::Float(NOW + 1.0));
        assert!(is_silenced(&s, "NoSyncX", NOW));
        assert_eq!(episode(&s, "NoSyncX", NOW), Some(format!("until:{}", NOW + 1.0)));
        assert_eq!(episode(&s, "NoSyncX", NOW).as_deref(), Some("until:1000001"));
        s.set("NoSyncX_SilenceUntil", Value::Float(NOW - 1.0));
        assert!(!is_silenced(&s, "NoSyncX", NOW));
        assert_eq!(episode(&s, "NoSyncX", NOW), None);
        // A false flag is not a silence.
        s.set("NoSyncX", Value::Boolean(false));
        assert!(!is_silenced(&s, "NoSyncX", NOW));
        // integerForKey: a missing selection reads 0.
        assert_eq!(saved_selection(&MapStore::default(), "NoSyncX"), 0);
    }

    #[test]
    fn preempt_follows_iterm2() {
        let ok_cancel = spec(Kind::Permanent, &["OK", "Cancel"], Some(1));
        let mut s = MapStore::default();
        assert_eq!(preempt(&ok_cancel, &s, NOW, false), Preempt::Show);
        permanent(&mut s, 0);
        assert_eq!(preempt(&ok_cancel, &s, NOW, false), Preempt::Answer(0));
        assert_eq!(preempt(&ok_cancel, &s, NOW, true), Preempt::ShowRemembered { saved_label: "OK".into() });
        // Cancel is never remembered, so a saved Cancel asks.
        permanent(&mut s, 1);
        assert_eq!(preempt(&ok_cancel, &s, NOW, false), Preempt::Show);
        // A "do not remember" label asks.
        permanent(&mut s, 0);
        let mut never = ok_cancel.clone();
        never.never_remember = vec![0];
        assert_eq!(preempt(&never, &s, NOW, false), Preempt::Show);
        // A persistent warning is never silenced.
        let persistent = spec(Kind::Persistent, &["OK", "Cancel"], Some(1));
        assert_eq!(preempt(&persistent, &s, NOW, false), Preempt::Show);
        // No identifier, nothing remembered.
        let mut anonymous = ok_cancel.clone();
        anonymous.id = None;
        assert_eq!(preempt(&anonymous, &s, NOW, false), Preempt::Show);
        // A selection past the buttons.
        permanent(&mut s, 2);
        assert_eq!(preempt(&ok_cancel, &s, NOW, false), Preempt::Show);
        permanent(&mut s, -1);
        assert_eq!(preempt(&ok_cancel, &s, NOW, false), Preempt::Show);
        // A lapsed temporary silence asks again; a live one answers.
        let temporary = spec(Kind::Temporary, &["OK", "Cancel"], Some(1));
        let mut s = MapStore::default();
        s.set("NoSyncX_selection", Value::Integer(0));
        s.set("NoSyncX_SilenceUntil", Value::Float(NOW - 1.0));
        assert_eq!(preempt(&temporary, &s, NOW, false), Preempt::Show);
        s.set("NoSyncX_SilenceUntil", Value::Float(NOW + 1.0));
        assert_eq!(preempt(&temporary, &s, NOW, false), Preempt::Answer(0));
    }

    #[test]
    fn remember_follows_iterm2() {
        let ok_cancel = spec(Kind::Permanent, &["OK", "Cancel"], Some(1));
        let mut s = MapStore::default();
        remember(&ok_cancel, &mut s, 0, true, NOW);
        assert_eq!(s.get("NoSyncX"), Some(Value::Boolean(true)));
        assert_eq!(s.get("NoSyncX_selection"), Some(Value::Integer(0)));
        assert_eq!(s.get("NoSyncX_SilenceUntil"), None);

        let mut s = MapStore::default();
        remember(&ok_cancel, &mut s, 0, false, NOW);
        assert!(s.0.is_empty(), "unchecked writes nothing");
        remember(&ok_cancel, &mut s, 1, true, NOW);
        assert!(s.0.is_empty(), "Cancel writes nothing");
        let mut never = ok_cancel.clone();
        never.never_remember = vec![0];
        remember(&never, &mut s, 0, true, NOW);
        assert!(s.0.is_empty(), "a do-not-remember label writes nothing");

        let temporary = spec(Kind::Temporary, &["OK", "Cancel"], Some(1));
        remember(&temporary, &mut s, 0, true, NOW);
        assert_eq!(s.get("NoSyncX_SilenceUntil"), Some(Value::Float(NOW + 600.0)));
        assert_eq!(s.get("NoSyncX_selection"), Some(Value::Integer(0)));
        assert_eq!(s.get("NoSyncX"), None);

        let mut s = MapStore::default();
        let month = spec(Kind::OneMonth, &["Yes", "No"], None);
        remember(&month, &mut s, 1, true, NOW);
        assert_eq!(s.get("NoSyncX_SilenceUntil"), Some(Value::Float(NOW + 2_592_000.0)));
        assert_eq!(s.get("NoSyncX_selection"), Some(Value::Integer(1)));

        let mut s = MapStore::default();
        let persistent = spec(Kind::Persistent, &["OK", "Cancel"], Some(1));
        remember(&persistent, &mut s, 0, true, NOW);
        assert!(s.0.is_empty(), "a persistent warning never remembers");

        let mut s = MapStore::default();
        s.set("NoSyncX", Value::Boolean(true));
        s.set("NoSyncX_selection", Value::Integer(0));
        s.set("NoSyncX_SilenceUntil", Value::Float(NOW));
        s.set("Other", Value::Boolean(true));
        clear_saved(&mut s, "NoSyncX");
        assert_eq!(s.0.keys().collect::<Vec<_>>(), ["Other"]);
        assert_eq!((TEMPORARY_SECS, ONE_MONTH_SECS), (600.0, 2_592_000.0));
    }

    #[test]
    fn checkbox_label_follows_iterm2() {
        let one = |kind| spec(kind, &["OK", "Cancel"], Some(1));
        let two = |kind| spec(kind, &["Yes", "No", "Cancel"], Some(2));
        assert_eq!(checkbox_label(&one(Kind::Permanent)), Some("Suppress this message permanently"));
        assert_eq!(checkbox_label(&two(Kind::Permanent)), Some("Remember my choice"));
        assert_eq!(checkbox_label(&one(Kind::Temporary)), Some("Suppress this message for ten minutes"));
        assert_eq!(checkbox_label(&two(Kind::Temporary)), Some("Remember my choice for ten minutes"));
        assert_eq!(checkbox_label(&one(Kind::OneMonth)), Some("Suppress this message for 30 days"));
        assert_eq!(checkbox_label(&two(Kind::OneMonth)), Some("Remember my choice for 30 days"));
        assert_eq!(checkbox_label(&one(Kind::Persistent)), None);
        assert_eq!(checkbox_label(&two(Kind::Persistent)), None);
        // Without a cancel label (the object API, restart), Cancel counts.
        let restart = spec(Kind::Permanent, &["OK", "Cancel"], None);
        assert_eq!(restart.non_cancel(), 2);
        assert_eq!(checkbox_label(&restart), Some("Remember my choice"));
        assert!(restart.rememberable(1));
        let ok_cancel = one(Kind::Permanent);
        assert_eq!(ok_cancel.non_cancel(), 1);
        assert!(ok_cancel.rememberable(0) && !ok_cancel.rememberable(1));
    }

    #[test]
    fn shortcuts_follow_iterm2() {
        let of = |labels: &[&str]| shortcuts(&labels.iter().map(|l| l.to_string()).collect::<Vec<_>>());
        assert_eq!(of(&["OK", "Cancel"]), [None, None]);
        assert_eq!(of(&["Replace", "Cancel"]), [None, None]);
        assert_eq!(of(&["Clear All", "Clear Current Session Only", "Cancel"]), [None, Some((0, 'c')), None]);
        assert_eq!(of(&["OK", "3 items"]), [None, Some((2, 'i'))]);
        assert_eq!(of(&["OK", "\u{e9}t\u{e9}"]), [None, Some((1, 't'))]);
        assert_eq!(of(&["OK", "123"]), [None, None]);
        assert_eq!(of(&[]), Vec::<Option<(usize, char)>>::new());
    }

    #[test]
    fn texts_follow_iterm2() {
        assert_eq!(
            remembered_text("Body", "OK"),
            "Body\n\nThis alert had a saved selection of \u{201c}OK\u{201d}. It is being shown because \u{201c}Always show alerts with remembered selections\u{201d} is turned on in Tandem > Suppressed Alerts."
        );
        assert_eq!(FORGET, "Permanently Forget Saved Selection");
        assert_eq!(DEFAULT_HEADING, "Warning");
    }

    #[test]
    fn catalog_follows_iterm2() {
        let x = spec(Kind::Permanent, &["OK", "Cancel"], Some(1));
        let mut s = MapStore::default();
        let mut c = Catalog::default();
        permanent(&mut s, 0);
        c.record(&x, "OK", episode(&s, "NoSyncX", NOW), NOW);
        c.record(&x, "OK", episode(&s, "NoSyncX", NOW + 1.0), NOW + 1.0);
        let e = &c.entries["NoSyncX"];
        assert_eq!((e.count, e.last, e.label.as_str(), e.title.as_str()), (2, NOW + 1.0, "OK", "Body"));
        assert_eq!((e.heading.as_deref(), e.episode.as_deref()), (Some("Heading"), Some("permanent")));

        // A new silence episode restarts the count.
        clear_saved(&mut s, "NoSyncX");
        let t = spec(Kind::Temporary, &["OK", "Cancel"], Some(1));
        remember(&t, &mut s, 0, true, NOW + 2.0);
        c.record(&t, "OK", episode(&s, "NoSyncX", NOW + 3.0), NOW + 3.0);
        assert_eq!(c.entries["NoSyncX"].count, 1);
        assert_eq!(c.entries["NoSyncX"].episode.as_deref(), Some("until:1000602"));

        // current: lapsed ones pruned, newest first.
        let mut y = x.clone();
        y.id = Some("NoSyncY".into());
        y.heading = String::new();
        s.set("NoSyncY", Value::Boolean(true));
        c.record(&y, "OK", episode(&s, "NoSyncY", NOW + 4.0), NOW + 4.0);
        let mut z = x.clone();
        z.id = Some("NoSyncZ".into());
        s.set("NoSyncZ", Value::Boolean(true));
        c.record(&z, "OK", episode(&s, "NoSyncZ", NOW + 5.0), NOW + 5.0);
        let ids = |c: &mut Catalog, now: f64| c.current(&s, now).iter().map(|e| e.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&mut c, NOW + 6.0), ["NoSyncZ", "NoSyncY", "NoSyncX"]);
        assert_eq!(ids(&mut c, NOW + 700.0), ["NoSyncZ", "NoSyncY"], "the ten minutes lapsed");
        assert!(!c.entries.contains_key("NoSyncX"), "pruned from the catalog");
        assert_eq!(c.entries["NoSyncY"].heading, None, "an empty heading is none");

        // Round trip through the stored table.
        let v = c.to_value();
        let y_table = v["NoSyncY"].as_table().unwrap();
        let mut keys: Vec<&str> = y_table.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["count", "episode", "lastSuppressed", "selectionLabel", "title"]);
        assert_eq!(v["NoSyncZ"]["heading"].as_str(), Some("Heading"));
        assert_eq!(v["NoSyncZ"]["lastSuppressed"].as_float(), Some(NOW + 5.0));
        assert_eq!(v["NoSyncZ"]["count"].as_integer(), Some(1));
        assert_eq!(Catalog::from_value(Some(&v)), c);
        assert_eq!(Catalog::from_value(None), Catalog::default());
        assert_eq!(Catalog::from_value(Some(&Value::Integer(3))), Catalog::default());

        // Un-suppress clears the three keys and the entry.
        s.set("NoSyncZ_selection", Value::Integer(0));
        s.set("NoSyncZ_SilenceUntil", Value::Float(NOW));
        c.unsuppress(&mut s, "NoSyncZ");
        assert!(!c.entries.contains_key("NoSyncZ"));
        assert!(["NoSyncZ", "NoSyncZ_selection", "NoSyncZ_SilenceUntil"].iter().all(|k| s.get(k).is_none()));
        assert!(s.get("NoSyncY").is_some());
        c.unsuppress_all(&mut s);
        assert!(c.entries.is_empty());
        assert!(s.get("NoSyncY").is_none() && s.get("NoSyncY_selection").is_none());
        assert_eq!(CATALOG_KEY, "NoSyncSuppressedAlertsCatalog");
    }

    #[test]
    fn relative_times_follow_iterm2() {
        assert_eq!(date_difference(30.0, false), "Moments ago");
        assert_eq!(date_difference(30.0, true), "moments ago");
        assert_eq!(date_difference(90.0, false), "2 minutes ago");
        assert_eq!(date_difference(90.0, true), "2 minutes ago");
        assert_eq!(date_difference(60.0, true), "1 minute ago");
        assert_eq!(date_difference(7200.0, false), "2 hours ago");
        assert_eq!(date_difference(3600.0, false), "1 hour ago");
        assert_eq!(date_difference(86400.0, true), "yesterday");
        assert_eq!(date_difference(86400.0, false), "Yesterday");
        assert_eq!(date_difference(604799.0, true), "one week ago");
        assert_eq!(date_difference(604799.0, false), "One week ago");
        assert_eq!(date_difference(3.0 * 86400.0, true), "3 days ago");
        assert_eq!(date_difference(604800.0, true), "last week");
        assert_eq!(date_difference(604800.0, false), "Last week");
        assert_eq!(date_difference(3.0 * 604800.0, false), "3 weeks ago");
        assert_eq!(compact_date_difference(30.0), "< 1 min");
        assert_eq!(compact_date_difference(60.0), "1 min");
        assert_eq!(compact_date_difference(600.0), "10 min");
        assert_eq!(compact_date_difference(3600.0), "1 hour");
        assert_eq!(compact_date_difference(7200.0), "2 hrs");
        assert_eq!(compact_date_difference(86400.0), "1 day");
        assert_eq!(compact_date_difference(2.0 * 86400.0), "2 days");
        assert_eq!(compact_date_difference(604799.0), "1 week");
        assert_eq!(compact_date_difference(604800.0), "1 week");
        assert_eq!(compact_date_difference(2.0 * 604800.0), "2 wks");
    }
}
