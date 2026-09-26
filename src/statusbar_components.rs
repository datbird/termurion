//! iTerm2's status bar content components (milestone 9h): every class in the setup
//! picker except the framework's own Spring and Fixed-size Spacer (`statusbar.rs`) and
//! the five classes deferred to milestone 9i (Battery, CPU, Memory, Network, git state).
//! Pure data: kinds, descriptions, knobs in iTerm2's order, default knobs, icons and
//! exemplars. `statusbar.rs` folds this module's registry into its own `registry`, `info`,
//! `knobs`, `default_knobs` and `exemplar`, in iTerm2's palette order.
//!
//! Citations use `.superpowers/research/m9-facts.md` section 2's abbreviations (`BASE` =
//! `iTermStatusBarBaseComponent.m`, `TXT` = `iTermStatusBarTextComponent.m`, `CLK` =
//! `iTermStatusBarClockComponent.m`, `ACT` = `iTermStatusBarActionComponent.m`, `VAR` =
//! `iTermStatusBarVariableBaseComponent.m`, `SRCH`/`FILT` = the Search/Filter field
//! components, `COMP` = `iTermStatusBarComposerComponent.m`, `SWS` =
//! `iTermStatusBarSwiftyStringComponent.m`, `FN` = `iTermStatusBarFunctionCallComponent.m`).

use toml::Value;

use crate::statusbar::{self, Algorithm, Info, Knob, KnobKind};

/// `iTermStatusBarClockComponent`.
pub const CLOCK_CLASS: &str = "iTermStatusBarClockComponent";
/// `iTermStatusBarActionComponent` (Custom Action).
pub const CUSTOM_ACTION_CLASS: &str = "iTermStatusBarActionComponent";
/// `iTermStatusBarActionMenuComponent`.
pub const ACTIONS_MENU_CLASS: &str = "iTermStatusBarActionMenuComponent";
/// `iTermStatusBarSnippetMenuComponent`.
pub const SNIPPETS_MENU_CLASS: &str = "iTermStatusBarSnippetMenuComponent";
/// `iTermStatusBarTriggersComponent`.
pub const TRIGGERS_MENU_CLASS: &str = "iTermStatusBarTriggersComponent";
/// `iTermStatusBarHostnameComponent`.
pub const HOST_NAME_CLASS: &str = "iTermStatusBarHostnameComponent";
/// `iTermStatusBarUsernameComponent`.
pub const USER_NAME_CLASS: &str = "iTermStatusBarUsernameComponent";
/// `iTermStatusBarJobComponent`.
pub const JOB_NAME_CLASS: &str = "iTermStatusBarJobComponent";
/// `iTermStatusBarWorkingDirectoryComponent` (Current Directory).
pub const CURRENT_DIRECTORY_CLASS: &str = "iTermStatusBarWorkingDirectoryComponent";
/// `iTermStatusBarSearchFieldComponent`.
pub const SEARCH_TOOL_CLASS: &str = "iTermStatusBarSearchFieldComponent";
/// `iTermStatusBarFilterComponent`.
pub const FILTER_TOOL_CLASS: &str = "iTermStatusBarFilterComponent";
/// `iTermStatusBarComposerComponent`.
pub const COMPOSER_CLASS: &str = "iTermStatusBarComposerComponent";
/// `iTermStatusBarSwiftyStringComponent` (Interpolated String).
pub const INTERPOLATED_STRING_CLASS: &str = "iTermStatusBarSwiftyStringComponent";
/// `iTermStatusBarFunctionCallComponent` (Call Script Function).
pub const CALL_SCRIPT_FUNCTION_CLASS: &str = "iTermStatusBarFunctionCallComponent";
/// `iTermStatusBarTmuxLeftComponent` (internal).
pub const TMUX_LEFT_CLASS: &str = "iTermStatusBarTmuxLeftComponent";
/// `iTermStatusBarTmuxRightComponent` (internal).
pub const TMUX_RIGHT_CLASS: &str = "iTermStatusBarTmuxRightComponent";
/// `iTermStatusBarProgressComponent` (internal).
pub const PROGRESS_CLASS: &str = "iTermStatusBarProgressComponent";

// Own knob keys (Resume notes; `BASE`'s shared keys are `statusbar`'s).
const FORMAT_KEY: &str = "format";
const LOCALIZE_KEY: &str = "localize";
const SSH_OFFSET_KEY: &str = "ssh_offset";
const SSH_TZ_KEY: &str = "ssh_tz";
const ABBREVIATE_LOCALHOST_KEY: &str = "abbreviate-localhost";
const ACTION_KEY: &str = "action";
const EXPRESSION_KEY: &str = "expression";
const LABEL_KEY: &str = "label";
const FUNCTION_INVOCATION_KEY: &str = "function invocation";
const TIMEOUT_KEY: &str = "timeout";

/// One content class this milestone knows, `com.iterm2.<class>` minus the prefix. Every
/// variant but the last three (internal: no palette entry, no Configure sheet) is offered
/// in the setup picker, in this declaration order (`statusbar::PALETTE`, `SETUP:107-134`
/// less the 9i classes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Clock,
    CustomAction,
    ActionsMenu,
    SnippetsMenu,
    TriggersMenu,
    HostName,
    UserName,
    JobName,
    CurrentDirectory,
    SearchTool,
    FilterTool,
    Composer,
    InterpolatedString,
    CallScriptFunction,
    TmuxLeft,
    TmuxRight,
    Progress,
}

/// Every `Kind`, in the order above (the palette's 14, then the 3 internal ones).
const ALL: [Kind; 17] = [
    Kind::Clock,
    Kind::CustomAction,
    Kind::ActionsMenu,
    Kind::SnippetsMenu,
    Kind::TriggersMenu,
    Kind::HostName,
    Kind::UserName,
    Kind::JobName,
    Kind::CurrentDirectory,
    Kind::SearchTool,
    Kind::FilterTool,
    Kind::Composer,
    Kind::InterpolatedString,
    Kind::CallScriptFunction,
    Kind::TmuxLeft,
    Kind::TmuxRight,
    Kind::Progress,
];

impl Kind {
    /// The kind for a layout component's class; None for a class this module does not
    /// know (a 9i class, the framework's own, or one Tandem truly does not recognize).
    pub fn of(class: &str) -> Option<Kind> {
        Some(match class {
            CLOCK_CLASS => Kind::Clock,
            CUSTOM_ACTION_CLASS => Kind::CustomAction,
            ACTIONS_MENU_CLASS => Kind::ActionsMenu,
            SNIPPETS_MENU_CLASS => Kind::SnippetsMenu,
            TRIGGERS_MENU_CLASS => Kind::TriggersMenu,
            HOST_NAME_CLASS => Kind::HostName,
            USER_NAME_CLASS => Kind::UserName,
            JOB_NAME_CLASS => Kind::JobName,
            CURRENT_DIRECTORY_CLASS => Kind::CurrentDirectory,
            SEARCH_TOOL_CLASS => Kind::SearchTool,
            FILTER_TOOL_CLASS => Kind::FilterTool,
            COMPOSER_CLASS => Kind::Composer,
            INTERPOLATED_STRING_CLASS => Kind::InterpolatedString,
            CALL_SCRIPT_FUNCTION_CLASS => Kind::CallScriptFunction,
            TMUX_LEFT_CLASS => Kind::TmuxLeft,
            TMUX_RIGHT_CLASS => Kind::TmuxRight,
            PROGRESS_CLASS => Kind::Progress,
            _ => return None,
        })
    }

    pub fn class(self) -> &'static str {
        match self {
            Kind::Clock => CLOCK_CLASS,
            Kind::CustomAction => CUSTOM_ACTION_CLASS,
            Kind::ActionsMenu => ACTIONS_MENU_CLASS,
            Kind::SnippetsMenu => SNIPPETS_MENU_CLASS,
            Kind::TriggersMenu => TRIGGERS_MENU_CLASS,
            Kind::HostName => HOST_NAME_CLASS,
            Kind::UserName => USER_NAME_CLASS,
            Kind::JobName => JOB_NAME_CLASS,
            Kind::CurrentDirectory => CURRENT_DIRECTORY_CLASS,
            Kind::SearchTool => SEARCH_TOOL_CLASS,
            Kind::FilterTool => FILTER_TOOL_CLASS,
            Kind::Composer => COMPOSER_CLASS,
            Kind::InterpolatedString => INTERPOLATED_STRING_CLASS,
            Kind::CallScriptFunction => CALL_SCRIPT_FUNCTION_CLASS,
            Kind::TmuxLeft => TMUX_LEFT_CLASS,
            Kind::TmuxRight => TMUX_RIGHT_CLASS,
            Kind::Progress => PROGRESS_CLASS,
        }
    }
}

/// Short description, detailed description, and can-stretch, for the 14 palette kinds
/// (`statusBarComponentShortDescription`, `statusBarComponentDetailedDescription`,
/// `statusBarComponentCanStretch`). Every content class but Composer and Call Script
/// Function can stretch (neither overrides `statusBarComponentCanStretch`, `COMP:54-66`,
/// `FN:127-167`); so do the 3 internal ones, whose own text below is never shown (no
/// palette entry, no Active Components slot).
fn short_detail_stretch(kind: Kind) -> (&'static str, &'static str, bool) {
    match kind {
        Kind::Clock => ("Clock", "Shows current date and time with a configurable format.", true),
        Kind::CustomAction => ("Custom Action", "Adds a button that performs a user-configurable action, similar to a key binding.", true),
        Kind::ActionsMenu => ("Actions Menu", "When clicked, opens a menu of actions. Actions are like custom key bindings, but without a keystroke attached.", true),
        Kind::SnippetsMenu => ("Snippets Menu", "When clicked, opens a menu of snippets. Snippets are saved text strings that can be pasted quickly.", true),
        Kind::TriggersMenu => ("Triggers Menu", "When clicked, opens a menu of triggers. You can use it to enable or disable triggers.", true),
        Kind::HostName => ("Host Name", "Current host name. Requires shell integration.", true),
        Kind::UserName => ("User Name", "Current user name. Requires shell integration.", true),
        Kind::JobName => ("Job Name", "Shows the currently running job. If space permits, parent process names are also shown.", true),
        Kind::CurrentDirectory => ("Current Directory", "Current directory. Best with shell integration.", true),
        Kind::SearchTool => ("Search Tool", "Search tool to find text in the terminal window.", true),
        Kind::FilterTool => ("Filter Tool", "Filter tool to remove non-matching lines from terminal window.", true),
        Kind::Composer => ("Composer", "Adds a text field for composing command lines.", false),
        Kind::InterpolatedString => (
            "Interpolated String",
            "Shows the evaluation of a string with inline expressions which may include session variables or the output of registered scripting functions",
            true,
        ),
        Kind::CallScriptFunction => ("Call Script Function", "Adds a button that invokes a script function with a user-provided invocation.", false),
        Kind::TmuxLeft => ("Tmux Left", "Tmux Left", true),
        Kind::TmuxRight => ("Tmux Right", "Tmux Right", true),
        Kind::Progress => ("Progress Indicator", "Progress Indicator", true),
    }
}

/// This module's own registry (`statusBarComponentShortDescription`,
/// `statusBarComponentDetailedDescription`, `statusBarComponentCanStretch`): the palette
/// entries, then the internal ones. `exemplar` is never read from these (see there); every
/// content class resolves through it instead, so a placeholder is enough here.
pub fn infos() -> Vec<Info> {
    ALL.iter()
        .map(|&kind| {
            let (short, detail, stretch) = short_detail_stretch(kind);
            Info { class: kind.class(), short, detail, exemplar: "", stretch, margins: true, internal: matches!(kind, Kind::Progress | Kind::TmuxLeft | Kind::TmuxRight) }
        })
        .collect()
}

/// The knob descriptors `statusBarComponentKnobs` returns, in order, for `kind` under
/// `algorithm` (research 2.25). An internal kind (no Configure sheet) returns none.
pub fn knobs(kind: Kind, algorithm: Algorithm) -> Vec<Knob> {
    let text = |mut own: Vec<Knob>| {
        own.extend(statusbar::text_component_knobs(algorithm));
        own
    };
    match kind {
        Kind::Clock => text(vec![
            Knob { label: "Date Format:".into(), kind: KnobKind::Text, key: FORMAT_KEY.into(), placeholder: Some("Date Format (Unicode TR 35)".into()), default: Some(Value::String("M/dd h:mm".into())) },
            Knob { label: "Localize Date Format".into(), kind: KnobKind::Checkbox, key: LOCALIZE_KEY.into(), placeholder: None, default: Some(Value::Boolean(true)) },
            Knob { label: "Show server time in SSH integration?".into(), kind: KnobKind::Checkbox, key: SSH_OFFSET_KEY.into(), placeholder: None, default: Some(Value::Boolean(true)) },
            Knob { label: "Use server time zone in SSH integration?".into(), kind: KnobKind::Checkbox, key: SSH_TZ_KEY.into(), placeholder: None, default: Some(Value::Boolean(true)) },
        ]),
        Kind::CustomAction => text(vec![Knob { label: "Action".into(), kind: KnobKind::Action, key: ACTION_KEY.into(), placeholder: None, default: None }]),
        Kind::ActionsMenu | Kind::SnippetsMenu | Kind::TriggersMenu | Kind::UserName | Kind::JobName | Kind::CurrentDirectory => text(Vec::new()),
        Kind::HostName => text(vec![Knob {
            label: "localhost replacement".into(),
            kind: KnobKind::Text,
            key: ABBREVIATE_LOCALHOST_KEY.into(),
            placeholder: Some("Enter replacement text for localhost".into()),
            default: Some(Value::String(String::new())),
        }]),
        Kind::InterpolatedString => text(vec![Knob {
            label: "String Value:".into(),
            kind: KnobKind::Text,
            key: EXPRESSION_KEY.into(),
            placeholder: Some("String with \\(expressions)".into()),
            default: Some(Value::String(String::new())),
        }]),
        Kind::SearchTool | Kind::FilterTool => vec![statusbar::color_knob("Color", statusbar::SHARED_TEXT_COLOR_KEY)],
        Kind::Composer => {
            let mut v = vec![statusbar::color_knob("Icon Color:", statusbar::SHARED_TEXT_COLOR_KEY), statusbar::color_knob("Background Color:", statusbar::SHARED_BACKGROUND_COLOR_KEY), statusbar::priority_knob()];
            v.extend(statusbar::min_max_width_knobs());
            v
        }
        Kind::CallScriptFunction => vec![
            Knob { label: "Label:".into(), kind: KnobKind::Text, key: LABEL_KEY.into(), placeholder: Some("Button Label".into()), default: None },
            Knob { label: "Function call:".into(), kind: KnobKind::Invocation, key: FUNCTION_INVOCATION_KEY.into(), placeholder: Some("foo(bar: \"baz\")".into()), default: None },
            Knob { label: "Timeout (seconds):".into(), kind: KnobKind::Double, key: TIMEOUT_KEY.into(), placeholder: None, default: Some(Value::Float(5.0)) },
            statusbar::color_knob("Background Color:", statusbar::SHARED_BACKGROUND_COLOR_KEY),
            statusbar::color_knob("Text Color:", statusbar::SHARED_TEXT_COLOR_KEY),
            statusbar::priority_knob(),
        ],
        Kind::TmuxLeft | Kind::TmuxRight | Kind::Progress => Vec::new(),
    }
}

/// `statusBarComponentDefaultKnobs`'s own additions for `kind`, on top of the base knobs
/// (`"base: compression resistance"`, `"base: priority"`) `statusbar::default_knobs`
/// already adds for every class.
pub fn default_knobs(kind: Kind) -> toml::Table {
    let mut t = toml::Table::new();
    match kind {
        Kind::Clock => {
            t.insert(FORMAT_KEY.into(), Value::String("M/dd h:mm".into()));
        }
        Kind::JobName | Kind::CurrentDirectory => {
            t.insert(statusbar::MAX_WIDTH_KEY.into(), Value::Float(f64::INFINITY));
            t.insert(statusbar::MIN_WIDTH_KEY.into(), Value::Float(0.0));
        }
        Kind::CallScriptFunction => {
            t.insert(TIMEOUT_KEY.into(), Value::Float(5.0));
        }
        _ => {}
    }
    t
}

/// A Segoe Fluent Icons glyph tinted with the component's text color
/// (`statusBarComponentIcon`), or None for a class with no icon.
pub fn icon(kind: Kind) -> Option<char> {
    match kind {
        Kind::Clock => Some('\u{E823}'),
        Kind::CustomAction | Kind::ActionsMenu => Some('\u{E945}'),
        Kind::SnippetsMenu => Some('\u{E8C6}'),
        Kind::TriggersMenu => Some('\u{E7C1}'),
        Kind::HostName => Some('\u{E774}'),
        Kind::UserName => Some('\u{E77B}'),
        Kind::JobName => Some('\u{E756}'),
        Kind::CurrentDirectory => Some('\u{E8B7}'),
        Kind::Composer => Some('\u{E70F}'),
        Kind::Progress => Some('\u{E77F}'),
        Kind::SearchTool | Kind::FilterTool | Kind::InterpolatedString | Kind::CallScriptFunction | Kind::TmuxLeft | Kind::TmuxRight => None,
    }
}

/// The Actions Menu's text on the bar (`ACT:164-166`).
pub const ACTIONS_MENU_TEXT: &str = "Perform Action\u{2026}";
/// The Snippets Menu's text on the bar (`iTermStatusBarSnippetComponent.m:41-43`).
pub const SNIPPETS_MENU_TEXT: &str = "Send Snippet\u{2026}";
/// The Triggers Menu's text on the bar (`iTermStatusBarTriggersComponent.swift:55-57`).
pub const TRIGGERS_MENU_TEXT: &str = "Triggers\u{2026}";

/// `statusBarComponentIsEmpty` for the menu components, from how many actions, snippets
/// and session triggers there are: the Actions Menu with no actions, the Snippets Menu
/// with no snippets, and the Triggers Menu when there ARE triggers (iTerm2's inverted
/// test, kept). Every other kind is not empty by this rule (Custom Action never is).
pub fn menu_empty(kind: Kind, actions: usize, snippets: usize, triggers: usize) -> bool {
    match kind {
        Kind::ActionsMenu => actions == 0,
        Kind::SnippetsMenu => snippets == 0,
        Kind::TriggersMenu => triggers > 0,
        _ => false,
    }
}

/// A trigger's item in the Triggers Menu (`PTYSession.m:23172-23180`): the action's title
/// less one trailing U+2026, U+2014, and the regex.
pub fn trigger_menu_title(action_title: &str, regex: &str) -> String {
    let title = action_title.strip_suffix('\u{2026}').unwrap_or(action_title);
    format!("{title} \u{2014} {regex}")
}

/// `statusBarComponentExemplarWithBackgroundColor:textColor:` for a content class Tandem
/// knows; None for one it does not (the framework's own classes, or a 9i one), so the
/// caller (`statusbar::exemplar`) falls back to that class's static `Info::exemplar`.
/// `clock_text` is the Clock's own text, already formatted (Task 2 builds the formatter);
/// `user` and `home` are the caller's OS lookups (`%USERNAME%`, `%USERPROFILE%`).
pub fn exemplar(class: &str, knobs: &toml::Table, clock_text: &str, user: &str, home: &str) -> Option<String> {
    let kind = Kind::of(class)?;
    Some(match kind {
        Kind::Clock => clock_text.to_string(),
        Kind::CustomAction => knobs.get(ACTION_KEY).and_then(crate::user_actions::UserAction::from_value).map(|a| a.title).unwrap_or_else(|| "Custom Action".to_string()),
        Kind::ActionsMenu => "Action\u{2026}".to_string(),
        Kind::SnippetsMenu => "Snippet\u{2026}".to_string(),
        Kind::TriggersMenu => "Triggers\u{2026}".to_string(),
        Kind::HostName => "example.com".to_string(),
        Kind::UserName => user.to_string(),
        Kind::JobName => "vim \u{25C2} bash".to_string(),
        Kind::CurrentDirectory => home.to_string(),
        Kind::SearchTool => "\u{1F50E} Search".to_string(),
        Kind::FilterTool => "\u{2979} Filter".to_string(),
        Kind::Composer => ">_ [Command] \u{1F4AC}".to_string(),
        Kind::InterpolatedString => {
            let expr = knobs.get(EXPRESSION_KEY).and_then(Value::as_str).filter(|s| !s.is_empty());
            expr.map(str::to_string).unwrap_or_else(|| "\\(expression)".to_string())
        }
        Kind::CallScriptFunction => {
            let label = knobs.get(LABEL_KEY).and_then(Value::as_str).filter(|s| !s.is_empty());
            label.map(str::to_string).unwrap_or_else(|| "foo(bar: \"baz\")".to_string())
        }
        // Internal: never placed by hand, never drawn through this path.
        Kind::TmuxLeft | Kind::TmuxRight | Kind::Progress => String::new(),
    })
}

/// `iTermStatusBarSearchComponentIsTemporaryKey`: the Search Tool Find puts on a bar
/// whose layout has none (`iTermStatusBarSearchFieldComponent.m:21`).
pub const SEARCH_TEMPORARY_KEY: &str = "search: temporary";
/// `StatusBarFilterComponent.isTemporaryKey` (`StatusBarFilterComponent.swift:12`).
pub const FILTER_TEMPORARY_KEY: &str = "filter: temporary";

/// A temporary component (`SessionView.m:713-760`, `iTermPasteViewManager.m:77-89`):
/// priority infinity, so the layout drops everything else first, and, for the Search and
/// Filter Tools, the key that marks it temporary (it shows a close glyph).
pub fn temporary(kind: Kind) -> statusbar::Component {
    let mut knobs = toml::Table::new();
    knobs.insert(statusbar::BASE_PRIORITY_KEY.into(), Value::Float(f64::INFINITY));
    match kind {
        Kind::SearchTool => {
            knobs.insert(SEARCH_TEMPORARY_KEY.into(), Value::Boolean(true));
        }
        Kind::FilterTool => {
            knobs.insert(FILTER_TEMPORARY_KEY.into(), Value::Boolean(true));
        }
        _ => {}
    }
    statusbar::Component { class: kind.class().into(), knobs, config: toml::Table::new() }
}

/// The component was made by `temporary` for a field (its knob says so).
pub fn is_temporary(c: &statusbar::Component) -> bool {
    [SEARCH_TEMPORARY_KEY, FILTER_TEMPORARY_KEY].iter().any(|k| c.knobs.get(*k).and_then(Value::as_bool) == Some(true))
}

/// `updateViews` (`iTermStatusBarViewController.m:248-293`): the temporary left component
/// first, then the layout's, then, for a temporary right one, a Spring whose spring
/// constant is 1 (`springComponentWithCompressionResistance:1`, which sets the Spring's
/// own constant key) and the component.
pub fn with_temporaries(components: &[statusbar::Component], left: Option<statusbar::Component>, right: Option<statusbar::Component>) -> Vec<statusbar::Component> {
    let mut out: Vec<statusbar::Component> = left.into_iter().collect();
    out.extend(components.iter().cloned());
    if let Some(right) = right {
        let mut knobs = toml::Table::new();
        knobs.insert(statusbar::SPRING_CONSTANT_KEY.into(), Value::Float(1.0));
        out.push(statusbar::Component { class: statusbar::SPRING_CLASS.into(), knobs, config: toml::Table::new() });
        out.push(right);
    }
    out
}

/// `iTermStatusBarLayout+tmux.m` tmuxLayoutWithController: the profile layout's advanced
/// configuration with tmux's left line, a Spring, and tmux's right line as its components,
/// each an Interpolated String of the session's `tmuxStatusLeft` or `tmuxStatusRight`.
pub fn tmux_layout(profile_layout: &statusbar::Layout) -> statusbar::Layout {
    let interpolated = |class: &str, variable: &str| {
        let mut knobs = toml::Table::new();
        knobs.insert(EXPRESSION_KEY.into(), Value::String(format!("\\({variable})")));
        statusbar::Component { class: class.into(), knobs, config: toml::Table::new() }
    };
    let spring = statusbar::Component { class: statusbar::SPRING_CLASS.into(), knobs: toml::Table::new(), config: toml::Table::new() };
    statusbar::Layout { components: vec![interpolated(TMUX_LEFT_CLASS, "tmuxStatusLeft"), spring, interpolated(TMUX_RIGHT_CLASS, "tmuxStatusRight")], ..profile_layout.clone() }
}

/// A tmux tab's bar shows `tmux_layout` (`PTYSession.m:10077-10091`): only on a tmux 2.9
/// or later server (`server`, its minimum version), and then with `UseTmuxStatusBar` on or
/// when the profile's layout has no components (shouldOverrideLayout).
pub fn uses_tmux_layout(use_tmux: bool, profile_layout: &statusbar::Layout, server: Option<f64>) -> bool {
    server.is_some_and(|v| v >= 2.9) && (use_tmux || profile_layout.components.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(pairs: &[(&str, Value)]) -> toml::Table {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    /// `iTermStatusBarLayout+tmux.m`.
    #[test]
    fn tmux_layout_follows_iterm2() {
        let clock = statusbar::Component { class: CLOCK_CLASS.into(), knobs: toml::Table::new(), config: toml::Table::new() };
        let mut profile = statusbar::Layout { components: vec![clock], ..Default::default() };
        profile.advanced.remove_empty = true;
        profile.advanced.font = Some("Menlo 13".into());
        let t = tmux_layout(&profile);
        assert_eq!(t.advanced, profile.advanced, "the profile's advanced configuration is kept");
        let classes: Vec<&str> = t.components.iter().map(|c| c.class.as_str()).collect();
        assert_eq!(classes, vec![TMUX_LEFT_CLASS, statusbar::SPRING_CLASS, TMUX_RIGHT_CLASS]);
        assert_eq!(t.components[0].knobs, table(&[("expression", Value::String("\\(tmuxStatusLeft)".into()))]));
        assert!(t.components[1].knobs.is_empty(), "a Spring with no knobs");
        assert_eq!(t.components[2].knobs, table(&[("expression", Value::String("\\(tmuxStatusRight)".into()))]));
        assert_eq!(Kind::of(&t.components[0].class), Some(Kind::TmuxLeft));
        let v = Some(2.9);
        assert!(uses_tmux_layout(true, &profile, v));
        assert!(!uses_tmux_layout(false, &profile, v));
        assert!(uses_tmux_layout(false, &statusbar::Layout::default(), v), "no components");
        assert!(!uses_tmux_layout(true, &profile, Some(2.8)), "2.8: the profile's own components");
        assert!(!uses_tmux_layout(true, &statusbar::Layout::default(), Some(2.8)));
        assert!(!uses_tmux_layout(true, &profile, None), "version not known");
    }

    /// `iTermStatusBarViewController.m:248-293`: the temporary left component goes first,
    /// the temporary right one last with a Spring (spring constant 1) before it;
    /// `SessionView.m:713-760`: each temporary has priority infinity and its own key;
    /// `iTermPasteViewManager.m:77-89`: the progress one only the priority.
    #[test]
    fn temporaries_follow_iterm2() {
        let search = temporary(Kind::SearchTool);
        assert_eq!(search.class, SEARCH_TOOL_CLASS);
        assert_eq!(search.knobs.get("base: priority"), Some(&Value::Float(f64::INFINITY)));
        assert_eq!(search.knobs.get("search: temporary"), Some(&Value::Boolean(true)));
        assert!(search.priority().is_infinite());
        let filter = temporary(Kind::FilterTool);
        assert_eq!(filter.knobs.get("filter: temporary"), Some(&Value::Boolean(true)));
        assert!(filter.knobs.get("search: temporary").is_none());
        let progress = temporary(Kind::Progress);
        assert_eq!(progress.knobs.len(), 1);
        assert!(is_temporary(&search) && is_temporary(&filter));

        let clock = statusbar::Component { class: CLOCK_CLASS.into(), knobs: toml::Table::new(), config: toml::Table::new() };
        let classes = |v: &[statusbar::Component]| v.iter().map(|c| c.class.clone()).collect::<Vec<_>>();
        assert_eq!(classes(&with_temporaries(std::slice::from_ref(&clock), None, None)), vec![CLOCK_CLASS]);
        let both = with_temporaries(std::slice::from_ref(&clock), Some(search.clone()), Some(filter.clone()));
        assert_eq!(classes(&both), vec![SEARCH_TOOL_CLASS, CLOCK_CLASS, statusbar::SPRING_CLASS, FILTER_TOOL_CLASS]);
        let spring = &both[2];
        assert_eq!(spring.knobs.get("iTermStatusBarSpringComponentSpringConstantKey"), Some(&Value::Float(1.0)));
        assert_eq!(spring.spring_constant(Algorithm::TightlyPacked), 1.0);
        assert_eq!(spring.spring_constant(Algorithm::Stable), 1.0);
        assert!(!is_temporary(spring), "the spring is not a field");
        assert_eq!(classes(&with_temporaries(&[], Some(search), None)), vec![SEARCH_TOOL_CLASS]);
        assert_eq!(classes(&with_temporaries(&[], None, Some(progress))), vec![statusbar::SPRING_CLASS, PROGRESS_CLASS]);
    }

    /// Review focus 2: the priority-infinity temporary outlives three priority-5 components
    /// on a bar too narrow for all of them, under both algorithms (`BLA:146-168` sorts it
    /// last, `SLA:100-139` removes the lowest priority first).
    #[test]
    fn temporary_search_stays_on_a_narrow_bar() {
        use crate::statusbar_layout::{layout, Item, ItemKind};
        let item = |min: f32, preferred: f32, priority: f32| Item { min, max: f32::INFINITY, preferred, stretch: true, spring: 1.0, margins: true, priority, kind: ItemKind::Normal, icon: false, empty: false, mandatory: false };
        let items = [item(100.0, 200.0, f32::INFINITY), item(40.0, 60.0, 5.0), item(40.0, 60.0, 5.0), item(40.0, 60.0, 5.0)];
        for algorithm in [Algorithm::TightlyPacked, Algorithm::Stable] {
            let placed = layout(&items, 150.0, algorithm, false);
            assert_eq!(placed.iter().map(|p| p.index).collect::<Vec<_>>(), vec![0], "{algorithm:?}");
            assert!(placed[0].width >= 100.0, "{algorithm:?}");
        }
    }

    #[test]
    fn content_registry_follows_iterm2() {
        let order = [
            CLOCK_CLASS,
            CUSTOM_ACTION_CLASS,
            ACTIONS_MENU_CLASS,
            SNIPPETS_MENU_CLASS,
            TRIGGERS_MENU_CLASS,
            HOST_NAME_CLASS,
            USER_NAME_CLASS,
            JOB_NAME_CLASS,
            CURRENT_DIRECTORY_CLASS,
            SEARCH_TOOL_CLASS,
            FILTER_TOOL_CLASS,
            COMPOSER_CLASS,
            statusbar::SPACER_CLASS,
            statusbar::SPRING_CLASS,
            INTERPOLATED_STRING_CLASS,
            CALL_SCRIPT_FUNCTION_CLASS,
        ];
        let reg = statusbar::registry();
        let classes: Vec<&str> = reg.iter().map(|i| i.class).collect();
        for (i, &class) in order.iter().enumerate() {
            assert_eq!(classes.get(i), Some(&class), "position {i}");
        }
        // 9i inserts its five (Battery, CPU, Memory, Network, git) at iTerm2's positions;
        // not one of them is here yet.
        for missing in ["iTermStatusBarBatteryComponent", "iTermStatusBarCPUUtilizationComponent", "iTermStatusBarMemoryUtilizationComponent", "iTermStatusBarNetworkUtilizationComponent", "iTermStatusBarGitComponent"] {
            assert!(!classes.contains(&missing), "{missing} is 9i's, not 9h's");
        }

        let get = |class: &str| statusbar::info(class).unwrap();
        let cases: [(&str, &str, &str, bool); 14] = [
            (CLOCK_CLASS, "Clock", "Shows current date and time with a configurable format.", true),
            (CUSTOM_ACTION_CLASS, "Custom Action", "Adds a button that performs a user-configurable action, similar to a key binding.", true),
            (ACTIONS_MENU_CLASS, "Actions Menu", "When clicked, opens a menu of actions. Actions are like custom key bindings, but without a keystroke attached.", true),
            (SNIPPETS_MENU_CLASS, "Snippets Menu", "When clicked, opens a menu of snippets. Snippets are saved text strings that can be pasted quickly.", true),
            (TRIGGERS_MENU_CLASS, "Triggers Menu", "When clicked, opens a menu of triggers. You can use it to enable or disable triggers.", true),
            (HOST_NAME_CLASS, "Host Name", "Current host name. Requires shell integration.", true),
            (USER_NAME_CLASS, "User Name", "Current user name. Requires shell integration.", true),
            (JOB_NAME_CLASS, "Job Name", "Shows the currently running job. If space permits, parent process names are also shown.", true),
            (CURRENT_DIRECTORY_CLASS, "Current Directory", "Current directory. Best with shell integration.", true),
            (SEARCH_TOOL_CLASS, "Search Tool", "Search tool to find text in the terminal window.", true),
            (FILTER_TOOL_CLASS, "Filter Tool", "Filter tool to remove non-matching lines from terminal window.", true),
            (COMPOSER_CLASS, "Composer", "Adds a text field for composing command lines.", false),
            (INTERPOLATED_STRING_CLASS, "Interpolated String", "Shows the evaluation of a string with inline expressions which may include session variables or the output of registered scripting functions", true),
            (CALL_SCRIPT_FUNCTION_CLASS, "Call Script Function", "Adds a button that invokes a script function with a user-provided invocation.", false),
        ];
        for (class, short, detail, stretch) in cases {
            let i = get(class);
            assert_eq!((i.short, i.detail, i.stretch), (short, detail, stretch), "{class}");
        }

        for internal in [PROGRESS_CLASS, TMUX_LEFT_CLASS, TMUX_RIGHT_CLASS] {
            assert!(get(internal).internal, "{internal} is internal");
        }
    }

    #[test]
    fn content_knobs_follow_iterm2() {
        let labels = |ks: &[Knob]| ks.iter().map(|k| k.label.clone()).collect::<Vec<_>>();

        // A stretchable text component's shared tail, either algorithm.
        let tail_tight = vec!["Text Color:", "Background Color:", "Custom Font", "Compression Resistance:", "Priority:", "Minimum Width:", "Maximum Width:"];
        let tail_stable = vec!["Text Color:", "Background Color:", "Custom Font", "Size Multiple:", "Priority:", "Minimum Width:", "Maximum Width:"];

        for kind in [Kind::ActionsMenu, Kind::SnippetsMenu, Kind::TriggersMenu, Kind::UserName, Kind::JobName, Kind::CurrentDirectory] {
            assert_eq!(labels(&knobs(kind, Algorithm::TightlyPacked)), tail_tight, "{kind:?}");
        }

        // Clock's four own knobs first, then the text tail.
        let clock = knobs(Kind::Clock, Algorithm::Stable);
        assert_eq!(labels(&clock)[..4], ["Date Format:", "Localize Date Format", "Show server time in SSH integration?", "Use server time zone in SSH integration?"]);
        assert_eq!(labels(&clock)[4..], tail_stable[..]);
        assert_eq!((clock[0].kind, clock[0].key.as_str(), clock[0].default.as_ref().and_then(Value::as_str)), (KnobKind::Text, FORMAT_KEY, Some("M/dd h:mm")));
        assert_eq!(clock[1].default, Some(Value::Boolean(true)));

        // Host Name's own knob first.
        let host = knobs(Kind::HostName, Algorithm::Stable);
        assert_eq!(host[0].label, "localhost replacement");
        assert_eq!((host[0].key.as_str(), host[0].default.as_ref().and_then(Value::as_str)), (ABBREVIATE_LOCALHOST_KEY, Some("")));
        assert_eq!(labels(&host)[1..], tail_stable[..]);

        // Custom Action's own Action knob first.
        let action = knobs(Kind::CustomAction, Algorithm::Stable);
        assert_eq!((action[0].label.as_str(), action[0].kind, action[0].key.as_str()), ("Action", KnobKind::Action, ACTION_KEY));
        assert_eq!(labels(&action)[1..], tail_stable[..]);

        // Interpolated String's own String Value knob first.
        let interp = knobs(Kind::InterpolatedString, Algorithm::Stable);
        assert_eq!((interp[0].label.as_str(), interp[0].kind, interp[0].key.as_str()), ("String Value:", KnobKind::Text, EXPRESSION_KEY));
        assert_eq!(labels(&interp)[1..], tail_stable[..]);

        // Search and Filter: only Color, keyed to the shared text color.
        for kind in [Kind::SearchTool, Kind::FilterTool] {
            let ks = knobs(kind, Algorithm::Stable);
            assert_eq!(labels(&ks), vec!["Color"]);
            assert_eq!((ks[0].kind, ks[0].key.as_str()), (KnobKind::Color, "shared text color"));
        }

        // Composer: Icon Color (the text color key), Background Color, Priority, min/max.
        let composer = knobs(Kind::Composer, Algorithm::Stable);
        assert_eq!(labels(&composer), vec!["Icon Color:", "Background Color:", "Priority:", "Minimum Width:", "Maximum Width:"]);
        assert_eq!(composer[0].key, "shared text color");
        assert_eq!(composer[1].key, "shared background color");

        // Call Script Function: Label, Function call (Invocation), Timeout (default 5),
        // Background Color, Text Color, Priority. No min/max, no compression, no font.
        let fn_call = knobs(Kind::CallScriptFunction, Algorithm::Stable);
        assert_eq!(labels(&fn_call), vec!["Label:", "Function call:", "Timeout (seconds):", "Background Color:", "Text Color:", "Priority:"]);
        assert_eq!((fn_call[0].key.as_str(), fn_call[0].placeholder.as_deref()), (LABEL_KEY, Some("Button Label")));
        assert_eq!((fn_call[1].kind, fn_call[1].key.as_str(), fn_call[1].placeholder.as_deref()), (KnobKind::Invocation, FUNCTION_INVOCATION_KEY, Some("foo(bar: \"baz\")")));
        assert_eq!((fn_call[2].key.as_str(), fn_call[2].default.clone(), fn_call[2].placeholder.as_deref()), (TIMEOUT_KEY, Some(Value::Float(5.0)), None));

        // Minimum Width's descriptor default is the STRING "0"; Maximum Width's is INFINITY.
        let mnmx = &knobs(Kind::UserName, Algorithm::Stable)[5..7];
        assert_eq!(mnmx[0].default, Some(Value::String("0".into())));
        assert_eq!(mnmx[1].default, Some(Value::Float(f64::INFINITY)));

        // Internal kinds have no Configure sheet.
        for kind in [Kind::Progress, Kind::TmuxLeft, Kind::TmuxRight] {
            assert!(knobs(kind, Algorithm::Stable).is_empty(), "{kind:?}");
        }

        // default_knobs: base always, plus each kind's own addition.
        assert_eq!(default_knobs(Kind::UserName), toml::Table::new());
        assert_eq!(default_knobs(Kind::Clock), table(&[(FORMAT_KEY, Value::String("M/dd h:mm".into()))]));
        for kind in [Kind::JobName, Kind::CurrentDirectory] {
            assert_eq!(default_knobs(kind), table(&[("maxwidth", Value::Float(f64::INFINITY)), ("minwidth", Value::Float(0.0))]), "{kind:?}");
        }
        assert_eq!(default_knobs(Kind::CallScriptFunction), table(&[(TIMEOUT_KEY, Value::Float(5.0))]));
        assert_eq!(
            statusbar::default_knobs(CLOCK_CLASS),
            table(&[("base: compression resistance", Value::Float(1.0)), ("base: priority", Value::Float(5.0)), (FORMAT_KEY, Value::String("M/dd h:mm".into()))])
        );

        // Icons: shared between Custom Action and Actions Menu; none for a field component.
        assert_eq!(icon(Kind::Clock), Some('\u{E823}'));
        assert_eq!(icon(Kind::CustomAction), Some('\u{E945}'));
        assert_eq!(icon(Kind::ActionsMenu), Some('\u{E945}'));
        assert_eq!(icon(Kind::SearchTool), None);
        assert_eq!(icon(Kind::InterpolatedString), None);
    }

    #[test]
    fn exemplars_follow_iterm2() {
        let none = toml::Table::new();
        assert_eq!(exemplar(CLOCK_CLASS, &none, "3:45 PM", "bob", "/home/bob"), Some("3:45 PM".to_string()));

        let action = table(&[(ACTION_KEY, crate::user_actions::UserAction { action: 0, title: "Run Tests".into(), parameter: String::new(), escaping: crate::keyaction::Escaping::Common, apply_mode: 0, version: crate::user_actions::VERSION }.to_value())]);
        assert_eq!(exemplar(CUSTOM_ACTION_CLASS, &action, "", "", ""), Some("Run Tests".to_string()));
        assert_eq!(exemplar(CUSTOM_ACTION_CLASS, &none, "", "", ""), Some("Custom Action".to_string()));

        assert_eq!(exemplar(USER_NAME_CLASS, &none, "", "bob", "/home/bob"), Some("bob".to_string()));
        assert_eq!(exemplar(CURRENT_DIRECTORY_CLASS, &none, "", "bob", "/home/bob"), Some("/home/bob".to_string()));

        let expr = table(&[(EXPRESSION_KEY, Value::String("session.name".into()))]);
        assert_eq!(exemplar(INTERPOLATED_STRING_CLASS, &expr, "", "", ""), Some("session.name".to_string()));
        assert_eq!(exemplar(INTERPOLATED_STRING_CLASS, &none, "", "", ""), Some("\\(expression)".to_string()));

        let label = table(&[(LABEL_KEY, Value::String("Reload".into()))]);
        assert_eq!(exemplar(CALL_SCRIPT_FUNCTION_CLASS, &label, "", "", ""), Some("Reload".to_string()));
        assert_eq!(exemplar(CALL_SCRIPT_FUNCTION_CLASS, &none, "", "", ""), Some("foo(bar: \"baz\")".to_string()));

        assert_eq!(exemplar(SEARCH_TOOL_CLASS, &none, "", "", ""), Some("\u{1F50E} Search".to_string()));
        assert_eq!(exemplar(FILTER_TOOL_CLASS, &none, "", "", ""), Some("\u{2979} Filter".to_string()));
        assert_eq!(exemplar(COMPOSER_CLASS, &none, "", "", ""), Some(">_ [Command] \u{1F4AC}".to_string()));

        // A class this module does not know: None, so the caller falls back.
        assert_eq!(exemplar("iTermStatusBarBatteryComponent", &none, "", "", ""), None);
    }

    /// `ACT:202-226`, the Snippets Menu's `statusBarComponentIsEmpty`, and the Triggers
    /// Menu's, which is inverted in iTerm2 (empty when there ARE triggers; kept, the 9c
    /// ruling); `PTYSession.m:23172-23180` for a trigger's title.
    #[test]
    fn menu_components_follow_iterm2() {
        assert!(menu_empty(Kind::ActionsMenu, 0, 3, 3));
        assert!(!menu_empty(Kind::ActionsMenu, 1, 0, 0));
        assert!(menu_empty(Kind::SnippetsMenu, 3, 0, 3));
        assert!(!menu_empty(Kind::SnippetsMenu, 0, 2, 0));
        assert!(menu_empty(Kind::TriggersMenu, 0, 0, 2), "iTerm2's bug: empty WITH triggers");
        assert!(!menu_empty(Kind::TriggersMenu, 3, 3, 0), "iTerm2's bug: shown with none");
        assert!(!menu_empty(Kind::CustomAction, 0, 0, 0), "Custom Action is never empty");

        assert_eq!(trigger_menu_title("Highlight Text\u{2026}", "err.*"), "Highlight Text \u{2014} err.*");
        assert_eq!(trigger_menu_title("Ring Bell", "^\\$ "), "Ring Bell \u{2014} ^\\$ ");
        assert_eq!(trigger_menu_title("A\u{2026}\u{2026}", "x"), "A\u{2026} \u{2014} x", "one suffix only");

        assert_eq!(ACTIONS_MENU_TEXT, "Perform Action\u{2026}");
        assert_eq!(SNIPPETS_MENU_TEXT, "Send Snippet\u{2026}");
        assert_eq!(TRIGGERS_MENU_TEXT, "Triggers\u{2026}");
    }
}
