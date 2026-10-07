//! The Claude Code integration in the app (Tandem > Install, Reinstall and Uninstall Claude
//! Code Integration): the setup window and its steps, the hook install and removal, the four
//! questions (every one closes to its safe answer, P10), the health monitor that runs once
//! per launch (`HM`) and the announcement bar at three Claude sessions (`CWT`, `NAG`). The
//! person's `settings.json` is read, merged and replaced by `claude_code`'s pure functions;
//! nothing here writes it any other way.

use super::scripts_alerts::{ask, Ask, Then};
use super::*;
use crate::cc_onboarding_ui::{OnboardAct, OnboardingWindow};
use crate::claude_code::{self as cc, Step};
use crate::toolbelt::ToolId;
use crate::warnings::Kind;
use std::path::Path;

/// The settings.json question for the alerts.
fn failed_ask(text: &str) -> Ask {
    ask(cc::HOOK_FAILED, text, &[crate::api_auth::OK], None, Kind::Persistent, Then::Nothing)
}

/// "Uninstall Claude Code Integration?": a closed window cancels.
fn uninstall_ask(path: &Path) -> Ask {
    ask(cc::UNINSTALL_HEADING, &cc::uninstall_text(path), &[cc::UNINSTALL, crate::api_auth::CANCEL], None, Kind::Persistent, Then::ClaudeUninstall)
}

/// "Couldn't Remove Hooks": a closed window cancels.
fn remove_failed_ask(detail: &str) -> Ask {
    ask(cc::REMOVE_FAILED_HEADING, &cc::remove_failed(detail), &[cc::CONTINUE_BUTTON, crate::api_auth::CANCEL], None, Kind::Persistent, Then::ClaudeHooksFailed)
}

/// "Disable the Python API?": a closed window leaves it enabled.
fn disable_api_ask() -> Ask {
    ask(cc::DISABLE_API_HEADING, cc::DISABLE_API_TEXT, &[cc::LEAVE_ENABLED, cc::DISABLE], None, Kind::Persistent, Then::ClaudeDisableApi)
}

/// "Claude Code Integration Looks Broken": silenceable for good under `REPAIR_ID`, a closed
/// window is Not Now, and Reinstall is never remembered.
fn repair_ask(path: &Path) -> Ask {
    ask(cc::BROKEN_HEADING, &cc::broken_text(path), &[cc::REINSTALL, cc::NOT_NOW], Some(cc::REPAIR_ID), Kind::Permanent, Then::ClaudeRepair)
}

/// "Remove the Claude Code workgroup?" when the person's own copy differs from the installer's:
/// a closed window keeps it (P8).
fn edited_workgroup_ask() -> Ask {
    ask(
        "Remove the Claude Code Workgroup?",
        "The \u{201c}Claude Code\u{201d} workgroup in your settings is not the one the installer adds: you or an import changed it. Keep it, or remove it too?",
        &["Keep It", "Remove"],
        None,
        Kind::Persistent,
        Then::ClaudeEditedWorkgroup,
    )
}

/// "Enable Session Cookies?": a closed window cancels.
fn cookie_ask() -> Ask {
    ask(cc::COOKIE_HEADING, cc::COOKIE_TEXT, &cc::COOKIE_BUTTONS, None, Kind::Persistent, Then::ClaudeCookie)
}

impl App {
    /// The cookie setting and whether Tandem turned it on.
    fn claude_cookie(&self) -> cc::CookieState {
        cc::CookieState { on: self.globals.bool(super::api_ui::SET_COOKIE_KEY), by_tandem: self.globals.bool(cc::COOKIE_BY_KEY) }
    }

    fn claude_set_cookie(&mut self, state: cc::CookieState) {
        self.globals.set(super::api_ui::SET_COOKIE_KEY, toml::Value::Boolean(state.on));
        self.globals.set(cc::COOKIE_BY_KEY, toml::Value::Boolean(state.by_tandem));
        self.save_globals();
    }

    /// Turns the cookie back off when Tandem is the one who turned it on (a person's own
    /// setting is never touched): the Disable answer, and a setup closed before the hook is in.
    fn claude_revert_cookie(&mut self) {
        let now = self.claude_cookie();
        if now.reverted() != now {
            self.claude_set_cookie(now.reverted());
        }
    }

    /// Claude Code's settings.json: `CLAUDE_CONFIG_DIR` from Tandem's own environment, else the
    /// profile folder's `.claude`. (Returns the folder and the file.)
    fn claude_paths(&self) -> Result<(PathBuf, PathBuf), String> {
        let profile = std::env::var_os("USERPROFILE").map(PathBuf::from).filter(|p| !p.as_os_str().is_empty()).ok_or_else(|| cc::NO_PROFILE.to_string())?;
        let dir = cc::config_dir(std::env::var("CLAUDE_CONFIG_DIR").ok().as_deref(), &profile);
        let file = cc::settings_path(&dir);
        Ok((dir, file))
    }

    /// The integration is installed as far as the menu knows.
    fn claude_known(&self) -> bool {
        cc::known(self.globals.bool(cc::HOOKS_KEY), self.claude_workgroup_installed(), self.globals.bool(cc::TRIGGERS_INSTALLED_KEY), self.globals.bool(cc::COMPLETED_KEY))
    }

    /// Enables Install, Reinstall and Uninstall by `known` in every window's menu bar.
    pub(super) fn refresh_claude_menu(&mut self) {
        let enabled = cc::menu_enabled(self.claude_known());
        for bar in self.menu_bars_mut() {
            if let Some(items) = &bar.claude_items {
                for (item, on) in items.iter().zip(enabled) {
                    item.set_enabled(on);
                }
            }
        }
    }

    /// Install and Reinstall: opens the setup window, or brings the open one forward.
    pub(super) fn install_claude_code(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.cc_onboarding {
            w.focus();
            return;
        }
        if self.gpu.is_none() {
            return;
        }
        // The cached triggers flag is read again from the profiles first: they may have changed since launch.
        self.reconcile_triggers_cache();
        let Some(g) = self.gpu.as_ref() else { return };
        let shown = match self.claude_paths() {
            Ok((_, file)) => file,
            Err(e) => return self.ask_scripts(failed_ask(&e)),
        };
        let steps = cc::steps(self.globals.bool(super::api_ui::ENABLE_KEY), self.globals.bool(super::api_ui::SET_COOKIE_KEY));
        let dark = self.dark();
        match OnboardingWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, steps, &shown, dark) {
            Ok(mut w) => {
                w.focus();
                self.claude_sync_steps(&mut w);
                self.cc_onboarding = Some(w);
            }
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Marks the steps whose work is already done (the API on, the hook in, the Session Status
    /// tool showing), so a Reinstall shows them with their check mark.
    fn claude_sync_steps(&self, w: &mut OnboardingWindow) {
        let api = self.globals.bool(super::api_ui::ENABLE_KEY);
        let cookie = self.globals.bool(super::api_ui::SET_COOKIE_KEY);
        let hook = self.globals.bool(cc::HOOKS_KEY);
        let belt = self.belt.visible && self.toolbelt_tools().contains(&ToolId::SessionStatus);
        for (step, done) in [(Step::EnablePythonApi, api), (Step::EnableCookie, cookie), (Step::InstallHook, hook), (Step::ShowToolbelt, belt)] {
            if done {
                w.mark_done(step);
            }
        }
        for step in cc::pre_completed(self.claude_workgroup_installed(), self.globals.bool(cc::TRIGGERS_INSTALLED_KEY)) {
            w.mark_done(step);
        }
    }

    /// Each pass: a setup an answer asked for opens, and the open setup window shows what has
    /// become done (the Enable alert answered, say).
    pub(super) fn claude_tick(&mut self, event_loop: &ActiveEventLoop) {
        if std::mem::take(&mut self.claude_install_due) {
            self.install_claude_code(event_loop);
        }
        if let Some(mut w) = self.cc_onboarding.take() {
            self.claude_sync_steps(&mut w);
            self.cc_onboarding = Some(w);
        }
    }

    /// Events for the setup window. False when `window` is not it.
    pub(super) fn cc_onboarding_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some(w) = self.cc_onboarding.as_mut().filter(|w| w.id() == window) else { return false };
        let acts = match event {
            WindowEvent::CloseRequested => vec![OnboardAct::Closed],
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                Vec::new()
            }
        };
        for act in acts {
            match act {
                OnboardAct::DoIt(step) => self.claude_do_step(step),
                OnboardAct::Picked(guids) => self.claude_install_triggers_picked(&guids),
                OnboardAct::Learn => {
                    if let Err(e) = super::semantic_ui::shell_open(cc::HELP_URL, false) {
                        self.banners.set("action", format!("Could not open {}: {e}", cc::HELP_URL));
                    }
                }
                OnboardAct::Closed => {
                    self.cc_onboarding = None;
                    // A setup left before the hook went in leaves nothing Tandem turned on.
                    if !self.globals.bool(cc::HOOKS_KEY) {
                        self.claude_revert_cookie();
                    }
                    break;
                }
            }
        }
        true
    }

    /// The current step's button.
    pub(super) fn claude_do_step(&mut self, step: Step) {
        match step {
            Step::EnablePythonApi => {
                // The forced "Enable Python API?" alert, only while the API is off (iTerm2 returns
                // early when it is on); the step is done once the API reads on.
                if !self.globals.bool(super::api_ui::ENABLE_KEY) {
                    self.queue_api_alert(super::api_ui::Alert::Enable);
                }
            }
            // The cookie has its own question; only its Enable answer turns it on.
            Step::EnableCookie => self.ask_scripts(cookie_ask()),
            Step::InstallHook => match self.install_hook() {
                Ok(_) => {
                    if let Some(w) = self.cc_onboarding.as_mut() {
                        w.mark_done(Step::InstallHook);
                    }
                }
                Err(e) => self.ask_scripts(failed_ask(&e)),
            },
            Step::InstallWorkgroup => match self.install_workgroup() {
                Ok(()) => {
                    if let Some(w) = self.cc_onboarding.as_mut() {
                        w.mark_done(Step::InstallWorkgroup);
                    }
                }
                Err(e) => self.ask_scripts(failed_ask(&e)),
            },
            // The profile picker opens in the setup window; its Install button does the work.
            Step::InstallTriggers => {
                let rows = self.claude_trigger_profiles();
                if rows.is_empty() {
                    self.ask_scripts(failed_ask("No profile can take the triggers: a dynamic profile has to be marked rewritable first."));
                } else if let Some(w) = self.cc_onboarding.as_mut() {
                    w.pick_profiles(rows);
                }
            }
            Step::ShowToolbelt => {
                // Shown when hidden, the Session Status tool added when absent; never toggled off.
                if !self.belt.visible {
                    self.set_toolbelt_visible(true, true);
                }
                if !self.toolbelt_tools().contains(&ToolId::SessionStatus) {
                    self.toggle_toolbelt_tool(ToolId::SessionStatus);
                }
                if let Some(w) = self.cc_onboarding.as_mut() {
                    w.mark_done(Step::ShowToolbelt);
                }
            }
        }
    }

    /// Writes the hook into Claude Code's settings.json; the file the hook went into. A failure
    /// is its one text and leaves the file as it was.
    pub(super) fn install_hook(&mut self) -> Result<PathBuf, String> {
        let (dir, file) = self.claude_paths()?;
        let exe = std::env::current_exe().map_err(|e| cc::read_failed(Path::new(cc::EXE), e))?.with_file_name(cc::EXE);
        if !exe.is_file() {
            return Err(cc::missing_exe(&exe));
        }
        cc::install_to(&file, &cc::hook_command(&exe))?;
        // The folder it went into before, when it differs (best effort: the new hook is in).
        // (Never the file just written: `old_folder_settings` compares folders by final path.)
        if let Some(old) = cc::old_folder_settings(&self.globals.string(cc::DIR_KEY), &file).filter(|old| !cc::same_path(old, &file)) {
            let _ = cc::uninstall_from(&old);
        }
        self.globals.set(cc::DIR_KEY, toml::Value::String(dir.to_string_lossy().into_owned()));
        self.globals.set(cc::HOOKS_KEY, toml::Value::Boolean(true));
        self.globals.set(cc::COMPLETED_KEY, toml::Value::Boolean(true));
        self.save_globals();
        self.refresh_claude_menu();
        Ok(file)
    }

    /// Uninstall: the confirm question first.
    pub(super) fn uninstall_claude_code(&mut self) {
        match self.claude_paths() {
            Ok((_, file)) => self.ask_scripts(uninstall_ask(&file)),
            Err(e) => self.ask_scripts(failed_ask(&e)),
        }
    }

    /// "Uninstall" pressed: strips the hook (a missing file is success); a failure asks whether
    /// to go on.
    pub(super) fn claude_uninstall_confirmed(&mut self) {
        let (_, file) = match self.claude_paths() {
            Ok(p) => p,
            Err(e) => return self.ask_scripts(failed_ask(&e)),
        };
        match cc::uninstall_from(&file) {
            Ok(()) => {
                // The folder it was installed in before a change of CLAUDE_CONFIG_DIR, best effort.
                if let Some(old) = cc::old_folder_settings(&self.globals.string(cc::DIR_KEY), &file) {
                    let _ = cc::uninstall_from(&old);
                }
                self.claude_uninstalled();
            }
            Err(e) => self.ask_scripts(remove_failed_ask(&e.text(&file))),
        }
    }

    /// The hook is gone (or the person went on anyway): the flags clear, the menu follows, and
    /// with the API on it is offered off.
    pub(super) fn claude_uninstalled(&mut self) {
        // The workgroup and the triggers go with the hook (`CCM:finishUninstall`).
        self.uninstall_workgroup();
        self.uninstall_triggers();
        self.globals.set(cc::COMPLETED_KEY, toml::Value::Boolean(false));
        self.globals.set(cc::HOOKS_KEY, toml::Value::Boolean(false));
        self.save_globals();
        self.refresh_claude_menu();
        if self.globals.bool(super::api_ui::ENABLE_KEY) {
            self.ask_scripts(disable_api_ask());
        }
    }

    /// "Disable" pressed: the API goes off and its server stops.
    pub(super) fn claude_disable_api(&mut self) {
        self.set_api_enabled(false);
        self.sync_api_server();
        // The cookie Tandem turned on for the hook goes with the API; the person's own stays.
        self.claude_revert_cookie();
    }

    /// Whether another question shows or waits (the health monitor never stacks on one).
    fn claude_alert_busy(&self) -> bool {
        self.scripts_alert.is_some() || !self.scripts_alerts.is_empty() || !self.api_alerts.is_empty() || self.script_alert.is_some() || self.ai_alert.is_some() || !self.ai_asks.is_empty()
    }

    /// The jobs changed: the health monitor and the announcement look at the panes running
    /// Claude. Only a local foreground job counts: the local hook cannot report a remote
    /// claude (P13).
    pub(super) fn claude_jobs_changed(&mut self) {
        let claude: Vec<PaneId> = self
            .panes
            .iter()
            .filter(|(_, p)| p.exited.is_none() && p.conductor.is_none() && p.remote_job().is_none() && p.tmux_io().is_none())
            .filter(|(_, p)| crate::session_state::kind(&p.own().shell.job_ancestors, p.own().alt_screen()) == crate::session_state::Kind::ClaudeCode)
            .map(|(id, _)| *id)
            .collect();
        if claude.is_empty() {
            return;
        }
        // The health monitor: once per launch, on the first Claude pane, while the integration
        // is recorded as installed and no other question is up (it waits for the next change).
        if !self.claude_health_done && self.globals.bool(cc::COMPLETED_KEY) && !self.claude_alert_busy() {
            self.claude_health_done = true;
            if let Ok((_, file)) = self.claude_paths() {
                let text = cc::read_settings(crate::localfs::judge_io(&file).and_then(|f| crate::localfs::read(&f)), &file).ok().flatten();
                if !cc::healthy(text.as_deref(), &cc::runnable_path) {
                    self.ask_scripts(repair_ask(&file));
                }
            }
        }
        // The announcement: three or more panes run Claude, and nothing says the person knows
        // the integration.
        let last_use = self.globals.number(crate::status_tool::LAST_USE_KEY).unwrap_or(0.0);
        let visible = self.belt.visible && self.toolbelt_tools().contains(&ToolId::SessionStatus);
        if claude.len() >= cc::WATCHER_THRESHOLD && cc::watcher_enabled(self.globals.bool(cc::WATCHER_OFF_KEY), last_use, visible) && !self.claude_watcher_off {
            let live: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.exited.is_none()).map(|(id, _)| *id).collect();
            for pane in live {
                if !self.bars.get(&pane).is_some_and(|b| b.has(cc::WATCHER_BAR)) {
                    self.ask(pane, cc::WATCHER_BAR, cc::WATCHER_TEXT, &cc::WATCHER_BUTTONS, false);
                }
            }
        }
    }

    /// The announcement bar's answer. False for other bars.
    pub(super) fn claude_watcher_bar(&mut self, _pane: PaneId, id: &str, button: Option<&str>) -> bool {
        if id != cc::WATCHER_BAR {
            return false;
        }
        let [yes, never, _later] = cc::WATCHER_BUTTONS;
        match button {
            Some(b) if b == yes || b == never => {
                self.globals.set(cc::WATCHER_OFF_KEY, toml::Value::Boolean(true));
                self.save_globals();
                // Yes opens the setup on the next pass (the bar has no event loop).
                self.claude_install_due |= b == yes;
            }
            // Ask Later, or the bar closed: quiet until the next launch.
            _ => self.claude_watcher_off = true,
        }
        // Any answer takes the bar from every pane.
        for bars in self.bars.values_mut() {
            bars.retain(|b| b.id != cc::WATCHER_BAR);
        }
        self.redraw();
        true
    }

    /// An answer to one of the four questions.
    pub(super) fn claude_answered(&mut self, then: &Then, index: usize) {
        match (then, index) {
            (Then::ClaudeUninstall, 0) => self.claude_uninstall_confirmed(),
            (Then::ClaudeHooksFailed, 0) => self.claude_uninstalled(),
            // Remove: the person's edited copy goes too, on their answer.
            (Then::ClaudeEditedWorkgroup, 1) => self.remove_claude_workgroup(),
            (Then::ClaudeDisableApi, 1) => self.claude_disable_api(),
            (Then::ClaudeRepair, 0) => self.claude_install_due = true,
            // Enable: the one place the cookie is turned on, and only by the person's answer.
            (Then::ClaudeCookie, 0) => {
                let next = self.claude_cookie().enabled();
                self.claude_set_cookie(next);
            }
            _ => {}
        }
    }
}

impl App {
    /// The workgroup the installer adds is in Tandem's workgroups (`workgroupAlreadyInstalled`).
    fn claude_workgroup_installed(&self) -> bool {
        self.workgroup_model.get(crate::workgroup::claude_code::WORKGROUP).is_some()
    }

    /// Install Workgroup (`installWorkgroupIfNeeded`): adds the Claude Code workgroup unless one with
    /// its id is there. A copy the person edited, or an import's, is never replaced. Nothing is added
    /// over a saved list Tandem could not read (the add would replace it).
    pub(super) fn install_workgroup(&mut self) -> Result<(), String> {
        if self.claude_workgroup_installed() {
            return Ok(());
        }
        if self.workgroup_model.is_undecodable() {
            return Err("Your saved workgroups could not be read, so the Claude Code workgroup was not added. Fix them in Settings > Arrangements > Workgroups first.".into());
        }
        if self.workgroup_model.workgroups.len() >= crate::workgroup::MAX_WORKGROUPS {
            return Err("There are too many workgroups to add another. Remove one in Settings > Arrangements > Workgroups first.".into());
        }
        // Tandem's own constant, added by the person's click: theirs, as the menu's New Workgroup is.
        self.workgroup_model.add(crate::workgroup::claude_code::config());
        self.workgroup_model.save(&mut self.globals);
        if !self.claude_workgroup_installed() {
            return Err("The Claude Code workgroup could not be stored.".into());
        }
        self.save_globals();
        self.menus_dirty = true;
        self.refresh_claude_menu();
        Ok(())
    }

    /// Uninstall's workgroup step (`uninstallWorkgroup`): the installer's own copy goes; one the person
    /// or an import changed is asked about, and a closed question keeps it (P8).
    pub(super) fn uninstall_workgroup(&mut self) {
        let Some(w) = self.workgroup_model.get(crate::workgroup::claude_code::WORKGROUP) else { return };
        if *w == crate::workgroup::claude_code::config() {
            self.remove_claude_workgroup();
        } else {
            self.ask_scripts(edited_workgroup_ask());
        }
    }

    pub(super) fn remove_claude_workgroup(&mut self) {
        self.workgroup_model.remove(crate::workgroup::claude_code::WORKGROUP);
        self.workgroup_model.save(&mut self.globals);
        self.save_globals();
        self.menus_dirty = true;
        self.refresh_claude_menu();
    }

    /// The profiles the installer may write triggers into: saved ones, and dynamic ones only when
    /// rewritable (their file is rewritten through the one write-back). Bonjour hosts and browser
    /// profiles never run triggers, so they are not offered.
    fn claude_trigger_profiles(&self) -> Vec<crate::cc_onboarding_ui::PickRow> {
        self.store
            .profiles
            .iter()
            .filter(|p| cc::eligible(p) && !self.store.transient.contains(&p.guid()) && !crate::browser::is_browser(&p.string("Custom Command")))
            .map(|p| crate::cc_onboarding_ui::PickRow { guid: p.guid(), name: crate::profile_list::display_text(&p.name()), dynamic: p.bool(crate::dynamic_profiles::DYNAMIC_KEY) })
            .collect()
    }

    /// The picker's Install: both triggers go into each chosen profile after the older copies are
    /// stripped (`addClaudeWorkgroupTriggers`), through `Profile::set` (the one write point) and, for a
    /// dynamic profile, the write-back queue. The step is done once every chosen profile holds them.
    fn claude_install_triggers_picked(&mut self, guids: &[String]) {
        let mut done = Vec::new();
        let mut full = Vec::new();
        for guid in guids {
            let Some(p) = self.store.get_mut(guid).filter(|p| cc::eligible(p)) else { continue };
            let stored = p.get("Triggers");
            match cc::with_ours(stored.as_ref()) {
                Some(list) => {
                    p.set("Triggers", list);
                    if cc::has_both(p.get("Triggers").as_ref()) {
                        done.push(guid.clone());
                    } else {
                        full.push(crate::profile_list::display_text(&p.name()));
                    }
                }
                None => full.push(crate::profile_list::display_text(&p.name())),
            }
        }
        self.claude_triggers_written(&done);
        // Session copies that override the triggers do not follow their profile: they get the pair too.
        let copies: Vec<String> = self.store.sessions.iter().filter(|s| done.contains(&s.original_guid()) && !cc::has_both(s.profile.get("Triggers").as_ref())).map(|s| s.profile.guid()).collect();
        for copy in copies {
            let stored = self.store.get(&copy).and_then(|p| p.get("Triggers"));
            if let Some(list) = cc::with_ours(stored.as_ref()) {
                self.store.set_session(&copy, &[("Triggers".to_string(), Some(list))]);
            }
        }
        self.refresh_panes();
        self.reconcile_triggers_cache();
        if !full.is_empty() {
            self.ask_scripts(failed_ask(&format!("These profiles have too many triggers to add two more: {}.", full.join(", "))));
        }
        if full.is_empty() && !done.is_empty() {
            if let Some(w) = self.cc_onboarding.as_mut() {
                w.mark_done(Step::InstallTriggers);
            }
        }
        self.refresh_claude_menu();
    }

    /// Saved, followed by the sessions, and the dynamic files queued (`dynamic_edited` ignores the rest).
    fn claude_triggers_written(&mut self, guids: &[String]) {
        for guid in guids {
            self.dynamic_edited(guid);
        }
        self.store.merge_sessions();
        self.save_due = Some(Instant::now() + SAVE_DELAY);
    }

    /// Uninstall's triggers step (`uninstallTriggers`): ours out of every profile the installer could
    /// write (a non-rewritable dynamic one is regenerated from its file, so it is skipped) and out of
    /// every session copy; nothing else in any list moves. The cache is recomputed.
    pub(super) fn uninstall_triggers(&mut self) {
        let mut changed = Vec::new();
        for p in self.store.profiles.iter_mut().filter(|p| cc::eligible(p)) {
            let Some(stored) = p.get("Triggers") else { continue };
            let Some(list) = stored.as_array() else { continue };
            let kept = cc::strip_ours(list);
            if kept.len() != list.len() {
                p.set("Triggers", toml::Value::Array(kept));
                changed.push(p.guid());
            }
        }
        self.claude_triggers_written(&changed);
        let copies: Vec<(String, Vec<toml::Value>)> = self
            .store
            .sessions
            .iter()
            .filter_map(|s| {
                let stored = s.profile.get("Triggers")?;
                let list = stored.as_array()?;
                let kept = cc::strip_ours(list);
                (kept.len() != list.len()).then(|| (s.profile.guid(), kept))
            })
            .collect();
        for (copy, kept) in copies {
            self.store.set_session(&copy, &[("Triggers".to_string(), Some(toml::Value::Array(kept)))]);
        }
        self.refresh_panes();
        self.reconcile_triggers_cache();
        self.refresh_claude_menu();
    }

    /// `triggersInstalledOnDisk`: every eligible profile holds both triggers, and there is one.
    fn triggers_installed_in_profiles(&self) -> bool {
        let mut any = false;
        for p in self.store.profiles.iter().filter(|p| cc::eligible(p) && !self.store.transient.contains(&p.guid())) {
            any = true;
            if !cc::has_both(p.get("Triggers").as_ref()) {
                return false;
            }
        }
        any
    }

    /// Reseeds the cached flag the menu reads (`reconcileTriggersCache`).
    pub(super) fn reconcile_triggers_cache(&mut self) {
        let on = self.triggers_installed_in_profiles();
        if self.globals.bool(cc::TRIGGERS_INSTALLED_KEY) != on {
            self.globals.set(cc::TRIGGERS_INSTALLED_KEY, toml::Value::Boolean(on));
            self.save_globals();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::scripts_alerts::{on_close, Answer};
    use super::*;

    /// The answer a closed window gives, as the label of the button it presses.
    fn closes_to(a: &Ask) -> String {
        let labels: Vec<&str> = a.spec.labels.iter().map(String::as_str).collect();
        match on_close(&a.then, crate::prompt_ui::close_index(&labels)) {
            Answer::Button(i) => labels[i].to_string(),
            Answer::Closed => "closed".into(),
        }
    }

    /// P10: every question closes to the answer that changes nothing, whatever the window's
    /// own close answer is.
    #[test]
    fn claude_questions_close_to_their_safe_answer() {
        let p = Path::new(r"C:\c\settings.json");
        assert_eq!(closes_to(&uninstall_ask(p)), "Cancel");
        assert_eq!(closes_to(&remove_failed_ask("x")), "Cancel");
        assert_eq!(closes_to(&disable_api_ask()), "Leave Enabled");
        assert_eq!(closes_to(&repair_ask(p)), "Not Now");
        assert_eq!(closes_to(&cookie_ask()), "Cancel");
        assert_eq!(closes_to(&edited_workgroup_ask()), "Keep It", "a closed window never removes the person's edited workgroup");
        let a = cookie_ask();
        assert_eq!((a.spec.heading.as_str(), a.spec.labels.clone(), a.spec.id), ("Enable Session Cookies?", vec!["Enable".to_string(), "Cancel".to_string()], None));
    }

    #[test]
    fn claude_questions_follow_iterm2() {
        let p = Path::new(r"C:\c\settings.json");
        let a = uninstall_ask(p);
        assert_eq!((a.spec.heading.as_str(), a.spec.labels.clone(), a.spec.cancel), ("Uninstall Claude Code Integration?", vec!["Uninstall".to_string(), "Cancel".to_string()], Some(1)));
        assert_eq!(a.spec.id, None, "never silenced");
        let a = remove_failed_ask("Could not read x.");
        assert_eq!((a.spec.heading.as_str(), a.spec.labels.clone()), ("Couldn\u{2019}t Remove Hooks", vec!["Continue".to_string(), "Cancel".to_string()]));
        assert!(a.spec.text.starts_with("Could not read x. Continue removing the workgroup and triggers anyway?"));
        let a = disable_api_ask();
        assert_eq!((a.spec.heading.as_str(), a.spec.labels.clone(), a.spec.id), ("Disable the Python API?", vec!["Leave Enabled".to_string(), "Disable".to_string()], None));
        // The repair prompt is silenceable for good, but never with Reinstall.
        let a = repair_ask(p);
        assert_eq!((a.spec.id.as_deref(), a.spec.kind, a.spec.heading.as_str()), (Some("NoSyncSuppressClaudeCodeIntegrationRepairPrompt"), Kind::Permanent, "Claude Code Integration Looks Broken"));
        assert_eq!(a.spec.never_remember, [0]);
        assert!(!a.spec.rememberable(0) && a.spec.rememberable(1));
        let a = failed_ask("Could not write to x: y");
        assert_eq!((a.spec.heading.as_str(), a.spec.labels.clone()), ("Failed to install hook", vec!["OK".to_string()]));
    }
}
