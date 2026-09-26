//! Runs app output through the terminal core and collects the replies the app expects.

use std::sync::mpsc::{self, Receiver, Sender};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Row};
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{ClearMode, CursorShape, CursorStyle, Handler, Processor};

use crate::mouse::{Modes, Tracking};
use crate::palette::Palette;

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
    pub scrollback: usize,
    /// The profile's Allow modifyOtherKeys: programs may change how keys are reported.
    pub app_key_modes: bool,
    /// The profile's Use libtickit protocol: keys start in CSI u mode.
    pub csi_u: bool,
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
            scrollback,
            app_key_modes: p.bool("Allow modifyOtherKeys"),
            csi_u: p.bool("Use libtickit protocol"),
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
}

impl Default for Limits {
    fn default() -> Self {
        Limits { images: 768 << 20, transfer: 512 << 20, copy: 100 << 20 }
    }
}

pub struct Screen {
    pub(crate) term: Term<Listener>,
    parser: Processor,
    events: Receiver<Event>,
    pub palette: Palette,
    title: Option<String>,
    cell_px: (u16, u16),
    osc: crate::osc::Scanner,
    pub marks: crate::marks::Marks,
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
    /// Key reporting chosen in the menu (on top of the program's requests).
    pub keys: KeyState,
    /// False when the global disablePotentiallyInsecureEscapeSequences is on.
    pub trusted: bool,
    /// What output asked the window to do, taken by the app.
    pub requests: Vec<Request>,
    /// OSC 1337 `Copy=` in progress, and `CopyToClipboard` capture.
    pub(crate) copy: crate::clipboard_osc::Copy,
    pub(crate) capture: Option<Vec<u8>>,
    /// The start of an escape sequence split across reads during a capture: dropped if it
    /// turns out to be one Tandem acts on, else added to the capture.
    capture_hold: Vec<u8>,
    /// OSC 1337 Block: the blocks this screen knows.
    pub blocks: crate::blocks::Blocks,
    /// OSC 1337 Button: the buttons written into the text.
    pub buttons: crate::blocks::Buttons,
    /// The profile's triggers and where checking has reached.
    pub triggers: crate::trigger_eval::TriggerState,
    /// Bells rung (iTerm2's bellCount).
    pub bell_count: u64,
    /// OSC 9;4 shows a progress bar now.
    pub progress_shown: bool,
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
    /// The program's `Env=report=all` environment, kept for the SSH integration.
    pub env: Option<String>,
    /// ExecFailed: the command could not start.
    pub exec_failed: bool,
    /// UnicodeVersion and the Touch Bar key labels.
    pub unicode: crate::osc1337::Unicode,
    pub key_labels: crate::osc1337::KeyLabels,
    /// SetColors changes, reapplied over a new profile palette.
    pub(crate) color_sets: Vec<(String, crate::osc1337::Color)>,
    /// Whether programs may write the clipboard (for the Capabilities report).
    pub clipboard_allowed: bool,
    /// Mode 2031 (color scheme reports) and the scheme last reported.
    color_reports: bool,
    last_dark: bool,
}

/// Something output asked for that the window must do.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
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
    /// SetBadgeFormat: the session's badge text.
    SetBadge(String),
    /// SetBadgeFormat with a function call: refused.
    BadgeRefused,
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

    /// Clears the status (a new prompt); nothing moves when there was none.
    pub(crate) fn clear_tab_status(&mut self) {
        if !self.tab_status.is_empty() {
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
        let term = Term::new(config(look), &GridSize { cols, rows }, Listener(tx));
        Self {
            term,
            parser: Processor::new(),
            events,
            palette,
            title: None,
            cell_px: (0, 0),
            osc: crate::osc::Scanner::default(),
            marks: crate::marks::Marks::default(),
            shell: crate::marks::Shell::default(),
            place_prompt_first_column: true,
            pty_backed: false,
            clock: None,
            images: crate::images::Store::default(),
            downloads: Vec::new(),
            multipart: None,
            kitty: crate::kitty::Kitty::default(),
            replies: Vec::new(),
            limits: Limits::default(),
            notes: crate::annotations::Notes::default(),
            folds: std::collections::HashMap::new(),
            next_fold: 0,
            feeds: 0,
            keys: KeyState::new(look.csi_u),
            trusted: true,
            tab_status: crate::tabstatus::Status::default(),
            tab_status_gen: 0,
            blocks: crate::blocks::Blocks::default(),
            buttons: crate::blocks::Buttons::default(),
            portholes: std::collections::HashMap::new(),
            triggers: crate::trigger_eval::TriggerState::default(),
            app_vars: std::collections::HashMap::new(),
            bell_count: 0,
            progress_shown: false,
            last_output: None,
            next_porthole: 0,
            porthole_wide_default: false,
            porthole_selection: None,
            env: None,
            exec_failed: false,
            unicode: crate::osc1337::Unicode::default(),
            key_labels: crate::osc1337::KeyLabels::default(),
            color_sets: Vec::new(),
            clipboard_allowed: false,
            requests: Vec::new(),
            copy: crate::clipboard_osc::Copy::default(),
            capture: None,
            capture_hold: Vec::new(),
            color_reports: false,
            last_dark: true,
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

    pub fn set_look(&mut self, look: Look) {
        // A new profile default for CSI u applies unless the menu chose otherwise.
        if self.keys.csi_u == self.keys.profile_csi_u {
            self.keys.csi_u = look.csi_u;
        }
        self.keys.profile_csi_u = look.csi_u;
        self.term.set_options(config(look));
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.term.resize(GridSize { cols, rows });
        // Reflow moved rows: triggers go on from the cursor.
        self.triggers_from_cursor();
    }

    pub fn term(&self) -> &Term<Listener> {
        &self.term
    }

    pub fn term_mut(&mut self) -> &mut Term<Listener> {
        &mut self.term
    }

    pub fn set_cell_px(&mut self, w: u16, h: u16) {
        self.cell_px = (w, h);
    }

    /// Set Title (a trigger): the session's title.
    pub fn set_title(&mut self, title: Option<String>) {
        self.title = title;
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Feeds app output. Returns the bytes the terminal must send back, such as
    /// answers to device, color, and size queries. An app that gets no answer may hang.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
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
        if self.keys.literal {
            let reply = self.feed_raw(&literal(bytes));
            self.run_triggers_after_feed();
            return reply;
        }
        // Shell-integration OSCs act where they appear in the stream, so the output is fed in
        // pieces that end at each one.
        let events = self.osc.scan_spans(bytes);
        let pending = self.osc.pending;
        let mut reply = Vec::new();
        let mut at = 0;
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
                    self.apply(osc);
                }
            }
            reply.append(&mut self.replies);
        }
        if !still_held {
            match pending.filter(|p| *p >= at) {
                Some(p) if self.capture.is_some() => {
                    self.capture_bytes(&bytes[at..p]);
                    self.capture_hold = bytes[p..].to_vec();
                }
                _ => self.capture_bytes(&bytes[at..]),
            }
        }
        reply.extend(self.feed_raw(&bytes[at..]));
        reply.extend(self.color_scheme_change());
        self.run_triggers_after_feed();
        reply
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
        self.term.selection = None;
        let removed = self.term.primary_grid_mut().splice(Line(first), Line(last), with);
        // Rows moved: triggers go on from the cursor.
        self.triggers_from_cursor();
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

    fn alt_screen(&self) -> bool {
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
        self.term.stamp = self.clock.unwrap_or_else(|| {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32)
        });
        self.parser.advance(&mut self.term, bytes);
        // Now and then, forget folds and notes whose rows scrolled out of the buffer.
        self.feeds = self.feeds.wrapping_add(1);
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
                Event::Title(title) => self.title = Some(title),
                Event::ResetTitle => self.title = None,
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
        let grid = self.term.primary_grid_mut();
        if line >= -(grid.history_size() as i32) && line < grid.screen_lines() as i32 {
            grid[Line(line)].set_mark(id);
        }
    }

    /// Puts a manual mark id on a row, beside any prompt mark there.
    pub fn set_row_manual(&mut self, line: i32, id: u32) {
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

    /// One logical line's text starting at `line`: its row and the rows it wraps onto,
    /// trailing spaces trimmed.
    pub fn line_text(&self, line: i32) -> String {
        let grid = self.term.grid();
        let mut out = String::new();
        let mut l = line;
        while l < grid.screen_lines() as i32 {
            let row = &grid[Line(l)];
            out.extend((0..grid.columns()).map(|c| row[Column(c)].c));
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
            let text: String = (0..grid.columns()).map(|c| row[Column(c)].c).collect();
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
        const DECSTR: &[u8] = b"\x1b[?25h\x1b[?1l\x1b>\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1005l\x1b[?1006l\
            \x1b[?2004l\x1b[?1004l\x1b[4l\x1b[0m\x1b[r\x1b[?6l\x1b[?7h\x1b(B";
        // Straight to the parser: Literal Controls would print the sequence instead.
        let reply = self.feed_raw(DECSTR);
        self.replies.extend(reply);
        // Key reporting goes back to the profile's: no program flags, no menu choices.
        self.term.clear_keyboard_modes();
        self.keys = KeyState { literal: self.keys.literal, ..KeyState::new(self.keys.profile_csi_u) };
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
        crate::scene::pane(&mut f, &s, crate::layout::Rect { x: 0.0, y: 0.0, w: 300.0, h: 100.0 }, (10.0, 20.0), true, true);
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

    fn row_text(s: &Screen, l: i32) -> String {
        (0..s.term.columns()).map(|c| s.term.grid()[Line(l)][Column(c)].c).collect::<String>().trim_end().to_string()
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

    #[test]
    fn turning_app_key_modes_off_clears_them() {
        use crate::keyproto::KeyMode;
        let mut s = Screen::new(20, 4);
        s.feed(b"\x1b[>4;2m");
        s.set_look(Look { app_key_modes: false, ..Look::default() });
        assert_eq!(s.keyboard().mode, KeyMode::Standard);
    }
}
