//! The Composer's pure model (milestone 9l): a per-session command editor built on
//! `text_edit::TextEdit`. Holds the typed text, an optional uneditable prompt prefix, where
//! and how it is shown (minimal dropdown, status bar field, or an Auto Composer replacing the
//! shell prompt), the saved draft, the key map, sending, send-each, enqueue-each and the
//! command queue. Drawing, prompt-state feed, syntax highlighting and suggestions are other
//! modules (`prompt_state`, `command_syntax`, `composer_suggest`) and the app side
//! (`src/app/composer_ui.rs`); this module only decides what a key means and what happens to
//! the text.
//!
//! iTerm2 sources (`~/gitrepos/iterm2-ref/sources/Composer`): `CM` `iTermComposerManager.m`
//! (toggling, routing, send and enqueue), `MCVC` `iTermMinimalComposerViewController.m` (the
//! dropdown's own send/enqueue plumbing and `stringValue`/`stringExcludingPrefix`), `CTV`
//! `ComposerTextView.swift` (the key map and `take()`).

use std::collections::VecDeque;
use std::ops::Range;

use crate::layout::Rect;
use crate::styled_copy;
use crate::text_edit::{self, EditKey, TextEdit, VisualLine};

/// Where the composer is shown: nowhere, the dropdown over the pane, or the status bar's
/// field (`CM:146-232`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Shown {
    #[default]
    Hidden,
    Dropdown,
    Field,
}

/// The prompt captured ahead of the Auto Composer's editable text (`PromptStateMachine`'s B
/// anchor to the command start, milestone 9l Task 3): `rows` is how it draws (colors from the
/// grid, `styled_copy::runs_of_lines`), `text` its plain lines (their joined length is the
/// uneditable prefix `set_string` and `apply` protect), `by_trigger` whether a trigger
/// (rather than shell integration) supplied it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prompt {
    pub rows: Vec<styled_copy::Row>,
    pub text: Vec<String>,
    pub by_trigger: bool,
}

/// A per-session composer. `edit.text` holds the prefix (if any, verbatim, at its start)
/// followed by the command; `contents` is the command alone (`stringExcludingPrefix`,
/// `MCVC:445-451`).
#[derive(Clone, Debug, Default)]
pub struct Composer {
    pub edit: TextEdit,
    pub prefix: Option<Prompt>,
    /// View > Auto Composer: this session replaces its shell prompt with the composer
    /// (`isAutoComposer`).
    pub auto: bool,
    pub shown: Shown,
    /// The draft kept across a dismiss with nothing sent (`_saved`, memory only, `CM:27`).
    pub saved: Option<String>,
    /// A temporary status bar component seeded from `saved` (`CM:192-215`).
    pub temporary: bool,
    pub focused: bool,
    /// The Auto Composer is suppressed (the alternate screen, or the feature turned off with
    /// one still up): drawing and routing skip it without losing its text.
    pub hidden: bool,
    /// Shell integration already put text at the prompt before the Auto Composer took it over
    /// (`haveShellProvidedText`, `PS:24546-24596`): a clear probe runs first.
    pub shell_provided: bool,
    /// Enqueued commands waiting for a mark (`_commandQueue`, `PS:16613-16617`); drained one
    /// per mark by `drain`.
    pub queue: VecDeque<String>,
}

/// What applying an `Action` asks the caller to do: write text or a control byte, open a
/// popup, or that the key was consumed with nothing further to do (never produced when the
/// key was purely a text edit).
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// Send this text, terminated as the caller composed it (a trailing "\n" already
    /// included where iTerm2 adds one).
    Send(String),
    Enqueue(String),
    Control(Vec<u8>),
    /// The status bar field's raw send (`stringValue + "\n"`, no shell history).
    SendRaw(String),
    OpenHistory {
        prefix: String,
        search: bool,
    },
    ClearScreen,
    AdvancedPaste(String),
    Completions {
        explicit: bool,
    },
    AcceptSuggestion,
    Dismissed,
}

/// The modifiers held with a key (Win is Cmd, per milestone 8's mapping).
#[derive(Clone, Copy, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

/// A key already decoded to what it would mean at rest: a plain edit key, Esc, or an
/// unmapped Ctrl+letter chord (kept apart from `EditKey` so the auto-mode chords in `action`
/// can look at it before `text_edit::ctrl_key` would).
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Edit(EditKey),
    Esc,
    Ctrl(char),
}

/// What a key means to the composer, from `action`. `Edit` is a plain editing key: the
/// caller applies it (or, for a shift-extended move, calls `TextEdit::key` itself, since
/// `apply` has no shift bit to pass on).
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Send,
    SendEach,
    EnqueueEach,
    Tab,
    Esc,
    /// Sends this byte to the shell (`sendKeystroke`, `CTV:735-736`; ^C is 3).
    Control(u8),
    /// Sends this byte only when the composer is empty, else the key falls through
    /// (`CTV:739-748`; ^D is 4).
    ControlIfEmpty(u8),
    ClearScreen,
    HistorySearch,
    DeleteAll,
    AddCaret {
        below: bool,
    },
    Edit(EditKey),
}

/// Whether `k` can change the text (the prefix guard only refuses these).
fn mutates(k: &EditKey) -> bool {
    matches!(
        k,
        EditKey::Text(_)
            | EditKey::Enter
            | EditKey::Tab
            | EditKey::Backspace
            | EditKey::Delete
            | EditKey::DeleteWordBack
            | EditKey::DeleteWordForward
            | EditKey::Cut
            | EditKey::Paste(_)
    )
}

/// The key map (`CTV:692-784` standard and auto action lists, `CTV:836-866` the order Esc,
/// mode actions, then the plain key; `MCTV:1146-1190` Ctrl+Shift+Up/Down ahead of everything).
/// `popup` is a completion or suggestion popup showing: it takes Up and Down itself (`None`
/// here) and Esc itself first (`None` here; the caller re-asks with `popup` false once it
/// declines). A Win chord is never the composer's (`None`; it reaches menus).
pub fn action(key: &Key, m: Mods, auto: bool, popup: bool) -> Option<Action> {
    if m.win {
        return None;
    }
    if matches!(key, Key::Esc) {
        return if popup { None } else { Some(Action::Esc) };
    }
    if m.ctrl && m.shift {
        match key {
            Key::Edit(EditKey::Up) => return Some(Action::AddCaret { below: false }),
            Key::Edit(EditKey::Down) => return Some(Action::AddCaret { below: true }),
            _ => {}
        }
    }
    if popup && matches!(key, Key::Edit(EditKey::Up) | Key::Edit(EditKey::Down)) {
        return None;
    }
    if matches!(key, Key::Edit(EditKey::Enter)) {
        // `CTV:692-717` standard, `CTV:727-784` auto: only these exact modifier sets are
        // bound; anything else (including a plain standard Enter, or a Ctrl chord) falls to
        // the text view's default, a literal newline.
        return Some(match (auto, m.ctrl, m.alt, m.shift) {
            (_, false, true, true) => Action::SendEach,
            (_, false, true, false) => Action::EnqueueEach,
            (false, false, false, true) => Action::Send,
            (true, false, false, false) => Action::Send,
            _ => Action::Edit(EditKey::Enter),
        });
    }
    if matches!(key, Key::Edit(EditKey::Tab)) {
        // `CTV:706,765`: Tab is bound with no modifiers at all; Ctrl+Tab switches tabs,
        // Shift/Alt+Tab are not the composer's either.
        return if !m.ctrl && !m.alt && !m.shift { Some(Action::Tab) } else { None };
    }
    match key {
        Key::Ctrl(c) => {
            if auto && !m.shift && !m.alt {
                // `CTV:735-762`: these five win over the Windows edit chords, but only for
                // exactly Ctrl; Ctrl+A is not one of them, so it keeps its Windows meaning
                // (Select All) below.
                match c.to_ascii_lowercase() {
                    'c' => return Some(Action::Control(3)),
                    'd' => return Some(Action::ControlIfEmpty(4)),
                    'l' => return Some(Action::ClearScreen),
                    'r' => return Some(Action::HistorySearch),
                    'u' => return Some(Action::DeleteAll),
                    _ => {}
                }
            }
            text_edit::ctrl_key(*c, m.shift).map(Action::Edit)
        }
        Key::Edit(k) => {
            // AltGr types text with both Ctrl and Alt reported held (winit does not fold Ctrl
            // out of the text); it must still reach the editor as a plain key, not be dropped
            // by the Alt gate below (`keyproto::is_altgr`'s precedent - a genuine Ctrl+Alt
            // chord on anything but a printable key is still not the composer's).
            let altgr = m.ctrl && m.alt && matches!(k, EditKey::Text(_));
            if m.alt && !altgr {
                return None;
            }
            if m.ctrl && !altgr {
                text_edit::ctrl_nav(k).map(Action::Edit)
            } else {
                Some(Action::Edit(k.clone()))
            }
        }
        Key::Esc => None,
    }
}

/// `_commandQueue` sends now rather than queuing when the shell prompt has nothing typed at
/// it (`PS:24375-24383`).
pub fn enqueue_now(current: Option<&str>) -> bool {
    current == Some("")
}

/// `currentCommand` (`PS:18516-18525`): the composer's own contents when it is the Auto
/// Composer, else whatever the shell prompt has typed (already read from the grid by the
/// caller).
pub fn current_command(auto_shown: bool, contents: &str, typed: Option<String>) -> Option<String> {
    if auto_shown { Some(contents.to_string()) } else { typed }
}

/// One queued command per mark since the last drain (`screenDidAddMark`, `PS:16613-16617`),
/// oldest first; a mark with nothing queued does nothing.
pub fn drain(queue: &mut VecDeque<String>, marks_added: u32) -> Vec<String> {
    let mut out = Vec::new();
    for _ in 0..marks_added {
        match queue.pop_front() {
            Some(c) => out.push(c),
            None => break,
        }
    }
    out
}

/// `shouldRevealStatusBarComposerInViewController:` (`CM:173-178`): a status bar composer is
/// used only when there is a status bar at all, and then only when the setting always wants
/// it or a Composer component is already visible.
pub fn using_status_bar(has_bar: bool, always: bool, component_visible: bool) -> bool {
    has_bar && (always || component_visible)
}

impl Composer {
    /// The full text, including any prefix (for the modules that align against it:
    /// highlighting, wrap).
    pub fn text(&self) -> &str {
        &self.edit.text
    }

    /// The command alone, the prefix stripped (`stringExcludingPrefix`, `MCVC:445-451`).
    pub fn contents(&self) -> String {
        self.edit.text[self.prefix_len()..].to_string()
    }

    /// The prefix's length in bytes, as it actually sits at the start of `edit.text` right
    /// now: 0 unless `edit.text` truly starts with it. `set_string` always keeps the two in
    /// sync, but `prefix` is a plain pub field, so a caller that assigns it directly (Task 3,
    /// seeding a freshly captured prompt) without re-syncing the text through `set_string`
    /// first must not make every slice on `edit.text[prefix_len()..]` a potential mid-char
    /// panic: `starts_with` both proves the byte length is a real prefix of the text and, on
    /// success, guarantees that length is already a valid char boundary.
    fn prefix_len(&self) -> usize {
        let Some(p) = &self.prefix else { return 0 };
        let raw = p.text.join("\n");
        if self.edit.text.starts_with(&raw) { raw.len() } else { 0 }
    }

    /// No caret or selection reaches into the prefix (`shouldChangeText`'s guard,
    /// `CTV:1281-1320`): called after every mutation.
    fn clamp_prefix(&mut self) {
        let n = self.prefix_len();
        if n == 0 {
            return;
        }
        self.edit.caret = self.edit.caret.max(n);
        self.edit.anchor = self.edit.anchor.map(|a| a.max(n));
        for c in &mut self.edit.extra {
            c.at = c.at.max(n);
            c.anchor = c.anchor.map(|a| a.max(n));
        }
    }

    /// A click at byte `at` (`TextEdit::click`), kept out of the prefix: a click inside the
    /// prompt puts the caret at its end, so the next key is typed there rather than refused by
    /// the prefix guard (NSTextView's `shouldChangeText` guard, `CTV:1281-1320`, only ever sees
    /// carets in the editable range).
    pub fn click(&mut self, at: usize, at_line_end: bool, shift: bool, now_ms: u128, double_ms: u128) {
        self.edit.click(at, at_line_end, shift, now_ms, double_ms);
        self.clamp_prefix();
    }

    /// After any other pointer edit (a drag, an added caret, a box of carets): no caret or
    /// selection end stays inside the prefix.
    pub fn keep_out_of_prefix(&mut self) {
        self.clamp_prefix();
    }

    /// `cursorAtEnd && !canMoveUp/!canMoveDown && count <= 1` (`CTV:885-908,1165-1173`): a
    /// single caret, nothing selected, sitting at the end of the *whole* text (not just the
    /// end of its own line: "abc\ndef" with the caret after "abc" is not this, even though
    /// that is the first visual line), and the direction has no line above (`first`) or below
    /// to move to.
    fn at_edge(&self, lines: &[VisualLine], width: &dyn Fn(&str) -> f32, first: bool) -> bool {
        if !self.edit.extra.is_empty() || self.edit.selection().is_some() || self.edit.caret != self.edit.text.len() {
            return false;
        }
        let (line, _) = self.edit.caret_line(lines, width);
        if first { line == 0 } else { line + 1 >= lines.len() }
    }

    /// `setStringIncludingPrefix`/`setStringExcludingPrefix` (`CTV:410-425`): both set the
    /// whole text storage and then re-derive the prefix range from the stored `prefix`
    /// (`updatePrefix`, `CTV:475-505`), so for content purposes they are the same call: `s` is
    /// always the command, and the current `prefix` (if any) goes in front of it, whatever `s`
    /// itself contains. `including_prefix` changes nothing observable here (Cocoa's two
    /// variants differ only in how they preserve unrelated text attributes); kept for the
    /// interface `apply`/`CM` calls through. The caret lands at the end (`TextEdit::load`).
    pub fn set_string(&mut self, s: &str, including_prefix: bool) {
        let _ = including_prefix;
        let full = match &self.prefix {
            Some(p) => format!("{}{}", p.text.join("\n"), s),
            None => s.to_string(),
        };
        self.edit.load(&full);
    }

    /// `setPrefix:` (`CM:553-557`, `CTV:475-505`): the prefix changes and the command after it
    /// is kept (the one way to change `prefix` without putting the text out of step).
    pub fn set_prefix(&mut self, prefix: Option<Prompt>) {
        let command = self.contents();
        self.prefix = prefix;
        self.set_string(&command, true);
    }

    /// `toggle` (`CM:146-158`): a shown dropdown saves its text and hides regardless of
    /// `status_bar` (the caller's `using_status_bar`); otherwise it opens, in the status bar
    /// when `status_bar`, else the dropdown. Idempotent (a refocus, no reload) when already
    /// shown in the place `status_bar` asks for.
    pub fn toggle(&mut self, status_bar: bool) -> Shown {
        match self.shown {
            Shown::Dropdown => {
                self.saved = Some(self.contents());
                self.shown = Shown::Hidden;
            }
            Shown::Field if status_bar => {}
            _ => self.reveal(status_bar),
        }
        self.shown
    }

    /// A fresh open (`showMinimalComposerInView:`, `CM:234-266`): the underlying view is
    /// always destroyed on dismiss and rebuilt empty (`finishDismissingMinimalView:` nils
    /// `_minimalViewController`), so opening starts blank and reloads `saved` only when there
    /// is one, consuming it (`if (_saved.length) { setString(_saved); _saved = nil; }`,
    /// `CM:257-260`) - without that, a composer that just sent a command would reopen still
    /// showing it.
    fn reveal(&mut self, status_bar: bool) {
        let s = self.saved.take().unwrap_or_default();
        self.set_string(&s, true);
        self.shown = if status_bar { Shown::Field } else { Shown::Dropdown };
    }

    /// `showWithCommand:` (key action 64, `CM:123-139`): sets the text and reveals it,
    /// wherever it already is or, when nothing is shown, per `status_bar`/`field_visible`.
    pub fn show_with(&mut self, s: &str, status_bar: bool, field_visible: bool) {
        self.saved = Some(s.to_string());
        self.set_string(s, true);
        self.shown = match self.shown {
            Shown::Dropdown | Shown::Field => self.shown,
            Shown::Hidden if field_visible => Shown::Field,
            Shown::Hidden if status_bar => Shown::Field,
            Shown::Hidden => Shown::Dropdown,
        };
    }

    /// `showOrAppendToDropdownWithString:` (`CM:84-94`): appends a joined line to an already
    /// open dropdown (`set_string` always reattaches the prefix, if any, so it stays), else
    /// opens it seeded with `s`.
    pub fn show_or_append(&mut self, s: &str) {
        if self.shown == Shown::Dropdown {
            let joined = format!("{}\n{}", self.contents(), s);
            self.set_string(&joined, true);
        } else {
            self.saved = Some(s.to_string());
            self.shown = Shown::Dropdown;
            self.set_string(s, false);
        }
    }

    /// `placeCommandInComposer:`/`showCommandInLargeComposer:` (`CM:38-82`): puts `s` into
    /// whichever view is already open; `force_large` reveals the dropdown when nothing is
    /// open (Show Composer's own "put it in front of me" case), otherwise nothing is
    /// revealed unless `status_bar` says a field should be.
    pub fn place(&mut self, s: &str, status_bar: bool, force_large: bool) {
        self.saved = Some(s.to_string());
        match self.shown {
            Shown::Dropdown | Shown::Field => self.set_string(s, false),
            Shown::Hidden if force_large => {
                self.shown = Shown::Dropdown;
                self.set_string(s, false);
            }
            Shown::Hidden if status_bar => {
                self.shown = Shown::Field;
                self.set_string(s, false);
            }
            Shown::Hidden => {}
        }
    }

    /// `dismissAnimated:` (`CM:271-283`): always turns off the Auto Composer flag, even when
    /// nothing was showing; saves the text and hides, false when already hidden.
    pub fn dismiss(&mut self) -> bool {
        self.auto = false;
        if self.shown == Shown::Hidden {
            return false;
        }
        self.saved = Some(self.contents());
        self.shown = Shown::Hidden;
        true
    }

    /// `minimalComposer:sendCommand:addNewline:dismiss:` (`MCVC:509-515`, `CM:341-360`): the
    /// manager's one send primitive. `command` is what to send (already the piece wanted, not
    /// necessarily the whole contents); `newline` appends "\n"; `dismiss` hides unless this is
    /// the Auto Composer, in which case it clears the text and stays up instead (`reset`).
    pub fn send(&mut self, command: &str, newline: bool, dismiss: bool) -> Vec<Act> {
        let string = self.contents();
        let reset = dismiss && self.auto;
        if dismiss && !reset {
            self.shown = Shown::Hidden;
        }
        if command.is_empty() && !self.auto {
            self.saved = Some(string);
            return Vec::new();
        }
        self.saved = None;
        let out = if newline { format!("{command}\n") } else { command.to_string() };
        let acts = vec![Act::Send(out)];
        if reset {
            self.set_string("", true);
        }
        acts
    }

    /// `minimalComposer:enqueueCommand:dismiss:` (`CM:326-339`): appends "\n" itself (unlike
    /// `send`, whose newline is the caller's choice).
    pub fn enqueue(&mut self, command: &str, dismiss: bool) -> Vec<Act> {
        let string = self.contents();
        if dismiss {
            self.shown = Shown::Hidden;
        }
        if command.is_empty() {
            self.saved = Some(string);
            return Vec::new();
        }
        self.saved = None;
        vec![Act::Enqueue(format!("{command}\n"))]
    }

    /// `sendAction` with a selection (`CTV:775-789`): the whole contents when nothing is
    /// selected (finish, dismissing), else every selected piece, "\n" joined onto all but the
    /// last, none of it removed from the text and the composer left open.
    fn apply_send(&mut self) -> Vec<Act> {
        let pieces: Vec<String> = self.edit.carets().into_iter().filter(|r| !r.is_empty()).map(|r| self.edit.text[r].to_string()).collect();
        if pieces.is_empty() {
            let command = self.contents();
            self.send(&command, true, true)
        } else {
            let n = pieces.len();
            let mut acts = Vec::new();
            for (i, p) in pieces.into_iter().enumerate() {
                let command = if i + 1 < n { format!("{p}\n") } else { p };
                acts.extend(self.send(&command, false, false));
            }
            acts
        }
    }

    /// `take()` (`CTV:920-953`): each caret's selection, or else `command_at` at a plain
    /// caret, removed one at a time from the last caret back (so earlier, not yet processed
    /// carets keep pointing at the same characters) and returned in text order. Never reaches
    /// into the prefix: a selection is clipped to the command portion (nothing taken if it was
    /// entirely inside the prefix) and a plain caret sitting in the prefix takes nothing.
    ///
    /// A caret's own `command_at` search can range far past its own position (a backslash
    /// continuation chases forward and backward through several lines), so it can cover -
    /// and remove - text an earlier, not yet processed caret was pointing into. Every not yet
    /// processed range is adjusted after each removal (`drop_or_shift`) instead of being read
    /// from a stale snapshot, so a caret whose target was swallowed by a neighbor's take
    /// contributes nothing instead of the next unrelated command, and a selection that
    /// reached into it is clipped to what still precedes it, dropped too if that empties it.
    ///
    /// The removals bypass the undo stack (several independent ranges, not one caret's edit),
    /// so it is cleared; the caret is left where the last (leftmost) piece was removed, or 0
    /// if nothing was.
    pub fn take(&mut self) -> Vec<String> {
        let n = self.prefix_len();
        let mut ranges: Vec<Option<Range<usize>>> = self.edit.carets().into_iter().map(Some).collect();
        let mut taken: Vec<String> = Vec::new();
        let mut caret_at = 0;
        for i in (0..ranges.len()).rev() {
            let Some(r) = ranges[i].clone() else { continue };
            let piece = if !r.is_empty() {
                let (start, end) = (r.start.max(n), r.end.max(n));
                (start < end).then(|| (start..end, self.edit.text[start..end].to_string()))
            } else if r.start >= n {
                command_at(&self.edit.text[n..], r.start - n).map(|(rel, text)| (rel.start + n..rel.end + n, text))
            } else {
                None
            };
            let Some((removed, text)) = piece else { continue };
            taken.push(text);
            self.edit.text.replace_range(removed.clone(), "");
            caret_at = removed.start;
            drop_or_shift(&mut ranges[..i], &removed);
        }
        if taken.is_empty() {
            return Vec::new();
        }
        self.edit.caret = caret_at.min(self.edit.text.len());
        self.edit.anchor = None;
        self.edit.extra.clear();
        self.edit.at_line_end = false;
        self.edit.clear_undo();
        taken.into_iter().rev().collect()
    }

    /// `statusBarComposerComponentDidEndEditing:` (`CM:314-322`): the temporary field goes
    /// away, its text kept as the draft.
    pub fn field_ended(&mut self, text: &str) {
        self.saved = Some(text.to_string());
        self.shown = Shown::Hidden;
        self.temporary = false;
    }

    /// The status bar field's Enter (`SBC`/`SBCVC`, popup already closed): sends the raw text
    /// with "\n" appended and clears it; nothing on an empty field.
    pub fn field_enter(&mut self) -> Vec<Act> {
        let text = self.contents();
        if text.is_empty() {
            return Vec::new();
        }
        self.set_string("", true);
        vec![Act::SendRaw(format!("{text}\n"))]
    }

    /// Back to a fresh, hidden composer (a new session, or a pane's screen buffer reset).
    pub fn reset(&mut self) {
        self.edit = TextEdit::default();
        self.prefix = None;
        self.shown = Shown::Hidden;
        self.saved = None;
        self.temporary = false;
        self.focused = false;
        self.hidden = false;
        self.shell_provided = false;
        self.queue.clear();
    }

    /// One decoded `Action`. `Send`/`SendEach`/`EnqueueEach` route through `send`/`enqueue`
    /// above; `Up`/`Down` at the composer's first/last visual line with a single, unselected
    /// caret open history instead of moving (`moveUp`/`moveDown`, `CTV:888-908`); other edit
    /// keys go to `TextEdit` (a mutating one starting inside the prefix is refused). Every
    /// path finishes with the prefix guard, so a caret never ends up inside it.
    pub fn apply(&mut self, a: Action, lines: &[VisualLine], width: &dyn Fn(&str) -> f32) -> Vec<Act> {
        let acts = match a {
            Action::Send => self.apply_send(),
            Action::SendEach => {
                let mut acts = Vec::new();
                for command in self.take() {
                    acts.extend(self.send(&command, true, false));
                }
                acts
            }
            Action::EnqueueEach => {
                let mut acts = Vec::new();
                for command in self.take() {
                    acts.extend(self.enqueue(&command, false));
                }
                acts
            }
            Action::Tab => vec![Act::Completions { explicit: true }],
            // `composerTextViewDidFinish(cancel: true)`: always the empty command, always a
            // newline, always dismiss (`send` turns that into "\n" and a reset for the Auto
            // Composer, or a plain hide with the text kept in `saved` otherwise).
            Action::Esc => self.send("", true, true),
            Action::Control(b) => vec![Act::Control(vec![b])],
            // `CTV:739-748`: with text present this is not handled as a control byte, but as
            // whatever else Ctrl+D is bound to; a plain text editor's binding for it is delete
            // forward. Through the same prefix guard as any other mutating key (in practice
            // this can only matter if the caret itself sits in the prefix, which should not
            // happen at rest, but a Delete's own reach never looks left of the caret anyway).
            Action::ControlIfEmpty(b) => {
                if self.contents().is_empty() {
                    vec![Act::Control(vec![b])]
                } else {
                    self.edit_guarded(EditKey::Delete, lines, width);
                    Vec::new()
                }
            }
            Action::ClearScreen => vec![Act::ClearScreen],
            Action::HistorySearch => {
                self.edit.key(EditKey::SelectAll, false, lines, width);
                vec![Act::OpenHistory { prefix: String::new(), search: true }]
            }
            Action::DeleteAll => {
                self.edit.key(EditKey::SelectAll, false, lines, width);
                // Select All reaches the prefix too (selecting it is harmless); clamp before
                // the Delete that follows so only the command is cleared.
                self.clamp_prefix();
                self.edit.key(EditKey::Delete, false, lines, width);
                Vec::new()
            }
            Action::AddCaret { below } => {
                self.edit.add_caret(below, lines, width);
                Vec::new()
            }
            Action::Edit(EditKey::Up) if self.at_edge(lines, width, true) => {
                vec![Act::OpenHistory { prefix: self.contents(), search: false }]
            }
            Action::Edit(EditKey::Down) if self.at_edge(lines, width, false) => {
                vec![Act::OpenHistory { prefix: self.contents(), search: false }]
            }
            Action::Edit(k) => {
                self.edit_guarded(k, lines, width);
                Vec::new()
            }
        };
        self.clamp_prefix();
        acts
    }

    /// One `EditKey`, refused outright when the range it would actually change (its
    /// selection, or where a delete reaches from the caret; an insert never reaches left of
    /// the caret) starts inside the prefix - not merely when the caret itself does, which
    /// would let a plain Backspace right after the prefix (caret == n, deleting n-1..n) eat
    /// its last character. Shared by `apply`'s `Action::Edit` and `ControlIfEmpty`'s
    /// not-empty fallback, so neither bypasses it.
    fn edit_guarded(&mut self, k: EditKey, lines: &[VisualLine], width: &dyn Fn(&str) -> f32) {
        let n = self.prefix_len();
        if n > 0 && mutates(&k) && self.edit.reach(&k).iter().any(|r| r.start < n) {
            return;
        }
        self.edit.key(k, false, lines, width);
    }
}

/// Adjusts every not-yet-processed range in `remaining` for the removal of `removed` (already
/// deleted from the text, so `remaining`'s entries must stop referring to characters at their
/// old positions). Not a port: a deliberate divergence from iTerm2's `take`, which keeps its
/// captured ranges and shifts them with `NSRange.minus` (`CTV:1330-1360`), collapsing a range
/// the removal covered to the removal's start (then asking what command sits there now, which
/// hands a swallowed caret its neighbor's next command, or crashes past the end). Here a plain
/// caret inside `removed` is dropped (there is nothing left where it pointed); a range reaching
/// into `removed` has its end clipped to `removed.start`, dropped too if that empties it;
/// anything strictly after `removed` shifts back by its length; anything strictly before is
/// untouched (the Task 2 ruling).
///
/// `removed` always reaches at least to the position of the caret that produced it, which (by
/// `TextEdit` never letting carets or selections overlap, and `remaining` holding only carets
/// at or before that position in the original text) is always at or after every entry here's
/// own end - so an entry's end can never survive past `removed.end`; there is no case of a
/// tail surviving a removal that cuts into an entry's middle.
fn drop_or_shift(remaining: &mut [Option<Range<usize>>], removed: &Range<usize>) {
    let len = removed.end - removed.start;
    for slot in remaining.iter_mut() {
        let Some(r) = slot.clone() else { continue };
        *slot = if r.is_empty() {
            if removed.contains(&r.start) {
                None
            } else if r.start >= removed.end {
                Some(r.start - len..r.start - len)
            } else {
                Some(r)
            }
        } else if r.start >= removed.end {
            Some(r.start - len..r.end - len)
        } else if r.end > removed.start {
            let end = removed.start.min(r.end);
            let start = r.start.min(end);
            (start < end).then_some(start..end)
        } else {
            Some(r)
        };
    }
}

/// `characterRangeOfCommand(atCharacterIndex:)` (`CTV:1045-1150`), without the prefix
/// adjustment (the caller strips it): the paragraph (a line, its trailing "\n" included
/// unless it is the text's last) holding byte index `at`, extended forward and backward over
/// any paragraph that ends in a line-continuing backslash, so a `"a \\\nb"` pair is taken as
/// one. Returns the raw text of the span (backslashes and newlines untouched; iTerm2 only
/// blanks them for its explainshell query, not for `take`). `None` on an empty document.
pub fn command_at(text: &str, at: usize) -> Option<(Range<usize>, String)> {
    fn paragraph_at(text: &str, i: usize) -> Option<Range<usize>> {
        if text.is_empty() {
            return None;
        }
        if i >= text.len() && text.ends_with('\n') {
            // An empty last line: NSString's paragraphRange there has length 0, so
            // `characterRangeOfCommand` returns nil rather than reaching for the line above.
            return None;
        }
        // The start of the last character when probing at or past the end (never mid-char).
        let probe = if i >= text.len() { text.char_indices().next_back().map_or(0, |(j, _)| j) } else { i };
        let start = text[..probe].rfind('\n').map_or(0, |p| p + 1);
        let end = match text[start..].find('\n') {
            Some(off) => start + off + 1,
            None => text.len(),
        };
        Some(start..end)
    }
    fn continues(text: &str, r: &Range<usize>) -> bool {
        text[r.clone()].trim_end_matches('\n').ends_with('\\')
    }

    let at = at.min(text.len());
    let mut span = paragraph_at(text, at)?;
    while span.end < text.len() && continues(text, &span) {
        let next = paragraph_at(text, span.end)?;
        span.end = next.end;
    }
    while span.start > 0 {
        let prev = paragraph_at(text, span.start - 1)?;
        if !continues(text, &prev) {
            break;
        }
        span.start = prev.start;
    }
    Some((span.clone(), text[span].to_string()))
}

// ---------------------------------------------------------------------------------------
// Geometry and texts of the minimal composer (milestone 9l Task 6)
// ---------------------------------------------------------------------------------------

/// The dropdown's height before any drag (`ComposerHeight`'s default, `MCVC:141-142`), in
/// points.
pub const DEFAULT_HEIGHT: f32 = 135.0;
/// The shortest the dropdown gets (`MCVC:319-329`).
pub const MIN_HEIGHT: f32 = 62.0;
/// Space left and right of the dropdown (`sideMargin`, `CM:221-223`; 0 for the Auto Composer).
pub const SIDE_MARGIN: f32 = 20.0;
/// The narrowest the dropdown gets, its decoration included (`PS:24236-24249`).
pub const MIN_WIDTH: f32 = 217.0;
/// The editor's inset inside the panel (`MCXIB:33`, the wrapper at 9, 9).
pub const INSET: f32 = 9.0;
/// The panel's corner radius (`MCVC:154-156`).
pub const RADIUS: f32 = 6.0;
/// The close button at the top right (`MCXIB:66-77`), width and height.
pub const CLOSE: (f32, f32) = (24.0, 23.0);
/// The fade in on show (`MCVC:16-17`) and out on dismiss (`CM:478`), seconds.
pub const FADE_IN: f32 = 0.25;
pub const FADE_OUT: f32 = 0.125;

/// The dropdown's frame over `pane` (points, top-left origin) for a wanted `height` and the
/// profile's `Composer Top Offset` (`PS:24236-24249`, flipped from AppKit's bottom-left
/// origin): `SIDE_MARGIN` in from the left, `top_offset` down from the top, pushed up to end
/// at the pane's bottom when that would run past it, at least `MIN_WIDTH` wide. iTerm2's
/// further shrink for a height taller than the pane assigns a variable it never reads again
/// (dead code, kept dead): the height is always the one asked for; `clamp_height` already keeps
/// it inside the pane.
pub fn minimal_rect(pane: Rect, height: f32, top_offset: f32) -> Rect {
    let width = MIN_WIDTH.max(pane.w - 2.0 * SIDE_MARGIN);
    let from_bottom = (pane.h - height - top_offset).max(0.0);
    Rect { x: pane.x + SIDE_MARGIN, y: pane.y + pane.h - from_bottom - height, w: width, h: height }
}

/// A dragged or saved height kept between `MIN_HEIGHT` and 8 points short of the pane
/// (`MCVC:319-329`, `CM:390-392`: `max(min, superview height - 8)`).
pub fn clamp_height(h: f32, pane_h: f32) -> f32 {
    h.min(MIN_HEIGHT.max(pane_h - 8.0)).max(MIN_HEIGHT)
}

// ---------------------------------------------------------------------------------------
// The Auto Composer (milestone 9l Task 7)
// ---------------------------------------------------------------------------------------

/// The Auto Composer's rows for `text_lines` laid-out lines (the prefix's included, not the
/// empty line after a final newline) and whether the command ends in a newline
/// (`MCVC:331-364`: `max(1, lines)`, the newline adding one first).
pub fn auto_rows(text_lines: usize, ends_newline: bool) -> usize {
    (text_lines + usize::from(ends_newline)).max(1)
}

/// The Auto Composer's frame over the pane's `grid` (pixels, rows `cell_h` high): from the
/// prompt's first row (with no prompt, the row after the cursor's, `PS:24212-24217`) down,
/// `rows + 0.5` rows high and the grid's full width (`PS:24218-24228`). Decision: the grid
/// never scrolls under it (ConPTY owns the rows), so when the rows below the prompt run out it
/// grows upward over earlier output, never above row 0, and ends at the grid's bottom (its
/// half row dropped) - cost if wrong: a tall command covers output instead of pushing it up.
pub fn auto_rect(grid: Rect, cell_h: f32, rows: usize, prompt_row: Option<usize>, cursor_row: usize) -> Rect {
    let screen = ((grid.h / cell_h.max(1.0)).floor() as usize).max(1);
    let start = prompt_row.unwrap_or(cursor_row + 1).min(screen - 1);
    let top = start.min(screen.saturating_sub(rows.max(1)));
    let y = grid.y + top as f32 * cell_h;
    let h = ((rows.max(1) as f32 + 0.5) * cell_h).min(grid.y + grid.h - y);
    Rect { x: grid.x, y, w: grid.w, h }
}

/// The captured prompt as the Auto Composer's prefix (`PS:8037-8058`,
/// `attributedStringForScreenChars:`): its rows joined, trailing whitespace trimmed, then one
/// space in the default colors `fg` on `bg`.
pub fn auto_prefix(rows: Vec<styled_copy::Row>, text: Vec<String>, fg: crate::palette::Rgb, bg: crate::palette::Rgb) -> Prompt {
    let mut rows = rows;
    let mut text = text;
    // Whole lines of whitespace at the end go first (`trimTrailingWhitespace` on the joined
    // string reaches through the newlines).
    while text.len() > 1 && text.last().is_some_and(|t| t.trim_end().is_empty()) {
        text.pop();
        rows.pop();
    }
    rows.truncate(text.len());
    if let Some(last) = text.last_mut() {
        let keep = last.trim_end().len();
        last.truncate(keep);
        last.push(' ');
        if let Some(row) = rows.last_mut() {
            let mut left = keep;
            row.runs.retain_mut(|r| {
                if left == 0 {
                    return false;
                }
                if r.text.len() > left {
                    let mut cut = left;
                    while !r.text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    r.text.truncate(cut);
                }
                left -= r.text.len().min(left);
                !r.text.is_empty()
            });
            row.runs.push(styled_copy::Run { text: " ".into(), fg, bg, bold: false, italic: false, underline: false, link: None });
        }
    }
    Prompt { rows, text, by_trigger: false }
}

/// The help popover's lines, key then description (`LCVC:361-377`), with Windows key words in
/// place of the Mac glyphs; the AI lookup line only when generative AI is allowed.
pub fn help_lines(ai_allowed: bool) -> Vec<(&'static str, &'static str)> {
    let mut v = vec![
        ("Ctrl+Shift+Up", "Add cursor above"),
        ("Ctrl+Shift+Down", "Add cursor below"),
        ("Ctrl+Shift+click", "Add cursor"),
        ("Alt+drag", "Add cursors"),
    ];
    if ai_allowed {
        v.push(("Win+Y", "Natural language AI lookup"));
    }
    v.extend([
        ("Win+F", "Open Find bar"),
        ("Win+Alt+V", "Open in Advanced Paste"),
        ("Win+click", "Open in explainshell.com"),
        ("Shift+Enter", "Send contents or selection"),
        ("Alt+Shift+Enter", "Send command at cursor"),
        ("Alt+Enter", "Enqueue command at cursor"),
        ("Win+Shift+;", "View command history"),
    ]);
    v
}

/// The Help button's tooltip (`LCXIB:62`).
pub const HELP_TIP: &str = "Describes how to use the composer.";
/// The accessory row's right-aligned tip (`LCXIB:89`, "Press \u{21e7}\u{21a9} to send.").
pub const SEND_TIP: &str = "Press Shift+Enter to send.";
/// The AI completion warning (`LCXIB:98`); shown only with AI completions on, which Tandem
/// does not have before milestone 12.
pub const AI_WARNING: &str = "AI completion enabled: all text sent to AI";
/// What the AI button, Win+Y and an AI completion answer until milestone 12.
pub const AI_LATER: &str = "AI features come in milestone 12.";

/// Whether the send tip shows at this accessory width (`LCVC:160-168`): hidden under 180
/// points, or under 418 while the AI warning shows.
pub fn send_tip_shows(width: f32, ai_warning: bool) -> bool {
    !((ai_warning && width < 418.0) || width < 180.0)
}

/// The explainshell question (`CE:63-86`): its heading, and its permanent-silence key.
pub const EXPLAIN_HEADING: &str = "Open ExplainShell?";
pub const EXPLAIN_KEY: &str = "NoSyncExplainShell";

/// The question's text (`CE:74`).
pub fn explain_text(url: &str, browser: &str) -> String {
    format!("This will open {url} in {browser}.")
}

/// `https://explainshell.com/explain?cmd=...` for `command` (`CE:66-73`, `URLQueryItem`
/// encoding). Decision: `&`, `=`, `+` and `#` are percent-encoded too, so a command can never
/// add a parameter or be cut short (`NSURLComponents` leaves `+` raw, which explainshell would
/// read as a space) - cost if wrong: none.
pub fn explain_url(command: &str) -> String {
    let mut q = String::new();
    for b in command.bytes() {
        let keep = b.is_ascii_alphanumeric() || b"-._~!$'()*,;:@/?".contains(&b);
        if keep {
            q.push(b as char);
        } else {
            q.push_str(&format!("%{b:02X}"));
        }
    }
    format!("https://explainshell.com/explain?cmd={q}")
}

/// The command a Win+click at byte `at` of the command text asks explainshell about
/// (`CTV:1045-1150`): `command_at`'s span with each continuation backslash and the line end
/// after it turned into spaces (the only place iTerm2 rewrites them).
pub fn explain_command(text: &str, at: usize) -> Option<String> {
    let (_, raw) = command_at(text, at)?;
    let mut out = String::with_capacity(raw.len());
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let body = line.strip_suffix('\n');
        match body.and_then(|b| b.strip_suffix('\\')) {
            Some(b) if i + 1 < lines.len() => {
                out.push_str(b);
                out.push_str("  ");
            }
            _ => out.push_str(line),
        }
    }
    Some(out)
}

/// Key actions that send text (`iTermKeyBindingAction.m:526-543`, `sendsText`): a profile
/// mapping to one of these is not run while the composer has the keyboard
/// (`PS:24389-24407`). Codes from `iTermKeyBindingAction.h`: escape sequence, hex, text,
/// ^H and ^? backspaces, run coprocess, vim text, the two Paste Specials, send snippet,
/// compose, send tmux command, copy or send, paste or send, vim text without broadcasting.
pub fn sends_text(action: i64) -> bool {
    matches!(action, 10 | 11 | 12 | 16 | 17 | 35 | 38 | 41 | 42 | 63 | 64 | 65 | 69 | 70 | 76)
}

/// A composer Win chord (`CTV:610-638`, `PS:24608-24625`, `LCVC:361-377`), from the chord's
/// key and exact modifiers (`c` the key's unshifted character, or an arrow).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Chord {
    /// Win+Shift+Up or Down: the previous or next mark.
    Mark { down: bool },
    AdvancedPaste,
    Ai,
    Find,
    Close,
    History,
}

/// The composer chord for a Win chord (`key` the unshifted character, `up`/`down` an arrow;
/// Win is implied), or None.
pub fn chord(key: Option<char>, up: bool, down: bool, alt: bool, shift: bool, ctrl: bool) -> Option<Chord> {
    if ctrl {
        return None;
    }
    match (key.map(|c| c.to_ascii_lowercase()), up, down, alt, shift) {
        (None, true, false, false, true) => Some(Chord::Mark { down: false }),
        (None, false, true, false, true) => Some(Chord::Mark { down: true }),
        (Some('v'), _, _, true, false) => Some(Chord::AdvancedPaste),
        (Some('y'), _, _, false, false) => Some(Chord::Ai),
        (Some('f'), _, _, false, false) => Some(Chord::Find),
        (Some('w'), _, _, false, false) => Some(Chord::Close),
        (Some(';'), _, _, false, true) => Some(Chord::History),
        _ => None,
    }
}

/// `chord` for a Win chord from the keyboard hook (None without Win).
pub fn chord_of(c: &crate::menus::Chord) -> Option<Chord> {
    use crate::menus::ChordKey;
    if !c.cmd {
        return None;
    }
    let key = match c.key {
        ChordKey::Char(ch) => Some(ch),
        _ => None,
    };
    chord(key, c.key == ChordKey::Up, c.key == ChordKey::Down, c.alt, c.shift, c.ctrl)
}

/// Where the dropdown's parts sit inside its panel (`MCXIB`, `LCXIB`; `scale` pixels per
/// point): the close button 8 in from the right and 9 down; the editor's wrapper `INSET` in;
/// in it the text 5 in from the sides, 11 from the top and 19 from the bottom, and the
/// accessory row (27 high, its bottom 1 below the wrapper's) with the Help button (16 by 17,
/// 5 up), the AI button (32 wide, 21 in) and the send tip's right edge 10 in from the
/// wrapper's; the bottom drag handle 8 in, 6 up and 10 high.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub panel: Rect,
    pub close: Rect,
    pub text: Rect,
    pub accessory: Rect,
    pub help: Rect,
    pub ai: Rect,
    pub handle: Rect,
}

pub fn layout(panel: Rect, scale: f32) -> Layout {
    let s = scale;
    let wrap = Rect { x: panel.x + INSET * s, y: panel.y + INSET * s, w: (panel.w - 2.0 * INSET * s).max(0.0), h: (panel.h - 2.0 * INSET * s).max(0.0) };
    let bottom = wrap.y + wrap.h + s;
    let accessory = Rect { x: wrap.x + 5.0 * s, y: bottom - 27.0 * s, w: (wrap.w - 15.0 * s).max(0.0), h: 27.0 * s };
    let text = Rect { x: wrap.x + 5.0 * s, y: wrap.y + 11.0 * s, w: (wrap.w - 10.0 * s).max(0.0), h: (wrap.h - 30.0 * s).max(0.0) };
    Layout {
        panel,
        close: Rect { x: panel.x + panel.w - (8.0 + CLOSE.0) * s, y: panel.y + 9.0 * s, w: CLOSE.0 * s, h: CLOSE.1 * s },
        text,
        accessory,
        help: Rect { x: accessory.x - s, y: bottom - 22.0 * s, w: 16.0 * s, h: 17.0 * s },
        ai: Rect { x: accessory.x + 21.0 * s, y: bottom - 27.0 * s, w: 32.0 * s, h: 27.0 * s },
        handle: Rect { x: panel.x + 8.0 * s, y: panel.y + panel.h - 16.0 * s, w: (panel.w - 20.0 * s).max(0.0), h: 10.0 * s },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_edit::{wrap, Caret};

    fn w(s: &str) -> f32 {
        s.chars().count() as f32 * 10.0
    }

    fn m(ctrl: bool, alt: bool, shift: bool) -> Mods {
        Mods { ctrl, alt, shift, win: false }
    }

    #[test]
    fn sends_follow_iterm2() {
        // Minimal composer, Shift+Enter with "ls": sends it with a newline, hides, forgets
        // the draft.
        let mut c = Composer::default();
        c.shown = Shown::Dropdown;
        c.edit.load("ls");
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::Send, &lines, &w), vec![Act::Send("ls\n".to_string())]);
        assert_eq!(c.shown, Shown::Hidden);
        assert_eq!(c.saved, None);

        // Empty text: nothing sent, hidden, the empty draft kept.
        let mut c = Composer::default();
        c.shown = Shown::Dropdown;
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::Send, &lines, &w), Vec::<Act>::new());
        assert_eq!(c.shown, Shown::Hidden);
        assert_eq!(c.saved.as_deref(), Some(""));

        // Esc in the minimal composer: nothing sent, hidden, the typed text kept as the draft.
        let mut c = Composer::default();
        c.shown = Shown::Dropdown;
        c.edit.load("draft");
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::Esc, &lines, &w), Vec::<Act>::new());
        assert_eq!(c.shown, Shown::Hidden);
        assert_eq!(c.saved.as_deref(), Some("draft"));

        // Esc in the Auto Composer: sends a bare newline, stays shown, empties the text.
        let mut c = Composer::default();
        c.auto = true;
        c.shown = Shown::Dropdown;
        c.edit.load("draft");
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::Esc, &lines, &w), vec![Act::Send("\n".to_string())]);
        assert_eq!(c.shown, Shown::Dropdown);
        assert_eq!(c.edit.text, "");

        // Auto Enter sends the text plus a newline and empties it.
        let mut c = Composer::default();
        c.auto = true;
        c.shown = Shown::Dropdown;
        c.edit.load("cmd");
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::Send, &lines, &w), vec![Act::Send("cmd\n".to_string())]);
        assert_eq!(c.edit.text, "");

        // Shift+Enter with two selections "a" and "b": each is sent, a newline after all but
        // the last, and the composer stays open.
        let mut c = Composer::default();
        c.shown = Shown::Dropdown;
        c.edit.load("a b");
        c.edit.set_carets(&[Caret { at: 0, anchor: Some(1) }, Caret { at: 2, anchor: Some(3) }]);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::Send, &lines, &w), vec![Act::Send("a\n".to_string()), Act::Send("b".to_string())]);
        assert_eq!(c.shown, Shown::Dropdown);

        // Alt+Shift+Enter over "one\ntwo" with the caret in "one" takes "one\n" whole and
        // sends it plus its own newline; "two" is kept in the composer.
        let mut c = Composer::default();
        c.shown = Shown::Dropdown;
        c.edit.load("one\ntwo");
        c.edit.caret = 1;
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::SendEach, &lines, &w), vec![Act::Send("one\n\n".to_string())]);
        assert_eq!(c.edit.text, "two");
        assert_eq!(c.shown, Shown::Dropdown);

        // A backslash-continued paragraph is taken whole (not split at its own newline).
        let mut c = Composer::default();
        c.edit.load("a \\\nb\nc");
        c.edit.caret = 1;
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::SendEach, &lines, &w), vec![Act::Send("a \\\nb\n\n".to_string())]);
        assert_eq!(c.edit.text, "c");

        // Alt+Enter enqueues the taken paragraph plus its own newline.
        let mut c = Composer::default();
        c.edit.load("one\ntwo");
        c.edit.caret = 1;
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::EnqueueEach, &lines, &w), vec![Act::Enqueue("one\n\n".to_string())]);
        assert_eq!(c.edit.text, "two");

        // An empty take enqueues nothing.
        let mut c = Composer::default();
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::EnqueueEach, &lines, &w), Vec::<Act>::new());
    }

    #[test]
    fn queue_follows_iterm2() {
        assert!(enqueue_now(Some("")));
        assert!(!enqueue_now(Some("x")));
        assert!(!enqueue_now(None));

        let mut q: VecDeque<String> = VecDeque::from(["a".to_string(), "b".to_string(), "c".to_string()]);
        assert_eq!(drain(&mut q, 2), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(q, VecDeque::from(["c".to_string()]));

        assert_eq!(current_command(true, "abc", None), Some("abc".to_string()));
        assert_eq!(current_command(false, "xyz", None), None);
    }

    #[test]
    fn keys_follow_iterm2() {
        // Standard.
        assert_eq!(action(&Key::Edit(EditKey::Enter), m(false, false, true), false, false), Some(Action::Send));
        assert_eq!(action(&Key::Edit(EditKey::Enter), m(false, true, true), false, false), Some(Action::SendEach));
        assert_eq!(action(&Key::Edit(EditKey::Enter), m(false, true, false), false, false), Some(Action::EnqueueEach));
        assert_eq!(action(&Key::Edit(EditKey::Enter), Mods::default(), false, false), Some(Action::Edit(EditKey::Enter)));
        assert_eq!(action(&Key::Edit(EditKey::Tab), Mods::default(), false, false), Some(Action::Tab));
        assert_eq!(action(&Key::Edit(EditKey::Up), m(true, false, true), false, false), Some(Action::AddCaret { below: false }));
        assert_eq!(action(&Key::Edit(EditKey::Down), m(true, false, true), false, false), Some(Action::AddCaret { below: true }));

        // Auto.
        assert_eq!(action(&Key::Edit(EditKey::Enter), Mods::default(), true, false), Some(Action::Send));
        assert_eq!(action(&Key::Edit(EditKey::Enter), m(false, false, true), true, false), Some(Action::Edit(EditKey::Enter)));
        assert_eq!(action(&Key::Ctrl('c'), Mods::default(), true, false), Some(Action::Control(3)));
        assert_eq!(action(&Key::Ctrl('d'), Mods::default(), true, false), Some(Action::ControlIfEmpty(4)));
        assert_eq!(action(&Key::Ctrl('l'), Mods::default(), true, false), Some(Action::ClearScreen));
        assert_eq!(action(&Key::Ctrl('r'), Mods::default(), true, false), Some(Action::HistorySearch));
        assert_eq!(action(&Key::Ctrl('u'), Mods::default(), true, false), Some(Action::DeleteAll));
        // Windows rule: Ctrl+A stays Select All even in the Auto Composer.
        assert_eq!(action(&Key::Ctrl('a'), Mods::default(), true, false), Some(Action::Edit(EditKey::SelectAll)));

        // Standard Ctrl+C is Copy (only the Auto Composer steals it for the shell).
        assert_eq!(action(&Key::Ctrl('c'), Mods::default(), false, false), Some(Action::Edit(EditKey::Copy)));

        // A popup takes Up and Down itself; Enter is unaffected (the caller only reaches
        // `action` for it once the popup has declined).
        assert_eq!(action(&Key::Edit(EditKey::Up), Mods::default(), false, true), None);
        assert_eq!(action(&Key::Edit(EditKey::Down), Mods::default(), false, true), None);
        assert_eq!(action(&Key::Edit(EditKey::Enter), m(false, false, true), false, true), Some(Action::Send));
        assert_eq!(action(&Key::Esc, Mods::default(), false, true), None);

        // Win chords are never the composer's.
        assert_eq!(action(&Key::Edit(EditKey::Enter), Mods { win: true, ..Mods::default() }, false, false), None);

        // Tab is bound only with no modifiers; Ctrl+Tab switches tabs, Shift+Tab is not
        // Completions either.
        assert_eq!(action(&Key::Edit(EditKey::Tab), m(true, false, false), false, false), None);
        assert_eq!(action(&Key::Edit(EditKey::Tab), m(false, false, true), false, false), None);

        // The auto-mode Ctrl chords need exactly Ctrl, no Shift or Alt.
        assert_eq!(action(&Key::Ctrl('c'), m(true, false, true), true, false), None);
        assert_eq!(action(&Key::Ctrl('l'), m(true, true, false), true, false), None);

        // AltGr (Ctrl+Alt held, but the key types text) reaches the editor as text, not
        // dropped by the Alt gate.
        assert_eq!(
            action(&Key::Edit(EditKey::Text("@".to_string())), m(true, true, false), false, false),
            Some(Action::Edit(EditKey::Text("@".to_string())))
        );
        // A genuine Ctrl+Alt chord on a non-text key is still not the composer's.
        assert_eq!(action(&Key::Edit(EditKey::Backspace), m(true, true, false), false, false), None);
    }

    #[test]
    fn routing_follows_iterm2() {
        assert!(using_status_bar(true, true, false));
        assert!(using_status_bar(true, false, true));
        assert!(!using_status_bar(false, true, true));

        let mut c = Composer::default();
        assert_eq!(c.toggle(true), Shown::Field);

        let mut c = Composer::default();
        c.shown = Shown::Dropdown;
        c.edit.load("typed");
        assert_eq!(c.toggle(true), Shown::Hidden);
        assert_eq!(c.saved.as_deref(), Some("typed"));

        let mut c = Composer::default();
        c.show_with("x", false, false);
        assert_eq!(c.shown, Shown::Dropdown);
        assert_eq!(c.edit.text, "x");
        assert_eq!(c.edit.caret, 1);

        let mut c = Composer::default();
        c.shown = Shown::Dropdown;
        c.edit.load("a");
        c.show_or_append("b");
        assert_eq!(c.edit.text, "a\nb");

        let mut c = Composer::default();
        c.field_ended("q");
        assert_eq!(c.saved.as_deref(), Some("q"));

        let mut c = Composer::default();
        c.edit.load("ls");
        assert_eq!(c.field_enter(), vec![Act::SendRaw("ls\n".to_string())]);
        assert_eq!(c.edit.text, "");
        assert_eq!(c.field_enter(), Vec::<Act>::new());

        // set_string with a prefix keeps it uneditable: a mutating key starting inside it is
        // refused (nothing changes) and the caret clamps to just after it.
        let mut c = Composer::default();
        c.prefix = Some(Prompt { rows: Vec::new(), text: vec!["$ ".to_string()], by_trigger: false });
        c.set_string("ls", false);
        assert_eq!(c.edit.text, "$ ls");
        assert_eq!(c.edit.caret, 4);
        c.edit.set_carets(&[Caret { at: 1, anchor: None }]);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::Edit(EditKey::Text("X".to_string())), &lines, &w), Vec::<Act>::new());
        assert_eq!(c.edit.text, "$ ls");
        assert_eq!(c.edit.caret, 2);
    }

    #[test]
    fn open_history_at_the_edges() {
        // Up on the composer's only (first) visual line opens history with the text.
        let mut c = Composer::default();
        c.edit.load("abc");
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::Edit(EditKey::Up), &lines, &w), vec![Act::OpenHistory { prefix: "abc".to_string(), search: false }]);

        // Ctrl+R (HistorySearch) selects everything and opens it for search, with no prefix.
        let mut c = Composer::default();
        c.edit.load("abc");
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::HistorySearch, &lines, &w), vec![Act::OpenHistory { prefix: String::new(), search: true }]);
        assert_eq!(c.edit.selection(), Some(0..3));
    }

    #[test]
    fn command_at_takes_backslash_continuations_whole() {
        assert_eq!(command_at("one\ntwo", 1), Some((0..4, "one\n".to_string())));
        assert_eq!(command_at("a \\\nb", 1), Some((0..5, "a \\\nb".to_string())));
        assert_eq!(command_at("", 0), None);
    }

    fn dollar_prefix() -> Option<Prompt> {
        Some(Prompt { rows: Vec::new(), text: vec!["$ ".to_string()], by_trigger: false })
    }

    #[test]
    fn command_at_handles_multibyte_and_empty_trailing_lines() {
        // A caret at the very end after a multi-byte character does not slice mid-char.
        assert_eq!(command_at("echo \u{e9}", 7), Some((0..7, "echo \u{e9}".to_string())));
        // An empty last line (the text ends in "\n") takes nothing, not the line above.
        assert_eq!(command_at("abc\n", 4), None);
    }

    #[test]
    fn prefix_survives_auto_reset_and_show_or_append() {
        // Auto Composer: Enter clears the command but keeps the prompt, and typing right
        // after does not panic (the caret used to end up past the end of a text whose prefix
        // had been dropped).
        let mut c = Composer::default();
        c.auto = true;
        c.shown = Shown::Dropdown;
        c.prefix = dollar_prefix();
        c.set_string("ls", false);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::Send, &lines, &w), vec![Act::Send("ls\n".to_string())]);
        assert_eq!(c.edit.text, "$ ");
        let lines = wrap(&c.edit.text, 1000.0, &w);
        c.apply(Action::Edit(EditKey::Text("x".to_string())), &lines, &w);
        assert_eq!(c.edit.text, "$ x");

        // Esc in the Auto Composer resets the same way; it must not panic either.
        let mut c = Composer::default();
        c.auto = true;
        c.shown = Shown::Dropdown;
        c.prefix = dollar_prefix();
        c.set_string("ls", false);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        c.apply(Action::Esc, &lines, &w);
        assert_eq!(c.edit.text, "$ ");

        // show_or_append onto an open dropdown keeps the prefix, joining only the commands.
        let mut c = Composer::default();
        c.shown = Shown::Dropdown;
        c.prefix = dollar_prefix();
        c.set_string("a", false);
        c.show_or_append("b");
        assert_eq!(c.edit.text, "$ a\nb");
    }

    #[test]
    fn take_stays_out_of_the_prefix_and_avoids_double_removal() {
        // send-each with a prefix: the prompt and the command share one hard line ("$ ls"), so
        // a naive paragraph search would sweep the prompt in too.
        let mut c = Composer::default();
        c.prefix = dollar_prefix();
        c.set_string("ls", false);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::SendEach, &lines, &w), vec![Act::Send("ls\n".to_string())]);
        assert_eq!(c.edit.text, "$ ");
        // The caret is left where the command was removed (just after the prefix), not reset
        // to 0.
        assert_eq!(c.edit.caret, 2);

        // A selection reaching into the prefix is clipped to the command portion.
        let mut c = Composer::default();
        c.prefix = dollar_prefix();
        c.set_string("ls", false);
        c.edit.set_carets(&[Caret { at: 0, anchor: Some(4) }]);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::SendEach, &lines, &w), vec![Act::Send("ls\n".to_string())]);
        assert_eq!(c.edit.text, "$ ");

        // Two carets landing in the same command (no prefix here) do not try to remove the
        // same range twice.
        let mut c = Composer::default();
        c.edit.load("ab");
        c.edit.set_carets(&[Caret { at: 0, anchor: None }, Caret { at: 1, anchor: None }]);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::SendEach, &lines, &w), vec![Act::Send("ab\n".to_string())]);
        assert_eq!(c.edit.text, "");

        // Alt+Enter then Ctrl+Z after a take does not panic (take bypasses the undo stack).
        let mut c = Composer::default();
        c.edit.load("one\ntwo");
        c.edit.caret = 1;
        let lines = wrap(&c.edit.text, 1000.0, &w);
        c.apply(Action::EnqueueEach, &lines, &w);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::Edit(EditKey::Undo), &lines, &w), Vec::<Act>::new());
        assert_eq!(c.edit.text, "two");
    }

    #[test]
    fn take_remaps_ranges_after_each_removal() {
        // A later (rightmost) caret's command can range, via backslash continuation, back
        // over an earlier caret's own position - captured once, that earlier range would then
        // be sliced against the shorter, post-removal text.
        //
        // Repro 1 (round 2, finding 3): a selection entirely swallowed by a later caret's much
        // larger command must be dropped, not sliced at its old (now out of range) offsets.
        let mut c = Composer::default();
        c.prefix = dollar_prefix();
        c.set_string("\u{e9} \\\nb", false);
        c.edit.set_carets(&[Caret { at: 2, anchor: Some(4) }, Caret { at: 8, anchor: None }]);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::SendEach, &lines, &w), vec![Act::Send("\u{e9} \\\nb\n".to_string())]);
        assert_eq!(c.edit.text, "$ ");

        // Repro 2: an earlier plain caret that lands inside a command a later caret already
        // took is dropped, not re-looked-up at its collapsed (and here, mid-character)
        // position.
        let mut c = Composer::default();
        c.prefix = dollar_prefix();
        c.set_string("ab\n\u{e9}x", false);
        c.edit.set_carets(&[Caret { at: 3, anchor: None }, Caret { at: 4, anchor: None }]);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::SendEach, &lines, &w), vec![Act::Send("ab\n\n".to_string())]);
        assert_eq!(c.edit.text, "$ \u{e9}x");

        // Repro 3: the same shape, but the text after the consumed command is itself a
        // plausible-looking command ("rm -rf x") - the dropped caret must not send it.
        let mut c = Composer::default();
        c.prefix = dollar_prefix();
        c.set_string("ab\nrm -rf x", false);
        c.edit.set_carets(&[Caret { at: 3, anchor: None }, Caret { at: 4, anchor: None }]);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::SendEach, &lines, &w), vec![Act::Send("ab\n\n".to_string())]);
        assert_eq!(c.edit.text, "$ rm -rf x");
    }

    #[test]
    fn prefix_length_is_safe_against_a_mismatched_text() {
        // `prefix` assigned directly (as Task 3 will do when a fresh prompt is captured)
        // without going through `set_string` to keep `edit.text` in sync: `contents` and
        // `apply` must not panic, and must treat the text as having no prefix rather than
        // slicing into the middle of it (the guessed length here would land mid-character).
        let mut c = Composer::default();
        c.prefix = dollar_prefix();
        c.edit.load("h\u{e9}");
        assert_eq!(c.contents(), "h\u{e9}");
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::Edit(EditKey::Text("!".to_string())), &lines, &w), Vec::<Act>::new());
        assert_eq!(c.edit.text, "h\u{e9}!");
    }

    #[test]
    fn prefix_survives_backspace_and_delete_all() {
        // Backspace right after the prefix does nothing (refused), not eat its last
        // character.
        let mut c = Composer::default();
        c.prefix = dollar_prefix();
        c.set_string("x", false);
        c.edit.set_carets(&[Caret { at: 2, anchor: None }]);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::Edit(EditKey::Backspace), &lines, &w), Vec::<Act>::new());
        assert_eq!(c.edit.text, "$ x");

        // Ctrl+Backspace (DeleteWordBack) right after the prefix is refused the same way.
        let mut c = Composer::default();
        c.prefix = dollar_prefix();
        c.set_string("word", false);
        c.edit.set_carets(&[Caret { at: 2, anchor: None }]);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::Edit(EditKey::DeleteWordBack), &lines, &w), Vec::<Act>::new());
        assert_eq!(c.edit.text, "$ word");

        // Ctrl+U (DeleteAll) clears only the command, keeping the prefix.
        let mut c = Composer::default();
        c.prefix = dollar_prefix();
        c.set_string("ls -la", false);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::DeleteAll, &lines, &w), Vec::<Act>::new());
        assert_eq!(c.edit.text, "$ ");
    }

    #[test]
    fn history_opens_only_at_the_true_end() {
        // "abc\ndef" with the caret right after "abc" (not the end of the whole text, even
        // though that is the first visual line): a plain move, not history.
        let mut c = Composer::default();
        c.edit.load("abc\ndef");
        c.edit.caret = 3;
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::Edit(EditKey::Up), &lines, &w), Vec::<Act>::new());
        assert_eq!(c.edit.caret, 0);

        // At the true end of a multi-line command, Down still opens history.
        let mut c = Composer::default();
        c.edit.load("abc\ndef");
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(
            c.apply(Action::Edit(EditKey::Down), &lines, &w),
            vec![Act::OpenHistory { prefix: "abc\ndef".to_string(), search: false }]
        );
    }

    #[test]
    fn saved_reloads_once_and_dismiss_clears_auto() {
        // After a successful send, saved is None; reopening starts empty, not with the sent
        // text (the underlying view is destroyed and rebuilt on every dismiss/show).
        let mut c = Composer::default();
        c.shown = Shown::Dropdown;
        c.edit.load("ls");
        let lines = wrap(&c.edit.text, 1000.0, &w);
        c.apply(Action::Send, &lines, &w);
        assert_eq!(c.saved, None);
        assert_eq!(c.toggle(false), Shown::Dropdown);
        assert_eq!(c.edit.text, "");

        // Esc keeps the draft as `saved`; reopening restores it once and then forgets it.
        let mut c = Composer::default();
        c.shown = Shown::Dropdown;
        c.edit.load("draft");
        let lines = wrap(&c.edit.text, 1000.0, &w);
        c.apply(Action::Esc, &lines, &w);
        assert_eq!(c.saved.as_deref(), Some("draft"));
        assert_eq!(c.toggle(false), Shown::Dropdown);
        assert_eq!(c.edit.text, "draft");
        assert_eq!(c.saved, None);

        // dismiss() clears the Auto Composer flag (`dismissAnimated:`).
        let mut c = Composer::default();
        c.auto = true;
        c.shown = Shown::Dropdown;
        assert!(c.dismiss());
        assert!(!c.auto);
    }

    #[test]
    fn control_if_empty_falls_through_when_not_empty() {
        // With text present, Ctrl+D is not swallowed: it falls through to its normal binding
        // (delete forward), distinguishable from "handled, nothing to do".
        let mut c = Composer::default();
        c.edit.load("abc");
        c.edit.caret = 0;
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::ControlIfEmpty(4), &lines, &w), Vec::<Act>::new());
        assert_eq!(c.edit.text, "bc");

        let mut c = Composer::default();
        let lines = wrap(&c.edit.text, 1000.0, &w);
        assert_eq!(c.apply(Action::ControlIfEmpty(4), &lines, &w), vec![Act::Control(vec![4])]);
    }

    #[test]
    fn geometry_follows_iterm2() {
        let pane = |w: f32| Rect { x: 0.0, y: 0.0, w, h: 600.0 };
        assert_eq!(minimal_rect(pane(800.0), 135.0, 0.0), Rect { x: 20.0, y: 0.0, w: 760.0, h: 135.0 });
        assert_eq!(minimal_rect(pane(200.0), 135.0, 0.0).w, 217.0);
        // An offset that would run past the bottom pushes it up to end at the bottom.
        assert_eq!(minimal_rect(pane(800.0), 135.0, 500.0).y, 465.0);
        assert_eq!(minimal_rect(pane(800.0), 135.0, 100.0).y, 100.0);
        assert_eq!(clamp_height(10.0, 600.0), 62.0);
        assert_eq!(clamp_height(900.0, 600.0), 592.0);
        assert_eq!(clamp_height(200.0, 600.0), 200.0);

        let plain = help_lines(false);
        assert_eq!(plain.len(), 11);
        assert_eq!(plain[0], ("Ctrl+Shift+Up", "Add cursor above"));
        assert_eq!(plain[4], ("Win+F", "Open Find bar"));
        assert_eq!(plain[10], ("Win+Shift+;", "View command history"));
        let ai = help_lines(true);
        assert_eq!(ai.len(), 12);
        assert_eq!(ai[4], ("Win+Y", "Natural language AI lookup"));
        assert_eq!(ai[5], ("Win+F", "Open Find bar"));

        assert!(!send_tip_shows(179.0, false));
        assert!(send_tip_shows(180.0, false));
        assert!(!send_tip_shows(300.0, true));
        assert!(send_tip_shows(420.0, true));
        assert_eq!(SEND_TIP, "Press Shift+Enter to send.");
        assert_eq!(AI_WARNING, "AI completion enabled: all text sent to AI");

        assert_eq!(EXPLAIN_HEADING, "Open ExplainShell?");
        assert_eq!(EXPLAIN_KEY, "NoSyncExplainShell");
        assert_eq!(explain_url("ls -l"), "https://explainshell.com/explain?cmd=ls%20-l");
        assert_eq!(explain_url("a&b=c+d#"), "https://explainshell.com/explain?cmd=a%26b%3Dc%2Bd%23");
        assert_eq!(explain_text("https://x", "Edge"), "This will open https://x in Edge.");
        // Continuations become spaces for explainshell; a following command is not included.
        assert_eq!(explain_command("ls \\\n-l\nrm", 0).as_deref(), Some("ls   -l\n"));
        assert_eq!(explain_command("", 0), None);

        assert!(sends_text(12) && sends_text(64) && !sends_text(0) && !sends_text(25));
        assert_eq!(chord(Some('y'), false, false, false, false, false), Some(Chord::Ai));
        assert_eq!(chord(Some('v'), false, false, true, false, false), Some(Chord::AdvancedPaste));
        assert_eq!(chord(None, true, false, false, true, false), Some(Chord::Mark { down: false }));
        assert_eq!(chord(Some(';'), false, false, false, true, false), Some(Chord::History));
        assert_eq!(chord(Some('c'), false, false, false, false, false), None);
    }

    #[test]
    fn auto_geometry_follows_iterm2() {
        // `MCVC:331-364`: one row at least, one more when the text ends in a newline.
        assert_eq!(auto_rows(1, true), 2);
        assert_eq!(auto_rows(0, false), 1);
        assert_eq!(auto_rows(0, true), 1);
        assert_eq!(auto_rows(3, false), 3);
        let grid = Rect { x: 4.0, y: 30.0, w: 800.0, h: 24.0 * 20.0 };
        let row = |r: f32| grid.y + r * 20.0;
        // The prompt on row 20 of 24, 2 rows wanted: rows 20 and 21 plus half a row, full width.
        assert_eq!(auto_rect(grid, 20.0, 2, Some(20), 23), Rect { x: 4.0, y: row(20.0), w: 800.0, h: 50.0 });
        // 6 wanted with 4 rows below the prompt: grows upward to rows 18 to 23.
        assert_eq!(auto_rect(grid, 20.0, 6, Some(20), 23), Rect { x: 4.0, y: row(18.0), w: 800.0, h: 120.0 });
        // Never above row 0.
        assert_eq!(auto_rect(grid, 20.0, 30, Some(20), 23), Rect { x: 4.0, y: row(0.0), w: 800.0, h: 480.0 });
        // No prompt row: the row after the cursor's (`PS:24212-24217`), kept on the screen.
        assert_eq!(auto_rect(grid, 20.0, 1, None, 10), Rect { x: 4.0, y: row(11.0), w: 800.0, h: 30.0 });
        assert_eq!(auto_rect(grid, 20.0, 1, None, 23).y, row(23.0));
    }

    #[test]
    fn auto_prefix_trims_and_ends_in_a_space() {
        use crate::palette::{rgb, Rgb};
        let (fg, bg, red) = (rgb(1, 1, 1), rgb(0, 0, 0), rgb(9, 0, 0));
        let run = |t: &str, c: Rgb| styled_copy::Run { text: t.into(), fg: c, bg, bold: false, italic: false, underline: false, link: None };
        let rows = vec![styled_copy::Row { runs: vec![run("info", fg)], hard_end: true }, styled_copy::Row { runs: vec![run("PS", red), run(">  ", fg)], hard_end: true }];
        let p = auto_prefix(rows, vec!["info".into(), "PS>  ".into()], fg, bg);
        // `PS:8037-8058`: trailing whitespace trimmed, then one space in the default colors.
        assert_eq!(p.text, vec!["info".to_string(), "PS> ".to_string()]);
        let last: Vec<&str> = p.rows[1].runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(last, vec!["PS", ">", " "]);
        assert_eq!(p.rows[1].runs[0].fg, red);
        assert!(!p.by_trigger);
        // A prefix set through `set_prefix` keeps the command after it.
        let mut c = Composer::default();
        c.set_string("ls", true);
        c.set_prefix(Some(p));
        assert_eq!(c.text(), "info\nPS> ls");
        assert_eq!(c.contents(), "ls");
    }

    #[test]
    fn a_click_in_the_prefix_types_normally() {
        // A click inside the prompt prefix puts the caret at the prefix's end, so the next key
        // is typed there instead of being refused by the prefix guard.
        let mut c = Composer::default();
        c.prefix = dollar_prefix();
        c.set_string("ls", false);
        c.click(0, false, false, 0, 500);
        assert_eq!(c.edit.caret, 2);
        let lines = wrap(&c.edit.text, 1000.0, &w);
        c.apply(Action::Edit(EditKey::Text("x".to_string())), &lines, &w);
        assert_eq!(c.edit.text, "$ xls");
        // A drag or box that reaches into the prefix is kept out of it too.
        c.edit.set_carets(&[Caret { at: 1, anchor: Some(4) }]);
        c.keep_out_of_prefix();
        assert_eq!(c.edit.selection(), Some(2..4));
        let lines = wrap(&c.edit.text, 1000.0, &w);
        c.apply(Action::Edit(EditKey::Text("y".to_string())), &lines, &w);
        assert_eq!(c.edit.text, "$ ys");
    }
}
