//! The save dialogs of Shell > Save Contents and Save Selected Text, with iTerm2's accessory
//! (`iTermSavePanelFileFormatAccessory`, `PTYTextView.m:4382-4440`): an "Include timestamps"
//! check box that remembers its state, and for Save Contents the plain text and RTF formats.
//!
//! macOS puts an accessory view in the save panel. Windows has the same thing in the common
//! item dialog (`IFileDialogCustomize`), so the check box sits in the dialog and the format
//! is the dialog's own "Save as type" list.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::vte::ansi::Rgb;

use crate::stamps::Parts;

/// The check box's label (`iTermSavePanelFileFormatAccessory.xib`).
pub const INCLUDE_TIMESTAMPS: &str = "Include timestamps";
/// Its remembered state (`iTermSaveWithTimestampsUserDefaultsKey`).
pub const TIMESTAMPS_KEY: &str = "NoSyncSaveWithTimestamps";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Text,
    Rtf,
}

/// What the person chose.
#[derive(Debug)]
pub struct Picked {
    pub path: crate::localfs::LocalPath,
    pub timestamps: bool,
    pub format: Format,
}

/// `[yyyy-MM-dd HH:mm:ss] ` for a line written at Unix time `t`; a line with no recorded time
/// gets blanks of the same width (`formattedTimestampForLine:`, `iTermTextExtractor.m:1637`).
pub fn stamp_prefix(t: u32) -> String {
    if t == 0 {
        return format!("[{}] ", " ".repeat(19));
    }
    let p = crate::stamps::local(t as u64);
    format!("[{:04}-{:02}-{:02} {:02}:{:02}:{:02}] ", p.year, p.month, p.day, p.hour, p.minute, p.second)
}

/// `text` with a stamp before each of its lines; `times[i]` is line `i`'s time (0 or missing:
/// none recorded). The empty piece after a final newline gets none.
pub fn stamp_lines(text: &str, times: &[u32]) -> String {
    let mut out = String::with_capacity(text.len() + text.lines().count() * 23);
    for (i, line) in text.split_inclusive('\n').enumerate() {
        out.push_str(&stamp_prefix(times.get(i).copied().unwrap_or(0)));
        out.push_str(line);
    }
    out
}

fn twelve(p: &Parts) -> (u16, &'static str) {
    let h = p.hour % 12;
    (if h == 0 { 12 } else { h }, if p.hour < 12 { "AM" } else { "PM" })
}

/// Save Contents' suggested name (`PseudoTerminal.m:13752-13772`): "Termurion Session Oct 17, 2026
/// at 03.04.05 PM.txt". The time uses dots, as iTerm2 turns its colons into.
pub fn session_filename(now: &Parts) -> String {
    let (h, ampm) = twelve(now);
    format!("Termurion Session {} at {:02}.{:02}.{:02} {ampm}.txt", crate::stamps::month_day_year(now), h, now.minute, now.second)
}

/// Save Selected Text's suggested name (`PTYTextView.m:4399-4412`): "Log at 2026-10-17 03-04-05 PM.txt".
pub fn log_filename(now: &Parts) -> String {
    let (h, ampm) = twelve(now);
    format!("Log at {:04}-{:02}-{:02} {:02}-{:02}-{:02} {ampm}.txt", now.year, now.month, now.day, h, now.minute, now.second)
}

/// The grid lines' write times for the lines of `Screen::all_text`, oldest first.
pub fn all_text_times(screen: &crate::screen::Screen) -> Vec<u32> {
    let grid = screen.term().grid();
    let top = -(grid.history_size() as i32);
    (top..grid.screen_lines() as i32).map(|l| screen.row_time(l)).collect()
}

/// RTF rows with a stamp run before each row that starts a line (a row after one that wraps
/// continues it); `times[i]` is row `i`'s time. The stamp uses the pane's plain colors.
pub fn stamp_rows(rows: &mut [crate::styled_copy::Row], times: &[u32], fg: Rgb, bg: Rgb) {
    let mut continues = false;
    for (i, row) in rows.iter_mut().enumerate() {
        if !continues {
            let text = stamp_prefix(times.get(i).copied().unwrap_or(0));
            row.runs.insert(0, crate::styled_copy::Run { text, fg, bg, bold: false, italic: false, underline: false, link: None, default_bg: true });
        }
        continues = !row.hard_end;
    }
}

#[cfg(windows)]
mod dialog {
    use super::*;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FileSaveDialog, IFileDialogCustomize, IFileSaveDialog, SIGDN_FILESYSPATH};
    use windows::core::{Interface, PCWSTR};

    const CHECK_ID: u32 = 1001;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// Shows the dialog. `formats` is the types offered, the first the default. None when cancelled.
    pub fn pick(hwnd: Option<isize>, name: &str, formats: &[Format], timestamps: bool) -> Option<Picked> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let options = dialog.GetOptions().ok()?;
            dialog.SetOptions(options | FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM).ok()?;
            let names: Vec<(Vec<u16>, Vec<u16>)> = formats
                .iter()
                .map(|f| match f {
                    Format::Text => (wide("Plain text document"), wide("*.txt")),
                    Format::Rtf => (wide("Rich text format (RTF)"), wide("*.rtf")),
                })
                .collect();
            let specs: Vec<COMDLG_FILTERSPEC> = names.iter().map(|(n, s)| COMDLG_FILTERSPEC { pszName: PCWSTR(n.as_ptr()), pszSpec: PCWSTR(s.as_ptr()) }).collect();
            dialog.SetFileTypes(&specs).ok()?;
            dialog.SetFileTypeIndex(1).ok()?;
            dialog.SetDefaultExtension(PCWSTR(wide("txt").as_ptr())).ok()?;
            dialog.SetFileName(PCWSTR(wide(name).as_ptr())).ok()?;
            let custom: IFileDialogCustomize = dialog.cast().ok()?;
            custom.AddCheckButton(CHECK_ID, PCWSTR(wide(INCLUDE_TIMESTAMPS).as_ptr()), timestamps).ok()?;
            let owner = hwnd.map_or(HWND(std::ptr::null_mut()), |h| HWND(h as *mut std::ffi::c_void));
            // Cancel is an error result.
            dialog.Show(Some(owner)).ok()?;
            let item = dialog.GetResult().ok()?;
            let raw = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let path = raw.to_string().ok();
            CoTaskMemFree(Some(raw.0 as *const std::ffi::c_void));
            let checked = custom.GetCheckButtonState(CHECK_ID).map(|b| b.as_bool()).unwrap_or(false);
            let index = dialog.GetFileTypeIndex().unwrap_or(1) as usize;
            let format = formats.get(index.saturating_sub(1)).copied().unwrap_or(Format::Text);
            Some(Picked { path: crate::localfs::save_panel_file(std::path::PathBuf::from(path?))?, timestamps: checked, format })
        }
    }
}

/// Shows the save dialog with the accessory. `timestamps` is the check box's starting state.
#[cfg(windows)]
pub fn pick(hwnd: Option<isize>, name: &str, formats: &[Format], timestamps: bool) -> Option<Picked> {
    dialog::pick(hwnd, name, formats, timestamps)
}

#[cfg(not(windows))]
pub fn pick(_hwnd: Option<isize>, name: &str, _formats: &[Format], _timestamps: bool) -> Option<Picked> {
    rfd::FileDialog::new().set_file_name(name).save_file().and_then(crate::localfs::save_panel_file).map(|path| Picked { path, timestamps: false, format: Format::Text })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(h: u16) -> Parts {
        Parts { year: 2026, month: 10, day: 17, weekday: 6, hour: h, minute: 4, second: 5 }
    }

    #[test]
    fn a_line_without_a_time_gets_blanks_as_wide_as_a_stamp() {
        let blank = stamp_prefix(0);
        assert_eq!(blank, format!("[{}] ", " ".repeat(19)));
        // The same width as a real stamp: "[2026-10-17 03:04:05] ".
        assert_eq!(blank.chars().count(), 22);
        assert!(stamp_prefix(1_000_000_000).starts_with("[2001-"));
        assert_eq!(stamp_prefix(1_000_000_000).chars().count(), 22);
    }

    #[test]
    fn each_line_gets_its_own_stamp_and_a_final_newline_none() {
        let out = stamp_lines("a\nb\n", &[0, 0]);
        let blank = stamp_prefix(0);
        assert_eq!(out, format!("{blank}a\n{blank}b\n"));
        // Fewer times than lines: the rest are blank.
        assert_eq!(stamp_lines("a\nb", &[0]), format!("{blank}a\n{blank}b"));
        assert_eq!(stamp_lines("", &[1]), "");
    }

    #[test]
    fn suggested_names_follow_iterm2() {
        assert_eq!(session_filename(&parts(15)), "Termurion Session Oct 17, 2026 at 03.04.05 PM.txt");
        assert_eq!(session_filename(&parts(0)), "Termurion Session Oct 17, 2026 at 12.04.05 AM.txt");
        assert_eq!(log_filename(&parts(15)), "Log at 2026-10-17 03-04-05 PM.txt");
        assert!(!session_filename(&parts(9)).contains(':') && !log_filename(&parts(9)).contains(':'));
    }

    #[test]
    fn rtf_rows_get_a_stamp_only_where_a_line_starts() {
        let c = Rgb { r: 1, g: 2, b: 3 };
        let run = |t: &str| crate::styled_copy::Run { text: t.into(), fg: c, bg: c, bold: false, italic: false, underline: false, link: None, default_bg: true };
        let mut rows = vec![
            crate::styled_copy::Row { runs: vec![run("one")], hard_end: false },
            crate::styled_copy::Row { runs: vec![run("two")], hard_end: true },
            crate::styled_copy::Row { runs: vec![run("three")], hard_end: true },
        ];
        stamp_rows(&mut rows, &[0, 0, 0], c, c);
        assert_eq!(rows[0].runs[0].text, stamp_prefix(0));
        assert_eq!(rows[1].runs.len(), 1, "the wrapped second row continues the first line");
        assert_eq!(rows[2].runs[0].text, stamp_prefix(0));
        assert_eq!(rows[2].runs[1].text, "three");
    }

    #[test]
    fn all_text_times_has_a_time_for_every_line_of_the_text() {
        let mut s = crate::screen::Screen::new(8, 3);
        s.feed(b"one\r\ntwo\r\nthree\r\nfour");
        let times = all_text_times(&s);
        let text = s.all_text();
        assert!(times.len() >= text.lines().count());
        let stamped = stamp_lines(&text, &times);
        assert_eq!(stamped.lines().count(), text.lines().count());
        assert!(stamped.lines().all(|l| l.starts_with('[')));
    }

    #[test]
    fn the_check_box_and_its_key_are_iterms() {
        assert_eq!(INCLUDE_TIMESTAMPS, "Include timestamps");
        assert_eq!(TIMESTAMPS_KEY, "NoSyncSaveWithTimestamps");
    }
}
