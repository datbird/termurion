//! Output parsed on a second thread while the main thread draws a frame (Parse While Drawing).
//! `render_frame` lends the panes to one scoped worker for the length of the draw; the work
//! that follows output (`App::pump_tail`) runs on the main thread after it, as after a slice.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::layout::PaneId;
use crate::pane::{PARSE_SLICE, Pane, Pump};

// The worker borrows the panes across a thread, so a pane must stay `Send`.
const _: fn() = || {
    fn send<T: Send>() {}
    send::<crate::pane::Pane>();
};

/// The settings each pane takes before its output is read.
#[derive(Clone, Copy, Debug)]
pub(super) struct PumpPrep {
    pub auto_composer: bool,
    pub smart_logging: bool,
    pub always_line_marks: bool,
}

/// The prompt state machine follows View > Auto Composer before it reads output (the Task 3
/// contract: every pane, new ones included).
pub(super) fn prep_for_pump(p: &mut Pane, prep: &PumpPrep) {
    p.own_mut().prompt_state.enabled = prep.auto_composer;
    p.log_compact = (prep.smart_logging, prep.auto_composer);
    // New prompt marks are line-style with Auto Composer or AlwaysUseLineStyleMarks
    // (`S:18934`); marks already made keep their style.
    p.screen.line_style_marks = prep.auto_composer || prep.always_line_marks;
}

/// Whether the worker may parse this pane: a plain local or hosted shell. tmux, SSH
/// integration, conductor and coprocess panes need the main thread's pumps.
pub(super) fn eligible(p: &Pane) -> bool {
    p.parses_alone()
}

/// What one turn of parsing did, for `App::pump_tail`.
#[derive(Debug, Default)]
pub(super) struct Parsed {
    /// Each pane parsed, once, in the order it was parsed.
    pub results: Vec<(PaneId, Pump)>,
    /// Output is still unread.
    pub owed: bool,
    /// The first pane in the rotation that ran out of time (counted from `start`).
    pub first_more: Option<usize>,
    /// Where the rotation started (`App::pump_from`).
    pub start: usize,
}

impl Parsed {
    /// Records pane number `k` of the rotation.
    pub fn note(&mut self, k: usize, id: PaneId, r: Pump) {
        self.owed |= r.more;
        if r.more && self.first_more.is_none() {
            self.first_more = Some(k);
        }
        self.results.push((id, r));
    }
}

/// The worker: the eligible panes of `order` (the rotation, `start` left to the caller) in
/// turn, up to `PARSE_SLICE` each, round after round until `stop` is set or nothing is
/// left unread. It reads one chunk a call, so a stop waits for one chunk at most.
pub(super) fn parse_meanwhile(panes: &mut HashMap<PaneId, Pane>, order: &[PaneId], prep: &PumpPrep, stop: &AtomicBool) -> Parsed {
    let stopped = || stop.load(Ordering::Acquire);
    let mut seen: Vec<Option<Pump>> = vec![None; order.len()];
    // A pane the stop came before has not been read this turn: it is owed.
    let mut unread = false;
    'rounds: loop {
        let mut more = false;
        for (k, id) in order.iter().enumerate() {
            let Some(p) = panes.get_mut(id) else { continue };
            if !eligible(p) {
                continue;
            }
            if stopped() {
                unread |= seen[k].is_none_or(|r| r.more);
                continue;
            }
            prep_for_pump(p, prep);
            let mut r = seen[k].unwrap_or_default();
            let end = Instant::now() + PARSE_SLICE;
            loop {
                let one = p.pump_until(Instant::now());
                r.changed |= one.changed;
                r.more = one.more;
                // A read can start tmux or SSH integration: the main thread takes the pane over.
                if !one.more || stopped() || Instant::now() >= end || !eligible(p) {
                    break;
                }
            }
            seen[k] = Some(r);
            more |= r.more && eligible(p);
        }
        if !more || stopped() {
            break 'rounds;
        }
    }
    let mut parsed = Parsed::default();
    for (k, (id, r)) in order.iter().zip(seen).enumerate() {
        if let Some(r) = r {
            parsed.note(k, *id, r);
        }
    }
    parsed.owed |= unread;
    parsed
}

/// Sets `stop` when dropped, so the worker ends however the draw returns.
pub(super) struct StopOnDrop<'a>(pub &'a AtomicBool);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger_eval::TriggerPrefs;
    use crate::triggers::Trigger;
    use std::time::Duration;

    const PREP: PumpPrep = PumpPrep { auto_composer: false, smart_logging: false, always_line_marks: false };

    fn trig(regex: &str, action: &str, parameter: &str) -> Trigger {
        Trigger { regex: regex.into(), action: action.into(), parameter: parameter.into(), instant: false, enabled: true, name: String::new(), job: String::new(), match_type: 0, extra: toml::Table::new() }
    }

    /// 200 chunks of colored text, titles and marks, with a highlight, a mark and an alert trigger.
    fn flood_pane(id: PaneId) -> Pane {
        let chunks = (0..200)
            .map(|i| {
                let mark = if i % 37 == 0 { "\x1b]1337;SetMark\x07" } else { "" };
                format!("\x1b[3{}mline {i} of pane {id} ERROR\x1b[0m done\r\n\x1b]0;pane {id} at {i}\x07{mark}", i % 8).into_bytes()
            })
            .collect();
        let mut p = Pane::chunked_for_test(id, chunks);
        p.screen.set_triggers(vec![trig("ERROR", "HighlightTrigger", "{#ff0000,#000000}"), trig("line 1[0-9]5 ", "MarkTrigger", ""), trig("line 7[0-9] ", "AlertTrigger", "seen")], TriggerPrefs::default());
        // Both copies must check every line, however slow the machine is.
        p.screen.triggers.unbudgeted_for_test();
        p
    }

    fn picture(p: &mut Pane) -> String {
        let requests = p.screen.take_requests();
        let text = format!("{:?}\n{:?}\n{:?}\n{:?}", p.screen.term().grid(), p.screen.marks.records, p.screen.title(), requests);
        // Each row keeps the second it was written: the two copies may straddle one.
        let mut parts = text.split("time: ");
        let mut out = parts.next().unwrap_or_default().to_string();
        for part in parts {
            out.push_str("time: _");
            out.push_str(part.trim_start_matches(|c: char| c.is_ascii_digit()));
        }
        out
    }

    #[test]
    fn the_worker_parses_like_the_slices() {
        let ids = [1, 2];
        let mut worked: HashMap<PaneId, Pane> = ids.iter().map(|id| (*id, flood_pane(*id))).collect();
        let mut sliced: HashMap<PaneId, Pane> = ids.iter().map(|id| (*id, flood_pane(*id))).collect();
        let mut rounds = 0;
        loop {
            rounds += 1;
            assert!(rounds < 10_000, "the worker never caught up");
            let stop = AtomicBool::new(false);
            let parsed = std::thread::scope(|s| {
                let w = s.spawn(|| parse_meanwhile(&mut worked, &ids, &PREP, &stop));
                std::thread::sleep(Duration::from_millis(1));
                stop.store(true, Ordering::Release);
                w.join().unwrap()
            });
            if !parsed.owed {
                break;
            }
        }
        loop {
            let mut more = false;
            for id in ids {
                let p = sliced.get_mut(&id).unwrap();
                prep_for_pump(p, &PREP);
                more |= p.pump_until(Instant::now() + PARSE_SLICE).more;
            }
            if !more {
                break;
            }
        }
        for id in ids {
            let (a, b) = (picture(worked.get_mut(&id).unwrap()), picture(sliced.get_mut(&id).unwrap()));
            assert!(a.contains("Alert"), "the alert trigger fired");
            assert_eq!(a, b, "pane {id}");
        }
    }

    #[test]
    fn stop_ends_the_parse_after_one_chunk() {
        let ids = [1, 2];
        let mut panes: HashMap<PaneId, Pane> = ids.iter().map(|id| (*id, flood_pane(*id))).collect();
        let stop = AtomicBool::new(true);
        let parsed = std::thread::scope(|s| s.spawn(|| parse_meanwhile(&mut panes, &ids, &PREP, &stop)).join().unwrap());
        for id in ids {
            assert!(panes[&id].waiting_chunks_for_test() >= 199, "pane {id} parsed at most one chunk");
        }
        assert!(parsed.owed);
    }

    #[test]
    fn only_plain_panes_are_eligible() {
        let plain = || Pane::chunked_for_test(1, vec![b"x".to_vec()]);
        assert!(eligible(&plain()));
        let mut gateway = plain();
        gateway.gateway = true;
        assert!(!eligible(&gateway), "a tmux gateway");
        let mut tmux = plain();
        tmux.hook_output_for_test(b"\x1bP1000p");
        assert!(!eligible(&tmux), "tmux integration");
        let mut ssh = plain();
        ssh.hook_output_for_test(b"\x1bP2000pnone 1 YQ== - Yg==\n");
        assert!(!eligible(&ssh), "SSH integration");
        let mut conductor = plain();
        conductor.conductor = Some(Box::new(crate::conductor::Conductor::new(crate::conductor::Setup::default(), None, "U")));
        assert!(!eligible(&conductor), "a conductor");
        let dir = crate::test_tmp::TempDir::new("parse-meanwhile");
        let mut co = plain();
        // Only Windows runs coprocesses.
        match co.run_coprocess("echo x", false, dir.path(), std::sync::Arc::new(|| {})) {
            Ok(()) => {
                assert!(!eligible(&co), "a coprocess");
                co.stop_coprocess();
            }
            Err(e) => assert!(cfg!(unix), "{e}"),
        }
        assert!(!eligible(&Pane::browser(1, 80, 24, crate::browser::BrowserIo::default())), "a browser pane");
    }

    #[test]
    fn an_exit_seen_by_the_worker_waits_for_the_tail() {
        let ids = [1];
        let mut panes: HashMap<PaneId, Pane> = HashMap::from([(1, flood_pane(1))]);
        let mut saw_owed = false;
        // Past END_DRAIN_CAP an exit is recorded even with output unread (a writer that
        // outlives the shell), so the check holds only within it.
        let started = Instant::now();
        for _ in 0..100_000 {
            let stop = AtomicBool::new(false);
            let parsed = std::thread::scope(|s| {
                let w = s.spawn(|| parse_meanwhile(&mut panes, &ids, &PREP, &stop));
                std::thread::sleep(Duration::from_micros(200));
                stop.store(true, Ordering::Release);
                w.join().unwrap()
            });
            if parsed.owed {
                saw_owed = true;
                if started.elapsed() < Duration::from_millis(150) {
                    assert_eq!(panes[&1].exited, None, "no exit while output is unread");
                }
                continue;
            }
            break;
        }
        assert!(saw_owed);
        let p = &panes[&1];
        assert_eq!(p.waiting_chunks_for_test(), 0);
        assert_eq!(p.exited, Some(0), "seen once everything was read");
        assert!(!p.ended, "the end action waits for pump_tail's handle_ended");
    }

    fn render_frame_src() -> &'static str {
        let src = include_str!("mod.rs");
        let rest = src.split("fn render_frame(&mut self, timing: bool)").nth(1).expect("render_frame");
        rest.split("\n    }\n").next().unwrap()
    }

    #[test]
    fn the_scope_reads_no_pane() {
        let render = render_frame_src();
        // The draw closure runs inside the scope too.
        let from = render.find("let draw = ").expect("the draw closure");
        let scope = &render[from..];
        let open = scope.find("thread::scope(").expect("the scope");
        let mut depth = 0;
        let mut end = scope.len();
        for (i, c) in scope[open..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let scope = &scope[..end];
        for banned in ["self.panes", "focused_pane(", "pane_cell(", "self.tabs"] {
            let inside = scope.replacen("let panes = &mut self.panes;", "", 1);
            assert!(!inside.contains(banned), "the drawing reads {banned}");
        }
    }

    #[test]
    fn the_switch_restores_the_old_path() {
        let src = include_str!("mod.rs");
        let render = render_frame_src();
        let job = render.find("parse_while_drawing").expect("the switch");
        assert!(job < render.find("parse_meanwhile(").expect("the worker"), "the worker only under the switch");
        let live = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        assert_eq!(live.matches("parse_meanwhile(").count(), 1, "one way in");
        let pump = live.split("fn pump_panes(&mut self, event_loop: &ActiveEventLoop) {").nth(1).unwrap();
        let pump = pump.split("\n    }\n").next().unwrap();
        assert!(pump.trim_end().ends_with("self.pump_tail(parsed, event_loop);"), "pump_panes ends in the tail");
        // The one frame that may parse runs the tail on what it parsed.
        let arm = live.split("let parsed = self.render_parsing();").nth(1).expect("the RedrawRequested arm");
        assert!(arm[..arm.find("\n            }\n").unwrap()].contains("self.pump_tail(parsed, event_loop);"));
        assert_eq!(live.matches("self.render_parsing()").count(), 1);
    }
}
