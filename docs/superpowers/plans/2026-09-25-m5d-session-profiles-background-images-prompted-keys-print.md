# Tandem Milestone 5d: Session Profiles, Background Images, the Prompted OSC 1337 Keys, and Print

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iTerm2's session-local ("divorced") profiles; profile background images (single image and folder rotation, the four scaling modes, Blend, per-pane or shared); the OSC 1337 keys that ask first (SetProfile, SetProfileProperty, SetBackgroundImageFile, OpenURL, ReportVariable, RequestUpload) with iTerm2's bars, texts, and stored answers; and Shell > Print (Screen, Selection, Buffer) and Page Setup.

**Architecture:** `session_profile.rs` holds the divorce model (a private copy with a fresh GUID and its original GUID, the overridden keys, the merge when the shared profile changes); `Store` keeps session profiles in memory beside the saved ones, and `Store::get` finds both. `bgimage.rs` decides geometry (the four modes, per-pane or shared container) and rotation order; the renderer draws background image quads before cell backgrounds, with the default background over them at `1 - blend`. `prompted.rs` holds the pure decisions for the six keys (permission lookups, bar texts, reply formats, the upload archive). `print.rs` lays out styled rows into pages; `winprint.rs` runs the Windows print and page setup dialogs and draws with GDI.

**Tech Stack:** as milestone 5c, plus the `tar` crate (RequestUpload archives).

**Spec:** `docs/superpowers/specs/2026-09-23-tandem-design.md`. Inventory rows: `03-features-and-menus.md` Shell > Print (Screen, Selection, Buffer), Shell > Page Setup..., OSC 1337 SetProfile, SetProfileProperty, SetBackgroundImageFile, OpenURL, ReportVariable, RequestUpload; `01-profile-settings.md` the Background Image rows and Blend; `02-global-settings.md` PerPaneBackgroundImage. iTerm2 facts: the ledger workspace's `m5c-iterm2-facts.md` and `m5d-iterm2-facts.md`.

## Global Constraints

- Everything from milestones 0 to 5c still holds.
- The six keys need trust (5c's gate) and then iTerm2's own permission: bars in the pane with iTerm2's texts and buttons, answers stored under iTerm2's keys (`PreventEscapeSequenceFromChangingProfile`, `NoSyncSetProfileProperty_<key>` with 0 always allow and 1 always deny, `AlwaysAllowBackgroundImage` / `AlwaysDenyBackgroundImage` path lists, `NoSyncDisableOpenURL` and per-host allows, `NoSyncVariablesToReport`).
- A bar with the same identifier replaces the older one (its request is dropped); a bar that is not important says "Press any key to dismiss this message." and a key dismisses it; `_X` in a button title means Alt+X.
- Background image drawing follows iTerm2 with an opaque window: the image, then the default background at alpha `1 - blend`, then cells whose background is not the default drawn opaque, then text.
- Moved to milestone 5e: Make Screenshot, and Block, UpdateBlock, and Button (with `render=1`).

## Review Focus

1. A shared profile edited while a pane is divorced. Expect: keys the pane overrode stay; the rest follow the edit; an override equal to the new shared value stops being an override. Test: Task 1 `overrides_survive_shared_edits`.
2. SetProfileProperty with one illegal key among legal ones. Expect: nothing applies. Test: Task 3 `one_illegal_key_rejects_all`.
3. A background image that is missing, corrupt, or 20000 px wide. Expect: plain background color, no panic, image downscaled to 5120 px. Test: Task 2 `bad_images_fall_back`.
4. ReportVariable for an unknown name, `session.name`, and a denied name. Expect: `ESC ] 1337 ; ReportVariable= BEL` (empty) for each. Test: Task 4 `report_variable_replies`.
5. RequestUpload cancelled in the file picker, and one with an unsupported format. Expect: `0x03` written for cancel; nothing written and a warning bar for the format. Test: Task 4 `upload_answers`.

---

### Task 1: Session profiles

**Files:** Create `src/session_profile.rs`; modify `src/store.rs`, `src/app/mod.rs`.

**Interfaces:** `SessionProfile { profile: Profile, overridden: BTreeSet<String> }`; `divorce(shared: &Profile, new_guid: String) -> SessionProfile` (a copy; `Guid` = new_guid, `Original Guid` = the shared GUID); `SessionProfile::set(&mut self, values: &[(String, Option<Value>)], shared: &Profile)` (None removes the key; then recompute overrides: a key is overridden when it differs from the shared profile); `SessionProfile::merge(&mut self, shared: &Profile)` (keys not overridden take the shared value or go when the shared profile lacks them; an override equal to the shared value stops being one). `Store.sessions: Vec<SessionProfile>` (never saved); `Store::get` looks in both; `Store::merge_sessions()` runs after every profile edit. `App::divorce(pane) -> guid` and `App::remarry(pane, shared_guid)`.

- [ ] Failing tests: `divorce_copies_with_fresh_guid`, `overrides_survive_shared_edits`, `none_removes_a_key`, `sessions_are_not_saved`. Commit `Add session profiles`.

### Task 2: Background images

**Files:** Create `src/bgimage.rs`; modify `src/scene.rs`, `src/render.rs`, `src/app/*`, `schema/global.toml` (`PerPaneBackgroundImage` bool true, Appearance > Panes).

**Interfaces:** `Mode { Stretch, Tile, Fill, Fit }` (profile `Background Image Mode` 0-3); `quads(mode, image: (u32, u32), container: Rect, pane: Rect, scale: f32) -> Vec<(Rect, [f32; 4] /*uv*/)>` (clipped to the pane; Tile at 1 image pixel per device pixel anchored at the container; Fit's bars are left for the background color); `load(path) -> Option<Arc<Image>>` (the `image` crate, downscaled to 5120 px on the long side, cached by path and modified time, None on any failure); `Rotator` (folder, interval at least 5 s, shuffled order with no repeat across a cycle, rescans each tick). The frame gets `backgrounds: Vec<ImageQuad>` drawn first; the pane's default background is drawn over them at `1 - blend`; cells whose background is the default draw nothing. `PerPaneBackgroundImage` off: the active pane's image, mode, and blend fill the whole tab content area and each pane shows its slice.

- [ ] Failing tests: `stretch_fill_fit_tile_geometry`, `shared_mode_slices_the_container`, `rotation_shows_each_once_per_cycle`, `bad_images_fall_back`, scene `background_image_under_cells`. Commit `Draw profile background images`.

### Task 3: Bar upgrades, SetProfile, SetProfileProperty, SetBackgroundImageFile

**Files:** Create `src/prompted.rs`; modify `src/infobar.rs`, `src/app/infobar_ui.rs`, `src/marks.rs`, `src/screen.rs` (`Request` variants), `src/app/requests_ui.rs`.

**Interfaces:** `Bar { important: bool }`; `Bars::push` replaces a waiting bar with the same id (returns the replaced one); a key press dismisses the front bar when it is not important (that key goes no further); Alt+letter presses the button whose title has `_letter`. `Request::SetProfile(String)`, `Request::SetProfileProperty(Vec<(String, String /*base64 JSON*/)>)`, `Request::SetBackgroundImageFile(String)`. `prompted::valid(key, &serde_json::Value) -> Option<toml::Value>` (the schema's type for the key: bool, int, float, string, enum values, color dictionaries; unknown keys are illegal); `prompted::property_decision(keys, stored: impl Fn(&str) -> Option<i64>) -> Decision { Apply, Deny, Ask }`. SetProfile: `PreventEscapeSequenceFromChangingProfile` unset asks "A control sequence attempted to change the current profile. Allow this in the future?" (`Always _Allow` stores false, `Always _Deny` stores true) and drops this request; allowed: the named profile (exact, case-sensitive; empty is the default profile) replaces the pane's, keeping its session name, and any divorce ends. SetProfileProperty: iTerm2's texts and buttons (`_Allow Once`, `Allow Always`, `Deny Always`), the listed keys on one line for several. SetBackgroundImageFile: "Set background image to “<path>”?" or "Remove background image?" (`Yes`, `Always`, `Never`), then a session override of `Background Image Location` (ignored in folder rotation).

- [ ] Failing tests: `same_id_replaces_the_bar`, `alt_letter_presses_a_button`, `one_illegal_key_rejects_all`, `values_follow_schema_types`, `property_permissions`, `set_profile_needs_permission`, `background_image_file_lists`. Commit `Add SetProfile, SetProfileProperty, and SetBackgroundImageFile`.

### Task 4: OpenURL, ReportVariable, RequestUpload

**Files:** Modify `src/prompted.rs`, `src/marks.rs`, `src/screen.rs`, `src/app/requests_ui.rs`, `Cargo.toml` (`tar`).

**Interfaces:** OpenURL: `<ignored>:<base64 url>`; `NoSyncDisableOpenURL` off; a host allowed (`OpenURL` + lowercase hex of the host) opens at once with `ShellExecuteW`; otherwise the bar "Open this URL? <url>" (`Allow`, `Always allow for this host`, `Never allow`; Never sets `NoSyncDisableOpenURL`). ReportVariable: base64 name; `NoSyncVariablesToReport` (`allow:`/`deny:` list, deny wins); unlisted: empty reply now and the bar "A request to report variable “<name>” was denied. Allow it in the future?" (`Always Allow` / `Always Deny`, appended to the list); reply `ESC ] 1337 ; ReportVariable=<base64 value> BEL`; names: bare session names (`name`, `id`, `termid`, `columns`, `rows`, `path`, `hostname`, `username`, `pid`, `profileName`, `presentationName`, `showingAlternateScreen`, `bellCount`, `selection`, `lastCommand`, `jobName`), `user.*`, `tab.title`, `tab.id`, `tab.window.id`, `tab.window.title`, `iterm2.pid`, `iterm2.effectiveTheme`; anything else empty. RequestUpload: `format=tgz` (other formats: the warning bar "An upload with an unsupported archive format was requested. You may need a newer version of iTerm2." and nothing written); a file picker (several files); cancel writes `0x03`; OK writes `ok\n`, builds a tgz with `./` names relative to the lowest common folder, sends it as base64 in 76-column lines each ending CR, then CR CR, unbracketed, 768 bytes every 10 ms; Ctrl+C during the upload asks "A file is being uploaded. Abort the upload?" (`OK` / `Cancel`).

- [ ] Failing tests: `open_url_parse_and_hosts`, `report_variable_replies`, `variable_permissions`, `upload_archive_and_lines`, `upload_answers`. Commit `Add OpenURL, ReportVariable, and RequestUpload`.

### Task 5: Print and Page Setup

**Files:** Create `src/print.rs`, `src/winprint.rs`; modify `src/app/menu.rs`, `src/commands.rs`, `schema/global.toml` (`Print In Black And White` bool true).

**Interfaces:** `print::rows(screen, what: Screen | Selection | Buffer) -> Vec<styled_copy::Row>` (Screen: the rows in view, full width, a final newline; Selection: as Copy with Styles; Buffer: scrollback and screen); `print::layout(rows, cols, cell: (f32, f32), page: (f32, f32)) -> Layout { scale, pages: Vec<Range<usize>> }` (rows laid out at the terminal's width, long rows wrapped by character, scaled down to fit the page width, never up, pages break between lines); `print::black_and_white(rows)` removes colors. `winprint::page_setup()` (PageSetupDlg; paper, orientation, and margins kept for the session); `winprint::print(rows, font, layout options)` (a task dialog first with the "Black and white" check box, saved as `Print In Black And White`, then PrintDlgEx, then GDI pages: white page, background only behind text runs, no header or footer). Print > Screen, Selection (needs a selection), Buffer; Page Setup.

- [ ] Failing tests: `print_rows_follow_iterm2`, `pages_break_between_lines`, `scale_fits_never_grows`, `black_and_white_drops_colors`. Commit `Add Print and Page Setup`.

### Task 6: Parity and acceptance

- [ ] Update the rows 5d closes, move Make Screenshot and Block/UpdateBlock/Button to milestone 5e in the rows and the roadmap, recount, write `docs/acceptance/m5d-checklist.md`. Commit.
