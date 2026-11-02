//! The Windows half of the web popovers: the popup window with its WebView2 child, the views' setups
//! and the default browser's name. The plan and the rules are in `web_popover`.

use std::time::{Duration, Instant};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::layout::Rect;
use crate::web_popover::{Act, BridgeLimiter, Kind, Layout, closes_for_foreground, layout, open_in_title, open_in_url};
use crate::webhost::{self, ViewId};
use crate::webhost_win::{ViewSetup, WebView};

/// The browser's name for the "Open in" button: the registered handler of http.
pub fn default_browser_name() -> Option<String> {
    use windows::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_FRIENDLYAPPNAME, AssocQueryStringW};
    use windows::core::{PCWSTR, PWSTR};
    let (assoc, verb): (Vec<u16>, Vec<u16>) = ("http\0".encode_utf16().collect(), "open\0".encode_utf16().collect());
    let mut buf = [0u16; 260];
    let mut n = buf.len() as u32;
    let hr = unsafe { AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_FRIENDLYAPPNAME, PCWSTR(assoc.as_ptr()), PCWSTR(verb.as_ptr()), Some(PWSTR(buf.as_mut_ptr())), &mut n) };
    if hr.is_err() {
        return None;
    }
    let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    let name = String::from_utf16_lossy(&buf[..len]);
    (!name.is_empty()).then_some(name)
}

/// A popover window: the popup, its optional button row, and its web view once the environment
/// exists.
pub struct WebPopover {
    host: EguiHost,
    kind: Kind,
    layout: Layout,
    view: Option<WebView>,
    view_id: ViewId,
    focused: bool,
    /// The document served to the view (HTML popovers); its URL is the only source the bridge trusts (P6).
    doc: Option<webhost::LocalDoc>,
    pub limiter: BridgeLimiter,
    /// The page's URL now (from the view's source events).
    current: Option<String>,
    opened: Instant,
}

impl WebPopover {
    /// Opens the window (its bottom centered on `at`, screen pixels, kept on `work`) at the final
    /// content `size` points (`content_size`); the view comes with `attach`.
    #[allow(clippy::too_many_arguments)]
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, kind: Kind, size: (f64, f64), at: (f64, f64), work: Option<Rect>) -> Result<WebPopover, String> {
        let attrs = crate::status_popover_ui::popover_attributes("Popover", size.0, size.1);
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        let scale = host.window.scale_factor();
        let (x, y) = crate::status_popover_ui::place(at, size.0 * scale, size.1 * scale, work);
        host.window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
        host.window.set_visible(true);
        host.focus();
        let layout = layout(&kind, size);
        Ok(WebPopover { host, kind, layout, view: None, view_id: 0, focused: false, doc: None, limiter: BridgeLimiter::default(), current: None, opened: Instant::now() })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn hwnd(&self) -> Option<isize> {
        self.host.hwnd()
    }

    pub fn kind(&self) -> &Kind {
        &self.kind
    }

    pub fn view_id(&self) -> ViewId {
        self.view_id
    }

    pub fn has_view(&self) -> bool {
        self.view.is_some()
    }

    pub fn view(&self) -> Option<&WebView> {
        self.view.as_ref()
    }

    pub fn age(&self) -> Duration {
        self.opened.elapsed()
    }

    /// The client area less the button row, physical pixels in the window's client coordinates.
    pub fn web_rect(&self) -> Rect {
        let size = self.host.window.inner_size();
        let top = (self.layout.web_top * self.host.window.scale_factor()) as f32;
        Rect { x: 0.0, y: top, w: size.width as f32, h: (size.height as f32 - top).max(0.0) }
    }

    /// Gives the popover its view and fits it.
    pub fn attach(&mut self, view: WebView, id: ViewId) {
        view.set_bounds(webhost::physical_bounds(self.web_rect()));
        view.set_visible(true);
        self.view = Some(view);
        self.view_id = id;
    }

    /// The page's URL now, as the view reports it.
    pub fn set_current(&mut self, url: &str) {
        if let Some(d) = self.doc.as_mut() {
            d.saw_source(url);
        }
        self.current = Some(url.to_string());
    }

    /// The document the view was told to load (an HTML popover).
    pub fn set_doc(&mut self, doc: webhost::LocalDoc) {
        self.doc = Some(doc);
    }

    /// Whether a message from `source` is the popover's own document's (P6).
    pub fn trusts(&self, source: &str) -> bool {
        self.doc.as_ref().is_some_and(|d| d.trusts(source))
    }

    /// Answers one request of the view: the document, or a refusal (the one serving function).
    pub fn answer(&self, token: u64, url: &str) {
        let answer = self.doc.as_ref().map_or(webhost::Answer::Block, |d| d.answer(url));
        if let Some(v) = &self.view {
            v.answer_request(token, answer);
        }
    }

    /// What "Open in ..." opens now (Quick Look only).
    pub fn open_in_target(&self) -> Option<String> {
        match &self.kind {
            Kind::QuickLook { url, .. } => Some(open_in_url(self.current.as_deref(), url)),
            Kind::Html { .. } => None,
        }
    }

    /// The popover lost the foreground to another window.
    pub fn lost_foreground(&mut self) -> bool {
        match self.hwnd() {
            Some(own) => closes_for_foreground(own, &mut self.focused, webhost_win_foreground()),
            None => false,
        }
    }

    /// Handles one of its window's events. Focus is read from the foreground window, not winit.
    pub fn event(&mut self, e: &WindowEvent) -> Act {
        match e {
            WindowEvent::CloseRequested => return Act::Close,
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.host.event(e);
                if let Some(v) = &self.view {
                    v.set_bounds(webhost::physical_bounds(self.web_rect()));
                }
                return Act::Keep;
            }
            WindowEvent::Focused(_) => return if self.lost_foreground() { Act::Close } else { Act::Keep },
            WindowEvent::RedrawRequested => return self.redraw(),
            _ => {}
        }
        self.host.event(e);
        Act::Keep
    }

    fn redraw(&mut self) -> Act {
        let mut act = Act::Keep;
        let (layout, kind) = (self.layout, &self.kind);
        let title = match kind {
            Kind::QuickLook { browser, .. } => Some(open_in_title(browser.as_deref())),
            Kind::Html { .. } => None,
        };
        self.host.paint(|ui| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                if let (Some(b), Some(t)) = (layout.button, &title) {
                    let r = egui::Rect::from_min_size(egui::pos2(b.x, b.y), egui::vec2(b.w, b.h));
                    if ui.put(r, egui::Button::new(t.as_str())).clicked() {
                        act = Act::OpenInBrowser;
                    }
                }
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                act = Act::Close;
            }
        });
        act
    }
}

fn webhost_win_foreground() -> Option<isize> {
    crate::webhost_win::foreground_root()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::browser::{html_setup, quick_look_setup};
    use crate::web_popover::{POPOVER_CSP, WEBRTC_OFF_JS, popover_document};


    /// No WebRTC in any frame, no frames at all, no network, no connections, as a header.
    #[test]
    fn popovers_have_no_webrtc_prefetch_or_quirks() {
        let h = html_setup();
        assert_eq!(h.scripts.len(), 2);
        assert_eq!(h.scripts[0], WEBRTC_OFF_JS, "removed before the bridge, in every frame");
        assert_eq!(h.scripts[1], crate::webhost::INVOKE_JS);
        for name in ["RTCPeerConnection", "webkitRTCPeerConnection", "RTCDataChannel", "RTCRtpSender"] {
            assert!(WEBRTC_OFF_JS.contains(name), "{name}");
        }
        for must in ["frame-src 'none'", "connect-src 'none'", "default-src 'none'"] {
            assert!(POPOVER_CSP.contains(must), "{must}");
        }
        let d = popover_document("<p>x</p>").unwrap();
        let webhost::Answer::Respond { headers, .. } = d.answer(d.url()) else { panic!("served") };
        assert!(headers.iter().any(|(k, v)| k == "X-DNS-Prefetch-Control" && v == "off"));
        assert!(headers.iter().any(|(k, v)| k == "Referrer-Policy" && v == "no-referrer"));
    }


    /// Review fix (Task 3, Important 1): records what a real WebView2 reports as the `Source` of a
    /// popover document loaded with `NavigateToString` and as the `Source` of a message that
    /// document posts, and asserts the bridge's trust accepts exactly that value. Needs a runtime
    /// and an interactive session: `scripts/xt test --lib web_popover -- --ignored --nocapture`
    /// (skips with a note otherwise). The value is in docs/superpowers/rulings/m13x-rulings.txt.
    #[test]
    #[ignore]
    fn popover_source_is_the_served_url() {
        use crate::webhost::{EnvOptions, Event, Posted, Store};
        use crate::webhost_win::Env;
        use std::cell::RefCell;
        use std::rc::Rc;
        use std::sync::mpsc;
        use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW, RegisterClassW, TranslateMessage, MSG, PM_REMOVE, WNDCLASSW, WS_POPUP};
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
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        }
        let class: Vec<u16> = "TermurionPopoverSourceTest\0".encode_utf16().collect();
        // SAFETY: a never-shown window of this thread, destroyed below.
        let hwnd = unsafe {
            let wc = WNDCLASSW { lpfnWndProc: Some(DefWindowProcW), lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
            RegisterClassW(&wc);
            CreateWindowExW(0, class.as_ptr(), std::ptr::null(), WS_POPUP, 0, 0, 200, 200, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null())
        };
        assert!(!hwnd.is_null());
        let folder = std::env::temp_dir().join(format!("termurion-popsrc-{}", std::process::id()));
        let (tx, rx) = mpsc::channel::<Posted>();
        let pump = |secs: u64, on: &mut dyn FnMut(&Event) -> bool| -> bool {
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
                    if p.event.as_ref().is_some_and(&mut *on) {
                        return true;
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        };
        let cell: Rc<RefCell<Option<Result<Env, String>>>> = Rc::new(RefCell::new(None));
        let c2 = cell.clone();
        Env::create(Store::Default, &folder, None, &EnvOptions::default(), Box::new(move |r| *c2.borrow_mut() = Some(r)));
        let end = Instant::now() + Duration::from_secs(20);
        while cell.borrow().is_none() && Instant::now() < end {
            pump(1, &mut |_| false);
        }
        let env = cell.borrow_mut().take().expect("an environment within 20 s").expect("the environment");
        let view = WebView::create(&env, hwnd as isize, 1, (0, 0, 200, 200), html_setup(), tx);
        assert!(pump(20, &mut |e| matches!(e, Event::Created(1))), "Created");
        let mut doc = popover_document("<script>iterm2Invoke('f()','cb')</script>").unwrap();
        view.navigate_termurion(doc.url());
        let mut sources: Vec<String> = Vec::new();
        let mut message_source = None;
        pump(10, &mut |e| {
            match e {
                Event::Request { token, url, .. } => view.answer_request(*token, doc.answer(url)),
                Event::Source(1, u) => {
                    sources.push(webhost::scheme_of(u));
                    doc.saw_source(u);
                }
                Event::Message { source, .. } => {
                    message_source = Some(source.clone());
                    return true;
                }
                _ => {}
            }
            false
        });
        println!("RECORDED sources (schemes, in order): {sources:?}; message source scheme: {:?}", message_source.as_deref().map(webhost::scheme_of));
        let src = message_source.expect("the document's message arrives");
        assert!(doc.trusts(&src), "the bridge trusts the source the runtime reports for the document's own message: {src}");
        assert_eq!(src, doc.url(), "the exact URL the view was told to load");
        drop(view);
        drop(env);
        // SAFETY: the handle this test made.
        unsafe { DestroyWindow(hwnd) };
        if let Some(lp) = crate::localfs::LocalPath::judge(&folder) {
            let _ = crate::localfs::remove_dir_all(&lp);
        }
    }


    #[test]
    fn popover_views_are_locked_down() {
        let h = html_setup();
        assert!(h.web_message, "the bridge");
        assert!(h.guard_local_schemes);
        assert!(h.in_private, "storage a popover script writes lives in memory and goes with the popover (nothing on disk under a never reused origin)");
        assert_eq!(h.allowed_schemes, Some(Vec::new()), "nothing loads but Termurion's own navigation to the served document");
        assert!(!h.new_windows && !h.devtools);
        assert_eq!(h.scripts, vec![WEBRTC_OFF_JS.to_string(), crate::webhost::INVOKE_JS.to_string()]);
        assert_eq!(h.resource_filters, vec!["*".to_string()], "every request goes to the one serving function");
        let q = quick_look_setup("");
        assert!(!q.web_message, "no bridge for a page the person pointed at");
        assert!(q.scripts.is_empty());
        assert!(q.guard_local_schemes && !q.new_windows && !q.devtools);
        assert!(q.in_private, "no cookies or storage shared with any other view (P7)");
        assert_eq!(q.allowed_schemes, Some(vec!["http".to_string(), "https".to_string()]));
        assert_eq!(q.user_agent.as_deref(), Some(""));
        assert_eq!(quick_look_setup("UA").user_agent.as_deref(), Some("UA"));
    }


    #[test]
    fn popovers_use_the_shared_popup_window() {
        let s = include_str!("../../status_popover_ui.rs");
        assert!(s.contains("pub(crate) fn popover_attributes(title: &str, w: f64, h: f64) -> winit::window::WindowAttributes"));
        assert!(s.contains("pub(crate) fn place(") && s.contains("pub(crate) fn closes("));
        let me = include_str!("web_popover.rs");
        let prod = &me[..me.find("#[cfg(test)]\nmod tests").unwrap()];
        assert!(prod.contains("popover_attributes("));
        assert!(!prod.contains("AddHostObject"));
        assert!(!prod.contains("navigate_to_string"), "program content is served, never NavigateToString");
    }
}
