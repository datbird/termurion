use std::sync::OnceLock;

use crate::globals::Globals;

/// The stored keys (`iTermTipController.m`, `iTermLaunchExperienceController.m`); none has a row,
/// only `NoSyncTipsDisabled` and `TimeBetweenTips` do (Advanced, "Tip of the Day").
pub const SEEN_KEY: &str = "NoSyncTipsToNotShow";
pub const LAST_TIP_KEY: &str = "NoSyncLastTipTime";
pub const DISABLED_KEY: &str = "NoSyncTipsDisabled";
pub const PERMISSION_KEY: &str = "NoSyncPermissionToShowTip";
pub const OFFSET_KEY: &str = "NoSyncTipOfTheDayOffset";
pub const RUN_COUNT_KEY: &str = "NoSyncLaunchExperienceControllerRunCount";
pub const NEXT_ANNOYANCE_KEY: &str = "NoSyncNextAnnoyanceTime";
pub const ELIGIBILITY_KEY: &str = "NoSyncTipOfTheDayEligibilityBeganTime";
pub const BETWEEN_KEY: &str = "TimeBetweenTips";

pub const DAY: f64 = 86400.0;
/// The shortest and longest gap honored: iTerm2 reads any value, but a zero or negative gap would
/// make the next try due at once, forever.
pub const MIN_BETWEEN: f64 = 60.0;
pub const MAX_BETWEEN: f64 = 365.0 * DAY;

/// One tip: its key (stored in `NoSyncTipsToNotShow`, never changed), title, body and optional
/// address (a fixed https page, `tips_urls_are_fixed`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tip {
    pub key: String,
    pub title: String,
    pub body: String,
    pub url: Option<String>,
}

const DATA: &str = include_str!("../resources/tips.toml");

/// Every tip, sorted by key as `compare:` sorts (byte order). Parsed once from the built-in data.
pub fn all() -> &'static [Tip] {
    static TIPS: OnceLock<Vec<Tip>> = OnceLock::new();
    TIPS.get_or_init(|| {
        let table: toml::Table = toml::from_str(DATA).expect("resources/tips.toml parses");
        let mut tips: Vec<Tip> = table
            .get("tip")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|t| {
                        let text = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_string);
                        Some(Tip { key: text("key")?, title: text("title")?, body: text("body")?, url: text("url") })
                    })
                    .collect()
            })
            .unwrap_or_default();
        tips.sort_by(|a, b| a.key.as_bytes().cmp(b.key.as_bytes()));
        tips
    })
}

/// `tipKeyAfter:respectUnshowable:` (`iTermTipController.m:188-205`): the first tip, in key order,
/// not in `unshowable` and (when `after` is given) sorting after it.
pub fn next_key(all: &[Tip], after: Option<&str>, unshowable: &[String]) -> Option<String> {
    all.iter()
        .filter(|t| !unshowable.contains(&t.key))
        .find(|t| after.map_or(true, |a| t.key.as_bytes() > a.as_bytes()))
        .map(|t| t.key.clone())
}

/// `tipKeyBefore:` (`:207-214`): the tip before `of`, None for the first or an unknown key.
pub fn prev_key(all: &[Tip], of: &str) -> Option<String> {
    match all.iter().position(|t| t.key == of) {
        Some(i) if i > 0 => Some(all[i - 1].key.clone()),
        _ => None,
    }
}

/// The tip the menu item shows (`showTip`, `:162-176`): the last one seen, else the first unseen.
/// A seen key the data no longer has is passed over.
pub fn menu_key(all: &[Tip], unshowable: &[String]) -> Option<String> {
    unshowable
        .iter()
        .rev()
        .find(|k| all.iter().any(|t| &t.key == *k))
        .cloned()
        .or_else(|| next_key(all, None, unshowable))
}

/// `doNotShowCurrentTipAgain` (`:233-243`): the seen list with duplicates removed and `key` last
/// (the last entry decides which tip the menu shows). Capped at the tip count, oldest dropped
/// first, so a hand-edited file cannot grow it.
pub fn seen(unshowable: &[String], key: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for k in unshowable {
        if k != key && !out.contains(k) {
            out.push(k.clone());
        }
    }
    out.push(key.to_string());
    let cap = all().len().max(1);
    if out.len() > cap {
        out.drain(..out.len() - cap);
    }
    out
}

/// `haveShownTipRecently` (`:178-182`).
pub fn shown_recently(now: f64, last: f64, between: f64) -> bool {
    now - last < between
}

/// What `tryToShowTip` does (`:136-160`). `Wait` tries again after the gap; `Never` does nothing
/// until Tandem becomes active again, so an idle Tandem is never woken for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Try {
    Show,
    Wait,
    Never,
}

/// `permission` is `NoSyncPermissionToShowTip` (None when never asked); `active` is a terminal
/// window being focused (P18), `showing` a tip card being open, `recently` `shown_recently`.
pub fn try_show(permission: Option<bool>, disabled: bool, active: bool, showing: bool, recently: bool) -> Try {
    if permission != Some(true) || disabled || !active {
        return Try::Never;
    }
    if showing || recently {
        return Try::Wait;
    }
    Try::Show
}

/// `willAskPermission` (`:98-104`): never asked and not disabled.
pub fn will_ask(permission: Option<bool>, disabled: bool) -> bool {
    permission.is_none() && !disabled
}

/// `tipFrequencyIsHigh` (`:290`): a day or less between tips.
pub fn frequency_high(between: f64) -> bool {
    between <= DAY
}

/// `toggleTipFrequency` (`:292-300`): daily becomes weekly and anything else daily.
pub fn toggled_frequency(between: f64) -> f64 {
    if frequency_high(between) {
        7.0 * DAY
    } else {
        DAY
    }
}

/// The gap as the schedule uses it: finite and between a minute and a year, else a day.
pub fn between_clamped(between: f64) -> f64 {
    if between.is_finite() {
        between.clamp(MIN_BETWEEN, MAX_BETWEEN)
    } else {
        DAY
    }
}

/// The one instant the next try is due: the last tip plus the gap. The app sets a single
/// deadline from this and sleeps until it, as `performSelector:afterDelay:` does; it never polls.
pub fn next_try_at(last: f64, between: f64) -> f64 {
    last + between_clamped(between)
}

/// What a launch does about tips (`iTermLaunchExperienceController.m:92-160`, without What's New
/// and the paste warning). `Nothing` is iTerm2's "none": a start that may not ask. `AskThenStart`
/// asks "See Tips of the Day?" (and quells annoyances for a day) then starts; `Start` starts with
/// no question (the steady state). The first-eligibility two-day quell (`LE:121-130`) needs the
/// stored `NoSyncTipOfTheDayEligibilityBeganTime`, so the app applies it to an `AskThenStart`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Launch {
    AskThenStart,
    Start,
    Nothing,
}

/// `run_count` counts this launch; `quelled` is `NoSyncNextAnnoyanceTime` still ahead;
/// `automatic_checks_on` is `update::automatic_checks_on` (the raw value): on the second launch
/// with it off, the update question takes the launch (`LE:117`).
pub fn launch_choice(run_count: i64, quelled: bool, will_ask: bool, automatic_checks_on: bool) -> Launch {
    if run_count == 2 && !automatic_checks_on {
        return Launch::Nothing;
    }
    if quelled {
        return Launch::Nothing;
    }
    if will_ask {
        Launch::AskThenStart
    } else {
        Launch::Start
    }
}

fn number_of(g: &Globals, key: &str) -> Option<f64> {
    g.raw(key).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
}

/// `NoSyncPermissionToShowTip`, None when never asked.
pub fn permission_of(g: &Globals) -> Option<bool> {
    g.raw(PERMISSION_KEY).and_then(|v| v.as_bool())
}

/// `NoSyncTipsToNotShow`: only entries naming a real tip, at most one per tip.
pub fn seen_of(g: &Globals) -> Vec<String> {
    let Some(list) = g.raw(SEEN_KEY).and_then(|v| v.as_array()) else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for k in list.iter().filter_map(|v| v.as_str()) {
        if all().iter().any(|t| t.key == k) && !out.iter().any(|o| o == k) {
            out.push(k.to_string());
        }
    }
    out
}

/// `NoSyncLastTipTime` in seconds, 0 when never shown.
pub fn last_tip_time_of(g: &Globals) -> f64 {
    number_of(g, LAST_TIP_KEY).filter(|v| v.is_finite()).unwrap_or(0.0)
}

/// `TimeBetweenTips` as the schedule uses it (`between_clamped`).
pub fn between_of(g: &Globals) -> f64 {
    between_clamped(g.number(BETWEEN_KEY).unwrap_or(DAY))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn tip(key: &str) -> Tip {
        Tip { key: key.into(), title: key.into(), body: key.into(), url: None }
    }

    #[test]
    fn tips_load_and_translate() {
        let all = all();
        assert_eq!(all.len(), 122);
        assert!(all.windows(2).all(|w| w[0].key.as_bytes() < w[1].key.as_bytes()), "sorted by key");
        let first = &all[0];
        assert_eq!(first.key, "000");
        assert_eq!(first.title, "Tip of the Day");
        assert_eq!(
            first.body,
            "This window shows the Tandem tip of the day. It\u{2019}ll appear every 24 hours to let you know about new features and hidden secrets. Hit \u{201c}More Options\u{201d} to view more tips or to stop getting them altogether."
        );
        let by = |k: &str| all.iter().find(|t| t.key == k).unwrap_or_else(|| panic!("{k}"));
        assert!(by("0003").body.contains("Shift+Win+O"));
        assert!(by("0027").body.starts_with("The taskbar button shows a count"));
        assert_eq!(by("0000").url.as_deref(), Some("https://iterm2.com/shell_integration.html"));
        assert!(all.iter().all(|t| t.key != "0066" && t.key != "0105"));
        for t in all {
            for bad in ["iTerm2", "\u{2318}", "\u{2325}", "\u{21e7}", "\u{2303}", "Finder"] {
                assert!(!t.title.contains(bad) && !t.body.contains(bad), "{} has {bad}", t.key);
            }
        }
    }

    /// P11: a tip's address is one of two fixed https hosts; the data is static.
    #[test]
    fn tips_urls_are_fixed() {
        for t in all() {
            if let Some(u) = &t.url {
                assert!(
                    u.starts_with("https://iterm2.com/")
                        || u.starts_with("https://www.iterm2.com/")
                        || u.starts_with("https://gitlab.com/gnachman/iterm2/"),
                    "{}: {u}",
                    t.key
                );
            }
        }
    }

    #[test]
    fn tip_order_follows_iterm2() {
        let all = [tip("a"), tip("b"), tip("c")];
        assert_eq!(next_key(&all, None, &[]).as_deref(), Some("a"));
        assert_eq!(next_key(&all, Some("a"), &[]).as_deref(), Some("b"));
        assert_eq!(next_key(&all, None, &keys(&["a"])).as_deref(), Some("b"));
        assert_eq!(next_key(&all, Some("c"), &[]), None);
        assert_eq!(next_key(&all, None, &keys(&["a", "b", "c"])), None);
        assert_eq!(prev_key(&all, "b").as_deref(), Some("a"));
        assert_eq!(prev_key(&all, "a"), None);
        assert_eq!(prev_key(&all, "zzz"), None);
        assert_eq!(menu_key(&all, &[]).as_deref(), Some("a"));
        assert_eq!(menu_key(&all, &keys(&["b", "a"])).as_deref(), Some("a"));
        // A seen key the data no longer has is not shown.
        assert_eq!(menu_key(&all, &keys(&["b", "gone"])).as_deref(), Some("b"));
        let s = seen(&keys(&["a", "b", "a"]), "c");
        assert_eq!(s.last().map(String::as_str), Some("c"));
        assert_eq!(s.iter().filter(|k| *k == "a").count(), 1);
        assert_eq!(s.iter().filter(|k| *k == "b").count(), 1);
        assert_eq!(s.len(), 3);
        // Seeing the last tip again keeps it last, once.
        assert_eq!(seen(&keys(&["a", "b"]), "a"), keys(&["b", "a"]));
    }

    /// The stored list never outgrows the tips (P11).
    #[test]
    fn seen_list_is_capped() {
        let long: Vec<String> = (0..400).map(|i| format!("k{i}")).collect();
        let s = seen(&long, "last");
        assert_eq!(s.len(), all().len());
        assert_eq!(s.last().map(String::as_str), Some("last"));
    }

    #[test]
    fn tip_schedule_follows_iterm2() {
        assert_eq!(try_show(Some(false), false, true, false, false), Try::Never);
        assert_eq!(try_show(None, false, true, false, false), Try::Never);
        assert_eq!(try_show(Some(true), true, true, false, false), Try::Never);
        assert_eq!(try_show(Some(true), false, false, false, false), Try::Never);
        assert_eq!(try_show(Some(true), false, true, true, false), Try::Wait);
        assert_eq!(try_show(Some(true), false, true, true, true), Try::Wait);
        assert_eq!(try_show(Some(true), false, true, false, true), Try::Wait);
        assert_eq!(try_show(Some(true), false, true, false, false), Try::Show);
        assert!(shown_recently(100.0, 50.0, DAY));
        assert!(!shown_recently(50.0 + DAY, 50.0, DAY));
        assert!(will_ask(None, false));
        assert!(!will_ask(None, true));
        assert!(!will_ask(Some(false), false));
        assert!(!will_ask(Some(true), false));
        assert!(frequency_high(DAY));
        assert!(!frequency_high(7.0 * DAY));
        assert_eq!(toggled_frequency(DAY), 604800.0);
        assert_eq!(toggled_frequency(604800.0), DAY);
    }

    /// The idle app is never woken to poll: the next try is one instant, the last tip plus the gap.
    #[test]
    fn schedule_names_one_deadline() {
        assert_eq!(next_try_at(1000.0, DAY), 1000.0 + DAY);
        // No tip yet: due now.
        assert_eq!(next_try_at(0.0, DAY), 0.0 + DAY);
        // A gap below a minute reads as a minute, so a bad value cannot spin the loop.
        assert_eq!(between_clamped(0.0), MIN_BETWEEN);
        assert_eq!(between_clamped(f64::NAN), DAY);
        assert_eq!(between_clamped(f64::INFINITY), DAY);
        assert_eq!(between_clamped(604800.0), 604800.0);
        assert_eq!(between_clamped(1e12), MAX_BETWEEN);
    }

    #[test]
    fn launch_experience_follows_iterm2() {
        assert_eq!(launch_choice(2, false, true, false), Launch::Nothing);
        assert_eq!(launch_choice(2, false, true, true), Launch::AskThenStart);
        assert_eq!(launch_choice(3, true, true, true), Launch::Nothing);
        assert_eq!(launch_choice(3, false, true, true), Launch::AskThenStart);
        assert_eq!(launch_choice(3, false, false, true), Launch::Start);
        assert_eq!(launch_choice(1, false, true, false), Launch::AskThenStart);
    }

    #[test]
    fn stored_state_reads_bounded() {
        use toml::Value;
        let dir = std::env::temp_dir().join("tandem-tips-state-test");
        let mut g = crate::globals::Globals::empty(&dir.join("settings.toml"));
        assert_eq!(permission_of(&g), None);
        assert!(seen_of(&g).is_empty());
        assert_eq!(last_tip_time_of(&g), 0.0);
        g.set(PERMISSION_KEY, Value::Boolean(false));
        assert_eq!(permission_of(&g), Some(false));
        g.set(SEEN_KEY, Value::Array(vec!["0001".into(), "bogus".into(), Value::Integer(3), "0000".into()]));
        assert_eq!(seen_of(&g), keys(&["0001", "0000"]));
        g.set(LAST_TIP_KEY, Value::Float(12.5));
        assert_eq!(last_tip_time_of(&g), 12.5);
        g.set(LAST_TIP_KEY, Value::Integer(7));
        assert_eq!(last_tip_time_of(&g), 7.0);
        assert_eq!(between_of(&g), DAY);
        g.set("TimeBetweenTips", Value::Float(0.0));
        assert_eq!(between_of(&g), MIN_BETWEEN);
    }
}
