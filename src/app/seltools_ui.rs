//! The selection tools in the window: Replace Selection, Clear to Start of Selection,
//! Selection Respects Soft Boundaries, and the selection endpoint moves.

use alacritty_terminal::index::Point;
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;

use super::*;
use super::find_ui::{to_point, to_pos};
use crate::find::Pos;
use crate::seltools::{self, End};

/// Menu items with a check mark that milestone 4c adds, by selector.
pub(super) const CHECKED: &[&str] =
    &["toggleSelectionRespectsSoftBoundaries:", "toggleCopyMode:", "toggleShowTimestamps:", "toggleCursorGuide:", "showHideNotes:", "editSessionNote:", "toggleBufferInput:", "toggleEnableTriggersInInteractiveApps:", "toggleNotifyOnStatusChange:", "togglePinHotkeyWindow:", "disableBroadcasting:", "enableSendInputToAllTabs:", "enableSendInputToAllPanes:", "toggleUseBackgroundPatternIndicator:", "toggleBroadcastSource:", "toggleTmuxPausePane:", "toggleAutoCommandHistory:", "toggleShowClippings:", "toggleShowInlineChat:", "toggleToolbeltVisibility:", "toggleAutoComposer:", "toggleFullScreenTabBar:", "toggleUseTransparency:", "toggleDisableTransparencyForActiveWindow:", "toggleLockSplitPaneWidth:", "debugLogging:"];

impl App {
    /// Edit > Copy with Styles: plain text, HTML, and RTF in the profile's font and colors.
    pub(super) fn copy_with_styles(&mut self, id: PaneId) {
        self.copy_with_styles_with(id, None);
    }

    /// `copy_with_styles` with the text already extracted, when the caller has it.
    pub(super) fn copy_with_styles_with(&mut self, id: PaneId, text: Option<&str>) {
        if self.copy_porthole_selection(id, super::porthole_ui::CopyAs::Styles) {
            return;
        }
        // The user selects in the screen on show: a Filter or Zoom view while one is up.
        let Some(text) = text.map(str::to_string).or_else(|| self.panes.get(&id).and_then(|p| p.screen.selection_text())) else { return };
        // An empty copy never clears the clipboard with DisallowCopyEmptyString (`PTYTextView.m:4189-4193`).
        if !crate::seltext::copy_decision(&text, self.globals.bool("DisallowCopyEmptyString")) {
            return;
        }
        // The styled copy reaches Paste History as the plain one does (`PTYTextView.m:4210`).
        self.remember_paste(&text);
        let Some(p) = self.panes.get(&id) else { return };
        let screen = &p.screen;
        let font = self.profile_of(id).map(|pr| pr.font("Normal Font")).unwrap_or(crate::profile::FontSpec { family: "Cascadia Mono".into(), size_pt: 12.0 });
        let rows = p.styled_rows();
        let html = crate::styled_copy::cf_html(&crate::styled_copy::html(&rows, &font.family, font.size_pt, screen.palette.bg, crate::styled_copy::StyleOptions::from_globals(&self.globals)));
        let rtf = crate::styled_copy::rtf(&rows, &font.family, font.size_pt, crate::styled_copy::StyleOptions::from_globals(&self.globals));
        if let Err(e) = crate::winclip::set(&text, &html, &rtf) {
            self.banners.set("clipboard", format!("Copy failed: {e}"));
        }
    }

    /// Shell > Print > Screen, Selection, or Buffer.
    pub(super) fn print(&mut self, id: PaneId, what: crate::print::What) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{IDCANCEL, IDYES, MB_DEFBUTTON1, MB_DEFBUTTON2, MB_ICONQUESTION, MB_YESNOCANCEL, MessageBoxW};
        use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
        let Some(hwnd) = self.hwnd() else { return };
        let Some(p) = self.panes.get(&id) else { return };
        // The user selects in the screen on show: a Filter or Zoom view while one is up.
        let screen = &p.screen;
        if what == crate::print::What::Selection && screen.selection_text().is_none() {
            unsafe { MessageBeep(0) };
            return;
        }
        let mut rows = crate::print::rows(screen, what);
        let cols = { use alacritty_terminal::grid::Dimensions; screen.term().columns() };
        let title = screen.title().unwrap_or("Tandem").to_string();
        let font = self.profile_of(id).map(|pr| pr.font("Normal Font")).unwrap_or(crate::profile::FontSpec { family: "Cascadia Mono".into(), size_pt: 12.0 });
        // iTerm2 keeps "Black and white" in its print panel; Tandem asks first.
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let default = if self.globals.bool("Print In Black And White") { MB_DEFBUTTON1 } else { MB_DEFBUTTON2 };
        let answer = unsafe {
            MessageBoxW(hwnd as _, wide("Print in black and white?\n\nYes leaves out colors; No prints the terminal's colors.").as_ptr(), wide("Print").as_ptr(), MB_YESNOCANCEL | MB_ICONQUESTION | default)
        };
        if answer == IDCANCEL {
            return;
        }
        let bw = answer == IDYES;
        if bw != self.globals.bool("Print In Black And White") {
            self.globals.set("Print In Black And White", toml::Value::Boolean(bw));
            self.save_globals();
        }
        if bw {
            crate::print::black_and_white(&mut rows);
        }
        if let Err(e) = crate::winprint::print(hwnd as _, &mut self.print_setup, &title, &rows, cols, &font.family, font.size_pt) {
            self.banners.set("print", format!("Printing failed: {e}"));
        }
    }

    /// Whether a checkable menu item shows its check mark.
    pub(super) fn is_checked(&self, selector: &str) -> bool {
        if let Some(on) = self.broadcast_checked(selector) {
            return on;
        }
        match selector {
            "debugLogging:" => crate::dlog::is_on(),
            "toggleSelectionRespectsSoftBoundaries:" => self.globals.bool("Selection Respects Soft Boundaries"),
            "togglePinHotkeyWindow:" => self.hotkey_pinned(),
            "toggleTmuxPausePane:" => self.tmux_pane_paused(),
            "toggleAutoCommandHistory:" => self.globals.bool("AutoCommandHistory"),
            "toggleShowClippings:" => self.clippings_shown(),
            "toggleShowInlineChat:" => self.inline_shown(),
            "toggleAutoComposer:" => self.auto_composer_checked(),
            "toggleToolbeltVisibility:" => self.belt.visible,
            "toggleUseTransparency:" | "toggleDisableTransparencyForActiveWindow:" => {
                crate::transparency::checks(self.window_uses_transparency(), self.globals.bool(crate::transparency::DISABLE_KEY_WINDOW_KEY)).iter().any(|(s, on)| *s == selector && *on)
            }
            "toggleLockSplitPaneWidth:" => self.width_lock_checked(),
            "toggleFullScreenTabBar:" => self.globals.bool("ShowFullScreenTabBar"),
            "toggleCopyMode:" => self.focused_id().is_some_and(|id| self.copy_modes.contains_key(&id) || self.is_browser_copy_mode(id)),
            "toggleShowTimestamps:" => self.focused_id().is_some_and(|id| self.stamps_on(id)),
            "editSessionNote:" => self.focused_id().is_some_and(|id| self.note_win.as_ref().is_some_and(|(_, p, n)| *p == id && n.is_none())),
            "toggleCursorGuide:" => self.focused_id().is_some_and(|id| self.guide_on(id)),
            "showHideNotes:" => self.focused_id().and_then(|id| self.panes.get(&id)).is_some_and(|p| p.own().notes.by_id.values().any(|n| !n.hidden)),
            "toggleBufferInput:" => self.focused_id().and_then(|id| self.panes.get(&id)).is_some_and(|p| p.buffer.on),
            "toggleNotifyOnStatusChange:" => self.status_armed,
            "toggleEnableTriggersInInteractiveApps:" => self.focused_id().and_then(|id| self.panes.get(&id)).is_some_and(|p| p.own().triggers.prefs.interactive),
            _ => false,
        }
    }

    pub(super) fn selection_pos(&self, id: PaneId) -> Option<(Pos, Pos)> {
        let p = self.panes.get(&id)?;
        let term = p.screen.term();
        let r = term.selection.as_ref()?.to_range(term)?;
        Some((to_pos(&p.screen, r.start), to_pos(&p.screen, r.end)))
    }

    fn set_simple_selection(&mut self, id: PaneId, a: Pos, b: Pos) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let mut sel = Selection::new(SelectionType::Simple, to_point(&p.screen, a), Side::Left);
        sel.update(to_point(&p.screen, b), Side::Right);
        p.set_selection(Some(sel), crate::seltext::Mode::Character);
    }

    /// The selection tools' menu commands. Returns false for other selectors.
    pub(super) fn run_seltools_command(&mut self, selector: &str) -> bool {
        let Some(id) = self.focused_id() else { return false };
        match selector {
            "replaceSelectionWithPrettyPrintedJSON:" => self.replace_selection(id, seltools::pretty_json),
            "replaceSelectionWithBase64Encoded:" => self.replace_selection(id, |t| Ok(seltools::base64_encode(t))),
            "replaceSelectionWithBase64Decoded:" => self.replace_selection(id, seltools::base64_decode),
            "clearToStartOfSelection:" => self.clear_to_start_of_selection(id),
            "toggleBufferInput:" => {
                if let Some(p) = self.panes.get_mut(&id) {
                    let on = !p.buffer.on;
                    p.set_buffering(on);
                }
            }
            // With a dropdown composer holding the keyboard these do nothing; an Auto
            // Composer forwards them to the terminal's selection (`CTV:348-398`).
            "copyWithStyles:" if self.composer_blocks_styled_copy() => {}
            "copyWithStyles:" => self.copy_with_styles(id),
            "runPageLayout:" => {
                if let Some(hwnd) = self.hwnd() {
                    crate::winprint::page_setup(hwnd as _, &mut self.print_setup);
                }
            }
            "copyWithControlSequences:" if self.composer_blocks_styled_copy() => {}
            "copyWithControlSequences:" => {
                if self.copy_porthole_selection(id, super::porthole_ui::CopyAs::ControlSequences) {
                    return true;
                }
                let text = self.panes.get(&id).map(|p| p.sgr_copy());
                if let (Some(text), Some(cb)) = (text, self.clipboard.as_mut()) {
                    if let Err(e) = cb.set_text(text) {
                        self.banners.set("clipboard", format!("Copy failed: {e}"));
                    }
                }
            }
            "toggleSelectionRespectsSoftBoundaries:" => {
                let on = !self.globals.bool("Selection Respects Soft Boundaries");
                self.globals.set("Selection Respects Soft Boundaries", toml::Value::Boolean(on));
                self.save_globals();
            }
            _ => return false,
        }
        self.sync_checks();
        self.redraw();
        true
    }

    /// Replace Selection: the selected whole lines are replaced in Tandem's buffer (not
    /// sent to the shell), as iTerm2 does.
    fn replace_selection(&mut self, id: PaneId, f: impl Fn(&str) -> Result<String, String>) {
        let Some((a, b)) = self.selection_pos(id) else { return self.banners.set("action", "Select whole lines first.") };
        let Some(p) = self.panes.get_mut(&id) else { return };
        if p.live.is_some() {
            return self.banners.set("action", "Replace Selection does not work in a Filter or Zoom view.");
        }
        if p.screen.term().mode().contains(TermMode::ALT_SCREEN) {
            return self.banners.set("action", "Replace Selection does not work while a full-screen app runs.");
        }
        let (pa, pb) = (to_point(&p.screen, a), to_point(&p.screen, b));
        let grid = p.screen.term().grid();
        let cols = grid.columns();
        let row = &grid[pb.line];
        let wraps = row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        let last_end = (0..cols).rev().find(|&c| row[Column(c)].c != ' ').unwrap_or(0);
        if wraps || !seltools::whole_lines(a, b, last_end) {
            return self.banners.set("action", "Replace Selection needs whole lines, from the start of the first.");
        }
        let text = p.screen.term().bounds_to_string(pa, Point::new(pb.line, Column(cols - 1)));
        // A visual (bidi) selection that skips cells inside its lines is not whole lines.
        if !p.screen.visual_selection_is_whole_lines(pa, pb) {
            return self.banners.set("action", "Replace Selection needs whole lines, from the start of the first.");
        }
        match f(text.trim_end_matches('\n')) {
            Err(e) => self.banners.set("action", e),
            Ok(new) => {
                let lines: Vec<String> = new.lines().map(str::to_string).collect();
                let rows = p.screen.text_rows(&lines);
                p.screen.splice_rows(pa.line.0, pb.line.0, rows);
            }
        }
    }

    /// Clear to Start of Selection: everything above the selection's first line goes.
    fn clear_to_start_of_selection(&mut self, id: PaneId) {
        let Some((a, _)) = self.selection_pos(id) else { return };
        let Some(p) = self.panes.get_mut(&id) else { return };
        if p.live.is_some() || p.screen.term().mode().contains(TermMode::ALT_SCREEN) {
            return self.banners.set("action", "Clear to Start of Selection works on the pane's own screen, not a view or a full-screen app.");
        }
        let first = to_point(&p.screen, a).line.0;
        let top = -(p.screen.term().grid().history_size() as i32);
        if first > top {
            p.screen.splice_rows(top, first - 1, Vec::new());
        }
    }

    /// Key actions 45 to 48: one end of the selection moves `count` cells.
    pub(super) fn move_selection_end(&mut self, id: PaneId, action: i64, count: &str) {
        let n = count.trim().parse::<usize>().unwrap_or(1);
        let Some(sel) = self.selection_pos(id) else { return };
        let Some(p) = self.panes.get(&id) else { return };
        let grid = p.screen.term().grid();
        let last_row = grid.history_size() + grid.screen_lines() - 1;
        let (which, left) = match action {
            45 => (End::End, true),
            46 => (End::End, false),
            47 => (End::Start, true),
            _ => (End::Start, false),
        };
        let (a, b) = seltools::move_endpoint(sel, which, left, n, grid.columns(), last_row);
        self.set_simple_selection(id, a, b);
        self.copy_on_selection(id);
    }
}
