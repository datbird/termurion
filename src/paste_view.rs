//! The paste progress view (`PasteView.xib`, `MinimalPasteView.xib`, `PasteViewController.m`,
//! `iTermPasteHelper.m`): "Pasting…", the progress bar, Cancel, and while a paste waits for the
//! shell prompt the keyboard button that lets keystrokes through to the terminal instead of
//! queueing them, with its hint. Pure: the texts and the rules; the drawing is `scene::paste_view`.

use std::time::{Duration, Instant};

/// A key held during a paste, with the modifiers down when it was typed (`M`), so it is replayed
/// with those and not with whatever is held when the paste ends.
#[derive(Clone, Debug, PartialEq)]
pub struct Held<E, M> {
    pub key: E,
    pub mods: M,
    pub phys_mods: M,
}

/// `PasteView.xib:25`; the label of an upload is "Sending…" (`PasteViewController.m:97`).
pub const PASTING: &str = "Pasting\u{2026}";
pub const SENDING: &str = "Sending\u{2026}";
/// `PasteViewController.m:139`, `:143-144`, `:218`.
pub const KEYSTROKES_ACCESSIBILITY: &str = "Send keystrokes to terminal";
pub const KEYSTROKES_TOOLTIP: &str = "Type directly to the terminal (for example to answer a password prompt) instead of queueing your keystrokes until the paste finishes.";
pub const QUEUED_HINT: &str = "Typing is queued while pasting. Click the keyboard to toggle queueing.";
/// How long the hint stays (`PasteViewController.m:250`).
pub const HINT_SECONDS: f64 = 4.0;
/// The widest the hint's callout is (`PasteViewController.m:215`).
pub const HINT_MAX_WIDTH: f32 = 220.0;
/// The gap between the keyboard button and Cancel (`kKeystrokePassthroughButtonGap`).
pub const BUTTON_GAP: f32 = 2.0;

/// `iTermPasteQueuedHintView.swift` `Metrics` (points, seconds): the animations run at
/// `animationSpeedup` times their durations' rate.
pub const CORNER_RADIUS: f32 = 7.0;
pub const POINTER_HEIGHT: f32 = 7.0;
pub const POINTER_HALF_WIDTH: f32 = 7.0;
pub const PADDING_H: f32 = 10.0;
pub const PADDING_V: f32 = 7.0;
pub const SPRING_TRAVEL: f32 = 14.0;
const SPEEDUP: f64 = 1.0 / 0.7;
const FADE_IN: f64 = 0.18;
const FADE_OUT: f64 = 0.14;
pub const SPRING_STIFFNESS: f64 = 320.0;
pub const SPRING_DAMPING: f64 = 15.0;

/// The spring's progress at `t` seconds of its own clock (mass 1, stiffness 320, damping 15,
/// starting at rest): 0 at the start, settling at 1 after overshooting (`CASpringAnimation`).
pub fn spring(t: f64) -> f64 {
    let w = SPRING_STIFFNESS.sqrt();
    let zeta = SPRING_DAMPING / (2.0 * w);
    let wd = w * (1.0 - zeta * zeta).sqrt();
    1.0 - (-zeta * w * t).exp() * ((wd * t).cos() + zeta * w / wd * (wd * t).sin())
}

/// The callout's drop shadow (`iTermPasteQueuedHintView.swift`): black at 0.35, blur radius 8,
/// 2 points down.
pub const SHADOW_ALPHA: f32 = 0.35;
pub const SHADOW_BLUR: f32 = 8.0;
pub const SHADOW_DROP: f32 = 2.0;

/// The shadow as nested layers, outermost first: `(points grown from the box, alpha)`. The
/// shadow's alpha falls off from `SHADOW_ALPHA` at the edge to nothing `SHADOW_BLUR` away, and
/// each layer adds the part of that fall between its edge and the next one in.
pub fn shadow_layers() -> Vec<(f32, f32)> {
    let n = SHADOW_BLUR as usize;
    let at = |d: f32| SHADOW_ALPHA * (1.0 - d / SHADOW_BLUR).max(0.0).powi(2);
    (1..=n).rev().map(|k| (k as f32, at(k as f32 - 1.0) - at(k as f32))).collect()
}

/// How long the paste view slides and fades in (`updateFrame`) and fades out
/// (`closeWithCompletion:`): `kAnimationDuration`, `PasteViewController.m:21`.
pub const VIEW_SECONDS: f64 = 0.25;

/// The animation curve (an ease in and out, Core Animation's default).
pub fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How the paste view looks at an instant: how opaque, and how much of its own height it
/// still has to slide down from above the pane (1 at the start of opening, 0 at rest).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewLook {
    pub alpha: f32,
    pub rise: f32,
}

pub const AT_REST: ViewLook = ViewLook { alpha: 1.0, rise: 0.0 };

/// The view `age` seconds after it was added: it comes down from above the pane while it fades in.
pub fn opening(age: f64) -> ViewLook {
    let p = ease(age / VIEW_SECONDS) as f32;
    ViewLook { alpha: p, rise: 1.0 - p }
}

/// The view `age` seconds after the paste ended: it fades out where it is; None once gone.
pub fn closing(age: f64) -> Option<ViewLook> {
    (age < VIEW_SECONDS).then(|| ViewLook { alpha: 1.0 - ease(age / VIEW_SECONDS) as f32, rise: 0.0 })
}

/// One pane's paste view animation: when it opened, the fraction it last showed, and once the
/// paste ended when it started to fade out.
#[derive(Clone, Copy, Debug)]
pub struct Anim {
    pub opened: Instant,
    pub fraction: f32,
    pub closed: Option<Instant>,
}

impl Anim {
    pub fn new(now: Instant) -> Anim {
        Anim { opened: now, fraction: 0.0, closed: None }
    }

    /// One tick: the view is on screen (`live`) with `fraction` done, or is not. False when
    /// nothing is left to draw: remove the animation.
    pub fn step(&mut self, now: Instant, live: bool, fraction: f32) -> bool {
        if live {
            if self.closed.is_some() {
                *self = Anim::new(now);
            }
            self.fraction = fraction;
            return true;
        }
        match self.closed {
            None => {
                self.closed = Some(now);
                true
            }
            Some(since) => closing(now.saturating_duration_since(since).as_secs_f64()).is_some(),
        }
    }

    /// How the view looks now; None when it has faded out.
    pub fn look(&self, now: Instant) -> Option<ViewLook> {
        match self.closed {
            None => Some(opening(now.saturating_duration_since(self.opened).as_secs_f64())),
            Some(since) => closing(now.saturating_duration_since(since).as_secs_f64()),
        }
    }

    /// Still moving: a frame is due.
    pub fn animating(&self, now: Instant) -> bool {
        match self.closed {
            None => now.saturating_duration_since(self.opened).as_secs_f64() < VIEW_SECONDS,
            Some(since) => closing(now.saturating_duration_since(since).as_secs_f64()).is_some(),
        }
    }
}

/// How the hint looks at an instant: how opaque, and how far it still is from its resting place
/// toward the button (a fraction of `SPRING_TRAVEL`, 1 at the start, negative past the end).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HintLook {
    pub alpha: f32,
    pub offset: f32,
}

/// One pane's view state: each paste starts with keystrokes queued (`:702`).
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    pub passthrough: bool,
    /// When the hint was presented (the spring starts there) and when it leaves.
    presented: Option<Instant>,
    leaves: Option<Instant>,
}

impl State {
    /// The keyboard button was clicked: the hint goes, and the new state is returned.
    pub fn toggle(&mut self, now: Instant) -> bool {
        self.hide(now);
        self.passthrough = !self.passthrough;
        self.passthrough
    }

    /// Typing while the paste waits for the prompt and keystrokes are queued shows the hint
    /// (`enqueueEvent:`, `iTermPasteHelper.m:570-575`). One already showing stays where it is and
    /// its four seconds start again; one fading out is presented anew.
    pub fn typed(&mut self, now: Instant, waiting_for_prompt: bool) {
        if !waiting_for_prompt || self.passthrough {
            return;
        }
        if self.look(now).is_none() {
            self.presented = Some(now);
        }
        self.leaves = Some(now + Duration::from_secs_f64(HINT_SECONDS));
    }

    /// The hint starts fading out now (`hideKeystrokeQueuedHint`).
    pub fn hide(&mut self, now: Instant) {
        if self.look(now).is_some_and(|l| l.alpha > 0.0) && self.leaves.is_some_and(|t| t > now) {
            self.leaves = Some(now);
        }
    }

    pub fn hint_shown(&self, now: Instant) -> bool {
        self.look(now).is_some()
    }

    /// The hint's opacity and offset at `now`, None when it is not on screen.
    pub fn look(&self, now: Instant) -> Option<HintLook> {
        let (since, leaves) = (self.presented?, self.leaves?);
        let age = now.saturating_duration_since(since).as_secs_f64();
        let fade_in = (age / (FADE_IN / SPEEDUP)).min(1.0);
        let out = if now > leaves { now.saturating_duration_since(leaves).as_secs_f64() / (FADE_OUT / SPEEDUP) } else { 0.0 };
        if out >= 1.0 {
            return None;
        }
        let alpha = (fade_in * (1.0 - out)) as f32;
        let offset = (1.0 - spring(age * SPEEDUP)) as f32;
        Some(HintLook { alpha, offset })
    }

    /// The paste stopped waiting: the hint has nothing to point at (`setWaitingForPrompt:NO`).
    pub fn stopped_waiting(&mut self, now: Instant) {
        self.hide(now);
    }
}

/// Whether a key aborts a paste that waits for the prompt (`eventAbortsPasteWaitingForPrompt:`,
/// `PTYSession.m:11628-11650`): Esc or Ctrl+C, unless keystrokes pass through.
pub fn aborts(waiting_for_prompt: bool, passthrough: bool, escape: bool, ctrl_c: bool) -> bool {
    waiting_for_prompt && !passthrough && (escape || ctrl_c)
}

/// Whether a typed key is held until the paste ends (`PTYTextView.m:7510`).
pub fn queues_keys(pasting: bool, passthrough: bool) -> bool {
    pasting && !passthrough
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_held_key_keeps_the_modifiers_it_was_typed_with() {
        let mut shift = crate::mouse::Mods { shift: true, ..Default::default() };
        let mut queue = Vec::new();
        queue.push(super::Held { key: 'a', mods: shift, phys_mods: shift });
        shift.shift = false; // released before the paste ends
        queue.push(super::Held { key: 'b', mods: shift, phys_mods: shift });
        assert!(queue[0].mods.shift && queue[0].phys_mods.shift);
        assert!(!queue[1].mods.shift);
    }

    use super::*;

    #[test]
    fn texts_are_iterm2s() {
        assert_eq!(PASTING, "Pasting\u{2026}");
        assert_eq!(SENDING, "Sending\u{2026}");
        assert_eq!(QUEUED_HINT, "Typing is queued while pasting. Click the keyboard to toggle queueing.");
        assert_eq!(KEYSTROKES_ACCESSIBILITY, "Send keystrokes to terminal");
        assert!(KEYSTROKES_TOOLTIP.starts_with("Type directly to the terminal (for example to answer a password prompt)"));
    }

    #[test]
    fn the_hint_shows_four_seconds_only_while_waiting_and_queueing() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut s = State::default();
        s.typed(t0, false);
        assert!(!s.hint_shown(t0), "no hint when not waiting for the prompt");
        s.typed(t0, true);
        assert!(s.hint_shown(at(3.0)));
        assert!(s.hint_shown(at(4.05)), "it fades out over a moment after the four seconds");
        assert!(!s.hint_shown(at(4.2)));
        s.typed(at(3.0), true);
        assert!(s.hint_shown(at(6.9)), "typing again starts the four seconds again");
        assert!(s.toggle(at(3.5)), "on");
        assert!(!s.hint_shown(at(3.8)), "engaging the control removes the hint");
        s.typed(at(4.0), true);
        assert!(!s.hint_shown(at(4.0)), "passthrough keystrokes are not held, so no hint");
        assert!(!s.toggle(at(4.0)));
        s.typed(at(5.0), true);
        s.stopped_waiting(at(5.5));
        assert!(!s.hint_shown(at(6.0)));
    }

    #[test]
    fn the_hint_springs_in_and_overshoots() {
        assert!(spring(0.0).abs() < 1e-9);
        assert!(spring(0.2) > 1.0, "it overshoots its resting place");
        assert!((spring(3.0) - 1.0).abs() < 1e-3, "and settles");
        let t0 = Instant::now();
        let mut s = State::default();
        s.typed(t0, true);
        let start = s.look(t0).unwrap();
        assert_eq!((start.alpha, start.offset), (0.0, 1.0));
        let mid = s.look(t0 + Duration::from_millis(60)).unwrap();
        assert!(mid.alpha > 0.0 && mid.offset < 1.0);
    }

    #[test]
    fn escape_and_ctrl_c_abort_a_waiting_paste_unless_keys_pass_through() {
        assert!(aborts(true, false, true, false) && aborts(true, false, false, true));
        assert!(!aborts(true, true, true, false), "passthrough: Esc reaches the shell");
        assert!(!aborts(false, false, true, false), "only while waiting for the prompt");
        assert!(queues_keys(true, false) && !queues_keys(true, true) && !queues_keys(false, false));
    }

    #[test]
    fn the_view_slides_and_fades_in_over_a_quarter_second() {
        assert_eq!(VIEW_SECONDS, 0.25);
        assert_eq!(opening(0.0), ViewLook { alpha: 0.0, rise: 1.0 });
        assert_eq!(opening(0.25), AT_REST);
        assert_eq!(opening(5.0), AT_REST);
        let mid = opening(0.125);
        assert!((mid.alpha - 0.5).abs() < 1e-6 && (mid.rise - 0.5).abs() < 1e-6, "{mid:?}");
        assert!(opening(0.05).alpha < opening(0.1).alpha, "it only gets more opaque");
    }

    #[test]
    fn the_view_fades_out_in_place_and_goes() {
        assert_eq!(closing(0.0), Some(ViewLook { alpha: 1.0, rise: 0.0 }));
        assert!(closing(0.2).is_some_and(|l| l.alpha < 0.2 && l.rise == 0.0));
        assert_eq!(closing(0.25), None);
    }

    #[test]
    fn an_animation_opens_ends_fades_and_is_forgotten() {
        let t0 = Instant::now();
        let at = |s: f64| t0 + Duration::from_secs_f64(s);
        let mut a = Anim::new(t0);
        assert!(a.step(at(0.1), true, 0.4));
        assert!(a.animating(at(0.1)) && !a.animating(at(0.3)), "opening takes a quarter second");
        assert_eq!(a.look(at(1.0)), Some(AT_REST));
        // The paste ended: it fades from the fraction it last showed.
        assert!(a.step(at(2.0), false, 0.0));
        assert_eq!(a.fraction, 0.4);
        assert!(a.look(at(2.1)).is_some_and(|l| l.alpha < 1.0 && l.alpha > 0.0));
        assert!(a.step(at(2.2), false, 0.0));
        assert!(!a.step(at(2.3), false, 0.0), "gone after the fade");
        assert_eq!(a.look(at(2.3)), None);
        // A new paste on a fading view starts again.
        let mut b = Anim::new(t0);
        b.step(at(1.0), false, 0.0);
        assert!(b.step(at(1.1), true, 0.2));
        assert!(b.closed.is_none() && b.opened == at(1.1));
    }

    #[test]
    fn the_hint_shadow_falls_off_like_a_blur_of_eight() {
        let layers = shadow_layers();
        assert_eq!(layers.len(), 8);
        assert_eq!(layers[0].0, 8.0, "outermost first");
        assert_eq!(layers[7].0, 1.0);
        // At the box's edge the layers add up to about 0.35; the sum of the alphas.
        let total: f32 = layers.iter().map(|l| l.1).sum();
        assert!((total - SHADOW_ALPHA).abs() < 1e-4, "{total}");
        assert!(layers.iter().all(|l| l.1 > 0.0));
        assert_eq!(SHADOW_DROP, 2.0);
    }
}
