# Tandem Milestone 5c: Clipboard, the Remaining OSC 1337 Keys, Tab Status, and the Copy Commands

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's clipboard sequences (OSC 52 write and read with iTerm2's policy, OSC 1337 `Copy=` and `CopyToClipboard`/`EndCopy`), the escape-sequence trust gate, the in-pane announcement bar that iTerm2 uses to ask, the small OSC 1337 keys (CursorShape, ClearScrollback, HighlightCursorLine, AddNote aliases, RequestAttention, ReportCellSize, SetWindowFrame, ReportWindowFrame, ReportScreenFrames, UnicodeVersion, SetColors, Capabilities, ExecFailed, Disinter, Env, SetKeyLabel, Push/PopKeyLabels, StealFocus, BeginFile/EndFile), OSC 133 E to H, OSC 21337 tab status, Edit > Copy with Styles and Copy with Control Sequences, Edit > Undo/Redo/Cut as iTerm2 enables them, and Shell > Toggle Buffer Input.

**Architecture:** The scanner catches the new OSCs; `Screen` acts on what is terminal state and queues `Request`s for what needs the window (focus, frames, flashing, the clipboard, prompts), which the app drains after each pump as it drains downloads. Pure modules decide: `infobar.rs` (the bar queue and its layout), `clipboard_osc.rs` (OSC 52 and the 1337 copy protocols), `osc1337.rs` (value grammars and replies), `tabstatus.rs` (OSC 21337), `styled_copy.rs` (HTML, RTF, and SGR text from cells). alacritty's own OSC 52 handling is turned off so Tandem applies iTerm2's policy.

**Tech Stack:** as milestone 5b.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. Inventory rows: `03-features-and-menus.md` section 2 (OSC 1337 keys named above, OSC 133 E-H, OSC 21337, OSC 52), Edit > Undo, Redo, Cut, Copy with Styles, Copy with Control Sequences, Shell > Toggle Buffer Input; `01-profile-settings.md` Buffer Input by Default, Unicode Version; `02-global-settings.md` AllowClipboardAccess, DisablePotentiallyInsecureEscapeSequences. iTerm2 facts: `.superpowers/research/m5c-iterm2-facts.md` (copied into the ledger's workspace).

## Global Constraints

- Everything from milestones 0 to 5b still holds.
- Trust is one global advanced setting, `DisablePotentiallyInsecureEscapeSequences` (default off, so all output is trusted), as in iTerm2; there is no per-source trust.
- Clipboard writes need the global `AllowClipboardAccess` (default off, General > Selection "Applications in terminal may access clipboard"). Clipboard reads have their own policy: never, always, ask each time (default ask).
- Anything that asks the user uses the announcement bar in the pane (iTerm2's nagging bar), never a modal dialog, except where iTerm2 uses a modal (StealFocus).
- Windows stand-ins: dock bounce is `FlashWindowEx`; AppKit bottom-left points are Windows virtual-screen DIPs with y flipped against the primary monitor; the Touch Bar has no equivalent (labels are kept, not shown).
- Moved out of 5c, with the rows saying where: milestone 5d (Print and Page Setup, Make Screenshot, SetProfile, SetProfileProperty, SetBackgroundImageFile, OpenURL, ReportVariable, RequestUpload, Block, UpdateBlock, Button); milestone 6 (SetBadgeFormat, Notification, OSC 9, ClearCapturedOutput, Custom); a new SSH integration milestone (it2ssh, SendConductor, EndSSH, wrap, Shell > ssh menu).

## Review Focus

1. A 1 MiB OSC 52 payload, and one past 1 MiB. Expect: the first is copied, the second is cut at the cap, not an unbounded buffer. Test: Task 2 `osc52_caps_the_payload`.
2. OSC 52 read with the default policy. Expect: nothing is sent until the user picks Just Once or Always in the bar; Never answers nothing from then on. Test: Task 2 `osc52_read_follows_policy`.
3. `CopyToClipboard` with no `EndCopy` (a program that crashed) and 200 MB of output after it. Expect: the capture stops at 100 MiB, output still draws. Test: Task 2 `copy_to_clipboard_caps_its_capture`.
4. OSC 21337 with escaped `\;` and `\\`, a key without `=`, and an unknown key. Expect: iTerm2's parse; nothing else changes. Test: Task 4 `tab_status_parses_escapes`.
5. Copy with Styles over wide characters, OSC 8 links, reverse video, and hidden cells. Expect: one run per style change, links as links, hidden text drawn in the background color. Test: Task 5 `styled_runs_follow_cells`.

---

### Task 1: The announcement bar

**Files:** Create `src/infobar.rs`; modify `src/scene.rs`, `src/app/mod.rs` (a new `src/app/infobar_ui.rs`).

**Interfaces:** `Bar { id: String, text: String, buttons: Vec<String> }`; `Bars` per pane: `push(bar) -> bool` (false when a bar with that id already waits), `front() -> Option<&Bar>`, `pop()`; `layout(bar, area, cell) -> (Rect /*bar*/, Vec<Rect> /*buttons, right to left*/)`; `hit(bar, area, cell, x, y) -> Option<usize>` (button index), a close box answers `None` with the bar dismissed. The app draws the front bar across the top of the pane (text left, buttons right, a close box); a click runs the pending `BarAction` for that bar's id and button. Choices that persist go in the global store under iTerm2's `NoSync*` keys.

- [ ] Failing tests: `bars_queue_one_per_id`, `buttons_lay_out_right_to_left`, `clicks_hit_buttons_and_close`. Commit `Add the announcement bar`.

### Task 2: Clipboard sequences and the trust gate

**Files:** Create `src/clipboard_osc.rs`; modify `src/osc.rs`, `src/screen.rs`, `src/marks.rs`, `crates/alacritty_terminal` (patch 8: DA1 `;52` follows a flag), `schema/global.toml`, `src/settings_ui.rs`, `src/app/*`.

**Interfaces:** Globals `AllowClipboardAccess` (bool false, General > Selection) and `DisablePotentiallyInsecureEscapeSequences` (bool false, Advanced > Security); the General and Advanced settings pages show the settings the schema has for them (the rest stays milestone 13). `Screen.trusted: bool` and `Screen.clipboard_access: bool`, set by the app. `clipboard_osc::osc52(payload) -> Option<Osc52 { Write(String) | Read(String /*Pc*/) }>`: Pc any run of `psc01234567` (ignored for writes), base64 body, C0 controls other than TAB LF CR removed, text cut at NUL. Scanner keeps `52;` payloads up to 1 MiB (past that, cut and finished) and `1337;Copy=` up to 512 MB. `Copy=` modes: empty or `1` monolithic, `2;uid` begin, `3;uid` append (uid mismatch aborts), `4[;uid]` end. `CopyToClipboard=<name>` captures the raw bytes of later output (they still draw) until `EndCopy`, 100 MiB cap (flushed at the cap). Screen queues `Request::ClipboardWrite(String)`, `Request::ClipboardRead(String)`, `Request::ClipboardDenied`; untrusted drops `Copy=`, `CopyToClipboard`, `EndCopy`. App: a write goes to the clipboard when access is on, else one bar "The terminal attempted to access the clipboard but it was denied." (Open Settings / Don't Show This Again, `NoSyncSuppressClipboardAccessDeniedWarning`); a read follows `NoSyncNeverAllowPaste` (never) and `NoSyncAllowPaste` (always), else a bar "Share clipboard contents with app in terminal?" (Just Once / Always / Never); the answer is `ESC ] 52 ; <Pc> ; <base64> ESC \`. DA1 includes `52` when access is on, or when it was never set and the warning is not suppressed. alacritty's OSC 52 is off (`Osc52::Disabled`).

- [ ] Failing tests: `osc52_writes_when_allowed`, `osc52_strips_controls`, `osc52_caps_the_payload`, `osc52_read_follows_policy` (pure policy function), `copy_monolithic_and_segmented`, `copy_to_clipboard_caps_its_capture`, `untrusted_sequences_are_refused`, `da1_advertises_52_by_policy`. Commit `Add the clipboard sequences and the trust gate`.

### Task 3: The small OSC 1337 keys and OSC 133 E to H

**Files:** Create `src/osc1337.rs`; modify `src/marks.rs`, `src/screen.rs`, `src/app/*` (a new `src/app/requests_ui.rs`), `src/scene.rs`.

**Interfaces:** Screen acts on: `CursorShape=N` (0 block, 1 bar, 2 underline; other numbers ignored, non-numbers 0), `ClearScrollback` (clears screen and scrollback, keeping the prompt lines, as iTerm2's `clearBufferSavingPrompt`), `HighlightCursorLine[=bool]` (queues `Request::CursorGuide(bool)`; empty is yes; NSString boolValue rules), `AddNote`/`AddHiddenNote` (aliases), `BeginFile`/`EndFile` (ignored), `UnicodeVersion=N|push [label]|pop [label]` (8 or 9 only, 0 the profile's; the pane's version is kept and reported, width tables unchanged), `SetColors=name=color,...` (the ANSI and theme names change the pane's palette; `tab` sets the pane's tab color; `badge` and `preset` are kept for milestone 6's badge and the color presets, which Tandem does not have yet; colors `RRGGBB`, `RGB`, optional `srgb:`/`rgb:`/`p3:` prefix, p3 treated as sRGB, `i:N` a palette index), `Capabilities` (T; the reply built from Tandem's real features in iTerm2's order), `ExecFailed` (a flag the pane keeps), `Disinter` (nothing to restore until milestone 7's Bury), `Env=report=all:<b64>` (T; kept for the SSH integration milestone), `SetKeyLabel`, `PushKeyLabels`, `PopKeyLabels` (kept, not shown), OSC 133 `E`, `F`, `G`, `H` (parsed and ignored, as iTerm2's empty handlers). Queued for the app: `Request::StealFocus` (T; a modal warning "A control sequence attempted to activate a session. Allow it?" Allow / Deny), `Request::Attention(Attention { Fireworks, Once, Flash, Start, Stop })` (`FlashWindowEx` or an in-pane flash or fireworks), `Request::ReportCellSize`, `Request::SetWindowFrame(x, y, w, h)` (refused when the profile disables window resizing), `Request::ReportWindowFrame`, `Request::ReportScreenFrames` (T). Replies: `ESC ] 1337 ; ReportCellSize=<h>;<w>;<scale> ESC \` in points (`%0.2f`, trailing zeros removed), `ESC ] 1337 ; WindowFrame=x;y;w;h ESC \`, `ESC ] 1337 ; ScreenFrames=x,y,w,h;... ESC \` (work areas), `ESC ] 1337 ; Capabilities=<codes> BEL`.

- [ ] Failing tests: `cursor_shape_and_clear_scrollback`, `set_colors_parse`, `unicode_version_stack`, `key_label_stack`, `capabilities_follow_features`, `number_format_matches_iterm2`, `frames_flip_to_appkit_points`, `osc133_e_to_h_are_ignored`, `requests_reach_the_app`. Commit `Add the remaining OSC 1337 keys`.

### Task 4: OSC 21337 tab status

**Files:** Create `src/tabstatus.rs`; modify `src/osc.rs`, `src/screen.rs`, `src/scene.rs`, `src/app/mod.rs`, `schema/global.toml`.

**Interfaces:** `parse(payload) -> Vec<(String, String)>` (split on unescaped `;`, `\;` and `\\` unescape, other `\x` kept, no `=` gives an empty value); `Status { indicator, status, status_color, detail: Option<String> }` with `apply(pairs) -> bool` (absent unchanged, empty cleared, value set; unknown keys ignored); `aggregate(focused: Option<&Status>, others, priorities) -> Option<Status>` (the focused pane's status wins, else the highest-priority status by case-insensitive substring, defaults `wait`, `work`, `idle`). The status clears on prompt start (OSC 133 A). The tab bar draws the aggregated indicator as a dot and, when the global `ShowSessionStatusInTabSubtitle` (default on) is on, the status text in its color before the title.

- [ ] Failing tests: `tab_status_parses_escapes`, `fields_are_tri_state`, `focused_pane_wins_then_priority`, `prompt_start_clears_status`, scene `tab_shows_status_dot`. Commit `Add OSC 21337 tab status`.

### Task 5: Copy with Styles, Copy with Control Sequences, Undo/Redo/Cut, Buffer Input

**Files:** Create `src/styled_copy.rs`, `src/winclip.rs`; modify `src/app/menu.rs`, `src/commands.rs`, `src/pane.rs`, `src/scene.rs`.

**Interfaces:** `runs(screen, selection) -> Vec<Line { runs: Vec<Run { text, fg, bg, bold, italic, underline, link: Option<String> }>, hard_end: bool }>` (fg blended onto bg, hidden cells fg = bg, reverse swapped, default background included as iTerm2's `copyBackgroundColor` default); `html(lines, font) -> String` (CF_HTML with inline spans, escaped); `rtf(lines, font) -> String` (color table, `\cf`, `\highlight`, `\b`, `\i`, `\ul`); `sgr_text(screen, selection) -> String` (iTerm2's extractor: `ESC[0...m` on each style change, 30-37/90-97/40-47/100-107, `38:5:n`, `38:2:1:r:g:b`, 1 2 3 4 4:3 21 4:4 4:5 5 7 8 9, a hard line end `ESC[0m\n`, wrapped rows joined, ends `ESC[0m`, no trimming; bright backgrounds as 100-107, not iTerm2's 108-115 bug). `winclip::set(text, html, rtf)` writes CF_UNICODETEXT, "HTML Format", and "Rich Text Format" in one open. Undo and Redo stay disabled and Cut is never enabled in a session, as iTerm2 (their rows close). Shell > Toggle Buffer Input (a check item) holds everything the user sends to the pane (keys, pastes) while on, releases it in order when off; replies to the program are not held; a pause glyph at the pane's top right shows while on; the profile's Buffer Input by Default starts it.

- [ ] Failing tests: `styled_runs_follow_cells`, `html_escapes_and_colors`, `rtf_has_a_color_table`, `sgr_copy_matches_iterm2`, `wrapped_rows_join_without_newline`, `buffered_input_waits_for_release`. Commit `Add Copy with Styles, Copy with Control Sequences, and Buffer Input`.

### Task 6: Parity and acceptance

- [ ] Update the rows 5c closes and the rows it moves (5d, 6, the SSH integration milestone), add the SSH integration milestone and 5d to `00-roadmap.md`, recount, write `docs/acceptance/m5c-checklist.md`. Commit.
