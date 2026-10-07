//! Navigation transitions, arrangements, deferred loading and the page color of browser panes
//! (milestone 13x2, Task 9). The pure parts are `browser_restore`.

use super::*;
use crate::browser_restore;
use serde_json::Value;
use std::time::Instant;

impl App {
    /// The launch is applying saved windows (`restoring_windows`, M9): browser pages wait.
    pub(super) fn browser_restoring(&self) -> bool {
        self.restoring_windows
    }

    /// The restoration is over: every browser pane's deferred page loads as it would have when its
    /// setup ended (final review I2). What waits is the pane's own load: the profile's Initial URL
    /// (which keeps `file:` and `about:` pages) or an arrangement's URL that `load_gate` already
    /// judged at `start_browser_pane`.
    pub(super) fn browser_restoration_done(&mut self) {
        let panes: Vec<PaneId> = self.browser_views.keys().copied().collect();
        for pane in panes {
            let Some(bv) = self.browser_views.get_mut(&pane) else { continue };
            // A view still in its setup loads when the setup ends (the flag is off by then).
            if bv.view.is_none() || bv.world.setup.is_some() {
                continue;
            }
            let Some(url) = bv.world.restore_load.take() else { continue };
            self.browser_load_own(pane, &url);
        }
    }

    /// Loads the pane's own URL (Tandem's load, never a page's): the one 13x verdict judges it here
    /// as the guarded NavigationStarting will, so a refused one says so instead of leaving a blank
    /// pane.
    pub(super) fn browser_load_own(&mut self, pane: PaneId, url: &str) {
        let judge = |p: &std::path::Path| crate::localfs::LocalPath::judge(p).is_some();
        let Some(url) = browser_restore::own_load(url, &|t| crate::webhost::file_url_local(t, &judge)) else {
            self.banners.set("action", "Tandem will not open that file location.");
            return;
        };
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        bv.world.history.arm();
        bv.bar.nav_state.will_load(&url);
        if let Some(v) = &bv.view {
            v.navigate_tandem(&url);
        }
    }

    /// A navigation of pane `pane`'s view began: how it began is what the history row it makes
    /// records (`BNS:60-82`). The world's own setup navigations are not the page's.
    pub(super) fn browser_nav_starting(&mut self, pane: PaneId, url: &str, kind: i32, user: bool, redirect: bool) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        if bv.world.setup.is_some() {
            return;
        }
        let t = bv.bar.nav_state.starting(url, kind, user, redirect);
        bv.bar.transition = Some(t);
    }

    /// The pane's page color for the minimal tab style's tint (`PS:5838-5850`): Some only for a
    /// browser pane that has reported one while the tab style is Minimal.
    #[allow(dead_code)] // 13e's minimal tab tint is its consumer
    pub(super) fn browser_page_color(&self, pane: PaneId) -> Option<(u8, u8, u8)> {
        if self.tab_style() != crate::tab_theme::TabStyle::Minimal {
            return None;
        }
        self.panes.get(&pane)?.browser_io()?.page_color
    }

    /// A page finished loading: its background color is read once, in the isolated world (a page
    /// must not choose what Tandem reads; without the world nothing is read).
    pub(super) fn browser_color_read(&mut self, pane: PaneId) {
        self.browser_eval_in_frame(pane, None, browser_restore::COLOR_SCRIPT, Box::new(|app, pane, r| {
            // Only a short string is parsed; anything else is no color.
            let color = match r {
                Ok(Value::String(s)) => browser_restore::parse_color(&s),
                _ => return,
            };
            app.browser_color_offer(pane, color);
        }));
    }

    /// A reading of the color: applied at most once per interval (`PS:5879-5898`).
    fn browser_color_offer(&mut self, pane: PaneId, color: Option<(u8, u8, u8)>) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        if let Some(c) = bv.bar.color.offer(color, Instant::now()) {
            self.browser_color_apply(pane, c);
        }
    }

    /// Every frame: a reading that waited for the interval is applied when it is over.
    pub(super) fn browser_color_tick(&mut self, pane: PaneId) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        if !bv.bar.color.waiting() {
            return;
        }
        match bv.bar.color.due(Instant::now()) {
            Some(c) => self.browser_color_apply(pane, c),
            None => self.redraw(),
        }
    }

    fn browser_color_apply(&mut self, pane: PaneId, color: Option<(u8, u8, u8)>) {
        if let Some(io) = self.panes.get_mut(&pane).and_then(|p| p.browser_io_mut()) {
            if io.page_color != color {
                io.page_color = color;
                self.redraw();
            }
        }
    }

    /// A restored pane takes the link target its arrangement kept (`PS:334`), bounded and clean.
    pub(super) fn browser_target_restored(&mut self, pane: PaneId, session: &toml::Value) {
        let target = restored_target(session);
        if let Some(io) = self.panes.get_mut(&pane).and_then(|p| p.browser_io_mut()) {
            io.target = target;
        }
    }
}

/// A session table's link target, judged again on load.
pub(super) fn restored_target(session: &toml::Value) -> Option<String> {
    browser_restore::target_text(session.get(browser_restore::TARGET_KEY)?.as_str()?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_restored_target_is_judged_again() {
        let mut t = toml::Table::new();
        t.insert(crate::browser_restore::TARGET_KEY.into(), toml::Value::String("docs".into()));
        assert_eq!(super::restored_target(&toml::Value::Table(t.clone())).as_deref(), Some("docs"));
        t.insert(crate::browser_restore::TARGET_KEY.into(), toml::Value::String("a\u{202e}b".into()));
        assert_eq!(super::restored_target(&toml::Value::Table(t.clone())), None);
        t.insert(crate::browser_restore::TARGET_KEY.into(), toml::Value::Integer(3));
        assert_eq!(super::restored_target(&toml::Value::Table(t)), None);
        assert_eq!(super::restored_target(&toml::Value::Table(toml::Table::new())), None);
        // A hostile URL in an arrangement never reaches a load: the profile's Initial URL starts.
        for bad in ["file://evil/s/x", "file:////evil/s/x", "\\\\evil\\s\\x", "javascript:x"] {
            let mut t = toml::Table::new();
            t.insert(crate::browser::URL_KEY.into(), toml::Value::String(bad.into()));
            let restored = super::super::browser_app::restored_url(&toml::Value::Table(t));
            assert_eq!(restored.and_then(|u| crate::browser_restore::load_gate(&u)), None, "{bad}");
        }
    }

    fn prod(src: &str) -> &str {
        &src[..src.find("#[cfg(test)]").unwrap_or(src.len())]
    }

    /// Arrangements keep the URL and the link target and nothing else of the page; a restored page
    /// starts at its URL through the one gate; a duplicate goes the same road.
    #[test]
    fn browser_arrangement_round_trips() {
        let arrange = include_str!("arrange_ui.rs");
        let save = &arrange[arrange.find("    fn session_value(").unwrap()..arrange.find("pub(super) fn snapshot_window").unwrap()];
        assert!(save.contains("crate::browser::arrangement_url(&io.url)") && save.contains("crate::browser::URL_KEY"), "13x's one URL key");
        assert!(save.contains("crate::browser_restore::TARGET_KEY") && save.contains("target_text"), "the target, bounded and clean");
        assert!(!save.contains("Browser State") && !save.contains("Tandem URL"), "no second key for the URL (B13)");
        let build = &arrange[arrange.find("self.browser_url_override = super::browser_app::restored_url(&session)").unwrap()..];
        assert!(build.find("browser_target_restored(id, &session)").unwrap() > build.find("self.start_dir_override = None").unwrap(), "the target goes on the started pane");
        let app = prod(include_str!("browser_app.rs"));
        let start = &app[app.find("pub(super) fn start_browser_pane").unwrap()..app.find("pub(super) fn place_browser_views").unwrap()];
        assert!(start.contains("self.browser_url_override.take()") && start.contains("browser_restore::load_gate"), "a restored URL passes the navigation gate again at load time");
    }

    /// While the launch restores windows a browser pane loads nothing; the end of the restoration loads it.
    #[test]
    fn restoration_defers_the_load() {
        let scripts = prod(include_str!("browser_scripts_app.rs"));
        let finish = &scripts[scripts.find("fn browser_world_finish").unwrap()..scripts.find("pub(super) fn browser_world_navigated").unwrap()];
        assert!(finish.contains("browser_restoring()") && finish.contains("restore_load"), "the load waits");
        let m = prod(include_str!("mod.rs"));
        assert!(m.contains("restoring_windows: bool") && m.contains("restoring_windows: false"));
        assert!(m.contains("self.restoring_windows = ") && m.contains("self.browser_restoration_done()"));
        let drain = &m[m.find("for w in std::mem::take(&mut self.pending_windows)").unwrap()..];
        assert!(drain[..drain.find("self.api_tmux_tabs_opened()").unwrap()].contains("browser_restoration_done"), "after the last saved window");
        let me = prod(include_str!("browser_restore_app.rs"));
        let done = &me[me.find("fn browser_restoration_done").unwrap()..me.find("pub(super) fn browser_load_own").unwrap()];
        assert!(done.contains("self.browser_load_own(pane, &url)") && !done.contains("load_gate("), "the deferred load loads as the setup's end would (final review I2)");
        let own = &me[me.find("pub(super) fn browser_load_own").unwrap()..me.find("pub(super) fn browser_nav_starting").unwrap()];
        assert!(own.contains("browser_restore::own_load(") && own.contains("self.banners.set(") && own.contains("navigate_tandem"), "a refused own load shows a banner, never a blank");
        assert!(finish.contains("self.browser_load_own(pane, &url)") && !finish.contains("navigate_tandem"), "one road for the pane's own load");
        assert!(me.contains("fn browser_restoring(&self) -> bool"));
    }

    /// Starting records the transition the next history row takes; the page color is read in the
    /// isolated world after a completed load, never in the page.
    #[test]
    fn transitions_and_the_page_color_are_wired() {
        let app = prod(include_str!("browser_app.rs"));
        let ev = &app[app.find("pub(super) fn browser_event").unwrap()..app.find("fn browser_view_failed").unwrap()];
        assert!(ev.contains("Event::Starting { url, kind, user, redirect, .. } => {") && ev.contains("self.browser_nav_starting(pane, url, *kind, *user, *redirect)"));
        assert!(ev.contains("self.browser_color_read(pane)"));
        let me = prod(include_str!("browser_restore_app.rs"));
        assert!(me.contains("browser_eval_in_frame(") && !me.contains("browser_eval(pane"), "the isolated world only (P17)");
        assert!(me.contains("parse_color(") && me.contains("bar.color") && me.contains("TabStyle::Minimal"));
        assert!(app.contains("will_load("), "a typed address is the requested URL");
        let rec = prod(include_str!("browser_db_app.rs"));
        assert!(rec.contains("bv.bar.transition.take()"), "record_visit receives the transition");
    }
}
