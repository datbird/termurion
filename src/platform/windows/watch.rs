//! The Windows watchers, from `dynamic_profiles_scan` and `scripts_scan` (Windows-only modules).

pub use crate::dynamic_profiles_scan::{stop_all, Scanner as DynamicScanner};
pub use crate::scripts_scan::{ScanEvent, Scanner as ScriptsScanner};
