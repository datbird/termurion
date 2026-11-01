//! OLE text drag.

use super::super::Unsupported;

pub fn drag_text(text: &str) -> Result<bool, Unsupported> {
    Ok(crate::dragtext_win::drag_text(text))
}
