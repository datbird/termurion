//! Input preferences on Unix.

/// There is no global key state to query without a display connection of our own.
pub fn alt_down() -> bool {
    false
}

/// The conventional 500 ms; the desktop setting is not read yet.
pub fn double_click_ms() -> u128 {
    500
}

/// The desktop's reduced-motion setting is not read yet, so animations stay on.
pub fn animations_on() -> bool {
    true
}
