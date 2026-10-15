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

/// The print dialog's extra page: iTerm2's print accessory (`iTermPrintAccessoryViewController.xib`),
/// one checkbox, "Black and white". The page's checkbox writes here as it is clicked; `print` sets
/// it before the dialog opens and reads it after.
static BLACK_AND_WHITE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// The checkbox's text (`iTermPrintAccessoryViewController.xib:23`) and the page's tab title.
pub const BLACK_AND_WHITE_LABEL: &str = "Black and white";
pub const PAGE_TITLE: &str = "Options";
const ID_BLACK_AND_WHITE: i32 = 1001;

/// The dialog template of the page in memory: a child dialog holding one automatic checkbox.
/// (`DLGTEMPLATE` then one `DLGITEMTEMPLATE`, words in little endian; the item starts on a
/// four-byte boundary.)
pub fn page_template() -> Vec<u16> {
    const WS_CHILD: u32 = 0x4000_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const WS_TABSTOP: u32 = 0x0001_0000;
    const DS_3DLOOK: u32 = 0x0004;
    const DS_CONTROL: u32 = 0x0400;
    const BS_AUTOCHECKBOX: u32 = 0x0003;
    let mut w: Vec<u16> = Vec::new();
    let push32 = |w: &mut Vec<u16>, v: u32| {
        w.push((v & 0xffff) as u16);
        w.push((v >> 16) as u16);
    };
    // DLGTEMPLATE: style, extended style, item count, x, y, cx, cy, menu, class, title.
    push32(&mut w, WS_CHILD | WS_VISIBLE | DS_CONTROL | DS_3DLOOK);
    push32(&mut w, 0);
    w.extend([1, 8, 8, 200, 40]);
    w.extend([0, 0, 0]);
    // DLGITEMTEMPLATE: style, extended style, x, y, cx, cy, id, class (button), text, no data.
    push32(&mut w, WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX);
    push32(&mut w, 0);
    w.extend([8, 8, 150, 12, ID_BLACK_AND_WHITE as u16]);
    w.extend([0xffff, 0x0080]);
    w.extend(BLACK_AND_WHITE_LABEL.encode_utf16());
    w.push(0);
    w.push(0);
    w
}

unsafe extern "system" fn page_proc(hwnd: HWND, msg: u32, wparam: usize, _lparam: isize) -> isize {
    use std::sync::atomic::Ordering::SeqCst;
    use windows_sys::Win32::UI::Controls::{BST_CHECKED, BST_UNCHECKED, CheckDlgButton, IsDlgButtonChecked};
    use windows_sys::Win32::UI::WindowsAndMessaging::{BN_CLICKED, WM_COMMAND, WM_INITDIALOG};
    match msg {
        WM_INITDIALOG => {
            let state = if BLACK_AND_WHITE.load(SeqCst) { BST_CHECKED } else { BST_UNCHECKED };
            unsafe { CheckDlgButton(hwnd, ID_BLACK_AND_WHITE, state) };
            1
        }
        WM_COMMAND if (wparam & 0xffff) as i32 == ID_BLACK_AND_WHITE && (wparam >> 16) as u32 == BN_CLICKED => {
            BLACK_AND_WHITE.store(unsafe { IsDlgButtonChecked(hwnd, ID_BLACK_AND_WHITE) } == BST_CHECKED, SeqCst);
            0
        }
        _ => 0,
    }
}

/// The printer, paper, and margins chosen in Page Setup, kept while Termurion runs (as
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
/// `black_and_white` is the box's state going in and coming out; when it is checked after the
/// dialog the colors are dropped from `rows` (`print::black_and_white`).
#[allow(clippy::too_many_arguments)]
pub fn print(owner: HWND, setup: &mut Setup, title: &str, rows: &mut [Row], black_and_white: &mut bool, cols: usize, family: &str, size_pt: f32) -> Result<bool, String> {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Controls::{CreatePropertySheetPageW, PROPSHEETPAGEW, PSP_DLGINDIRECT, PSP_USETITLE};
    BLACK_AND_WHITE.store(*black_and_white, std::sync::atomic::Ordering::SeqCst);
    // The page, when Windows makes it; without it the dialog is the plain one and the stored choice stands.
    let mut template = page_template();
    let title_w = wide(PAGE_TITLE);
    let mut page: PROPSHEETPAGEW = unsafe { std::mem::zeroed() };
    page.dwSize = std::mem::size_of::<PROPSHEETPAGEW>() as u32;
    page.dwFlags = PSP_DLGINDIRECT | PSP_USETITLE;
    page.hInstance = unsafe { GetModuleHandleW(std::ptr::null()) };
    page.Anonymous1.pResource = template.as_mut_ptr().cast();
    page.pszTitle = title_w.as_ptr();
    page.pfnDlgProc = Some(page_proc);
    let mut handles = [unsafe { CreatePropertySheetPageW(&mut page) }];
    let mut p: PRINTDLGEXW = unsafe { std::mem::zeroed() };
    if !handles[0].is_null() {
        p.nPropertyPages = 1;
        p.lphPropertyPages = handles.as_mut_ptr();
    }
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
    if !handles[0].is_null() {
        *black_and_white = BLACK_AND_WHITE.load(std::sync::atomic::Ordering::SeqCst);
    }
    if *black_and_white {
        crate::print::black_and_white(rows);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The print dialog's extra page holds one checkbox named as iTerm2's accessory names it, and
    /// the template is laid out as a dialog template must be (the item on a four-byte boundary).
    #[test]
    fn the_print_page_holds_the_black_and_white_box() {
        assert_eq!(BLACK_AND_WHITE_LABEL, "Black and white");
        let t = page_template();
        // One item; the header is 12 words, so the item starts at byte 24.
        assert_eq!(t[4], 1, "one control");
        let item = 12;
        let style = u32::from(t[item]) | u32::from(t[item + 1]) << 16;
        assert_eq!(style & 0xf, 3, "an automatic checkbox");
        assert_eq!(&t[item + 9..item + 11], &[0xffff, 0x0080], "the button class");
        let text: Vec<u16> = BLACK_AND_WHITE_LABEL.encode_utf16().collect();
        assert_eq!(&t[item + 11..item + 11 + text.len()], &text[..]);
        assert_eq!((item * 2) % 4, 0);
    }
}
