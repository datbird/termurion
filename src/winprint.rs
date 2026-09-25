//! Windows printing for Shell > Print and Page Setup: the page setup and print dialogs,
//! then each page drawn with GDI in the profile's font: a white page, text runs in their
//! colors with their backgrounds behind them, no header or footer.

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontW, DEFAULT_CHARSET, DeleteDC, DeleteObject, ETO_OPAQUE, ExtTextOutW, FW_BOLD, FW_NORMAL, GetDeviceCaps, GetTextMetricsW, HDC, HFONT,
    HORZRES, LOGPIXELSX, LOGPIXELSY, OPAQUE, PHYSICALOFFSETX, PHYSICALOFFSETY, SelectObject, SetBkColor, SetBkMode, SetTextColor, TEXTMETRICW, TRANSPARENT,
    VERTRES,
};
use windows_sys::Win32::Storage::Xps::{DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
use windows_sys::Win32::UI::Controls::Dialogs::{
    PAGESETUPDLGW, PD_NOCURRENTPAGE, PD_NOPAGENUMS, PD_NOSELECTION, PD_RESULT_PRINT, PD_RETURNDC, PRINTDLGEXW, PSD_INTHOUSANDTHSOFINCHES, PSD_MARGINS,
    PageSetupDlgW, PrintDlgExW, START_PAGE_GENERAL,
};

use crate::styled_copy::Row;

/// The printer, paper, and margins chosen in Page Setup, kept while Tandem runs (as
/// iTerm2's shared print info is).
pub struct Setup {
    dev_mode: *mut core::ffi::c_void,
    dev_names: *mut core::ffi::c_void,
    /// Margins in thousandths of an inch.
    margins: RECT,
}

impl Default for Setup {
    fn default() -> Self {
        // Half an inch all round until Page Setup says otherwise.
        Setup { dev_mode: std::ptr::null_mut(), dev_names: std::ptr::null_mut(), margins: RECT { left: 500, top: 500, right: 500, bottom: 500 } }
    }
}

/// Shell > Page Setup...
pub fn page_setup(owner: HWND, setup: &mut Setup) {
    let mut p: PAGESETUPDLGW = unsafe { std::mem::zeroed() };
    p.lStructSize = std::mem::size_of::<PAGESETUPDLGW>() as u32;
    p.hwndOwner = owner;
    p.hDevMode = setup.dev_mode;
    p.hDevNames = setup.dev_names;
    p.Flags = PSD_MARGINS | PSD_INTHOUSANDTHSOFINCHES;
    p.rtMargin = setup.margins;
    if unsafe { PageSetupDlgW(&mut p) } != 0 {
        setup.dev_mode = p.hDevMode;
        setup.dev_names = p.hDevNames;
        setup.margins = p.rtMargin;
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn colorref(c: alacritty_terminal::vte::ansi::Rgb) -> u32 {
    c.r as u32 | (c.g as u32) << 8 | (c.b as u32) << 16
}

/// The print dialog, then the pages. Ok(false) when the user cancelled.
pub fn print(owner: HWND, setup: &mut Setup, title: &str, rows: &[Row], cols: usize, family: &str, size_pt: f32) -> Result<bool, String> {
    let mut p: PRINTDLGEXW = unsafe { std::mem::zeroed() };
    p.lStructSize = std::mem::size_of::<PRINTDLGEXW>() as u32;
    p.hwndOwner = owner;
    p.hDevMode = setup.dev_mode;
    p.hDevNames = setup.dev_names;
    p.Flags = PD_RETURNDC | PD_NOPAGENUMS | PD_NOSELECTION | PD_NOCURRENTPAGE;
    p.nStartPage = START_PAGE_GENERAL;
    let hr = unsafe { PrintDlgExW(&mut p) };
    if hr < 0 {
        return Err(format!("the print dialog failed (0x{:08x})", hr as u32));
    }
    setup.dev_mode = p.hDevMode;
    setup.dev_names = p.hDevNames;
    let dc = p.hDC;
    if p.dwResultAction != PD_RESULT_PRINT {
        if !dc.is_null() {
            unsafe { DeleteDC(dc) };
        }
        return Ok(false);
    }
    if dc.is_null() {
        return Err("the printer gave no drawing surface".into());
    }
    let result = draw_pages(dc, setup, title, rows, cols, family, size_pt);
    unsafe { DeleteDC(dc) };
    result.map(|_| true)
}

struct Fonts {
    dc: HDC,
    family: Vec<u16>,
    height: i32,
    made: Vec<((bool, bool, bool), HFONT)>,
}

impl Fonts {
    fn get(&mut self, bold: bool, italic: bool, underline: bool) -> HFONT {
        if let Some((_, f)) = self.made.iter().find(|(k, _)| *k == (bold, italic, underline)) {
            return *f;
        }
        let weight = if bold { FW_BOLD } else { FW_NORMAL };
        let f = unsafe {
            CreateFontW(-self.height, 0, 0, 0, weight as i32, italic as u32, underline as u32, 0, DEFAULT_CHARSET as u32, 0, 0, 0, 0, self.family.as_ptr())
        };
        self.made.push(((bold, italic, underline), f));
        f
    }

    /// The width and height of a cell in this font.
    fn cell(&mut self) -> (f32, f32) {
        let f = self.get(false, false, false);
        unsafe { SelectObject(self.dc, f) };
        let mut tm: TEXTMETRICW = unsafe { std::mem::zeroed() };
        unsafe { GetTextMetricsW(self.dc, &mut tm) };
        (tm.tmAveCharWidth.max(1) as f32, tm.tmHeight.max(1) as f32)
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for (_, f) in &self.made {
            unsafe { DeleteObject(*f) };
        }
    }
}

fn draw_pages(dc: HDC, setup: &Setup, title: &str, rows: &[Row], cols: usize, family: &str, size_pt: f32) -> Result<(), String> {
    let caps = |i| unsafe { GetDeviceCaps(dc, i) } as f32;
    let (dpi_x, dpi_y) = (caps(LOGPIXELSX as i32), caps(LOGPIXELSY as i32));
    let (off_x, off_y) = (caps(PHYSICALOFFSETX as i32), caps(PHYSICALOFFSETY as i32));
    let (w, h) = (caps(HORZRES as i32), caps(VERTRES as i32));
    let m = setup.margins;
    // Margins are from the paper's edge; the drawing surface starts at the printable edge.
    let left = (m.left as f32 * dpi_x / 1000.0 - off_x).max(0.0);
    let top = (m.top as f32 * dpi_y / 1000.0 - off_y).max(0.0);
    let right = (m.right as f32 * dpi_x / 1000.0 - off_x).max(0.0);
    let bottom = (m.bottom as f32 * dpi_y / 1000.0 - off_y).max(0.0);
    let page = ((w - left - right).max(1.0), (h - top - bottom).max(1.0));
    let base = (size_pt * dpi_y / 72.0).round() as i32;
    let mut fonts = Fonts { dc, family: wide(family), height: base.max(1), made: Vec::new() };
    let layout = crate::print::layout(rows.len(), cols, fonts.cell(), page);
    if layout.scale < 1.0 {
        drop(fonts);
        fonts = Fonts { dc, family: wide(family), height: ((base as f32 * layout.scale).round() as i32).max(1), made: Vec::new() };
    }
    let (cw, ch) = fonts.cell();
    let name = wide(title);
    let doc = DOCINFOW { cbSize: std::mem::size_of::<DOCINFOW>() as i32, lpszDocName: name.as_ptr(), lpszOutput: std::ptr::null(), lpszDatatype: std::ptr::null(), fwType: 0 };
    if unsafe { StartDocW(dc, &doc) } <= 0 {
        return Err(format!("the printer did not start the document ({})", std::io::Error::last_os_error()));
    }
    const WHITE: u32 = 0x00ff_ffff;
    for range in &layout.pages {
        if unsafe { StartPage(dc) } <= 0 {
            unsafe { EndDoc(dc) };
            return Err(format!("the printer did not start a page ({})", std::io::Error::last_os_error()));
        }
        for (i, row) in rows[range.clone()].iter().enumerate() {
            let y = (top + i as f32 * ch) as i32;
            let mut col = 0usize;
            for run in &row.runs {
                let font = fonts.get(run.bold, run.italic, run.underline || run.link.is_some());
                let text: Vec<u16> = run.text.encode_utf16().collect();
                // One advance per character, by its cell width, so wide characters and
                // fallback fonts stay on the grid.
                let mut dx: Vec<i32> = Vec::with_capacity(text.len());
                let mut cells = 0;
                for c in run.text.chars() {
                    let n = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                    cells += n;
                    dx.push((n as f32 * cw).round() as i32);
                    if c.len_utf16() == 2 {
                        dx.push(0);
                    }
                }
                let x = (left + col as f32 * cw) as i32;
                let rect = RECT { left: x, top: y, right: x + (cells as f32 * cw).round() as i32, bottom: y + ch.round() as i32 };
                let bg = colorref(run.bg);
                unsafe {
                    SelectObject(dc, font);
                    SetTextColor(dc, colorref(run.fg));
                    if bg == WHITE {
                        SetBkMode(dc, TRANSPARENT as i32);
                    } else {
                        SetBkMode(dc, OPAQUE as i32);
                        SetBkColor(dc, bg);
                    }
                    ExtTextOutW(dc, x, y, if bg == WHITE { 0 } else { ETO_OPAQUE }, &rect, text.as_ptr(), text.len() as u32, dx.as_ptr());
                }
                col += cells;
            }
        }
        unsafe { EndPage(dc) };
    }
    unsafe { EndDoc(dc) };
    Ok(())
}
