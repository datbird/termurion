//! Terminal emulation switches in the window (13o): the Advanced rules handed to every
//! pane's screen, the profile's encoding, and the OSC 7 conflict warning.

use super::*;
use crate::emu;
use crate::warnings::Kind;

use super::scripts_alerts::{self, Then};

impl App {
    /// The Advanced emulation rules as the globals now stand.
    pub(super) fn emu_rules(&self) -> emu::Rules {
        emu::Rules::from_globals(&self.globals)
    }

    /// Hands the rules and the profile's Terminal Type to a pane's own screen and any view. A
    /// tmux pane keeps the type tmux reports for it (`PS:3253-3255`).
    pub(super) fn apply_emu_rules(&mut self, pane: PaneId) {
        let rules = self.emu_rules();
        let (radius, idem, every) = (self.globals.int("TriggerRadius") as i32, self.globals.bool("AllowIdempotentTriggers"), crate::alerts::safe_secs(self.globals.number("IdempotentTriggerModeRateLimit").unwrap_or(0.25), 0.0, 3600.0));
        let Some(p) = self.panes.get(&pane) else { return };
        let term = if p.tmux_io().is_some() {
            None
        } else {
            Some(self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile()).terminal_type())
        };
        if let Some(p) = self.panes.get_mut(&pane) {
            p.set_emu(rules, term.as_deref());
            // The trigger limits are Advanced settings, read here with the other emulation knobs.
            p.own_mut().set_trigger_limits(radius, idem, every);
        }
        // The profile's Answerback String (`PS:6207`); a tmux pane has none of its own.
        let answerback = self.panes.get(&pane).filter(|p| p.tmux_io().is_none()).map(|p| self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile()).string("Answerback String"));
        if let (Some(text), Some(p)) = (answerback, self.panes.get_mut(&pane)) {
            p.set_answerback(&text);
        }
        // The profile's Character Encoding (`PS:18899`); a tmux pane's output and input are
        // always UTF-8 (control mode), so it keeps none.
        let encoding = self.panes.get(&pane).filter(|p| p.tmux_io().is_none() && !p.gateway).map(|p| self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile()).int("Character Encoding"));
        if let (Some(ns), Some(p)) = (encoding, self.panes.get_mut(&pane)) {
            p.set_encoding(if ns == 0 { crate::encoding::UTF8 } else { ns });
        }
    }

    /// CSI 3 J with Prevent CSI 3 J unset (`N:795-815`): the pane's bar asks once. The sequence
    /// that asked was dropped.
    pub(super) fn ask_clear_history(&mut self, pane: PaneId) {
        use crate::emu::{ALWAYS_ALLOW, ALWAYS_DENY, CLEAR_HISTORY_ID, CLEAR_HISTORY_TEXT};
        self.ask(pane, CLEAR_HISTORY_ID, CLEAR_HISTORY_TEXT, &[ALWAYS_ALLOW, ALWAYS_DENY], false);
    }

    /// The answer to that bar: "Always Allow" stores false, "Always Deny" true (iTerm2 stores
    /// the key the same way); every pane takes the new rules. A closed bar stores nothing.
    pub(super) fn clear_history_answered(&mut self, button: &str) {
        self.globals.set("PreventEscapeSequenceFromClearingHistory", toml::Value::Boolean(button.contains("Deny")));
        self.save_globals();
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        for id in ids {
            self.apply_emu_rules(id);
        }
    }

    /// DECRQCRA with Disable DECRQCRA unset (`PS:17182-17212`): the pane's bar asks once. The
    /// sequence that asked was not answered.
    pub(super) fn ask_decrqcra(&mut self, pane: PaneId) {
        use crate::emu::{DECRQCRA_ID, DECRQCRA_TEXT};
        self.ask(pane, DECRQCRA_ID, DECRQCRA_TEXT, &["Yes", "No"], false);
    }

    /// "Yes" stores Disable DECRQCRA false, "No" true (iTerm2's `NoSyncDisableDECRQCRA`); either
    /// is kept in the file (`KEPT_WHEN_DEFAULT`), so the question is asked once. A closed bar
    /// stores nothing.
    pub(super) fn decrqcra_answered(&mut self, button: &str) {
        self.globals.set(crate::emu::DECRQCRA_ID, toml::Value::Boolean(button == "No"));
        self.save_globals();
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        for id in ids {
            self.apply_emu_rules(id);
        }
    }

    /// OSC 7 with a machine id arrived while Accept OSC 7 is off (`PS:15534-15558`): asked once
    /// a run, however many panes send it.
    pub(super) fn osc7_disabled(&mut self) {
        // A warning queued before the key was turned back on does not spend the latch
        // (`MS:4646-4653` re-checks when it is shown).
        if self.emu_rules().accept_osc7 || !self.osc7_latch.first() {
            return;
        }
        let was_silenced = crate::warnings::is_silenced(&self.globals, emu::OSC7_WARNING_ID, super::warnings_app::unix_now());
        let ask = scripts_alerts::ask(emu::OSC7_HEADING, emu::OSC7_TEXT, &[emu::OSC7_TURN_ON, crate::api_auth::CANCEL], Some(emu::OSC7_WARNING_ID), Kind::Permanent, Then::TurnOnOsc7 { was_silenced });
        self.ask_scripts(ask);
    }

    /// "Turn On OSC 7" sets the key only when the question was not silenced: a remembered
    /// answer never flips a setting (`selection == kiTermWarningSelection0 && !wasSilenced`).
    pub(super) fn osc7_answered(&mut self, was_silenced: bool, index: usize) {
        if index == 0 && !was_silenced {
            self.globals.set("AcceptOSC7", toml::Value::Boolean(true));
            self.globals_changed(false);
        }
    }

    /// An escape-sequence print (`CSI 4 i` after `CSI 5 i`, or `CSI i`): the pane's print guard
    /// decides (`PG:20-75`); a second attempt within 30 s asks, and only the person's answer at
    /// the print dialog prints anything.
    pub(super) fn print_request(&mut self, pane: PaneId, job: crate::screen::PrintJob) {
        use crate::emu::Guard;
        // One question at a time per pane: while "Allow Printing?" waits (shown or queued) a
        // further attempt is dropped, and is not counted against the guard (P21).
        if self.alert_waits(|t| matches!(t, Then::PrintGuard { pane: p, .. } if *p == pane)) {
            return;
        }
        let disabled = self.profile_of(pane).is_some_and(|p| p.bool("Disable Printing"));
        let guard = self.print_guards.entry(pane).or_default().check(std::time::Instant::now(), disabled, true);
        match guard {
            Guard::Print => self.print_job(pane, &job),
            Guard::Refuse => {}
            Guard::Ask => {
                let ask = scripts_alerts::ask(emu::PRINT_HEADING, emu::PRINT_TEXT, &emu::PRINT_BUTTONS, Some(emu::PRINT_ID), Kind::Persistent, Then::PrintGuard { pane, job });
                self.ask_scripts(ask);
            }
        }
    }

    /// The answer to "Allow Printing?": Allow prints; Disable Permanently also turns the saved
    /// profile's Disable Printing on (the original of a session copy, `PG:57-62`).
    pub(super) fn print_guard_answered(&mut self, pane: PaneId, job: crate::screen::PrintJob, index: usize) {
        // A pane that closed since has no guard and prints nothing.
        let now = std::time::Instant::now();
        let Some(guard) = self.print_guards.get_mut(&pane) else { return };
        // The guard is checked again now: an Allow that waited behind a Disable prints nothing
        // (iTerm2 asks and answers synchronously, `PG:31-33`).
        let refused = guard.refused(now);
        let permanent = guard.answer(now, index);
        if permanent {
            let guid = self.panes.get(&pane).map(|p| self.store.shared_guid(&p.profile));
            if let Some(profile) = guid.and_then(|g| self.store.get_mut(&g)) {
                profile.set("Disable Printing", toml::Value::Boolean(true));
                if let Err(e) = self.save_store() {
                    self.banners.set("print", format!("Could not save the profile: {e}"));
                }
                self.refresh_panes();
            }
        }
        if index == 0 && !refused {
            self.print_job(pane, &job);
        }
    }

    /// Opens the print dialog for the job (the person cancels there or prints).
    fn print_job(&mut self, pane: PaneId, job: &crate::screen::PrintJob) {
        // The pane's own window parents the dialog, whichever window is current.
        self.enter_pane_window(pane);
        let Some(hwnd) = self.hwnd() else { return };
        let Some(p) = self.panes.get(&pane) else { return };
        let screen = &p.screen;
        let mut rows = match job {
            crate::screen::PrintJob::Screen => crate::print::rows(screen, crate::print::What::Screen),
            crate::screen::PrintJob::Text(t) => crate::print::text_rows(t, screen.palette.fg, screen.palette.bg),
        };
        let cols = { use alacritty_terminal::grid::Dimensions; screen.term().columns() };
        let title = screen.title().unwrap_or("Termurion").to_string();
        let font = self.profile_of(pane).map(|pr| pr.font("Normal Font")).unwrap_or(crate::profile::FontSpec { family: "Cascadia Mono".into(), size_pt: 12.0 });
        if self.globals.bool("Print In Black And White") {
            crate::print::black_and_white(&mut rows);
        }
        if let Err(e) = crate::winprint::print(hwnd as _, &mut self.print_setup, &title, &rows, cols, &font.family, font.size_pt) {
            self.banners.set("print", format!("Printing failed: {e}"));
        }
    }
}

/// OSC 72 drops (13o Task 9): the pane under the pointer whose program registered gets the
/// drag and the drop; every other pane keeps the file-name paste.
impl App {
    /// The pane's program registered for drops and its screen is trusted.
    pub(super) fn dnd_accepts(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).is_some_and(|p| p.own().dnd_accepting())
    }

    /// The pointer in this window's client pixels as the system reports it now
    /// (`GetCursorPos`, less the client origin). winit 0.30 sends no `CursorMoved` during an OLE
    /// drag, so `self.cursor` is where the pointer was before the drag began: never a drop target.
    /// None when the system does not say, and then nothing is routed (fail closed).
    pub(super) fn pointer_now(&self) -> Option<(f32, f32)> {
        let screen = super::tab_drag_ui::cursor_screen()?;
        let origin = self.gpu.as_ref()?.window.inner_position().ok()?;
        Some(client_point(screen, (origin.x, origin.y)))
    }

    /// The registered pane at `point`, with the cell and the pixel (inside its text area) the
    /// protocol names.
    pub(super) fn dnd_place(&self, point: (f32, f32)) -> Option<(PaneId, (i64, i64), (i64, i64))> {
        let (x, y) = point;
        let (id, rect) = self.pane_at(x, y)?;
        if !self.dnd_accepts(id) {
            return None;
        }
        let (col, row, _) = self.cell_at(id, rect, x, y);
        let inner = self.text_rect(id, rect);
        let px = ((x - inner.x).max(0.0) as i64, (y - inner.y).max(0.0) as i64);
        Some((id, (i64::from(col), i64::from(row)), px))
    }

    /// winit's `HoveredFile` (once per file): the first one starts the drag over the pane the
    /// pointer is over now.
    pub(super) fn dnd_hover(&mut self) {
        let Some((id, cell, px)) = self.pointer_now().and_then(|pt| self.dnd_place(pt)) else { return };
        let Some(p) = self.panes.get_mut(&id) else { return };
        let replies = p.own_mut().dnd.drag_entered(cell, px, &[crate::kitty_dnd::MIME_URI_LIST.to_string()]);
        Self::dnd_send(p, replies);
    }

    /// `HoveredFileCancelled`: the drag left every pane that was told about it.
    pub(super) fn dnd_cancel(&mut self) {
        self.dnd_exit_hovering(None);
    }

    /// Tells every hovering pane except `keep` that the drag left it (a drag that entered pane A and
    /// was dropped on pane B must not leave A hovering: 13o parked minor).
    pub(super) fn dnd_exit_hovering(&mut self, keep: Option<PaneId>) {
        for p in self.panes.values_mut().filter(|p| Some(p.id) != keep) {
            if p.own().dnd.hovering() {
                // A screen that lost trust since is not told anything.
                let replies = if p.own().dnd_accepting() { p.own_mut().dnd.drag_exited() } else { Vec::new() };
                p.own_mut().dnd.forget_hover();
                Self::dnd_send(p, replies);
            }
        }
    }

    /// A drop that fell back to the file-name paste ends the drag too: a pane told of it (and
    /// since untrusted) must not stay hovered, or its next drag would report nothing.
    pub(super) fn dnd_forget_hover(&mut self) {
        for p in self.panes.values_mut().filter(|p| p.own().dnd.hovering()) {
            p.own_mut().dnd.forget_hover();
        }
    }

    /// The files of one drop, handed to the program that registered. A folder's path ends
    /// with a separator; a network path or anything that is not a plain file is never read.
    pub(super) fn dnd_flush(&mut self) {
        // A drop that outlived its minute is freed here, on the pass every wake-up makes (no
        // timer of its own, so an idle app is never woken for it).
        let now = std::time::Instant::now();
        for p in self.panes.values_mut().filter(|p| p.own().dnd.has_drop()) {
            p.own_mut().dnd.expire(now);
        }
        if self.dnd_dropped.is_empty() {
            return;
        }
        let dropped = std::mem::take(&mut self.dnd_dropped);
        let (pane, _, point) = dropped[0].clone();
        self.enter_pane_window(pane);
        // The pointer as the drop event saw it (`GetCursorPos`), not a later or an older one.
        let (cell, px) = self.dnd_place(point).filter(|(id, ..)| *id == pane).map_or(((0, 0), (0, 0)), |(_, c, p)| (c, p));
        let files: Vec<std::path::PathBuf> = dropped.into_iter().filter(|(id, ..)| *id == pane).map(|(_, f, _)| if is_plain_dir(&f) { f.join("") } else { f }).collect();
        let Some(p) = self.panes.get_mut(&pane) else { return };
        if !p.own().dnd_accepting() {
            return;
        }
        let replies = p.own_mut().dnd.drop(cell, px, files, crate::kitty_dnd::Backing { size: dropped_size, read: read_dropped });
        Self::dnd_send(p, replies);
    }

    /// The program's replies, written to its input.
    fn dnd_send(p: &mut crate::pane::Pane, replies: Vec<String>) {
        for r in replies {
            p.reply(r.as_bytes());
        }
    }
}

/// A screen point in a window's client pixels (`ScreenToClient`): the screen point less the
/// client area's screen origin.
fn client_point(screen: (i32, i32), origin: (i32, i32)) -> (f32, f32) {
    ((screen.0 - origin.0) as f32, (screen.1 - origin.1) as f32)
}

/// The entry's own metadata (never a link's target) when it is a plain local one: not a network
/// path, not a device name, not a symbolic link, junction or other reparse point. A link may
/// point at `\\host\share` or at a device, and nothing of that is probed (invariant 5).
fn plain_meta(path: &std::path::Path) -> Option<(crate::localfs::LocalPath, crate::localfs::Stat)> {
    // The one judge first (network and device paths, mapped drives, links to shares), then the entry.
    let path = crate::localfs::LocalPath::judge(path)?;
    let m = crate::localfs::lstat(&path).ok()?;
    (!m.is_reparse).then_some((path, m))
}

/// A dropped folder: a plain local directory.
fn is_plain_dir(path: &std::path::Path) -> bool {
    plain_meta(path).is_some_and(|(_, m)| m.is_dir)
}

/// A dropped file's size for the budget; None when it is not a plain local file.
fn dropped_size(path: &std::path::Path) -> Option<u64> {
    plain_meta(path).filter(|(_, m)| m.is_file).map(|(_, m)| m.len)
}

/// A dropped file's bytes for an OSC 72 reader: a plain local file, at most the size the
/// protocol serves plus one byte (so the controller can tell a file that is too big).
fn read_dropped(path: &std::path::Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let (path, m) = plain_meta(path)?;
    if !m.is_file {
        return None;
    }
    let mut bytes = Vec::new();
    crate::localfs::open(&path).ok()?.take(crate::kitty_dnd::MAX_FILE_BYTES as u64 + 1).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod print_tests {
    /// 13o final review (new item 8): the print dialog is parented to the window that holds the
    /// pane, not to whichever window is current.
    #[test]
    fn print_job_enters_the_panes_window() {
        let own = include_str!("emu_ui.rs");
        let f = own.find("fn print_job").unwrap();
        let body = &own[f..];
        assert!(body.find("self.enter_pane_window(pane)").expect("enters the pane's window") < body.find("self.hwnd()").unwrap());
    }
}

#[cfg(test)]
mod dnd_tests {
    use super::*;

    /// 13o Task 9 (Review Focus 8): a drop goes to the registered pane under the pointer first,
    /// and anything else keeps the file-name paste.
    #[test]
    fn dnd_routes_drops_to_registered_panes() {
        let src = include_str!("mod.rs");
        let hover = src.find("WindowEvent::HoveredFile(_) => self.dnd_hover()").expect("hover arm");
        assert!(src.contains("WindowEvent::HoveredFileCancelled => self.dnd_cancel()"));
        let drop = src.find("WindowEvent::DroppedFile(path)").unwrap();
        let routed = src[drop..].find("self.dnd_accepts(*p)").expect("registered pane first") + drop;
        let paste = src[drop..].find("self.dropped.push").unwrap() + drop;
        assert!(hover < drop && routed < paste, "the OSC 72 route comes before the name paste");
        assert!(!src[drop..routed].contains("focused_id"), "the focused pane is not a drop target for OSC 72");
        let flush = include_str!("coprocess_ui.rs");
        let f = flush.find("fn flush_dropped").unwrap();
        assert!(flush[f..].contains("self.dnd_flush()"));
        // The sweep that frees an expired drop runs in that flush, at the top (before the empty return).
        let own = include_str!("emu_ui.rs");
        let d = own.find("pub(super) fn dnd_flush").unwrap();
        assert!(own[d..].find("dnd.expire(now)").unwrap() < own[d..].find("self.dnd_dropped.is_empty()").unwrap());
    }

    /// 13o final review I1: a drop is placed by the pointer as the system reports it, never by
    /// the last pointer move the window saw (stale during an OLE drag).
    #[test]
    fn drop_point_is_the_real_pointer() {
        assert_eq!(client_point((1300, 700), (100, 50)), (1200.0, 650.0));
        assert_eq!(client_point((90, 40), (100, 50)), (-10.0, -10.0), "outside the window is negative, so no pane");
        let src = include_str!("mod.rs");
        let arm = src.find("WindowEvent::DroppedFile(path)").unwrap();
        let end = src[arm..].find("self.dnd_dropped.push").unwrap() + arm;
        let hover = src.find("WindowEvent::HoveredFile(_)").unwrap();
        assert!(!src[arm..end].contains("self.cursor"), "a drop to a registered pane never reads the stale pointer");
        assert!(!src[hover..arm].contains("self.cursor"));
        let own = include_str!("emu_ui.rs");
        let place = own.find("fn dnd_place").unwrap();
        let place_end = own[place..].find("/// winit's `HoveredFile`").unwrap() + place;
        assert!(!own[place..place_end].contains("self.cursor"));
    }

    /// 13o parked minor: a drop on pane B ends the drag over pane A too.
    #[test]
    fn a_drop_on_one_pane_exits_the_hover_of_another() {
        let src = include_str!("mod.rs");
        let arm = src.find("WindowEvent::DroppedFile(path)").unwrap();
        let end = src[arm..].find("self.dnd_dropped.push").unwrap() + arm;
        assert!(src[arm..end].contains("self.dnd_exit_hovering(Some(p));"));
        let own = include_str!("emu_ui.rs");
        let f = &own[own.find("fn dnd_exit_hovering").unwrap()..];
        assert!(f[..f.find("\n    }\n").unwrap()].contains("Some(p.id) != keep") && f.contains("drag_exited()"));
    }

    /// 13o final review I2 and Task 9 minor 5: a dropped link, junction, device or network path is
    /// never statted through, read or listed (invariant 5).
    #[test]
    fn dropped_links_and_devices_are_refused() {
        let dir = std::env::temp_dir().join(format!("tandem-dnd-links-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.txt");
        std::fs::write(&real, b"hello").unwrap();
        assert_eq!(dropped_size(&real), Some(5));
        assert!(!is_plain_dir(&real) && is_plain_dir(&dir));
        // A junction (needs no privilege) to a folder is not a plain folder.
        let junction = dir.join("junction");
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&dir).output().map(|o| o.status.success()).unwrap_or(false);
        assert!(made, "mklink /J failed");
        assert!(!is_plain_dir(&junction), "a junction may point at a share");
        assert_eq!(dropped_size(&junction.join("real.txt")), Some(5), "a file reached through a folder link is the person's own navigation");
        // A file symlink needs a privilege; where it cannot be made the check is skipped.
        let link = dir.join("link.txt");
        if std::os::windows::fs::symlink_file(&real, &link).is_ok() {
            assert_eq!(dropped_size(&link), None);
            assert_eq!(read_dropped(&link), None);
        }
        // Devices and their names, in any case or with an extension, are not files.
        for name in ["NUL", "nul.txt", "C:\\x\\COM1", "LPT3.log", "\\\\.\\NUL", "CON"] {
            assert_eq!(dropped_size(std::path::Path::new(name)), None, "{name}");
            assert_eq!(read_dropped(std::path::Path::new(name)), None, "{name}");
        }
        std::fs::remove_dir(&junction).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dropped_files_are_read_only_when_plain_and_local() {
        assert_eq!(read_dropped(std::path::Path::new("\\\\host\\share\\a.txt")), None);
        assert_eq!(read_dropped(std::path::Path::new("//host/share/a.txt")), None);
        let dir = std::env::temp_dir();
        assert_eq!(read_dropped(&dir), None, "a folder has no bytes");
        let file = dir.join(format!("tandem-dnd-{}.txt", std::process::id()));
        std::fs::write(&file, b"hello").unwrap();
        assert_eq!(read_dropped(&file).as_deref(), Some(&b"hello"[..]));
        std::fs::remove_file(&file).unwrap();
        assert_eq!(read_dropped(&file), None);
    }
}
