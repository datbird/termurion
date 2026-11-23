# iTerm2 upstream since the fork (triage, 2026-11-23)

Termurion was ported from iTerm2 at 64791fc (2026-09-23). Upstream master is at a81a54b7 (2026-11-23): 1136 commits since then. Tags in the range: v3.7.4beta1, v3.7.5, v3.7.7beta1, v3.7.7beta2, v3.7.8beta1, v3.7.8beta2, v3.7.8, v3.7.10beta1.

| Class | Commits |
|---|---|
| A, port or check | 320 |
| B, new features Termurion lacks | 184 |
| C, not relevant (Cocoa, translations, build, tests, refactors, iPhone app) | 632 |

Counts come from a rule-based sort plus a read of the 3.7 release notes and the main diffs; they are accurate to a few percent.

## Security fixes (port first)

| Upstream | Change | Termurion today |
|---|---|---|
| 5964ff23 | Instant Replay import applies only an allowlist of drawing keys (a crafted recording could carry triggers that run) | copies the whole profile from the file, `src/app/replay_ui.rs:444-458` |
| 15d9b471, 00ff77e7 | SetWindowFrame is validated and clamped | checks only for four numbers, `src/marks.rs:1644`, `src/app/requests_ui.rs:295-305`; a program can shrink the window to 1 px or move it off screen |
| 807bf9b1 | kitty `t=f` refuses /etc, /var/db, /var/root; error text "EBADF:Failed to read image file" | no denylist on Linux or macOS, `src/kitty.rs:80`; text "EBADF:Cannot use the file" |

## Behavior Termurion still has the old way

- tmux `%exit` inside a reply is data on tmux 1.9+ (cecaae04); Termurion ends the connection, `src/tmux_gateway.rs:300-304`.
- tmux keeps a zoomed window zoomed on attach (ba2f3ca8); Termurion unzooms, `src/tmux_controller.rs:1480`.
- DECRQCRA reply is always four hex digits; Termurion prints eight, `src/emu.rs:133` (test at :498).
- Inline images: too wide takes the grid width, and the 255-row cap narrows the width; `src/images.rs:143-154`.
- kitty image cache evicts the least recently displayed; Termurion evicts the oldest, `src/kitty.rs:25`.
- A skin-toned emoji is one 2-column cell; Termurion keeps the modifier in its own cell, `crates/alacritty_terminal/src/term/mod.rs:1631`.
- Show Clippings moved to cmd-shift-opt-G; Termurion keeps shift+cmd+= (collides with Make Text Bigger), `schema/menus.toml:1209`.
- Advanced setting `NoSyncDisableDECRQCRA` renamed `DisableDECRQCRA`, `schema/global.toml:4530`.
- Session Status checkbox now "Show status on tabs"; Termurion "Show status in tab subtitle", `src/status_tool.rs:321`.
- AI model catalog grew 493 lines; `src/ai_catalog.rs` has the fork-point copy.
- API protocol 1.24 and Python library 2.31; Termurion reports 1.19 and 2.24, `src/api_proto.rs:12`, `src/api_server.rs:969`.

## Missing settings, menus and small behavior

- Profile keys: Bell Sound, HDR Cursor Brightness.
- Global keys: the Quick Look hover keys, ShowMenuBarItem, MenuBarItemString, ShowTabSwitcherWhenCyclingTabs, FFM Including Subviews, MaximumConcurrentTokenExecutions.
- 15 Advanced settings, including progressReportTimeout (15), sshFlowControl, offerToResumeAgentConversations.
- Menus: Lock Location, Floating Panes (and submenu), Move Tab to New Sub-group, Web > Show Toolbar. Key actions 77 New Floating Pane, 78 Set Character Encoding.
- OSC 9;4 15-second progress timeout; mode 2031 offer to turn light/dark reports off; right-prompt marks (OSC 133;P;k=r) and the tcsh script; the nested `tmux -CC` warning; Grok and the manual-model API key choice; Password Manager export and import.
- API: session notes, `async_eval_javascript`, context-menu submenus, `it2 session list --format`, OSC 7501 Program Status Protocol, BDSM/SCP, DECCRA/DECFRA/DECERA.
- No existing default changed upstream.

## New features (class B)

| Feature | Size |
|---|---|
| Floating panes (protocol 1.21) | L |
| SSH Integration overhaul (93 commits: output pacing, flow control, `it2` over SSH, remote git state, Claude Code on remote hosts; `resources/ssh/framer.py` is the fork-point file, upstream grew 1359 lines) | L |
| Workgroups in tmux Integration | L |
| Nested tab groups (protocol 1.24) | M |
| Control-Tab tab switcher | M |
| Quick Look on the browser engine and on hover | M |
| Menu bar item and `iterm2.workingSessionCount` variables | M |
| Output-fairness limits | M |
| nushell shell integration | M |
| imgcat Markdown rendering | S |
| Code review model picker | S |
| Workgroup browser that follows a variable | S |

## Already matching upstream

kitty reply and OSC 72 echo hardening, title-report sanitizing, per-screen kitty keyboard flags, kitty over modifyOtherKeys, safe huge CSI counts (vte u16), DECRQCRA "Yes" remembered.
