//! The built-in render plugins, compiled in: the 13 cursor effects of ghostty-cursor-fx
//! (MIT, copyright 2026 AONIR; resources/render_plugins/README.md). Generated from that folder.

/// (folder name, plugin.toml, [(shader file, text)]).
pub type Builtin = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

pub const LICENSE: &str = include_str!("../resources/render_plugins/aurora/LICENSE");

pub const BUILTIN: &[Builtin] = &[
    ("aurora", include_str!("../resources/render_plugins/aurora/plugin.toml"), &[("aurora-cursor.glsl", include_str!("../resources/render_plugins/aurora/aurora-cursor.glsl"))]),
    ("cyber", include_str!("../resources/render_plugins/cyber/plugin.toml"), &[("cyber-cursor.glsl", include_str!("../resources/render_plugins/cyber/cyber-cursor.glsl"))]),
    ("embers", include_str!("../resources/render_plugins/embers/plugin.toml"), &[("ember-cursor.glsl", include_str!("../resources/render_plugins/embers/ember-cursor.glsl"))]),
    ("fireworks", include_str!("../resources/render_plugins/fireworks/plugin.toml"), &[("firework-cursor.glsl", include_str!("../resources/render_plugins/fireworks/firework-cursor.glsl"))]),
    ("glitch", include_str!("../resources/render_plugins/glitch/plugin.toml"), &[("glitch-cursor.glsl", include_str!("../resources/render_plugins/glitch/glitch-cursor.glsl"))]),
    ("hologram", include_str!("../resources/render_plugins/hologram/plugin.toml"), &[("hologram-cursor.glsl", include_str!("../resources/render_plugins/hologram/hologram-cursor.glsl"))]),
    ("katana", include_str!("../resources/render_plugins/katana/plugin.toml"), &[("katana-cursor.glsl", include_str!("../resources/render_plugins/katana/katana-cursor.glsl"))]),
    ("lightning", include_str!("../resources/render_plugins/lightning/plugin.toml"), &[("lightning-cursor.glsl", include_str!("../resources/render_plugins/lightning/lightning-cursor.glsl"))]),
    ("mecha", include_str!("../resources/render_plugins/mecha/plugin.toml"), &[("mecha-cursor.glsl", include_str!("../resources/render_plugins/mecha/mecha-cursor.glsl"))]),
    ("neon", include_str!("../resources/render_plugins/neon/plugin.toml"), &[("neon-cursor.glsl", include_str!("../resources/render_plugins/neon/neon-cursor.glsl"))]),
    ("phosphor", include_str!("../resources/render_plugins/phosphor/plugin.toml"), &[("phosphor-cursor.glsl", include_str!("../resources/render_plugins/phosphor/phosphor-cursor.glsl"))]),
    ("shockwave", include_str!("../resources/render_plugins/shockwave/plugin.toml"), &[("shockwave-cursor.glsl", include_str!("../resources/render_plugins/shockwave/shockwave-cursor.glsl"))]),
    ("teleport", include_str!("../resources/render_plugins/teleport/plugin.toml"), &[("teleport-cursor.glsl", include_str!("../resources/render_plugins/teleport/teleport-cursor.glsl"))]),
];
