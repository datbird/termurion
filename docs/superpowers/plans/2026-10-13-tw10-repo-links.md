# TWEAKS 10: every iTerm2 link points at the Termurion repository

Robert (2026-10-13): Help and About already open the repo (684fd3da). Change every other link
to iTerm2's site too. His answers: documentation links open one repo help page per topic; download
links point at the repo now (they fail while the repo is private, accepted).

## Bases (one module, `src/links.rs`)

- `REPO` = `https://github.com/datbird/termurion` (move `update::REPO_URL` here, keep `update::REPO_URL` as a re-export or update its users).
- `ISSUES` = `https://github.com/datbird/termurion/issues`
- `help(slug)` = `https://github.com/datbird/termurion/blob/main/docs/help/<slug>.md` (anchors: `help_at(slug, anchor)` adds `#anchor`).
- `raw(path)` = `https://raw.githubusercontent.com/datbird/termurion/main/<path>`
- Constants where a `const &str` is needed (`concat!` from one base macro), so every URL comes from one place.

## Map (old -> new)

| Old | New |
|---|---|
| iterm2.com/shell_integration.html, documentation-shell-integration.html (http and https) | help `shell-integration` |
| www.iterm2.com/smartselection.html | help `smart-selection` |
| www.iterm2.com/colorgallery | help `color-presets` |
| iterm2.com/python-api (also in repl_banner.txt, script_archive.rs) | help `python-api` |
| python-api-security-model | help `python-api-security` |
| python-api-auth.html | help `python-api-auth` |
| documentation-scripting-fundamentals.html (+ text anchor) | help `scripting-fundamentals` (anchor `interpolated-strings` where the old one pointed there) |
| documentation-variables.html | help `variables` |
| documentation-copymode.html | help `copy-mode` |
| documentation-triggers.html | help `triggers` |
| search_syntax.html | help `search-syntax` |
| paste_bracketing | help `paste-bracketing` |
| images.html | help `inline-images` |
| dynamic-profiles.html | help `dynamic-profiles` |
| documentation-web.html | help `web-browser` |
| coprocesses.html | help `coprocesses` |
| claude-code-integration.html | help `claude-code` |
| captured_output.html (http and https) | help `captured-output` |
| badges.html | help `badges` |
| 3.3/documentation-status-bar.html, status-bar-layout | help `status-bar` |
| gitlab wikis/TmuxIntegration, iterm2.com//tmux22bug.html | help `tmux-integration` (tmux22bug -> anchor `tmux-22`) |
| gitlab -/wikis/SSH-Integration | help `ssh-integration` |
| lastpass-cli | help `password-manager` |
| documentation-workgroups.html | help `workgroups` |
| iterm2.com/bugs, bugs. | `ISSUES` (keep the trailing period of the sentence as text) |
| iterm2.com/, www.iterm2.com/, gitlab.com/gnachman/iterm2/ | `REPO` |
| shell_integration/install_shell_integration.sh | raw `dist/install_shell_integration.sh` |
| shell_integration/install_shell_integration_and_utilities.sh | raw `dist/install_shell_integration_and_utilities.sh` |
| download.sh | raw `dist/download.sh` |
| downloads/pyenv/manifest.json, betamanifest.json, uv/manifest.json, ai/manifest.json | raw `dist/downloads/pyenv/manifest.json` etc. (setting defaults only; nothing in Termurion fetches them, the AI help text already says so) |

## Keep (not links to iTerm2's site)

- `api.iterm2.com` WebSocket subprotocol (iTerm2's Python library sends it).
- LICENSE-iTerm2 / COPYING-iTerm2 files and source comments that cite iTerm2 code (`AD:402` style references, `iterm2-ref` paths).
- iTerm2 compatibility keys (plist domain, `iTerm.app`, selectors, OSC 1337).

## Repo files

- `docs/help/README.md` index + one `<slug>.md` per help slug above. Each page describes what
  Termurion does on Windows (real menu paths and default keys, Shortcut Key default Ctrl+Shift),
  read from Termurion's source, with iTerm2's docs in `~/gitrepos/iterm2-ref` as background.
  Plain English, short sentences, no em dashes, no emoji.
- `dist/install_shell_integration.sh`, `dist/install_shell_integration_and_utilities.sh`: adapted
  from `~/gitrepos/iterm2-ref/submodules/iTerm2-shell-integration/shell_integration/`, fetching from
  the repo raw base (scripts under `resources/ssh/shell_integration/`, utilities under `dist/utilities/`).
  GPL notice kept.
- `dist/download.sh`: the download helper the tip offers (from the same submodule's utilities).

## Checks

- A test fails if any `iterm2.com` or `gitlab.com/gnachman` URL appears in `src/`, `schema/` or
  `resources/` outside the Keep list.
- A test that every `help(slug)` used in code has `docs/help/<slug>.md`.
- `tips_urls_are_fixed` accepts only the repo bases.
- Full suite on the Windows test machine via `scripts/xt test --release`.
