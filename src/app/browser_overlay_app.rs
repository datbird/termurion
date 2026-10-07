//! Overlays above a browser page (milestone 13x2, Task 5): the hovered link, the toast, and
//! announcement bars. A child web view covers anything the scene draws in its rectangle, so the
//! link and the toast are `browser_tip`'s popup window; a bar is drawn by the scene over the top of
//! the pane and the web view gives way below it. Everything shown is Tandem's text or a URL that
//! `browser_tip::tip_text` made inert; the hovered link is accepted only from the Ready isolated
//! world and only while the real pointer is over the rectangle it reports (P14).

use super::*;
use crate::browser_tip::{self, Announce, Hover, TipWindow};

/// What an announcement's answer does: the pressed button's place, None when it was closed
/// (iTerm2's `announce(_:)` answering nil for its -2 and -1, `PSB:420-450`).
pub(super) type BarDone = Box<dyn FnOnce(&mut App, PaneId, Option<usize>)>;

/// An announcement's text is cut here (P15).
#[allow(dead_code)] // used by `browser_announce`, whose callers are later plans (13x6 audio, 13x7 passwords)
const BAR_CHARS: usize = 400;

/// A tip or toast waiting for the next `web_tick`.
#[allow(dead_code)] // Toast: made by `browser_toast`, called by Task 7's Copy Link Address
pub(super) enum TipAsk {
    Hover(PaneId, Hover),
    Toast(PaneId, String, f64),
    /// The newest ask wins, so a Clear cancels a Show that waits.
    Clear(PaneId),
}

impl App {
    /// Where pane `id`'s web view goes: `r`, the pane's `visible()` rectangle (already below Tandem's
    /// chrome and the toolbar strip: cut once, never again), and below the front announcement bar
    /// when one covers the top of it (P21; the scene draws the bar at the top of `r`).
    pub(super) fn browser_web_rect(&self, id: PaneId, r: Rect) -> Rect {
        let Some(bar) = self.bars.get(&id).and_then(|b| b.front()).filter(|_| self.is_browser_pane(id)) else { return r };
        let bottom = r.y + crate::infobar::layout(bar, r, self.pane_cell(id)).bar.h;
        let (y, h) = browser_tip::below_bar(r.y, r.h, bottom);
        Rect { y, h, ..r }
    }

    /// The toolbar strip of browser pane `id`, cut once from the pane's FULL rectangle (`visible()`
    /// returns what is left below it, so `pane_chrome` must never be run on that).
    pub(super) fn browser_strip(&self, tab: &Tab, id: PaneId) -> Option<Rect> {
        let (_, full) = crate::layout::visible_rects(&tab.layout, tab.zoomed, self.pane_area_for(tab), self.gap()).into_iter().find(|(p, _)| *p == id)?;
        let strip = self.pane_chrome(tab, id, full).browser_bar?;
        // As wide as the web view: never over the Clippings or inline-chat panel beside it.
        Some(self.gutter_split(id, strip).0)
    }

    /// The rectangle of pane `pane`'s shown web view in this window's pixels; None when it is not shown.
    fn browser_shown_rect(&self, pane: PaneId) -> Option<Rect> {
        let tab = self.tabs.get(self.active)?;
        let (_, r) = self.visible(tab).into_iter().find(|(id, _)| *id == pane)?;
        Some(self.browser_web_rect(pane, r))
    }

    // --- The hovered link and the toast ---

    /// The link under the pointer (or none) in pane `pane`. A Show waits for the next `web_tick`
    /// (the window needs the event loop; the newest one wins, which also bounds a page's flood).
    pub(super) fn browser_hover(&mut self, pane: PaneId, hover: Hover) {
        match hover {
            Hover::Clear => self.browser_tip_next = Some(TipAsk::Clear(pane)),
            show => self.browser_tip_next = Some(TipAsk::Hover(pane, show)),
        }
    }

    fn browser_hover_now(&mut self, pane: PaneId, hover: Hover, event_loop: &ActiveEventLoop) {
        let Hover::Show { url, rect } = hover else { return };
        let Some(view) = self.browser_shown_rect(pane) else {
            self.browser_tip_close(pane);
            return;
        };
        let zoom = self.panes.get(&pane).and_then(|p| p.browser_io()).map_or(1.0, |io| if io.zoom > 0.0 { io.zoom / 100.0 } else { 1.0 });
        let scale = f64::from(self.scale());
        let (x, y) = crate::browser_js::from_js((rect.0, rect.1), scale, zoom);
        let (w, h) = crate::browser_js::from_js((rect.2, rect.3), scale, zoom);
        let Some(origin) = self.gpu.as_ref().and_then(|g| g.window.inner_position().ok()) else { return };
        let (ox, oy) = (f64::from(origin.x), f64::from(origin.y));
        // The page's rectangle, cut to the view, in screen pixels.
        let (vx, vy) = (f64::from(view.x) + ox, f64::from(view.y) + oy);
        let (left, top) = ((vx + x).max(vx), (vy + y).max(vy));
        let (right, bottom) = ((vx + x + w).min(vx + f64::from(view.w)), (vy + y + h).min(vy + f64::from(view.h)));
        let anchor = (left, top, (right - left).max(0.0), (bottom - top).max(0.0));
        // The hover script is copied unchanged and does not check isTrusted: only a pointer that is
        // really over the rectangle makes it a link under the pointer.
        if !browser_tip::pointer_now().is_some_and(|p| browser_tip::pointer_in(p, anchor)) {
            self.browser_tip_close(pane);
            return;
        }
        let pane_rect = (vx, vy, f64::from(view.w), f64::from(view.h));
        // The window fits this to its measured width when it paints (the end of the host stays).
        let text = crate::browser::display_url(&url);
        if text.is_empty() {
            self.browser_tip_close(pane);
            return;
        }
        // A toast that still shows is not replaced by a hover.
        if self.browser_tip.as_ref().is_some_and(|t| t.is_toast() && !t.expired(Instant::now())) {
            return;
        }
        // The previous owner's page forgets its link when another pane's takes the window.
        let previous = self.browser_tip.as_ref().and_then(|t| t.owner).filter(|o| *o != pane as u64).map(|o| o as PaneId);
        if let Some(old) = previous {
            self.browser_tip_clear(old);
        }
        match self.browser_tip_life.show(Instant::now()) {
            browser_tip::Step::Reuse => {
                if let Some(tip) = self.browser_tip.as_mut() {
                    tip.owner = Some(pane as u64);
                    tip.set(&text, anchor);
                    tip.bound(pane_rect);
                }
            }
            browser_tip::Step::Create => {
                if let Some(mut tip) = self.browser_tip_open(&text, anchor, event_loop) {
                    tip.owner = Some(pane as u64);
                    tip.bound(pane_rect);
                    self.browser_tip = Some(tip);
                } else {
                    self.browser_tip_life.dropped();
                }
            }
            _ => {}
        }
    }

    fn browser_tip_open(&mut self, text: &str, at: (f64, f64, f64, f64), event_loop: &ActiveEventLoop) -> Option<TipWindow> {
        let g = self.gpu.as_ref()?;
        match TipWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, text, at) {
            Ok(t) => Some(t),
            Err(_) => {
                crate::dlog!("browser tip: the window could not be made");
                None
            }
        }
    }

    /// "Copied" and its like: `text` (Tandem's own) at the pointer for `secs`.
    #[allow(dead_code)] // Task 7 (Copy Link Address)
    pub(super) fn browser_toast(&mut self, pane: PaneId, text: &str, secs: f64) {
        self.browser_tip_next = Some(TipAsk::Toast(pane, text.to_string(), secs));
    }

    fn browser_toast_now(&mut self, pane: PaneId, text: &str, secs: f64, event_loop: &ActiveEventLoop) {
        let Some(at) = browser_tip::pointer_now() else { return };
        let text = crate::inert_files::display_text(text, browser_tip::TOAST_CHARS, false);
        if let Some(tip) = self.browser_tip.as_mut() {
            tip.owner = Some(pane as u64);
            tip.toast(&text, at, secs);
            self.browser_tip_life.show(Instant::now());
            return;
        }
        if self.browser_tip_life.show(Instant::now()) != browser_tip::Step::Create {
            return;
        }
        if let Some(mut tip) = self.browser_tip_open(&text, (at.0, at.1, 0.0, 0.0), event_loop) {
            tip.owner = Some(pane as u64);
            tip.toast(&text, at, secs);
            self.browser_tip = Some(tip);
        } else {
            self.browser_tip_life.dropped();
        }
    }

    /// The tip's window events. False for other windows.
    pub(super) fn browser_tip_event(&mut self, window: WindowId, e: &WindowEvent) -> bool {
        let Some(tip) = self.browser_tip.as_mut().filter(|t| t.id() == window) else { return false };
        tip.event(e);
        true
    }

    /// Closes the tip of pane `pane` (a toast ends by its time, and goes with its pane).
    pub(super) fn browser_tip_close(&mut self, pane: PaneId) {
        self.browser_tip_cancel(pane);
        if let Some(t) = self.browser_tip.as_mut().filter(|t| t.owner == Some(pane as u64)) {
            t.hide();
            self.browser_tip_life.hide();
        }
    }

    /// A Show that waits for pane `pane` is cancelled.
    fn browser_tip_cancel(&mut self, pane: PaneId) {
        if matches!(&self.browser_tip_next, Some(TipAsk::Hover(p, _)) if *p == pane) {
            self.browser_tip_next = None;
        }
    }

    /// Drops the window itself (the pane went, the app lost the foreground).
    pub(super) fn browser_tip_drop(&mut self, pane: PaneId) {
        self.browser_tip_cancel(pane);
        if self.browser_tip.as_ref().is_some_and(|t| t.owner == Some(pane as u64)) {
            self.browser_tip = None;
            self.browser_tip_life.dropped();
        }
    }

    /// Closes pane `pane`'s tip and tells its page to forget the link (iTerm2 does the same on
    /// `mouseExited`, `BWV:858-870`, `BM:952-955`; WebView2 reports no exit). Evaluated in the main
    /// frame's Tandem world, where the secret-checking handler lives, never the page world.
    pub(super) fn browser_tip_clear(&mut self, pane: PaneId) {
        self.browser_tip_cancel(pane);
        if !self.browser_tip.as_ref().is_some_and(|t| t.owner == Some(pane as u64)) {
            return;
        }
        self.browser_tip_close(pane);
        if let Some(secret) = self.browser_view_secret(pane).map(str::to_string) {
            self.browser_eval_in_frame(pane, None, &browser_tip::clear_script(&secret), Box::new(|_, _, _| {}));
        }
    }

    /// The window lost the foreground (or was minimized): whatever tip shows goes.
    pub(super) fn browser_tip_clear_all(&mut self) {
        if let Some(pane) = self.browser_tip.as_ref().and_then(|t| t.owner) {
            self.browser_tip_clear(pane as PaneId);
            self.browser_tip_drop(pane as PaneId);
        }
        self.browser_tip = None;
        self.browser_tip_next = None;
        self.browser_tip_life.dropped();
    }

    /// Runs the newest hover or toast that waits, and ends a toast whose time is over. Called from
    /// `web_tick`.
    pub(super) fn browser_tip_tick(&mut self, event_loop: &ActiveEventLoop) {
        match self.browser_tip_next.take() {
            Some(TipAsk::Hover(pane, hover)) => self.browser_hover_now(pane, hover, event_loop),
            Some(TipAsk::Toast(pane, text, secs)) => self.browser_toast_now(pane, &text, secs, event_loop),
            Some(TipAsk::Clear(pane)) => self.browser_tip_close(pane),
            None => {}
        }
        if self.browser_tip.as_ref().is_some_and(|t| t.expired(Instant::now())) {
            if let Some(t) = self.browser_tip.as_mut() {
                t.hide();
            }
            self.browser_tip_life.hide();
        }
    }

    // --- Announcement bars ---

    /// Shows an announcement bar on pane `pane` and answers `done` with the pressed button's place,
    /// or None when it is closed (`PSB:420-450`). A bar with the same identifier already showing
    /// answers None at once and queues nothing. The bar is drawn by the scene; the page below it moves.
    #[allow(dead_code)] // 13x6 (audio, permissions) and 13x7 (passwords)
    pub(super) fn browser_announce(&mut self, pane: PaneId, id: &str, text: &str, buttons: &[&str], done: BarDone) {
        let showing = self.bars.get(&pane).is_some_and(|b| b.has(id));
        let stale = self.browser_views.get(&pane).is_some_and(|b| b.bars.contains_key(id));
        match browser_tip::announce_plan(showing, stale) {
            Announce::Refuse => return done(self, pane, None),
            Announce::EndStale => {
                if let Some((_, old)) = self.browser_views.get_mut(&pane).and_then(|b| b.bars.remove(id)) {
                    old(self, pane, None);
                }
            }
            Announce::Queue => {}
        }
        let Some(bv) = self.browser_views.get_mut(&pane) else { return done(self, pane, None) };
        bv.bars.insert(id.to_string(), (buttons.iter().map(|b| b.to_string()).collect(), done));
        let shown = crate::inert_files::display_text(text, BAR_CHARS, false);
        if !self.ask(pane, id, &shown, buttons, true) {
            if let Some((_, refused)) = self.browser_views.get_mut(&pane).and_then(|b| b.bars.remove(id)) {
                refused(self, pane, None);
            }
        }
    }

    /// A bar's answer (`bar_answered` asks this first): true when it was a browser announcement.
    pub(super) fn browser_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        let Some((buttons, done)) = self.browser_views.get_mut(&pane).and_then(|b| b.bars.remove(id)) else { return false };
        done(self, pane, browser_tip::bar_index(&buttons, button));
        true
    }

    /// A pane that goes away answers its waiting announcements with None (iTerm2's -2).
    pub(super) fn browser_bars_end(&mut self, pane: PaneId) {
        let waiting: Vec<(Vec<String>, BarDone)> = self.browser_views.get_mut(&pane).map(|b| b.bars.drain().map(|(_, v)| v).collect()).unwrap_or_default();
        for (_, done) in waiting {
            done(self, pane, None);
        }
    }
}

#[cfg(test)]
mod tests {
    fn prod() -> &'static str {
        let src = include_str!("browser_overlay_app.rs");
        &src[..src.find("#[cfg(test)]").unwrap()]
    }

    /// A real bug on main: `App::visible()` already returns each pane's rectangle after Tandem's
    /// chrome (title bar, status bar and the browser strip), so running `pane_chrome` on it cuts the
    /// strip a second time. The strip is cut once, from the pane's full rectangle (`browser_strip`),
    /// and the web view is the `visible()` rectangle itself, below the front announcement bar.
    #[test]
    fn the_browser_strip_is_cut_once() {
        let strip = &prod()[prod().find("pub(super) fn browser_strip").expect("one helper cuts the strip")..];
        assert!(strip.contains("visible_rects(") && strip.contains("pane_chrome(tab, id, full)"), "from the full rectangle");
        let web = &prod()[prod().find("pub(super) fn browser_web_rect").unwrap()..prod().find("pub(super) fn browser_strip").unwrap()];
        assert!(!web.contains("pane_chrome("), "visible() is already the web view's rectangle");
        let app = include_str!("browser_app.rs");
        let app = &app[..app.find("#[cfg(test)]").unwrap()];
        assert!(!app.contains("pane_chrome("), "no browser code re-cuts a visible() rectangle");
        assert_eq!(app.matches("self.browser_strip(tab,").count(), 2, "the bar's rectangles and the hit test");
        let m = include_str!("mod.rs");
        let m = &m[m.find("// A browser pane's view covers its rectangle").unwrap()..];
        let m = &m[..m.find("continue;").unwrap()];
        assert!(m.contains("self.browser_strip(tab, id)") && !m.contains("pane_chrome("));
        assert!(m.contains("self.draw_info_bar(&mut frame, id, rect, self.pane_cell(id))"), "an announcement bar is drawn for a browser pane too");
    }

    /// The overlay is Tandem's: never inside the page, never the page's text unmade inert, never
    /// focused, and the real pointer decides (P14).
    #[test]
    fn overlays_are_tandems_own() {
        let p = prod();
        let hover = &p[p.find("fn browser_hover_now").unwrap()..p.find("fn browser_tip_open").unwrap()];
        assert!(hover.contains("pointer_now()") && hover.contains("pointer_in(") && hover.contains("display_url(&url)") && include_str!("../browser_tip.rs").contains("fit_measured(&text, room"));
        assert!(hover.contains("from_js("), "page pixels become window pixels");
        assert!(!p.contains(".focus()") && !p.contains("frame.overlay") && !p.contains("execute_script("), "no focus, no scene overlay, no page-world script");
        let clear = &p[p.find("pub(super) fn browser_tip_clear(").unwrap()..p.find("pub(super) fn browser_tip_clear_all").unwrap()];
        assert!(clear.contains("browser_eval_in_frame(pane, None,") && clear.contains("clear_script("), "the clear runs in the main frame's Tandem world");
        assert!(!clear.contains("browser_eval(") && !clear.contains("browser_page_call"));
        let toast = &p[p.find("fn browser_toast_now").unwrap()..p.find("pub(super) fn browser_tip_event").unwrap()];
        assert!(toast.contains("display_text("), "even a toast's text is made inert");
    }

    /// iTerm2's `announce(_:)`: a second bar with the same identifier answers nil at once, a closed
    /// bar answers nil, a press answers its index; the page gives way only below the bar (P21).
    #[test]
    fn announcements_follow_iterm2() {
        let p = prod();
        let a = &p[p.find("pub(super) fn browser_announce").unwrap()..p.find("pub(super) fn browser_bar(").unwrap()];
        assert!(a.contains("self.bars.get(&pane).is_some_and(|b| b.has(id))"), "Bars::has first: push would replace it");
        assert!(a.contains("Announce::Refuse => return done(self, pane, None)") && a.contains("self.ask(pane, id, &shown, buttons, true)"));
        let b = &p[p.find("pub(super) fn browser_bar(").unwrap()..p.find("pub(super) fn browser_bars_end").unwrap()];
        assert!(b.contains("bar_index(&buttons, button)") && b.contains("bars.remove(id)"), "an answer runs once");
        let chain = include_str!("infobar_ui.rs");
        let chain = &chain[chain.find("fn bar_answered").unwrap()..];
        assert!(chain.find("!self.browser_bar(pane, id, button)").unwrap() < chain.find("!self.host_state_bar(").unwrap(), "browser bars first");
        let place = include_str!("browser_app.rs");
        let place = &place[place.find("pub(super) fn place_browser_views").unwrap()..place.find("fn show_browser_view").unwrap()];
        assert!(place.contains("self.browser_web_rect(id, r)"));
        let m = include_str!("mod.rs");
        assert!(m.contains("browser_tip: Option<crate::browser_tip::TipWindow>") && m.contains("self.browser_tip_event(id, &event)"));
        assert!(!include_str!("windows_ui.rs").contains("browser_tip"), "the tip is app-wide, not per window");
        let close = include_str!("browser_app.rs");
        let close = &close[close.find("pub(super) fn close_browser_view").unwrap()..];
        assert!(close.find("browser_bars_end(pane)").unwrap() < close.find("browser_views.remove(&pane)").unwrap(), "waiting announcements answer None first");
    }

    /// Each reason the link should go runs the clear.
    #[test]
    fn the_tip_clears_for_each_reason() {
        let app = include_str!("browser_app.rs");
        assert!(app.contains("if matches!(e, Event::Focus(_, false)) {\n            self.browser_tip_clear(pane);"), "focus loss");
        let hidden = &app[app.find("pub(super) fn place_browser_views").unwrap()..app.find("fn show_browser_view").unwrap()];
        assert!(hidden.contains("self.browser_tip_clear(pane)"), "hidden pane or another tab");
        let scripts = include_str!("browser_scripts_app.rs");
        let starting = &scripts[scripts.find("// A main-frame navigation began").unwrap()..scripts.find("Event::ScriptDone { token, json, .. } if self.browser_owns_token").unwrap()];
        assert!(starting.contains("self.browser_tip_clear(pane)"), "navigation");
        let m = include_str!("mod.rs");
        assert!(m.contains("self.browser_tip_clear_all()"), "deactivation");
        assert!(include_str!("webhost_app.rs").contains("self.browser_tip_tick(event_loop)"));
    }

    /// Review I2: a Clear is queued like a Show, a clear hides the one window instead of dropping it,
    /// and only the pane going or the app losing the foreground drops it.
    #[test]
    fn the_tip_window_is_made_once() {
        let p = prod();
        assert!(p.contains("Hover::Clear => self.browser_tip_next = Some(TipAsk::Clear(pane))"));
        let close = &p[p.find("pub(super) fn browser_tip_close").unwrap()..p.find("fn browser_tip_cancel").unwrap()];
        assert!(close.contains("t.hide()") && !close.contains("browser_tip = None"), "a clear hides, never drops");
        assert!(p.contains("fn browser_tip_cancel") && p.contains("self.browser_tip_life.show(Instant::now())"));
        let open = &p[p.find("fn browser_tip_open").unwrap()..p.find("pub(super) fn browser_toast(").unwrap()];
        assert!(!open.contains("browser_tip = None"), "opening never drops a tip first");
        assert!(include_str!("mod.rs").contains("self.browser_tip_drop(id)"), "the pane going drops it");
    }

    /// Review minor: the toolbar strip is as wide as the web view, so it never runs over the
    /// Clippings or inline-chat panel beside a browser pane.
    #[test]
    fn the_strip_stops_at_the_panels() {
        let p = prod();
        let f = &p[p.find("pub(super) fn browser_strip").unwrap()..p.find("fn browser_shown_rect").unwrap()];
        assert!(f.contains("self.gutter_split(id, strip).0"), "{f}");
    }
}
