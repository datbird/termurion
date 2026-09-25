//! The tmux Dashboard in the app: which connection it shows, its data from that
//! connection's controller, and what its buttons do.

use super::*;
use crate::tmux_dashboard as model;
use crate::tmux_dashboard_ui::{DashData, DashEvent, TmuxDashboardWindow};

impl App {
    /// The gateways in a fixed order (the connections popup's order).
    fn tmux_gateways_sorted(&self) -> Vec<PaneId> {
        let mut g: Vec<PaneId> = self.tmux.keys().copied().collect();
        g.sort_unstable();
        g
    }

    /// Shell > tmux > Dashboard (Shift+Ctrl+Win+Return).
    pub(super) fn open_tmux_dashboard(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.tmux_dash {
            return w.focus();
        }
        let focused_gw = self.focused_id().and_then(|id| if self.tmux.contains_key(&id) { Some(id) } else { self.tmux_of(id).map(|(g, _, _)| g) });
        self.tmux_dash_gw = focused_gw.or_else(|| self.tmux_gateways_sorted().first().copied());
        let Some(g) = self.gpu.as_ref() else { return };
        match TmuxDashboardWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue) {
            Ok(w) => self.tmux_dash = Some(w),
            Err(e) => return self.banners.set("window", format!("Could not open the window: {e}")),
        }
        if let Some(gw) = self.tmux_dash_gw {
            if let Some(t) = self.tmux.get_mut(&gw) {
                t.ctl.list_sessions(&mut t.gw);
                if let Some((s, _)) = t.ctl.session.clone() {
                    let clock = self.started.elapsed().as_secs_f64();
                    t.ctl.dashboard_windows(s, clock - 1.5);
                }
            }
            self.tmux_flush(gw);
        }
        self.tmux_dashboard_refresh();
    }

    /// Each pass: the controllers' due lists go out, and the Dashboard shows what changed.
    pub(super) fn tmux_dashboard_tick(&mut self) {
        let clock = self.started.elapsed().as_secs_f64();
        for gw in self.tmux_gateways_sorted() {
            if let Some(t) = self.tmux.get_mut(&gw) {
                t.ctl.poll(clock, &mut t.gw);
            }
            self.tmux_flush(gw);
        }
        self.tmux_dashboard_refresh();
    }

    fn tmux_dashboard_refresh(&mut self) {
        if self.tmux_dash.is_none() {
            return;
        }
        let gws = self.tmux_gateways_sorted();
        if self.tmux_dash_gw.is_none_or(|g| !gws.contains(&g)) {
            self.tmux_dash_gw = gws.first().copied();
        }
        let connections: Vec<String> = gws.iter().filter_map(|g| self.tmux.get(g).map(|t| t.client.clone())).collect();
        let connection = self.tmux_dash_gw.and_then(|g| gws.iter().position(|x| *x == g)).unwrap_or(0);
        let selected = self.tmux_dash.as_ref().and_then(|w| w.selected_session);
        let mut data = DashData { connections, connection, limit: self.globals.int("TmuxDashboardLimit"), open_if_hidden: self.globals.bool("openTmuxDashboardIfHiddenWindows"), ..Default::default() };
        if let Some(t) = self.tmux_dash_gw.and_then(|g| self.tmux.get(&g)) {
            data.sessions = t.ctl.sessions.iter().map(|(id, name)| model::SessionRow { id: *id, name: name.clone() }).collect();
            data.attached = t.ctl.session.as_ref().map(|(s, _)| *s);
            let s = selected.or(data.attached);
            data.windows = s
                .and_then(|s| t.ctl.session_windows.get(&s))
                .map(|rows| rows.iter().map(|w| model::WindowRow { open: data.attached == s && t.ctl.windows.contains_key(&w.id), ..w.clone() }).collect())
                .unwrap_or_default();
        }
        if let Some(w) = self.tmux_dash.as_mut() {
            if w.data != data {
                w.set_data(data);
            }
        }
    }

    /// Events for the Dashboard window. False when `window` is not it.
    pub(super) fn tmux_dashboard_event(&mut self, window: WindowId, event: &WindowEvent, event_loop: &ActiveEventLoop) -> bool {
        let Some(w) = self.tmux_dash.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        match event {
            WindowEvent::CloseRequested => self.tmux_dash = None,
            WindowEvent::RedrawRequested => {
                let e = w.redraw();
                self.tmux_dashboard_act(e, event_loop);
            }
            other => {
                w.event(other);
            }
        }
        true
    }

    fn tmux_dashboard_act(&mut self, e: DashEvent, event_loop: &ActiveEventLoop) {
        let clock = self.started.elapsed().as_secs_f64();
        if let DashEvent::Connection(i) = e {
            self.tmux_dash_gw = self.tmux_gateways_sorted().get(i).copied();
            if let Some(w) = self.tmux_dash.as_mut() {
                w.selected_session = None;
            }
            if let Some(gw) = self.tmux_dash_gw {
                if let Some(t) = self.tmux.get_mut(&gw) {
                    t.ctl.list_sessions(&mut t.gw);
                }
                self.tmux_flush(gw);
            }
            return self.tmux_dashboard_refresh();
        }
        match e {
            DashEvent::Limit(n) => {
                self.globals.set("TmuxDashboardLimit", toml::Value::Integer(n));
                self.save_globals();
                return self.tmux_dashboard_refresh();
            }
            DashEvent::OpenIfHidden(on) => {
                self.globals.set("openTmuxDashboardIfHiddenWindows", toml::Value::Boolean(on));
                self.save_globals();
                return self.tmux_dashboard_refresh();
            }
            DashEvent::Close => {
                self.tmux_dash = None;
                return;
            }
            DashEvent::None => return,
            _ => {}
        }
        let Some(gw) = self.tmux_dash_gw else { return };
        if matches!(e, DashEvent::Detach) {
            return self.tmux_request_detach_of(gw);
        }
        let mut actions = Vec::new();
        let mut reveal = None;
        if let Some(t) = self.tmux.get_mut(&gw) {
            let c = &mut t.ctl;
            let g = &mut t.gw;
            match e {
                DashEvent::Session(s) => c.dashboard_windows(s, clock - 1.5),
                DashEvent::Attach(s) => c.command(&model::attach_session(s), g),
                DashEvent::AddSession(name) => c.command_then_list(&model::new_session(&name), false, g),
                DashEvent::RemoveSession(s) => c.command_then_list(&model::kill_session(s), true, g),
                DashEvent::RenameSession(s, name) => {
                    c.command(&model::rename_session(s, &name), g);
                    c.list_sessions(g);
                }
                DashEvent::AddWindow(s) => {
                    let recycling = t.hs.at_least(1.9);
                    c.command(&crate::tmux_edits::new_window(Some(s), &crate::tmux_edits::Dir::Home, recycling), g);
                    c.dashboard_windows(s, clock);
                }
                DashEvent::RemoveWindows(ids) => {
                    for w in ids {
                        c.command(&model::unlink_window(w), g);
                    }
                    if let Some(s) = self.tmux_dash.as_ref().and_then(|d| d.selected_session) {
                        c.dashboard_windows(s, clock);
                    }
                }
                DashEvent::OpenInTabs(ids) => c.open_windows(&ids, true, g),
                DashEvent::OpenInWindows(ids) => c.open_windows(&ids, false, g),
                DashEvent::Hide(ids) => {
                    for w in ids {
                        actions.extend(c.hide(w, g));
                    }
                }
                DashEvent::RenameWindow(s, w, name) => c.command(&crate::tmux_edits::rename_window(w, Some(s), &name), g),
                DashEvent::Reveal(w, true) => reveal = Some(w),
                DashEvent::Reveal(w, false) => c.open_windows(&[w], false, g),
                _ => {}
            }
        }
        self.tmux_flush(gw);
        if let Some(w) = reveal {
            if let Some(first) = self.tmux.get(&gw).and_then(|t| t.panes.iter().filter(|(_, (_, win))| *win == w).map(|(_, (id, _))| *id).min()) {
                self.reveal_pane(first);
            }
        }
        if !actions.is_empty() {
            self.tmux_actions(gw, actions, event_loop);
        }
        self.tmux_dashboard_refresh();
    }
}
