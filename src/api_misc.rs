//! The Python API's Selection (`H:3952-4099`), SetBroadcastDomains (`H:4141-4187`), Tmux
//! (`H:3610-3729`) and ColorPreset (`H:3883-3950`) requests: their validation and answers,
//! made from the request and a snapshot of the app, so the app only carries them out.

use std::collections::{BTreeMap, BTreeSet};

use alacritty_terminal::selection::SelectionType;

use crate::api_proto::pb;
use crate::api_session::Snapshot;

/// One sub-selection in buffer terms: start and end as (buffer index counted from the oldest
/// line still kept, column), the end exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sel {
    pub ty: SelectionType,
    pub mode: crate::seltext::Mode,
    pub start: (i64, i32),
    pub end: (i64, i32),
    /// The column window `(first, last)` inclusive, when the request gave one.
    pub window: Option<(usize, usize)>,
    pub connected: bool,
}

/// A sub-selection of a SetSelection request (`H:4050-4091`) on a screen `width` columns
/// wide that lost `lost` lines off the top of its history. None for what iTerm2 refuses, in
/// its order: an end past the width, a negative column window length, a column window
/// beyond the width, a negative start column, window location, start line or end line. A
/// line above the history is the first line kept (`MAX(0, ..)`). iTerm2 keeps the range as
/// sent for every mode (`iTermSelection.m:1082-1093`), so only BOX is a block and the word
/// and line modes are the exact range (alacritty's would widen it); an unknown mode reads as
/// the default CHARACTER.
pub fn to_sel(s: &pb::SubSelection, width: i32, lost: i64) -> Option<Sel> {
    use pb::SelectionMode as M;
    let w = s.windowed_coord_range.unwrap_or_default();
    let cr = w.coord_range.unwrap_or_default();
    let (start, end) = (cr.start.unwrap_or_default(), cr.end.unwrap_or_default());
    let cols = w.columns.unwrap_or_default();
    let width = width as i64;
    if end.x() as i64 > width || cols.length() < 0 || cols.location() + cols.length() > width || start.x() < 0 || cols.location() < 0 || start.y() < 0 || end.y() < 0 {
        return None;
    }
    let api_mode = s.selection_mode.and_then(|m| M::try_from(m).ok()).unwrap_or(M::Character);
    let ty = match api_mode {
        M::Box => SelectionType::Block,
        M::Character | M::Word | M::Smart | M::Line | M::WholeLine => SelectionType::Simple,
    };
    use crate::seltext::Mode;
    let mode = match api_mode {
        M::Character => Mode::Character,
        M::Word => Mode::Word,
        M::Line => Mode::Line,
        M::Smart => Mode::Smart,
        M::Box => Mode::Box,
        M::WholeLine => Mode::WholeLine,
    };
    // A window of no columns is none.
    let window = (cols.length() > 0).then(|| (cols.location() as usize, (cols.location() + cols.length() - 1) as usize));
    Some(Sel { ty, mode, start: ((start.y() - lost).max(0), start.x()), end: ((end.y() - lost).max(0), end.x()), window, connected: s.connected() })
}

/// A GetSelection sub-selection (`H:3970-4015`) from a selection of type `ty` whose start
/// and inclusive end are (buffer index, column): lines numbered from the first line ever
/// (`lost` added), the end one past its last column (at most the width), not connected, and
/// no column window (Termurion keeps none).
pub fn from_sel(ty: SelectionType, start: (i64, i32), end_inclusive: (i64, i32), lost: i64, width: i32) -> pb::SubSelection {
    use pb::SelectionMode as M;
    let coord = |x: i32, y: i64| Some(pb::Coord { x: Some(x), y: Some(lost + y) });
    let mode = match ty {
        SelectionType::Simple => M::Character,
        SelectionType::Block => M::Box,
        SelectionType::Semantic => M::Word,
        SelectionType::Lines => M::Line,
    };
    let range = pb::CoordRange { start: coord(start.1, start.0), end: coord((end_inclusive.1 + 1).min(width), end_inclusive.0) };
    pb::SubSelection { windowed_coord_range: Some(pb::WindowedCoordRange { coord_range: Some(range), columns: None }), selection_mode: Some(mode as i32), connected: Some(false) }
}

/// A GetSelection sub-selection from `sub` (buffer rows, the end inclusive): its mode, its
/// column window and whether it is connected to the next.
pub fn from_sub(sub: &crate::seltext::Sub, lost: i64, width: i32) -> pb::SubSelection {
    use crate::seltext::Mode;
    let ty = if sub.mode == Mode::Box { SelectionType::Block } else { SelectionType::Simple };
    let mut out = from_sel(ty, (sub.start.row as i64, sub.start.col as i32), (sub.end.row as i64, sub.end.col as i32), lost, width);
    let mode = match sub.mode {
        Mode::Character => pb::SelectionMode::Character,
        Mode::Word => pb::SelectionMode::Word,
        Mode::Line => pb::SelectionMode::Line,
        Mode::Smart => pb::SelectionMode::Smart,
        Mode::Box => pb::SelectionMode::Box,
        Mode::WholeLine => pb::SelectionMode::WholeLine,
    };
    out.selection_mode = Some(mode as i32);
    out.connected = Some(sub.connected);
    if let (Some((lo, hi)), Some(w)) = (sub.window, out.windowed_coord_range.as_mut()) {
        w.columns = Some(pb::Range { location: Some(lo as i64), length: Some((hi - lo + 1) as i64) });
    }
    out
}

impl Sel {
    /// This sub-selection as a `Sub` on a `width`-column buffer (the API's end is exclusive);
    /// None when it covers nothing.
    pub fn to_sub(&self, width: i32) -> Option<crate::seltext::Sub> {
        use crate::find::Pos;
        let (row, x) = self.end;
        let end = if x > 0 {
            Pos { row: row as usize, col: (x.min(width) - 1) as usize }
        } else if self.mode != crate::seltext::Mode::Box && row > self.start.0 {
            Pos { row: (row - 1) as usize, col: (width - 1).max(0) as usize }
        } else {
            return None;
        };
        let start = Pos { row: self.start.0 as usize, col: self.start.1.max(0) as usize };
        (end.row > start.row || (end.row == start.row && end.col >= start.col))
            .then_some(crate::seltext::Sub { start, end, mode: self.mode, window: self.window, connected: self.connected })
    }
}

/// GetSelection sub-selections for a visual (bidi) drag: one Character sub-selection per
/// logical run, as iTerm2's `logicalSubSelections` answers (`iTermSelection.m`
/// `decomposedLogicalSubSelectionsForVisualLiveRange`). Each run is `(first, last)` inclusive
/// (buffer index, column). A run that starts at column 0 continues the previous one when that
/// ended at the last column of the line above, and merges into it; a run followed by another on
/// its own line is `connected`.
pub fn from_runs(runs: &[((i64, i32), (i64, i32))], lost: i64, width: i32) -> Vec<pb::SubSelection> {
    crate::seltext::merge_runs(runs, width)
        .into_iter()
        .map(|(a, b, connected)| {
            let mut sub = from_sel(SelectionType::Simple, a, b, lost, width);
            sub.connected = Some(connected);
            sub
        })
        .collect()
}

/// SetBroadcastDomains' check (`H:4146-4182`): each domain's sessions, by id as
/// `sessionForAPIIdentifier` finds them (buried ones too), all in one window (a buried
/// session is in none: first in a domain it leaves the window to the next one, later it
/// differs from a window); an unknown one SESSION_NOT_FOUND, two windows
/// SESSIONS_NOT_IN_SAME_WINDOW. iTerm2 never fills its set of seen ids, so overlapping
/// domains are not refused (BROADCAST_DOMAINS_NOT_DISJOINT is never answered); the
/// window's helper sorts them out (`Broadcast::set_domains`). The domains by the Registry
/// key of their first session's window, in request order; empty ones and ones whose first
/// session is buried are dropped.
pub fn domains(r: &pb::SetBroadcastDomainsRequest, s: &Snapshot) -> Result<BTreeMap<u64, Vec<BTreeSet<u64>>>, pb::set_broadcast_domains_response::Status> {
    use pb::set_broadcast_domains_response::Status;
    let window_of = |p: u64| s.pane_tab(p).and_then(|t| s.tab_window(t));
    let mut groups = Vec::new();
    for d in &r.broadcast_domains {
        let mut sessions = Vec::new();
        let mut window: Option<u64> = None;
        for id in &d.session_ids {
            let p = s.pane(id).ok_or(Status::SessionNotFound)?;
            let this = window_of(p);
            if window.is_none() {
                window = this;
            } else if window != this {
                return Err(Status::SessionsNotInSameWindow);
            }
            sessions.push(p);
        }
        if let Some(first) = sessions.first() {
            groups.push((window_of(*first), sessions));
        }
    }
    let mut out: BTreeMap<u64, Vec<BTreeSet<u64>>> = BTreeMap::new();
    for (w, sessions) in groups {
        if let Some(w) = w {
            out.entry(w).or_default().push(sessions.into_iter().collect());
        }
    }
    Ok(out)
}

/// A tmux window id as SetWindowVisible takes it (`isNumeric`, `NSStringITerm.m:2055-2062`):
/// decimal digits only, not empty.
pub fn window_number(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// SetWindowVisible's refusal (`H:3680-3708`): showing a window that is not hidden, or
/// hiding one that is, is INVALID_WINDOW_ID.
pub fn visibility_status(hidden: bool, want_visible: bool) -> Option<pb::tmux_response::Status> {
    (hidden != want_visible).then_some(pb::tmux_response::Status::InvalidWindowId)
}

/// ListConnections (`H:3629-3640`): each client name with its gateway session's id, OK. With
/// no connection the list is never touched, so no payload is set.
pub fn connections(list: &[(String, u64)]) -> pb::TmuxResponse {
    use pb::tmux_response::list_connections::Connection;
    let payload = (!list.is_empty()).then(|| {
        let connections = list.iter().map(|(name, gw)| Connection { connection_id: Some(name.clone()), owning_session_id: Some(gw.to_string()) }).collect();
        pb::tmux_response::Payload::ListConnections(pb::tmux_response::ListConnections { connections })
    });
    pb::TmuxResponse { status: Some(pb::tmux_response::Status::Ok as i32), payload }
}

/// CreateWindow's answer (`H:3651-3663`, status unset): nothing when no window was made
/// (iTerm2's completion gets a negative id); else the window's tab id, "0" when the window
/// has no tab (it never opened or closed first: `tab.uniqueId` of a nil tab).
pub fn create_window_answer(made: bool, tab: Option<u64>) -> pb::TmuxResponse {
    let payload = made.then(|| pb::tmux_response::Payload::CreateWindow(pb::tmux_response::CreateWindow { tab_id: Some(tab.unwrap_or(0).to_string()) }));
    pb::TmuxResponse { status: None, payload }
}

/// The tabs (id, panes) that hold any of `panes`, in the order given: the tabs a closing
/// tmux window's panes were waiting in.
pub fn tabs_holding(tabs: &[(u64, Vec<u64>)], panes: &[u64]) -> Vec<u64> {
    tabs.iter().filter(|(_, ps)| ps.iter().any(|p| panes.contains(p))).map(|(t, _)| *t).collect()
}

/// ColorPreset (`H:3883-3950`): a list names every preset; a get answers each key's raw
/// components (alpha 1 and color space "Calibrated" when absent, `H:3917-3940`); an unknown name is
/// PRESET_NOT_FOUND, an unset request REQUEST_MALFORMED.
pub fn color_preset(r: &pb::ColorPresetRequest, find: &dyn Fn(&str) -> Option<crate::color_presets::Preset>, names: &[String]) -> pb::ColorPresetResponse {
    use pb::color_preset_request::Request;
    use pb::color_preset_response::{get_preset::ColorSetting, GetPreset, ListPresets, Response, Status};
    let (status, response) = match &r.request {
        None => (Status::RequestMalformed, None),
        Some(Request::ListPresets(_)) => (Status::Ok, Some(Response::ListPresets(ListPresets { name: names.to_vec() }))),
        Some(Request::GetPreset(g)) => match g.name.as_deref().and_then(find) {
            None => (Status::PresetNotFound, None),
            Some(p) => {
                let color_settings = p
                    .iter()
                    .map(|(key, c)| ColorSetting {
                        red: Some(c.r as f32),
                        green: Some(c.g as f32),
                        blue: Some(c.b as f32),
                        alpha: Some(c.a.unwrap_or(1.0) as f32),
                        color_space: Some(c.space.clone().unwrap_or_else(|| "Calibrated".to_string())),
                        key: Some(key.clone()),
                    })
                    .collect();
                (Status::Ok, Some(Response::GetPreset(GetPreset { color_settings })))
            }
        },
    };
    pb::ColorPresetResponse { status: Some(status as i32), response }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb::set_broadcast_domains_response::Status as DStatus;
    use pb::tmux_response::Status as TStatus;

    fn sub(start: (i32, i64), end: (i32, i64), columns: Option<(i64, i64)>, mode: pb::SelectionMode) -> pb::SubSelection {
        let range = pb::CoordRange { start: Some(pb::Coord { x: Some(start.0), y: Some(start.1) }), end: Some(pb::Coord { x: Some(end.0), y: Some(end.1) }) };
        let columns = columns.map(|(location, length)| pb::Range { location: Some(location), length: Some(length) });
        pb::SubSelection { windowed_coord_range: Some(pb::WindowedCoordRange { coord_range: Some(range), columns }), selection_mode: Some(mode as i32), connected: None }
    }

    #[test]
    fn visual_runs_are_logical_sub_selections() {
        // Two runs on one line (a bidi split): both Character, the first connected.
        let got = from_runs(&[((2, 0), (2, 1)), ((2, 4), (2, 6)), ((3, 2), (3, 3))], 10, 80);
        let span = |s: &pb::SubSelection| {
            let r = s.windowed_coord_range.clone().unwrap().coord_range.unwrap();
            (r.start.unwrap().y(), r.start.unwrap().x(), r.end.unwrap().y(), r.end.unwrap().x())
        };
        assert_eq!(got.len(), 3);
        assert_eq!(span(&got[0]), (12, 0, 12, 2));
        assert_eq!(span(&got[1]), (12, 4, 12, 7));
        assert_eq!(span(&got[2]), (13, 2, 13, 4));
        assert_eq!(got.iter().map(|s| s.connected()).collect::<Vec<_>>(), vec![true, false, false]);
        assert!(got.iter().all(|s| s.selection_mode == Some(pb::SelectionMode::Character as i32)));
        // A run that continues from the right edge of the line above is one sub-selection.
        let got = from_runs(&[((2, 5), (2, 79)), ((3, 0), (3, 3))], 0, 80);
        assert_eq!(got.len(), 1);
        assert_eq!(span(&got[0]), (2, 5, 3, 4));
        assert!(from_runs(&[], 0, 80).is_empty());
    }

    #[test]
    fn selection_follows_iterm2() {
        use pb::SelectionMode as M;
        let c = |start, end| to_sel(&sub(start, end, None, M::Character), 80, 10);
        assert_eq!(c((0, 12), (5, 13)), Some(Sel { ty: SelectionType::Simple, mode: crate::seltext::Mode::Character, start: (2, 0), end: (3, 5), window: None, connected: false }));
        // A line above the history is the first one kept (`H:4046-4049`).
        assert_eq!(c((0, 5), (5, 13)).map(|s| s.start), Some((0, 0)));
        assert_eq!(c((0, 12), (80, 13)).map(|s| s.end), Some((3, 80)), "the width itself is allowed");
        assert_eq!(c((0, 12), (81, 13)), None);
        assert_eq!(c((-1, 12), (5, 13)), None);
        assert_eq!(c((0, -1), (5, 13)), None);
        assert_eq!(c((0, 12), (5, -1)), None);
        assert_eq!(to_sel(&sub((0, 12), (5, 13), Some((0, -1)), M::Character), 80, 10), None);
        assert_eq!(to_sel(&sub((0, 12), (5, 13), Some((70, 20)), M::Character), 80, 10), None);
        assert_eq!(to_sel(&sub((0, 12), (5, 13), Some((-1, 5)), M::Character), 80, 10), None);
        assert_eq!(to_sel(&sub((0, 12), (5, 13), Some((70, 10)), M::Character), 80, 10).map(|s| s.window), Some(Some((70, 79))), "a window is checked, then kept");
        // iTerm2 keeps the range as sent for every mode (`iTermSelection.m:1082-1093`): only a
        // box stays a box; the others are the exact range, never widened to words or lines.
        let ty = |m| to_sel(&sub((0, 12), (5, 13), None, m), 80, 10).map(|s| s.ty);
        assert_eq!(ty(M::Box), Some(SelectionType::Block));
        for m in [M::Word, M::Smart, M::Line, M::WholeLine] {
            assert_eq!(ty(m), Some(SelectionType::Simple), "{m:?}");
        }
        // Unset and unknown modes read as CHARACTER; unset coordinates as zero.
        let mut s = sub((0, 12), (5, 13), None, M::Character);
        s.selection_mode = Some(99);
        assert_eq!(to_sel(&s, 80, 10).map(|s| s.ty), Some(SelectionType::Simple));
        assert_eq!(to_sel(&pb::SubSelection::default(), 80, 10), Some(Sel { ty: SelectionType::Simple, mode: crate::seltext::Mode::Character, start: (0, 0), end: (0, 0), window: None, connected: false }));

        let got = from_sel(SelectionType::Simple, (2, 0), (3, 4), 10, 80);
        let range = got.windowed_coord_range.unwrap();
        let cr = range.coord_range.unwrap();
        assert_eq!((cr.start.unwrap().x(), cr.start.unwrap().y(), cr.end.unwrap().x(), cr.end.unwrap().y()), (0, 12, 5, 13));
        assert_eq!((got.selection_mode, got.connected, range.columns), (Some(M::Character as i32), Some(false), None));
        let end = from_sel(SelectionType::Simple, (2, 0), (3, 79), 10, 80).windowed_coord_range.unwrap().coord_range.unwrap().end.unwrap();
        assert_eq!(end.x(), 80);
        let mode = |ty| from_sel(ty, (0, 0), (0, 0), 0, 80).selection_mode;
        assert_eq!(mode(SelectionType::Block), Some(M::Box as i32));
        assert_eq!(mode(SelectionType::Semantic), Some(M::Word as i32));
        assert_eq!(mode(SelectionType::Lines), Some(M::Line as i32));
    }

    /// Several sub-selections with their modes, windows and connection go through `to_sel` and
    /// back through `from_sub` unchanged (the "one sub only" refusal is gone).
    #[test]
    fn api_selection_round_trips_many() {
        use pb::SelectionMode as M;
        let mut a = sub((2, 12), (7, 12), None, M::Character);
        a.connected = Some(false);
        let mut b = sub((1, 13), (6, 15), None, M::Box);
        b.connected = Some(false);
        let mut c = sub((0, 16), (80, 17), Some((10, 20)), M::WholeLine);
        c.connected = Some(true);
        let mut w = sub((3, 18), (9, 18), None, M::Word);
        w.connected = Some(false);
        for s in [a, b, c, w] {
            let sel = to_sel(&s, 80, 10).unwrap();
            let sub = sel.to_sub(80).expect("it covers cells");
            assert_eq!(from_sub(&sub, 10, 80), s, "{:?}", s.selection_mode);
        }
        let sel = to_sel(&sub((0, 16), (80, 17), Some((10, 20)), M::WholeLine), 80, 10).unwrap();
        let got = sel.to_sub(80).unwrap();
        assert_eq!((got.window, got.mode, got.start, got.end), (Some((10, 29)), crate::seltext::Mode::WholeLine, crate::find::Pos { row: 6, col: 0 }, crate::find::Pos { row: 7, col: 79 }));
        // An empty range (the end is exclusive) covers nothing.
        assert_eq!(to_sel(&sub((4, 12), (4, 12), None, M::Character), 80, 10).unwrap().to_sub(80), None);
        // An end at column 0 ends the line above.
        let got = to_sel(&sub((2, 12), (0, 14), None, M::Character), 80, 10).unwrap().to_sub(80).unwrap();
        assert_eq!(got.end, crate::find::Pos { row: 3, col: 79 });
    }

    /// Final review minor 7: the column-0 end rule is for run modes; a Box ending at column 0
    /// covers no column, not the last one.
    #[test]
    fn a_box_end_at_column_zero_covers_nothing() {
        use pb::SelectionMode as M;
        let b = to_sel(&sub((2, 12), (0, 14), None, M::Box), 80, 10).unwrap();
        assert_eq!(b.to_sub(80), None);
        let line = to_sel(&sub((2, 12), (0, 14), None, M::Character), 80, 10).unwrap();
        assert_eq!(line.to_sub(80).unwrap().end, crate::find::Pos { row: 3, col: 79 });
    }

    /// Window key 1 "pty-A" with tabs 5 (panes 1, 2) and 6 (pane 3), window key 2 "pty-B"
    /// with tab 9 (pane 7); pane 8 buried; pane 3 exited; pane 2 active (Task 1's fixture).
    fn fixture() -> Snapshot {
        Snapshot {
            windows: vec![(1, "pty-A".into(), vec![(5, vec![1, 2]), (6, vec![3])]), (2, "pty-B".into(), vec![(9, vec![7])])],
            buried: vec![8],
            exited: BTreeSet::from([3]),
            active: Some(2),
        }
    }

    fn req(domains: &[&[&str]]) -> pb::SetBroadcastDomainsRequest {
        pb::SetBroadcastDomainsRequest { broadcast_domains: domains.iter().map(|d| pb::BroadcastDomain { session_ids: d.iter().map(|s| s.to_string()).collect() }).collect() }
    }

    #[test]
    fn set_domains_follow_iterm2() {
        let s = fixture();
        let set = |ids: &[u64]| ids.iter().copied().collect::<BTreeSet<u64>>();
        // iTerm2 never records the ids it saw (`H:4150-4159`): overlap is left to the helper.
        assert_eq!(domains(&req(&[&["1", "2"], &["2", "3"]]), &s), Ok(BTreeMap::from([(1, vec![set(&[1, 2]), set(&[2, 3])])])));
        assert_eq!(domains(&req(&[&["1", "99"]]), &s), Err(DStatus::SessionNotFound));
        assert_eq!(domains(&req(&[&["1", "7"]]), &s), Err(DStatus::SessionsNotInSameWindow));
        assert_eq!(domains(&req(&[&[]]), &s), Ok(BTreeMap::new()));
        assert_eq!(domains(&req(&[]), &s), Ok(BTreeMap::new()));
        assert_eq!(domains(&req(&[&["1", "2"], &["7"]]), &s), Ok(BTreeMap::from([(1, vec![set(&[1, 2])]), (2, vec![set(&[7])])])));
        assert_eq!(domains(&req(&[&["active", "3"]]), &s), Ok(BTreeMap::from([(1, vec![set(&[2, 3])])])), "active and exited sessions count");
        // A buried session is in no window: first it defers to the next, later it differs.
        assert_eq!(domains(&req(&[&["8", "1"]]), &s), Ok(BTreeMap::new()), "led by a buried session: no window takes it");
        assert_eq!(domains(&req(&[&["1", "8"]]), &s), Err(DStatus::SessionsNotInSameWindow));
        // The first error wins, in request order.
        assert_eq!(domains(&req(&[&["1", "7"], &["99"]]), &s), Err(DStatus::SessionsNotInSameWindow));
    }

    #[test]
    fn tmux_follows_iterm2() {
        assert_eq!(window_number("3"), Some(3));
        assert_eq!(window_number("12"), Some(12));
        for s in ["@3", "", "x", "-1", "+1", " 3", "3 "] {
            assert_eq!(window_number(s), None, "{s:?}");
        }
        assert_eq!(visibility_status(false, true), Some(TStatus::InvalidWindowId), "showing a shown window");
        assert_eq!(visibility_status(true, false), Some(TStatus::InvalidWindowId), "hiding a hidden window");
        assert_eq!(visibility_status(true, true), None);
        assert_eq!(visibility_status(false, false), None);
        let r = connections(&[("gw1".into(), 12)]);
        assert_eq!(r.status, Some(TStatus::Ok as i32));
        let Some(pb::tmux_response::Payload::ListConnections(l)) = r.payload else { panic!("no list") };
        assert_eq!(l.connections.len(), 1);
        assert_eq!((l.connections[0].connection_id.as_deref(), l.connections[0].owning_session_id.as_deref()), (Some("gw1"), Some("12")));
        assert_eq!(connections(&[]), pb::TmuxResponse { status: Some(TStatus::Ok as i32), payload: None });
    }

    #[test]
    fn create_window_always_answers() {
        use pb::tmux_response::{CreateWindow, Payload};
        let tab = |r: pb::TmuxResponse| match r.payload {
            Some(Payload::CreateWindow(CreateWindow { tab_id })) => tab_id,
            _ => None,
        };
        // Status unset in every case (`H:3651-3663`).
        assert_eq!(create_window_answer(true, Some(12)).status, None);
        assert_eq!(tab(create_window_answer(true, Some(12))).as_deref(), Some("12"));
        // The window was made but its tab is missing (it never opened, or closed first): a nil
        // tab's uniqueId.
        assert_eq!(tab(create_window_answer(true, None)).as_deref(), Some("0"));
        assert_eq!(create_window_answer(false, None), pb::TmuxResponse::default(), "no window made: nothing set");
    }

    #[test]
    fn closing_panes_find_their_pending_tabs() {
        let tabs = [(4, vec![10, 11]), (5, vec![12]), (6, vec![])];
        assert_eq!(tabs_holding(&tabs, &[11]), vec![4]);
        assert_eq!(tabs_holding(&tabs, &[12, 10]), vec![4, 5]);
        assert!(tabs_holding(&tabs, &[99]).is_empty());
        assert!(tabs_holding(&tabs, &[]).is_empty(), "a window with no panes left finds none");
    }

    #[test]
    fn api_color_presets_follow_iterm2() {
        use crate::color_presets::{Preset, PresetColor};
        use pb::color_preset_request::{GetPreset, ListPresets, Request};
        use pb::color_preset_response::{Response, Status};
        let mut tango = Preset::new();
        tango.insert("Ansi 0 Color".into(), PresetColor { r: 0.25, g: 0.5, b: 0.75, a: None, space: None });
        tango.insert("Foreground Color".into(), PresetColor { r: 1.0, g: 0.0, b: 0.0, a: Some(0.5), space: Some("P3".into()) });
        let find = |n: &str| (n == "Tango Dark").then(|| tango.clone());
        let names = vec!["Solarized".to_string(), "Tango Dark".to_string()];
        let answer = |r: Option<Request>| color_preset(&pb::ColorPresetRequest { request: r }, &find, &names);
        let list = answer(Some(Request::ListPresets(ListPresets {})));
        assert_eq!(list.status, Some(Status::Ok as i32));
        assert_eq!(list.response, Some(Response::ListPresets(pb::color_preset_response::ListPresets { name: names.clone() })));
        let get = answer(Some(Request::GetPreset(GetPreset { name: Some("Tango Dark".into()) })));
        assert_eq!(get.status, Some(Status::Ok as i32));
        let Some(Response::GetPreset(g)) = get.response else { panic!("no preset") };
        assert_eq!(g.color_settings.len(), 2);
        let ansi = g.color_settings.iter().find(|c| c.key.as_deref() == Some("Ansi 0 Color")).unwrap();
        assert_eq!((ansi.red, ansi.green, ansi.blue, ansi.alpha), (Some(0.25), Some(0.5), Some(0.75), Some(1.0)));
        assert_eq!(ansi.color_space.as_deref(), Some("Calibrated"));
        let fg = g.color_settings.iter().find(|c| c.key.as_deref() == Some("Foreground Color")).unwrap();
        assert_eq!((fg.red, fg.alpha, fg.color_space.as_deref()), (Some(1.0), Some(0.5), Some("P3")), "raw components, not converted");
        let missing = answer(Some(Request::GetPreset(GetPreset { name: Some("Nope".into()) })));
        assert_eq!((missing.status, missing.response), (Some(Status::PresetNotFound as i32), None));
        assert_eq!(answer(None).status, Some(Status::RequestMalformed as i32));
    }
}
