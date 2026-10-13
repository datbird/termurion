//! Web popovers (milestone 13x, Task 3): a script status bar component's HTML popover
//! (`BASE:363-389`) and URL Quick Look (`ARC:976-994`), each a borderless, taskbar-less popup
//! window holding a WebView2 child. The HTML a script supplies and the URL a person points at are
//! untrusted: the HTML loads under a Content-Security-Policy that allows no network, nothing
//! navigates after the first load (P6), the bridge is a closed set from the popover's own
//! document (an exact URL on Termurion's own origin, `webhost::LocalDoc`), and Quick Look is an InPrivate view that may reach http and https only, through the
//! one navigation verdict (B4, P7).

use std::time::{Duration, Instant};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::layout::Rect;
use crate::webhost::{self, ViewId};
use crate::webhost_win::{ViewSetup, WebView};

/// Quick Look's content size, points (`WVW:44`).
pub const QUICK_LOOK_SIZE: (f64, f64) = (800.0, 600.0);
/// The button's margin from the left, the top and the page, points (`WVW:55-73`).
pub const BUTTON_MARGIN: f64 = 8.0;
/// The "Open in ..." button's height, points (a textured rounded push button; Decision).
pub const BUTTON_HEIGHT: f64 = 22.0;
/// The most HTML a popover loads, bytes: well under NavigateToString's 2 MB.
pub const MAX_HTML: usize = 1024 * 1024;
/// Bridge calls a popover may make in one second (P6).
pub const BRIDGE_PER_SECOND: usize = 20;
/// The policy every popover document is served under, as a response header: no network, no frames,
/// no forms, no base, images only as data. The bridge needs the page's own inline script, which the
/// policy allows.
pub const POPOVER_CSP: &str = "default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'; frame-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'";

/// Removes WebRTC before any page script runs, in every frame: a content policy does not govern
/// it, and a STUN request is network traffic (review fix, Minor 1).
pub const WEBRTC_OFF_JS: &str = "(function(){['RTCPeerConnection','webkitRTCPeerConnection','RTCDataChannel','RTCRtpSender'].forEach(function(n){try{Object.defineProperty(window,n,{value:undefined,configurable:false,writable:false});}catch(e){}});})();";

/// What a popover shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A script component's HTML (iTerm2's `loadHTMLString:baseURL:nil`), no button row.
    Html { html: String },
    /// A page the person pointed at, with an "Open in <browser>" button above it.
    QuickLook { url: String, browser: Option<String> },
}

/// What a popover event asks of the app.
#[derive(Debug, PartialEq)]
pub enum Act {
    Keep,
    Close,
    OpenInBrowser,
}

/// "Open in <name>", else "Open in Default Browser" (`WVW:53`, `:98`). The name comes from the
/// registry: shown without control or bidi characters, at most 64 characters.
pub fn open_in_title(browser: Option<&str>) -> String {
    // A control character is a space (as before the merge), then THE display sanitizer.
    let spaced = |b: &str| b.chars().take(256).map(|c| if c.is_control() { ' ' } else { c }).collect::<String>();
    let name: String = browser.map(|b| crate::inert_files::display_text(&spaced(b), 64, false)).unwrap_or_default().trim().to_string();
    if name.is_empty() { "Open in Default Browser".to_string() } else { format!("Open in {name}") }
}

/// Whether Quick Look may show `url`: its scheme is http or https (`ARC:2682-2684`).
pub fn quick_look_scheme_ok(url: &str) -> bool {
    matches!(webhost::scheme_of(url).as_str(), "http" | "https")
}

/// The URL a selection or a link names, when Quick Look may show it: http or https with a host,
/// no white space, control or bidi characters. None otherwise.
pub fn quick_look_target(text: &str) -> Option<String> {
    let t = text.trim();
    let clean = !t.is_empty() && crate::inert_files::display_text(t, usize::MAX, false) == t && !t.chars().any(char::is_whitespace);
    (clean && quick_look_scheme_ok(t) && crate::toolbelt::url_host(t).is_some()).then(|| t.to_string())
}

/// A selection of `len` characters is short in a pane `columns` wide (`PTYTextView.m:3893-3896`).
pub fn short_selection(len: usize, columns: usize) -> bool {
    len <= columns.max(80)
}

/// Whether the context menu offers "Quick Look Link" for `selected` (`CMH:557-558`).
pub fn quick_look_offered(selected: &str, columns: usize) -> bool {
    short_selection(selected.chars().count(), columns) && quick_look_target(selected).is_some()
}

/// The popover's content size in points: HTML at the size asked for, Quick Look at its own, both
/// kept inside `work` and the device's largest surface.
pub fn content_size(kind: &Kind, requested: (f64, f64), work: Option<Rect>, scale: f64, max_px: f64) -> (f64, f64) {
    let asked = match kind {
        Kind::Html { .. } => requested,
        Kind::QuickLook { .. } => QUICK_LOOK_SIZE,
    };
    crate::status_popover_ui::popover_size(asked, work, scale, max_px)
}

/// The popover's layout in points: the button (Quick Look: a margin in and down from the top
/// left, `WVW:55-73`, a flipped view) and where the page starts (below the button and a margin).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub button: Option<Rect>,
    pub web_top: f64,
}

pub fn layout(kind: &Kind, size: (f64, f64)) -> Layout {
    match kind {
        Kind::Html { .. } => Layout { button: None, web_top: 0.0 },
        Kind::QuickLook { .. } => {
            let w = (size.0 - 2.0 * BUTTON_MARGIN).clamp(1.0, 160.0) as f32;
            let button = Rect { x: BUTTON_MARGIN as f32, y: BUTTON_MARGIN as f32, w, h: BUTTON_HEIGHT as f32 };
            Layout { button: Some(button), web_top: f64::from(button.y + button.h) + BUTTON_MARGIN }
        }
    }
}

/// The document a status bar popover is served: the script's HTML, byte for byte, under the policy
/// as a response header (nothing is parsed or inserted). Refused over `MAX_HTML` (the caller falls
/// back to the text popover with a log line).
pub fn popover_document(html: &str) -> Result<webhost::LocalDoc, &'static str> {
    if html.len() > MAX_HTML {
        return Err("the popover's HTML is too large");
    }
    Ok(webhost::LocalDoc::new(html.to_string(), POPOVER_CSP))
}

/// The HTML popover's view: the bridge script, a message channel for it, no automatic windows, no
/// developer tools, every request answered by the popover's `LocalDoc` (the document, or 403), and
/// nothing may load after Termurion's own navigation to it (P6, B4). InPrivate: whatever the script
/// stores stays in memory and goes with the popover (each document has its own origin, so nothing
/// on disk would ever be read again).
pub fn html_setup() -> ViewSetup {
    ViewSetup {
        scripts: vec![WEBRTC_OFF_JS.to_string(), webhost::INVOKE_JS.to_string()],
        guard_local_schemes: true,
        in_private: true,
        allowed_schemes: Some(Vec::new()),
        web_message: true,
        resource_filters: vec!["*".to_string()],
        ..Default::default()
    }
}

/// Quick Look's view (`WebUserAgent` as the factory applies it): no bridge, InPrivate (P7), http
/// and https only, through the one navigation verdict (B4).
pub fn quick_look_setup(user_agent: &str) -> ViewSetup {
    ViewSetup {
        user_agent: Some(user_agent.to_string()),
        in_private: true,
        guard_local_schemes: true,
        allowed_schemes: Some(vec!["http".to_string(), "https".to_string()]),
        ..Default::default()
    }
}

/// What the invoke bridge does with one message (`WVW:189-215`).
#[derive(Clone, Debug, PartialEq)]
pub enum BridgeCall {
    /// Not a message the bridge answers (a view with no delegate, or outside the closed set).
    Ignore,
    /// No invocation: the callback (when given) is answered with null and `log` goes to the log.
    Missing { reply: Option<String>, log: &'static str },
    Invoke { invocation: String, callback: Option<String> },
}

pub fn bridge_call(is_html: bool, json: &str) -> BridgeCall {
    if !is_html {
        return BridgeCall::Ignore;
    }
    match webhost::invoke_request(json) {
        None => BridgeCall::Ignore,
        Some((None, callback)) => BridgeCall::Missing { reply: callback.map(|cb| webhost::return_script(&cb, "null")), log: "missing invocation" },
        Some((Some(invocation), callback)) => BridgeCall::Invoke { invocation, callback },
    }
}

/// At most `BRIDGE_PER_SECOND` calls in any one second.
#[derive(Debug, Default)]
pub struct BridgeLimiter {
    window: Option<(Instant, usize)>,
}

impl BridgeLimiter {
    pub fn allow(&mut self, now: Instant) -> bool {
        match &mut self.window {
            Some((start, n)) if now.duration_since(*start) < Duration::from_secs(1) => {
                if *n >= BRIDGE_PER_SECOND {
                    return false;
                }
                *n += 1;
                true
            }
            _ => {
                self.window = Some((now, 1));
                true
            }
        }
    }
}

/// The semitransient rule, read from the foreground window (the page's child window takes the
/// keyboard, so winit's `Focused(false)` is not a loss): once the popover has been in front, it
/// closes when another window root is. No foreground window at all is a transition.
pub fn closes_for_foreground(own: isize, had_focus: &mut bool, foreground_root: Option<isize>) -> bool {
    match foreground_root {
        Some(r) if r == own => {
            *had_focus = true;
            false
        }
        Some(_) => *had_focus,
        None => false,
    }
}

/// What "Open in ..." opens: the page now showing, else the original (`WVW:76-86`). What the view
/// says is judged again: only http and https reach the shell.
pub fn open_in_url(current: Option<&str>, original: &str) -> String {
    match current {
        Some(c) if !c.is_empty() && c != "about:blank" && quick_look_scheme_ok(c) => c.to_string(),
        _ => original.to_string(),
    }
}

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
    use crate::layout::Rect;
    use std::time::{Duration, Instant};

    fn html(s: &str) -> Kind {
        Kind::Html { html: s.to_string() }
    }
    fn quick(url: &str) -> Kind {
        Kind::QuickLook { url: url.to_string(), browser: None }
    }

    /// Review Focus 3: sizes and the button row (`BASE:363-373`, `WVW:44-73`).
    #[test]
    fn popover_plan_follows_iterm2() {
        let work = Some(Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 });
        // An HTML popover keeps the requested size after the usual cap and has no button row.
        let size = content_size(&html("<p>x</p>"), (300.0, 200.0), work, 1.0, 8192.0);
        assert_eq!(size, (300.0, 200.0));
        let l = layout(&html("<p>x</p>"), size);
        assert_eq!(l.button, None);
        assert_eq!(l.web_top, 0.0);
        // A hostile size is capped as a text popover's is.
        assert_eq!(content_size(&html(""), (2147483647.0, 2147483647.0), work, 1.0, 8192.0), (1920.0, 1040.0));
        // Quick Look is 800 by 600 with the button at the top (a flipped view) and the page below it.
        assert_eq!(QUICK_LOOK_SIZE, (800.0, 600.0));
        assert_eq!(BUTTON_MARGIN, 8.0);
        let size = content_size(&quick("https://a.b"), (1.0, 1.0), work, 1.0, 8192.0);
        assert_eq!(size, QUICK_LOOK_SIZE, "the requested size is not used for Quick Look");
        let l = layout(&quick("https://a.b"), size);
        let b = l.button.expect("a button row");
        assert_eq!((b.x, b.y), (BUTTON_MARGIN as f32, BUTTON_MARGIN as f32));
        assert_eq!(l.web_top, f64::from(b.y + b.h) + BUTTON_MARGIN, "the web view starts a margin below the button");
        // A small monitor shrinks it.
        let small = Some(Rect { x: 0.0, y: 0.0, w: 640.0, h: 480.0 });
        assert_eq!(content_size(&quick("https://a.b"), (1.0, 1.0), small, 1.0, 8192.0), (640.0, 480.0));
        // Titles.
        assert_eq!(open_in_title(Some("Microsoft Edge")), "Open in Microsoft Edge");
        assert_eq!(open_in_title(None), "Open in Default Browser");
        assert_eq!(open_in_title(Some("")), "Open in Default Browser");
        // A name from the registry is shown without control or bidi characters and is bounded.
        assert_eq!(open_in_title(Some("Br\u{202e}ow\nser")), "Open in Brow ser");
        assert!(open_in_title(Some(&"x".repeat(500))).chars().count() <= "Open in ".len() + 64);
    }

    #[test]
    fn quick_look_follows_iterm2() {
        for ok in ["https://x", "HTTP://x", "http://x", "HtTpS://x/y"] {
            assert!(quick_look_scheme_ok(ok), "{ok}");
        }
        for bad in ["ftp://x", "example.com", "file:///c:/x", "javascript:alert(1)", "", "mailto:a@b", "https", "ms-msdt:id", "data:text/html,x"] {
            assert!(!quick_look_scheme_ok(bad), "{bad}");
        }
        // What the person points at is judged before a view exists: a host, no spaces or controls.
        assert_eq!(quick_look_target("  https://a.b/c  ").as_deref(), Some("https://a.b/c"));
        for bad in ["https://", "https:///x", "https://a b", "https://a\u{7}b", "ftp://a.b", "a.b/c", "https://a.b/\u{202e}x"] {
            assert_eq!(quick_look_target(bad), None, "{bad}");
        }
    }

    /// `CMH:557-558`, `PTYTextView.m:3893-3896`.
    #[test]
    fn quick_look_link_offered_as_iterm2() {
        assert!(quick_look_offered("https://a.b/c", 80));
        assert!(!quick_look_offered("a.b/c", 80));
        let long = format!("https://a.b/{}", "x".repeat(188));
        assert_eq!(long.chars().count(), 200);
        assert!(!quick_look_offered(&long, 80), "not a short selection");
        let hundred = format!("https://a.b/{}", "x".repeat(88));
        assert_eq!(hundred.chars().count(), 100);
        assert!(quick_look_offered(&hundred, 120));
        assert!(!quick_look_offered(&hundred, 80));
        assert!(short_selection(80, 80) && short_selection(81, 120) && !short_selection(121, 120) && !short_selection(81, 80));
        assert!(!quick_look_offered("ftp://a.b/c", 80));
    }

    /// `WVW:189-215`: what the bridge does with a message.
    #[test]
    fn invoke_bridge_follows_iterm2() {
        // No invocation: the callback gets null and "missing invocation" is logged.
        match bridge_call(true, r#"{"callback":"cb"}"#) {
            BridgeCall::Missing { reply, log } => {
                assert_eq!(reply.as_deref(), Some("try { cb(null); } catch(err) { alert('Caught error while sending return value to javascript: ' + err.message + '\\n' + err.stack); }"));
                assert_eq!(log, "missing invocation");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(bridge_call(true, "{}"), BridgeCall::Missing { reply: None, .. }));
        // With an invocation: a call, the callback kept.
        assert_eq!(
            bridge_call(true, r#"{"invocation":"f(x: 1)","callback":"cb"}"#),
            BridgeCall::Invoke { invocation: "f(x: 1)".into(), callback: Some("cb".into()) }
        );
        assert_eq!(bridge_call(true, r#"{"invocation":"f()"}"#), BridgeCall::Invoke { invocation: "f()".into(), callback: None });
        // A Quick Look or tool view has no delegate: ignored (`WVW:189`).
        assert_eq!(bridge_call(false, r#"{"invocation":"f()","callback":"cb"}"#), BridgeCall::Ignore);
        // The closed set (B5): anything else is ignored.
        for bad in ["not json", r#"{"invocation":"f()","extra":"x"}"#, r#"{"invocation":1}"#, r#"{"invocation":"f()","callback":"a b"}"#, r#"["f()"]"#, r#"{"error":"x"}"#] {
            assert_eq!(bridge_call(true, bad), BridgeCall::Ignore, "{bad}");
        }
    }

    /// P6 and the class ruling: the popover's HTML is served as it is, under its policy as a header,
    /// and only its own document's exact URL may call the bridge (no markup parsing anywhere).
    #[test]
    fn popover_documents_are_served_not_rewritten() {
        let hostile = "<!-- --!><img src=\"https://x.test/p\"><script>fetch('https://x.test')</script><!-- --><!doctype html>";
        let mut d = popover_document(hostile).unwrap();
        let webhost::Answer::Respond { body, headers, .. } = d.answer(d.url()) else { panic!("served") };
        assert_eq!(body, hostile.as_bytes(), "byte for byte: no meta tag, no doctype scan");
        assert!(headers.iter().any(|(k, v)| k == "Content-Security-Policy" && v == POPOVER_CSP));
        assert_eq!(d.answer("https://x.test/p"), webhost::Answer::Block);
        assert_eq!(d.answer("https://x.test/"), webhost::Answer::Block);
        let url = d.url().to_string();
        d.saw_source(&url);
        assert!(d.trusts(&url) && !d.trusts("data:text/html;charset=utf-8,x") && !d.trusts("about:blank"));
    }

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
        let class: Vec<u16> = "TandemPopoverSourceTest\0".encode_utf16().collect();
        // SAFETY: a never-shown window of this thread, destroyed below.
        let hwnd = unsafe {
            let wc = WNDCLASSW { lpfnWndProc: Some(DefWindowProcW), lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
            RegisterClassW(&wc);
            CreateWindowExW(0, class.as_ptr(), std::ptr::null(), WS_POPUP, 0, 0, 200, 200, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null())
        };
        assert!(!hwnd.is_null());
        let folder = std::env::temp_dir().join(format!("tandem-popsrc-{}", std::process::id()));
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
                    if p.event.as_ref().is_some_and(|e| on(e)) {
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
        view.navigate_tandem(doc.url());
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
    fn bridge_calls_are_rate_limited() {
        let mut l = BridgeLimiter::default();
        let t0 = Instant::now();
        for _ in 0..BRIDGE_PER_SECOND {
            assert!(l.allow(t0));
        }
        assert!(!l.allow(t0), "the 21st in one second");
        assert!(!l.allow(t0 + Duration::from_millis(900)));
        assert!(l.allow(t0 + Duration::from_millis(1100)), "a new second");
    }

    /// The HTML a script supplies is untrusted: it loads nothing and goes nowhere.
    #[test]
    fn popover_html_is_sandboxed() {
        let doc = popover_document("<p>hello</p>").unwrap();
        let webhost::Answer::Respond { body, .. } = doc.answer(doc.url()) else { panic!("served") };
        assert_eq!(body, b"<p>hello</p>");
        for must in ["default-src 'none'", "img-src data:", "style-src 'unsafe-inline'", "form-action 'none'", "base-uri 'none'", "frame-src 'none'", "connect-src 'none'"] {
            assert!(POPOVER_CSP.contains(must), "{must}");
        }
        for never in ["http:", "https:", "'unsafe-eval'", "*"] {
            assert!(!POPOVER_CSP.contains(never), "{never}");
        }
        assert!(popover_document(&"x".repeat(MAX_HTML + 1)).is_err(), "over the bound");
        assert!(popover_document(&"x".repeat(MAX_HTML - 1000)).is_ok());
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

    /// The semitransient rule reads the foreground window, not winit's focus (the page's child window takes it).
    #[test]
    fn popover_closes_when_it_loses_the_foreground() {
        let mut had = false;
        // Not yet in front: the popover is still being made.
        assert!(!closes_for_foreground(100, &mut had, Some(7)));
        assert!(!had);
        assert!(!closes_for_foreground(100, &mut had, Some(100)), "in front");
        assert!(had);
        assert!(!closes_for_foreground(100, &mut had, Some(100)), "the page's child window has the keyboard; the root is still the popover");
        assert!(!closes_for_foreground(100, &mut had, None), "between windows: asked again next tick");
        assert!(closes_for_foreground(100, &mut had, Some(7)), "another window is in front");
    }

    #[test]
    fn open_in_browser_opens_the_page_it_is_showing() {
        assert_eq!(open_in_url(Some("https://b.c/next"), "https://a.b/"), "https://b.c/next");
        assert_eq!(open_in_url(None, "https://a.b/"), "https://a.b/");
        assert_eq!(open_in_url(Some(""), "https://a.b/"), "https://a.b/");
        assert_eq!(open_in_url(Some("about:blank"), "https://a.b/"), "https://a.b/");
        // What the view says now is judged again before it reaches the shell.
        assert_eq!(open_in_url(Some("file:///C:/Windows/system32/calc.exe"), "https://a.b/"), "https://a.b/");
        assert_eq!(open_in_url(Some("ms-msdt:id"), "https://a.b/"), "https://a.b/");
    }

    #[test]
    fn popovers_use_the_shared_popup_window() {
        let s = include_str!("status_popover_ui.rs");
        assert!(s.contains("pub(crate) fn popover_attributes(title: &str, w: f64, h: f64) -> winit::window::WindowAttributes"));
        assert!(s.contains("pub(crate) fn place(") && s.contains("pub(crate) fn closes("));
        let me = include_str!("web_popover.rs");
        let prod = &me[..me.find("#[cfg(test)]\nmod tests").unwrap()];
        assert!(prod.contains("popover_attributes("));
        assert!(!prod.contains("AddHostObject"));
        assert!(!prod.contains("navigate_to_string"), "program content is served, never NavigateToString");
    }
}
