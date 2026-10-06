//! Settings > Advanced, the pane's model: iTerm2's categories and the keys it keeps outside every pane.

use std::cmp::Ordering;

use toml::Value;

use crate::schema::{self, Kind, Setting};

/// iTerm2's Advanced categories, the SECTION_* values in `iTermAdvancedSettingsModel.m` order.
pub const CATEGORIES: [&str; 19] = [
    "Tabs",
    "Mouse",
    "Terminal",
    "Hotkey",
    "General",
    "Drawing",
    "Semantic History",
    "Debugging",
    "Session",
    "Windows",
    "Tmux Integration",
    "SSH Integration",
    "Warnings",
    "Pasteboard",
    "Tip of the Day",
    "Badge",
    "Web Browser",
    "Experimental Features",
    "Scripting",
];

/// The tab of keys iTerm2 keeps outside every settings pane: a dialog's remembered value, a menu
/// toggle, a warning's silence. No page draws it.
pub const STORED_TAB: &str = "Stored";

/// How a row's value is edited (`iTermAdvancedSettingType`).
#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    /// A No/Yes popup.
    Bool,
    /// iTerm2's DEFINE_OPTIONAL_BOOL: Unspecified/No/Yes; Unspecified removes the key.
    OptionalBool,
    /// A popup of the titles; the stored value is the chosen option's value.
    Enum(Vec<(Value, String)>),
    Int,
    Float,
    Text,
}

/// One setting of the Advanced pane.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: &'static str,
    pub category: &'static str,
    /// The description's first line, drawn at the system size.
    pub label: &'static str,
    /// The rest of the description (after its first newline), drawn small and gray.
    pub subtitle: Option<&'static str>,
    /// iTerm2's whole description: what the search reads and the order uses.
    pub description: &'static str,
    pub kind: RowKind,
    pub default: Option<Value>,
    pub macos_only: bool,
}

fn row_of(s: &'static Setting) -> Row {
    let description = if s.help.is_empty() { s.label.as_str() } else { s.help.as_str() };
    let kind = match s.kind {
        Kind::Bool if s.optional_bool() => RowKind::OptionalBool,
        Kind::Bool => RowKind::Bool,
        Kind::Enum => RowKind::Enum(s.choices.iter().map(|c| (c.value.clone(), c.label.clone())).collect()),
        Kind::Int => RowKind::Int,
        Kind::Float => RowKind::Float,
        _ => RowKind::Text,
    };
    Row {
        key: &s.key,
        category: &s.section,
        label: &s.label,
        subtitle: description.split_once('\n').map(|(_, rest)| rest),
        description,
        kind,
        default: s.default.clone(),
        macos_only: s.macos_only,
    }
}

/// Every setting on the Advanced tab, ordered as iTerm2 lists them: by category, then by
/// description, both Finder-style (`compareAdvancedSettingDict:toDict:`).
pub fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = schema::global().iter().filter(|s| s.tab == "Advanced").map(row_of).collect();
    rows.sort_by(|a, b| standard_compare(a.category, b.category).then_with(|| standard_compare(a.description, b.description)));
    rows
}

/// A run of ASCII digits starting at `i`, as (end, value without leading zeros).
fn digit_run(chars: &[char], i: usize) -> (usize, &[char]) {
    let mut end = i;
    while end < chars.len() && chars[end].is_ascii_digit() {
        end += 1;
    }
    let mut start = i;
    while start + 1 < end && chars[start] == '0' {
        start += 1;
    }
    (end, &chars[start..end])
}

/// Finder-style order (`localizedStandardCompare:`): case folded, runs of digits compared by
/// value, and strings that are equivalent but not identical still ordered (forced ordering) by
/// their original text so the sort is stable.
pub fn standard_compare(a: &str, b: &str) -> Ordering {
    let x: Vec<char> = a.chars().flat_map(char::to_lowercase).collect();
    let y: Vec<char> = b.chars().flat_map(char::to_lowercase).collect();
    let (mut i, mut j) = (0, 0);
    while i < x.len() && j < y.len() {
        if x[i].is_ascii_digit() && y[j].is_ascii_digit() {
            let (ei, di) = digit_run(&x, i);
            let (ej, dj) = digit_run(&y, j);
            let by_value = di.len().cmp(&dj.len()).then_with(|| di.cmp(dj)).then_with(|| (ei - i).cmp(&(ej - j)));
            if by_value != Ordering::Equal {
                return by_value;
            }
            i = ei;
            j = ej;
            continue;
        }
        match x[i].cmp(&y[j]) {
            Ordering::Equal => {
                i += 1;
                j += 1;
            }
            other => return other,
        }
    }
    (x.len() - i).cmp(&(y.len() - j)).then_with(|| a.cmp(b))
}

/// The search: the query split on single spaces, empty words skipped, and every other word found
/// in `searchable` ignoring case (`description:matchesQuery:`).
pub fn matches(searchable: &str, query: &str) -> bool {
    let hay = searchable.to_lowercase();
    query.split(' ').filter(|w| !w.is_empty()).all(|w| hay.contains(&w.to_lowercase()))
}

/// What the search reads: the description, the English category and the localized category
/// (Tandem shows English, so the category appears twice as in iTerm2's English build).
pub fn searchable(row: &Row) -> String {
    format!("{} {} {}", row.description, row.category, row.category)
}

/// Numbers closer than this to the default count as the default.
pub const EPSILON: f64 = 0.0001;

fn number(v: &Value) -> Option<f64> {
    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
}

/// True when "Show only non-default values" keeps the row: a key never set is hidden, a number
/// less than `EPSILON` from its default is hidden, and a value with no default (an optional bool set
/// to No or Yes) is shown.
pub fn is_non_default(stored: Option<&Value>, default: Option<&Value>) -> bool {
    let Some(stored) = stored else { return false };
    let Some(default) = default else { return true };
    match (number(stored), number(default)) {
        (Some(a), Some(b)) => (a - b).abs() >= EPSILON,
        _ => stored != default,
    }
}

/// One line of the pane's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A category header, bold, above the first shown row of its category.
    Header(&'static str),
    /// An index into the rows.
    Setting(usize),
}

/// The list the pane shows for `rows` (as `rows()` orders them): rows the query matches and,
/// with `only_non_default`, whose stored value is not the default. `keep` names the row being
/// edited, which stays until editing ends (iTerm2 does not reload while a field is edited).
pub fn filtered(rows: &[Row], query: &str, only_non_default: bool, stored: &dyn Fn(&str) -> Option<Value>, keep: Option<&str>) -> Vec<Item> {
    let mut items = Vec::new();
    let mut category: Option<&'static str> = None;
    for (i, row) in rows.iter().enumerate() {
        if !matches(&searchable(row), query) {
            continue;
        }
        if only_non_default && keep != Some(row.key) && !is_non_default(stored(row.key).as_ref(), row.default.as_ref()) {
            continue;
        }
        if category != Some(row.category) {
            category = Some(row.category);
            items.push(Item::Header(row.category));
        }
        items.push(Item::Setting(i));
    }
    items
}

/// A typed integer as `-[NSString integerValue]` reads it: optional leading spaces, an optional
/// sign and the leading digits (saturating), else 0.
pub fn parse_int(text: &str) -> i64 {
    let t = text.trim_start();
    let (negative, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let mut v: i64 = 0;
    for b in digits.bytes().take_while(u8::is_ascii_digit) {
        let d = i64::from(b - b'0');
        v = if negative { v.saturating_mul(10).saturating_sub(d) } else { v.saturating_mul(10).saturating_add(d) };
    }
    v
}

/// A typed number as `-[NSString doubleValue]` reads it: optional leading spaces, then the
/// longest prefix that is a number (sign, digits, one ".", an exponent), else 0.
pub fn parse_float(text: &str) -> f64 {
    let t = text.trim_start().as_bytes();
    let digits = |from: usize| from + t[from..].iter().take_while(|b| b.is_ascii_digit()).count();
    let mut end = usize::from(matches!(t.first(), Some(b'-' | b'+')));
    let int_end = digits(end);
    let mut mantissa = int_end > end;
    end = int_end;
    if t.get(end) == Some(&b'.') {
        let frac_end = digits(end + 1);
        mantissa |= frac_end > end + 1;
        end = frac_end;
    }
    if !mantissa {
        return 0.0;
    }
    if matches!(t.get(end), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(t.get(end + 1), Some(b'-' | b'+')));
        let exp_end = digits(end + 1 + sign);
        if exp_end > end + 1 + sign {
            end = exp_end;
        }
    }
    std::str::from_utf8(&t[..end]).ok().and_then(|s| s.parse().ok()).unwrap_or(0.0)
}

/// A float as the pane shows it.
pub fn format_float(v: f64) -> String {
    format!("{v}")
}

/// The bool popup's items.
pub const NO_YES: [&str; 2] = ["No", "Yes"];
/// The optional bool popup's items.
pub const TRISTATE: [&str; 3] = ["Unspecified", "No", "Yes"];

/// The optional bool popup's selected item.
pub fn tristate_index(v: Option<bool>) -> usize {
    match v {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    }
}

pub const ONLY_NON_DEFAULT: &str = "Show only non-default values";
pub const ONLY_NON_DEFAULT_TIP: &str = "When checked, removes items from the list whose value equals the default value.";
pub const SETTING_COLUMN: &str = "Setting";
pub const VALUE_COLUMN: &str = "Value";
/// The empty search field's placeholder (macOS draws "Search" in an empty search field).
pub const SEARCH_HINT: &str = "Search";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{self, Kind};

    #[test]
    fn advanced_rows_use_iterm2_categories() {
        for s in schema::global().iter().filter(|s| s.tab == "Advanced") {
            assert!(CATEGORIES.contains(&s.section.as_str()), "{} has section {}", s.key, s.section);
        }
        let expect = [
            ("defaultWideMode", "Terminal"),
            ("nativeRenderingCSSDark", "Drawing"),
            ("nativeRenderingCSSLight", "Drawing"),
            ("disableTopRightIndicators", "Drawing"),
            ("silentUserNotifications", "General"),
            ("simpleNotifications", "General"),
            ("CommandHistoryUsePower", "General"),
            ("CommandHistoryAgePower", "General"),
            ("AutocompleteMaxOptions", "General"),
            ("sessionEndMessageText", "Session"),
            ("showAutomaticProfileSwitchingBanner", "Session"),
            ("preserveFontSizeOnAutomaticProfileSwitch", "Session"),
            ("fileDropCoprocess", "Terminal"),
            ("StripZeroWidthFormatCharactersOnPaste", "Terminal"),
            ("openProfilesInNewWindow", "Tabs"),
            ("moveLeftAfterClosingTab", "Tabs"),
            ("EscapeWithQuotes", "Experimental Features"),
            // iTerm2 defines it among the experimental ones but with SECTION_GENERAL (ASM:924).
            ("GenerativeAIAllowed", "General"),
            ("clippingSeparator", "Pasteboard"),
            ("toolbeltFont", "Windows"),
            ("toolbeltFontSize", "Windows"),
            ("PinEditSession", "General"),
            ("AutoLockSessionNameOnEdit", "Session"),
        ];
        for (key, section) in expect {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str()), ("Advanced", section), "{key}");
        }
    }

    #[test]
    fn stored_rows_follow_iterm2() {
        let int = |i: i64| Some(toml::Value::Integer(i));
        let float = |f: f64| Some(toml::Value::Float(f));
        let bool = |b: bool| Some(toml::Value::Boolean(b));
        let text = |t: &str| Some(toml::Value::String(t.into()));
        let expect = [
            ("PasteSpecialChunkSize", "Advanced Paste", Kind::Int, int(1024)),
            ("PasteSpecialChunkDelay", "Advanced Paste", Kind::Float, float(0.01)),
            ("NumberOfSpacesPerTab", "Advanced Paste", Kind::Int, int(4)),
            ("TabTransform", "Advanced Paste", Kind::Int, int(0)),
            ("EscapeShellCharsWithBackslash", "Advanced Paste", Kind::Bool, bool(false)),
            ("ConvertUnicodePunctuation", "Advanced Paste", Kind::Bool, bool(false)),
            ("ConvertDosNewlines", "Advanced Paste", Kind::Bool, bool(true)),
            ("RemoveControlCodes", "Advanced Paste", Kind::Bool, bool(true)),
            ("BracketedPasteMode", "Advanced Paste", Kind::Bool, bool(true)),
            ("PasteSpecialUseRegexSubstitution", "Advanced Paste", Kind::Bool, bool(false)),
            ("PasteSpecialRegex", "Advanced Paste", Kind::String, text("")),
            ("PasteSpecialSubstitution", "Advanced Paste", Kind::String, text("")),
            ("PasteTabToStringTabStopSize", "Paste", Kind::Int, int(4)),
            ("RequireAuthenticationToOpenPasswordManager", "Password Manager", Kind::Bool, bool(true)),
            ("RequireAuthenticationAfterScreenLocks", "Password Manager", Kind::Bool, bool(false)),
            ("RequireAuthenticationOnEveryPasswordManagerOpen", "Password Manager", Kind::Bool, bool(false)),
            ("NoSyncTmuxHung", "Warnings", Kind::Bool, bool(false)),
            ("NoSyncAutoBurialReveal", "Warnings", Kind::Bool, bool(false)),
            ("NoSyncEnableAutomaticProfileSwitchingLogging", "Script Console", Kind::Bool, bool(false)),
            ("Use background pattern indicator", "Broadcast", Kind::Bool, bool(false)),
            ("NoSyncHideDefaultValuedSettings", "Settings", Kind::Bool, bool(false)),
            ("NoSyncNeverRemindPrefsChangesLostForFile_selection", "Settings", Kind::Int, int(0)),
            ("NoSyncPromptBeforeLoadingPrefsFromURL", "Settings", Kind::String, text("")),
            ("HotkeyMigratedFromSingleToMulti", "Hotkey", Kind::Bool, bool(false)),
            ("NoSyncSuppressedAlertsCatalog", "Warnings", Kind::Dict, Some(toml::Value::Table(toml::Table::new()))),
            ("NoSyncNeverAskAboutSettingAlternateMouseScroll", "Warnings", Kind::Bool, bool(false)),
            ("NoSyncAppSupportMoved", "Warnings", Kind::Bool, bool(false)),
            ("NoSyncAutoProvideConsent", "AI", Kind::Int, int(0)),
            ("DisableTransparencyForKeyWindow", "Transparency", Kind::Bool, bool(false)),
            ("Size Changes Affect Profile", "Windows", Kind::Bool, bool(false)),
            ("Custom Color Presets", "Colors", Kind::Dict, Some(toml::Value::Table(toml::Table::new()))),
            ("NoSyncProfilesWindowTagsOpen", "Profiles", Kind::Bool, bool(false)),
            ("CloseBookmarksWindowAfterOpening", "Profiles", Kind::Bool, bool(false)),
        ];
        for (key, section, kind, default) in &expect {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!((s.tab.as_str(), s.section.as_str(), s.kind), (STORED_TAB, *section, *kind), "{key}");
            assert_eq!(&s.default, default, "{key}");
        }
        let stored = schema::global().iter().filter(|s| s.tab == STORED_TAB).count();
        assert_eq!(stored, expect.len());
    }

    fn row(key: &'static str, category: &'static str, help: &'static str, kind: RowKind) -> Row {
        let label = help.split('\n').next().unwrap_or(help);
        Row { key, category, label, subtitle: help.split_once('\n').map(|(_, r)| r), description: help, kind, default: None, macos_only: false }
    }

    fn index(rows: &[Row], key: &str) -> usize {
        rows.iter().position(|r| r.key == key).unwrap_or_else(|| panic!("{key}"))
    }

    fn none(_: &str) -> Option<Value> {
        None
    }

    #[test]
    fn order_follows_iterm2() {
        use std::cmp::Ordering::*;
        // localizedStandardCompare folds case but forces an order between strings that differ
        // only in case (NSForcedOrderingSearch), so they never compare Equal.
        assert_ne!(standard_compare("Tabs", "tabs"), Equal);
        assert_eq!(standard_compare("Tabs", "tabu"), Less);
        assert_eq!(standard_compare("TABU", "tabs"), Greater);
        assert_eq!(standard_compare("Tabs", "Tabs"), Equal);
        assert_eq!(standard_compare("item 2", "item 10"), Less);
        assert_eq!(standard_compare("item 10", "item 9"), Greater);
        assert_eq!(standard_compare("item 02", "item 2"), Greater);
        assert_eq!(standard_compare("Debugging", "Drawing"), Less);
        assert_eq!(standard_compare("Tip of the Day", "Tmux Integration"), Less);
        assert_eq!(standard_compare("Tab", "Tabs"), Less);

        let rows = rows();
        let mut keys: Vec<&str> = rows.iter().map(|r| r.key).collect();
        let mut want: Vec<&str> = schema::global().iter().filter(|s| s.tab == "Advanced").map(|s| s.key.as_str()).collect();
        keys.sort_unstable();
        want.sort_unstable();
        assert_eq!(keys, want);
        let mut seen: Vec<&str> = Vec::new();
        for pair in rows.windows(2) {
            if pair[0].category == pair[1].category {
                assert_ne!(standard_compare(pair[0].description, pair[1].description), Greater, "{} {}", pair[0].key, pair[1].key);
            } else {
                assert_eq!(standard_compare(pair[0].category, pair[1].category), Less);
            }
        }
        for r in &rows {
            if seen.last() != Some(&r.category) {
                assert!(!seen.contains(&r.category), "{} is split", r.category);
                seen.push(r.category);
            }
        }
        let order = ["Drawing", "Experimental Features", "General", "Hotkey", "Pasteboard", "Scripting", "Session", "Tabs", "Terminal", "Tmux Integration", "Warnings", "Windows"];
        let at = |c: &str| seen.iter().position(|s| *s == c).unwrap_or_else(|| panic!("{c}"));
        for pair in order.windows(2) {
            assert!(at(pair[0]) < at(pair[1]), "{} before {}", pair[0], pair[1]);
        }
    }

    #[test]
    fn row_text_follows_iterm2() {
        let rows = rows();
        let get = |key: &str| rows[index(&rows, key)].clone();
        let cookie = get("setCookie");
        assert_eq!(cookie.label, "Set ITERM2_COOKIE environment variable, allowing Python scripts to be launched without confirmation?");
        assert_eq!(cookie.subtitle, Some("This will only affect sessions created after changing this setting."));
        assert_eq!(cookie.description, schema::find_global("setCookie").unwrap().help);
        assert_eq!(cookie.kind, RowKind::Bool);
        assert_eq!(cookie.category, "Scripting");
        assert_eq!(cookie.default, Some(Value::Boolean(false)));
        assert_eq!(get("NoSyncDisableOpenURL").subtitle, None);
        assert_eq!(get("PreventEscapeSequenceFromChangingProfile").kind, RowKind::OptionalBool);
        assert_eq!(get("PreventEscapeSequenceFromChangingProfile").default, None);
        let choices = schema::find_global("anonymousTmuxWindowsOpenInCurrentWindow").unwrap().choices.iter().map(|c| (c.value.clone(), c.label.clone())).collect::<Vec<_>>();
        assert_eq!(choices.len(), 3);
        assert_eq!(get("anonymousTmuxWindowsOpenInCurrentWindow").kind, RowKind::Enum(choices));
        assert_eq!(get("maximumBytesToProvideToPythonAPI").kind, RowKind::Int);
        assert_eq!(get("statusBarHeight").kind, RowKind::Float);
        assert_eq!(get("tmuxTitlePrefix").kind, RowKind::Text);
        let empty_help = row("k", "General", "", RowKind::Bool);
        assert_eq!(empty_help.subtitle, None);
    }

    #[test]
    fn search_follows_iterm2() {
        assert!(matches("Show the tab bar Tabs Tabs", "tab BAR"));
        assert!(matches("a b", "a  b"));
        assert!(!matches("abc", "abd"));
        assert!(matches("x", ""));
        assert!(!matches("x", "x y"));

        let rows = rows();
        let tmux = index(&rows, "tmuxTitlePrefix");
        assert_eq!(searchable(&rows[tmux]), format!("{} Tmux Integration Tmux Integration", rows[tmux].description));
        assert_eq!(filtered(&rows, "tmux titles", false, &none, None), vec![Item::Header("Tmux Integration"), Item::Setting(tmux)]);
        // The key is not searched (iTerm2 matches the description and category only).
        assert_eq!(filtered(&rows, "tmux prefix", false, &none, None), vec![]);
        let mut want = vec![Item::Header("Pasteboard")];
        want.extend(rows.iter().enumerate().filter(|(_, r)| r.category == "Pasteboard").map(|(i, _)| Item::Setting(i)));
        assert!(want.len() > 2);
        // TabsWrapAround's help (iTerm2's) mentions the pasteboard: the description matches too.
        want.push(Item::Header("Terminal"));
        want.push(Item::Setting(index(&rows, "TabsWrapAround")));
        assert_eq!(filtered(&rows, "pasteboard", false, &none, None), want);
        assert_eq!(filtered(&rows, "zzqqxx", false, &none, None), vec![]);

        let fake = [row("a", "One", "alpha", RowKind::Bool), row("b", "One", "beta", RowKind::Bool), row("c", "Two", "alpha two", RowKind::Bool)];
        assert_eq!(filtered(&fake, "alpha", false, &none, None), vec![Item::Header("One"), Item::Setting(0), Item::Header("Two"), Item::Setting(2)]);
        assert_eq!(filtered(&fake, "two", false, &none, None), vec![Item::Header("Two"), Item::Setting(2)]);
        assert_eq!(filtered(&fake, "", false, &none, None).len(), 5);
    }

    #[test]
    fn non_default_filter_follows_iterm2() {
        let b = Value::Boolean;
        let f = Value::Float;
        assert!(!is_non_default(None, Some(&b(false))));
        assert!(!is_non_default(None, None));
        assert!(!is_non_default(Some(&b(false)), Some(&b(false))));
        assert!(is_non_default(Some(&b(true)), Some(&b(false))));
        assert!(!is_non_default(Some(&f(0.25005)), Some(&f(0.25))));
        assert!(is_non_default(Some(&f(0.251)), Some(&f(0.25))));
        // A difference of exactly EPSILON is not approximately equal (`fabs(diff) < epsilon`).
        assert!(is_non_default(Some(&f(EPSILON)), Some(&f(0.0))));
        assert!(!is_non_default(Some(&Value::Integer(5)), Some(&f(5.0))));
        assert!(is_non_default(Some(&Value::Integer(6)), Some(&Value::Integer(5))));
        assert!(is_non_default(Some(&b(false)), None));
        assert!(!is_non_default(Some(&Value::String("x".into())), Some(&Value::String("x".into()))));
        assert!(is_non_default(Some(&Value::String("y".into())), Some(&Value::String("x".into()))));

        let rows = rows();
        let height = index(&rows, "statusBarHeight");
        let stored = |k: &str| (k == "statusBarHeight").then_some(f(22.0));
        assert_eq!(filtered(&rows, "", true, &stored, None), vec![Item::Header("General"), Item::Setting(height)]);
        let git = index(&rows, "gitTimeout");
        assert_eq!(filtered(&rows, "", true, &none, Some("gitTimeout")), vec![Item::Header("General"), Item::Setting(git)]);
        // The row being edited is kept past the non-default filter only; the query still applies.
        assert_eq!(filtered(&rows, "tmux titles", true, &none, Some("gitTimeout")), vec![]);
    }

    #[test]
    fn text_values_follow_iterm2() {
        assert_eq!(parse_int("12abc"), 12);
        assert_eq!(parse_int("abc"), 0);
        assert_eq!(parse_int(" -3"), -3);
        assert_eq!(parse_int("+7"), 7);
        assert_eq!(parse_int(""), 0);
        assert_eq!(parse_int("99999999999999999999"), i64::MAX);
        assert_eq!(parse_int("-99999999999999999999"), i64::MIN);
        assert_eq!(parse_float("1e3"), 1000.0);
        assert_eq!(parse_float(".5"), 0.5);
        assert_eq!(parse_float("x"), 0.0);
        assert_eq!(parse_float("2.5kg"), 2.5);
        assert_eq!(parse_float("-"), 0.0);
        assert_eq!(parse_float(" -1.5"), -1.5);
        assert_eq!(parse_float("3e"), 3.0);
        assert_eq!(parse_float("inf"), 0.0);
        assert_eq!(parse_float("4."), 4.0);
        assert_eq!(format_float(10.0), "10");
        assert_eq!(format_float(0.25), "0.25");
        assert_eq!(format_float(0.125), "0.125");
    }

    /// Every Advanced row carries iTerm2's description (`iTermAdvancedSettingsModel.m`) with
    /// "iTerm2" read as "Tandem": `label` is its first line and `help` the whole of it.
    #[test]
    fn advanced_texts_follow_iterm2() {
        for s in schema::global().iter().filter(|s| s.tab == "Advanced") {
            assert_eq!(s.help.split('\n').next(), Some(s.label.as_str()), "{}", s.key);
        }
        let text = |key: &str| {
            let s = schema::find_global(key).unwrap_or_else(|| panic!("{key}"));
            (s.label.as_str(), s.help.as_str())
        };
        assert_eq!(
            text("statusBarHeight"),
            ("Height of the status bar in points.", "Height of the status bar in points.\nThis will also affect the height of per-pane title bars becuase the status bar may be embedded in it. A change here takes effect at once.")
        );
        assert_eq!(
            text("anonymousTmuxWindowsOpenInCurrentWindow"),
            (
                "Where should new tmux windows not created by Tandem open?",
                "Where should new tmux windows not created by Tandem open?\n\u{201c}Focused window\u{201d} (the default) attaches the window as a tab of whichever Tandem window currently has keyboard focus. \u{201c}Topmost session window\u{201d} attaches it to the frontmost Tandem window that already shows a tab from this tmux session, regardless of focus."
            )
        );
        assert_eq!(
            text("killJobsInServersOnQuit"),
            ("User-initiated Quit (Win+Q) of Tandem will kill all running jobs.", "User-initiated Quit (Win+Q) of Tandem will kill all running jobs.\nApplies only when session restoration is on.")
        );
        assert_eq!(
            text("fileDropCoprocess").1,
            "When files are dropped into a terminal window, execute a silent coprocess.\nThis is an interpolated string. Use \\(filenames) to reference the shell-quoted, space-delimited full paths to the dropped files. If this preference is empty, the filenames get pasted instead."
        );
        assert_eq!(text("disableTmuxWindowResizing"), ("Don't automatically resize tmux windows", "Don't automatically resize tmux windows"));
        // iTerm2's description ends in a newline with nothing after it.
        assert_eq!(text("PasteHistoryMaxOptions").1, "Number of entries to save in Paste History.\n");
        // Where Tandem behaves differently on Windows, the help says what Tandem does.
        assert_eq!(
            text("gitSearchPath"),
            (
                "PATH used when running git for the status bar component.",
                "PATH used when running git for the status bar component.\nChange this to use a custom install of git. The first folder in this ;-separated list holds the git.exe the status bar runs; empty runs git from PATH. A change here takes effect at once."
            )
        );
        assert_eq!(
            text("timeoutForDaemonAttachment").1,
            "How long to wait when trying to attach to the session host at startup when restoring windows (in seconds)?"
        );
        // The Windows folder stands in for iTerm2's ~/.config/iterm2.
        assert_eq!(text("PreferredBaseDir").1, "Folder for config files. There must not be a space in the path.\nIf empty, then %APPDATA%\\Tandem will be the default location.");
    }

    #[test]
    fn texts_follow_iterm2() {
        assert_eq!(ONLY_NON_DEFAULT, "Show only non-default values");
        assert_eq!(ONLY_NON_DEFAULT_TIP, "When checked, removes items from the list whose value equals the default value.");
        assert_eq!(SETTING_COLUMN, "Setting");
        assert_eq!(VALUE_COLUMN, "Value");
        assert_eq!(SEARCH_HINT, "Search");
        assert_eq!(NO_YES, ["No", "Yes"]);
        assert_eq!(TRISTATE, ["Unspecified", "No", "Yes"]);
        assert_eq!(tristate_index(None), 0);
        assert_eq!(tristate_index(Some(false)), 1);
        assert_eq!(tristate_index(Some(true)), 2);
    }
}
