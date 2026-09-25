# Tandem Milestone 6b: Bell, Notifications, Activity, Event Triggers, and Badges

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's bell (Silence bell, Show bell icon in tabs, Flash visual bell, the 0.1 s bell limit, the "bell is ringing a lot" offer), notifications (Notification Center alerts and the Filter Alerts switches, the Bell, Idle, New Output, Session Ended, and escape-sequence alerts with iTerm2's texts, OSC 9 and OSC 1337 Notification, click to reveal the session, Window > Notify on Status Change), idle and new-output tracking with the tab indicators, "Periodically send an ASCII code when idle", event triggers (Match Against: the 16 events), and badges (the profile's interpolated badge text, font, color, margins, maximum size, OSC 1337 SetBadgeFormat).

**Architecture:** Pure models decide everything that has a rule: `bell.rs` (the bell's effects and the annoying-bell detector), `activity.rs` (output, idle, and new-output bursts per session, with iTerm2's timings), `alerts.rs` (whether an alert posts, and its title and body), `trigger_events.rs` (event triggers: which fire for an event, their filters and captured strings), `badge.rs` (the badge's font size by binary search against a measure function, and its place). The screen reports bells, notifications, and custom sequences as requests; the window applies the models, draws the tab indicators and the badge, and posts notifications through 4b's tray notification (Windows shows it as a toast), whose click reveals the session.

**Tech Stack:** as milestone 6a.

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. Inventory rows: `01-profile-settings.md` Terminal (bell, notifications, Filter Alerts), Session (Send Code When Idle), General (Badge), Colors (Badge Color); `02-global-settings.md` the bell, idle, notification, and badge advanced rows and ShowNewOutputIndicator; `03-features-and-menus.md` OSC 9, OSC 1337 Notification, SetBadgeFormat, Custom, Window > Notify on Status Change, the tab indicators. iTerm2 facts: `.superpowers/research/m6-iterm2-facts.md` sections 1.7, 4, and 5.

## Global Constraints

- Everything from milestones 0 to 6a still holds.
- Notifications use the tray notification from milestone 4 (a balloon Windows shows as a toast; no app ID or installer needed); `silentUserNotifications` maps to a silent balloon. Clicking one brings Tandem forward and selects the session it names.
- iTerm2's texts, verbatim: "Bell" / "Session %1 #%2 just rang a bell!", "Idle" / "Session %1 in tab #%2 became idle.", "New Output" / "New output was received in %1, tab #%2.", "Session Ended" (or `sessionEndMessageText`) / `Session "%1" in tab #%2 just terminated.`, "Alert" / "Session %1 #%2: %3" (or only the message with `simpleNotifications`), a trigger's "A trigger fired in session “%1” in tab #%2.".
- The bell beeps with the Windows "Default Beep" sound (MessageBeep MB_OK). The Bell notification posts only when "Show bell icon in tabs" is on (iTerm2 posts it inside its bell-icon code).
- Idle = no output for more than 3 s (idleTimeSeconds 2 + 1); output within 0.25 s of a resize does not count; idle and new-output notifications need the last resize to be more than 5 s ago and post once per burst.
- A notification posts only when the user is not looking at that session (Tandem not in front, or another tab), unless it is escape-sequence generated; "Suppress alerts from active session" also blocks the active session.
- Badge: red at 50% by default; right-aligned at the top right with 10 px margins, within 50% of the pane's width and 20% of its height; the largest font size from 4 to 100 that fits; "Helvetica" and missing fonts become Arial; drawn behind text, pinned to the view; filled with the badge color and outlined in the background color.
- SetBadgeFormat is refused when the text calls a function, with iTerm2's message.
- Event triggers use iTerm2's match types 100 to 115 and `eventParams` keys; Job Started and Job Ended wait for 6c (listed, not fired).

## Review Focus

1. A program that rings the bell 1000 times a second. Expect: at most 10 beeps a second (the 0.1 s limit), the offer to silence it once, no notification flood. Test: Task 1 `bell_limit_and_the_offer`.
2. Output in bursts around a window resize. Expect: no idle or new-output notification for output that follows a resize by 0.25 s, and none within 5 s of it. Test: Task 3 `resize_output_is_not_activity`.
3. OSC 9 and OSC 1337 Notification from an untrusted program, and a flood of them. Expect: posted only with "Send escape sequence-generated alerts" on and notifications allowed; at most one a second per session. Test: Task 4 `terminal_alerts_follow_the_filters`.
4. A badge text that is empty, very long, or one character in a tiny pane. Expect: no badge for empty, a smaller font for long text down to 4, never outside the pane. Test: Task 6 `badge_fits_the_box`.
5. An Idle event trigger whose action is Send Text. Expect: it fires once per idle period, not every tick. Test: Task 5 `idle_event_fires_once_per_quiet_spell`.

---

### Task 1: The bell model

**Files:** Create `src/bell.rs`; modify `src/screen.rs` (`Request::Bell`, `bell_count`).

**Interfaces:** `BellPrefs { silence, icon, flash }` from the profile (`Silence Bell`, `Visual Bell`, `Flashing Bell`); `Bell::ring(now, prefs) -> Option<Effects { beep, icon, flash }>` (None within 0.1 s of the last one, `bellRateLimit`); the annoying-bell detector (a moving average of the gaps, weight 0.95, threshold 0.02 s) returns an offer once, not again for 10 s after a decline, never after "Don't Offer Again"; the screen turns BEL (and the Ring Bell trigger) into `Request::Bell` and counts `bellCount`.

- [ ] Failing tests: `bell_follows_the_profile`, `bell_limit_and_the_offer`, `bel_becomes_a_request`. Commit `Add the bell model`.

### Task 2: The bell in the window

**Files:** Create `src/app/bell_ui.rs`; modify `src/app/triggers_ui.rs` (Ring Bell), `src/scene.rs` (the tab bell icon), `src/app/requests_ui.rs`.

**Interfaces:** a beep (MessageBeep MB_OK) when audible; the tab shows a bell icon until the session is selected; Flash visual bell flashes the pane (5c's flash at alpha 0.5); the offer bar "The bell is ringing a lot. Silence it?" with "Silence Bell Temporarily" (60 s), "Suppress All Output" (the pane stops drawing new output until a key is pressed), "Don't Offer Again", and "Silence Automatically" (a session value); `session.bellCount` for interpolated strings.

- [ ] Pure test `tab_icons_in_iterm2_order` (status dot, dead, bell, new output, idle). Commit `Ring the bell in the window`.

### Task 3: Activity

**Files:** Create `src/activity.rs`; modify `src/app/*` (tab indicators), `schema/global.toml` if ShowNewOutputIndicator is missing.

**Interfaces:** `Activity::output(now)`, `::resized(now)`, `::tick(now) -> Vec<ActivityEvent { Idle, NewOutput, ActivityAfterIdle }>`; `is_processing` (output within 2 s), `is_idle` (none for more than 3 s); new output only when the last resize was more than 0.25 s before; idle and new-output events once per burst, not within 5 s of a resize; the tab shows the new-output indicator (when ShowNewOutputIndicator is on) and the idle one.

- [ ] Failing tests: `idle_after_three_quiet_seconds`, `new_output_once_per_burst`, `resize_output_is_not_activity`. Commit `Track output and idle`.

### Task 4: Notifications

**Files:** Create `src/alerts.rs`; modify `src/notify.rs` (click to reveal, silent), `src/marks.rs` or `src/osc1337.rs` (OSC 9, `Notification=`, `Custom=`), `src/app/*`, `schema/menus.toml` and `src/commands.rs` (Window > Notify on Status Change).

**Interfaces:** `AlertPrefs` from the profile (`BM Growl`, the `Send ... Alert` filters, `Suppress Alerts in Active Session`); `should_post(kind, prefs, looking, active) -> bool`; `text(kind, session, tab, message, simple) -> (title, body)`; OSC 9 `;text` (not `9;4;` progress, which becomes a Progress Bar Changed event) and OSC 1337 `Notification=` (`title`, `message`, `subtitle` in base64; the image ignored) become terminal alerts, at most one a second per session; Session Ended when the process exits; the click reveals the session named by the notification; Notify on Status Change posts on the next status change, once; Send Code When Idle sends `Idle Code` every `Idle Period` seconds without output.

- [ ] Failing tests: `alert_texts_follow_iterm2`, `terminal_alerts_follow_the_filters`, `looking_blocks_posting`, `osc9_and_notification_parse`, `idle_code_every_period`. Commit `Post notifications`.

### Task 5: Event triggers

**Files:** Create `src/trigger_events.rs`; modify `src/trigger_eval.rs`, `src/settings_triggers.rs`, `src/app/*`.

**Interfaces:** `EventKind` for match types 100 to 115 with iTerm2's names; `Screen::fire_event(kind, captures, facts)` runs the enabled triggers of that type whose filters pass (`exitCodeFilter` with `*`, numbers, `!n`; `directoryRegex`, `hostRegex`, `userRegex`, `commandRegex`, `messageRegex` as regexes that fall back to substring; `sequenceId`; `progressBarFilter`; `threshold` 60 s; `timeout` 30 s; `variableName` and `variableValueRegex`); sources: prompts, command end (exit code), directory, host, user, idle and activity after idle (6b's activity), session end, bell, long-running command, OSC 1337 `Custom=id=...:payload` (trusted), notifications posted, progress bar, variable changes; Job Started and Job Ended listed until 6c. The editor's "Match Against:" popup ("Regular Expression", a separator, then the events by name) and each event's filter fields.

- [ ] Failing tests: `events_match_their_triggers`, `exit_code_filters`, `regex_filters_fall_back_to_substring`, `idle_event_fires_once_per_quiet_spell`, `custom_sequence_needs_trust`. Commit `Add event triggers`.

### Task 6: Badges

**Files:** Create `src/badge.rs`; modify `src/scene.rs`, `src/render.rs` (a badge font span), `src/app/*`, `src/marks.rs` (SetBadgeFormat).

**Interfaces:** `layout(text, pane: (w, h), prefs: BadgePrefs { max_w, max_h, top, right }, measure: &dyn Fn(&str, f32) -> (f32, f32)) -> Option<BadgeLayout { size, lines, rect }>` (words wrap; the largest size 4 to 100 that fits; None for empty text); the text is the profile's `Badge Text` as an interpolated string over the session's variables (errors shown in the badge as iTerm2 does); SetBadgeFormat stores a session value, refused with iTerm2's message when the text calls a function; the scene draws the badge behind text, pinned to the view, in the badge color at its alpha with a background-colored outline.

- [ ] Failing tests: `badge_fits_the_box`, `badge_text_interpolates`, `set_badge_format_refuses_calls`, scene `badge_behind_text`. Commit `Draw badges`.

### Task 7: Parity and acceptance

- [ ] Update the rows 6b closes, recount, write `docs/acceptance/m6b-checklist.md`. Commit.
