# Termurion roadmap

Where the port stands and what comes next. Update this file when a milestone or a release lands.

## Status

Termurion started on 2026-09-23. Every planned parity milestone is done, and the first public
beta is out.

| | |
|---|---|
| Release | 0.0.1-beta8 (2026-10-31) |
| Milestones done | 0 to 14, including 13a to 13y2 and the AI browser tools |
| Tests | 6,785 |
| Rust | about 496,000 lines |
| Commits | about 2,230 |

## Next

- macOS and Linux ports, from one tree with a shared Unix layer: [`superpowers/specs/2026-10-31-macos-linux-port-design.md`](superpowers/specs/2026-10-31-macos-linux-port-design.md). Render plugins (beta8, [`render-plugins.md`](render-plugins.md)) already use only wgpu.
- tmux on Windows: psmux (native), WSL tmux, and tmux over ssh each have a live suite (FIXES item 3, and `docs/probe/2026-10-14-psmux-wsl-tmux-results.md`).
- Fixes from beta testers. File bugs at https://github.com/datbird/termurion/issues.
- The open items in [`FIXES.md`](FIXES.md) (found after the beta) and [`TWEAKS.md`](TWEAKS.md).
- The rows still marked `partial` or `missing` in the parity inventory.

## Where the detail lives

- The parity roadmap and inventory: [`parity/00-roadmap.md`](parity/00-roadmap.md).
- One hands-on checklist per milestone: [`acceptance/`](acceptance/).
- Plans and rulings for each milestone (every kept iTerm2 quirk and every Windows-specific
  choice): [`superpowers/`](superpowers/).
