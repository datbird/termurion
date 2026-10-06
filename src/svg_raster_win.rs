//! Renders one model-made SVG to a 128 pixel PNG in a hidden WebView2 (milestone 13x, Task 4).
//! Windows only, and one of the two files that touch COM. The SVG is untrusted: `svg_icon::check`
//! judges it first, the view is InPrivate with scripting only in Tandem's own wrapper page (the SVG
//! is a base64 `<img>`, so no script inside it ever runs), a content policy of `default-src 'none'`,
//! no navigation, a filter that blocks every request that would reach a network or a disk, and a
//! 5 second limit. The renderer is WebView2's sandboxed process, which stands for iTerm2's sandboxed
//! decode (`CIG:340-362`).
//!
//! The view needs a parent window that WebView2 treats as shown, or it can suspend rendering and
//! hand back a blank capture: a 128 by 128 tool window on the screen, layered and fully
//! transparent (alpha 0), click-through and never activated. It is not a terminal window and never
//! registered as one.

use std::ffi::c_void;
use std::time::Instant;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetLayeredWindowAttributes, ShowWindow, LWA_ALPHA, SW_SHOWNOACTIVATE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP};

use crate::svg_icon;
use crate::webhost::{Event, LocalDoc, Sink, ViewId};
use crate::webhost_win::{Env, ViewSetup, WebView};

/// The script token of the one capture.
const CAPTURE_TOKEN: u64 = 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Loading,
    Capturing,
    Done,
}

/// The view's setup: InPrivate (preflight P7), the page message bridge on (the wrapper posts "ok"
/// or "fail"), no navigation after the first load, no new windows, no devtools, every request
/// answered by the page's `LocalDoc` (the page, data images, else 403), transparent at scale 1.0.
pub fn setup() -> ViewSetup {
    ViewSetup {
        transparent: true,
        scale: Some(1.0),
        in_private: true,
        guard_local_schemes: true,
        allowed_schemes: Some(Vec::new()),
        web_message: true,
        resource_filters: vec!["*".to_string()],
        ..Default::default()
    }
}

fn make_window() -> Result<HWND, String> {
    let class: Vec<u16> = "TandemSvgRaster\0".encode_utf16().collect();
    let side = svg_icon::SIDE as i32;
    // SAFETY: plain Win32 calls on this thread with a NUL-terminated class name that outlives them;
    // registering the class twice fails harmlessly.
    unsafe {
        let wc = WNDCLASSW { lpfnWndProc: Some(DefWindowProcW), lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        let h = CreateWindowExW(WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE, class.as_ptr(), std::ptr::null(), WS_POPUP, 0, 0, side, side, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
        if h.is_null() {
            return Err("the icon window could not be made".to_string());
        }
        SetLayeredWindowAttributes(h, 0, 0, LWA_ALPHA);
        ShowWindow(h, SW_SHOWNOACTIVATE);
        Ok(h)
    }
}

/// One candidate being rendered.
pub struct Rasterizer {
    hwnd: isize,
    view: Option<WebView>,
    id: ViewId,
    started: Instant,
    doc: LocalDoc,
    stage: Stage,
}

impl Rasterizer {
    /// Starts rendering `svg` in a new view `id`. Err when the candidate is refused by
    /// `svg_icon::check` or the window cannot be made. Events of the view come back through
    /// `on_event`; the result after at most `svg_icon::RENDER_TIMEOUT` (see `expired`).
    pub fn start(env: &Env, svg: &str, id: ViewId, sink: Sink) -> Result<Rasterizer, String> {
        svg_icon::check(svg).map_err(|why| format!("the SVG was not rendered ({why:?})"))?;
        let hwnd = make_window()?;
        let side = svg_icon::SIDE as i32;
        let view = WebView::create(env, hwnd as isize, id, (0, 0, side, side), setup(), sink);
        // Visible before the capture, so the renderer does not treat the page as occluded.
        view.set_visible(true);
        // Served by the host's one function on Tandem's own origin (never NavigateToString).
        let doc = svg_icon::wrapper_doc(svg);
        view.navigate_tandem(doc.url());
        Ok(Rasterizer { hwnd: hwnd as isize, view: Some(view), id, started: Instant::now(), doc, stage: Stage::Loading })
    }

    pub fn id(&self) -> ViewId {
        self.id
    }

    /// When this render started (its deadline is `svg_icon::RENDER_TIMEOUT` later).
    pub fn started(&self) -> Instant {
        self.started
    }

    /// The 5 second limit passed.
    pub fn expired(&self) -> bool {
        self.started.elapsed() >= svg_icon::RENDER_TIMEOUT
    }

    /// One event. None while still working; Some(Ok(png)) once captured, Some(Err) after "fail", a
    /// failed view, a failed capture or the time limit. Events of other views answer None.
    pub fn on_event(&mut self, e: &Event) -> Option<Result<Vec<u8>, ()>> {
        if self.stage == Stage::Done {
            return None;
        }
        if e.view() != self.id {
            return self.limit();
        }
        match e {
            Event::Source(_, url) => self.doc.saw_source(url),
            Event::Request { token, url, .. } => {
                if let Some(v) = &self.view {
                    v.answer_request(*token, self.doc.answer(url));
                }
            }
            Event::Message { json, source, .. } if self.stage == Stage::Loading => match svg_icon::verdict(json).filter(|_| self.doc.trusts(source)) {
                Some(true) => {
                    self.stage = Stage::Capturing;
                    if let Some(v) = &self.view {
                        v.capture_png(CAPTURE_TOKEN);
                    }
                }
                Some(false) => return self.end(Err(())),
                None => {}
            },
            Event::ScriptDone { token: CAPTURE_TOKEN, json, .. } if self.stage == Stage::Capturing => {
                return self.end(capture_result(json));
            }
            Event::Failed(..) => return self.end(Err(())),
            Event::Navigated { ok: false, .. } => return self.end(Err(())),
            _ => {}
        }
        self.limit()
    }

    /// The time limit alone, for a tick with no event.
    pub fn limit(&mut self) -> Option<Result<Vec<u8>, ()>> {
        if self.stage != Stage::Done && self.expired() { self.end(Err(())) } else { None }
    }

    fn end(&mut self, r: Result<Vec<u8>, ()>) -> Option<Result<Vec<u8>, ()>> {
        self.stage = Stage::Done;
        // The view closes at once; no later event is acted on.
        self.view = None;
        Some(r)
    }
}

/// The capture's base64 PNG as the stored icon.
fn capture_result(json: &Result<String, String>) -> Result<Vec<u8>, ()> {
    use base64::Engine as _;
    let b64 = json.as_ref().map_err(|_| ())?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim_matches('"')).map_err(|_| ())?;
    svg_icon::normalize_png(&bytes).map_err(|_| ())
}

impl Drop for Rasterizer {
    fn drop(&mut self) {
        // The view first (its parent must still exist), then the window.
        self.view = None;
        // SAFETY: the handle is the one this value made and destroys once.
        unsafe { DestroyWindow(self.hwnd as *mut c_void) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webhost::{EnvOptions, Posted, Store};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn the_rasterizer_view_is_closed_down() {
        let s = setup();
        assert!(s.in_private && s.guard_local_schemes && s.web_message && s.transparent);
        assert_eq!(s.allowed_schemes, Some(Vec::new()));
        assert!(!s.new_windows && !s.devtools && s.scripts.is_empty());
        assert_eq!(s.resource_filters, vec!["*".to_string()]);
        assert_eq!(s.scale, Some(1.0));
    }

    /// P18 and the class rules: no `navigate_to_string` caller (the wrapper is served), no COM handler of
    /// its own, no winit window (so never a terminal window), checked before any window or view.
    #[test]
    fn the_rasterizer_has_one_page_and_no_handlers() {
        let me = include_str!("svg_raster_win.rs");
        let prod = &me[..me.find("#[cfg(test)]\nmod tests").unwrap()];
        assert_eq!(prod.matches(".navigate_to_string(").count(), 0);
        assert!(prod.contains("svg_icon::wrapper_doc(") && prod.contains("navigate_tandem(doc.url())"));
        assert!(!prod.contains("Handler::create") && !prod.contains("winit"));
        let check = prod.find("svg_icon::check(").expect("the candidate is judged");
        assert!(check < prod.find("make_window()?").unwrap() && check < prod.find("WebView::create(").unwrap());
        for needed in ["WS_EX_LAYERED", "WS_EX_TOOLWINDOW", "WS_EX_TRANSPARENT", "WS_EX_NOACTIVATE", "LWA_ALPHA", "set_visible(true)"] {
            assert!(prod.contains(needed), "{needed}");
        }
    }

    #[test]
    fn a_capture_is_judged_before_it_is_kept() {
        assert!(capture_result(&Err("x".into())).is_err());
        assert!(capture_result(&Ok("!!!".into())).is_err());
        assert!(capture_result(&Ok("AAAA".into())).is_err());
    }

    fn pump(rx: &mpsc::Receiver<Posted>, secs: u64, mut on: impl FnMut(&Event) -> bool) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            // SAFETY: the usual message pump of this thread.
            unsafe {
                let mut m: MSG = std::mem::zeroed();
                while PeekMessageW(&mut m, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&m);
                    DispatchMessageW(&m);
                }
            }
            while let Ok(p) = rx.try_recv() {
                if let Some(e) = &p.event {
                    if on(e) {
                        return true;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// Needs a real WebView2 Runtime in an interactive session: `scripts/xt test --lib svg_raster_win -- --ignored`.
    /// With no runtime, or in session 0, it prints a note and passes.
    #[test]
    #[ignore]
    fn icon_png_is_square() {
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
        if crate::webhost_win::runtime_version(None).is_err() {
            println!("WebView2 Runtime not installed; skipped");
            return;
        }
        let mut session = 0u32;
        // SAFETY: plain out parameter.
        unsafe { windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(std::process::id(), &mut session) };
        if session == 0 {
            println!("session 0 cannot host WebView2 windows; skipped");
            return;
        }
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let folder = std::env::temp_dir().join(format!("tandem-svg-{}", std::process::id()));
        let (tx, rx) = mpsc::channel::<Posted>();
        let cell: Rc<RefCell<Option<Result<Env, String>>>> = Rc::new(RefCell::new(None));
        let c2 = cell.clone();
        Env::create(Store::Default, &folder, None, &EnvOptions::default(), Box::new(move |r| *c2.borrow_mut() = Some(r)));
        let end = Instant::now() + Duration::from_secs(20);
        while cell.borrow().is_none() && Instant::now() < end {
            pump(&rx, 1, |_| false);
        }
        let env = cell.borrow_mut().take().expect("an environment within 20 s").expect("the environment");
        let run = |id: ViewId, svg: &str| -> Result<Vec<u8>, ()> {
            let mut r = Rasterizer::start(&env, svg, id, tx.clone()).map_err(|_| ())?;
            let mut out = None;
            pump(&rx, 8, |e| {
                out = r.on_event(e);
                out.is_some()
            });
            out.unwrap_or_else(|| r.limit().unwrap_or(Err(())))
        };
        let red = run(1, "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'><rect width='10' height='10' fill='#ff0000'/></svg>").expect("a red square");
        let img = image::load_from_memory(&red).unwrap().to_rgba8();
        assert_eq!((img.width(), img.height()), (128, 128));
        let c = img.get_pixel(64, 64).0;
        assert!(c[0] > 240 && c[1] < 16 && c[2] < 16 && c[3] == 255, "centre {c:?}");
        // Malformed, hostile and empty documents give Err, never a crash.
        assert!(run(2, "<svg><rect").is_err());
        assert!(run(3, "<svg xmlns='http://www.w3.org/2000/svg' onload='alert(1)'><script>1</script></svg>").is_err());
        assert!(run(4, "<svg xmlns='http://www.w3.org/2000/svg'><image href='https://example.invalid/a.png'/></svg>").is_err());
        drop(env);
        if let Some(lp) = crate::localfs::LocalPath::judge(&folder) {
            let _ = crate::localfs::remove_dir_all(&lp);
        }
    }
}
