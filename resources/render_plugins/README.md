# Built-in render plugins

The 13 cursor effects here are from ghostty-cursor-fx by AONIR (MIT, see each folder's
LICENSE), taken unchanged at commit ed2816d0ed0c93ed90a5e6f11a9563fa8fbdeecf:
https://github.com/Avarce/ghostty-cursor-fx

The shader prefix that runs them (`src/render_fx_prefix.glsl`) is adapted from Ghostty's custom
shader prefix, MIT, copyright (c) 2024 Mitchell Hashimoto, Ghostty contributors (see
`LICENSE-ghostty`).

Termurion compiles the effects with two changes made in memory, never in these files: the
switches under Settings > Profiles > Window > Effects set their `const bool` settings, and
`render_fx::patch_input_box` points their input box search at the lines Termurion reads from the
grid, so a box that grows with a long prompt keeps its effects.

`hooks/` holds the Claude Code side (optional: Termurion makes the signals itself). Copy `hooks/fx-signal.sh` to `~/.termurion/` on the host
where Claude Code runs, then merge `hooks/claude-code-hooks.json` into that host's
`~/.claude/settings.json`. Put your effect's `cursor_color` (from its `plugin.toml`) in both
commands. See docs/render-plugins.md.

## Testing with AI agents

Do this for each agent: Claude Code 2.1.319, Codex 0.167.0, opencode 1.19.5, Hermes Agent 0.19.0
and agy 1.5.3.

1. Pick the Shockwave effect and turn it on in Settings > Profiles > Window > Effects.
2. Start the agent in Termurion.
3. Type a word, press Enter, and wait for the answer.
4. Check these:
   - The cursor ring shows.
   - On Enter, both lines of the input box light and take the effect's color. This is for Claude
     Code, Hermes Agent and agy. For Codex and opencode the input box is shaded, so both edges
     of the shaded box light instead.
   - The prompt mark changes look. Skip this for opencode, which has no prompt mark.
   - The done surge plays when the answer is in.
