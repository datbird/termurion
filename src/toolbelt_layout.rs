//! The toolbelt's split view math and panel layout: wrapper minimums, collapse and expand,
//! divider limits and dragging, proportion persistence, the panel/pane split, and the panel's
//! own layout and hit test. Pure geometry; the tool registry is `toolbelt`, the shared list
//! widget is `tool_list`, each tool's rows are in `toolbelt_tools`.

use toml::Value;

use crate::layout::Rect;
use crate::toolbelt::ToolId;

/// Wrapper title bar height (`TW:15`).
pub const TITLE_H: f32 = 19.0;
/// Margin between the title bar and the content (`TW:16`).
pub const TITLE_GAP: f32 = 4.0;
/// Left and right margin around the content (`TW:17-18`).
pub const SIDE: f32 = 4.0;
/// Margin below the content (`TW:19`).
pub const BOTTOM: f32 = 8.0;
/// Close button width and height (`TW:20`).
pub const CLOSE: f32 = 17.0;
/// Close button's left margin (`TW:22`, `kCloseButtonLeftMargin`).
pub const CLOSE_X: f32 = 2.0;
/// Margin above the title bar and close button (`TW:21`).
pub const TOP: f32 = 3.0;
/// A collapsed wrapper's height, title and close kept (`TW:23`).
pub const COLLAPSED_H: f32 = 22.0;
/// The drag handle's width, on the panel's left edge (`TBV:355-358`).
pub const HANDLE_W: f32 = 3.0;
/// A divider's thickness between two wrappers (thin style, `TBV:334`).
pub const DIVIDER: f32 = 1.0;

/// A wrapper's minimum height when expanded: the tool's own minimum plus the title bar,
/// the gap below it and the bottom margin (`TW:115-124`).
pub fn wrapper_minimum(tool: ToolId) -> f32 {
    tool.minimum_height() + TITLE_H + TITLE_GAP + BOTTOM
}

/// The tools in a toolbelt's split view, each wrapper's height (points; collapsed wrappers
/// are exactly `COLLAPSED_H`) and whether it is collapsed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Split {
    pub tools: Vec<ToolId>,
    pub heights: Vec<f32>,
    pub collapsed: Vec<bool>,
}

impl Split {
    /// Whether `tool` is in the split and its wrapper is not collapsed: only then can it have
    /// the keyboard (a collapsed wrapper shows no field and no list).
    pub fn expanded(&self, tool: ToolId) -> bool {
        self.tools.iter().position(|&t| t == tool).is_some_and(|i| !self.collapsed.get(i).copied().unwrap_or(false))
    }
}

fn mins(tools: &[ToolId]) -> Vec<f32> {
    tools.iter().map(|&t| wrapper_minimum(t)).collect()
}

/// Every wrapper's minimum right now: `COLLAPSED_H` where collapsed, else its expanded
/// minimum (`TBV`'s `minimumHeightForSplitterSubviews`, without the divider term; dividers
/// are left out of every function in this module that works in heights, not positions).
fn min_total(tools: &[ToolId], collapsed: &[bool]) -> f32 {
    tools.iter().zip(collapsed).map(|(&t, &c)| if c { COLLAPSED_H } else { wrapper_minimum(t) }).sum()
}

/// Deficits repaid proportionally from wrappers over their minimum, the last taking whatever
/// is left (`TBV:516-556`, `forceSplitterSubviewsToRespectSizeConstraints`). Dividers are not
/// part of `total`; when nothing is under its minimum the heights come back unchanged (the
/// caller is trusted to have already made them sum to `total`).
pub fn respect_minimums(heights: &[f32], mins: &[f32], total: f32) -> Vec<f32> {
    let mut slop = 0.0_f32;
    let mut deficit = 0.0_f32;
    for (h, m) in heights.iter().zip(mins) {
        let excess = h - m;
        if excess < 0.0 {
            deficit -= excess;
        } else {
            slop += excess;
        }
    }
    if deficit <= 0.0 {
        return heights.to_vec();
    }
    let fraction = (deficit / slop).min(1.0);
    let n = heights.len();
    let mut out = Vec::with_capacity(n);
    let mut y = 0.0_f32;
    for i in 0..n {
        let h = if i == n - 1 {
            total - y
        } else {
            let excess = heights[i] - mins[i];
            if excess < 0.0 { mins[i] } else { heights[i] - (excess * fraction).ceil() }
        };
        y += h;
        out.push(h);
    }
    out
}

/// A window resize (`TBV:743-821`, `resizeSubviewsWithOldSize:`): wrappers collapse bottom-up
/// while their minimums (using `COLLAPSED_H` for whatever is already collapsed) exceed
/// `new_total`; only when nothing collapsed and the panel grew do collapsed wrappers expand
/// top-down, one at a time, while the running minimum plus that wrapper's own expanded minimum
/// still fits. A collapsed wrapper is then pinned at exactly `COLLAPSED_H`; the room left over is
/// split among the non-collapsed wrappers, scaled from their old heights and passed through
/// `respect_minimums`. (Real NSSplitView instead proportions every subview, collapsed or not,
/// which can leave a collapsed wrapper taller than its title bar until the next deficit pass
/// catches it; pinning it here is a deliberate simplification so a collapsed row never has to
/// give up room it does not have. See the report.)
pub fn resize(split: &Split, old_total: f32, new_total: f32) -> Split {
    let tools = &split.tools;
    let n = tools.len();
    let mut collapsed = split.collapsed.clone();

    let mut collapsed_any = false;
    for i in (0..n).rev() {
        if !collapsed[i] && min_total(tools, &collapsed) > new_total {
            collapsed[i] = true;
            collapsed_any = true;
        }
    }
    if !collapsed_any && new_total > old_total {
        for i in 0..n {
            if collapsed[i] {
                if min_total(tools, &collapsed) + wrapper_minimum(tools[i]) <= new_total {
                    collapsed[i] = false;
                } else {
                    break;
                }
            }
        }
    }

    let collapsed_count = collapsed.iter().filter(|&&c| c).count() as f32;
    let free_total = (new_total - collapsed_count * COLLAPSED_H).max(0.0);
    let active_old: Vec<f32> = (0..n).filter(|&i| !collapsed[i]).map(|i| split.heights[i]).collect();
    let active_old_sum: f32 = active_old.iter().sum();
    let scale = if active_old_sum > 0.0 { free_total / active_old_sum } else { 0.0 };
    let scaled: Vec<f32> = active_old.iter().map(|&h| h * scale).collect();
    let active_mins: Vec<f32> = (0..n).filter(|&i| !collapsed[i]).map(|i| wrapper_minimum(tools[i])).collect();
    let active_heights = respect_minimums(&scaled, &active_mins, free_total);

    let mut heights = Vec::with_capacity(n);
    let mut ai = 0;
    for &c in &collapsed {
        if c {
            heights.push(COLLAPSED_H);
        } else {
            heights.push(active_heights[ai]);
            ai += 1;
        }
    }
    Split { tools: tools.clone(), heights, collapsed }
}

/// The tool list changed (a tool switched on or off; `TBV:256-271`, `TBV:585`). A matching list
/// (same tools, same order) returns `split` unchanged, even if `total` differs (a resize is a
/// separate call). A new tool is appended at the bottom starting at
/// `total / max(1, visible_count - 1)` (`TBV:585`, `visible_count` counting it, always added
/// expanded); a dropped tool's height is folded back into the rest. A wrapper already collapsed
/// stays pinned at `COLLAPSED_H` (the same reasoning as `resize`); the non-collapsed wrappers
/// share what is left, scaled from their current heights and passed through `respect_minimums`.
pub fn reconcile(split: &Split, tools: &[ToolId], total: f32) -> Split {
    if split.tools == tools {
        return split.clone();
    }
    let mut new_tools = Vec::new();
    let mut new_heights = Vec::new();
    let mut new_collapsed = Vec::new();
    for (i, &t) in split.tools.iter().enumerate() {
        if tools.contains(&t) {
            new_tools.push(t);
            new_heights.push(split.heights[i]);
            new_collapsed.push(split.collapsed[i]);
        }
    }
    for &t in tools {
        if !new_tools.contains(&t) {
            let visible = new_tools.len() + 1;
            let h = total / (visible.saturating_sub(1).max(1) as f32);
            new_tools.push(t);
            new_heights.push(h);
            new_collapsed.push(false);
        }
    }

    let n = new_tools.len();
    let collapsed_count = new_collapsed.iter().filter(|&&c| c).count() as f32;
    let free_total = (total - collapsed_count * COLLAPSED_H).max(0.0);
    let active_old: Vec<f32> = (0..n).filter(|&i| !new_collapsed[i]).map(|i| new_heights[i]).collect();
    let active_old_sum: f32 = active_old.iter().sum();
    let scale = if active_old_sum > 0.0 { free_total / active_old_sum } else { 0.0 };
    let scaled: Vec<f32> = active_old.iter().map(|&h| h * scale).collect();
    let active_mins: Vec<f32> = (0..n).filter(|&i| !new_collapsed[i]).map(|i| wrapper_minimum(new_tools[i])).collect();
    let active_heights = respect_minimums(&scaled, &active_mins, free_total);

    let mut heights = Vec::with_capacity(n);
    let mut ai = 0;
    for &c in &new_collapsed {
        if c {
            heights.push(COLLAPSED_H);
        } else {
            heights.push(active_heights[ai]);
            ai += 1;
        }
    }
    Split { tools: new_tools, heights, collapsed: new_collapsed }
}

/// Every wrapper's own minimum right now, `COLLAPSED_H` where collapsed (`wrapper.minimumHeight`,
/// `TW:115-120`), else its expanded minimum. Distinct from `min_total`, which sums these.
fn effective_mins(split: &Split) -> Vec<f32> {
    split.tools.iter().zip(&split.collapsed).map(|(&t, &c)| if c { COLLAPSED_H } else { wrapper_minimum(t) }).collect()
}

/// The divider bounds NSSplitView enforces while dragging (`TBV:743-770`,
/// `constrainMinCoordinate`/`constrainMaxCoordinate`): the divider's position (from the top of
/// the stack, dividers included) may not put the wrapper above it under its minimum, nor the one
/// below over what its own room allows. Both wrappers' minimums are collapse-aware, `COLLAPSED_H`
/// while collapsed (`TW:115-120`), matching `wrapper.minimumHeight`. iTerm2 counts an extra
/// divider thickness on the far side even for the last wrapper (kept, `TBV:787`); a divider limit
/// can therefore sit up to `DIVIDER` short of truly protecting its lower neighbor's minimum,
/// matching the source. When a wrapper already sits under its own minimum (only the split's last
/// wrapper can, from `respect_minimums`'s "last takes the remainder" rule), the two bounds can
/// invert; `drag_divider` never lets that produce a negative height.
pub fn divider_limits(split: &Split, index: usize) -> (f32, f32) {
    let m = effective_mins(split);
    let h = &split.heights;
    let mut min_y = 0.0_f32;
    for i in 0..=index {
        if i == index {
            min_y += m[i];
        } else {
            min_y += h[i] + DIVIDER;
        }
    }
    let mut max_y = 0.0_f32;
    for i in 0..=(index + 1) {
        if i == index + 1 {
            max_y += (h[i] - m[i]).max(0.0) + DIVIDER;
        } else {
            max_y += h[i] + DIVIDER;
        }
    }
    (min_y, max_y)
}

/// Dragging divider `index` to position `y` (clamped to `divider_limits`, the two bounds taken in
/// whichever order they come, since they can invert): the two adjacent wrappers split their
/// combined height at the new position, each clamped to `[0, combined]` so an inverted or
/// otherwise degenerate limit can never leave either wrapper negative; every other wrapper is
/// untouched.
pub fn drag_divider(split: &Split, index: usize, y: f32) -> Split {
    let (a, b) = divider_limits(split, index);
    let (lo, hi) = (a.min(b), a.max(b));
    let y = y.clamp(lo, hi);
    let prefix: f32 = split.heights[..index].iter().sum::<f32>() + (index as f32) * DIVIDER;
    let combined = split.heights[index] + split.heights[index + 1];
    let new_h0 = (y - prefix).clamp(0.0, combined);
    let new_h1 = combined - new_h0;
    let mut heights = split.heights.clone();
    heights[index] = new_h0;
    heights[index + 1] = new_h1;
    Split { tools: split.tools.clone(), heights, collapsed: split.collapsed.clone() }
}

/// The saved-proportions value for a split (`TBV:844-861`, `-proportions`): each tool's share of
/// the total height, in wrapper order. `{}` when the heights sum to nothing.
pub fn proportions_value(split: &Split) -> Value {
    let sum: f32 = split.heights.iter().sum();
    if sum <= 0.0 {
        return Value::Table(toml::Table::new());
    }
    let mut arr = Vec::with_capacity(split.tools.len());
    for (t, h) in split.tools.iter().zip(&split.heights) {
        let mut e = toml::Table::new();
        e.insert("name".into(), Value::String(t.name().to_string()));
        e.insert("heightAsFraction".into(), Value::Float((h / sum) as f64));
        arr.push(Value::Table(e));
    }
    let mut t = toml::Table::new();
    t.insert("proportions".into(), Value::Array(arr));
    Value::Table(t)
}

fn parse_entry(e: &Value) -> Option<(String, f32)> {
    let t = e.as_table()?;
    let name = t.get("name")?.as_str()?.to_string();
    let frac = match t.get("heightAsFraction")? {
        Value::Float(f) => *f as f32,
        Value::Integer(i) => *i as f32,
        _ => return None,
    };
    Some((name, frac))
}

/// Applying a saved proportions value at `total` (`TBV:863-947`, `-setProportions:`). Refused
/// (`None`) when it is not `{"proportions": [...]}`, an entry lacks a name or a number, or a
/// fraction is not greater than 0; names are checked against `split.tools` with iTerm2's `zip`,
/// so a saved list that is a shorter prefix of the current tools is accepted and only advances
/// the tools it names (`NSArray+iTerm.m:627-633`, kept). A named tool below its minimum at its
/// fraction is raised to it, the shortfall taken proportionally from the others' room above
/// their own minimums (among the named tools only); refused if that room is not enough.
pub fn apply_proportions(split: &Split, v: &Value, total: f32) -> Option<Split> {
    if total <= 0.0 {
        return None;
    }
    let arr = v.get("proportions")?.as_array()?;
    let mut parsed = Vec::with_capacity(arr.len());
    for e in arr {
        parsed.push(parse_entry(e)?);
    }
    let n = parsed.len().min(split.tools.len());
    for i in 0..n {
        if parsed[i].0 != split.tools[i].name() || parsed[i].1 <= 0.0 {
            return None;
        }
    }
    let m = mins(&split.tools[..n]);
    let mut desired: Vec<f32> = parsed[..n].iter().map(|&(_, f)| f * total).collect();
    let mut added = 0.0_f32;
    for i in 0..n {
        let proposed = desired[i];
        let accepted = proposed.max(m[i]);
        added += (accepted - proposed).max(0.0);
        desired[i] = accepted;
    }
    if added > 0.0 {
        let wiggle: Vec<f32> = desired.iter().zip(&m).map(|(&d, &mi)| (d - mi).max(0.0)).collect();
        let total_wiggle: f32 = wiggle.iter().sum();
        if total_wiggle < added {
            return None;
        }
        for i in 0..n {
            if wiggle[i] == 0.0 {
                continue;
            }
            desired[i] -= (wiggle[i] / total_wiggle) * added;
        }
    }
    let mut heights = split.heights.clone();
    heights[..n].copy_from_slice(&desired);
    Some(Split { tools: split.tools.clone(), heights, collapsed: split.collapsed.clone() })
}

/// A window's content area split into the terminal room and the panel's, the panel on the
/// right (`RTV:412-438`). Hidden gives the whole area to the terminal and no panel.
pub fn split_window(area: Rect, visible: bool, width: f32) -> (Rect, Option<Rect>) {
    if !visible {
        return (area, None);
    }
    let w = width.clamp(0.0, area.w);
    let term = Rect { w: area.w - w, ..area };
    let panel = Rect { x: area.x + area.w - w, w, ..area };
    (term, Some(panel))
}

/// One wrapper's rectangles (`TW:88-107`). `content` is `None` while collapsed.
#[derive(Clone, Debug, PartialEq)]
pub struct WrapperLayout {
    pub frame: Rect,
    pub close: Rect,
    pub title: Rect,
    pub content: Option<Rect>,
}

/// The panel's whole layout: the drag handle, the hamburger button, the empty-panel label, each
/// wrapper and the dividers between them.
#[derive(Clone, Debug, PartialEq)]
pub struct BeltLayout {
    pub panel: Rect,
    pub handle: Rect,
    pub hamburger: Rect,
    pub empty_label: Option<Rect>,
    pub wrappers: Vec<WrapperLayout>,
    pub dividers: Vec<Rect>,
}

fn wrapper_layout(frame: Rect, collapsed: bool, top: f32, close: f32, close_x: f32, title_h: f32, title_gap: f32, side: f32, bottom: f32) -> WrapperLayout {
    let side_margin = close_x + close;
    let close_r = Rect { x: frame.x + close_x, y: frame.y + top, w: close, h: close };
    let title = Rect { x: frame.x + side_margin, y: frame.y + top, w: (frame.w - close - side - side_margin).max(0.0), h: title_h - top };
    let content = if collapsed {
        None
    } else {
        Some(Rect {
            x: frame.x + side,
            y: frame.y + title_h + title_gap,
            w: (frame.w - 2.0 * side).max(0.0),
            h: (frame.h - title_h - title_gap - bottom).max(0.0),
        })
    };
    WrapperLayout { frame, close: close_r, title, content }
}

/// The panel's layout at `scale` (`TBV:311-429`, `TW:15-124`). `split.heights` are points, like
/// this module's own constants (`TITLE_H` and the rest); both are multiplied by `scale` here to
/// get pixels. `panel` is trusted to already be in pixels (the app's own room for it, already
/// scaled), matching how `clippings::layout` takes an already-pixel `panel` alongside its own
/// scaled constants.
pub fn layout(panel: Rect, split: &Split, scale: f32) -> BeltLayout {
    let title_h = TITLE_H * scale;
    let title_gap = TITLE_GAP * scale;
    let side = SIDE * scale;
    let bottom = BOTTOM * scale;
    let close = CLOSE * scale;
    let close_x = CLOSE_X * scale;
    let top = TOP * scale;
    let collapsed_h = COLLAPSED_H * scale;
    let handle_w = HANDLE_W * scale;
    let divider = DIVIDER * scale;

    let handle = Rect { x: panel.x, y: panel.y, w: handle_w, h: panel.h };
    let hamburger = Rect { x: panel.x + panel.w - close - 4.0 * scale, y: panel.y + 4.5 * scale, w: close, h: close };

    let empty_label = if split.tools.is_empty() {
        let w = (panel.w - 2.0 * side).max(0.0);
        if w < 16.0 * scale {
            None
        } else {
            Some(Rect { x: panel.x + side, y: panel.y + collapsed_h, w, h: (panel.h - collapsed_h).max(0.0) })
        }
    } else {
        None
    };

    let mut wrappers = Vec::with_capacity(split.tools.len());
    let mut dividers = Vec::new();
    let mut y = panel.y;
    for (i, &collapsed) in split.collapsed.iter().enumerate() {
        let h = if collapsed { collapsed_h } else { split.heights[i] * scale };
        let frame = Rect { x: panel.x, y, w: panel.w, h };
        wrappers.push(wrapper_layout(frame, collapsed, top, close, close_x, title_h, title_gap, side, bottom));
        y += h;
        if i + 1 < split.collapsed.len() {
            dividers.push(Rect { x: panel.x, y, w: panel.w, h: divider });
            y += divider;
        }
    }

    BeltLayout { panel, handle, hamburger, empty_label, wrappers, dividers }
}

/// What the panel is showing at a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeltHit {
    Handle,
    Hamburger,
    Close(usize),
    Divider(usize),
    Title(usize),
    Content(usize),
    Nothing,
}

pub fn hit(l: &BeltLayout, x: f32, y: f32) -> BeltHit {
    if l.handle.contains(x, y) {
        return BeltHit::Handle;
    }
    if l.hamburger.contains(x, y) {
        return BeltHit::Hamburger;
    }
    for (i, w) in l.wrappers.iter().enumerate() {
        if w.close.contains(x, y) {
            return BeltHit::Close(i);
        }
        if w.title.contains(x, y) {
            return BeltHit::Title(i);
        }
        if let Some(c) = w.content {
            if c.contains(x, y) {
                return BeltHit::Content(i);
            }
        }
    }
    for (i, d) in l.dividers.iter().enumerate() {
        if d.contains(x, y) {
            return BeltHit::Divider(i);
        }
    }
    BeltHit::Nothing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_expanded_tool_can_have_the_keyboard() {
        let split = Split { tools: vec![ToolId::Snippets, ToolId::Profiles], heights: vec![COLLAPSED_H, 200.0], collapsed: vec![true, false] };
        assert!(!split.expanded(ToolId::Snippets), "collapsed");
        assert!(split.expanded(ToolId::Profiles));
        assert!(!split.expanded(ToolId::Actions), "not in the split");
    }
    use crate::toolbelt::ToolId::{Actions, Profiles, Snippets};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn a_dynamic_tool_reconciles_like_any_other() {
        let web = ToolId::Dynamic("Web");
        assert_eq!(wrapper_minimum(web), 46.0);
        let split = Split { tools: vec![Profiles], heights: vec![300.0], collapsed: vec![false] };
        let two = reconcile(&split, &[Profiles, web], 400.0);
        assert_eq!(two.tools, vec![Profiles, web]);
        assert_eq!(two.heights.len(), 2);
        assert!((two.heights.iter().sum::<f32>() - 400.0).abs() < 0.01);
        assert!(two.heights[1] >= wrapper_minimum(web));
        assert!(two.expanded(web));
        let one = reconcile(&two, &[Profiles], 400.0);
        assert_eq!(one.tools, vec![Profiles]);
    }

    #[test]
    fn panel_takes_room_before_the_panes() {
        let area = rect(0.0, 30.0, 1000.0, 600.0);
        let (term, panel) = split_window(area, true, 250.0);
        assert_eq!(term, rect(0.0, 30.0, 750.0, 600.0));
        assert_eq!(panel, Some(rect(750.0, 30.0, 250.0, 600.0)));

        let (term, panel) = split_window(area, false, 250.0);
        assert_eq!(term, area);
        assert_eq!(panel, None);
    }

    #[test]
    fn split_view_follows_iterm2() {
        assert_eq!(wrapper_minimum(Profiles), 141.0);
        assert_eq!(wrapper_minimum(Snippets), 118.0);
        assert_eq!(wrapper_minimum(Actions), 91.0);

        let split = Split { tools: vec![Profiles, Snippets, Actions], heights: vec![200.0, 150.0, 100.0], collapsed: vec![false, false, false] };

        let shrunk = resize(&split, 450.0, 250.0);
        assert_eq!(shrunk.collapsed, vec![false, true, true]);
        assert_eq!(shrunk.heights[1], COLLAPSED_H);
        assert_eq!(shrunk.heights[2], COLLAPSED_H);
        assert_eq!(shrunk.heights.iter().sum::<f32>(), 250.0);
        assert!(shrunk.heights[0] >= wrapper_minimum(Profiles));

        let grown = resize(&shrunk, 250.0, 320.0);
        assert_eq!(grown.collapsed, vec![false, false, true]);
        assert_eq!(grown.heights[2], COLLAPSED_H);
        assert_eq!(grown.heights.iter().sum::<f32>(), 320.0);
        assert!(grown.heights[0] >= wrapper_minimum(Profiles));
        assert!(grown.heights[1] >= wrapper_minimum(Snippets));

        assert_eq!(respect_minimums(&[50.0, 300.0], &[141.0, 118.0], 350.0), vec![141.0, 209.0]);

        let two = Split { tools: vec![Profiles, Snippets], heights: vec![200.0, 150.0], collapsed: vec![false, false] };
        assert_eq!(divider_limits(&two, 0), (141.0, 234.0));

        let dragged_low = drag_divider(&two, 0, -50.0);
        assert_eq!(dragged_low.heights, vec![141.0, 209.0]);
        let dragged_high = drag_divider(&two, 0, 10_000.0);
        assert_eq!(dragged_high.heights[0], 234.0);
        assert_eq!(dragged_high.heights[0] + dragged_high.heights[1], 350.0);
    }

    #[test]
    fn reconcile_adds_at_the_bottom_and_keeps_heights() {
        let one = Split { tools: vec![Profiles], heights: vec![400.0], collapsed: vec![false] };
        let two = reconcile(&one, &[Profiles, Snippets], 400.0);
        assert_eq!(two.tools, vec![Profiles, Snippets]);
        assert_eq!(two.collapsed, vec![false, false]);
        assert_eq!(two.heights.iter().sum::<f32>(), 400.0);
        assert!(two.heights[0] >= wrapper_minimum(Profiles));
        assert!(two.heights[1] >= wrapper_minimum(Snippets));

        let removed = reconcile(&two, &[Profiles], 400.0);
        assert_eq!(removed.tools, vec![Profiles]);
        assert_eq!(removed.heights, vec![400.0]);

        let unchanged = reconcile(&two, &[Profiles, Snippets], 900.0);
        assert_eq!(unchanged, two);
    }

    #[test]
    fn proportions_round_trip_like_iterm2() {
        let split = Split { tools: vec![Profiles, Snippets], heights: vec![300.0, 100.0], collapsed: vec![false, false] };
        let v = proportions_value(&split);
        let arr = v.get("proportions").unwrap().as_array().unwrap();
        assert_eq!(arr[0].get("name").unwrap().as_str().unwrap(), "Profiles");
        assert_eq!(arr[0].get("heightAsFraction").unwrap().as_float().unwrap(), 0.75);
        assert_eq!(arr[1].get("name").unwrap().as_str().unwrap(), "Snippets");
        assert_eq!(arr[1].get("heightAsFraction").unwrap().as_float().unwrap(), 0.25);

        // Snippets' 0.25 share of 400 (100) is below its 118 minimum; iTerm2's algorithm
        // (TBV:900-921) raises it to 118 and steals the 18 shortfall from Profiles' room above
        // its own minimum (159), not the brief's literal "300, 100" for this step. Followed the
        // source per the task instructions; see the report.
        let applied = apply_proportions(&split, &v, 400.0).unwrap();
        assert_eq!(applied.heights, vec![282.0, 118.0]);

        assert!(apply_proportions(&split, &Value::Table(toml::Table::new()), 400.0).is_none());
        let mut bad = toml::Table::new();
        bad.insert("proportions".into(), Value::Array(vec![Value::Integer(1)]));
        assert!(apply_proportions(&split, &Value::Table(bad), 400.0).is_none());

        let mut zero_frac = toml::Table::new();
        let mut e0 = toml::Table::new();
        e0.insert("name".into(), Value::String("Profiles".into()));
        e0.insert("heightAsFraction".into(), Value::Float(0.0));
        zero_frac.insert("proportions".into(), Value::Array(vec![Value::Table(e0)]));
        assert!(apply_proportions(&split, &Value::Table(zero_frac), 400.0).is_none());

        let mut missing_name = toml::Table::new();
        let mut e1 = toml::Table::new();
        e1.insert("heightAsFraction".into(), Value::Float(0.5));
        missing_name.insert("proportions".into(), Value::Array(vec![Value::Table(e1)]));
        assert!(apply_proportions(&split, &Value::Table(missing_name), 400.0).is_none());

        assert!(apply_proportions(&split, &Value::String("nope".into()), 400.0).is_none());

        let mut mismatched = toml::Table::new();
        let mut e2 = toml::Table::new();
        e2.insert("name".into(), Value::String("Snippets".into()));
        e2.insert("heightAsFraction".into(), Value::Float(0.5));
        let mut e3 = toml::Table::new();
        e3.insert("name".into(), Value::String("Profiles".into()));
        e3.insert("heightAsFraction".into(), Value::Float(0.5));
        mismatched.insert("proportions".into(), Value::Array(vec![Value::Table(e2), Value::Table(e3)]));
        assert!(apply_proportions(&split, &Value::Table(mismatched), 400.0).is_none());

        let mut prefix = toml::Table::new();
        let mut e4 = toml::Table::new();
        e4.insert("name".into(), Value::String("Profiles".into()));
        e4.insert("heightAsFraction".into(), Value::Float(0.5));
        prefix.insert("proportions".into(), Value::Array(vec![Value::Table(e4)]));
        let partial = apply_proportions(&split, &Value::Table(prefix), 400.0).unwrap();
        assert_eq!(partial.heights[0], 200.0);
        assert_eq!(partial.heights[1], 100.0);

        let empty = Split { tools: vec![Profiles, Snippets], heights: vec![0.0, 0.0], collapsed: vec![false, false] };
        assert_eq!(proportions_value(&empty), Value::Table(toml::Table::new()));
    }

    #[test]
    fn belt_layout_follows_iterm2() {
        let panel = rect(750.0, 0.0, 250.0, 600.0);
        let one = Split { tools: vec![Profiles], heights: vec![600.0], collapsed: vec![false] };
        let l = layout(panel, &one, 1.0);
        assert_eq!(l.handle, rect(750.0, 0.0, 3.0, 600.0));
        let w = &l.wrappers[0];
        assert_eq!(w.close, rect(752.0, 3.0, 17.0, 17.0));
        assert_eq!(w.title, rect(769.0, 3.0, 250.0 - 17.0 - 4.0 - 19.0, 16.0));
        assert_eq!(w.content, Some(rect(754.0, 23.0, 242.0, 600.0 - 31.0)));

        // Heights are points, like the module's own constants; both are scaled together. A
        // point Split laid into a (pixel) panel double the point size, at scale 2, lands exactly
        // where it would if the whole thing (panel included) had been drawn at scale 1 and then
        // doubled.
        let panel2 = rect(1500.0, 0.0, 500.0, 1200.0);
        let one_pt = Split { tools: vec![Profiles], heights: vec![600.0], collapsed: vec![false] };
        let l2s = layout(panel2, &one_pt, 2.0);
        assert_eq!(l2s.handle, rect(1500.0, 0.0, 6.0, 1200.0));
        let w2 = &l2s.wrappers[0];
        assert_eq!(w2.close, rect(1504.0, 6.0, 34.0, 34.0));
        assert_eq!(w2.title, rect(1538.0, 6.0, 2.0 * (250.0 - 17.0 - 4.0 - 19.0), 32.0));
        assert_eq!(w2.content, Some(rect(1508.0, 46.0, 484.0, 1200.0 - 62.0)));

        let collapsed_split = Split { tools: vec![Profiles], heights: vec![22.0], collapsed: vec![true] };
        let lc = layout(panel, &collapsed_split, 1.0);
        assert_eq!(lc.wrappers[0].content, None);

        let no_tools = Split::default();
        let le = layout(panel, &no_tools, 1.0);
        assert!(le.empty_label.is_some());
        let band = le.empty_label.unwrap();
        assert_eq!(band.y, panel.y + 22.0);
        assert_eq!(band.x, panel.x + 4.0);
        assert_eq!(band.w, panel.w - 8.0);

        let narrow = rect(750.0, 0.0, 15.0, 600.0);
        let ln = layout(narrow, &no_tools, 1.0);
        assert_eq!(ln.empty_label, None);

        let two = Split { tools: vec![Profiles, Snippets], heights: vec![300.0, 269.0], collapsed: vec![false, false] };
        let l2 = layout(panel, &two, 1.0);
        assert_eq!(l2.dividers.len(), 1);
        assert_eq!(l2.dividers[0], rect(750.0, 300.0, 250.0, 1.0));

        assert_eq!(hit(&l2, 751.0, 300.0), BeltHit::Handle);
        assert_eq!(hit(&l2, l2.hamburger.x + 1.0, l2.hamburger.y + 1.0), BeltHit::Hamburger);
        assert_eq!(hit(&l2, 753.0, 4.0), BeltHit::Close(0));
        assert_eq!(hit(&l2, 800.0, 300.0), BeltHit::Divider(0));
        assert_eq!(hit(&l2, 800.0, 400.0), BeltHit::Content(1));
    }

    #[test]
    fn divider_limits_are_collapse_aware_and_drag_never_goes_negative() {
        // The state `resize(.., 450, 250)` leaves behind: Snippets and Actions collapsed at 22,
        // Profiles holding the rest. Dragging the divider between the two collapsed wrappers
        // must use their COLLAPSED_H, not their expanded minimums (TW:115-120), or the limits
        // invert and dragging can drive a height negative.
        let split = Split { tools: vec![Profiles, Snippets, Actions], heights: vec![206.0, 22.0, 22.0], collapsed: vec![false, true, true] };
        assert_eq!(divider_limits(&split, 1), (229.0, 231.0));

        let low = drag_divider(&split, 1, -1_000.0);
        assert_eq!(low.heights[1], 22.0);
        assert_eq!(low.heights[2], 22.0);

        let high = drag_divider(&split, 1, 1_000.0);
        assert_eq!(high.heights[1], 24.0);
        assert_eq!(high.heights[2], 20.0);
        assert!(high.heights.iter().all(|&h| h >= 0.0));

        // A directly-constructed (not `resize`-derived) Split where the upper wrapper of the
        // pair is itself under its own minimum inverts `divider_limits` (its minimum alone
        // exceeds the two wrappers' combined height). `drag_divider` must still never produce a
        // negative height.
        let inverted = Split { tools: vec![Profiles, Snippets], heights: vec![50.0, 10.0], collapsed: vec![false, false] };
        let (lo, hi) = divider_limits(&inverted, 0);
        assert!(lo > hi, "expected inverted limits, got ({lo}, {hi})");

        let dragged = drag_divider(&inverted, 0, 1_000.0);
        assert_eq!(dragged.heights[0], 60.0);
        assert_eq!(dragged.heights[1], 0.0);
        assert!(dragged.heights.iter().all(|&h| h >= 0.0));
    }
}
