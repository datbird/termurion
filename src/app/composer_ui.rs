//! The minimal composer in the pane (milestone 9l Task 6): the dropdown drawn natively over
//! its pane (`scene::composer`) with its completion popup (`scene::composer_popup`), its keys
//! (a `TakenKeys` pairing right after the toolbelt's, so nothing it takes reaches the session),
//! its Win chords, sending through `user_write`, the command queue, the syntax highlighting
//! (rate-limited, checks on a worker thread) and the suggestions (history on this thread, the
//! file and PATH listing on the worker), the Help popover and the explainshell question.
//!
//! iTerm2 sources (`~/gitrepos/iterm2-ref/sources`): `MCVC`
//! `Composer/iTermMinimalComposerViewController.m`, `CM` `Composer/iTermComposerManager.m`,
//! `CTV` `Composer/ComposerTextView.swift`, `LCVC`
//! `StatusBar/Components/iTermStatusBarLargeComposerViewController.m`, `PS`
//! `PTYSession/PTYSession.m`, `CE` `Composer/CommandExplainer.swift`.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use unicode_width::UnicodeWidthChar;
use winit::keyboard::{Key as WKey, NamedKey};

use super::*;
use super::toolbelt_tools_ui::{is_modifier, now_ms};
use crate::broadcast::Write;
use crate::command_syntax::{self, Check, Checker};
use crate::composer_worker::{Job, Reply};
use crate::composer::{self, Act, Key as CKey, Mods as CMods, Shown};
use crate::composer_suggest::{self as suggest, Item, ItemKind};
use crate::palette::Rgb;
use crate::prompt_state::{self, ClearProbe, Event};
use crate::text_edit::{self, EditKey, EditOut, VisualLine};

/// A fade's frame interval.
const FRAME: Duration = Duration::from_millis(16);
/// The info bar the AI button and Win+Y show.
/// The profile key of the dropdown's distance from the pane's top (`ITAddressBookMgr.h:384`),
/// undeclared, read with its default 0.
const TOP_OFFSET: &str = "Composer Top Offset";
/// The global dropdown height (`MCVC:141-142`), drag-only.
const HEIGHT_KEY: &str = "ComposerHeight";
/// The accessory row's text against the pane's font.
const SMALL: f32 = 0.8;
/// View > Auto Composer's global (`kPreferenceAutoComposer`), menu-only.
const AUTO_KEY: &str = "AutoComposer";
/// Advanced > Session: Show Composer always uses the status bar when one shows (`ADV:676`).
const ALWAYS_BAR_KEY: &str = "alwaysUseStatusBarComposer";
/// Shell integration fed the history once (`kCommandHistoryHasEverBeenUsed`).
const EVER_USED_KEY: &str = "NoSyncCommandHistoryHasEverBeenUsed";
/// Popup row glyphs (Segoe Fluent Icons: History, Page, Folder, CommandPrompt).
const HISTORY_GLYPH: char = '\u{E81C}';
const FILE_GLYPH: char = '\u{E8A5}';
const FOLDER_GLYPH: char = '\u{E8B7}';
const COMMAND_GLYPH: char = '\u{E756}';

/// Whether a focused composer acts on this key itself (Review Focus 5): whatever
/// `composer::action` maps (Esc, the send and enqueue Enters, Tab, the Auto Composer's Ctrl
/// chords, adding carets), and of the plain edit keys what `text_edit::takes` allows (every
/// plain key, the Windows Ctrl edit chords and moves, AltGr typing; never a Win or Alt chord).
/// A key it does not take may still run a profile mapping (`composer_key`), else it is dropped.
pub(super) fn composer_takes(m: CMods, key: &CKey, auto: bool) -> bool {
    match composer::action(key, m, auto, false) {
        None => false,
        Some(composer::Action::Edit(k)) => {
            let altgr = m.ctrl && m.alt && matches!(k, EditKey::Text(_));
            text_edit::takes(m.ctrl && !altgr, m.alt && !altgr, m.win, &k)
        }
        Some(_) => true,
    }
}

/// A winit key as the composer reads it, and the modifiers `composer::action` should see:
/// Esc; the Windows clipboard keys (Shift+Insert, Ctrl+Insert, Shift+Delete) as plain Paste,
/// Copy and Cut; named keys as their raw edit key (the action applies Ctrl's meaning); a
/// Ctrl+letter as `Key::Ctrl`; else the typed text.
fn decode(key: &WKey, text: Option<&str>, m: CMods) -> Option<(CKey, CMods)> {
    let only = |ctrl: bool, shift: bool| m.ctrl == ctrl && m.shift == shift && !m.alt && !m.win;
    let plain = CMods::default();
    match key {
        WKey::Named(NamedKey::Escape) => return Some((CKey::Esc, m)),
        WKey::Named(NamedKey::Insert) if only(false, true) => return Some((CKey::Edit(EditKey::Paste(String::new())), plain)),
        WKey::Named(NamedKey::Insert) if only(true, false) => return Some((CKey::Edit(EditKey::Copy), plain)),
        WKey::Named(NamedKey::Delete) if only(false, true) => return Some((CKey::Edit(EditKey::Cut), plain)),
        _ => {}
    }
    let named = match key {
        WKey::Named(NamedKey::Enter) => Some(EditKey::Enter),
        WKey::Named(NamedKey::Tab) => Some(EditKey::Tab),
        WKey::Named(NamedKey::Backspace) => Some(EditKey::Backspace),
        WKey::Named(NamedKey::Delete) => Some(EditKey::Delete),
        WKey::Named(NamedKey::ArrowLeft) => Some(EditKey::Left),
        WKey::Named(NamedKey::ArrowRight) => Some(EditKey::Right),
        WKey::Named(NamedKey::ArrowUp) => Some(EditKey::Up),
        WKey::Named(NamedKey::ArrowDown) => Some(EditKey::Down),
        WKey::Named(NamedKey::Home) => Some(EditKey::Home),
        WKey::Named(NamedKey::End) => Some(EditKey::End),
        WKey::Named(NamedKey::PageUp) => Some(EditKey::PageUp),
        WKey::Named(NamedKey::PageDown) => Some(EditKey::PageDown),
        _ => None,
    };
    if let Some(k) = named {
        return Some((CKey::Edit(k), m));
    }
    if let WKey::Character(c) = key {
        if m.ctrl && !m.alt {
            return c.chars().next().map(|c| (CKey::Ctrl(c), m));
        }
    }
    let printable = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control))?;
    Some((CKey::Edit(EditKey::Text(printable.to_string())), m))
}

/// `keystroke.isNavigation` (`iTermKeystroke.m:439-453`): the arrows, Page Up and Down, Home
/// and End.
fn is_navigation(key: &WKey) -> bool {
    matches!(
        key,
        WKey::Named(NamedKey::ArrowUp | NamedKey::ArrowDown | NamedKey::ArrowLeft | NamedKey::ArrowRight | NamedKey::PageUp | NamedKey::PageDown | NamedKey::Home | NamedKey::End)
    )
}

/// A caret move (Shift extends it through `TextEdit::key` directly).
fn is_move(k: &EditKey) -> bool {
    matches!(
        k,
        EditKey::Left
            | EditKey::Right
            | EditKey::Up
            | EditKey::Down
            | EditKey::Home
            | EditKey::End
            | EditKey::PageUp
            | EditKey::PageDown
            | EditKey::WordLeft
            | EditKey::WordRight
            | EditKey::TextStart
            | EditKey::TextEnd
    )
}

/// Text width in the pane's font: its cells (a tab is 4) at `cw` each.
fn text_width(s: &str, cw: f32) -> f32 {
    s.chars().map(|c| if c == '\t' { 4 } else { c.width().unwrap_or(0) }).sum::<usize>() as f32 * cw
}

/// The composer's one wrap (the Task 1 ruling): every draw and hit test goes through it, so a
/// prompt prefix can never put drawing and hit testing out of step. `text` is the editor's
/// text (with the ghost inserted when drawing). The prefix sits in the same storage
/// (`CTV:475-505`), so it wraps with the text: the first line's indent and `prefix_breaks`
/// come from `wrap_params` (none for the dropdown; Task 7's drawn prompt sets them).
fn composer_wrap(c: &composer::Composer, text: &str, max_w: f32, cw: f32) -> Vec<VisualLine> {
    let (indent, breaks) = wrap_params(c);
    text_edit::wrap_indented(text, max_w, indent, breaks, &|s: &str| text_width(s, cw))
}

/// The first line's indent and whether the prefix ends in a break opportunity, for
/// `composer_wrap`: a prefix kept in the storage needs neither.
fn wrap_params(c: &composer::Composer) -> (f32, bool) {
    let _ = c;
    (0.0, false)
}

/// Which pane a press on a composer focuses (a left press on another pane's composer focuses
/// that pane, as a left press on its terminal does), or None.
fn press_focus(left: bool, pane: PaneId, focused: Option<PaneId>) -> Option<PaneId> {
    (left && focused != Some(pane)).then_some(pane)
}

/// The composers of one window's `panes` (each with whether it holds the keyboard) that let it
/// go when `focused` is that window's focused pane: every holder but the focused pane's.
fn composers_to_release(panes: &[(PaneId, bool)], focused: Option<PaneId>) -> Vec<PaneId> {
    panes.iter().filter(|(id, holds)| *holds && Some(*id) != focused).map(|(id, _)| *id).collect()
}

/// `textViewOrComposerIsFirstResponder` (`PS:21021-21026`, `CM:97`) for the focused pane of an
/// active window: its dropdown has the keyboard (`dropdown`), or nothing else does
/// (`elsewhere`: a status bar field, a toolbelt tool, find typing, Copy Mode, the completion
/// list, the shortcut labels or the Clippings panel), so the terminal has it.
fn terminal_or_dropdown_has_keys(dropdown: bool, elsewhere: bool) -> bool {
    dropdown || !elsewhere
}

/// `notifyActive` (`PS:13557-13564`): the focused pane's terminal just got the keyboard back
/// (`regained`: a press on it, the pane becoming the focused one, or the key consumer that had
/// it letting go), and a shown Auto Composer there (`have_auto`, not `hidden` by Copy Mode)
/// takes it, unless it has it already or something else still does (`elsewhere`, as
/// `terminal_or_dropdown_has_keys`; Copy Mode is one, `makeComposerFirstResponderIfAllowed`).
fn auto_takes_keys_back(regained: bool, have_auto: bool, hidden: bool, dropdown: bool, elsewhere: bool) -> bool {
    regained && have_auto && !hidden && !dropdown && !elsewhere
}

/// A status bar field keeps the keyboard only while its pane's bar shows (`bar_shown`) and
/// places the field (`placed`); else its edit ends (the review ruling: no key edits a field
/// nobody sees).
pub(super) fn field_keeps_keyboard(bar_shown: bool, placed: bool) -> bool {
    bar_shown && placed
}

/// A line as drawn: tabs as 4 spaces (as `text_width` measures them).
fn shown(s: &str) -> String {
    s.replace('\t', "    ")
}


fn glyph(kind: ItemKind) -> char {
    match kind {
        ItemKind::History => HISTORY_GLYPH,
        ItemKind::File => FILE_GLYPH,
        ItemKind::Folder => FOLDER_GLYPH,
        ItemKind::Command | ItemKind::AiSuggestion | ItemKind::AiReplacement => COMMAND_GLYPH,
    }
}

/// Byte ranges of `text` for UTF-16 unit ranges (the highlighter's units).
fn utf16_to_bytes(text: &str, spans: &[command_syntax::Span]) -> Vec<(Range<usize>, Rgb, bool)> {
    let mut at = Vec::with_capacity(text.len() + 1);
    for (i, c) in text.char_indices() {
        for _ in 0..c.len_utf16() {
            at.push(i);
        }
    }
    at.push(text.len());
    let byte = |u: usize| at.get(u).copied().unwrap_or(text.len());
    spans.iter().map(|s| (byte(s.range.start)..byte(s.range.end), s.color, s.bold)).filter(|(r, _, _)| r.start < r.end).collect()
}

/// The colored runs of `display[line]`: `segs` (sorted, non-overlapping byte ranges with a
/// color and bold), `default` elsewhere.
fn runs(display: &str, line: &VisualLine, segs: &[(Range<usize>, Rgb, bool)], default: Rgb) -> Vec<(String, Rgb, bool)> {
    let mut out = Vec::new();
    let mut at = line.start;
    for (r, color, bold) in segs {
        if r.end <= at || r.start >= line.end {
            continue;
        }
        let start = r.start.max(at);
        if start > at {
            out.push((shown(&display[at..start]), default, false));
        }
        let end = r.end.min(line.end);
        out.push((shown(&display[start..end]), *color, *bold));
        at = end;
    }
    if at < line.end {
        out.push((shown(&display[at..line.end]), default, false));
    }
    out
}

/// Highlight spans kept only where they still fit the text (the text changed since they were
/// computed; they are redone within 0.05 s).
fn valid_spans(text: &str, spans: &[(Range<usize>, Rgb, bool)]) -> Vec<(Range<usize>, Rgb, bool)> {
    spans.iter().filter(|(r, _, _)| r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end)).cloned().collect()
}

/// The Auto Composer's prompt prefix as colored byte ranges of the editor's text: (range,
/// foreground, bold, background), from the captured rows (`PS:8037-8058`); nothing when the
/// text does not start with it.
fn prefix_runs(c: &composer::Composer) -> Vec<(Range<usize>, Rgb, bool, Rgb)> {
    let Some(p) = &c.prefix else { return Vec::new() };
    if !c.edit.text.starts_with(&p.text.join("\n")) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut at = 0;
    for (i, line) in p.text.iter().enumerate() {
        let end = at + line.len();
        let mut x = at;
        for run in p.rows.get(i).map_or(&[][..], |r| &r.runs[..]) {
            let stop = (x + run.text.len()).min(end);
            if stop > x && c.edit.text.is_char_boundary(stop) {
                out.push((x..stop, run.fg, run.bold, run.bg));
            }
            x = stop;
        }
        at = end + 1;
    }
    out
}

/// Drops what the pane drew into `overlay` from index `from` on (link dashes, annotation and
/// hover underlines) and into `underlines` from `underline_from` on (SGR underlines and
/// strikethrough, link dashes) inside the rows `cover` spans: the rows under an Auto Composer
/// are skipped. A quad reaching past them (the dimming over the whole pane) stays.
pub(super) fn skip_covered(overlay: &mut Vec<scene::Quad>, from: usize, underlines: &mut Vec<scene::Quad>, underline_from: usize, strikes: &mut Vec<scene::Quad>, strike_from: usize, cover: Rect) {
    for (quads, from) in [(overlay, from), (underlines, underline_from), (strikes, strike_from)] {
        let tail = quads.split_off(from.min(quads.len()));
        quads.extend(tail.into_iter().filter(|q| {
            let r = q.rect;
            !(r.y >= cover.y - 0.5 && r.y + r.h <= cover.y + cover.h + 0.5)
        }));
    }
}

/// A completion popup: its items, the typed line its rows show in bold, and the selected row.
struct Popup {
    items: Vec<Item>,
    typed: String,
    selected: usize,
}

/// A suggestion fetch waiting on the worker's file listing.
pub(super) struct Fetch {
    generation: u64,
    explicit: bool,
    line: String,
    name: String,
    history: Vec<Item>,
    /// What the AI request knows (`SR:49-60`) and the previous commands it is given.
    inputs: crate::ai_complete::Inputs,
    ai_history: Vec<String>,
    /// The early item pinned first while the model answers (`LC:715-739`).
    first: Option<Item>,
    /// The listing's file items (quoted), the unquoted values the expansion uses, and the
    /// prefix the parser compares with; set when the request is sent.
    files: Vec<Item>,
    expand: Vec<Item>,
    ai_prefix: String,
}

/// Whether a model's answer is for the fetch still waiting (a newer fetch or a text change
/// makes it stale).
fn ai_reply_current(waiting_generation: Option<u64>, reply_generation: u64) -> bool {
    waiting_generation == Some(reply_generation)
}

/// Per-pane composer view state (drawing, highlighting, suggestions); the text itself is the
/// Pane's `composer`.
#[derive(Default)]
pub(super) struct View {
    opened: Option<Instant>,
    closed: Option<Instant>,
    scroll: f32,
    spans: Vec<(Range<usize>, Rgb, bool)>,
    highlight_due: Option<Instant>,
    highlighted: Option<Instant>,
    checker: Checker,
    ghost: Option<String>,
    popup: Option<Popup>,
    generation: u64,
    waiting: Option<Fetch>,
}

/// A drag that began on the composer: a text selection (Alt: a box of carets from where it
/// began, as a visual line and x), the bottom drag handle, or the border moving the panel.
#[derive(Clone, Copy, Debug)]
pub(super) enum ComposerDrag {
    Select { from: Option<(usize, f32)> },
    Resize { start_y: f32, start_h: f32 },
    Move { start_y: f32, start_offset: f32 },
}

/// Windows the composer opens on the next turn of the event loop (they need it), and the
/// ones it has open.
#[derive(Default)]
pub(super) struct Windows {
    help_at: Option<(f64, f64)>,
    help: Option<crate::status_popover_ui::InfoPopover>,
    explain_ask: Option<(PaneId, String, String)>,
    explain: Option<(crate::prompt_ui::ConfirmWindow, PaneId, String)>,
    history: Option<(PaneId, String, bool)>,
    advanced_paste: Option<PaneId>,
}

fn env_list(name: &str) -> Vec<String> {
    std::env::var(name).unwrap_or_default().split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

fn home() -> String {
    std::env::var("USERPROFILE").unwrap_or_default()
}

impl App {
    /// The pane whose shown dropdown has the keyboard: the focused pane's, while no toolbelt
    /// tool has it.
    fn focused_composer(&self) -> Option<PaneId> {
        let id = self.focused_id()?;
        let c = &self.panes.get(&id)?.composer;
        (c.shown == Shown::Dropdown && c.focused && !c.hidden && self.belt.focus.is_none()).then_some(id)
    }

    /// Which composer part has the keyboard: (a dropdown, a status bar field). The IME's
    /// commit goes where a typed key would.
    pub(super) fn composer_keys_owner(&self) -> (bool, bool) {
        (self.focused_composer().is_some(), self.focused_field().is_some())
    }

    /// Typed text from the IME into the composer dropdown or field that has the keyboard, as
    /// `composer_key_press` and `composer_field_press` insert a printable key. False when none has it.
    pub(super) fn composer_type_text(&mut self, text: &str) -> bool {
        if let Some(pane) = self.focused_composer() {
            self.composer_apply(pane, composer::Action::Edit(EditKey::Text(text.to_string())), false);
            return true;
        }
        if let Some((pane, _)) = self.live_field() {
            self.composer_field_edit(pane, EditKey::Text(text.to_string()), false);
            return true;
        }
        false
    }

    /// A composer has the keyboard (the keyboard hook takes its Win chords then).
    pub(super) fn composer_has_keys(&self) -> bool {
        self.focused_composer().is_some()
    }

    /// Another key consumer takes the keyboard (find typing, Copy Mode, the autocomplete list,
    /// the shortcut labels, a toolbelt tool): no composer keeps it; their popups close.
    pub(super) fn composer_yield_keys(&mut self) {
        self.composer_yield_keys_but(None);
    }

    /// `composer_yield_keys`, except `keep`'s composer (a press on its own status bar field).
    pub(super) fn composer_yield_keys_but(&mut self, keep: Option<PaneId>) {
        let ids: Vec<PaneId> = self.panes.iter().filter(|(id, p)| (p.composer.focused || p.composer.field_focused) && Some(**id) != keep).map(|(id, _)| *id).collect();
        let any = !ids.is_empty();
        for id in ids {
            self.composer_let_go(id);
        }
        if any {
            self.refresh_route();
            self.redraw();
        }
    }

    /// `pane`'s composer lets the keyboard go: its popup and ghost close, and a status bar
    /// field ends its edit (`controlTextDidEndEditing:`, `SBCVC`), so a temporary one leaves
    /// with its text kept (`CM:314-322`) and the paste Progress or Filter temporary comes
    /// back (`PS:24370-24373`). The caller refreshes the route.
    fn composer_let_go(&mut self, pane: PaneId) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        p.composer.focused = false;
        p.composer.field_focused = false;
        p.composer.field_pick = None;
        if p.composer.shown == Shown::Field {
            let text = p.composer.contents();
            p.composer.field_ended(&text);
        }
        if let Some(v) = self.composers.get_mut(&pane) {
            v.popup = None;
            v.ghost = None;
        }
    }

    /// `pane`'s composer takes the keyboard: every other key consumer lets it go, and the
    /// pane's find field stops typing.
    pub(super) fn composer_take_keys(&mut self, pane: PaneId) {
        self.belt.focus = None;
        self.composer_yield_keys_but(Some(pane));
        if let Some(p) = self.panes.get_mut(&pane) {
            p.composer.focused = true;
            p.composer.field_focused = false;
        }
        if let Some(f) = self.finds.get_mut(&pane) {
            f.typing = false;
        }
        self.refresh_route();
    }

    /// Engage AI's answer from the terminal or the status bar: `text` goes into the large
    /// Composer, which opens if it was hidden and takes the keyboard (`PS:21385-21399`). The
    /// person still has to send it.
    pub(super) fn composer_ai_place(&mut self, pane: PaneId, text: &str) {
        let before = self.panes.get(&pane).map(|p| p.composer.shown);
        self.set_composer_string(pane, text, true);
        let Some(p) = self.panes.get(&pane) else { return };
        let (shown, hidden) = (p.composer.shown, p.composer.hidden);
        if shown != Shown::Dropdown {
            return;
        }
        if before != Some(Shown::Dropdown) {
            self.composer_opened(pane);
        }
        if !hidden {
            self.composer_take_keys(pane);
        }
        self.redraw();
    }

    /// Engage AI's answer from the large Composer: it replaces the selection, else the whole
    /// text after the prompt, and the Composer has the keyboard again (`LC:314-343`).
    pub(super) fn composer_ai_replace(&mut self, pane: PaneId, text: &str) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        if p.composer.shown != Shown::Dropdown {
            return;
        }
        p.composer.replace_selection_or_all(text);
        self.composer_ai_cancel(pane);
        let v = self.composers.entry(pane).or_default();
        v.generation += 1;
        v.ghost = None;
        v.popup = None;
        v.waiting = None;
        v.highlight_due = Some(Instant::now());
        self.composer_take_keys(pane);
        self.redraw();
    }

    /// The global dropdown height (`ComposerHeight`, 135).
    fn composer_height(&self) -> f32 {
        self.globals.number(HEIGHT_KEY).map_or(composer::DEFAULT_HEIGHT, |f| f as f32)
    }

    /// The profile's `Composer Top Offset` (a session copy reads its saved profile, which is
    /// where a drag writes it).
    fn composer_top_offset(&self, pane: PaneId) -> f32 {
        let Some(p) = self.panes.get(&pane) else { return 0.0 };
        let guid = self.store.shared_guid(&p.profile);
        self.store.get(&guid).or_else(|| self.store.get(&p.profile)).map_or(0.0, |pr| pr.float(TOP_OFFSET) as f32)
    }

    /// The dropdown's parts over pane rectangle `rect` (pixels): `minimal_rect` in points, or
    /// the Auto Composer's frame.
    pub(super) fn composer_layout(&self, pane: PaneId, rect: Rect) -> composer::Layout {
        if let Some(l) = self.auto_layout(pane, rect) {
            return l;
        }
        let s = self.scale();
        let pts = Rect { x: rect.x / s, y: rect.y / s, w: rect.w / s, h: rect.h / s };
        let h = composer::clamp_height(self.composer_height(), pts.h);
        let r = composer::minimal_rect(pts, h, self.composer_top_offset(pane));
        composer::layout(Rect { x: r.x * s, y: r.y * s, w: r.w * s, h: r.h * s }, s)
    }

    /// The Auto Composer's parts (a composer with `auto` set): `auto_rect` over the pane's grid
    /// for its laid-out rows, the half row it may not have inside the grid taken from the
    /// pane's bottom margin; the text starts under the separator (`MCVC:249-270`: 2 points, or
    /// half a line less 6 when that is more). It has no close, Help, AI or drag handle.
    fn auto_layout(&self, pane: PaneId, rect: Rect) -> Option<composer::Layout> {
        let p = self.panes.get(&pane)?;
        let c = &p.composer;
        if !c.auto {
            return None;
        }
        let grid = self.text_rect(pane, rect);
        let (cw, line_h) = self.cell();
        let s = self.scale();
        let lines = composer_wrap(c, &c.edit.text, grid.w, cw);
        let ends = c.contents().ends_with('\n');
        let rows = composer::auto_rows(lines.len().saturating_sub(usize::from(ends)), ends);
        let screen = p.own();
        let mut panel = composer::auto_rect(grid, line_h, rows, screen.prompt_start_row(), screen.primary_cursor_row());
        panel.h = panel.h.max(((rows as f32 + 0.5) * line_h).min(rect.y + rect.h - panel.y));
        let offset = (2.0 * s).max(line_h / 2.0 - 6.0 * s);
        let text = Rect { x: grid.x, y: panel.y + offset, w: grid.w, h: (panel.h - offset).max(0.0) };
        let none = Rect { x: panel.x, y: panel.y, w: 0.0, h: 0.0 };
        Some(composer::Layout { panel, close: none, text, accessory: none, help: none, ai: none, handle: none })
    }

    /// `haveAutoComposer` (`PS:24264-24266`): the pane's dropdown shows as an Auto Composer.
    pub(super) fn have_auto_composer(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_some_and(|p| p.composer.auto && p.composer.shown == Shown::Dropdown)
    }

    /// The rows an Auto Composer covers (its frame), while it is drawn: the pane's terminal
    /// cursor is hidden and those rows skipped (`PTYTextView.m:2119-2123`).
    pub(super) fn auto_composer_cover(&self, pane: PaneId, rect: Rect) -> Option<Rect> {
        let hidden = self.panes.get(&pane).is_none_or(|p| p.composer.hidden);
        (self.have_auto_composer(pane) && !hidden).then(|| self.composer_layout(pane, rect).panel)
    }

    /// `shouldShowAutoComposer` (`PS:7981-7987`): Auto Composer is on and the session's
    /// prompt state allows it.
    pub(super) fn should_show_auto_composer(&self, pane: PaneId) -> bool {
        self.globals.bool(AUTO_KEY) && self.panes.get(&pane).is_some_and(|p| p.own().prompt_state.eligible)
    }

    /// `commandHistoryHasEverBeenUsed` (`iTermShellHistoryController.m:528-531`).
    /// Remembered once true, as the About Shell Integration check does, so the check mark can
    /// read it without loading the history.
    fn command_history_ever_used(&mut self) -> bool {
        let used = self.shell_history_pub().ever_used() || self.globals.bool(EVER_USED_KEY);
        if used && !self.globals.bool(EVER_USED_KEY) {
            self.globals.set(EVER_USED_KEY, toml::Value::Boolean(true));
        }
        used
    }

    /// View > Auto Composer's check mark (`PT:13982-13993`): off until shell history was used.
    pub(super) fn auto_composer_checked(&self) -> bool {
        self.globals.bool(AUTO_KEY) && (self.globals.bool(EVER_USED_KEY) || self.shell_history.as_ref().is_some_and(|h| h.ever_used()))
    }

    /// `textViewOrComposerIsFirstResponder` (`PS:21021-21026`): the pane is the focused one of
    /// an active window and its terminal or its dropdown has the keyboard
    /// (`terminal_or_dropdown_has_keys`; a status bar field that has it does not count).
    fn terminal_has_keys(&self, pane: PaneId) -> bool {
        if !self.focused || self.focused_id() != Some(pane) {
            return false;
        }
        let dropdown = self.panes.get(&pane).is_some_and(|p| p.composer.focused && p.composer.shown == Shown::Dropdown);
        terminal_or_dropdown_has_keys(dropdown, self.keys_elsewhere(pane))
    }

    /// Something other than `pane`'s terminal and dropdown has the keyboard: one of its status
    /// bar fields, a toolbelt tool, find typing, Copy Mode, the completion list, the shortcut
    /// labels or the Clippings panel.
    fn keys_elsewhere(&self, pane: PaneId) -> bool {
        let c = self.panes.get(&pane).map(|p| &p.composer);
        let field = c.is_some_and(|c| c.field_focused || (c.shown == Shown::Field && c.focused));
        field
            || self.belt.focus.is_some()
            || self.typing_in_find().is_some()
            || self.copy_modes.contains_key(&pane)
            || self.completion.is_some()
            || self.shortcuts.is_some()
            || self.clips.get(&pane).is_some_and(|c| c.keys && c.model.visible)
    }

    /// Whether something other than the focused pane's terminal and dropdown has the keyboard
    /// now (read before an input event, for `composer_take_back`).
    pub(super) fn focused_keys_elsewhere(&self) -> bool {
        self.focused_id().is_some_and(|id| self.keys_elsewhere(id))
    }

    /// `notifyActive` (`PS:13557-13564`, `auto_takes_keys_back`): the focused pane's terminal
    /// got the keyboard back (`regained`), so its Auto Composer takes it.
    pub(super) fn composer_take_back(&mut self, regained: bool) {
        let Some(id) = self.focused_id() else { return };
        let Some(c) = self.panes.get(&id).map(|p| &p.composer) else { return };
        let (hidden, dropdown) = (c.hidden, c.focused);
        if auto_takes_keys_back(regained, self.have_auto_composer(id), hidden, dropdown, self.keys_elsewhere(id)) {
            self.composer_take_keys(id);
            self.redraw();
        }
    }

    /// The prompt state machine's events for `pane` (`MS:8409-8445` to the session,
    /// `PS:15647-15682`). Reveal: unless an Auto Composer already shows, the composer is reset,
    /// becomes the Auto Composer with the prompt as its prefix, and takes the keyboard only if
    /// the terminal or composer had it in an active window. Dismiss: the session is no longer
    /// eligible and whatever dropdown shows goes away at once. Append: text the shell echoed
    /// goes into a shown Auto Composer, which then clears the line with ^U before its send.
    pub(super) fn composer_events(&mut self, pane: PaneId, events: Vec<Event>) {
        for ev in events {
            match ev {
                Event::Reveal { rows, text } => {
                    if !self.globals.bool(AUTO_KEY) || self.have_auto_composer(pane) {
                        continue;
                    }
                    let had_keys = self.terminal_has_keys(pane);
                    let copy = self.copy_modes.contains_key(&pane);
                    let (status_bar, visible) = self.composer_routing(pane);
                    let Some(p) = self.panes.get_mut(&pane) else { continue };
                    match composer::reveal_at(status_bar, visible, p.composer.shown) {
                        composer::RevealAt::Nothing => continue,
                        composer::RevealAt::Field { permanent } => {
                            // `showComposerInStatusBar:` makes the field first responder;
                            // here it takes the keyboard when the terminal, dropdown or a
                            // field had it, as the dropdown does (Copy Mode keeps it).
                            let field_had = p.composer.field_focused || (p.composer.shown == Shown::Field && p.composer.focused);
                            p.composer.reveal_in_status_bar(permanent);
                            if (had_keys || field_had) && !copy {
                                self.composer_field_take(pane, permanent);
                            }
                            continue;
                        }
                        composer::RevealAt::Dropdown => {}
                    }
                    let (fg, bg) = (p.screen.palette.fg, p.screen.palette.bg);
                    let c = &mut p.composer;
                    // `reset` (`CM:559-565`), then `isAutoComposer`, the reveal and `setPrefix:`.
                    c.set_prefix(None);
                    c.set_string("", true);
                    c.saved = None;
                    c.auto = true;
                    c.shown = Shown::Dropdown;
                    c.hidden = copy;
                    c.set_prefix(Some(composer::auto_prefix(rows, text, fg, bg)));
                    c.shell_provided = false;
                    self.composer_opened(pane);
                    if had_keys && !copy {
                        self.composer_take_keys(pane);
                    }
                }
                Event::Dismiss => {
                    let Some(p) = self.panes.get_mut(&pane) else { continue };
                    p.own_mut().prompt_state.eligible = false;
                    if p.composer.dismiss() {
                        self.composer_gone(pane);
                    }
                }
                Event::Append(text) => {
                    if !self.have_auto_composer(pane) {
                        continue;
                    }
                    self.composer_insert(pane, &text);
                    if let Some(p) = self.panes.get_mut(&pane) {
                        p.composer.shell_provided = true;
                    }
                }
            }
        }
        self.redraw();
    }

    /// Every pane's prompt state events since the last read.
    pub(super) fn check_composer_events(&mut self) {
        let batches: Vec<(PaneId, Vec<Event>)> = self
            .panes
            .iter_mut()
            .filter_map(|(id, p)| {
                let events = p.own_mut().take_composer_events();
                (!events.is_empty()).then_some((*id, events))
            })
            .collect();
        for (id, events) in batches {
            self.composer_events(id, events);
        }
    }

    /// View > Auto Composer (`PT:14215-14219`): nothing until shell history was ever used
    /// (the item is disabled then); flips the global and every session's machine
    /// (`PS:18940-18943`, `MS:599-602`). Turning it off leaves a shown Auto Composer up
    /// (kept). Turning it on reveals or dismisses again at once in each session: one entering
    /// a command gets an Auto Composer, any other has its dropdown dismissed and stops being
    /// eligible (`PS:15663-15667`), so Show Composer works there again.
    pub(super) fn toggle_auto_composer(&mut self) {
        if !self.command_history_ever_used() {
            return;
        }
        let on = !self.globals.bool(AUTO_KEY);
        self.globals.set(AUTO_KEY, toml::Value::Boolean(on));
        self.save_globals();
        let mut batches = Vec::new();
        for (id, p) in self.panes.iter_mut() {
            let events = p.own_mut().prompt_state.set_enabled(on);
            if !events.is_empty() {
                batches.push((*id, events));
            }
        }
        for (id, events) in batches {
            self.composer_events(id, events);
        }
        self.menus_dirty = true;
        self.sync_checks();
        self.redraw();
    }

    /// `setComposerString:forceLarge:` (`PS:8690-8727`): the clear sequence
    /// (`composerClearSequence`, "0x15 0x0b") goes to the shell as sent text, a probe then
    /// waits 0.1 s for its control characters to echo back, and `s` is placed in the composer.
    pub(super) fn set_composer_string(&mut self, pane: PaneId, s: &str, force_large: bool) {
        if !self.composer_allowed(pane) {
            return;
        }
        let seq = self.globals.get(prompt_state::CLEAR_KEY).and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| prompt_state::DEFAULT_CLEAR.to_string());
        let bytes = crate::keyaction::hex_bytes(&seq);
        if !bytes.is_empty() {
            self.user_write(pane, &bytes, Write::SendText);
        }
        // Assigned only after the write, which would cancel it (`PS:8716-8720`).
        if let Some(p) = self.panes.get_mut(&pane) {
            p.composer.probe = ClearProbe::start(&bytes, Instant::now());
            p.composer.place(s, false, force_large);
        }
    }

    /// A composer can open on `pane`: not a tmux gateway (its pty carries tmux commands) nor
    /// an ended session (`PS:11996-12003`).
    fn composer_allowed(&self, pane: PaneId) -> bool {
        !self.is_tmux_gateway(pane) && self.panes.get(&pane).is_some_and(|p| p.exited.is_none() && !p.gateway)
    }

    /// Output arrived in `panes`: a clear probe still waiting writes the profile's Backspace
    /// once per control character of the sequence, without broadcasting (`PS:8729-8735`).
    pub(super) fn composer_output(&mut self, panes: &[PaneId]) {
        let now = Instant::now();
        for &pane in panes {
            let Some(p) = self.panes.get_mut(&pane) else { continue };
            let Some(probe) = p.composer.probe.as_mut() else { continue };
            let owed = probe.output(now);
            if probe.expired(now) {
                p.composer.probe = None;
            }
            let Some(n) = owed.filter(|n| *n > 0) else { continue };
            let bs = self.backspace_data(pane);
            if bs.is_empty() {
                continue;
            }
            for _ in 0..n {
                self.user_write(pane, &bs, Write::SendTextLocal);
            }
        }
    }

    /// `backspaceData` (`PS:12893-12922`): what the profile's Backspace mapping sends.
    pub(super) fn backspace_data(&self, pane: PaneId) -> Vec<u8> {
        // `iTermKeystroke.backspace`: kVK_Delete, no modifiers, 0x7f.
        let stroke = keystroke::Keystroke::new(0x7f, 0, Some(0x33), 0x7f);
        let map = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).and_then(|p| p.get("Keyboard Map")).and_then(|v| v.as_table().cloned());
        let agnostic = self.globals.bool("LanguageAgnosticKeyBindings");
        let binding = map.as_ref().and_then(|m| stroke.find(m, agnostic).and_then(|k| m.get(k))).and_then(crate::keyaction::Binding::from_value);
        prompt_state::backspace_bytes(binding.as_ref())
    }

    /// Copy Mode in `pane` hides its composer and takes its keyboard; leaving gives an Auto
    /// Composer the keyboard back (`PS:23381-23398`, `setTemporarilyHidden:`).
    pub(super) fn composer_follow_copy_mode(&mut self, pane: PaneId) {
        let on = self.copy_modes.contains_key(&pane);
        let Some(p) = self.panes.get_mut(&pane) else { return };
        if p.composer.hidden == on {
            return;
        }
        p.composer.hidden = on;
        if on {
            if p.composer.focused {
                self.composer_let_go(pane);
                self.refresh_route();
            }
        } else if self.have_auto_composer(pane) && self.focused_id() == Some(pane) {
            self.composer_take_keys(pane);
        }
        self.redraw();
    }

    /// The pane's dropdown went away at once (`dismissAnimated:NO`): no fade.
    fn composer_gone(&mut self, pane: PaneId) {
        self.composer_closed(pane);
        if let Some(v) = self.composers.get_mut(&pane) {
            v.opened = None;
            v.closed = None;
        }
    }

    /// The pane's dropdown layout and its text's visual lines (no ghost), when it shows.
    fn composer_lines(&self, pane: PaneId) -> Option<(composer::Layout, Vec<VisualLine>)> {
        let rect = self.pane_rect(pane)?;
        let l = self.composer_layout(pane, rect);
        let cw = self.cell().0;
        let c = &self.panes.get(&pane)?.composer;
        let lines = composer_wrap(c, &c.edit.text, l.text.w, cw);
        Some((l, lines))
    }

    /// View > Show Composer for `pane` (`PS:8655-8661`, `CM:146-158`): the composer becomes an
    /// Auto Composer when the session wants one; a command already typed at the prompt is
    /// cleared from the shell and carried into it (`set_composer_string`, which with a visible
    /// Composer component puts it in that field, `CM:38-50`); then it toggles. With a status
    /// bar that wants the composer (`using_status_bar`: the bar shows,
    /// `alwaysUseStatusBarComposer` or a visible Composer component) a shown dropdown saves
    /// and hides, else a status bar field takes the keyboard (`CM:192-215`): the visible
    /// component with its own text, or a temporary right one seeded from the saved text; a
    /// field that already has the keyboard opens the dropdown with its text (`SBCVC:78-84`).
    /// Otherwise the dropdown toggles, taking the keyboard when it opens.
    pub(super) fn compose(&mut self, pane: PaneId) {
        if !self.composer_allowed(pane) {
            return;
        }
        let auto = self.should_show_auto_composer(pane);
        let (status_bar, visible) = self.composer_routing(pane);
        let Some(p) = self.panes.get_mut(&pane) else { return };
        p.composer.auto = auto;
        let auto_shown = p.composer.auto && p.composer.shown == Shown::Dropdown;
        let current = composer::current_command(auto_shown, &p.composer.contents(), p.own().typed_command());
        if let Some(command) = current.filter(|c| !c.is_empty()) {
            self.set_composer_string(pane, &command, false);
            if let Some(p) = self.panes.get_mut(&pane).filter(|p| status_bar && visible && p.composer.shown == Shown::Hidden) {
                p.composer.field.load(&command);
            }
        }
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let before = p.composer.shown;
        if before == Shown::Field {
            if status_bar && !visible && p.composer.focused {
                return self.composer_field_expand(pane, false);
            }
            // The temporary field lost its reason to be (the setting changed or a component
            // now shows): its edit ends before the composer toggles.
            if !status_bar || visible {
                let text = p.composer.contents();
                p.composer.field_ended(&text);
            }
        }
        if status_bar && visible {
            match p.composer.shown {
                Shown::Dropdown => {
                    p.composer.toggle(true);
                    self.composer_closed(pane);
                }
                _ if p.composer.field_focused => return self.composer_field_expand(pane, true),
                _ => self.composer_field_take(pane, true),
            }
            return self.redraw();
        }
        let after = p.composer.toggle(status_bar);
        if after == Shown::Dropdown {
            self.composer_opened(pane);
            self.composer_take_keys(pane);
        } else if before == Shown::Dropdown {
            self.composer_closed(pane);
        } else if after == Shown::Field {
            if let Some(p) = self.panes.get_mut(&pane) {
                p.composer.temporary = true;
            }
            self.composer_field_take(pane, false);
        }
        self.redraw();
    }

    /// Where Show Composer goes for `pane` (`CM:173-178`): whether the status bar takes it,
    /// and whether the pane's bar shows a Composer component of its layout.
    fn composer_routing(&mut self, pane: PaneId) -> (bool, bool) {
        let shown = self.bar_shown_for(pane);
        let visible = shown && self.composer_component_visible(pane);
        (composer::using_status_bar(shown, self.globals.bool(ALWAYS_BAR_KEY), visible), visible)
    }

    /// A status bar field of `pane` takes the keyboard: the layout's own (`permanent`), else
    /// the temporary one (`shown` is `Field`). Every other key consumer lets it go, the
    /// pane's dropdown too (the two have their own texts), and the find field stops typing.
    fn composer_field_take(&mut self, pane: PaneId, permanent: bool) {
        self.belt.focus = None;
        self.composer_yield_keys_but(Some(pane));
        if let Some(p) = self.panes.get_mut(&pane) {
            let c = &mut p.composer;
            c.field_pick = None;
            c.field_focused = permanent;
            c.focused = !permanent;
        }
        if let Some(v) = self.composers.get_mut(&pane) {
            v.popup = None;
            v.ghost = None;
        }
        if let Some(f) = self.finds.get_mut(&pane) {
            f.typing = false;
        }
        self.refresh_route();
        self.redraw();
    }

    /// A press on the text of one of `pane`'s Composer fields (`temporary`: the temporary
    /// right one): it takes the keyboard with its own text.
    pub(super) fn composer_field_focus(&mut self, pane: PaneId, temporary: bool) {
        if !self.composer_allowed(pane) {
            return;
        }
        let Some(c) = self.panes.get(&pane).map(|p| &p.composer) else { return };
        let has = if temporary { c.shown == Shown::Field && c.focused } else { c.field_focused };
        if temporary && c.shown != Shown::Field {
            return;
        }
        if !has {
            self.composer_field_take(pane, !temporary);
        }
        self.redraw();
    }

    /// A field's edit ends (Esc, `cancelOperation:`, `SBCVC:152-158`): the layout's field
    /// keeps its text; the temporary one leaves, its text saved; the terminal has the
    /// keyboard again.
    fn composer_field_end(&mut self, pane: PaneId) {
        self.composer_let_go(pane);
        self.refresh_route();
        self.redraw();
    }

    /// The expand button (`Composer::expand`), or Show Composer while a field has the
    /// keyboard: the dropdown shows with the field's text and takes the keyboard.
    /// `permanent`: the layout's own field, else the temporary one.
    pub(super) fn composer_field_expand(&mut self, pane: PaneId, permanent: bool) {
        if !self.composer_allowed(pane) {
            return;
        }
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let opened = p.composer.shown != Shown::Dropdown;
        p.composer.expand(permanent);
        if opened && p.composer.shown == Shown::Dropdown {
            self.composer_opened(pane);
        } else if let Some(v) = self.composers.get_mut(&pane) {
            v.highlight_due = Some(Instant::now());
        }
        if !self.panes.get(&pane).is_some_and(|p| p.composer.hidden) {
            self.composer_take_keys(pane);
        }
        self.redraw();
    }

    /// A pick from a field's history menu fills that field (the combo box's selection): the
    /// temporary one while it shows, else the layout's own; it takes the keyboard.
    pub(super) fn composer_field_fill(&mut self, pane: PaneId, text: &str) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let temporary = p.composer.shown == Shown::Field;
        if temporary {
            p.composer.set_string(text, true);
        } else {
            p.composer.field.load(text);
        }
        self.composer_field_focus(pane, temporary);
        self.redraw();
    }

    /// Key action 64, Compose... (`PS:11996-12003`, `CM:114-139`): ignored on an ended session
    /// or a tmux gateway; `Composer::show_with`, then whatever shows takes the keyboard (the
    /// layout's field when the text went there).
    pub(super) fn composer_show_with(&mut self, pane: PaneId, text: &str) {
        if !self.composer_allowed(pane) {
            return;
        }
        let (status_bar, visible) = self.composer_routing(pane);
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let before = p.composer.shown;
        p.composer.show_with(text, status_bar, visible);
        match p.composer.shown {
            Shown::Dropdown => {
                if before != Shown::Dropdown {
                    self.composer_opened(pane);
                } else if let Some(v) = self.composers.get_mut(&pane) {
                    v.highlight_due = Some(Instant::now());
                }
                if !self.panes.get(&pane).is_some_and(|p| p.composer.hidden) {
                    self.composer_take_keys(pane);
                }
            }
            Shown::Field => self.composer_field_take(pane, false),
            Shown::Hidden if visible => self.composer_field_take(pane, true),
            Shown::Hidden => {}
        }
        self.redraw();
    }

    /// The Snippets tool's Composer (`PS:15202-15205` `openComposerWithString:escaping:`):
    /// the snippet escaped by its mode, appended to a shown dropdown or opening it
    /// (`showOrAppendToDropdownWithString:`), which takes the keyboard.
    pub(super) fn open_composer_with(&mut self, pane: PaneId, text: &str, escaping: crate::keyaction::Escaping) {
        if !self.composer_allowed(pane) {
            return;
        }
        let escaped = crate::keyaction::expand(text, escaping);
        let Some(p) = self.panes.get_mut(&pane) else { return };
        if p.composer.shown == Shown::Field {
            let t = p.composer.contents();
            p.composer.field_ended(&t);
        }
        let opened = p.composer.shown != Shown::Dropdown;
        p.composer.show_or_append(&escaped);
        if opened {
            self.composer_opened(pane);
        } else if let Some(v) = self.composers.get_mut(&pane) {
            v.highlight_due = Some(Instant::now());
        }
        // Copy Mode hides it; it takes the keyboard only when shown (`composer_show_with`).
        if !self.panes.get(&pane).is_some_and(|p| p.composer.hidden) {
            self.composer_take_keys(pane);
        }
        self.redraw();
    }

    /// The dropdown appeared: it fades in and is highlighted.
    fn composer_opened(&mut self, pane: PaneId) {
        self.composer_ai_cancel(pane);
        let v = self.composers.entry(pane).or_default();
        v.opened = Some(Instant::now());
        v.closed = None;
        v.scroll = 0.0;
        v.ghost = None;
        v.popup = None;
        v.waiting = None;
        v.generation += 1;
        v.highlight_due = Some(Instant::now());
    }

    /// The dropdown went away: it fades out, lets the keyboard go, and its file checks are
    /// forgotten (`FC:46-50`, the dismissal resets the file cache).
    fn composer_closed(&mut self, pane: PaneId) {
        let had = self.panes.get_mut(&pane).is_some_and(|p| std::mem::take(&mut p.composer.focused));
        // A request still running would be paid for and its answer dropped.
        self.composer_ai_cancel(pane);
        let v = self.composers.entry(pane).or_default();
        v.closed = Some(Instant::now());
        v.ghost = None;
        v.popup = None;
        v.waiting = None;
        v.generation += 1;
        v.checker.clear_files();
        if had {
            self.refresh_route();
        }
    }

    /// Keys for a focused composer (Review Focus 5): nothing it takes reaches the session, and
    /// neither does its release (`TakenKeys`); a key it does not take goes on only when the
    /// profile maps it to an action that sends no text and is not a navigation key
    /// (`PS:24389-24407`), or it is Ctrl+Tab or Ctrl+Shift+Tab (tab switching, a menu key
    /// equivalent in iTerm2); anything else is dropped.
    pub(super) fn composer_key(&mut self, ev: &KeyEvent) -> bool {
        if ev.state != ElementState::Pressed {
            return self.composer_keys.release(&ev.physical_key);
        }
        let taken = if let Some(pane) = self.focused_composer() {
            self.composer_key_press(pane, ev)
        } else if let Some((pane, _)) = self.live_field() {
            self.composer_field_press(pane, ev)
        } else {
            false
        };
        self.composer_keys.press(ev.physical_key, taken);
        taken
    }

    /// The focused pane's status bar field that has the keyboard, while no toolbelt tool has
    /// it: (pane, whether it is the layout's own field rather than the temporary one).
    fn focused_field(&self) -> Option<(PaneId, bool)> {
        let id = self.focused_id()?;
        let c = &self.panes.get(&id)?.composer;
        if self.belt.focus.is_some() {
            return None;
        }
        if c.field_focused {
            Some((id, true))
        } else {
            (c.shown == Shown::Field && c.focused).then_some((id, false))
        }
    }

    /// `focused_field`, while its bar still shows and places it (`field_keeps_keyboard`);
    /// otherwise its edit ends there and then (the bar was turned off, the component hidden
    /// or dropped for room), so no key edits a field nobody sees.
    fn live_field(&mut self) -> Option<(PaneId, bool)> {
        let (pane, permanent) = self.focused_field()?;
        let placed = self.composer_placed(pane, !permanent).is_some();
        if field_keeps_keyboard(self.bar_shown_for(pane), placed) {
            return Some((pane, permanent));
        }
        self.composer_let_go(pane);
        self.refresh_route();
        self.redraw();
        None
    }

    /// Before each frame: a field whose bar went away lets the keyboard go (`live_field`).
    pub(super) fn check_composer_fields(&mut self) {
        let _ = self.live_field();
    }

    /// A key for the focused Composer field: as the dropdown's, nothing it takes reaches the
    /// session; a key it does not take goes on only as `composer_unhandled` lets it.
    fn composer_field_press(&mut self, pane: PaneId, ev: &KeyEvent) -> bool {
        if is_modifier(&ev.logical_key) {
            return true;
        }
        let m = CMods { ctrl: self.mods.ctrl, alt: self.mods.alt, shift: self.mods.shift, win: self.mods.cmd };
        let Some((key, _)) = decode(&ev.logical_key, ev.text.as_deref(), m) else { return self.composer_unhandled(pane, ev) };
        if self.composer_field_key(pane, key) {
            return true;
        }
        self.composer_unhandled(pane, ev)
    }

    /// One key in `pane`'s focused Composer field, a one-line combo box (`SBCVC:162-176`):
    /// Enter sends the raw text plus "\n" through `user_write` and clears it, even when empty
    /// (no shell history, `SBC:168-171`, a kept bug); Esc and Tab end the edit
    /// (`cancelOperation:`, `controlTextDidEndEditing:`); bare Up and Down step through the
    /// history list, filling the text (`NSComboBox`, `step_field_pick`); the plain edit keys
    /// edit it. False for a key it does not take.
    pub(super) fn composer_field_key(&mut self, pane: PaneId, key: CKey) -> bool {
        let m = CMods { ctrl: self.mods.ctrl, alt: self.mods.alt, shift: self.mods.shift, win: self.mods.cmd };
        let Some(permanent) = self.panes.get(&pane).map(|p| p.composer.field_focused) else { return false };
        if m.win {
            return false;
        }
        let bare = !m.ctrl && !m.alt && !m.shift;
        match key {
            CKey::Esc => self.composer_field_end(pane),
            CKey::Edit(EditKey::Enter) => {
                let acts = self.panes.get_mut(&pane).map(|p| p.composer.field_enter(permanent)).unwrap_or_default();
                for act in acts {
                    if let Act::SendRaw(s) = act {
                        self.user_write(pane, &line_ends(&s), Write::SendText);
                    }
                }
                self.redraw();
            }
            CKey::Edit(EditKey::Tab) if bare => self.composer_field_end(pane),
            CKey::Edit(EditKey::Up | EditKey::Down) if bare => {
                let down = key == CKey::Edit(EditKey::Down);
                let host = self.history_host_pub(pane);
                let list = super::statusbar_content::field_history(&self.shell_history_pub().uses(&host));
                let Some(p) = self.panes.get_mut(&pane) else { return true };
                let c = &mut p.composer;
                if let Some(i) = composer::step_field_pick(c.field_pick, list.len(), down).filter(|i| Some(*i) != c.field_pick) {
                    if permanent {
                        c.field.load(&list[i]);
                    } else {
                        c.set_string(&list[i], true);
                    }
                    c.field_pick = Some(i);
                }
                self.redraw();
            }
            key => {
                let Some(composer::Action::Edit(k)) = composer::action(&key, m, false, false) else { return false };
                if !composer_takes(m, &key, false) {
                    return false;
                }
                self.composer_field_edit(pane, k, m.shift);
            }
        }
        true
    }

    /// One edit key in the focused field (the layout's own text or the temporary field's), on
    /// its one line (the composer's wrap at no width limit): a Shift move extends, Copy and
    /// Cut fill the clipboard, Paste reads it. A change forgets the history list's place.
    fn composer_field_edit(&mut self, pane: PaneId, k: EditKey, shift: bool) {
        let cw = self.cell().0;
        let width = move |s: &str| text_width(s, cw);
        let clip = matches!(k, EditKey::Paste(_)).then(|| self.clipboard_text().unwrap_or_default());
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let permanent = p.composer.field_focused;
        let text = if permanent { p.composer.field.text.clone() } else { p.composer.edit.text.clone() };
        let lines = composer_wrap(&p.composer, &text, f32::INFINITY, cw);
        let edit = if permanent { &mut p.composer.field } else { &mut p.composer.edit };
        let copied = match k {
            k if shift && is_move(&k) => {
                edit.key(k, true, &lines, &width);
                None
            }
            EditKey::Copy => match edit.key(EditKey::Copy, false, &lines, &width) {
                EditOut::Copied(t) => Some(t),
                _ => None,
            },
            EditKey::Cut => {
                let piece = edit.clone().key(EditKey::Copy, false, &lines, &width);
                edit.key(EditKey::Cut, false, &lines, &width);
                match piece {
                    EditOut::Copied(t) if edit.text != text => Some(t),
                    _ => None,
                }
            }
            EditKey::Paste(_) => {
                edit.key(EditKey::Paste(clip.unwrap_or_default()), false, &lines, &width);
                None
            }
            k => {
                edit.key(k, false, &lines, &width);
                None
            }
        };
        if edit.text != text {
            p.composer.field_pick = None;
        }
        if let Some(t) = copied {
            if let Some(cb) = self.clipboard.as_mut() {
                if let Err(e) = cb.set_text(t) {
                    self.banners.set("clipboard", format!("Could not copy: {e}"));
                }
            }
        }
        self.redraw();
    }

    fn composer_key_press(&mut self, pane: PaneId, ev: &KeyEvent) -> bool {
        if is_modifier(&ev.logical_key) {
            return true;
        }
        let m = CMods { ctrl: self.mods.ctrl, alt: self.mods.alt, shift: self.mods.shift, win: self.mods.cmd };
        let Some((key, km)) = decode(&ev.logical_key, ev.text.as_deref(), m) else { return self.composer_unhandled(pane, ev) };
        let auto = self.panes.get(&pane).is_some_and(|p| p.composer.auto);
        let bare = !(km.ctrl || km.alt || km.shift || km.win);
        // Esc first (`CTV:836-851`): the ghost, then the popup, then finishing.
        if key == CKey::Esc && !km.win {
            let v = self.composers.entry(pane).or_default();
            let had_ghost = v.ghost.take().is_some();
            if v.popup.is_some() {
                v.popup = None;
                self.redraw();
                return true;
            }
            if had_ghost {
                self.redraw();
                return true;
            }
        }
        // The popup's own keys, with no modifiers (`CTV:664-690`).
        if bare && self.composers.get(&pane).is_some_and(|v| v.popup.is_some()) {
            match &key {
                CKey::Edit(EditKey::Up) | CKey::Edit(EditKey::Down) => {
                    let down = key == CKey::Edit(EditKey::Down);
                    self.composer_popup_step(pane, down);
                    return true;
                }
                CKey::Edit(EditKey::Enter) if self.composers.get(&pane).is_some_and(|v| v.ghost.as_deref().is_some_and(|g| !g.is_empty())) => {
                    self.composer_accept(pane);
                    return true;
                }
                _ => {}
            }
        }
        let action = composer::action(&key, km, auto, false);
        // The profile's key map comes before the text view (`CTV:853-866`).
        if matches!(action, None | Some(composer::Action::Edit(_))) && !self.leader_on && self.composer_profile_binding(pane, ev) {
            return false;
        }
        if !composer_takes(km, &key, auto) {
            return self.composer_unhandled(pane, ev);
        }
        let Some(action) = action else { return true };
        self.composer_apply(pane, action, km.shift);
        true
    }

    /// A key the composer does not take: Ctrl+Tab and Ctrl+Shift+Tab go on (tab switching),
    /// Ctrl+Shift+C and V copy and paste in the composer, a profile mapping that sends no text
    /// and is no navigation goes on; the rest is dropped. True when it was swallowed.
    fn composer_unhandled(&mut self, pane: PaneId, ev: &KeyEvent) -> bool {
        if !self.leader_on && self.composer_profile_binding(pane, ev) {
            return false;
        }
        let base = keystroke::key_of(&ev.key_without_modifiers(), None);
        match base.and_then(|b| keys::shortcut(&b, self.mods)) {
            Some(Action::NextTab | Action::PrevTab) => false,
            Some(Action::Copy) => {
                self.composer_copy(pane);
                true
            }
            Some(Action::Paste) => {
                self.composer_paste(pane);
                true
            }
            _ => true,
        }
    }

    /// The profile maps this key to an action the composer lets run (`PS:24389-24407`): not
    /// one that sends text (`composer::sends_text`), not on a navigation key, not a remapping
    /// marker. Only the profile's own map counts (iTerm2 reads `KEY_KEYBOARD_MAP` alone).
    fn composer_profile_binding(&self, pane: PaneId, ev: &KeyEvent) -> bool {
        if is_navigation(&ev.logical_key) {
            return false;
        }
        let Some(stroke) = keystroke::from_key_event(ev, self.mods) else { return false };
        let Some(map) = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).and_then(|p| p.get("Keyboard Map")).and_then(|v| v.as_table().cloned()) else { return false };
        let agnostic = self.globals.bool("LanguageAgnosticKeyBindings");
        let Some(b) = stroke.find(&map, agnostic).and_then(|k| map.get(k)).and_then(crate::keyaction::Binding::from_value) else { return false };
        !matches!(b.action, 22 | 24) && !composer::sends_text(b.action)
    }

    /// One composer action at the focused dropdown: Shift moves extend through `TextEdit`,
    /// Copy, Cut and Paste use the clipboard (a multi-caret copy pastes back one group per
    /// caret), everything else through `Composer::apply`; then the text change, the caret
    /// kept in view, and what the action asks for.
    fn composer_apply(&mut self, pane: PaneId, action: composer::Action, shift: bool) {
        let Some((l, lines)) = self.composer_lines(pane) else { return };
        let (cw, line_h) = self.cell();
        let width = move |s: &str| text_width(s, cw);
        let clip_text = match &action {
            composer::Action::Edit(EditKey::Paste(_)) => Some(self.clipboard_text().unwrap_or_default()),
            _ => None,
        };
        let groups = self.composer_clip.clone();
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let c = &mut p.composer;
        c.edit.page_lines = ((l.text.h / line_h.max(1.0)).floor() as usize).max(1);
        let before = (c.edit.text.clone(), c.edit.caret, c.edit.carets());
        let mut copied: Option<(String, Option<Vec<usize>>)> = None;
        let acts = match action {
            composer::Action::Edit(k) if shift && is_move(&k) => {
                c.edit.key(k, true, &lines, &width);
                Vec::new()
            }
            composer::Action::Edit(EditKey::Copy) => {
                if let EditOut::Copied(t) = c.edit.key(EditKey::Copy, false, &lines, &width) {
                    copied = Some((t, c.edit.copy_groups().map(|(_, g)| g)));
                }
                Vec::new()
            }
            composer::Action::Edit(EditKey::Cut) => {
                let piece = c.edit.clone().key(EditKey::Copy, false, &lines, &width);
                let g = c.edit.copy_groups().map(|(_, g)| g);
                let acts = c.apply(composer::Action::Edit(EditKey::Cut), &lines, &width);
                if let (EditOut::Copied(t), true) = (piece, c.edit.text != before.0) {
                    copied = Some((t, g));
                }
                acts
            }
            composer::Action::Edit(EditKey::Paste(_)) => {
                let text = clip_text.unwrap_or_default();
                match groups.filter(|(t, _)| *t == text && c.edit.has_extra() && c.prefix.is_none()) {
                    Some((t, g)) => {
                        c.edit.paste_groups(&t, &g, &lines, &width);
                        Vec::new()
                    }
                    None => c.apply(composer::Action::Edit(EditKey::Paste(text)), &lines, &width),
                }
            }
            a => c.apply(a, &lines, &width),
        };
        let changed = c.edit.text != before.0;
        let moved = c.edit.caret != before.1 || c.edit.carets() != before.2;
        if let Some((text, g)) = copied {
            self.composer_clip = g.map(|g| (text.clone(), g));
            if let Some(cb) = self.clipboard.as_mut() {
                if let Err(e) = cb.set_text(text) {
                    self.banners.set("clipboard", format!("Could not copy: {e}"));
                }
            }
        }
        if changed {
            self.composer_changed(pane);
        } else if moved {
            // A caret move ends the popup (`CTV:266-270`) and the ghost, and the request.
            self.composer_ai_cancel(pane);
            if let Some(v) = self.composers.get_mut(&pane) {
                v.popup = None;
                v.ghost = None;
                v.generation += 1;
                v.waiting = None;
            }
        }
        self.composer_scroll_to_caret(pane);
        self.composer_acts(pane, acts);
        self.redraw();
    }

    /// Keeps the primary caret inside the text's room.
    fn composer_scroll_to_caret(&mut self, pane: PaneId) {
        let Some((l, lines)) = self.composer_lines(pane) else { return };
        let (cw, line_h) = self.cell();
        let Some(p) = self.panes.get(&pane) else { return };
        let (line, _) = p.composer.edit.caret_line(&lines, &|s: &str| text_width(s, cw));
        let v = self.composers.entry(pane).or_default();
        let top = line as f32 * line_h;
        if top < v.scroll {
            v.scroll = top;
        } else if top + line_h > v.scroll + l.text.h {
            v.scroll = top + line_h - l.text.h;
        }
        let content = lines.len() as f32 * line_h;
        v.scroll = v.scroll.clamp(0.0, (content - l.text.h).max(0.0));
    }

    /// The text changed (`LCVC:503-550` `textDidChange:`): the ghost and popup go, the
    /// highlight is due (at most every 0.05 s, `CTV:47-48`), and suggestions are fetched
    /// unless the line before the caret is empty or the caret sits inside a word.
    fn composer_changed(&mut self, pane: PaneId) {
        // A request for the text before this change would only be dropped (ruling P12).
        self.composer_ai_cancel(pane);
        let now = Instant::now();
        let v = self.composers.entry(pane).or_default();
        v.generation += 1;
        v.ghost = None;
        v.popup = None;
        v.waiting = None;
        let due = v.highlighted.map_or(now, |t| (t + command_syntax::RATE).max(now));
        v.highlight_due = Some(v.highlight_due.map_or(due, |d| d.min(due)));
        let Some(p) = self.panes.get(&pane) else { return };
        let c = &p.composer;
        if c.edit.has_extra() || c.edit.selection().is_some() {
            return;
        }
        let text = c.text();
        let caret = c.edit.caret.min(text.len());
        let line = suggest::line_before_cursor(text, caret);
        if suggest::should_fetch(line, &text[caret..]) {
            self.composer_fetch(pane, false);
        }
    }

    /// Suggestions for the line before the caret (`LCVC:606-704`): the host's history here,
    /// the file or PATH listing on the worker. A tmux pane gets the best history item as ghost
    /// text only (`PS:24466-24479`, `LCVC:622-643`). `explicit` (Tab) shows the popup, with
    /// "Thinking..." until the listing arrives.
    pub(super) fn composer_fetch(&mut self, pane: PaneId, explicit: bool) {
        self.composer_ai_cancel(pane);
        let ai_history = if self.globals.bool(crate::ai_settings::AI_COMPLETIONS) {
            let host = self.history_host_pub(pane);
            crate::ai_complete::history_for_prompt(&self.shell_history_pub().uses(&host))
        } else {
            Vec::new()
        };
        let Some(p) = self.panes.get(&pane) else { return };
        let c = &p.composer;
        let text = c.text().to_string();
        let caret = c.edit.caret.min(text.len());
        let at_end = caret == text.len();
        let line = suggest::line_before_cursor(&text, caret).to_string();
        let s = p.own();
        let cwd = s.shell.cwd.as_deref().and_then(crate::profile::local_path);
        let host = self.history_host_pub(pane);
        let now = super::toolbelt_tools_ui::unix(std::time::SystemTime::now());
        let history = suggest::history(self.shell_history_pub(), &host, &line, suggest::HISTORY_MAX, now);
        let tmux = self.tmux_of(pane).is_some();
        let v = self.composers.entry(pane).or_default();
        v.generation += 1;
        let generation = v.generation;
        if tmux {
            v.ghost = history.first().map(|i| suggest::shown_ghost(i.value.trim_end_matches(['\r', '\n']), at_end).to_string()).filter(|g| !g.is_empty());
            return;
        }
        let (on_first_word, prefix) = suggest::word_to_complete(&line);
        let inputs = crate::ai_complete::Inputs { full_prefix: text[..caret].to_string(), full_suffix: text[caret..].to_string(), working_directory: cwd.as_ref().map(|d| d.display().to_string()), executable: on_first_word };
        let (dir_part, name) = match prefix.rfind(['\\', '/']) {
            Some(i) => (&prefix[..=i], &prefix[i + 1..]),
            None => ("", prefix),
        };
        let fetch = Fetch { generation, explicit, line: line.clone(), name: name.to_string(), history, inputs, ai_history, first: None, files: Vec::new(), expand: Vec::new(), ai_prefix: String::new() };
        v.waiting = Some(fetch);
        if name.is_empty() || crate::prompted::network_path(dir_part) {
            return self.composer_found(pane, generation, Vec::new());
        }
        // A pane whose conductor frames asks the remote (14d); anything else lists locally.
        let after: String = text[caret..].chars().take(256).collect();
        if self.ssh_composer_fetch(pane, generation, &line, &after, name, explicit) {
            return;
        }
        let base = cwd.clone().unwrap_or_else(|| PathBuf::from(home()));
        let join = |d: &Path| if dir_part.is_empty() { d.to_path_buf() } else { d.join(dir_part) };
        let dirs: Vec<(PathBuf, bool)> = if Path::new(dir_part).is_absolute() {
            vec![(PathBuf::from(dir_part), false)]
        } else if on_first_word && dir_part.is_empty() {
            env_list("PATH").into_iter().map(|d| (PathBuf::from(d), true)).collect()
        } else {
            vec![(join(&base), false)]
        };
        let job = Job::Files { pane, generation, dirs };
        if self.composer_worker.send(job).is_err() {
            self.composer_found(pane, generation, Vec::new());
        }
        self.redraw();
    }

    /// The listing for fetch `generation` arrived (`didFindCompletions:`, `LCVC:745-803`):
    /// files first, then history, deduplicated; the ghost from them (`composer_suggest::ghost`,
    /// suggestion-only while typing); the popup only for Tab.
    pub(super) fn composer_found(&mut self, pane: PaneId, generation: u64, entries: Vec<(String, bool)>) {
        let Some(p) = self.panes.get(&pane) else { return };
        let c = &p.composer;
        let at_end = c.edit.caret >= c.text().len();
        let shell = p.own().shell.shell.clone();
        if self.composers.get(&pane).and_then(|v| v.waiting.as_ref()).is_none_or(|f| f.generation != generation) {
            return;
        }
        // The model is asked only while AI Completion is on, never for a tmux session (its
        // fetch returned before this), and only past the gate (`LC:622-643`, `AC:16-95`).
        let ai = self.globals.bool(crate::ai_settings::AI_COMPLETIONS) && self.ai_check(true);
        let Some(v) = self.composers.get_mut(&pane) else { return };
        let Some(mut f) = v.waiting.take() else { return };
        let limit = if f.explicit { suggest::FILES_TAB } else { suggest::FILES_TYPING };
        let files = suggest::files(&entries, &f.name, limit, shell.as_deref());
        if ai {
            f.first = crate::ai_complete::early_item(&files, &f.history, &f.line);
            // The early item is the ghost at once (`setEarlyResult:`).
            v.ghost = f.first.as_ref().map(|i| suggest::shown_ghost(i.value.trim_end_matches(['\r', '\n']), at_end).to_string()).filter(|g| !g.is_empty());
            // iTerm2 hands the model the unescaped values (the remainders after the word).
            f.expand = files.iter().map(|i| Item { value: crate::shell_history::remainder(&i.detail, &f.name), ..i.clone() }).collect();
            let values: Vec<String> = f.expand.iter().map(|i| i.value.clone()).collect();
            let (text, prefix) = crate::ai_complete::prompt(&f.inputs, &f.ai_history, &values);
            f.ai_prefix = prefix;
            f.files = files;
            v.waiting = Some(f);
            // The request can answer inside this call (a closed gate), so the fetch is kept first.
            if let Some(id) = self.ai_one_shot(text, super::ai_runs::Owner::Complete { pane, generation }) {
                self.composer_ai.insert(pane, id);
            }
            self.redraw();
            return;
        }
        let mut items = files;
        items.extend(f.history);
        let items = suggest::dedupe(items);
        if items.is_empty() {
            v.ghost = None;
            v.popup = None;
            self.redraw();
            return;
        }
        v.ghost = suggest::ghost(&items, !f.explicit).map(|g| suggest::shown_ghost(&g, at_end).to_string()).filter(|g| !g.is_empty());
        v.popup = f.explicit.then(|| Popup { items, typed: f.line, selected: 0 });
        self.redraw();
    }

    /// Ends the pane's request in flight, if any (ruling P12: a new fetch, a change and a
    /// closing pane cancel it).
    pub(super) fn composer_ai_cancel(&mut self, pane: PaneId) {
        if let Some(id) = self.composer_ai.remove(&pane) {
            self.ai_run_cancel(id);
        }
    }

    /// The model's answer for fetch `generation` (`AC:49-95`, `LC:745-803`): its items pinned
    /// behind the early item, then the history; the popup opens while typing (suggestion-only
    /// false). Any failure, including a closed gate, leaves the files.
    pub(super) fn composer_ai_done(&mut self, pane: PaneId, generation: u64, finished: super::ai_runs::Finished) {
        use super::ai_runs::Finished;
        self.composer_ai.remove(&pane);
        let Some(p) = self.panes.get(&pane) else { return };
        let at_end = p.composer.edit.caret >= p.composer.text().len();
        let Some(v) = self.composers.get_mut(&pane) else { return };
        if !ai_reply_current(v.waiting.as_ref().map(|f| f.generation), generation) {
            return;
        }
        let Some(f) = v.waiting.take() else { return };
        let ai_items = match finished {
            Finished::Choice(text) => crate::ai_complete::items_from_response_with(&text, &f.ai_prefix, &f.expand, &f.files),
            Finished::Failed(_) => f.files.clone(),
            Finished::Canceled => {
                self.redraw();
                return;
            }
        };
        let items = crate::ai_complete::with_first(f.first, ai_items);
        let (rows, ghost) = crate::ai_complete::shown(&items, &f.history);
        v.ghost = ghost.map(|g| suggest::shown_ghost(&g, at_end).to_string()).filter(|g| !g.is_empty());
        v.popup = (!rows.is_empty()).then(|| Popup { items: rows, typed: f.line, selected: 0 });
        self.redraw();
    }

    /// Up or Down in the popup: the selection moves and its value becomes the ghost
    /// (`CTV:195-206`).
    fn composer_popup_step(&mut self, pane: PaneId, down: bool) {
        let at_end = self.panes.get(&pane).is_some_and(|p| p.composer.edit.caret >= p.composer.text().len());
        let Some(v) = self.composers.get_mut(&pane) else { return };
        let Some(pop) = v.popup.as_mut() else { return };
        let n = pop.items.len();
        if n == 0 {
            return;
        }
        pop.selected = if down { (pop.selected + 1).min(n - 1) } else { pop.selected.saturating_sub(1) };
        let item = &pop.items[pop.selected];
        v.ghost = (!item.replacement).then(|| suggest::shown_ghost(&item.value, at_end).to_string()).filter(|g| !g.is_empty());
        self.redraw();
    }

    /// Accepts the ghost text (`acceptSuggestion`, `CTV:545-551`): it is typed at the caret,
    /// the popup closes, and nothing is fetched until the next change.
    fn composer_accept(&mut self, pane: PaneId) {
        let Some(v) = self.composers.get_mut(&pane) else { return };
        v.popup = None;
        let Some(g) = v.ghost.take() else { return self.redraw() };
        let Some((_, lines)) = self.composer_lines(pane) else { return };
        let cw = self.cell().0;
        let Some(p) = self.panes.get_mut(&pane) else { return };
        p.composer.apply(composer::Action::Edit(EditKey::Text(g)), &lines, &|s: &str| text_width(s, cw));
        let v = self.composers.entry(pane).or_default();
        v.generation += 1;
        v.highlight_due = Some(Instant::now());
        self.composer_scroll_to_caret(pane);
        self.redraw();
    }

    /// Types `text` into `pane`'s composer at the caret (a history or directory pick,
    /// `PS:20422-20425`).
    pub(super) fn composer_insert(&mut self, pane: PaneId, text: &str) {
        if text.is_empty() {
            return;
        }
        let Some((_, lines)) = self.composer_lines(pane) else { return };
        let cw = self.cell().0;
        let Some(p) = self.panes.get_mut(&pane) else { return };
        p.composer.apply(composer::Action::Edit(EditKey::Text(text.to_string())), &lines, &|s: &str| text_width(s, cw));
        self.composer_changed(pane);
        self.composer_scroll_to_caret(pane);
        self.redraw();
    }

    /// Whether `pane`'s dropdown shows (a history pick goes into it).
    pub(super) fn composer_shown(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_some_and(|p| p.composer.shown == Shown::Dropdown && !p.composer.hidden)
    }

    /// What composer actions ask for (`Composer::apply`'s acts).
    pub(super) fn composer_acts(&mut self, pane: PaneId, acts: Vec<Act>) {
        for act in acts {
            match act {
                Act::Send(s) => self.composer_send(pane, &s),
                Act::Enqueue(s) => self.composer_enqueue(pane, &s),
                Act::Control(bytes) => self.user_write(pane, &bytes, Write::SendText),
                Act::SendRaw(s) => self.user_write(pane, &line_ends(&s), Write::SendText),
                Act::OpenHistory { prefix, search } => self.composer_windows.history = Some((pane, prefix, !search)),
                // The Auto Composer's Ctrl+L (`composerManagerClear:`, `PS:24352-24356`).
                Act::ClearScreen => {
                    if let Some(p) = self.panes.get_mut(&pane) {
                        p.clear_for_composer();
                    }
                }
                Act::AdvancedPaste(text) => {
                    // `CM:370-376`: dismissed with nothing saved, then Advanced Paste of the
                    // text, which replaces the clipboard first (`PS:15193-15200`).
                    if let Some(p) = self.panes.get_mut(&pane) {
                        p.composer.shown = Shown::Hidden;
                        p.composer.saved = None;
                    }
                    if let Some(cb) = self.clipboard.as_mut() {
                        if let Err(e) = cb.set_text(text) {
                            self.banners.set("clipboard", format!("Could not copy: {e}"));
                        }
                    }
                    self.composer_windows.advanced_paste = Some(pane);
                }
                Act::Completions { .. } => {
                    // Tab (`CTV:706-715`): the ghost is accepted, else suggestions are asked
                    // for explicitly.
                    if self.composers.get(&pane).is_some_and(|v| v.ghost.as_deref().is_some_and(|g| !g.is_empty())) {
                        self.composer_accept(pane);
                    } else {
                        self.composer_fetch(pane, true);
                    }
                }
                Act::AcceptSuggestion => self.composer_accept(pane),
                Act::Dismissed => {}
            }
        }
        // A send or Esc that hid the dropdown.
        let hidden = self.panes.get(&pane).is_some_and(|p| p.composer.shown != Shown::Dropdown);
        let open = self.composers.get(&pane).is_some_and(|v| v.closed.is_none() && v.opened.is_some());
        if hidden && open {
            self.composer_closed(pane);
        }
    }

    /// `sendCommand:` (`PS:24546-24596`): with no command range (not at a shell integration
    /// prompt) the command is recorded in the host's history here; the Auto Composer sends ^U
    /// first when the shell had put text at the prompt, drops its prefix and tells the prompt
    /// state machine; then the text goes through `user_write` (broadcast, tmux send-keys) with
    /// its line ends as Enter.
    pub(super) fn composer_send(&mut self, pane: PaneId, command: &str) {
        let Some(p) = self.panes.get(&pane) else { return };
        let s = p.own();
        if s.at_prompt() != Some(true) {
            let dir = s.shell.cwd.clone();
            let host = self.history_host_pub(pane);
            let now = super::toolbelt_tools_ui::unix(std::time::SystemTime::now());
            self.shell_history_pub().record_command(&host, command, dir.as_deref(), now);
            self.save_shell_history();
            self.tools_changed();
        }
        let mut out = command.to_string();
        let mut auto = false;
        if let Some(p) = self.panes.get_mut(&pane) {
            if p.composer.auto {
                auto = true;
                if p.composer.shell_provided {
                    out.insert(0, '\u{15}');
                }
                let contents = p.composer.contents();
                p.composer.prefix = None;
                p.composer.set_string(&contents, false);
                p.own_mut().prompt_state.will_send();
            }
        }
        self.user_write(pane, &line_ends(&out), Write::SendText);
        if auto {
            // Dismissed at once, not faded (`dismissAnimated:NO`).
            if let Some(p) = self.panes.get_mut(&pane) {
                p.composer.dismiss();
            }
            self.composer_gone(pane);
        }
        self.redraw();
    }

    /// `enqueueCommand:` (`PS:24375-24383`): sent now when the shell prompt has nothing typed,
    /// else queued for the next mark.
    pub(super) fn composer_enqueue(&mut self, pane: PaneId, command: &str) {
        let Some(p) = self.panes.get(&pane) else { return };
        let auto_shown = p.composer.auto && p.composer.shown == Shown::Dropdown;
        let current = composer::current_command(auto_shown, &p.composer.contents(), p.own().typed_command());
        if composer::enqueue_now(current.as_deref()) {
            self.composer_send(pane, command);
        } else if let Some(p) = self.panes.get_mut(&pane) {
            p.composer.queue.push_back(command.to_string());
        }
    }

    /// One queued command per new mark (`screenDidAddMark`, `PS:16613-16617`), every pane.
    pub(super) fn drain_composer_queues(&mut self) {
        let mut sends = Vec::new();
        for (id, p) in self.panes.iter_mut() {
            let added = p.own_mut().take_marks_added();
            for c in composer::drain(&mut p.composer.queue, added) {
                sends.push((*id, c));
            }
        }
        for (id, c) in sends {
            self.composer_send(id, &c);
        }
    }

    /// Copy in a focused composer (`CTV:384-392`): its own selection; else, with an Auto
    /// Composer, the terminal's (`PS:24442-24447`). True when a composer had the keyboard.
    pub(super) fn composer_copy_menu(&mut self) -> bool {
        let Some(pane) = self.focused_composer().or_else(|| self.live_field().map(|(p, _)| p)) else { return false };
        self.composer_copy(pane);
        true
    }

    fn composer_copy(&mut self, pane: PaneId) {
        let Some(p) = self.panes.get(&pane) else { return };
        if p.composer.field_focused || p.composer.shown == Shown::Field {
            return self.composer_field_edit(pane, EditKey::Copy, false);
        }
        let has_selection = p.composer.edit.carets().iter().any(|r| !r.is_empty());
        if has_selection {
            self.composer_apply(pane, composer::Action::Edit(EditKey::Copy), false);
        } else if p.composer.auto {
            self.copy_selection(pane);
        }
    }

    /// Paste in a focused composer, or into the focused pane's Auto Composer, which takes the
    /// keyboard unless Copy Mode hides it (`PS:13053-13057`). True when a composer took it.
    pub(super) fn composer_paste_menu(&mut self) -> bool {
        if let Some((pane, _)) = self.live_field() {
            self.composer_paste(pane);
            return true;
        }
        let auto = self.focused_id().filter(|id| self.have_auto_composer(*id) && self.panes.get(id).is_some_and(|p| !p.composer.hidden));
        let Some(pane) = self.focused_composer().or(auto) else { return false };
        self.composer_take_keys(pane);
        self.composer_paste(pane);
        true
    }

    fn composer_paste(&mut self, pane: PaneId) {
        if self.panes.get(&pane).is_some_and(|p| p.composer.field_focused || p.composer.shown == Shown::Field) {
            return self.composer_field_edit(pane, EditKey::Paste(String::new()), false);
        }
        self.composer_apply(pane, composer::Action::Edit(EditKey::Paste(String::new())), false);
    }

    /// Edit > Paste Special > Advanced Paste in a focused composer (`LCVC:152-158`): its
    /// whole text. True when a composer had the keyboard.
    pub(super) fn composer_advanced_paste_menu(&mut self) -> bool {
        let Some(pane) = self.focused_composer() else { return false };
        let text = self.panes.get(&pane).map(|p| p.composer.text().to_string()).unwrap_or_default();
        self.composer_acts(pane, vec![Act::AdvancedPaste(format!("{text}\n"))]);
        self.redraw();
        true
    }

    /// Copy with Styles and Copy with Control Sequences while a composer has the keyboard
    /// (`CTV:348-398`): they act on the terminal's selection only for an Auto Composer; with a
    /// dropdown they do nothing. True when they must do nothing.
    pub(super) fn composer_blocks_styled_copy(&self) -> bool {
        self.focused_composer().is_some_and(|id| self.panes.get(&id).is_some_and(|p| !p.composer.auto))
    }

    /// A Win chord (`composer::chord_of`): Win+W closes a shown dropdown (never an Auto
    /// Composer, `PS:8737-8743`); with a composer that has the keyboard, Win+Shift+Up and Down
    /// step marks (`PS:24608-24625`), Win+Alt+V sends the text to Advanced Paste, Win+Y engages AI, Win+Shift+; opens the command history for the text before the
    /// caret on its line. Win+F goes on to Find, which takes the keyboard. True when used.
    pub(super) fn composer_chord(&mut self, chord: &crate::menus::Chord) -> bool {
        let Some(c) = composer::chord_of(chord) else { return false };
        if c == composer::Chord::Close {
            let Some(pane) = self.focused_id() else { return false };
            let Some(p) = self.panes.get_mut(&pane) else { return false };
            if p.composer.shown != Shown::Dropdown || p.composer.auto {
                return false;
            }
            p.composer.dismiss();
            self.composer_closed(pane);
            self.redraw();
            return true;
        }
        let Some(pane) = self.focused_composer() else { return false };
        match c {
            composer::Chord::Mark { down } => self.step_mark(pane, down),
            composer::Chord::AdvancedPaste => {
                return self.composer_advanced_paste_menu();
            }
            composer::Chord::Ai => self.engage_from_composer(pane),
            composer::Chord::Find | composer::Chord::Close => return false,
            composer::Chord::History => {
                let Some(p) = self.panes.get(&pane) else { return false };
                let text = p.composer.text();
                let caret = p.composer.edit.caret.min(text.len());
                let line = suggest::line_before_cursor(text, caret).to_string();
                self.composer_windows.history = Some((pane, line, true));
            }
        }
        self.redraw();
        true
    }

    /// A press: in a shown dropdown's popup (a row is chosen and accepted), or its panel
    /// (the close button, Help, AI, the drag handle, the text, or the border); a press
    /// anywhere else lets the composers' keyboard go. True when the press was the composer's.
    pub(super) fn composer_press(&mut self, button: MouseButton, x: f32, y: f32) -> bool {
        let panes: Vec<(PaneId, Rect)> = self.tabs.get(self.active).map(|t| self.visible(t)).unwrap_or_default();
        for (pane, rect) in panes {
            if !self.composer_shown(pane) {
                continue;
            }
            if let Some(hit) = self.composer_popup_hit(pane, rect, x, y) {
                self.composer_focus_pane(button, pane);
                if let (Some(row), MouseButton::Left) = (hit, button) {
                    if let Some(v) = self.composers.get_mut(&pane) {
                        if let Some(pop) = v.popup.as_mut() {
                            pop.selected = row;
                            let item = pop.items[row].clone();
                            v.ghost = (!item.replacement).then_some(item.value).filter(|g| !g.is_empty());
                        }
                    }
                    self.composer_accept(pane);
                }
                return true;
            }
            let l = self.composer_layout(pane, rect);
            if !l.panel.contains(x, y) {
                continue;
            }
            if button != MouseButton::Left {
                return true;
            }
            self.composer_focus_pane(button, pane);
            self.composer_take_keys(pane);
            if let Some(v) = self.composers.get_mut(&pane) {
                v.popup = None;
                v.ghost = None;
            }
            let scale = self.scale();
            let ai = self.globals.bool("GenerativeAIAllowed");
            if l.close.contains(x, y) {
                // The close button is Esc (`MCVC:388-393`).
                if let Some((_, lines)) = self.composer_lines(pane) {
                    let cw = self.cell().0;
                    let acts = self.panes.get_mut(&pane).map(|p| p.composer.apply(composer::Action::Esc, &lines, &|s: &str| text_width(s, cw))).unwrap_or_default();
                    self.composer_acts(pane, acts);
                }
            } else if l.help.contains(x, y) {
                let origin = self.gpu.as_ref().and_then(|g| g.window.inner_position().ok()).unwrap_or_default();
                self.composer_windows.help_at = Some((origin.x as f64 + f64::from(l.help.x + l.help.w / 2.0), origin.y as f64 + f64::from(l.help.y)));
            } else if ai && l.ai.contains(x, y) {
                self.engage_from_composer(pane);
            } else if l.handle.contains(x, y) {
                self.drag = Drag::Composer { pane, kind: ComposerDrag::Resize { start_y: y, start_h: l.panel.h / scale } };
            } else if l.text.contains(x, y) {
                self.composer_text_press(pane, x, y);
            } else if !self.have_auto_composer(pane) {
                let offset = (l.panel.y - rect.y) / scale;
                self.drag = Drag::Composer { pane, kind: ComposerDrag::Move { start_y: y, start_offset: offset } };
            }
            self.redraw();
            return true;
        }
        // A press on a Composer field keeps its keyboard (the bar acts on it next).
        let keep = self.composer_field_under(x, y);
        self.composer_yield_keys_but(keep);
        false
    }

    /// A press on `pane`'s composer focuses that pane first, the way a press on its terminal
    /// does (`press_focus`).
    fn composer_focus_pane(&mut self, button: MouseButton, pane: PaneId) {
        if let Some(p) = press_focus(button == MouseButton::Left, pane, self.focused_id()) {
            if let Some(t) = self.tabs.get_mut(self.active) {
                t.focused = p;
            }
            self.refresh_route();
        }
    }

    /// The wheel over a shown dropdown scrolls its text (never the program under it). True
    /// when the pointer is over one.
    pub(super) fn composer_wheel(&mut self, dy: f32) -> bool {
        let (x, y) = self.cursor;
        let panes: Vec<(PaneId, Rect)> = self.tabs.get(self.active).map(|t| self.visible(t)).unwrap_or_default();
        for (pane, rect) in panes {
            if !self.composer_shown(pane) {
                continue;
            }
            // The suggestion popup, where it reaches past the panel, swallows the wheel too.
            if self.composer_popup_hit(pane, rect, x, y).is_some() {
                return true;
            }
            if !self.composer_layout(pane, rect).panel.contains(x, y) {
                continue;
            }
            let Some((l, lines)) = self.composer_lines(pane) else { return true };
            let line_h = self.cell().1;
            let content = lines.len() as f32 * line_h;
            let v = self.composers.entry(pane).or_default();
            v.scroll = (v.scroll - dy * line_h).clamp(0.0, (content - l.text.h).max(0.0));
            self.redraw();
            return true;
        }
        false
    }

    /// The focused pane or tab changed (from the keyboard as from the tab bar): a composer in
    /// a pane that is no longer focused lets the keyboard go (`PS:21021-21026`: it takes it
    /// back only with the terminal's).
    pub(super) fn composer_focus_follows(&mut self) {
        let focused = self.focused_id();
        // Only this window's panes: another window keeps its composer's keyboard for when it
        // comes back (a window restores its first responder).
        let mine: Vec<(PaneId, bool)> = self.tabs.iter().flat_map(|t| t.layout.panes()).filter_map(|id| self.panes.get(&id).map(|p| (id, p.composer.focused || p.composer.field_focused))).collect();
        let release = composers_to_release(&mine, focused);
        let any = !release.is_empty();
        for id in release {
            self.composer_let_go(id);
        }
        if any {
            self.refresh_route();
            self.redraw();
        }
        // `notifyActive` (`PS:13557-13570`): a session that becomes the active one gives its
        // Auto Composer the keyboard, unless Copy Mode or another key consumer has it.
        self.composer_take_back(focused.is_some());
    }

    /// The pointer over a dropdown's Help button shows its tooltip (the Clippings buttons'
    /// precedent).
    fn composer_hover_move(&mut self, x: f32, y: f32) {
        let panes: Vec<(PaneId, Rect)> = self.tabs.get(self.active).map(|t| self.visible(t)).unwrap_or_default();
        let hover = panes.into_iter().find(|(pane, rect)| self.composer_shown(*pane) && self.composer_layout(*pane, *rect).help.contains(x, y)).map(|(p, _)| p);
        if hover != self.composer_hover {
            self.composer_hover = hover;
            self.redraw();
        }
    }

    /// The byte index under a point of `pane`'s text, its visual line and x, and whether it is
    /// a soft-wrapped line's end.
    fn composer_index_at(&self, pane: PaneId, x: f32, y: f32) -> Option<(usize, bool, usize, f32)> {
        let (l, lines) = self.composer_lines(pane)?;
        let (cw, line_h) = self.cell();
        let scroll = self.composers.get(&pane).map_or(0.0, |v| v.scroll);
        let text = &self.panes.get(&pane)?.composer.edit.text;
        let line = (((y - l.text.y + scroll) / line_h.max(1.0)).floor().max(0.0) as usize).min(lines.len().saturating_sub(1));
        let lx = x - l.text.x;
        let at = text_edit::index_at(text, &lines, line, lx, &|s: &str| text_width(s, cw));
        Some((at, text_edit::ends_line(&lines, line, at), line, lx))
    }

    /// A press in the text (`MCTV:211-368`): Win alone asks explainshell about the command
    /// there (`CTV:984-1000`), Ctrl+Shift adds a caret, Alt starts a box of carets, else the
    /// caret goes there (Shift extends, a double press selects a word) and a drag selects.
    fn composer_text_press(&mut self, pane: PaneId, x: f32, y: f32) {
        let Some((at, end, line, lx)) = self.composer_index_at(pane, x, y) else { return };
        let m = self.mods;
        if m.cmd && !m.ctrl && !m.alt && !m.shift {
            return self.composer_explain(pane, at);
        }
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let c = &mut p.composer;
        let kind = if m.ctrl && m.shift && !m.alt {
            c.edit.click_add(at);
            None
        } else if m.alt && !m.ctrl {
            c.edit.set_carets(&[text_edit::Caret { at, anchor: None }]);
            Some(ComposerDrag::Select { from: Some((line, lx)) })
        } else {
            c.click(at, end, m.shift, now_ms(), super::clippings_ui::double_click_ms());
            Some(ComposerDrag::Select { from: None })
        };
        // A press inside the prompt prefix lands at its end (the next key is typed there).
        c.keep_out_of_prefix();
        if let Some(kind) = kind {
            self.drag = Drag::Composer { pane, kind };
        }
        self.composer_ai_cancel(pane);
        if let Some(v) = self.composers.get_mut(&pane) {
            v.generation += 1;
            v.waiting = None;
        }
    }

    /// Win+click on a command (`CE:63-86`): "Open ExplainShell?" unless silenced, then the
    /// browser; nothing without a browser for https.
    fn composer_explain(&mut self, pane: PaneId, at: usize) {
        let Some(p) = self.panes.get(&pane) else { return };
        let c = &p.composer;
        let n = c.text().len() - c.contents().len();
        if at < n {
            return;
        }
        let Some(command) = composer::explain_command(&c.contents(), at - n) else { return };
        let url = composer::explain_url(&command);
        let Some(browser) = browser_name() else { return };
        if self.globals.bool(composer::EXPLAIN_KEY) {
            return self.open_url(pane, &url);
        }
        if self.composer_windows.explain.is_none() {
            self.composer_windows.explain_ask = Some((pane, url, browser));
        }
    }

    /// Pointer motion during a composer drag: the selection (or the box of carets), the
    /// height (saved on every move, `MCVC:581-607`), or the offset from the pane's top.
    pub(super) fn composer_move(&mut self, x: f32, y: f32) {
        self.composer_hover_move(x, y);
        let Drag::Composer { pane, kind } = self.drag else { return };
        let scale = self.scale();
        match kind {
            ComposerDrag::Select { from } => {
                let Some((at, end, line, lx)) = self.composer_index_at(pane, x, y) else { return };
                let Some((_, lines)) = self.composer_lines(pane) else { return };
                let cw = self.cell().0;
                let Some(p) = self.panes.get_mut(&pane) else { return };
                match from {
                    Some(start) => p.composer.edit.box_select(start, (line, lx), &lines, &|s: &str| text_width(s, cw)),
                    None => p.composer.edit.drag_to(at, end),
                }
                p.composer.keep_out_of_prefix();
            }
            ComposerDrag::Resize { start_y, start_h } => {
                let Some(rect) = self.pane_rect(pane) else { return };
                let h = composer::clamp_height(start_h + (y - start_y) / scale, rect.h / scale);
                self.globals.set(HEIGHT_KEY, toml::Value::Float(f64::from(h)));
            }
            ComposerDrag::Move { start_y, start_offset } => {
                let Some(rect) = self.pane_rect(pane) else { return };
                let h = composer::clamp_height(self.composer_height(), rect.h / scale);
                let offset = (start_offset + (y - start_y) / scale).clamp(0.0, (rect.h / scale - h).max(0.0));
                let Some(p) = self.panes.get(&pane) else { return };
                let guid = self.store.shared_guid(&p.profile);
                if let Some(profile) = self.store.get_mut(&guid) {
                    profile.set(TOP_OFFSET, toml::Value::Float(f64::from(offset)));
                }
            }
        }
        self.redraw();
    }

    /// A composer drag let go: the height or the offset is written.
    pub(super) fn composer_release(&mut self, pane: PaneId, kind: ComposerDrag) {
        match kind {
            ComposerDrag::Select { .. } => self.composer_scroll_to_caret(pane),
            ComposerDrag::Resize { .. } => self.save_globals(),
            ComposerDrag::Move { .. } => self.save_due = Some(Instant::now() + SAVE_DELAY),
        }
        self.redraw();
    }

    /// What a point on `pane`'s popup hits: Some(Some(row)) a row, Some(None) the popup
    /// without a row (the "Thinking..." placeholder), None not the popup.
    fn composer_popup_hit(&self, pane: PaneId, rect: Rect, x: f32, y: f32) -> Option<Option<usize>> {
        let (caret, rows, selected, detail, widest, thinking) = self.composer_popup_geometry(pane, rect)?;
        let scale = self.scale();
        let (r, first) = scene::composer_popup_frame(caret, rect, rows, selected, detail, widest, scale);
        if !r.contains(x, y) {
            return None;
        }
        if thinking {
            return Some(None);
        }
        let k = ((y - r.y - scene::POPUP_SPACING * scale) / ((scene::POPUP_ROW + scene::POPUP_SPACING) * scale)).floor();
        let row = first + k.max(0.0) as usize;
        Some(Some(row.min(rows.saturating_sub(1))))
    }

    /// The popup's caret (x, top, bottom), rows, selection, whether it has a detail strip,
    /// its widest row (the same numbers `composer_popup` draws with), and whether it is the
    /// "Thinking..." placeholder alone.
    fn composer_popup_geometry(&self, pane: PaneId, rect: Rect) -> Option<((f32, f32, f32), usize, Option<usize>, bool, f32, bool)> {
        let v = self.composers.get(&pane)?;
        let caret = self.composer_caret_rect(pane, rect)?;
        let at = (caret.x, caret.y, caret.y + caret.h);
        let cw = self.cell().0;
        match v.popup.as_ref() {
            Some(pop) => {
                let widest = pop.items.iter().map(|i| (suggest::row_text(i, &pop.typed).0.chars().count() as f32 + 3.0) * cw).fold(0.0, f32::max);
                Some((at, pop.items.len(), Some(pop.selected), true, widest, false))
            }
            None if v.waiting.as_ref().is_some_and(|f| f.explicit) => Some((at, 1, None, false, 0.0, true)),
            None => None,
        }
    }

    /// The primary caret's rectangle on screen.
    fn composer_caret_rect(&self, pane: PaneId, rect: Rect) -> Option<Rect> {
        let l = self.composer_layout(pane, rect);
        let (cw, line_h) = self.cell();
        let c = &self.panes.get(&pane)?.composer;
        let lines = composer_wrap(c, &c.edit.text, l.text.w, cw);
        let (line, x) = c.edit.caret_line(&lines, &|s: &str| text_width(s, cw));
        let scroll = self.composers.get(&pane).map_or(0.0, |v| v.scroll);
        Some(Rect { x: l.text.x + x, y: l.text.y + line as f32 * line_h - scroll, w: 1.0, h: line_h })
    }

    /// Draws `pane`'s dropdown over pane rectangle `rect` (`MCVC`): fading in over 0.25 s and
    /// out over 0.125 s, the text in the pane's font and colors with the highlight, the ghost
    /// at half alpha after the caret, the selection as Notes draws it, the carets in the
    /// cursor color while it has the keyboard, the accessory row; then its popup.
    pub(super) fn draw_composer(&self, frame: &mut Frame, pane: PaneId, rect: Rect, cell: (f32, f32)) -> Vec<(crate::mouse::Overlay, Rect)> {
        use crate::mouse::Overlay;
        let mut out = Vec::new();
        let Some(p) = self.panes.get(&pane) else { return out };
        let c = &p.composer;
        let v = self.composers.get(&pane);
        let now = Instant::now();
        let alpha = if c.shown == Shown::Dropdown && !c.hidden {
            v.and_then(|v| v.opened).map_or(1.0, |t| (now.duration_since(t).as_secs_f32() / composer::FADE_IN).min(1.0))
        } else {
            match v.and_then(|v| v.closed) {
                Some(t) if now.duration_since(t).as_secs_f32() < composer::FADE_OUT => 1.0 - now.duration_since(t).as_secs_f32() / composer::FADE_OUT,
                _ => return out,
            }
        };
        let scale = self.scale();
        let l = self.composer_layout(pane, rect);
        let (cw, line_h) = cell;
        let width = |s: &str| text_width(s, cw);
        let pal = &p.screen.palette;
        let text = c.edit.text.as_str();
        let caret = c.edit.caret.min(text.len());
        let open = c.shown == Shown::Dropdown;
        let ghost = v.and_then(|v| v.ghost.as_deref()).filter(|g| open && !g.is_empty() && c.edit.selection().is_none() && !c.edit.has_extra());
        let g = ghost.map_or(0, str::len);
        let display = match ghost {
            Some(gh) => format!("{}{}{}", &text[..caret], gh, &text[caret..]),
            None => text.to_string(),
        };
        let at = |i: usize| if i > caret { i + g } else { i };
        let lines = composer_wrap(c, &display, l.text.w, cw);
        let mut segs: Vec<(Range<usize>, Rgb, bool)> = Vec::new();
        for (r, color, bold) in valid_spans(text, v.map_or(&[][..], |v| &v.spans)) {
            if g > 0 && r.start < caret && r.end > caret {
                segs.push((r.start..caret, color, bold));
                segs.push((caret + g..r.end + g, color, bold));
            } else {
                segs.push((at(r.start)..at(r.end), color, bold));
            }
        }
        if g > 0 {
            segs.push((caret..caret + g, scene::blend(pal.bg, pal.fg, 0.5), false));
        }
        // The Auto Composer's prompt in its own colors (it sits before the caret, so the ghost
        // never shifts it), the default background left out (`PS:8005-8015`).
        let prompt = if c.auto { prefix_runs(c) } else { Vec::new() };
        segs.extend(prompt.iter().map(|(r, fg, bold, _)| (r.clone(), *fg, *bold)));
        segs.sort_by_key(|(r, _, _)| r.start);
        let content = lines.len() as f32 * line_h;
        let scroll = v.map_or(0.0, |v| v.scroll).clamp(0.0, (content - l.text.h).max(0.0));
        let y_of = |i: usize| l.text.y + i as f32 * line_h - scroll;
        let shown_lines: Vec<(f32, f32, Vec<(String, Rgb, bool)>)> = lines
            .iter()
            .enumerate()
            .filter(|(i, _)| y_of(*i) + line_h > l.text.y && y_of(*i) < l.text.y + l.text.h)
            .map(|(i, vl)| (l.text.x, y_of(i), runs(&display, vl, &segs, pal.fg)))
            .collect();
        let space = width(" ");
        let mut backs: Vec<(Rect, Rgb)> = Vec::new();
        for (r, _, _, bg) in prompt.iter().filter(|(_, _, _, bg)| *bg != pal.bg) {
            for (i, vl) in lines.iter().enumerate() {
                let (a, b) = (r.start.max(vl.start), r.end.min(vl.end));
                if a < b {
                    let x0 = width(&display[vl.start..a]);
                    let x1 = width(&display[vl.start..b]);
                    backs.push((Rect { x: l.text.x + x0, y: y_of(i), w: x1 - x0, h: line_h }, *bg));
                }
            }
        }
        let mut selection = Vec::new();
        for r in c.edit.carets().into_iter().filter(|r| !r.is_empty()) {
            let r = at(r.start)..at(r.end);
            for (i, vl) in lines.iter().enumerate() {
                if r.end < vl.start || r.start > vl.end || (r.end == vl.start && vl.start != vl.end) {
                    continue;
                }
                let (a, b) = (r.start.max(vl.start), r.end.min(vl.end));
                let x0 = width(&display[vl.start..a]);
                let x1 = width(&display[vl.start..b]) + if r.end > vl.end && lines.get(i + 1).is_some_and(|n| n.start > vl.end) { space } else { 0.0 };
                if x1 > x0 {
                    selection.push(Rect { x: l.text.x + x0, y: y_of(i), w: x1 - x0, h: line_h });
                }
            }
        }
        let has_keys = open && self.focused && self.focused_composer() == Some(pane);
        let caret_rect = has_keys.then(|| {
            let (line, x) = text_edit::caret_pos(&display, &lines, caret, c.edit.at_line_end, &width);
            (Rect { x: l.text.x + x.min((l.text.w - 1.0).max(0.0)), y: y_of(line), w: scale.max(1.0), h: line_h }, pal.cursor)
        });
        let ai = self.globals.bool("GenerativeAIAllowed");
        let tip_w = composer::SEND_TIP.chars().count() as f32 * cw * SMALL;
        let auto = c.auto;
        let warn = crate::ai_complete::warning_shows(self.globals.bool(crate::ai_settings::AI_COMPLETIONS), ai);
        let warning = (!auto && warn).then(|| (l.ai.x + l.ai.w + 4.0 * scale, l.accessory.y + l.accessory.h - 6.0 * scale - 14.0 * scale + ((14.0 * scale - line_h * SMALL) / 2.0), composer::AI_WARNING));
        let tip = (!auto && composer::send_tip_shows(l.accessory.w / scale, warn)).then(|| (l.accessory.x + l.accessory.w - tip_w, l.accessory.y + l.accessory.h - 6.0 * scale - 14.0 * scale + ((14.0 * scale - line_h * SMALL) / 2.0), composer::SEND_TIP));
        let d = scene::ComposerDraw {
            panel: l.panel,
            radius: composer::RADIUS * scale,
            alpha,
            bg: pal.bg,
            under: pal.bg,
            fg: pal.fg,
            clip: l.text,
            lines: &shown_lines,
            selection: &selection,
            caret: caret_rect,
            close: l.close,
            help: l.help,
            ai: (ai && !auto).then_some(l.ai),
            tip,
            warning,
            tooltip: (self.composer_hover == Some(pane) && open && !auto).then_some(composer::HELP_TIP),
            small: SMALL,
            line_h,
            // The Auto Composer's 1 point line above it in the success mark color
            // (`PS:24640-24650`, `MCVC:221-246`).
            separator: auto.then(|| (scene::rgba(scene::mark_color(crate::marks::MarkColor::Success, pal, false), 1.0), scale.max(1.0))),
            backs: &backs,
        };
        out.extend(scene::composer(frame, &d, cell).map(|r| (Overlay::Composer, r)));
        // Extra carets.
        if has_keys {
            for e in &c.edit.extra {
                let (line, x) = text_edit::caret_pos(&display, &lines, at(e.at), false, &width);
                let r = Rect { x: l.text.x + x, y: y_of(line), w: scale.max(1.0), h: line_h };
                if r.y >= l.text.y && r.y + r.h <= l.text.y + l.text.h + 0.5 {
                    frame.popups.push(scene::Quad { rect: r, color: rgba(pal.cursor, alpha) });
                }
            }
        }
        if !open {
            return out;
        }
        // The popup below (or above) the caret.
        let thinking = v.and_then(|v| v.waiting.as_ref()).is_some_and(|f| f.explicit);
        let Some(caret_r) = self.composer_caret_rect(pane, rect) else { return out };
        let caret_at = (caret_r.x, caret_r.y, caret_r.y + caret_r.h);
        if let Some(pop) = v.and_then(|v| v.popup.as_ref()) {
            let rows: Vec<(String, Option<Range<usize>>, char)> = pop
                .items
                .iter()
                .map(|i| {
                    let (t, b) = suggest::row_text(i, &pop.typed);
                    (t, b, glyph(i.kind))
                })
                .collect();
            let detail = pop.items.get(pop.selected).map(|i| i.detail.as_str());
            let d = scene::ComposerPopupDraw { caret: caret_at, bounds: rect, rows: &rows, selected: Some(pop.selected), detail, placeholder: None, dark: self.dark(), scale };
            out.push((Overlay::ComposerPopup, scene::composer_popup(frame, &d, cell)));
        } else if thinking {
            let d = scene::ComposerPopupDraw { caret: caret_at, bounds: rect, rows: &[], selected: None, detail: None, placeholder: Some(suggest::THINKING), dark: self.dark(), scale };
            out.push((Overlay::ComposerPopup, scene::composer_popup(frame, &d, cell)));
        }
        out
    }

    /// The composers' timers: the worker's answers, the rate-limited highlighting (checks
    /// dispatched to the worker, rechecked every 0.1 s while any is pending) and the fades.
    /// When the next is due.
    pub(super) fn tick_composer(&mut self) -> Option<Instant> {
        let now = Instant::now();
        for reply in self.composer_worker.replies() {
            match reply {
                Reply::Files { pane, generation, entries } => self.composer_found(pane, generation, entries),
                Reply::Check { pane, request, ok } => {
                    if let Some(v) = self.composers.get_mut(&pane) {
                        match ok {
                            Some(ok) => v.checker.answer(request, ok, now),
                            None => v.checker.lost(request),
                        }
                        v.highlight_due = Some(v.highlight_due.map_or(now, |d| d.min(now)));
                    }
                }
            }
        }
        let mut next: Option<Instant> = None;
        let mut soonest = |t: Instant| next = Some(next.map_or(t, |n: Instant| n.min(t)));
        let ids: Vec<PaneId> = self.composers.keys().copied().collect();
        let mut redraw = false;
        for id in ids {
            let Some(p) = self.panes.get(&id) else {
                self.composers.remove(&id);
                continue;
            };
            let open = p.composer.shown == Shown::Dropdown && !p.composer.hidden;
            let Some((opened, closed, due)) = self.composers.get(&id).map(|v| (v.opened, v.closed, v.highlight_due)) else { continue };
            if open && opened.is_some_and(|t| now.duration_since(t).as_secs_f32() < composer::FADE_IN) {
                soonest(now + FRAME);
                redraw = true;
            }
            if !open && closed.is_some_and(|t| now.duration_since(t).as_secs_f32() < composer::FADE_OUT + 0.02) {
                soonest(now + FRAME);
                redraw = true;
            }
            match due {
                Some(due) if open && due <= now => {
                    self.composer_highlight(id, now);
                    redraw = true;
                    if let Some(d) = self.composers.get(&id).and_then(|v| v.highlight_due) {
                        soonest(d);
                    }
                }
                Some(due) if open => soonest(due),
                _ => {}
            }
        }
        if redraw {
            self.redraw();
        }
        next
    }

    /// Highlights `pane`'s text (`SH:94-143`): builtins, cmdlets and aliases are commands (the
    /// controller ruling), everything else asks the checker; its new requests go to the worker,
    /// each ending in exactly one answer or `Checker::lost`.
    fn composer_highlight(&mut self, pane: PaneId, now: Instant) {
        let Some(p) = self.panes.get(&pane) else { return };
        let c = &p.composer;
        let text = c.text().to_string();
        let prefix = &text[..text.len() - c.contents().len()];
        let start = prefix.encode_utf16().count();
        let end = text.encode_utf16().count();
        let pal = &p.screen.palette;
        let colors = command_syntax::colors(pal.bg, pal.fg, &pal.ansi);
        let cwd = p.own().shell.cwd.as_deref().and_then(crate::profile::local_path);
        let Some(v) = self.composers.get_mut(&pane) else { return };
        let pending = Cell::new(false);
        let checker = RefCell::new(&mut v.checker);
        let command_ok = |cmd: &str| {
            if command_syntax::known_shell_command(cmd) {
                return Check::Yes;
            }
            let r = checker.borrow_mut().command(cmd, now);
            if r == Check::Pending {
                pending.set(true);
            }
            r
        };
        let file_exists = |f: &str| {
            let r = checker.borrow_mut().file(f);
            if r == Check::Pending {
                pending.set(true);
            }
            r
        };
        let spans = command_syntax::highlight(&text, start..end, &colors, &command_ok, &file_exists);
        let requests = checker.borrow_mut().requests();
        drop(checker);
        v.spans = utf16_to_bytes(&text, &spans);
        v.highlighted = Some(now);
        v.highlight_due = pending.get().then(|| now + command_syntax::RECHECK);
        for request in requests {
            let job = Job::Check { pane, request, cwd: cwd.clone() };
            if let Err(Job::Check { request, .. }) = self.composer_worker.send(job) {
                if let Some(v) = self.composers.get_mut(&pane) {
                    v.checker.lost(request);
                }
            }
        }
    }

    /// Opens what the composer asked for on the last pass: the Help popover, the explainshell
    /// question, the command history, Advanced Paste.
    pub(super) fn open_composer_windows(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(pane) = self.composer_windows.advanced_paste.take() {
            self.open_advanced_paste(event_loop, pane, None);
        }
        if let Some((pane, prefix, chronological)) = self.composer_windows.history.take() {
            self.open_command_history_with(event_loop, pane, &prefix, chronological, true);
        }
        if let Some(at) = self.composer_windows.help_at.take() {
            if self.composer_windows.help.take().is_none() {
                let ai = self.globals.bool("GenerativeAIAllowed");
                let lines: Vec<(String, String)> = composer::help_lines(ai).into_iter().map(|(k, d)| (k.to_string(), d.to_string())).collect();
                let dark = self.dark();
                let work = self.monitor_rects().map(|(w, _)| w);
                if let Some(g) = self.gpu.as_ref() {
                    match crate::status_popover_ui::InfoPopover::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, lines, at, work, dark) {
                        Ok(w) => self.composer_windows.help = Some(w),
                        Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
                    }
                }
            }
        }
        if let Some((pane, url, browser)) = self.composer_windows.explain_ask.take() {
            let text = composer::explain_text(&url, &browser);
            if let Some(g) = self.gpu.as_ref() {
                match crate::prompt_ui::ConfirmWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, composer::EXPLAIN_HEADING, &text) {
                    Ok(w) => self.composer_windows.explain = Some((w, pane, url)),
                    Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
                }
            }
        }
    }

    /// Events for the Help popover and the explainshell question. False for other windows.
    pub(super) fn composer_window_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        use crate::prompt_ui::ConfirmResult;
        if let Some(h) = self.composer_windows.help.as_mut().filter(|h| h.id() == window) {
            if !h.event(event) {
                self.composer_windows.help = None;
            }
            return true;
        }
        let Some((w, pane, url)) = self.composer_windows.explain.as_mut().filter(|(w, _, _)| w.id() == window) else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => ConfirmResult::Cancel,
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                ConfirmResult::Open
            }
        };
        let (pane, url) = (*pane, url.clone());
        match answer {
            ConfirmResult::Open => {}
            // Only an `actions` window answers Action; this one never does.
            ConfirmResult::Cancel | ConfirmResult::Action { .. } => self.composer_windows.explain = None,
            ConfirmResult::Ok { suppress } => {
                self.composer_windows.explain = None;
                // Remembered only on OK (`iTermWarning.m:517-520`).
                if suppress {
                    self.globals.set(composer::EXPLAIN_KEY, toml::Value::Boolean(true));
                    self.save_globals();
                }
                self.open_url(pane, &url);
            }
        }
        true
    }
}

/// Line ends as Enter: the composer's "\n" become "\r", as a typed Enter and a paste send
/// them (ConPTY reads a bare line feed as Ctrl+Enter, not Enter).
fn line_ends(s: &str) -> Vec<u8> {
    s.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
}

/// The https handler's friendly name (`AssocQueryStringW`, `ASSOCSTR_FRIENDLYAPPNAME`), for
/// "This will open ... in <browser>." (`CE:88-104`); None when there is no handler.
fn browser_name() -> Option<String> {
    use windows_sys::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_FRIENDLYAPPNAME, AssocQueryStringW};
    let assoc: Vec<u16> = "https\0".encode_utf16().collect();
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let mut buf = vec![0u16; 512];
    let mut len = buf.len() as u32;
    let hr = unsafe { AssocQueryStringW(ASSOCF_NONE as _, ASSOCSTR_FRIENDLYAPPNAME, assoc.as_ptr(), verb.as_ptr(), buf.as_mut_ptr(), &mut len) };
    if hr != 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let name = String::from_utf16_lossy(&buf[..end]);
    (!name.trim().is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_ai_reply_is_dropped() {
        assert!(ai_reply_current(Some(3), 3));
        assert!(!ai_reply_current(Some(4), 3));
        assert!(!ai_reply_current(None, 3));
    }

    fn m(ctrl: bool, alt: bool, shift: bool, win: bool) -> CMods {
        CMods { ctrl, alt, shift, win }
    }

    #[test]
    fn composer_keys_stay_in_the_composer() {
        let none = m(false, false, false, false);
        let text = |s: &str| CKey::Edit(EditKey::Text(s.to_string()));
        // Printable keys, Enter, Tab, Esc and Backspace.
        assert!(composer_takes(none, &text("a"), false));
        assert!(composer_takes(m(false, false, true, false), &text("A"), false));
        assert!(composer_takes(none, &CKey::Edit(EditKey::Enter), false));
        assert!(composer_takes(none, &CKey::Edit(EditKey::Tab), false));
        assert!(composer_takes(none, &CKey::Esc, false));
        assert!(composer_takes(none, &CKey::Edit(EditKey::Backspace), false));
        assert!(composer_takes(none, &CKey::Edit(EditKey::Left), false));
        // The Ctrl edit chords and moves.
        let ctrl = m(true, false, false, false);
        for c in ['a', 'c', 'x', 'v', 'z', 'y'] {
            assert!(composer_takes(ctrl, &CKey::Ctrl(c), false), "Ctrl+{c}");
        }
        assert!(composer_takes(m(true, false, true, false), &CKey::Ctrl('z'), false));
        assert!(composer_takes(ctrl, &CKey::Edit(EditKey::Left), false));
        assert!(composer_takes(ctrl, &CKey::Edit(EditKey::Backspace), false));
        // Ctrl+Shift+Up adds a caret.
        assert!(composer_takes(m(true, false, true, false), &CKey::Edit(EditKey::Up), false));
        // Other Ctrl chords are not the editor's (Ctrl+T, Ctrl+Enter, Ctrl+Tab).
        assert!(!composer_takes(ctrl, &CKey::Ctrl('t'), false));
        assert!(!composer_takes(ctrl, &CKey::Edit(EditKey::Enter), false));
        assert!(!composer_takes(ctrl, &CKey::Edit(EditKey::Tab), false));
        // The Auto Composer's own Ctrl chords.
        for c in ['c', 'd', 'l', 'r', 'u'] {
            assert!(composer_takes(ctrl, &CKey::Ctrl(c), true), "auto Ctrl+{c}");
        }
        // A Win chord is not; an Alt+letter is not; Alt+Enter and Alt+Shift+Enter are.
        assert!(!composer_takes(m(false, false, false, true), &text("c"), false));
        assert!(!composer_takes(m(false, false, false, true), &CKey::Edit(EditKey::Enter), false));
        assert!(!composer_takes(m(false, true, false, false), &text("x"), false));
        assert!(!composer_takes(m(false, true, false, false), &CKey::Edit(EditKey::Left), false));
        assert!(composer_takes(m(false, true, false, false), &CKey::Edit(EditKey::Enter), false));
        assert!(composer_takes(m(false, true, true, false), &CKey::Edit(EditKey::Enter), false));
        // AltGr typing is typing.
        assert!(composer_takes(m(true, true, false, false), &text("@"), false));
    }

    /// Final review 4: covering a screen of dotted underline is one pass, not a `remove` per quad.
    #[test]
    fn skip_covered_is_linear_in_the_quads() {
        let q = |y: f32| scene::Quad { rect: Rect { x: 0.0, y, w: 1.0, h: 1.0 }, color: [1.0; 4] };
        let mut underlines: Vec<scene::Quad> = (0..120_000).map(|i| q(if i % 10 == 0 { 500.0 } else { 45.0 })).collect();
        let (mut overlay, mut strikes) = (Vec::new(), Vec::new());
        let t = std::time::Instant::now();
        skip_covered(&mut overlay, 0, &mut underlines, 0, &mut strikes, 0, Rect { x: 0.0, y: 40.0, w: 100.0, h: 30.0 });
        assert_eq!(underlines.len(), 12_000);
        assert!(t.elapsed() < std::time::Duration::from_millis(300), "took {:?}", t.elapsed());
    }

    #[test]
    fn auto_composer_covers_its_rows() {
        // Underlines inside the covered rows go; the dimming over the whole pane and anything
        // outside the rows stay; nothing drawn before `from` is touched.
        let q = |y: f32, h: f32| scene::Quad { rect: Rect { x: 0.0, y, w: 10.0, h }, color: [1.0; 4] };
        let mut overlay = vec![q(45.0, 1.0), q(45.0, 1.0), q(0.0, 200.0), q(10.0, 1.0), q(69.0, 1.0)];
        let mut underlines = vec![q(45.0, 1.0), q(45.0, 1.0), q(10.0, 1.0), q(69.0, 1.0)];
        let mut strikes = vec![q(45.0, 1.0), q(10.0, 1.0)];
        skip_covered(&mut overlay, 1, &mut underlines, 1, &mut strikes, 0, Rect { x: 0.0, y: 40.0, w: 100.0, h: 30.0 });
        assert_eq!(strikes.iter().map(|o| o.rect.y).collect::<Vec<_>>(), vec![10.0], "strikethrough under the composer goes too");
        let ys: Vec<(f32, f32)> = overlay.iter().map(|o| (o.rect.y, o.rect.h)).collect();
        assert_eq!(ys, vec![(45.0, 1.0), (0.0, 200.0), (10.0, 1.0)]);
        // `underlines_skip_an_auto_composer_cover`: the same rule for the strokes.
        let ys: Vec<f32> = underlines.iter().map(|o| o.rect.y).collect();
        assert_eq!(ys, vec![45.0, 10.0]);

        // The prompt prefix's colors map onto the editor's text, line by line.
        use crate::palette::rgb;
        let (fg, bg, red) = (rgb(1, 1, 1), rgb(0, 0, 0), rgb(9, 0, 0));
        let run = |t: &str, c: Rgb| crate::styled_copy::Run { text: t.into(), fg: c, bg, bold: false, italic: false, underline: false, link: None };
        let rows = vec![crate::styled_copy::Row { runs: vec![run("ab", red)], hard_end: true }, crate::styled_copy::Row { runs: vec![run("$", fg)], hard_end: true }];
        let mut c = composer::Composer::default();
        c.set_prefix(Some(composer::auto_prefix(rows, vec!["ab".into(), "$".into()], fg, bg)));
        c.set_string("ls", true);
        let runs: Vec<(Range<usize>, Rgb)> = prefix_runs(&c).into_iter().map(|(r, f, _, _)| (r, f)).collect();
        assert_eq!(c.text(), "ab\n$ ls");
        assert_eq!(runs, vec![(0..2, red), (3..4, fg), (4..5, fg)]);
    }

    #[test]
    fn a_press_on_a_composer_focuses_its_pane() {
        // Another pane's composer: focus moves to it; its own pane: nothing to do.
        assert_eq!(press_focus(true, 2, Some(1)), Some(2));
        assert_eq!(press_focus(true, 2, Some(2)), None);
        assert_eq!(press_focus(true, 2, None), Some(2));
        // Only a left press.
        assert_eq!(press_focus(false, 2, Some(1)), None);
    }

    #[test]
    fn only_the_windows_other_panes_let_the_keyboard_go() {
        // A tab or pane change inside the window: the old pane's composer lets go.
        assert_eq!(composers_to_release(&[(1, true), (2, false)], Some(2)), vec![1]);
        // The focused pane's composer keeps it.
        assert_eq!(composers_to_release(&[(1, true), (2, false)], Some(1)), Vec::<PaneId>::new());
        // Another window's panes are never passed in, so switching windows releases nothing
        // there; nothing held, nothing released.
        assert_eq!(composers_to_release(&[(3, false)], Some(3)), Vec::<PaneId>::new());
    }

    #[test]
    fn composer_keys_decode_to_what_the_model_reads() {
        let none = m(false, false, false, false);
        let ctrl = m(true, false, false, false);
        let ch = |c: &str| WKey::Character(c.into());
        // Ctrl+letter stays a Ctrl chord (the auto-mode chords look at it first).
        assert_eq!(decode(&ch("l"), Some("\u{c}"), ctrl).map(|(k, _)| k), Some(CKey::Ctrl('l')));
        // Named keys are raw; the action applies Ctrl's meaning.
        assert_eq!(decode(&WKey::Named(NamedKey::ArrowLeft), None, ctrl).map(|(k, _)| k), Some(CKey::Edit(EditKey::Left)));
        assert_eq!(decode(&WKey::Named(NamedKey::Escape), None, none).map(|(k, _)| k), Some(CKey::Esc));
        // The Windows clipboard keys, taken as plain edit keys.
        let (k, km) = decode(&WKey::Named(NamedKey::Insert), None, m(false, false, true, false)).unwrap();
        assert_eq!(k, CKey::Edit(EditKey::Paste(String::new())));
        assert!(composer_takes(km, &k, false));
        let (k, km) = decode(&WKey::Named(NamedKey::Insert), None, ctrl).unwrap();
        assert_eq!(k, CKey::Edit(EditKey::Copy));
        assert!(composer_takes(km, &k, false));
        // Typing.
        assert_eq!(decode(&ch("a"), Some("a"), none).map(|(k, _)| k), Some(CKey::Edit(EditKey::Text("a".into()))));
        assert!(decode(&WKey::Named(NamedKey::F5), None, none).is_none());
        assert_eq!(line_ends("a\nb\r\n"), b"a\rb\r".to_vec());
    }

    #[test]
    fn an_auto_composer_takes_the_keyboard_back_with_the_terminal() {
        // A press on the terminal after a drag-select, the pane becoming the focused one, or
        // find, a tool or the completion list letting go: the Auto Composer takes it.
        assert!(auto_takes_keys_back(true, true, false, false, false));
        // Nothing happened to the terminal's keyboard: no change.
        assert!(!auto_takes_keys_back(false, true, false, false, false));
        // No Auto Composer, or it is hidden by Copy Mode, or it has the keyboard already.
        assert!(!auto_takes_keys_back(true, false, false, false, false));
        assert!(!auto_takes_keys_back(true, true, true, false, false));
        assert!(!auto_takes_keys_back(true, true, false, true, false));
        // Another consumer still has it (Copy Mode, a field, find typing, a tool).
        assert!(!auto_takes_keys_back(true, true, false, false, true));
    }

    #[test]
    fn a_status_bar_field_with_the_keyboard_is_not_the_terminal() {
        // The dropdown has it, or nothing else does: the terminal or composer is first
        // responder, so a prompt's reveal may take the keyboard.
        assert!(terminal_or_dropdown_has_keys(true, false));
        assert!(terminal_or_dropdown_has_keys(false, false));
        // The layout's field (or a tool, find typing, Copy Mode) has it: not the terminal, so
        // a reveal leaves the keyboard where it is.
        assert!(!terminal_or_dropdown_has_keys(false, true));
    }

    /// Every place that drops a pending suggestion answer also cancels the request itself
    /// (ruling P12, Task 5 review minor 1): a closed Composer, a caret move, a click, an Engage
    /// replace and a reopen. The App has no test harness here, so the source is checked.
    #[test]
    fn dropping_an_answer_cancels_its_request() {
        let src = include_str!("composer_ui.rs");
        let body = |name: &str| {
            let at = src.find(&format!("fn {name}(")).unwrap_or_else(|| panic!("no {name}"));
            let rest = &src[at..];
            let end = rest[1..].find("\n    fn ").or_else(|| rest[1..].find("\n    pub(super) fn ")).map_or(rest.len(), |e| e + 1);
            rest[..end].to_string()
        };
        for name in ["composer_closed", "composer_opened", "composer_ai_replace"] {
            assert!(body(name).contains("composer_ai_cancel(pane)"), "{name}");
        }
        assert!(src.matches("self.composer_ai_cancel(pane);").count() >= 7, "caret move and press cancel too");
    }
}
