//! UI Automation, from `uia_win`.

use super::super::Unsupported;

pub use crate::uia_win::{Host, disconnect_all};

pub fn available() -> Result<(), Unsupported> {
    Ok(())
}
