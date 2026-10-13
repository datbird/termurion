//! The pointer shape over the terminal (`PTYTextView+ARC.m:728-771`) and the I-beams Termurion
//! draws itself (iTerm2's `IBarCursor*.png` are GPL-2.0 and are not copied).

/// A pointer shape. The first five are iTerm2's; the two resize shapes are Termurion's own for the
/// side bar handle and the split dividers (`RTV:1886`), decided outside `choose`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Arrow,
    IBeam,
    IBeamWithCircle,
    PointingHand,
    Crosshair,
    /// Horizontal resize: a side bar handle or the divider of a side by side split.
    ResizeH,
    /// Vertical resize: the divider of a stacked split.
    ResizeV,
    /// OSC 22 only: not allowed, resize toward one edge, an open hand, zoom in and out.
    NotAllowed,
    ResizeUp,
    ResizeDown,
    ResizeLeft,
    ResizeRight,
    Grab,
    ZoomIn,
    ZoomOut,
}

/// The shape an X11 cursor name asks for in OSC 22 (`PTYSession.m:18420-18457`). An exact lookup
/// in a fixed table: a name the table does not hold gives None (the caller goes back to the
/// default), and the text is never used for anything else.
pub fn from_x11(name: &str) -> Option<Shape> {
    Some(match name {
        "X_cursor" => Shape::NotAllowed,
        "arrow" | "left_ptr" | "watch" => Shape::Arrow,
        "based_arrow_down" => Shape::ResizeDown,
        "based_arrow_up" | "sb_up_arrow" => Shape::ResizeUp,
        "cross" | "cross_reverse" | "crosshair" | "tcross" => Shape::Crosshair,
        "fleur" => Shape::Grab,
        "hand1" | "hand2" => Shape::PointingHand,
        "left_side" | "sb_left_arrow" => Shape::ResizeLeft,
        "right_side" | "sb_right_arrow" => Shape::ResizeRight,
        "minus" => Shape::ZoomOut,
        "plus" => Shape::ZoomIn,
        "sb_h_double_arrow" => Shape::ResizeH,
        "sb_v_double_arrow" => Shape::ResizeV,
        "xterm" => Shape::IBeam,
        _ => return None,
    })
}

/// What is under the pointer, as `PTYTextView+ARC.m:728-771` asks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Over {
    /// Termurion's block selection chord (Ctrl+Alt; Win+Alt in iTerm2).
    pub rect_mods: bool,
    /// A Win-click target is under the pointer (Win held, Alt not).
    pub action_with_win: bool,
    /// An image tile.
    pub image: bool,
    /// The program would receive a click here (`mouseReportingAllowedForEvent` and tracking on).
    pub reporting: bool,
    /// One of Termurion's buttons or overlays.
    pub button: bool,
    /// A fold's summary line.
    pub fold: bool,
    /// A pane's text area (not a bar, the tab bar, a divider or a panel).
    pub terminal: bool,
    /// The shape the pane's program asked for with OSC 22: it replaces the I-beam and the
    /// reporting ring (`PTYTextView+ARC.m:750-770`).
    pub program: Option<Shape>,
}

/// The shape iTerm2 picks, in its order: a rectangle chord, a link with Win, an image, a
/// reporting program, a button, a fold, else the I-beam. Outside a terminal: the arrow.
pub fn choose(o: Over) -> Shape {
    if !o.terminal {
        Shape::Arrow
    } else if o.rect_mods {
        Shape::Crosshair
    } else if o.action_with_win {
        Shape::PointingHand
    } else if o.image {
        Shape::Arrow
    } else if o.reporting {
        o.program.unwrap_or(Shape::IBeamWithCircle)
    } else if o.button || o.fold {
        Shape::Arrow
    } else {
        o.program.unwrap_or(Shape::IBeam)
    }
}

/// The two drawn cursors, one set for every window.
pub struct Cursors {
    pub ibeam: winit::window::CustomCursor,
    pub ibeam_circle: winit::window::CustomCursor,
}

/// iTerm2's hot spot: the center of the image.
pub fn hot_spot(w: u16, h: u16) -> (u16, u16) {
    (w / 2, h / 2)
}

/// RGBA pixels of a `w` by `h` image: black ink with a one pixel white outline around it, so it
/// shows on dark and light cells.
fn render(w: usize, h: usize, ink: impl Fn(usize, usize) -> bool) -> Vec<u8> {
    let mut px = vec![0u8; w * h * 4];
    let black: Vec<bool> = (0..w * h).map(|i| ink(i % w, i / w)).collect();
    for y in 0..h {
        for x in 0..w {
            let at = (y * w + x) * 4;
            if black[y * w + x] {
                px[at..at + 4].copy_from_slice(&[0, 0, 0, 255]);
                continue;
            }
            let near = (y.saturating_sub(1)..=(y + 1).min(h - 1)).any(|yy| (x.saturating_sub(1)..=(x + 1).min(w - 1)).any(|xx| black[yy * w + xx]));
            if near {
                px[at..at + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    px
}

/// The I-beam's ink at (x, y): a bar one device pixel per point wide at the center column and a
/// serif across the top and bottom, `s` device pixels per point.
fn beam(w: usize, h: usize, s: usize, x: usize, y: usize) -> bool {
    let cx = w / 2;
    let bar = x + s > cx && x <= cx && y >= s && y < h - s;
    // Half the serif's width: short enough that its outline leaves every corner clear.
    let half = if s == 1 { 1 } else { 5 };
    let serif = x + half >= cx && x <= cx + half && ((y >= s && y < 2 * s) || (y >= h - 2 * s && y < h - s));
    bar || serif
}

/// The text I-beam, 8 by 16 at 1x and 16 by 32 at 2x.
pub fn draw_ibeam(scale2: bool) -> (Vec<u8>, u16, u16) {
    let s = if scale2 { 2 } else { 1 };
    let (w, h) = (8 * s, 16 * s);
    (render(w, h, |x, y| beam(w, h, s, x, y)), w as u16, h as u16)
}

/// The I-beam with a ring around its middle, iTerm2's mark for a program that wants the mouse:
/// 8 by 15 at 1x and 16 by 31 at 2x (the sizes of iTerm2's `IBarCursorXMR`).
pub fn draw_ibeam_circle(scale2: bool) -> (Vec<u8>, u16, u16) {
    let s = if scale2 { 2 } else { 1 };
    let (w, h) = (8 * s, 16 * s - 1);
    let (cx, cy, r) = ((w / 2) as f32, (h / 2) as f32, 3.0 * s as f32);
    let ink = |x: usize, y: usize| {
        let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
        beam(w, h, s, x, y) || (d - r).abs() <= s as f32 / 2.0
    };
    (render(w, h, ink), w as u16, h as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn over(f: impl FnOnce(&mut Over)) -> Over {
        let mut o = Over { terminal: true, ..Over::default() };
        f(&mut o);
        o
    }

    #[test]
    fn pointer_shape_follows_iterm2() {
        assert_eq!(choose(Over::default()), Shape::Arrow, "not over a terminal");
        assert_eq!(choose(over(|_| {})), Shape::IBeam);
        assert_eq!(choose(over(|o| o.reporting = true)), Shape::IBeamWithCircle);
        assert_eq!(choose(over(|o| {
            o.image = true;
            o.reporting = true;
        })), Shape::Arrow, "images come first");
        assert_eq!(choose(over(|o| {
            o.rect_mods = true;
            o.action_with_win = true;
        })), Shape::Crosshair, "the rectangle chord comes before the link hand");
        assert_eq!(choose(over(|o| {
            o.action_with_win = true;
            o.reporting = true;
        })), Shape::PointingHand);
        assert_eq!(choose(over(|o| o.button = true)), Shape::Arrow);
        assert_eq!(choose(over(|o| o.fold = true)), Shape::Arrow);
        assert_eq!(choose(over(|o| {
            o.reporting = true;
            o.button = true;
        })), Shape::IBeamWithCircle, "reporting comes before buttons");
    }

    #[test]
    fn osc22_names_map_to_shapes() {
        let t = [
            ("X_cursor", Shape::NotAllowed), ("arrow", Shape::Arrow), ("based_arrow_down", Shape::ResizeDown),
            ("based_arrow_up", Shape::ResizeUp), ("cross", Shape::Crosshair), ("cross_reverse", Shape::Crosshair),
            ("crosshair", Shape::Crosshair), ("fleur", Shape::Grab), ("hand1", Shape::PointingHand),
            ("hand2", Shape::PointingHand), ("left_ptr", Shape::Arrow), ("left_side", Shape::ResizeLeft),
            ("minus", Shape::ZoomOut), ("plus", Shape::ZoomIn), ("right_side", Shape::ResizeRight),
            ("sb_h_double_arrow", Shape::ResizeH), ("sb_left_arrow", Shape::ResizeLeft),
            ("sb_right_arrow", Shape::ResizeRight), ("sb_up_arrow", Shape::ResizeUp),
            ("sb_v_double_arrow", Shape::ResizeV), ("tcross", Shape::Crosshair), ("watch", Shape::Arrow),
            ("xterm", Shape::IBeam),
        ];
        for (name, shape) in t {
            assert_eq!(from_x11(name), Some(shape), "{name}");
        }
        for bad in ["", "Arrow", "xterm ", "nonsense", "arrow\x1b[31m", "..\\..\\x"] {
            assert_eq!(from_x11(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_program_shape_replaces_the_beam_and_the_ring_only() {
        let prog = Some(Shape::PointingHand);
        assert_eq!(choose(over(|o| o.program = prog)), Shape::PointingHand);
        assert_eq!(choose(over(|o| {
            o.program = prog;
            o.reporting = true;
        })), Shape::PointingHand, "honored while the program tracks the mouse");
        assert_eq!(choose(over(|o| {
            o.program = prog;
            o.image = true;
        })), Shape::Arrow);
        assert_eq!(choose(over(|o| {
            o.program = prog;
            o.button = true;
        })), Shape::Arrow);
        assert_eq!(choose(Over { program: prog, ..Over::default() }), Shape::Arrow, "outside a pane");
    }

    #[test]
    fn cursor_images_are_drawn() {
        for (img, w, h, len) in [
            (draw_ibeam(false), 8, 16, 512),
            (draw_ibeam(true), 16, 32, 2048),
            (draw_ibeam_circle(false), 8, 15, 8 * 15 * 4),
            (draw_ibeam_circle(true), 16, 31, 16 * 31 * 4),
        ] {
            let (px, iw, ih) = img;
            assert_eq!((iw, ih, px.len()), (w, h, len));
            let at = |x: usize, y: usize| &px[(y * iw as usize + x) * 4..][..4];
            let mid = iw as usize / 2;
            assert!((0..ih as usize).any(|y| at(mid, y) == [0, 0, 0, 255]), "an opaque black pixel on the center column");
            assert_eq!(at(0, 0)[3], 0, "transparent corner");
            assert_eq!(at(iw as usize - 1, ih as usize - 1)[3], 0, "transparent corner");
        }
        assert_eq!(hot_spot(8, 16), (4, 8));
    }

    #[test]
    fn the_ring_marks_the_reporting_cursor() {
        let (plain, w, _) = draw_ibeam(false);
        let (ring, ..) = draw_ibeam_circle(false);
        // Left of the bar at the middle row: empty in the plain I-beam, ring ink in the other.
        let mid_row = 7usize;
        assert!(ring[(mid_row * w as usize) * 4 + 3] > 0 || ring[(mid_row * w as usize + 1) * 4 + 3] > 0);
        assert_eq!(plain[(mid_row * w as usize) * 4 + 3], 0);
    }
}
