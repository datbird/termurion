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
use std::sync::mpsc;
use std::time::Duration;

use unicode_width::UnicodeWidthChar;
use winit::keyboard::{Key as WKey, NamedKey};

use super::*;
use super::toolbelt_tools_ui::{is_modifier, now_ms};
use crate::broadcast::Write;
use crate::command_syntax::{self, Check, Checker, Request};
use crate::composer::{self, Act, Key as CKey, Mods as CMods, Shown};
use crate::composer_suggest::{self as suggest, Item, ItemKind};
use crate::palette::Rgb;
use crate::text_edit::{self, EditKey, EditOut, VisualLine};

/// Most entries read from one directory for a listing (a huge folder or a long PATH never
/// stalls the worker for long, and the popup shows at most 256 anyway).
const MAX_DIR_ENTRIES: usize = 4096;
/// How long a PATH directory's listing is reused before it is read again.
const PATH_TTL: Duration = Duration::from_secs(30);
/// A fade's frame interval.
const FRAME: Duration = Duration::from_millis(16);
/// The info bar the AI button and Win+Y show.
const AI_BAR: &str = "composer-ai";
/// The profile key of the dropdown's distance from the pane's top (`ITAddressBookMgr.h:384`),
/// undeclared, read with its default 0.
const TOP_OFFSET: &str = "Composer Top Offset";
/// The global dropdown height (`MCVC:141-142`), drag-only.
const HEIGHT_KEY: &str = "ComposerHeight";
/// The accessory row's text against the pane's font.
const SMALL: f32 = 0.8;
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

/// A line as drawn: tabs as 4 spaces (as `text_width` measures them).
fn shown(s: &str) -> String {
    s.replace('\t', "    ")
}

/// `a` at `alpha` over `b`.
fn blend(a: Rgb, b: Rgb, alpha: f32) -> Rgb {
    let m = |x: u8, y: u8| (f32::from(x) * alpha + f32::from(y) * (1.0 - alpha)).round().clamp(0.0, 255.0) as u8;
    crate::palette::rgb(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

fn glyph(kind: ItemKind) -> char {
    match kind {
        ItemKind::History => HISTORY_GLYPH,
        ItemKind::File => FILE_GLYPH,
        ItemKind::Folder => FOLDER_GLYPH,
        ItemKind::Command => COMMAND_GLYPH,
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

/// A completion popup: its items, the typed line its rows show in bold, and the selected row.
struct Popup {
    items: Vec<Item>,
    typed: String,
    selected: usize,
}

/// A suggestion fetch waiting on the worker's file listing.
struct Fetch {
    generation: u64,
    explicit: bool,
    line: String,
    name: String,
    history: Vec<Item>,
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

enum Job {
    Files { pane: PaneId, generation: u64, dirs: Vec<(PathBuf, bool)>, pathext: Vec<String> },
    Check { pane: PaneId, request: Request, cwd: Option<PathBuf> },
}

enum Reply {
    Files { pane: PaneId, generation: u64, entries: Vec<(String, bool)> },
    /// None: the worker failed on it (it will never be answered).
    Check { pane: PaneId, request: Request, ok: Option<bool> },
}

/// The composer's worker thread: file and PATH listings and the command and file checks, off
/// the UI thread (as the git poller runs git). Started on first use; a thread that ended is
/// started again on the next job.
pub(super) struct Worker {
    notify: Notify,
    link: Option<(mpsc::Sender<Job>, mpsc::Receiver<Reply>)>,
}

impl Worker {
    pub(super) fn new(notify: Notify) -> Worker {
        Worker { notify, link: None }
    }

    /// Hands a job to the thread; the job back when no thread takes it.
    fn send(&mut self, job: Job) -> Result<(), Job> {
        if self.link.is_none() {
            let (jtx, jrx) = mpsc::channel::<Job>();
            let (rtx, rrx) = mpsc::channel::<Reply>();
            let notify = self.notify.clone();
            let started = std::thread::Builder::new().name("composer".into()).spawn(move || {
                let mut cache: HashMap<PathBuf, (Instant, Vec<(String, bool)>)> = HashMap::new();
                for job in jrx {
                    let reply = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_job(&job, &mut cache))) {
                        Ok(r) => r,
                        Err(_) => failed(&job),
                    };
                    if rtx.send(reply).is_err() {
                        break;
                    }
                    notify();
                }
            });
            if started.is_err() {
                return Err(job);
            }
            self.link = Some((jtx, rrx));
        }
        let Some((tx, _)) = &self.link else { return Err(job) };
        tx.send(job).map_err(|e| {
            self.link = None;
            e.0
        })
    }

    fn replies(&mut self) -> Vec<Reply> {
        let Some((_, rx)) = &self.link else { return Vec::new() };
        let mut out = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(r) => out.push(r),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.link = None;
                    break;
                }
            }
        }
        out
    }
}

/// The reply for a job the worker could not finish.
fn failed(job: &Job) -> Reply {
    match job {
        Job::Files { pane, generation, .. } => Reply::Files { pane: *pane, generation: *generation, entries: Vec::new() },
        Job::Check { pane, request, .. } => Reply::Check { pane: *pane, request: request.clone(), ok: None },
    }
}

fn env_list(name: &str) -> Vec<String> {
    std::env::var(name).unwrap_or_default().split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

fn home() -> String {
    std::env::var("USERPROFILE").unwrap_or_default()
}

/// A directory's entries, at most `MAX_DIR_ENTRIES` (name, is a directory). Never a UNC path.
fn list_dir(dir: &Path) -> Vec<(String, bool)> {
    if crate::prompted::network_path(&dir.to_string_lossy()) {
        return Vec::new();
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten().take(MAX_DIR_ENTRIES).map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().is_ok_and(|t| t.is_dir()))).collect()
}

/// One job on the worker. Listings: a PATH directory (`true`) gives its executables (PATHEXT
/// files, extension dropped) from a cache kept `PATH_TTL`; any other directory everything.
/// Checks: a command is valid when a PATH/PATHEXT candidate is a file (`FC:195-242`), a file
/// argument when it exists (relative to the session's directory). Nothing UNC is touched, and
/// nothing is run.
fn run_job(job: &Job, cache: &mut HashMap<PathBuf, (Instant, Vec<(String, bool)>)>) -> Reply {
    match job {
        Job::Files { pane, generation, dirs, pathext } => {
            let mut entries: Vec<(String, bool)> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for (dir, on_path) in dirs {
                let listed = if *on_path {
                    let fresh = cache.get(dir).filter(|(t, _)| t.elapsed() < PATH_TTL).map(|(_, v)| v.clone());
                    let v = fresh.unwrap_or_else(|| {
                        let v: Vec<(String, bool)> = list_dir(dir)
                            .into_iter()
                            .filter(|(_, is_dir)| !is_dir)
                            .filter_map(|(name, _)| {
                                let lower = name.to_lowercase();
                                pathext.iter().find(|e| lower.ends_with(&e.to_lowercase())).map(|e| (name[..name.len() - e.len()].to_string(), false))
                            })
                            .collect();
                        cache.insert(dir.clone(), (Instant::now(), v.clone()));
                        v
                    });
                    v
                } else {
                    list_dir(dir)
                };
                for (name, is_dir) in listed {
                    if !name.is_empty() && seen.insert(name.to_lowercase()) {
                        entries.push((name, is_dir));
                    }
                }
            }
            Reply::Files { pane: *pane, generation: *generation, entries }
        }
        Job::Check { pane, request, cwd } => {
            let ok = match request {
                Request::Command(cmd) => {
                    let candidates = command_syntax::command_candidates(cmd, &env_list("PATH"), &env_list("PATHEXT"), &home());
                    candidates.iter().filter(|c| !crate::prompted::network_path(c)).any(|c| Path::new(c).is_file())
                }
                Request::File(arg) => {
                    let p = if arg == "~" {
                        Some(PathBuf::from(home()))
                    } else if let Some(rest) = arg.strip_prefix("~/").or_else(|| arg.strip_prefix("~\\")) {
                        Some(Path::new(&home()).join(rest))
                    } else if Path::new(arg).is_absolute() || arg.starts_with('\\') || arg.starts_with('/') {
                        Some(PathBuf::from(arg))
                    } else {
                        cwd.as_ref().map(|d| d.join(arg))
                    };
                    p.is_some_and(|p| !crate::prompted::network_path(&p.to_string_lossy()) && p.exists())
                }
            };
            Reply::Check { pane: *pane, request: request.clone(), ok: Some(ok) }
        }
    }
}

impl App {
    /// The pane whose shown dropdown has the keyboard: the focused pane's, while no toolbelt
    /// tool has it.
    fn focused_composer(&self) -> Option<PaneId> {
        let id = self.focused_id()?;
        let c = &self.panes.get(&id)?.composer;
        (c.shown == Shown::Dropdown && c.focused && !c.hidden && self.belt.focus.is_none()).then_some(id)
    }

    /// A composer has the keyboard (the keyboard hook takes its Win chords then).
    pub(super) fn composer_has_keys(&self) -> bool {
        self.focused_composer().is_some()
    }

    /// Another key consumer takes the keyboard (find typing, Copy Mode, the autocomplete list,
    /// the shortcut labels, a toolbelt tool): no composer keeps it; their popups close.
    pub(super) fn composer_yield_keys(&mut self) {
        let mut any = false;
        for (id, p) in self.panes.iter_mut() {
            if p.composer.focused {
                p.composer.focused = false;
                any = true;
                if let Some(v) = self.composers.get_mut(id) {
                    v.popup = None;
                    v.ghost = None;
                }
            }
        }
        if any {
            self.refresh_route();
            self.redraw();
        }
    }

    /// `pane`'s composer takes the keyboard: every other key consumer lets it go.
    fn composer_take_keys(&mut self, pane: PaneId) {
        self.belt_yield_keys();
        if let Some(p) = self.panes.get_mut(&pane) {
            p.composer.focused = true;
        }
        self.refresh_route();
    }

    /// The global dropdown height (`ComposerHeight`, 135).
    fn composer_height(&self) -> f32 {
        self.globals.get(HEIGHT_KEY).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).map_or(composer::DEFAULT_HEIGHT, |f| f as f32)
    }

    /// The profile's `Composer Top Offset` (a session copy reads its saved profile, which is
    /// where a drag writes it).
    fn composer_top_offset(&self, pane: PaneId) -> f32 {
        let Some(p) = self.panes.get(&pane) else { return 0.0 };
        let guid = self.store.shared_guid(&p.profile);
        self.store.get(&guid).or_else(|| self.store.get(&p.profile)).and_then(|pr| pr.get(TOP_OFFSET)).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).map_or(0.0, |f| f as f32)
    }

    /// The dropdown's parts over pane rectangle `rect` (pixels): `minimal_rect` in points.
    fn composer_layout(&self, pane: PaneId, rect: Rect) -> composer::Layout {
        let s = self.scale();
        let pts = Rect { x: rect.x / s, y: rect.y / s, w: rect.w / s, h: rect.h / s };
        let h = composer::clamp_height(self.composer_height(), pts.h);
        let r = composer::minimal_rect(pts, h, self.composer_top_offset(pane));
        composer::layout(Rect { x: r.x * s, y: r.y * s, w: r.w * s, h: r.h * s }, s)
    }

    /// The pane's dropdown layout and its text's visual lines (no ghost), when it shows.
    fn composer_lines(&self, pane: PaneId) -> Option<(composer::Layout, Vec<VisualLine>)> {
        let rect = self.pane_rect(pane)?;
        let l = self.composer_layout(pane, rect);
        let cw = self.cell().0;
        let lines = text_edit::wrap(&self.panes.get(&pane)?.composer.edit.text, l.text.w, &|s: &str| text_width(s, cw));
        Some((l, lines))
    }

    /// View > Show Composer for `pane` (`CM:146-158`, `PS:8655-8661`): the dropdown toggles,
    /// taking the keyboard when it opens. (The status bar route, the Auto Composer and the
    /// current command's clear sequence come with Tasks 7 and 8.)
    pub(super) fn compose(&mut self, pane: PaneId) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let before = p.composer.shown;
        let after = p.composer.toggle(false);
        if after == Shown::Dropdown {
            self.composer_opened(pane);
            self.composer_take_keys(pane);
        } else if before == Shown::Dropdown {
            self.composer_closed(pane);
        }
        self.redraw();
    }

    /// The dropdown appeared: it fades in and is highlighted.
    fn composer_opened(&mut self, pane: PaneId) {
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
        let Some(pane) = self.focused_composer() else {
            self.composer_keys.press(ev.physical_key, false);
            return false;
        };
        let taken = self.composer_key_press(pane, ev);
        self.composer_keys.press(ev.physical_key, taken);
        taken
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
            // A caret move ends the popup (`CTV:266-270`) and the ghost.
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
    fn composer_fetch(&mut self, pane: PaneId, explicit: bool) {
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
        let (first, prefix) = suggest::word_to_complete(&line);
        let (dir_part, name) = match prefix.rfind(['\\', '/']) {
            Some(i) => (&prefix[..=i], &prefix[i + 1..]),
            None => ("", prefix),
        };
        let fetch = Fetch { generation, explicit, line: line.clone(), name: name.to_string(), history };
        if name.is_empty() || crate::prompted::network_path(dir_part) {
            v.waiting = Some(fetch);
            return self.composer_found(pane, generation, Vec::new());
        }
        let base = cwd.clone().unwrap_or_else(|| PathBuf::from(home()));
        let join = |d: &Path| if dir_part.is_empty() { d.to_path_buf() } else { d.join(dir_part) };
        let dirs: Vec<(PathBuf, bool)> = if Path::new(dir_part).is_absolute() {
            vec![(PathBuf::from(dir_part), false)]
        } else if first && dir_part.is_empty() {
            env_list("PATH").into_iter().map(|d| (PathBuf::from(d), true)).collect()
        } else {
            vec![(join(&base), false)]
        };
        v.waiting = Some(fetch);
        let job = Job::Files { pane, generation, dirs, pathext: env_list("PATHEXT") };
        if self.composer_worker.send(job).is_err() {
            self.composer_found(pane, generation, Vec::new());
        }
        self.redraw();
    }

    /// The listing for fetch `generation` arrived (`didFindCompletions:`, `LCVC:745-803`):
    /// files first, then history, deduplicated; the ghost from them (`composer_suggest::ghost`,
    /// suggestion-only while typing); the popup only for Tab.
    fn composer_found(&mut self, pane: PaneId, generation: u64, entries: Vec<(String, bool)>) {
        let Some(p) = self.panes.get(&pane) else { return };
        let c = &p.composer;
        let at_end = c.edit.caret >= c.text().len();
        let shell = p.own().shell.shell.clone();
        let Some(v) = self.composers.get_mut(&pane) else { return };
        if v.waiting.as_ref().is_none_or(|f| f.generation != generation) {
            return;
        }
        let Some(f) = v.waiting.take() else { return };
        let limit = if f.explicit { suggest::FILES_TAB } else { suggest::FILES_TYPING };
        let mut items = suggest::files(&entries, &f.name, limit, shell.as_deref());
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
                // The Auto Composer's Ctrl+L (Task 7).
                Act::ClearScreen => {}
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
            self.composer_closed(pane);
            if let Some(v) = self.composers.get_mut(&pane) {
                v.opened = None;
                v.closed = None;
            }
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
        let Some(pane) = self.focused_composer() else { return false };
        self.composer_copy(pane);
        true
    }

    fn composer_copy(&mut self, pane: PaneId) {
        let Some(p) = self.panes.get(&pane) else { return };
        let has_selection = p.composer.edit.carets().iter().any(|r| !r.is_empty());
        if has_selection {
            self.composer_apply(pane, composer::Action::Edit(EditKey::Copy), false);
        } else if p.composer.auto {
            self.copy_selection(pane);
        }
    }

    /// Paste in a focused composer. True when a composer had the keyboard.
    pub(super) fn composer_paste_menu(&mut self) -> bool {
        let Some(pane) = self.focused_composer() else { return false };
        self.composer_paste(pane);
        true
    }

    fn composer_paste(&mut self, pane: PaneId) {
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
    /// step marks (`PS:24608-24625`), Win+Alt+V sends the text to Advanced Paste, Win+Y answers
    /// that AI comes later, Win+Shift+; opens the command history for the text before the
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
            composer::Chord::Ai => {
                self.ask(pane, AI_BAR, composer::AI_LATER, &["OK"], false);
            }
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
            if let Some(row) = self.composer_popup_hit(pane, rect, x, y) {
                if button == MouseButton::Left {
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
                self.ask(pane, AI_BAR, composer::AI_LATER, &["OK"], false);
            } else if l.handle.contains(x, y) {
                self.drag = Drag::Composer { pane, kind: ComposerDrag::Resize { start_y: y, start_h: l.panel.h / scale } };
            } else if l.text.contains(x, y) {
                self.composer_text_press(pane, x, y);
            } else {
                let offset = (l.panel.y - rect.y) / scale;
                self.drag = Drag::Composer { pane, kind: ComposerDrag::Move { start_y: y, start_offset: offset } };
            }
            self.redraw();
            return true;
        }
        self.composer_yield_keys();
        false
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
        let e = &mut p.composer.edit;
        let kind = if m.ctrl && m.shift && !m.alt {
            e.click_add(at);
            None
        } else if m.alt && !m.ctrl {
            e.set_carets(&[text_edit::Caret { at, anchor: None }]);
            Some(ComposerDrag::Select { from: Some((line, lx)) })
        } else {
            e.click(at, end, m.shift, now_ms(), super::clippings_ui::double_click_ms());
            Some(ComposerDrag::Select { from: None })
        };
        if let Some(kind) = kind {
            self.drag = Drag::Composer { pane, kind };
        }
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

    /// The popup row under a point, when `pane`'s popup shows.
    fn composer_popup_hit(&self, pane: PaneId, rect: Rect, x: f32, y: f32) -> Option<usize> {
        let (caret, rows, selected, detail, widest) = self.composer_popup_geometry(pane, rect)?;
        let scale = self.scale();
        let (r, first) = scene::composer_popup_frame(caret, rect, rows, selected, detail, widest, scale);
        if !r.contains(x, y) {
            return None;
        }
        let k = ((y - r.y - scene::POPUP_SPACING * scale) / ((scene::POPUP_ROW + scene::POPUP_SPACING) * scale)).floor();
        let row = first + k.max(0.0) as usize;
        Some(row.min(rows.saturating_sub(1)))
    }

    /// The popup's caret (x, top, bottom), rows, selection, whether it has a detail strip,
    /// and its widest row (the same numbers `composer_popup` draws with).
    fn composer_popup_geometry(&self, pane: PaneId, rect: Rect) -> Option<((f32, f32, f32), usize, Option<usize>, bool, f32)> {
        let v = self.composers.get(&pane)?;
        let pop = v.popup.as_ref()?;
        let caret = self.composer_caret_rect(pane, rect)?;
        let cw = self.cell().0;
        let widest = pop.items.iter().map(|i| (suggest::row_text(i, &pop.typed).0.chars().count() as f32 + 3.0) * cw).fold(0.0, f32::max);
        Some(((caret.x, caret.y, caret.y + caret.h), pop.items.len(), Some(pop.selected), true, widest))
    }

    /// The primary caret's rectangle on screen.
    fn composer_caret_rect(&self, pane: PaneId, rect: Rect) -> Option<Rect> {
        let l = self.composer_layout(pane, rect);
        let (cw, line_h) = self.cell();
        let c = &self.panes.get(&pane)?.composer;
        let lines = text_edit::wrap(&c.edit.text, l.text.w, &|s: &str| text_width(s, cw));
        let (line, x) = c.edit.caret_line(&lines, &|s: &str| text_width(s, cw));
        let scroll = self.composers.get(&pane).map_or(0.0, |v| v.scroll);
        Some(Rect { x: l.text.x + x, y: l.text.y + line as f32 * line_h - scroll, w: 1.0, h: line_h })
    }

    /// Draws `pane`'s dropdown over pane rectangle `rect` (`MCVC`): fading in over 0.25 s and
    /// out over 0.125 s, the text in the pane's font and colors with the highlight, the ghost
    /// at half alpha after the caret, the selection as Notes draws it, the carets in the
    /// cursor color while it has the keyboard, the accessory row; then its popup.
    pub(super) fn draw_composer(&self, frame: &mut Frame, pane: PaneId, rect: Rect, cell: (f32, f32)) {
        let Some(p) = self.panes.get(&pane) else { return };
        let c = &p.composer;
        let v = self.composers.get(&pane);
        let now = Instant::now();
        let alpha = if c.shown == Shown::Dropdown && !c.hidden {
            v.and_then(|v| v.opened).map_or(1.0, |t| (now.duration_since(t).as_secs_f32() / composer::FADE_IN).min(1.0))
        } else {
            match v.and_then(|v| v.closed) {
                Some(t) if now.duration_since(t).as_secs_f32() < composer::FADE_OUT => 1.0 - now.duration_since(t).as_secs_f32() / composer::FADE_OUT,
                _ => return,
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
        let lines = text_edit::wrap(&display, l.text.w, &width);
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
            segs.push((caret..caret + g, blend(pal.fg, pal.bg, 0.5), false));
        }
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
        let mut selection = Vec::new();
        let space = width(" ");
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
        let tip = composer::send_tip_shows(l.accessory.w / scale, false).then(|| (l.accessory.x + l.accessory.w - tip_w, l.accessory.y + l.accessory.h - 6.0 * scale - 14.0 * scale + ((14.0 * scale - line_h * SMALL) / 2.0), composer::SEND_TIP));
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
            ai: ai.then_some(l.ai),
            tip,
            small: SMALL,
            line_h,
        };
        scene::composer(frame, &d, cell);
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
            return;
        }
        // The popup below (or above) the caret.
        let thinking = v.and_then(|v| v.waiting.as_ref()).is_some_and(|f| f.explicit);
        let Some(caret_r) = self.composer_caret_rect(pane, rect) else { return };
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
            scene::composer_popup(frame, &d, cell);
        } else if thinking {
            let d = scene::ComposerPopupDraw { caret: caret_at, bounds: rect, rows: &[], selected: None, detail: None, placeholder: Some(suggest::THINKING), dark: self.dark(), scale };
            scene::composer_popup(frame, &d, cell);
        }
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
            ConfirmResult::Cancel => self.composer_windows.explain = None,
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
}
