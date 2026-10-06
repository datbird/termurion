//! The taskbar button's overlay icon (the bell count) and the sound mark alerts play
//! (`iTermDockBadgeController.swift`, `PTYSession.m:16657-16667`).
//!
//! `ITaskbarList3` lives on ONE worker thread of its own (an apartment, `HrInit` once): the UI
//! thread only queues jobs, so a slow Explorer never blocks a window. A per-window subclass
//! (id 0x7462_6267) hears `TaskbarButtonCreated` (a new button, also after Explorer restarts)
//! and queues the overlay again; it chains `DefSubclassProc` and removes itself on
//! `WM_NCDESTROY`. Every entry is panic-safe. The icon text is digits only (`icon_text`), and the
//! accessible description is "{n} bells", so nothing a program wrote reaches the shell.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Sender, channel};
use std::sync::OnceLock;
use std::time::Duration;

use windows::Win32::Foundation::HWND as WinHwnd;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::HICON as WinIcon;
use windows::core::PCWSTR;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DIB_RGB_COLORS, DeleteDC, DeleteObject, Ellipse, FF_DONTCARE, FW_BOLD, GetStockObject, NULL_PEN,
    DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DrawTextW, OUT_DEFAULT_PRECIS, SelectObject, SetBkMode, SetTextColor, TRANSPARENT, ANTIALIASED_QUALITY, DEFAULT_PITCH,
};
use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_FILENAME, SND_NODEFAULT, SND_NOSTOP};
use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_READ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{ChangeWindowMessageFilterEx, CreateIconIndirect, MSGFLT_ALLOW, DestroyIcon, ICONINFO, RegisterWindowMessageW, WM_NCDESTROY};

use crate::bell_badge::{self, Sound};

const SUBCLASS_ID: usize = 0x7462_6267;
const SIZE: i32 = 16;

enum Job {
    /// The label of a window's button ("" removes the overlay).
    Set(isize, String),
    /// The button was (re)created: put the overlay on again.
    Reapply(isize),
    /// The window is gone.
    Forget(isize),
    /// Every overlay off (the process ends); answered when done.
    ClearAll(Sender<()>),
}

static JOBS: OnceLock<Option<Sender<Job>>> = OnceLock::new();

fn jobs() -> Option<&'static Sender<Job>> {
    JOBS.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        std::thread::Builder::new().name("tandem-taskbar".into()).spawn(move || worker(rx)).ok().map(|_| tx)
    })
    .as_ref()
}

fn send(job: Job) -> Result<(), String> {
    jobs().ok_or_else(|| "no taskbar thread".to_string())?.send(job).map_err(|e| e.to_string())
}

fn worker(rx: std::sync::mpsc::Receiver<Job>) {
    // SAFETY: first COM use on this thread; an apartment, as the shell object wants.
    let com_ok = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let list: Option<ITaskbarList3> = com_ok.then(|| unsafe { CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER).ok() }).flatten().filter(|l| unsafe { l.HrInit() }.is_ok());
    let mut labels: HashMap<isize, String> = HashMap::new();
    while let Ok(job) = rx.recv() {
        // A panic ends this job only.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
            Job::Set(h, label) => {
                if label.is_empty() { labels.remove(&h) } else { labels.insert(h, label.clone()) };
                if let Some(l) = &list {
                    apply(l, h, &label);
                }
            }
            Job::Reapply(h) => {
                if let (Some(l), Some(label)) = (&list, labels.get(&h)) {
                    apply(l, h, label);
                }
            }
            Job::Forget(h) => {
                labels.remove(&h);
            }
            Job::ClearAll(done) => {
                if let Some(l) = &list {
                    for h in labels.keys() {
                        apply(l, *h, "");
                    }
                }
                labels.clear();
                let _ = done.send(());
            }
        }));
    }
}

fn apply(list: &ITaskbarList3, hwnd: isize, label: &str) {
    let h = WinHwnd(hwnd as *mut _);
    let text = bell_badge::icon_text(label);
    if text.is_empty() {
        // SAFETY: a window of this process; no icon removes the overlay.
        let _ = unsafe { list.SetOverlayIcon(h, WinIcon::default(), PCWSTR::null()) };
        return;
    }
    let Some(icon) = draw_icon(&text) else { return };
    let description: Vec<u16> = format!("{} bells", text.trim_end_matches('+')).encode_utf16().chain(Some(0)).collect();
    // SAFETY: the icon and the description outlive the call; the icon is destroyed after it.
    let _ = unsafe { list.SetOverlayIcon(h, WinIcon(icon as *mut _), PCWSTR::from_raw(description.as_ptr())) };
    unsafe { DestroyIcon(icon) };
}

/// A 16 by 16 icon: a red disc with the text in white bold.
fn draw_icon(text: &str) -> Option<windows_sys::Win32::UI::WindowsAndMessaging::HICON> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let height = match wide.len() {
        1 => 13,
        2 => 11,
        _ => 8,
    };
    // SAFETY: plain GDI on objects made here and released here.
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: SIZE, biHeight: -SIZE, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..std::mem::zeroed() };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if color.is_null() || bits.is_null() {
            DeleteDC(dc);
            return None;
        }
        let old = SelectObject(dc, color);
        let brush = CreateSolidBrush(0x0000_00D0); // BGR: a red
        let old_brush = SelectObject(dc, brush);
        let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        Ellipse(dc, 0, 0, SIZE + 1, SIZE + 1);
        let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
        let font = CreateFontW(-height, 0, 0, 0, FW_BOLD as i32, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32, ANTIALIASED_QUALITY as u32, (DEFAULT_PITCH | FF_DONTCARE) as u32, face.as_ptr());
        let old_font = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, 0x00FF_FFFF);
        let mut rect = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: SIZE, bottom: SIZE };
        DrawTextW(dc, wide.as_ptr(), wide.len() as i32, &mut rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        // GDI writes no alpha: every drawn pixel becomes opaque.
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (SIZE * SIZE) as usize);
        for p in px.iter_mut() {
            if *p & 0x00FF_FFFF != 0 {
                *p |= 0xFF00_0000;
            }
        }
        SelectObject(dc, old_font);
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        SelectObject(dc, old);
        DeleteObject(font);
        DeleteObject(brush);
        // The mask is all zeros (every pixel opaque, the colour bitmap carries the alpha): a null
        // buffer would leave its contents undefined.
        let mask_bits = vec![0u8; (SIZE as usize).div_ceil(16) * 2 * SIZE as usize];
        let mask = windows_sys::Win32::Graphics::Gdi::CreateBitmap(SIZE, SIZE, 1, 1, mask_bits.as_ptr().cast());
        let icon_info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&icon_info);
        DeleteObject(mask);
        DeleteObject(color);
        DeleteDC(dc);
        (!icon.is_null()).then_some(icon)
    }
}

/// Puts `label` (the bell count; "" removes it) on the window's taskbar button. Queued: it
/// never waits for the shell.
pub fn set_badge(hwnd: isize, label: &str) -> Result<(), String> {
    std::panic::catch_unwind(|| send(Job::Set(hwnd, label.chars().take(8).collect()))).unwrap_or_else(|_| Err("panic".into()))
}

fn created_message() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    *MSG.get_or_init(|| {
        let name: Vec<u16> = "TaskbarButtonCreated\0".encode_utf16().collect();
        // SAFETY: a NUL-terminated name.
        unsafe { RegisterWindowMessageW(name.as_ptr()) }
    })
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    // A panic must never leave a window procedure: the body runs guarded.
    let _ = std::panic::catch_unwind(|| {
        if msg == created_message() {
            let _ = send(Job::Reapply(hwnd as isize));
        } else if msg == WM_NCDESTROY {
            let _ = send(Job::Forget(hwnd as isize));
            // SAFETY: removing this proc from the window being destroyed.
            unsafe { RemoveWindowSubclass(hwnd, Some(proc), SUBCLASS_ID) };
        }
    });
    // SAFETY: forwarding the message the system delivered, unchanged.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Hears `TaskbarButtonCreated` on a window (once; a window made again on a new handle gets it again).
pub fn watch(hwnd: isize) {
    let message = created_message();
    // SAFETY: a live window of this process. An elevated Tandem would otherwise never hear the
    // message from Explorer (a lower integrity process) after Explorer restarts, and the badge would
    // be lost.
    unsafe { ChangeWindowMessageFilterEx(hwnd as HWND, message, MSGFLT_ALLOW, std::ptr::null_mut()) };
    // SAFETY: a live window of this process; the proc only queues and forwards.
    unsafe { SetWindowSubclass(hwnd as HWND, Some(proc), SUBCLASS_ID, 0) };
}

/// Every overlay off, waiting a moment for it (the process is ending).
pub fn clear_all() {
    // A process that never showed a badge has no worker: do not start one (and wait) at exit.
    if JOBS.get().is_none() {
        return;
    }
    let (tx, rx) = channel();
    if send(Job::ClearAll(tx)).is_ok() {
        let _ = rx.recv_timeout(Duration::from_millis(300));
    }
}

/// The names registered under `HKCU\AppEvents\EventLabels`, read once.
fn aliases() -> &'static HashSet<String> {
    static NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut out = HashSet::new();
        let path: Vec<u16> = "AppEvents\\EventLabels\0".encode_utf16().collect();
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: registry reads of names only, into buffers of their stated size.
        unsafe {
            if RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key) != 0 {
                return out;
            }
            for i in 0..4096u32 {
                let mut buf = [0u16; 256];
                let mut len = buf.len() as u32;
                if RegEnumKeyExW(key, i, buf.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) != 0 {
                    break;
                }
                out.insert(String::from_utf16_lossy(&buf[..len as usize]));
            }
            RegCloseKey(key);
        }
        out
    })
}

/// The sound a name means, through the one judge (`bell_badge::resolve`).
pub fn resolve_name(name: &str) -> Option<Sound> {
    let windir = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("WINDIR")).map(std::path::PathBuf::from)?;
    bell_badge::resolve(name, &windir, &|n| aliases().contains(n), &bell_badge::accept_file)
}

/// Plays a judged sound: asynchronous, never the default beep, never over a sound still
/// playing (a flood of marks never stacks sounds). False when nothing played.
pub fn play_sound(sound: &Sound) -> bool {
    let (wide, flags): (Vec<u16>, u32) = match (sound.alias(), sound.file()) {
        (Some(name), _) => (name.encode_utf16().chain(Some(0)).collect(), SND_ALIAS),
        (None, Some(path)) => (path.as_os_str().encode_wide_nul(), SND_FILENAME),
        (None, None) => return false,
    };
    // SAFETY: `wide` is NUL-terminated and lives past the call (the system copies a file name).
    unsafe { PlaySoundW(wide.as_ptr(), std::ptr::null_mut(), flags | SND_ASYNC | SND_NODEFAULT | SND_NOSTOP) != 0 }
}

/// A named sound through the judge: None when it is refused, else whether it started (false while
/// another sound plays, `SND_NOSTOP`). See `bell_badge::beeps_after`.
pub fn play_named(name: &str) -> Option<bool> {
    std::panic::catch_unwind(|| resolve_name(name).map(|s| play_sound(&s))).unwrap_or(None)
}

trait EncodeWideNul {
    fn encode_wide_nul(&self) -> Vec<u16>;
}

impl EncodeWideNul for std::ffi::OsStr {
    fn encode_wide_nul(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(Some(0)).collect()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn play_sound_is_called_in_one_function_that_takes_a_judged_sound() {
        let src = include_str!("taskbar_win.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert_eq!(code.matches("PlaySoundW(").count(), 1);
        let at = code.find("PlaySoundW(").unwrap();
        let start = code[..at].rfind("\npub fn ").unwrap();
        assert!(code[start..at].starts_with("\npub fn play_sound(sound: &Sound)"), "PlaySoundW is called only in play_sound(&Sound)");
    }

    #[test]
    fn every_entry_is_panic_safe_and_the_subclass_chains() {
        let src = include_str!("taskbar_win.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        for f in ["pub fn set_badge", "unsafe extern \"system\" fn proc", "pub fn play_named"] {
            let body = &code[code.find(f).unwrap()..];
            let body = &body[..body.find("\n}\n").unwrap()];
            assert!(body.contains("catch_unwind"), "{f}");
        }
        assert!(code.contains("DefSubclassProc(hwnd, msg, wparam, lparam)") && code.contains("RemoveWindowSubclass(hwnd, Some(proc), SUBCLASS_ID)"));
        assert!(code.contains("0x7462_6267"));
    }

    #[test]
    fn the_overlay_is_drawn_and_set_without_a_window() {
        // No window: the icon is drawn (GDI), the job queues and nothing panics.
        assert!(super::draw_icon("7").is_some());
        assert!(super::draw_icon("99+").is_some());
        assert!(super::set_badge(0, "3").is_ok());
        assert!(super::set_badge(0, "").is_ok());
        super::clear_all();
    }
}
