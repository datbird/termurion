//! Broadcast input (iTerm2's iTermBroadcastInputHelper): per window, a mode (off, all panes
//! in the current tab, all tabs, or custom domains), per-tab flags, and broadcast sources;
//! who sends, who receives, and every transition the menus make, quirks included.

use std::collections::BTreeSet;

pub type PaneId = u64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Off,
    AllPanes,
    AllTabs,
    Custom,
}

/// A tab as the model sees it: its broadcast flag and its panes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabView {
    pub broadcasting: bool,
    pub panes: Vec<PaneId>,
}

/// The window: its tabs (flags are changed in place), the current tab and pane, and the
/// panes whose shell ended.
#[derive(Clone, Debug)]
pub struct View {
    pub tabs: Vec<TabView>,
    pub current_tab: usize,
    pub current: PaneId,
    pub exited: BTreeSet<PaneId>,
}

impl View {
    fn tab_of(&self, p: PaneId) -> Option<&TabView> {
        self.tabs.iter().find(|t| t.panes.contains(&p))
    }

    fn all(&self) -> BTreeSet<PaneId> {
        self.tabs.iter().flat_map(|t| t.panes.iter().copied()).collect()
    }

    /// This view with pane `p` and its tab current, or None when `p` is not in it (an answered
    /// warning acts where it was asked).
    pub fn at(mut self, p: PaneId) -> Option<View> {
        self.current_tab = self.tabs.iter().position(|t| t.panes.contains(&p))?;
        self.current = p;
        Some(self)
    }

    fn cur(&self) -> Option<&TabView> {
        self.tabs.get(self.current_tab)
    }

    fn cur_mut(&mut self) -> Option<&mut TabView> {
        self.tabs.get_mut(self.current_tab)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Broadcast {
    /// The mode as set; `mode` is what the menus see.
    pub raw: Mode,
    /// Custom mode's disjoint sets of panes.
    pub domains: Vec<BTreeSet<PaneId>>,
    /// Broadcast sources: in a domain with a source, only the source sends.
    pub sources: BTreeSet<PaneId>,
}

impl Broadcast {
    /// The mode the menus see: All Panes shows only in a tab that broadcasts.
    pub fn mode(&self, v: &View) -> Mode {
        match self.raw {
            Mode::AllPanes if !v.cur().is_some_and(|t| t.broadcasting) => Mode::Off,
            m => m,
        }
    }

    pub fn domain_containing(&self, v: &View, p: PaneId) -> Option<BTreeSet<PaneId>> {
        match self.raw {
            Mode::Off => None,
            Mode::AllTabs => Some(v.all()),
            Mode::AllPanes => v.tab_of(p).filter(|t| t.broadcasting).map(|t| t.panes.iter().copied().collect()),
            Mode::Custom => self.domains.iter().find(|d| d.contains(&p)).cloned(),
        }
    }

    pub fn current_domain(&self, v: &View) -> BTreeSet<PaneId> {
        match self.raw {
            Mode::Off => BTreeSet::new(),
            Mode::AllTabs => v.all(),
            Mode::AllPanes => v.cur().filter(|t| t.broadcasting).map(|t| t.panes.iter().copied().collect()).unwrap_or_default(),
            Mode::Custom => self.domains.iter().find(|d| d.contains(&v.current)).cloned().unwrap_or_default(),
        }
    }

    /// Whether `p` is in a broadcast domain; with `sender`, whether that sender may send
    /// there (a domain with a source takes input only from it).
    pub fn should_broadcast_to(&self, v: &View, p: PaneId, sender: Option<PaneId>) -> bool {
        if let (Some(sender), Some(d)) = (sender, self.domain_containing(v, p)) {
            if self.sources.iter().any(|s| d.contains(s)) && !self.sources.contains(&sender) {
                return false;
            }
        }
        match self.raw {
            Mode::Off => false,
            Mode::AllTabs => true,
            Mode::AllPanes => v.tab_of(p).is_some_and(|t| t.broadcasting),
            Mode::Custom => self.domains.iter().any(|d| d.contains(&p)),
        }
    }

    /// Where `sender`'s typed input goes: every live pane of the window's current domain
    /// (the sender included) when it sends, else the sender alone.
    pub fn targets(&self, v: &View, sender: PaneId) -> Vec<PaneId> {
        // A pane of another window is not this window's to broadcast from.
        if !v.all().contains(&sender) || !self.should_broadcast_to(v, sender, Some(sender)) {
            return vec![sender];
        }
        let all = v.all();
        let out: Vec<PaneId> = self.current_domain(v).into_iter().filter(|p| all.contains(p) && !v.exited.contains(p)).collect();
        if out.is_empty() { vec![sender] } else { out }
    }

    /// Every pane of the window's current domain still in the window (Clear, Reset, and
    /// Scroll to End act on these, sources or not, as iTerm2's broadcastSessions).
    pub fn domain_panes(&self, v: &View) -> Vec<PaneId> {
        let all = v.all();
        self.current_domain(v).into_iter().filter(|p| all.contains(p)).collect()
    }

    pub fn receives(&self, v: &View, p: PaneId) -> bool {
        self.should_broadcast_to(v, p, None)
    }

    pub fn sends(&self, v: &View, p: PaneId) -> bool {
        self.receives(v, p) && self.should_broadcast_to(v, p, Some(p))
    }

    /// The core transition. `confirm` shows the warning (going from off to on) and says
    /// whether to go on. False when nothing changed.
    pub fn set_mode(&mut self, v: &mut View, mode: Mode, confirm: &mut dyn FnMut(&str) -> bool) -> bool {
        let seen = self.mode(v);
        if mode != Mode::Custom && mode == seen {
            // "Again" in the mode the menus show.
            if mode == Mode::AllPanes {
                if let Some(t) = v.cur_mut() {
                    t.broadcasting = !t.broadcasting;
                }
                return true;
            }
            return false;
        }
        if mode != Mode::Off && seen == Mode::Off && !confirm(&warning_title(v, mode)) {
            return false;
        }
        if let Some(t) = v.cur_mut() {
            t.broadcasting = mode == Mode::AllPanes;
        }
        self.raw = mode;
        true
    }

    /// The warning before turning broadcasting on answered OK: All Tabs (`all_tabs`) or All
    /// Panes in Current Tab goes on, but only while broadcasting is still off (the warning
    /// is not modal, so the mode may have changed since it was asked). False when nothing
    /// changed.
    pub fn grant(&mut self, v: &mut View, all_tabs: bool) -> bool {
        if self.mode(v) != Mode::Off {
            return false;
        }
        let to = if all_tabs { Mode::AllTabs } else { Mode::AllPanes };
        self.set_mode(v, to, &mut |_| true)
    }

    /// Shell > Broadcast Input > Send Input to Current Session Only.
    pub fn disable(&mut self, v: &mut View) -> bool {
        self.set_mode(v, Mode::Off, &mut |_| true)
    }

    /// Broadcast Input to All Panes in All Tabs (again: off).
    pub fn all_tabs(&mut self, v: &mut View, confirm: &mut dyn FnMut(&str) -> bool) -> bool {
        let to = if self.raw == Mode::AllTabs { Mode::Off } else { Mode::AllTabs };
        self.set_mode(v, to, confirm)
    }

    /// Broadcast Input to All Panes in Current Tab (again: the whole window off).
    pub fn current_tab(&mut self, v: &mut View, confirm: &mut dyn FnMut(&str) -> bool) -> bool {
        let to = if self.mode(v) == Mode::AllPanes { Mode::Off } else { Mode::AllPanes };
        self.set_mode(v, to, confirm)
    }

    /// Toggle Broadcast Input to Current Session, and the context menu's toggle.
    pub fn toggle_session(&mut self, v: &mut View, p: PaneId) {
        match self.mode(v) {
            Mode::AllPanes => {
                let panes = v.cur().map(|t| t.panes.iter().copied().collect()).unwrap_or_default();
                if let Some(t) = v.cur_mut() {
                    t.broadcasting = false;
                }
                self.domains = vec![panes];
            }
            Mode::AllTabs => self.domains = vec![v.all()],
            Mode::Off => self.domains.clear(),
            Mode::Custom => {}
        }
        self.raw = Mode::Custom;
        let mut removed = false;
        for d in &mut self.domains {
            removed |= d.remove(&p);
        }
        if removed {
            self.domains.retain(|d| d.len() >= 2);
        } else if self.domains.is_empty() {
            self.domains.push(BTreeSet::from([p, v.current]));
        } else if self.domains.len() == 1 {
            self.domains[0].insert(p);
        } else {
            let i = self.domains.iter().position(|d| d.contains(&v.current)).unwrap_or(0);
            self.domains[i].insert(p);
        }
        if self.domains.is_empty() {
            self.raw = Mode::Off;
        }
    }

    /// The Python API's SetBroadcastDomains for this window (iTerm2's setBroadcastDomains:,
    /// `BI:154-194`): the domains sanitized (a pane stays in the last domain naming it, panes
    /// the window does not have go, and so do domains of fewer than two). None left while off
    /// changes nothing (false). Otherwise no tab broadcasts any more and the mode is custom
    /// with those domains, or off when none are left.
    pub fn set_domains(&mut self, v: &mut View, domains: Vec<BTreeSet<PaneId>>) -> bool {
        let valid = v.all();
        let mut index = std::collections::BTreeMap::new();
        for (i, d) in domains.iter().enumerate() {
            for p in d {
                index.insert(*p, i);
            }
        }
        let mut grouped: Vec<BTreeSet<PaneId>> = vec![BTreeSet::new(); domains.len()];
        for (p, i) in index {
            if valid.contains(&p) {
                grouped[i].insert(p);
            }
        }
        grouped.retain(|d| d.len() > 1);
        if grouped.is_empty() && self.raw == Mode::Off {
            return false;
        }
        for t in &mut v.tabs {
            t.broadcasting = false;
        }
        self.raw = if grouped.is_empty() { Mode::Off } else { Mode::Custom };
        self.domains = grouped;
        true
    }

    /// Panes that left the window (their tab moved to another): they leave its domains and
    /// sources; a domain of fewer than two goes, and custom mode with none left is off.
    pub fn forget(&mut self, panes: &[PaneId]) {
        for d in &mut self.domains {
            for p in panes {
                d.remove(p);
            }
        }
        self.domains.retain(|d| d.len() >= 2);
        self.sources.retain(|s| !panes.contains(s));
        if self.raw == Mode::Custom && self.domains.is_empty() {
            self.raw = Mode::Off;
        }
    }

    /// Current Session is Broadcast Source.
    pub fn toggle_source(&mut self, v: &View, p: PaneId) {
        let was = self.sources.contains(&p);
        if let Some(d) = self.domain_containing(v, p) {
            self.sources.retain(|s| !d.contains(s));
        }
        self.sources.remove(&p);
        if !was {
            self.sources.insert(p);
        }
    }
}

/// A write to a session, by where it comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    Key,
    KeyRelease,
    Paste,
    Drop,
    SendText,
    /// Key action 76, Send Text without Broadcasting.
    SendTextLocal,
    AltClickArrows,
    /// The alternate-screen wheel's arrow keys (13j: iTerm2 broadcasts them, unlike mouse reports).
    WheelArrows,
    MouseReport,
    FocusReport,
    Trigger,
    Upload,
    BufferRelease,
    /// An AI chat's tool typing into its linked session (iTerm2's writeTaskNoBroadcast).
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// To every session of the sender's broadcast domain.
    Broadcast,
    /// To the sender only.
    Local,
}

/// Which writes broadcast (iTerm2's canBroadcast).
pub fn route(w: Write) -> Route {
    match w {
        Write::Key | Write::KeyRelease | Write::Paste | Write::Drop | Write::SendText | Write::AltClickArrows | Write::WheelArrows => Route::Broadcast,
        Write::SendTextLocal | Write::MouseReport | Write::FocusReport | Write::Trigger | Write::Upload | Write::BufferRelease | Write::Agent => Route::Local,
    }
}

/// iTerm2's sendInputToAllSessions skips gateways: a gateway's keys stay in it, and no
/// other pane's typing reaches one.
pub fn without_gateways(targets: Vec<u64>, sender: u64, gateways: &BTreeSet<u64>) -> Vec<u64> {
    if gateways.contains(&sender) {
        return vec![sender];
    }
    targets.into_iter().filter(|t| !gateways.contains(t)).collect()
}

/// Where a write from `sender` goes: the rule of `route`, then the sender's domain or the sender
/// alone, then `without_gateways` (`App::user_write`'s own target rule, here so it is testable
/// without an App).
pub fn write_targets(b: &Broadcast, v: &View, sender: PaneId, what: Write, gateways: &BTreeSet<u64>) -> Vec<PaneId> {
    let targets = match route(what) {
        Route::Broadcast => b.targets(v, sender),
        Route::Local => vec![sender],
    };
    without_gateways(targets, sender, gateways)
}

/// What a program-chosen write checks on each target pane before it is typed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TargetState {
    pub exited: bool,
    pub ended: bool,
    pub password: bool,
}

/// Program-chosen text (Command Info, the navigator, the buttons) is never typed into a pane
/// that is ended or reading a password, also when broadcast input carries it there. Returns the
/// panes to write and how many were skipped. A pane that is gone is dropped silently.
pub fn program_targets(targets: Vec<PaneId>, state: impl Fn(PaneId) -> Option<TargetState>) -> (Vec<PaneId>, usize) {
    let mut skipped = 0;
    let mut keep = Vec::new();
    for t in targets {
        match state(t) {
            None => {}
            Some(s) if s.exited || s.ended || s.password => skipped += 1,
            Some(_) => keep.push(t),
        }
    }
    (keep, skipped)
}

/// Writes that are typing (iTerm2's writeTask): a tmux gateway reads them as its Command
/// Menu's keys. The rest never reach a gateway.
pub fn typed(w: Write) -> bool {
    matches!(w, Write::Key | Write::Paste | Write::Drop | Write::SendText | Write::SendTextLocal | Write::AltClickArrows | Write::WheelArrows | Write::Agent)
}

/// Clear Buffer with two or more broadcast sessions: (heading, text, buttons).
pub fn clear_question(n: usize) -> (&'static str, String, [&'static str; 3]) {
    ("Clear in All Broadcasted-to Sessions?", format!("Clear all sessions to which input is broadcast? This will affect {n} sessions."), ["Clear All", "Clear Current Session Only", "Cancel"])
}

/// Reset with two or more broadcast sessions: (heading, text, buttons).
pub fn reset_question(n: usize) -> (&'static str, String, [&'static str; 3]) {
    ("Reset in All Broadcasted-to Sessions?", format!("Reset all sessions to which input is broadcast? This will affect {n} sessions."), ["Reset All", "Reset Current Session Only", "Cancel"])
}

/// The warning's text for turning `mode` on.
pub fn warning_title(v: &View, mode: Mode) -> String {
    match mode {
        Mode::AllTabs => {
            let n = v.all().len();
            if n < 2 { "Keyboard input will be sent to all panes in all tabs in this window.".into() } else { format!("Keyboard input will be sent to {n} panes across all tabs in this window.") }
        }
        Mode::AllPanes => {
            let n = v.cur().map_or(0, |t| t.panes.iter().filter(|p| !v.exited.contains(p)).count());
            if n < 2 { "Keyboard input will be sent to all panes in the current tab.".into() } else { format!("Keyboard input will be sent to {n} panes in the current tab.") }
        }
        Mode::Custom => "Keyboard input will be sent to multiple sessions.".into(),
        Mode::Off => "Keyboard input will be sent to no sessions.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tab 0: panes 1 and 2; tab 1: pane 3. Current: tab 0, pane 1.
    fn view() -> View {
        View { tabs: vec![TabView { broadcasting: false, panes: vec![1, 2] }, TabView { broadcasting: false, panes: vec![3] }], current_tab: 0, current: 1, exited: BTreeSet::new() }
    }

    fn yes() -> impl FnMut(&str) -> bool {
        |_| true
    }

    #[test]
    fn modes_toggle_like_iterm2() {
        let (mut b, mut v) = (Broadcast::default(), view());
        let mut asked = Vec::new();
        assert!(b.all_tabs(&mut v, &mut |t| {
            asked.push(t.to_string());
            true
        }));
        assert_eq!(asked, vec!["Keyboard input will be sent to 3 panes across all tabs in this window.".to_string()]);
        assert_eq!(b.mode(&v), Mode::AllTabs);
        assert_eq!(b.targets(&v, 1), vec![1, 2, 3]);
        b.all_tabs(&mut v, &mut yes());
        assert_eq!(b.mode(&v), Mode::Off, "again: off");
        assert_eq!(b.targets(&v, 1), vec![1]);
        let mut asked = Vec::new();
        b.current_tab(&mut v, &mut |t| {
            asked.push(t.to_string());
            true
        });
        assert_eq!(asked, vec!["Keyboard input will be sent to 2 panes in the current tab.".to_string()]);
        assert!(v.tabs[0].broadcasting && b.mode(&v) == Mode::AllPanes);
        assert_eq!(b.targets(&v, 2), vec![1, 2]);
        b.current_tab(&mut v, &mut yes());
        assert!(!v.tabs[0].broadcasting && b.mode(&v) == Mode::Off, "again: the window goes off");
        // Cancel leaves everything as it was.
        assert!(!b.current_tab(&mut v, &mut |_| false));
        assert_eq!((b.raw, v.tabs[0].broadcasting), (Mode::Off, false));
        // Current Tab to All Tabs asks nothing (the menu mode was not off).
        b.current_tab(&mut v, &mut yes());
        let mut asked = 0;
        b.all_tabs(&mut v, &mut |_| {
            asked += 1;
            true
        });
        assert_eq!((asked, b.mode(&v)), (0, Mode::AllTabs));
    }

    /// An answered warning acts where it was asked: the anchor pane's tab and the anchor
    /// become current; a pane no longer in the window gives nothing.
    #[test]
    fn a_view_anchors_on_the_asking_pane() {
        let v = view().at(3).unwrap();
        assert_eq!((v.current_tab, v.current), (1, 3));
        let mut b = Broadcast::default();
        let mut v = view().at(2).unwrap();
        b.current_tab(&mut v, &mut yes());
        assert_eq!((v.tabs[0].broadcasting, v.tabs[1].broadcasting), (true, false));
        let mut v = view().at(3).unwrap();
        let mut b = Broadcast::default();
        b.current_tab(&mut v, &mut yes());
        assert_eq!((v.tabs[0].broadcasting, v.tabs[1].broadcasting), (false, true), "the anchor's tab, not the front one");
        assert!(view().at(9).is_none());
    }

    /// The warning is not modal: an OK acts only while broadcasting is still off, so a second
    /// queued warning's OK never turns it back off.
    #[test]
    fn a_granted_warning_turns_on_only_while_off() {
        for all_tabs in [true, false] {
            let (mut b, mut v) = (Broadcast::default(), view());
            assert!(b.grant(&mut v, all_tabs));
            let on = if all_tabs { Mode::AllTabs } else { Mode::AllPanes };
            assert_eq!(b.mode(&v), on);
            assert!(!b.grant(&mut v, all_tabs), "the second OK changes nothing");
            assert_eq!(b.mode(&v), on);
            assert!(!b.grant(&mut v, !all_tabs), "nor the other item's");
            assert_eq!(b.mode(&v), on);
        }
    }

    #[test]
    fn panes_that_leave_the_window_leave_its_broadcast() {
        let mut b = Broadcast { raw: Mode::Custom, domains: vec![BTreeSet::from([1, 2, 3]), BTreeSet::from([4, 5])], sources: BTreeSet::from([2, 4]) };
        b.forget(&[3, 4]);
        assert_eq!((b.raw, b.domains.clone(), b.sources.clone()), (Mode::Custom, vec![BTreeSet::from([1, 2])], BTreeSet::from([2])), "a domain of one goes");
        b.forget(&[1]);
        assert_eq!((b.raw, b.domains.is_empty()), (Mode::Off, true), "custom with no domain left is off");
        let mut all = Broadcast { raw: Mode::AllTabs, ..Default::default() };
        all.forget(&[7]);
        assert_eq!(all.raw, Mode::AllTabs, "other modes follow the window's tabs");
    }

    #[test]
    fn toggle_session_builds_domains() {
        let (mut b, mut v) = (Broadcast::default(), view());
        b.toggle_session(&mut v, 3);
        assert_eq!((b.raw, b.domains.clone()), (Mode::Custom, vec![BTreeSet::from([1, 3])]), "the pane and the current one");
        b.toggle_session(&mut v, 2);
        assert_eq!(b.domains, vec![BTreeSet::from([1, 2, 3])]);
        b.toggle_session(&mut v, 3);
        assert_eq!(b.domains, vec![BTreeSet::from([1, 2])]);
        b.toggle_session(&mut v, 2);
        assert_eq!((b.raw, b.domains.len()), (Mode::Off, 0), "a domain of one goes, and with none the mode is off");
        // The current pane alone: a one-member domain, kept on purpose.
        b.toggle_session(&mut v, 1);
        assert_eq!(b.domains, vec![BTreeSet::from([1])]);
        assert!(b.receives(&v, 1) && b.sends(&v, 1));
        // From Current Tab: the tab's panes become the domain, and the tab flag goes.
        let (mut b, mut v) = (Broadcast::default(), view());
        b.current_tab(&mut v, &mut yes());
        b.toggle_session(&mut v, 3);
        assert_eq!((b.raw, v.tabs[0].broadcasting), (Mode::Custom, false));
        assert_eq!(b.domains, vec![BTreeSet::from([1, 2, 3])]);
    }

    #[test]
    fn set_domains_apply_like_iterm2() {
        let set = |ids: &[PaneId]| ids.iter().copied().collect::<BTreeSet<PaneId>>();
        // Off, and nothing valid: nothing changes (`BI:180-184`).
        let (mut b, mut v) = (Broadcast::default(), view());
        v.tabs[1].broadcasting = true;
        assert!(!b.set_domains(&mut v, vec![set(&[1])]));
        assert!(!b.set_domains(&mut v, vec![]));
        assert_eq!((b.raw, v.tabs[1].broadcasting), (Mode::Off, true));
        // A domain: custom mode, and every tab stops broadcasting.
        assert!(b.set_domains(&mut v, vec![set(&[1, 2])]));
        assert_eq!((b.raw, b.domains.clone()), (Mode::Custom, vec![set(&[1, 2])]));
        assert!(v.tabs.iter().all(|t| !t.broadcasting));
        assert_eq!(b.targets(&v, 1), vec![1, 2]);
        // One pane, or a pane the window does not have, leaves nothing: off.
        assert!(b.set_domains(&mut v, vec![set(&[1])]));
        assert_eq!((b.raw, b.domains.clone()), (Mode::Off, vec![]));
        b.set_domains(&mut v, vec![set(&[1, 2])]);
        assert!(b.set_domains(&mut v, vec![set(&[1, 99])]));
        assert_eq!((b.raw, b.domains.clone()), (Mode::Off, vec![]));
        // From All Panes the tab's flag is cleared too.
        let (mut b, mut v) = (Broadcast::default(), view());
        b.current_tab(&mut v, &mut yes());
        assert!(b.set_domains(&mut v, vec![set(&[2, 3, 99])]));
        assert_eq!((b.raw, b.domains.clone(), v.tabs[0].broadcasting), (Mode::Custom, vec![set(&[2, 3])], false));
        // Overlapping domains: a pane stays in the last one naming it (`BI:161-168`).
        assert!(b.set_domains(&mut v, vec![set(&[1, 2]), set(&[2, 3])]));
        assert_eq!(b.domains, vec![set(&[2, 3])]);
        assert!(b.set_domains(&mut v, vec![set(&[1, 2, 3]), set(&[3])]));
        assert_eq!(b.domains, vec![set(&[1, 2])]);
    }

    #[test]
    fn sources_limit_senders() {
        let (mut b, mut v) = (Broadcast::default(), view());
        b.all_tabs(&mut v, &mut yes());
        b.toggle_source(&v, 1);
        assert_eq!(b.sources, BTreeSet::from([1]));
        assert!(b.sends(&v, 1) && !b.sends(&v, 2) && b.receives(&v, 2), "2 only receives");
        assert_eq!(b.targets(&v, 2), vec![2], "typing in 2 stays in 2");
        assert_eq!(b.targets(&v, 1), vec![1, 2, 3]);
        b.toggle_source(&v, 2);
        assert_eq!(b.sources, BTreeSet::from([2]), "one source per domain");
        b.toggle_source(&v, 2);
        assert!(b.sources.is_empty());
    }

    #[test]
    fn quirks_follow_iterm2() {
        let (mut b, mut v) = (Broadcast::default(), view());
        b.current_tab(&mut v, &mut yes());
        // In the other tab the menus see Off, and Current Session Only changes nothing.
        v.current_tab = 1;
        v.current = 3;
        assert_eq!(b.mode(&v), Mode::Off);
        b.disable(&mut v);
        assert!(v.tabs[0].broadcasting && b.should_broadcast_to(&v, 1, Some(1)), "tab 0 still broadcasts inside itself");
        // A stale flag comes back: All Tabs from tab 1 leaves tab 0's flag, and Current Tab
        // revives it.
        let (mut b, mut v) = (Broadcast::default(), view());
        b.current_tab(&mut v, &mut yes());
        v.current_tab = 1;
        v.current = 3;
        b.all_tabs(&mut v, &mut yes());
        assert!(v.tabs[0].broadcasting);
        b.current_tab(&mut v, &mut yes());
        assert!(b.receives(&v, 1) && b.receives(&v, 3), "both tabs broadcast");
    }

    #[test]
    fn targets_skip_gone_panes() {
        let mut v = view();
        v.exited.insert(2);
        let b = Broadcast { raw: Mode::Custom, domains: vec![BTreeSet::from([1, 2, 9])], sources: BTreeSet::new() };
        assert_eq!(b.targets(&v, 1), vec![1], "9 left the window and 2 ended");
        // A sender outside the window's current domain: iTerm2 writes to the current domain;
        // when that is empty, the input stays with the sender.
        v.current = 3;
        v.current_tab = 1;
        assert_eq!(b.targets(&v, 1), vec![1]);
    }

    #[test]
    fn local_writes_stay_local() {
        for w in [Write::Key, Write::KeyRelease, Write::Paste, Write::Drop, Write::SendText, Write::AltClickArrows] {
            assert_eq!(route(w), Route::Broadcast, "{w:?}");
        }
        for w in [Write::SendTextLocal, Write::MouseReport, Write::FocusReport, Write::Trigger, Write::Upload, Write::BufferRelease] {
            assert_eq!(route(w), Route::Local, "{w:?}");
        }
    }

    #[test]
    fn gateways_never_broadcast() {
        let gateways: BTreeSet<u64> = [2].into();
        assert_eq!(without_gateways(vec![1, 2, 3], 1, &gateways), vec![1, 3], "no typing reaches a gateway");
        assert_eq!(without_gateways(vec![1, 2, 3], 2, &gateways), vec![2], "a gateway's keys stay in it");
        assert_eq!(without_gateways(vec![2], 2, &gateways), vec![2]);
    }

    #[test]
    fn wheel_arrows_broadcast_but_wheel_reports_stay_local() {
        // 13j: Tandem can now tell the two apart (m7f kept both local before).
        let b = Broadcast { raw: Mode::AllPanes, ..Default::default() };
        let v = View { tabs: vec![TabView { broadcasting: true, panes: vec![7, 8] }], current_tab: 0, current: 7, exited: BTreeSet::new() };
        let none = BTreeSet::new();
        assert_eq!(write_targets(&b, &v, 7, Write::WheelArrows, &none), vec![7, 8]);
        assert!(typed(Write::WheelArrows));
        assert_eq!(write_targets(&b, &v, 7, Write::MouseReport, &none), vec![7]);
    }

    #[test]
    fn agent_writes_are_local_and_typed() {
        assert_eq!(route(Write::Agent), Route::Local, "an AI's typing never broadcasts");
        assert!(typed(Write::Agent), "a tmux pane gets it as send-keys");
    }

    #[test]
    fn agent_writes_reach_only_the_sender() {
        let b = Broadcast { raw: Mode::AllPanes, ..Default::default() };
        let v = View { tabs: vec![TabView { broadcasting: true, panes: vec![7, 8] }], current_tab: 0, current: 7, exited: BTreeSet::new() };
        let none = BTreeSet::new();
        assert_eq!(write_targets(&b, &v, 7, Write::Key, &none), vec![7, 8], "the control: broadcasting is on");
        assert_eq!(write_targets(&b, &v, 7, Write::Agent, &none), vec![7]);
        let gateways: BTreeSet<u64> = [7].into();
        assert_eq!(write_targets(&b, &v, 7, Write::Agent, &gateways), vec![7], "without_gateways keeps the sender");
    }

    #[test]
    fn program_text_skips_panes_at_a_password_prompt_or_ended() {
        let b = Broadcast { raw: Mode::AllPanes, ..Default::default() };
        let v = View { tabs: vec![TabView { broadcasting: true, panes: vec![7, 8, 9] }], current_tab: 0, current: 7, exited: BTreeSet::new() };
        let none = BTreeSet::new();
        let targets = write_targets(&b, &v, 7, Write::SendText, &none);
        assert_eq!(targets, vec![7, 8, 9], "the control: all three receive a broadcast");
        let state = |p: PaneId| match p {
            8 => Some(TargetState { password: true, ..Default::default() }),
            9 => Some(TargetState { ended: true, ..Default::default() }),
            _ => Some(TargetState::default()),
        };
        assert_eq!(program_targets(targets, state), (vec![7], 2), "the password pane and the ended pane are skipped and counted");
        assert_eq!(program_targets(vec![7, 5], |p| (p == 7).then(TargetState::default)), (vec![7], 0), "a pane that is gone is dropped");
    }

    #[test]
    fn gateways_read_only_typing() {
        for w in [Write::Key, Write::Paste, Write::Drop, Write::SendText, Write::SendTextLocal, Write::AltClickArrows] {
            assert!(typed(w), "{w:?}");
        }
        for w in [Write::KeyRelease, Write::MouseReport, Write::FocusReport, Write::Trigger, Write::Upload, Write::BufferRelease] {
            assert!(!typed(w), "{w:?}");
        }
    }

    #[test]
    fn clear_and_reset_questions() {
        assert_eq!(clear_question(3), ("Clear in All Broadcasted-to Sessions?", "Clear all sessions to which input is broadcast? This will affect 3 sessions.".to_string(), ["Clear All", "Clear Current Session Only", "Cancel"]));
        assert_eq!(reset_question(2), ("Reset in All Broadcasted-to Sessions?", "Reset all sessions to which input is broadcast? This will affect 2 sessions.".to_string(), ["Reset All", "Reset Current Session Only", "Cancel"]));
    }

    #[test]
    fn senders_outside_the_window_stay_local() {
        let (mut b, mut v) = (Broadcast::default(), view());
        b.all_tabs(&mut v, &mut yes());
        assert_eq!(b.targets(&v, 42), vec![42], "a pane of another window (a drop there) is not this window's");
        b.toggle_source(&v, 1);
        v.exited.insert(3);
        assert_eq!(b.domain_panes(&v), vec![1, 2, 3], "sources and ended panes do not narrow it");
        assert!(Broadcast::default().domain_panes(&v).is_empty());
    }

    #[test]
    fn warning_titles_count_panes() {
        let one = View { tabs: vec![TabView { broadcasting: false, panes: vec![1] }], current_tab: 0, current: 1, exited: BTreeSet::new() };
        assert_eq!(warning_title(&one, Mode::AllTabs), "Keyboard input will be sent to all panes in all tabs in this window.");
        assert_eq!(warning_title(&one, Mode::AllPanes), "Keyboard input will be sent to all panes in the current tab.");
        let mut v = view();
        v.exited.insert(2);
        assert_eq!(warning_title(&v, Mode::AllPanes), "Keyboard input will be sent to all panes in the current tab.", "live panes only");
        assert_eq!(warning_title(&v, Mode::AllTabs), "Keyboard input will be sent to 3 panes across all tabs in this window.", "ended ones count here");
        assert_eq!(warning_title(&v, Mode::Custom), "Keyboard input will be sent to multiple sessions.");
    }
}
