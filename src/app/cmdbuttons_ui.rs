//! The buttons on a selected command (`PTYTextView.m:6741-7030`): drawing them over the pane,
//! the press, their menus and what each does. Nothing here types into a terminal: the buttons
//! copy to the clipboard, save a file the person names, add a snippet (through Command Info's
//! guard), rename a mark, fold, or open Command Info. The command text is program-written, so a
//! write would have to pass `cmdinfo::writable`; none is made.

use super::*;
use crate::cmdbuttons::{self, Button};

/// The id prefix of a button menu's items.
const MENU_ID: &str = "cmdbtn:";

impl App {
    /// The fold in command `mark`'s region, if any: the Unfold button's target.
    fn command_fold(&self, pane: PaneId, mark: u32) -> Option<u32> {
        let p = self.panes.get(&pane)?;
        if p.screen.fold_count() == 0 {
            return None;
        }
        p.screen.command_fold(mark)
    }

    /// Draws the buttons for the pane's selected command and answers their hit rectangles.
    pub(super) fn draw_command_buttons(&self, frame: &mut Frame, id: PaneId, inner: Rect, cell: (f32, f32)) -> Vec<(PaneId, Rect, Button)> {
        let Some(p) = self.panes.get(&id) else { return Vec::new() };
        let Some(mark) = p.selected_command else { return Vec::new() };
        if p.screen.alt_screen() {
            return Vec::new();
        }
        let Some(m) = p.screen.command_record(mark) else { return Vec::new() };
        if !cmdbuttons::shown(Some(mark), m.command.trim().is_empty(), m.line_style, self.globals.bool("ShowButtonsForSelectedCommand")) {
            return Vec::new();
        }
        let Some((first, last)) = p.screen.command_grid_lines(mark) else { return Vec::new() };
        let grid = p.screen.term().grid();
        let (off, rows, cols) = (grid.display_offset() as i64, grid.screen_lines() as i64, grid.columns());
        let Some(line) = cmdbuttons::row(i64::from(first), -off, rows - 1 - off, i64::from(last)) else { return Vec::new() };
        let folded = self.command_fold(id, mark).is_some();
        let list: Vec<(Button, usize)> = cmdbuttons::buttons(folded).into_iter().zip(cmdbuttons::columns(cols)).collect();
        scene::command_buttons(frame, inner, cell, (line + off) as usize, &list, p.own().palette.fg).into_iter().map(|(b, r)| (id, r, b)).collect()
    }

    /// A left press on a button. True when it hit one.
    pub(super) fn cmd_button_press(&mut self, x: f32, y: f32, event_loop: &ActiveEventLoop) -> bool {
        let Some(&(pane, rect, button)) = self.cmd_button_hits.iter().find(|(_, r, _)| r.contains(x, y)) else { return false };
        let Some(mark) = self.panes.get(&pane).and_then(|p| p.selected_command) else { return true };
        let Some(command) = self.panes.get(&pane).and_then(|p| p.screen.command_record(mark)).map(|m| m.command.clone()) else { return true };
        let items = cmdbuttons::menu(button, &command);
        match button {
            Button::Settings | Button::Copy | Button::Share => self.cmd_button_menu_show(pane, mark, button, &items),
            Button::Bookmark => self.cmd_bookmark(pane, mark, event_loop),
            Button::Info => self.show_command_info(pane, mark, (rect.x, rect.y + rect.h), event_loop),
            Button::Fold => self.cmd_fold(pane, mark),
            Button::Unfold => {
                if let Some(f) = self.command_fold(pane, mark) {
                    self.unfold(pane, f);
                }
            }
        }
        self.redraw();
        true
    }

    /// Pops the menu at the pointer; its pick arrives as `cmdbtn:<n>`.
    fn cmd_button_menu_show(&mut self, pane: PaneId, mark: u32, button: Button, items: &[&'static str]) {
        use muda::ContextMenu;
        if items.is_empty() {
            return;
        }
        let Some(hwnd) = self.hwnd() else { return };
        let menu = muda::Menu::new();
        for (i, label) in items.iter().enumerate() {
            if let Err(e) = menu.append(&muda::MenuItem::with_id(format!("{MENU_ID}{i}"), label.replace('&', "&&"), true, None)) {
                return self.banners.set("menu", format!("Command menu: {e}"));
            }
        }
        self.cmd_button_menu = Some((pane, mark, button));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, None) };
        self.popup_menu = Some(menu);
    }

    /// A pick from a button's menu. False for other ids.
    pub(super) fn cmd_button_picked(&mut self, id: &str, _event_loop: &ActiveEventLoop) -> bool {
        let Some(n) = id.strip_prefix(MENU_ID).and_then(|n| n.parse::<usize>().ok()) else { return false };
        let Some((pane, mark, button)) = self.cmd_button_menu.take() else { return true };
        let Some(m) = self.panes.get(&pane).and_then(|p| p.screen.command_record(mark)) else { return true };
        let command = m.command.clone();
        let Some(&label) = cmdbuttons::menu(button, &command).get(n) else { return true };
        match label {
            cmdbuttons::DISABLE_SELECTION => self.disable_command_selection(),
            cmdbuttons::HELP => menu::info_box(cmdbuttons::HELP_TEXT),
            cmdbuttons::COPY_COMMAND => {
                self.cmdinfo_copy(command);
                self.show_toast(cmdbuttons::COMMAND_COPIED);
            }
            cmdbuttons::COPY_OUTPUT => {
                let text = self.panes.get(&pane).and_then(|p| p.screen.output_text(mark)).unwrap_or_default();
                self.cmdinfo_copy(text);
                self.show_toast(cmdbuttons::OUTPUT_COPIED);
            }
            crate::cmdinfo::ADD_SNIPPET => {
                if self.cmdinfo_add_snippet(&command) {
                    self.show_toast(crate::cmdinfo::SNIPPET_ADDED);
                }
            }
            cmdbuttons::SAVE_OUTPUT => self.cmd_save_output(pane, mark, &command),
            crate::cmdinfo::COPY_URL => {
                if let Some((info, _)) = self.command_info(pane, mark) {
                    self.cmdinfo_copy(crate::cmdinfo::command_url(&info.command, info.directory.as_deref(), info.host.as_deref(), info.user.as_deref(), info.local));
                    self.show_toast(crate::cmdinfo::COPIED);
                }
            }
            _ => {}
        }
        self.redraw();
        true
    }

    /// Bookmark: a named mark loses its name (iTerm2's remove bookmark), else the Set Named Mark
    /// prompt opens for it with an empty name (`TV:7018-7030`).
    fn cmd_bookmark(&mut self, pane: PaneId, mark: u32, event_loop: &ActiveEventLoop) {
        let named = self.panes.get(&pane).and_then(|p| p.screen.command_record(mark)).is_some_and(|m| m.name.is_some());
        if named {
            if let Some(p) = self.panes.get_mut(&pane) {
                p.screen.rename_mark(mark, None);
            }
        } else {
            self.open_rename_mark_prompt(pane, mark, "", event_loop);
        }
    }

    /// Fold: the command's output rows become one line.
    fn cmd_fold(&mut self, pane: PaneId, mark: u32) {
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let Some((a, b)) = p.screen.output_range_of(mark) else {
            return self.banners.set("action", "This command has no output to fold.");
        };
        if p.screen.fold(a.line.0, b.line.0).is_none() {
            self.banners.set("action", "Tandem cannot fold the cursor's line, or while a full-screen app runs.");
        }
    }

    /// Save Command & Output: the command line then its output as text, in a file the person
    /// names in a Save dialog.
    fn cmd_save_output(&mut self, pane: PaneId, mark: u32, command: &str) {
        let output = self.panes.get(&pane).and_then(|p| p.screen.output_text(mark)).unwrap_or_default();
        let Some(path) = rfd::FileDialog::new().set_title("Save Command & Output").set_file_name(save_name(command)).add_filter("Text", &["txt"]).save_file() else { return };
        if let Err(e) = crate::localfs::picked(&path).and_then(|p| crate::localfs::write(&p, format!("{command}\n{output}\n"))) {
            self.banners.set("action", format!("Could not save {}: {e}", path.display()));
        }
    }
}

/// The Save dialog's file name: the command's program name (`defaultCommand`), else "Tandem
/// Command", with only characters a file name may hold.
fn save_name(command: &str) -> String {
    let program = command.split_whitespace().next().unwrap_or("");
    let base = program.rsplit(['\\', '/']).next().unwrap_or("");
    let stem: String = base.chars().filter(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '+')).collect();
    let stem = stem.trim_matches('.');
    format!("{}.txt", if stem.is_empty() { "Tandem Command" } else { stem })
}

#[cfg(test)]
mod tests {
    use super::*;
    const SRC: &str = include_str!("cmdbuttons_ui.rs");

    /// Merge seam (S6): one region rule. 13s's `Screen::command_region` (buffer rows) and
    /// `App::command_region` are defined once and derive from 13r's `command_grid_lines`, which
    /// the buttons keep reading in grid lines.
    #[test]
    fn the_command_region_is_one_rule() {
        let code = &SRC[..SRC.find("#[cfg(test)]").unwrap()];
        assert!(!code.contains("fn command_region"), "App::command_region lives in selection_ui");
        assert!(code.contains("command_grid_lines("));
        let marks = include_str!("../marks.rs");
        assert!(marks.contains("pub fn command_grid_lines(&self"));
        assert_eq!(marks.matches("pub fn command_region(&mut self, id: u32) -> Option<(usize, usize)>").count(), 1);
        let region = &marks[marks.find("pub fn command_region(").unwrap()..];
        assert!(region[..region.find("\n    }").unwrap()].contains("self.command_grid_lines(id)"));
        let sel = include_str!("selection_ui.rs");
        assert_eq!(sel.matches("pub(super) fn command_region(&mut self, id: PaneId) -> Option<(usize, usize)>").count(), 1);
    }

    #[test]
    fn the_save_name_is_the_program() {
        assert_eq!(save_name("C:\\tools\\make all"), "make.txt");
        assert_eq!(save_name("./build.sh x"), "build.sh.txt");
        assert_eq!(save_name(""), "Tandem Command.txt");
        assert_eq!(save_name("a:b*c?"), "abc.txt");
        assert_eq!(save_name("..\\.."), "Tandem Command.txt");
    }

    /// B5: program text (the command, its output) is never typed into a terminal from here.
    #[test]
    fn the_buttons_write_nothing_to_a_terminal() {
        let code = &SRC[..SRC.find("#[cfg(test)]").unwrap()];
        for needle in ["user_write(", ".input(", ".paste(", "reply(", "open_composer"] {
            assert!(!code.contains(needle), "{needle}");
        }
    }

    /// The press checks the buttons before any selection starts (they come before `pane_at`).
    #[test]
    fn the_press_comes_before_the_selection() {
        let m = include_str!("mod.rs");
        let body = &m[m.find("fn mouse_press").unwrap()..];
        assert!(body.find("cmd_button_press(").unwrap() < body.find("self.pane_at(x, y)").unwrap());
    }

    /// Each window keeps its own hits.
    #[test]
    fn the_hits_are_swapped_with_the_window() {
        let w = include_str!("windows_ui.rs");
        assert!(w.matches("cmd_button_hits").count() >= 4);
        assert!(w.contains("swap(&mut self.cmd_button_hits"));
    }
}
