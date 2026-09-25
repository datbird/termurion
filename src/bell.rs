//! iTerm2's bell: what a bell does under the profile's switches, the 0.1 s limit, and the
//! offer to silence a bell that rings too often.

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

/// iTerm2's bellRateLimit.
const LIMIT: Duration = Duration::from_millis(100);
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
        if ignored || self.last_shown.is_some_and(|t| now < t + LIMIT) {
            return Ring { effects: None, offer };
        }
        self.last_shown = Some(now);
        Ring { effects: Some(Effects { beep: !prefs.silence, icon: prefs.icon, flash: prefs.flash }), offer }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const ON: BellPrefs = BellPrefs { silence: false, icon: true, flash: false, offer: true, auto_silence: false };

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
        let bells = s.take_requests().into_iter().filter(|r| matches!(r, crate::screen::Request::Bell)).count();
        assert_eq!(bells, 2);
        assert_eq!(s.bell_count, 2);
    }
}
