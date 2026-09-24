//! Tandem: an iTerm2-style terminal for Windows.

pub mod mouse;
pub mod schema;
pub mod scene;
pub mod input;
pub mod keys;
pub mod layout;
pub mod palette;
pub mod screen;

#[cfg(windows)]
pub mod conpty;

#[cfg(windows)]
pub mod probe;
#[cfg(windows)]
pub mod pty;

#[cfg(windows)]
pub mod pane;

#[cfg(windows)]
pub mod render;

#[cfg(windows)]
pub mod app;
