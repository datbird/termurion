//! The menu bar status item: the macOS form of the Windows notification area icon. It is there
//! under the same rule (`resident::icon_wanted`), and a click opens the same menu
//! (`App::show_tray_menu`), popped from the item itself. Main thread only.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use block2::RcBlock;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, AnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSEventMask, NSImage, NSMenu, NSStatusBar, NSStatusItem};
use objc2_foundation::{NSData, NSNotification, NSNotificationCenter, NSObject, NSSize, NSString};

type Click = Box<dyn Fn(bool) + Send + Sync>;
static ON_CLICK: OnceLock<Click> = OnceLock::new();

/// NSSquareStatusItemLength: as wide as the bar is tall.
const SQUARE: f64 = -2.0;
/// The picture is drawn at 36 px and shown at 18 points (sharp on Retina).
const PIXELS: u32 = 36;
const POINTS: f64 = 18.0;

define_class!(
    // Takes the item's click and hands it to the app, which opens the menu.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermurionStatusItemTarget"]
    struct Target;

    impl Target {
        #[unsafe(method(clicked:))]
        fn clicked(&self, _sender: &AnyObject) {
            fire(ON_CLICK.get());
        }
    }
);

/// Any click is the menu, as a status item's is (true is the tray's menu click).
fn fire(on: Option<&Click>) {
    if let Some(f) = on {
        f(true);
    }
}

struct Item {
    item: Retained<NSStatusItem>,
    _target: Retained<Target>,
}

thread_local! {
    static ITEM: RefCell<Option<Item>> = const { RefCell::new(None) };
}

/// What a click on the item does.
pub fn on_click(f: Click) {
    let _ = ON_CLICK.set(f);
}

fn picture() -> Option<Retained<NSImage>> {
    let png = crate::presence::tray_template_png(PIXELS);
    if png.is_empty() {
        return None;
    }
    let data = NSData::with_bytes(&png);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    image.setSize(NSSize::new(POINTS, POINTS));
    // A template is tinted by AppKit, so it reads on a light and a dark menu bar.
    image.setTemplate(true);
    Some(image)
}

/// Puts the item in the menu bar, or takes it out. Twice is safe.
pub fn set(on: bool) -> Result<(), String> {
    let Some(mtm) = MainThreadMarker::new() else { return Err("The menu bar icon can only change on the main thread".into()) };
    ITEM.with_borrow_mut(|slot| {
        if !on {
            if let Some(old) = slot.take() {
                // The button's target is unretained: clear it so no pointer outlives the target.
                if let Some(b) = old.item.button(mtm) {
                    unsafe { b.setTarget(None) };
                }
                NSStatusBar::systemStatusBar().removeStatusItem(&old.item);
            }
            return Ok(());
        }
        if slot.is_some() {
            return Ok(());
        }
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(SQUARE);
        let Some(button) = item.button(mtm) else {
            NSStatusBar::systemStatusBar().removeStatusItem(&item);
            return Err("The menu bar gave Termurion no icon button".into());
        };
        match picture() {
            Some(image) => button.setImage(Some(&image)),
            None => button.setTitle(&NSString::from_str("T")),
        }
        button.setToolTip(Some(&NSString::from_str(crate::presence::TRAY_TIP)));
        let target: Retained<Target> = unsafe { msg_send![Target::alloc(mtm), init] };
        // SAFETY: the target lives as long as the item (both are kept in ITEM).
        unsafe {
            button.setTarget(Some(&target));
            button.setAction(Some(sel!(clicked:)));
        }
        // Open on the press, as menu bar items do, for either button.
        let _ = button.sendActionOn(NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown);
        *slot = Some(Item { item, _target: target });
        Ok(())
    })
}

/// The item's button view (the menu's anchor), while it is in the menu bar.
pub fn button_view() -> Option<isize> {
    let mtm = MainThreadMarker::new()?;
    ITEM.with_borrow(|slot| slot.as_ref().and_then(|i| i.item.button(mtm)).map(|b| Retained::as_ptr(&b) as isize))
}

/// Opens `menu` from the item, under it in the menu bar. True only when an item was picked;
/// false for a dismissed menu or when `view` is not the item's button.
pub fn pop(view: isize, menu: *mut std::ffi::c_void) -> bool {
    if menu.is_null() || button_view() != Some(view) {
        return false;
    }
    let Some(mtm) = MainThreadMarker::new() else { return false };
    // Taken out of ITEM first: the menu runs its own loop, and a pass in it may change the item.
    // The target is kept too, so it outlives the click even if the item is taken down meanwhile.
    let Some((item, _target)) = ITEM.with_borrow(|slot| slot.as_ref().map(|i| (i.item.clone(), i._target.clone()))) else { return false };
    let Some(button) = item.button(mtm) else { return false };
    // SAFETY: muda's live NSMenu, kept by the caller for this call.
    let menu: &NSMenu = unsafe { &*(menu as *const NSMenu) };
    // The item owns the menu only while it is open, so a later click comes to `clicked:` again.
    item.setMenu(Some(menu));
    // The menu posts this when an item sends its action; a dismissal posts nothing.
    let picked = Arc::new(AtomicBool::new(false));
    let seen = picked.clone();
    let block = RcBlock::new(move |_n: NonNull<NSNotification>| seen.store(true, Ordering::Relaxed));
    let center = NSNotificationCenter::defaultCenter();
    let name = NSString::from_str("NSMenuDidSendActionNotification");
    // SAFETY: main thread; the block touches only an atomic. Removed again below.
    let token = unsafe { center.addObserverForName_object_queue_usingBlock(Some(&name), None, None, &block) };
    unsafe { button.performClick(None) };
    unsafe { center.removeObserver(&*(Retained::as_ptr(&token) as *const AnyObject)) };
    item.setMenu(None);
    picked.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A click on the item is the tray's menu click, and no handler is no crash.
    #[test]
    fn a_click_is_the_tray_menu_click() {
        let got = Arc::new(AtomicBool::new(false));
        let seen = got.clone();
        let handler: Click = Box::new(move |menu| seen.store(menu, Ordering::SeqCst));
        fire(None);
        assert!(!got.load(Ordering::SeqCst));
        fire(Some(&handler));
        assert!(got.load(Ordering::SeqCst));
    }
}
