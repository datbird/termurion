//! The prompt path navigator in the app (13r Task 9, `TV:7897-7934`, `PS:14926-14933`,
//! `PS.swift:1185-1242`, `PCH`): a plain click on a folder in a shell prompt lists its
//! subfolders in the completion popup; a pick changes to it or adds it to the typed command.

use super::*;

/// Which input takes a click that comes up on a pane's text (`TV:7897-7934`): fold marks, block
/// indicators and command marks first, then path marks, then the command selection. First true
/// wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Taker {
    Fold,
    Block,
    CommandMark,
    PathMark,
    CommandSelection,
}

pub(super) fn click_taker(fold: bool, block: bool, command_mark: bool, path_mark: bool) -> Taker {
    if fold {
        Taker::Fold
    } else if block {
        Taker::Block
    } else if command_mark {
        Taker::CommandMark
    } else if path_mark {
        Taker::PathMark
    } else {
        Taker::CommandSelection
    }
}

/// The key that turns the navigator on (`PS:14926-14933`).
const KEY: &str = "Prompt Path Click Opens Navigator";

/// The list's placeholder until the folders are read (`PCH:175`).
const GETTING: &str = "Getting directory listing\u{2026}";

/// What a worker found.
enum Reply {
    /// Folders among a pane's prompt paths.
    Marks(PaneId, Vec<crate::pathnav::PathMark>),
    /// The subfolders of a folder the navigator lists.
    Listing(PaneId, String, Vec<String>),
}

/// The navigator's workers: one sniff at a time (a program printing prompts cannot start more),
/// listings on their own thread. Every disk or drive question is asked here, never where bytes
/// are parsed (B4).
pub(super) struct PathNav {
    tx: std::sync::mpsc::Sender<Reply>,
    rx: std::sync::mpsc::Receiver<Reply>,
    notify: crate::pty::Notify,
    busy: Arc<std::sync::atomic::AtomicBool>,
}

impl PathNav {
    pub(super) fn new(notify: crate::pty::Notify) -> PathNav {
        let (tx, rx) = std::sync::mpsc::channel();
        PathNav { tx, rx, notify, busy: Arc::new(std::sync::atomic::AtomicBool::new(false)) }
    }
}

/// Clears the busy flag when the sniff thread ends, however it ends.
struct Idle(Arc<std::sync::atomic::AtomicBool>);

impl Drop for Idle {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

impl App {
    /// Whether the navigator may work for this pane at all: its profile key is on and the pane is
    /// a live local session (a framed or remote pane's paths name another machine; an ended,
    /// archived or replayed one has no shell to `cd`; B4).
    fn navigator_pane_ok(&self, pane: PaneId) -> bool {
        let Some(p) = self.panes.get(&pane) else { return false };
        let local = !self.remote_text(pane) && !p.own().shell.host.as_deref().is_some_and(|h| !self.host_is_this_machine(h));
        local && !p.archived && !p.ended && p.exited.is_none() && p.replay.is_none() && self.profile_of(pane).is_some_and(|prof| prof.bool(KEY))
    }

    /// The index of the path mark under a cell while the pane is at its prompt.
    pub(super) fn path_mark_index(&self, pane: PaneId, point: Point) -> Option<usize> {
        if !self.navigator_pane_ok(pane) {
            return None;
        }
        let s = self.panes.get(&pane)?.own();
        if s.at_prompt() != Some(true) {
            return None;
        }
        s.path_mark_at(point).map(|(i, _)| i)
    }

    /// The path mark under a cell (`TV:7897-7934`): the lookup 13s's click handler asks before its
    /// command selection.
    #[allow(dead_code)]
    pub(super) fn path_mark_at(&self, pane: PaneId, point: Point) -> Option<crate::pathnav::PathMark> {
        let i = self.path_mark_index(pane, point)?;
        self.panes.get(&pane)?.own().path_marks().get(i).cloned()
    }

    /// A plain click came up on a path mark: the navigator opens after the double-click time
    /// unless a second click comes first (`TV:7924-7933`). 13s's command selection (its
    /// `command_click_plan`) is not asked when a path mark took the click.
    pub(super) fn start_navigator_click(&mut self, pane: PaneId, index: usize) {
        if click_taker(false, false, false, true) != Taker::PathMark {
            return;
        }
        let wait = Duration::from_millis(super::clippings_ui::double_click_ms() as u64);
        self.pathnav_due = Some((Instant::now() + wait, pane, index));
    }

    /// One turn: workers' answers, a sniff for the newest prompt's candidates, the click timer.
    /// When the timer is due.
    pub(super) fn pathnav_tick(&mut self) -> Option<Instant> {
        while let Ok(reply) = self.pathnav.rx.try_recv() {
            match reply {
                Reply::Marks(pane, found) => {
                    if let Some(p) = self.panes.get_mut(&pane) {
                        p.own_mut().add_path_marks(found);
                    }
                }
                Reply::Listing(pane, base, subs) => self.navigator_listed(pane, &base, &subs),
            }
        }
        self.pathnav_sniff();
        let (due, pane, index) = self.pathnav_due?;
        if Instant::now() < due {
            return Some(due);
        }
        self.pathnav_due = None;
        if let Some(mark) = self.panes.get(&pane).and_then(|p| p.own().path_marks().get(index).cloned()) {
            self.open_navigator(pane, mark);
        }
        None
    }

    /// Hands the newest prompt's candidates to a worker (B4): nothing runs when there are none, a
    /// pane the navigator may not work for loses them unread, and a second sniff waits for the
    /// first.
    fn pathnav_sniff(&mut self) {
        let ids: Vec<PaneId> = self.panes.iter().filter(|(_, p)| p.own().has_path_candidates()).map(|(id, _)| *id).collect();
        for id in ids {
            let ok = self.navigator_pane_ok(id);
            let busy = self.pathnav.busy.load(std::sync::atomic::Ordering::SeqCst);
            if ok && busy {
                continue;
            }
            let Some(cands) = self.panes.get_mut(&id).map(|p| p.own_mut().take_path_candidates()) else { continue };
            if !ok {
                continue;
            }
            let (probe, home) = (self.path_probe(), crate::profile::home().to_string_lossy().into_owned());
            let (tx, notify, idle) = (self.pathnav.tx.clone(), self.pathnav.notify.clone(), self.pathnav.busy.clone());
            idle.store(true, std::sync::atomic::Ordering::SeqCst);
            let guard = Idle(idle.clone());
            let started = std::thread::Builder::new().name("pathnav".into()).spawn(move || {
                let _idle = guard;
                let found = crate::pathnav::sniff(&cands, &home, &probe, &|p| std::path::Path::new(p).is_dir());
                if !found.is_empty() && tx.send(Reply::Marks(id, found)).is_ok() {
                    notify();
                }
            });
            if started.is_err() {
                idle.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            break;
        }
    }

    /// The navigator list at the cursor, with its placeholder until the folders are read
    /// (`PCH:94-150`, `PS.swift:1185-1242`: only at the command prompt).
    pub(super) fn open_navigator(&mut self, pane: PaneId, mark: crate::pathnav::PathMark) {
        if !self.navigator_pane_ok(pane) || self.focused_id() != Some(pane) || self.panes.get(&pane).is_none_or(|p| p.own().at_prompt() != Some(true)) {
            return;
        }
        let row = crate::completion::Row { shown: GETTING.to_string(), value: String::new() };
        let kind = crate::completion::Kind::Paths { base: mark.path.clone() };
        self.completion = Some((pane, crate::completion::List::new(kind, crate::autocomplete::Query::default(), vec![row])));
        self.belt_yield_keys();
        let (probe, base, tx, notify) = (self.path_probe(), mark.path, self.pathnav.tx.clone(), self.pathnav.notify.clone());
        let started = std::thread::Builder::new().name("pathnav-list".into()).spawn(move || {
            // The folder was probed when its prompt ended; it is asked again before it is read.
            let subs = if crate::netpath::may_probe(&base, &probe) { subfolders(&base) } else { Vec::new() };
            if tx.send(Reply::Listing(pane, base, subs)).is_ok() {
                notify();
            }
        });
        if started.is_err() {
            self.completion = None;
        }
        self.redraw();
    }

    /// The folders arrived: the list shows them, or closes when there is nothing to show.
    fn navigator_listed(&mut self, pane: PaneId, base: &str, subs: &[String]) {
        let Some((p, list)) = self.completion.as_mut() else { return };
        if *p != pane || !matches!(&list.kind, crate::completion::Kind::Paths { base: b } if b == base) {
            return;
        }
        let rows: Vec<crate::completion::Row> = crate::pathnav::list(base, subs)
            .into_iter()
            .map(|(value, detail)| crate::completion::Row { shown: if detail == "Parent directory" { format!("{detail}: ") } else { String::new() }, value })
            .collect();
        if rows.is_empty() {
            self.completion = None;
        } else {
            list.rows = rows;
            list.selected = 0;
        }
        self.redraw();
    }

    /// A pick (Enter or a click): `cd <path>` and Enter at an empty command line, else the
    /// Composer opens with the typed command and the path (`PS.swift:1233-1242`). The path is
    /// program text and a folder name is disk text: it goes through `pathnav::choose`'s guard and
    /// quoting (B5), an ended session or a password prompt takes nothing.
    pub(super) fn navigator_chosen(&mut self, pane: PaneId, value: &str) {
        let Some(p) = self.panes.get(&pane) else { return };
        if p.exited.is_some() || p.ended {
            return self.banners.set("action", super::cmdinfo_app::ENDED);
        }
        if p.at_password_prompt() {
            return self.banners.set("action", super::cmdinfo_app::PASSWORD_PROMPT);
        }
        // Only at the command prompt.
        if p.own().at_prompt() != Some(true) {
            return;
        }
        let typed = p.own().typed_command().unwrap_or_default();
        let shell = self.composer_shell(pane);
        match crate::pathnav::choose(value, &typed, shell.as_deref()) {
            Err(why) => self.banners.set("action", why),
            Ok(crate::pathnav::Choice::Cd(line)) => {
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.scroll(Scroll::Bottom);
                    p.input(line.as_bytes());
                }
            }
            Ok(crate::pathnav::Choice::Append(text)) => self.open_composer_with(pane, &text, crate::keyaction::Escaping::None),
        }
    }

    /// A left press on the navigator list picks the row under it; a press elsewhere closes the
    /// list and goes on. True when the list took the press.
    pub(super) fn navigator_press(&mut self, x: f32, y: f32) -> bool {
        let Some((pane, list)) = self.completion.as_ref().filter(|(_, l)| matches!(l.kind, crate::completion::Kind::Paths { .. })) else { return false };
        let pane = *pane;
        let values: Vec<String> = list.rows.iter().map(|r| r.value.clone()).collect();
        let Some((id, rect)) = self.pane_at(x, y).filter(|(id, _)| *id == pane) else {
            self.completion = None;
            return false;
        };
        let cell = self.pane_cell(id);
        let Some((rows, selected, footer, at, bounds)) = self.completion_box(pane, rect, cell) else { return false };
        let Some((box_rect, first, shown)) = scene::completion_rect(&rows, selected, footer, at, bounds, cell) else { return false };
        if !box_rect.contains(x, y) {
            self.completion = None;
            return false;
        }
        let i = (first + ((y - box_rect.y - 2.0).max(0.0) / cell.1) as usize).min(first + shown - 1);
        self.completion = None;
        if let Some(v) = values.get(i).filter(|v| !v.is_empty()) {
            self.navigator_chosen(pane, v);
        }
        self.redraw();
        true
    }
}

/// The subfolders of `dir` by name (at most 4096 entries read; names are disk text).
fn subfolders(dir: &str) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten().take(4096).filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.file_name().to_string_lossy().into_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_marks_take_the_click_before_command_selection() {
        assert_eq!(click_taker(false, false, false, true), Taker::PathMark);
        assert_eq!(click_taker(false, false, false, false), Taker::CommandSelection);
        assert_eq!(click_taker(true, false, false, true), Taker::Fold);
        assert_eq!(click_taker(false, true, true, true), Taker::Block);
        assert_eq!(click_taker(false, false, true, true), Taker::CommandMark);
    }

    const CODE: &str = include_str!("pathnav_ui.rs");
    const MOD: &str = include_str!("mod.rs");

    /// B4: the sniff is a worker started from the loop's drain; the parse path holds no disk call.
    #[test]
    fn the_sniff_runs_on_a_worker_for_local_live_panes() {
        let sniff = &CODE[CODE.find("fn pathnav_sniff").unwrap()..CODE.find("fn open_navigator").unwrap()];
        assert!(sniff.contains("thread::Builder") && sniff.contains("crate::pathnav::sniff("));
        assert!(sniff.find("navigator_pane_ok").unwrap() < sniff.find("thread::Builder").unwrap());
        let ok = &CODE[CODE.find("fn navigator_pane_ok").unwrap()..CODE.find("fn path_mark_index").unwrap()];
        for needle in ["remote_text(", "host_is_this_machine(", "p.archived", "p.ended", "p.exited"] {
            assert!(ok.contains(needle), "{needle}");
        }
        let marks = include_str!("../marks.rs");
        let record = &marks[marks.find("fn record_path_candidates").unwrap()..marks.find("pub fn add_path_marks").unwrap()];
        assert!(!record.contains("is_dir") && !record.contains("may_probe") && !record.contains("exists("));
    }

    /// B5: a pick is written through the guard, and not into an ended pane or a password read.
    #[test]
    fn a_pick_goes_through_the_guard() {
        let pick = &CODE[CODE.find("fn navigator_chosen").unwrap()..CODE.find("fn navigator_press").unwrap()];
        let (ended, password, prompt, choose, input, composer) = (
            pick.find("ENDED").unwrap(),
            pick.find("at_password_prompt").unwrap(),
            pick.find("at_prompt() != Some(true)").unwrap(),
            pick.find("pathnav::choose(").unwrap(),
            pick.find("p.input(").unwrap(),
            pick.find("open_composer_with(").unwrap(),
        );
        assert!(ended < password && password < prompt && prompt < choose && choose < input && choose < composer);
    }

    /// The click: a press arms it, any later press cancels it, the timer is folded into the wake.
    #[test]
    fn the_click_waits_out_the_double_click_time() {
        let press = &MOD[MOD.find("fn mouse_press(").unwrap()..];
        assert!(press[..1500].contains("self.pathnav_due = None"));
        assert!(MOD.contains("pathnav_due, ") || MOD.contains("cmdinfo_due, pathnav_due"));
        assert!(CODE.contains("double_click_ms()"));
        let close = &MOD[MOD.find("fn close_pane").unwrap()..];
        assert!(close[..4000].contains("pathnav_due"));
    }
}
