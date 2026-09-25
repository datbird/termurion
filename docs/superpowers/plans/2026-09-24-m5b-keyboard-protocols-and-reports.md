# Tandem Milestone 5b: Keyboard Protocols, Terminal State, and Terminal Reports

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's keyboard protocols (modifyOtherKeys 1 and 2, CSI u, raw key reporting, the kitty keyboard protocol with all five flags), the Session > Terminal State keyboard modes, Literal Controls, and Emulation Level menus, the profile's "Apps can change how keys are reported" and "Report keys using CSI u", and the reports iTerm2 answers: DA1 by emulation level, DA2, DA3, XTVERSION, `CSI 1337 n`, DECRQSS, XTGETTCAP, synchronized output (mode 2026 and the DCS form), and the dark-mode query (996/997 and mode 2031).

**Architecture:** A pure `keyproto.rs` turns a key press or release into bytes for each protocol. `Screen` keeps the pane's keyboard state (modifyOtherKeys level from an alacritty patch, kitty flags from alacritty's stack, the menu's mode and flag overrides, emulation level, literal mode, 2031 reports) and answers reports; the scanner also catches the few CSI and DCS queries alacritty does not answer. The app sends key releases when a protocol wants them.

**Tech Stack:** as milestone 5a.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. Inventory rows: `03-features-and-menus.md` Session > Terminal State (milestone 5 rows), section 2 (DECRQSS, kitty keyboard, synchronized output, 996/997, DA); `01-profile-settings.md` Allow modifyOtherKeys, Use libtickit protocol.

## Global Constraints

- Everything from milestones 0 to 5a still holds. iTerm2 facts are the research in the ledger (iTermModernKeyMapper.swift, iTermModifyOtherKeysMapper.m, iTermTermkeyKeyMapper.m, iTermRawKeyMapper.m, VT100Output.m, VT100Terminal.m).
- Kitty flags, when any is set, win over the mode radio group, as in iTerm2. Apps change modes only when the profile allows it (default yes).
- Win (Cmd) is never reported as a modifier, as iTerm2 never reports Cmd.
- Raw key reporting sends Windows virtual key codes where iTerm2 sends macOS key codes (the format is iTerm2's).

## Review Focus

1. Kitty flag 2 (event types) with a key held down: press, repeats, release, in order; no release for keys Tandem swallowed (Win chords). Test: Task 1 `kitty_reports_press_repeat_release`.
2. A program that sets kitty flags and exits without popping them: the next prompt still gets plain keys after a reset. Test: Task 2 `reset_clears_keyboard_modes`.
3. AltGr on a German layout under every protocol: the character, not Ctrl+Alt. Test: Task 1 `altgr_types_its_character`.
4. Synchronized output that never ends (a crashed app). Expect: the screen updates after the timeout. Test: Task 4 `an_unended_sync_flushes`.
5. Queries from untrusted output (DECRQSS, XTGETTCAP with odd keys, huge parameters). Expect: a well-formed error reply, no panic. Test: Task 3 `bad_queries_reply_errors`.

---

### Task 1: Key encoders

**Files:** Create `src/keyproto.rs`.

**Interfaces:** `KeyEvent { key: Key (the named key or its character), shifted: Option<char>, base: Option<char>, text: Option<String>, mods: Mods (shift, alt, ctrl; win ignored), kind: Press | Repeat | Release, vk: u16, scan: u16 }`; `modify_other_keys(ev, level: u8) -> Option<Vec<u8>>` (None = use the standard bytes); `csi_u(ev) -> Option<Vec<u8>>` (fixterms, iTerm2's termkey mapper); `kitty(ev, flags: u8) -> Option<Vec<u8>>` (flag 1 exemptions, flag 2 events, flag 4 alternates, flag 8 all keys, flag 16 text; iTerm2's F1-F4 as `11~`..`14~`); `raw(ev) -> Vec<u8>` (`OSC 1337 ; d|u ; flags ; hex(text) ; vk ; hex(base) BEL`); `raw_modifiers(flags) -> Vec<u8>`.

- [ ] Failing tests: `mok2_encodes_modified_keys`, `mok1_leaves_plain_keys`, `csi_u_follows_fixterms`, `kitty_disambiguates_escape`, `kitty_reports_press_repeat_release`, `kitty_alternate_keys_and_text`, `kitty_all_keys_as_escapes`, `altgr_types_its_character`, `raw_reports_key_down_and_up`. Commit `Add the keyboard protocol encoders`.

### Task 2: Keyboard state, Terminal State menus, and key routing

**Files:** alacritty patch 9 (`set_modify_other_keys`, `report_modify_other_keys`, a Tandem flag that lets the profile refuse app changes); modify `src/screen.rs`, `src/app/mod.rs`, `src/app/menu.rs`, `src/commands.rs`.

**Interfaces:** `Screen::keyboard() -> Keyboard { mode: Standard | Mok1 | Mok2 | CsiU | Raw, kitty: u8 }` combining the app's requests with the menu's choices; the profile's Allow modifyOtherKeys gates app requests; Use libtickit protocol starts CSI u. Menu: Standard Key Reporting Mode, modifyOtherKeys Mode 1, modifyOtherKeys Mode 2, CSI u Mode, Raw Key Reporting Mode (radio); Disambiguate Escape, Report All Event Types, Report Alternate Keys, Report All Keys as Escape Codes, Report Associated Text (toggles on top of the app's flags); Literal Controls; Emulation Level VT100 to VT500 (radio). The app sends releases (and modifier changes in raw mode) when the mode wants them.

- [ ] Failing tests: `app_requests_follow_the_profile`, `menu_flags_join_the_apps`, `reset_clears_keyboard_modes`, `literal_controls_show_carets`. Commit `Add keyboard modes and the Terminal State menus`.

### Task 3: Reports

**Files:** Modify `src/osc.rs` (catch `CSI ? 996 n`, `CSI ? 2031 h/l`, `CSI 1337 n`, `CSI > q`, `CSI = c`, `CSI > c`, `CSI c` answers by level, `DCS $ q`, `DCS + q`, `DCS = 1 s`/`= 2 s`), `src/screen.rs` (answers), alacritty patch 8 revised (DA1 and DA2 answered by Tandem per level).

**Interfaces:** DA1 per level (100 `1;2`, 200/300 `62;1;2;4;6;22;28`, 400/500 `64;1;2;4;6;17;18;21;22`, plus `;52`); DA2 `> vt;2500;0 c` (vt 0/1/32/41/64); DA3 (level 400+) `DCS ! | 6954726D ST`; XTVERSION `DCS > | Tandem <version> ST`; `CSI 1337 n` answers `CSI ITERM2 <version> n`; DECRQSS for SGR, DECSCL, DECSCUSR, DECSCA, DECSTBM, DECSLRM, DECSLPP (valid `DCS 1 $ r ... ST`, else `DCS 0 $ r ST`); XTGETTCAP for `TN`, `name`, `Co`, `colors`, `RGB` (else `DCS 0 + r ST`).

- [ ] Failing tests: `device_attributes_follow_the_level`, `xtversion_and_1337n`, `decrqss_reports_settings`, `xtgettcap_answers_known_keys`, `bad_queries_reply_errors`. Commit `Answer iTerm2's terminal reports`.

### Task 4: Synchronized output and the dark-mode query

**Files:** Modify `src/screen.rs`, `src/app/mod.rs`.

**Interfaces:** `Screen::sync_deadline() -> Option<Instant>` and `Screen::flush_sync()` (vte buffers between BSU and ESU; the app flushes at the deadline); DCS `=1s`/`=2s` start and end it too. `CSI ? 996 n` answers `CSI ? 997 ; 1 n` (dark background) or `; 2 n` (light), judged from the pane's background color as iTerm2 does; with mode 2031 on, the same report is sent when the background crosses between dark and light.

- [ ] Failing tests: `an_unended_sync_flushes`, `dcs_sync_brackets_work`, `dark_mode_query_follows_the_background`, `mode_2031_reports_changes`. Commit `Add synchronized output and the dark-mode query`.

### Task 5: Settings, parity, acceptance

- [ ] Profile Keys tab: "Apps can change how keys are reported" and "Report keys using CSI u" take effect (labels as iTerm2). Update the rows 5b closes, recount `00-roadmap.md`, write `docs/acceptance/m5b-checklist.md`. Commit.
