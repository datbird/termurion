//! Paste history (every copy and paste) and the two windows that use it: Session > Open
//! Paste History... and Edit > Paste Special > Advanced Paste....

use super::*;
use crate::advanced_paste::{Defaults, Form, Session};
use crate::advanced_paste_ui::{AdvEvent, AdvancedPasteWindow};
use crate::paste_history::{self, History};
use crate::paste_history_ui::{HistEvent, PasteHistoryWindow};

fn history_path() -> std::path::PathBuf {
    std::env::var_os("APPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Tandem").join("pbhistory.json")
}

fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// Text shortened for a list: newlines as a return arrow, cut with an ellipsis.
fn ellipsized(text: &str, max: usize) -> String {
    let flat: String = text.chars().map(|c| if c == '\n' || c == '\r' { '\u{21a9}' } else { c }).collect();
    if flat.chars().count() > max { format!("{}\u{2026}", flat.chars().take(max).collect::<String>()) } else { flat }
}

impl App {
    /// The history, read from disk the first time (iTerm2 reads it whether or not saving
    /// is on).
    fn history(&mut self) -> &mut History {
        let max = self.globals.int("PasteHistoryMaxOptions").max(0) as usize;
        let h = self.paste_history.get_or_insert_with(|| match std::fs::read_to_string(history_path()) {
            Ok(t) => History::from_json(&t, max),
            Err(_) => History { max, ..History::default() },
        });
        h.max = max;
        h
    }

    /// A copy or a paste goes into the history (saved to disk with "Save copy/paste and
    /// command history to disk").
    pub(super) fn remember_paste(&mut self, text: &str) {
        if !self.history().save(text, unix_now()) || !self.globals.bool("SavePasteHistory") {
            return;
        }
        let json = self.history().to_json();
        let path = history_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&path, json) {
            self.banners.set("history", format!("Could not save the paste history: {e}"));
        }
    }

    /// Session > Open Paste History....
    pub(super) fn open_paste_history(&mut self, event_loop: &ActiveEventLoop) {
        let Some(pane) = self.focused_id() else { return };
        let now = unix_now();
        let rows: Vec<(String, String)> = self.history().newest_first().into_iter().map(|e| (e.value.clone(), paste_history::time_text(&e.value, e.time, now))).collect();
        if let Some((w, p)) = self.paste_history_win.as_mut() {
            *p = pane;
            w.set_rows(rows);
            return w.focus();
        }
        let Some(g) = self.gpu.as_ref() else { return };
        match PasteHistoryWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, rows) {
            Ok(w) => self.paste_history_win = Some((w, pane)),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    pub(super) fn paste_history_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some((w, pane)) = self.paste_history_win.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        let pane = *pane;
        match event {
            WindowEvent::CloseRequested => self.paste_history_win = None,
            WindowEvent::RedrawRequested => match w.redraw() {
                HistEvent::Pick(i) => {
                    let value = self.history().newest_first().get(i).map(|e| e.value.clone());
                    self.paste_history_win = None;
                    // iTerm2 puts it on the clipboard, then pastes (with the usual questions).
                    if let Some(v) = value {
                        if let Some(cb) = &mut self.clipboard {
                            let _ = cb.set_text(v.clone());
                        }
                        self.user_paste(pane, &v, crate::broadcast::Write::Paste);
                    }
                }
                HistEvent::Close => self.paste_history_win = None,
                HistEvent::None => {}
            },
            other => {
                w.event(other);
            }
        }
        true
    }

    fn paste_defaults(&self) -> Defaults {
        let g = &self.globals;
        let f = |k: &str| g.get(k).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(0.0);
        Defaults {
            chunk: g.int("PasteSpecialChunkSize").max(1) as usize,
            delay: f("PasteSpecialChunkDelay").max(0.001),
            spaces: g.int("NumberOfSpacesPerTab").clamp(0, 100) as usize,
            tab_transform: g.int("TabTransform"),
            escape: g.bool("EscapeShellCharsWithBackslash"),
            punctuation: g.bool("ConvertUnicodePunctuation"),
            dos_newlines: g.bool("ConvertDosNewlines"),
            remove_controls: g.bool("RemoveControlCodes"),
            bracket: g.bool("BracketedPasteMode"),
            use_regex: g.bool("PasteSpecialUseRegexSubstitution"),
            regex: g.string("PasteSpecialRegex"),
            substitution: g.string("PasteSpecialSubstitution"),
        }
    }

    fn save_paste_defaults(&mut self, d: &Defaults, speed: bool) {
        use toml::Value;
        let g = &mut self.globals;
        if speed {
            g.set("PasteSpecialChunkSize", Value::Integer(d.chunk as i64));
            g.set("PasteSpecialChunkDelay", Value::Float(d.delay));
        }
        g.set("NumberOfSpacesPerTab", Value::Integer(d.spaces as i64));
        g.set("TabTransform", Value::Integer(d.tab_transform));
        g.set("EscapeShellCharsWithBackslash", Value::Boolean(d.escape));
        g.set("ConvertUnicodePunctuation", Value::Boolean(d.punctuation));
        g.set("ConvertDosNewlines", Value::Boolean(d.dos_newlines));
        g.set("RemoveControlCodes", Value::Boolean(d.remove_controls));
        g.set("BracketedPasteMode", Value::Boolean(d.bracket));
        g.set("PasteSpecialUseRegexSubstitution", Value::Boolean(d.use_regex));
        g.set("PasteSpecialRegex", Value::String(d.regex.clone()));
        g.set("PasteSpecialSubstitution", Value::String(d.substitution.clone()));
        self.save_globals();
    }

    /// Advanced Paste: from the menu (`text` None: the clipboard, with the history), or
    /// from a paste question's "Advanced..." (that paste's text; the speed is not saved).
    pub(super) fn open_advanced_paste(&mut self, event_loop: &ActiveEventLoop, pane: PaneId, text: Option<String>) {
        let clip = self.clipboard.as_mut().and_then(|cb| cb.get_text().ok()).unwrap_or_default();
        let mut sources: Vec<(String, String)> = Vec::new();
        let from_menu = text.is_none();
        if from_menu && self.globals.bool("IncludePasteHistoryInAdvancedPaste") && self.history().entries.len() > 1 {
            let entries: Vec<String> = self.history().entries.iter().map(|e| e.value.clone()).collect();
            let n = entries.len();
            for (i, v) in entries.iter().enumerate() {
                // Consecutive duplicates once; the newest when it is the clipboard's text.
                if (i > 0 && entries[i - 1] == *v) || (i + 1 == n && *v == clip.trim()) {
                    continue;
                }
                sources.push((format!("Text: \"{}\"", ellipsized(v, 50)), v.clone()));
            }
        }
        let body = text.unwrap_or(clip);
        // The clipboard's (or the asked paste's) text, as iTerm2 names a plain-text item.
        sources.push((format!("Plain Text: \"{}\"", ellipsized(&body, 100)), body.clone()));
        let source = sources.len() - 1;
        let p = self.panes.get(&pane);
        let at = p.and_then(|p| p.own().at_prompt());
        let session = Session { bracketed: self.paste_bracketed_pub(pane), shell_integration: at.is_some(), at_prompt: at == Some(true), wait_default: self.globals.bool("AdvancedPasteWaitsForPromptByDefault") };
        let mut d = self.paste_defaults();
        if !from_menu {
            let s = crate::paste::QUICK;
            d.chunk = s.units;
            d.delay = s.delay;
        }
        let form = Form::new(&body, &d, session);
        if let Some((w, _, _)) = &self.advanced_paste_win {
            w.focus();
            return;
        }
        let Some(g) = self.gpu.as_ref() else { return };
        match AdvancedPasteWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, sources, source, form, session) {
            Ok(w) => self.advanced_paste_win = Some((w, pane, from_menu)),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    pub(super) fn advanced_paste_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some((w, pane, from_menu)) = self.advanced_paste_win.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        let (pane, from_menu) = (*pane, *from_menu);
        match event {
            WindowEvent::CloseRequested => self.advanced_paste_win = None,
            WindowEvent::RedrawRequested => match w.redraw() {
                AdvEvent::Paste => {
                    let (form, preview, text, session) = (w.form.clone(), w.preview.clone(), w.text(), w.session);
                    self.advanced_paste_win = None;
                    let mut d = self.paste_defaults();
                    form.remember(&form.enabled(&text, session), &mut d);
                    self.save_paste_defaults(&d, from_menu);
                    let (mut job, speed) = form.job(&preview);
                    // iTerm2 skips the multi-line question for an Advanced Paste.
                    job.flags |= crate::paste::NO_WARNINGS;
                    self.remember_paste(&preview);
                    self.paste_job(pane, job, speed, crate::broadcast::Write::Paste);
                }
                AdvEvent::Cancel => self.advanced_paste_win = None,
                AdvEvent::None => {}
            },
            other => {
                w.event(other);
            }
        }
        true
    }

    /// A paste question's "Advanced..." asked for the window (opened on the next pass).
    pub(super) fn open_requested_advanced_paste(&mut self, event_loop: &ActiveEventLoop) {
        if let Some((pane, text)) = self.advanced_paste_request.take() {
            self.open_advanced_paste(event_loop, pane, Some(text));
        }
    }
}
