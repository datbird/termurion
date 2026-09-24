//! Tandem: an iTerm2-style terminal for Windows.

pub mod mouse;

#[cfg(windows)]
pub mod conpty;
