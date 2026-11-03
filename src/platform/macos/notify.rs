//! Desktop notifications through UserNotifications. They only work from the signed Termurion.app;
//! a bare binary has no bundle id and says so. Completion handlers run on other threads, so they
//! touch nothing of the App, only statics and the center.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass};
use objc2_app_kit::NSRunningApplication;
use objc2_foundation::{NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions, UNNotificationRequest,
    UNNotificationResponse, UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

use crate::dlog;

const NO_BUNDLE: &str = "Notifications need Termurion to run as Termurion.app";
const TURNED_OFF: &str = "Notifications are turned off for Termurion in System Settings";

static SHOWN: AtomicU64 = AtomicU64::new(0);
static ASKED: AtomicBool = AtomicBool::new(false);
type Click = Arc<dyn Fn() + Send + Sync>;
static CLICK: Mutex<Option<Click>> = Mutex::new(None);

/// The identifier of the n-th notification.
pub fn request_id(n: u64) -> String {
    format!("termurion-{n}")
}

/// What a click on a notification does.
pub fn on_click(f: Box<dyn Fn() + Send + Sync>) {
    *CLICK.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::from(f));
}

/// The Dock icon is the way back on macOS (Task 3's reopen); there is no tray icon.
pub fn on_tray(_f: Box<dyn Fn(bool) + Send + Sync>) {}

/// Nothing like Explorer restarting happens on macOS.
pub fn on_taskbar_created(_f: Box<dyn Fn() + Send + Sync>) {}

/// There is no hidden notification window; the Dock icon is the way back.
pub fn window_handle() -> Option<isize> {
    None
}

/// No resident tray icon on macOS; the Dock icon stays.
pub fn set_resident(_on: bool) -> Result<(), String> {
    Ok(())
}

/// No tray icon to remove on macOS.
pub fn remove(_owner: isize) {}

pub fn shown_count() -> u64 {
    SHOWN.load(Ordering::SeqCst)
}

/// Takes `termurion-<at>` off the screen when nothing was shown since.
pub fn clear_if_shown(at: u64) -> bool {
    if !super::super::notify::still_shown(at, shown_count()) {
        return false;
    }
    if has_bundle() {
        let ids = NSArray::from_retained_slice(&[NSString::from_str(&request_id(at))]);
        let center = UNUserNotificationCenter::currentNotificationCenter();
        center.removeDeliveredNotificationsWithIdentifiers(&ids);
        center.removePendingNotificationRequestsWithIdentifiers(&ids);
    }
    true
}

fn has_bundle() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some()
}

define_class!(
    // Presents banners while we are in front and passes a click on.
    #[unsafe(super(NSObject))]
    #[name = "TermurionNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(&self, _center: &UNUserNotificationCenter, _n: &UNNotification, done: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>) {
            let front = NSRunningApplication::currentApplication().isActive();
            let opts = if front { UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List | UNNotificationPresentationOptions::Sound } else { UNNotificationPresentationOptions(0) };
            done.call((opts,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(&self, _center: &UNUserNotificationCenter, _r: &UNNotificationResponse, done: &block2::DynBlock<dyn Fn()>) {
            let f = CLICK.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(f) = f {
                f();
            }
            done.call(());
        }
    }
);

/// Keeps the delegate alive; the center only holds it weakly.
struct Keep(#[allow(dead_code)] Retained<Delegate>);
// SAFETY: the delegate has no state; its methods read statics behind locks.
unsafe impl Send for Keep {}
unsafe impl Sync for Keep {}
static DELEGATE: OnceLock<Keep> = OnceLock::new();

fn center() -> Retained<UNUserNotificationCenter> {
    let c = UNUserNotificationCenter::currentNotificationCenter();
    DELEGATE.get_or_init(|| {
        let d: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(), init] };
        c.setDelegate(Some(ProtocolObject::from_ref(&*d)));
        Keep(d)
    });
    c
}

/// Adds the request to the center; a failure is only logged (it happens off the UI thread).
fn post(title: &str, body: &str, silent: bool, id: &str) {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    if !silent {
        content.setSound(Some(&UNNotificationSound::defaultSound()));
    }
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(id), &content, None);
    let done = RcBlock::new(|err: *mut NSError| {
        if !err.is_null() {
            dlog!("The notification was not shown");
        }
    });
    center().addNotificationRequest_withCompletionHandler(&request, Some(&done));
}

/// The authorization status, waiting at most a second for the system.
fn status() -> Option<UNAuthorizationStatus> {
    let (tx, rx) = channel();
    let tx = Mutex::new(tx);
    let got = RcBlock::new(move |settings: std::ptr::NonNull<UNNotificationSettings>| {
        // SAFETY: the system passes a live settings object for the duration of the call.
        let s = unsafe { settings.as_ref() }.authorizationStatus();
        let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send(s);
    });
    center().getNotificationSettingsWithCompletionHandler(&got);
    match rx.recv_timeout(Duration::from_secs(1)) {
        Ok(s) => Some(s),
        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
    }
}

/// Shows a notification with `title` and `body`; `silent` plays no sound.
pub fn show_with(title: &str, body: &str, silent: bool) -> Result<(), String> {
    if !has_bundle() {
        return Err(NO_BUNDLE.to_string());
    }
    let status = status();
    if status == Some(UNAuthorizationStatus::Denied) {
        return Err(TURNED_OFF.to_string());
    }
    let n = SHOWN.fetch_add(1, Ordering::SeqCst) + 1;
    let id = request_id(n);
    if status == Some(UNAuthorizationStatus::NotDetermined) {
        // Ask once; the answer arrives later and the post follows a yes.
        if ASKED.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let (title, body) = (title.to_string(), body.to_string());
        let answer = RcBlock::new(move |granted: Bool, _err: *mut NSError| {
            if granted.as_bool() {
                post(&title, &body, silent, &id);
            } else {
                dlog!("The notification was not shown");
            }
        });
        center().requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &answer);
        return Ok(());
    }
    post(title, body, silent, &id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_are_numbered() {
        assert_eq!(request_id(7), "termurion-7");
    }

    #[test]
    fn notifications_outside_the_bundle_say_so() {
        let e = show_with("t", "b", false).unwrap_err();
        assert!(e.contains("Termurion.app"), "{e}");
        assert_eq!(shown_count(), 0);
    }
}
