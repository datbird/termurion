//! The guard for Settings wording: every schema row with a control holds iTerm2's label, taken from
//! its xib by `scripts/gen-ui-labels.py` into `schema/ui_labels.toml`, and every row draws a control.
//!
//! The only rewrites allowed are the Mac words Termurion maps (`mac_words`); a label that has to
//! differ for another Windows reason is in `OVERRIDES` with that reason, and a row iTerm2 draws no
//! label for (or has no such control) is in `EXEMPT` with why. Both tables are checked for rot.

#![cfg(test)]

use serde::Deserialize;

#[derive(Deserialize)]
struct Table {
    control: Vec<Control>,
}

#[derive(Deserialize)]
struct Control {
    scope: String,
    key: String,
    label: String,
    source: String,
    #[serde(default)]
    choices: Vec<(String, String)>,
    pane: Option<String>,
    top: Option<f64>,
    left: Option<f64>,
    item_index: Option<i64>,
}

fn table() -> &'static Vec<Control> {
    static T: std::sync::OnceLock<Vec<Control>> = std::sync::OnceLock::new();
    T.get_or_init(|| toml::from_str::<Table>(include_str!("../schema/ui_labels.toml")).expect("schema/ui_labels.toml must parse").control)
}

/// iTerm2's wording with the Mac words Termurion maps (the menu rule: Finder is Explorer, Cmd is
/// Win, Option is Alt, Control is Ctrl, Return is Enter, and the app is Termurion).
pub fn mac_words(label: &str) -> String {
    let mut s = label.to_string();
    for (from, to) in [
        ("option (\u{2325})", "Alt"),
        ("Option (\u{2325})", "Alt"),
        ("command (\u{2318})", "Win"),
        ("Command (\u{2318})", "Win"),
        ("control (\u{2303})", "Ctrl"),
        ("Control (\u{2303})", "Ctrl"),
        ("\u{2318}", "Win"),
        ("\u{2325}", "Alt"),
        ("\u{2303}", "Ctrl"),
        ("^-", "Ctrl-"),
        ("Finder", "Explorer"),
        ("iTerm2", "Termurion"),
        ("Return", "Enter"),
    ] {
        s = s.replace(from, to);
    }
    s.trim().to_string()
}

/// Labels that differ from `mac_words(iTerm2's label)` for a Windows reason: (scope, key, label, why).
const OVERRIDES: &[(&str, &str, &str, &str)] = &[
    ("profile", "Space", "Desktop:", "Windows calls Spaces virtual desktops"),
    ("global", "PromptOnQuit", "Confirm \"Quit Termurion\"", "iTerm2 names its Cmd+Q chord in the label; Termurion's quit chord is the user's shortcut key"),
    ("global", "RestoreWindowsToSameSpaces", "Restore windows to same desktops", "Windows calls Spaces virtual desktops"),
    ("global", "HideFromDockAndAppSwitcher", "Exclude from taskbar and Alt+Tab switcher", "Windows has a taskbar and Alt+Tab, not a Dock and Cmd+Tab"),
    ("global", "AllowSymbolicHotKeys", "Respect shortcuts from Windows keyboard shortcuts", "iTerm2 names a macOS System Settings path that does not exist on Windows"),
];

/// Rows with no label to hold, in groups: (scope, keys, why). A group is a reason, not a favor.
const EXEMPT: &[(&str, &[&str], &str)] = &[
    (
        "profile",
        &["Custom Icon Path", "Working Directory", "Custom Directory", "AWDS Window Option", "AWDS Window Directory", "AWDS Tab Option", "AWDS Tab Directory", "AWDS Pane Option", "AWDS Pane Directory"],
        "iTerm2 draws these as radio buttons and a text field inside a box or sheet that has no label of their own (the titles are the radios'); the radio titles are held by schema_choices_are_iterm2s",
    ),
    ("profile", &["Badge Font"], "iTerm2's badge font is the sheet's Font button, drawn by the font panel; the row's label is never drawn"),
    ("profile", &["Title Function"], "iTerm2 draws the function popup next to Title: with no label of its own"),
    ("profile", &["SSH", "Session Hotkey", "Status Bar Layout", "Keyboard Map", "Bound Hosts", "Semantic History", "Triggers", "Smart Selection Rules"], "a button, sheet or table whose row label is never drawn; the sheet has its own wording, held by its own tests"),
    ("profile", &["Horizontal Spacing", "Vertical Spacing"], "iTerm2's font picker shows the spacing fields with an icon and no text; the row is drawn by the font panel"),
    (
        "profile",
        &["Columns", "Rows", "Width Percentage", "Height Percentage", "Width in Points", "Height in Points"],
        "iTerm2 draws the window size as one line (a units popup, two fields and 'columns by'); Termurion draws that line in one row and these labels are never drawn",
    ),
    ("profile", &["Custom Window Title", "Custom Tab Title", "Idle Code", "Idle Period"], "iTerm2's text field has no label of its own; it follows its checkbox"),
    ("profile", &["Custom Locale"], "iTerm2's Change Locale button; the row is drawn by the locale button, not by its label"),
    ("profile", &["Render Plugins", "Effects Claude Enter", "Effects Claude Done", "Effects Claude Line Color", "Effects Claude Live Lines", "Effects Claude Prompt", "Effects Signals", "Effects Jumps", "Effects Idle"], "Termurion only: iTerm2 has no render plugins; drawn by the Effects editor (settings_effects)"),
    ("profile", &["Jobs to Ignore"], "iTerm2's Edit button next to 'If there are jobs besides:'; drawn by the jobs list editor"),
    (
        "profile",
        &[
            "HotKey Key Code",
            "HotKey Characters",
            "HotKey Characters Ignoring Modifiers",
            "HotKey Modifier Flags",
            "HotKey Window AutoHides",
            "HotKey Window Reopens On Activation",
            "HotKey Window Animates",
            "HotKey Window Floats",
            "HotKey Window Dock Click Action",
            "HotKey Activated By Modifier",
            "HotKey Modifier Activation",
            "HotKey Alternate Shortcuts",
        ],
        "drawn by the Configure Hotkey Window sheet, whose strings are iTerm2's own (iTermHotkeyPreferencesWindowController.xib)",
    ),
    ("profile", &["Touch Bar Map"], "macOS only (Touch Bar), no Windows equivalent"),
    ("profile", &["Browser Extensions Root", "Browser Extension Active IDs"], "written by the extension approval window, never by a control"),
    ("profile", &["Command"], "iTerm2's command text field shares the 'Command:' label with the popup above it (Custom Command)"),
    ("profile", &["Bold Color"], "iTerm2 draws the bold color well next to the 'Use custom color for bold text' checkbox"),
    (
        "profile",
        &["Ansi 9 Color", "Ansi 10 Color", "Ansi 11 Color", "Ansi 12 Color", "Ansi 13 Color", "Ansi 14 Color", "Ansi 15 Color"],
        "iTerm2's bright ANSI wells have no label: the 'Bright' caption names the column and the name at the row's left is the normal well's",
    ),
    ("global", &["GlobalKeyMap", "PointerActions", "Leader", "ShortcutKey", "KeyboardLocale"], "a table or a Termurion-only row on the Keys or Pointer page; its label is drawn by that page's own code"),
    ("global", &["HotkeyCode", "HotkeyChar", "HotkeyModifiers", "OpenQuicklyHotkeyCode", "OpenQuicklyHotkeyChar", "OpenQuicklyHotkeyModifiers"], "drawn by the App Hotkey and Open Quickly Hotkey recorders, which are iTerm2's"),
    ("global", &["Selection Respects Soft Boundaries", "ShowSessionStatusInTabSubtitle", "LeftTabBarWidth", "Default Toolbelt Width", "AutoCommandHistory", "Print In Black And White"], "iTerm2 has no Settings control for this key (an Advanced setting, a menu item, a saved value, or a Termurion row); the label is Termurion's"),
    ("global", &["OpenNoWindowsAtStartup"], "an item of the Window restoration policy popup (Only Restore Hotkey Window), not a row of its own"),
    ("global", &["WordSelectionMode", "CustomScriptsFolder", "PrefsCustomFolder"], "iTerm2's popup or text field has no label of its own; the row beside it carries the title"),
    ("global", &["TmuxDashboardLimit", "OpenTmuxDashboardIfHiddenWindows"], "Termurion's tmux dashboard rows; iTerm2 has no such controls"),
    (
        "global",
        &[
            "EnableAI",
            "AICompletions",
            "AIVendor",
            "UseRecommendedAIModel",
            "AiModel",
            "AiMaxTokens",
            "AiResponseMaxTokens",
            "AitermURL",
            "AitermUseLegacyAPI",
            "AITermAPI",
            "AIFeatureHostedFileSearch",
            "AIFeatureHostedCodeInterpeter",
            "AIFeatureHostedWebSearch",
            "AIFeatureFunctionCalling",
            "AIFeatureStreamingResponses",
            "AIVectorStore",
            "AIManualModelConfigurations",
            "AIEconomyModelName",
            "AIOllamaRegularModel",
            "AIOllamaEconomyModel",
            "AICustomHeadersEnabled",
            "AICustomHeaders",
            "AI Prompt",
            "AI Prompt for AI Chat with no function calling",
            "AI Prompt for AI Chat with ReadOnlyTerminal",
            "AI Prompt for AI Chat with ReadWriteTerminal",
            "AI Prompt for AI Chat with Browser",
            "AI Prompt for AI Chat with ReadOnlyTerminalBrowser",
            "AI Prompt for AI Chat with ReadWriteTerminalBrowser",
            "AI Prompt for AI Chat Orchestration",
            "AI Prompt for Code Review",
            "AI Prompt for Code Review System",
            "AI Prompt for Chat Icon",
        ],
        "iTerm2 builds its AI settings in code (GeneralPreferencesViewController.m), not in a xib; settings_ai.rs draws them with those strings and its own tests hold them",
    ),
];

fn exempt(scope: &str, key: &str) -> bool {
    EXEMPT.iter().any(|(sc, keys, _)| *sc == scope && keys.contains(&key))
}

/// Exempt rows iTerm2 does label, because the label is shared with a neighbor row.
const DUPLICATES: &[&str] = &["Command"];

fn rows() -> Vec<(&'static str, &'static crate::schema::Setting)> {
    let mut v: Vec<(&'static str, &'static crate::schema::Setting)> = Vec::new();
    for s in crate::schema::profile() {
        if s.tab.is_empty() {
            continue;
        }
        v.push(("profile", s));
    }
    // The Advanced page's labels are iTerm2's setting descriptions, held by the Advanced tests.
    for s in crate::schema::global() {
        if s.tab == "Stored" || s.tab == "Advanced" {
            continue;
        }
        v.push(("global", s));
    }
    v
}

fn control_for(scope: &str, key: &str) -> Option<&'static Control> {
    table().iter().find(|c| c.scope == scope && c.key == key)
}

fn expected(scope: &str, key: &str) -> Option<String> {
    if let Some((_, _, label, _)) = OVERRIDES.iter().find(|(s, k, _, _)| *s == scope && *k == key) {
        return Some((*label).to_string());
    }
    control_for(scope, key).filter(|c| !c.label.trim().is_empty()).map(|c| mac_words(&c.label))
}

/// Every schema row with an iTerm2 label wears it, to the letter.
#[test]
fn schema_labels_are_iterm2s() {
    let mut bad = Vec::new();
    for (scope, s) in rows() {
        if exempt(scope, &s.key) {
            continue;
        }
        match expected(scope, &s.key) {
            Some(want) if want == s.label => {}
            Some(want) => bad.push(format!("{scope} {}: have {:?}, iTerm2 has {:?} ({})", s.key, s.label, want, control_for(scope, &s.key).map_or("override", |c| c.source.as_str()))),
            None => bad.push(format!("{scope} {} ({:?}): no iTerm2 label and not exempt", s.key, s.label)),
        }
    }
    assert!(bad.is_empty(), "{} rows differ:\n{}", bad.len(), bad.join("\n"));
}

/// The two tables do not rot: an override differs from the mapped label, an exempt row is not one
/// the xib labels, and each names a row that exists.
#[test]
fn override_and_exempt_tables_stay_honest() {
    for (scope, key, label, why) in OVERRIDES {
        assert!(!why.is_empty(), "{key}");
        let c = control_for(scope, key).unwrap_or_else(|| panic!("override for {scope} {key} has no iTerm2 control"));
        assert_ne!(mac_words(&c.label), *label, "{key}: the override equals the mapped label");
        assert!(rows().iter().any(|(sc, s)| sc == scope && s.key == *key), "{key} is not a schema row");
    }
    for (scope, keys, why) in EXEMPT {
        assert!(!why.is_empty(), "{keys:?}");
        for key in *keys {
            assert!(rows().iter().any(|(sc, s)| sc == scope && s.key == *key), "{scope} {key} is not a schema row");
            assert!(control_for(scope, key).is_none_or(|c| c.label.trim().is_empty() || DUPLICATES.contains(key)), "{scope} {key}: iTerm2 labels it, so it is not exempt");
        }
    }
}

/// A popup's item names are iTerm2's too. An item's value is its tag (an item with none has tag 0, as
/// AppKit has it); a popup whose items share a tag is skipped, as it has no per-item value.
#[test]
fn schema_choices_are_iterm2s() {
    let mut bad = Vec::new();
    for (scope, s) in rows() {
        let Some(c) = control_for(scope, &s.key) else { continue };
        if c.choices.is_empty() || CHOICES_EXEMPT.iter().any(|(sc, k, _)| *sc == scope && *k == s.key) {
            continue;
        }
        let tags: Vec<i64> = c.choices.iter().map(|(t, _)| t.parse::<i64>().unwrap_or(0)).collect();
        let mut unique = tags.clone();
        unique.sort_unstable();
        unique.dedup();
        if unique.len() != tags.len() {
            continue;
        }
        for choice in &s.choices {
            let Some(v) = choice.value.as_integer() else { continue };
            let theirs = tags.iter().position(|t| *t == v).map(|i| mac_words(&c.choices[i].1));
            if let Some(want) = theirs
                && want != choice.label
            {
                bad.push(format!("{scope} {} [{v}]: have {:?}, iTerm2 has {:?}", s.key, choice.label, want));
            }
        }
    }
    assert!(bad.is_empty(), "{} choices differ:\n{}", bad.len(), bad.join("\n"));
}

/// Popups whose item names stay Termurion's: (scope, key, why).
const CHOICES_EXEMPT: &[(&str, &str, &str)] = &[
    ("profile", "Space", "Windows calls Spaces virtual desktops, so the items say Desktop"),
    ("global", "SwitchTabModifier", "iTerm2 writes a key glyph and 'Number'; Termurion writes the Windows chord"),
    ("global", "SwitchPaneModifier", "iTerm2 writes a key glyph and 'Number'; Termurion writes the Windows chord"),
    ("global", "SwitchWindowModifier", "iTerm2 writes a key glyph and 'Number'; Termurion writes the Windows chord"),
    ("global", "LeftControl", "iTerm2 puts a key glyph before each key's name; Termurion names the Windows key"),
    ("global", "RightControl", "iTerm2 puts a key glyph before each key's name; Termurion names the Windows key"),
    ("global", "LeftOption", "iTerm2 puts a key glyph before each key's name; Termurion names the Windows key"),
    ("global", "RightOption", "iTerm2 puts a key glyph before each key's name; Termurion names the Windows key"),
    ("global", "LeftCommand", "iTerm2 puts a key glyph before each key's name; Termurion names the Windows key"),
    ("global", "RightCommand", "iTerm2 puts a key glyph before each key's name; Termurion names the Windows key"),
    ("global", "FunctionKey", "iTerm2 puts the fn glyph before the name; Windows has no fn key"),
    ("global", "API Authentication Method", "iTerm2 asks for macOS Automation permission; Termurion's methods are the cookie and all apps (the Python API rulings)"),
];

/// The Colors tab draws every one of its rows (the grid, or the header above it) and in the xib's order.
#[test]
fn the_colors_grid_draws_every_colors_row() {
    let mut p = crate::profile::Profile::new("ui-labels-test");
    let mut ctx_keys = Vec::new();
    for s in crate::schema::profile().iter().filter(|s| s.tab == "Colors") {
        ctx_keys.push(s.key.as_str());
    }
    let src = include_str!("settings_colors.rs");
    let missing: Vec<&&str> = ctx_keys.iter().filter(|k| **k != "Use Separate Colors for Light and Dark Mode" && !src.contains(&format!("\"{k}\""))).collect();
    assert!(missing.is_empty(), "Colors rows the grid does not draw: {missing:?}");
    let drawn = headless_height(&mut |ui| {
        crate::settings_colors::body(ui, &mut p, false);
    });
    assert!(drawn > 300.0, "the grid draws its rows");
}

/// Global rows drawn by their own code rather than by `global_control`: (key, where).
const GLOBAL_ELSEWHERE: &[(&str, &str)] = &[
    ("GlobalKeyMap", "settings_keys::keys_page, the key bindings table"),
    ("Leader", "settings_keys::keys_page, the Leader recorder"),
    ("PointerActions", "settings_keys::pointer_page, the pointer bindings table"),
    ("HotkeyCode", "settings_hotkey::keys_section, the App Hotkey recorder"),
    ("HotkeyChar", "settings_hotkey::keys_section, the App Hotkey recorder"),
    ("HotkeyModifiers", "settings_hotkey::keys_section, the App Hotkey recorder"),
    ("OpenQuicklyHotkeyCode", "settings_hotkey::keys_section, the Open Quickly Hotkey recorder"),
    ("OpenQuicklyHotkeyChar", "settings_hotkey::keys_section, the Open Quickly Hotkey recorder"),
    ("OpenQuicklyHotkeyModifiers", "settings_hotkey::keys_section, the Open Quickly Hotkey recorder"),
    ("Hotkey", "settings_hotkey::keys_section, the checkbox over the recorders"),
    ("CustomScriptsFolder", "settings_keys::custom_scripts_folder_row"),
    ("LoadPrefsFromCustomFolder", "settings_keys::custom_folder_rows"),
    ("PrefsCustomFolder", "settings_keys::custom_folder_rows"),
    ("OpenTmuxWindowsIn", "settings_keys::tmux_attach_rows, a checkbox and a popup over one value"),
    ("OpenArrangementAtStartup", "settings_keys::restoration_row, the Window restoration policy popup"),
    ("OpenNoWindowsAtStartup", "settings_keys::restoration_row, an item of that popup"),
    ("PhonyAllowSendingClipboardContents", "settings_keys::clipboard_sending_row, a popup over two stored switches"),
];

fn headless_height(draw: &mut dyn FnMut(&mut egui::Ui)) -> f32 {
    let ctx = egui::Context::default();
    let mut drawn = 0.0;
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        let before = ui.min_rect().height();
        draw(ui);
        drawn = ui.min_rect().height() - before;
    });
    out.textures_delta.clear();
    drawn
}

/// Every global row of a page Settings draws is a control: the generic arm draws it, or a
/// named function does (`GLOBAL_ELSEWHERE`). No row is left without one.
#[test]
fn every_global_row_draws_a_control() {
    let mut missing = Vec::new();
    for s in crate::schema::global() {
        let on_a_page = matches!(s.tab.as_str(), "General" | "Appearance" | "Keys" | "Pointer") && !(s.tab == "General" && s.section == "AI");
        if !on_a_page || crate::settings_keys::row_hidden(&s.key) || GLOBAL_ELSEWHERE.iter().any(|(k, _)| *k == s.key) {
            continue;
        }
        let mut g = crate::globals::Globals::empty(std::path::Path::new("ui-labels-test.toml"));
        // The word rows show one at a time; draw the one that does not.
        if !crate::settings_keys::row_visible(&g, &s.key) {
            continue;
        }
        let drawn = headless_height(&mut |ui| {
            crate::settings_keys::global_control(ui, &mut g, s, true);
        });
        if drawn <= 0.0 {
            missing.push(format!("{} ({})", s.key, s.label));
        }
    }
    assert!(missing.is_empty(), "global rows that draw nothing: {missing:?}");
}

/// Profile rows the profile loop draws with a function of their own, not the generic control.
const PROFILE_ELSEWHERE: &[&str] = &[
    "Profile Type (Phony)",
    "Has Hotkey",
    "Session Hotkey",
    "Keyboard Map",
    "Bound Hosts",
    "Shortcut",
    "Status Bar Layout",
    "Custom Locale",
    "SSH",
    "Window Type",
    "Columns",
    "Rows",
    "Width Percentage",
    "Height Percentage",
    "Title Components",
    "Title Function",
    "Tags",
    "Snippets Filter",
    "Subtitle",
    "Screen",
    "Special Font Config",
    "Jobs to Ignore",
    "Triggers",
    "Smart Selection Rules",
    "Semantic History",
    "Browser Zoom",
    "Touch Bar Map",
    "Browser Extensions Root",
    "Browser Extension Active IDs",
    "Progress Bar Height",
];

/// Every profile row of a tab is a control: `control` draws it, or the profile loop does with a
/// function named in `PROFILE_ELSEWHERE` (and the font block's grouped rows).
#[test]
fn every_profile_row_draws_a_control() {
    let mut missing = Vec::new();
    for s in crate::schema::profile() {
        if s.tab.is_empty() || s.key.starts_with("HotKey ") || PROFILE_ELSEWHERE.contains(&s.key.as_str()) || crate::settings_fonts::GROUPED.contains(&s.key.as_str()) {
            continue;
        }
        let mut p = crate::profile::Profile::new("ui-labels-test");
        let drawn = headless_height(&mut |ui| {
            crate::settings_ui::control(ui, &mut p, s, false);
        });
        if drawn <= 0.0 {
            missing.push(format!("{} ({})", s.key, s.label));
        }
    }
    assert!(missing.is_empty(), "profile rows that draw nothing: {missing:?}");
}

/// The font block draws its rows and the profile loop leaves them to it.
#[test]
fn the_font_block_draws() {
    let mut p = crate::profile::Profile::new("ui-labels-test");
    p.set("Use Non-ASCII Font", toml::Value::Boolean(true));
    let drawn = headless_height(&mut |ui| {
        crate::settings_fonts::group(ui, &mut p, &[], false, &|_, _| false, &|_| None);
    });
    assert!(drawn > 0.0);
}

/// No row draws a placeholder: not an "n items" count, not a promise of a later milestone.
#[test]
fn no_row_draws_a_placeholder() {
    let count = concat!("item", "(s)");
    let promise = concat!("arrives with its ", "feature's milestone");
    let later = concat!("per-profile fonts ", "come later");
    for (name, src) in [
        ("settings_ui.rs", include_str!("settings_ui.rs")),
        ("settings_keys.rs", include_str!("settings_keys.rs")),
        ("settings_advanced.rs", include_str!("settings_advanced.rs")),
        ("settings_lists.rs", include_str!("settings_lists.rs")),
        ("settings_fonts.rs", include_str!("settings_fonts.rs")),
    ] {
        for needle in [count, promise, later] {
            assert!(!src.contains(needle), "{name} still draws {needle:?}");
        }
    }
}

/// Where a control sits in iTerm2's xib: (item index of its tab view, top, left), y AppKit-style (up).
type Place = (i64, f64, f64);

/// The key a group of keys is drawn at: one function draws these together, at the lead key's place.
fn lead(key: &str) -> &str {
    crate::settings_search::lead(key)
}

fn place(scope: &str, key: &str, tab: &str) -> Option<Place> {
    const PROFILE: [(&str, &str); 8] = [
        ("ProfilesGeneralPreferencesViewController", "General"),
        ("ProfilesColorsPreferencesViewController", "Colors"),
        ("ProfilesTextPreferencesViewController", "Text"),
        ("ProfilesWindowPreferencesViewController", "Window"),
        ("ProfilesTerminalPreferencesViewController", "Terminal"),
        ("ProfilesSessionPreferencesViewController", "Session"),
        ("ProfilesKeysPreferencesViewController", "Keys"),
        ("ProfilesAdvancedPreferencesViewController", "Advanced"),
    ];
    const GLOBAL: [(&str, &str); 4] = [
        ("GeneralPreferencesViewController", "General"),
        ("AppearancePreferencesViewController", "Appearance"),
        ("KeysPreferencesViewController", "Keys"),
        ("PointerPreferencesViewController", "Pointer"),
    ];
    let panes: &[(&str, &str)] = if scope == "profile" { &PROFILE } else { &GLOBAL };
    for k in [lead(key), key] {
        if let Some(c) = control_for(scope, k)
            && let (Some(pane), Some(top), Some(left)) = (&c.pane, c.top, c.left)
            && panes.iter().any(|(p, t)| p == pane && *t == tab)
        {
            return Some((c.item_index.unwrap_or(0), top, left));
        }
    }
    None
}

/// Whether `a` draws before `b` (or on the same line, left of it): a row is six points tall.
fn not_after(a: Place, b: Place) -> bool {
    if a.0 != b.0 {
        return a.0 < b.0;
    }
    if (a.1 - b.1).abs() > 6.0 {
        return a.1 > b.1;
    }
    a.2 <= b.2
}

/// Every pane draws its controls in the order of iTerm2's xib, top to bottom and left to right: the
/// schema's row order is the draw order (`scripts/order-schema.py` writes it). A page's section is
/// one group of rows; a profile tab is one group. Rows with no place in the xib are left where they are.
#[test]
fn rows_are_in_the_xibs_order() {
    let mut bad = Vec::new();
    let mut check = |scope: &str, group: String, rows: Vec<(&str, &str)>| {
        let placed: Vec<(&str, Place)> = rows.iter().filter_map(|(key, tab)| place(scope, key, tab).map(|p| (*key, p))).collect();
        for w in placed.windows(2) {
            if !not_after(w[0].1, w[1].1) {
                bad.push(format!("{scope} {group}: {} should come after {}", w[0].0, w[1].0));
            }
        }
    };
    for tab in crate::schema::TABS {
        check("profile", (*tab).to_string(), crate::schema::profile().iter().filter(|s| s.tab == *tab).map(|s| (s.key.as_str(), s.tab.as_str())).collect());
    }
    for tab in ["General", "Appearance", "Keys", "Pointer"] {
        let mut sections: Vec<&str> = Vec::new();
        for s in crate::schema::global().iter().filter(|s| s.tab == tab) {
            if !sections.contains(&s.section.as_str()) {
                sections.push(&s.section);
            }
        }
        for section in sections {
            check("global", format!("{tab}/{section}"), crate::schema::global().iter().filter(|s| s.tab == tab && s.section == section).map(|s| (s.key.as_str(), s.tab.as_str())).collect());
        }
    }
    assert!(bad.is_empty(), "{} rows out of the xib's order:\n{}", bad.len(), bad.join("\n"));
}

/// The profile tabs have no section headings, as iTerm2's panes have none: one group, no name.
#[test]
fn profile_tabs_draw_no_section_headings() {
    for tab in crate::schema::TABS {
        for (name, rows) in crate::settings_ui::sections(tab) {
            assert!(name.is_empty() && !rows.is_empty(), "{tab}: {name:?}");
        }
    }
}
