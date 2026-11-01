//! Shell > Print and Page Setup. Windows draws the page with GDI; Unix has no printing yet.

#[cfg(unix)]
use super::unix::print as imp;
#[cfg(windows)]
use super::windows::print as imp;

/// The print dialog's checkbox text (`iTermPrintAccessoryViewController.xib:23`) and the page's tab title.
pub const BLACK_AND_WHITE_LABEL: &str = "Black and white";
pub const PAGE_TITLE: &str = "Options";

pub use imp::{Setup, page_setup, print};
