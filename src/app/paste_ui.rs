//! Pasting (iTerm2's iTermPasteHelper): each paste is transformed, may ask a question
//! first, and is written in chunks on the event loop's tick with a progress bar (on the
//! pane's status bar when one is shown).

use super::*;
use crate::broadcast::Write;
use crate::paste::{self, Job, Speed, Warning};
use crate::warnings;

/// The progress bar (its Cancel button).
const PASTE_BAR: &str = "paste-progress";

/// A literal write that waits for a pane's pastes to drain and then for a delay (13y1 Task 7).
pub(super) struct LiteralAfter {
    pub pane: PaneId,
    pub text: String,
    pub delay: Duration,
    pub due: Option<Instant>,
    pub owner: String,
}

/// A paste waiting for its questions' answers.
pub(super) struct Asking {
    job: Job,
    speed: Speed,
    what: Write,
    warnings: Vec<Warning>,
}

impl App {
    /// Ctrl+Shift+V, middle click, drops, and every plain paste: iTerm2's default flags
    /// (newlines to CR, unsafe controls removed, bracketing when the program asks).
    pub(super) fn user_paste(&mut self, sender: PaneId, text: &str, what: Write) {
        self.paste_text(sender, text, what, false);
    }

    /// A plain paste, quickly or slowly (Paste Slowly).
    pub(super) fn paste_text(&mut self, sender: PaneId, text: &str, what: Write, slow: bool) {
        let job = Job::new(text, paste::NEWLINES | paste::CONTROLS | paste::BRACKET | paste::PLAIN);
        let speed = self.paste_speed(slow);
        self.paste_job(sender, job, speed, what);
    }

    /// The speed from the settings (0 means iTerm2's default).
    pub(super) fn paste_speed(&self, slow: bool) -> Speed {
        let (units, delay, d) = if slow { ("SlowPasteBytesPerCall", "SlowPasteDelayBetweenCalls", paste::SLOW) } else { ("QuickPasteBytesPerCall", "QuickPasteDelayBetweenCalls", paste::QUICK) };
        let t = self.globals.get(delay).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
        paste::speed_from(self.globals.int(units), t, d)
    }

    fn warn_prefs(&self) -> paste::WarnPrefs {
        let g = &self.globals;
        paste::WarnPrefs {
            when_not_at_prompt: g.bool("PromptForPasteWhenNotAtPrompt"),
            skip_one_line_newline: g.bool("SuppressMultilinePasteWarningWhenPastingOneLineWithTerminalNewline"),
            skip_not_at_prompt: g.bool("SuppressMultilinePasteWarningWhenNotAtShellPrompt"),
            size_limit: g.int("AlwaysWarnBeforePastingOverSize"),
        }
    }

    /// A paste with its flags and speed: its questions first, then the queue.
    pub(super) fn paste_job(&mut self, sender: PaneId, job: Job, speed: Speed, what: Write) {
        if job.text.is_empty() {
            return;
        }
        // A replaying pane takes no input (`Pane::input`): no question, no progress bar.
        let Some(p) = self.panes.get(&sender).filter(|p| p.replay.is_none()) else { return };
        let bracketed = self.paste_bracketed(sender);
        let warnings = paste::check(&job.text, p.own().at_prompt(), bracketed, job.flags, &self.warn_prefs());
        if warnings.is_empty() {
            return self.paste_run(sender, job, speed, what);
        }
        // A paste while the pane's question waits is dropped: iTerm2's question is modal, so
        // no second paste starts under it, and the answer belongs to the first.
        if self.paste_asks.contains_key(&sender) {
            return;
        }
        self.paste_asks.insert(sender, Asking { job, speed, what, warnings });
        self.paste_ask_next(sender);
    }

    pub(super) fn paste_bracketed_pub(&self, pane: PaneId) -> bool {
        self.paste_bracketed(pane)
    }

    /// The program asked for bracketed paste and the profile allows it.
    fn paste_bracketed(&self, pane: PaneId) -> bool {
        let Some(p) = self.panes.get(&pane) else { return false };
        let allowed = self.store.get(&p.profile).is_none_or(|pr| pr.bool("Allow Paste Bracketing"));
        allowed && p.own().term().mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE)
    }

    /// The paste's next question, as an alert (iTerm2's modal `iTermWarning`); a silenced one
    /// answers its remembered button at once.
    fn paste_ask_next(&mut self, pane: PaneId) {
        let Some(w) = self.paste_asks.get(&pane).and_then(|a| a.warnings.first().cloned()) else { return };
        let spec = w.spec(w.heading(), &w.text());
        // Issue 5115 (`iTermPasteHelper.m:850`): a multi-line question silenced with Cancel
        // asks again.
        if let (Warning::Lines { .. } | Warning::OneLine { .. }, Some(id)) = (&w, spec.id.as_deref()) {
            if warnings::is_silenced(&self.globals, id, super::warnings_app::unix_now()) && spec.cancel == Some(warnings::saved_selection(&self.globals, id)) {
                self.globals.unset(id);
                self.save_globals();
            }
        }
        self.ask_scripts(scripts_alerts::ask(&spec.heading, &spec.text, w.buttons(), spec.id.as_deref(), spec.kind, scripts_alerts::Then::Paste(pane)));
    }

    /// Transforms the paste and queues it; the first chunk goes now.
    fn paste_run(&mut self, sender: PaneId, mut job: Job, speed: Speed, what: Write) {
        if !self.paste_bracketed(sender) {
            job.flags &= !paste::BRACKET;
        }
        let prefs = paste::Prefs { escape_with_quotes: self.globals.bool("EscapeWithQuotes"), strip_zero_width: self.globals.bool("StripZeroWidthFormatCharactersOnPaste") };
        let mut text = paste::sanitize(&job, &prefs);
        if job.flags & paste::BRACKET != 0 {
            text = paste::bracket(&text, false);
        }
        if text.is_empty() {
            return;
        }
        // The history keeps what was pasted (before bracketing; iTerm2's).
        self.remember_paste(&paste::sanitize(&job, &prefs));
        let total = text.encode_utf16().count();
        let commands = job.flags & paste::COMMANDS != 0;
        let entry = self.pastes.entry(sender).or_insert_with(|| (paste::Queue::default(), what, false));
        entry.0.push(&text, speed, commands);
        entry.1 = what;
        entry.2 |= paste::shows_progress(total, speed, commands);
        // iTerm2: pasting clears the selection (PastingClearsSelection).
        if self.globals.bool("PastingClearsSelection") {
            if let Some(p) = self.panes.get_mut(&sender) {
                p.clear_selection();
            }
        }
        self.paste_tick();
    }

    /// An automatic paste (13y1 Task 7, B6): the job goes straight to the queue, never through a question
    /// (nobody is there to answer one). The caller has already refused every case the paste's own check
    /// would have asked about: a pane that did not ask for bracketed paste, a password prompt, a size.
    pub(super) fn paste_auto(&mut self, sender: PaneId, job: Job) {
        let speed = self.paste_speed(false);
        self.paste_run(sender, job, speed, Write::Paste);
    }

    /// Writes `text` to `pane` as is (no bracket, no transform) `delay` after the pane's pastes in flight
    /// have fully drained: the clock starts when the content finished, not when it was queued
    /// (`pasteLiteralString(_:afterDelay:)`). `owner` is the workgroup instance it belongs to: it never
    /// fires into a pane that is no longer that instance's, and `cancel_paste_after` drops it.
    pub(super) fn paste_literal_after(&mut self, pane: PaneId, text: &str, delay: Duration, owner: &str) {
        self.paste_after.push(LiteralAfter { pane, text: text.to_string(), delay, due: None, owner: owner.to_string() });
    }

    /// Drops every waiting literal write of `owner` (a teardown, a member that ended, a toggle that went off).
    pub(super) fn cancel_paste_after(&mut self, owner: &str) {
        self.paste_after.retain(|l| l.owner != owner);
    }

    /// Fires the literal writes whose pane has drained and whose delay passed. When the next is due.
    fn paste_after_tick(&mut self) -> Option<Instant> {
        if self.paste_after.is_empty() {
            return None;
        }
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        let (mut keep, mut fire) = (Vec::new(), Vec::new());
        for mut l in std::mem::take(&mut self.paste_after) {
            if self.pastes.contains_key(&l.pane) {
                l.due = None;
                keep.push(l);
                continue;
            }
            let due = *l.due.get_or_insert(now + l.delay);
            if due <= now {
                fire.push(l);
            } else {
                next = Some(next.map_or(due, |n: Instant| n.min(due)));
                keep.push(l);
            }
        }
        self.paste_after = keep;
        for l in fire {
            // The instance must still be this pane's workgroup (a reused pane id gets nothing).
            let live = self.workgroups.get(&l.owner).is_some() && self.panes.get(&l.pane).is_some_and(|p| p.workgroup.as_deref() == Some(l.owner.as_str()) && p.exited.is_none());
            if live {
                self.user_write(l.pane, l.text.as_bytes(), Write::Paste);
            }
        }
        next
    }

    /// The paste queues' chunks, then the literal writes behind them. When the next is due.
    pub(super) fn paste_tick(&mut self) -> Option<Instant> {
        let queued = self.paste_queue_tick();
        let after = self.paste_after_tick();
        [queued, after].into_iter().flatten().min()
    }

    /// Writes the chunks that are due. When the next is due, for the event loop.
    fn paste_queue_tick(&mut self) -> Option<Instant> {
        if self.pastes.is_empty() {
            return None;
        }
        let now = self.started.elapsed().as_secs_f64();
        let mut writes = Vec::new();
        let mut due: Option<f64> = None;
        let mut bars = Vec::new();
        let mut done = Vec::new();
        for (id, (q, what, bar)) in self.pastes.iter_mut() {
            let prompts = self.panes.get(id).map_or(0, |p| p.own().prompts_seen());
            if let Some(chunk) = q.tick(now, prompts) {
                writes.push((*id, chunk, *what));
            }
            if let Some(t) = q.next_due() {
                due = Some(due.map_or(t, |d: f64| d.min(t)));
            }
            match (q.progress(), q.busy()) {
                (Some((sent, total)), _) if *bar => bars.push((*id, Some(sent * 100 / total.max(1)))),
                (_, false) => {
                    bars.push((*id, None));
                    done.push(*id);
                }
                _ => {}
            }
        }
        for id in done {
            self.pastes.remove(&id);
        }
        for (id, chunk, what) in writes {
            self.user_write(id, chunk.as_bytes(), what);
        }
        // With a status bar shown for the pane, the progress is its temporary right
        // component (`iTermPasteViewManager.m:70-89`); without one, the info bar.
        for (id, pct) in bars {
            let on_bar = self.bar_shown_for(id);
            match pct {
                Some(pct) if !on_bar => {
                    self.ask(id, PASTE_BAR, &format!("Pasting... {pct}%"), &["Cancel"], true);
                }
                _ => {
                    self.drop_bar(id, PASTE_BAR);
                    if on_bar {
                        self.redraw();
                        self.redraw_all();
                    }
                }
            }
        }
        due.map(|t| self.started + std::time::Duration::from_secs_f64(t.max(now)))
    }

    /// The pane's paste shows its progress (a slow one, still sending).
    pub(super) fn paste_shows_progress(&self, pane: PaneId) -> bool {
        self.pastes.get(&pane).is_some_and(|(q, _, bar)| *bar && q.progress().is_some())
    }

    /// Cancels the pane's paste (the progress's Cancel).
    pub(super) fn cancel_paste(&mut self, pane: PaneId) {
        if let Some((q, _, _)) = self.pastes.get_mut(&pane) {
            q.cancel();
        }
        self.drop_bar(pane, PASTE_BAR);
        self.paste_tick();
    }

    /// The progress bar's answer. True when the bar was it.
    pub(super) fn paste_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        if id != PASTE_BAR {
            return false;
        }
        if button == Some("Cancel") {
            self.cancel_paste(pane);
        }
        true
    }

    /// The paste question's answer, button `index` of its warning (`Then::Paste`); None when
    /// the question will never show (the alert queue was full), which cancels the paste.
    pub(super) fn paste_answered(&mut self, pane: PaneId, index: Option<usize>) {
        let Some(mut a) = self.paste_asks.remove(&pane) else { return };
        let w = a.warnings.remove(0);
        match index.and_then(|i| w.buttons().get(i).copied()) {
            Some("Paste") | Some("OK") => {}
            Some("Paste Without Newline") => a.job.text = a.job.text.trim_end_matches(['\r', '\n']).to_string(),
            Some("Convert tabs to spaces") => {
                let n = self.globals.int("PasteTabToStringTabStopSize").clamp(0, 100) as usize;
                a.job.tabs = paste::Tabs::Spaces(n);
            }
            // Advanced...: the Advanced Paste window with this text; this paste stops.
            Some("Advanced...") => {
                self.advanced_paste_request = Some((pane, a.job.text));
                return;
            }
            // Cancel, or dropped.
            _ => return,
        }
        if a.warnings.is_empty() {
            self.paste_run(pane, a.job, a.speed, a.what);
        } else {
            self.paste_asks.insert(pane, a);
            self.paste_ask_next(pane);
        }
    }

    /// Edit > Paste Special's items (by label where selectors repeat). False for others.
    pub(super) fn paste_command(&mut self, selector: &str, label: &str) -> bool {
        match (selector, label) {
            ("paste:", "Paste Slowly") => {
                if let (Some(text), Some(id)) = (self.clipboard_text(), self.focused_id()) {
                    self.paste_text(id, &text, Write::Paste, true);
                }
            }
            ("pasteFaster:", _) => self.paste_speed_change(false, 1.5),
            ("pasteSlower:", _) => self.paste_speed_change(false, 0.66),
            ("pasteSlowlyFaster:", _) => self.paste_speed_change(true, 1.5),
            ("pasteSlowlySlower:", _) => self.paste_speed_change(true, 0.66),
            ("pasteBase64Encoded:", _) => self.paste_file_base64(),
            ("toggleMultiLinePasteWarning:", l) => {
                let key = paste_toggle_key(l);
                let on = !self.globals.bool(key);
                self.globals.set(key, toml::Value::Boolean(on));
                self.save_globals();
                self.menus_dirty = true;
            }
            // Off silences the tabs question with OK remembered; on unsilences it
            // (`togglePromptToConvertTabsToSpacesWhenPasting`, `iTermPasteHelper.m:165-171`).
            ("promptToConvertTabsToSpacesWhenPasting:", _) => {
                const TABS_ID: &str = "AboutToPasteTabsWithCancel";
                if self.globals.bool(TABS_ID) {
                    self.globals.unset(TABS_ID);
                } else {
                    self.globals.set(TABS_ID, toml::Value::Boolean(true));
                    self.globals.set(&warnings::selection_key(TABS_ID), toml::Value::Integer(0));
                }
                self.save_globals();
                self.menus_dirty = true;
            }
            _ => return false,
        }
        true
    }

    /// Whether a Paste Special toggle shows its check mark.
    pub(super) fn paste_checked(&self, label: &str) -> Option<bool> {
        match label {
            "Warn Before Multi-Line Paste" | "Limit Multi-Line Paste Warning to Shell Prompt" | "Warn Before Pasting One Line Ending in a Newline at Shell Prompt" => Some(!self.globals.bool(paste_toggle_key(label))),
            "Prompt to Convert Tabs to Spaces when Pasting" => Some(!self.globals.bool("AboutToPasteTabsWithCancel")),
            _ => None,
        }
    }

    /// Paste Faster, Slower, and their Slowly forms: the speed changes and says so.
    fn paste_speed_change(&mut self, slow: bool, factor: f64) {
        let s = paste::faster(self.paste_speed(slow), factor);
        let (units, delay) = if slow { ("SlowPasteBytesPerCall", "SlowPasteDelayBetweenCalls") } else { ("QuickPasteBytesPerCall", "QuickPasteDelayBetweenCalls") };
        self.globals.set(units, toml::Value::Integer(s.units as i64));
        self.globals.set(delay, toml::Value::Float(s.delay));
        self.save_globals();
        self.show_toast(&paste::rate_text(s));
    }

    /// Paste File Base64-Encoded: the first file on the clipboard, as base64 lines.
    fn paste_file_base64(&mut self) {
        let Some(id) = self.focused_id() else { return };
        let Some(path) = crate::winclip::first_file() else {
            return self.banners.set("paste", "The clipboard holds no file.".to_string());
        };
        match crate::localfs::judge_io(&path).and_then(|p| crate::localfs::read(&p)) {
            Ok(bytes) => self.user_paste(id, &paste::base64_lines(&bytes), Write::Paste),
            Err(e) => self.banners.set("paste", format!("Could not read {}: {e}", path.display())),
        }
    }

    pub(super) fn clipboard_text(&mut self) -> Option<String> {
        match self.clipboard.as_mut().map(|cb| cb.get_text()) {
            Some(Ok(t)) => Some(t),
            Some(Err(e)) => {
                self.banners.set("clipboard", format!("Paste failed: {e}"));
                None
            }
            None => None,
        }
    }

    /// Key actions 41 and 42, and the pointer paste actions with a configuration.
    pub(super) fn paste_with_config(&mut self, sender: PaneId, text: &str, config: &str) {
        let (job, mut speed) = paste::Config::decode(config).job(text);
        // A configuration without a speed pastes at the normal one.
        if speed.units == 0 {
            speed.units = paste::QUICK.units;
        }
        if speed.delay <= 0.0 {
            speed.delay = paste::QUICK.delay;
        }
        self.paste_job(sender, job, speed, Write::Paste);
    }
}

/// The setting each Paste Special warning toggle flips.
fn paste_toggle_key(label: &str) -> &'static str {
    match label {
        "Limit Multi-Line Paste Warning to Shell Prompt" => "PromptForPasteWhenNotAtPrompt",
        "Warn Before Pasting One Line Ending in a Newline at Shell Prompt" => "NoSyncDoNotWarnBeforePastingOneLineEndingInNewlineAtShellPrompt",
        _ => "NoSyncDoNotWarnBeforeMultilinePaste",
    }
}
