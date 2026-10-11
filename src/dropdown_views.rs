//! iTerm2's find bar and paste progress layouts, old and new.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub struct FindLayout {
    pub size: (f32, f32),
    pub field: Frame,
    pub arrows: Frame,
    pub close: Frame,
    pub modes: [&'static str; 6],
}

pub struct PasteLayout {
    pub size: (f32, f32),
    pub label: Frame,
    pub bar: Frame,
    pub cancel: Frame,
    pub label_text: &'static str,
    pub cancel_text: &'static str,
}

const fn fr(x: f32, y: f32, w: f32, h: f32) -> Frame {
    Frame { x, y, w, h }
}

const MODES: [&str; 6] = ["Smart Case Substring", "Case-Sensitive Substring", "Case-Insensitive Substring", "Case-Sensitive Regex", "Case-Insensitive Regex", "Erase Search History"];

/// The find bar: `FindView.xib` (old) or `MinimalFindView.xib` (new, the search wrapper's frames
/// added to its children's). Origin bottom-left, points.
pub fn find_layout(old_style: bool) -> FindLayout {
    if old_style {
        FindLayout { size: (288.0, 28.0), field: fr(8.0, 4.0, 206.0, 19.0), arrows: fr(219.0, 4.0, 44.0, 20.0), close: fr(261.0, 3.0, 24.0, 23.0), modes: MODES }
    } else {
        FindLayout { size: (380.0, 95.0), field: fr(16.0, 63.0, 278.0, 22.0), arrows: fr(302.0, 63.0, 42.0, 21.0), close: fr(343.0, 62.0, 24.0, 23.0), modes: MODES }
    }
}

/// The paste progress: `PasteView.xib` (old) or `MinimalPasteView.xib` (new).
pub fn paste_layout(old_style: bool) -> PasteLayout {
    if old_style {
        PasteLayout { size: (334.0, 29.0), label: fr(9.0, 7.0, 72.0, 14.0), bar: fr(87.0, 5.0, 159.0, 20.0), cancel: fr(249.0, -1.0, 82.0, 28.0), label_text: "Pasting\u{2026}", cancel_text: "Cancel" }
    } else {
        PasteLayout { size: (323.0, 60.0), label: fr(26.0, 22.0, 97.0, 17.0), bar: fr(129.0, 19.0, 131.0, 20.0), cancel: fr(268.0, 13.0, 28.0, 32.0), label_text: "Pasting\u{2026}", cancel_text: "\u{2715}" }
    }
}

/// The old previous/next pair's segment widths (`FindView.xib:27-28`).
pub const ARROW_SEGMENTS: (f32, f32) = (19.0, 18.0);

/// What a window recorded when it opened.
#[derive(Clone, Copy, Debug, Default)]
pub struct Opened(bool);

impl Opened {
    /// The window opens: it records `UseOldStyleDropDownViews` now ("only affects new windows").
    pub fn open(&mut self, key: bool) {
        self.0 = key;
    }
    pub fn old_style(&self) -> bool {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(x: f32, y: f32, w: f32, h: f32) -> Frame {
        Frame { x, y, w, h }
    }

    #[test]
    fn old_style_views_follow_iterm2() {
        let l = find_layout(true);
        assert_eq!(l.size, (288.0, 28.0));
        assert_eq!((l.field, l.arrows, l.close), (f(8.0, 4.0, 206.0, 19.0), f(219.0, 4.0, 44.0, 20.0), f(261.0, 3.0, 24.0, 23.0)));
        assert_eq!(l.modes, ["Smart Case Substring", "Case-Sensitive Substring", "Case-Insensitive Substring", "Case-Sensitive Regex", "Case-Insensitive Regex", "Erase Search History"]);
        assert_eq!(ARROW_SEGMENTS, (19.0, 18.0));
        let p = paste_layout(true);
        assert_eq!(p.size, (334.0, 29.0));
        assert_eq!((p.label, p.bar, p.cancel), (f(9.0, 7.0, 72.0, 14.0), f(87.0, 5.0, 159.0, 20.0), f(249.0, -1.0, 82.0, 28.0)));
        assert_eq!((p.label_text, p.cancel_text), ("Pasting\u{2026}", "Cancel"));
        let n = paste_layout(false);
        assert_eq!(n.size, (323.0, 60.0));
        assert_eq!((n.label, n.bar, n.cancel), (f(26.0, 22.0, 97.0, 17.0), f(129.0, 19.0, 131.0, 20.0), f(268.0, 13.0, 28.0, 32.0)));
        // MinimalFindView.xib: the search wrapper at (16, 63, 328, 22) holds the field (0, 0, 278, 22)
        // and the two 21-point arrows; the close button is at (343, 62, 24, 23).
        let m = find_layout(false);
        assert_eq!(m.size, (380.0, 95.0));
        assert_eq!((m.field, m.arrows, m.close), (f(16.0, 63.0, 278.0, 22.0), f(302.0, 63.0, 42.0, 21.0), f(343.0, 62.0, 24.0, 23.0)));
        assert_ne!(find_layout(true).size, m.size);
    }

    #[test]
    fn a_window_keeps_the_layout_it_opened_with() {
        let mut w = Opened::default();
        w.open(true);
        // The key turns off; the window keeps what it recorded.
        assert!(w.old_style());
        w.open(false);
        assert!(!w.old_style());
    }
}
