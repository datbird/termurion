//! Session Status, pure rules: priority patterns, row sort, snooze, shortcut labels, and the
//! display and notification text (iTerm2's `ToolStatus.swift` (`TS`) and
//! `StatusPrioritySettings.swift` (`SPS`); the wire status text and its default patterns live
//! in `tabstatus`). The app wires rows, storage and the settings popover in
//! `src/app/toolbelt_more_ui.rs`.

use std::collections::BTreeMap;

use toml::Value;

use crate::layout::PaneId;

/// One priority pattern: a lowercase substring to match against status text, and whether a
/// match posts a notification (`SPS`, `StatusPriorityEntry`).
#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub pattern: String,
    pub notify: bool,
}

/// The new-format storage key: an array of `{pattern, notify}` tables (`SPS:14`). iTerm2
/// stores this as JSON data; Tandem's globals are TOML, so it is a TOML array of tables
/// instead (ruling: no other storage format exists in Tandem's globals).
pub const PATTERNS_KEY: &str = "StatusPriorityEntries";
/// The legacy key: a plain array of pattern strings, migrated once (`SPS:13`).
pub const LEGACY_KEY: &str = "StatusPriorities";
/// Collapses each workgroup to one row. Tandem has no workgroups, so this is stored but
/// changes nothing (ruling).
pub const MERGE_KEY: &str = "StatusMergeWorkgroups";
/// Whether a tab's subtitle shows its status text (already a schema key; `requests_ui.rs`
/// reads it today with the fixed `PRIORITIES` list, which Task 6 changes to the user's
/// patterns per the correction below).
pub const SUBTITLE_KEY: &str = "ShowSessionStatusInTabSubtitle";
/// When the Session Status tool was last shown, for the toolbelt's "recently used" ordering
/// (`ToolStatus.swift:22`).
pub const LAST_USE_KEY: &str = "NoSyncStatusToolLastUseDate";

/// The stored patterns: the new format when present, else the legacy array migrated (notify
/// off), else iTerm2's defaults (`tabstatus::PRIORITIES`, notify off) (`SPS`'s `init`, trying
/// the new format first).
pub fn load_patterns(entries: Option<&Value>, legacy: Option<&Value>) -> Vec<Pattern> {
    if let Some(arr) = entries.and_then(Value::as_array) {
        return arr
            .iter()
            .filter_map(|item| {
                let t = item.as_table()?;
                let pattern = t.get("pattern")?.as_str()?.to_string();
                let notify = t.get("notify").and_then(Value::as_bool).unwrap_or(false);
                Some(Pattern { pattern, notify })
            })
            .collect();
    }
    if let Some(arr) = legacy.and_then(Value::as_array) {
        return arr.iter().filter_map(|item| item.as_str().map(|s| Pattern { pattern: s.to_string(), notify: false })).collect();
    }
    crate::tabstatus::PRIORITIES.iter().map(|p| Pattern { pattern: p.to_string(), notify: false }).collect()
}

/// Patterns as the new-format value: an array of `{pattern, notify}` tables.
pub fn patterns_value(patterns: &[Pattern]) -> Value {
    Value::Array(
        patterns
            .iter()
            .map(|p| {
                let mut t = toml::Table::new();
                t.insert("pattern".to_string(), Value::String(p.pattern.clone()));
                t.insert("notify".to_string(), Value::Boolean(p.notify));
                Value::Table(t)
            })
            .collect(),
    )
}

/// A status text's priority: the index of the first pattern it contains, case-insensitively;
/// `patterns.len()` when it matches none; `patterns.len() + 1` when there is no text at all,
/// or no pattern at all (`SPS:86-97`, `unmatchedPriority`: an empty list ranks every status
/// the same).
pub fn priority(text: Option<&str>, patterns: &[Pattern]) -> usize {
    match text {
        Some(t) if !patterns.is_empty() => {
            let low = t.to_lowercase();
            patterns.iter().position(|p| low.contains(&p.pattern.to_lowercase())).unwrap_or(patterns.len())
        }
        _ => patterns.len() + 1,
    }
}

/// Whether a status text should notify: the first matching pattern's flag, or false with no
/// text or no match.
pub fn should_notify(text: Option<&str>, patterns: &[Pattern]) -> bool {
    let Some(t) = text else { return false };
    let low = t.to_lowercase();
    patterns.iter().find(|p| low.contains(&p.pattern.to_lowercase())).is_some_and(|p| p.notify)
}

/// A wire status value's display text: the three canonical values map to their localized,
/// capitalized form; anything else (free text, or a different case) passes through unchanged
/// (`SetTabStatusTrigger.localizedStatusForDisplay`, exact wire values only).
pub fn display_status(s: &str) -> String {
    match s {
        "work" => "Working".to_string(),
        "wait" => "Waiting".to_string(),
        "idle" => "Idle".to_string(),
        other => other.to_string(),
    }
}

/// One row of the Session Status tool: a pane's status plus what the tool needs to draw and
/// sort it. `changed` is a monotonic tick (iTerm2's `NSDate.it_timeSinceBoot()`), not a wall
/// clock.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusRow {
    pub pane: u64,
    pub snoozed: bool,
    pub priority: usize,
    pub changed: u64,
    pub name: String,
    pub status: Option<String>,
    pub detail: Option<String>,
    pub shortcut: Option<String>,
}

/// Sorts rows the way `ToolStatus.Status` orders (`TS:38-52`): un-snoozed before snoozed
/// regardless of priority or recency, then by priority, then older `changed` first, then by
/// pane (iTerm2 breaks a full tie on session id; a pane id is Tandem's stand-in).
pub fn sort_rows(rows: &mut [StatusRow]) {
    rows.sort_by(|a, b| a.snoozed.cmp(&b.snoozed).then(a.priority.cmp(&b.priority)).then(a.changed.cmp(&b.changed)).then(a.pane.cmp(&b.pane)));
}

/// The row index to draw a separator above: the first snoozed row after an un-snoozed one, or
/// `None` when there is no such row (including when every row is snoozed: the separator never
/// sits at row 0). Assumes `rows` is already sorted by `sort_rows`.
pub fn separator_before(rows: &[StatusRow]) -> Option<usize> {
    let i = rows.iter().position(|r| r.snoozed)?;
    (i > 0).then_some(i)
}

/// The modifier word for a `SwitchTabModifier`/`SwitchPaneModifier` tag
/// (`iTermPreferences.maskForModifierTag:`, `keyroute::tag_mask`), or `None` for its "none"
/// and "function" tags. Win stands for Cmd and Alt for Option (project convention).
pub fn modifier_prefix(tag: i64) -> Option<&'static str> {
    match tag {
        1 => Some("Ctrl"),
        4 => Some("Win"),
        5 => Some("Alt"),
        6 => Some("Win+Alt"),
        9 | 10 => None,
        _ => Some("Win+Alt"),
    }
}

/// A row's shortcut label (`ToolStatus.shortcutLabel`, `TS:980-1026`): a pane shortcut when
/// the session is in the current tab and that tab has more than one pane, else a tab shortcut
/// for tabs 1 to 9 (a session in a background tab is reached by switching tabs, never by a
/// pane shortcut, even for the tab's only or active pane); tab 10 and beyond has no shortcut
/// at all. Each form is the modifier word plus the number, or "Pane N" / "Tab N" when that
/// switch modifier is "none".
pub fn shortcut_label(tab_index: usize, current_tab: bool, pane_ordinal: usize, panes_in_tab: usize, tab_tag: i64, pane_tag: i64) -> Option<String> {
    if current_tab && panes_in_tab > 1 && pane_ordinal != 0 {
        return Some(match modifier_prefix(pane_tag) {
            Some(prefix) => format!("{prefix}+{pane_ordinal}"),
            None => format!("Pane {pane_ordinal}"),
        });
    }
    if tab_index < 1 || tab_index > 9 {
        return None;
    }
    Some(match modifier_prefix(tab_tag) {
        Some(prefix) => format!("{prefix}+{tab_index}"),
        None => format!("Tab {tab_index}"),
    })
}

/// The automatic per-pattern notification's title and body, posted when a matched pattern's
/// `notify` flag is set and the status text changed (`PS:25683-25700`). Unlike `watch_text`,
/// this uses the raw wire status text, not its display form.
pub fn notify_text(name: &str, status: &str) -> (String, String) {
    (format!("Session \u{201c}{name}\u{201d}"), format!("Status changed to \u{201c}{status}\u{201d}"))
}

/// The Bell button's one-shot watch alert body (`iTermNotifyOnStatusChangeController.swift`,
/// `presentAlert`): the session name, then the old and new status in their display form,
/// `"none"` for an absent one.
pub fn watch_text(name: &str, from: Option<&str>, to: Option<&str>) -> String {
    let show = |s: Option<&str>| s.map(display_status).unwrap_or_else(|| "none".to_string());
    format!("{name} changed from \u{201c}{}\u{201d} to \u{201c}{}\u{201d}.", show(from), show(to))
}

/// What a pass of the watches decided: the alerts to show (pane, from, to), then the watches
/// that fired or lost their session and so disarm (`NOC`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fired {
    pub alerts: Vec<(PaneId, Option<String>, Option<String>)>,
    pub disarm_windows: Vec<u64>,
    pub disarm_sessions: Vec<PaneId>,
}

/// One pass of Notify on Status Change (`NOC:31-275`). A session watch compares the status its
/// pane has now with the one captured at arming and fires when they differ (a change back
/// before the pass gives nothing); a pane that is gone drops the watch with no alert. A
/// window watch (`armed_windows`: the window and its panes now, so panes made later count)
/// fires on the first change among its own panes. One alert per pane, whichever watch saw it
/// first; every watch that fired disarms.
pub fn fire(
    changes: &[(PaneId, Option<String>, Option<String>)],
    armed_windows: &[(u64, Vec<PaneId>)],
    sessions: &BTreeMap<PaneId, Option<String>>,
    now: &dyn Fn(PaneId) -> Option<Option<String>>,
) -> Fired {
    let mut out = Fired::default();
    for (pane, base) in sessions {
        match now(*pane) {
            None => out.disarm_sessions.push(*pane),
            Some(cur) if cur != *base => {
                out.alerts.push((*pane, base.clone(), cur));
                out.disarm_sessions.push(*pane);
            }
            Some(_) => {}
        }
    }
    for (window, panes) in armed_windows {
        let Some(change) = changes.iter().find(|c| panes.contains(&c.0)) else { continue };
        if !out.alerts.iter().any(|a| a.0 == change.0) {
            out.alerts.push(change.clone());
        }
        out.disarm_windows.push(*window);
    }
    out
}

/// The settings popover's instructional label (`SPS`, `StatusPriority.InstructionsLabel`).
pub const SETTINGS_TEXT: &str = "Statuses are sorted by priority. Items near the top have higher priority. Drag to reorder. Click to edit.";

/// The "+" button's new-pattern prompt: title, then informative text (`SPS`,
/// `StatusPriority.NewPatternTitle` / `NewPatternInfo`).
pub const NEW_PATTERN: (&str, &str) = ("New Priority Pattern", "Enter a substring to match against status text (case-insensitive).");

/// The "Merge workgroup statuses" checkbox: label, then tooltip (`SPS`,
/// `StatusPriority.MergeWorkgroups` / `MergeWorkgroupsTooltip`). Stored but a no-op in Tandem
/// (ruling: no workgroups).
pub const MERGE_LABEL: (&str, &str) = ("Merge workgroup statuses", "When enabled, only the most recent status from each workgroup is shown.");

/// The "Show status in tab subtitle" checkbox's label (`SPS`,
/// `StatusPriority.ShowStatusInTabSubtitle`).
pub const SUBTITLE_LABEL: &str = "Show status in tab subtitle";

/// The help popover's markdown (`ToolStatus.helpMarkdown`).
pub const HELP_TEXT: &str = "## Session Status\n\nThe **Session Status** tool shows the status of sessions across all tabs. Each entry displays the session name, a colored indicator dot, status text, and a keyboard shortcut to jump to that session.\n\n### Setting Status\n\n**Triggers:** Add a **Set Tab Status** trigger in **Prefs > Profiles > Advanced > Triggers**. It lets you set the status text, dot color, and text color when a regex matches terminal output.\n\n**Control Sequence (OSC 21337):** Programs can set tab status directly:\n\n`printf '\\e]21337;status=Working;indicator=#ffa500\\a'`\n\nSupported keys (separated by `;`):\n\n* `status=TEXT` \u{2014} Sets the status text (e.g., \u{201c}Working\u{201d}, \u{201c}Waiting\u{201d}).\n* `indicator=COLOR` \u{2014} Sets the dot color.\n* `status-color=COLOR` \u{2014} Sets the status text color.\n* `detail=TEXT` \u{2014} Sets optional detail text shown below the status (wraps up to 3 lines).\n\nColors use xterm format: `rgb:RR/GG/BB` (hex, 1\u{2013}4 digits per component) or `#RRGGBB`.\n\nSet a key to empty to clear it: `status=` clears the status text.\n\n**Clear all status:** `printf '\\e]21337;status=;indicator=\\a'`\n\n### Priority Sorting\n\nSessions are sorted by priority. Click the \u{2699} button to configure which status keywords have the highest priority. The default order is: waiting, working, idle.\n\n### Snoozing\n\nRight-click a row and choose **Snooze** to move it to the bottom of the list and dim it. A snoozed entry automatically un-snoozes the next time its status changes.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabstatus::{Status, aggregate};

    fn st(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn watches_follow_iterm2() {
        let sessions: BTreeMap<PaneId, Option<String>> = [(1, st("idle"))].into();
        let working = |p: PaneId| (p == 1).then(|| st("working"));
        // A session watch fires on a status that differs from the one captured at arming.
        let f = fire(&[(1, st("idle"), st("working"))], &[], &sessions, &working);
        assert_eq!(f, Fired { alerts: vec![(1, st("idle"), st("working"))], disarm_windows: vec![], disarm_sessions: vec![1] });
        // A change back to the baseline before the tick gives nothing, and the watch stays.
        let back = |p: PaneId| (p == 1).then(|| st("idle"));
        assert_eq!(fire(&[(1, st("working"), st("idle"))], &[], &sessions, &back), Fired::default());
        // A watched pane that is gone is dropped silently.
        assert_eq!(fire(&[], &[], &sessions, &|_| None), Fired { alerts: vec![], disarm_windows: vec![], disarm_sessions: vec![1] });
        // A window fires on the first change among its own panes, and only those.
        let none = BTreeMap::new();
        let nothing = |_: PaneId| Some(None);
        let win = [(7, vec![1, 2])];
        let f = fire(&[(3, None, st("working")), (2, None, st("waiting"))], &win, &none, &nothing);
        assert_eq!(f, Fired { alerts: vec![(2, None, st("waiting"))], disarm_windows: vec![7], disarm_sessions: vec![] });
        assert_eq!(fire(&[(3, None, st("working"))], &win, &none, &nothing), Fired::default());
        // Armed both ways: one alert, both disarm.
        let both: BTreeMap<PaneId, Option<String>> = [(2, None)].into();
        let waiting = |p: PaneId| (p == 2).then(|| st("waiting"));
        let f = fire(&[(2, None, st("waiting"))], &win, &both, &waiting);
        assert_eq!(f, Fired { alerts: vec![(2, None, st("waiting"))], disarm_windows: vec![7], disarm_sessions: vec![2] });
        // A pane made after arming (a change from None) fires its window.
        let win = [(7, vec![1, 2, 4])];
        let f = fire(&[(4, None, st("idle"))], &win, &none, &nothing);
        assert_eq!(f.alerts, vec![(4, None, st("idle"))]);
        assert_eq!(f.disarm_windows, vec![7]);
        // Two windows armed over different panes each fire on their own change.
        let wins = [(7, vec![1]), (8, vec![2])];
        let f = fire(&[(1, None, st("a")), (2, None, st("b"))], &wins, &none, &nothing);
        assert_eq!(f.alerts, vec![(1, None, st("a")), (2, None, st("b"))]);
        assert_eq!(f.disarm_windows, vec![7, 8]);
    }

    fn row(pane: u64, snoozed: bool, changed: u64) -> StatusRow {
        StatusRow { pane, snoozed, priority: 0, changed, name: String::new(), status: None, detail: None, shortcut: None }
    }

    #[test]
    fn status_rows_sort_like_iterm2() {
        let patterns = vec![
            Pattern { pattern: "wait".to_string(), notify: false },
            Pattern { pattern: "work".to_string(), notify: false },
            Pattern { pattern: "idle".to_string(), notify: false },
        ];
        assert_eq!(priority(Some("Waiting for input"), &patterns), 0);
        assert_eq!(priority(Some("WORKING"), &patterns), 1);
        assert_eq!(priority(Some("done"), &patterns), 3, "unmatched ranks as count");
        assert_eq!(priority(None, &patterns), 4, "no text ranks below every pattern");
        // An empty list ranks everything count + 1, text or not (`SPS:87-89`).
        assert_eq!(priority(Some("waiting"), &[]), 1);
        assert_eq!(priority(None, &[]), 1);

        // A snoozed wait row (priority 0) still sinks after an un-snoozed idle row (priority 4).
        let mut rows = vec![
            StatusRow { pane: 1, snoozed: true, priority: 0, changed: 1, name: "wait".to_string(), status: None, detail: None, shortcut: None },
            StatusRow { pane: 2, snoozed: false, priority: 4, changed: 2, name: "idle".to_string(), status: None, detail: None, shortcut: None },
        ];
        sort_rows(&mut rows);
        assert_eq!(rows.iter().map(|r| r.pane).collect::<Vec<_>>(), vec![2, 1]);

        // Same priority: the older change (smaller `changed`) sorts first.
        let mut by_age = vec![row(5, false, 20), row(3, false, 10)];
        sort_rows(&mut by_age);
        assert_eq!(by_age.iter().map(|r| r.pane).collect::<Vec<_>>(), vec![3, 5]);

        // A full tie on snoozed, priority and changed breaks on pane id.
        let mut by_pane = vec![row(9, false, 10), row(4, false, 10)];
        sort_rows(&mut by_pane);
        assert_eq!(by_pane.iter().map(|r| r.pane).collect::<Vec<_>>(), vec![4, 9]);

        assert_eq!(separator_before(&[]), None);
        assert_eq!(separator_before(&[row(1, false, 0), row(2, false, 1)]), None, "nothing snoozed");
        assert_eq!(separator_before(&[row(1, true, 0), row(2, true, 1)]), None, "never row 0");
        assert_eq!(separator_before(&[row(1, false, 0), row(2, false, 1), row(3, true, 2)]), Some(2));

        // tabstatus::aggregate with a custom pattern list, "done" first: it beats "wait" when
        // the focused pane has no status.
        let custom = ["done", "wait", "work", "idle"];
        let focused = Status::default();
        let waiting = Status { status: Some("waiting".to_string()), ..Status::default() };
        let done = Status { status: Some("all done".to_string()), ..Status::default() };
        assert_eq!(aggregate(Some(&focused), &[&waiting, &done], &custom), Some(&done));
    }

    #[test]
    fn patterns_and_texts_follow_iterm2() {
        let defaults = load_patterns(None, None);
        assert_eq!(
            defaults,
            vec![
                Pattern { pattern: "wait".to_string(), notify: false },
                Pattern { pattern: "work".to_string(), notify: false },
                Pattern { pattern: "idle".to_string(), notify: false },
            ]
        );

        let legacy = Value::Array(vec![Value::String("a".to_string()), Value::String("b".to_string())]);
        let migrated = load_patterns(None, Some(&legacy));
        assert_eq!(migrated, vec![Pattern { pattern: "a".to_string(), notify: false }, Pattern { pattern: "b".to_string(), notify: false }]);

        // entries wins over legacy when both are present.
        let entries = patterns_value(&[Pattern { pattern: "c".to_string(), notify: true }]);
        assert_eq!(load_patterns(Some(&entries), Some(&legacy)), vec![Pattern { pattern: "c".to_string(), notify: true }]);

        // patterns_value round-trips as an array of {pattern, notify} tables.
        let value = patterns_value(&migrated);
        let arr = value.as_array().expect("array");
        assert_eq!(arr[0]["pattern"].as_str(), Some("a"));
        assert_eq!(arr[0]["notify"].as_bool(), Some(false));
        assert_eq!(load_patterns(Some(&value), None), migrated);

        let notify_patterns = vec![Pattern { pattern: "err".to_string(), notify: true }, Pattern { pattern: "wait".to_string(), notify: false }];
        assert!(should_notify(Some("an ERROR occurred"), &notify_patterns), "case-insensitive, first match's flag");
        assert!(!should_notify(Some("waiting"), &notify_patterns));
        assert!(!should_notify(None, &notify_patterns));

        assert_eq!(display_status("work"), "Working");
        assert_eq!(display_status("wait"), "Waiting");
        assert_eq!(display_status("idle"), "Idle");
        assert_eq!(display_status("Work"), "Work", "exact wire values only");

        assert_eq!(notify_text("zsh", "done"), ("Session \u{201c}zsh\u{201d}".to_string(), "Status changed to \u{201c}done\u{201d}".to_string()));

        // The brief's literal example uses "work" unmapped in watch_text's output, but the
        // source (iTermNotifyOnStatusChangeController.presentAlert) runs both sides through
        // SetTabStatusTrigger.localizedStatusForDisplay, the same table as display_status.
        // Followed the source here; see the report.
        assert_eq!(watch_text("zsh", None, Some("work")), "zsh changed from \u{201c}none\u{201d} to \u{201c}Working\u{201d}.");
        assert_eq!(watch_text("zsh", Some("idle"), Some("free text")), "zsh changed from \u{201c}Idle\u{201d} to \u{201c}free text\u{201d}.");

        // The named tooltip/prompt/label constants exist with iTerm2's exact wire values.
        assert_eq!(SETTINGS_TEXT, "Statuses are sorted by priority. Items near the top have higher priority. Drag to reorder. Click to edit.");
        assert_eq!(NEW_PATTERN, ("New Priority Pattern", "Enter a substring to match against status text (case-insensitive)."));
        assert_eq!(MERGE_LABEL, ("Merge workgroup statuses", "When enabled, only the most recent status from each workgroup is shown."));
        assert_eq!(SUBTITLE_LABEL, "Show status in tab subtitle");
        assert!(HELP_TEXT.starts_with("## Session Status"));
        assert!(HELP_TEXT.contains("waiting, working, idle"));
    }

    #[test]
    fn shortcuts_follow_iterm2() {
        assert_eq!(modifier_prefix(1), Some("Ctrl"));
        assert_eq!(modifier_prefix(4), Some("Win"));
        assert_eq!(modifier_prefix(5), Some("Alt"));
        assert_eq!(modifier_prefix(6), Some("Win+Alt"));
        assert_eq!(modifier_prefix(9), None);
        assert_eq!(modifier_prefix(10), None);

        // Another pane (ordinal 2 of 3) of the current tab.
        assert_eq!(shortcut_label(1, true, 2, 3, 9, 1), Some("Ctrl+2".to_string()));
        assert_eq!(shortcut_label(1, true, 2, 3, 9, 9), Some("Pane 2".to_string()), "pane modifier none");

        // Tab 4, another tab.
        assert_eq!(shortcut_label(4, false, 1, 1, 4, 9), Some("Win+4".to_string()));
        assert_eq!(shortcut_label(4, false, 1, 1, 9, 9), Some("Tab 4".to_string()), "tab modifier none");

        // Tab 10 and beyond has no shortcut at all.
        assert_eq!(shortcut_label(10, false, 1, 1, 4, 1), None);

        // The focused pane of a background tab still gets its tab's label, never a pane label,
        // even though it looks like "the" pane (ordinal 1 of a multi-pane tab).
        assert_eq!(shortcut_label(2, false, 1, 3, 4, 1), Some("Win+2".to_string()));

        // A single-pane current tab also falls through to the tab label (iTerm2 only gives a
        // pane shortcut when the current tab has more than one session).
        assert_eq!(shortcut_label(3, true, 1, 1, 4, 1), Some("Win+3".to_string()));
    }
}
