//! No screen lock event is read on Unix yet, so the password manager does not lock with the screen.

use crate::platform::pty::Notify;

/// Nothing to watch; the app holds no window handle here.
pub fn watch(_hwnd: isize, _wake: Notify) {}

/// How many windows are watched: none.
pub fn watched() -> usize {
    0
}

/// The screen never reports locking.
pub fn take() -> bool {
    false
}
