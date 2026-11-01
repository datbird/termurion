//! The bell in the window: the beep, the tab's bell icon, the flash, and the offer to
//! silence a bell that rings too often.

use std::time::{Duration, Instant};

use super::*;
use crate::bell::{Answer, BellPrefs};

/// The most bells of one burst counted on the badge (the label caps at "99+" long before).
const MAX_BURST: u32 = 100_000;
/// The overlay is rebuilt at most this often.
const BADGE_EVERY: Duration = Duration::from_millis(250);
const OFFER_BAR: &str = "annoying-bell";
const SUPPRESS_OFFER: &str = "NoSyncSuppressAnnyoingBellOffer";
const SILENCE_AUTOMATICALLY: &str = "NoSyncSilenceAnnoyingBellAutomatically";

impl App {
    fn bell_prefs(&self, pane: PaneId) -> BellPrefs {
        let p = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile));
        let b = |k: &str| p.is_some_and(|p| p.bool(k));
        BellPrefs {
            silence: b("Silence Bell"),
            icon: b("Visual Bell"),
            flash: b("Flashing Bell"),
            offer: !self.globals.bool(SUPPRESS_OFFER),
            auto_silence: self.globals.bool(SILENCE_AUTOMATICALLY),
            // Bounded at `Globals::set` (finite, at least the beep floor).
            limit: Duration::try_from_secs_f64(self.globals.number("BellRateLimit").unwrap_or(0.1)).unwrap_or(crate::bell::DEFAULT_LIMIT).max(crate::bell::BELL_FLOOR),
        }
    }

    /// Whether Windows plays animations ("Show animations in Windows"; off for reduced motion).
    fn animations_on() -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};
        let mut on: i32 = 1;
        // A failed query leaves the default: animations on.
        unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut on as *mut i32).cast(), 0) };
        on != 0
    }

    /// The one entry for every pane flash (a bell's, RequestAttention's): it passes the visual
    /// floor (3 a second whatever `BellRateLimit` says, WCAG 2.3.1), becomes the fixed centered
    /// icon when Windows animations are off, and records the start in the map the drawing reads.
    pub(super) fn start_flash(&mut self, pane: PaneId, visual: crate::bell::Visual, now: Instant) {
        if !self.bell_limiter.visual(now) {
            return;
        }
        match crate::bell::shown_visual(visual, Self::animations_on()) {
            (crate::bell::Visual::FullFlash, _) => {
                self.flashes.insert(pane, now);
            }
            (crate::bell::Visual::Indicator, fades) => {
                self.bell_indicators.insert(pane, (now, fades));
            }
        }
        self.redraw();
    }

    /// Whether the user is looking at this pane (Termurion in front, its tab shown, focused).
    pub(super) fn looking_at(&self, pane: PaneId) -> bool {
        self.focused && self.tabs.get(self.active).is_some_and(|t| t.focused == pane)
    }

    /// A bell from a program (`count` BELs of one burst) or a Ring Bell trigger (1).
    pub(super) fn ring_bell(&mut self, pane: PaneId, count: u32) {
        let prefs = self.bell_prefs(pane);
        let now = Instant::now();
        let ring = self.bells.entry(pane).or_default().ring(now, prefs);
        crate::dlog!("bell in pane {}: {} bells, effects shown {}", pane, count, ring.effects.is_some());
        if let Some(e) = ring.effects {
            crate::dlog!("bell effects: beep {} icon {} flash {}", e.beep, e.icon, e.flash);
            if e.beep && self.bell_limiter.beep(now) {
                unsafe {
                    windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(windows_sys::Win32::UI::WindowsAndMessaging::MB_OK);
                }
            }
            let marked = e.icon && !self.looking_at(pane) && self.bell_icons.insert(pane);
            if crate::bell::bell_notification_due(e.icon, marked) {
                self.post_alert(pane, crate::alerts::AlertKind::Bell);
            }
            if e.flash {
                let traditional = self.globals.bool("TraditionalVisualBell");
                self.start_flash(pane, crate::bell::visual(traditional), now);
            }
            self.redraw();
        }
        // The taskbar: every bell of the burst that was not ignored counts while Termurion is in the
        // background (quelled ones too, `PTYSession.m:19083-19116`), and a shown effect or a
        // counted bell flashes the button when `BounceOnInactiveBell` is on.
        let mut notified = ring.effects.is_some();
        if ring.badge {
            let enabled = self.globals.bool("IndicateBellsInDockBadgeLabel");
            for _ in 0..count.clamp(1, MAX_BURST) {
                notified |= self.bell_badge.bell(enabled, self.app_active);
            }
        }
        if crate::bell_badge::bounce(notified, self.app_active, self.globals.bool("BounceOnInactiveBell")) {
            // Until Termurion is in front (`NSCriticalRequest`); the one limiter holds the rate.
            self.flash_taskbar(true);
        }
        if ring.offer {
            let text = if prefs.silence { "The bell is ringing a lot. Want to suppress all output until things calm down?" } else { "The bell is ringing a lot. Silence it?" };
            if !self.ask(pane, OFFER_BAR, text, &["_Silence Bell Temporarily", "Suppress _All Output", "Don't Offer Again", "Silence Automatically"], false) {
                self.bells.entry(pane).or_default().offer_not_shown();
            }
        }
    }

    /// The offer's answer. False for other bars.
    pub(super) fn bell_bar(&mut self, pane: PaneId, id: &str, button: Option<&str>) -> bool {
        if id != OFFER_BAR {
            return false;
        }
        let answer = match button {
            Some("_Silence Bell Temporarily") => Answer::SilenceTemporarily,
            Some("Suppress _All Output") => Answer::SuppressOutput,
            Some("Don't Offer Again") => Answer::Never,
            Some("Silence Automatically") => Answer::Automatically,
            _ => Answer::Dismissed,
        };
        match answer {
            Answer::Never => {
                self.globals.set(SUPPRESS_OFFER, toml::Value::Boolean(true));
                self.save_globals();
            }
            Answer::Automatically => {
                self.globals.set(SILENCE_AUTOMATICALLY, toml::Value::Boolean(true));
                self.save_globals();
            }
            Answer::SuppressOutput => {
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.suppress_output = true;
                }
            }
            _ => {}
        }
        self.bells.entry(pane).or_default().answer(Instant::now(), answer);
        true
    }

    /// A pane the user looks at loses its bell icon; with `ClearBellIconAggressively` off only a
    /// key typed in it does (`PT:5052-5054`, `PTYSession.m:4180-4189`).
    pub(super) fn tick_bell(&mut self) {
        if crate::bell_badge::clear_on(self.globals.bool("ClearBellIconAggressively")) != crate::bell_badge::ClearOn::Activation {
            return;
        }
        if let Some(id) = self.tabs.get(self.active).map(|t| t.focused).filter(|_| self.focused)
            && self.bell_icons.remove(&id) {
                self.redraw();
            }
    }

    /// The count goes onto every terminal window's taskbar button when it changed (one badge per
    /// app in iTerm2; Windows has a button per window). Queued, never waited for.
    /// When to come back: a change the throttle held back is pushed then.
    pub(super) fn tick_badge(&mut self) -> Option<Instant> {
        let label = self.bell_badge.label();
        let now = Instant::now();
        match crate::bell_badge::badge_step(self.badge_pushed.as_ref().map(|(l, t)| (l.as_str(), *t)), &label, now, BADGE_EVERY) {
            crate::bell_badge::BadgeStep::Idle => return None,
            crate::bell_badge::BadgeStep::Wait(at) => return Some(at),
            crate::bell_badge::BadgeStep::Push => {}
        }
        let hwnds: Vec<isize> = self.hwnd().into_iter().chain(self.windows.parked().filter_map(|(_, s)| s.gpu.as_ref().and_then(|g| super::menu::hwnd_of(&g.window)))).collect();
        for h in hwnds {
            // A queue that cannot take the job leaves the button as it was; the next change retries.
            let _ = crate::platform::shell_extras::taskbar_progress(h, &label);
        }
        self.badge_pushed = Some((label, now));
        None
    }

    /// A window's taskbar button hears `TaskbarButtonCreated`, and the next pass sets its badge.
    pub(super) fn taskbar_watch(&mut self, hwnd: isize) {
        crate::platform::shell_extras::taskbar_watch(hwnd);
        self.badge_pushed = None;
    }
}

#[cfg(test)]
mod tests {
    fn body<'a>(src: &'a str, from: &str, to: &str) -> &'a str {
        let at = src.find(from).unwrap_or_else(|| panic!("{from}"));
        let rest = &src[at..];
        &rest[..rest.find(to).unwrap_or(rest.len())]
    }

    /// The taskbar is wired where the brief says: the burst's count, the one flash limiter, the
    /// three subclass sites, the activation edges, the tab selection and the teardown.
    #[test]
    fn taskbar_is_wired() {
        let bell = include_str!("bell_ui.rs");
        let ring = body(bell, "pub(super) fn ring_bell", "if ring.offer");
        assert!(ring.contains("count.clamp(1, MAX_BURST)"), "the badge adds the burst's count");
        assert!(ring.contains("self.flash_taskbar(true)"), "bounce goes through the shared flash path");
        assert!(include_str!("triggers_ui.rs").matches("FlashWindowEx(").count() == 1, "one FlashWindowEx");
        for f in ["windows_ui.rs", "transparency_ui.rs", "mod.rs"] {
            assert!(include_str!("mod.rs").contains("taskbar_watch") && (f == "mod.rs" || std::fs::read_to_string(format!("{}/src/app/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap().contains("self.taskbar_watch(h)")), "{f}");
        }
        let m = include_str!("mod.rs");
        assert!(m.contains("let badge_due = self.tick_badge();") && m.contains("self.attention_due, badge_due,"), "a held-back badge change wakes the loop");
        assert!(body(m, "pub(crate) fn teardown_process()", "\n}\n").contains("shell_extras::taskbar_clear_all"));
        assert!(body(m, "pub(super) fn select_tab", "\n    }\n").contains("tab_selected"));
        assert!(body(m, "let active = hotkey_ui::termurion_in_front();", "self.hotkey_reactivated()").contains("became_active"));
        assert!(include_str!("alerts_ui.rs").contains("bell_badge::is_waiting"));
        assert!(include_str!("marks_ui.rs").contains("taskbar_win::play_named") && include_str!("marks_ui.rs").contains("sound_for"));
    }

    /// Typing clears a bell icon; with ClearBellIconAggressively off the tick leaves it.
    #[test]
    fn bell_icon_clearing_is_wired() {
        let bell = include_str!("bell_ui.rs");
        assert!(body(bell, "pub(super) fn tick_bell", "\n    }\n").contains("clear_on(self.globals.bool(\"ClearBellIconAggressively\"))"));
        let w = body(include_str!("broadcast_ui.rs"), "fn write_to_domain(", "tmux_send_input");
        assert!(w.contains("!program && self.bell_icons.remove(&t)"));
    }
}
