//! The system's quit, Dock click and hide, through winit's application delegate. Its class is
//! given `applicationShouldTerminate:` and `applicationShouldHandleReopen:hasVisibleWindows:`
//! once; the app answers a quit later (NSTerminateLater) after its own question.

use std::ffi::CString;
use std::sync::{Mutex, OnceLock};

use objc2::encode::Encode;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_app_kit::NSApplication;

use crate::platform::app_events::{AppEvent, Terminate};

pub const HAS_APP_HIDING: bool = true;

type Sink = Box<dyn Fn(AppEvent) + Send + Sync>;

static SINK: OnceLock<Sink> = OnceLock::new();
static TERMINATE: Mutex<Option<Terminate>> = Mutex::new(None);

/// NSTerminateLater: the app replies when it knows.
const TERMINATE_LATER: usize = 2;

fn send(event: AppEvent) {
    if let Some(sink) = SINK.get() {
        sink(event);
    }
}

extern "C-unwind" fn should_terminate(_this: &AnyObject, _sel: Sel, _sender: &AnyObject) -> usize {
    if SINK.get().is_none() {
        // Not installed: let the system end the app as it would.
        return 1;
    }
    TERMINATE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Terminate::default).ask();
    send(AppEvent::Terminate);
    TERMINATE_LATER
}

extern "C-unwind" fn should_handle_reopen(_this: &AnyObject, _sel: Sel, _sender: &AnyObject, _visible: Bool) -> Bool {
    send(AppEvent::Reopen);
    // The app opens its own window, so AppKit does nothing more.
    Bool::NO
}

fn add(class: *mut AnyClass, sel: Sel, imp: Imp, types: &str) {
    let Ok(types) = CString::new(types) else { return };
    // SAFETY: the signature matches `types`; the class is winit's delegate and lives forever.
    let added = unsafe { objc2::ffi::class_addMethod(class, sel, imp, types.as_ptr()) };
    if !added.as_bool() {
        eprintln!("the application delegate already answers {}", sel.name().to_string_lossy());
    }
}

pub fn install(sink: Sink) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    if SINK.set(sink).is_err() {
        return;
    }
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: `delegate` returns the object winit set, or nil.
    let delegate: Option<objc2::rc::Retained<AnyObject>> = unsafe { msg_send![&*app, delegate] };
    let Some(delegate) = delegate else {
        eprintln!("there is no application delegate to take the system quit and Dock click");
        return;
    };
    let class = delegate.class() as *const AnyClass as *mut AnyClass;
    let f: extern "C-unwind" fn(&AnyObject, Sel, &AnyObject) -> usize = should_terminate;
    let g: extern "C-unwind" fn(&AnyObject, Sel, &AnyObject, Bool) -> Bool = should_handle_reopen;
    // SAFETY: same-size function pointers.
    let (f, g): (Imp, Imp) = unsafe { (std::mem::transmute(f), std::mem::transmute(g)) };
    add(class, sel!(applicationShouldTerminate:), f, &format!("{}@:@", usize::ENCODING));
    add(class, sel!(applicationShouldHandleReopen:hasVisibleWindows:), g, &format!("{}@:@{}", Bool::ENCODING, Bool::ENCODING));
}

pub fn answer_terminate(quit: bool) {
    let reply = TERMINATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|t| t.answer(quit));
    let (Some(reply), Some(mtm)) = (reply, MainThreadMarker::new()) else { return };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: the reply for the terminate that is waiting.
    let _: () = unsafe { msg_send![&*app, replyToApplicationShouldTerminate: Bool::new(reply)] };
}

fn with_app(act: impl FnOnce(&NSApplication)) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    act(&NSApplication::sharedApplication(mtm));
    true
}

pub fn hide_app() -> bool {
    with_app(|app| app.hide(None))
}

pub fn unhide_app() -> bool {
    with_app(|app| app.unhideWithoutActivation())
}

pub fn hide_others() -> bool {
    with_app(|app| app.hideOtherApplications(None))
}

pub fn show_all() -> bool {
    with_app(|app| app.unhideAllApplications(None))
}
