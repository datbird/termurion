//! The session toolbar's strip (milestone 13y1, Task 5): drawing each member's 40 points, its
//! clicks and shortcuts, and the AI Usage runner. The pure side is `workgroup_toolbar`.
//!
//! Citations: `STV` `iTermSessionToolbarView.swift`, `WPS` `WorkgroupPeerSegmentedControl.swift`,
//! `WUT` `WorkgroupUsageToolbarItem.swift`, `WPP` `iTermWorkgroupPeerPort.swift` under
//! `~/gitrepos/iterm2-ref/sources`.
//!
//! The strip draws the items this task owns: the Name label, the peer Mode Switcher, Reload, the
//! Spacer and AI Usage. The git items, navigation and the two auto toggles are Tasks 6 and 7;
//! until they land those items take no room.

use super::*;
use crate::workgroup::{Mode, ToolbarItem};
use crate::workgroup_peers as peers;
use crate::workgroup_toolbar::{self as wt, Owner, UsageReport};
use crate::layout::visible_rects;
use crate::palette::Rgb;
use crate::{semantic, session_state};
use crate::workgroup_git as wgit;
use super::workgroup_git_app::Nav;
use super::code_review_app::Toggle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

/// A usage command that runs longer than this is killed and the item says so (P11).
pub(super) const USAGE_TIMEOUT: Duration = Duration::from_secs(10);
/// A usage command's output is read to this many bytes at most (`WUT:376`).
pub(super) const USAGE_OUTPUT_CAP: usize = wt::OUTPUT_CAP;
/// The most diagnostic bytes of a child's stderr kept for a report.
const STDERR_CAP: usize = 64 * 1024;
/// Padding inside a Mode Switcher segment and a button, in points.
const SEG_PAD: f32 = 10.0;
const BUTTON_W: f32 = 28.0;
/// The Reload glyph (Segoe Fluent Icons Refresh), the busy member's mark (Sync).
const GLYPH_RELOAD: char = '\u{E72C}';
const GLYPH_BUSY: char = '\u{E895}';
/// Back, Forward (Segoe Fluent Icons) and the pulldown's chevron.
const GLYPH_BACK: char = '\u{E72B}';
const GLYPH_FORWARD: char = '\u{E72A}';
const GLYPH_CHEVRON: char = '\u{E70D}';
/// The auto-send toggle (Send) and the auto-request toggle (Review), in Segoe Fluent Icons.
const GLYPH_SEND: char = '\u{E724}';
const GLYPH_REVIEW: char = '\u{E7C3}';
/// A usage row's text size against the terminal font, and a bar's height in points (`WUT`: 5).
const USAGE_TEXT: f32 = 0.8;
const BAR_H: f32 = 5.0;

/// One AI Usage item's state: the last report, when it last ran, the run in flight. Lives in its
/// instance, so a teardown drops it (and `Drop` tells a running command to stop, P15).
pub(super) struct UsageSlot {
    report: Option<UsageReport>,
    custom: bool,
    started: Option<Instant>,
    rx: Option<mpsc::Receiver<UsageReport>>,
    cancel: Arc<AtomicBool>,
}

impl Drop for UsageSlot {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The key of an item in its instance's usage map: the item's command and interval.
fn usage_key(command: &str, interval: f64) -> String {
    format!("{interval}:{}", command.trim())
}

/// A segment of the Mode Switcher.
struct Seg {
    config: String,
    label: String,
    active: bool,
    busy: bool,
    rect: Rect,
}

enum View {
    Name(String),
    Switcher(Vec<Seg>),
    Reload,
    Spacer,
    Usage(String),
    /// The git status text (cleaned) and the component's icon.
    Git(String),
    /// The changed-file pulldown's label.
    Files { label: String, enabled: bool },
    /// Back, the "X/Y" label, Forward and Reload, each with its rectangle.
    Nav { back: (bool, Rect), label: Option<(String, Rect)>, forward: (bool, Rect), reload: Rect },
    /// An auto toggle: on, and whether it can be turned on (auto-request needs one review session).
    Toggle { which: Toggle, on: bool, enabled: bool },
    /// The base field: its text (or the edit's), and the chevron's rectangle.
    Base { text: String, editing: bool, chevron: Rect },
}

struct Laid {
    rect: Rect,
    view: View,
}

/// What a press on the strip does.
enum Click {
    Peer(String),
    Reload,
    Usage(String),
    Files(Rect),
    Toggle(Toggle),
    Nav(Nav),
    BaseField,
    BaseChevron(Rect),
    Nothing,
}

/// The strip of one shown member: its items and where they come from.
struct Plan {
    items: Vec<ToolbarItem>,
    name: String,
}

/// The shortcuts a toolbar item answers to (Reload, and Navigation's three).
fn item_shortcuts(item: &ToolbarItem, browser: bool) -> Vec<&crate::workgroup::Shortcut> {
    match item {
        // A browser member's reload is the page's own (13x2 seam S3): no Reload key is taken from it.
        ToolbarItem::Reload(Some(_)) if browser => Vec::new(),
        ToolbarItem::Reload(Some(s)) => vec![s],
        ToolbarItem::Navigation(n) if browser => [&n.back, &n.forward].into_iter().flatten().collect(),
        ToolbarItem::Navigation(n) => [&n.back, &n.forward, &n.reload].into_iter().flatten().collect(),
        _ => Vec::new(),
    }
}

fn rgb_mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round().clamp(0.0, 255.0) as u8;
    Rgb { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b) }
}

const ZERO: Rect = Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 };

fn width_of(t: &str, cw: f32) -> f32 {
    unicode_width::UnicodeWidthStr::width(t) as f32 * cw
}

/// A 1 point outline inside `r`.
fn outline(frame: &mut scene::Frame, r: Rect, color: Rgb, s: f32) {
    use scene::Quad;
    let t = s.max(1.0);
    for q in [Rect { h: t, ..r }, Rect { y: r.y + r.h - t, h: t, ..r }, Rect { w: t, ..r }, Rect { x: r.x + r.w - t, w: t, ..r }] {
        frame.quads.push(Quad { rect: q, color: scene::rgba(color, 1.0) });
    }
}

impl App {
    /// Whether the pane wears a session toolbar: a shown member of a workgroup. A tmux pane is never a
    /// member (P6), and a hidden peer is in no layout, so none asks.
    pub(super) fn workgroup_toolbar_member(&self, id: PaneId) -> bool {
        self.panes.get(&id).is_some_and(|p| p.workgroup.is_some() && p.tmux_io().is_none()) && self.workgroup_of(id).is_some()
    }

    /// The items and name of `pane`'s strip. The member's own items with the Name injected (`WTB`).
    fn toolbar_plan(&self, pane: PaneId) -> Option<Plan> {
        let inst = self.workgroup_of(pane)?;
        let w = &inst.workgroup;
        let config = inst.config_of(pane).map(str::to_string).or_else(|| inst.non_peers.iter().find(|(_, p)| *p == pane).map(|(c, _)| c.clone()))?;
        let cfg = w.session(&config)?;
        let items = wt::items_for(cfg);
        let browser = self.is_browser_pane(pane);
        // The page's own reload runs in a browser member (13x2 seam S3), so the item is not offered.
        let items = items.into_iter().filter(|i| Self::drawn(i) && !(browser && matches!(i, ToolbarItem::Reload(_)))).collect();
        Some(Plan { items, name: wt::shown_text(&cfg.display_name) })
    }

    /// The items this task draws; the rest are Tasks 6 and 7.
    fn drawn(item: &ToolbarItem) -> bool {
        matches!(item, ToolbarItem::Name | ToolbarItem::ModeSwitcher | ToolbarItem::Reload(_) | ToolbarItem::Spacer { .. } | ToolbarItem::Usage { .. } | ToolbarItem::GitStatus | ToolbarItem::ChangedFileSelector | ToolbarItem::Navigation(_) | ToolbarItem::GitBaseSelector | ToolbarItem::AutoSendClippingsWhenIdle | ToolbarItem::AutoRequestReviewWhenIdle)
    }

    /// Whether member `config` of instance `inst` is busy (an agent working): the reported state.
    fn member_busy(&self, pane: Option<PaneId>) -> bool {
        pane.and_then(|p| self.panes.get(&p)).is_some_and(|p| crate::workgroup_peers::busy(session_state::reported_state(&p.own().tab_status)))
    }

    /// The git status text of an instance: the poller's first variant, cleaned (P19).
    fn git_status_text(&self, inst: &super::workgroup_app::Instance) -> String {
        let state = inst.git.as_ref().and_then(|g| g.poller.state.as_ref());
        wt::shown_text(crate::git_state::git_variants(state, None, state.is_some()).first().map_or("", String::as_str))
    }

    /// The changed-file pulldown's label and whether it can open (`STV:421-470`).
    fn files_label(&self, inst: &super::workgroup_app::Instance, pane: PaneId) -> (String, bool) {
        let sel = inst.config_of(pane).map(str::to_string).or_else(|| inst.non_peers.iter().find(|(_, p)| *p == pane).map(|(c, _)| c.clone())).and_then(|c| inst.git.as_ref()?.selectors.get(&c));
        let Some(sel) = sel else { return (wgit::all_files_title(0), false) };
        let rows = sel.rows();
        let label = match sel.picked() {
            Some(p) => rows.iter().find_map(|r| match r {
                wgit::Row::File { letter, text, path } if path == p => Some(format!("{letter}  {text}")),
                _ => None,
            }),
            None => None,
        };
        let label = label.or_else(|| rows.iter().find_map(|r| if let wgit::Row::AllFiles(t) = r { Some(t.clone()) } else { None })).unwrap_or_default();
        (label, sel.count() > 0)
    }

    /// Back and Forward enabled, and the "X/Y" label of a member's navigation (`WNT:83-90`).
    fn nav_state(&self, inst: &super::workgroup_app::Instance, pane: PaneId) -> (bool, bool, Option<String>) {
        let sel = inst.config_of(pane).map(str::to_string).or_else(|| inst.non_peers.iter().find(|(_, p)| *p == pane).map(|(c, _)| c.clone())).and_then(|c| inst.git.as_ref()?.selectors.get(&c));
        sel.map_or((false, false, None), |s| (s.can_back(), s.can_forward(), wgit::nav_label(s.position(), s.count())))
    }

    /// The strips of the active tab: (pane, strip rectangle, the pane's full rectangle).
    fn toolbar_strips(&self) -> Vec<(PaneId, Rect)> {
        let Some(tab) = self.tabs.get(self.active) else { return Vec::new() };
        visible_rects(&tab.layout, tab.zoomed, self.pane_area_for(tab), self.gap()).into_iter().filter_map(|(id, r)| self.pane_chrome(tab, id, r).toolbar.map(|t| (id, t))).collect()
    }

    /// Each item's rectangle across `strip`, in pixels; what runs past the right edge is clipped.
    fn toolbar_laid(&self, pane: PaneId, strip: Rect, cell: (f32, f32)) -> Vec<Laid> {
        let (Some(plan), Some(inst)) = (self.toolbar_plan(pane), self.workgroup_of(pane)) else { return Vec::new() };
        let s = self.scale().max(0.01);
        let cw = cell.0 / s;
        let mut widths = Vec::new();
        let mut views: Vec<View> = Vec::new();
        for item in &plan.items {
            let (w, view) = match item {
                ToolbarItem::Name => ((width_of(&plan.name, cw) + 8.0, None), View::Name(plan.name.clone())),
                ToolbarItem::Reload(_) => ((BUTTON_W, None), View::Reload),
                ToolbarItem::Spacer { min_width, max_width } => ((0.0, Some((*min_width as f32, (*max_width as f32).max(*min_width as f32)))), View::Spacer),
                ToolbarItem::Usage { command, interval_seconds, .. } => ((0.0, Some((100.0, 150.0))), View::Usage(usage_key(command, *interval_seconds))),
                ToolbarItem::ModeSwitcher => {
                    let members = peers::members(&inst.workgroup);
                    let count = members.len();
                    let segs: Vec<Seg> = members
                        .iter()
                        .enumerate()
                        .map(|(i, (config, label, shortcut))| {
                            let text = peers::segment_label(&wt::shown_text(label), shortcut.as_ref().and_then(peers::custom_label).as_deref(), i + 1, count);
                            let busy = self.member_busy(inst.pane_of(config));
                            Seg { config: config.clone(), label: text, active: *config == inst.active, busy, rect: Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 } }
                        })
                        .collect();
                    let w: f32 = segs.iter().map(|g| width_of(&g.label, cw) + 2.0 * SEG_PAD + if g.busy { cw + 4.0 } else { 0.0 }).sum();
                    ((w, None), View::Switcher(segs))
                }
                ToolbarItem::GitStatus => {
                    let text = self.git_status_text(inst);
                    ((width_of(&text, cw) + cw + 4.0, None), View::Git(text))
                }
                ToolbarItem::ChangedFileSelector => {
                    let (label, enabled) = self.files_label(inst, pane);
                    ((width_of(&label, cw) + 28.0, None), View::Files { label, enabled })
                }
                ToolbarItem::Navigation(_) => {
                    let (back, fwd, label) = self.nav_state(inst, pane);
                    let lw = label.as_deref().map_or(0.0, |l| width_of(l, cw) + 8.0);
                    ((3.0 * BUTTON_W + lw, None), View::Nav { back: (back, ZERO), label: label.map(|l| (l, ZERO)), forward: (fwd, ZERO), reload: ZERO })
                }
                ToolbarItem::AutoSendClippingsWhenIdle => ((BUTTON_W, None), View::Toggle { which: Toggle::AutoSend, on: self.toggle_on(pane, Toggle::AutoSend), enabled: true }),
                ToolbarItem::AutoRequestReviewWhenIdle => ((BUTTON_W, None), View::Toggle { which: Toggle::AutoRequest, on: self.toggle_on(pane, Toggle::AutoRequest), enabled: self.auto_request_enabled(pane) }),
                ToolbarItem::GitBaseSelector => {
                    let editing = self.workgroups.base_edit.as_ref().is_some_and(|e| e.pane == pane);
                    let text = if editing { self.workgroups.base_edit.as_ref().map(|e| e.edit.text.clone()).unwrap_or_default() } else { wt::shown_text(&inst.git_base) };
                    (((width_of(&text, cw) + 48.0).clamp(80.0, 400.0), None), View::Base { text, editing, chevron: ZERO })
                }
            };
            widths.push(w);
            views.push(view);
        }
        let placed = wt::layout(&widths, strip.w / s);
        let y = strip.y + 2.0 * s;
        let h = (strip.h - 4.0 * s - s).max(0.0);
        placed
            .into_iter()
            .zip(views)
            .filter_map(|(p, mut view)| {
                let vis = wt::visible_width(&p, strip.w / s)?;
                let rect = Rect { x: strip.x + p.x * s, y, w: vis * s, h };
                if let View::Switcher(segs) = &mut view {
                    let mut x = rect.x;
                    for g in segs.iter_mut() {
                        let w = (width_of(&g.label, cw) + 2.0 * SEG_PAD + if g.busy { cw + 4.0 } else { 0.0 }) * s;
                        g.rect = Rect { x, y: rect.y, w: w.min((rect.x + rect.w - x).max(0.0)), h: rect.h };
                        x += w;
                    }
                }
                match &mut view {
                    View::Nav { back, label, forward, reload } => {
                        let bw = BUTTON_W * s;
                        let mut x = rect.x;
                        let mut part = |w: f32| {
                            let r = Rect { x, w: w.min((rect.x + rect.w - x).max(0.0)), ..rect };
                            x += w;
                            r
                        };
                        back.1 = part(bw);
                        if let Some((l, r)) = label {
                            *r = part((width_of(l, cw) + 8.0) * s);
                        }
                        forward.1 = part(bw);
                        *reload = part(bw);
                    }
                    View::Base { chevron, .. } => {
                        let w = (20.0 * s).min(rect.w);
                        *chevron = Rect { x: rect.x + rect.w - w, w, ..rect };
                    }
                    _ => {}
                }
                Some(Laid { rect, view })
            })
            .collect()
    }

    /// Draws the active tab's session toolbars: a background, a 1 point bottom divider, the items
    /// with a divider after each but the last (`STV:200-250`). Text a session or a program supplies
    /// was cleaned and cut when the plan was made; quads first, rows over them.
    pub(super) fn draw_workgroup_toolbars(&mut self, frame: &mut scene::Frame, cell: (f32, f32)) {
        use scene::{Quad, Span, TextRow};
        let strips = self.toolbar_strips();
        if strips.is_empty() {
            return;
        }
        let s = self.scale();
        let (cw, ch) = cell;
        let span = |text: String, color: Rgb, icon: bool| Span { text, color, bold: false, italic: false, alt_font: false, icon, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
        let row = |x: f32, y: f32, spans: Vec<Span>, clip: Rect, scale: f32| TextRow { x, y, spans, clip, scale, face: crate::render::DEFAULT_FACE };
        for (pane, strip) in strips {
            let Some(p) = self.panes.get(&pane) else { continue };
            let (bg, fg) = (p.screen.palette.bg, p.screen.palette.fg);
            let (bar_bg, track, dim, line, selected) = (rgb_mix(bg, fg, 0.06), rgb_mix(bg, fg, 0.2), rgb_mix(bg, fg, 0.5), rgb_mix(bg, fg, 0.3), p.screen.palette.selection);
            frame.quads.push(Quad { rect: strip, color: scene::rgba(bar_bg, 1.0) });
            frame.quads.push(Quad { rect: Rect { y: strip.y + strip.h - s, h: s, ..strip }, color: scene::rgba(line, 1.0) });
            let laid = self.toolbar_laid(pane, strip, cell);
            let centered_y = |r: Rect, h: f32| r.y + (r.h - h) / 2.0;
            let usage = self.workgroup_of(pane).map(|i| &i.usage);
            for (n, l) in laid.iter().enumerate() {
                if n + 1 < laid.len() {
                    let x = l.rect.x + l.rect.w + wt::DIVIDER_PAD * s;
                    let inset = wt::DIVIDER_V_INSET * s;
                    if x < strip.x + strip.w - wt::INSET * s {
                        frame.quads.push(Quad { rect: Rect { x, y: l.rect.y + inset, w: wt::DIVIDER_W * s, h: (l.rect.h - 2.0 * inset).max(0.0) }, color: scene::rgba(line, 1.0) });
                    }
                }
                let r = l.rect;
                match &l.view {
                    View::Name(text) => frame.rows.push(row(r.x + 4.0 * s, centered_y(r, ch), vec![span(text.clone(), fg, false)], r, 1.0)),
                    View::Spacer => {}
                    View::Reload => frame.rows.push(row(r.x + (r.w - cw) / 2.0, centered_y(r, ch), vec![span(GLYPH_RELOAD.to_string(), fg, true)], r, 1.0)),
                    View::Switcher(segs) => {
                        frame.quads.push(Quad { rect: r, color: scene::rgba(rgb_mix(bg, fg, 0.1), 1.0) });
                        for g in segs {
                            if g.active {
                                frame.quads.push(Quad { rect: Rect { x: g.rect.x + s, y: g.rect.y + s, w: (g.rect.w - 2.0 * s).max(0.0), h: (g.rect.h - 2.0 * s).max(0.0) }, color: scene::rgba(selected, 1.0) });
                            }
                            let mut x = g.rect.x + SEG_PAD * s;
                            if g.busy {
                                // The busy member's spinner box (`WPS`), a glyph at the left.
                                frame.rows.push(row(x, centered_y(g.rect, ch), vec![span(GLYPH_BUSY.to_string(), dim, true)], g.rect, 1.0));
                                x += cw + 4.0 * s;
                            }
                            frame.rows.push(row(x, centered_y(g.rect, ch), vec![span(g.label.clone(), if g.active { fg } else { dim }, false)], g.rect, 1.0));
                        }
                    }
                    View::Git(text) => {
                        let icon = crate::statusbar_components::icon(crate::statusbar_components::Kind::Git).unwrap_or('\u{E8AB}');
                        frame.rows.push(row(r.x, centered_y(r, ch), vec![span(icon.to_string(), dim, true), span(format!(" {text}"), fg, false)], r, 1.0));
                    }
                    View::Files { label, enabled } => {
                        outline(frame, r, line, s);
                        let color = if *enabled { fg } else { dim };
                        frame.rows.push(row(r.x + 6.0 * s, centered_y(r, ch), vec![span(label.clone(), color, false)], Rect { w: (r.w - 22.0 * s).max(0.0), ..r }, 1.0));
                        frame.rows.push(row(r.x + r.w - 16.0 * s, centered_y(r, ch), vec![span(GLYPH_CHEVRON.to_string(), color, true)], r, 1.0));
                    }
                    View::Nav { back, label, forward, reload } => {
                        let glyph = |frame: &mut scene::Frame, g: char, rect: Rect, on: bool| frame.rows.push(row(rect.x + (rect.w - cw) / 2.0, centered_y(rect, ch), vec![span(g.to_string(), if on { fg } else { dim }, true)], rect, 1.0));
                        glyph(frame, GLYPH_BACK, back.1, back.0);
                        if let Some((text, lr)) = label {
                            frame.rows.push(row(lr.x + 4.0 * s, centered_y(*lr, ch), vec![span(text.clone(), fg, false)], *lr, 1.0));
                        }
                        glyph(frame, GLYPH_FORWARD, forward.1, forward.0);
                        glyph(frame, GLYPH_RELOAD, *reload, true);
                    }
                    View::Toggle { which, on, enabled } => {
                        // On reads from the fill and the full color; off from the dim glyph; unavailable from the dimmest.
                        if *on && *enabled {
                            frame.quads.push(Quad { rect: Rect { x: r.x + s, y: r.y + s, w: (r.w - 2.0 * s).max(0.0), h: (r.h - 2.0 * s).max(0.0) }, color: scene::rgba(selected, 1.0) });
                        }
                        let color = if !*enabled { rgb_mix(bg, fg, 0.3) } else if *on { fg } else { dim };
                        let glyph = if *which == Toggle::AutoSend { GLYPH_SEND } else { GLYPH_REVIEW };
                        frame.rows.push(row(r.x + (r.w - cw) / 2.0, centered_y(r, ch), vec![span(glyph.to_string(), color, true)], r, 1.0));
                    }
                    View::Base { text, editing, chevron } => {
                        outline(frame, r, if *editing { fg } else { line }, s);
                        let field = Rect { w: (r.w - chevron.w).max(0.0), ..r };
                        let ty = centered_y(r, ch);
                        if *editing
                            && let Some(e) = self.workgroups.base_edit.as_ref() {
                                let caret_x = width_of(&text[..e.edit.caret.min(text.len())], cw) * s;
                                if let Some(sel) = e.edit.selection().filter(|sel| sel.end <= text.len()) {
                                    let (a, b) = (width_of(&text[..sel.start], cw) * s, width_of(&text[..sel.end], cw) * s);
                                    frame.quads.push(Quad { rect: Rect { x: r.x + 6.0 * s + a, y: ty, w: b - a, h: ch }, color: scene::rgba(selected, 1.0) });
                                }
                                frame.quads.push(Quad { rect: Rect { x: r.x + 6.0 * s + caret_x, y: ty, w: s.max(1.0), h: ch }, color: scene::rgba(fg, 1.0) });
                            }
                        frame.rows.push(row(r.x + 6.0 * s, ty, vec![span(text.clone(), fg, false)], field, 1.0));
                        frame.rows.push(row(chevron.x + (chevron.w - cw) / 2.0, ty, vec![span(GLYPH_CHEVRON.to_string(), dim, true)], *chevron, 1.0));
                    }
                    View::Usage(key) => {
                        let report = usage.and_then(|u| u.get(key)).and_then(|u| u.report.as_ref());
                        let small = ch * USAGE_TEXT;
                        match report {
                            None => frame.rows.push(row(r.x + 2.0 * s, centered_y(r, small), vec![span(wt::USAGE_LOADING.to_string(), dim, false)], r, USAGE_TEXT)),
                            Some(rep) if rep.bars.is_empty() => {
                                let text = wt::shown_text(rep.error.as_deref().unwrap_or(wt::USAGE_UNAVAILABLE));
                                frame.rows.push(row(r.x + 2.0 * s, centered_y(r, small), vec![span(wt::ERROR_GLYPH.to_string(), dim, true), span(format!(" {text}"), dim, false)], r, USAGE_TEXT));
                            }
                            Some(rep) => {
                                let n = rep.bars.len().min(3) as f32;
                                let gap = 2.0 * s;
                                let row_h = ((r.h - gap * (n - 1.0)) / n).max(0.0);
                                let label_w = 26.0 * s;
                                let pct_w = 34.0 * s;
                                for (i, bar) in rep.bars.iter().take(3).enumerate() {
                                    let rr = Rect { x: r.x, y: r.y + i as f32 * (row_h + gap), w: r.w, h: row_h };
                                    let text_y = rr.y + (rr.h - small) / 2.0;
                                    frame.rows.push(row(rr.x + 2.0 * s, text_y, vec![span(wt::shown_text(&wt::short_label(bar)), fg, false)], Rect { w: label_w, ..rr }, USAGE_TEXT));
                                    let bar_x = rr.x + 2.0 * s + label_w + 4.0 * s;
                                    let bar_w = (rr.w - (bar_x - rr.x) - 4.0 * s - pct_w - 2.0 * s).max(0.0);
                                    let by = rr.y + (rr.h - BAR_H * s) / 2.0;
                                    frame.quads.push(Quad { rect: Rect { x: bar_x, y: by, w: bar_w, h: BAR_H * s }, color: scene::rgba(track, 1.0) });
                                    frame.quads.push(Quad { rect: Rect { x: bar_x, y: by, w: bar_w * wt::clamped(bar) as f32, h: BAR_H * s }, color: scene::rgba(fg, 1.0) });
                                    frame.rows.push(row(rr.x + rr.w - pct_w - 2.0 * s, text_y, vec![span(wt::percent(bar), fg, false)], Rect { x: rr.x + rr.w - pct_w - 2.0 * s, w: pct_w, ..rr }, USAGE_TEXT));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// The pane whose strip is under a point.
    pub(super) fn workgroup_toolbar_at(&self, x: f32, y: f32) -> Option<PaneId> {
        self.toolbar_strips().into_iter().find(|(_, r)| r.contains(x, y)).map(|(id, _)| id)
    }

    /// What a press at a point of `pane`'s strip does.
    fn toolbar_click(&self, pane: PaneId, x: f32, y: f32) -> Click {
        let Some((_, strip)) = self.toolbar_strips().into_iter().find(|(id, _)| *id == pane) else { return Click::Nothing };
        for l in self.toolbar_laid(pane, strip, self.pane_cell(pane)) {
            if !l.rect.contains(x, y) {
                continue;
            }
            return match l.view {
                View::Switcher(segs) => segs.into_iter().find(|g| g.rect.contains(x, y)).map_or(Click::Nothing, |g| Click::Peer(g.config)),
                View::Reload => Click::Reload,
                View::Usage(key) => Click::Usage(key),
                View::Files { enabled, .. } => if enabled { Click::Files(l.rect) } else { Click::Nothing },
                View::Toggle { which, .. } => Click::Toggle(which),
                View::Nav { back, forward, reload, .. } => {
                    if back.1.contains(x, y) {
                        if back.0 { Click::Nav(Nav::Back) } else { Click::Nothing }
                    } else if forward.1.contains(x, y) {
                        if forward.0 { Click::Nav(Nav::Forward) } else { Click::Nothing }
                    } else if reload.contains(x, y) {
                        Click::Nav(Nav::Reload)
                    } else {
                        Click::Nothing
                    }
                }
                View::Base { chevron, .. } => if chevron.contains(x, y) { Click::BaseChevron(l.rect) } else { Click::BaseField },
                View::Git(_) | View::Name(_) | View::Spacer => Click::Nothing,
            };
        }
        Click::Nothing
    }

    /// A right press on a member's strip (`provideMenu`, `iTermSessionToolbarView.swift:925-942`): over the git
    /// item while it shows a plain branch name (not a repository state), a menu with Copy, which puts the branch
    /// on the clipboard. Nothing elsewhere.
    pub(super) fn workgroup_toolbar_menu(&mut self, pane: PaneId, x: f32, y: f32) {
        if !self.workgroup_toolbar_member(pane) {
            return;
        }
        let Some((_, strip)) = self.toolbar_strips().into_iter().find(|(id, _)| *id == pane) else { return };
        if !self.toolbar_laid(pane, strip, self.pane_cell(pane)).into_iter().any(|l| l.rect.contains(x, y) && matches!(l.view, View::Git(_))) {
            return;
        }
        let branch = self.workgroup_of(pane).and_then(|inst| inst.git.as_ref()?.poller.state.as_ref()).filter(|s| s.repo_state == crate::git_state::RepoState::None && !s.branch.is_empty()).map(|s| s.branch.clone());
        let Some(branch) = branch else { return };
        let mut pend = super::ctx_menu_app::CtxPending::for_pane(pane, Vec::new());
        pend.set_copy(branch);
        self.ctx_pending = Some(pend);
        self.show_context_rows(&[crate::ctx_menu::item("copytext", "Copy", true)]);
    }

    /// A left press on a member's strip. Always taken (nothing under it is the program's); the
    /// item acts on the press and only on the person's pointer, never on anything a program prints.
    pub(super) fn workgroup_toolbar_press(&mut self, pane: PaneId, x: f32, y: f32, _event_loop: &ActiveEventLoop) -> bool {
        if !self.workgroup_toolbar_member(pane) {
            return false;
        }
        let click = self.toolbar_click(pane, x, y);
        // A press anywhere but the base field ends its editing, and commits (`controlTextDidEndEditing`).
        if !matches!(click, Click::BaseField) && self.workgroups.base_edit.is_some() {
            self.base_field_blur(false);
        }
        let at = |r: Rect| super::statusbar_content::MenuAt { x: r.x, y: r.y + r.h, above: false };
        let member = self.member_config(pane);
        match click {
            Click::Files(r) => self.open_file_menu(pane, at(r)),
            Click::Toggle(which) => self.toggle_auto(pane, which),
            Click::BaseChevron(r) => self.open_recents_menu(pane, at(r)),
            Click::BaseField => {
                if self.workgroups.base_edit.as_ref().is_none_or(|e| e.pane != pane) {
                    self.base_field_focus(pane);
                }
            }
            Click::Nav(action) => {
                if let Some((instance, owner)) = member {
                    self.navigation(&instance, &owner, action);
                }
            }
            Click::Peer(config) => {
                if let Some(inst) = self.workgroup_of(pane).map(|i| i.id.clone()) {
                    self.activate_peer(&inst, &config);
                }
            }
            Click::Reload => {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    t.focused = pane;
                }
                self.workgroup_reload(pane);
            }
            Click::Usage(key) => self.usage_clicked(pane, &key),
            Click::Nothing => {}
        }
        self.refresh_route();
        self.redraw();
        true
    }

    /// The toolbar's Reload (`WPP:754-791`): a Regular member starts its program again (a running one
    /// asks first, as Restart Session does). Diff and Code Review members reload through Tasks 6 and 7.
    pub(super) fn workgroup_reload(&mut self, pane: PaneId) {
        let mode = self.workgroup_of(pane).and_then(|i| i.config_of(pane).or_else(|| i.non_peers.iter().find(|(_, p)| *p == pane).map(|(c, _)| c.as_str())).and_then(|c| i.workgroup.session(c)).map(|s| s.mode));
        if mode == Some(Mode::Diff) {
            self.diff_reload(pane);
            return;
        }
        if mode == Some(Mode::CodeReview) {
            self.review_reload(pane);
            return;
        }
        if mode != Some(Mode::Regular) {
            return;
        }
        if self.restartable(pane) {
            self.restart_via(pane, crate::session_end::Via::Menu);
        } else {
            self.banners.set("workgroup", "This session cannot be restarted.");
        }
    }

    /// The toolbar shortcut of the focused member, run after global bindings and the peers' keys and
    /// before the menu's chord (`WI:1696-1700`). True when a Reload shortcut took the key.
    pub(super) fn workgroup_toolbar_shortcut(&mut self, pane: PaneId, key: &crate::keystroke::Keystroke) -> bool {
        let Some(plan) = self.toolbar_plan(pane) else { return false };
        let browser = self.is_browser_pane(pane);
        let is = |s: &Option<crate::workgroup::Shortcut>| s.as_ref().is_some_and(|s| peers::shortcut_matches(s, key));
        let act = plan.items.iter().find_map(|i| match i {
            ToolbarItem::Reload(s) if is(s) => Some(Nav::Reload),
            ToolbarItem::Navigation(n) if is(&n.back) => Some(Nav::Back),
            ToolbarItem::Navigation(n) if is(&n.forward) => Some(Nav::Forward),
            ToolbarItem::Navigation(n) if !browser && is(&n.reload) => Some(Nav::Reload),
            _ => None,
        });
        if wt::key_owner(false, false, false, act.is_some()) != Owner::Toolbar {
            return false;
        }
        match (act, self.member_config(pane)) {
            (Some(Nav::Reload), _) | (None, _) => self.workgroup_reload(pane),
            (Some(a), Some((instance, owner))) => self.navigation(&instance, &owner, a),
            _ => {}
        }
        true
    }

    /// The forms of every Reload shortcut with Win in it, for the keyboard hook (Win+R is Windows's
    /// unless Termurion takes it first).
    pub(super) fn toolbar_route_keys(&self) -> std::collections::HashSet<String> {
        let mut keys = std::collections::HashSet::new();
        for inst in self.workgroups.values() {
            for cfg in &inst.workgroup.sessions {
                for item in &cfg.toolbar_items {
                    for s in item_shortcuts(item, false) {
                        if let Some(k) = crate::keystroke::parse(&s.serialized).filter(|k| k.flags & crate::keystroke::COMMAND != 0) {
                            keys.insert(k.legacy());
                            keys.insert(k.modified_key());
                        }
                    }
                }
            }
        }
        keys
    }

    /// The keys a browser member's key gate hands to Termurion (13x2 seam S3): the custom shortcuts of the
    /// peer group `pane` shows, then its toolbar's own, the lists `peer_shortcut` and
    /// `workgroup_toolbar_shortcut` match. Win chords are the keyboard hook's and never reach a gate.
    pub(super) fn workgroup_gate_keys(&self, pane: PaneId) -> Vec<crate::workgroup::Shortcut> {
        let mut keys: Vec<crate::workgroup::Shortcut> = Vec::new();
        // A hidden peer is in no layout: no key could act there, so its gate offers none.
        if self.pane_window_id(pane).is_none() {
            return keys;
        }
        let browser = self.is_browser_pane(pane);
        if let Some(inst) = self.peer_group_of(pane) {
            keys.extend(peers::members(&inst.workgroup).into_iter().filter_map(|(_, _, s)| s));
        }
        if let Some(plan) = self.workgroup_toolbar_member(pane).then(|| self.toolbar_plan(pane)).flatten() {
            keys.extend(plan.items.iter().flat_map(|i| item_shortcuts(i, browser)).cloned());
        }
        keys
    }

    /// A click on an AI Usage item: a failure the person can report opens the report alert, a good
    /// report shows each bar's tooltip, a quiet failure shows its summary.
    fn usage_clicked(&mut self, pane: PaneId, key: &str) {
        let Some(slot) = self.workgroup_of(pane).and_then(|i| i.usage.get(key)) else { return };
        let (report, custom) = (slot.report.clone(), slot.custom);
        let Some(r) = report else { return };
        if r.bars.is_empty() {
            let summary = wt::shown_text(r.error.as_deref().unwrap_or(wt::USAGE_UNAVAILABLE));
            if r.reportable == Some(true) {
                let text = wt::report_text(env!("CARGO_PKG_VERSION"), &r, custom);
                let body = format!("{}\n\n{}", wt::REPORT_GUIDANCE, crate::inert_files::display_text(&text, 8 * 1024, true));
                let ask = super::scripts_alerts::ask(&summary, &body, &wt::report_buttons(), None, crate::warnings::Kind::Persistent, super::scripts_alerts::Then::UsageReport { title: summary.clone(), text });
                self.ask_scripts(ask);
            } else {
                self.banners.set("workgroup", summary);
            }
            return;
        }
        let lines: Vec<String> = r.bars.iter().map(|b| wt::shown_text(&wt::usage_tooltip(&b.label, b.detail.as_deref()))).collect();
        self.banners.set("workgroup", lines.join("   "));
    }

    /// The report alert's answer: Report Bug copies the full report and opens the issue form.
    pub(super) fn usage_report_answered(&mut self, title: &str, text: &str) {
        self.set_clipboard_text(text.to_string());
        let url = wt::bug_report_url(title, text);
        self.open_url_in_browser(&url, crate::browser::Style::Tab, None);
    }

    /// Runs and collects the AI Usage commands. Each item runs at most once in flight, every
    /// `interval_seconds` (never under 5), and only while its member is shown in the active tab.
    pub(super) fn usage_tick(&mut self) {
        if self.workgroups.is_empty() {
            return;
        }
        let shown: Vec<PaneId> = self.tabs.get(self.active).map(|t| t.layout.panes()).unwrap_or_default();
        let now = Instant::now();
        let mut redraw = false;
        let mut starts: Vec<(String, String, ToolbarItem)> = Vec::new();
        for inst in self.workgroups.values_mut() {
            for slot in inst.usage.values_mut() {
                if let Some(rx) = &slot.rx {
                    match rx.try_recv() {
                        Ok(r) => {
                            slot.report = Some(r);
                            slot.rx = None;
                            redraw = true;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            slot.rx = None;
                        }
                        Err(mpsc::TryRecvError::Empty) => {}
                    }
                }
            }
            if !inst.members().iter().any(|m| shown.contains(m)) {
                continue;
            }
            for cfg in &inst.workgroup.sessions {
                for item in &cfg.toolbar_items {
                    if let ToolbarItem::Usage { command, interval_seconds, .. } = item {
                        // A command runs only when it is on the list the person answered for: the item's own,
                        // or the built-in run an item without one makes (final review minor 5).
                        let shown = if command.trim().is_empty() { crate::workgroup_run::BUILTIN_USAGE } else { command.trim() };
                        if !inst.approved.allows(shown) {
                            continue;
                        }
                        let key = usage_key(command, *interval_seconds);
                        let due = inst.usage.get(&key).is_none_or(|s| s.rx.is_none() && s.started.is_none_or(|t| now.duration_since(t) >= Duration::from_secs_f64(interval_seconds.max(wt::MIN_INTERVAL).min(86_400.0))));
                        if due && !starts.iter().any(|(i, k, _)| *i == inst.id && *k == key) {
                            starts.push((inst.id.clone(), key, item.clone()));
                        }
                    }
                }
            }
        }
        for (instance, key, item) in starts {
            let ToolbarItem::Usage { command, .. } = &item else { continue };
            let (tx, rx) = mpsc::channel();
            let cancel = Arc::new(AtomicBool::new(false));
            let custom = !command.trim().is_empty();
            let command = command.trim().to_string();
            let flag = cancel.clone();
            std::thread::spawn(move || {
                let report = run_usage(&command, &flag);
                let _ = tx.send(report);
            });
            if let Some(inst) = self.workgroups.get_mut(&instance) {
                let slot = inst.usage.entry(key).or_insert(UsageSlot { report: None, custom, started: None, rx: None, cancel: cancel.clone() });
                slot.cancel.store(true, Ordering::Relaxed);
                (slot.started, slot.rx, slot.cancel, slot.custom) = (Some(now), Some(rx), cancel, custom);
            }
            redraw = true;
        }
        if redraw {
            self.redraw();
        }
    }
}

/// Runs the usage command and decodes what it printed: the built-in `claude -p /usage` (only a
/// `claude.exe` on the path, 12f ruling 12f-5) parsed here, or the item's own command through
/// cmd.exe, as written, printing the JSON contract. Hidden, no stdin, `ANTHROPIC_API_KEY`
/// removed, the output capped, the run stopped at `USAGE_TIMEOUT` or when `cancel` is set (P11).
fn run_usage(command: &str, cancel: &AtomicBool) -> UsageReport {
    use std::io::Read;
    use std::process::Stdio;
    let custom = !command.is_empty();
    let mut cmd = if custom {
        let mut c = std::process::Command::new("cmd.exe");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.raw_arg(semantic::cmd_args(command));
        }
        c
    } else {
        let path = std::env::var_os("PATH");
        let Some(program) = semantic::resolve_on_path(std::path::Path::new("claude.exe"), path.as_deref()) else { return wt::not_found() };
        let mut c = std::process::Command::new(program);
        c.args(["-p", "/usage"]);
        c
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.env_remove("ANTHROPIC_API_KEY").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // Never the current folder: the temp folder, judged local first.
    if let Some(dir) = crate::localfs::LocalPath::judge(std::env::temp_dir()) {
        crate::localfs::set_cwd(&mut cmd, &dir);
    }
    let Ok(mut child) = cmd.spawn() else { return wt::not_found() };
    let read = |mut r: Box<dyn Read + Send>, cap: usize| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = (&mut r).take(cap as u64).read_to_end(&mut out);
            // Drain what is over the cap so the child never blocks on a full pipe.
            let _ = std::io::copy(&mut r, &mut std::io::sink());
            out
        })
    };
    let out = child.stdout.take().map(|s| read(Box::new(s), USAGE_OUTPUT_CAP));
    let err = child.stderr.take().map(|s| read(Box::new(s), STDERR_CAP));
    let start = Instant::now();
    let finished = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if cancel.load(Ordering::Relaxed) || start.elapsed() >= USAGE_TIMEOUT => break false,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break false,
        }
    };
    if !finished {
        let _ = child.kill();
        let _ = child.wait();
    }
    let stdout = out.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = err.and_then(|h| h.join().ok()).unwrap_or_default();
    if !finished && stdout.is_empty() {
        return UsageReport { bars: Vec::new(), error: Some(wt::USAGE_UNAVAILABLE.to_string()), diagnostic: None, reportable: Some(false) };
    }
    if custom {
        return wt::decode_usage(&stdout);
    }
    let text = String::from_utf8_lossy(&stdout).into_owned();
    let report = wt::parse_claude_usage(&text);
    if report.bars.is_empty() && !stderr.is_empty() {
        return wt::parse_claude_usage(&format!("{text}\n--- stderr ---\n{}", String::from_utf8_lossy(&stderr)));
    }
    report
}

#[cfg(test)]
mod tests {

    /// A restart of a member starts its own command with its instance's environment, never the profile's
    /// shell (`WPP:789-791`): `restart_confirmed` takes both from `workgroup_launch`.
    #[test]
    fn a_member_restarts_with_its_own_command() {
        let m = include_str!("mod.rs");
        let f = &m[m.find("    fn restart_launching(&mut self").unwrap()..m.find("    fn restart_confirmed(&mut self").unwrap()];
        assert!(f.contains("self.workgroup_launch(id)") && f.contains("self.start_command_override = command.clone();") && f.contains("self.start_extra_env = env.clone();"), "{f}");
        let w = include_str!("workgroup_app.rs");
        let launch = &w[w.find("pub(super) fn workgroup_launch").unwrap()..];
        assert!(launch.contains("self.plan_command(") && launch.contains("ITERM_WORKGROUP_ID={instance}"));
    }
    const THIS: &str = include_str!("workgroup_toolbar_ui.rs");
    const MOD: &str = include_str!("mod.rs");

    /// `WI:1696-1700`: a key bound globally runs the global action even when a toolbar shortcut
    /// matches; a toolbar match with no global binding runs before the main menu's chord.
    #[test]
    fn shortcut_dispatch_follows_iterm2() {
        let global = MOD.find("Route::Binding(b) => self.run_key_action(&b, fallback, chord, event_loop),").expect("global arm");
        let terminal = MOD.find("Route::Terminal => {").expect("terminal arm");
        let peer = MOD.find("if self.peer_key(&press.stroke, chord) {").expect("peer keys");
        let toolbar = MOD.find("self.workgroup_toolbar_shortcut(").expect("toolbar shortcut");
        let menu = MOD.find("keyroute::terminal_chord(&shortcuts, c)").expect("menu chord");
        assert!(global < terminal && terminal < peer && peer < toolbar && toolbar < menu, "global, peer, toolbar, then the menu");
        
        assert!(MOD.contains("(snapshot.toolbar_keys) = self.toolbar_route_keys();") || MOD.contains("self.toolbar_route_keys()"), "the keyboard hook takes the toolbar's chords");
    }

    /// Nothing a program prints can press a toolbar item: items act on the person's pointer and
    /// keys only, and no item writes into a session.
    #[test]
    fn no_item_is_triggered_by_output() {
        let prod = &THIS[..THIS.find("#[cfg(test)]").unwrap()];
        for forbidden in ["program_write", "cmdinfo_write", "paste_run", "send_text", "interp::"] {
            assert!(!prod.contains(forbidden), "{forbidden} in the toolbar");
        }
        assert!(prod.contains("pub(super) fn workgroup_toolbar_press"));
        assert!(prod.contains("shown_text("), "texts a program supplies are cleaned and capped");
    }

    /// The usage command: hidden, one in flight, timed out and capped, the key removed.
    #[test]
    fn usage_runner_is_bounded() {
        let prod = &THIS[..THIS.find("#[cfg(test)]").unwrap()];
        assert!(prod.contains("CREATE_NO_WINDOW"));
        assert!(prod.contains("env_remove(\"ANTHROPIC_API_KEY\")"));
        assert!(prod.contains("USAGE_OUTPUT_CAP") && prod.contains("USAGE_TIMEOUT"));
        assert!(prod.contains("semantic::cmd_args"), "a custom command runs through cmd.exe as written");
        assert!(prod.contains("resolve_on_path"), "the built-in finds claude.exe only");
    }

    /// The 13x2 seams the merge checks (preflight S2-S4): the press order over the two strips, a
    /// browser member's key gate offering the member's keys through the one browser owner order, and
    /// one move refusal shared by the terminal and the page's context menu.
    #[test]
    fn the_13x2_seams_hold() {
        // S2: the toolbar strip takes a press before the browser toolbar, and both come from pane_chrome.
        let press = MOD.find("self.workgroup_toolbar_press(pane, x, y, event_loop)").expect("toolbar press");
        assert!(press < MOD.find("self.browser_bar_at(x, y)").expect("browser bar press"));
        // S3: the gate reads the member's keys and the shared browser_keys::key_owner places them.
        let input = include_str!("browser_input_app.rs");
        assert!(input.contains("workgroup_keys: self.workgroup_gate_keys(pane)"));
        assert!(input.contains("Some(Owner::Workgroup) if taken") && input.contains("a.peer_shortcut(pane, &stroke) || a.workgroup_toolbar_shortcut(pane, &stroke)"));
        let keys = include_str!("../browser_keys.rs");
        assert!(keys.contains("key_owner(bar, address, workgroup, copy, mapped)"));
        // S4, narrowed to iTerm2 (merge review I1, MovePaneController.m:71-133, :315): a move, swap or
        // break-out is refused only for Lock Layout and the session's own lock (and Termurion's recording
        // and tmux rules). A member in a layout moves like any pane; the one member guard is for a pane
        // in no layout (a hidden peer, reachable only by id), in the pick, the drop target and every
        // break-out.
        let mv = include_str!("pane_move_ui.rs");
        let refused = &mv[mv.find("pub(super) fn pane_move_refused").unwrap()..];
        assert!(refused[..refused.find('}').unwrap()].contains("self.member_refuses_move(pane)"));
        let guard = &mv[mv.find("pub(super) fn member_refuses_move").unwrap()..];
        let guard = &guard[..guard.find("\n    }\n").unwrap()];
        assert!(guard.contains("self.pane_window_id(pane).is_none()"), "only a member in no layout: {guard}");
        let win = include_str!("windows_ui.rs");
        assert_eq!(win.matches("self.member_refuses_move(pane)").count(), 4, "to window, to tab, to a new tab, to a window at a point");
        // The peer swap follows the shown member wherever it moved (PTYTab.m:5909-5940): the slot is
        // looked up in the window that holds it now, and Exit puts the leader's tab group back there.
        let wg = include_str!("workgroup_app.rs");
        let swap = &wg[wg.find("    fn swap_slot(").unwrap()..];
        let swap = &swap[..swap.find("\n    }\n").unwrap()];
        assert!(swap.contains("if !self.enter_pane_window(from)") && swap.contains("self.tabs.iter().position(|t| t.layout.panes().contains(&from))"), "{swap}");
        let activate = &wg[wg.find("pub(super) fn activate_peer(").unwrap()..];
        assert!(activate[..activate.find("\n    }\n").unwrap()].contains("self.swap_slot("));
        let teardown = &wg[wg.find("    fn teardown_workgroup(").unwrap()..];
        let teardown = &teardown[..teardown.find("\n    }\n").unwrap()];
        let group = &teardown[teardown.find("inst.group_before.take()").unwrap()..];
        assert!(group.contains("self.enter_pane_window(inst.leader)"), "a moved member's group is found in its own window: {group}");
        let ctx = include_str!("browser_ctx_app.rs");
        assert!(ctx.contains("a.start_pane_move(pane, false, event_loop)") && ctx.contains("a.move_session_to_tab()") && ctx.contains("a.move_session_to_window()"), "the page menu takes the terminal's paths");
    }

    /// Merge review minors 1 and 2: a browser member's gate offers no Reload key (the page's own reload
    /// runs there, as the Reload item is left out), Navigation's reload included; and a hidden peer's
    /// gate offers nothing (it is in no layout, so no key could act).
    #[test]
    fn the_gate_offers_only_keys_that_act() {
        let production = include_str!("workgroup_toolbar_ui.rs").split("#[cfg(test)]").next().unwrap();
        let gate = &production[production.find("pub(super) fn workgroup_gate_keys").unwrap()..];
        let gate = &gate[..gate.find("\n    }\n").unwrap()];
        assert!(gate.contains("if self.pane_window_id(pane).is_none()"), "{gate}");
        assert!(gate.contains("item_shortcuts(i, browser)"), "{gate}");
        let short = &production[production.find("pub(super) fn workgroup_toolbar_shortcut").unwrap()..];
        assert!(short.contains("ToolbarItem::Navigation(n) if !browser && is(&n.reload)"));
        let s = |k: &str| Some(crate::workgroup::Shortcut { serialized: k.into() });
        use crate::workgroup::ToolbarItem;
        let nav = ToolbarItem::Navigation(crate::workgroup::NavigationShortcuts { back: s("a"), forward: s("b"), reload: s("c") });
        assert_eq!(super::item_shortcuts(&nav, false).len(), 3);
        assert_eq!(super::item_shortcuts(&nav, true).len(), 2, "no reload key from a page");
        assert!(super::item_shortcuts(&ToolbarItem::Reload(s("r")), true).is_empty());
    }

    /// 13x2 seam S7 (merge review minor 3): broadcast is built from the tabs' layouts, so a hidden peer
    /// (in no layout) never sends or receives, and after a swap the shown peer is the one that does.
    #[test]
    fn broadcast_skips_hidden_peers() {
        use crate::broadcast::{Broadcast, TabView, View};
        assert!(include_str!("broadcast_ui.rs").contains("panes: t.layout.panes()"), "the view is the layouts");
        let (leader, split, hidden) = (1, 2, 3);
        let mut layout = crate::layout::Layout::new(leader);
        layout.split(leader, crate::layout::Axis::Row, split);
        let view = |l: &crate::layout::Layout| View { tabs: vec![TabView { broadcasting: false, panes: l.panes() }], current_tab: 0, current: split, exited: Default::default() };
        let mut v = view(&layout);
        let mut b = Broadcast::default();
        b.current_tab(&mut v, &mut |_| true);
        assert!(!b.receives(&v, hidden) && !b.sends(&v, hidden));
        assert_eq!(b.targets(&v, split), vec![leader, split]);
        assert!(!b.targets(&v, leader).contains(&hidden));
    }

    #[test]
    fn broadcast_into_a_member_follows_the_visible_peer() {
        use crate::broadcast::{Broadcast, TabView, View};
        let (leader, split, peer) = (1, 2, 3);
        let mut layout = crate::layout::Layout::new(leader);
        layout.split(leader, crate::layout::Axis::Row, split);
        assert!(layout.replace(leader, peer), "the swap puts the peer in the leader's slot");
        let mut v = View { tabs: vec![TabView { broadcasting: false, panes: layout.panes() }], current_tab: 0, current: split, exited: Default::default() };
        let mut b = Broadcast::default();
        b.current_tab(&mut v, &mut |_| true);
        assert_eq!(b.targets(&v, split), vec![split, peer]);
        assert!(!b.receives(&v, leader), "the buried leader is out of the domain");
    }

}
