//! Cmd key-downs go to the key path before the menu's key equivalents. AppKit offers a Cmd key
//! to the main menu first, so without this a key mapping on Cmd-T would lose to File > New Tab.
//! A `performKeyEquivalent:` is added to the view's class (winit's) that sends `keyDown:` for it.
//! The class is shared by every winit window, so only the views handed to `install` (terminal
//! windows) are claimed; Settings and chat windows keep AppKit's own Cmd+C, Cmd+V, Cmd+W and so on.

use std::collections::HashSet;
use std::ffi::CString;
use std::sync::Mutex;

use objc2::encode::Encode;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{ClassType, MainThreadMarker, msg_send, sel};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType, NSView};

/// Whether a key-down with these keys goes to the key path. Cmd-` and Cmd-Shift-` stay AppKit's
/// window cycling; Termurion has no command on them.
/// Cmd-Q, Cmd-H and Cmd-M stay the menu bar's too: the key path has no command it would not also
/// get from the menu, and a swallowed one would leave the app with no way to quit or hide.
pub fn claims(cmd: bool, chars_ignoring: &str) -> bool {
    cmd && !matches!(chars_ignoring, "`" | "~" | "q" | "h" | "m")
}

/// The whole rule: a registered terminal view and a claimed key.
pub fn takes(registered: bool, cmd: bool, chars_ignoring: &str) -> bool {
    registered && claims(cmd, chars_ignoring)
}

/// Terminal views (addresses) that take Cmd keys.
static VIEWS: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

fn is_registered(view: usize) -> bool {
    VIEWS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(&view))
}

extern "C-unwind" fn perform_key_equivalent(this: &AnyObject, _sel: Sel, event: &NSEvent) -> Bool {
    if event.r#type() == NSEventType::KeyDown && event.modifierFlags().contains(NSEventModifierFlags::Command) {
        let chars = event.charactersIgnoringModifiers().map(|s| s.to_string()).unwrap_or_default();
        if takes(is_registered(this as *const AnyObject as usize), true, &chars) {
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
pub fn install(view: isize) -> Result<(), String> {
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
    use super::{claims, takes};

    /// Review Focus 1: a Cmd key-down is taken for the key path (so a mapping beats the menu), but
    /// AppKit keeps its window-cycling keys.
    #[test]
    fn a_cmd_key_down_is_claimed_for_the_key_path() {
        assert!(claims(true, "t"));
        assert!(claims(true, "]"));
        assert!(!claims(true, "`"));
        assert!(!claims(true, "~"));
        assert!(!claims(false, "t"));
        assert!(!claims(true, "q") && !claims(true, "h") && !claims(true, "m"));
    }

    /// Only a terminal window's view claims keys: Cmd+C, Cmd+V and Cmd+W in Settings and chat go to AppKit.
    #[test]
    fn only_terminal_views_claim_keys() {
        assert!(takes(true, true, "t"));
        for key in ["c", "v", "x", "a", "z", "w"] {
            assert!(!takes(false, true, key), "{key} in a window that is not a terminal");
        }
    }
}
