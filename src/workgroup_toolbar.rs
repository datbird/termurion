//! The session toolbar (milestone 13y1, Task 5): the pure side. Which items a member's strip
//! shows, where they sit across its 40 points, who owns a key, and the AI Usage item's report.
//! The app side (drawing, clicks, the usage runner) is `app/workgroup_toolbar_ui.rs`.
//!
//! Citations: `STV` `iTermSessionToolbarView.swift`, `WTB` `WorkgroupToolbarBuilder.swift`,
//! `WUT` `WorkgroupUsageToolbarItem.swift`, `WI` `iTermWorkgroupInstance.swift`, `claude_usage.sh`
//! under `~/gitrepos/iterm2-ref/sources`.

use crate::workgroup::{SessionConfig, ToolbarItem};

/// A member's toolbar is 40 points high (`SessionView.m:57-59`).
pub const TOOLBAR_H: f32 = 40.0;
/// Space in from each side of the strip (`STV:187`).
pub const INSET: f32 = 8.0;
/// Padding on each side of a divider (`STV:190`).
pub const DIVIDER_PAD: f32 = 8.0;
/// Space above and below a divider (`STV:195`).
pub const DIVIDER_V_INSET: f32 = 8.0;
/// The divider's thickness (`STV:188`).
pub const DIVIDER_W: f32 = 1.0;
/// The gap after every item: padding, the divider, padding (`spacerWidth`, `STV:263`).
pub const GAP: f32 = DIVIDER_PAD * 2.0 + DIVIDER_W;
/// The most characters of a program's or session's text the strip shows.
pub const MAX_TEXT: usize = 64;

/// Text a session, a program or a command supplies, cleaned of control, bidirectional and invisible
/// characters and cut to `MAX_TEXT` (P19).
pub fn shown_text(s: &str) -> String {
    crate::inert_files::display_text(s, MAX_TEXT, false)
}

/// The items a session's strip is built from (`WorkgroupToolbarBuilder.injectAutoItems`,
/// `WTB:101-136`): a stored Name never counts, navigation and the base selector need the
/// changed-file selector (without it they are dead buttons), and the Name label goes first unless a
/// mode switcher is there (the switcher shows the active peer's name).
pub fn inject_name(items: &[ToolbarItem]) -> Vec<ToolbarItem> {
    let has_files = items.contains(&ToolbarItem::ChangedFileSelector);
    let mut out: Vec<ToolbarItem> = items
        .iter()
        .filter(|i| match i {
            ToolbarItem::Name => false,
            ToolbarItem::Navigation(_) | ToolbarItem::GitBaseSelector => has_files,
            _ => true,
        })
        .cloned()
        .collect();
    if !out.contains(&ToolbarItem::ModeSwitcher) {
        out.insert(0, ToolbarItem::Name);
    }
    out
}

/// The strip of one member (`WI:526-532`, `WPP:135-146`): its own items with the Name injected. A peer
/// group member (the main session or a peer) keeps its Mode Switcher; a split or a tab never has
/// one (it would swap the leader's slot from another pane).
pub fn items_for(member: &SessionConfig) -> Vec<ToolbarItem> {
    use crate::workgroup::Kind;
    if matches!(member.kind, Kind::Root | Kind::Peer) {
        inject_name(&member.toolbar_items)
    } else {
        let own: Vec<ToolbarItem> = member.toolbar_items.iter().filter(|i| **i != ToolbarItem::ModeSwitcher).cloned().collect();
        inject_name(&own)
    }
}

/// One item's place across the strip.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub item: usize,
    pub x: f32,
    pub w: f32,
}

/// Where each item sits in a strip `width` wide. `widths` has one entry per item: its fixed width, and
/// for an item that may stretch (a spacer, the usage bars) the range its width stays in. Fixed widths
/// and the minimum of each range come first; what is left grows the stretching items equally up to
/// their maximum. A gap (padding, a divider, padding) follows every item. The result is the natural
/// geometry: what runs past the right edge is clipped by `visible_width` (`STV:200-250`).
pub fn layout(widths: &[(f32, Option<(f32, f32)>)], width: f32) -> Vec<Placed> {
    let mut w: Vec<f32> = widths.iter().map(|(fixed, range)| range.map_or(*fixed, |(min, _)| min)).collect();
    let gaps = widths.len().saturating_sub(1) as f32 * GAP;
    let mut left = width - 2.0 * INSET - gaps - w.iter().sum::<f32>();
    let mut open: Vec<usize> = (0..widths.len()).filter(|&i| widths[i].1.is_some_and(|(min, max)| max > min)).collect();
    // Equal shares until every stretching item has what it can take or nothing is left.
    while left > 0.0 && !open.is_empty() {
        let share = left / open.len() as f32;
        let mut used = 0.0;
        for &i in &open {
            let max = widths[i].1.map_or(w[i], |(_, max)| max);
            let add = share.min(max - w[i]);
            w[i] += add;
            used += add;
        }
        left -= used;
        open.retain(|&i| widths[i].1.is_some_and(|(_, max)| w[i] < max));
        if used <= 0.0 {
            break;
        }
    }
    let mut x = INSET;
    w.iter()
        .enumerate()
        .map(|(item, &w)| {
            let p = Placed { item, x, w };
            x += w + GAP;
            p
        })
        .collect()
}

/// How much of `p` shows in a strip `width` wide: None when it starts past the right inset.
pub fn visible_width(p: &Placed, width: f32) -> Option<f32> {
    let room = width - INSET - p.x;
    (room > 0.0).then(|| p.w.min(room))
}

/// Whether the toolbar takes a key: a global binding wins (`WI:1696-1700`).
pub fn shortcut_owner(global_bound: bool, toolbar_match: bool) -> bool {
    !global_bound && toolbar_match
}

/// Who runs a key pressed in a workgroup member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Global,
    PeerShortcut,
    PeerDigit,
    Toolbar,
    Menu,
}

/// The one order for keys in a workgroup member (P13): the global binding, a peer's custom
/// shortcut, Alt+Shift+Win+digit, a toolbar item's shortcut, then the main menu.
pub fn key_owner(global_bound: bool, peer_shortcut: bool, peer_digit: bool, toolbar_match: bool) -> Owner {
    if global_bound {
        Owner::Global
    } else if peer_shortcut {
        Owner::PeerShortcut
    } else if peer_digit {
        Owner::PeerDigit
    } else if shortcut_owner(false, toolbar_match) {
        Owner::Toolbar
    } else {
        Owner::Menu
    }
}

// ----- AI Usage (`WUT`, `claude_usage.sh`) -----

pub const USAGE_LOADING: &str = "AI usage\u{2026}";
pub const USAGE_UNAVAILABLE: &str = "AI usage unavailable";
pub const USAGE_NO_OUTPUT: &str = "No usage data";
pub const USAGE_BAD_OUTPUT: &str = "Unreadable usage data";
pub const USAGE_SCRIPT_MISSING: &str = "The AI usage helper is missing from Tandem. This is a bug.";
pub const USAGE_CLICK_FOR_DETAILS: &str = "(click for details)";
/// The script's text when `claude` is not on the path.
pub const NOT_FOUND: &str = "The Claude Code command-line tool (claude) was not found in your PATH.";
/// The glyph a failed item shows before its summary (Segoe Fluent Icons Bug; iTerm2 shows a ladybug).
pub const ERROR_GLYPH: char = '\u{EBE8}';
/// The most bytes a usage command's output is read to (`WUT:376`).
pub const OUTPUT_CAP: usize = 1024 * 1024;
/// The least seconds between runs (`WUT`: `max(5, intervalSeconds)`).
pub const MIN_INTERVAL: f64 = 5.0;
/// Where a bug report goes.
const ISSUES: &str = "https://github.com/datbird/tandem/issues/new";
/// The report body a link carries, at most (`maxURLDescription`).
const MAX_URL_BODY: usize = 2000;

#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
pub struct UsageBar {
    pub label: String,
    #[serde(default)]
    pub short: Option<String>,
    pub fraction: f64,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct UsageReport {
    #[serde(default)]
    pub bars: Vec<UsageBar>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub diagnostic: Option<String>,
    #[serde(default)]
    pub reportable: Option<bool>,
}

fn failure(error: &str, reportable: bool, diagnostic: Option<String>) -> UsageReport {
    UsageReport { bars: Vec::new(), error: Some(error.to_string()), diagnostic, reportable: Some(reportable) }
}

/// The fraction a bar draws: held to 0..1 so a bad command cannot blow out the geometry.
pub fn clamped(bar: &UsageBar) -> f64 {
    if bar.fraction.is_finite() { bar.fraction.clamp(0.0, 1.0) } else { 0.0 }
}

/// "41%" for a bar.
pub fn percent(bar: &UsageBar) -> String {
    format!("{}%", (clamped(bar) * 100.0).round() as i64)
}

/// What a usage command printed (`decode`, `WUT:438-465`): nothing is "No usage data" and not
/// reportable (a slow start); output that is not the contract is reportable with the raw text.
pub fn decode_usage(stdout: &[u8]) -> UsageReport {
    if stdout.is_empty() {
        return failure(USAGE_NO_OUTPUT, false, None);
    }
    match serde_json::from_slice::<UsageReport>(&stdout[..stdout.len().min(OUTPUT_CAP)]) {
        Ok(r) => r,
        Err(_) => failure(USAGE_BAD_OUTPUT, true, Some(String::from_utf8_lossy(&stdout[..stdout.len().min(MAX_DIAGNOSTIC)]).into_owned())),
    }
}

/// The diagnostic a report keeps, in bytes.
const MAX_DIAGNOSTIC: usize = 16 * 1024;

/// The bar's caption: its `short`, else initials of its label (`WUT:653-675`, `derivedShortLabel`).
pub fn short_label(bar: &UsageBar) -> String {
    if let Some(s) = bar.short.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return s.to_string();
    }
    let words: Vec<&str> = bar.label.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    match words.as_slice() {
        [] => "?".to_string(),
        [one] => one.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default(),
        [a, b, ..] => [a, b].iter().filter_map(|w| w.chars().next()).flat_map(char::to_uppercase).collect(),
    }
}

/// A bar's tooltip: "Label: detail".
pub fn usage_tooltip(label: &str, detail: Option<&str>) -> String {
    match detail.filter(|d| !d.is_empty()) {
        Some(d) => format!("{label}: {d}"),
        None => label.to_string(),
    }
}

/// The report when `claude` is not on the path (not reportable).
pub fn not_found() -> UsageReport {
    failure(NOT_FOUND, false, None)
}

/// The text of `claude -p /usage`, as `claude_usage.sh` reads it: lines holding "% used" that start
/// "Current session:" or "Current week (...):", the number before "% used" over 100, the text from
/// "resets" as the detail. With no bars the failure is classified (`claude_usage.sh:200-300`).
pub fn parse_claude_usage(text: &str) -> UsageReport {
    let bars: Vec<UsageBar> = text.lines().filter_map(usage_line).collect();
    if !bars.is_empty() {
        return UsageReport { bars, error: None, diagnostic: None, reportable: None };
    }
    let diag = |t: &str| Some(t.lines().take(60).collect::<Vec<_>>().join("\n")).filter(|d| !d.is_empty());
    if text.contains("% used") || text.contains("Current session") || text.contains("Current week") {
        return failure("Could not read the Claude usage report. Tandem may need to be updated.", true, diag(text));
    }
    if text.contains("API key") || text.contains("api key") || text.contains("ANTHROPIC_API_KEY") || text.contains("api_key") {
        return failure("AI usage needs a Claude subscription (Pro or Max); it isn\u{2019}t available for API-billing accounts.", false, None);
    }
    if text.contains("subscription") || text.contains("Subscription") {
        return failure("AI usage is available only on a Claude subscription (Pro or Max).", false, None);
    }
    if text.contains("nknown command") || text.contains("available commands") || text.contains("not a valid") {
        return failure("Update Claude Code to a version that supports usage reporting.", false, None);
    }
    if ["log in", "Log in", "login", "Login", "sign in", "signed in", "uthenticat"].iter().any(|k| text.contains(k)) {
        return failure("Sign in to Claude Code with a Pro or Max plan to see AI usage.", false, None);
    }
    if ["rate limit", "Rate limit", "overloaded", "529", "429"].iter().any(|k| text.contains(k)) {
        return failure("Claude usage is temporarily unavailable. Try again later.", false, None);
    }
    if text.trim().is_empty() {
        return failure("Claude Code produced no usage output.", false, None);
    }
    failure("Could not read Claude usage. If this keeps happening, please report it.", true, diag(text))
}

/// One line of the report as a bar, None when it is not one.
fn usage_line(line: &str) -> Option<UsageBar> {
    let used = line.find("% used")?;
    let (label, short) = if line.starts_with("Current session:") {
        ("Session".to_string(), "S".to_string())
    } else if line.starts_with("Current week (all models):") {
        ("Week".to_string(), "W".to_string())
    } else if let Some(rest) = line.strip_prefix("Current week (") {
        let inner = rest.split("):").next().unwrap_or(rest);
        // `W` and the model's first ASCII letter or digit, uppercased; none leaves the short empty
        // for the caption's own fallback.
        let initial = inner.trim_start_matches(|c: char| !c.is_ascii_alphanumeric()).chars().next().filter(char::is_ascii_alphanumeric);
        (format!("Week ({inner})"), initial.map_or(String::new(), |c| format!("W{}", c.to_ascii_uppercase())))
    } else {
        return None;
    };
    // The number ending just before "% used": digits with an optional fraction.
    let before = &line[..used];
    let digits = before.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    let num = &before[digits.len()..];
    let fraction = num.parse::<f64>().ok()?.max(0.0) / 100.0;
    let detail = line.find("resets").map(|i| line[i..].trim().to_string()).unwrap_or_default();
    Some(UsageBar { label, short: Some(short), fraction, detail: Some(detail) })
}

/// The buttons of the report alert: Cancel first (the default), then Report Bug.
pub fn report_buttons() -> [&'static str; 2] {
    ["Cancel", "Report Bug"]
}

/// The alert's text above the report (`WUT:541-570`).
pub const REPORT_GUIDANCE: &str = "Tandem couldn\u{2019}t read the AI usage report. Updating Tandem to the latest version may fix this. If you\u{2019}re already up to date, click \u{201c}Report Bug\u{201d} to open a pre-filled report. The full details below are also copied to your clipboard.";

/// The clipboard text of a report, markdown like the AI Usage issue template (`WUT:572-600`).
pub fn report_text(version: &str, r: &UsageReport, custom_command: bool) -> String {
    let summary = r.error.as_deref().unwrap_or(USAGE_UNAVAILABLE);
    let mut lines = vec![
        "## What happened".to_string(),
        String::new(),
        summary.to_string(),
        String::new(),
        "## Environment".to_string(),
        String::new(),
        format!("- Tandem version: {version}"),
        format!("- Provider: {}", crate::workgroup::UsageProvider::AnthropicClaudeCode.display_name()),
        format!("- Custom command: {}", if custom_command { "yes" } else { "no (built in)" }),
    ];
    if let Some(d) = r.diagnostic.as_deref().filter(|d| !d.is_empty()) {
        lines.extend([String::new(), "## Diagnostic".to_string(), String::new(), "```".to_string(), d.to_string(), "```".to_string()]);
    }
    lines.join("\n")
}

/// The new-issue link with the report pre-filled, the body cut to `MAX_URL_BODY` characters (the
/// clipboard has all of it).
pub fn bug_report_url(title: &str, body: &str) -> String {
    let cut: String = if body.chars().count() > MAX_URL_BODY {
        body.chars().take(MAX_URL_BODY).chain("\n\n(truncated; the full report is on your clipboard)".chars()).collect()
    } else {
        body.to_string()
    };
    format!("{ISSUES}?title={}&body={}", crate::semantic::percent_encode(title), crate::semantic::percent_encode(&cut))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workgroup::{claude_code, Kind, SessionConfig, ToolbarItem};

    fn member(items: Vec<ToolbarItem>, kind: Kind) -> SessionConfig {
        let mut s = claude_code::config().root().expect("root").clone();
        s.kind = kind;
        s.toolbar_items = items;
        s
    }

    /// Review Focus 4: the strip's items, the name injected, the layout across the width.
    #[test]
    fn toolbar_items_follow_iterm2() {
        use ToolbarItem::{GitStatus, ModeSwitcher, Name, Reload};
        // `WTB:101-136` (the source wins over the brief): a stored Name is dropped, and the Name
        // goes first only when no mode switcher is there (the switcher shows the active peer).
        assert_eq!(inject_name(&[ModeSwitcher, Name, GitStatus]), vec![ModeSwitcher, GitStatus]);
        assert_eq!(inject_name(&[GitStatus, Name]), vec![Name, GitStatus]);
        assert_eq!(inject_name(&[]), vec![Name]);
        // Navigation and the base selector are dead buttons without a changed-file selector.
        let nav = ToolbarItem::Navigation(crate::workgroup::NavigationShortcuts::defaults());
        assert_eq!(inject_name(&[nav.clone(), ToolbarItem::GitBaseSelector, Reload(None)]), vec![Name, Reload(None)]);
        assert_eq!(inject_name(&[ToolbarItem::ChangedFileSelector, nav.clone(), ToolbarItem::GitBaseSelector]), vec![Name, ToolbarItem::ChangedFileSelector, nav, ToolbarItem::GitBaseSelector]);
        // A split or tab never has a switcher; a peer keeps its own items, never another peer's.
        let split = member(vec![ModeSwitcher, Reload(None)], Kind::Tab);
        assert_eq!(items_for(&split), vec![Name, Reload(None)]);
        let peer = member(vec![ModeSwitcher, Name, GitStatus], Kind::Peer);
        assert_eq!(items_for(&peer), vec![ModeSwitcher, GitStatus]);
        let chat = member(vec![ModeSwitcher, GitStatus], Kind::Root);
        assert_eq!(items_for(&chat), vec![ModeSwitcher, GitStatus], "Chat shown with a Diff peer that carries Reload: no Reload");
        let bare = member(vec![GitStatus], Kind::Peer);
        assert_eq!(items_for(&bare), vec![Name, GitStatus], "a peer without a switcher gets the Name");
        // The constants are the source's (`STV:187-196`, `SessionView.m:57-59`).
        assert_eq!((TOOLBAR_H, INSET, DIVIDER_PAD, DIVIDER_V_INSET), (40.0, 8.0, 8.0, 8.0));
        assert_eq!(GAP, 17.0, "padding, a 1 point divider, padding");
        // `layout`: fixed widths, a spacer between its bounds, a gap after every item.
        let items = [(60.0, None), (40.0, None), (0.0, Some((8.0, 100.0)))];
        let wide = layout(&items, 300.0);
        assert_eq!(wide, vec![Placed { item: 0, x: 8.0, w: 60.0 }, Placed { item: 1, x: 85.0, w: 40.0 }, Placed { item: 2, x: 142.0, w: 100.0 }]);
        let narrow = layout(&items, 120.0);
        assert_eq!(narrow[2].w, 8.0, "the spacer gives way to its minimum");
        assert_eq!((narrow[0].w, narrow[1].w), (60.0, 40.0), "fixed items keep their widths");
        assert_eq!(visible_width(&narrow[1], 120.0), Some(27.0), "the rest is clipped at the right");
        assert_eq!(visible_width(&narrow[2], 120.0), None, "past the edge");
        assert_eq!(visible_width(&narrow[0], 120.0), Some(60.0));
        // Two growable items share what is left, each held to its bounds.
        let two = layout(&[(0.0, Some((8.0, 40.0))), (0.0, Some((8.0, 200.0)))], 200.0);
        assert_eq!((two[0].w, two[1].w), (40.0, 200.0 - 16.0 - 17.0 - 40.0));
        assert!(layout(&[], 100.0).is_empty());
        // Everything the strip shows from a program or a session goes through `display_text` and a cap.
        assert_eq!(shown_text("a\u{202e}b\u{7}c"), "abc");
        assert_eq!(shown_text(&"x".repeat(500)).chars().count(), MAX_TEXT);
        // Global bindings win over the toolbar, which wins over the menu (`WI:1696-1700`).
        assert!(!shortcut_owner(true, true));
        assert!(shortcut_owner(false, true));
        assert!(!shortcut_owner(false, false));
    }

    #[test]
    fn key_owner_is_one_order() {
        use Owner::*;
        assert_eq!(key_owner(true, true, true, true), Global);
        assert_eq!(key_owner(false, true, true, true), PeerShortcut);
        assert_eq!(key_owner(false, false, true, true), PeerDigit);
        assert_eq!(key_owner(false, false, false, true), Toolbar);
        assert_eq!(key_owner(false, false, false, false), Menu);
    }

    fn json(text: &str) -> Vec<u8> {
        text.as_bytes().to_vec()
    }

    #[test]
    fn usage_follows_iterm2() {
        let r = decode_usage(br#"{"bars":[{"label":"Session","short":"S","fraction":0.02,"detail":"resets 5pm"}],"error":null}"#);
        assert_eq!(r.bars.len(), 1);
        assert_eq!((r.bars[0].label.as_str(), r.bars[0].short.as_deref(), r.bars[0].fraction), ("Session", Some("S"), 0.02));
        assert_eq!(r.error, None);
        let empty = decode_usage(b"");
        assert_eq!((empty.error.as_deref(), empty.reportable), (Some(USAGE_NO_OUTPUT), Some(false)));
        let bad = decode_usage(b"nope");
        assert_eq!((bad.error.as_deref(), bad.reportable, bad.diagnostic.as_deref()), (Some(USAGE_BAD_OUTPUT), Some(true), Some("nope")));
        // A fraction outside 0..1 cannot blow out the bar.
        let big = decode_usage(&json(r#"{"bars":[{"label":"x","fraction":7.5}]}"#));
        assert_eq!(clamped(&big.bars[0]), 1.0);
        assert_eq!(percent(&big.bars[0]), "100%");
        // A caption without `short` is derived from the label (`WUT:653-675`).
        let bar = |label: &str, short: Option<&str>| UsageBar { label: label.into(), short: short.map(str::to_string), fraction: 0.5, detail: None };
        assert_eq!(short_label(&bar("Session", None)), "S");
        assert_eq!(short_label(&bar("Week (Opus)", None)), "WO");
        assert_eq!(short_label(&bar("Weekly", Some("  "))), "W", "a blank short is no short");
        assert_eq!(short_label(&bar("Weekly", Some("Wk"))), "Wk");
        assert_eq!(short_label(&bar("!!!", None)), "?");
        assert_eq!(usage_tooltip("Week", Some("resets Mon")), "Week: resets Mon");
        assert_eq!(usage_tooltip("Week", None), "Week");
        assert_eq!(usage_tooltip("Week", Some("")), "Week");
        let text = "Current session: 2% used \u{b7} resets 5pm\nCurrent week (all models): 41% used \u{b7} resets Mon\nCurrent week (Opus): 12.5% used \u{b7} resets Mon\n";
        let r = parse_claude_usage(text);
        let got: Vec<(&str, &str, f64, &str)> = r.bars.iter().map(|b| (b.label.as_str(), b.short.as_deref().unwrap_or(""), b.fraction, b.detail.as_deref().unwrap_or(""))).collect();
        assert_eq!(got, vec![("Session", "S", 0.02, "resets 5pm"), ("Week", "W", 0.41, "resets Mon"), ("Week (Opus)", "WO", 0.125, "resets Mon")]);
        assert_eq!(r.error, None);
        // A non-Latin model has no ASCII initial: an empty short, derived later.
        let r = parse_claude_usage("Current week (\u{30e2}\u{30c7}\u{30eb}): 10% used resets Tue\n");
        assert_eq!(r.bars[0].short.as_deref(), Some(""));
        // The classification of no bars (`claude_usage.sh:200-300`).
        let none = parse_claude_usage("");
        assert_eq!((none.bars.len(), none.error.as_deref(), none.reportable), (0, Some("Claude Code produced no usage output."), Some(false)));
        let drift = parse_claude_usage("Current session: lots used\n");
        assert_eq!((drift.error.as_deref(), drift.reportable), (Some("Could not read the Claude usage report. Tandem may need to be updated."), Some(true)));
        assert!(drift.diagnostic.as_deref().unwrap().contains("lots used"));
        let nope = parse_claude_usage("nope");
        assert_eq!((nope.error.as_deref(), nope.reportable), (Some("Could not read Claude usage. If this keeps happening, please report it."), Some(true)));
        let key = parse_claude_usage("Error: invalid API key");
        assert_eq!((key.error.as_deref(), key.reportable), (Some("AI usage needs a Claude subscription (Pro or Max); it isn\u{2019}t available for API-billing accounts."), Some(false)));
        assert_eq!(parse_claude_usage("a Subscription is needed").error.as_deref(), Some("AI usage is available only on a Claude subscription (Pro or Max)."));
        assert_eq!(parse_claude_usage("Unknown command: /usage").error.as_deref(), Some("Update Claude Code to a version that supports usage reporting."));
        assert_eq!(parse_claude_usage("please log in").error.as_deref(), Some("Sign in to Claude Code with a Pro or Max plan to see AI usage."));
        assert_eq!(parse_claude_usage("429 too many").error.as_deref(), Some("Claude usage is temporarily unavailable. Try again later."));
        assert_eq!(NOT_FOUND, "The Claude Code command-line tool (claude) was not found in your PATH.");
        let nf = not_found();
        assert_eq!((nf.error.as_deref(), nf.reportable), (Some(NOT_FOUND), Some(false)));
        // The diagnostic keeps 60 lines.
        let many = "Current session: x\n".repeat(100);
        assert_eq!(parse_claude_usage(&many).diagnostic.unwrap().lines().count(), 60);
    }

    #[test]
    fn usage_report_text_and_url() {
        let r = UsageReport { bars: Vec::new(), error: Some("It broke".into()), diagnostic: Some("raw\ntext".into()), reportable: Some(true) };
        let text = report_text("0.1.0", &r, false);
        assert!(text.starts_with("## What happened\n\nIt broke\n"));
        assert!(text.contains("- Tandem version: 0.1.0") && text.contains("- Custom command: no (built in)"));
        assert!(text.contains("## Diagnostic\n\n```\nraw\ntext\n```"));
        assert!(report_text("1", &r, true).contains("- Custom command: yes"));
        let url = bug_report_url("It broke", &"y".repeat(5000));
        assert!(url.starts_with("https://github.com/datbird/tandem/issues/new?title=It%20broke&body="));
        assert!(url.len() < 4000 && !url.contains(' '), "the link carries a bounded, encoded body");
        assert_eq!(USAGE_CLICK_FOR_DETAILS, "(click for details)");
        assert_eq!(report_buttons(), ["Cancel", "Report Bug"]);
    }
}
