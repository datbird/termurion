//! Cmd key-downs go to the key path before the menu's key equivalents. AppKit offers a Cmd key
//! to the main menu first, so without this a key mapping on Cmd-T would lose to File > New Tab.
//! A `performKeyEquivalent:` is added to the view's class (winit's) that sends `keyDown:` for it.
//! The class is shared by every winit window, so only the views handed to `install` (terminal
//! windows) are claimed; Settings and chat windows keep AppKit's own Cmd+C, Cmd+V, Cmd+W and so on.
//! While a shortcut recorder listens, every view claims Cmd keys, so Cmd-T records instead of
//! opening a tab. Cmd-Q, Cmd-H and Cmd-M stay the menu's even then: a terminal never takes them,
//! so a recorded one could not run.

use std::collections::HashSet;
use std::ffi::CString;
use std::sync::{Arc, Mutex, RwLock};

use objc2::encode::Encode;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{ClassType, MainThreadMarker, msg_send, sel};
use crate::keyroute::Snapshot;
use crate::menus::{Chord, ChordKey};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType, NSView};

/// Cmd-` and Cmd-Shift-` stay AppKit's window cycling; Termurion has no command on them.
pub fn claims(cmd: bool, chars_ignoring: &str) -> bool {
    cmd && chars_ignoring != "`" && chars_ignoring != "~"
}

/// The system keys that stay the menu bar's: Cmd-Q, Cmd-H, Cmd-Opt-H and Cmd-M exactly (case does
/// not matter; Cmd-Shift-H and the like are the person's to map).
pub fn system_chord(c: &Chord) -> bool {
    let plain = c.cmd && !c.shift && !c.ctrl;
    plain && matches!((&c.key, c.alt), (ChordKey::Char('q' | 'm'), false) | (ChordKey::Char('h'), _))
}

/// The whole rule: a registered terminal view, and a chord the key path takes (`bound`: a key
/// mapping or a command). An unbound chord such as Cmd-Ctrl-Space (Emoji & Symbols) is AppKit's.
/// While the recorder listens (`recording`), any view takes any chord but the system keys.
pub fn takes(registered: bool, chord: &Chord, bound: bool, recording: bool) -> bool {
    (recording || (registered && bound)) && !system_chord(chord)
}

/// Terminal views (addresses) that take Cmd keys.
static VIEWS: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

/// What the key path takes, shared with the app (`App::route_snapshot`).
static ROUTE: Mutex<Option<Arc<RwLock<Snapshot>>>> = Mutex::new(None);

fn is_registered(view: usize) -> bool {
    VIEWS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(&view))
}

/// The view is going away: it no longer takes keys, and its address may be reused.
pub fn forget(view: isize) {
    if let Some(set) = VIEWS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        set.remove(&(view as usize));
    }
}

fn bound(chord: &Chord) -> bool {
    let route = ROUTE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    route.is_some_and(|r| crate::keyroute::chord_bound(&r.read().unwrap_or_else(|e| e.into_inner()), chord))
}

extern "C-unwind" fn perform_key_equivalent(this: &AnyObject, _sel: Sel, event: &NSEvent) -> Bool {
    if event.r#type() == NSEventType::KeyDown && event.modifierFlags().contains(NSEventModifierFlags::Command) {
        let chars = event.charactersIgnoringModifiers().map(|s| s.to_string()).unwrap_or_default();
        let flags = event.modifierFlags();
        let chord = crate::menus::chord_of_key(&chars, true, flags.contains(NSEventModifierFlags::Option), flags.contains(NSEventModifierFlags::Shift), flags.contains(NSEventModifierFlags::Control));
        let wanted = claims(true, &chars) && chord.as_ref().is_some_and(|c| takes(is_registered(this as *const AnyObject as usize), c, bound(c), crate::platform::unix::global_keys::recording()));
        if wanted {
            // SAFETY: `this` is the view; keyDown: takes the event.
            let _: () = unsafe { msg_send![this, keyDown: event] };
            return Bool::YES;
        }
    }
    // SAFETY: the view's own default answer, as if nothing was added.
    unsafe { msg_send![super(this, NSView::class()), performKeyEquivalent: event] }
}

/// Classes already given the method (winit's view class is shared by every window).
static DONE: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

/// Adds the method to the view's class, once per class. Err says why it could not.
pub fn install(view: isize, route: Arc<RwLock<Snapshot>>) -> Result<(), String> {
    if MainThreadMarker::new().is_none() {
        return Err("Key equivalents can only be set up on the main thread.".to_string());
    }
    if view == 0 {
        return Err("There is no view to take the keys.".to_string());
    }
    // SAFETY: the app passes the live NSView of its window.
    let obj: &AnyObject = unsafe { &*(view as *const AnyObject) };
    if obj.downcast_ref::<NSView>().is_none() {
        return Err("The window handle is not an NSView.".to_string());
    }
    *ROUTE.lock().unwrap_or_else(|e| e.into_inner()) = Some(route);
    VIEWS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).insert(view as usize);
    let class: &AnyClass = obj.class();
    let key = class as *const AnyClass as usize;
    let mut done = DONE.lock().unwrap_or_else(|e| e.into_inner());
    let done = done.get_or_insert_with(HashSet::new);
    if done.contains(&key) {
        return Ok(());
    }
    let types = CString::new(format!("{}@:@", Bool::ENCODING)).map_err(|e| e.to_string())?;
    let f: extern "C-unwind" fn(&AnyObject, Sel, &NSEvent) -> Bool = perform_key_equivalent;
    // SAFETY: same-size function pointers; the signature matches the type string above.
    let imp: Imp = unsafe { std::mem::transmute(f) };
    // SAFETY: adds one method to a class that has none of this name; the class lives forever.
    let added = unsafe { objc2::ffi::class_addMethod(class as *const AnyClass as *mut AnyClass, sel!(performKeyEquivalent:), imp, types.as_ptr()) };
    if !added.as_bool() {
        return Err(format!("The view class {} already answers performKeyEquivalent:.", class.name().to_string_lossy()));
    }
    done.insert(key);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(key: char) -> Chord {
        Chord { key: ChordKey::Char(key), cmd: true, alt: false, shift: false, ctrl: false }
    }

    /// Review Focus 1: a Cmd key-down is taken for the key path (so a mapping beats the menu), but
    /// AppKit keeps its window-cycling keys.
    #[test]
    fn a_cmd_key_down_is_claimed_for_the_key_path() {
        assert!(claims(true, "t"));
        assert!(claims(true, "]"));
        assert!(!claims(true, "`"));
        assert!(!claims(true, "~"));
        assert!(!claims(false, "t"));
        assert!(takes(true, &cmd('t'), true, false));
    }

    /// Only a terminal window's view claims keys: Cmd+C, Cmd+V and Cmd+W in Settings and chat go to AppKit.
    #[test]
    fn only_terminal_views_claim_keys() {
        for key in ['c', 'v', 'x', 'a', 'z', 'w'] {
            assert!(!takes(false, &cmd(key), true, false), "{key} in a window that is not a terminal");
        }
    }

    /// Only a chord the key path takes is claimed, and the system keys stay the menu's by exact chord.
    #[test]
    fn unbound_and_system_chords_stay_with_appkit() {
        let emoji = Chord { key: ChordKey::Space, ctrl: true, ..cmd('t') };
        assert!(!takes(true, &emoji, false, false), "Cmd+Ctrl+Space is Emoji & Symbols");
        for key in ['q', 'h', 'm'] {
            assert!(!takes(true, &cmd(key), true, false));
        }
        assert!(!takes(true, &Chord { alt: true, ..cmd('h') }, true, false), "Hide Others");
        assert!(takes(true, &Chord { shift: true, ..cmd('h') }, true, false), "a mapping on Cmd+Shift+H wins");
        assert!(takes(true, &Chord { alt: true, ..cmd('m') }, true, false));
    }

    /// While the recorder listens, a Settings or Profiles view takes a Cmd chord that has a menu
    /// item (Cmd-T records, no tab opens), bound or not; Cmd-Q, Cmd-H, Cmd-Opt-H and Cmd-M stay the menu's.
    #[test]
    fn the_recorder_gets_cmd_chords_in_any_window() {
        for key in ['t', 'd', 'k', 'w', 'c', 'v', '1'] {
            assert!(takes(false, &cmd(key), false, true), "{key} while recording");
            assert!(!takes(false, &cmd(key), false, false), "{key} when not recording");
        }
        assert!(takes(false, &Chord { shift: true, ..cmd('h') }, false, true));
        for key in ['q', 'h', 'm'] {
            assert!(!takes(false, &cmd(key), true, true), "{key} stays the menu's");
            assert!(!takes(true, &cmd(key), true, true));
        }
        assert!(!takes(false, &Chord { alt: true, ..cmd('h') }, false, true), "Hide Others");
    }

    /// A destroyed window's view no longer takes keys.
    #[test]
    fn a_forgotten_view_is_not_registered() {
        VIEWS.lock().unwrap().get_or_insert_with(HashSet::new).insert(0x1234);
        assert!(is_registered(0x1234));
        forget(0x1234);
        assert!(!is_registered(0x1234));
    }
}
