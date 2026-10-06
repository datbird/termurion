//! iTerm2's bell: what a bell does under the profile's switches, the rate limit, the visual
//! (centered icon or whole-pane flash), and the offer to silence a bell that rings too often.
//! `Limiter` holds the floors under every flash and beep (B6).

use std::time::{Duration, Instant};

/// The profile's bell switches and the two stored answers to the offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BellPrefs {
    /// Not "Silence bell".
    pub silence: bool,
    /// "Show bell icon in tabs".
    pub icon: bool,
    /// "Flash visual bell".
    pub flash: bool,
    /// The offer may show (not "Don't Offer Again").
    pub offer: bool,
    /// "Silence Automatically" was chosen once.
    pub auto_silence: bool,
    /// `BellRateLimit`: a bell this soon after the last shown one is quelled.
    pub limit: Duration,
}

/// What a bell does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effects {
    pub beep: bool,
    pub icon: bool,
    pub flash: bool,
}

/// One bell: its effects (None when limited or silenced) and whether to offer silencing it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ring {
    pub effects: Option<Effects>,
    pub offer: bool,
    /// Counts on the taskbar badge: every bell that was not ignored, rate-limited ones too
    /// (`PTYSession.m:19083-19116`).
    pub badge: bool,
}

/// The answers to "The bell is ringing a lot. Silence it?".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    SilenceTemporarily,
    SuppressOutput,
    Never,
    Automatically,
    Dismissed,
}

/// iTerm2's bellRateLimit default.
pub const DEFAULT_LIMIT: Duration = Duration::from_millis(100);
/// How long a flash or the centered indicator fades (`iTermIndicatorsHelper.m:61-65`).
pub const FLASH_SECS: f32 = 0.3;
/// B6: the least gap between two beeps or icons, whatever `BellRateLimit` says.
pub const BELL_FLOOR: Duration = Duration::from_millis(50);
/// B6 and WCAG 2.3.1: any flash (the pane, the centered indicator) at most 3 a second.
pub const VISUAL_FLOOR: Duration = Duration::from_millis(334);
/// B6: the least gap between two taskbar flashes of one window.
pub const TASKBAR_FLASH_FLOOR: Duration = Duration::from_secs(1);

/// How a bell's flash looks (`PTYTextView.m:4500-4509`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visual {
    /// The centered bell icon.
    Indicator,
    /// The whole pane in the text color.
    FullFlash,
}

/// `traditionalVisualBell` picks the whole pane, else the centered icon.
pub fn visual(traditional: bool) -> Visual {
    if traditional { Visual::FullFlash } else { Visual::Indicator }
}

/// The whole-pane flash's alpha `elapsed` seconds in (`iTermIndicatorsHelper.m:477-480`).
pub fn flash_alpha(elapsed: f32, initial: f32) -> f32 {
    (1.0 - elapsed / FLASH_SECS).max(0.0) * initial
}

/// The centered indicator's alpha (`iTermIndicatorsHelper.m:61-65`): it starts at 1.
pub fn indicator_alpha(elapsed: f32) -> f32 {
    (1.0 - elapsed / FLASH_SECS).max(0.0)
}

/// P13: with Windows animations off a whole-pane flash is the centered icon at a fixed alpha.
/// Returns what to draw and whether it fades.
pub fn shown_visual(v: Visual, animations: bool) -> (Visual, bool) {
    if animations { (v, true) } else { (Visual::Indicator, false) }
}

/// B6: the one place the floors live, shared by every source (bells, RequestAttention, the
/// Bounce trigger). Pure: the caller gives the clock.
#[derive(Debug, Default)]
pub struct Limiter {
    beep: Option<Instant>,
    visual: Option<Instant>,
    taskbar: std::collections::HashMap<isize, Instant>,
}

impl Limiter {
    fn pass(last: &mut Option<Instant>, now: Instant, floor: Duration) -> bool {
        if last.is_some_and(|t| now.saturating_duration_since(t) < floor && now >= t) {
            return false;
        }
        *last = Some(now);
        true
    }

    /// A beep or a bell icon may go now.
    pub fn beep(&mut self, now: Instant) -> bool {
        Self::pass(&mut self.beep, now, BELL_FLOOR)
    }

    /// A pane flash or a centered indicator may show now (3 a second at most).
    pub fn visual(&mut self, now: Instant) -> bool {
        Self::pass(&mut self.visual, now, VISUAL_FLOOR)
    }

    /// A taskbar flash of `window` may go now.
    pub fn taskbar(&mut self, window: isize, now: Instant) -> bool {
        if self.taskbar.len() > 64 {
            self.taskbar.retain(|_, t| now.saturating_duration_since(*t) < TASKBAR_FLASH_FLOOR);
        }
        if self.taskbar.get(&window).is_some_and(|t| now >= *t && now.saturating_duration_since(*t) < TASKBAR_FLASH_FLOOR) {
            return false;
        }
        self.taskbar.insert(window, now);
        true
    }
}
/// Gaps shorter than this are not sampled.
const SAMPLE: Duration = Duration::from_millis(10);
/// The average gap below which the bell is annoying.
const ANNOYING: f64 = 0.02;
/// The average starts high (one bell a second takes about 3 s to bring it below 4).
const MAX_GAP: f64 = 20.0;
const ALPHA: f64 = 0.95;
const QUIET_FOR: Duration = Duration::from_secs(60);
const AFTER_DECLINE: Duration = Duration::from_secs(10);

/// A session's bell state.
#[derive(Clone, Debug, Default)]
pub struct Bell {
    last_shown: Option<Instant>,
    last_sample: Option<Instant>,
    /// The moving average of the gaps between bells, and when the last gap started.
    average: Option<f64>,
    timer: Option<Instant>,
    ignore_until: Option<Instant>,
    declined_at: Option<Instant>,
    offer_open: bool,
}

impl Bell {
    pub fn ring(&mut self, now: Instant, prefs: BellPrefs) -> Ring {
        let mut offer = false;
        let ignored = self.ignore_until.is_some_and(|t| now < t) || {
            // iTerm2's shouldIgnoreBell: sample the gaps; an annoying bell is offered once.
            if self.last_sample.is_some_and(|t| now < t + SAMPLE) {
                false
            } else {
                self.last_sample = Some(now);
                let gap = self.timer.map_or(MAX_GAP, |t| now.saturating_duration_since(t).as_secs_f64().min(MAX_GAP));
                self.average = Some(match (self.average, self.timer) {
                    (_, Some(t)) if now.saturating_duration_since(t) > Duration::from_secs(1) => MAX_GAP * ALPHA,
                    (Some(a), _) => ALPHA * a + (1.0 - ALPHA) * gap,
                    (None, _) => gap,
                });
                self.timer = Some(now);
                let annoying = self.average.is_some_and(|a| a < ANNOYING);
                if annoying && prefs.auto_silence {
                    self.ignore_until = Some(now + QUIET_FOR);
                    true
                } else {
                    if annoying && prefs.offer && !self.offer_open && self.declined_at.is_none_or(|d| now.saturating_duration_since(d) > AFTER_DECLINE) {
                        self.offer_open = true;
                        offer = true;
                    }
                    false
                }
            }
        };
        if ignored {
            return Ring { effects: None, offer, badge: false };
        }
        if self.last_shown.is_some_and(|t| now < t + prefs.limit) {
            return Ring { effects: None, offer, badge: true };
        }
        self.last_shown = Some(now);
        Ring { effects: Some(Effects { beep: !prefs.silence, icon: prefs.icon, flash: prefs.flash }), offer, badge: true }
    }

    /// The window could not show the offer (its bar row was full): it may come again.
    pub fn offer_not_shown(&mut self) {
        self.offer_open = false;
    }

    /// The offer's answer. Never and Automatically are saved by the window.
    pub fn answer(&mut self, now: Instant, a: Answer) {
        self.offer_open = false;
        self.average = None;
        self.timer = None;
        match a {
            Answer::SilenceTemporarily | Answer::Automatically => self.ignore_until = Some(now + QUIET_FOR),
            Answer::Dismissed => self.declined_at = Some(now),
            Answer::SuppressOutput | Answer::Never => {}
        }
    }
}

/// The Bell notification posts when the tab's bell icon has just turned on (iTerm2 posts it
/// inside its bell-icon code), so it posts once until the session is seen.
pub fn bell_notification_due(icon_on: bool, newly_marked: bool) -> bool {
    icon_on && newly_marked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const ON: BellPrefs = BellPrefs { silence: false, icon: true, flash: false, offer: true, auto_silence: false, limit: DEFAULT_LIMIT };

    /// `PTYTextView.m:4500-4509`: the centered indicator by default, the whole pane by choice;
    /// `iTermIndicatorsHelper.m:61-65`: the indicator fades linearly over 0.3 s.
    #[test]
    fn visual_bell_follows_iterm2() {
        assert_eq!(visual(false), Visual::Indicator);
        assert_eq!(visual(true), Visual::FullFlash);
        assert_eq!(indicator_alpha(0.0), 1.0);
        assert!((indicator_alpha(0.15) - 0.5).abs() < 1e-6);
        assert_eq!(indicator_alpha(0.4), 0.0);
        assert_eq!(FLASH_SECS, 0.3);
    }

    /// `iTermIndicatorsHelper.m:477-480`: `max(0, 1 - t/0.3) * initial`.
    #[test]
    fn flash_alpha_follows_iterm2() {
        assert_eq!(flash_alpha(0.0, 0.5), 0.5);
        assert!((flash_alpha(0.15, 0.5) - 0.25).abs() < 1e-6);
        assert_eq!(flash_alpha(0.3, 0.5), 0.0);
        assert_eq!(flash_alpha(0.0, 0.8), 0.8);
    }

    /// `PTYSession.m:19083-19116`: a bell inside the limit has no effects but still counts; an
    /// ignored bell (silenced temporarily) counts nothing.
    #[test]
    fn quelled_bells_still_count() {
        let t = Instant::now();
        let mut b = Bell::default();
        let first = b.ring(t, ON);
        assert!(first.effects.is_some() && first.badge);
        let second = b.ring(t + Duration::from_millis(50), ON);
        assert!(second.effects.is_none() && second.badge, "limited, still counted");
        let mut fast = Bell::default();
        fast.ring(t, BellPrefs { limit: Duration::from_millis(10), ..ON });
        assert!(fast.ring(t + Duration::from_millis(50), BellPrefs { limit: Duration::from_millis(10), ..ON }).effects.is_some());
        let mut silenced = Bell::default();
        silenced.answer(t, Answer::SilenceTemporarily);
        let r = silenced.ring(t + Duration::from_secs(1), ON);
        assert!(r.effects.is_none() && !r.badge, "an ignored bell counts nothing");
    }

    /// B6: whatever `BellRateLimit` says, 1 000 bells in one second give at most 3 visuals and
    /// 20 beeps, and every one still counts on the badge. A fake clock; nothing sleeps.
    #[test]
    fn bell_floors_hold() {
        let t0 = Instant::now();
        let (mut b, mut limiter) = (Bell::default(), Limiter::default());
        let prefs = BellPrefs { flash: true, limit: Duration::ZERO, ..ON };
        let (mut beeps, mut visuals, mut counted) = (0, 0, 0);
        for ms in 0..1000u64 {
            let now = t0 + Duration::from_millis(ms);
            let r = b.ring(now, prefs);
            counted += r.badge as u32;
            if let Some(e) = r.effects {
                beeps += (e.beep && limiter.beep(now)) as u32;
                visuals += (e.flash && limiter.visual(now)) as u32;
            }
        }
        assert!(visuals <= 3, "visuals {visuals}");
        assert!(beeps <= 20, "beeps {beeps}");
        assert!(visuals >= 1 && beeps >= 1);
        assert_eq!(counted, 1000, "every bell counts");
        // A flash right after the limiter's last one is refused; after the floor it shows again.
        let now = t0 + Duration::from_secs(1);
        assert!(limiter.visual(now + VISUAL_FLOOR));
        assert!(!limiter.visual(now + VISUAL_FLOOR + Duration::from_millis(1)));
    }

    /// B6: RequestAttention=flash goes through the same visual floor as a bell's flash, so a
    /// bell flash and an attention flash together are still 3 per second; the app's one flash
    /// entry is used by both.
    #[test]
    fn request_attention_flash_is_limited() {
        let t = Instant::now();
        let mut l = Limiter::default();
        assert!(l.visual(t));
        assert!(!l.visual(t + Duration::from_millis(100)), "an attention flash right after a bell flash");
        let ui = include_str!("app/bell_ui.rs").split("\n#[cfg(test)]").next().unwrap();
        let rq = include_str!("app/requests_ui.rs").split("\n#[cfg(test)]").next().unwrap();
        assert!(ui.contains("fn start_flash(") && ui.contains("bell_limiter.visual("));
        assert!(rq.contains("start_flash(") && !rq.contains("flashes.insert"), "attention flashes only through start_flash");
        assert!(!ui.contains("flashes.insert") || ui.matches("flashes.insert").count() == 1);
    }

    /// B6: every `FlashWindowEx` goes through the per-window floor.
    #[test]
    fn taskbar_flash_is_limited() {
        let t = Instant::now();
        let mut l = Limiter::default();
        assert!(l.taskbar(1, t));
        assert!(!l.taskbar(1, t + Duration::from_millis(900)));
        assert!(l.taskbar(2, t + Duration::from_millis(900)), "another window has its own floor");
        assert!(l.taskbar(1, t + TASKBAR_FLASH_FLOOR));
        let rq = include_str!("app/requests_ui.rs").split("\n#[cfg(test)]").next().unwrap();
        let tr = include_str!("app/triggers_ui.rs").split("\n#[cfg(test)]").next().unwrap();
        assert_eq!((tr.matches("FlashWindowEx(").count(), rq.matches("FlashWindowEx(").count()), (1, 0), "one call site");
        assert!(tr.contains("bell_limiter.taskbar(") && rq.contains("flash_taskbar_raw("), "RequestAttention shares the limiter");
        assert!(tr.contains("fn flash_taskbar(&mut self") && tr.matches("flash_taskbar_raw(").count() >= 3);
    }

    /// P13: with Windows animations off, a whole-pane flash becomes the centered indicator at
    /// a fixed alpha; with them on nothing changes.
    #[test]
    fn reduced_motion_swaps_the_flash() {
        assert_eq!(shown_visual(Visual::FullFlash, true), (Visual::FullFlash, true));
        assert_eq!(shown_visual(Visual::FullFlash, false), (Visual::Indicator, false));
        assert_eq!(shown_visual(Visual::Indicator, true), (Visual::Indicator, true));
        assert_eq!(shown_visual(Visual::Indicator, false), (Visual::Indicator, false));
    }

    #[test]
    fn bell_follows_the_profile() {
        let mut b = Bell::default();
        let t = Instant::now();
        assert_eq!(b.ring(t, ON).effects, Some(Effects { beep: true, icon: true, flash: false }));
        let quiet = BellPrefs { silence: true, icon: false, flash: true, ..ON };
        assert_eq!(b.ring(t + Duration::from_secs(1), quiet).effects, Some(Effects { beep: false, icon: false, flash: true }));
    }

    #[test]
    fn bell_limit_and_the_offer() {
        let mut b = Bell::default();
        let t0 = Instant::now();
        let (mut shown, mut offers) = (0, 0);
        for i in 0..3000 {
            let r = b.ring(t0 + Duration::from_millis(i), ON);
            shown += r.effects.is_some() as u32;
            offers += r.offer as u32;
        }
        assert!(shown <= 31, "at most one every 0.1 s: {shown}");
        assert_eq!(offers, 1, "the offer comes once while it shows");
        b.answer(t0 + Duration::from_secs(3), Answer::Dismissed);
        let mut later = 0;
        for i in 0..5000 {
            later += b.ring(t0 + Duration::from_secs(3) + Duration::from_millis(i), ON).offer as u32;
        }
        assert_eq!(later, 0, "no offer for 10 s after a decline");
        let mut b2 = Bell::default();
        let never = BellPrefs { offer: false, ..ON };
        assert!((0..3000).all(|i| !b2.ring(t0 + Duration::from_millis(i), never).offer), "Don't Offer Again");
        let mut b3 = Bell::default();
        let auto = BellPrefs { auto_silence: true, ..ON };
        let rang_late = (0..3000).map(|i| b3.ring(t0 + Duration::from_millis(i), auto)).skip(2500).filter(|r| r.effects.is_some()).count();
        assert_eq!(rang_late, 0, "Silence Automatically quiets it for 60 s");
        let mut b4 = Bell::default();
        b4.answer(t0, Answer::SilenceTemporarily);
        assert!(b4.ring(t0 + Duration::from_secs(59), ON).effects.is_none());
        assert!(b4.ring(t0 + Duration::from_secs(61), ON).effects.is_some());
    }

    #[test]
    fn bel_becomes_a_request() {
        let mut s = crate::screen::Screen::new(20, 4);
        s.feed(b"a\x07b\x07");
        let bells: u32 = s.take_requests().into_iter().map(|r| if let crate::screen::Request::Bell { count } = r { count } else { 0 }).sum();
        assert_eq!(bells, 2);
        assert_eq!(s.bell_count, 2);
    }

    /// B6/P14: the BELs of one feed are one request carrying the count, and fire one event;
    /// `bell_count` still counts each. A flood leaves nothing to wake for afterwards.
    #[test]
    fn bells_in_one_feed_coalesce() {
        let mut s = crate::screen::Screen::new(20, 4);
        s.feed(&[0x07; 65_000]);
        let reqs: Vec<_> = s.take_requests().into_iter().filter(|r| matches!(r, crate::screen::Request::Bell { .. })).collect();
        assert_eq!(reqs.len(), 1, "one request per burst");
        assert!(matches!(reqs[0], crate::screen::Request::Bell { count: 65_000 }));
        assert_eq!(s.bell_count, 65_000);
        assert!(s.take_requests().is_empty(), "nothing is left pending after the flood");
        s.feed(b"a");
        assert!(s.take_requests().iter().all(|r| !matches!(r, crate::screen::Request::Bell { .. })), "no bell without a BEL");
    }

    #[test]
    fn bell_notification_once_until_seen() {
        assert!(bell_notification_due(true, true), "the icon just went on");
        assert!(!bell_notification_due(true, false), "already marked: no second toast");
        assert!(!bell_notification_due(false, true), "no icon, no notification (iTerm2)");
    }

    #[test]
    fn an_offer_not_shown_comes_again() {
        let mut b = Bell::default();
        let t0 = Instant::now();
        let first = (0..3000).position(|i| b.ring(t0 + Duration::from_millis(i), ON).offer).expect("an offer");
        b.offer_not_shown();
        let again = (first as u64 + 1..6000).any(|i| b.ring(t0 + Duration::from_millis(i), ON).offer);
        assert!(again, "a full bar row must not end the offers for good");
    }
}
