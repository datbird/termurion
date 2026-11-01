//! UI Automation, from `uia_win`.

use super::super::Unsupported;

pub use crate::uia_win::{Host, PaneSnapshot, Request, WindowSnapshot, disconnect_all};

pub fn available() -> Result<(), Unsupported> {
    Ok(())
}
