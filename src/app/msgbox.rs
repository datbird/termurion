//! Every Termurion message box (OK, OK and Cancel, Yes and No): one door to `MessageBoxW`.
//!
//! A box always asks to come to the front (`MB_SETFOREGROUND`). A box with an owner window
//! disables that window until it closes, so a box that opens behind its window leaves the window
//! dead to clicks (each click only dings). FIXES 4: the close question did that once. A watcher
//! thread finds the owned box, writes its state to host.log, and shows and raises it when it is
//! hidden, behind its window, or not the foreground window.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, GW_HWNDNEXT, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetForegroundWindow, GetWindow, GetWindowLongPtrW, HWND_TOP, IsWindowVisible,
    MB_SETFOREGROUND, MessageBoxW, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetForegroundWindow, SetWindowPos, WS_EX_TOPMOST,
};

/// How long the watcher looks for the box, and how often.
const WATCH_TRIES: u32 = 30;
const WATCH_STEP: std::time::Duration = std::time::Duration::from_millis(100);

/// The flags every box uses: the caller's, plus a request to come to the front.
pub(super) fn flags(style: u32) -> u32 {
    style | MB_SETFOREGROUND
}

/// Shows a message box and returns its answer (IDOK, IDYES, ...). `owner` is the window the box
/// belongs to (disabled while the box is open), or None for a box of its own.
pub(super) fn show(owner: Option<isize>, title: &str, text: &str, style: u32) -> i32 {
    show_with(owner, title, text, style, crate::host::log)
}

/// `show`, writing the watcher's lines with `log`.
fn show_with(owner: Option<isize>, title: &str, text: &str, style: u32, log: fn(&str)) -> i32 {
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let done = Arc::new(AtomicBool::new(false));
    if let Some(owner) = owner {
        let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        let (done, title) = (done.clone(), title.to_string());
        std::thread::spawn(move || watch(owner, thread, &title, &done, log));
    }
    let owner_hwnd = owner.map_or(std::ptr::null_mut(), |h| h as HWND);
    let answer = unsafe { MessageBoxW(owner_hwnd, wide(text).as_ptr(), wide(title).as_ptr(), flags(style)) };
    done.store(true, Ordering::Relaxed);
    answer
}

/// Finds the box `thread` shows for `owner`, logs where it is, and raises it when it is behind
/// its window or not in the foreground.
fn watch(owner: isize, thread: u32, title: &str, done: &AtomicBool, log: fn(&str)) {
    for _ in 0..WATCH_TRIES {
        std::thread::sleep(WATCH_STEP);
        if done.load(Ordering::Relaxed) {
            return;
        }
        let Some(dialog) = find_box(thread, owner) else { continue };
        let foreground = unsafe { GetForegroundWindow() } as isize == dialog;
        let visible = unsafe { IsWindowVisible(dialog as HWND) } != 0;
        let above = is_above(dialog, owner);
        let owner_topmost = unsafe { GetWindowLongPtrW(owner as HWND, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST != 0;
        log(&format!("dialog: \"{title}\" shown, visible {visible}, foreground {foreground}, above its window {above}, window topmost {owner_topmost}"));
        if !visible || !foreground || !above {
            unsafe {
                SetWindowPos(dialog as HWND, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
                SetForegroundWindow(dialog as HWND);
            }
            let now = unsafe { GetForegroundWindow() } as isize == dialog;
            log(&format!("dialog: \"{title}\" raised, visible now {}, foreground now {now}, above its window now {}", unsafe { IsWindowVisible(dialog as HWND) } != 0, is_above(dialog, owner)));
        }
        return;
    }
    if !done.load(Ordering::Relaxed) {
        log(&format!("dialog: \"{title}\" not found after {} ms", WATCH_TRIES * WATCH_STEP.as_millis() as u32));
    }
}

/// The dialog window (class `#32770`) of `thread` that `owner` owns, shown or not.
fn find_box(thread: u32, owner: isize) -> Option<isize> {
    struct Find {
        owner: isize,
        found: Option<isize>,
    }
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
        let find = unsafe { &mut *(lparam as *mut Find) };
        let mut class = [0u16; 16];
        let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
        let is_dialog = String::from_utf16_lossy(&class[..n.max(0) as usize]) == "#32770";
        if is_dialog && unsafe { GetWindow(hwnd, GW_OWNER) } as isize == find.owner {
            find.found = Some(hwnd as isize);
            return 0;
        }
        1
    }
    let mut find = Find { owner, found: None };
    unsafe { EnumThreadWindows(thread, Some(each), &mut find as *mut Find as LPARAM) };
    find.found
}

/// True when `window` is in front of `other` in the z-order (`other` comes after it).
fn is_above(window: isize, other: isize) -> bool {
    let mut next = unsafe { GetWindow(window as HWND, GW_HWNDNEXT) };
    while !next.is_null() {
        if next as isize == other {
            return true;
        }
        next = unsafe { GetWindow(next, GW_HWNDNEXT) };
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OKCANCEL};

    #[test]
    fn every_box_asks_for_the_front() {
        assert_eq!(flags(MB_OKCANCEL | MB_ICONWARNING), MB_OKCANCEL | MB_ICONWARNING | MB_SETFOREGROUND);
    }

    /// FIXES 4: no box opens outside this door, so none skips the front request and the watcher.
    #[test]
    fn no_message_box_bypasses_the_door() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![dir];
        let mut offenders = Vec::new();
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("app/msgbox.rs") && !p.ends_with("app\\msgbox.rs") {
                    let code = std::fs::read_to_string(&p).unwrap();
                    if code.contains(concat!("MessageBox", "W(")) {
                        offenders.push(p.display().to_string());
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "message boxes outside app/msgbox.rs: {offenders:?}");
    }


    /// (window, class, owner, visible) for each top-level window of `thread`.
    fn thread_windows(thread: u32) -> Vec<(isize, String, isize, bool)> {
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
            let out = unsafe { &mut *(lparam as *mut Vec<(isize, String, isize, bool)>) };
            let mut class = [0u16; 64];
            let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
            out.push((hwnd as isize, String::from_utf16_lossy(&class[..n.max(0) as usize]), unsafe { GetWindow(hwnd, GW_OWNER) } as isize, unsafe { IsWindowVisible(hwnd) } != 0));
            1
        }
        let mut out = Vec::new();
        unsafe { EnumThreadWindows(thread, Some(each), &mut out as *mut Vec<(isize, String, isize, bool)> as LPARAM) };
        out
    }

    static LINES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    fn keep(line: &str) {
        LINES.lock().unwrap().push(line.to_string());
    }

    /// A real owned box behind a real window: the watcher finds it and raises it. Run where no
    /// one sits at the desktop (an ssh session); a second thread cancels the box.
    #[test]
    #[ignore]
    fn watcher_finds_and_raises_an_owned_box() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, IDCANCEL, PostMessageW, WM_CLOSE, WS_OVERLAPPEDWINDOW};
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let owner = unsafe { CreateWindowExW(0, wide("STATIC").as_ptr(), wide("owner").as_ptr(), WS_OVERLAPPEDWINDOW, 0, 0, 400, 300, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null()) } as isize;
        assert_ne!(owner, 0);
        let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        let closer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let found = find_box(thread, owner);
            let all = thread_windows(thread);
            println!("thread windows: {all:?}");
            // Close every dialog of the thread so a missed box never hangs the run.
            for (h, class, _, _) in &all {
                if class == "#32770" {
                    unsafe { PostMessageW(*h as HWND, WM_CLOSE, 0, 0) };
                }
            }
            found.map(|b| is_above(b, owner))
        });
        let answer = show_with(Some(owner), "Watch test", "x", MB_OKCANCEL | MB_ICONWARNING, keep);
        let above = closer.join().unwrap();
        unsafe { DestroyWindow(owner as HWND) };
        let lines = LINES.lock().unwrap().clone();
        println!("{lines:?} above at close {above:?}");
        assert_eq!(answer, IDCANCEL);
        assert_eq!(above, Some(true), "the box ends in front of its window");
        assert!(lines.first().is_some_and(|l| l.starts_with("dialog: \"Watch test\" shown")), "{lines:?}");
    }
}
