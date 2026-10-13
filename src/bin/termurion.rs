//! The Termurion terminal.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    termurion::dll::harden_process();
    #[cfg(windows)]
    termurion::presence::set_app_user_model_id();
    #[cfg(windows)]
    termurion::app::run();
}
