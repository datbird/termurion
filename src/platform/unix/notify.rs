//! No desktop notifications on Unix yet. A show reports Unsupported, so the caller can say so.

use super::super::Unsupported;

const FEATURE: &str = "Notifications";

fn no() -> Result<(), String> {
    Err(Unsupported { feature: FEATURE }.into())
}

/// What a click on a notification does. Nothing is shown here, so nothing is clicked.
pub fn on_click(_f: Box<dyn Fn() + Send + Sync>) {}

/// What a click on the resident icon does. There is no icon on Unix.
pub fn on_tray(_f: Box<dyn Fn(bool) + Send + Sync>) {}

/// What Explorer restarting does. Unix has no such event.
pub fn on_taskbar_created(_f: Box<dyn Fn() + Send + Sync>) {}

/// No hidden notification window exists on Unix.
pub fn window_handle() -> Option<isize> {
    None
}

pub fn set_resident(_on: bool) -> Result<(), String> {
    no()
}

/// Nothing is ever shown on Unix.
pub fn shown_count() -> u64 {
    0
}

pub fn show_with(_title: &str, _body: &str, _silent: bool) -> Result<(), String> {
    no()
}

/// Nothing was shown, so there is nothing to clear.
pub fn clear_if_shown(_at: u64) -> bool {
    false
}

pub fn show(_owner: isize, _title: &str, _body: &str) -> Result<(), String> {
    no()
}

pub fn remove(_owner: isize) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_notify_unsupported() {
        let e = show_with("t", "b", false).unwrap_err();
        assert!(e.contains("Notifications is not available"), "{e}");
        assert!(show(0, "t", "b").is_err());
        assert!(set_resident(true).is_err());
        assert_eq!(shown_count(), 0);
    }
}
