//! What each content component on a status bar shows (milestone 9h): a component and its
//! session become a `Content` (width variants, copy text, empty, icon, click), which
//! `statusbar_ui` measures, lays out and draws. Text components here: Clock, Host Name,
//! User Name, Job Name, Current Directory (and its click menu), Interpolated String; the
//! menu and button components: Custom Action, Actions Menu, Snippets Menu, Triggers Menu
//! (and their menus), Call Script Function; the field components: Search Tool, Filter
//! Tool (both bound to the pane's find bar), the Composer stub, and the paste Progress
//! Indicator.
//!
//! Citations use `.superpowers/research/m9-facts.md` section 2's abbreviations (`TXT` =
//! `iTermStatusBarTextComponent.m`, `VAR` = `iTermStatusBarVariableBaseComponent.m`, `JOB`
//! = `iTermStatusBarJobComponent.m`, `CLK` = `iTermStatusBarClockComponent.m`, `SWS` =
//! `iTermStatusBarSwiftyStringComponent.m`, `ACT` = `iTermStatusBarActionComponent.m`,
//! `FN` = `iTermStatusBarFunctionCallComponent.m`, `PS` = `PTYSession.m`).

use super::statusbar_ui::Click;
use super::*;
use crate::broadcast::Write;
use crate::statusbar::Component;
use crate::statusbar_components::{self as sbc, Kind};
use crate::statusbar_text as sbt;

const FORMAT_KEY: &str = "format";
const LOCALIZE_KEY: &str = "localize";
/// `CLK`: the format when the knob is absent.
const DEFAULT_CLOCK_FORMAT: &str = "M/dd h:mm";
const ABBREVIATE_LOCALHOST_KEY: &str = "abbreviate-localhost";
const EXPRESSION_KEY: &str = "expression";
const ACTION_KEY: &str = "action";
const LABEL_KEY: &str = "label";
/// Call Script Function's click until milestone 11 brings the Python API.
pub(super) const SCRIPT_REFUSAL: &str = "Script functions need the Python API, which arrives in milestone 11.";
/// A button's face is its label plus this many points (6 on each side).
pub(super) const BUTTON_PAD_PT: f32 = 12.0;
/// `FN`: `statusBarComponentVerticalOffset`.
pub(super) const BUTTON_DY_PT: f32 = 0.5;
/// `SWS:130`: what an Interpolated String shows when its expression fails.
pub(super) const LADYBUG: &str = "\u{1F41E}";
/// The fields' placeholders (the xibs' `placeholderString`, and the Composer's).
const SEARCH_PLACEHOLDER: &str = "Search";
const FILTER_PLACEHOLDER: &str = "Filter";
const COMPOSER_PLACEHOLDER: &str = "Command";
/// The Search and Filter Tools' and the Progress Indicator's (minimum, preferred) points.
const FIELD_SIZE: (f32, f32) = (125.0, 200.0);
/// The Composer's click until its milestone.
pub(super) const COMPOSER_REFUSAL: &str = "The Composer arrives with its own milestone.";
/// A glyph's column in a field (the magnifier, a chevron, the close glyph), in points.
pub(super) const GLYPH_PT: f32 = 17.0;
/// The room between a field's text, its label and its edges, in points.
const FIELD_PAD_PT: f32 = 4.0;
/// Segoe Fluent Icons: Search, ChevronLeft, ChevronRight, Cancel.
pub(super) const MAGNIFIER: char = '\u{E721}';
pub(super) const CHEVRON_LEFT: char = '\u{E76B}';
pub(super) const CHEVRON_RIGHT: char = '\u{E76C}';
pub(super) const CLOSE_GLYPH: char = '\u{E711}';

/// What a field component draws in its room, instead of text.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Widget {
    /// A bordered field: the query (or the placeholder at half alpha), the results label,
    /// and the glyphs it has.
    /// `focused`: it has the keyboard.
    Field { text: String, placeholder: &'static str, label: String, error: bool, focused: bool, magnifier: bool, chevrons: bool, close: bool },
    /// The paste's track, filled to the fraction, and a cancel glyph.
    Progress(f32),
}

/// A field's parts, in points from its left edge: (from, to) each.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FieldParts {
    pub(super) mode: Option<(f32, f32)>,
    pub(super) text: (f32, f32),
    pub(super) label: (f32, f32),
    pub(super) prev: Option<(f32, f32)>,
    pub(super) next: Option<(f32, f32)>,
    pub(super) close: Option<(f32, f32)>,
}

/// Where a click on a field lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FieldPart {
    Mode,
    Text,
    Prev,
    Next,
    Close,
}

/// A field `w` points wide (`iTermMiniSearchFieldViewController.xib`, `MiniFilterViewController
/// .xib`): the magnifier's column at the left; at the right the close glyph, before it the
/// two chevrons, before them the results label `label_w` wide; the text in what is left.
pub(super) fn field_parts(w: f32, magnifier: bool, chevrons: bool, close: bool, label_w: f32) -> FieldParts {
    let mut right = w;
    let mut take = |on: bool| {
        on.then(|| {
            let part = ((right - GLYPH_PT).max(0.0), right);
            right = part.0;
            part
        })
    };
    let close = take(close);
    let next = take(chevrons);
    let prev = take(chevrons);
    let label_right = if label_w > 0.0 { (right - FIELD_PAD_PT).max(0.0) } else { right };
    let label = ((label_right - label_w).max(0.0), label_right);
    let text_right = if label_w > 0.0 { label.0 } else { right };
    let left = if magnifier { GLYPH_PT.min(w) } else { FIELD_PAD_PT.min(w) };
    FieldParts { mode: magnifier.then_some((0.0, left)), text: (left, (text_right - FIELD_PAD_PT).max(left)), label, prev, next, close }
}

/// The part of a field under `x` (points from its left); the text and label focus it.
pub(super) fn part_at(parts: &FieldParts, x: f32) -> FieldPart {
    let within = |p: Option<(f32, f32)>| p.is_some_and(|(a, b)| x >= a && x < b);
    if within(parts.mode) {
        FieldPart::Mode
    } else if within(parts.close) {
        FieldPart::Close
    } else if within(parts.next) {
        FieldPart::Next
    } else if within(parts.prev) {
        FieldPart::Prev
    } else {
        FieldPart::Text
    }
}

/// What one content component shows and does now.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Content {
    /// Width variants (`stringVariants`), in the component's own order; the widest one
    /// that fits is drawn (`TXT:178-229`).
    pub(super) variants: Vec<String>,
    /// User Name: the minimum width is the narrower of the full name and this; the full
    /// name is always drawn (`VAR:234-247`).
    pub(super) min_text: Option<String>,
    /// The right-click Copy text when it is not the widest variant (Current Directory's
    /// raw path, `VAR:314-316`).
    pub(super) copy: Option<String>,
    /// `statusBarComponentIsEmpty`.
    pub(super) empty: bool,
    /// A Segoe Fluent Icons glyph in a 17 point column at the left.
    pub(super) icon: Option<char>,
    /// The text sits at the right of the component's room.
    pub(super) right_align: bool,
    /// A component sized by numbers, not text: (minimum, preferred) points.
    pub(super) fixed: Option<(f32, f32)>,
    /// A button (Call Script Function): its label on a rounded box in its background
    /// color, `BUTTON_PAD_PT` wider than the label.
    pub(super) button: bool,
    /// `statusBarComponentCopyableString` is nil: no Copy in the right-click menu.
    pub(super) uncopyable: bool,
    pub(super) click: Click,
    /// A field component's drawing, in place of text.
    pub(super) widget: Option<Widget>,
}

impl Content {
    /// A text component with these variants: empty when the widest is "" (`TXT`); no
    /// variants at all is one empty variant.
    pub(super) fn text(variants: Vec<String>) -> Content {
        let variants = if variants.is_empty() { vec![String::new()] } else { variants };
        let empty = variants.iter().all(|v| v.is_empty());
        Content { variants, min_text: None, copy: None, empty, icon: None, right_align: false, fixed: None, button: false, uncopyable: false, click: Click::None, widget: None }
    }
}

/// A knob's string value, else its default.
fn knob_str(c: &Component, kind: Kind, key: &str) -> Option<String> {
    c.knobs.get(key).or(sbc::default_knobs(kind).get(key)).and_then(toml::Value::as_str).map(str::to_string)
}

/// `JOB:109-149`: the deepest job first, then its parents up to and including the
/// session's root process, unless that is `login`. Tandem's ancestors are the job and its
/// parents below the shell, deepest first (empty when the job is the shell itself), so the
/// job's own name comes first, the ancestors after their first entry follow, and `root`
/// (the shell's name) ends the chain. Empty before the window has seen the session's
/// processes; one empty name once it has and no job is left (the shell exited), which
/// `job_variants` shows as U+2026 (`JOB:92-99`).
pub(super) fn job_chain(job: Option<&str>, ancestors: &[String], root: Option<&str>) -> Vec<String> {
    let Some(name) = job else { return if root.is_some() { vec![String::new()] } else { Vec::new() } };
    let mut chain: Vec<String> = std::iter::once(name.to_string()).chain(ancestors.iter().skip(1).cloned()).collect();
    if let Some(root) = root.filter(|r| !ancestors.is_empty() && *r != "login") {
        chain.push(root.to_string());
    }
    chain
}

/// The last component of a path, either separator (`lastPathComponent`).
pub(super) fn folder_name(path: &str) -> String {
    let trimmed = path.trim_end_matches(['\\', '/']);
    trimmed.rsplit(['\\', '/']).next().filter(|s| !s.is_empty()).unwrap_or(trimmed).to_string()
}

/// A recent directory's menu title (`VAR:361-369`): starred ones are marked.
pub(super) fn dir_title(path: &str, starred: bool) -> String {
    if starred { format!("\u{2605} {path}") } else { path.to_string() }
}

/// `SWS:132`: the message a click on a failed Interpolated String shows.
pub(super) fn evaluation_error(expression: &str, error: &str) -> String {
    format!("Error while evaluating \u{201C}{expression}\u{201D}:\n{error}")
}

/// `ACT:75-80`: what a Custom Action shows when its title does not evaluate.
pub(super) fn custom_action_error(error: &str) -> String {
    format!("{LADYBUG} {error}")
}

/// The session's own trigger dictionaries, in order (`PS`: `_config.triggerProfileDicts`).
fn trigger_list(p: Option<&crate::profile::Profile>) -> Vec<crate::triggers::Trigger> {
    p.and_then(|p| p.get("Triggers")).map(|v| crate::triggers::parse_list(&v)).unwrap_or_default()
}

/// The Triggers Menu's items (`PS:23172-23187`): (place in the list, title, on) for each
/// trigger whose action Tandem knows; iTerm2 leaves out a trigger it cannot build.
pub(super) fn trigger_items(list: &[crate::triggers::Trigger]) -> Vec<(usize, String, bool)> {
    list.iter()
        .enumerate()
        .filter_map(|(i, t)| crate::triggers::action_info(&t.action).map(|a| (i, sbc::trigger_menu_title(a.title, &t.regex), t.enabled)))
        .collect()
}

/// Where a component's menu opens, in window pixels: at the component's left, beside the
/// bar on the terminal's side (`above`: its bottom at `y`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MenuAt {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) above: bool,
}

/// A reported path as shown and copied: the Windows form of an OSC 7 path (`/C:/x`),
/// else as reported.
fn shown_path(raw: &str) -> String {
    crate::profile::local_path(raw).map_or_else(|| raw.to_string(), |p| p.to_string_lossy().into_owned())
}

/// The names this computer answers to, for `is_local_host`: localhost, the computer name
/// and its full DNS name.
pub(super) fn local_names() -> &'static [String] {
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names = vec!["localhost".to_string()];
        names.extend(std::env::var("COMPUTERNAME").ok().filter(|n| !n.is_empty()));
        let full = super::aps_ui::local_host();
        if !full.is_empty() {
            names.push(full);
        }
        names
    })
}

/// `session.isLocalhost`: a session with no reported host is on this computer (iTerm2
/// starts every session with the local host).
pub(super) fn is_localhost(host: Option<&str>) -> bool {
    host.is_none_or(|h| sbt::is_local_host(h, local_names()))
}

/// The local time, broken down.
#[cfg(windows)]
fn local_civil() -> sbt::Civil {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    sbt::Civil {
        year: t.wYear as i32,
        month: t.wMonth as u32,
        day: t.wDay as u32,
        weekday: t.wDayOfWeek as u32,
        hour: t.wHour as u32,
        minute: t.wMinute as u32,
        second: t.wSecond as u32,
        millis: t.wMilliseconds as u32,
    }
}

/// `dateFormatFromTemplate:options:0 locale:currentLocale` (`CLK`): ICU's best pattern for
/// the template in the user's locale, or the template itself when ICU fails. Cached per
/// template.
#[cfg(windows)]
fn best_pattern(template: &str) -> String {
    use windows_sys::Win32::Globalization::{U_ZERO_ERROR, udatpg_close, udatpg_getBestPattern, udatpg_open};
    thread_local! {
        static PATTERNS: std::cell::RefCell<HashMap<String, String>> = std::cell::RefCell::new(HashMap::new());
    }
    if let Some(p) = PATTERNS.with(|m| m.borrow().get(template).cloned()) {
        return p;
    }
    let found = (|| {
        let mut err = U_ZERO_ERROR;
        // A null locale is ICU's default, the user's.
        let g = unsafe { udatpg_open(std::ptr::null(), &mut err) };
        if g.is_null() || err > U_ZERO_ERROR {
            return None;
        }
        let skeleton: Vec<u16> = template.encode_utf16().collect();
        let mut buf = vec![0u16; 256];
        let n = unsafe { udatpg_getBestPattern(g, skeleton.as_ptr(), skeleton.len() as i32, buf.as_mut_ptr(), buf.len() as i32, &mut err) };
        unsafe { udatpg_close(g) };
        (err <= U_ZERO_ERROR && n > 0 && (n as usize) <= buf.len()).then(|| String::from_utf16_lossy(&buf[..n as usize]))
    })();
    let pattern = found.unwrap_or_else(|| template.to_string());
    PATTERNS.with(|m| m.borrow_mut().insert(template.to_string(), pattern.clone()));
    pattern
}

/// The Clock's text for its knobs (`CLK`): the format, through ICU when localized.
#[cfg(windows)]
pub(super) fn clock_text(format: &str, localize: bool) -> String {
    let pattern = if localize { best_pattern(format) } else { format.to_string() };
    sbt::format_tr35(&pattern, &local_civil())
}

/// A Clock's text for its knob values (defaults for the missing ones): the setup
/// window's sample.
#[cfg(windows)]
pub(super) fn clock_for_knobs(knobs: &toml::Table) -> String {
    let format = knobs.get(FORMAT_KEY).and_then(toml::Value::as_str).unwrap_or(DEFAULT_CLOCK_FORMAT);
    clock_text(format, knobs.get(LOCALIZE_KEY).and_then(toml::Value::as_bool).unwrap_or(true))
}

/// The next whole second of the wall clock (the Clock's cadence is 1 s).
pub(super) fn next_whole_second(now: Instant) -> Instant {
    let into = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_millis());
    now + std::time::Duration::from_millis(u64::from(1000 - into.min(999)))
}

impl App {
    /// What a content component shows for a session now.
    pub(super) fn content(&self, pane: PaneId, kind: Kind, c: &Component) -> Content {
        // The session's `hostname`, `username` and `path` as `variables` reports them, read
        // directly: this runs every frame, and `variables` copies the selection.
        let shell = self.panes.get(&pane).map(|p| &p.own().shell);
        let reported_host = shell.and_then(|s| s.host.clone());
        let mut content = match kind {
            // `CLK`: one variant; the SSH knobs wait for milestone 14.
            Kind::Clock => Content::text(vec![clock_for_knobs(&c.knobs)]),
            // `VAR:169-202`: the replacement alone on this computer, else the host's
            // variants. The local host name stands in before shell integration reports one.
            Kind::HostName => {
                let local = is_localhost(reported_host.as_deref());
                let host = reported_host.unwrap_or_else(super::aps_ui::local_host);
                let replacement = knob_str(c, kind, ABBREVIATE_LOCALHOST_KEY).unwrap_or_default();
                if !replacement.is_empty() && local { Content::text(vec![replacement]) } else { Content::text(sbt::host_variants(&host)) }
            }
            // `VAR:234-247`: the full name, always; this computer's user before shell
            // integration reports one.
            Kind::UserName => {
                let user = shell.and_then(|s| s.user.clone()).unwrap_or_else(|| std::env::var("USERNAME").unwrap_or_default());
                let min_text = sbt::user_min_text(&user);
                Content { min_text: (!min_text.is_empty()).then_some(min_text), ..Content::text(vec![user]) }
            }
            // `JOB:85-149`: [""] before the first job; U+2026 when every name is empty.
            Kind::JobName => {
                let chain = shell.map_or_else(Vec::new, |s| job_chain(s.job.as_ref().map(|j| j.name.as_str()), &s.job_ancestors, s.job_root.as_deref()));
                Content::text(sbt::job_variants(&chain))
            }
            // `VAR:252-345`: home as `~`, compressed from the first component; empty when
            // blank; Copy is the whole path; a click opens the directory menu.
            Kind::CurrentDirectory => {
                // A tmux pane's own directory stands in until shell integration reports one.
                let tmux_cwd = || self.panes.get(&pane).and_then(|p| p.tmux_io()).map(|io| io.info.borrow().cwd.clone());
                let raw = shell.and_then(|s| s.cwd.clone()).or_else(tmux_cwd).unwrap_or_default();
                let path = shown_path(&raw);
                let home = std::env::var("USERPROFILE").unwrap_or_default();
                let display = sbt::display_dir(&path, &home);
                let blank = raw.trim().is_empty();
                Content {
                    copy: (!blank).then(|| path.clone()),
                    empty: blank,
                    click: Click::Menu(Kind::CurrentDirectory),
                    ..Content::text(if blank { Vec::new() } else { sbt::dir_variants(&display) })
                }
            }
            // `SWS`: one variant, the value; a failed expression shows the ladybug and a
            // click shows why.
            Kind::InterpolatedString => {
                let expression = knob_str(c, kind, EXPRESSION_KEY).unwrap_or_default();
                match self.evaluate_for(pane, &expression) {
                    Ok(value) => Content::text(vec![value]),
                    Err(e) => Content { click: Click::Error(evaluation_error(&expression, &e)), ..Content::text(vec![LADYBUG.to_string()]) },
                }
            }
            // `ACT:19-135`: the action's title, evaluated, shortened a word at a time; a
            // failed title shows the ladybug and the error. Never empty, never copied; a
            // click performs the action.
            Kind::CustomAction => {
                let action = c.knobs.get(ACTION_KEY).and_then(crate::user_actions::UserAction::from_value);
                let title = action.as_ref().map(|a| a.title.clone()).unwrap_or_default();
                let variants = match self.evaluate_for(pane, &title) {
                    Ok(value) => sbt::action_variants(&value),
                    Err(e) => vec![custom_action_error(&e)],
                };
                let click = if action.is_some() { Click::Run } else { Click::None };
                Content { empty: false, uncopyable: true, click, ..Content::text(variants) }
            }
            // `ACT:137-232`, `iTermStatusBarSnippetComponent.m`,
            // `iTermStatusBarTriggersComponent.swift`: one fixed text, a click opens the
            // component's menu, never copied, empty by `menu_empty`.
            Kind::ActionsMenu | Kind::SnippetsMenu | Kind::TriggersMenu => {
                let text = match kind {
                    Kind::ActionsMenu => sbc::ACTIONS_MENU_TEXT,
                    Kind::SnippetsMenu => sbc::SNIPPETS_MENU_TEXT,
                    _ => sbc::TRIGGERS_MENU_TEXT,
                };
                let empty = match kind {
                    Kind::ActionsMenu => sbc::menu_empty(kind, crate::user_actions::load(self.globals.get("Actions").as_ref()).len(), 0, 0),
                    Kind::SnippetsMenu => sbc::menu_empty(kind, 0, self.snippets.snippets.len(), 0),
                    _ => sbc::menu_empty(kind, 0, 0, self.session_trigger_count(pane)),
                };
                Content { empty, uncopyable: true, click: Click::Menu(kind), ..Content::text(vec![text.to_string()]) }
            }
            // `FN`: a button sized to its label; a click would call the function (milestone
            // 11), so it says why it does not.
            Kind::CallScriptFunction => {
                let label = c.knobs.get(LABEL_KEY).and_then(toml::Value::as_str).unwrap_or_default().to_string();
                Content { empty: false, button: true, uncopyable: true, click: Click::Refuse(SCRIPT_REFUSAL), ..Content::text(vec![label]) }
            }
            // `SRCH`, `FILT`: minimum 125, preferred 200; the pane's query, bound to its
            // find bar. The Search Tool shows the query Find keeps between opens, its match
            // count while find is open, and the chevrons; a temporary one closes. The
            // Filter Tool shows the filter's query while Filter is on and always closes.
            Kind::SearchTool | Kind::FilterTool => {
                let f = self.finds.get(&pane);
                let filtering = f.is_some_and(|f| f.open && f.filter);
                // `MiniFilterViewController.xib` has no chevrons.
                let active = if kind == Kind::SearchTool { f.is_some_and(|f| f.open && !f.filter) } else { filtering };
                let (text, label, chevrons, close, placeholder) = if kind == Kind::SearchTool {
                    let text = f.filter(|f| !f.filter).map(|f| f.query.clone()).unwrap_or_default();
                    let label = f.filter(|f| active && !f.query.is_empty()).map(|f| f.results.label()).unwrap_or_default();
                    (text, label, true, sbc::is_temporary(c), SEARCH_PLACEHOLDER)
                } else {
                    let text = f.filter(|_| filtering).map(|f| f.query.clone()).unwrap_or_default();
                    (text, String::new(), false, true, FILTER_PLACEHOLDER)
                };
                let error = active && f.is_some_and(|f| f.error.is_some());
                let focused = active && f.is_some_and(|f| f.typing);
                let widget = Widget::Field { text, placeholder, label, error, focused, magnifier: true, chevrons, close };
                Content { empty: false, uncopyable: true, fixed: Some(FIELD_SIZE), click: Click::Field, widget: Some(widget), ..Content::text(Vec::new()) }
            }
            // `COMP:54-66`: minimum 200, preferred infinity, not stretchable; a stub until
            // its milestone: an inert field, and a click that says so.
            Kind::Composer => {
                let widget = Widget::Field { text: String::new(), placeholder: COMPOSER_PLACEHOLDER, label: String::new(), error: false, focused: false, magnifier: false, chevrons: false, close: false };
                Content { empty: false, uncopyable: true, fixed: Some((200.0, f32::INFINITY)), click: Click::Refuse(COMPOSER_REFUSAL), widget: Some(widget), ..Content::text(Vec::new()) }
            }
            // `iTermStatusBarProgressComponent.m`: minimum 125, preferred 200; the paste's
            // fraction sent, and a cancel glyph.
            Kind::Progress => {
                let fraction = self.pastes.get(&pane).and_then(|(q, _, _)| q.progress()).map_or(0.0, |(sent, total)| sent as f32 / total.max(1) as f32);
                Content { empty: false, uncopyable: true, fixed: Some(FIELD_SIZE), click: Click::Cancel, widget: Some(Widget::Progress(fraction.clamp(0.0, 1.0))), ..Content::text(Vec::new()) }
            }
            // `iTermStatusBarLayout+tmux.m`: an Interpolated String (of `tmuxStatusLeft` or
            // `tmuxStatusRight`) whose variants drop `" | "` parts, trailing ones on the
            // left and leading ones on the right; the right one sits at the right.
            Kind::TmuxLeft | Kind::TmuxRight => {
                let expression = knob_str(c, kind, EXPRESSION_KEY).unwrap_or_default();
                match self.evaluate_for(pane, &expression) {
                    Ok(value) if kind == Kind::TmuxLeft => Content::text(sbt::tmux_left_variants(&value)),
                    Ok(value) => Content { right_align: true, ..Content::text(sbt::tmux_right_variants(&value)) },
                    Err(e) => Content { click: Click::Error(evaluation_error(&expression, &e)), ..Content::text(vec![LADYBUG.to_string()]) },
                }
            }
        };
        content.icon = sbc::icon(kind);
        content
    }

    /// An interpolated string evaluated in the session (`iTermSwiftyString` with no side
    /// effects). The session's variables are gathered only when the string names one.
    fn evaluate_for(&self, pane: PaneId, expression: &str) -> Result<String, String> {
        let vars = std::cell::OnceCell::new();
        let lookup = |n: &str| crate::prompted::variable(n.strip_prefix("session.").unwrap_or(n), vars.get_or_init(|| self.variables(pane)));
        crate::interp::evaluate(expression, &lookup, &[])
    }

    /// How many triggers the session's profile has (`PS:23168-23170`).
    fn session_trigger_count(&self, pane: PaneId) -> usize {
        self.profile_of(pane).and_then(|p| p.get("Triggers")).and_then(|v| v.as_array().map(Vec::len)).unwrap_or(0)
    }

    /// Custom Action's click (`ACT:129-133`, `PS:23137-23139`): the action runs as Edit >
    /// Actions runs it, apply mode and all, in the bar's session (already the focused one).
    pub(super) fn run_custom_action(&mut self, c: &Component, event_loop: &ActiveEventLoop) {
        if let Some(a) = c.knobs.get(ACTION_KEY).and_then(crate::user_actions::UserAction::from_value) {
            self.run_key_action(&a.binding(), None, None, event_loop);
        }
    }

    /// Shows a component's menu at `at`. Windows drops a menu down from its point; one
    /// that must open above the bar is moved up as it appears, its bottom on `at.y`.
    pub(super) fn show_menu_at(&mut self, menu: muda::Menu, at: MenuAt) {
        use muda::ContextMenu;
        let Some(hwnd) = self.hwnd() else { return };
        let position = muda::dpi::Position::Physical(muda::dpi::PhysicalPosition::new(at.x as i32, at.y as i32));
        let lift = at.above.then(|| above::Lift::new(hwnd, at.x, at.y));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, Some(position)) };
        drop(lift);
        self.popup_menu = Some(menu);
    }

    /// The Actions, Snippets and Triggers Menus' clicks. Actions (`ACT:196-226`): one item
    /// per action, with Edit > Actions' ids, a separator (there even with no actions), and
    /// Edit Actions. Snippets (`iTermStatusBarSnippetComponent.m:77-92`): Edit > Snippets'
    /// items, a separator, and Edit Snippets. Triggers (`iTermStatusBarTriggersComponent
    /// .swift:91-111`): one checked-when-on item per trigger, a separator, Add Trigger and
    /// Edit Triggers.
    pub(super) fn show_component_menu(&mut self, pane: PaneId, kind: Kind, at: MenuAt) {
        use muda::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
        let menu = Menu::new();
        let mut errors = Vec::new();
        let add = |item: &dyn muda::IsMenuItem, errors: &mut Vec<String>| {
            if let Err(e) = menu.append(item) {
                errors.push(e.to_string());
            }
        };
        match kind {
            Kind::ActionsMenu => {
                for (i, a) in crate::user_actions::load(self.globals.get("Actions").as_ref()).iter().enumerate() {
                    add(&MenuItem::with_id(format!("{}{i}", super::snippets_ui::ACTION_ID), a.title.replace('&', "&&"), true, None), &mut errors);
                }
                add(&PredefinedMenuItem::separator(), &mut errors);
                add(&MenuItem::with_id("statusbar:edit-actions", "Edit Actions\u{2026}", true, None), &mut errors);
            }
            Kind::SnippetsMenu => {
                self.append_snippet_items(&menu, &mut errors);
                add(&PredefinedMenuItem::separator(), &mut errors);
                add(&MenuItem::with_id("statusbar:edit-snippets", "Edit Snippets\u{2026}", true, None), &mut errors);
            }
            Kind::TriggersMenu => {
                for (i, title, on) in trigger_items(&trigger_list(self.profile_of(pane))) {
                    add(&CheckMenuItem::with_id(format!("statusbar:trigger:{i}"), title.replace('&', "&&"), true, on, None), &mut errors);
                }
                add(&PredefinedMenuItem::separator(), &mut errors);
                add(&MenuItem::with_id("statusbar:add-trigger", "Add Trigger\u{2026}", true, None), &mut errors);
                add(&MenuItem::with_id("statusbar:edit-triggers", "Edit Triggers\u{2026}", true, None), &mut errors);
            }
            _ => return,
        }
        if let Some(e) = errors.first() {
            self.banners.set("menu", format!("Status bar menu: {e}"));
        }
        self.status_menu = Some(super::statusbar_ui::StatusMenu::for_component(pane, kind.class(), Vec::new(), None));
        self.show_menu_at(menu, at);
    }

    /// A pick from the Actions, Snippets or Triggers Menu (their action and snippet items
    /// go through Edit > Actions' and Edit > Snippets' own path). True when `what` was one.
    pub(super) fn component_menu_picked(&mut self, what: &str, pane: PaneId, event_loop: &ActiveEventLoop) -> bool {
        use crate::settings_shortcuts::Tab;
        match what {
            // `statusBarEditActions`, `statusBarEditSnippets` (`PS:23141-23147`).
            "edit-actions" => self.open_shortcuts_tab(Tab::Actions, event_loop),
            "edit-snippets" => self.open_shortcuts_tab(Tab::Snippets, event_loop),
            // `PS:23189-23195`: as Session > Triggers does it.
            "add-trigger" => {
                self.run_trigger_command_menu("addTrigger:", event_loop);
            }
            "edit-triggers" => {
                self.run_trigger_command_menu("editTriggers:", event_loop);
            }
            _ => {
                let Some(i) = what.strip_prefix("trigger:").and_then(|i| i.parse::<usize>().ok()) else { return false };
                self.toggle_session_trigger(pane, i);
            }
        }
        true
    }

    /// `PS:4506-4512`: the trigger's `disabled` flips in the session's own profile (a
    /// session copy, made now when there is none), so other sessions keep theirs.
    fn toggle_session_trigger(&mut self, pane: PaneId, index: usize) {
        let mut list = trigger_list(self.profile_of(pane));
        let Some(t) = list.get_mut(index) else { return };
        t.enabled = !t.enabled;
        self.set_session_values(pane, &[("Triggers".to_string(), Some(crate::triggers::to_value(&list)))]);
        self.sync_checks();
        self.redraw_all();
    }

    /// Settings > Shortcuts on one tab.
    fn open_shortcuts_tab(&mut self, tab: crate::settings_shortcuts::Tab, event_loop: &ActiveEventLoop) {
        self.open_settings_page(event_loop, crate::settings_ui::Page::Shortcuts);
        if let Some(s) = self.settings.as_mut() {
            s.show_shortcuts_tab(tab);
        }
    }

    /// Current Directory's click (`VAR:355-413`): the host's ten best recent directories
    /// (picking one types `cd <path>`), then, with a known path, Copy Path, Copy Folder
    /// Name, Show in Explorer, New Session Here in New Window and in New Tab.
    pub(super) fn show_dir_menu(&mut self, pane: PaneId, path: Option<String>, at: MenuAt) {
        use muda::{Menu, MenuItem, PredefinedMenuItem};
        let host = self.history_host_pub(pane);
        let dirs: Vec<crate::shell_history::Dir> = self.shell_history_pub().dirs(&host).into_iter().take(10).collect();
        if dirs.is_empty() && path.is_none() {
            return;
        }
        let menu = Menu::new();
        let mut results = Vec::new();
        for (n, d) in dirs.iter().enumerate() {
            results.push(menu.append(&MenuItem::with_id(format!("statusbar:dir:{n}"), dir_title(&d.path, d.starred).replace('&', "&&"), true, None)));
        }
        if path.is_some() {
            results.push(menu.append(&PredefinedMenuItem::separator()));
            results.push(menu.append(&MenuItem::with_id("statusbar:dir-copy-path", "Copy Path", true, None)));
            results.push(menu.append(&MenuItem::with_id("statusbar:dir-copy-name", "Copy Folder Name", true, None)));
            results.push(menu.append(&MenuItem::with_id("statusbar:dir-reveal", "Show in Explorer", true, None)));
            results.push(menu.append(&PredefinedMenuItem::separator()));
            results.push(menu.append(&MenuItem::with_id("statusbar:dir-new-window", "New Session Here in New Window", true, None)));
            results.push(menu.append(&MenuItem::with_id("statusbar:dir-new-tab", "New Session Here in New Tab", true, None)));
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Status bar menu: {e}"));
        }
        let choices = dirs.into_iter().map(|d| d.path).collect();
        self.status_menu = Some(super::statusbar_ui::StatusMenu::for_component(pane, Kind::CurrentDirectory.class(), choices, path));
        self.show_menu_at(menu, at);
    }

    /// A pick from the directory menu (`VAR:415-495`). True when `what` was one.
    pub(super) fn dir_menu_picked(&mut self, what: &str, pane: PaneId, choices: &[String], path: Option<&str>, event_loop: &ActiveEventLoop) -> bool {
        if let Some(n) = what.strip_prefix("dir:") {
            // `cd <path>`, shell-escaped, with no newline.
            let Some(dir) = n.parse::<usize>().ok().and_then(|n| choices.get(n)) else { return true };
            let shell = self.panes.get(&pane).and_then(|p| p.own().shell.shell.clone());
            let text = format!("cd {}", crate::shell_history::insertable_dir(dir, shell.as_deref()));
            self.user_write(pane, text.as_bytes(), Write::SendText);
            return true;
        }
        let Some(path) = path else { return what.starts_with("dir-") };
        match what {
            "dir-copy-path" => self.copy_status_text(path.to_string()),
            "dir-copy-name" => self.copy_status_text(folder_name(path)),
            // "Reveal in Finder": Explorer opens the folder; nothing runs.
            "dir-reveal" => match crate::profile::local_path(path).filter(|p| p.is_dir()) {
                Some(dir) => {
                    if let Err(e) = std::process::Command::new("explorer.exe").arg(&dir).spawn() {
                        self.banners.set("action", format!("Could not show {path}: {e}"));
                    }
                }
                None => self.banners.set("action", format!("{path} is not a folder on this computer.")),
            },
            // The default profile, started in the path. A tab that must open as a window
            // ("Prevent Opening in a Tab") opens it now, so the folder goes with it.
            "dir-new-window" | "dir-new-tab" => {
                self.start_dir_override = Some(crate::arrangement::Folder::Forced(path.to_string()));
                if what == "dir-new-window" || self.new_tab_opens_window(None) {
                    self.new_window(event_loop, None);
                } else {
                    self.new_tab(None);
                }
                self.start_dir_override = None;
                self.redraw();
            }
            _ => return false,
        }
        true
    }

    /// Puts text on the clipboard for a status bar item.
    pub(super) fn copy_status_text(&mut self, text: String) {
        if let Some(cb) = self.clipboard.as_mut() {
            if let Err(err) = cb.set_text(text) {
                self.banners.set("clipboard", format!("Could not copy: {err}"));
            }
        }
    }

    /// The Clock's cadence (`CLK`, 1 s): while a bar on screen shows a Clock, every window
    /// redraws on each whole second. When to come back.
    pub(super) fn tick_status_bars(&mut self) -> Option<Instant> {
        let now = Instant::now();
        if let Some(due) = self.status_clock_due {
            if now < due {
                return Some(due);
            }
        }
        let was_due = self.status_clock_due.take().is_some();
        if !self.any_clock_shown() {
            return None;
        }
        if was_due {
            self.redraw();
            self.redraw_all();
        }
        let next = next_whole_second(now);
        self.status_clock_due = Some(next);
        Some(next)
    }
}

/// Opening a menu above a point: while a `Lift` lives, the first menu window this thread
/// creates (class `#32768`) is subclassed, and each move it gets puts its bottom on the
/// point (never above the top of the monitor's work area). Submenus open where Windows
/// puts them.
mod above {
    use std::cell::Cell;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetClassNameW, GetWindowRect, SetWindowsHookExW, UnhookWindowsHookEx, HCBT_CREATEWND, HHOOK, SWP_NOMOVE, SWP_NOSIZE, WH_CBT, WINDOWPOS, WM_NCDESTROY,
        WM_WINDOWPOSCHANGING,
    };

    const SUBCLASS_ID: usize = 0x7462_6D75;

    thread_local! {
        /// The screen y the menu's bottom goes to, while a `Lift` lives.
        static BOTTOM: Cell<Option<i32>> = const { Cell::new(None) };
        /// The menu window being lifted, once created.
        static MENU: Cell<Option<isize>> = const { Cell::new(None) };
    }

    pub(super) struct Lift {
        hook: HHOOK,
    }

    impl Lift {
        /// `x`, `y`: the point in `hwnd`'s client pixels.
        pub(super) fn new(hwnd: isize, x: f32, y: f32) -> Lift {
            let mut pt = POINT { x: x as i32, y: y as i32 };
            unsafe { ClientToScreen(hwnd as HWND, &mut pt) };
            BOTTOM.with(|b| b.set(Some(pt.y)));
            MENU.with(|m| m.set(None));
            let hook = unsafe { SetWindowsHookExW(WH_CBT, Some(cbt), std::ptr::null_mut(), GetCurrentThreadId()) };
            Lift { hook }
        }
    }

    impl Drop for Lift {
        fn drop(&mut self) {
            if !self.hook.is_null() {
                unsafe { UnhookWindowsHookEx(self.hook) };
            }
            BOTTOM.with(|b| b.set(None));
            MENU.with(|m| m.set(None));
        }
    }

    unsafe extern "system" fn cbt(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HCBT_CREATEWND as i32 && BOTTOM.with(Cell::get).is_some() && MENU.with(Cell::get).is_none() {
            let hwnd = wparam as HWND;
            let mut name = [0u16; 16];
            let n = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
            if name[..n.max(0) as usize] == *"#32768".encode_utf16().collect::<Vec<u16>>() {
                MENU.with(|m| m.set(Some(hwnd as isize)));
                unsafe { SetWindowSubclass(hwnd, Some(place), SUBCLASS_ID, 0) };
            }
        }
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
    }

    unsafe extern "system" fn place(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        if msg == WM_WINDOWPOSCHANGING {
            let wp = unsafe { &mut *(lparam as *mut WINDOWPOS) };
            if let Some(bottom) = BOTTOM.with(Cell::get).filter(|_| wp.flags & SWP_NOMOVE == 0) {
                let cy = if wp.flags & SWP_NOSIZE == 0 {
                    wp.cy
                } else {
                    let mut r: RECT = unsafe { std::mem::zeroed() };
                    unsafe { GetWindowRect(hwnd, &mut r) };
                    r.bottom - r.top
                };
                let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
                info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                let top = if unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) } != 0 { info.rcWork.top } else { i32::MIN };
                wp.y = (bottom - cy).max(top);
            }
        } else if msg == WM_NCDESTROY {
            unsafe { RemoveWindowSubclass(hwnd, Some(place), SUBCLASS_ID) };
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `JOB:109-149` on the chain the job watcher really produces (`proctree`): the job's
    /// own name, its parents, then the shell; the shell alone at the prompt; nothing before
    /// the first snapshot; U+2026 once the shell is gone or a name is empty (`JOB:92-99`).
    #[test]
    fn job_chain_follows_iterm2() {
        use crate::proctree::{self, Proc};
        let p = |pid: u32, ppid: u32, name: &str, created: u64| Proc { pid, ppid, name: name.into(), created, gui: false, cmdline: None };
        let chain_of = |procs: &[Proc]| {
            let job = proctree::display_job(procs, 10);
            job_chain(job.as_ref().map(|j| j.name.as_str()), &proctree::ancestors(procs, 10), proctree::root_name(procs, 10).as_deref())
        };
        let vim_in_cmd = vec![p(10, 1, "pwsh.exe", 100), p(11, 10, "cmd.exe", 110), p(12, 11, "Vim.exe", 120)];
        assert_eq!(chain_of(&vim_in_cmd), vec!["Vim", "cmd", "pwsh"]);
        assert_eq!(sbt::job_variants(&chain_of(&vim_in_cmd[..2])), vec!["cmd", "cmd \u{25C2} pwsh"]);
        assert_eq!(chain_of(&vim_in_cmd[..1]), vec!["pwsh"], "at the prompt the shell is the job");
        assert_eq!(sbt::job_variants(&job_chain(None, &[], None)), vec![String::new()], "before the first value");
        assert_eq!(sbt::job_variants(&job_chain(None, &[], Some("pwsh"))), vec!["\u{2026}"], "the shell exited");
        assert_eq!(sbt::job_variants(&job_chain(Some(""), &[], Some("pwsh"))), vec!["\u{2026}"]);
        let login = vec!["vim".to_string()];
        assert_eq!(job_chain(Some("vim"), &login, Some("login")), vec!["vim"], "login is left out");
    }

    /// `PS:23172-23187`: a known trigger's item is its action title less U+2026, U+2014,
    /// its regex, checked when on, by its place in the list; an unknown action is left out.
    /// `ACT:75-80`: a Custom Action whose title fails shows the ladybug and the error.
    #[test]
    fn trigger_menu_items_follow_iterm2() {
        use crate::triggers::Trigger;
        let t = |regex: &str, action: &str, enabled: bool| Trigger { regex: regex.into(), action: action.into(), parameter: String::new(), instant: false, enabled, name: String::new(), job: String::new(), match_type: 0, extra: toml::Table::new() };
        let list = vec![t("err", "HighlightTrigger", true), t("x", "NoSuchTrigger", true), t("y", "BellTrigger", false)];
        assert_eq!(trigger_items(&list), vec![(0, "Highlight Text \u{2014} err".to_string(), true), (2, "Ring Bell \u{2014} y".to_string(), false)]);
        assert!(trigger_items(&[]).is_empty());
        assert_eq!(custom_action_error("bad"), "\u{1F41E} bad");
        assert_eq!(SCRIPT_REFUSAL, "Script functions need the Python API, which arrives in milestone 11.");
    }

    #[test]
    fn directory_menu_text_follows_iterm2() {
        assert_eq!(dir_title(r"C:\src", true), "\u{2605} C:\\src");
        assert_eq!(dir_title(r"C:\src", false), r"C:\src");
        assert_eq!(folder_name(r"C:\Users\rob\src\"), "src");
        assert_eq!(folder_name("/home/rob/tandem"), "tandem");
        assert_eq!(folder_name(r"C:\"), "C:");
        assert_eq!(shown_path("/C:/Users/rob"), r"C:\Users\rob");
        assert_eq!(shown_path("/home/rob"), "/home/rob");
        assert_eq!(evaluation_error("\\(x", "bad"), "Error while evaluating \u{201C}\\(x\u{201D}:\nbad");
    }

    /// A field's parts (`iTermMiniSearchFieldViewController.xib`): the magnifier at the
    /// left, the chevrons and the close glyph at the right, the results label before the
    /// chevrons, the text between; a click lands on one of them.
    #[test]
    fn field_parts_follow_iterm2() {
        let search = field_parts(200.0, true, true, true, 30.0);
        assert_eq!(search.mode, Some((0.0, 17.0)));
        assert_eq!(search.close, Some((183.0, 200.0)));
        assert_eq!(search.next, Some((166.0, 183.0)));
        assert_eq!(search.prev, Some((149.0, 166.0)));
        assert_eq!(search.label, (115.0, 145.0));
        assert_eq!(search.text, (17.0, 111.0));
        assert_eq!(part_at(&search, 5.0), FieldPart::Mode);
        assert_eq!(part_at(&search, 150.0), FieldPart::Prev);
        assert_eq!(part_at(&search, 170.0), FieldPart::Next);
        assert_eq!(part_at(&search, 199.0), FieldPart::Close);
        assert_eq!(part_at(&search, 60.0), FieldPart::Text);
        assert_eq!(part_at(&search, 120.0), FieldPart::Text, "the label focuses the field");

        // A layout's own Search Tool: no close glyph; the chevrons move right.
        let permanent = field_parts(200.0, true, true, false, 0.0);
        assert_eq!((permanent.close, permanent.next), (None, Some((183.0, 200.0))));
        assert_eq!(permanent.text, (17.0, 162.0));

        // The Filter Tool: a close glyph, no chevrons, no label.
        let filter = field_parts(125.0, true, false, true, 0.0);
        assert_eq!((filter.prev, filter.next, filter.close), (None, None, Some((108.0, 125.0))));
        assert_eq!(filter.text, (17.0, 104.0));

        // The Composer's inert field: text only.
        let composer = field_parts(200.0, false, false, false, 0.0);
        assert_eq!((composer.mode, composer.text), (None, (4.0, 196.0)));
        assert_eq!(part_at(&composer, 2.0), FieldPart::Text);

        // Too narrow: the text's room never goes negative.
        let tiny = field_parts(40.0, true, true, true, 30.0);
        assert!(tiny.text.1 >= tiny.text.0);

        assert_eq!(COMPOSER_REFUSAL, "The Composer arrives with its own milestone.");
    }

    #[test]
    fn text_content_is_empty_by_its_widest_variant() {
        assert!(Content::text(Vec::new()).empty);
        assert_eq!(Content::text(Vec::new()).variants, vec![String::new()]);
        assert!(!Content::text(vec![String::new(), "a".into()]).empty);
    }

    /// ICU answers for the default template (Windows 10 1903 and later), and the Clock
    /// renders it; an unlocalized format is used as given.
    #[cfg(windows)]
    #[test]
    fn clock_pattern_comes_from_icu() {
        let pattern = best_pattern(DEFAULT_CLOCK_FORMAT);
        assert!(pattern.contains('M') && pattern.contains("mm"), "{pattern}");
        assert!(clock_text(DEFAULT_CLOCK_FORMAT, true).chars().any(|c| c.is_ascii_digit()));
        assert_eq!(clock_text("'x'", false), "x");
        let knobs: toml::Table = [(FORMAT_KEY.to_string(), toml::Value::String("'x'".into())), (LOCALIZE_KEY.to_string(), toml::Value::Boolean(false))].into_iter().collect();
        assert_eq!(clock_for_knobs(&knobs), "x", "the setup sample follows the knobs");
    }

    #[test]
    fn clock_wakes_on_whole_seconds() {
        let now = Instant::now();
        let next = next_whole_second(now);
        assert!(next > now && next <= now + std::time::Duration::from_millis(1000));
    }
}
