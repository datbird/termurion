//! Why the loop woke. A wake that only moves the effect (its frame timer, or the redraw it
//! asked for) needs none of the App's upkeep, which about_to_wait does for any other reason.

#[derive(Default)]
pub struct FxWake {
    fx: bool,
    other: bool,
}

impl FxWake {
    /// The effect's frame timer (or its own redraw) woke the loop.
    pub fn fx_fired(&mut self) {
        self.fx = true;
    }

    /// Anything else woke it: input, a user event, pty output, a resize, focus, a setting, any
    /// other timer.
    pub fn other(&mut self) {
        self.other = true;
    }

    /// True only when the effect was the sole reason since the last take. Resets.
    pub fn take(&mut self) -> bool {
        let only = self.fx && !self.other;
        *self = FxWake::default();
        only
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{FULL_PASS_MAX, FastPass, fx_wake_time};
    use std::time::{Duration, Instant};

    #[test]
    fn only_the_fx_timer_is_fx_only() {
        let mut w = FxWake::default();
        w.fx_fired();
        assert!(w.take());
    }

    #[test]
    fn an_input_wake_is_never_fx_only() {
        let mut w = FxWake::default();
        w.fx_fired();
        w.other();
        assert!(!w.take());
        w.other();
        w.fx_fired();
        assert!(!w.take());
    }

    #[test]
    fn take_resets() {
        let mut w = FxWake::default();
        w.fx_fired();
        assert!(w.take());
        assert!(!w.take());
        w.other();
        assert!(!w.take());
        w.fx_fired();
        assert!(w.take(), "an earlier other does not outlive its take");
    }

    /// The body of `fn name` in the App's handler: up to the next method at the same indent.
    fn body<'a>(src: &'a str, head: &str) -> &'a str {
        let at = src.find(head).unwrap_or_else(|| panic!("{head} not found"));
        let rest = &src[at + head.len()..];
        let end = rest.find("\n    fn ").or_else(|| rest.find("\n    pub fn ")).unwrap_or(rest.len());
        &rest[..end]
    }

    #[test]
    fn every_wake_source_marks_other() {
        let src = include_str!("mod.rs");
        let handler = &src[src.find("impl ApplicationHandler<UserEvent> for App {").unwrap()..];
        // Every window event but the redraw marks, before anything else can return early.
        let win = body(handler, "    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {");
        let call = win.find("self.fx_only.other()").expect("window_event marks other");
        assert!(win[..call].contains("if !matches!(event, WindowEvent::RedrawRequested) || self.gpu.as_ref().is_none_or(|g| g.window.id() != id) {"), "guarded by not-a-redraw of the fx window only: a panel's repaint marks other");
        assert!(call < win.find("return").unwrap(), "marked before any return");
        assert!(call < win.find("self.quitting").unwrap(), "marked before the quitting guard");
        // The redraw arm marks only for a frame that builds a scene: marking always would make every fx frame a full pass.
        let redraw = &win[win.rfind("WindowEvent::RedrawRequested => {").unwrap()..];
        let first = redraw.find("self.fx_only.other()").expect("a built frame is not fx only");
        assert!(redraw[..first].contains("if !reuse {") && redraw.matches("fx_only").count() == 1, "only the not-reused branch marks");
        // User events and the platform's resume: first statement.
        for head in ["    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {", "    fn resumed(&mut self, event_loop: &ActiveEventLoop) {"] {
            let b = body(handler, head);
            assert!(b.trim_start().starts_with("self.fx_only.other();"), "{head} marks other first");
        }
        // new_events marks other unless the cause is the effect's own timer or redraw.
        let ne = body(handler, "    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {");
        assert!(ne.contains("self.fx_only.fx_fired()") && ne.contains("self.fx_only.other()") && ne.contains("!self.web_envs.any()"));
        assert!(ne.contains("fx_wake_at") && ne.contains("only_fx_asked"));
        // A new arm elsewhere cannot skip it: no other method of the handler is a wake source.
        for m in ["fn device_event", "fn suspended", "fn memory_warning", "fn proxy_wake_up"] {
            assert!(!handler.contains(m), "{m} is a wake source that does not mark other");
        }
    }

    #[test]
    fn untracked_queues_wake_the_loop() {
        use crate::webhost::{Posted, Sink, Store};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, mpsc};
        // Behavior: a post wakes the loop, once per post, after the item is queued.
        let woke = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel::<Posted>();
        let w = woke.clone();
        let sink = Sink::new(tx, Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        sink.clone().send(Posted::env(Store::Default, Ok(()))).unwrap();
        sink.send(Posted::env(Store::Default, Ok(()))).unwrap();
        assert_eq!(woke.load(Ordering::SeqCst), 2, "each post wakes the loop");
        assert_eq!(rx.try_iter().count(), 2);
        // Wiring: the App's sink is made with the app's notify, and the view's handler posts through it.
        let app = include_str!("mod.rs");
        assert!(app.contains("webhost_app::Envs::seeded(adblock_app::proxy_args_of(&globals), globals.bool(\"WebKitAdblockEnabled\"), notify.clone())"), "the web host's queue gets the loop's notify");
        let host = include_str!("../webhost.rs");
        assert!(!host.contains("pub type Sink"), "Sink is the waking type, not a bare Sender");
        let win = include_str!("../webhost_win.rs");
        let post = &win[win.find("impl Poster {").unwrap()..];
        assert!(post.contains("self.tx.send(Posted::event(ev, Some(charge)))") && win.contains("tx: Sink,"), "a view's posts go through the Sink");
        // The worker queues the full pass drains whose threads reported nothing before.
        for (file, src) in [("web_replay_app.rs", include_str!("web_replay_app.rs")), ("workgroup_toolbar_ui.rs", include_str!("workgroup_toolbar_ui.rs"))] {
            assert!(src.contains("let notify = self.notify.clone();"), "{file}: its worker thread notifies the loop");
        }
    }

    #[test]
    fn fx_wake_time_is_the_effects_only_when_it_is_earliest_alone() {
        let t = Instant::now();
        let at = |ms| t + Duration::from_millis(ms);
        assert_eq!(fx_wake_time(Some(at(5)), Some(at(5)), Some(at(9)), None, Some(at(7))), Some(at(5)), "earlier than the rest");
        assert_eq!(fx_wake_time(Some(at(5)), Some(at(5)), Some(at(5)), None, None), None, "equal to another wake");
        assert_eq!(fx_wake_time(Some(at(5)), Some(at(5)), None, Some(at(5)), None), None, "equal to the gpu recovery");
        assert_eq!(fx_wake_time(Some(at(3)), Some(at(5)), Some(at(3)), None, None), None, "not the earliest");
        assert_eq!(fx_wake_time(Some(at(5)), None, Some(at(5)), None, None), None, "no effect frame");
    }

    #[test]
    fn the_fast_pass_falls_back_to_the_full_one() {
        let now = Instant::now();
        let ok = FastPass { cached: Some(None), now, full_pass_at: now, parse_owed: false, quitting: false, has_gpu: true, resident: false, front_moved: false };
        assert!(ok.allowed());
        assert!(FastPass { cached: Some(Some(now + Duration::from_millis(50))), ..ok }.allowed(), "a later wake waits");
        for bad in [
            FastPass { cached: None, ..ok },
            FastPass { cached: Some(Some(now)), ..ok },
            FastPass { now: now + FULL_PASS_MAX, ..ok },
            FastPass { parse_owed: true, ..ok },
            FastPass { quitting: true, ..ok },
            FastPass { has_gpu: false, ..ok },
            FastPass { resident: true, ..ok },
            FastPass { front_moved: true, ..ok },
        ] {
            assert!(!bad.allowed());
        }
    }
}
