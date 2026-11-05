//! The hotkey window's native calls on macOS. A handle is the window's content view
//! (`window::view_of`) when positive and minus the pid of another app when negative. A view is
//! never touched until it is found among the app's own windows, so a stale handle is just refused.

use std::collections::HashMap;
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_foundation::NSRect;
use objc2_app_kit::{NSScreen, NSApplication, NSApplicationActivationOptions, NSRunningApplication, NSWindow, NSWindowCollectionBehavior, NSWorkspace};

use crate::layout::Rect;
use crate::platform::Unsupported;

/// What a foreground handle names.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Token {
    View(isize),
    App(i32),
    Nothing,
}

/// Positive is one of our views, negative an app's pid, zero nothing.
pub fn token_kind(t: isize) -> Token {
    match t {
        0 => Token::Nothing,
        t if t > 0 => Token::View(t),
        t => Token::App(t.unsigned_abs() as i32),
    }
}

/// The alpha a window shows: nothing while cloaked, else the fade's.
fn shown_alpha(cloaked: bool, fade: f32) -> f64 {
    if cloaked { 0.0 } else { fade.clamp(0.0, 1.0) as f64 }
}

/// Per window: (cloaked, fade alpha).
static STATE: Mutex<Option<HashMap<isize, (bool, f32)>>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut HashMap<isize, (bool, f32)>) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

/// The window is going away: its cloak and fade are dropped, so a new window at the same address
/// starts clean.
pub fn forget(view: isize) {
    with_state(|m| m.remove(&view));
}

/// The window of ours whose content view is `view`.
fn window_of(view: isize) -> Option<Retained<NSWindow>> {
    let mtm = MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(mtm);
    app.windows().iter().find(|w| w.contentView().is_some_and(|v| Retained::as_ptr(&v) as isize == view))
}

pub fn available() -> Result<(), Unsupported> {
    Ok(())
}

/// Hides the window by opacity (it also stops taking the mouse), or shows it at the fade alpha.
pub fn cloak(view: isize, on: bool) {
    let Some(w) = window_of(view) else { return };
    let fade = with_state(|m| {
        let e = m.entry(view).or_insert((false, 1.0));
        e.0 = on;
        e.1
    });
    w.setAlphaValue(shown_alpha(on, fade));
    w.setIgnoresMouseEvents(on);
}

/// A fade step: kept, and shown unless the window is cloaked. False when there is no window.
pub fn set_alpha(view: isize, a: f32) -> bool {
    let Some(w) = window_of(view) else { return false };
    let cloaked = with_state(|m| {
        let e = m.entry(view).or_insert((false, 1.0));
        e.1 = a;
        e.0
    });
    w.setAlphaValue(shown_alpha(cloaked, a));
    true
}

/// Our key window's view while Termurion is active, else minus the front app's pid, else 0.
pub fn foreground() -> isize {
    let Some(mtm) = MainThreadMarker::new() else { return 0 };
    let app = NSApplication::sharedApplication(mtm);
    if app.isActive() {
        return app.keyWindow().and_then(|w| w.contentView()).map_or(0, |v| Retained::as_ptr(&v) as isize);
    }
    NSWorkspace::sharedWorkspace().frontmostApplication().map_or(0, |a| -(a.processIdentifier() as isize))
}

pub fn ours(t: isize) -> bool {
    matches!(token_kind(t), Token::View(v) if window_of(v).is_some())
}

pub fn is_window(t: isize) -> bool {
    match token_kind(t) {
        Token::View(v) => window_of(v).is_some(),
        Token::App(pid) => NSRunningApplication::runningApplicationWithProcessIdentifier(pid).is_some_and(|a| !a.isTerminated()),
        Token::Nothing => false,
    }
}

/// On top of the others without activating; a minimized window comes back first.
pub fn order_front(view: isize, minimized: bool) {
    let Some(w) = window_of(view) else { return };
    if minimized {
        w.deminiaturize(None);
    }
    w.orderFront(None);
}

/// The app is activated by the show path already, so there is nothing more to ask.
pub fn take_focus(_view: isize) {}

/// The window shows on the Space the person is on, also over a full screen app (iTerm2's hotkey window).
pub fn join_active_space(view: isize) {
    let Some(w) = window_of(view) else { return };
    let kept = w.collectionBehavior();
    w.setCollectionBehavior(kept | NSWindowCollectionBehavior::MoveToActiveSpace | NSWindowCollectionBehavior::FullScreenAuxiliary);
}

/// The screen the window is on, as (work area, whole area) in pixels with the origin at the top left
/// of the main screen (AppKit counts from the bottom left, in points).
pub fn monitor_rects(view: isize) -> Option<(Rect, Rect)> {
    let w = window_of(view)?;
    let screen = w.screen()?;
    let mtm = MainThreadMarker::new()?;
    let main_h = NSScreen::screens(mtm).firstObject()?.frame().size.height;
    let k = screen.backingScaleFactor();
    Some((flip(screen.visibleFrame(), main_h, k), flip(screen.frame(), main_h, k)))
}

fn flip(r: NSRect, main_h: f64, k: f64) -> Rect {
    Rect { x: (r.origin.x * k) as f32, y: ((main_h - r.origin.y - r.size.height) * k) as f32, w: (r.size.width * k) as f32, h: (r.size.height * k) as f32 }
}

/// Activates the app of a negative handle, or makes our window key. False when it could not.
pub fn bring_front(t: isize) -> bool {
    match token_kind(t) {
        Token::App(pid) => {
            let Some(a) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else { return false };
            #[allow(deprecated)]
            a.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps)
        }
        Token::View(v) => {
            let (Some(w), Some(mtm)) = (window_of(v), MainThreadMarker::new()) else { return false };
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
            w.makeKeyAndOrderFront(None);
            true
        }
        Token::Nothing => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foreground_tokens_name_apps_or_views() {
        assert_eq!(token_kind(0x7f00), Token::View(0x7f00));
        assert_eq!(token_kind(-412), Token::App(412));
        assert_eq!(token_kind(0), Token::Nothing);
    }

    #[test]
    fn forgetting_a_window_drops_its_state() {
        let v = 0x1234_5000;
        with_state(|m| m.insert(v, (true, 0.3)));
        forget(v);
        assert!(with_state(|m| !m.contains_key(&v)));
    }

    #[test]
    fn a_cloaked_window_shows_nothing_and_the_fade_comes_back() {
        assert_eq!(shown_alpha(true, 0.7), 0.0);
        assert_eq!(shown_alpha(false, 0.5), 0.5);
        assert_eq!(shown_alpha(false, 3.0), 1.0);
    }
}
