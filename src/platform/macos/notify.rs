//! Desktop notifications through UserNotifications. They only work from the signed Termurion.app;
//! a bare binary has no bundle id and says so. Completion handlers run on other threads, so they
//! touch nothing of the App, only statics and the center.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass};
use objc2_foundation::{NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions, UNNotificationRequest,
    UNNotificationResponse, UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

use super::super::notify::Click;
use crate::dlog;

const NO_BUNDLE: &str = "Notifications need Termurion to run as Termurion.app";
const TURNED_OFF: &str = "Notifications are turned off for Termurion in System Settings";

static SHOWN: AtomicU64 = AtomicU64::new(0);
static PENDING: Mutex<Pending> = Mutex::new(Pending::new());
const QUEUE_CAP: usize = 8;
type OnClick = Arc<dyn Fn(Click) + Send + Sync>;
static CLICK: Mutex<Option<OnClick>> = Mutex::new(None);

/// A show waiting for the permission answer.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    title: String,
    body: String,
    silent: bool,
}

/// What a show does while permission is not decided.
#[derive(Debug, PartialEq)]
enum Step {
    /// First one: ask the system.
    Ask,
    /// A prompt is already open: it waits in the queue.
    Wait,
    /// The queue is full.
    Full,
}

/// What the answer to the prompt does with the queued shows.
#[derive(Debug, PartialEq)]
enum Settled {
    Post(Vec<Item>),
    Drop(usize),
}

/// The shows made while the permission prompt is open, and whether it is open.
struct Pending {
    asked: bool,
    items: Vec<Item>,
}

impl Pending {
    const fn new() -> Self {
        Pending { asked: false, items: Vec::new() }
    }

    fn on_show(&mut self, item: Item) -> Step {
        if self.items.len() >= QUEUE_CAP {
            return Step::Full;
        }
        self.items.push(item);
        if std::mem::replace(&mut self.asked, true) { Step::Wait } else { Step::Ask }
    }

    /// The prompt is over. `undecided`: it closed with no answer, so a later show asks again.
    fn settle(&mut self, granted: bool, undecided: bool) -> Settled {
        let items = std::mem::take(&mut self.items);
        if !granted && undecided {
            self.asked = false;
        }
        if granted { Settled::Post(items) } else { Settled::Drop(items.len()) }
    }
}

/// Whether Termurion is in front. The center asks only while Termurion is in front, and the answer
/// is always to show (`presentation`), so nothing needs it; the facade keeps the call.
pub fn set_active(_on: bool) {}

/// How a notification shows while Termurion is the active app: as a banner, in the list, with its
/// sound, for the focused session too (iTerm2's `willPresentNotification:` answers Banner and Sound
/// every time; only Suppress Alerts in Active Session holds a session's own alert back).
fn presentation() -> UNNotificationPresentationOptions {
    UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List | UNNotificationPresentationOptions::Sound
}

/// The identifier of the n-th notification.
pub fn request_id(n: u64) -> String {
    format!("termurion-{n}")
}

/// The show number in a request id (`request_id`'s inverse).
fn shown_of(id: &str) -> Option<u64> {
    id.strip_prefix("termurion-")?.parse().ok()
}

/// What a click on a notification does; it is told which show was clicked.
pub fn on_click(f: Box<dyn Fn(Click) + Send + Sync>) {
    *CLICK.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::from(f));
}

pub const MENU_AT_ICON: bool = true;

/// What a click on the menu bar status item does.
pub fn on_tray(f: Box<dyn Fn(bool) + Send + Sync>) {
    super::status_item::on_click(f);
}

/// Nothing like Explorer restarting happens on macOS.
pub fn on_taskbar_created(_f: Box<dyn Fn() + Send + Sync>) {}

/// Tray menu picks come back as menu events here; nothing else to hear.
pub fn on_tray_pick(_f: Box<dyn Fn(String) + Send + Sync>) {}

/// The icon counts as up here: a failed add is retried as before, and nothing quits for it.
pub fn icon_state() -> crate::resident::Icon {
    crate::resident::Icon::Up
}

/// Nothing to hear: the icon's state never changes the resident rule here.
pub fn on_icon_lost(_f: Box<dyn Fn(Option<String>) + Send + Sync>) {}

/// No activation tokens here.
pub fn take_tray_token() -> Option<String> {
    None
}

/// The tray menu's anchor: the menu bar status item's button, while it is there.
pub fn window_handle() -> Option<isize> {
    super::status_item::button_view()
}

/// The menu bar status item (the Windows notification area icon).
pub fn set_resident(on: bool) -> Result<(), String> {
    super::status_item::set(on)
}

/// Takes the status item out of the menu bar.
pub fn remove(_owner: isize) {
    let _ = super::status_item::set(false);
}

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
            done.call((presentation(),));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(&self, _center: &UNUserNotificationCenter, r: &UNNotificationResponse, done: &block2::DynBlock<dyn Fn()>) {
            let f = CLICK.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let id = r.notification().request().identifier().to_string();
            if let Some(f) = f {
                f(Click { shown: shown_of(&id).unwrap_or_else(shown_count), activation: None });
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
fn not_shown() {
    dlog!("The notification was not shown");
}

fn post(title: &str, body: &str, silent: bool) {
    // Only a real post counts, so clear_if_shown(at) stays right.
    let id = request_id(SHOWN.fetch_add(1, Ordering::SeqCst) + 1);
    let id = id.as_str();
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    if !silent {
        content.setSound(Some(&UNNotificationSound::defaultSound()));
    }
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(id), &content, None);
    let done = RcBlock::new(|err: *mut NSError| {
        if !err.is_null() {
            not_shown();
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
    match status() {
        Some(UNAuthorizationStatus::Denied) => return Err(TURNED_OFF.to_string()),
        Some(UNAuthorizationStatus::Authorized | UNAuthorizationStatus::Provisional | UNAuthorizationStatus::Ephemeral) => {
            post(title, body, silent);
            return Ok(());
        }
        // Not decided, or the read timed out. Asking is safe either way: an app that is already
        // allowed gets a yes at once with no prompt, and the queue posts.
        _ => {}
    }
    let item = Item { title: title.to_string(), body: body.to_string(), silent };
    let step = PENDING.lock().unwrap_or_else(|e| e.into_inner()).on_show(item);
    match step {
        Step::Wait => {}
        Step::Full => not_shown(),
        Step::Ask => {
            let answer = RcBlock::new(move |granted: Bool, _err: *mut NSError| {
                let undecided = !granted.as_bool() && matches!(status(), None | Some(UNAuthorizationStatus::NotDetermined));
                let settled = PENDING.lock().unwrap_or_else(|e| e.into_inner()).settle(granted.as_bool(), undecided);
                match settled {
                    Settled::Post(items) => items.iter().for_each(|i| post(&i.title, &i.body, i.silent)),
                    Settled::Drop(n) => (0..n.min(1)).for_each(|_| not_shown()),
                }
            });
            center().requestAuthorizationWithOptions_completionHandler(UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound, &answer);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_are_numbered() {
        assert_eq!(request_id(7), "termurion-7");
        assert_eq!(shown_of(&request_id(7)), Some(7), "a click names its own show");
        assert_eq!(shown_of("other"), None);
    }

    #[test]
    fn notifications_outside_the_bundle_say_so() {
        let e = show_with("t", "b", false).unwrap_err();
        assert!(e.contains("Termurion.app"), "{e}");
        assert_eq!(shown_count(), 0);
    }

    fn item(n: usize) -> Item {
        Item { title: format!("t{n}"), body: String::new(), silent: false }
    }

    #[test]
    fn shows_during_the_prompt_queue_and_post_on_yes() {
        let mut p = Pending::new();
        assert_eq!(p.on_show(item(1)), Step::Ask);
        assert_eq!(p.on_show(item(2)), Step::Wait);
        assert_eq!(p.settle(true, false), Settled::Post(vec![item(1), item(2)]));
        assert!(p.items.is_empty());
    }

    #[test]
    fn a_no_drops_the_queue() {
        let mut p = Pending::new();
        p.on_show(item(1));
        p.on_show(item(2));
        assert_eq!(p.settle(false, false), Settled::Drop(2));
        assert!(p.asked, "a decided no is not asked again");
    }

    #[test]
    fn a_prompt_closed_undecided_asks_again() {
        let mut p = Pending::new();
        p.on_show(item(1));
        assert_eq!(p.settle(false, true), Settled::Drop(1));
        assert_eq!(p.on_show(item(2)), Step::Ask);
    }

    #[test]
    fn the_queue_is_bounded() {
        let mut p = Pending::new();
        for n in 0..QUEUE_CAP {
            p.on_show(item(n));
        }
        assert_eq!(p.on_show(item(99)), Step::Full);
        assert_eq!(p.items.len(), QUEUE_CAP);
    }

    /// Step 7: a notification for the focused session shows while Termurion is in front, as in
    /// iTerm2, whatever the window focus said last (a focus change between two windows used to
    /// leave the flag off and hide the banner).
    #[test]
    fn notifications_show_while_termurion_is_in_front() {
        set_active(false);
        let p = presentation();
        assert!(p.contains(UNNotificationPresentationOptions::Banner) && p.contains(UNNotificationPresentationOptions::Sound) && p.contains(UNNotificationPresentationOptions::List));
    }
}
