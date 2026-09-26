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
pub mod captured;
pub mod find;
pub mod smartsel;
pub mod semantic;
pub mod synthetic;
pub mod tabstatus;
pub mod status_tool;
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
pub mod completion;
pub mod menus;
pub mod import;
pub mod store;
pub mod print;
pub mod porthole;
pub mod triggers;
pub mod interp;
pub mod activity;
pub mod alerts;
pub mod badge;
pub mod bell;
pub mod trigger_eval;
pub mod trigger_events;
pub mod proctree;
pub mod winmodel;
pub mod frames;
pub mod placement;
pub mod arrangement;
pub mod arrangements;
pub mod advanced_paste;
pub mod advanced_paste_ui;
pub mod aps;
pub mod autocomplete;
pub mod coprocess;
pub mod close_prompt;
pub mod close_rules;
pub mod undo_close;
pub mod startup;
pub mod jobtitle;
pub mod profile;
pub mod prompted;
pub mod schema;
pub mod scene;
pub mod input;
pub mod keys;
pub mod keyproto;
pub mod reports;
pub mod layout;
pub mod list_popup_ui;
pub mod palette;
pub mod paste;
pub mod paste_history;
pub mod popup_filter;
pub mod screen;
pub mod screenterm;
pub mod shell_history;
pub mod snippets;
pub mod clippings;
pub mod clip_add_ui;
pub mod user_actions;
pub mod statusbar;
pub mod statusbar_components;
pub mod statusbar_layout;
pub mod statusbar_metrics;
pub mod statusbar_setup_ui;
pub mod statusbar_sparkline;
pub mod statusbar_text;
pub mod git_state;
pub mod git_wrapper;

#[cfg(windows)]
pub mod conpty;

#[cfg(windows)]
pub mod probe;
#[cfg(windows)]
pub mod pty;
#[cfg(windows)]
pub mod jobwatch;
#[cfg(windows)]
pub mod coproc_win;
#[cfg(windows)]
pub mod metrics_win;
#[cfg(windows)]
pub mod git_run_win;

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
pub mod git_log_ui;
#[cfg(windows)]
pub mod status_popover_ui;
#[cfg(windows)]
pub mod shot_ui;
#[cfg(windows)]
pub mod global_find_ui;
#[cfg(windows)]
pub mod open_quickly_ui;

pub mod settings_keys;
pub mod settings_advanced;
pub mod settings_triggers;
pub mod settings_lists;
pub mod settings_arrangements;
pub mod settings_shortcuts;
pub mod settings_hotkey;
pub mod restore_state;
pub mod host_proto;
pub mod host;
pub mod host_client;
pub mod reattach;
pub mod hotkey;
pub mod broadcast;
mod tmux_probe;
pub mod tmux_hook;
pub mod tmux_proto;
pub mod tmux_gateway;
pub mod tmux_handshake;
pub mod tmux_layout;
pub mod tmux_monitor;
pub mod tmux_state;
pub mod tmux_windows;
pub mod tmux_clients;
pub mod tmux_controller;
pub mod tmux_keys;
pub mod tmux_sizes;
pub mod tmux_edits;
pub mod tmux_persist;
pub mod tmux_pause;
pub mod tmux_dashboard;
pub mod tmux_dashboard_ui;
pub mod bury;
pub mod tmux_prefix;
pub mod doubletap;
pub mod hotkey_motion;
pub mod winpipe;
pub mod toolbelt;
pub mod toolbelt_layout;
pub mod tool_list;
pub mod toolbelt_tools;
pub mod text_edit;
pub mod composer;
pub mod notes;
pub mod open_quickly;

#[cfg(windows)]
pub mod winclip;
#[cfg(windows)]
pub mod winprint;
#[cfg(windows)]
pub mod winkeys;

#[cfg(windows)]
pub mod notify;
