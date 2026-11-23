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
        assert!(win[..call].contains("if !matches!(event, WindowEvent::RedrawRequested) {"), "the call is guarded by not-a-redraw only");
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
        assert!(ne.contains("self.fx_only.fx_fired()") && ne.contains("self.fx_only.other()"));
        assert!(ne.contains("fx_wake_at") && ne.contains("only_fx_asked"));
        // A new arm elsewhere cannot skip it: no other method of the handler is a wake source.
        for m in ["fn device_event", "fn suspended", "fn memory_warning", "fn proxy_wake_up"] {
            assert!(!handler.contains(m), "{m} is a wake source that does not mark other");
        }
    }
}
