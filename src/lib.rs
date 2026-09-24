//! Tandem: an iTerm2-style terminal for Windows.

pub mod mouse;
pub mod screen;

#[cfg(windows)]
pub mod conpty;

#[cfg(windows)]
pub mod pty;
