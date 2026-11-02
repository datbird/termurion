//! Screen reader support. Windows answers UI Automation (`uia_win`, a Windows-only module); Unix has
//! no accessibility backend yet, so `available` reports Unsupported and `Host::install` fails with
//! the same text. The plain data a host is fed is here, so the app builds it on every OS.

use super::Unsupported;
use crate::access;

#[cfg(unix)]
use super::unix::a11y as imp;
#[cfg(windows)]
use super::windows::a11y as imp;

pub use imp::{Host, disconnect_all};

/// One pane as the provider sees it (screen coordinates).
#[derive(Clone, Debug, Default)]
pub struct PaneSnapshot {
    pub id: u64,
    pub text: access::Text,
    /// The selection as text offsets (empty when `.0 == .1`).
    pub selection: (usize, usize),
    pub cursor: usize,
    /// (left, top, right, bottom).
    pub rect: (i32, i32, i32, i32),
    /// One cell's width and height in pixels.
    pub cell: (f32, f32),
    /// Where text row `first_visible_row`, column 0 sits on screen.
    pub origin: (i32, i32),
    pub first_visible_row: usize,
    pub visible_rows: usize,
    pub focused: bool,
    /// The pane is at a password prompt (13v2 B5/P12): the text is only the prompt, nothing is
    /// announced, and `Select` and `ScrollIntoView` are refused.
    pub secure: bool,
    /// A browser pane (13x B7): its name ("Browser: <title>"), a Group with no text range and no
    /// caret; WebView2 raises its own UI Automation tree from its child window.
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct WindowSnapshot {
    pub title: String,
    /// (left, top, right, bottom).
    pub rect: (i32, i32, i32, i32),
    pub panes: Vec<PaneSnapshot>,
}

/// What a client asks of Termurion; posted to the window thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Select { pane: u64, start: usize, end: usize },
    ScrollIntoView { pane: u64, offset: usize },
    /// A client asked the window for its root provider: the window thread publishes a snapshot
    /// now (nothing else would wake it, and nothing is built until a client is there).
    Attach,
}

/// Ok when a screen reader can attach to a window here.
pub fn available() -> Result<(), Unsupported> {
    imp::available()
}
