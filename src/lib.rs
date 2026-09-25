//! Tandem: an iTerm2-style terminal for Windows.

pub mod mouse;
pub mod globals;
pub mod keystroke;
pub mod keyaction;
pub mod pointer;
pub mod remap;
pub mod keyroute;
pub mod mru;
pub mod osc;
pub mod marks;
pub mod find;
pub mod smartsel;
pub mod semantic;
pub mod synthetic;
pub mod commands;
pub mod menus;
pub mod import;
pub mod store;
pub mod profile;
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

#[cfg(windows)]
pub mod settings_ui;

#[cfg(windows)]
pub mod egui_host;

#[cfg(windows)]
pub mod prompt_ui;
#[cfg(windows)]
pub mod global_find_ui;

pub mod settings_keys;
pub mod settings_advanced;

#[cfg(windows)]
pub mod winkeys;

#[cfg(windows)]
pub mod notify;
