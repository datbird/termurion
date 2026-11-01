//! The quit question with a disclosure (iTerm2's `iTermDisclosableView` accessory,
//! `iTermApplicationDelegate.m:1049-1071`): "Quit Termurion?", what happens to the sessions, OK and
//! Cancel, and a collapsed "Why am I being prompted?" that opens the reasons. A message box has no
//! room for an accessory, so this is a small modal dialog of its own, run the way a message box
//! runs (`msgbox::run_modal`: the watcher that raises it, the paint guard). If Windows refuses to
//! make the dialog, the plain box with the reasons inline asks instead.

use std::sync::atomic::{AtomicIsize, Ordering};

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, DialogBoxIndirectParamW, EndDialog, GWLP_USERDATA, GetDlgItem, GetWindowLongPtrW, IDCANCEL, IDOK, MapDialogRect, MoveWindow, SW_HIDE, SW_SHOW, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOZORDER, SetDlgItemTextW, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, WM_COMMAND, WM_INITDIALOG,
};

/// The disclosure's prompt (`QuitAlert.WhyPrompt`); its lead-in is `close_rules::quit_parts`'.
pub(super) const WHY_PROMPT: &str = "Why am I being prompted?";

const ID_TEXT: i32 = 100;
const ID_WHY: i32 = 101;
const ID_DETAIL: i32 = 102;
/// Dialog units: the client width, the margin, a text line, the button size.
const W: i32 = 250;
const M: i32 = 7;
const LINE: i32 = 8;
const BTN_W: i32 = 50;
const BTN_H: i32 = 14;
/// Characters of the dialog font that fit across the text, as a wrap estimate.
const WRAP: usize = 50;
const DETAIL_MAX: i32 = 96;

/// How many lines `text` takes at about `WRAP` characters across.
pub(super) fn lines_for(text: &str) -> i32 {
    text.split('\n').map(|l| (l.chars().count().max(1) as i32 + WRAP as i32 - 1) / WRAP as i32).sum::<i32>().max(1)
}

/// Where the parts sit in dialog units for a text of `text_lines` lines, a detail of
/// `detail_lines` when open: (disclosure y, detail y and height, buttons y, client height).
pub(super) fn layout(text_lines: i32, detail_lines: i32, expanded: bool) -> (i32, Option<(i32, i32)>, i32, i32) {
    let why_y = M + LINE * text_lines + 6;
    let (detail, below) = if expanded {
        let h = (LINE * detail_lines + 8).min(DETAIL_MAX);
        (Some((why_y + 16, h)), why_y + 16 + h)
    } else {
        (None, why_y + 12)
    };
    let buttons_y = below + 8;
    (why_y, detail, buttons_y, buttons_y + BTN_H + M)
}

struct Ctx {
    text: String,
    prompt: String,
    detail: String,
    expanded: bool,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn align4(w: &mut Vec<u16>) {
    if w.len() % 2 == 1 {
        w.push(0);
    }
}

fn push32(w: &mut Vec<u16>, v: u32) {
    w.push((v & 0xffff) as u16);
    w.push((v >> 16) as u16);
}

fn item(w: &mut Vec<u16>, style: u32, ex: u32, rect: [i32; 4], id: i32, class: u16, text: &str) {
    align4(w);
    push32(w, style);
    push32(w, ex);
    w.extend(rect.iter().map(|v| *v as u16));
    w.push(id as u16);
    w.extend([0xffff, class]);
    w.extend(wide(text));
    w.push(0);
}

/// The dialog template: a modal frame, the font, and the five controls at their closed positions.
fn template(title: &str, topmost: bool, ctx: &Ctx) -> Vec<u16> {
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_CAPTION: u32 = 0x00c0_0000;
    const WS_SYSMENU: u32 = 0x0008_0000;
    const WS_CHILD: u32 = 0x4000_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const WS_TABSTOP: u32 = 0x0001_0000;
    const WS_VSCROLL: u32 = 0x0020_0000;
    const WS_EX_TOPMOST: u32 = 0x0000_0008;
    const WS_EX_CLIENTEDGE: u32 = 0x0000_0200;
    const DS_SETFONT: u32 = 0x40;
    const DS_MODALFRAME: u32 = 0x80;
    const DS_CENTER: u32 = 0x800;
    const BS_DEFPUSHBUTTON: u32 = 1;
    const BS_LEFT: u32 = 0x100;
    const ES_MULTILINE: u32 = 4;
    const ES_AUTOVSCROLL: u32 = 0x40;
    const ES_READONLY: u32 = 0x800;
    let (why_y, _, buttons_y, height) = layout(lines_for(&ctx.text), lines_for(&ctx.detail), false);
    let mut w: Vec<u16> = Vec::new();
    push32(&mut w, WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_SETFONT | DS_MODALFRAME | DS_CENTER);
    push32(&mut w, if topmost { WS_EX_TOPMOST } else { 0 });
    w.extend([5u16, 0, 0, W as u16, height as u16]);
    w.extend([0, 0]);
    w.extend(wide(title));
    // The font: point size, then the face.
    w.push(9);
    w.extend(wide("Segoe UI"));
    item(&mut w, WS_CHILD | WS_VISIBLE, 0, [M, M, W - 2 * M, LINE * lines_for(&ctx.text)], ID_TEXT, 0x0082, "");
    item(&mut w, WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_LEFT, 0, [M, why_y, 150, 12], ID_WHY, 0x0080, "");
    item(&mut w, WS_CHILD | WS_TABSTOP | WS_VSCROLL | ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY, WS_EX_CLIENTEDGE, [M, why_y + 16, W - 2 * M, 40], ID_DETAIL, 0x0081, "");
    item(&mut w, WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON, 0, [W - 2 * M - 2 * BTN_W - 4 + M, buttons_y, BTN_W, BTN_H], IDOK, 0x0080, "OK");
    item(&mut w, WS_CHILD | WS_VISIBLE | WS_TABSTOP, 0, [W - M - BTN_W, buttons_y, BTN_W, BTN_H], IDCANCEL, 0x0080, "Cancel");
    align4(&mut w);
    w
}

fn to_px(dialog: HWND, du: [i32; 4]) -> RECT {
    let mut r = RECT { left: du[0], top: du[1], right: du[0] + du[2], bottom: du[1] + du[3] };
    unsafe { MapDialogRect(dialog, &mut r) };
    r
}

/// Puts every control where `expanded` says and sizes the dialog to match.
unsafe fn arrange(dialog: HWND, ctx: &Ctx) {
    let (why_y, detail, buttons_y, height) = layout(lines_for(&ctx.text), lines_for(&ctx.detail), ctx.expanded);
    let place = |id: i32, du: [i32; 4]| {
        let r = to_px(dialog, du);
        unsafe { MoveWindow(GetDlgItem(dialog, id), r.left, r.top, r.right - r.left, r.bottom - r.top, 1) };
    };
    place(ID_TEXT, [M, M, W - 2 * M, LINE * lines_for(&ctx.text)]);
    place(ID_WHY, [M, why_y, 150, 12]);
    if let Some((y, h)) = detail {
        place(ID_DETAIL, [M, y, W - 2 * M, h]);
    }
    unsafe { ShowWindow(GetDlgItem(dialog, ID_DETAIL), if detail.is_some() { SW_SHOW } else { SW_HIDE }) };
    place(IDOK, [W - 2 * M - 2 * BTN_W - 4 + M, buttons_y, BTN_W, BTN_H]);
    place(IDCANCEL, [W - M - BTN_W, buttons_y, BTN_W, BTN_H]);
    let mut outer = to_px(dialog, [0, 0, W, height]);
    let style = unsafe { GetWindowLongPtrW(dialog, windows_sys::Win32::UI::WindowsAndMessaging::GWL_STYLE) } as u32;
    let ex = unsafe { GetWindowLongPtrW(dialog, windows_sys::Win32::UI::WindowsAndMessaging::GWL_EXSTYLE) } as u32;
    unsafe {
        AdjustWindowRectEx(&mut outer, style, 0, ex);
        SetWindowPos(dialog, std::ptr::null_mut(), 0, 0, outer.right - outer.left, outer.bottom - outer.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        let arrow = if ctx.expanded { "\u{25bc} " } else { "\u{25b6} " };
        SetDlgItemTextW(dialog, ID_WHY, wide(&format!("{arrow}{}", ctx.prompt)).as_ptr());
        InvalidateRect(dialog, std::ptr::null(), 1);
    }
}

unsafe extern "system" fn proc(dialog: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => {
            unsafe {
                SetWindowLongPtrW(dialog, GWLP_USERDATA, lparam);
                let ctx = &*(lparam as *const Ctx);
                SetDlgItemTextW(dialog, ID_TEXT, wide(&ctx.text).as_ptr());
                // An edit control wants CR LF.
                SetDlgItemTextW(dialog, ID_DETAIL, wide(&ctx.detail.replace('\n', "\r\n")).as_ptr());
                arrange(dialog, ctx);
                SetForegroundWindow(dialog);
            }
            1
        }
        WM_COMMAND => {
            let (id, code) = ((wparam & 0xffff) as i32, (wparam >> 16) as u32);
            match id {
                ID_WHY if code == 0 => {
                    let ctx = unsafe { &mut *(GetWindowLongPtrW(dialog, GWLP_USERDATA) as *mut Ctx) };
                    ctx.expanded = !ctx.expanded;
                    unsafe { arrange(dialog, ctx) };
                    1
                }
                IDOK | IDCANCEL => {
                    unsafe { EndDialog(dialog, id as isize) };
                    1
                }
                _ => 0,
            }
        }
        _ => 0,
    }
}

static LAST: AtomicIsize = AtomicIsize::new(0);

/// `text`, OK and Cancel, and a collapsed disclosure `prompt` over `detail`. True for OK. `None`
/// when Windows could not make the dialog (the caller asks another way).
pub(super) fn confirm(owner: Option<isize>, topmost: bool, title: &str, text: &str, prompt: &str, detail: &str) -> Option<bool> {
    let mut ctx = Ctx { text: text.to_string(), prompt: prompt.to_string(), detail: detail.to_string(), expanded: false };
    let words = template(title, topmost, &ctx);
    let parent = owner.map_or(std::ptr::null_mut(), |h| h as HWND);
    let answer = super::msgbox::run_modal(owner, title, crate::host_base::log, true, || unsafe {
        DialogBoxIndirectParamW(GetModuleHandleW(std::ptr::null()), words.as_ptr().cast(), parent, Some(proc), &mut ctx as *mut Ctx as LPARAM)
    });
    LAST.store(answer, Ordering::Relaxed);
    (answer >= 0).then_some(answer == IDOK as isize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_disclosure_sits_under_the_text_and_opens_downward() {
        assert_eq!(lines_for("All sessions will be closed."), 1);
        assert_eq!(lines_for(&"x".repeat(120)), 3);
        assert_eq!(lines_for("a\n\nb"), 3);
        let (why, none, buttons, height) = layout(1, 5, false);
        assert!(none.is_none() && why > M + LINE && buttons > why && height > buttons);
        let (_, open, buttons_open, height_open) = layout(1, 5, true);
        let (y, h) = open.unwrap();
        assert!(y > why && buttons_open >= y + h && height_open > height, "the open dialog is taller and its buttons move down");
        assert_eq!(layout(1, 200, true).1.unwrap().1, DETAIL_MAX, "a long reason list scrolls");
    }

    #[test]
    fn the_template_holds_five_controls_on_word_boundaries() {
        let ctx = Ctx { text: "t".into(), prompt: "p".into(), detail: "d".into(), expanded: false };
        let t = template("Quit Termurion?", false, &ctx);
        assert_eq!(t[4], 5, "five controls");
        assert_eq!(t.len() % 2, 0, "whole double words");
    }
}
