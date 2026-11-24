//! The screens as AppKit sees them, for SetWindowFrame (`PseudoTerminal windowSetFrame:`): the
//! visible frame of each (the menu bar and the Dock left out) and the narrowest a titled window can
//! be dragged.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSScreen, NSWindow, NSWindowStyleMask};
use objc2_foundation::NSString;

/// Each screen's visible frame in points, top-left origin and y down from the top of the main screen
/// (the space `CGDisplayBounds` and winit use). None off the main thread.
pub fn visible_frames() -> Option<Vec<(f64, f64, f64, f64)>> {
    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    let main_height = screens.firstObject()?.frame().size.height;
    Some(
        screens
            .iter()
            .map(|s| {
                let v = s.visibleFrame();
                crate::osc1337::from_appkit_frame((v.origin.x, v.origin.y, v.size.width, v.size.height), main_height)
            })
            .collect(),
    )
}

/// `NSWindow.minFrameWidth(withTitle: "", styleMask: .titled)`: how narrow a person can drag a titled
/// window (its title bar buttons need that much), in points.
pub fn min_titled_width() -> Option<f64> {
    let mtm = MainThreadMarker::new()?;
    Some(NSWindow::minFrameWidthWithTitle_styleMask(&NSString::from_str(""), NSWindowStyleMask::Titled, mtm))
}
