//! Every Termurion message box (OK, OK and Cancel, Yes and No): one door to `MessageBoxW`.
//!
//! A box always asks to come to the front (`MB_SETFOREGROUND`). A box with an owner window
//! disables that window until it closes, so a box that opens behind its window leaves the window
//! dead to clicks (each click only dings). FIXES 4: the close question did that once. A watcher
//! thread finds the owned box, writes its state to host.log, and shows and raises it when it is
//! hidden, behind its window, or not the foreground window.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, EnumThreadWindows, GW_HWNDNEXT, GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetForegroundWindow, GetWindow, GetWindowLongPtrW, HWND_TOP, IsWindowVisible,
    MB_SETFOREGROUND, MessageBoxW, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetForegroundWindow, SetWindowPos, WM_NCDESTROY, WM_PAINT, WS_EX_TOPMOST,
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
    show_with(owner, title, text, style, crate::host_base::log)
}

/// `show`, writing the watcher's lines with `log`.
fn show_with(owner: Option<isize>, title: &str, text: &str, style: u32, log: fn(&str)) -> i32 {
    show_inner(owner, title, text, style, log, true)
}

/// `show_with`; `guard` holds back winit's paint loop while the box is open (always true outside
/// the experiment that proves the loop).
fn show_inner(owner: Option<isize>, title: &str, text: &str, style: u32, log: fn(&str), guard: bool) -> i32 {
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let owner_hwnd = owner.map_or(std::ptr::null_mut(), |h| h as HWND);
    run_modal(owner, title, log, guard, || unsafe { MessageBoxW(owner_hwnd, wide(text).as_ptr(), wide(title).as_ptr(), flags(style)) })
}

/// Runs `show`, a modal window of this thread that `owner` owns, the way a message box runs: the
/// watcher finds it and raises it when it opens behind, and the paint guard holds winit's paint
/// loop back while it is open. The disclosure dialog comes through here.
pub(super) fn run_modal<R>(owner: Option<isize>, title: &str, log: fn(&str), guard: bool, show: impl FnOnce() -> R) -> R {
    let done = Arc::new(AtomicBool::new(false));
    if let Some(owner) = owner {
        let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        let (done, title) = (done.clone(), title.to_string());
        std::thread::spawn(move || watch(owner, thread, &title, &done, log));
    }
    let _paint = guard.then(PaintGuard::install);
    let answer = show();
    done.store(true, Ordering::Relaxed);
    answer
}

/// Holds back winit's paint loop on this thread's windows while a box is open.
///
/// A box runs its own message loop inside a winit event handler. winit 0.30 answers WM_PAINT in
/// that state by validating and then asking for another paint at once (`RDW_INTERNALPAINT`), so
/// each of its windows gets WM_PAINT after WM_PAINT for as long as the box is open. That flood
/// can keep the box from being drawn or brought forward (FIXES 4; winit issue 4708 is the same
/// loop during a window drag). While the guard lives, this thread's windows answer WM_PAINT with
/// the default handler, which validates and asks for nothing. Dropping it removes the guard and
/// repaints them.
struct PaintGuard(Vec<isize>);

const PAINT_GUARD_SUBCLASS_ID: usize = 0x4D42_4F58; // "MBOX"

impl PaintGuard {
    fn install() -> Self {
        let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        let mut windows: Vec<isize> = Vec::new();
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
            unsafe { (*(lparam as *mut Vec<isize>)).push(hwnd as isize) };
            1
        }
        unsafe { EnumThreadWindows(thread, Some(each), &mut windows as *mut Vec<isize> as LPARAM) };
        let guarded = windows.into_iter().filter(|&h| unsafe { SetWindowSubclass(h as HWND, Some(paint_default), PAINT_GUARD_SUBCLASS_ID, 0) } != 0).collect();
        PaintGuard(guarded)
    }
}

impl Drop for PaintGuard {
    fn drop(&mut self) {
        for &h in &self.0 {
            unsafe {
                RemoveWindowSubclass(h as HWND, Some(paint_default), PAINT_GUARD_SUBCLASS_ID);
                InvalidateRect(h as HWND, std::ptr::null(), 0);
            }
        }
    }
}

unsafe extern "system" fn paint_default(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_PAINT {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    if msg == WM_NCDESTROY {
        unsafe { RemoveWindowSubclass(hwnd, Some(paint_default), PAINT_GUARD_SUBCLASS_ID) };
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
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

    /// Main thread CPU time in ms.
    fn cpu_ms(thread: isize) -> u64 {
        use windows_sys::Win32::Foundation::FILETIME;
        let z = || FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut a, mut b, mut k, mut u) = (z(), z(), z(), z());
        unsafe { windows_sys::Win32::System::Threading::GetThreadTimes(thread as _, &mut a, &mut b, &mut k, &mut u) };
        let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) / 10_000;
        t(k) + t(u)
    }

    /// One box over `owner` from inside a winit handler: (main thread CPU ms over one second,
    /// box still waiting for its first paint).
    fn one_box(owner: isize, guard: bool) -> (u64, Option<bool>) {
        use windows_sys::Win32::Graphics::Gdi::GetUpdateRect;
        use windows_sys::Win32::System::Threading::{GetCurrentThreadId, OpenThread, THREAD_QUERY_LIMITED_INFORMATION};
        use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
        let thread = unsafe { GetCurrentThreadId() };
        let main = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, thread) } as isize;
        let closer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            unsafe { InvalidateRect(owner as HWND, std::ptr::null(), 0) };
            let before = cpu_ms(main);
            std::thread::sleep(std::time::Duration::from_millis(1000));
            let used = cpu_ms(main) - before;
            let pending = find_box(thread, owner).map(|d| unsafe { GetUpdateRect(d as HWND, std::ptr::null_mut(), 0) } != 0);
            for (h, class, _, _) in thread_windows(thread) {
                if class == "#32770" {
                    unsafe { PostMessageW(h as HWND, WM_CLOSE, 0, 0) };
                }
            }
            (used, pending)
        });
        show_inner(Some(owner), "Paint test", "x", MB_OKCANCEL | MB_ICONWARNING, keep, guard);
        closer.join().unwrap()
    }

    /// FIXES 4: a box opened inside a winit handler spins the thread on WM_PAINT for its window
    /// (winit re-arms each paint), and the paint guard stops it. Needs a window station that
    /// paints; prints what it saw.
    #[test]
    #[ignore]
    fn winit_paint_loop_and_the_guard() {
        use winit::application::ApplicationHandler;
        use winit::event_loop::{ActiveEventLoop, EventLoop};
        use winit::platform::windows::EventLoopBuilderExtWindows;
        #[derive(Default)]
        struct Run {
            seen: Vec<(bool, u64, Option<bool>)>,
            window: Option<winit::window::Window>,
        }
        impl ApplicationHandler for Run {
            fn resumed(&mut self, el: &ActiveEventLoop) {
                let w = el.create_window(winit::window::Window::default_attributes().with_title("paint loop")).unwrap();
                let owner = super::super::menu::hwnd_of(&w).unwrap();
                w.request_redraw();
                self.window = Some(w);
                for guard in [false, true] {
                    let (cpu, pending) = one_box(owner, guard);
                    self.seen.push((guard, cpu, pending));
                }
                el.exit();
            }
            fn window_event(&mut self, _: &ActiveEventLoop, _: winit::window::WindowId, _: winit::event::WindowEvent) {}
        }
        let el = EventLoop::builder().with_any_thread(true).build().unwrap();
        let mut run = Run::default();
        el.run_app(&mut run).unwrap();
        for (guard, cpu, pending) in &run.seen {
            println!("PAINT guard {guard}: main thread CPU {cpu} ms in 1000 ms, box waiting for paint {pending:?}");
        }
        let unguarded = run.seen[0].1;
        let guarded = run.seen[1].1;
        assert!(guarded < 100, "the guard leaves the thread idle: {guarded} ms");
        println!("PAINT spin without the guard: {}", unguarded > 500);
    }
}
