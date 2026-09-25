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
pub mod osc1337;
pub mod marks;
pub mod find;
pub mod smartsel;
pub mod semantic;
pub mod synthetic;
pub mod tabstatus;
pub mod seltools;
pub mod copymode;
pub mod fold;
pub mod annotations;
pub mod images;
pub mod infobar;
pub mod inline_file;
pub mod sixel;
pub mod kitty;
pub mod stamps;
pub mod styled_copy;
pub mod bgimage;
pub mod blocks;
pub mod clipboard_osc;
pub mod commands;
pub mod menus;
pub mod import;
pub mod store;
pub mod print;
pub mod porthole;
pub mod triggers;
pub mod interp;
pub mod activity;
pub mod alerts;
pub mod bell;
pub mod trigger_eval;
pub mod trigger_events;
pub mod profile;
pub mod prompted;
pub mod schema;
pub mod scene;
pub mod input;
pub mod keys;
pub mod keyproto;
pub mod reports;
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

pub mod screenshot;
pub mod shotimage;
pub mod session_profile;

#[cfg(windows)]
pub mod settings_ui;

#[cfg(windows)]
pub mod egui_host;

#[cfg(windows)]
pub mod prompt_ui;
#[cfg(windows)]
pub mod note_ui;
#[cfg(windows)]
pub mod shot_ui;
#[cfg(windows)]
pub mod global_find_ui;

pub mod settings_keys;
pub mod settings_advanced;
pub mod settings_triggers;

#[cfg(windows)]
pub mod winclip;
#[cfg(windows)]
pub mod winprint;
#[cfg(windows)]
pub mod winkeys;

#[cfg(windows)]
pub mod notify;
