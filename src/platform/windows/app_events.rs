//! No system quit, Dock or hide events here: the app has its own shutdown and launch paths.

use crate::platform::app_events::AppEvent;

pub const HAS_APP_HIDING: bool = false;

pub fn install(_sink: Box<dyn Fn(AppEvent) + Send + Sync>) {}

pub fn answer_terminate(_quit: bool) {}

pub fn hide_app() -> bool {
    false
}

pub fn hide_others() -> bool {
    false
}

pub fn show_all() -> bool {
    false
}
