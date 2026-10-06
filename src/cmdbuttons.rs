//! The buttons on a selected command (`PTYTextView.m:6741-7030`, `CommandShareMenuProvider`):
//! which show, in what order and where, their glyphs, tooltips and menu items. Pure; the pills
//! are drawn by `scene::command_buttons` and the actions are `app/cmdbuttons_ui`.

pub const DISABLE_SELECTION: &str = "Disable Command Selection";
pub const HELP: &str = "Help";
/// `Resources/Help/CommandSelectionHelp.md` with "iTerm2" as "Tandem".
pub const HELP_TEXT: &str = "## Command Selection\n\nWhen you click on a command in a terminal, it becomes \u{201c}selected\u{201d}. Tandem draws selected commands with an outline and dims non-selected commands. Some features, like Search and Filter, then operate only on the selected command. \n\nYou can disable this feature with \u{201c}Disable Command Selection\u{201d} in the settings menu from which you opened this Help window. If you\u{2019}d like to re-enable it later, search app Settings for \u{201c}Clicking: on a command selects it.\u{201d}";
pub const COPY_COMMAND: &str = "Copy Command";
pub const COPY_OUTPUT: &str = "Copy Output";
pub const COMMAND_COPIED: &str = "Command Copied";
pub const OUTPUT_COPIED: &str = "Output Copied";
pub const SAVE_OUTPUT: &str = "Save Command & Output\u{2026}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Settings,
    Copy,
    Bookmark,
    Share,
    Info,
    Fold,
    Unfold,
}

impl Button {
    /// Segoe Fluent Icons (no emoji): Settings, Copy, Pin, Share, Info, ChevronDown, ChevronUp.
    pub fn glyph(self) -> char {
        match self {
            Button::Settings => '\u{E713}',
            Button::Copy => '\u{E8C8}',
            Button::Bookmark => '\u{E718}',
            Button::Share => '\u{E72D}',
            Button::Info => '\u{E946}',
            Button::Fold => '\u{E70D}',
            Button::Unfold => '\u{E70E}',
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            Button::Settings => "Settings",
            Button::Copy => "Copy",
            Button::Bookmark => "Bookmark",
            Button::Share => "Share",
            Button::Info => "Command Info",
            Button::Fold => "Fold",
            Button::Unfold => "Unfold",
        }
    }
}

/// Whether the buttons show (`TV:6741-6745`): a selected command with text, not a line-style
/// mark, with `ShowButtonsForSelectedCommand` on.
pub fn shown(selected: Option<u32>, command_empty: bool, line_style: bool, key_on: bool) -> bool {
    selected.is_some() && !command_empty && !line_style && key_on
}

/// Right to left: settings, copy, bookmark, share, info, then fold or unfold.
pub fn buttons(folded: bool) -> [Button; 6] {
    [Button::Settings, Button::Copy, Button::Bookmark, Button::Share, Button::Info, if folded { Button::Unfold } else { Button::Fold }]
}

/// Each button's first column: 3 cells in from the right edge, then 3 cells less each
/// (`TV:6841-6860`).
pub fn columns(width: usize) -> [usize; 6] {
    std::array::from_fn(|i| width.saturating_sub(3 + 3 * i))
}

/// The row the buttons sit on: the command's first row, or the top visible row plus one when
/// that scrolled off and the region still shows (`TV:6745-6751`). None when the region is
/// wholly out of view.
pub fn row(command_first_row: i64, visible_top: i64, visible_bottom: i64, region_end: i64) -> Option<i64> {
    if region_end < visible_top || command_first_row > visible_bottom {
        return None;
    }
    let row = if command_first_row >= visible_top { command_first_row } else { visible_top + 1 };
    (row <= visible_bottom).then_some(row)
}

/// A button's menu items (`TV:6918-6980`, `CSM`); empty for a button that acts at once. The
/// share sheet's two items are not offered on Windows.
pub fn menu(b: Button, command: &str) -> Vec<&'static str> {
    match b {
        Button::Settings => vec![DISABLE_SELECTION, HELP],
        Button::Copy if !command.is_empty() => vec![COPY_COMMAND, COPY_OUTPUT],
        Button::Share => vec![crate::cmdinfo::ADD_SNIPPET, SAVE_OUTPUT, crate::cmdinfo::COPY_URL],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_buttons_follow_iterm2() {
        assert!(shown(Some(3), false, false, true));
        assert!(!shown(None, false, false, true));
        assert!(!shown(Some(3), true, false, true));
        assert!(!shown(Some(3), false, true, true));
        assert!(!shown(Some(3), false, false, false));
        assert_eq!(buttons(false), [Button::Settings, Button::Copy, Button::Bookmark, Button::Share, Button::Info, Button::Fold]);
        let folded = buttons(true);
        assert_eq!(folded[..5], buttons(false)[..5]);
        assert_eq!(folded[5], Button::Unfold);
        assert_eq!(columns(80), [77, 74, 71, 68, 65, 62]);
        assert_eq!(columns(2), [0, 0, 0, 0, 0, 0], "a narrow pane saturates, never wraps");
        let all = [Button::Settings, Button::Copy, Button::Bookmark, Button::Share, Button::Info, Button::Fold, Button::Unfold];
        for (i, a) in all.iter().enumerate() {
            assert!(!a.tooltip().is_empty(), "{a:?}");
            assert_ne!(a.glyph(), ' ');
            assert!(a.glyph() as u32 >= 0xE000, "an icon-font glyph, never an emoji: {a:?}");
            for b in &all[i + 1..] {
                assert_ne!(a.glyph(), b.glyph(), "{a:?} and {b:?}");
            }
        }
        assert_eq!(Button::Settings.tooltip(), "Settings");
        assert_eq!(Button::Info.tooltip(), "Command Info");
    }

    #[test]
    fn buttons_float_follow_iterm2() {
        assert_eq!(row(10, 0, 23, 30), Some(10));
        assert_eq!(row(-5, 0, 23, 30), Some(1));
        assert_eq!(row(-5, 0, 23, -1), None);
        assert_eq!(row(30, 0, 23, 40), None);
        assert_eq!(row(0, 0, 23, 30), Some(0));
        assert_eq!(row(-5, 0, 0, 30), None, "the floating row must be inside the view");
    }

    #[test]
    fn button_menus_follow_iterm2() {
        assert_eq!(menu(Button::Settings, "ls"), ["Disable Command Selection", "Help"]);
        assert_eq!(menu(Button::Copy, "ls"), ["Copy Command", "Copy Output"]);
        assert!(menu(Button::Copy, "").is_empty());
        assert_eq!(menu(Button::Share, "ls"), ["Add Command as Snippet", "Save Command & Output\u{2026}", "Copy Command URL to Clipboard"]);
        for b in [Button::Bookmark, Button::Info, Button::Fold, Button::Unfold] {
            assert!(menu(b, "ls").is_empty(), "{b:?} acts at once");
        }
        assert!(HELP_TEXT.starts_with("## Command Selection"));
        assert!(HELP_TEXT.contains("Tandem") && !HELP_TEXT.contains("iTerm2"));
        assert_eq!((COMMAND_COPIED, OUTPUT_COPIED, HELP), ("Command Copied", "Output Copied", "Help"));
        assert!(include_str!("cmdbuttons.rs").is_ascii(), "escapes outside ASCII in the source");
    }
}
