//! Engage Artificial Intelligence from the terminal and from both Composers (iTerm2's
//! `PTYSession.m:21095-21399`, `LC:306-343`, `SBC:46-51`): the dialog, the wait window, the
//! request through `ai_runs` and what each answer does. The rules themselves are `ai_engage`'s.

use std::collections::HashMap;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::ai_runs::{Finished, Owner};
use super::{App, PaneId};
use crate::ai_engage;
use crate::ai_engage_ui::{EngageResult, EngageWindow, WaitResult, WaitWindow};
use crate::ai_gate::{Alert, Then};
use crate::composer::Shown;

/// Where an Engage run was started, and so where its answer goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum EngageFrom {
    Terminal(PaneId),
    Composer(PaneId),
    /// The status bar field, with the query it held when asked (the field is erased after the
    /// answer only if it still holds it).
    StatusBar { pane: PaneId, query: String },
}

impl EngageFrom {
    fn pane(&self) -> PaneId {
        match self {
            EngageFrom::Terminal(p) | EngageFrom::Composer(p) => *p,
            EngageFrom::StatusBar { pane, .. } => *pane,
        }
    }
}

/// While the dialog or the wait window is up a second Engage only focuses that window, as
/// iTerm2's sheet blocks the window (minor M6).
pub(super) fn engage_busy(dialog_open: bool, waiting: bool) -> bool {
    dialog_open || waiting
}

/// Which input Engage reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Route {
    Terminal,
    Composer,
    StatusBar,
}

/// The large Composer when it is shown and has the keyboard, a status bar field when one has
/// it, else the terminal's selection or command. A shown Composer without the keyboard does
/// not own Win+Y.
pub(super) fn route(shown: Shown, composer_has_keys: bool, field_focused: bool) -> Route {
    if shown == Shown::Dropdown && composer_has_keys {
        Route::Composer
    } else if field_focused {
        Route::StatusBar
    } else {
        Route::Terminal
    }
}

/// Forgets a run's origin (a Cancel, or the answer arriving): a later answer for the same id
/// finds nothing and does nothing.
pub(super) fn pending_after_cancel(pending: &mut HashMap<u64, EngageFrom>, id: u64) -> Option<EngageFrom> {
    pending.remove(&id)
}

impl App {
    /// Edit > Engage Artificial Intelligence (Win+Y outside a Composer).
    pub(super) fn engage_ai(&mut self, _event_loop: &ActiveEventLoop) {
        if engage_busy(self.ai_engage.is_some() || self.ai_engage_open.is_some(), self.ai_wait.is_some() || !self.ai_engage_pending.is_empty()) {
            if let Some((w, _, _)) = &self.ai_engage {
                w.focus();
            } else if let Some((w, _)) = &self.ai_wait {
                w.focus();
            }
            return;
        }
        let Some(pane) = self.focused_id() else { return };
        if self.engage_route_of(pane) != Route::Terminal {
            return self.engage_from_composer(pane);
        }
        // The gate first, not silent: a closed one explains itself and ends it (`PS:21121-21124`).
        if !self.ai_check(false) {
            return;
        }
        let Some(p) = self.panes.get(&pane) else { return };
        let own = p.own();
        let query = ai_engage::query_source(own.selection_text().as_deref(), own.typed_command().as_deref());
        let ask = ai_engage::ask_for(&query, self.globals.int(crate::ai_settings::RESPONSE_TOKEN_LIMIT));
        if ai_engage::send_at_once(&ask, self.globals.bool(ai_engage::SKIP_KEY)) {
            self.engage_send(EngageFrom::Terminal(pane), &ask.prefill);
        } else {
            self.ai_engage_open = Some((pane, ask));
        }
    }

    /// Which input `pane`'s Engage reads now.
    fn engage_route_of(&self, pane: PaneId) -> Route {
        let Some(p) = self.panes.get(&pane) else { return Route::Terminal };
        let c = &p.composer;
        let field = c.field_focused || (c.shown == Shown::Field && c.focused);
        route(c.shown, self.composer_has_keys(), field)
    }

    /// The large Composer's AI button and Win+Y in it, and Win+Y in a status bar field: the
    /// query goes straight to the model with no dialog (`LC:306-334`). No gate runs up front
    /// (iTerm2 reaches it only through its registration, `RH:11-24`), but the request goes
    /// through `ai_run`, whose gate runs before any byte is sent (ruling R12e-1).
    pub(super) fn engage_from_composer(&mut self, pane: PaneId) {
        let Some(p) = self.panes.get(&pane) else { return };
        let c = &p.composer;
        match self.engage_route_of(pane) {
            Route::Composer => {
                let after = c.text().len() - c.contents().len();
                let selected = c.edit.selection().map(|r| r.start.max(after)..r.end.max(after)).filter(|r| r.start < r.end).map(|r| c.text()[r].to_string());
                let query = ai_engage::composer_query(&c.contents(), selected.as_deref());
                self.engage_send(EngageFrom::Composer(pane), &query);
            }
            Route::StatusBar => {
                let field = if c.field_focused { c.field.text.clone() } else { c.contents() };
                if !ai_engage::status_bar_query_enabled(self.globals.bool(crate::ai_settings::ADMIN), &field) {
                    return;
                }
                self.engage_send(EngageFrom::StatusBar { pane, query: field.clone() }, &field);
            }
            Route::Terminal => {}
        }
    }

    /// Sends the query: the `AI Prompt` template is evaluated in the session's scope with
    /// `ai.prompt` the trimmed query (a template that fails sends nothing and shows nothing,
    /// as iTerm2's nil result, `ATO:380-396`), then one request through `ai_run`.
    pub(super) fn engage_send(&mut self, from: EngageFrom, query: &str) {
        // An empty query is sent too: iTerm2 has no guard (`PS:21095-21104`, `PS:21324-21327`
        // returns only for nil, `ATO:380-394` evaluates the template with `ai.prompt` "").
        let trimmed = ai_engage::trimmed_query(query);
        let template = self.globals.string("AI Prompt");
        let Ok(prompt) = self.ai_prompt(from.pane(), &template, &[(ai_engage::PROMPT_VAR, trimmed.as_str())]) else { return };
        // The answer can come back inside the call that starts the run, so the origin is
        // registered under the id the run is about to get.
        let id = self.ai_next_run_id();
        self.ai_engage_pending.insert(id, from);
        if self.ai_one_shot(prompt, Owner::Engage).is_none() {
            pending_after_cancel(&mut self.ai_engage_pending, id);
        }
    }

    /// An Engage run finished: the wait window closes; a pane that closed meanwhile, or a run
    /// the person cancelled, makes it do nothing.
    pub(super) fn engage_done(&mut self, id: u64, result: Finished) {
        let Some(from) = pending_after_cancel(&mut self.ai_engage_pending, id) else { return };
        if self.ai_wait.as_ref().is_some_and(|(_, run)| *run == id) {
            self.ai_wait = None;
        }
        let pane = from.pane();
        if !self.panes.contains_key(&pane) {
            return;
        }
        match result {
            Finished::Canceled => {}
            Finished::Failed(text) => self.ask_ai(Alert { heading: ai_engage::ERROR_HEADING.into(), text, buttons: vec!["OK"], silence: None, then: Then::Nothing, gate: false, close: None }),
            Finished::Choice(choice) => match from {
                EngageFrom::Terminal(pane) => self.composer_ai_place(pane, &choice),
                EngageFrom::Composer(pane) => self.composer_ai_replace(pane, &ai_engage::accept_text(&choice)),
                EngageFrom::StatusBar { pane, query } => {
                    if let Some(p) = self.panes.get_mut(&pane) {
                        let c = &mut p.composer;
                        if c.shown == Shown::Field {
                            // The temporary field's text is the Composer's own: it ends, and the
                            // answer opens the large Composer.
                            let text = c.contents();
                            c.field_ended(&text);
                        } else if ai_engage::erase_status_bar(&c.field.text, &query) {
                            c.field.load("");
                        }
                    }
                    self.composer_ai_place(pane, &choice);
                }
            },
        }
        self.redraw();
    }

    /// A pane is closing: its Engage requests and windows end with it.
    pub(super) fn engage_pane_gone(&mut self, pane: PaneId) {
        let ids: Vec<u64> = self.ai_engage_pending.iter().filter(|(_, f)| f.pane() == pane).map(|(id, _)| *id).collect();
        for id in ids {
            pending_after_cancel(&mut self.ai_engage_pending, id);
            self.ai_run_cancel(id);
            if self.ai_wait.as_ref().is_some_and(|(_, run)| *run == id) {
                self.ai_wait = None;
            }
        }
        if self.ai_engage.as_ref().is_some_and(|(_, p, _)| *p == pane) {
            self.ai_engage = None;
        }
        if self.ai_engage_open.as_ref().is_some_and(|(p, _)| *p == pane) {
            self.ai_engage_open = None;
        }
    }

    /// Opens the dialog queued by `engage_ai`, and the wait window of a run that is going.
    pub(super) fn open_ai_engage_windows(&mut self, event_loop: &ActiveEventLoop) {
        let Some(g) = self.gpu.as_ref() else { return };
        if self.ai_engage.is_none() {
            if let Some((pane, ask)) = self.ai_engage_open.take() {
                match EngageWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, &ask) {
                    Ok(w) => self.ai_engage = Some((w, pane, ask)),
                    Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
                }
            }
        }
        if self.ai_wait.is_none() {
            let live = self.ai_engage_pending.keys().copied().filter(|id| self.ai_runs.contains_key(id)).min();
            if let Some(id) = live {
                match WaitWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, ai_engage::THINKING) {
                    Ok(w) => self.ai_wait = Some((w, id)),
                    Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
                }
            }
        }
    }

    /// Events for the Engage dialog and the wait window. False for other windows.
    pub(super) fn ai_engage_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        if self.ai_engage.as_ref().is_some_and(|(w, _, _)| w.id() == window) {
            let Some((w, _, _)) = self.ai_engage.as_mut() else { return true };
            let answer = match event {
                WindowEvent::CloseRequested => EngageResult::Cancel,
                WindowEvent::RedrawRequested => w.redraw(),
                other => {
                    w.event(other);
                    EngageResult::Open
                }
            };
            if answer == EngageResult::Open {
                return true;
            }
            let Some((_, pane, _)) = self.ai_engage.take() else { return true };
            if let EngageResult::Send { text, skip_next } = answer {
                if skip_next {
                    self.globals.set(ai_engage::SKIP_KEY, toml::Value::Boolean(true));
                    self.save_globals();
                }
                self.engage_send(EngageFrom::Terminal(pane), &text);
            }
            return true;
        }
        if self.ai_wait.as_ref().is_some_and(|(w, _)| w.id() == window) {
            let Some((w, _)) = self.ai_wait.as_mut() else { return true };
            let answer = match event {
                WindowEvent::CloseRequested => WaitResult::Cancel,
                WindowEvent::RedrawRequested => w.redraw(),
                other => {
                    w.event(other);
                    WaitResult::Open
                }
            };
            if answer == WaitResult::Cancel {
                if let Some((_, id)) = self.ai_wait.take() {
                    pending_after_cancel(&mut self.ai_engage_pending, id);
                    self.ai_run_cancel(id);
                }
            }
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composer::Shown;

    #[test]
    fn empty_query_is_sent_as_iterm2() {
        // The template evaluates with an empty `ai.prompt`, and the send has no emptiness guard.
        assert_eq!(ai_engage::trimmed_query(" \n "), "");
        let lookup = |_: &str| None;
        assert_eq!(crate::ai_prompts::evaluate("[\\(ai.prompt)]", &[(ai_engage::PROMPT_VAR, "")], &lookup), Ok("[]".to_string()));
        let production = include_str!("ai_engage_app.rs").split("\n#[cfg(test)]\nmod tests").next().unwrap();
        let send = production.split("fn engage_send").nth(1).unwrap().split("\n    }\n").next().unwrap();
        assert!(!send.contains("is_empty()"), "engage_send must not skip an empty query");
    }

    #[test]
    fn engage_route_follows_iterm2() {
        assert_eq!(route(Shown::Dropdown, true, false), Route::Composer);
        assert_eq!(route(Shown::Field, false, true), Route::StatusBar);
        assert_eq!(route(Shown::Hidden, false, false), Route::Terminal);
        assert_eq!(route(Shown::Dropdown, false, false), Route::Terminal, "a shown Composer without the keyboard does not own Win+Y");
    }

    #[test]
    fn late_answer_after_cancel_is_dropped() {
        let mut pending = HashMap::new();
        pending.insert(7, EngageFrom::Terminal(3));
        pending.insert(8, EngageFrom::StatusBar { pane: 3, query: "q".into() });
        assert_eq!(pending_after_cancel(&mut pending, 7), Some(EngageFrom::Terminal(3)));
        assert_eq!(pending.get(&7), None, "a later answer for it finds nothing");
        assert_eq!(pending_after_cancel(&mut pending, 7), None);
        assert!(pending.contains_key(&8), "other runs stay");
        assert!(!engage_busy(false, false));
        assert!(engage_busy(true, false));
        assert!(engage_busy(false, true));
    }
}
