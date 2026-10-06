//! Badges in the window: the text from the profile (or SetBadgeFormat), laid out with the
//! badge font, drawn behind the pane's text.

use std::cell::RefCell;

use super::*;
use crate::badge::{self, BadgeLayout, BadgePrefs};

/// The profile's own value for a badge key: None when it stores none (`PP:1594-1624`).
fn stored(profile: &Profile, key: &str) -> Option<f64> {
    profile.values.get(key).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
}

/// The badge's sizes: each from the profile only when the profile stores it, else from its
/// Advanced key (`PP:1594-1624`; iTerm2's 0.5, 0.2, 10, 10 when those are unset).
pub(super) fn badge_prefs_for(profile: &Profile, g: &Globals) -> BadgePrefs {
    let pick = |own: &str, advanced: &str, fallback: f64| stored(profile, own).or_else(|| g.number(advanced)).unwrap_or(fallback) as f32;
    BadgePrefs {
        max_w: pick("Badge Max Width", "BadgeMaxWidthFraction", 0.5),
        max_h: pick("Badge Max Height", "BadgeMaxHeightFraction", 0.2),
        right: pick("Badge Right Margin", "BadgeRightMargin", 10.0),
        top: pick("Badge Top Margin", "BadgeTopMargin", 10.0),
    }
}

/// The profile's stored "Badge Font", if any.
pub(super) fn profile_font(profile: &Profile) -> Option<&str> {
    profile.values.get("Badge Font").and_then(|v| v.as_str())
}

/// The badge's font family: a stored "Badge Font" wins, else the Advanced BadgeFont
/// (`PP:1720-1736`). A family name only; the renderer looks it up among installed fonts.
pub(super) fn badge_family(profile_font: Option<&str>, advanced: &str) -> String {
    match profile_font.map(|f| crate::profile::parse_font(f).family).filter(|f| !f.is_empty()) {
        Some(family) => family,
        None => advanced.trim().to_string(),
    }
}

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
            let prefs = badge_prefs_for(p, &self.globals);
            let (family, bold) = (badge_family(profile_font(p), &self.globals.string("BadgeFont")), self.globals.bool("BadgeFontIsBold"));
            let key = format!("{text}\u{1}{}\u{1}{}\u{1}{prefs:?}\u{1}{scale}\u{1}{family}\u{1}{bold}", inner.w, inner.h);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::globals::Globals;
    use toml::Value;

    fn globals(name: &str) -> Globals {
        Globals::empty(&std::env::temp_dir().join(format!("tandem-badge-{name}.toml")))
    }

    /// 13w Task 8 (`PP:1594-1624`): a profile's own badge size wins, key by key; every other
    /// size is the Advanced key (iTerm2's 0.5, 0.2, 10, 10 when unset).
    #[test]
    fn badge_defaults_follow_the_advanced_keys() {
        let mut g = globals("prefs");
        let profile = Profile::default();
        assert_eq!(badge_prefs_for(&profile, &g), BadgePrefs { max_w: 0.5, max_h: 0.2, right: 10.0, top: 10.0 });
        g.set("BadgeMaxWidthFraction", Value::Float(0.3));
        g.set("BadgeMaxHeightFraction", Value::Float(0.4));
        g.set("BadgeRightMargin", Value::Integer(20));
        g.set("BadgeTopMargin", Value::Integer(5));
        assert_eq!(badge_prefs_for(&profile, &g), BadgePrefs { max_w: 0.3, max_h: 0.4, right: 20.0, top: 5.0 });
        let mut own = Profile::default();
        own.set("Badge Max Width", Value::Float(0.7));
        assert_eq!(badge_prefs_for(&own, &g), BadgePrefs { max_w: 0.7, max_h: 0.4, right: 20.0, top: 5.0 });
        own.set("Badge Top Margin", Value::Float(0.0));
        assert_eq!(badge_prefs_for(&own, &g).top, 0.0, "a stored zero is the profile's own");
    }

    /// `PP:1720-1736`: a stored Badge Font wins; else the Advanced BadgeFont.
    #[test]
    fn badge_font_follows_iterm2() {
        assert_eq!(badge_family(None, "Helvetica"), "Helvetica");
        assert_eq!(badge_family(None, "Segoe UI"), "Segoe UI");
        assert_eq!(badge_family(Some("Consolas"), "Segoe UI"), "Consolas");
        assert_eq!(badge_family(Some("Consolas 14"), "Segoe UI"), "Consolas");
        assert_eq!(badge_family(Some(""), "Segoe UI"), "Segoe UI", "an empty font is no font");
    }
}
