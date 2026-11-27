# Termurion roadmap

Where the port stands and what comes next. Update this file when a milestone or a release lands.

## Status

Termurion started on 2026-09-23. Every planned parity milestone is done. 1.0.0, the first stable
release, is out on Windows, macOS, and Linux.

| | |
|---|---|
| Release | 1.1.12 (2026-11-27), the current stable release, on Windows, macOS, and Linux. 1.1.12 draws with native Metal on macOS and makes effects cheap enough to leave on ([`PERFORMANCE.md`](PERFORMANCE.md)). 1.1.0 added FX profiles ([`help/fx-profiles.md`](help/fx-profiles.md)) |
| Milestones done | 0 to 14, including 13a to 13y2 and the AI browser tools |
| Tests | 7,995 (`#[test]` functions in `src` and `tests`) |
| Rust | about 645,000 lines (`src`, `crates`, and `tests`) |
| Commits | about 2,860 (`git rev-list --count HEAD`) |

## Next

- macOS and Linux ports, from one tree with a shared Unix layer: [`superpowers/specs/2026-10-31-macos-linux-port-design.md`](superpowers/specs/2026-10-31-macos-linux-port-design.md). P0 (compile everywhere) is done: the whole crate and all five programs build on Linux and macOS, and a feature with no Unix backend reports "<feature> is not available on <OS> yet". Test status on Unix: [`port/unix-test-gaps.md`](port/unix-test-gaps.md). P1 (the Unix core) is done: a local shell, the session host, job tracking, paths, the clipboard, tmux `-CC` and window restore run on Linux and macOS, and the Win32 twins moved under `src/platform/`; its Windows test run is pending until the Windows host is back. P2 (macOS: the Cmd shortcut key, the native menu bar, Quit and Dock handling, the ad hoc signed `Termurion.app`, system-wide hotkeys, notifications, the Keychain and Touch ID, quarantine, Finder aliases and the iTerm2 settings import) is done on the macOS VM; the owner's own run on a Mac and the Windows test run are pending, and the record is in [`port/unix-test-gaps.md`](port/unix-test-gaps.md). P3 (Linux: an AppImage, the GNOME tray icon, the launch socket, Wayland and X11) is done. 1.0.0 ships all three builds. It has the same code as 0.0.1-beta11, the first release with macOS and Linux builds. Render plugins (beta8, [`render-plugins.md`](render-plugins.md)) run on wgpu on Windows and Linux, and on Metal on macOS since 1.1.12.
- Flood throughput, the next performance item. On macOS a flood of output takes 86 MB/s in Termurion against 122 MB/s in Ghostty on the same run. The frame rate holds at about 60 frames a second and an effect costs nothing there, so the gap is in reading and parsing output, not in drawing ([`PERFORMANCE.md`](PERFORMANCE.md)).
- tmux on Windows: psmux (native), WSL tmux, and tmux over ssh each have a live suite (FIXES item 3, and `docs/probe/2026-10-14-psmux-wsl-tmux-results.md`).
- Fixes from people who use the stable release. File bugs at https://github.com/datbird/termurion/issues.
- The open items in [`FIXES.md`](FIXES.md) (found after the first beta) and [`TWEAKS.md`](TWEAKS.md).
- The rows still marked `partial` or `missing` in the parity inventory.
- Linux app stores, after 1.1.12. Flathub first: it feeds the Mint Software Manager, GNOME Software, KDE Discover, Fedora, Pop!_OS and the Steam Deck. It needs a Flatpak build from source with vendored crates, AppStream metadata with screenshots, and shells started on the host through `flatpak-spawn --host` (a pty and host layer change), then a reviewed PR to `flathub/flathub`. The Snap Store second (the Ubuntu App Center): a `snapcraft.yaml` with classic confinement, which Snap Store staff approve by hand; no code change. A `.deb` on each GitHub release is a quick extra. Debian and Ubuntu's own archives are out: they need every Rust crate packaged in Debian.

## Where the detail lives

- The parity roadmap and inventory: [`parity/00-roadmap.md`](parity/00-roadmap.md).
- One hands-on checklist per milestone: [`acceptance/`](acceptance/).
- Plans and rulings for each milestone (every kept iTerm2 quirk and every Windows-specific
  choice): [`superpowers/`](superpowers/).
