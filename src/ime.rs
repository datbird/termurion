//! The IME composition drawn at the cursor (`iTermTextDrawingHelper.m:3621-3770`). Pure: the
//! app stores a `Preedit`, lays it over the grid with `layout`, and the scene draws the result.

use crate::layout::Rect;
use crate::palette::Rgb;

/// What the IME is composing: its text and the caret as a byte offset (None: at the end).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preedit {
    pub text: String,
    pub cursor: Option<usize>,
}

/// A run of composition text on one row: from `col`, `cells` columns wide.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub row: usize,
    pub col: usize,
    pub text: String,
    pub cells: usize,
}

/// A composition ready to draw: its pieces, the caret cell and the caret's color.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawn {
    pub pieces: Vec<Piece>,
    pub caret: (usize, usize),
    pub caret_color: Rgb,
}

/// What the app does with one `WindowEvent::Ime`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// A new entry begins (Windows sends `Enabled` only at the start of a composition): a cancel
    /// no longer waits for a stray commit.
    Begin,
    /// The IME went away: a composition in progress is dropped, nothing is sent.
    Clear,
    /// The composition ended with nothing to draw (an empty Preedit; its Commit may follow).
    End,
    /// A composition is in progress: draw it, send nothing.
    Store(Preedit),
    /// The person accepted this text: it is typed input now.
    Commit(String),
}

/// winit delivers `Preedit("")` to end a composition: that clears, it is not an empty
/// composition to draw. The caret is the start of the cursor range (a byte offset).
pub fn step(ime: &winit::event::Ime) -> Step {
    use winit::event::Ime;
    match ime {
        Ime::Enabled => Step::Begin,
        Ime::Disabled => Step::Clear,
        Ime::Preedit(text, _) if text.is_empty() => Step::End,
        Ime::Preedit(text, cursor) => Step::Store(Preedit { text: text.clone(), cursor: cursor.map(|(start, _)| start) }),
        Ime::Commit(text) if text.is_empty() => Step::End,
        Ime::Commit(text) => Step::Commit(text.clone()),
    }
}

/// Keys belong to the IME, not the session, while a composition is non-empty.
pub fn holds_keys(p: Option<&Preedit>) -> bool {
    p.is_some_and(|p| !p.text.is_empty())
}

/// One window's IME state. Panes are shared by every window but the key window and its focused
/// pane are not, so the composition is owned by (this window, a pane) and carries a generation
/// that every cancel bumps: a Commit is delivered only to its owner, only while that pane is
/// still this window's focused pane and the window is key.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// The composition being drawn, on the owner pane.
    pub preedit: Option<Preedit>,
    /// The pane that owns the composition, and the generation it began in. Kept after the IME
    /// ends the composition with an empty Preedit, since its Commit follows.
    owner: Option<(u64, u64)>,
    generation: u64,
    /// A cancel is waiting for the stray Commit it may provoke: the commit of the cancelled
    /// composition is dropped. It ends at the first sign of new input (a key press, `Enabled`, a
    /// non-empty preedit), so a commit with no composition before it (a Pinyin comma, the emoji
    /// panel) typed after an Esc is delivered.
    dropping: bool,
    /// The candidate window's area `ime_area` last gave the system, to set it only on a change.
    pub area: Option<[i32; 4]>,
}

impl State {
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The composition of `pane`, if it owns one.
    pub fn of(&self, pane: u64) -> Option<&Preedit> {
        self.preedit.as_ref().filter(|_| self.owner.is_some_and(|(p, g)| p == pane && g == self.generation))
    }

    /// A composition is visible on its owner pane (the keys are the IME's): focus follows mouse
    /// waits while it is (13j's seam).
    pub fn composing(&self) -> bool {
        self.owner.is_some_and(|(pane, _)| holds_keys(self.of(pane)))
    }

    /// The IME reports a composition on `pane`.
    pub fn store(&mut self, pane: u64, p: Preedit) {
        self.owner = Some((pane, self.generation));
        self.dropping = false;
        self.preedit = Some(p);
    }

    /// A new entry begins (`Ime::Enabled`).
    pub fn begin(&mut self) {
        self.dropping = false;
    }

    /// A key press reached the app: whatever the IME sends next is new input.
    pub fn input_started(&mut self) {
        self.dropping = false;
    }

    /// The system IME context was reset or its window replaced: it has no candidate area yet.
    pub fn system_reset(&mut self) {
        self.area = None;
    }

    /// The IME ended the composition (an empty Preedit): nothing is drawn, a Commit may follow.
    pub fn end(&mut self) {
        self.preedit = None;
    }

    /// Our own cancel (focus lost, a pane switch, Esc): the composition is gone uncommitted, and
    /// so is whatever the IME commits for it afterwards.
    pub fn cancel(&mut self) {
        self.preedit = None;
        self.owner = None;
        self.generation += 1;
        self.dropping = true;
    }

    /// The IME commits text. Returns the pane that takes it as typed input, None to drop it.
    pub fn commit(&mut self, focused: Option<u64>, key_window: bool) -> Option<u64> {
        self.preedit = None;
        if std::mem::take(&mut self.dropping) {
            self.owner = None;
            return None;
        }
        match self.owner.take() {
            Some((pane, g)) => Some(pane).filter(|p| g == self.generation && Some(*p) == focused && key_window),
            None => focused.filter(|_| key_window),
        }
    }

    /// Called at each frame of this window: a composition belongs to its focused pane while the
    /// window is key; anything else cancels it. True when it did.
    pub fn sync(&mut self, focused: Option<u64>, key_window: bool) -> bool {
        let Some((pane, _)) = self.owner.filter(|_| self.preedit.is_some()) else { return false };
        if key_window && Some(pane) == focused {
            return false;
        }
        self.cancel();
        true
    }
}

/// Who has the keyboard: a commit goes where a typed key would (`App::key`'s order).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Owners {
    /// A toolbelt tool's text entry has the keyboard: the Notes editor, Codecierge's goal and
    /// reply, or a tool's search field.
    pub tool: bool,
    /// A toolbelt tool has the keyboard with no text entry (a list): it takes the key.
    pub belt: bool,
    /// A Composer dropdown has the keyboard.
    pub composer: bool,
    /// A Composer status bar field has the keyboard.
    pub field: bool,
    /// An inline chat panel's input has the keyboard.
    pub inline: bool,
    /// The shortcut labels: any key puts them away.
    pub shortcuts: bool,
    /// The find bar is being typed in.
    pub find: bool,
    /// Copy Mode takes every key.
    pub copy_mode: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    Terminal,
    Find,
    Composer,
    Field,
    Tool,
    Inline,
    Drop,
}

pub fn dest(o: &Owners) -> Dest {
    if o.tool {
        Dest::Tool
    } else if o.belt {
        Dest::Drop
    } else if o.composer {
        Dest::Composer
    } else if o.field {
        Dest::Field
    } else if o.inline {
        Dest::Inline
    } else if o.shortcuts {
        Dest::Drop
    } else if o.find {
        Dest::Find
    } else if o.copy_mode {
        Dest::Drop
    } else {
        Dest::Terminal
    }
}

/// An owner that has no text entry for the commit right now (a composer that let go, a
/// Codecierge still onboarding) leaves it to the next owner, as its key would pass: the
/// terminal. Find and Drop are final.
pub fn after_refusal(d: Dest, taken: bool) -> Dest {
    match d {
        Dest::Composer | Dest::Field | Dest::Tool | Dest::Inline if !taken => Dest::Terminal,
        d => d,
    }
}

/// Lays `p` over the grid from `start` (row, col) in `cols` columns, as iTerm2 does
/// (`iTermTextDrawingHelper.m:3621-3770`): a character that would be cut at the right edge
/// moves to the next row whole. Returns the pieces and the caret cell: the cell the character
/// at the caret offset takes (after its wrap), or just past the end of the text; a string that
/// ends exactly at the right edge leaves the caret at `(row, cols)`.
pub fn layout(p: &Preedit, start: (usize, usize), cols: usize) -> (Vec<Piece>, (usize, usize)) {
    use unicode_width::UnicodeWidthChar;
    let cols = cols.max(1);
    let (mut row, mut col) = start;
    let mut pieces: Vec<Piece> = Vec::new();
    let mut caret = None;
    for (at, c) in p.text.char_indices() {
        let w = c.width().unwrap_or(0);
        if w > 0 && col + w > cols && col > 0 {
            row += 1;
            col = 0;
        }
        if caret.is_none() && p.cursor.is_some_and(|k| k <= at) {
            caret = Some((row, col));
        }
        match pieces.last_mut() {
            Some(last) if last.row == row => {
                last.text.push(c);
                last.cells += w;
            }
            _ => pieces.push(Piece { row, col, text: c.to_string(), cells: w }),
        }
        col += w;
    }
    (pieces, caret.unwrap_or((row, col)))
}

/// The caret's rectangle (`iTermTextDrawingHelper.m:3738-3765`): 2 points wide and the font's
/// own cell high (`base_h`), centered in the line, and kept inside the right margin.
pub fn caret_rect(caret: (usize, usize), origin: (f32, f32), cell: (f32, f32), base_h: f32, cols: usize, scale: f32) -> Rect {
    let (cw, ch) = cell;
    let w = 2.0 * scale;
    let h = if base_h > 0.0 { base_h } else { ch };
    let x = (origin.0 + caret.1 as f32 * cw).min(origin.0 + cols as f32 * cw - w);
    let y = origin.1 + caret.0 as f32 * ch + ((ch - h) / 2.0).round().max(0.0);
    Rect { x, y, w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pre(text: &str, cursor: Option<usize>) -> Preedit {
        Preedit { text: text.to_string(), cursor }
    }

    fn piece(row: usize, col: usize, text: &str, cells: usize) -> Piece {
        Piece { row, col, text: text.to_string(), cells }
    }

    #[test]
    fn ime_layout_follows_iterm2() {
        let (p, c) = layout(&pre("abc", Some(3)), (3, 2), 10);
        assert_eq!((p, c), (vec![piece(3, 2, "abc", 3)], (3, 5)));
        // Two wide characters from column 8: the second wraps; the caret at the end follows it.
        let (p, c) = layout(&pre("\u{65e5}\u{672c}", Some(6)), (3, 8), 10);
        assert_eq!(p, vec![piece(3, 8, "\u{65e5}", 2), piece(4, 0, "\u{672c}", 2)]);
        assert_eq!(c, (4, 2));
        // The caret before the wrapped character moves to the next row with it.
        let (_, c) = layout(&pre("\u{65e5}\u{672c}", Some(3)), (3, 8), 10);
        assert_eq!(c, (4, 0));
        // A wide character that does not fit is not split: row 3 gets nothing.
        let (p, c) = layout(&pre("\u{65e5}", None), (3, 9), 10);
        assert_eq!(p, vec![piece(4, 0, "\u{65e5}", 2)]);
        assert_eq!(c, (4, 2));
        // A string ending exactly at the right edge leaves the caret at (row, cols).
        let (p, c) = layout(&pre("abc", Some(3)), (3, 7), 10);
        assert_eq!(p, vec![piece(3, 7, "abc", 3)]);
        assert_eq!(c, (3, 10));
    }

    #[test]
    fn ime_caret_follows_iterm2() {
        let r = caret_rect((2, 3), (0.0, 0.0), (10.0, 24.0), 20.0, 10, 1.0);
        assert_eq!(r, Rect { x: 30.0, y: 50.0, w: 2.0, h: 20.0 });
        // At the right margin the caret is shoved left to stay inside the text area.
        let r = caret_rect((2, 10), (0.0, 0.0), (10.0, 24.0), 20.0, 10, 1.0);
        assert_eq!(r.x, 98.0);
        let r = caret_rect((2, 3), (0.0, 0.0), (10.0, 24.0), 20.0, 10, 1.5);
        assert_eq!(r.w, 3.0);
    }

    #[test]
    fn ime_events_become_steps() {
        use winit::event::Ime;
        assert_eq!(step(&Ime::Enabled), Step::Begin);
        assert_eq!(step(&Ime::Disabled), Step::Clear);
        // An empty preedit ends a composition; it is not drawn as an empty one.
        assert_eq!(step(&Ime::Preedit(String::new(), None)), Step::End);
        assert_eq!(step(&Ime::Preedit("\u{306b}".to_string(), Some((3, 3)))), Step::Store(pre("\u{306b}", Some(3))));
        assert_eq!(step(&Ime::Preedit("ab".to_string(), None)), Step::Store(pre("ab", None)));
        assert_eq!(step(&Ime::Commit("\u{65e5}\u{672c}".to_string())), Step::Commit("\u{65e5}\u{672c}".to_string()));
        assert_eq!(step(&Ime::Commit(String::new())), Step::End);
    }

    #[test]
    fn keys_are_held_only_while_composing() {
        assert!(!holds_keys(None));
        assert!(!holds_keys(Some(&pre("", None))));
        assert!(holds_keys(Some(&pre("a", None))));
    }

    #[test]
    fn a_commit_goes_to_its_owner_pane() {
        let mut s = State::default();
        s.store(7, pre("\u{306b}", Some(3)));
        // winit ends the composition with an empty Preedit, then Commits.
        s.end();
        assert_eq!(s.commit(Some(7), true), Some(7));
        // A commit with no composition before it is plain typed input for the focused pane.
        assert_eq!(s.commit(Some(7), true), Some(7));
        assert_eq!(s.commit(None, true), None);
    }

    #[test]
    fn a_commit_after_a_cancel_is_dropped() {
        let mut s = State::default();
        s.store(7, pre("a", None));
        s.cancel();
        assert!(s.preedit.is_none());
        assert_eq!(s.commit(Some(7), true), None, "the stray commit of a cancelled composition");
        // The next composition works again.
        s.store(7, pre("b", None));
        assert_eq!(s.commit(Some(7), true), Some(7));
    }

    #[test]
    fn a_commit_after_a_pane_switch_is_dropped() {
        let mut s = State::default();
        s.store(7, pre("a", None));
        assert_eq!(s.commit(Some(8), true), None, "pane 8 is focused now, pane 7 owned it");
        let mut s = State::default();
        s.store(7, pre("a", None));
        assert_eq!(s.commit(Some(7), false), None, "its window is not the key window");
    }

    #[test]
    fn another_windows_repaint_keeps_the_preedit() {
        // Each window has its own state: a frame of a window with nothing composing, whatever its
        // focus, never touches the key window's.
        let mut key = State::default();
        key.store(7, pre("a", None));
        let mut other = State::default();
        assert!(!other.sync(None, false));
        assert!(!key.sync(Some(7), true), "its own frame keeps it");
        assert!(key.preedit.is_some());
        // Its own pane switch drops it, and bumps the generation.
        let before = key.generation();
        assert!(key.sync(Some(8), true));
        assert!(key.preedit.is_none());
        assert_ne!(key.generation(), before);
    }

    #[test]
    fn a_commit_goes_where_typed_keys_go() {
        let none = Owners::default();
        assert_eq!(dest(&none), Dest::Terminal);
        assert_eq!(dest(&Owners { find: true, ..none }), Dest::Find);
        assert_eq!(dest(&Owners { composer: true, ..none }), Dest::Composer);
        assert_eq!(dest(&Owners { field: true, ..none }), Dest::Field);
        // The toolbelt's text entries and the inline chat input take typed text, so they take a commit.
        assert_eq!(dest(&Owners { tool: true, ..none }), Dest::Tool);
        assert_eq!(dest(&Owners { inline: true, ..none }), Dest::Inline);
        // A tool list, the shortcut labels and Copy Mode take the key and have no text entry.
        assert_eq!(dest(&Owners { belt: true, ..none }), Dest::Drop);
        assert_eq!(dest(&Owners { shortcuts: true, ..none }), Dest::Drop);
        assert_eq!(dest(&Owners { copy_mode: true, ..none }), Dest::Drop);
        // `key`'s order: a tool, then the composer, then the inline input, the labels, find, Copy Mode.
        assert_eq!(dest(&Owners { tool: true, composer: true, find: true, ..none }), Dest::Tool);
        assert_eq!(dest(&Owners { belt: true, tool: false, composer: true, ..none }), Dest::Drop);
        assert_eq!(dest(&Owners { composer: true, inline: true, ..none }), Dest::Composer);
        assert_eq!(dest(&Owners { inline: true, shortcuts: true, find: true, ..none }), Dest::Inline);
        assert_eq!(dest(&Owners { shortcuts: true, find: true, ..none }), Dest::Drop);
        assert_eq!(dest(&Owners { find: true, copy_mode: true, ..none }), Dest::Find);
    }

    #[test]
    fn an_owner_that_let_go_leaves_the_commit_to_the_terminal() {
        for d in [Dest::Composer, Dest::Field, Dest::Tool, Dest::Inline] {
            assert_eq!(after_refusal(d, false), Dest::Terminal, "{d:?} had no text entry");
            assert_eq!(after_refusal(d, true), d);
        }
        assert_eq!(after_refusal(Dest::Drop, false), Dest::Drop);
        assert_eq!(after_refusal(Dest::Find, false), Dest::Find);
    }

    #[test]
    fn a_commit_after_the_next_key_is_delivered() {
        let mut s = State::default();
        s.store(7, pre("a", None));
        s.cancel();
        // The person types again (a Pinyin comma): the next key press is new input.
        s.input_started();
        assert_eq!(s.commit(Some(7), true), Some(7));
    }

    #[test]
    fn a_commit_after_a_new_entry_begins_is_delivered() {
        let mut s = State::default();
        s.store(7, pre("a", None));
        s.cancel();
        // Windows sends Enabled only when a new composition starts (emoji panel, voice typing).
        s.begin();
        assert_eq!(s.commit(Some(7), true), Some(7));
    }

    #[test]
    fn the_stray_commit_of_a_cancelled_composition_is_still_dropped() {
        let mut s = State::default();
        s.store(7, pre("a", None));
        s.cancel();
        // Preedit(""), Commit, Disabled: nothing new began in between.
        s.end();
        assert_eq!(s.commit(Some(7), true), None);
        // A non-empty preedit is new input too.
        let mut s = State::default();
        s.store(7, pre("a", None));
        s.cancel();
        s.store(7, pre("b", None));
        assert_eq!(s.commit(Some(7), true), Some(7));
    }

    #[test]
    fn a_reset_ime_has_no_candidate_area() {
        let mut s = State::default();
        s.area = Some([1, 2, 3, 4]);
        s.system_reset();
        assert_eq!(s.area, None);
    }

    /// 13j seam: focus follows mouse reads `composing` and never acts while it is true: a
    /// visible composition on its owner, the same test that holds keys.
    #[test]
    fn composing_is_a_visible_composition() {
        let mut s = State::default();
        assert!(!s.composing(), "nothing yet");
        s.store(7, pre("", None));
        assert!(!s.composing(), "an empty preedit holds nothing");
        s.store(7, pre("ni", Some(2)));
        assert!(s.composing());
        s.end();
        assert!(!s.composing(), "the IME ended it; only a commit may follow");
        s.store(7, pre("ni", Some(2)));
        s.cancel();
        assert!(!s.composing(), "a cancel ends it");
        s.store(7, pre("ha", Some(2)));
        assert_eq!(s.commit(Some(7), true), Some(7));
        assert!(!s.composing(), "a commit ends it");
    }
}
