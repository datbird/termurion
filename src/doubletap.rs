//! A modifier double tap (iTerm2's DoubleTapHotkeyStateMachine), fed by the low-level
//! keyboard hook. Windows sends repeated key-downs while a key is held, reports left and
//! right keys apart, and sends AltGr as a fake left Control then right Alt; the machine
//! sees only real changes of which modifiers are held.

use std::collections::HashSet;
use std::time::Duration;

/// iTerm2's defaults (hotKeyDoubleTapMinDelay, hotKeyDoubleTapMaxDelay).
pub const MIN_DELAY: Duration = Duration::from_millis(10);
pub const MAX_DELAY: Duration = Duration::from_millis(300);

/// One hook event: the virtual key, down or up, its time, and the scan code (AltGr's fake
/// Control has bit 0x200 set).
#[derive(Clone, Copy, Debug)]
pub struct Ev {
    pub vk: u32,
    pub down: bool,
    pub time: Duration,
    pub scan: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    FirstDown,
    FirstUp,
    SecondDown,
}

pub struct Machine {
    /// 0 Control, 1 Shift, 2 Alt, 3 Win.
    modifier: u8,
    min: Duration,
    max: Duration,
    state: State,
    last_change: Duration,
    held: HashSet<u32>,
}

/// The modifier a virtual key is (0 Control, 1 Shift, 2 Alt, 3 Win), left or right alike.
pub fn modifier_of(vk: u32) -> Option<u8> {
    match vk {
        0x11 | 0xA2 | 0xA3 => Some(0),
        0x10 | 0xA0 | 0xA1 => Some(1),
        0x12 | 0xA4 | 0xA5 => Some(2),
        0x5B | 0x5C => Some(3),
        _ => None,
    }
}

impl Machine {
    pub fn new(modifier: u8, min: Duration, max: Duration) -> Machine {
        Machine { modifier, min, max, state: State::Ground, last_change: Duration::ZERO, held: HashSet::new() }
    }

    /// Feeds one event. True when it completes a double tap.
    pub fn event(&mut self, e: Ev) -> bool {
        let Some(_) = modifier_of(e.vk) else {
            // Any other key between the taps: "Control, A, Control" is no double tap.
            if e.down {
                self.set(State::Ground, e.time);
            }
            return false;
        };
        // AltGr's fake left Control is not a Control press.
        if e.vk == 0xA2 && e.scan & 0x200 != 0 {
            return false;
        }
        if e.down {
            // A repeat of a held key changes nothing.
            if !self.held.insert(e.vk) {
                return false;
            }
        } else if !self.held.remove(&e.vk) {
            return false;
        }
        let kinds: HashSet<u8> = self.held.iter().filter_map(|vk| modifier_of(*vk)).collect();
        let only = kinds.len() == 1 && kinds.contains(&self.modifier);
        let any = !kinds.is_empty();
        self.step(only, any, e.time)
    }

    fn set(&mut self, s: State, now: Duration) {
        self.state = s;
        self.last_change = now;
    }

    fn timely(&self, now: Duration) -> bool {
        let gap = now.saturating_sub(self.last_change);
        gap >= self.min && gap < self.max.max(self.min)
    }

    /// iTerm2's machine, one change of the held modifiers at a time.
    fn step(&mut self, only: bool, any: bool, now: Duration) -> bool {
        match self.state {
            State::Ground => {
                if only {
                    self.set(State::FirstDown, now);
                }
                false
            }
            State::FirstDown => self.key_up(only, any, now, Some(State::FirstUp)),
            State::SecondDown => self.key_up(only, any, now, None),
            State::FirstUp => {
                if !self.timely(now) {
                    self.set(State::Ground, now);
                    return self.step(only, any, now);
                }
                if !any {
                    return false;
                }
                if !only {
                    self.set(State::Ground, now);
                    return false;
                }
                self.set(State::SecondDown, now);
                false
            }
        }
    }

    fn key_up(&mut self, only: bool, any: bool, now: Duration, next: Option<State>) -> bool {
        if !self.timely(now) || !(only || !any) {
            self.set(State::Ground, now);
            return false;
        }
        if any {
            return false;
        }
        match next {
            Some(n) => {
                self.set(n, now);
                false
            }
            None => {
                self.set(State::Ground, now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LCTRL: u32 = 0xA2;
    const RCTRL: u32 = 0xA3;
    const LSHIFT: u32 = 0xA0;
    const RMENU: u32 = 0xA5;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn feed(m: &mut Machine, events: &[(u32, bool, u64)]) -> Vec<bool> {
        events.iter().map(|&(vk, down, t)| m.event(Ev { vk, down, time: ms(t), scan: 0x1D })).collect()
    }

    fn ctrl() -> Machine {
        Machine::new(0, MIN_DELAY, MAX_DELAY)
    }

    #[test]
    fn double_tap_fires_on_second_release() {
        let mut m = ctrl();
        assert_eq!(feed(&mut m, &[(LCTRL, true, 1000), (LCTRL, false, 1050), (LCTRL, true, 1100), (LCTRL, false, 1150)]), vec![false, false, false, true]);
        // Left then right Control is still Control.
        let mut m = ctrl();
        assert_eq!(feed(&mut m, &[(LCTRL, true, 0 + 1000), (LCTRL, false, 1050), (RCTRL, true, 1100), (RCTRL, false, 1150)]).last(), Some(&true));
        // Again right after: a fresh double tap.
        assert_eq!(feed(&mut m, &[(LCTRL, true, 1300), (LCTRL, false, 1350), (LCTRL, true, 1400), (LCTRL, false, 1450)]).last(), Some(&true));
        assert_eq!((modifier_of(0x5B), modifier_of(0x5C), modifier_of(0x12), modifier_of(0x41)), (Some(3), Some(3), Some(2), None));
    }

    #[test]
    fn slow_or_mixed_taps_reset() {
        let mut m = ctrl();
        assert!(!feed(&mut m, &[(LCTRL, true, 1000), (LCTRL, false, 1050), (LCTRL, true, 1500), (LCTRL, false, 1550)]).contains(&true), "a slow second press");
        // ...but that slow press starts a new tap.
        assert_eq!(feed(&mut m, &[(LCTRL, true, 1600), (LCTRL, false, 1650)]).last(), Some(&true));
        let mut m = ctrl();
        assert!(!feed(&mut m, &[(LCTRL, true, 1000), (LCTRL, false, 1003), (LCTRL, true, 1006), (LCTRL, false, 1009)]).contains(&true), "too fast");
        let mut m = ctrl();
        assert!(!feed(&mut m, &[(LCTRL, true, 1000), (LCTRL, false, 1050), (LCTRL, true, 1100), (LSHIFT, true, 1120), (LSHIFT, false, 1140), (LCTRL, false, 1150)]).contains(&true), "Shift joined the second press");
        let mut m = ctrl();
        assert!(!feed(&mut m, &[(LCTRL, true, 1000), (LCTRL, false, 1050), (0x41, true, 1070), (0x41, false, 1080), (LCTRL, true, 1100), (LCTRL, false, 1150)]).contains(&true), "a letter between");
        let mut s = Machine::new(1, MIN_DELAY, MAX_DELAY);
        assert!(!feed(&mut s, &[(LCTRL, true, 1000), (LCTRL, false, 1050), (LCTRL, true, 1100), (LCTRL, false, 1150)]).contains(&true), "Control taps do not fire a Shift machine");
    }

    #[test]
    fn repeats_and_altgr_never_fire() {
        // Holding Control: Windows repeats the key-down; the release comes too late.
        let mut m = ctrl();
        let mut held: Vec<(u32, bool, u64)> = (0..30).map(|i| (LCTRL, true, 1000 + i * 33)).collect();
        held.push((LCTRL, false, 2000));
        assert!(!feed(&mut m, &held).contains(&true));
        // Repeats inside a quick tap do not break it.
        let mut m = ctrl();
        assert_eq!(feed(&mut m, &[(LCTRL, true, 1000), (LCTRL, true, 1030), (LCTRL, false, 1050), (LCTRL, true, 1100), (LCTRL, true, 1120), (LCTRL, false, 1150)]).last(), Some(&true));
        // AltGr twice: a fake left Control (scan 0x21D) with right Alt.
        let mut m = ctrl();
        let mut fired = false;
        for t in [1000u64, 1100] {
            for (vk, down, dt, scan) in [(LCTRL, true, 0, 0x21D), (RMENU, true, 0, 0x38), (RMENU, false, 40, 0x38), (LCTRL, false, 40, 0x21D)] {
                fired |= m.event(Ev { vk, down, time: ms(t + dt), scan });
            }
        }
        assert!(!fired, "AltGr is not Control");
    }
}
