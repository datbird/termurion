//! No edge-drag snap on Unix (see `platform::sizing`).

use super::super::sizing::Grid;

pub fn install(_hwnd: isize) {}

pub fn publish(_hwnd: isize, _snap: Option<Grid>) {}
