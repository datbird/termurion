//! The Tandem terminal.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    tandem::dll::harden_process();
    #[cfg(windows)]
    tandem::app::run();
}
