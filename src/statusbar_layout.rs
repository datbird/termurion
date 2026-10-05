//! iTerm2's status bar layout algorithms: how a row of component sizes becomes a row of
//! x/width placements, plus the separator and background-band geometry drawn between and
//! behind them. Pure geometry: plain item sizes in, placed positions out. No renderer, no
//! knob parsing (`statusbar::Component` already resolves a knob against its component's
//! own min/max/spring before an `Item` is built; that is milestone 9h's job, in Task 3).
//!
//! Citations use the abbreviations from `.superpowers/research/m9-facts.md` section 1.2:
//! `LA` = `iTermStatusBarLayoutAlgorithm.m`, `BLA` = `iTermStatusBarBaseLayoutAlgorithm.m`,
//! `SLA` = `iTermStatusBarStableLayoutAlgorithm.m`, `TLA` =
//! `iTermStatusBarTightlyPackedLayoutAlgorithm.m`, `CV` = `iTermStatusBarContainerView.m`,
//! `VC` = `iTermStatusBarViewController.m`, `SV` = `iTermStatusBarView.m`.

use crate::layout::Rect;
use crate::statusbar::Algorithm;
use std::cmp::Ordering;
use std::collections::HashMap;

/// `iTermStatusBarViewControllerMargin` (`LA:17`).
const MARGIN: f32 = 10.0;
/// `iTermStatusBarViewControllerIconWidth` (`CV:22`).
const ICON_WIDTH: f32 = 17.0;

/// What kind of framework component an `Item` stands in for. Only the Spring and the
/// Fixed-size Spacer get special treatment in the algorithms (see `statusbar::SPRING_CLASS`
/// and `SPACER_CLASS`); everything else, including a class Tandem does not know yet, is
/// `Normal`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemKind {
    #[default]
    Normal,
    Spring,
    FixedSpacer,
}

/// One component's plain sizing facts, as the layout algorithms consume them. `min`, `max`
/// and `preferred` are already resolved against the component's own knobs (`BLA:261-283`):
/// this module does no knob lookups. `priority` uses `f32::INFINITY` for a preallocated
/// view (`SLA:198-199`, the temporary search/filter component; not used by milestone 9h's
/// content components).
#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub min: f32,
    pub max: f32,
    pub preferred: f32,
    pub stretch: bool,
    /// `statusBarComponentSpringConstant`: already resolved by
    /// `statusbar::Component::spring_constant` for the active algorithm.
    pub spring: f32,
    /// `statusBarComponentHasMargins` (`BC:349-351`; the Spring overrides it to `NO`).
    pub margins: bool,
    pub priority: f32,
    pub kind: ItemKind,
    /// Whether the component has an icon (`statusBarComponentIcon != nil`); only its
    /// presence matters here, adding `ICON_WIDTH` to the space it needs.
    pub icon: bool,
    /// `statusBarComponentIsEmpty`; removed before layout when `remove_empty` is set
    /// (`BLA:135-144`).
    pub empty: bool,
    /// The mandatory view (`VC:84-91`, the search component when it must stay on screen):
    /// always sorts last by priority and is never a removal candidate.
    pub mandatory: bool,
}

/// One placed item: `index` is its position in the `items` slice passed to `layout`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub index: usize,
    pub x: f32,
    pub width: f32,
}

fn min_for(it: &Item) -> f32 {
    it.min
}

/// `maximumWidthForComponent:` (`BLA:275-283`): "if they conflict, minimum takes
/// precedence over maximum."
fn max_for(it: &Item) -> f32 {
    it.max.max(it.min)
}

fn icon_extra(it: &Item) -> f32 {
    if it.icon { ICON_WIDTH } else { 0.0 }
}

/// `minimumWidthForComponent:` plus the icon width, added separately at call sites
/// (`BLA:174`, `BLA:44-48`, `BLA:195-206`). No `+10`: that only happens in
/// `minimumWidthIncludingIcon` below, which Stable alone uses.
fn min_with_icon(it: &Item) -> f32 {
    min_for(it) + icon_extra(it)
}

/// `iTermStatusBarContainerView.minimumWidthIncludingIcon` (`CV:182-192`): always adds the
/// full margin constant (10) on top of the icon-inclusive minimum, regardless of whether
/// the component actually has margins. Stable alone calls this (for its removal-fit check,
/// its fixed-spacer widths and its preallocation), which is why Stable double-counts
/// margins against `BLA`'s own margin bookkeeping (m9-facts.md 1.0 #3): port the quirk.
fn min_including_icon_cv(it: &Item) -> f32 {
    min_with_icon(it) + MARGIN
}

fn preferred_with_icon(it: &Item) -> f32 {
    it.preferred + icon_extra(it)
}

/// `updateMargins:` (`BLA:170-193`): left margin `10/2 + 1` = 6 for a component with
/// margins, else 0; right margin, scanning from the end, 10 for the last component with
/// margins, 5 for every other one with margins, 0 for one without. Recomputed fresh for
/// whatever order it is given (iTerm2 calls `updateMargins:` again each time the candidate
/// list changes), so this is a pure function of an explicit order rather than mutable state
/// carried between calls.
fn margins_for(order: &[usize], items: &[Item]) -> Vec<(f32, f32)> {
    let mut out = vec![(0.0f32, 0.0f32); order.len()];
    for (i, &idx) in order.iter().enumerate() {
        out[i].0 = if items[idx].margins { MARGIN / 2.0 + 1.0 } else { 0.0 };
    }
    let mut found = false;
    for i in (0..order.len()).rev() {
        let idx = order[i];
        out[i].1 = if items[idx].margins {
            if !found {
                found = true;
                MARGIN
            } else {
                MARGIN / 2.0
            }
        } else {
            0.0
        };
    }
    out
}

fn total_margin_width(order: &[usize], items: &[Item]) -> f32 {
    margins_for(order, items).iter().map(|(l, r)| l + r).sum()
}

/// `containerViewsSortedByPriority:` (`BLA:146-168`): ascending by priority, the mandatory
/// view always last, ties broken by original index (earlier first). `ids` elements are
/// themselves original indices into `items`, so the tie-break is just `a.cmp(&b)`.
fn sort_by_priority(ids: &[usize], items: &[Item]) -> Vec<usize> {
    let mut v = ids.to_vec();
    v.sort_by(|&a, &b| {
        let (ia, ib) = (&items[a], &items[b]);
        match (ia.mandatory, ib.mandatory) {
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => ia.priority.partial_cmp(&ib.priority).unwrap_or(Ordering::Equal).then(a.cmp(&b)),
        }
    });
    v
}

// ---------------------------------------------------------------------------------------
// Tightly Packed (`TLA`, `BLA`'s defaults)
// ---------------------------------------------------------------------------------------

/// `minimumWidthOfContainerViews:` as `BLA` defines it (`BLA:206-218`): margins recomputed
/// for whatever order is passed, plus each component's icon-inclusive minimum. No `+10`
/// (that is Stable's `CV` version, `min_including_icon_cv`).
fn min_total_width_tight(order: &[usize], items: &[Item]) -> f32 {
    let margins = margins_for(order, items);
    order.iter().enumerate().map(|(i, &idx)| margins[i].0 + margins[i].1 + min_with_icon(&items[idx])).sum()
}

/// `fittingSubsetOfContainerViewsFrom:` (`BLA:221-252`): removes the lowest-priority item
/// with a nonzero minimum (or an icon) until the fit works; a base-class adjacent-removal
/// hook always returns nil here, so Tight never removes a neighboring spring (`BLA:208-211`,
/// only Stable overrides it). Returns visible items in original order.
fn fitting_subset_tight(unhidden: &[usize], items: &[Item], width: f32) -> Vec<usize> {
    if width < MARGIN * 2.0 {
        return Vec::new();
    }
    let mut prioritized = sort_by_priority(unhidden, items);
    let mut nonzero: Vec<usize> =
        prioritized.iter().copied().filter(|&idx| min_for(&items[idx]) > 0.0 || items[idx].icon).collect();
    let mut desired = min_total_width_tight(&prioritized, items);
    while desired > width {
        if nonzero.is_empty() {
            break;
        }
        let remove = nonzero[0];
        prioritized.retain(|&i| i != remove);
        nonzero.remove(0);
        desired = min_total_width_tight(&nonzero, items);
    }
    prioritized.sort_unstable();
    prioritized
}

/// `updateDesiredWidthsForViews:` (`BLA:97-133`) with Tight's own growth step (`TLA:16-38`).
fn update_desired_widths_tight(order: &[usize], items: &[Item], width: f32) -> HashMap<usize, f32> {
    let margins = margins_for(order, items);
    let mut available = width - margins.iter().map(|(l, r)| l + r).sum::<f32>();
    let mut desired: HashMap<usize, f32> = HashMap::new();
    for &idx in order {
        let w = min_with_icon(&items[idx]);
        desired.insert(idx, w);
        available -= w;
    }
    if available < 1.0 {
        return desired;
    }
    let can_grow = |idx: usize, desired: &HashMap<usize, f32>| {
        let it = &items[idx];
        it.stretch && desired[&idx] < max_for(it) && preferred_with_icon(it).floor() > desired[&idx].floor()
    };
    let mut growable: Vec<usize> = order.iter().copied().filter(|&idx| can_grow(idx, &desired)).collect();
    while !growable.is_empty() {
        let sum_spring: f32 = growable.iter().map(|&idx| items[idx].spring).sum();
        let mut growth_total = 0.0f32;
        for &idx in &growable {
            let it = &items[idx];
            let weight = it.spring / sum_spring;
            let mut delta = (available * weight).floor();
            let max_w = max_for(it);
            let d = desired[&idx];
            if d + delta > max_w {
                delta = max_w - d;
            }
            let maximum = preferred_with_icon(it);
            let proposed = d + delta;
            let overage = (proposed - maximum).max(0.0).floor();
            delta -= overage;
            *desired.get_mut(&idx).unwrap() += delta;
            growth_total += delta;
        }
        available -= growth_total;
        if available < 1.0 {
            break;
        }
        let before = growable.len();
        growable.retain(|&idx| {
            let it = &items[idx];
            preferred_with_icon(it).floor() > desired[&idx].ceil()
        });
        if growth_total < 1.0 && growable.len() == before {
            break;
        }
    }
    desired
}

// ---------------------------------------------------------------------------------------
// Stable (`SLA`)
// ---------------------------------------------------------------------------------------

/// `minimumWidthOfContainerViews:` as `SLA` overrides it (`SLA:54-62`): the single largest
/// `minimumWidthIncludingIcon` (the `CV` version, always `+10`) times the number of
/// non-fixed-spacer views (a Spring counts here; only a Fixed-size Spacer is excluded),
/// plus every fixed spacer's own width, plus margins.
fn min_total_width_stable(order: &[usize], items: &[Item]) -> f32 {
    if order.is_empty() {
        return 0.0;
    }
    let largest = order.iter().map(|&idx| min_including_icon_cv(&items[idx])).fold(0.0f32, f32::max);
    let non_fixed_count = order.iter().filter(|&&idx| items[idx].kind != ItemKind::FixedSpacer).count() as f32;
    let fixed_total: f32 =
        order.iter().filter(|&&idx| items[idx].kind == ItemKind::FixedSpacer).map(|&idx| min_including_icon_cv(&items[idx])).sum();
    largest * non_fixed_count + fixed_total + total_margin_width(order, items)
}

fn is_spacer(it: &Item) -> bool {
    matches!(it.kind, ItemKind::Spring | ItemKind::FixedSpacer)
}

/// `bestViewToRemoveFrom:` (`SLA:100-139`): springs first (score 2), then fixed spacers
/// (score 1), then among non-spacers the lowest priority; ties by larger minimum width,
/// then larger index within the candidate list.
fn best_view_to_remove(candidates: &[usize], items: &[Item]) -> usize {
    let score = |it: &Item| match it.kind {
        ItemKind::Spring => 2,
        ItemKind::FixedSpacer => 1,
        ItemKind::Normal => 0,
    };
    *candidates
        .iter()
        .max_by(|&&a, &&b| {
            let (ia, ib) = (&items[a], &items[b]);
            let (sa, sb) = (score(ia), score(ib));
            if sa == 0 && sb == 0 {
                let by_priority = ib.priority.partial_cmp(&ia.priority).unwrap_or(Ordering::Equal);
                if by_priority != Ordering::Equal {
                    return by_priority;
                }
                let by_min = min_including_icon_cv(ia).partial_cmp(&min_including_icon_cv(ib)).unwrap_or(Ordering::Equal);
                if by_min != Ordering::Equal {
                    return by_min;
                }
                let pa = candidates.iter().position(|&x| x == a).unwrap();
                let pb = candidates.iter().position(|&x| x == b).unwrap();
                return pa.cmp(&pb);
            }
            sa.cmp(&sb)
        })
        .unwrap()
}

/// `viewToRemoveAdjacentToViewBeingRemoved:` (`SLA:148-166`): if the removed view has
/// spacers (spring or fixed spacer) on both sides, remove the one with the larger spring
/// constant; ties go to the right neighbor in the first half of the list, else the left.
fn adjacent_to_remove(removed: usize, ordered: &[usize], items: &[Item]) -> Option<usize> {
    let idx = ordered.iter().position(|&x| x == removed)?;
    if idx == 0 || idx + 1 == ordered.len() {
        return None;
    }
    let (left, right) = (ordered[idx - 1], ordered[idx + 1]);
    if !is_spacer(&items[left]) || !is_spacer(&items[right]) {
        return None;
    }
    let (ls, rs) = (items[left].spring, items[right].spring);
    if ls > rs {
        Some(left)
    } else if ls < rs {
        Some(right)
    } else if idx < ordered.len() / 2 {
        Some(right)
    } else {
        Some(left)
    }
}

/// `visibleContainerViewsAllowingEqualSpacingFromSortedViews:orderedViews:` (`SLA:70-96`).
fn visible_stable(candidates: &[usize], items: &[Item], width: f32) -> Vec<usize> {
    if width <= 0.0 {
        return Vec::new();
    }
    let sorted = sort_by_priority(candidates, items);
    let mut sorted = sorted;
    let mut ordered = candidates.to_vec();
    loop {
        if width >= min_total_width_stable(&ordered, items) {
            return ordered;
        }
        if ordered.is_empty() {
            return Vec::new();
        }
        let removal_candidates: Vec<usize> = sorted.iter().copied().filter(|&idx| !items[idx].mandatory).collect();
        if removal_candidates.is_empty() {
            return ordered;
        }
        let remove = best_view_to_remove(&removal_candidates, items);
        if let Some(adjacent) = adjacent_to_remove(remove, &ordered, items) {
            sorted.retain(|&i| i != adjacent);
            ordered.retain(|&i| i != adjacent);
        }
        sorted.retain(|&i| i != remove);
        ordered.retain(|&i| i != remove);
    }
}

/// `preallocatedWidthInViews:fromWidth:` (`SLA:195-252`): used by the temporary
/// search/filter component (priority `INFINITY`); not exercised by milestone 9h's content
/// components, ported for completeness and because a future preallocated view must not
/// silently misbehave.
fn preallocate(views_ex_fixed: &[usize], items: &[Item], total_width: f32, desired: &mut HashMap<usize, f32>) -> f32 {
    let prealloc: Vec<usize> = views_ex_fixed.iter().copied().filter(|&idx| items[idx].priority.is_infinite()).collect();
    if prealloc.is_empty() {
        return 0.0;
    }
    let single_unit = (total_width / views_ex_fixed.len() as f32).floor();
    let sum_preferred: f32 = views_ex_fixed
        .iter()
        .map(|&idx| {
            let it = &items[idx];
            max_for(it).min(single_unit).max(min_including_icon_cv(it))
        })
        .sum::<f32>()
        .round();
    if sum_preferred <= total_width {
        let mut total = 0.0f32;
        for &idx in &prealloc {
            let w = single_unit.max(min_including_icon_cv(&items[idx]));
            desired.insert(idx, w);
            total += w;
        }
        total
    } else {
        for &idx in &prealloc {
            desired.insert(idx, 0.0);
        }
        let mut available = total_width;
        let mut growable = prealloc.len() as f32;
        let mut total = 0.0f32;
        while available.round() > 0.0 && available >= growable {
            let apportionment = (available / growable).floor();
            growable = 0.0;
            for &idx in &prealloc {
                let it = &items[idx];
                let max_w = max_for(it);
                let old_w = desired[&idx];
                let new_w = (old_w + apportionment).min(max_w);
                if new_w.round() == old_w.round() {
                    continue;
                }
                desired.insert(idx, new_w);
                let growth = new_w - old_w;
                available -= growth;
                total += growth;
                if new_w.round() < max_w {
                    growable += 1.0;
                }
            }
        }
        total
    }
}

/// `distributeNonPreallocatedAvailableWidth:amongViews:changed:` (`SLA:280-319`): spring
/// constants summed over every non-fixed-spacer view (not filtered by `canStretch`, the
/// "every component gets a share" quirk, m9-facts.md 1.0 #4); a fixed spacer always gets
/// its own icon-inclusive minimum instead of a share.
fn distribute(available: f32, views: &[usize], items: &[Item], desired: &mut HashMap<usize, f32>) -> (f32, bool) {
    let ex_fixed: Vec<usize> = views.iter().copied().filter(|&idx| items[idx].kind != ItemKind::FixedSpacer).collect();
    let sum_spring: f32 = ex_fixed.iter().map(|&idx| items[idx].spring).sum();
    let apportionment = available / sum_spring;
    let mut remaining = available;
    let mut changed = false;
    for &idx in views {
        let it = &items[idx];
        let old_w = desired[&idx];
        let new_w = if it.kind == ItemKind::FixedSpacer {
            min_including_icon_cv(it)
        } else {
            let (min_size, max_size) = (min_for(it), max_for(it));
            let mut w = (old_w + apportionment * it.spring).min(min_size.max(max_size));
            if old_w == 0.0 {
                w = w.max(min_size);
            }
            w
        };
        if new_w.round() != old_w.round() {
            changed = true;
        }
        desired.insert(idx, new_w);
        remaining -= new_w - old_w;
    }
    (remaining, changed)
}

/// `updateDesiredWidthsForViews:` (`SLA:254-272`).
fn update_desired_widths_stable(order: &[usize], items: &[Item], width: f32) -> HashMap<usize, f32> {
    let total_margin = total_margin_width(order, items);
    let fixed_total: f32 =
        order.iter().filter(|&&idx| items[idx].kind == ItemKind::FixedSpacer).map(|&idx| min_including_icon_cv(&items[idx])).sum();
    let available_before_prealloc = width - total_margin - fixed_total;
    let views_ex_fixed: Vec<usize> = order.iter().copied().filter(|&idx| items[idx].kind != ItemKind::FixedSpacer).collect();

    let mut desired: HashMap<usize, f32> = HashMap::new();
    let preallocated_width = preallocate(&views_ex_fixed, items, available_before_prealloc, &mut desired);
    let mut available = available_before_prealloc - preallocated_width;

    let mut ex_prealloc: Vec<usize> = order.iter().copied().filter(|&idx| !items[idx].priority.is_infinite()).collect();
    for &idx in &ex_prealloc {
        desired.insert(idx, 0.0);
    }

    while available.round() > 0.0 && !ex_prealloc.is_empty() {
        let (new_available, changed) = distribute(available, &ex_prealloc, items, &mut desired);
        available = new_available;
        if !changed {
            break;
        }
        ex_prealloc.retain(|&idx| desired[&idx] < max_for(&items[idx]));
    }
    desired
}

// ---------------------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------------------

/// Lays out `items` in a bar `width` wide: which are visible and where. `remove_empty` is
/// the layout's advanced "remove empty components" setting (`BLA:135-144`); Stable is
/// iTerm2 value 0, the default (`statusbar::Algorithm`). Widths are ceiled to integers at
/// the end (`makeWidthsAndOriginsIntegers`, `BLA:254-259`); origins are then computed fresh
/// in original order (`updateDesiredOrigins`, `VC:232-240`), which is why a preceding
/// `floor` on an origin nobody has set yet is dead code in iTerm2 itself.
pub fn layout(items: &[Item], width: f32, algorithm: Algorithm, remove_empty: bool) -> Vec<Placed> {
    let unhidden: Vec<usize> = (0..items.len()).filter(|&i| !remove_empty || !items[i].empty).collect();
    let visible = match algorithm {
        Algorithm::TightlyPacked => fitting_subset_tight(&unhidden, items, width),
        Algorithm::Stable => visible_stable(&unhidden, items, width),
    };
    if visible.is_empty() {
        return Vec::new();
    }
    let mut desired = match algorithm {
        Algorithm::TightlyPacked => update_desired_widths_tight(&visible, items, width),
        Algorithm::Stable => update_desired_widths_stable(&visible, items, width),
    };
    for w in desired.values_mut() {
        *w = w.ceil();
    }

    let margins = margins_for(&visible, items);
    let mut x = 0.0f32;
    let mut placed = Vec::with_capacity(visible.len());
    for (i, &idx) in visible.iter().enumerate() {
        x += margins[i].0;
        let origin = x;
        let w = desired[&idx];
        x += w;
        x += margins[i].1;
        placed.push(Placed { index: idx, x: origin, width: w });
    }
    placed
}

/// `desiredSeparatorOffsets` (`VC:101-144`): one offset per visible item except the last;
/// a fixed spacer never contributes its own (the dead branch that would have added one on
/// its right, m9-facts.md 1.0 #2, never fires because `haveFoundNonSpacer` starts false and
/// only that same unreachable branch would ever set it true); consecutive duplicate offsets
/// are removed (`uniq`, `NSArray+iTerm.m:352-370`, which only compares to the previous
/// element, matching `Vec::dedup`).
pub fn separators(placed: &[Placed], items: &[Item]) -> Vec<f32> {
    if placed.len() <= 1 {
        return Vec::new();
    }
    let mut offsets: Vec<f32> = Vec::new();
    for p in &placed[..placed.len() - 1] {
        let it = &items[p.index];
        if it.kind == ItemKind::FixedSpacer {
            continue;
        }
        let margin = if it.kind == ItemKind::Spring { 0.0 } else { MARGIN / 2.0 };
        offsets.push(p.x + p.width + margin);
    }
    offsets.dedup();
    offsets
}

/// `desiredBackgroundColors` (`VC:146-154`) plus `SV`'s band drawing (`SV:53-71`): a band
/// per visible item from the previous boundary to `origin + width + margin + 0.5` (margin 0
/// for a spring, else 5); the last band runs to the bar's full `width` instead. A component
/// with no background color still advances the boundary (Task 3's concern when drawing:
/// look up the item's color by `bands`'s own index and skip an unfilled one).
pub fn bands(placed: &[Placed], items: &[Item], width: f32) -> Vec<(f32, f32, usize)> {
    let mut out = Vec::with_capacity(placed.len());
    let mut last_x = 0.0f32;
    for (i, p) in placed.iter().enumerate() {
        let it = &items[p.index];
        let margin = if it.kind == ItemKind::Spring { 0.0 } else { MARGIN / 2.0 };
        let x = if i + 1 == placed.len() { width } else { p.x + p.width + margin + 0.5 };
        out.push((last_x, x, p.index));
        last_x = x;
    }
    out
}

/// A click at `x` (points from the bar's left): the item, if any, whose own margins
/// (`updateMargins:`, `margins_for`) bound it. Unlike `bands`, which tiles the whole bar so
/// a click anywhere hits some item, this leaves empty space before the first item or past
/// the last one hitting nothing (the 9g review fix: a right-click there must offer only
/// Configure Status Bar and Disable Status Bar, `m9g-checklist.md` #17). Returns the hit
/// item's index into `items`.
pub fn hit_index(placed: &[Placed], items: &[Item], x: f32) -> Option<usize> {
    let order: Vec<usize> = placed.iter().map(|p| p.index).collect();
    let margins = margins_for(&order, items);
    placed.iter().zip(margins).find(|(p, (l, r))| x >= p.x - l && x < p.x + p.width + r).map(|(p, _)| p.index)
}

// ---------------------------------------------------------------------------------------
// Where a bar goes
// ---------------------------------------------------------------------------------------

/// `StatusBarPosition` (`iTermPreferences.h:39-42`): 0 Top, iTerm2's default, 1 Bottom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Position {
    #[default]
    Top,
    Bottom,
}

impl Position {
    pub fn from_code(code: i64) -> Position {
        if code == 1 { Position::Bottom } else { Position::Top }
    }
}

/// A bar `height` tall at the top or bottom of `area`, and what is left for the terminal
/// (`iTermLayoutCalculator.m:443-469` for the window's shared bar, which spans the tab
/// content area; `SessionView.m:2222-2260` for a pane's own bar). The same split serves
/// both: the shared bar splits the window's content area, a per-pane bar its pane's
/// rectangle. The terminal always keeps at least one pixel.
pub fn bar_rects(area: Rect, height: f32, position: Position) -> (Rect, Rect) {
    let h = height.min(area.h - 1.0).max(0.0);
    match position {
        Position::Top => (Rect { h, ..area }, Rect { y: area.y + h, h: area.h - h, ..area }),
        Position::Bottom => (Rect { y: area.y + area.h - h, h, ..area }, Rect { h: area.h - h, ..area }),
    }
}

/// A pane's rectangle less its own bar, and the bar: `bar` is the bar's height and
/// position when the pane shows one, None when it does not.
pub fn pane_split(rect: Rect, bar: Option<(f32, Position)>) -> (Rect, Option<Rect>) {
    match bar {
        Some((height, position)) => {
            let (bar, rest) = bar_rects(rect, height, position);
            (rest, Some(bar))
        }
        None => (rect, None),
    }
}

/// What a window's terminals were sized for: its content area and each pane's terminal
/// rectangle in every tab. Any bar that comes, goes or changes height or place, shared
/// or per pane, changes it, so a window compares it before each frame and lays out
/// again (resizing the ptys and telling tmux) when it differs.
#[derive(Clone, Debug, PartialEq)]
pub struct Room {
    pub area: Rect,
    pub panes: Vec<(u64, Rect)>,
    /// The width each pane (in the same order) gives up at its right edge: its scrollbar and
    /// its Adjacent timestamp column.
    pub rooms: Vec<f32>,
    /// The terminal margins in pixels (side, top/bottom): a margin edit changes the grids.
    pub margins: (f32, f32),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normal(min: f32, max: f32, preferred: f32, priority: f32) -> Item {
        Item { min, max, preferred, stretch: true, spring: 1.0, margins: true, priority, kind: ItemKind::Normal, icon: false, empty: false, mandatory: false }
    }

    fn spring(priority: f32) -> Item {
        Item { min: 0.0, max: f32::INFINITY, preferred: f32::INFINITY, stretch: true, spring: 1.0, margins: false, priority, kind: ItemKind::Spring, icon: false, empty: false, mandatory: false }
    }

    #[test]
    fn stable_layout_follows_iterm2() {
        // Three fixed-size text components (min == max == 30), priorities 5, 3, 5: too
        // narrow to fit all three, so the lowest-priority one (index 1) goes first
        // (SLA:100-139's priority tiebreak), leaving the other two at their full width.
        let items = [normal(30.0, 30.0, 30.0, 5.0), normal(30.0, 30.0, 30.0, 3.0), normal(30.0, 30.0, 30.0, 5.0)];
        let placed = layout(&items, 140.0, Algorithm::Stable, false);
        assert_eq!(placed, vec![Placed { index: 0, x: 6.0, width: 30.0 }, Placed { index: 2, x: 47.0, width: 30.0 }]);

        // A spring at equal priority to two fixed-size text components: too narrow for all
        // three, but the spring (score 2 in bestViewToRemoveFrom) is removed before either
        // text component even though its own minimum width is smaller than theirs.
        let with_spring = [spring(5.0), normal(30.0, 30.0, 30.0, 5.0), normal(30.0, 30.0, 30.0, 5.0)];
        let placed = layout(&with_spring, 130.0, Algorithm::Stable, false);
        assert_eq!(placed.len(), 2);
        assert!(placed.iter().all(|p| items_kind(&with_spring, p.index) != ItemKind::Spring));
        assert_eq!(placed[0].index, 1);
        assert_eq!(placed[1].index, 2);
    }

    fn items_kind(items: &[Item], index: usize) -> ItemKind {
        items[index].kind
    }

    #[test]
    fn tight_layout_follows_iterm2() {
        // A spring between two stretchable text components, all priority 5. The text
        // components' spring constant (1) dwarfs the spring's own Tightly Packed default
        // (0.01), so they race to their preferred width (40) first and stop; the spring
        // (preferred == infinity) absorbs whatever is left.
        let a = normal(20.0, 100.0, 40.0, 5.0);
        let mut mid = spring(5.0);
        mid.spring = 0.01;
        let items = [a, mid, normal(20.0, 100.0, 40.0, 5.0)];
        let placed = layout(&items, 200.0, Algorithm::TightlyPacked, false);
        assert_eq!(
            placed,
            vec![
                Placed { index: 0, x: 6.0, width: 40.0 },
                Placed { index: 1, x: 51.0, width: 93.0 },
                Placed { index: 2, x: 150.0, width: 40.0 },
            ]
        );
    }

    #[test]
    fn separators_and_bands_follow_iterm2() {
        let items = [
            normal(1.0, 1.0, 1.0, 5.0),
            Item { min: 5.0, max: 5.0, preferred: 5.0, stretch: false, spring: 1.0, margins: true, priority: 5.0, kind: ItemKind::FixedSpacer, icon: false, empty: false, mandatory: false },
            normal(1.0, 1.0, 1.0, 5.0),
            spring(5.0),
            normal(1.0, 1.0, 1.0, 5.0),
        ];
        let placed = vec![
            Placed { index: 0, x: 0.0, width: 40.0 },
            Placed { index: 1, x: 45.0, width: 5.0 },
            Placed { index: 2, x: 55.0, width: 30.0 },
            Placed { index: 3, x: 90.0, width: 0.0 },
            Placed { index: 4, x: 95.0, width: 20.0 },
        ];

        // One separator per boundary, none after the last component, none on the fixed
        // spacer's right (its own contribution is simply never added); the spring's offset
        // duplicates the preceding component's and is deduped.
        assert_eq!(separators(&placed, &items), vec![45.0, 90.0]);

        // A band per placed item; the last one runs to the bar's own width instead of its
        // own formula.
        assert_eq!(
            bands(&placed, &items, 120.0),
            vec![
                (0.0, 45.5, 0),
                (45.5, 55.5, 1),
                (55.5, 90.5, 2),
                (90.5, 90.5, 3),
                (90.5, 120.0, 4),
            ]
        );
    }

    #[test]
    fn hit_index_bounds_each_item_by_its_own_margins() {
        // Unlike `bands`, which tiles the whole bar so a click anywhere hits some item,
        // `hit_index` bounds each item by its own left and right margin (`updateMargins:`)
        // and nothing else: empty space before the first item or past the last one hits
        // nothing (the 9g review fix for the bar's right-click menu on empty space,
        // `m9g-checklist.md` #17).
        let items = [normal(20.0, 20.0, 20.0, 5.0), normal(20.0, 20.0, 20.0, 5.0)];
        let placed = vec![Placed { index: 0, x: 6.0, width: 20.0 }, Placed { index: 1, x: 37.0, width: 20.0 }];
        // margins_for([0, 1], items) = [(6, 5), (6, 10)]: item 0 spans 0..31, item 1 31..67.
        assert_eq!(hit_index(&placed, &items, -1.0), None, "before item 0's left margin");
        assert_eq!(hit_index(&placed, &items, 0.0), Some(0));
        assert_eq!(hit_index(&placed, &items, 30.0), Some(0));
        assert_eq!(hit_index(&placed, &items, 31.0), Some(1));
        assert_eq!(hit_index(&placed, &items, 66.0), Some(1));
        assert_eq!(hit_index(&placed, &items, 67.0), None, "past the last item's own right margin: no component, unlike bands");
        assert_eq!(hit_index(&placed, &items, 200.0), None, "empty bar space far past everything hits nothing");
    }

    #[test]
    fn bar_rects_follow_the_position() {
        // The shared bar: the window's content area (already below the tab bar and above
        // any banner) gives the bar its top or bottom edge, full width.
        let area = Rect { x: 0.0, y: 30.0, w: 800.0, h: 570.0 };
        let (bar, rest) = bar_rects(area, 31.0, Position::Top);
        assert_eq!(bar, Rect { x: 0.0, y: 30.0, w: 800.0, h: 31.0 });
        assert_eq!(rest, Rect { x: 0.0, y: 61.0, w: 800.0, h: 539.0 });
        let (bar, rest) = bar_rects(area, 31.0, Position::Bottom);
        assert_eq!(bar, Rect { x: 0.0, y: 569.0, w: 800.0, h: 31.0 });
        assert_eq!(rest, Rect { x: 0.0, y: 30.0, w: 800.0, h: 539.0 });

        // A per-pane bar: the same split inside one pane's rectangle.
        let pane = Rect { x: 402.0, y: 30.0, w: 398.0, h: 200.0 };
        let (bar, rest) = bar_rects(pane, 21.0, Position::Top);
        assert_eq!(bar, Rect { x: 402.0, y: 30.0, w: 398.0, h: 21.0 });
        assert_eq!(rest, Rect { x: 402.0, y: 51.0, w: 398.0, h: 179.0 });
        let (bar, rest) = bar_rects(pane, 21.0, Position::Bottom);
        assert_eq!(bar, Rect { x: 402.0, y: 209.0, w: 398.0, h: 21.0 });
        assert_eq!(rest, Rect { x: 402.0, y: 30.0, w: 398.0, h: 179.0 });

        // A pane shorter than the bar keeps one pixel for its terminal.
        let short = Rect { x: 0.0, y: 0.0, w: 100.0, h: 10.0 };
        let (bar, rest) = bar_rects(short, 21.0, Position::Top);
        assert_eq!((bar.h, rest.y, rest.h), (9.0, 9.0, 1.0));

        // StatusBarPosition: 0 is Top (the default), 1 Bottom, anything else Top.
        assert_eq!(Position::from_code(0), Position::Top);
        assert_eq!(Position::from_code(1), Position::Bottom);
        assert_eq!(Position::from_code(7), Position::Top);
    }
    #[test]
    fn a_margin_edit_changes_the_room() {
        let area = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        let room = |m: (f32, f32)| Room { area, panes: vec![(1, area)], rooms: vec![0.0], margins: m };
        assert_eq!(room((5.0, 2.0)), room((5.0, 2.0)));
        assert_ne!(room((5.0, 2.0)), room((6.0, 2.0)), "a window compares the room, so it must differ");
        assert_ne!(room((5.0, 2.0)), room((5.0, 3.0)));
        // The window's own room reads the margins, so every window relays out on an edit.
        assert!(include_str!("app/statusbar_ui.rs").contains("margins: self.margins()"));
    }

    #[test]
    fn a_per_pane_bar_changes_the_room() {
        // Two panes side by side; the room a window compares before each frame.
        let area = Rect { x: 0.0, y: 30.0, w: 800.0, h: 570.0 };
        let left = Rect { w: 399.0, ..area };
        let right = Rect { x: 401.0, w: 399.0, ..area };
        let room = |bars: [Option<(f32, Position)>; 2]| Room { area, panes: vec![(1, pane_split(left, bars[0]).0), (2, pane_split(right, bars[1]).0)], rooms: vec![0.0, 0.0], margins: (5.0, 2.0) };
        let none = room([None, None]);
        assert_eq!(none, room([None, None]));
        assert_eq!(none.panes[0].1, left);
        // The content area is the same in every case: only the pane rectangles move.
        let one = room([None, Some((21.0, Position::Top))]);
        assert_ne!(none, one);
        assert_eq!(one.panes[1].1, Rect { y: 51.0, h: 549.0, ..right });
        // Height, position and which pane shows a bar each change it.
        assert_ne!(one, room([None, Some((28.0, Position::Top))]));
        assert_ne!(one, room([None, Some((21.0, Position::Bottom))]));
        assert_ne!(one, room([Some((21.0, Position::Top)), None]));
        assert_eq!(room([None, Some((21.0, Position::Bottom))]).panes[1].1, Rect { h: 549.0, ..right });
        let (rest, bar) = pane_split(right, Some((21.0, Position::Bottom)));
        assert_eq!((rest.h + bar.unwrap().h, bar.unwrap().y), (right.h, right.y + right.h - 21.0));
    }
}
