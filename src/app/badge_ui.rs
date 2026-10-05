//! Badges in the window: the text from the profile (or SetBadgeFormat), laid out with the
//! badge font, drawn behind the pane's text.

use std::cell::RefCell;

use super::*;
use crate::badge::{self, BadgeLayout, BadgePrefs};

impl App {
    /// The badge's text for a pane: its profile's Badge Text over the session's variables.
    pub(super) fn badge_text(&self, pane: PaneId) -> String {
        let Some(p) = self.panes.get(&pane) else { return String::new() };
        let template = self.store.get(&p.profile).map(|pr| pr.string("Badge Text")).unwrap_or_default();
        let s = p.own();
        badge::text(&template, &|n| s.trigger_var(n, 0))
    }

    /// Lays out the badges of the panes in view whose text or size changed.
    pub(super) fn prepare_badges(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let panes: Vec<(PaneId, Rect)> = self.visible(tab);
        let scale = self.scale();
        let mut keep = std::collections::HashSet::new();
        for (id, rect) in panes {
            keep.insert(id);
            let text = self.badge_text(id);
            let inner = self.text_rect(id, rect);
            let Some(p) = self.panes.get(&id).and_then(|p| self.store.get(&p.profile)) else { continue };
            let prefs = BadgePrefs {
                max_w: p.float("Badge Max Width") as f32,
                max_h: p.float("Badge Max Height") as f32,
                top: p.float("Badge Top Margin") as f32,
                right: p.float("Badge Right Margin") as f32,
            };
            let key = format!("{text}\u{1}{}\u{1}{}\u{1}{prefs:?}\u{1}{scale}", inner.w, inner.h);
            if self.badges.get(&id).is_some_and(|(k, _)| *k == key) {
                continue;
            }
            let Some(gpu) = self.gpu.as_mut() else { return };
            let r = RefCell::new(&mut gpu.renderer);
            // Sizes and margins are in points; the pane is in pixels.
            let measure = |t: &str, size: f32| r.borrow_mut().badge_width(t, size * scale) / scale;
            let laid = badge::layout(&text, (inner.w / scale, inner.h / scale), prefs, &measure).map(|l| BadgeLayout {
                size: l.size * scale,
                widths: l.widths.iter().map(|w| w * scale).collect(),
                rect: Rect { x: l.rect.x * scale, y: l.rect.y * scale, w: l.rect.w * scale, h: l.rect.h * scale },
                lines: l.lines,
            });
            self.badges.insert(id, (key, laid));
        }
        self.badges.retain(|id, _| keep.contains(id) || self.panes.contains_key(id));
    }

    /// Draws a pane's badge (before its text, so it sits behind).
    pub(super) fn draw_badge(&self, frame: &mut Frame, id: PaneId, rect: Rect) {
        let Some((_, Some(l))) = self.badges.get(&id) else { return };
        let Some(p) = self.panes.get(&id) else { return };
        let Some(pr) = self.store.get(&p.profile) else { return };
        let dark = self.dark();
        let font_px = self.gpu.as_ref().map_or(16.0, |g| g.renderer.font_px());
        scene::badge(frame, self.text_rect(id, rect), l, pr.color_for("Badge Color", dark), pr.alpha_for("Badge Color", dark), p.own().palette.bg, font_px);
    }

    /// SetBadgeFormat: the session's badge text, or iTerm2's refusal.
    pub(super) fn set_badge(&mut self, pane: PaneId, text: Option<String>) {
        match text {
            Some(t) => self.set_session_values(pane, &[("Badge Text".into(), Some(toml::Value::String(t)))]),
            None => {
                self.ask(pane, "badge-refused", "The application attempted to set the badge to a value that would invoke a function call. For security reasons, this is not allowed and the badge was not updated.", &["OK"], false);
            }
        }
    }
}
