# Built-in render plugins

The 13 cursor effects here are from ghostty-cursor-fx by AONIR (MIT, see each folder's
LICENSE), taken unchanged at commit ed2816d0ed0c93ed90a5e6f11a9563fa8fbdeecf:
https://github.com/Avarce/ghostty-cursor-fx

The shader prefix that runs them (`src/render_fx_prefix.glsl`) is adapted from Ghostty's custom
shader prefix, MIT, copyright (c) 2024 Mitchell Hashimoto, Ghostty contributors (see
`LICENSE-ghostty`).

`hooks/` holds the Claude Code side. Copy `hooks/fx-signal.sh` to `~/.termurion/` on the host
where Claude Code runs, then merge `hooks/claude-code-hooks.json` into that host's
`~/.claude/settings.json`. Put your effect's `cursor_color` (from its `plugin.toml`) in both
commands. See docs/render-plugins.md.
