//! Runs app output through the terminal core and collects the replies the app expects.

use std::sync::mpsc::{self, Receiver, Sender};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{ClearMode, CursorShape, CursorStyle, Handler, Processor};

use crate::mouse::{Modes, Tracking};
use crate::palette::{Palette, Rgb};

#[derive(Clone)]
pub struct Listener(Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

struct GridSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// The profile settings the terminal core needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    /// The profile's Allow Change Cursor Blink: programs may change the blink.
    pub blink_change: bool,
    pub scrollback: usize,
    /// The profile's Allow modifyOtherKeys: programs may change how keys are reported.
    pub app_key_modes: bool,
    /// The profile's Use libtickit protocol: keys start in CSI u mode.
    pub csi_u: bool,
    /// The profile's Blink Allowed: text with the blink attribute hides in the off phase.
    pub blink_allowed: bool,
    /// The profile's Scrollback in Alternate Screen: lines that scroll off the alternate
    /// screen are kept as history.
    pub alt_scrollback: bool,
    /// The profile's Ambiguous Double Width: East Asian ambiguous characters take two cells.
    pub ambiguous_wide: bool,
    /// The profile's "Unicode Version" (8 or 9, `osc1337::profile_unicode`): which width table
    /// applies until an OSC 1337 UnicodeVersion sets one.
    pub unicode_version: u8,
    /// The profile's Allow Title Reporting: CSI 20 t and CSI 21 t answer with the title.
    pub allow_title_reporting: bool,
    /// The profile's Disable Smcup Rmcup: DECSET 47, 1047, 1048 and 1049 do nothing.
    pub disable_smcup: bool,
    /// The profile's Disable Printing: no escape-sequence print starts or is requested.
    pub disable_printing: bool,
    /// The profile's Unicode Normalization (`encoding::Form`).
    pub normalization: crate::encoding::Form,
}

/// How `Term::input` measures characters (milestone 13o): iTerm2's width tables for the
/// session's Unicode version (`version_override` is the one an OSC 1337 UnicodeVersion set,
/// 0 for the profile's), ambiguous width from the profile, and the Advanced switches in `rules`.
pub fn width_rule(look: &Look, rules: &crate::emu::Rules, version_override: u8) -> alacritty_terminal::term::WidthRule {
    alacritty_terminal::term::WidthRule {
        width: rule_width,
        ambiguous_wide: look.ambiguous_wide,
        unicode_version: if version_override == 0 { look.unicode_version } else { version_override },
        full_width_flags: rules.full_width_flags,
        zwsp_advances: rules.zwsp_advances,
        vs16: rules.vs16_all,
        vs16_primary: rules.vs16_primary,
        accepts_vs16: |c| crate::widths::accepts_vs16(c as u32),
        is_modifier: |c| crate::widths::is_skin_tone(c as u32),
        is_pictograph: |c| crate::widths::is_extended_pictographic(c as u32),
    }
}

/// iTerm2's default-ignorable set (`iTermIsIgnorableCharacter`, `CS:792-806`), U+200B aside.
fn is_ignorable(c: char) -> bool {
    matches!(c as u32,
        0xAD | 0x34F | 0x61C | 0x115F..=0x1160 | 0x17B4..=0x17B5 | 0x180B..=0x180F | 0x200C..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0xFFF0..=0xFFF8 | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0000..=0xE0FFF)
}

/// iTerm2's `StringToScreenChars` width of one character: U+200B by its switch, no cell for an
/// ignorable base, zero for a mark or a selector (it rides on its base: iTerm2 measures only the
/// base of a cluster, `enumerateComposedCharacters`), two cells by the tables, else one.
fn rule_width(c: char, rule: &alacritty_terminal::term::WidthRule) -> Option<usize> {
    use unicode_width::UnicodeWidthChar;
    if c == '\u{200B}' {
        return Some(usize::from(rule.zwsp_advances));
    }
    // Marks and selectors are asked before the tables: the ambiguous and full-width tables hold
    // U+0300-036F, U+FE00-FE0F and U+3099-309A, which would otherwise take two cells of their own.
    if c.width() == Some(0) {
        return Some(0);
    }
    // An ignorable character that is not a zero-width mark (the Hangul fillers, the soft hyphen)
    // has no cell at all. (Ignorables of width 0 ride on the previous cell, as before.)
    if is_ignorable(c) {
        return None;
    }
    if crate::widths::is_double_width(c as u32, rule.ambiguous_wide, rule.unicode_version, rule.full_width_flags) {
        return Some(2);
    }
    c.width().map(|w| w.min(1))
}

impl Look {
    pub fn from_profile(p: &crate::profile::Profile) -> Look {
        let cursor_shape = match p.int("Cursor Type") {
            0 => CursorShape::Underline,
            1 => CursorShape::Beam,
            _ => CursorShape::Block,
        };
        // alacritty_terminal keeps at most 100,000 lines; that is Tandem's "unlimited".
        let scrollback = if p.bool("Unlimited Scrollback") { 100_000 } else { p.int("Scrollback Lines").clamp(0, 100_000) as usize };
        Look {
            cursor_shape,
            cursor_blink: p.bool("Blinking Cursor"),
            blink_change: p.bool("Allow Change Cursor Blink"),
            scrollback,
            app_key_modes: p.bool("Allow modifyOtherKeys"),
            csi_u: p.bool("Use libtickit protocol"),
            blink_allowed: p.bool("Blink Allowed"),
            alt_scrollback: p.bool("Scrollback in Alternate Screen"),
            ambiguous_wide: p.bool("Ambiguous Double Width"),
            unicode_version: crate::osc1337::profile_unicode(p.values.get("Unicode Version").and_then(|v| v.as_integer())),
            allow_title_reporting: p.bool("Allow Title Reporting"),
            disable_smcup: p.bool("Disable Smcup Rmcup"),
            disable_printing: p.bool("Disable Printing"),
            normalization: crate::encoding::Form::from_profile(p.int("Unicode Normalization")),
        }
    }
}

impl Default for Look {
    fn default() -> Self {
        Look::from_profile(&crate::profile::Profile::stock())
    }
}

/// Literal Controls: each C0 control byte and DEL as a caret glyph in reverse video. A
/// line feed also starts a new line, so the output stays readable.
fn literal(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        if b < 0x20 || b == 0x7f {
            out.extend_from_slice(b"\x1b[7m^");
            out.push(b ^ 0x40);
            out.extend_from_slice(b"\x1b[27m");
            if b == b'\n' {
                out.extend_from_slice(b"\r\n");
            }
        } else {
            out.push(b);
        }
    }
    out
}

fn config(look: Look) -> Config {
    Config {
        scrolling_history: look.scrollback,
        kitty_keyboard: look.app_key_modes,
        // Tandem applies iTerm2's clipboard policy to OSC 52 itself.
        osc52: alacritty_terminal::term::Osc52::Disabled,
        default_cursor_style: CursorStyle { shape: look.cursor_shape, blinking: look.cursor_blink },
        ..Config::default()
    }
}

/// Caps on what terminal output may make a screen keep.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Bytes of pixels all inline images together may hold.
    pub images: usize,
    /// Bytes a kitty chunked transfer or a MultipartFile may reach.
    pub transfer: usize,
    /// Bytes `CopyToClipboard` captures before it stops.
    pub copy: usize,
    /// Bytes of output held unread behind an inline file that waits for the person (a tmux
    /// pane cannot pause); past it the file is dropped, as a No.
    pub held: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { images: 768 << 20, transfer: 512 << 20, copy: 100 << 20, held: 16 << 20 }
    }
}

/// A cached row order with the row it was made for, compared on every hit.
struct BidiEntry {
    sig: (usize, bool, bool),
    texts: Vec<crate::bidi::CellText>,
    order: Option<std::sync::Arc<crate::bidi::Order>>,
}

/// The most bytes ENQ answers (a profile's Answerback String is cut to this).
pub const ANSWERBACK_MAX: usize = 256;

pub struct Screen {
    pub(crate) term: Term<Listener>,
    /// What the live selection is beyond alacritty's own: its mode, column window and anchor.
    /// It travels with the screen (a Zoom or Filter view swaps both), so a view never reads
    /// another screen's window.
    pub sel: crate::seltext::State,
    parser: Processor,
    events: Receiver<Event>,
    pub palette: Palette,
    /// The profile's Blink Allowed (`Look.blink_allowed`).
    pub blink_allowed: bool,
    /// Right-to-left display rules (the globals `Bidi` and its Advanced siblings); the app sets them.
    pub bidi_rules: crate::bidi::Rules,
    /// Display orders by row content (`row_order`); `pane_with` holds `&Screen` and fills it.
    bidi_cache: std::cell::RefCell<std::collections::HashMap<u64, BidiEntry>>,
    /// When the program hid the cursor (DECTCEM), until it shows it again (Reduce Flicker).
    pub cursor_hidden_at: Option<std::time::Instant>,
    title: Option<String>,
    /// The profile's Allow Title Setting: whether programs may set the title (OSC 0, 1, 2).
    pub allow_title: bool,
    cell_px: (u16, u16),
    osc: crate::osc::Scanner,
    pub marks: crate::marks::Marks,
    /// New prompt marks are line-style (Auto Composer on, or `AlwaysUseLineStyleMarks`); the
    /// app sets it (`S:18934`).
    pub line_style_marks: bool,
    pub shell: crate::marks::Shell,
    /// The profile's Place Prompt at First Column.
    pub place_prompt_first_column: bool,
    /// Output comes through ConPTY, whose own screen must match this one, so the terminal
    /// never moves the cursor on its own (no fresh line for a prompt).
    pub pty_backed: bool,
    /// A fixed clock for row stamps (tests); None uses the time.
    clock: Option<u32>,
    /// Inline images, by id.
    pub images: crate::images::Store,
    /// Files the terminal was sent to save; the app asks before writing them.
    pub downloads: Vec<crate::inline_file::Download>,
    /// A MultipartFile in progress.
    pub multipart: Option<crate::inline_file::Multipart>,
    /// An inline file waiting for the person to allow its display; while it waits, the output
    /// after it is held unread in `held_output` (iTerm2 pauses its parser for the question).
    pub(crate) inline_hold: Option<crate::inline_file::InlineHold>,
    pub(crate) held_output: Vec<u8>,
    /// How many inline files have waited: each one's number, so a late answer to a dropped
    /// one never answers the next.
    pub(crate) inline_holds: u64,
    /// Kitty graphics images and transfers.
    pub kitty: crate::kitty::Kitty,
    /// Answers the terminal owes the program (kitty graphics), sent with `feed`'s reply.
    pub replies: Vec<u8>,
    /// Caps on what output may make Tandem keep.
    pub limits: Limits,
    /// Annotations, by id.
    pub notes: crate::annotations::Notes,
    /// Folded lines, by fold id.
    pub(crate) folds: std::collections::HashMap<u32, Vec<Row<Cell>>>,
    next_fold: u32,
    feeds: u32,
    /// Whether any byte has been fed: iTerm2's tab width applies to a new session only.
    fed: bool,
    /// The Advanced emulation rules this screen follows (`set_rules`).
    pub(crate) rules: crate::emu::Rules,
    /// The profile's Terminal Type (`set_term_name`).
    term_name: String,
    /// Whether OSC 1337 CurrentDir or RemoteHost arrived: iTerm2's per-session
    /// `_sawProprietaryLocationCode`, which keeps the OSC 7 conflict warning quiet.
    pub(crate) saw_location_code: bool,
    /// Moves with every change to the main grid's rows, marks or notes; the scrollbar's tick cache keys on it.
    tick_gen: u64,
    /// `offscreen_facts`' answer for the tick digest it was made at.
    pub(crate) offscreen_cache: Option<(u64, Vec<crate::offscreen::PromptFacts>)>,
    /// Key reporting chosen in the menu (on top of the program's requests).
    pub keys: KeyState,
    /// False when the global disablePotentiallyInsecureEscapeSequences is on.
    pub trusted: bool,
    /// OSC 72 drops: what the program registered for and the drop it may read (13o Task 9).
    pub dnd: crate::kitty_dnd::Controller,
    /// What output asked the window to do, taken by the app.
    pub requests: Vec<Request>,
    /// OSC 1337 `Copy=` in progress, and `CopyToClipboard` capture.
    pub(crate) copy: crate::clipboard_osc::Copy,
    pub(crate) capture: Option<Vec<u8>>,
    /// The start of an escape sequence split across reads during a capture: dropped if it
    /// turns out to be one Tandem acts on, else added to the capture.
    capture_hold: Vec<u8>,
    /// An escape-sequence print job collecting text (`CSI 5 i` to `CSI 4 i`); bounded by
    /// `limits.copy` and by time, ended by RIS, a prompt mark and a command end (B8).
    pub(crate) print_capture: Option<crate::emu::PrintCapture>,
    /// Reports answered in the current `feed` (a batch): at most one answerback, and DECRQCRA
    /// replies up to `DECRQCRA_PER_BATCH`, so a hostile file cannot flood the input queue.
    answerback_sent: bool,
    /// A title report was sent in this batch (one reply per `feed`, like the answerback).
    title_sent: bool,
    decrqcra_sent: u16,
    /// OSC 72 messages that produced a reply in this batch (`DND_ANSWERS_PER_BATCH`).
    pub(crate) dnd_answered: u16,
    /// OSC 1337 Block: the blocks this screen knows.
    pub blocks: crate::blocks::Blocks,
    /// OSC 1337 Button: the buttons written into the text.
    pub buttons: crate::blocks::Buttons,
    /// The profile's triggers and where checking has reached.
    pub triggers: crate::trigger_eval::TriggerState,
    /// Bells rung (iTerm2's bellCount).
    pub bell_count: u64,
    /// What OSC 9;4 last reported (`VT100ScreenProgress`).
    pub progress: crate::progress::Progress,
    /// `Term::resets` as last looked at.
    resets_seen: u64,
    /// When output last arrived (Idle and Activity After Idle event triggers).
    pub last_output: Option<std::time::Instant>,
    /// Variables the window owns, for interpolated strings (`session.name`, `tab.title`).
    pub app_vars: std::collections::HashMap<String, String>,
    /// Portholes by id, and the last id given.
    pub portholes: std::collections::HashMap<u32, crate::porthole::store::Porthole>,
    pub(crate) next_porthole: u32,
    /// Text selected in a porthole: its id and the two ends.
    pub porthole_selection: Option<(u32, crate::porthole::render::Pos, crate::porthole::render::Pos)>,
    /// The global defaultWideMode: portholes start in Wide mode.
    pub porthole_wide_default: bool,
    /// OSC 21337: what the pane's program says it is doing.
    pub tab_status: crate::tabstatus::Status,
    /// When `tab_status` last changed: a tick shared by every screen, so it also orders
    /// changes across panes (Session Status' "older first", its snooze, and the tool's
    /// refresh). 0 until the first change.
    pub tab_status_gen: u64,
    /// The program's `Env=report=all` environment as `(name, value)` tuples in its order,
    /// kept for the SSH integration (iTerm2's `exfiltratedEnvironment`).
    pub env: Option<Vec<(String, String)>>,
    /// `it2ssh=`'s flags, until a `SendConductor=` takes them (iTerm2's `_sshIntegrationFlags`).
    pub(crate) ssh_flags: Option<Vec<String>>,
    /// ExecFailed: the command could not start.
    pub exec_failed: bool,
    /// UnicodeVersion and the Touch Bar key labels.
    pub unicode: crate::osc1337::Unicode,
    /// The profile's "Unicode Version" (`osc1337::profile_unicode`), the session's version
    /// until a UnicodeVersion sequence sets one.
    pub profile_unicode: u8,
    /// The profile's Unicode Normalization Form: applied where text leaves the grid (drawing,
    /// copy, find, `line_text`, the API), never stored in it.
    pub normalization: crate::encoding::Form,
    /// The profile settings last handed to `set_look`; the width rule reads them.
    look: Look,
    /// The profile's Answerback String, what ENQ answers (at most `ANSWERBACK_MAX` bytes).
    answerback: String,
    /// The code page the answerback leaves in (0 is UTF-8).
    answerback_cp: u32,
    pub key_labels: crate::osc1337::KeyLabels,
    /// SetColors changes, reapplied over a new profile palette.
    pub(crate) color_sets: Vec<(String, crate::osc1337::Color)>,
    /// Whether programs may write the clipboard (for the Capabilities report).
    pub clipboard_allowed: bool,
    /// What a host change does with modes left on (the app's settings for this pane).
    pub host_policy: crate::marks::HostPolicy,
    /// Mode 2031 (color scheme reports) and the scheme last reported.
    color_reports: bool,
    last_dark: bool,
    /// The Auto Composer's prompt state machine (milestone 9l), fed by OSC 133, the grid's
    /// echo, and the alternate screen.
    pub prompt_state: crate::prompt_state::PromptState,
    /// What the machine asked of the app, taken by it.
    pub(crate) composer_events: Vec<crate::prompt_state::Event>,
    /// A Python API connection watches this screen's prompts or custom escape sequences:
    /// `api_events` fills only then (screens outside panes never drain it).
    pub api_watch: bool,
    /// A chat streams this session's finished commands: `Request::CommandEnded` is queued at
    /// each command's end only then (`chat_session_app`).
    pub chat_watch: bool,
    /// A Codecierge session runs in this pane: the same `Request::CommandEnded`, one event kind
    /// for both watchers (ruling P14).
    pub codecierge_watch: bool,
    /// Prompt, command and custom escape sequence events for the API, taken by the app.
    pub api_events: Vec<crate::api_notify::ApiEvent>,
    /// The cursor (absolute line, column) at the last accrue check; set at B.
    pub(crate) composer_from: Option<(i64, usize)>,
    /// Cells the accrue check has read (a test counter for its bound).
    pub(crate) composer_scanned: u64,
    /// The alternate screen was showing at the last check.
    composer_alt: bool,
    /// `Term::printed` when the running command's output started (`marks.rs`).
    pub(crate) printed_at_output: u64,
    /// Instant Replay: the rows and cursor of the last capture, so a line Tandem edited
    /// outside alacritty's damage (image tiles, trigger highlights, folds) is still dirty,
    /// and the cursor line alacritty always damages is dirty only when it changed.
    replay_rows: Vec<Row<Cell>>,
    replay_cursor: Option<(usize, usize)>,
}

/// One Instant Replay frame from the screen: the visible rows of the active grid, which
/// of them are unchanged since the last capture, and the cursor (column, line).
#[derive(Clone, Debug)]
pub struct Capture {
    pub rows: Vec<Row<Cell>>,
    pub clean: Vec<bool>,
    pub cursor: (usize, usize),
}

/// DECRQCRA replies answered in one `feed` (the rest get none).
const DECRQCRA_PER_BATCH: u16 = 64;
/// OSC 72 messages answered in one `feed`.
pub const DND_ANSWERS_PER_BATCH: u16 = 64;

/// What an escape-sequence print prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrintJob {
    /// The text between `CSI 5 i` and `CSI 4 i`.
    Text(String),
    /// The screen (`CSI i`, `CSI 0 i` and any other value).
    Screen,
}

/// Something output asked for that the window must do.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// An escape-sequence print (`CSI 4 i` after `CSI 5 i`, or `CSI i`): the app asks the pane's
    /// print guard, then the person's print dialog.
    Print(PrintJob),
    /// Absolute row numbers below `below` moved down by `by` (rows spliced into the history
    /// when the alternate screen ended); the app moves the ones it holds.
    /// `api_from` is the API numbering's first on-screen line before the splice (lost plus kept
    /// history), where rows named by an absolute line moved down by `by`.
    HistoryRebased { below: i64, by: i64, api_from: i64 },
    /// CSI 3 J arrived while Prevent CSI 3 J is unset: ask once whether to allow it.
    AskClearHistory,
    /// DECRQCRA arrived while Disable DECRQCRA is unset: ask once whether to allow it.
    AskDecrqcra,
    /// OSC 7 with a machine id arrived while Accept OSC 7 is off.
    Osc7Disabled,
    /// Put text on the clipboard (if the user allows programs to).
    ClipboardWrite(String),
    /// Report the clipboard; the selection parameter to echo.
    ClipboardRead(String),
    /// HighlightCursorLine: the pane's cursor guide on or off.
    CursorGuide(bool),
    /// RequestAttention.
    Attention(crate::osc1337::Attention),
    /// StealFocus: bring this pane to the front (after asking).
    StealFocus,
    /// OSC 1337 Disinter: a buried session asks to be restored.
    Disinter,
    /// Answer with the cell size, the window frame, or the screens' work areas.
    ReportCellSize,
    ReportWindowFrame,
    ReportScreenFrames,
    /// SetWindowFrame, in AppKit points.
    SetWindowFrame(f64, f64, f64, f64),
    /// SetProfile, SetProfileProperty (key, base64 JSON), SetBackgroundImageFile (base64).
    SetProfile(String),
    SetProfileProperty(Vec<(String, String)>),
    SetBackgroundImageFile(String),
    /// OpenURL, ReportVariable (base64 name), RequestUpload (its parameters).
    OpenUrl(String),
    ReportVariable(String),
    RequestUpload(String),
    /// SetColors `tab`: the pane's tab color, or back to none.
    TabColor(Option<alacritty_terminal::vte::ansi::Rgb>),
    /// A trigger's action the window runs.
    Trigger(crate::trigger_eval::TriggerRequest),
    /// BEL.
    Bell,
    /// OSC 9 or OSC 1337 Notification: an escape-sequence alert.
    Notify { title: Option<String>, message: String },
    /// OSC 9;4: a progress bar appeared (true) or went away.
    Progress(bool),
    /// A program set (Some) or reset (None) the session's title: the window's other sessions
    /// take it when SeparateWindowTitlePerTab is off.
    WindowTitle(Option<String>),
    /// SetBadgeFormat: the session's badge text.
    SetBadge(String),
    /// SetBadgeFormat with a function call: refused.
    BadgeRefused,
    /// Shell integration reported another account (user or host, the first report included):
    /// a waiting outdated scripts notice goes, and at a host change (`marks::host_changed`)
    /// these modes, left on by the previous host, are offered for turning off (empty when
    /// nothing is offered).
    HostChanged(crate::marks::LeftOn),
    /// The reported host name changed from a known one (m14e Task 4): the host stack decides
    /// what to restore. Locality is as reported, decided when each host was. `mode` and `title`
    /// are the key mode and title in effect at the report (fix r1), never after the chunk.
    HostMoved { previous: crate::host_state::Remote, current: crate::host_state::Remote, mode: crate::keyproto::KeyMode, title: Option<String> },
    /// ShellIntegrationVersion: the version and the shell this report names (None when it
    /// names none), for the outdated scripts notice.
    ShellIntegrationReported { version: Option<u32>, shell: Option<String> },
    /// `it2ssh=`: the pane starts connecting (`screenWillBeginSSHIntegration`).
    SshWillBegin,
    /// A version 3 `SendConductor=`: type the conductor bootstrap.
    SshSendConductor(crate::ssh_handshake::Handshake),
    /// Trusted `EndSSH=`: end the pane's hops through this it2ssh unique id.
    SshEnd(String),
    /// A command finished in a session a chat streams or Codecierge runs: its exit code and mark
    /// (`chat_watch`, `codecierge_watch`).
    CommandEnded { code: i32, mark: Option<u32> },
}

/// Key reporting and emulation chosen in Session > Terminal State.
#[derive(Clone, Copy, Debug)]
pub struct KeyState {
    /// CSI u mode (the profile's Use libtickit protocol, or the menu).
    pub csi_u: bool,
    /// Raw key reporting (the menu).
    pub raw: bool,
    /// Kitty flags the menu turned on, and ones it turned off.
    pub kitty_on: u8,
    pub kitty_off: u8,
    /// Literal Controls: control bytes show as carets instead of acting.
    pub literal: bool,
    /// The profile's Use libtickit protocol.
    pub profile_csi_u: bool,
}

impl KeyState {
    pub fn new(csi_u: bool) -> Self {
        KeyState { csi_u, raw: false, kitty_on: 0, kitty_off: 0, literal: false, profile_csi_u: csi_u }
    }
}

/// The tick behind `Screen::tab_status_gen`.
static TAB_STATUS_TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// What a program's end (and a restart's end message) writes to the parser before the cursor is
/// put back: SGR, charset, an open hyperlink, the scroll region, origin mode, wrap, insert mode,
/// and a synchronized update or color scheme reports the program left on (13q B9).
pub(crate) const END_RESET: &str = "\x1b[0m\x1b(B\x0f\x1b]8;;\x1b\\\x1b[r\x1b[?6l\x1b[?7h\x1b[4l\x1b[?2026l\x1b[?2031l";

impl Screen {
    /// Applies OSC 21337 or Set Tab Status fields (`tabstatus::Status::apply`); a change moves
    /// `tab_status_gen`. True when something changed.
    pub fn apply_tab_status(&mut self, pairs: &[(String, String)]) -> bool {
        let changed = self.tab_status.apply(pairs);
        if changed {
            self.tab_status_changed();
        }
        changed
    }

    /// Stores the status's background task count; a change moves `tab_status_gen`.
    pub fn set_tab_background_tasks(&mut self, n: i64) -> bool {
        let changed = self.tab_status.set_background_tasks(n);
        if changed {
            self.tab_status_changed();
        }
        changed
    }

    /// Clears an active status and its background count (a new prompt); an inactive status, a
    /// count alone included, stays (`PTYSession.m:25702-25705`).
    pub(crate) fn clear_tab_status(&mut self) {
        if self.tab_status.is_active() {
            self.tab_status = crate::tabstatus::Status::default();
            self.tab_status_changed();
        }
    }

    fn tab_status_changed(&mut self) {
        self.tab_status_gen = TAB_STATUS_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    }

    pub fn new(cols: usize, rows: usize) -> Self {
        Screen::with_look(cols, rows, Palette::default(), Look::default())
    }

    pub fn with_look(cols: usize, rows: usize, palette: Palette, look: Look) -> Self {
        let (tx, events) = mpsc::channel();
        let mut term = Term::new(config(look), &GridSize { cols, rows }, Listener(tx));
        term.set_cursor_blink_change(look.blink_change);
        term.set_alt_scrollback(look.alt_scrollback);
        term.width_rule = Some(width_rule(&look, &crate::emu::Rules::default(), 0));
        Self {
            term,
            sel: Default::default(),
            blink_allowed: look.blink_allowed,
            bidi_rules: crate::bidi::Rules::default(),
            bidi_cache: Default::default(),
            parser: Processor::new(),
            events,
            palette,
            cursor_hidden_at: None,
            title: None,
            allow_title: true,
            cell_px: (0, 0),
            osc: crate::osc::Scanner::default(),
            marks: crate::marks::Marks::default(),
            line_style_marks: false,
            shell: crate::marks::Shell::default(),
            place_prompt_first_column: true,
            pty_backed: false,
            clock: None,
            images: crate::images::Store::default(),
            downloads: Vec::new(),
            multipart: None,
            inline_hold: None,
            held_output: Vec::new(),
            inline_holds: 0,
            kitty: crate::kitty::Kitty::default(),
            replies: Vec::new(),
            limits: Limits::default(),
            notes: crate::annotations::Notes::default(),
            folds: std::collections::HashMap::new(),
            next_fold: 0,
            feeds: 0,
            fed: false,
            rules: crate::emu::Rules::default(),
            term_name: "xterm-256color".to_string(),
            saw_location_code: false,
            tick_gen: 0,
            offscreen_cache: None,
            keys: KeyState::new(look.csi_u),
            trusted: true,
            dnd: Default::default(),
            tab_status: crate::tabstatus::Status::default(),
            tab_status_gen: 0,
            blocks: crate::blocks::Blocks::default(),
            buttons: crate::blocks::Buttons::default(),
            portholes: std::collections::HashMap::new(),
            triggers: crate::trigger_eval::TriggerState::default(),
            app_vars: std::collections::HashMap::new(),
            bell_count: 0,
            progress: crate::progress::Progress::Stopped,
            resets_seen: 0,
            last_output: None,
            next_porthole: 0,
            porthole_wide_default: false,
            porthole_selection: None,
            env: None,
            ssh_flags: None,
            exec_failed: false,
            unicode: crate::osc1337::Unicode::default(),
            profile_unicode: look.unicode_version,
            normalization: look.normalization,
            look,
            answerback: String::new(),
            answerback_cp: 0,
            key_labels: crate::osc1337::KeyLabels::default(),
            color_sets: Vec::new(),
            clipboard_allowed: false,
            host_policy: Default::default(),
            requests: Vec::new(),
            copy: crate::clipboard_osc::Copy::default(),
            capture: None,
            capture_hold: Vec::new(),
            print_capture: None,
            answerback_sent: false,
            title_sent: false,
            dnd_answered: 0,
            decrqcra_sent: 0,
            color_reports: false,
            last_dark: true,
            prompt_state: Default::default(),
            composer_events: Vec::new(),
            api_watch: false,
            chat_watch: false,
            codecierge_watch: false,
            api_events: Vec::new(),
            composer_from: None,
            composer_scanned: 0,
            composer_alt: false,
            printed_at_output: 0,
            replay_rows: Vec::new(),
            replay_cursor: None,
        }
    }

    /// A screen with `lines` of scrollback.
    pub fn with_scrollback(cols: usize, rows: usize, lines: usize) -> Self {
        Screen::with_look(cols, rows, Palette::default(), Look { scrollback: lines, ..Look::default() })
    }

    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
        // What a program set with SetColors stays over the profile's colors.
        for (name, color) in self.color_sets.clone() {
            self.apply_color(&name, color);
        }
        let report = self.color_scheme_change();
        self.replies.extend(report);
    }

    /// The display order of grid line `line` (negative lines are history), or None when the row
    /// needs none: Bidi is off, the alternate screen with its setting, no right-to-left letter,
    /// or the row holds an inline image (its tiles sit at logical columns). The order is cached
    /// by row content, so a redrawn row never keeps an old one, and the cache is cleared past
    /// four screens of distinct rows. It covers the row's used cells and, with right
    /// justification, is padded to the row's width (`bidi::pad`).
    pub fn row_order(&self, line: i32) -> Option<std::sync::Arc<crate::bidi::Order>> {
        use alacritty_terminal::term::cell::Flags;
        use std::hash::{Hash, Hasher};
        let rules = self.bidi_rules;
        let alternate = self.term.mode().contains(TermMode::ALT_SCREEN);
        if !rules.enabled || (alternate && rules.off_in_alternate) {
            return None;
        }
        let grid = self.term.grid();
        if line < -(grid.history_size() as i32) || line >= grid.screen_lines() as i32 {
            return None;
        }
        let row = &grid[Line(line)];
        let cols = grid.columns();
        let spacer = Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER;
        // Quick scan: an ASCII row, the common case, costs one pass and no allocation.
        let mut rtl = false;
        let mut used = 0;
        for col in 0..cols {
            let c = &row[Column(col)];
            if c.image().is_some() {
                return None;
            }
            let blank = (c.c == ' ' || c.c == '\0') && c.zerowidth().is_none() && !c.flags.intersects(spacer);
            if !blank {
                used = col + 1;
            }
            rtl |= crate::bidi::is_rtl_letter(c.c);
        }
        if !rtl {
            return None;
        }
        let texts: Vec<crate::bidi::CellText> = (0..used)
            .map(|col| {
                let c = &row[Column(col)];
                let is_spacer = c.flags.intersects(spacer);
                let mut text = String::new();
                if !is_spacer {
                    text.push(c.c);
                    text.extend(c.zerowidth().into_iter().flatten());
                }
                crate::bidi::CellText { text, wide: c.flags.contains(Flags::WIDE_CHAR), spacer: is_spacer }
            })
            .collect();
        let sig = (cols, rules.detect_direction, rules.right_justify);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (sig, &texts).hash(&mut h);
        let key = h.finish();
        #[cfg(test)]
        let key = tests::FORCE_KEY.with(|f| f.get()).unwrap_or(key);
        // The 64-bit digest only picks a bucket: a hit must hold this very row, so a crafted
        // collision can never show one row in another row's order.
        if let Some(hit) = self.bidi_cache.borrow().get(&key) {
            if hit.sig == sig && hit.texts == texts {
                return hit.order.clone();
            }
        }
        let made = crate::bidi::order(&texts, rules, alternate).map(|o| std::sync::Arc::new(if rules.right_justify && used < cols { crate::bidi::pad(&o, cols) } else { o }));
        let mut cache = self.bidi_cache.borrow_mut();
        if cache.len() >= 4 * grid.screen_lines().max(1) {
            cache.clear();
        }
        cache.insert(key, BidiEntry { sig, texts, order: made.clone() });
        made
    }

    /// The display order of viewport row `row` (as drawn: the view's scroll offset applied),
    /// or None when it has none or does not cover this width. The pointer, the copy-mode
    /// cursor and the IME go through here, so they land where `scene::pane_with` drew.
    fn viewport_order(&self, row: usize) -> Option<std::sync::Arc<crate::bidi::Order>> {
        let grid = self.term.grid();
        self.row_order(row as i32 - grid.display_offset() as i32).filter(|o| o.lut.len() <= grid.columns())
    }

    /// The visual column at which logical column `col` of viewport row `row` is drawn.
    pub fn visual_col(&self, row: usize, col: usize) -> usize {
        crate::bidi::visual_for_logical(self.viewport_order(row).as_deref(), col)
    }

    /// The logical column drawn at visual column `col` of viewport row `row`, and whether that
    /// cell lies in a right-to-left run (a pointer on its left half is then at the character's
    /// end).
    pub fn logical_col(&self, row: usize, col: usize) -> (usize, bool) {
        let order = self.viewport_order(row);
        let logical = crate::bidi::logical_for_visual(order.as_deref(), col);
        (logical, order.is_some_and(|o| o.rtl.get(logical).copied().unwrap_or(false)))
    }

    /// The visual pieces the logical columns `cols` of viewport row `row` are drawn in.
    pub fn visual_runs(&self, row: usize, cols: std::ops::Range<usize>) -> Vec<std::ops::Range<usize>> {
        match self.viewport_order(row) {
            Some(o) => crate::bidi::visual_spans(&o, cols),
            None if cols.is_empty() => Vec::new(),
            None => vec![cols],
        }
    }

    /// A character drag on a row with a display order is VISUAL, as iTerm2's
    /// (`PTYMouseHandler.m:393-402`, `iTermSelection.m:409`): the selection is the cells the
    /// pointer swept on screen, copied in logical order. `a` and `b` are the press and the
    /// pointer as (grid line, visual column, right half of that visual cell). The selection
    /// takes the logical runs those cells show; false when no swept row has an order (the
    /// logical selection then does the same job) or there is no selection or nothing swept.
    pub fn drag_visual_selection(&mut self, a: (i32, usize, bool), b: (i32, usize, bool)) -> bool {
        use alacritty_terminal::index::Point;
        // A boundary on the visual axis: before a cell, or after it for its right half.
        let bound = |p: (i32, usize, bool)| (p.0, p.1 + p.2 as usize);
        let (mut lo, mut hi) = (bound(a), bound(b));
        if lo > hi {
            std::mem::swap(&mut lo, &mut hi);
        }
        let cols = self.term.columns();
        let orders: Vec<_> = (lo.0..=hi.0).map(|line| self.row_order(line).filter(|o| o.lut.len() <= cols)).collect();
        if orders.iter().all(|o| o.is_none()) {
            return false;
        }
        let mut runs = Vec::new();
        for (i, line) in (lo.0..=hi.0).enumerate() {
            let from = if line == lo.0 { lo.1 } else { 0 };
            let to = if line == hi.0 { hi.1 } else { cols };
            let mut logical: Vec<usize> = (from..to.min(cols)).map(|v| crate::bidi::logical_for_visual(orders[i].as_deref(), v)).collect();
            logical.sort_unstable();
            let mut k = 0;
            while k < logical.len() {
                let mut e = k;
                while e + 1 < logical.len() && logical[e + 1] == logical[e] + 1 {
                    e += 1;
                }
                runs.push((Point::new(Line(line), Column(logical[k])), Point::new(Line(line), Column(logical[e]))));
                k = e + 1;
            }
        }
        let Some(sel) = self.term.selection.as_mut().filter(|s| s.ty == alacritty_terminal::selection::SelectionType::Simple) else { return false };
        if runs.is_empty() {
            return false;
        }
        sel.set_visual(runs);
        true
    }

    /// Where the drag in progress began, as `drag_visual_selection` takes it; the selection
    /// carries it, so output scrolling the grid moves it with the content.
    pub fn visual_anchor(&self) -> Option<(i32, usize, bool)> {
        self.term.selection.as_ref().and_then(|s| s.visual_anchor())
    }

    /// Whether a visual (bidi) selection reads the whole lines `a.line..=b.line` from `a`:
    /// Replace Selection refuses runs that skip cells inside their lines, since it rewrites
    /// the lines. True when the selection is not visual.
    pub fn visual_selection_is_whole_lines(&self, a: alacritty_terminal::index::Point, b: alacritty_terminal::index::Point) -> bool {
        use alacritty_terminal::index::{Column, Point};
        if self.term.selection.as_ref().is_none_or(|s| s.runs.is_empty()) {
            return true;
        }
        let text = self.term.bounds_to_string(a, Point::new(b.line, Column(self.term.columns() - 1)));
        self.term.selection_to_string().as_deref().map(str::trim_end) == Some(text.trim_end())
    }

    /// `drag_visual_selection` from the stored anchor to the pointer at `b`.
    pub fn drag_visual_to(&mut self, b: (i32, usize, bool)) -> bool {
        match self.visual_anchor() {
            Some(a) => self.drag_visual_selection(a, b),
            None => false,
        }
    }

    pub fn set_look(&mut self, look: Look) {
        // A new profile default for CSI u applies unless the menu chose otherwise.
        if self.keys.csi_u == self.keys.profile_csi_u {
            self.keys.csi_u = look.csi_u;
        }
        self.keys.profile_csi_u = look.csi_u;
        self.term.set_alt_scrollback(look.alt_scrollback);
        self.term.set_options(config(look));
        self.term.set_cursor_blink_change(look.blink_change);
        self.blink_allowed = look.blink_allowed;
        self.profile_unicode = look.unicode_version;
        self.normalization = look.normalization;
        self.look = look;
        self.push_width_rule();
    }

    /// Hands the `Term` the width rule for the profile's settings, the Advanced rules and the
    /// session's Unicode version. Runs on every change of any of them; cells already written
    /// keep their widths, as in iTerm2.
    pub(crate) fn push_width_rule(&mut self) {
        self.term.width_rule = Some(width_rule(&self.look, &self.rules, self.unicode.version));
    }

    /// The profile's Unicode version, from a profile edit (`screen_follows_profile`).
    pub fn set_profile_unicode(&mut self, version: u8) {
        self.profile_unicode = version;
        self.look.unicode_version = version;
        self.push_width_rule();
    }

    /// The cells `text` takes under this screen's width rule (the profile's Ambiguous Double
    /// Width, Unicode version and the Advanced switches), as the grid draws it.
    pub(crate) fn text_width(&self, text: &str) -> usize {
        match &self.term.width_rule {
            Some(rule) => text.chars().map(|c| (rule.width)(c, rule).unwrap_or(0)).sum(),
            None => unicode_width::UnicodeWidthStr::width(text),
        }
    }

    /// Whether the profile makes ambiguous-width characters two cells.
    pub fn ambiguous_wide(&self) -> bool {
        self.look.ambiguous_wide
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.bump_ticks();
        self.term.resize(GridSize { cols, rows });
        // Reflow moved rows: triggers go on from the cursor.
        self.triggers_from_cursor();
        self.composer_rows_moved();
    }

    pub fn term(&self) -> &Term<Listener> {
        &self.term
    }

    pub fn term_mut(&mut self) -> &mut Term<Listener> {
        self.bump_ticks();
        &mut self.term
    }

    /// Replaces the live selection (None clears it) made in `mode`. The column window and the
    /// word or line a drag snaps around start over.
    pub fn set_selection(&mut self, sel: Option<alacritty_terminal::selection::Selection>, mode: crate::seltext::Mode) {
        self.sel = crate::seltext::State { live_mode: mode, ..Default::default() };
        self.term_mut().selection = sel;
    }

    pub fn clear_selection(&mut self) {
        self.set_selection(None, crate::seltext::Mode::Character);
    }

    /// Reshapes the live selection a drag is extending, keeping its mode, window and anchor.
    pub fn reshape_selection(&mut self, sel: alacritty_terminal::selection::Selection) {
        self.term_mut().selection = Some(sel);
    }

    /// Instant Replay's frame (`TV:2876-2917`, `S:683-699`): None when nothing changed since
    /// the last capture (`TV:2910-2917` saves only when something is dirty). Dirty lines are
    /// alacritty's damage, which only this reads and resets (reset either way). Full damage,
    /// or a view scrolled back, leaves no line clean (iTerm2's scrolled case), but gives no
    /// frame when the size, the cursor and every row are as last captured (alacritty reports
    /// full damage on every read in insert mode). The cursor's old and new lines are dirty
    /// when it moved.
    pub fn replay_capture(&mut self) -> Option<Capture> {
        let (lines, cols) = (self.term.screen_lines(), self.term.columns());
        let point = self.term.grid().cursor.point;
        let cursor = (point.column.0, (point.line.0.max(0) as usize).min(lines.saturating_sub(1)));
        let scrolled = self.term.grid().display_offset() > 0;
        let damaged = match self.term.damage() {
            TermDamage::Full => None,
            TermDamage::Partial(it) => {
                let mut d = vec![false; lines];
                for l in it {
                    if let Some(x) = d.get_mut(l.line) {
                        *x = true;
                    }
                }
                Some(d)
            }
        };
        self.term.reset_damage();
        let same_size = self.replay_rows.len() == lines && self.replay_rows.first().is_some_and(|r| r.len() == cols);
        let moved = self.replay_cursor != Some(cursor);
        let grid = self.term.grid();
        let same = |y: usize| grid[Line(y as i32)] == self.replay_rows[y];
        let clean: Vec<bool> = match damaged {
            Some(mut d) if !scrolled && same_size => {
                // alacritty damages the cursor's cell on every read; iTerm2 marks the cursor
                // line only when the cursor moved, else when the line itself changed.
                if !moved {
                    d[cursor.1] = false;
                    if !d.iter().any(|x| *x) && same(cursor.1) {
                        return None;
                    }
                }
                (0..lines).map(|y| !d[y] && same(y)).collect()
            }
            _ => {
                if same_size && !moved && (0..lines).all(same) {
                    return None;
                }
                vec![false; lines]
            }
        };
        let rows: Vec<Row<Cell>> = (0..lines).map(|y| grid[Line(y as i32)].clone()).collect();
        if same_size {
            // Clean lines already equal the copy.
            for y in (0..lines).filter(|y| !clean[*y]) {
                self.replay_rows[y].clone_from(&rows[y]);
            }
        } else {
            self.replay_rows.clone_from(&rows);
        }
        self.replay_cursor = Some(cursor);
        Some(Capture { rows, clean, cursor })
    }

    /// Instant Replay without room for a frame: the damage is read and dropped, nothing is
    /// copied or compared.
    pub fn replay_discard(&mut self) {
        let _ = self.term.damage();
        self.term.reset_damage();
    }

    pub fn set_cell_px(&mut self, w: u16, h: u16) {
        self.cell_px = (w, h);
    }

    /// Set Title (a trigger), or a title another session of the window set: the session's
    /// title. It asks nothing of the window.
    pub fn set_title(&mut self, title: Option<String>) {
        self.title = title;
    }

    /// A drop may be reported to the program: it registered and the screen is trusted (B7).
    pub fn dnd_accepting(&self) -> bool {
        self.trusted && self.dnd.accepting()
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Feeds app output. Returns the bytes the terminal must send back, such as
    /// answers to device, color, and size queries. An app that gets no answer may hang.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.answerback_sent = false;
        self.title_sent = false;
        self.decrqcra_sent = 0;
        self.dnd_answered = 0;
        self.bump_ticks();
        if !bytes.is_empty() && !self.triggers.list.is_empty() {
            self.events_on_output();
        }
        // With triggers, a read holding more lines than the buffer keeps goes in pieces, so
        // every line is checked before it scrolls away.
        let g = self.term.grid();
        let limit = ((g.max_scroll_limit() + g.screen_lines()) / 2).max(1);
        if self.triggers.list.is_empty() || bytes.len() < limit {
            return self.feed_chunk(bytes);
        }
        let mut reply = Vec::new();
        let (mut start, mut lines) = (0, 0);
        for (i, b) in bytes.iter().enumerate() {
            if *b == b'\n' {
                lines += 1;
                if lines == limit {
                    reply.extend(self.feed_chunk(&bytes[start..=i]));
                    start = i + 1;
                    lines = 0;
                }
            }
        }
        reply.extend(self.feed_chunk(&bytes[start..]));
        reply
    }

    fn feed_chunk(&mut self, bytes: &[u8]) -> Vec<u8> {
        // An inline file waits for the person: nothing after it is read until they answer.
        if self.inline_hold.is_some() {
            if self.held_output.len() + bytes.len() <= self.limits.held {
                self.held_output.extend_from_slice(bytes);
                return Vec::new();
            }
            // Too much waits on the person: the file is dropped, as a No, and all of it is read.
            self.inline_hold = None;
            let mut held = std::mem::take(&mut self.held_output);
            held.extend_from_slice(bytes);
            return self.feed(&held);
        }
        if self.keys.literal {
            let reply = self.feed_raw(&literal(bytes));
            self.run_triggers_after_feed();
            return reply;
        }
        // Shell-integration OSCs act where they appear in the stream, so the output is fed in
        // pieces that end at each one.
        // A CUP is held only while it could be a running command's first draw (13o Task 4).
        self.osc.cup_armed = self.rules.save_above_prompt
            && self.command_pending()
            && !self.term.mode().contains(TermMode::ALT_SCREEN)
            && (!self.command_running() || self.term.printed == self.printed_at_output);
        let events = self.osc.scan_spans(bytes);
        let mut pending = self.osc.pending;
        let mut reply = Vec::new();
        let mut at = 0;
        // Where this chunk's reading stops: its end, or just after an inline file that waits.
        let mut stop = bytes.len();
        let mut still_held = false;
        if self.capture.is_some() && !self.capture_hold.is_empty() {
            if events.first().is_some_and(|e| e.0 == 0) {
                // The held bytes began a sequence that ended here as an event: not text.
                self.capture_hold.clear();
            } else if events.is_empty() && pending == Some(0) {
                // Still inside the same sequence.
                let room = self.limits.copy.saturating_sub(self.capture_hold.len());
                self.capture_hold.extend_from_slice(&bytes[..bytes.len().min(room)]);
                still_held = true;
            } else {
                let held = std::mem::take(&mut self.capture_hold);
                self.capture_bytes(&held);
            }
        }
        for (start, end, osc) in events {
            self.capture_bytes(&bytes[at..start.max(at)]);
            if let crate::osc::Osc::Held(held) = &osc {
                // A sequence a setting may drop or rewrite: what came before it is fed, then
                // whatever `pass` lets through (never the original bytes).
                reply.extend(self.feed_raw(&bytes[at..start.max(at)]));
                at = end;
                if held.reads_state() {
                    if self.sync_deadline().is_some() {
                        reply.extend(self.flush_sync());
                    }
                    self.composer_accrue();
                }
                let fed = match held {
                    crate::emu::Held::Drop => Vec::new(),
                    crate::emu::Held::CancelString => vec![0x18],
                    _ => {
                        let keep = self.pass(held);
                        crate::emu::held_bytes(keep)
                    }
                };
                reply.extend(self.feed_raw(&fed));
                // A cursor move that may be a running command's first draw: iTerm2's rule looks
                // at where the cursor landed.
                if matches!(held, crate::emu::Held::Cup { .. }) {
                    self.cursor_moved_above_output();
                }
                reply.append(&mut self.replies);
                continue;
            }
            reply.extend(self.feed_raw(&bytes[at..end]));
            at = end;
            match osc {
                // The DCS form of synchronized output starts and ends it as mode 2026 does.
                crate::osc::Osc::Sync(on) => {
                    let bsu: &[u8] = if on { b"\x1b[?2026h" } else { b"\x1b[?2026l" };
                    reply.extend(self.feed_raw(bsu));
                }
                osc => {
                    // What Tandem does for an event acts on the screen as the bytes before it
                    // left it, so a synchronized update in progress is drawn first.
                    if self.sync_deadline().is_some() {
                        reply.extend(self.flush_sync());
                    }
                    // The echo before the event is accrued first, as iTerm2 sees it token by
                    // token.
                    self.composer_accrue();
                    self.apply(osc);
                }
            }
            reply.append(&mut self.replies);
            if self.inline_hold.is_some() {
                // The rest of the chunk is read again when the person answers, by a scanner
                // back where this sequence left it (between sequences).
                self.held_output.extend_from_slice(&bytes[at..]);
                self.osc = Default::default();
                pending = None;
                stop = at;
                break;
            }
        }
        if !still_held {
            match pending.filter(|p| *p >= at) {
                Some(p) if self.capture.is_some() => {
                    self.capture_bytes(&bytes[at..p]);
                    self.capture_hold = bytes[p..].to_vec();
                }
                _ => self.capture_bytes(&bytes[at..stop]),
            }
        }
        reply.extend(self.feed_raw(&bytes[at..stop]));
        reply.extend(self.color_scheme_change());
        self.composer_accrue();
        self.run_triggers_after_feed();
        reply
    }

    /// REP after a character with combining marks repeats the whole cell, base and marks, as
    /// iTerm2 does (`lastCharacter` is a cell); vte alone would repeat the last mark. The text to
    /// feed instead, or None when the cell before the cursor has no marks (vte's repeat is then the
    /// same).
    fn rep_cell(&self, n: u16) -> Option<Vec<u8>> {
        use alacritty_terminal::term::cell::Flags;
        let g = self.term.grid();
        let cur = g.cursor.point;
        let mut col = cur.column.0;
        if !g.cursor.input_needs_wrap {
            col = col.saturating_sub(1);
        }
        if g[cur.line][Column(col)].flags.contains(Flags::WIDE_CHAR_SPACER) {
            col = col.saturating_sub(1);
        }
        let cell = &g[cur.line][Column(col)];
        let marks = cell.zerowidth().filter(|m| !m.is_empty())?;
        let mut one = String::new();
        one.push(cell.c);
        one.extend(marks);
        // The sequence's own bound (vte repeats at most 65535 times; 0 is once).
        Some(one.repeat(n.max(1) as usize).into_bytes())
    }

    /// Gives the bytes to a running print job. None: all taken. Some: the job ended (the bound,
    /// its age or RIS) with nothing printed, and these bytes are for the parser.
    fn divert_to_print(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        let cap = self.print_capture.as_mut()?;
        if cap.started.elapsed().as_secs() >= crate::emu::PRINT_CAPTURE_SECS {
            // The capture ends by age. Inside a sequence, its tail is dropped (the parser never saw
            // its start, so the tail would print as text).
            let rest = cap.after_sequence(bytes);
            self.print_capture = None;
            return rest.map(|(at, esc)| {
                let mut out = if esc { vec![0x1b] } else { Vec::new() };
                out.extend_from_slice(&bytes[at..]);
                out
            });
        }
        match cap.push(bytes, self.limits.copy) {
            None => None,
            Some((at, esc)) => {
                self.print_capture = None;
                let mut rest = if esc { vec![0x1b] } else { Vec::new() };
                rest.extend_from_slice(&bytes[at..]);
                Some(rest)
            }
        }
    }

    /// What reaches the parser for a held sequence: its canonical bytes, a rewrite, or nothing.
    /// Later tasks add arms; until then every sequence passes as it would without being held.
    pub(crate) fn pass(&mut self, held: &crate::emu::Held) -> Option<Vec<u8>> {
        use crate::emu::{rebuild, Held};
        use crate::prompted::Decision;
        // While a print job collects, a sequence is text to drop (Decision, P8); only the job's
        // own end acts.
        if self.print_capture.is_some() && !matches!(held, Held::Print(_)) {
            return None;
        }
        match held {
            Held::AltScreen { params, set } if self.look.disable_smcup => {
                crate::emu::strip_alt_screen(params).map(|params| rebuild(&Held::AltScreen { params, set: *set }))
            }
            Held::Print(n) => {
                match n {
                    5 if self.look.disable_printing || self.print_capture.is_some() => {}
                    5 => self.print_capture = Some(crate::emu::PrintCapture::default()),
                    4 => {
                        if let Some(job) = self.print_capture.take() {
                            let text = job.text();
                            if !text.is_empty() {
                                self.push_print(PrintJob::Text(text));
                            }
                        }
                    }
                    _ if self.look.disable_printing || self.print_capture.is_some() => {}
                    _ => self.push_print(PrintJob::Screen),
                }
                None
            }
            Held::Rep(_) if !self.rules.rep => None,
            Held::Rep(n) => Some(self.rep_cell(*n).unwrap_or_else(|| rebuild(held))),
            Held::ClearHistory => match crate::emu::clear_history_decision(self.rules.clear_history) {
                Decision::Apply => Some(rebuild(held)),
                Decision::Deny => None,
                Decision::Ask => {
                    // One question at a time: a flood of CSI 3 J queues one request.
                    if !self.requests.iter().any(|r| matches!(r, Request::AskClearHistory)) {
                        self.requests.push(Request::AskClearHistory);
                    }
                    None
                }
            },
            Held::Decrqcra(p) => {
                // iTerm2: only at VT400 and up, only when trusted, then the key (`T:2109-2124`).
                if self.level() >= 400 && self.trusted {
                    match self.rules.decrqcra_off {
                        Some(false) if self.decrqcra_sent >= DECRQCRA_PER_BATCH => {}
                        Some(false) => {
                            self.decrqcra_sent += 1;
                            let params: Vec<i64> = p.iter().map(|v| i64::from(*v)).collect();
                            let sum = crate::emu::decrqcra_rect(&params, self.term.columns(), self.term.screen_lines()).map_or(0, |r| self.checksum_in(r));
                            self.replies.extend(crate::emu::checksum_reply(params.first().copied().unwrap_or(0), sum).into_bytes());
                        }
                        Some(true) => {}
                        None => {
                            if !self.requests.iter().any(|r| matches!(r, Request::AskDecrqcra)) {
                                self.requests.push(Request::AskDecrqcra);
                            }
                        }
                    }
                }
                None
            }
            Held::TitleReport(k) => {
                // Only with Allow Title Reporting and trust (`S:18316-18351`); a tmux pane's
                // replies are dropped by its pane. One title serves 20 and 21 (Decision).
                if self.look.allow_title_reporting && self.trusted && !std::mem::replace(&mut self.title_sent, true) {
                    self.replies.extend(crate::emu::title_report(*k, self.title().unwrap_or("")).into_bytes());
                }
                None
            }
            // ENQs in one batch coalesce: one answerback (a flood would queue 256 bytes each).
            Held::Enq if std::mem::replace(&mut self.answerback_sent, true) => None,
            Held::Enq => {
                let bytes = crate::encoding::encode_whole(self.answerback_cp, &self.answerback, crate::encoding::system());
                self.replies.extend_from_slice(&bytes);
                None
            }
            h => Some(rebuild(h)),
        }
    }

    /// Queues a print, one at a time: while a request waits (a frame has not taken it) further
    /// ones are dropped, so a flood of CSI i is one question, not one alert each (P21).
    fn push_print(&mut self, job: PrintJob) {
        if !self.requests.iter().any(|r| matches!(r, Request::Print(_))) {
            self.requests.push(Request::Print(job));
        }
    }

    /// DECRQCRA's checksum over a rectangle (top, left, bottom, right): the UTF-16 units of every
    /// cell that holds text. Alacritty cannot tell a written space from an untouched cell, so
    /// a blank counts 0 (Decision); the spacer of a wide character holds nothing.
    fn checksum_in(&self, (top, left, bottom, right): (usize, usize, usize, usize)) -> i32 {
        use alacritty_terminal::term::cell::Flags;
        let grid = self.term.grid();
        let cells = (top..=bottom).flat_map(|y| (left..=right).map(move |x| (y, x))).filter_map(|(y, x)| {
            let cell = &grid[Line(y as i32)][Column(x)];
            let blank = (cell.c == ' ' || cell.c == '\0') && cell.zerowidth().is_none_or(|z| z.is_empty());
            (!blank && !cell.flags.contains(Flags::WIDE_CHAR_SPACER)).then_some(cell)
        });
        crate::emu::checksum(cells.flat_map(|c| std::iter::once(c.c).chain(c.zerowidth().into_iter().flatten().copied())))
    }

    /// What the prompt state machine asked of the app since the last call.
    pub fn take_composer_events(&mut self) -> Vec<crate::prompt_state::Event> {
        std::mem::take(&mut self.composer_events)
    }

    /// How keys are reported now: the program's requests and the menu's choices.
    pub fn keyboard(&self) -> crate::keyproto::Keyboard {
        use crate::keyproto::{self as kp, KeyMode, Keyboard};
        let m = self.term.mode();
        let mut app = 0;
        for (bit, flag) in [
            (kp::DISAMBIGUATE, TermMode::DISAMBIGUATE_ESC_CODES),
            (kp::EVENT_TYPES, TermMode::REPORT_EVENT_TYPES),
            (kp::ALTERNATE_KEYS, TermMode::REPORT_ALTERNATE_KEYS),
            (kp::ALL_AS_ESCAPES, TermMode::REPORT_ALL_KEYS_AS_ESC),
            (kp::ASSOCIATED_TEXT, TermMode::REPORT_ASSOCIATED_TEXT),
        ] {
            if m.contains(flag) {
                app |= bit;
            }
        }
        let kitty = (app | self.keys.kitty_on) & !self.keys.kitty_off;
        let mode = if self.keys.raw {
            KeyMode::Raw
        } else {
            match self.term.modify_other_keys {
                1 => KeyMode::Mok1,
                2 => KeyMode::Mok2,
                _ if self.keys.csi_u => KeyMode::CsiU,
                _ => KeyMode::Standard,
            }
        };
        Keyboard { mode, kitty }
    }

    /// Session > Terminal State's key reporting radio group.
    pub fn set_key_mode(&mut self, mode: crate::keyproto::KeyMode) {
        use crate::keyproto::KeyMode;
        self.keys.raw = mode == KeyMode::Raw;
        self.keys.csi_u = mode == KeyMode::CsiU;
        self.term.modify_other_keys = match mode {
            KeyMode::Mok1 => 1,
            KeyMode::Mok2 => 2,
            _ => 0,
        };
    }

    /// When a synchronized update that has not ended must be drawn anyway.
    pub fn sync_deadline(&self) -> Option<std::time::Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    /// Ends a synchronized update now (its time ran out). Returns the replies it produced.
    pub fn flush_sync(&mut self) -> Vec<u8> {
        self.parser.stop_sync(&mut self.term);
        let mut reply = self.drain_events();
        reply.extend(self.color_scheme_change());
        reply
    }

    /// Whether the pane's background is dark, as iTerm2 judges it for `CSI ? 996 n`.
    pub fn is_dark(&self) -> bool {
        use alacritty_terminal::vte::ansi::NamedColor;
        let bg = self.term.colors()[NamedColor::Background as usize].unwrap_or(self.palette.bg);
        let brightness = 0.299 * bg.r as f32 + 0.587 * bg.g as f32 + 0.114 * bg.b as f32;
        brightness < 128.0
    }

    /// Mode 2031: turns color scheme reports on or off.
    pub(crate) fn set_color_scheme_reports(&mut self, on: bool) {
        self.color_reports = on;
        self.last_dark = self.is_dark();
    }

    /// The color scheme report owed after the background crossed between dark and light.
    fn color_scheme_change(&mut self) -> Vec<u8> {
        let dark = self.is_dark();
        if !self.color_reports || dark == self.last_dark {
            return Vec::new();
        }
        self.last_dark = dark;
        crate::reports::color_scheme(dark)
    }

    /// SetColors: one named color on the pane's palette. False for an unknown name.
    pub(crate) fn apply_color(&mut self, name: &str, color: crate::osc1337::Color) -> bool {
        let rgb = match color {
            crate::osc1337::Color::Rgb(c) => c,
            crate::osc1337::Color::Index(i) => self.palette.indexed(i),
        };
        const ANSI: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
        let p = &mut self.palette;
        match name {
            "fg" => p.fg = rgb,
            "bg" => p.bg = rgb,
            "bold" => p.bold = rgb,
            "link" => p.link = rgb,
            "match" => p.match_bg = rgb,
            "selbg" => p.selection = rgb,
            "selfg" => p.selected_text = Some(rgb),
            "curbg" => p.cursor = rgb,
            "curfg" => p.cursor_text = rgb,
            n => {
                let bright = n.strip_prefix("br_");
                match ANSI.iter().position(|a| *a == bright.unwrap_or(n)) {
                    Some(i) => p.ansi[i + if bright.is_some() { 8 } else { 0 }] = rgb,
                    None => return false,
                }
            }
        }
        true
    }

    /// OSC 1337 Button: a button at the cursor, then 4 spaces (the button's room).
    pub fn add_button(&mut self, kind: crate::blocks::ButtonKind) {
        use alacritty_terminal::vte::ansi::Handler;
        let point = self.term.grid().cursor.point;
        let key = match self.term.grid()[point.line].button() {
            0 => {
                self.prune_buttons();
                let k = self.buttons.new_key();
                self.term.grid_mut()[point.line].set_button(k);
                k
            }
            k => k,
        };
        self.buttons.add(key, point.column.0, kind);
        for _ in 0..4 {
            self.term.input(' ');
        }
    }

    /// Drops the buttons of rows that are gone, once there are many keys.
    fn prune_buttons(&mut self) {
        if !self.buttons.prune_due() {
            return;
        }
        let mut live = std::collections::HashSet::new();
        for g in [self.term.primary_grid(), self.term.grid()] {
            let (top, bottom) = (-(g.history_size() as i32), g.screen_lines() as i32 - 1);
            live.extend((top..=bottom).map(|l| g[Line(l)].button()).filter(|k| *k != 0));
        }
        self.buttons.rows.retain(|k, _| live.contains(k));
        self.buttons.pruned();
    }

    /// The buttons on a line of the grid in use.
    pub fn buttons_on_line(&self, line: i32) -> Vec<crate::blocks::Button> {
        let key = self.term.grid()[Line(line)].button();
        if key == 0 {
            return Vec::new();
        }
        self.buttons.rows.get(&key).cloned().unwrap_or_default()
    }

    /// A block's text for Copy: plain, from the main screen, soft-wrapped rows joined, each
    /// line's trailing spaces trimmed, every line ending in a newline.
    pub fn block_text(&self, id: &str) -> Option<String> {
        use alacritty_terminal::term::cell::Flags;
        let (first, last) = self.block_rows(id)?;
        let g = self.term.primary_grid();
        let mut out = String::new();
        let mut line = String::new();
        let mut shown = std::collections::HashSet::new();
        for l in first..=last {
            let row = &g[Line(l)];
            // A rendered block copies the raw lines its porthole keeps.
            if let Some(p) = Some(row.porthole()).filter(|p| *p != 0).and_then(|p| self.portholes.get(&p)) {
                if shown.insert(p.id) {
                    for saved in &p.saved {
                        line.extend((0..saved.len()).map(|c| &saved[Column(c)]).filter(|c| !c.flags.contains(Flags::WIDE_CHAR_SPACER)).map(|c| c.c));
                        if !saved.last().is_some_and(|c| c.flags.contains(Flags::WRAPLINE)) {
                            out.push_str(line.trim_end());
                            out.push('\n');
                            line.clear();
                        }
                    }
                }
                continue;
            }
            line.extend((0..g.columns()).map(|c| &row[Column(c)]).filter(|c| !c.flags.contains(Flags::WIDE_CHAR_SPACER)).map(|c| c.c));
            let wrapped = l < last && row[Column(g.columns() - 1)].flags.contains(Flags::WRAPLINE);
            if !wrapped {
                out.push_str(line.trim_end());
                out.push('\n');
                line.clear();
            }
        }
        Some(out)
    }

    /// The grid lines of the newest block started with `id`, from its stamped rows.
    pub fn block_rows(&self, id: &str) -> Option<(i32, i32)> {
        let n = *self.blocks.by_id.get(id)?;
        let g = self.term.primary_grid();
        let (top, bottom) = (-(g.history_size() as i32), g.screen_lines() as i32 - 1);
        let rows: Vec<i32> = (top..=bottom).filter(|l| g[Line(*l)].block() == n).collect();
        Some((*rows.first()?, *rows.last()?))
    }

    /// UpdateBlock: folds a block into one summary line (iTerm2's text: first line, the
    /// count of the lines between, last line), tagged with the block so it still shows as one.
    pub fn fold_block(&mut self, id: &str) -> bool {
        let Some(&n) = self.blocks.by_id.get(id) else { return false };
        let Some((first, last)) = self.block_rows(id) else { return false };
        // A fold opened another way (a click on its summary line) is no longer the block's.
        if let Some(f) = self.blocks.folds.get(&n).copied() {
            if self.folds.contains_key(&f) {
                return false;
            }
            self.blocks.folds.remove(&n);
        }
        if last <= first {
            return false;
        }
        let text = |s: &Self, l: i32| {
            let row = &s.term.primary_grid()[Line(l)];
            (0..s.term.columns()).map(|c| row[Column(c)].c).collect::<String>()
        };
        let caption = crate::fold::summary(&text(self, first), text(self, last).trim_start(), (last - first + 1 - 2) as usize, self.term.columns());
        let Some(fold) = self.fold_as(first, last, Some(caption)) else { return false };
        let g = self.term.primary_grid_mut();
        let (top, bottom) = (-(g.history_size() as i32), g.screen_lines() as i32 - 1);
        if let Some(l) = (top..=bottom).find(|l| g[Line(*l)].fold() == fold) {
            g[Line(l)].set_block(n);
        }
        self.blocks.folds.insert(n, fold);
        true
    }

    /// UpdateBlock: unfolds a folded block. False when it is not folded or its summary line
    /// left the buffer.
    pub fn unfold_block(&mut self, id: &str) -> bool {
        let Some(n) = self.blocks.by_id.get(id).copied() else { return false };
        let Some(&fold) = self.blocks.folds.get(&n) else { return false };
        // The fold stays the block's when it cannot open now (the alternate screen is up).
        if !self.unfold(fold) {
            return false;
        }
        self.blocks.folds.remove(&n);
        true
    }

    /// The block a view row belongs to: its number, and its first and last grid lines.
    pub fn block_at_line(&self, line: i32) -> Option<(u32, i32, i32)> {
        let g = self.term.grid();
        if line < -(g.history_size() as i32) || line >= g.screen_lines() as i32 {
            return None;
        }
        let n = g[Line(line)].block();
        if n == 0 {
            return None;
        }
        // Walk the block's own rows out from this one (a mouse move must not scan the
        // whole buffer).
        let (top, bottom) = (-(g.history_size() as i32), g.screen_lines() as i32 - 1);
        let (mut first, mut last) = (line, line);
        while first > top && g[Line(first - 1)].block() == n {
            first -= 1;
        }
        while last < bottom && g[Line(last + 1)].block() == n {
            last += 1;
        }
        Some((n, first, last))
    }

    /// The exfiltrated tuples whose name is in `names`, in stored order; all of them for
    /// None; none without an environment (iTerm2's `exfiltratedEnvironmentVariables:`).
    pub fn exfiltrated(&self, names: Option<&[&str]>) -> Vec<(String, String)> {
        let Some(env) = &self.env else { return Vec::new() };
        env.iter().filter(|(k, _)| names.is_none_or(|n| n.contains(&k.as_str()))).cloned().collect()
    }

    /// What output asked the window to do since the last call.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    /// Whether DA1 claims clipboard support (`52`).
    pub fn set_clipboard_advertised(&mut self, on: bool) {
        self.term.da1_clipboard = on;
    }

    /// Whether lines leaving the top of a scroll region that starts at the top but stops above
    /// the bottom row (an app's status line, like tmux's) go to scrollback (iTerm2's "Save lines
    /// to scrollback when an app status bar is present").
    pub fn set_scrollback_with_status_bar(&mut self, on: bool) {
        self.term.set_scrollback_with_region(on);
    }

    /// The Advanced emulation rules (`emu::Rules`): the Term's own knobs follow them. The tab
    /// width is iTerm2's "for new sessions": it resets every stop, so it is taken only while
    /// the screen has fed no byte.
    pub fn set_rules(&mut self, rules: crate::emu::Rules) {
        if !self.fed {
            self.term.set_tab_width(rules.tab_width);
        }
        self.term.xterm_version = rules.xterm_version;
        self.term.color_report_16 = rules.color_16;
        self.term.focus_reporting_allowed = rules.focus_reporting;
        self.term.max_url_len = rules.max_url;
        self.term.save_screen_on_clear = rules.save_on_clear;
        self.term.tabs_wrap = rules.tabs_wrap;
        self.rules = rules;
        self.push_width_rule();
    }

    /// The profile's Answerback String (iTerm2 sends it, in the session's encoding, for ENQ).
    pub fn set_answerback(&mut self, mut text: String) {
        if text.len() > ANSWERBACK_MAX {
            let mut cut = ANSWERBACK_MAX;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
        }
        self.answerback = text;
    }

    /// The code page the answerback leaves in (0 is UTF-8); the pane sets it with its encoding.
    pub fn set_answerback_encoding(&mut self, cp: u32) {
        self.answerback_cp = cp;
    }

    /// DEC mode 2048 (in-band resize reports) is set.
    pub fn resize_reports(&self) -> bool {
        self.term.resize_reports
    }

    /// Sets or clears DEC mode 2048 (a host change turns it off, the stuck-report offer too).
    pub fn set_resize_reports(&mut self, on: bool) {
        self.term.resize_reports = on;
    }

    pub fn rules(&self) -> crate::emu::Rules {
        self.rules
    }

    /// The profile's Terminal Type, what XTGETTCAP `TN` and `name` report.
    pub fn set_term_name(&mut self, name: &str) {
        self.term_name = name.to_string();
    }

    pub fn term_name(&self) -> &str {
        &self.term_name
    }

    /// Whether the profile lets a program turn DECSET 1007 on (iTerm2's Allow Alternate Mouse
    /// Scroll); turning it off again is always allowed.
    pub fn set_alternate_scroll_allowed(&mut self, on: bool) {
        self.term.alternate_scroll_allowed = on;
    }

    /// Turns DECSET 1007 off (an initial prompt mark, `VT100Terminal.m:4944`).
    pub fn clear_alternate_scroll(&mut self) {
        self.term.clear_alternate_scroll();
    }

    /// Whether DECSET 1007 (alternate scroll) is on.
    pub fn alternate_scroll_mode(&self) -> bool {
        self.term.mode().contains(alacritty_terminal::term::TermMode::ALTERNATE_SCROLL)
    }

    pub(crate) fn capture_hold_clear(&mut self) {
        self.capture_hold.clear();
    }

    /// `CopyToClipboard`: output bytes join the capture, up to the cap.
    fn capture_bytes(&mut self, bytes: &[u8]) {
        let cap = self.limits.copy;
        if let Some(c) = &mut self.capture {
            let room = cap.saturating_sub(c.len());
            c.extend_from_slice(&bytes[..bytes.len().min(room)]);
        }
    }

    /// Replies owed outside `feed` (a color scheme report after a profile change).
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    /// The emulation level (100 to 500), which device attributes report.
    pub fn level(&self) -> u16 {
        self.term.emulation_level
    }

    pub fn set_level(&mut self, level: u16) {
        self.term.emulation_level = level.clamp(100, 500);
    }

    /// Flips one kitty flag from the menu.
    pub fn toggle_kitty_flag(&mut self, flag: u8) {
        if self.keyboard().kitty & flag != 0 {
            self.keys.kitty_on &= !flag;
            self.keys.kitty_off |= flag;
        } else {
            self.keys.kitty_off &= !flag;
            self.keys.kitty_on |= flag;
        }
    }

    /// The rows folds keep.
    /// Rows kept off the grid: folded lines, and the lines portholes stand in for.
    pub(crate) fn folded_rows(&self) -> impl Iterator<Item = &Row<Cell>> {
        self.folds.values().flatten().chain(self.portholes.values().flat_map(|p| &p.saved))
    }

    /// The cell size in pixels, as the app set it.
    pub fn cell_px(&self) -> (u16, u16) {
        self.cell_px
    }

    /// Fixes the clock that stamps written rows (seconds), for tests; None uses the time.
    pub fn set_clock(&mut self, secs: Option<u32>) {
        self.clock = secs;
    }

    /// When a row (grid line) was last written, in Unix seconds; 0 for never.
    pub fn row_time(&self, line: i32) -> u32 {
        let grid = self.term.grid();
        if line < -(grid.history_size() as i32) || line >= grid.screen_lines() as i32 {
            return 0;
        }
        grid[Line(line)].time()
    }

    /// Replaces the main screen's rows `first..=last` (grid lines) with `with`, returning
    /// the removed rows, top first. Rows above slide down when fewer come back.
    pub fn splice_rows(&mut self, first: i32, last: i32, with: Vec<Row<Cell>>) -> Vec<Row<Cell>> {
        // Edits go to the main screen, whose rows a full-screen app's screen hides.
        if self.alt_screen() {
            return Vec::new();
        }
        self.bump_ticks();
        self.term.selection = None;
        let removed = self.term.primary_grid_mut().splice(Line(first), Line(last), with);
        // Rows moved: triggers go on from the cursor.
        self.triggers_from_cursor();
        self.composer_rows_moved();
        removed
    }

    /// Rows holding `lines` as plain text; a line longer than the width wraps onto more rows.
    pub fn text_rows(&self, lines: &[String]) -> Vec<Row<Cell>> {
        use alacritty_terminal::term::cell::Flags;
        use unicode_width::UnicodeWidthChar;
        let cols = self.term.columns();
        let mut out = Vec::new();
        for l in lines {
            let mut row: Row<Cell> = Row::new(cols);
            let mut c = 0;
            for ch in l.chars() {
                let w = ch.width().unwrap_or(0);
                if w == 0 || w > cols {
                    continue;
                }
                if c + w > cols {
                    row[Column(cols - 1)].flags.insert(Flags::WRAPLINE);
                    out.push(std::mem::replace(&mut row, Row::new(cols)));
                    c = 0;
                }
                row[Column(c)].c = ch;
                if w == 2 {
                    row[Column(c)].flags.insert(Flags::WIDE_CHAR);
                    row[Column(c + 1)].flags.insert(Flags::WIDE_CHAR_SPACER);
                }
                c += w;
            }
            out.push(row);
        }
        out
    }

    /// Folds grid lines `first..=last` of the main screen into one summary line. None
    /// when the alternate screen is up or the range holds the cursor.
    pub fn fold(&mut self, first: i32, last: i32) -> Option<u32> {
        self.fold_as(first, last, None)
    }

    /// `fold` with a caption of its own for the summary line (Fold All's).
    pub fn fold_as(&mut self, first: i32, last: i32, caption: Option<String>) -> Option<u32> {
        use alacritty_terminal::term::cell::Flags;
        let grid = self.term.primary_grid();
        let (top, bottom) = (-(grid.history_size() as i32), grid.screen_lines() as i32 - 1);
        let cursor = grid.cursor.point.line.0;
        if self.term.mode().contains(TermMode::ALT_SCREEN) || first > last || first < top || last > bottom || (first..=last).contains(&cursor) {
            return None;
        }
        let text = |l: i32| (0..grid.columns()).map(|c| grid[Line(l)][Column(c)].c).collect::<String>();
        let summary = caption.unwrap_or_else(|| crate::fold::summary(&text(first), &text(last), (last - first + 1) as usize, grid.columns()));
        self.next_fold += 1;
        let id = self.next_fold;
        let mut rows = self.text_rows(&[summary]);
        rows.truncate(1);
        for row in rows.iter_mut() {
            for c in 0..row.len() {
                row[Column(c)].flags.insert(Flags::DIM | Flags::ITALIC);
            }
            row.set_fold(id);
        }
        let removed = self.splice_rows(first, last, rows);
        self.folds.insert(id, removed);
        Some(id)
    }

    /// Puts a fold's lines back. False when no row stands for it.
    pub fn unfold(&mut self, id: u32) -> bool {
        use alacritty_terminal::term::cell::Flags;
        if self.alt_screen() {
            return false;
        }
        let grid = self.term.primary_grid();
        let (top, bottom) = (-(grid.history_size() as i32), grid.screen_lines() as i32 - 1);
        let Some(line) = (top..=bottom).find(|l| grid[Line(*l)].fold() == id) else { return false };
        // A narrower pane may have wrapped the summary line onto more rows.
        let mut last = line;
        while last < bottom && grid[Line(last)][Column(grid.columns() - 1)].flags.contains(Flags::WRAPLINE) {
            last += 1;
        }
        let Some(rows) = self.folds.remove(&id) else { return false };
        self.splice_rows(line, last, rows);
        true
    }

    /// How many folds keep lines.
    pub fn fold_count(&self) -> usize {
        self.folds.len()
    }

    /// Drops folds and notes whose rows left the buffer (cleared, or trimmed off the top of
    /// the scrollback), so their saved lines do not stay in memory.
    pub fn prune_tags(&mut self) {
        self.bump_ticks();
        let grid = self.term.primary_grid();
        let top = -(grid.history_size() as i32);
        let mut folds = std::collections::HashSet::new();
        let mut notes = std::collections::HashSet::new();
        for l in top..grid.screen_lines() as i32 {
            let row = &grid[Line(l)];
            folds.insert(row.fold());
            notes.insert(row.note());
        }
        // Rows a porthole keeps count too (a fold or note it covers comes back on close).
        for row in self.portholes.values().flat_map(|p| &p.saved) {
            folds.insert(row.fold());
            notes.insert(row.note());
        }
        self.folds.retain(|id, _| folds.contains(id));
        self.notes.by_id.retain(|_, n| notes.contains(&n.row_key));
        self.prune_portholes();
        self.prune_images();
    }

    /// Whether the cursor blinks now: the profile's blink, or a program's where it may change it.
    pub fn cursor_blinks(&self) -> bool {
        self.term.cursor_style().blinking
    }

    /// Whether any visible cell carries the blink attribute (iTerm2's
    /// `textViewWillNeedUpdateForBlink`): the blink clock runs while it does.
    pub fn has_blinking_text(&self) -> bool {
        let grid = self.term.grid();
        let offset = grid.display_offset() as i32;
        (0..grid.screen_lines() as i32).any(|l| {
            let row = &grid[Line(l - offset)];
            (0..grid.columns()).any(|c| row[Column(c)].flags.contains(alacritty_terminal::term::cell::Flags::BLINK))
        })
    }

    /// Each visible row's first and last cell background, None where it is the default
    /// (`iTermTextDrawingHelper.m:619-647`): the colors the margins of an alternate screen take.
    pub fn edge_backgrounds(&self) -> Vec<(Option<Rgb>, Option<Rgb>)> {
        use alacritty_terminal::index::Point;
        let grid = self.term.grid();
        let (cols, lines, offset) = (self.term.columns(), self.term.screen_lines(), grid.display_offset() as i32);
        let edge = |row: usize, col: usize| {
            let c = &grid[Point::new(Line(row as i32 - offset), Column(col))];
            let bg = self.palette.cell(c.fg, c.bg, c.flags).1;
            (bg != self.palette.bg).then_some(bg)
        };
        (0..lines).map(|row| (edge(row, 0), edge(row, cols.saturating_sub(1)))).collect()
    }

    pub(crate) fn alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// The fold a row stands for.
    pub fn fold_at(&self, line: i32) -> Option<u32> {
        if self.alt_screen() {
            return None;
        }
        let grid = self.term.primary_grid();
        if line < -(grid.history_size() as i32) || line >= grid.screen_lines() as i32 {
            return None;
        }
        Some(grid[Line(line)].fold()).filter(|id| *id != 0)
    }

    pub(crate) fn feed_raw(&mut self, bytes: &[u8]) -> Vec<u8> {
        // A print job collects the text and drops the sequences; what follows its end is parsed.
        let rest;
        let bytes = if self.print_capture.is_some() {
            match self.divert_to_print(bytes) {
                Some(r) => {
                    rest = r;
                    &rest[..]
                }
                None => return Vec::new(),
            }
        } else {
            bytes
        };
        self.bump_ticks();
        self.term.stamp = self.clock.unwrap_or_else(|| {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32)
        });
        self.parser.advance(&mut self.term, bytes);
        // RIS stops the progress bar (`terminalDidReset`).
        if self.term.resets != self.resets_seen {
            self.resets_seen = self.term.resets;
            self.set_progress(crate::progress::Progress::Stopped);
        }
        let splices = self.term.take_history_splices();
        let mut after: usize = splices.iter().map(|(_, by)| *by).sum();
        for (below, by) in splices {
            // The API numbering's base before this splice: what is above the screen now less the
            // rows this and later splices added (main output between splices is not counted).
            let api_from = crate::api_buffer::above(self.term.primary_grid()) - after as i64;
            after -= by;
            self.rebase_history_rows(below, by, api_from);
        }
        // DECTCEM: the time the cursor went away (the first, while it stays away).
        match (self.term.mode().contains(TermMode::SHOW_CURSOR), self.cursor_hidden_at) {
            (true, Some(_)) => self.cursor_hidden_at = None,
            (false, None) => self.cursor_hidden_at = Some(std::time::Instant::now()),
            _ => {}
        }
        // The alternate screen turns the prompt state machine off (`MS:839-845`).
        let alt = self.term.mode().contains(TermMode::ALT_SCREEN);
        if alt != self.composer_alt {
            self.composer_alt = alt;
            let events = self.prompt_state.set_allowed(!alt);
            self.composer_events.extend(events);
            // Back on the main screen: rows may have moved meanwhile (a resize), and the
            // checkpoint goes to the main screen's cursor.
            if !alt {
                self.composer_rows_moved();
            }
        }
        // Now and then, forget folds and notes whose rows scrolled out of the buffer.
        self.feeds = self.feeds.wrapping_add(1);
        self.fed = true;
        if self.feeds % 256 == 0 && (!self.folds.is_empty() || !self.notes.by_id.is_empty() || !self.images.by_id.is_empty() || !self.portholes.is_empty()) {
            self.prune_tags();
        }
        self.drain_events()
    }

    /// The replies the terminal's events ask for, and the title changes they carry.
    fn drain_events(&mut self) -> Vec<u8> {
        let mut reply = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::PtyWrite(text) => reply.extend_from_slice(text.as_bytes()),
                Event::ColorRequest(index, answer) => {
                    let color = self.term.colors()[index].unwrap_or_else(|| match index {
                        0..=255 => self.palette.indexed(index as u8),
                        257 => self.palette.bg,
                        258 => self.palette.cursor,
                        _ => self.palette.fg,
                    });
                    reply.extend_from_slice(answer(color).as_bytes());
                }
                Event::TextAreaSizeRequest(answer) => {
                    let size = WindowSize {
                        num_lines: self.term.screen_lines() as u16,
                        num_cols: self.term.columns() as u16,
                        cell_width: self.cell_px.0,
                        cell_height: self.cell_px.1,
                    };
                    reply.extend_from_slice(answer(size).as_bytes());
                }
                // Allow Title Setting gates programs' titles (`PS:19364-19371`); an applied
                // one may go to the window's other sessions.
                Event::Title(title) if self.allow_title => {
                    self.title = Some(title.clone());
                    self.requests.push(Request::WindowTitle(Some(title)));
                }
                Event::ResetTitle if self.allow_title => {
                    self.title = None;
                    self.requests.push(Request::WindowTitle(None));
                }
                Event::Bell => {
                    self.bell_count += 1;
                    self.requests.push(Request::Bell);
                    self.fire_event(crate::trigger_events::EventKind::BellReceived, &Default::default(), &[]);
                }
                _ => {}
            }
        }
        reply
    }

    /// Puts a mark id on a row (grid line: 0 is the top of the screen, negative is history).
    pub fn set_row_mark(&mut self, line: i32, id: u32) {
        self.bump_ticks();
        let grid = self.term.primary_grid_mut();
        if line >= -(grid.history_size() as i32) && line < grid.screen_lines() as i32 {
            grid[Line(line)].set_mark(id);
        }
    }

    /// Puts a manual mark id on a row, beside any prompt mark there.
    pub fn set_row_manual(&mut self, line: i32, id: u32) {
        self.bump_ticks();
        let grid = self.term.primary_grid_mut();
        if line >= -(grid.history_size() as i32) && line < grid.screen_lines() as i32 {
            grid[Line(line)].set_manual(id);
        }
    }

    /// Every mark on a row, top first: (grid line, mark id). A row's prompt mark comes
    /// before its manual mark.
    pub fn row_marks(&self) -> Vec<(i32, u32)> {
        // Marks live on the main screen, also while a full-screen app has the alternate one.
        let grid = self.term.primary_grid();
        let top = -(grid.history_size() as i32);
        (top..grid.screen_lines() as i32)
            .flat_map(|l| {
                let row = &grid[Line(l)];
                [(l, row.mark()), (l, row.manual())]
            })
            .filter(|(_, id)| *id != 0)
            .collect()
    }

    /// What the scrollbar's mark and note ticks are keyed on: the grid generation (bumped by
    /// every output batch, reset, clear, resize, fold and mark or note edit, so a row erased
    /// or moved in place is seen too) and the screen in view.
    pub fn tick_digest(&self) -> u64 {
        self.tick_gen ^ (u64::from(self.term.mode().contains(TermMode::ALT_SCREEN)) << 63)
    }

    /// `text` in the profile's normalization form.
    pub fn normalized<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        crate::encoding::normalize(text, self.normalization)
    }

    /// The selected text as it leaves the grid (Copy, find, the API): normalized.
    pub fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string().map(|mut t| {
            crate::encoding::normalize_in_place(&mut t, self.normalization);
            t
        })
    }

    /// Notes that the main grid's rows, marks or notes may have changed.
    pub(crate) fn bump_ticks(&mut self) {
        self.tick_gen = (self.tick_gen.wrapping_add(1)) & !(1 << 63);
    }

    /// A row's base characters as text, each cell in the profile's form (never across cells).
    fn push_row_text(&self, out: &mut String, row: &Row<Cell>) {
        for c in 0..self.term.columns() {
            let ch = row[Column(c)].c;
            if self.normalization == crate::encoding::Form::None || ch.is_ascii() {
                out.push(ch);
            } else {
                let mut one = ch.to_string();
                crate::encoding::normalize_in_place(&mut one, self.normalization);
                out.push_str(&one);
            }
        }
    }

    /// One logical line's text starting at `line`: its row and the rows it wraps onto,
    /// trailing spaces trimmed.
    pub fn line_text(&self, line: i32) -> String {
        let grid = self.term.grid();
        let mut out = String::new();
        let mut l = line;
        while l < grid.screen_lines() as i32 {
            let row = &grid[Line(l)];
            self.push_row_text(&mut out, row);
            let wrapped = row[Column(grid.columns() - 1)].flags.contains(alacritty_terminal::term::cell::Flags::WRAPLINE);
            if !wrapped {
                break;
            }
            l += 1;
        }
        out.trim_end().to_string()
    }

    /// The whole buffer as text, oldest line first, trailing spaces trimmed.
    pub fn all_text(&self) -> String {
        let grid = self.term.grid();
        let top = -(grid.history_size() as i32);
        let mut out = String::new();
        for line in top..grid.screen_lines() as i32 {
            let row = &grid[Line(line)];
            let mut text = String::new();
            self.push_row_text(&mut text, row);
            out.push_str(text.trim_end());
            out.push('\n');
        }
        let trimmed = out.trim_end_matches('\n').len();
        out.truncate(trimmed);
        out.push('\n');
        out
    }

    /// iTerm2's Clear Scrollback Buffer (`scrollback_only`) or Clear Buffer (everything but
    /// the cursor line, which moves to the top, as iTerm2 keeps the prompt).
    pub fn clear(&mut self, scrollback_only: bool) {
        self.bump_ticks();
        // Scrolling and clearing push lines into scrollback, so the screen goes first,
        // then the scrollback.
        if !scrollback_only {
            let cursor = self.term.grid().cursor.point;
            if cursor.line.0 > 0 {
                self.term.scroll_up(cursor.line.0 as usize);
            }
            if self.term.screen_lines() > 1 {
                self.term.goto(1, 0);
                self.term.clear_screen(ClearMode::Below);
            }
            self.term.goto(0, cursor.column.0);
        }
        self.term.clear_screen(ClearMode::Saved);
        self.prune_tags();
        self.prune_marks();
    }

    /// iTerm2's Reset (a soft reset, DECSTR): modes, attributes, margins, and charset go back
    /// to their defaults and the text stays. alacritty has no DECSTR, so the equivalent
    /// sequences are fed in.
    pub fn soft_reset(&mut self) {
        // The person's Reset ends a print job that is collecting; it must reach the parser.
        self.print_capture = None;
        const DECSTR: &[u8] = b"\x1b[?25h\x1b[?1l\x1b>\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l\
            \x1b[?2004l\x1b[?1004l\x1b[?1007l\x1b[4l\x1b[0m\x1b[r\x1b[?6l\x1b[?7h\x1b[?2048l\x1b(B";
        // Straight to the parser: Literal Controls would print the sequence instead.
        // DECSTBM (the `r`) homes the cursor; a soft reset leaves it where it was.
        let c = self.term.grid().cursor.point;
        let reply = self.feed_raw(DECSTR);
        self.replies.extend(reply);
        let _ = self.feed_raw(format!("\x1b[{};{}H", c.line.0.max(0) + 1, c.column.0 + 1).as_bytes());
        // The ESC in it cancelled any sequence the program left open; the scanner follows (it
        // reads the program's bytes only, so it would still be inside that sequence).
        self.restart_scanner();
        // Key reporting goes back to the profile's: no program flags, no menu choices.
        self.term.clear_keyboard_modes();
        self.keys = KeyState { literal: self.keys.literal, ..KeyState::new(self.keys.profile_csi_u) };
    }

    /// The scanner leaves a string the program left open (an OSC or DCS cut short by its end).
    pub(crate) fn restart_scanner(&mut self) {
        if self.osc.pending.is_some() {
            self.osc.restart();
        }
    }

    /// The program on this screen has ended (an exit, before a restart): everything bound to it
    /// goes (held output, an inline file's question, captures, transfers in progress, drop
    /// registrations, status, host and user the shell reported, an open command mark) and the
    /// terminal leaves what it left on (13q B3, B9): the alternate screen, SGR, charset, an
    /// open hyperlink, the scroll region, origin mode, wrap, insert mode, key reporting, and a
    /// string the scanner was inside. The text, scrollback, marks and images stay.
    pub fn program_ended(&mut self) {
        self.held_output.clear();
        self.inline_hold = None;
        self.multipart = None;
        self.capture = None;
        self.capture_hold.clear();
        self.print_capture = None;
        self.copy = Default::default();
        self.dnd = Default::default();
        self.kitty.pending = None;
        self.tab_status = Default::default();
        // A dead program's OSC 9;4 bar goes with it (13p seam S8, P21), through the one setter so
        // the window redraws.
        self.set_progress(crate::progress::Progress::Stopped);
        self.unicode = Default::default();
        self.exec_failed = false;
        self.color_sets.clear();
        self.key_labels = Default::default();
        self.shell.host = None;
        self.shell.user = None;
        self.shell.host_local = None;
        self.marks.program_ended();
        self.restart_scanner();
        // The reset reaches the parser itself (the answers to it are not wanted).
        let _ = self.feed_raw(b"\x1b[?1049l");
        let (row, col) = {
            let c = self.term.grid().cursor.point;
            (c.line.0.max(0) + 1, c.column.0 + 1)
        };
        // DECSTBM homes the cursor, so it is put back: the text stays where it was.
        let _ = self.feed_raw(format!("{END_RESET}\x1b[{row};{col}H").as_bytes());
        // The parser has no mode 2031; the scanner's own flag goes off here.
        self.set_color_scheme_reports(false);
        self.restart_scanner();
        self.term.clear_keyboard_modes();
        self.keys = KeyState { literal: self.keys.literal, ..KeyState::new(self.keys.profile_csi_u) };
    }

    /// What an SSH conductor restores when it unhooks (iTerm2's savedState, Decision: the
    /// remote host and the working directory).
    pub fn ssh_saved(&self) -> crate::conductor::SavedState {
        crate::conductor::SavedState { cwd: self.shell.cwd.clone(), host: self.shell.host.clone(), user: self.shell.user.clone() }
    }

    /// restoreSavedState (`PS:20183-20185`) for what `ssh_saved` kept.
    pub fn restore_ssh_saved(&mut self, s: &crate::conductor::SavedState) {
        self.shell.cwd = s.cwd.clone();
        self.shell.host = s.host.clone();
        self.shell.user = s.user.clone();
    }

    pub fn mouse_modes(&self) -> Modes {
        let mode = self.term.mode();
        let tracking = if mode.contains(TermMode::MOUSE_MOTION) {
            Tracking::Any
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            Tracking::Drag
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            Tracking::Click
        } else {
            Tracking::Off
        };
        Modes { tracking, sgr: mode.contains(TermMode::SGR_MOUSE) }
    }

    pub fn text(&self) -> String {
        let grid = self.term.grid();
        let mut out = String::new();
        for row in 0..grid.screen_lines() {
            for col in 0..grid.columns() {
                out.push(grid[Line(row as i32)][Column(col)].c);
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The margin colors of an alternate screen come from each row's edge cells.
    #[test]
    fn edge_backgrounds_are_the_rows_first_and_last_non_default_cells() {
        let mut s = Screen::new(10, 4);
        assert_eq!(s.edge_backgrounds(), vec![(None, None); 4], "a fresh screen is all default");
        let red = s.palette.color(alacritty_terminal::vte::ansi::Color::Indexed(1));
        let blue = s.palette.color(alacritty_terminal::vte::ansi::Color::Indexed(4));
        s.feed(b"\x1b[?1049h\x1b[44m\x1b[2J\x1b[2;1H\x1b[41mX\x1b[0m\x1b[3;1H\x1b[0mY");
        let e = s.edge_backgrounds();
        assert_eq!(e.len(), 4);
        assert_eq!(e[0], (Some(blue), Some(blue)));
        assert_eq!(e[1], (Some(red), Some(blue)));
        assert_eq!(e[2], (None, Some(blue)), "a default first cell is None");
    }

    #[test]
    fn blink_flag_follows_sgr() {
        let mut s = Screen::new(10, 3);
        s.feed(b"\x1b[5ma\x1b[25mb\x1b[6mc\x1b[0md");
        let blinks: Vec<bool> = (0..4).map(|c| s.term.grid()[Line(0)][Column(c)].flags.contains(alacritty_terminal::term::cell::Flags::BLINK)).collect();
        assert_eq!(blinks, [true, false, true, false]);
        // RIS clears the template: "b" after it is not flagged.
        let mut s = Screen::new(10, 3);
        s.feed(b"\x1b[5ma\x1bcb");
        let b = &s.term.grid()[Line(0)][Column(0)];
        assert_eq!(b.c, 'b');
        assert!(!b.flags.contains(alacritty_terminal::term::cell::Flags::BLINK));
    }

    #[test]
    fn has_blinking_text_sees_visible_cells_only() {
        let mut s = Screen::new(10, 2);
        assert!(!s.has_blinking_text());
        s.feed(b"\x1b[5mx");
        assert!(s.has_blinking_text());
        // Two more lines push the row off the screen into history: nothing visible blinks.
        s.feed(b"\x1b[0m\r\ny\r\nz");
        assert!(!s.has_blinking_text());
    }

    thread_local! {
        /// Test seam: every row's cache key becomes this one, as a found hash collision would.
        pub(super) static FORCE_KEY: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    }

    #[test]
    fn a_hash_collision_never_shows_another_rows_order() {
        let mut s = Screen::new(20, 4);
        s.feed("ab \u{5d0}\u{5d1}\r\n\u{5d0}\u{5d1}\u{5d2}  ".as_bytes());
        FORCE_KEY.with(|f| f.set(Some(7)));
        let a = s.row_order(0).unwrap();
        let b = s.row_order(1).unwrap();
        FORCE_KEY.with(|f| f.set(None));
        assert_eq!(&a.lut[..5], &[0, 1, 2, 4, 3]);
        assert_eq!(&b.lut[..3], &[2, 1, 0], "the second row gets its own order, not the first's");
    }

    #[test]
    fn row_orders_are_cached_by_content() {
        use std::sync::Arc;
        let mut s = Screen::new(20, 4);
        s.feed("ab \u{5d0}\u{5d1}\r\nplain".as_bytes());
        let a = s.row_order(0).expect("a Hebrew row has an order");
        assert_eq!(&a.lut[..5], &[0, 1, 2, 4, 3]);
        assert!(Arc::ptr_eq(&a, &s.row_order(0).unwrap()), "same content, same order");
        assert!(s.row_order(1).is_none(), "an English row has none");
        // Rewriting the row changes its order.
        s.feed("\x1b[1;1H\u{5d0}\u{5d1}\u{5d2}  ".as_bytes());
        let b = s.row_order(0).unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(&b.lut[..3], &[2, 1, 0]);
        // The rules change the answer: off, and off in the alternate screen only.
        s.bidi_rules.enabled = false;
        assert!(s.row_order(0).is_none());
        s.bidi_rules = crate::bidi::Rules { off_in_alternate: true, ..crate::bidi::Rules::default() };
        assert!(s.row_order(0).is_some());
        s.feed(b"\x1b[?1049h");
        s.feed("\u{5d0}\u{5d1}".as_bytes());
        assert!(s.row_order(0).is_none(), "the alternate screen is left alone with the setting");
    }

    #[test]
    fn right_justify_pads_a_detected_rtl_row() {
        let mut s = Screen::new(10, 2);
        s.bidi_rules = crate::bidi::Rules { detect_direction: true, ..crate::bidi::Rules::default() };
        s.feed("\u{5d0}\u{5d1}".as_bytes());
        let o = s.row_order(0).unwrap();
        assert!(o.paragraph_rtl);
        assert_eq!(o.lut.len(), 10, "padded to the pane's width");
        assert_eq!(&o.lut[..2], &[9, 8], "the text sits at the right edge");
        s.bidi_rules.right_justify = false;
        let o = s.row_order(0).unwrap();
        assert_eq!(o.lut.len(), 2);
    }

    #[test]
    fn a_drag_selects_the_cells_the_pointer_covered() {
        use alacritty_terminal::index::{Column, Line, Point, Side};
        use alacritty_terminal::selection::{Selection, SelectionType};
        let mut s = Screen::new(10, 3);
        // Logical "ab " + aleph (3) + bet (4); drawn as visual a b _ bet aleph.
        s.feed("ab \u{5d0}\u{5d1}\r\nplain".as_bytes());
        let p = |col: usize| Point::new(Line(0), Column(col));
        s.term_mut().selection = Some(Selection::new(SelectionType::Simple, p(0), Side::Left));
        // From the left of visual 0 to the right half of visual 3 (the bet).
        assert!(s.drag_visual_selection((0, 0, false), (0, 3, true)));
        assert_eq!(s.term().selection_to_string().as_deref(), Some("ab \u{5d1}"), "the aleph, which the pointer never reached, is left out");
        let sel = s.term().selection.as_ref().unwrap();
        let covered: Vec<usize> = (0..6).filter(|c| sel.covers(p(*c))).collect();
        assert_eq!(covered, vec![0, 1, 2, 4], "the highlight is the same cells");
        // The same drag backwards (end before the anchor) covers the same cells.
        s.term_mut().selection = Some(Selection::new(SelectionType::Simple, p(4), Side::Right));
        assert!(s.drag_visual_selection((0, 3, true), (0, 0, false)));
        assert_eq!(s.term().selection_to_string().as_deref(), Some("ab \u{5d1}"));
        // Across two rows: the rest of the first row in reading order, then the second's start.
        s.term_mut().selection = Some(Selection::new(SelectionType::Simple, p(0), Side::Left));
        assert!(s.drag_visual_selection((0, 3, false), (1, 1, true)));
        assert_eq!(s.term().selection_to_string().as_deref(), Some("\u{5d0}\u{5d1}\npl"), "visual 3 to 9 of row 0, copied in logical order");
        // A drag over rows with no right-to-left text is left to the logical selection.
        let mut t = Screen::new(10, 3);
        t.feed(b"abc\r\ndef");
        assert!(!t.drag_visual_selection((0, 0, false), (1, 2, true)));
    }

    #[test]
    fn output_scrolling_during_a_drag_keeps_the_anchor_and_the_runs() {
        use alacritty_terminal::index::{Column, Line, Point, Side};
        use alacritty_terminal::selection::{Selection, SelectionType};
        let mut s = Screen::new(10, 3);
        s.feed("ab \u{5d0}\u{5d1}\r\nx\r\ny".as_bytes());
        s.term_mut().selection = Some(Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left));
        s.term_mut().selection.as_mut().unwrap().set_visual_anchor((0, 0, false));
        assert!(s.drag_visual_to((0, 3, true)));
        let want = Some("ab \u{5d1}");
        assert_eq!(s.term().selection_to_string().as_deref(), want);
        // Two lines of output scroll the row into history.
        s.feed(b"\r\nz\r\nw");
        assert_eq!(s.term().selection_to_string().as_deref(), want, "scrolling alone copies only the swept cells");
        let sel = s.term().selection.as_ref().unwrap();
        assert_eq!((0..5).filter(|c| sel.covers(Point::new(Line(-2), Column(*c)))).collect::<Vec<_>>(), vec![0, 1, 2, 4], "and highlights them");
        assert_eq!(s.visual_anchor(), Some((-2, 0, false)), "the anchor stays on the same text");
        // The next move sweeps from the same text, not from whatever now sits on grid line 0.
        assert!(s.drag_visual_to((-2, 4, true)));
        assert_eq!(s.term().selection_to_string().as_deref(), Some("ab \u{5d0}\u{5d1}"));
        assert_eq!(s.term().selection.as_ref().unwrap().covers(Point::new(Line(0), Column(0))), false);
    }

    #[test]
    fn visual_runs_leave_with_the_oldest_line() {
        use alacritty_terminal::index::{Column, Line, Point, Side};
        use alacritty_terminal::selection::{Selection, SelectionType};
        let drag = |s: &mut Screen, to: (i32, usize, bool)| {
            s.term_mut().selection = Some(Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left));
            s.term_mut().selection.as_mut().unwrap().set_visual_anchor((0, 0, false));
            assert!(s.drag_visual_to(to));
        };
        // The alternate screen without scrollback keeps no history: a row scrolled off its top is gone, and so
        // are the runs and the anchor on it. Nothing indexes a line that does not exist.
        let mut s = Screen::with_look(10, 3, Palette::default(), Look { alt_scrollback: false, ..Look::default() });
        s.feed(b"\x1b[?1049h");
        s.feed("ab \u{5d0}\u{5d1}".as_bytes());
        drag(&mut s, (0, 3, true));
        assert_eq!(s.term().selection_to_string().as_deref(), Some("ab \u{5d1}"));
        s.feed(b"\r\n\r\n\r\n\r\n");
        assert_eq!(s.term().selection_to_string(), None, "no row is copied that nobody selected");
        assert!(s.term().selection.is_none());
        assert_eq!(s.visual_anchor(), None);
        // A full scrollback drops its oldest line on every scroll: of two selected rows only
        // the one still in the buffer is copied and highlighted.
        let mut s = Screen::with_scrollback(10, 3, 1);
        s.feed("ab \u{5d0}\u{5d1}\r\nplain".as_bytes());
        drag(&mut s, (1, 2, true));
        s.feed(b"\r\n\r\n\r\n");
        let sel = s.term().selection.as_ref().expect("row 1 is still in the buffer");
        assert!(sel.runs.iter().all(|(a, _)| a.line.0 >= -1), "{:?}", sel.runs);
        assert!(!sel.covers(Point::new(Line(2), Column(0))), "the newest rows are not highlighted");
        let copy = s.term().selection_to_string().unwrap();
        assert!(copy.starts_with("pl") || copy.starts_with("pla"), "{copy:?}");
        assert!(!copy.contains('\u{5d0}'), "{copy:?}");
        let (line, _, _) = s.visual_anchor().unwrap_or((-1, 0, false));
        assert!(line >= -1, "the anchor never leaves the buffer: {line}");
    }

    /// 13o Task 4: with alternate scrollback the oldest-line rules of patch 19 hold on the
    /// alternate grid: a full history drops its oldest row, and the runs and the anchor on it.
    #[test]
    fn visual_runs_leave_with_the_oldest_alternate_line() {
        use alacritty_terminal::index::{Column, Line, Point, Side};
        use alacritty_terminal::selection::{Selection, SelectionType};
        let look = Look { scrollback: 1, alt_scrollback: true, ..Look::default() };
        let mut s = Screen::with_look(10, 3, Palette::default(), look);
        s.feed(b"\x1b[?1049h\x1b[H");
        s.feed("ab \u{5d0}\u{5d1}\r\nplain".as_bytes());
        s.term_mut().selection = Some(Selection::new(SelectionType::Simple, Point::new(Line(0), Column(0)), Side::Left));
        s.term_mut().selection.as_mut().unwrap().set_visual_anchor((0, 0, false));
        assert!(s.drag_visual_to((1, 2, true)));
        s.feed(b"\r\n\r\n\r\n");
        let sel = s.term().selection.as_ref().expect("row 1 is still in the buffer");
        assert!(sel.runs.iter().all(|(a, _)| a.line.0 >= -1), "{:?}", sel.runs);
        let (line, _, _) = s.visual_anchor().unwrap_or((-1, 0, false));
        assert!(line >= -1, "the anchor never leaves the buffer: {line}");
        // Two more scrolls push the oldest line out of the buffer: the selection goes.
        s.feed(b"\r\n\r\n\r\n\r\n");
        assert_eq!(s.term().selection_to_string(), None);
    }

    /// Lines 0..2 of a 3-row primary screen are a scroll region and Scrollback With Status Bar
    /// is off, so a scroll moves only the region's rows: history stays, row 0 is discarded.
    fn in_place_region() -> Screen {
        let mut s = Screen::with_scrollback(10, 3, 100);
        s.set_scrollback_with_status_bar(false);
        s.feed("h2 \u{5d0}\u{5d1}\r\nh1 \u{5d2}\r\nab \u{5d0}\u{5d1}\r\nmid\r\nst \u{5d3}".as_bytes());
        s.feed(b"\x1b[1;2r");
        s
    }

    fn visual_drag(s: &mut Screen, from: (i32, usize, bool), to: (i32, usize, bool)) {
        use alacritty_terminal::index::{Column, Line, Point, Side};
        use alacritty_terminal::selection::{Selection, SelectionType};
        let start = Point::new(Line(from.0), Column(from.1));
        s.term_mut().selection = Some(Selection::new(SelectionType::Simple, start, Side::Left));
        s.term_mut().selection.as_mut().unwrap().set_visual_anchor(from);
        assert!(s.drag_visual_to(to));
    }

    fn runs(s: &Screen) -> Vec<(i32, usize, usize)> {
        let sel = s.term().selection.as_ref().expect("a selection");
        sel.runs.iter().map(|(a, b)| (a.line.0, a.column.0, b.column.0)).collect()
    }

    /// A region that scrolls in place moves only its own rows: runs and the drag anchor inside
    /// it move with them, the ones on its discarded top row go, and history and the rows below
    /// it stay put. Copy, GetSelection's runs and the highlight never reach a line nobody
    /// selected.
    #[test]
    fn visual_runs_follow_an_in_place_region_scroll() {
        // Across the region's top: a history line and region row 0. The history run stays;
        // row 0's text is discarded, so its run goes.
        let mut s = in_place_region();
        visual_drag(&mut s, (-1, 0, false), (0, 9, true));
        assert_eq!(runs(&s), vec![(-1, 0, 9), (0, 0, 9)]);
        s.feed(b"\x1b[2;1H\n");
        assert_eq!(runs(&s), vec![(-1, 0, 9)], "history does not move");
        assert_eq!(s.term().selection_to_string().as_deref().map(str::trim_end), Some("h1 \u{5d2}"));
        assert_eq!(s.visual_anchor(), Some((-1, 0, false)));
        // Inside the region: row 1 moves up to row 0; row 0's run goes, and the anchor on it
        // stops at the region's top instead of jumping into history.
        let mut s = in_place_region();
        visual_drag(&mut s, (0, 0, false), (1, 2, true));
        s.feed(b"\x1b[2;1H\n");
        assert_eq!(runs(&s), vec![(0, 0, 2)]);
        assert_eq!(s.term().selection_to_string().as_deref(), Some("mid"));
        let (line, _, _) = s.visual_anchor().expect("the drag goes on");
        assert!(line >= 0, "the anchor never jumps into history: {line}");
        // A second scroll discards that row too: nothing selected is left.
        s.feed(b"\n");
        assert!(s.term().selection.is_none());
        assert_eq!(s.term().selection_to_string(), None);
        // Across the region's bottom: the status line below it stays put.
        let mut s = in_place_region();
        visual_drag(&mut s, (1, 0, false), (2, 9, true));
        s.feed(b"\x1b[2;1H\n");
        assert_eq!(runs(&s), vec![(0, 0, 9), (2, 0, 9)]);
        assert_eq!(s.term().selection_to_string().as_deref().map(|t| t.lines().map(str::trim_end).collect::<Vec<_>>()), Some(vec!["mid", "st \u{5d3}"]));
        // Below the region only: nothing moves.
        let mut s = in_place_region();
        visual_drag(&mut s, (2, 0, false), (2, 9, true));
        s.feed(b"\x1b[2;1H\n");
        assert_eq!(runs(&s), vec![(2, 0, 9)]);
        assert_eq!(s.visual_anchor(), Some((2, 0, false)));
    }

    /// A reverse index at the top of a region scrolls only the region down (history never
    /// moves when the screen scrolls down): a selection on a history line stays on it.
    #[test]
    fn selections_on_history_stay_when_a_region_scrolls_down() {
        use alacritty_terminal::index::{Column, Line, Point, Side};
        use alacritty_terminal::selection::{Selection, SelectionType};
        let mut s = in_place_region();
        visual_drag(&mut s, (-1, 0, false), (-1, 9, true));
        s.feed(b"\x1b[1;1H\x1bM");
        assert_eq!(runs(&s), vec![(-1, 0, 9)]);
        assert_eq!(s.visual_anchor(), Some((-1, 0, false)));
        assert_eq!(s.term().selection_to_string().as_deref().map(str::trim_end), Some("h1 \u{5d2}"));
        // A logical selection on history stays too, with the region over the whole screen.
        let mut s = Screen::with_scrollback(10, 3, 100);
        s.feed(b"old\r\na\r\nb\r\nc");
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(-1), Column(0)), Side::Left);
        sel.update(Point::new(Line(-1), Column(2)), Side::Right);
        s.term_mut().selection = Some(sel);
        s.feed(b"\x1b[1;1H\x1bM");
        assert_eq!(s.term().selection_to_string().as_deref(), Some("old"));
    }

    /// Replace Selection rewrites whole lines, so a visual selection must read them all: a drag
    /// that skips the aleph inside the line is refused, one over the whole line is allowed.
    #[test]
    fn visual_selection_is_whole_lines_only_when_the_runs_cover_them() {
        use alacritty_terminal::index::{Column, Line, Point};
        let mut s = Screen::new(10, 3);
        s.feed("ab \u{5d0}\u{5d1}\r\nplain".as_bytes());
        let (a, b) = (Point::new(Line(0), Column(0)), Point::new(Line(0), Column(4)));
        // Visual 0..4 shows logical 0, 1, 2 and the bet (4): the aleph (3) is skipped.
        visual_drag(&mut s, (0, 0, false), (0, 3, true));
        assert_eq!(runs(&s), vec![(0, 0, 2), (0, 4, 4)]);
        assert!(!s.visual_selection_is_whole_lines(a, b), "lines nobody selected are not rewritten");
        visual_drag(&mut s, (0, 0, false), (0, 9, true));
        assert!(s.visual_selection_is_whole_lines(a, b));
        // A logical selection is checked by the caller's whole-line rule alone.
        s.term_mut().selection.as_mut().unwrap().update(b, alacritty_terminal::index::Side::Right);
        assert!(s.visual_selection_is_whole_lines(a, b));
    }

    /// The copy, a line per row with the padding trimmed.
    fn copied(s: &Screen) -> Option<Vec<String>> {
        s.term().selection_to_string().map(|t| t.lines().map(|l| l.trim_end().to_string()).collect())
    }

    /// Every run and the drag anchor sit on a line the buffer holds, and the highlight
    /// (`selection_spans`) is the runs.
    fn in_buffer(s: &Screen) {
        let (top, bottom) = (s.term().topmost_line().0, s.term().bottommost_line().0);
        if let Some(sel) = s.term().selection.as_ref() {
            assert!(sel.runs.iter().all(|(a, _)| a.line.0 >= top && a.line.0 <= bottom), "{:?} outside {top}..={bottom}", sel.runs);
        }
        if let Some((line, _, _)) = s.visual_anchor() {
            assert!(line >= top && line <= bottom, "anchor {line} outside {top}..={bottom}");
        }
        let spans = crate::styled_copy::selection_spans(s).0;
        assert!(spans.iter().all(|(l, _, _)| *l >= top && *l <= bottom), "{spans:?}");
    }

    /// A height-only shrink with the cursor high cuts the rows below the new bottom: runs on
    /// them go (the selection with them when nothing is left), and a later grow never
    /// highlights, copies or reports the fresh rows put there.
    #[test]
    fn a_shrink_with_the_cursor_high_drops_the_cut_rows() {
        let five = || {
            let mut s = Screen::new(10, 5);
            s.feed("r0\r\nr1\r\nr2 \u{5d0}\u{5d1}\r\nr3 \u{5d2}\u{5d3}\r\nr4 \u{5d4}\x1b[H".as_bytes());
            s
        };
        // Only the cut rows selected: nothing is left.
        let mut s = five();
        visual_drag(&mut s, (3, 0, false), (4, 9, true));
        s.resize(10, 3);
        in_buffer(&s);
        assert!(s.term().selection.is_none());
        assert_eq!(s.term().selection_to_string(), None);
        assert_eq!(s.visual_anchor(), None);
        s.resize(10, 5);
        assert!(s.term().selection.is_none(), "the fresh rows are not selected");
        assert!(crate::styled_copy::selection_spans(&s).0.is_empty());
        // Across the new bottom: row 2 stays as it was, row 3's run goes.
        let mut s = five();
        visual_drag(&mut s, (2, 0, false), (3, 9, true));
        s.resize(10, 3);
        in_buffer(&s);
        assert_eq!(runs(&s), vec![(2, 0, 9)]);
        assert_eq!(copied(&s), Some(vec!["r2 \u{5d0}\u{5d1}".to_string()]));
        assert_eq!(s.visual_anchor(), Some((2, 0, false)));
        s.resize(10, 5);
        assert_eq!(runs(&s), vec![(2, 0, 9)], "the rows grown back are not selected");
        assert_eq!(copied(&s), Some(vec!["r2 \u{5d0}\u{5d1}".to_string()]));
    }

    /// The alternate screen without scrollback keeps no history: a shrink with the cursor at the bottom pushes
    /// its top rows out, and the runs and the anchor on them go.
    #[test]
    fn a_shrink_at_the_bottom_of_the_alternate_screen_drops_its_top_rows() {
        let five = || {
            let mut s = Screen::with_look(10, 5, Palette::default(), Look { alt_scrollback: false, ..Look::default() });
            s.feed(b"\x1b[?1049h");
            s.feed("a0 \u{5d0}\r\na1 \u{5d1}\r\na2 \u{5d2}\r\na3\r\na4".as_bytes());
            s
        };
        let mut s = five();
        visual_drag(&mut s, (0, 0, false), (1, 9, true));
        s.resize(10, 3);
        in_buffer(&s);
        assert!(s.term().selection.is_none());
        assert_eq!(s.term().selection_to_string(), None);
        // Across the top: row 2 moves up to row 0 and stays selected; the anchor's row is gone.
        let mut s = five();
        visual_drag(&mut s, (1, 0, false), (2, 9, true));
        s.resize(10, 3);
        in_buffer(&s);
        assert_eq!(runs(&s), vec![(0, 0, 9)]);
        assert_eq!(copied(&s), Some(vec!["a2 \u{5d2}".to_string()]));
        assert_eq!(s.visual_anchor(), None, "the press row is gone");
        assert!(!s.drag_visual_to((0, 4, true)), "no drag goes on from a row that is gone");
    }

    /// A full scrollback loses its oldest lines when a shrink at the bottom pushes rows into
    /// it: runs and the anchor on them go, the rest move up with their text.
    #[test]
    fn a_shrink_with_a_full_scrollback_drops_the_oldest_lines() {
        let filled = || {
            let mut s = Screen::with_scrollback(10, 5, 1);
            s.feed("h0\r\ns0 \u{5d0}\r\ns1 \u{5d1}\r\ns2\r\ns3\r\ns4".as_bytes());
            assert_eq!(s.term().topmost_line().0, -1);
            s
        };
        let mut s = filled();
        visual_drag(&mut s, (-1, 0, false), (0, 9, true));
        s.resize(10, 3);
        in_buffer(&s);
        assert!(s.term().selection.is_none());
        assert_eq!(s.term().selection_to_string(), None);
        let mut s = filled();
        visual_drag(&mut s, (0, 0, false), (1, 9, true));
        s.resize(10, 3);
        in_buffer(&s);
        assert_eq!(runs(&s), vec![(-1, 0, 9)]);
        assert_eq!(copied(&s), Some(vec!["s1 \u{5d1}".to_string()]));
        assert_eq!(s.visual_anchor(), None);
        // A grow pulls history back: the run moves down with its text.
        s.resize(10, 5);
        in_buffer(&s);
        assert_eq!(copied(&s), Some(vec!["s1 \u{5d1}".to_string()]));
    }

    /// A width change (with or without a height change, and the reflow it brings) clears a
    /// visual selection as alacritty clears any selection: nothing maps cell for cell.
    #[test]
    fn a_width_change_clears_a_visual_selection() {
        for (cols, rows) in [(12, 3), (4, 3), (12, 5), (4, 2)] {
            let mut s = Screen::new(10, 3);
            s.feed("ab \u{5d0}\u{5d1}\r\nplain".as_bytes());
            visual_drag(&mut s, (0, 0, false), (1, 3, true));
            s.resize(cols, rows);
            assert!(s.term().selection.is_none(), "{cols}x{rows}");
            assert_eq!(s.term().selection_to_string(), None);
            assert_eq!(s.visual_anchor(), None);
            assert!(crate::styled_copy::selection_spans(&s).0.is_empty());
        }
    }

    /// Clearing scrollback, the screen or the lines under a visual selection, a reset, a
    /// screen switch and Clear To Mark leave it exact or clear it: no run or anchor points at
    /// a row that moved or went.
    #[test]
    fn clears_resets_and_screen_switches_keep_a_visual_selection_exact_or_clear_it() {
        let fresh = || {
            let mut s = Screen::with_scrollback(10, 3, 100);
            s.set_rules(crate::emu::Rules { clear_history: Some(false), ..Default::default() });
            s.feed("h \u{5d0}\r\nx\r\ny \u{5d1}\r\nz".as_bytes());
            assert_eq!(s.term().topmost_line().0, -1);
            s
        };
        // Clear scrollback: a selection on the screen stays on its text; one reaching into
        // history goes.
        let mut s = fresh();
        visual_drag(&mut s, (1, 0, false), (1, 9, true));
        s.feed(b"\x1b[3J");
        assert_eq!(runs(&s), vec![(1, 0, 9)]);
        assert_eq!(copied(&s), Some(vec!["y \u{5d1}".to_string()]));
        let mut s = fresh();
        visual_drag(&mut s, (-1, 0, false), (1, 9, true));
        s.feed(b"\x1b[3J");
        assert!(s.term().selection.is_none());
        // The rest clear it.
        for (what, bytes) in [
            ("clear screen", &b"\x1b[2J"[..]),
            ("erase above", b"\x1b[3;1H\x1b[1J"),
            ("erase below", b"\x1b[1;1H\x1b[0J"),
            ("erase line", b"\x1b[2;1H\x1b[2K"),
            ("reset", b"\x1bc"),
            ("alternate screen", b"\x1b[?1049h"),
        ] {
            let mut s = fresh();
            visual_drag(&mut s, (1, 0, false), (1, 9, true));
            s.feed(bytes);
            assert!(s.term().selection.is_none(), "{what}");
            assert_eq!(s.visual_anchor(), None, "{what}");
            in_buffer(&s);
        }
        // Clear To Mark cuts rows and moves the cursor row to the top.
        let mut s = fresh();
        visual_drag(&mut s, (1, 0, false), (1, 9, true));
        s.clear_from(0);
        assert!(s.term().selection.is_none(), "clear to mark");
        in_buffer(&s);
    }

    #[test]
    fn pointer_maps_through_the_order() {
        let mut s = Screen::new(10, 3);
        s.feed("ab \u{5d0}\u{5d1}\r\nplain".as_bytes());
        // Visual column 4 shows the aleph (logical 3); visual 3 shows the bet (logical 4).
        assert_eq!(s.logical_col(0, 4), (3, true));
        assert_eq!(s.logical_col(0, 3), (4, true));
        assert_eq!(s.logical_col(0, 1), (1, false));
        assert_eq!(s.visual_col(0, 3), 4);
        assert_eq!(s.visual_col(0, 9), 9, "past the text: itself");
        // A row with no right-to-left text maps to itself.
        assert_eq!(s.logical_col(1, 3), (3, false));
        assert_eq!(s.visual_col(1, 3), 3);
        // A drag from visual column 4 to 3 (the right half of 4 to the left half of 3, as the
        // pointer's sides turn over in a right-to-left cell) selects logical columns 3 and 4.
        use alacritty_terminal::index::{Column, Line, Point, Side};
        use alacritty_terminal::selection::{Selection, SelectionType};
        let side = |right: bool, rtl: bool| if right != rtl { Side::Right } else { Side::Left };
        let (a, a_rtl) = s.logical_col(0, 4);
        let (b, b_rtl) = s.logical_col(0, 3);
        let mut sel = Selection::new(SelectionType::Simple, Point::new(Line(0), Column(a)), side(true, a_rtl));
        sel.update(Point::new(Line(0), Column(b)), side(false, b_rtl));
        s.term_mut().selection = Some(sel);
        assert_eq!(s.term().selection_to_string().as_deref(), Some("\u{5d0}\u{5d1}"), "the copy is logical, as the program wrote it");
        // The highlight pieces of that selection cover visual 3 and 4 as one.
        assert_eq!(s.visual_runs(0, 3..5), vec![3..5]);
        assert_eq!(s.visual_runs(0, 3..4), vec![4..5]);
    }

    /// A click or hover on a visual cell acts on the cell shown there: its OSC 8 link, never a
    /// neighbour's.
    #[test]
    fn links_under_the_pointer_are_the_ones_shown() {
        use alacritty_terminal::index::{Column, Line, Point};
        let mut s = Screen::new(10, 2);
        s.feed("ab \x1b]8;;https://one.test\x1b\\\u{5d0}\x1b]8;;\x1b\\\x1b]8;;https://two.test\x1b\\\u{5d1}\x1b]8;;\x1b\\".as_bytes());
        let link_at = |s: &Screen, visual: usize| {
            let (logical, _) = s.logical_col(0, visual);
            s.term().grid()[Point::new(Line(0), Column(logical))].hyperlink().map(|h| h.uri().to_string())
        };
        // The aleph (link one) is drawn at visual 4, the bet (link two) at visual 3.
        assert_eq!(link_at(&s, 4).as_deref(), Some("https://one.test"));
        assert_eq!(link_at(&s, 3).as_deref(), Some("https://two.test"));
        assert_eq!(s.logical_col(0, 4).0, 3, "a click on the cell shown acts on its logical character");
        assert_eq!(link_at(&s, 1), None);
    }

    #[test]
    fn right_justify_follows_iterm2() {
        let row = |detect: bool, justify: bool| {
            let mut s = Screen::new(6, 2);
            s.bidi_rules = crate::bidi::Rules { enabled: true, detect_direction: detect, right_justify: justify, off_in_alternate: false };
            s.feed("\u{5d0}\u{5d1}".as_bytes());
            s.row_order(0).unwrap().lut.clone()
        };
        // Both switches: the paragraph pads to the right edge, the fill reversed.
        assert_eq!(row(true, true), vec![5, 4, 3, 2, 1, 0]);
        // iTerm2 needs both: no padding with either off.
        assert_eq!(row(true, false), vec![1, 0]);
        // Detection off: the paragraph is left to right, so the fill follows in order and the text stays left.
        assert_eq!(row(false, true), vec![1, 0, 2, 3, 4, 5]);
    }

    #[test]
    fn the_order_cache_is_bounded() {
        let mut s = Screen::new(20, 3);
        for i in 0..400u32 {
            let c = char::from_u32(0x5d0 + i % 27).unwrap();
            s.feed(format!("\r\n{c}{c}{i}").as_bytes());
            for line in 0..3 {
                let _ = s.row_order(line);
            }
        }
        assert!(s.bidi_cache.borrow().len() <= 4 * 3 + 3, "{}", s.bidi_cache.borrow().len());
    }

    /// Security: only the grid's own draw and the pointer mapping ask for an order. Tab bars,
    /// title bars, status bars, find bars and popups never reorder Tandem's own text.
    #[test]
    fn chrome_is_never_reordered() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let allowed = ["bidi.rs", "screen.rs", "scene.rs"];
        for f in files {
            let name = f.file_name().unwrap().to_string_lossy().to_string();
            let text = std::fs::read_to_string(&f).unwrap();
            let calls = text.contains("bidi::order(") || text.contains(".row_order(");
            assert!(!calls || allowed.contains(&name.as_str()), "{} reorders text outside the grid", f.display());
        }
    }

    #[test]
    fn blink_allowed_follows_the_look() {
        assert!(!Look::default().blink_allowed, "the stock profile's Blink Allowed is off");
        let s = Screen::with_look(10, 2, Palette::default(), Look { blink_allowed: true, ..Look::default() });
        assert!(s.blink_allowed);
        let mut s = Screen::new(10, 2);
        assert!(!s.blink_allowed);
        s.set_look(Look { blink_allowed: true, ..Look::default() });
        assert!(s.blink_allowed);
    }

    #[test]
    fn program_blink_needs_permission() {
        let look = |allow| Look { cursor_blink: false, blink_change: allow, ..Look::default() };
        let mut s = Screen::with_look(80, 24, Palette::default(), look(false));
        s.feed(b"\x1b[1 q");
        assert_eq!(s.term().cursor_style().shape, CursorShape::Block);
        assert!(!s.cursor_blinks());
        s.feed(b"\x1b[?12h");
        assert!(!s.cursor_blinks());
        s.feed(b"\x1b[5 q");
        assert_eq!(s.term().cursor_style().shape, CursorShape::Beam);
        assert!(!s.cursor_blinks());
        let mut s = Screen::with_look(80, 24, Palette::default(), look(true));
        s.feed(b"\x1b[1 q");
        assert!(s.cursor_blinks());
        s.feed(b"\x1b[2 q");
        assert!(!s.cursor_blinks());
        s.feed(b"\x1b[?12h");
        assert!(s.cursor_blinks());
        s.feed(b"\x1b[?12l");
        assert!(!s.cursor_blinks());
        s.feed(b"\x1b[?12h\x1b[0 q");
        assert!(!s.cursor_blinks(), "DECSCUSR 0 is the profile's blink");
        // Turning the permission off keeps the program's shape.
        s.feed(b"\x1b[6 q");
        s.set_look(look(false));
        assert_eq!(s.term().cursor_style().shape, CursorShape::Beam);
        s.feed(b"\x1b[?12h");
        assert!(!s.cursor_blinks());
        // RIS restores the profile in every case.
        s.set_look(look(true));
        s.feed(b"\x1b[1 q\x1bc");
        assert!(!s.cursor_blinks());
        assert_eq!(s.term().cursor_style().shape, Look::default().cursor_shape);
    }

    /// 13l P15: a program's underline style and color are per cell, bounded and reset.
    #[test]
    fn underline_state_is_bounded_and_reset() {
        use alacritty_terminal::term::cell::Flags;
        let cell = |s: &Screen, col: usize| {
            let g = s.term().grid();
            g[alacritty_terminal::index::Point::new(alacritty_terminal::index::Line(0), alacritty_terminal::index::Column(col))].clone()
        };
        let mut s = Screen::new(80, 24);
        // Colon and semicolon forms of SGR 58 and the 4:x styles reach the cells.
        s.feed(b"\x1b[4:3;58:2::255:0:0mA\x1b[58;2;1;2;3mB\x1b[4:4mC\x1b[4:5mD\x1b[4:2mE\x1b[0mF");
        assert!(cell(&s, 0).flags.contains(Flags::UNDERCURL));
        assert_eq!(cell(&s, 0).underline_color(), Some(alacritty_terminal::vte::ansi::Color::Spec(alacritty_terminal::vte::ansi::Rgb { r: 255, g: 0, b: 0 })));
        assert_eq!(cell(&s, 1).underline_color(), Some(alacritty_terminal::vte::ansi::Color::Spec(alacritty_terminal::vte::ansi::Rgb { r: 1, g: 2, b: 3 })));
        assert!(cell(&s, 2).flags.contains(Flags::DOTTED_UNDERLINE));
        assert!(cell(&s, 3).flags.contains(Flags::DASHED_UNDERLINE));
        assert!(cell(&s, 4).flags.contains(Flags::DOUBLE_UNDERLINE));
        assert!(!cell(&s, 5).flags.intersects(Flags::ALL_UNDERLINES) && cell(&s, 5).underline_color().is_none(), "SGR 0 clears both");
        // SGR 59 and 24 clear each on its own.
        s.feed(b"\r\x1b[4;58;5;9mA\x1b[59mB\x1b[24mC\x1b[0m");
        assert!(cell(&s, 0).underline_color().is_some());
        assert!(cell(&s, 1).flags.contains(Flags::UNDERLINE) && cell(&s, 1).underline_color().is_none());
        assert!(!cell(&s, 2).flags.intersects(Flags::ALL_UNDERLINES));
        // RIS clears the pen; an erase draws no underline color.
        s.feed(b"\x1b[4;58;5;9m\x1bc\rX");
        assert!(!cell(&s, 0).flags.intersects(Flags::ALL_UNDERLINES) && cell(&s, 0).underline_color().is_none());
        s.feed(b"\x1b[4;58;5;9m\x1b[2J\x1b[H");
        assert!(cell(&s, 0).underline_color().is_none());
        // Out of range values and a pseudo-random SGR stream never panic.
        s.feed(b"\x1b[0m\x1b[58:5:300mA\x1b[58:2:::999mB\x1b[4:9mC\x1b[0m");
        // Out of range values set no wrong color: A and B carry none, and an unknown 4:n is a Single underline.
        assert!(cell(&s, 0).underline_color().is_none() && cell(&s, 1).underline_color().is_none());
        assert!(cell(&s, 2).flags.contains(Flags::UNDERLINE) && !cell(&s, 2).flags.intersects(Flags::UNDERCURL | Flags::DOUBLE_UNDERLINE | Flags::DOTTED_UNDERLINE | Flags::DASHED_UNDERLINE));
        let mut seed = 12345u32;
        let mut bytes = Vec::new();
        for _ in 0..5000 {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            let n = (seed >> 16) % 70;
            bytes.extend_from_slice(format!("\x1b[{n}m").as_bytes());
        }
        s.feed(&bytes);
        s.feed(b"\x1bc");
        assert_eq!(s.term().columns(), 80);
    }

    #[test]
    fn exfiltrated_follows_iterm2() {
        let mut s = Screen::new(80, 24);
        let p = |k: &str, v: &str| (k.to_string(), v.to_string());
        assert_eq!(s.exfiltrated(None), vec![]);
        s.env = Some(vec![p("A", "1"), p("B", "2"), p("A", "3")]);
        assert_eq!(s.exfiltrated(Some(&["A"])), vec![p("A", "1"), p("A", "3")]);
        assert_eq!(s.exfiltrated(None), vec![p("A", "1"), p("B", "2"), p("A", "3")]);
        assert_eq!(s.exfiltrated(Some(&[])), vec![]);
    }

    #[test]
    fn answers_background_color_query() {
        let mut s = Screen::new(80, 24);
        let reply = String::from_utf8(s.feed(b"\x1b]11;?\x07")).unwrap();
        let bg = s.palette.bg;
        let want = format!("rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}", bg.r, bg.g, bg.b);
        assert!(reply.contains(&want), "{reply:?} vs {want}");
    }

    #[test]
    fn answers_text_area_size_query() {
        let mut s = Screen::new(80, 24);
        s.set_cell_px(9, 18);
        let reply = String::from_utf8(s.feed(b"\x1b[14t")).unwrap();
        assert!(reply.contains("4;432;720t"), "{reply:?}");
    }

    #[test]
    fn tracks_title() {
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b]0;hello\x07");
        assert_eq!(s.title(), Some("hello"));
    }

    /// Allow Title Setting gates OSC 0 and 2 and the title stack's reset (`PS:19364-19371`);
    /// an applied title asks the window to share it (`PT:15578-15584`). OSC 1 (the icon name)
    /// is not reported by vte 0.15, so it sets nothing either way.
    #[test]
    fn title_setting_follows_iterm2() {
        for allow in [true, false] {
            let mut s = Screen::new(80, 24);
            s.allow_title = allow;
            s.feed(b"\x1b]1;hello\x07");
            assert_eq!(s.title(), None, "{allow}");
            assert_eq!(s.take_requests(), vec![], "{allow}");
        }
        for seq in [&b"\x1b]2;hello\x07"[..], b"\x1b]0;hello\x07"] {
            let mut s = Screen::new(80, 24);
            assert!(s.allow_title);
            s.feed(seq);
            assert_eq!(s.title(), Some("hello"), "{seq:?}");
            assert_eq!(s.take_requests(), vec![Request::WindowTitle(Some("hello".into()))], "{seq:?}");
            let mut s = Screen::new(80, 24);
            s.allow_title = false;
            s.feed(seq);
            assert_eq!(s.title(), None, "{seq:?}");
            assert_eq!(s.take_requests(), vec![], "{seq:?}");
        }
        // Push (no title yet), set, pop: the pop resets the title only when allowed.
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b[22;0t\x1b]2;hello\x07");
        s.take_requests();
        s.allow_title = false;
        s.feed(b"\x1b[23;0t");
        assert_eq!(s.title(), Some("hello"));
        assert_eq!(s.take_requests(), vec![]);
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b[22;0t\x1b]2;hello\x07");
        s.take_requests();
        s.feed(b"\x1b[23;0t");
        assert_eq!(s.title(), None);
        assert_eq!(s.take_requests(), vec![Request::WindowTitle(None)]);
        // A copied title does not ask again (no loop between two panes).
        s.set_title(Some("copied".into()));
        assert_eq!(s.title(), Some("copied"));
        assert_eq!(s.take_requests(), vec![]);
    }

    #[test]
    fn resize_changes_the_grid() {
        let mut s = Screen::new(80, 24);
        s.resize(100, 30);
        assert_eq!((s.term().columns(), s.term().screen_lines()), (100, 30));
    }

    #[test]
    fn look_follows_the_profile() {
        let mut p = crate::profile::Profile::default();
        p.set("Cursor Type", toml::Value::Integer(1));
        p.set("Blinking Cursor", toml::Value::Boolean(true));
        p.set("Scrollback Lines", toml::Value::Integer(5000));
        let look = Look::from_profile(&p);
        assert_eq!((look.cursor_shape, look.cursor_blink, look.scrollback), (CursorShape::Beam, true, 5000));
        p.set("Unlimited Scrollback", toml::Value::Boolean(true));
        assert_eq!(Look::from_profile(&p).scrollback, 100_000);
    }

    #[test]
    fn the_profile_cursor_shape_is_used() {
        let mut p = crate::profile::Profile::default();
        p.set("Cursor Type", toml::Value::Integer(0));
        let s = Screen::with_look(80, 24, Palette::default(), Look::from_profile(&p));
        assert_eq!(s.term().renderable_content().cursor.shape, CursorShape::Underline);
    }

    #[test]
    fn alternate_scroll_mode_follows_iterm2() {
        // iTerm2 starts with alternate scroll off and clears it on reset (VT100Terminal.m:399, :1697).
        let mut s = Screen::new(80, 24);
        assert!(!s.alternate_scroll_mode());
        s.feed(b"\x1b[?1007h");
        assert!(s.alternate_scroll_mode());
        s.feed(b"\x1b[?1007l");
        assert!(!s.alternate_scroll_mode());
        s.feed(b"\x1b[?1007h");
        s.feed(b"\x1bc");
        assert!(!s.alternate_scroll_mode());
        // The profile's gate is a Term field, so a RIS keeps it.
        s.set_alternate_scroll_allowed(false);
        s.feed(b"\x1bc");
        s.feed(b"\x1b[?1007h");
        assert!(!s.alternate_scroll_mode());
        s.set_alternate_scroll_allowed(true);
        // DECRQM answers from the mode.
        let ask = |s: &mut Screen| s.feed(b"\x1b[?1007$p");
        assert_eq!(ask(&mut s), b"\x1b[?1007;2$y");
        s.feed(b"\x1b[?1007h");
        assert_eq!(ask(&mut s), b"\x1b[?1007;1$y");
    }

    #[test]
    fn soft_reset_and_a_prompt_clear_alternate_scroll() {
        // DECSTR (soft_reset) clears 1007 (VT100Terminal.m:1697).
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b[?1007h");
        assert!(s.alternate_scroll_mode());
        s.soft_reset();
        assert!(!s.alternate_scroll_mode());
        // An initial prompt mark clears it, as ssh dying would need (VT100Terminal.m:4944).
        s.feed(b"\x1b[?1007h\x1b]133;A\x07");
        assert!(!s.alternate_scroll_mode());
        // Also from the alternate screen, where the prompt mark itself is ignored.
        s.feed(b"\x1b[?1049h\x1b[?1007h\x1b]133;A\x07");
        assert!(!s.alternate_scroll_mode());
        s.feed(b"\x1b[?1049l");
        // A secondary or continuation prompt does not disturb it.
        s.feed(b"\x1b[?1007h\x1b]133;A;k=s\x07\x1b]133;A;k=c\x07");
        assert!(s.alternate_scroll_mode());
    }

    #[test]
    fn alternate_scroll_is_gated_by_the_profile() {
        // VT100Terminal.m:877-881: a set needs the profile's Allow Alternate Mouse Scroll.
        let mut s = Screen::new(80, 24);
        s.set_alternate_scroll_allowed(false);
        s.feed(b"\x1b[?1007h");
        assert!(!s.alternate_scroll_mode());
        // A reset always applies, even when the profile now refuses.
        s.set_alternate_scroll_allowed(true);
        s.feed(b"\x1b[?1007h");
        assert!(s.alternate_scroll_mode());
        s.set_alternate_scroll_allowed(false);
        s.feed(b"\x1b[?1007l");
        assert!(!s.alternate_scroll_mode());
    }

    #[test]
    fn scroll_regions_follow_scrollback_with_status_bar() {
        use alacritty_terminal::grid::Dimensions;
        // Six lines through rows 1 to 4 of a 10 by 5 screen whose fifth row holds a status line.
        let run = |on: bool, region: &[u8]| {
            let mut s = Screen::new(10, 5);
            s.set_scrollback_with_status_bar(on);
            s.feed(b"\x1b[5;1Hstatus");
            s.feed(region);
            for i in 0..6 {
                s.feed(format!("line{i}\r\n").as_bytes());
            }
            (s.term().grid().history_size(), s.line_text(4))
        };
        let (on, off) = (run(true, b"\x1b[1;4r"), run(false, b"\x1b[1;4r"));
        assert!(on.0 > 0, "{on:?}");
        assert_eq!(on.1.trim_end(), "status");
        assert_eq!(off.0, 0, "{off:?}");
        assert_eq!(off.1.trim_end(), "status");
        // With no region, a full-screen scroll saves either way.
        for flag in [true, false] {
            let mut s = Screen::new(10, 5);
            s.set_scrollback_with_status_bar(flag);
            for i in 0..8 {
                s.feed(format!("line{i}\r\n").as_bytes());
            }
            assert!(s.term().grid().history_size() > 0, "flag {flag}");
        }
        // A region below the top never saves.
        assert_eq!(run(true, b"\x1b[2;4r").0, 0);
        assert_eq!(run(false, b"\x1b[2;4r").0, 0);
    }

    fn normal(form: crate::encoding::Form) -> Screen {
        Screen::with_look(20, 3, crate::palette::Palette::default(), Look { normalization: form, ..Look::default() })
    }

    /// Text leaves the grid in the profile's form; the grid keeps what the program wrote.
    #[test]
    fn text_leaves_the_grid_normalized() {
        use crate::encoding::Form;
        let mut s = normal(Form::Nfd);
        s.feed("\u{e9}x\r\nab".as_bytes());
        assert_eq!(s.line_text(0), "e\u{301}x");
        assert_eq!(s.all_text(), "e\u{301}x\nab\n");
        assert_eq!(s.term().grid()[Line(0)][Column(0)].c, '\u{e9}', "the grid is not changed");
        assert_eq!(s.normalized("\u{e9}"), "e\u{301}");
        let mut sel = alacritty_terminal::selection::Selection::new(alacritty_terminal::selection::SelectionType::Simple, alacritty_terminal::index::Point::new(Line(0), Column(0)), alacritty_terminal::index::Side::Left);
        sel.update(alacritty_terminal::index::Point::new(Line(0), Column(0)), alacritty_terminal::index::Side::Right);
        s.term_mut().selection = Some(sel);
        assert_eq!(s.selection_text().as_deref(), Some("e\u{301}"));
        let mut none = normal(Form::None);
        none.feed("\u{e9}".as_bytes());
        assert_eq!(none.line_text(0), "\u{e9}");
        assert_eq!(none.normalized("e\u{301}"), "e\u{301}");
    }

    /// A profile edit reaches the screen through `set_look`, and `Look::from_profile` reads the
    /// stored Unicode Normalization (HFS+ does nothing on Windows).
    #[test]
    fn look_carries_the_normalization_form() {
        use crate::encoding::Form;
        let mut p = crate::profile::Profile::stock();
        assert_eq!(Look::from_profile(&p).normalization, Form::None);
        for (v, want) in [(1, Form::Nfc), (2, Form::Nfd), (3, Form::None)] {
            p.set("Unicode Normalization", toml::Value::Integer(v));
            assert_eq!(Look::from_profile(&p).normalization, want);
        }
        let mut s = Screen::new(10, 2);
        assert_eq!(s.normalization, Form::None);
        s.set_look(Look { normalization: Form::Nfc, ..Look::default() });
        assert_eq!(s.normalization, Form::Nfc);
    }

    #[test]
    fn all_text_includes_scrollback() {
        let mut s = Screen::new(20, 3);
        for i in 0..6 {
            s.feed(format!("line{i}\r\n").as_bytes());
        }
        let t = s.all_text();
        assert!(t.starts_with("line0\nline1\n"), "{t:?}");
        assert!(t.contains("line5"));
    }

    #[test]
    fn clear_scrollback_keeps_the_screen() {
        let mut s = Screen::new(20, 3);
        for i in 0..6 {
            s.feed(format!("line{i}\r\n").as_bytes());
        }
        s.clear(true);
        let t = s.all_text();
        assert!(!t.contains("line0") && t.contains("line5"), "{t:?}");
        s.clear(false);
        assert!(!s.all_text().contains("line5"));
    }

    #[test]
    fn a_mark_moves_into_history_with_its_row() {
        let mut s = Screen::new(20, 3);
        s.feed(b"prompt> ");
        s.set_row_mark(0, 7);
        for i in 0..5 {
            s.feed(format!("\r\nout{i}").as_bytes());
        }
        let marks = s.row_marks();
        assert_eq!(marks.len(), 1);
        let (line, id) = marks[0];
        assert_eq!(id, 7);
        assert!(line < 0, "{line}");
        assert!(s.line_text(line).starts_with("prompt>"));
    }

    #[test]
    fn a_mark_survives_reflow() {
        let mut s = Screen::new(10, 5);
        s.feed(b"$ ");
        s.set_row_mark(0, 3);
        s.feed(b"abcdefghijklmnop\r\nnext");
        s.resize(6, 5);
        s.resize(30, 5);
        let (line, id) = s.row_marks()[0];
        assert_eq!(id, 3);
        assert!(s.line_text(line).starts_with("$ abcdefghijklmnop"), "{:?}", s.line_text(line));
    }

    #[test]
    fn a_cleared_row_loses_its_mark() {
        let mut s = Screen::new(20, 3);
        s.set_row_mark(1, 5);
        s.feed(b"\x1b[H\x1b[J");
        assert!(s.row_marks().is_empty());
    }

    #[test]
    fn clear_buffer_keeps_the_cursor_line_at_the_top() {
        let mut s = Screen::new(20, 5);
        s.feed(b"old1\r\nold2\r\nold3\r\nprompt> ");
        s.clear(false);
        assert_eq!(s.all_text(), "prompt>\n");
        let cursor = s.term().grid().cursor.point;
        assert_eq!((cursor.line.0, cursor.column.0), (0, 8));
    }

    #[test]
    fn ssh_saved_follows_iterm2() {
        let mut s = Screen::new(80, 24);
        s.shell.cwd = Some("/home/me".into());
        s.shell.host = Some("laptop".into());
        s.shell.user = Some("me".into());
        let saved = s.ssh_saved();
        assert_eq!(saved, crate::conductor::SavedState { cwd: Some("/home/me".into()), host: Some("laptop".into()), user: Some("me".into()) });
        s.shell.cwd = Some("/srv".into());
        s.shell.host = Some("far".into());
        s.shell.user = None;
        s.restore_ssh_saved(&saved);
        assert_eq!((s.shell.cwd.as_deref(), s.shell.host.as_deref(), s.shell.user.as_deref()), (Some("/home/me"), Some("laptop"), Some("me")));
    }

    #[test]
    fn soft_reset_and_program_ended_leave_the_cursor() {
        // DECSTBM inside the reset homes the cursor; a restart must write under the old text.
        for ended in [false, true] {
            let mut s = Screen::new(20, 5);
            s.feed(b"one\r\ntwo\r\n");
            let before = s.term().grid().cursor.point;
            if ended {
                s.program_ended();
            } else {
                s.soft_reset();
            }
            assert_eq!(s.term().grid().cursor.point, before, "ended={ended}");
        }
    }

    #[test]
    fn soft_reset_keeps_the_text_and_drops_the_modes() {
        let mut s = Screen::new(20, 5);
        s.feed(b"keep me\r\n\x1b[?1002h\x1b[?1006h\x1b[?25l\x1b[?2004h\x1b[1;31m");
        s.soft_reset();
        assert!(s.all_text().contains("keep me"));
        assert_eq!(s.mouse_modes(), Modes { tracking: Tracking::Off, sgr: false });
        let mode = *s.term().mode();
        assert!(mode.contains(TermMode::SHOW_CURSOR) && !mode.contains(TermMode::BRACKETED_PASTE));
    }

    #[test]
    fn tracks_drag_and_sgr() {
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b[?1002h\x1b[?1006h");
        assert_eq!(s.mouse_modes(), Modes { tracking: Tracking::Drag, sgr: true });
    }

    #[test]
    fn any_motion_wins() {
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b[?1000h\x1b[?1003h");
        assert_eq!(s.mouse_modes().tracking, Tracking::Any);
    }

    #[test]
    fn reset_turns_tracking_off() {
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b[?1002h\x1b[?1002l");
        assert_eq!(s.mouse_modes().tracking, Tracking::Off);
    }

    #[test]
    fn answers_device_attributes() {
        let mut s = Screen::new(80, 24);
        let reply = s.feed(b"\x1b[c");
        assert!(reply.starts_with(b"\x1b[?"), "{:?}", String::from_utf8_lossy(&reply));
    }

    #[test]
    fn text_shows_output() {
        let mut s = Screen::new(80, 24);
        s.feed(b"hello\r\nworld");
        let text = s.text();
        assert!(text.contains("hello") && text.contains("world"), "{text}");
    }

    #[test]
    fn output_stamps_the_rows_it_writes() {
        let mut s = Screen::new(20, 6);
        s.set_clock(Some(100));
        s.feed(b"a\r\nb");
        s.set_clock(Some(200));
        s.feed(b"\r\nc");
        assert_eq!((s.row_time(0), s.row_time(1), s.row_time(2), s.row_time(3)), (100, 100, 200, 0));
    }

    #[test]
    fn row_slots_survive_reflow() {
        let mut s = Screen::new(20, 6);
        s.set_clock(Some(100));
        s.feed(b"0123456789012345678901234\r\n");
        s.set_clock(Some(200));
        s.feed(b"x");
        // Every row by its text, oldest first.
        let rows = |s: &Screen| {
            let top = -(s.term().grid().history_size() as i32);
            (top..6)
                .map(|l| {
                    let g = s.term().grid();
                    let text: String = (0..g.columns()).map(|c| g[Line(l)][Column(c)].c).collect();
                    (text.trim_end().to_string(), s.row_time(l))
                })
                .filter(|(t, _)| !t.is_empty())
                .collect::<Vec<_>>()
        };
        s.resize(10, 6);
        let t = |xs: &[(&str, u32)]| xs.iter().map(|(a, b)| (a.to_string(), *b)).collect::<Vec<_>>();
        assert_eq!(rows(&s), t(&[("0123456789", 100), ("0123456789", 100), ("01234", 100), ("x", 200)]));
        s.resize(30, 6);
        assert_eq!(rows(&s), t(&[("0123456789012345678901234", 100), ("x", 200)]));
    }

    #[test]
    fn splice_replaces_rows_and_keeps_the_cursor_row() {
        let mut s = Screen::new(20, 6);
        s.feed(b"l0\r\nl1\r\nl2\r\nl3\r\nl4\r\nprompt");
        let cursor = s.term().grid().cursor.point.line.0;
        let placeholder = s.text_rows(&["folded".to_string()]);
        let removed = s.splice_rows(1, 3, placeholder);
        assert_eq!(removed.len(), 3);
        // Rows above slide down; with no history, blank rows fill in at the top.
        assert_eq!(s.all_text().trim_start_matches('\n'), "l0\nfolded\nl4\nprompt\n");
        assert_eq!(s.term().grid().cursor.point.line.0, cursor);
        assert_eq!(s.line_text(cursor), "prompt");
        let line = (0..6).find(|l| s.line_text(*l) == "folded").unwrap();
        s.splice_rows(line, line, removed);
        assert_eq!(s.all_text().trim_start_matches('\n'), "l0\nl1\nl2\nl3\nl4\nprompt\n");
        assert_eq!(s.line_text(s.term().grid().cursor.point.line.0), "prompt");
    }

    #[test]
    fn text_rows_wrap_long_lines() {
        let s = Screen::new(4, 3);
        let rows = s.text_rows(&["abcdefg".to_string(), "h".to_string()]);
        let text = |r: &Row<Cell>| (0..4).map(|c| r[Column(c)].c).collect::<String>();
        assert_eq!(rows.iter().map(text).collect::<Vec<_>>(), vec!["abcd", "efg ", "h   "]);
        use alacritty_terminal::term::cell::Flags;
        assert!(rows[0][Column(3)].flags.contains(Flags::WRAPLINE));
        assert!(!rows[1][Column(3)].flags.contains(Flags::WRAPLINE));
    }

    fn lines_text(s: &Screen) -> String {
        s.all_text().trim_start_matches('\n').to_string()
    }

    #[test]
    fn fold_then_unfold_is_lossless() {
        let mut s = Screen::with_scrollback(20, 6, 100);
        s.feed(b"l0\r\nl1\r\nl2\r\nl3\r\n$ ");
        s.set_row_mark(1, 7);
        let id = s.fold(1, 3).unwrap();
        assert_eq!(lines_text(&s), "l0\nl1 \u{2026}3 lines\u{2026} l3\n$\n");
        let line = (0..6).find(|l| s.fold_at(*l) == Some(id)).unwrap();
        assert_eq!(s.line_text(line), "l1 \u{2026}3 lines\u{2026} l3");
        assert!(s.unfold(id));
        assert_eq!(lines_text(&s), "l0\nl1\nl2\nl3\n$\n");
        assert!(s.row_marks().iter().any(|(_, m)| *m == 7), "the mark came back with its row");
        assert!(!s.unfold(id));
    }

    #[test]
    fn unfold_restores_lines_after_scrolling() {
        let mut s = Screen::with_scrollback(20, 4, 100);
        s.feed(b"a\r\nb\r\nc\r\n$ ");
        let id = s.fold(0, 1).unwrap();
        for i in 0..10 {
            s.feed(format!("\r\nout{i}").as_bytes());
        }
        assert!(s.unfold(id));
        let text = lines_text(&s);
        assert!(text.starts_with("a\nb\nc\n$\nout0\n"), "{text}");
        assert!(text.ends_with("out9\n"), "{text}");
    }

    #[test]
    fn fold_refuses_the_cursor_row() {
        let mut s = Screen::with_scrollback(20, 4, 100);
        s.feed(b"a\r\nb\r\n$ ");
        assert_eq!(s.fold(1, 2), None);
        s.feed(b"\x1b[?1049h");
        assert_eq!(s.fold(0, 0), None, "not on the alternate screen");
    }

    #[test]
    fn unfold_after_widening_fits_the_rows() {
        let mut s = Screen::with_scrollback(20, 5, 100);
        s.feed(b"l0\r\nl1\r\nl2\r\n$ ");
        let id = s.fold(0, 1).unwrap();
        s.resize(30, 5);
        assert!(s.unfold(id));
        let text = lines_text(&s);
        assert!(text.starts_with("l0\nl1\nl2\n$"), "{text}");
        let mut f = crate::scene::Frame::default();
        crate::scene::pane(&mut f, &s, crate::layout::Rect { x: 0.0, y: 0.0, w: 300.0, h: 100.0 }, (10.0, 20.0), true, &crate::cursor_look::CursorLook::shown(true));
    }

    #[test]
    fn unfold_after_narrowing_leaves_no_summary() {
        let mut s = Screen::with_scrollback(30, 5, 100);
        s.feed(b"first line here\r\nmiddle\r\nlast line here\r\n$ ");
        let id = s.fold(0, 2).unwrap();
        s.resize(10, 5);
        assert!(s.unfold(id));
        let text = lines_text(&s);
        assert!(!text.contains('\u{2026}'), "{text}");
        assert!(text.contains("middle"), "{text}");
    }

    #[test]
    fn buffer_edits_refuse_the_alternate_screen() {
        let mut s = Screen::with_scrollback(20, 5, 100);
        s.feed(b"a\r\nb\r\nc\r\n$ ");
        let id = s.fold(0, 1).unwrap();
        s.feed(b"\x1b[?1049h");
        assert!(s.splice_rows(0, 0, Vec::new()).is_empty());
        assert!(!s.unfold(id), "the fold waits for the main screen");
        assert_eq!(s.add_note(0, 0, 0, 1, "x", false), 0);
        s.feed(b"\x1b[?1049l");
        assert!(s.unfold(id));
    }

    #[test]
    fn clearing_forgets_folds_and_notes() {
        let mut s = Screen::with_scrollback(20, 5, 100);
        s.feed(b"a\r\nb\r\nc\r\n$ ");
        s.fold(0, 1).unwrap();
        s.add_note(2, 0, 2, 0, "n", false);
        s.clear(false);
        assert_eq!(s.fold_count(), 0);
        assert!(s.notes.by_id.is_empty());
    }
    #[test]
    fn app_requests_follow_the_profile() {
        use crate::keyproto::{KeyMode, Keyboard};
        let mut s = Screen::new(80, 24);
        assert_eq!(s.keyboard(), Keyboard { mode: KeyMode::Standard, kitty: 0 });
        s.feed(b"\x1b[>4;2m\x1b[>1u");
        assert_eq!(s.keyboard(), Keyboard { mode: KeyMode::Mok2, kitty: 1 });
        assert_eq!(s.feed(b"\x1b[?4m\x1b[?u"), b"\x1b[>4;2m\x1b[?1u");
        s.feed(b"\x1b[>4;0m");
        assert_eq!(s.keyboard().mode, KeyMode::Standard);

        let mut locked = Screen::with_look(80, 24, Palette::default(), Look { app_key_modes: false, ..Look::default() });
        locked.feed(b"\x1b[>4;2m\x1b[>1u");
        assert_eq!(locked.keyboard(), Keyboard { mode: KeyMode::Standard, kitty: 0 });

        let tickit = Screen::with_look(80, 24, Palette::default(), Look { csi_u: true, ..Look::default() });
        assert_eq!(tickit.keyboard().mode, KeyMode::CsiU);
    }

    #[test]
    fn menu_flags_join_the_apps() {
        use crate::keyproto::{self, KeyMode};
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b[>1u");
        s.toggle_kitty_flag(keyproto::EVENT_TYPES);
        assert_eq!(s.keyboard().kitty, keyproto::DISAMBIGUATE | keyproto::EVENT_TYPES);
        s.toggle_kitty_flag(keyproto::DISAMBIGUATE);
        assert_eq!(s.keyboard().kitty, keyproto::EVENT_TYPES, "the menu can turn off an app's flag");
        s.set_key_mode(KeyMode::Mok1);
        assert_eq!(s.keyboard().mode, KeyMode::Mok1);
        assert_eq!(s.feed(b"\x1b[?4m"), b"\x1b[>4;1m");
        s.set_key_mode(KeyMode::CsiU);
        assert_eq!(s.keyboard().mode, KeyMode::CsiU);
        s.set_key_mode(KeyMode::Raw);
        assert_eq!(s.keyboard().mode, KeyMode::Raw);
        s.set_key_mode(KeyMode::Standard);
        assert_eq!(s.keyboard().mode, KeyMode::Standard);
    }

    #[test]
    fn reset_clears_keyboard_modes() {
        use crate::keyproto::{KeyMode, Keyboard};
        let mut s = Screen::new(80, 24);
        s.feed(b"\x1b[>4;2m\x1b[>11u");
        s.feed(b"\x1bc");
        assert_eq!(s.keyboard(), Keyboard { mode: KeyMode::Standard, kitty: 0 }, "RIS");
        s.feed(b"\x1b[>4;2m\x1b[>11u");
        s.toggle_kitty_flag(crate::keyproto::ASSOCIATED_TEXT);
        s.set_key_mode(KeyMode::Raw);
        s.soft_reset();
        assert_eq!(s.keyboard(), Keyboard { mode: KeyMode::Standard, kitty: 0 }, "Terminal State > Reset");
        // A program that pushes past the stack's depth does not crash the terminal.
        for _ in 0..5000 {
            s.feed(b"\x1b[>1u");
        }
        assert_eq!(s.keyboard().kitty, 1);
    }

    #[test]
    fn literal_controls_show_carets() {
        use alacritty_terminal::term::cell::Flags;
        let mut s = Screen::new(20, 4);
        s.keys.literal = true;
        s.feed(b"a\x1bb\r\nc");
        let row = |l: i32| (0..20).map(|c| s.term.grid()[Line(l)][Column(c)].c).collect::<String>().trim_end().to_string();
        assert_eq!(row(0), "a^[b^M^J");
        assert_eq!(row(1), "c");
        assert!(s.term.grid()[Line(0)][Column(1)].flags.contains(Flags::INVERSE));
        assert!(!s.term.grid()[Line(0)][Column(3)].flags.contains(Flags::INVERSE));
    }
    use alacritty_terminal::vte::ansi::Rgb;

    impl Screen {
        fn backdate_print_capture(&mut self) {
            if let Some(c) = &mut self.print_capture {
                c.started -= std::time::Duration::from_secs(crate::emu::PRINT_CAPTURE_SECS + 1);
            }
        }
    }

    fn row_text(s: &Screen, l: i32) -> String {
        (0..s.term.columns()).map(|c| s.term.grid()[Line(l)][Column(c)].c).collect::<String>().trim_end().to_string()
    }

    #[test]
    fn cursor_hide_is_noticed() {
        let mut s = Screen::new(20, 4);
        assert_eq!(s.cursor_hidden_at, None);
        s.feed(b"\x1b[?25l");
        let first = s.cursor_hidden_at.expect("hidden");
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.feed(b"\x1b[?25l");
        assert_eq!(s.cursor_hidden_at, Some(first), "hiding twice keeps the first time");
        s.feed(b"\x1b[?25h");
        assert_eq!(s.cursor_hidden_at, None);
    }

    #[test]
    fn an_unended_sync_flushes() {
        let mut s = Screen::new(20, 4);
        s.feed(b"\x1b[?2026hhello");
        assert_eq!(row_text(&s, 0), "", "held while the update runs");
        let due = s.sync_deadline().expect("a deadline");
        assert!(due > std::time::Instant::now());
        s.flush_sync();
        assert_eq!(row_text(&s, 0), "hello");
        assert_eq!(s.sync_deadline(), None);
        s.feed(b"\x1b[?2026hab\x1b[?2026l");
        assert_eq!(row_text(&s, 0), "helloab");
        assert_eq!(s.sync_deadline(), None);
    }

    #[test]
    fn dcs_sync_brackets_work() {
        let mut s = Screen::new(20, 4);
        s.feed(b"\x1bP=1s\x1b\\abc");
        assert_eq!(row_text(&s, 0), "");
        assert!(s.sync_deadline().is_some());
        s.feed(b"\x1bP=2s\x1b\\");
        assert_eq!(row_text(&s, 0), "abc");
        assert_eq!(s.sync_deadline(), None);
        // A query inside an update sees the state before it.
        let reply = s.feed(b"\x1b[?2026h\x1b[1m\x1bP$qm\x1b\\");
        assert_eq!(String::from_utf8(reply).unwrap(), "\x1bP1$r0;1m\x1b\\");
    }

    #[test]
    fn dark_mode_query_follows_the_background() {
        let mut s = Screen::new(20, 4);
        s.palette.bg = Rgb { r: 0, g: 0, b: 0 };
        assert_eq!(s.feed(b"\x1b[?996n"), b"\x1b[?997;1n");
        s.feed(b"\x1b]11;rgb:ffff/ffff/ffff\x07");
        assert_eq!(s.feed(b"\x1b[?996n"), b"\x1b[?997;2n");
        let mut light = Screen::new(20, 4);
        light.palette.bg = Rgb { r: 250, g: 250, b: 240 };
        assert_eq!(light.feed(b"\x1b[?996n"), b"\x1b[?997;2n");
    }

    #[test]
    fn mode_2031_reports_changes() {
        let mut s = Screen::new(20, 4);
        s.palette.bg = Rgb { r: 0, g: 0, b: 0 };
        assert_eq!(s.feed(b"\x1b[?2031h"), b"");
        assert_eq!(s.feed(b"\x1b]11;rgb:ffff/ffff/ffff\x07"), b"\x1b[?997;2n");
        assert_eq!(s.feed(b"\x1b]11;rgb:eeee/eeee/eeee\x07"), b"", "still light");
        assert_eq!(s.feed(b"\x1b]111\x07"), b"\x1b[?997;1n", "back to the profile's dark color");
        let mut light = Palette::default();
        light.bg = Rgb { r: 250, g: 250, b: 250 };
        s.set_palette(light);
        assert_eq!(s.take_replies(), b"\x1b[?997;2n", "a profile change reports too");
        s.feed(b"\x1b[?2031l");
        assert_eq!(s.feed(b"\x1b]11;rgb:ffff/ffff/ffff\x07"), b"");
    }
    #[test]
    fn reset_works_under_literal_controls() {
        let mut s = Screen::new(20, 4);
        s.feed(b"\x1b[?1h");
        s.keys.literal = true;
        s.soft_reset();
        assert!(!s.term.mode().contains(TermMode::APP_CURSOR));
        assert_eq!(row_text(&s, 0), "", "no carets from the reset itself");
    }

    /// `TV:2876-2917` with `S:683-699`: a frame when a line is dirty, the cursor's old and
    /// new lines dirty when it moved, no clean line after a scroll, the current grid's rows
    /// and cursor.
    #[test]
    fn capture_follows_iterm2() {
        let text = |r: &Row<Cell>| (0..r.len()).map(|c| r[Column(c)].c).collect::<String>().trim_end().to_string();
        let mut s = Screen::new(10, 4);
        let c = s.replay_capture().expect("a new screen is all dirty");
        assert_eq!((c.rows.len(), c.clean), (4, vec![false; 4]));
        assert_eq!(c.cursor, (0, 0));
        assert!(s.replay_capture().is_none(), "no output, no frame");
        s.feed(b"\x1b[3;1Hx");
        let c = s.replay_capture().expect("row 2 was written");
        assert_eq!(c.clean, vec![false, true, false, true], "row 2 and the cursor's old row");
        assert_eq!(text(&c.rows[2]), "x");
        assert_eq!(c.cursor, (1, 2));
        assert!(s.replay_capture().is_none(), "the cursor did not move");
        s.feed(b"y\x08");
        let c = s.replay_capture().expect("the cursor's row changed under a cursor that ended where it was");
        assert_eq!(c.clean, vec![true, true, false, true]);
        assert_eq!(text(&c.rows[2]), "xy");
        s.feed(b"\r\n\n\nz");
        let c = s.replay_capture().expect("output scrolled");
        assert_eq!(c.clean, vec![false; 4], "a scroll leaves no clean line");
        assert_eq!(text(&c.rows[3]), "z");
        s.feed(b"\x1b[?1049h\x1b[Halt");
        let c = s.replay_capture().expect("the alternate screen");
        assert_eq!(text(&c.rows[0]), "alt");
        assert_eq!(c.cursor, (3, 0), "the alternate grid's cursor");
        assert!(c.rows[1..].iter().all(|r| text(r).is_empty()));
        s.feed(b"\x1b[?1049l");
        let c = s.replay_capture().expect("back to the main screen");
        assert_eq!(text(&c.rows[3]), "z");
        assert_eq!(c.cursor, (1, 3), "the main grid's cursor");
    }

    /// `TV:2910-2917`: a frame is saved only when something is dirty. alacritty reports full
    /// damage on every read in insert mode, and a view scrolled back reads as full damage;
    /// an idle screen still gives no frame. A resize gives one frame, then none.
    #[test]
    fn idle_full_damage_captures_nothing() {
        use alacritty_terminal::grid::Scroll;
        let mut s = Screen::with_scrollback(10, 4, 100);
        s.feed(b"1\r\n2\r\n3\r\n4\r\n5\r\n6");
        assert!(s.replay_capture().is_some());
        s.term_mut().scroll_display(Scroll::Delta(1));
        assert!(s.term().grid().display_offset() > 0);
        assert!(s.replay_capture().is_none(), "scrolling the view is not a change of the screen");
        assert!(s.replay_capture().is_none());
        s.feed(b"7");
        let c = s.replay_capture().expect("output while scrolled back");
        assert_eq!(c.clean, vec![false; 4]);
        assert!(s.replay_capture().is_none());
        s.term_mut().scroll_display(Scroll::Bottom);
        s.feed(b"\x1b[4h");
        let _ = s.replay_capture();
        assert!(s.replay_capture().is_none(), "insert mode, nothing written");
        s.feed(b"x");
        assert!(s.replay_capture().is_some(), "insert mode, a character written");
        assert!(s.replay_capture().is_none());
        s.feed(b"\x1b[4l");
        let _ = s.replay_capture();
        s.resize(12, 5);
        let c = s.replay_capture().expect("a resize");
        assert_eq!((c.rows.len(), c.rows[0].len(), c.clean), (5, 12, vec![false; 5]));
        assert!(s.replay_capture().is_none(), "nothing since the resize");
    }

    #[test]
    fn turning_app_key_modes_off_clears_them() {
        use crate::keyproto::KeyMode;
        let mut s = Screen::new(20, 4);
        s.feed(b"\x1b[>4;2m");
        s.set_look(Look { app_key_modes: false, ..Look::default() });
        assert_eq!(s.keyboard().mode, KeyMode::Standard);
    }

    #[test]
    fn the_tick_digest_follows_marks_notes_and_rows() {
        let mut s = Screen::new(20, 4);
        let base = s.tick_digest();
        assert_eq!(s.tick_digest(), base, "nothing changed");
        let mark = s.set_manual_mark(0, None);
        let with_mark = s.tick_digest();
        assert_ne!(with_mark, base, "a mark was added");
        assert!(s.remove_mark(mark));
        assert_ne!(s.tick_digest(), with_mark, "and removed");
        s.add_note(0, 0, 0, 3, "n", false);
        let with_note = s.tick_digest();
        assert_ne!(with_note, base, "a note was added");
        s.feed(b"1\r\n2\r\n3\r\n4\r\n5\r\n");
        assert_ne!(s.tick_digest(), with_note, "rows scrolled");
        let before = s.tick_digest();
        s.feed(b"x");
        assert_ne!(s.tick_digest(), before, "every output batch bumps the generation");
        let again = s.tick_digest();
        assert_eq!(s.tick_digest(), again, "and nothing else moves it");
        let before = again;
        s.feed(b"\x1b[?1049h");
        assert_ne!(s.tick_digest(), before, "the alternate screen shows");
        // An exit code arriving on an existing prompt mark changes the ticks' series.
        let mut p = Screen::new(20, 4);
        p.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07");
        let running = p.tick_digest();
        p.feed(b"\x1b]133;D;1\x07");
        assert_ne!(p.tick_digest(), running, "the command finished with a code");
    }

    /// The tick cache's key moves with any grid change: erasers and movers that touch no record.
    #[test]
    fn the_tick_generation_follows_every_grid_change() {
        fn moves(stale: &mut Vec<String>, what: &str, setup: &[u8], act: &dyn Fn(&mut Screen)) {
            let mut s = Screen::new(20, 6);
            s.feed(setup);
            s.set_manual_mark(1, None);
            let before = s.tick_digest();
            act(&mut s);
            if s.tick_digest() == before {
                stale.push(what.to_string());
            }
        }
        let mut stale = Vec::new();
        let text = b"a\r\nb\r\nc\r\nd";
        moves(&mut stale, "ED 0", text, &|s| {
            s.feed(b"\x1b[2;1H\x1b[J");
        });
        moves(&mut stale, "ED 1", text, &|s| {
            s.feed(b"\x1b[3;1H\x1b[1J");
        });
        moves(&mut stale, "IL", text, &|s| {
            s.feed(b"\x1b[2;1H\x1b[L");
        });
        moves(&mut stale, "DL", text, &|s| {
            s.feed(b"\x1b[2;1H\x1b[M");
        });
        moves(&mut stale, "a region scroll", text, &|s| {
            s.feed(b"\x1b[1;3r\x1b[3;1H\n");
        });
        moves(&mut stale, "fold", text, &|s| {
            s.fold(0, 1);
        });
        moves(&mut stale, "RIS with no history", text, &|s| {
            s.feed(b"\x1bc");
        });
        moves(&mut stale, "clear scrollback", text, &|s| s.clear(true));
        moves(&mut stale, "resize", text, &|s| s.resize(10, 6));
        moves(&mut stale, "a mark removed", text, &|s| {
            let id = s.row_marks()[0].1;
            s.remove_mark(id);
        });
        assert!(stale.is_empty(), "the ticks would go stale after: {stale:?}");
    }

    /// A screenshot or preview draws rows as the pane does: Bidi off stays off, and Blink
    /// Allowed carries over.
    #[test]
    fn a_slice_keeps_the_bidi_and_blink_rules() {
        let mut s = crate::screen::Screen::new(10, 3);
        s.feed("\u{5d0}\u{5d1}".as_bytes());
        assert!(crate::screenshot::slice(&s, 0, 3).row_order(0).is_some(), "Bidi on reorders the slice");
        s.bidi_rules.enabled = false;
        s.blink_allowed = true;
        let cut = crate::screenshot::slice(&s, 0, 3);
        assert!(cut.row_order(0).is_none(), "Bidi off stays off in the slice");
        assert!(cut.blink_allowed);
    }

    /// 13o Task 1: the plumb rules reach the grid and the scanner as iTerm2's do.
    #[test]
    fn plumb_sites_follow_iterm2() {
        use crate::emu::Rules;
        let col_of = |s: &Screen, ch: char| (0..20).find(|c| s.term.grid()[Line(0)][Column(*c)].c == ch);
        let mut s = Screen::new(20, 3);
        s.set_rules(Rules { tab_width: 4, ..Default::default() });
        s.feed(b"\ta");
        assert_eq!(col_of(&s, 'a'), Some(4));
        let mut s = Screen::new(20, 3);
        s.feed(b"\ta");
        assert_eq!(col_of(&s, 'a'), Some(8));
        // Only a new session takes the width: a screen that has fed bytes keeps its stops.
        let mut s = Screen::new(20, 3);
        s.feed(b"x");
        s.set_rules(Rules { tab_width: 4, ..Default::default() });
        s.feed(b"\ta");
        assert_eq!(col_of(&s, 'a'), Some(8));
        assert_eq!(s.rules().tab_width, 4, "the rules are stored either way");

        let mut s = Screen::new(20, 3);
        s.set_rules(Rules { xterm_version: 95, ..Default::default() });
        assert_eq!(String::from_utf8(s.feed(b"\x1b[>c")).unwrap(), "\x1b[>64;95;0c");

        let mut s = Screen::new(20, 3);
        let bg = s.palette.bg;
        s.set_rules(Rules { color_16: false, ..Default::default() });
        let want = format!("\x1b]11;rgb:{:02x}/{:02x}/{:02x}\x07", bg.r, bg.g, bg.b);
        assert_eq!(String::from_utf8(s.feed(b"\x1b]11;?\x07")).unwrap(), want);
        let mut s = Screen::new(20, 3);
        let reply = String::from_utf8(s.feed(b"\x1b]11;?\x07")).unwrap();
        assert_eq!(reply, format!("\x1b]11;rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}\x07", bg.r, bg.g, bg.b));

        let mut s = Screen::new(20, 3);
        s.set_rules(Rules { accept_osc7: false, ..Default::default() });
        s.feed(b"\x1b]7;file://h/tmp\x07");
        assert_eq!(s.shell.cwd, None);
        assert!(s.take_requests().is_empty(), "no machine id: no warning");
        s.feed(b"\x1b]7;file://h/tmp?machineID=ab\x07");
        assert_eq!(s.shell.cwd, None);
        let asked = |s: &mut Screen| s.take_requests().iter().filter(|r| matches!(r, Request::Osc7Disabled)).count();
        assert_eq!(asked(&mut s), 1);
        s.feed(b"\x1b]7;file://h/tmp?machineID=ab\x07");
        assert_eq!(asked(&mut s), 1, "the screen does not latch");
        s.feed(b"\x1b]1337;CurrentDir=/x\x07");
        s.take_requests();
        s.feed(b"\x1b]7;file://h/tmp?machineID=ab\x07");
        assert_eq!(asked(&mut s), 0, "a proprietary code was seen on this screen");
        let mut other = Screen::new(20, 3);
        other.set_rules(Rules { accept_osc7: false, ..Default::default() });
        other.feed(b"\x1b]7;file://h/tmp?machineID=ab\x07");
        assert_eq!(asked(&mut other), 1);
        // On, OSC 7 sets the folder and never warns.
        let mut on = Screen::new(20, 3);
        on.feed(b"\x1b]7;file://h/tmp?machineID=ab\x07");
        assert_eq!(on.shell.cwd.as_deref(), Some("/tmp"));
        assert_eq!(asked(&mut on), 0);
    }

    /// Preflight B5: the key gates the MODE, so a refused DECSET 1004 shows in DECRQM.
    #[test]
    fn focus_reporting_key_gates_the_mode() {
        use crate::emu::Rules;
        let mut s = Screen::new(20, 3);
        s.set_rules(Rules { focus_reporting: false, ..Default::default() });
        s.feed(b"\x1b[?1004h");
        assert!(!s.term().mode().contains(TermMode::FOCUS_IN_OUT));
        assert_eq!(String::from_utf8(s.feed(b"\x1b[?1004$p")).unwrap(), "\x1b[?1004;2$y");
        s.set_rules(Rules::default());
        s.feed(b"\x1b[?1004h");
        assert!(s.term().mode().contains(TermMode::FOCUS_IN_OUT));
        assert_eq!(String::from_utf8(s.feed(b"\x1b[?1004$p")).unwrap(), "\x1b[?1004;1$y");
        // A reset is never refused.
        s.set_rules(Rules { focus_reporting: false, ..Default::default() });
        s.feed(b"\x1b[?1004l");
        assert!(!s.term().mode().contains(TermMode::FOCUS_IN_OUT));
    }

    /// 13o Task 2 (Review Focus 2): a held sequence whose start was fed and whose end is dropped
    /// leaves the parser in the ground state.
    #[test]
    fn held_sequences_survive_split_reads() {
        use crate::emu::{held_bytes, Rules};
        let mut s = Screen::new(20, 3);
        s.set_rules(Rules { rep: false, ..Default::default() });
        s.feed(b"a\x1b[3");
        s.feed(b"bc");
        assert_eq!(row_text(&s, 0), "ac");
        s.feed(b"\x1b[1md");
        assert!(s.term.grid()[Line(0)][Column(2)].flags.contains(alacritty_terminal::term::cell::Flags::BOLD), "the parser is in the ground state");
        assert_eq!(row_text(&s, 0), "acd");
        // The same drop inside one read, and with the sequence split in two places.
        let mut s = Screen::new(20, 3);
        s.set_rules(Rules { rep: false, ..Default::default() });
        s.feed(b"a\x1b[");
        s.feed(b"0");
        s.feed(b"3b");
        s.feed(b"c");
        assert_eq!(row_text(&s, 0), "ac");
        // A CAN goes first whatever happens to the sequence, so the parser is in the ground state.
        assert_eq!(held_bytes(None), vec![0x18]);
        assert_eq!(held_bytes(Some(b"\x1b[5b".to_vec())), b"\x18\x1b[5b");
    }

    /// 13o Task 2: the REP switch, and REP read by number.
    #[test]
    fn rep_follows_iterm2() {
        use crate::emu::Rules;
        let mut s = Screen::new(20, 3);
        s.feed(b"a\x1b[3b");
        assert_eq!(row_text(&s, 0), "aaaa");
        let mut s = Screen::new(20, 3);
        s.set_rules(Rules { rep: false, ..Default::default() });
        s.feed(b"a\x1b[3b");
        assert_eq!(row_text(&s, 0), "a");
        assert_eq!(s.term.grid().cursor.point.column, Column(1));
        // Padding and the colon form still reach the switch, and a padded REP runs shortened.
        let mut s = Screen::new(20, 3);
        s.set_rules(Rules { rep: false, ..Default::default() });
        s.feed(b"a\x1b[0000003b\x1b[3:1b");
        assert_eq!(row_text(&s, 0), "a");
        let mut s = Screen::new(20, 3);
        s.feed(b"a\x1b[0000003b");
        assert_eq!(row_text(&s, 0), "aaaa");
        // A huge count is bounded by the parameter width (65535), however many digits.
        let mut s = Screen::new(10, 2);
        s.feed(b"a\x1b[99999999999999999999b");
        assert_eq!(s.term.grid().cursor.point.line, Line(1));
    }

    /// 13o Task 6 (Review Focus 5): DECRQCRA answers only when allowed, trusted and at VT400.
    #[test]
    fn decrqcra_is_gated() {
        use crate::emu::Rules;
        let sequence = b"\x1b[1;1;1;1;1;2*y";
        let make = |off: Option<bool>, level: u16, trusted: bool| {
            let mut s = Screen::new(20, 3);
            s.set_rules(Rules { decrqcra_off: off, ..Default::default() });
            s.set_level(level);
            s.trusted = trusted;
            s.feed(b"ab");
            s
        };
        let mut s = make(None, 500, true);
        assert_eq!(s.feed(sequence), b"", "unset: nothing answered");
        assert_eq!(s.take_requests(), vec![Request::AskDecrqcra]);
        s.feed(sequence);
        s.feed(sequence);
        assert_eq!(s.take_requests(), vec![Request::AskDecrqcra], "a flood asks once");
        let mut s = make(Some(true), 500, true);
        assert_eq!(s.feed(sequence), b"");
        assert!(s.take_requests().is_empty());
        let mut s = make(Some(false), 500, true);
        assert_eq!(s.feed(sequence), b"\x1bP1!~00c3\x1b\\");
        assert!(s.take_requests().is_empty());
        assert_eq!(s.feed(b"\x1b[7;1;1;1;1;1*y"), b"\x1bP7!~0061\x1b\\", "the id is echoed; one cell");
        assert_eq!(s.feed(b"\x1b[7;1;3;3;2;2*y"), b"\x1bP7!~0000\x1b\\", "a negative rectangle is checksum 0");
        let mut s = make(Some(false), 500, true);
        s.feed(b"\r\n\x1b[2;1H\xe2\x82\xac");
        assert_eq!(s.feed(b"\x1b[3;1;2;1;2;1*y"), b"\x1bP3!~20ac\x1b\\", "a euro sign is one UTF-16 unit");
        for (level, trusted) in [(300, true), (500, false), (300, false)] {
            let mut s = make(Some(false), level, trusted);
            assert_eq!(s.feed(sequence), b"", "level {level} trusted {trusted}");
            assert!(s.take_requests().is_empty());
            let mut s = make(None, level, trusted);
            s.feed(sequence);
            assert!(s.take_requests().is_empty(), "never asks at level {level} trusted {trusted}");
        }
        // The sequence is dropped, never printed, and text after it prints.
        let mut s = make(Some(false), 500, true);
        s.feed(b"\x1b[1;1;1;1;1;2*yZ");
        assert_eq!(row_text(&s, 0), "abZ");
    }

    /// 13o Task 6 (Review Focus 5): CSI 21 t answers only with Allow Title Reporting.
    #[test]
    fn title_report_is_gated() {
        let allow = Look { allow_title_reporting: true, ..Look::default() };
        let mut s = Screen::new(20, 3);
        assert_eq!(s.feed(b"\x1b]2;x\x07\x1b[21t"), b"", "off by default");
        let mut s = Screen::with_look(20, 3, Palette::default(), allow);
        assert_eq!(s.feed(b"\x1b]2;x\x07\x1b[21t"), b"\x1b]lx\x1b\\");
        assert_eq!(s.feed(b"\x1b[20t"), b"\x1b]Lx\x1b\\");
        s.trusted = false;
        assert_eq!(s.feed(b"\x1b[21t"), b"", "untrusted");
        s.trusted = true;
        // A title with controls in it never reaches the shell as controls.
        s.feed(b"\x1b]2;a|b;c\x03d\x0fe\x07");
        assert_eq!(s.feed(b"\x1b[21t"), b"\x1b]la b cde\x1b\\", "vte drops C0 inside an OSC; the rest is spaced");
        let mut t = Screen::with_look(20, 3, Palette::default(), allow);
        t.feed(b"\x1b]2;\xc2\x9b31m\x07");
        let report = t.feed(b"\x1b[21t");
        assert!(!String::from_utf8_lossy(&report).contains('\u{9b}'), "{report:?}");
        // Rows still print around a held report.
        s.feed(b"\x1b[21tok");
        assert!(row_text(&s, 0).ends_with("ok"));
    }

    /// 13o Task 6: ENQ answers the profile's answerback.
    #[test]
    fn answerback_follows_iterm2() {
        let mut s = Screen::new(20, 3);
        assert_eq!(s.feed(b"\x05"), b"", "empty by default");
        s.set_answerback("hi".into());
        assert_eq!(s.feed(b"\x05"), b"hi");
        assert_eq!(s.feed(b"a\x05b"), b"hi");
        assert_eq!(row_text(&s, 0), "ab", "ENQ prints nothing");
        assert_eq!(s.feed(b"\x1b]2;a\x05b\x07"), b"", "ENQ inside an OSC is data");
        assert_eq!(s.title(), Some("ab"), "vte drops it from the title");
        s.set_answerback("x".repeat(1000));
        assert_eq!(s.feed(b"\x05").len(), 256, "bounded");
        s.set_answerback("\u{e9}".repeat(200));
        let r = s.feed(b"\x05");
        assert!(r.len() <= 256 && String::from_utf8(r).is_ok(), "cut on a character");
    }

    /// 13o Task 6 (Review Focus 10, R8): DEC mode 2048.
    #[test]
    fn resize_reports_follow_iterm2() {
        let mut s = Screen::new(20, 3);
        assert!(!s.resize_reports());
        assert_eq!(s.feed(b"\x1b[?2048$p"), b"\x1b[?2048;2$y");
        assert_eq!(s.feed(b"\x1b[?1;2048h"), b"", "no report on enabling (iTerm2)");
        assert!(s.resize_reports());
        assert_eq!(s.feed(b"\x1b[?2048$p"), b"\x1b[?2048;1$y");
        s.feed(b"\x1b[?2048l");
        assert!(!s.resize_reports());
        s.feed(b"\x1b[?2048h\x1bc");
        assert!(!s.resize_reports(), "RIS");
        s.feed(b"\x1b[?2048h");
        s.soft_reset();
        assert!(!s.resize_reports(), "DECSTR");
        s.feed(b"\x1b[?2048h");
        s.set_resize_reports(false);
        assert!(!s.resize_reports());
        s.set_resize_reports(true);
        assert!(s.resize_reports());
    }

    /// 13o Task 6 fix: ENQs in one output batch coalesce and DECRQCRA replies are bounded.
    #[test]
    fn report_floods_are_bounded() {
        use crate::emu::Rules;
        let mut s = Screen::new(20, 3);
        s.set_answerback("hi".into());
        assert_eq!(s.feed(&b"\x05".repeat(1000)), b"hi", "one answerback per batch");
        assert_eq!(s.feed(b"\x05\x05"), b"hi", "the next batch answers again");
        let mut s = Screen::new(20, 3);
        s.set_rules(Rules { decrqcra_off: Some(false), ..Default::default() });
        s.set_level(500);
        s.trusted = true;
        let one = b"\x1b[1;1;1;1;1;1*y";
        let reply = s.feed(&one.repeat(1000));
        assert_eq!(reply.len(), 64 * b"\x1bP1!~0000\x1b\\".len(), "64 replies a batch");
    }

    /// 13o Task 7 (Review Focus 6): Disable Smcup Rmcup keeps the primary screen up and the other
    /// modes of the list apply.
    #[test]
    fn disable_smcup_follows_iterm2() {
        let mut s = Screen::new(20, 3);
        s.feed(b"\x1b[?1;1049h");
        assert!(s.alt_screen());
        let mut s = Screen::new(20, 3);
        s.set_look(Look { disable_smcup: true, ..Look::default() });
        s.feed(b"\x1b[?1;1049h");
        assert!(!s.alt_screen());
        assert!(s.term().mode().contains(TermMode::APP_CURSOR));
        // Padded spellings are the same sequence; the alone switch does nothing.
        s.feed(b"\x1b[?0001049h\x1b[?47h\x1b[?1047h");
        assert!(!s.alt_screen());
    }

    /// 13o Task 7 (B8): CSI 5 i collects text until CSI 4 i; the text never reaches the screen.
    #[test]
    fn print_capture_follows_iterm2() {
        let mut s = Screen::new(20, 4);
        s.feed(b"a\x1b[5ib\x1b[31mc\r\nd\x1b[4ie");
        assert_eq!(row_text(&s, 0), "ae");
        assert_eq!(s.take_requests(), vec![Request::Print(PrintJob::Text("bc\r\nd".into()))]);
        s.feed(b"\x1b[i");
        assert_eq!(s.take_requests(), vec![Request::Print(PrintJob::Screen)]);
        s.feed(b"\x1b[0i\x1b[2i");
        assert_eq!(s.take_requests(), vec![Request::Print(PrintJob::Screen)], "one pending print at a time");
        // Split reads.
        s.feed(b"\x1b[");
        s.feed(b"5ixy");
        s.feed(b"z\x1b[4");
        s.feed(b"i");
        assert_eq!(s.take_requests(), vec![Request::Print(PrintJob::Text("xyz".into()))]);
        // An empty job prints nothing.
        s.feed(b"\x1b[5i\x1b[4i");
        assert_eq!(s.take_requests(), vec![]);
        // Disable Printing: no capture starts, nothing is requested.
        let mut s = Screen::new(20, 4);
        s.set_look(Look { disable_printing: true, ..Look::default() });
        s.take_requests();
        s.feed(b"\x1b[5iabc\x1b[4i\x1b[i");
        assert_eq!(row_text(&s, 0), "abc");
        assert!(!s.take_requests().iter().any(|r| matches!(r, Request::Print(_))));
    }

    /// Task 7 fix (P21): a flood of CSI i queues one print request, not one alert each.
    #[test]
    fn print_floods_are_bounded() {
        let mut s = Screen::new(20, 4);
        s.feed(&b"\x1b[i".repeat(1000));
        assert_eq!(s.take_requests(), vec![Request::Print(PrintJob::Screen)]);
        s.feed(b"\x1b[i");
        assert_eq!(s.take_requests(), vec![Request::Print(PrintJob::Screen)], "the next batch asks again once it was taken");
    }

    /// Task 7 fix: a title report answers once a batch, as the answerback does.
    #[test]
    fn title_reports_are_one_per_batch() {
        let mut s = Screen::new(20, 4);
        s.set_look(Look { allow_title_reporting: true, ..Look::default() });
        s.trusted = true;
        s.feed(b"\x1b]2;hello\x07");
        let one = s.feed(b"\x1b[21t");
        assert_eq!(one, b"\x1b]lhello\x1b\\", "a title report answers");
        assert_eq!(s.feed(&b"\x1b[21t".repeat(1000)), one, "one reply per batch");
        assert_eq!(s.feed(&b"\x1b[20t\x1b[21t".repeat(10)), b"\x1b]Lhello\x1b\\", "20 and 21 share the one reply");
    }

    /// 13o Task 7 (B8): a capture with no end is bounded and never swallows all output.
    #[test]
    fn print_capture_is_bounded() {
        let mut s = Screen::new(20, 4);
        s.limits.copy = 64;
        s.feed(b"\x1b[5i");
        for _ in 0..40 {
            s.feed(b"0123456789");
        }
        s.feed(b"\x1b[2J\x1b[Hshown");
        assert_eq!(row_text(&s, 0), "shown");
        assert_eq!(s.take_requests(), vec![], "nothing is printed");
        assert!(s.print_capture.is_none());
        // Time: a capture older than the bound ends at the next output.
        s.feed(b"\x1b[5iold");
        s.backdate_print_capture();
        s.feed(b"\r\nnew");
        assert_eq!(row_text(&s, 1), "new");
        assert_eq!(s.take_requests(), vec![]);
    }

    /// 13o final review (Task 7 minor 3): a capture that ends by age in the middle of a sequence
    /// drops the rest of that sequence, so its tail never prints as text.
    #[test]
    fn a_print_capture_ending_by_age_mid_sequence_drops_the_tail() {
        let mut s = Screen::new(20, 4);
        s.feed(b"\x1b[5ihidden\x1b[1");
        s.backdate_print_capture();
        s.feed(b";31mshown");
        assert_eq!(row_text(&s, 0), "shown", "the tail of the sequence is not text");
        assert!(s.print_capture.is_none());
        assert_eq!(s.take_requests(), vec![]);
        // A string (OSC) cut the same way.
        let mut s = Screen::new(20, 4);
        s.feed(b"\x1b[5i\x1b]0;ti");
        s.backdate_print_capture();
        s.feed(b"tle\x07shown");
        assert_eq!(row_text(&s, 0), "shown");
    }

    /// 13o final review (new item 7): Reset cancels a sequence the program left open, in the
    /// scanner as in the screen's parser, so the tail of that sequence is not read as an event.
    #[test]
    fn reset_cancels_an_open_sequence_in_the_scanner() {
        let mut s = Screen::new(20, 4);
        s.feed(b"\x1b]52;c;QQ==");
        s.soft_reset();
        s.feed(b"\x07text");
        assert_eq!(row_text(&s, 0), "text");
        assert!(!s.take_requests().iter().any(|r| matches!(r, Request::ClipboardWrite(_))), "the cancelled OSC 52 wrote nothing");
    }

    /// 13o final review (Task 7 minor 1): the person's Reset works while a print job collects.
    #[test]
    fn reset_ends_a_print_capture() {
        let mut s = Screen::new(20, 4);
        s.feed(b"\x1b[?2004h\x1b[5ihidden");
        assert!(s.print_capture.is_some());
        s.soft_reset();
        assert!(s.print_capture.is_none(), "the capture ends");
        assert!(!s.term().mode().contains(TermMode::BRACKETED_PASTE), "and the reset reached the parser");
        s.feed(b"after");
        assert_eq!(row_text(&s, 0), "after");
    }

    /// 13o Task 7 (B8): RIS, a prompt mark and a command end each end the capture unprinted.
    #[test]
    fn print_capture_ends_at_a_prompt() {
        for end in [&b"\x1bc"[..], b"\x1b]133;A\x07", b"\x1b]133;D;0\x07"] {
            let mut s = Screen::new(20, 4);
            s.feed(b"\x1b[5ihidden");
            s.feed(end);
            s.feed(b"visible");
            assert_eq!(row_text(&s, 0), "visible", "{end:?}");
            assert_eq!(s.take_requests(), vec![], "{end:?}");
        }
    }

    /// 13o final review (Task 7 minor 2): with the switch on, a tab at the last column wraps even
    /// with auto-wrap off (`MS:2814-2817` does not read DECAWM).
    #[test]
    fn tabs_wrap_ignores_auto_wrap() {
        let cell = |s: &Screen, l: i32, c: usize| s.term.grid()[Line(l)][Column(c)].c;
        let mut s = Screen::new(10, 3);
        s.set_rules(crate::emu::Rules { tabs_wrap: true, ..Default::default() });
        s.feed(b"\x1b[?7l\x1b[1;10H\tx");
        assert_eq!(cell(&s, 1, 8), 'x', "wrapped to the next row, to the stop after column 0");
        assert_eq!(cell(&s, 0, 9), ' ');
    }

    /// 13o Task 7: tabs wrap only with the switch; the pending-wrap tab does nothing either way.
    #[test]
    fn tabs_wrap_follows_iterm2() {
        use alacritty_terminal::term::cell::Flags;
        let cell = |s: &Screen, l: i32, c: usize| s.term.grid()[Line(l)][Column(c)].c;
        let mut s = Screen::new(10, 3);
        s.feed(b"\x1b[1;10H\tx");
        assert_eq!(cell(&s, 0, 9), 'x');
        assert_eq!(cell(&s, 1, 8), ' ');
        let mut s = Screen::new(10, 3);
        s.set_rules(crate::emu::Rules { tabs_wrap: true, ..Default::default() });
        s.feed(b"\x1b[1;10H\tx");
        assert_eq!(cell(&s, 1, 8), 'x');
        assert!(s.term.grid()[Line(0)][Column(9)].flags.contains(Flags::WRAPLINE), "soft wrap");
        // Mid-line tabs are unchanged with the switch on.
        s.feed(b"\x1b[3;1H\ty");
        assert_eq!(cell(&s, 2, 8), 'y');
        // A pending wrap: the tab does nothing, and the next character wraps as usual.
        let mut s = Screen::new(10, 3);
        s.feed(b"0123456789\tz");
        assert_eq!(cell(&s, 0, 9), '9');
        assert_eq!(cell(&s, 1, 0), 'z');
    }

    /// 13o Task 3 (Review Focus 1): CSI 3 J asks while the key is unset and the history stays.
    #[test]
    fn csi_3j_is_held_and_asked() {
        use crate::emu::Rules;
        let hist = |s: &Screen| s.term().grid().history_size();
        let filled = |rules: Rules| {
            let mut s = Screen::new(20, 3);
            s.set_rules(rules);
            for i in 0..52 {
                s.feed(format!("line{i}\r\n").as_bytes());
            }
            s.take_requests();
            assert_eq!(hist(&s), 50);
            s
        };
        let mut s = filled(Rules::default());
        s.feed(b"\x1b[3J");
        assert_eq!(hist(&s), 50);
        assert_eq!(s.take_requests(), vec![Request::AskClearHistory]);
        // Output keeps flowing while the ask is pending, and a flood asks once.
        s.feed(b"\x1b[3J\x1b[3J\x1b[000003J\x1b[3:1Jtail");
        assert_eq!(hist(&s), 50);
        assert!(row_text(&s, 2).contains("tail") || row_text(&s, 1).contains("tail"));
        assert_eq!(s.take_requests(), vec![Request::AskClearHistory]);
        // A split read leaves the parser clean.
        s.feed(b"\x1b[");
        s.feed(b"3Jok");
        s.take_requests();
        assert!((0..3).any(|l| row_text(&s, l).contains("ok")));
        let mut s = filled(Rules { clear_history: Some(false), ..Default::default() });
        s.feed(b"\x1b[3J");
        assert_eq!(hist(&s), 0);
        assert!(s.take_requests().is_empty());
        let mut s = filled(Rules { clear_history: Some(true), ..Default::default() });
        s.feed(b"\x1b[3J\x1b[0003J");
        assert_eq!(hist(&s), 50);
        assert!(s.take_requests().is_empty());
        // ED 2 is never held and never touches the history.
        let mut s = filled(Rules::default());
        s.feed(b"\x1b[2J");
        assert!(hist(&s) >= 50);
        assert!(s.take_requests().is_empty());
    }

    /// 13o Task 3: the user's clears and OSC 1337 ClearScrollback are never gated.
    #[test]
    fn user_clears_are_not_gated() {
        let hist = |s: &Screen| s.term().grid().history_size();
        let filled = || {
            let mut s = Screen::new(20, 3);
            for i in 0..52 {
                s.feed(format!("line{i}\r\n").as_bytes());
            }
            s
        };
        let mut s = filled();
        assert_eq!(hist(&s), 50);
        s.clear(true);
        assert_eq!(hist(&s), 0);
        let mut s = filled();
        s.feed(b"\x1b]1337;ClearScrollback\x07");
        assert_eq!(hist(&s), 0);
        assert!(s.take_requests().is_empty());
    }

    /// 13o Task 2: the OSC 8 length cap.
    #[test]
    fn url_length_follows_iterm2() {
        use crate::emu::Rules;
        let link = |s: &Screen| s.term.grid()[Line(0)][Column(0)].hyperlink().map(|h| h.uri().to_string());
        let feed = |max: usize, text: &[u8]| {
            let mut s = Screen::new(20, 3);
            s.set_rules(Rules { max_url: max, ..Default::default() });
            s.feed(text);
            s
        };
        let long = b"\x1b]8;;http://a.b/cdefgh\x1b\\x\x1b]8;;\x1b\\";
        assert_eq!(link(&feed(10, long)), None);
        assert_eq!(link(&feed(2_097_152, long)).as_deref(), Some("http://a.b/cdefgh"));
        assert_eq!(link(&feed(10, b"\x1b]8;;http://a\x1b\\y")).as_deref(), Some("http://a"));
        // A refused link neither starts nor ends one: the open link stays open.
        let s = feed(10, b"\x1b]8;;http://a\x1b\\x\x1b]8;;http://a.b/cdefgh\x1b\\y");
        assert_eq!(s.term.grid()[Line(0)][Column(1)].hyperlink().map(|h| h.uri().to_string()).as_deref(), Some("http://a"));
        // A later OSC 8 with no URL still ends the link.
        let s = feed(10, b"\x1b]8;;http://a\x1b\\x\x1b]8;;\x1b\\y");
        assert!(s.term.grid()[Line(0)][Column(1)].hyperlink().is_none());
    }

    /// 13o Task 2 (B2): a held event that reads state runs after a synchronized update is drawn.
    #[test]
    fn held_reads_flush_synchronized_output() {
        let mut s = Screen::new(20, 3);
        s.feed(b"\x1b[?2026hab\x1b[5i");
        assert_eq!(row_text(&s, 0), "ab", "the update was flushed before the held sequence");
    }

    /// 13o Task 2: a string over the cap is cut at the cap and its tail does not print.
    #[test]
    fn a_huge_osc_does_not_print_its_tail() {
        let mut s = Screen::new(20, 3);
        let mut big = b"\x1b]0;".to_vec();
        big.extend(std::iter::repeat_n(b'A', (1 << 20) + 10));
        big.extend(b"\x07z");
        s.feed(&big);
        assert_eq!(row_text(&s, 0), "z");
        // Like iTerm2, the string is cut at the cap (vte ends it on CAN).
        assert!(s.title().is_some_and(|t| t.len() <= 1 << 20), "the string is cut at the cap");
        s.feed(b"\x1b]0;ok\x07");
        assert_eq!(s.title(), Some("ok"));
    }

    /// Every spelling vte takes as CSI 3 J is gated, including a C0 byte, DEL or a byte of 0x80 or
    /// more between the ESC and the `[` (vte stays in Escape through them) and C0 bytes inside the
    /// sequence (13o Task 2 fix, B1).
    #[test]
    fn every_vte_spelling_of_csi_3j_is_gated() {
        use crate::emu::Rules;
        let forms: Vec<Vec<u8>> = vec![
            b"\x1b[3J".to_vec(),
            b"\x1b\x00[3J".to_vec(),
            b"\x1b\x07[3J".to_vec(),
            b"\x1b\r[3J".to_vec(),
            b"\x1b\x7f[3J".to_vec(),
            b"\x1b\xc3[3J".to_vec(),
            b"\x1b\x9b[3J".to_vec(),
            b"\x1b\x00\x7f\xff[3J".to_vec(),
            b"\x1b[\x003J".to_vec(),
            b"\x1b[3\rJ".to_vec(),
            b"\x1b[3\x7fJ".to_vec(),
            b"\x1b[0003J".to_vec(),
            b"\x1b[3:1J".to_vec(),
            b"\x1b\x1b[3J".to_vec(),
            b"\x1b[1\x1b[3J".to_vec(),
        ];
        for form in forms {
            let mut s = Screen::with_scrollback(20, 3, 100);
            for i in 0..52 {
                s.feed(format!("line{i}\r\n").as_bytes());
            }
            s.take_requests();
            let before = s.term().grid().history_size();
            s.feed(&form);
            s.feed(b"after");
            assert_eq!(s.term().grid().history_size(), before, "{form:?} cleared history");
            assert_eq!(s.take_requests(), vec![Request::AskClearHistory], "{form:?} did not ask");
            assert!((0..3).any(|l| row_text(&s, l).contains("after")), "{form:?} left the parser mid-sequence");
            // The same spelling split at every byte.
            for cut in 1..form.len() {
                let mut s = Screen::with_scrollback(20, 3, 100);
                for i in 0..52 {
                    s.feed(format!("line{i}\r\n").as_bytes());
                }
                s.take_requests();
                s.feed(&form[..cut]);
                s.feed(&form[cut..]);
                s.feed(b"after");
                assert_eq!(s.term().grid().history_size(), before, "{form:?} split at {cut} cleared history");
                assert!((0..3).any(|l| row_text(&s, l).contains("after")), "{form:?} split at {cut} swallowed text");
            }
            // With the key set to allow, the same spelling clears.
            let mut s = Screen::with_scrollback(20, 3, 100);
            s.set_rules(Rules { clear_history: Some(false), ..Default::default() });
            for i in 0..52 {
                s.feed(format!("line{i}\r\n").as_bytes());
            }
            s.feed(&form);
            assert_eq!(s.term().grid().history_size(), 0, "{form:?} should clear when allowed");
        }
    }

    /// The REP switch and the OSC cut see the same forms vte does.
    #[test]
    fn other_gates_see_vte_spellings() {
        use crate::emu::Rules;
        let mut s = Screen::new(20, 3);
        s.set_rules(Rules { rep: false, ..Default::default() });
        s.feed(b"a\x1b\x00[3b\x1b\x7f[3b\x1b[\x003b");
        assert_eq!(row_text(&s, 0), "a");
        let mut s = Screen::new(20, 3);
        let mut big = b"\x1b\x00]0;".to_vec();
        big.extend(std::iter::repeat_n(b'A', (1 << 20) + 10));
        big.extend(b"\x07z");
        s.feed(&big);
        assert_eq!(row_text(&s, 0), "z");
        assert!(s.title().is_some_and(|t| t.len() <= 1 << 20), "the string is cut at the cap");
    }

    /// Random streams of the bytes that matter never clear history while the key is unset: what
    /// the scanner holds is what vte (alacritty) would run, whatever the spelling or the splits.
    #[test]
    fn random_streams_never_clear_history_unasked() {
        const ALPHABET: &[u8] = b"\x1b\x1b\x1b\x1b[[[3333JJJ;:0\x00\x07\r\x7f\x18\x1a\xc3\x9b\\]P_X^?q";
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..3000 {
            let mut s = Screen::with_scrollback(20, 3, 100);
            for i in 0..52 {
                s.feed(format!("line{i}\r\n").as_bytes());
            }
            s.take_requests();
            let before = s.term().grid().history_size();
            let len = 4 + (next() % 20) as usize;
            let stream: Vec<u8> = (0..len).map(|_| ALPHABET[(next() % ALPHABET.len() as u64) as usize]).collect();
            let mut at = 0;
            while at < stream.len() {
                let n = 1 + (next() % 7) as usize;
                let end = (at + n).min(stream.len());
                s.feed(&stream[at..end]);
                at = end;
            }
            assert!(s.term().grid().history_size() >= before, "round {round}: {stream:?} cleared history");
        }
    }
    /// 13o Task 2 fix 4: REP after a character with combining marks repeats the whole cell, and a
    /// cell keeps a bounded number of marks.
    #[test]
    fn rep_repeats_the_whole_cell_and_marks_are_bounded() {
        let cell = |s: &Screen, c: usize| {
            let cell = &s.term.grid()[Line(0)][Column(c)];
            let mut t = cell.c.to_string();
            t.extend(cell.zerowidth().unwrap_or(&[]));
            t
        };
        let mut s = Screen::new(20, 3);
        s.feed("e\u{301}\x1b[2b".as_bytes());
        assert_eq!((cell(&s, 0), cell(&s, 1), cell(&s, 2), cell(&s, 3)), ("e\u{301}".to_string(), "e\u{301}".to_string(), "e\u{301}".to_string(), " ".to_string()));
        // A plain character repeats as before, and so does a wide one.
        let mut s = Screen::new(20, 3);
        s.feed("a\x1b[2b".as_bytes());
        assert_eq!((cell(&s, 0), cell(&s, 1), cell(&s, 2)), ("a".into(), "a".into(), "a".into()));
        let mut s = Screen::new(20, 3);
        s.feed("\u{4e2d}\u{301}\x1b[1b".as_bytes());
        assert_eq!((cell(&s, 0), cell(&s, 2)), ("\u{4e2d}\u{301}".to_string(), "\u{4e2d}\u{301}".to_string()));
        // The switch still drops it.
        let mut s = Screen::new(20, 3);
        s.set_rules(crate::emu::Rules { rep: false, ..Default::default() });
        s.feed("e\u{301}\x1b[2b".as_bytes());
        assert_eq!(cell(&s, 1), " ");
        // A flood adds no marks to the first cell, and no cell holds more than the cap.
        let mut s = Screen::new(20, 3);
        s.feed("e\u{301}\x1b[65535b\x1b[65535b".as_bytes());
        assert_eq!(s.term.grid()[Line(0)][Column(0)].zerowidth().map(<[char]>::len).unwrap_or(0) <= 1, true);
        let mut s = Screen::new(20, 3);
        s.feed(format!("e{}", "\u{301}".repeat(100)).as_bytes());
        let marks = s.term.grid()[Line(0)][Column(0)].zerowidth().map(<[char]>::len).unwrap_or(0);
        assert_eq!(marks, alacritty_terminal::term::cell::MAX_ZEROWIDTH);
        s.feed("\x1b[65535b".as_bytes());
        let most = (0..3).flat_map(|l| (0..20).map(move |c| (l, c))).map(|(l, c)| s.term.grid()[Line(l)][Column(c)].zerowidth().map(<[char]>::len).unwrap_or(0)).max();
        assert_eq!(most, Some(alacritty_terminal::term::cell::MAX_ZEROWIDTH));
    }
    /// 13o Task 2 fix 3: the title stack stays small however long the titles and however many
    /// pushes (a 1 MiB title pushed 4096 times held 4 GiB).
    #[test]
    fn the_title_stack_is_bounded() {
        let mut s = Screen::new(20, 3);
        let mut title = b"\x1b]0;".to_vec();
        title.extend(std::iter::repeat_n(b'T', 600_000));
        title.push(7);
        s.feed(&title);
        for _ in 0..4096 {
            s.feed(b"\x1b[22t");
        }
        let stack = s.term.title_stack();
        assert!(stack.len() <= 10, "depth {}", stack.len());
        let bytes: usize = stack.iter().flatten().map(String::len).sum();
        assert!(bytes <= 10 * 4096, "{bytes} bytes held");
        // The newest title comes back (cut), and a short title is unchanged.
        s.feed(b"\x1b]0;short\x07\x1b[22t\x1b]0;other\x07\x1b[23t");
        assert_eq!(s.title(), Some("short"));
    }

    /// The text of grid row `l` of the grid shown (history above line 0 is negative).
    fn shown_row(s: &Screen, l: i32) -> String {
        row_text(s, l)
    }

    fn history_texts(g: &alacritty_terminal::grid::Grid<alacritty_terminal::term::cell::Cell>) -> Vec<String> {
        use alacritty_terminal::grid::Dimensions;
        (1..=g.history_size() as i32)
            .rev()
            .map(|n| (0..g.columns()).map(|c| g[Line(-n)][Column(c)].c).collect::<String>().trim_end().to_string())
            .collect()
    }

    /// 13o Task 4 (Review Focus 4, R3): lines that scroll off the alternate screen are kept, move
    /// into the main history when the program leaves, and are dropped with the switch off.
    #[test]
    fn alternate_screen_keeps_history() {
        let look = |on| Look { alt_scrollback: on, scrollback: 100, ..Look::default() };
        let mut s = Screen::with_look(10, 3, Palette::default(), look(true));
        s.feed(b"\x1b[?1049h");
        s.feed(b"1\r\n2\r\n3\r\n4\r\n5");
        assert_eq!(s.term().grid().history_size(), 2);
        assert_eq!(history_texts(s.term().grid()), ["1", "2"]);
        // The main screen's own history is separate while the alternate screen is up.
        assert_eq!(s.term().primary_grid().history_size(), 0);
        s.feed(b"\x1b[?1049l");
        assert_eq!(history_texts(s.term().grid()), ["1", "2"]);
        assert_eq!(s.term().grid().history_size(), 2);
        // Entering again starts the alternate history afresh.
        s.feed(b"\x1b[?1049h");
        assert_eq!(s.term().grid().history_size(), 0);
        s.feed(b"\x1b[?1049l");
        assert_eq!(s.term().grid().history_size(), 2);
        // Switch off: nothing kept, nothing added.
        let mut s = Screen::with_look(10, 3, Palette::default(), look(false));
        s.feed(b"\x1b[?1049h1\r\n2\r\n3\r\n4\r\n5");
        assert_eq!(s.term().grid().history_size(), 0);
        s.feed(b"\x1b[?1049l");
        assert_eq!(s.term().grid().history_size(), 0);
        assert!(Look::default().alt_scrollback);
        // A new profile value reaches a running screen, both ways.
        let mut s = Screen::with_look(10, 3, Palette::default(), look(false));
        s.set_look(look(true));
        s.feed(b"\x1b[?1049h1\r\n2\r\n3\r\n4");
        assert_eq!(s.term().grid().history_size(), 1);
        s.set_look(look(false));
        assert_eq!(s.term().grid().history_size(), 0);
    }

    /// 13o Task 4: leaving the alternate screen puts its history above the main history without
    /// renumbering the screen's rows (`scrolled_off`, which marks and triggers number rows by),
    /// and a full history drops its oldest rows into `dropped`.
    #[test]
    fn alternate_history_joins_the_main_history() {
        let mut s = Screen::with_scrollback(10, 3, 4);
        s.feed(b"m1\r\nm2\r\nm3\r\nm4");
        let (scrolled, dropped) = (s.term().grid().scrolled_off(), s.term().grid().dropped());
        assert_eq!(history_texts(s.term().grid()), ["m1"]);
        s.feed(b"\x1b[?1049h\x1b[Ha\r\nb\r\nc\r\nd\r\ne");
        s.feed(b"\x1b[?1049l");
        assert_eq!(history_texts(s.term().grid()), ["m1", "a", "b"]);
        assert_eq!(s.term().grid().scrolled_off(), scrolled);
        assert_eq!(s.term().grid().dropped(), dropped);
        // Past the limit (4) the oldest rows are lost and counted.
        s.feed(b"\x1b[?1049h\x1b[H1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\x1b[?1049l");
        assert_eq!(history_texts(s.term().grid()), ["1", "2", "3", "4"]);
        assert_eq!(s.term().grid().dropped(), dropped + 3);
        // RIS in the alternate screen leaves no history behind.
        s.feed(b"\x1b[?1049h1\r\n2\r\n3\r\n4\x1bc");
        assert_eq!(s.term().grid().history_size(), 0);
    }

    /// 13o Task 4 fix: ED 2 in the alternate screen saves the screen first, as iTerm2
    /// (`MS:1712-1720`), whatever Save Scroll Buffer When Clearing says.
    #[test]
    fn full_clear_saves_the_alternate_screen() {
        let look = |on| Look { alt_scrollback: on, scrollback: 100, ..Look::default() };
        let mut s = Screen::with_look(10, 4, Palette::default(), look(true));
        s.feed(b"\x1b[?1049ha\r\nb\x1b[2J");
        assert_eq!(history_texts(s.term().grid()), ["a", "b"]);
        assert_eq!(shown_row(&s, 0), "");
        let mut s = Screen::with_look(10, 4, Palette::default(), look(false));
        s.feed(b"\x1b[?1049ha\r\nb\x1b[2J");
        assert_eq!(s.term().grid().history_size(), 0);
        // The main screen's ED 2 is unchanged.
        let mut s = Screen::with_look(10, 4, Palette::default(), look(true));
        s.feed(b"a\r\nb\x1b[2J");
        assert_eq!(history_texts(s.term().grid()), ["a", "b"]);
    }

    /// 13o Task 4 (MS:1729-1742): ED 0 at home in the alternate screen saves the screen first.
    #[test]
    fn clear_at_home_saves_the_screen() {
        use crate::emu::Rules;
        let look = |on| Look { alt_scrollback: on, scrollback: 100, ..Look::default() };
        let run = |on: bool, rules: Rules, alt: bool| {
            let mut s = Screen::with_look(10, 4, Palette::default(), look(on));
            s.set_rules(rules);
            if alt {
                s.feed(b"\x1b[?1049h");
            }
            s.feed(b"a\r\nb\x1b[H\x1b[J");
            s
        };
        let s = run(true, Rules::default(), true);
        assert_eq!(history_texts(s.term().grid()), ["a", "b"]);
        assert_eq!(shown_row(&s, 0), "");
        assert_eq!(shown_row(&s, 1), "");
        let s = run(true, Rules { save_on_clear: false, ..Rules::default() }, true);
        assert_eq!(s.term().grid().history_size(), 0);
        // The main screen never saves (zsh, issue 8822).
        let s = run(true, Rules::default(), false);
        assert_eq!(s.term().grid().history_size(), 0);
        // Alternate scrollback off: iTerm2 hands a nil line buffer, nothing is saved.
        let s = run(false, Rules::default(), true);
        assert_eq!(s.term().grid().history_size(), 0);
        // Only from the home cell, and a plain ED 0 elsewhere clears below as before.
        let mut s = Screen::with_look(10, 4, Palette::default(), look(true));
        s.feed(b"\x1b[?1049ha\r\nb\x1b[2;1H\x1b[J");
        assert_eq!(s.term().grid().history_size(), 0);
        assert_eq!(shown_row(&s, 0), "a");
        // A blank screen saves nothing.
        let mut s = Screen::with_look(10, 4, Palette::default(), look(true));
        s.feed(b"\x1b[?1049h\x1b[H\x1b[J");
        assert_eq!(s.term().grid().history_size(), 0);
    }

    /// 13o Task 4 (`MS+TD:480-521`): a CUP above the running command's output saves the screen,
    /// before the command has printed anything.
    #[test]
    fn cursor_above_prompt_saves_the_screen() {
        use crate::emu::Rules;
        let look = Look { scrollback: 100, ..Look::default() };
        let start = |rules: Rules| {
            let mut s = Screen::with_look(10, 6, Palette::default(), look);
            s.set_rules(rules);
            s.feed(b"old1\r\nold2\r\n\x1b]133;A\x07\x1b]133;B\x07");
            s
        };
        let mut s = start(Rules::default());
        s.feed(b"\x1b]133;C\x07");
        s.feed(b"\x1b[H");
        assert_eq!(history_texts(s.term().grid()), ["old1", "old2"]);
        assert_eq!((0..6).map(|l| shown_row(&s, l)).collect::<String>(), "");
        // Painting then moving home saves nothing more.
        s.feed(b"x\x1b[3;1Hy\x1b[H");
        assert_eq!(s.term().grid().history_size(), 2);
        // Output first, in a read of its own or the same read, saves nothing.
        let mut s = start(Rules::default());
        s.feed(b"\x1b]133;C\x07");
        s.feed(b"x");
        s.feed(b"\x1b[H");
        assert_eq!(s.term().grid().history_size(), 0);
        let mut s = start(Rules::default());
        s.feed(b"\x1b]133;C\x07x\x1b[H");
        assert_eq!(s.term().grid().history_size(), 0);
        assert_eq!(shown_row(&s, 0), "old1");
        // The same read can also start the command and move home.
        let mut s = start(Rules::default());
        s.feed(b"\x1b]133;C\x07\x1b[H");
        assert_eq!(s.term().grid().history_size(), 2);
        // CUP with parameters and HVP both count.
        let mut s = start(Rules::default());
        s.feed(b"\x1b]133;C\x07\x1b[1;1f");
        assert_eq!(s.term().grid().history_size(), 2);
        // A CUP that stays at or below the output start saves nothing.
        let mut s = start(Rules::default());
        s.feed(b"\x1b]133;C\x07\x1b[3;1H");
        assert_eq!(s.term().grid().history_size(), 0);
        // No running command: nothing.
        let mut s = Screen::with_look(10, 6, Palette::default(), look);
        s.feed(b"old1\r\nold2\r\n\x1b[H");
        assert_eq!(s.term().grid().history_size(), 0);
        let mut s = start(Rules::default());
        s.feed(b"\x1b[H");
        assert_eq!(s.term().grid().history_size(), 0);
        // The command ended: nothing.
        let mut s = start(Rules::default());
        s.feed(b"\x1b]133;C\x07\x1b]133;D;0\x07\x1b[H");
        assert_eq!(s.term().grid().history_size(), 0);
        // The switch off: nothing.
        let mut s = start(Rules { save_above_prompt: false, ..Rules::default() });
        s.feed(b"\x1b]133;C\x07\x1b[H");
        assert_eq!(s.term().grid().history_size(), 0);
        // The alternate screen: nothing.
        let mut s = start(Rules::default());
        s.feed(b"\x1b]133;C\x07\x1b[?1049h\x1b[H\x1b[?1049l");
        assert_eq!(history_texts(s.term().grid()), Vec::<String>::new());
    }

    // ---- 13o Task 5: character widths ----

    fn width_cell(s: &Screen, line: i32, col: usize) -> Cell {
        s.term().grid()[alacritty_terminal::index::Point::new(Line(line), Column(col))].clone()
    }

    /// The column of the first cell holding `ch` on `line`.
    fn column_of(s: &Screen, line: i32, ch: char) -> Option<usize> {
        (0..s.term().columns()).find(|&c| width_cell(s, line, c).c == ch)
    }

    fn wide_look() -> Look {
        Look::default()
    }

    #[test]
    fn widths_follow_iterm2() {
        use alacritty_terminal::term::cell::Flags;
        // U+200B advances the cursor by default and takes a cell of its own (R3).
        let mut s = Screen::new(10, 3);
        s.feed("a\u{200B}b".as_bytes());
        assert_eq!(column_of(&s, 0, 'a'), Some(0));
        assert_eq!(width_cell(&s, 0, 1).c, '\u{200B}');
        assert_eq!(column_of(&s, 0, 'b'), Some(2));
        // With the switch off it rides on the previous cell.
        let mut s = Screen::new(10, 3);
        s.set_rules(crate::emu::Rules { zwsp_advances: false, ..crate::emu::Rules::default() });
        s.feed("a\u{200B}b".as_bytes());
        assert_eq!(column_of(&s, 0, 'b'), Some(1));
        assert_eq!(width_cell(&s, 0, 0).zerowidth(), Some(&['\u{200B}'][..]));
        // Ambiguous characters widen only with the profile switch.
        let mut s = Screen::new(10, 3);
        s.feed("\u{00A7}x".as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(1));
        let mut s = Screen::with_look(10, 3, Palette::default(), Look { ambiguous_wide: true, ..wide_look() });
        s.feed("\u{00A7}x".as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(2));
        assert!(width_cell(&s, 0, 0).flags.contains(Flags::WIDE_CHAR));
        assert!(width_cell(&s, 0, 1).flags.contains(Flags::WIDE_CHAR_SPACER));
        // Emoji are two cells from Unicode 9, one before.
        let mut s = Screen::with_look(10, 3, Palette::default(), Look { unicode_version: 8, ..wide_look() });
        s.feed("\u{1F600}x".as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(1));
        let mut s = Screen::with_look(10, 3, Palette::default(), Look { unicode_version: 9, ..wide_look() });
        s.feed("\u{1F600}x".as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(2));
        // A CJK character is two cells in both.
        let mut s = Screen::with_look(10, 3, Palette::default(), Look { unicode_version: 8, ..wide_look() });
        s.feed("\u{4E00}x".as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(2));
        // OSC 1337 UnicodeVersion changes the table, and a pop restores the profile's.
        let mut s = Screen::new(10, 3);
        s.feed(b"\x1b]1337;UnicodeVersion=8\x07");
        s.feed("\u{1F600}x".as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(1));
        s.feed(b"\r\n\x1b]1337;UnicodeVersion=0\x07");
        s.feed("\u{1F600}y".as_bytes());
        assert_eq!(column_of(&s, 1, 'y'), Some(2));
        // A later profile edit re-sends the rule.
        let mut s = Screen::new(10, 3);
        s.set_look(Look { ambiguous_wide: true, ..wide_look() });
        s.feed("\u{00A7}x".as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(2));
        let mut s = Screen::new(10, 3);
        s.set_profile_unicode(8);
        s.feed("\u{1F600}x".as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(1));
    }

    /// 13o final review I6: an unterminated OSC 52 of 3 MiB does not wedge the screen, and what
    /// follows its terminator prints normally.
    #[test]
    fn a_runaway_osc52_ends_cleanly() {
        let mut s = Screen::new(10, 3);
        s.feed(b"\x1b]52;c;");
        let chunk = vec![b'A'; 64 * 1024];
        for _ in 0..48 {
            s.feed(&chunk);
        }
        s.feed(b"\x07ok");
        assert_eq!(column_of(&s, 0, 'o'), Some(0));
        assert_eq!(column_of(&s, 0, 'k'), Some(1));
        assert_eq!(column_of(&s, 0, 'A'), None, "the string never printed");
    }

    /// 13o final review I5: a mark or a selector never has a width of its own, so it stays on its
    /// base with Ambiguous Double Width on or off (the tables are only asked about bases), and
    /// iTerm2's ignorable characters take no cell (`SC:520-522`).
    #[test]
    fn ambiguous_wide_keeps_marks_on_their_base() {
        let x_at = |adw: bool, text: &str| {
            let mut s = Screen::with_look(12, 3, Palette::default(), Look { ambiguous_wide: adw, ..wide_look() });
            s.feed(format!("{text}x").as_bytes());
            column_of(&s, 0, 'x')
        };
        for adw in [true, false] {
            assert_eq!(x_at(adw, "e\u{301}"), Some(1), "e + accent, ADW {adw}");
            assert_eq!(x_at(adw, "\u{2764}\u{FE0F}"), Some(2), "heart + VS16, ADW {adw}");
            assert_eq!(x_at(adw, "\u{304B}\u{3099}"), Some(2), "ka + voiced mark, ADW {adw}");
            assert_eq!(x_at(adw, "a\u{FE00}"), Some(1), "a + VS1, ADW {adw}");
            // Ignorable characters get no cell (the Hangul fillers are in the full-width tables).
            for filler in ["\u{115F}", "\u{3164}", "\u{FFA0}", "\u{AD}"] {
                assert_eq!(x_at(adw, &format!("a{filler}")), Some(1), "{filler:?}, ADW {adw}");
            }
        }
        assert_eq!(x_at(true, "\u{A7}"), Some(2), "a base still widens");
        assert_eq!(x_at(false, "\u{A7}"), Some(1));
    }

    /// Task 5 fix round 1: a skin tone modifier joins its base, and a ZWJ sequence is one cell
    /// run as wide as its first emoji (iTerm2: grapheme clusters, `StringToScreenChars`).
    #[test]
    fn skin_tones_keep_their_own_cells_and_zwj_joins() {
        let x_at = |text: &str, prefix: &str| {
            let mut s = Screen::new(12, 3);
            s.feed(format!("{prefix}{text}x").as_bytes());
            column_of(&s, 0, 'x')
        };
        // iTerm2 (AggressiveBaseCharacterDetection, default on): a skin tone takes its own cells
        // (CharsetTests.swift:453-515): victory hand 1 + 2, thumbs up 2 + 2, a tone alone 2.
        assert_eq!(x_at("\u{270C}\u{1F3FD}", ""), Some(3));
        assert_eq!(x_at("\u{1F44D}\u{1F3FD}", ""), Some(4));
        assert_eq!(x_at("\u{1F44D}\u{1F3FB}", ""), Some(4));
        assert_eq!(x_at("\u{270C}\u{1F3FD}", "\x1b[?1049h"), Some(3));
        assert_eq!(x_at("a\u{1F3FD}", ""), Some(3));
        assert_eq!(x_at("\u{1F3FD}", ""), Some(2));
        // ZWJ sequences: a family is two cells; a tone after a ZWJ joins the pictograph before it.
        assert_eq!(x_at("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", ""), Some(2));
        assert_eq!(x_at("\u{1F468}\u{1F3FD}\u{200D}\u{1F469}\u{1F3FD}", ""), Some(6), "man | tone ZWJ woman | tone");
        assert_eq!(x_at("a\u{200D}\u{1F469}", ""), Some(3));
        assert_eq!(x_at("\u{2764}\u{FE0F}\u{200D}\u{1F525}", ""), Some(2), "heart on fire");
        // Extended_Pictographic is the exact table: a check mark and a cross are not pictographs.
        assert_eq!(x_at("\u{2713}\u{200D}\u{2717}", ""), Some(2));
    }

    #[test]
    fn vs16_widens_in_place() {
        use alacritty_terminal::term::cell::Flags;
        let heart = "\u{2764}\u{FE0F}";
        // The primary screen widens by default (R3).
        let mut s = Screen::new(10, 3);
        s.feed(format!("{heart}x").as_bytes());
        assert!(width_cell(&s, 0, 0).flags.contains(Flags::WIDE_CHAR));
        assert!(width_cell(&s, 0, 1).flags.contains(Flags::WIDE_CHAR_SPACER));
        assert_eq!(column_of(&s, 0, 'x'), Some(2));
        assert_eq!(width_cell(&s, 0, 0).zerowidth(), Some(&['\u{FE0F}'][..]), "the mark stays");
        // The alternate screen leaves it narrow, unless vs16_all.
        let mut s = Screen::new(10, 3);
        s.feed(format!("\x1b[?1049h{heart}x").as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(1));
        let mut s = Screen::new(10, 3);
        s.set_rules(crate::emu::Rules { vs16_all: true, ..crate::emu::Rules::default() });
        s.feed(format!("\x1b[?1049h{heart}x").as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(2));
        // The primary switch off: narrow there too.
        let mut s = Screen::new(10, 3);
        s.set_rules(crate::emu::Rules { vs16_primary: false, ..crate::emu::Rules::default() });
        s.feed(format!("{heart}x").as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(1));
        // A base that does not take VS16 stays narrow.
        let mut s = Screen::new(10, 3);
        s.feed("A\u{FE0F}x".as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(1));
        // In the last column the widening wraps the heart as a wide character written there would.
        let mut s = Screen::new(10, 3);
        s.feed(format!("123456789{heart}x").as_bytes());
        assert!(width_cell(&s, 0, 9).flags.contains(Flags::LEADING_WIDE_CHAR_SPACER));
        assert_eq!(width_cell(&s, 1, 0).c, '\u{2764}');
        assert!(width_cell(&s, 1, 0).flags.contains(Flags::WIDE_CHAR));
        assert!(width_cell(&s, 1, 1).flags.contains(Flags::WIDE_CHAR_SPACER));
        assert_eq!(column_of(&s, 1, 'x'), Some(2));
        assert_eq!(width_cell(&s, 1, 0).zerowidth(), Some(&['\u{FE0F}'][..]), "the mark moved with it");
        // With line wrap off it stays narrow.
        let mut s = Screen::new(10, 3);
        s.feed(format!("\x1b[?7l123456789{heart}").as_bytes());
        assert!(!width_cell(&s, 0, 9).flags.contains(Flags::WIDE_CHAR));
        assert_eq!(width_cell(&s, 0, 9).c, '\u{2764}');
        // A base that is already wide is left as it is.
        let mut s = Screen::new(10, 3);
        s.feed(format!("{heart}\u{FE0F}x").as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(2));
        // Insert mode shifts the text to the right of the widened cell.
        let mut s = Screen::new(10, 3);
        s.feed(format!("Q\x1b[1G\x1b[4h{heart}").as_bytes());
        assert!(width_cell(&s, 0, 0).flags.contains(Flags::WIDE_CHAR));
        assert!(width_cell(&s, 0, 1).flags.contains(Flags::WIDE_CHAR_SPACER));
        assert_eq!(width_cell(&s, 0, 2).c, 'Q');
        // The first half of a wide character to its right loses its spacer.
        let mut s = Screen::new(10, 3);
        s.feed("a\u{4E00}\x1b[1G".as_bytes());
        s.feed(heart.as_bytes());
        assert!(width_cell(&s, 0, 0).flags.contains(Flags::WIDE_CHAR));
        assert!(width_cell(&s, 0, 1).flags.contains(Flags::WIDE_CHAR_SPACER));
        assert!(!width_cell(&s, 0, 1).flags.contains(Flags::WIDE_CHAR));
        assert!(!width_cell(&s, 0, 2).flags.contains(Flags::WIDE_CHAR_SPACER), "no stray spacer");
        // At the left edge with nothing before it, nothing widens.
        let mut s = Screen::new(10, 3);
        s.feed("\u{FE0F}x".as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(0));
    }

    #[test]
    fn flags_stay_two_cells() {
        use alacritty_terminal::term::cell::Flags;
        let us = "\u{1F1FA}\u{1F1F8}";
        let off = crate::emu::Rules { full_width_flags: false, ..crate::emu::Rules::default() };
        // A pair is two cells with the switch on and off, and one cluster.
        for rules in [crate::emu::Rules::default(), off] {
            let mut s = Screen::new(10, 3);
            s.set_rules(rules);
            s.feed(format!("{us}x").as_bytes());
            assert_eq!(column_of(&s, 0, 'x'), Some(2), "{rules:?}");
            assert!(width_cell(&s, 0, 0).flags.contains(Flags::WIDE_CHAR));
            assert!(width_cell(&s, 0, 1).flags.contains(Flags::WIDE_CHAR_SPACER));
            assert_eq!(width_cell(&s, 0, 0).zerowidth(), Some(&['\u{1F1F8}'][..]));
        }
        // A lone indicator is two cells with the switch on (R3), one with it off.
        let mut s = Screen::new(10, 3);
        s.feed("\u{1F1FA}x".as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(2));
        let mut s = Screen::new(10, 3);
        s.set_rules(off);
        s.feed("\u{1F1FA}x".as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(1));
        // With the switch off a pair starting in the last column wraps as a wide character.
        let mut s = Screen::new(10, 3);
        s.set_rules(off);
        s.feed(format!("123456789{us}x").as_bytes());
        assert!(width_cell(&s, 0, 9).flags.contains(Flags::LEADING_WIDE_CHAR_SPACER));
        assert_eq!(width_cell(&s, 1, 0).c, '\u{1F1FA}');
        assert!(width_cell(&s, 1, 0).flags.contains(Flags::WIDE_CHAR));
        assert_eq!(column_of(&s, 1, 'x'), Some(2));
        // With it on the first indicator wraps itself and the second joins.
        let mut s = Screen::new(10, 3);
        s.feed(format!("123456789{us}x").as_bytes());
        assert_eq!(width_cell(&s, 1, 0).c, '\u{1F1FA}');
        assert_eq!(column_of(&s, 1, 'x'), Some(2));
        // A third indicator starts a new cell.
        let mut s = Screen::new(10, 3);
        s.feed("\u{1F1FA}\u{1F1F8}\u{1F1E6}x".as_bytes());
        assert_eq!(width_cell(&s, 0, 2).c, '\u{1F1E6}');
        assert_eq!(column_of(&s, 0, 'x'), Some(4));
    }

    #[test]
    fn unicode_version_defaults_to_9() {
        assert_eq!(Look::default().unicode_version, 9);
        assert_eq!(Look::from_profile(&crate::profile::Profile::stock()).unicode_version, 9);
        let mut s = Screen::new(10, 3);
        s.feed("\u{1F600}x".as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(2));
        let mut p = crate::profile::Profile::stock();
        p.values.insert("Unicode Version".into(), toml::Value::Integer(8));
        let look = Look::from_profile(&p);
        assert_eq!(look.unicode_version, 8);
        let mut s = Screen::with_look(10, 3, Palette::default(), look);
        s.feed("\u{1F600}x".as_bytes());
        assert_eq!(column_of(&s, 0, 'x'), Some(1));
        let row = crate::schema::find("Unicode Version").unwrap();
        assert_eq!(row.default, Some(toml::Value::Integer(9)));
    }

    #[test]
    fn capabilities_report_ambiguous_width() {
        let mut s = Screen::with_look(10, 3, Palette::default(), Look { ambiguous_wide: true, ..wide_look() });
        s.clipboard_allowed = true;
        let reply = String::from_utf8(s.feed(b"\x1b]1337;Capabilities\x07")).unwrap();
        assert!(reply.contains("Aw") && reply.contains("Uw9"), "{reply}");
        let mut s = Screen::new(10, 3);
        let reply = String::from_utf8(s.feed(b"\x1b]1337;Capabilities\x07")).unwrap();
        assert!(!reply.contains("Aw") && reply.contains("Uw9"), "{reply}");
    }

    /// 13o Task 9: OSC 72 registers a program for drops, and only a trusted screen answers.
    #[test]
    fn kitty_dnd_registers_on_the_screen() {
        let mut s = Screen::new(10, 3);
        assert!(!s.dnd.accepting());
        let reply = s.feed(b"\x1b]72;t=q:i=5\x1b\\\x1b]72;t=a;text/uri-list\x1b\\");
        assert_eq!(reply, b"\x1b]72;t=q:i=5\x1b\\");
        assert!(s.dnd.accepting());
        assert!(s.dnd_accepting());
        s.feed(b"\x1b]72;t=A\x07");
        assert!(!s.dnd.accepting(), "BEL ends it too");
        // A split read registers once.
        s.feed(b"\x1b]72;t=a;text/ur");
        assert!(!s.dnd.accepting());
        s.feed(b"i-list\x1b\\");
        assert!(s.dnd.accepting());
        // Untrusted: nothing answers and nothing registers (B7); a held registration is dropped.
        let mut s = Screen::new(10, 3);
        s.trusted = false;
        assert_eq!(s.feed(b"\x1b]72;t=q:i=5\x1b\\\x1b]72;t=a;text/uri-list\x1b\\"), b"");
        assert!(!s.dnd.accepting() && !s.dnd_accepting());
        s.trusted = true;
        s.feed(b"\x1b]72;t=a;text/uri-list\x1b\\");
        s.trusted = false;
        assert!(s.dnd.accepting() && !s.dnd_accepting(), "trust lost: no drop is reported");
        // Replies are bounded in a batch: a flood of queries answers 64.
        let mut s = Screen::new(10, 3);
        let flood = "\x1b]72;t=q\x1b\\".repeat(1000);
        assert_eq!(s.feed(flood.as_bytes()).len(), 64 * "\x1b]72;t=q\x1b\\".len());
        // Only replies are counted: a message of 100 chunks in one read still completes, and its
        // first and last parts are never joined with the middle skipped.
        let mut s = Screen::new(10, 3);
        let mut chunks = "\x1b]72;t=a:m=1;a\x1b\\".repeat(100);
        chunks.push_str("\x1b]72;t=a:m=0;text/uri-list\x1b\\");
        s.feed(chunks.as_bytes());
        assert!(s.dnd.accepting(), "the m=0 chunk was not skipped");
        // And the next batch starts clean: no leaked half message.
        s.feed(b"\x1b]72;t=A\x1b\\");
        assert!(!s.dnd.accepting());
    }

    /// 13p S8 x 13q P4: the dead program's OSC 9;4 bar goes when it ends (and so before a
    /// restart's relaunch), and the window is asked to redraw.
    #[test]
    fn program_ended_clears_the_progress_bar() {
        let mut s = Screen::new(20, 4);
        s.feed(b"\x1b]9;4;1;40\x07");
        assert!(s.progress.visible());
        let _ = s.take_requests();
        s.program_ended();
        assert!(!s.progress.visible());
        assert!(s.take_requests().iter().any(|r| matches!(r, Request::Progress(false))));
    }

    /// A program that ended mid synchronized update, or with color reports on, leaves neither
    /// behind, through either reset (one string, item 20).
    #[test]
    fn the_end_reset_clears_sync_and_color_reports() {
        for ended in [true, false] {
            let mut s = Screen::new(20, 4);
            s.feed(b"\x1b[?2031h\x1b[?2026hheld");
            assert!(s.sync_deadline().is_some() && s.color_reports, "set up");
            if ended {
                s.program_ended();
            } else {
                s.append_end_message("Session Ended", "-");
            }
            assert_eq!(s.sync_deadline(), None, "ended={ended}: the synchronized update is over");
            assert!(!s.color_reports, "ended={ended}: color reports are off");
        }
    }

    /// A soft reset leaves a string the program left open, as the scanner restart does.
    #[test]
    fn a_soft_reset_restarts_the_scanner() {
        let mut s = Screen::new(20, 4);
        s.feed(b"\x1b]0;half a title");
        s.soft_reset();
        s.feed(b"ok");
        assert!(s.all_text().contains("ok"), "{:?}", s.all_text());
    }
}
