//! The WebView2 host (milestone 13x, Task 1): the only file that touches COM for web views.
//! `Env` is one `ICoreWebView2Environment` per user data folder, created asynchronously;
//! `WebView` is one `ICoreWebView2Controller` whose parent is a winit window's HWND, with its
//! events queued as `webhost::Event`s the app drains in `about_to_wait`. Callers hold a `WebView`
//! and never a COM pointer.
//!
//! Everything here runs on the UI thread (winit's thread is an STA and owns COM: this file never
//! initializes or uninitializes it). The loader is webview2-com's static one (`WebView2LoaderStatic.lib`,
//! linked into tandem.exe): there is no `WebView2Loader.dll` to ship, plant or miss. Web content is
//! untrusted: pages get no host objects, every page-controlled value is capped before it is queued
//! (`webhost::ViewGate`), navigations pass the one rule (`webhost::navigation_allowed`), external
//! protocols are cancelled, permissions are denied, and downloads are left to WebView2's own
//! defaults (Edge's download UI, which applies the Mark of the Web; Tandem registers no
//! download handler and never changes that).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{
    AcceleratorKeyPressedEventHandler, CallDevToolsProtocolMethodCompletedHandler, FrameChildFrameCreatedEventHandler, FrameCreatedEventHandler, FrameDestroyedEventHandler, FrameWebMessageReceivedEventHandler, CapturePreviewCompletedHandler, DevToolsProtocolEventReceivedEventHandler, CoreWebView2CustomSchemeRegistration, CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler, DocumentTitleChangedEventHandler, ExecuteScriptCompletedHandler, FaviconChangedEventHandler, FocusChangedEventHandler, GetFaviconCompletedHandler, HistoryChangedEventHandler,
    LaunchingExternalUriSchemeEventHandler, NavigationCompletedEventHandler, NavigationStartingEventHandler, NewWindowRequestedEventHandler, PermissionRequestedEventHandler,
    ProcessFailedEventHandler, SourceChangedEventHandler, WebMessageReceivedEventHandler, WebResourceRequestedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{HWND, RECT};
use windows::core::{BOOL, Interface, PCWSTR, PWSTR};

use crate::browser_world;
use crate::webhost::{self, Answer, EnvOptions, Event, Posted, Sink, Store, ViewGate, ViewId};

/// Runs the body of a COM callback. A panic must never unwind into COM (that aborts Tandem with
/// every session, the 13u rule for callbacks): it becomes `E_FAIL`, so WebView2 ignores the event.
fn guard(body: impl FnOnce() -> windows::core::Result<()>) -> windows::core::Result<()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).unwrap_or_else(|_| Err(windows::Win32::Foundation::E_FAIL.into()))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn pwstr_out(f: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>) -> Option<String> {
    let mut p = PWSTR::null();
    f(&mut p).ok()?;
    Some(take_pwstr(p))
}

fn bool_out(f: impl FnOnce(*mut BOOL) -> windows::core::Result<()>) -> bool {
    let mut b = BOOL(0);
    f(&mut b).is_ok() && b.as_bool()
}

/// The root window that is in front right now (`GetAncestor(GetForegroundWindow(), GA_ROOT)`), None
/// when no window is.
pub fn foreground_root() -> Option<isize> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor, GetForegroundWindow};
    // SAFETY: both calls take and return plain handles.
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_null() {
        return None;
    }
    let root = unsafe { GetAncestor(fg, GA_ROOT) };
    (!root.is_null()).then_some(root as isize)
}

/// Gives the keyboard back to a Tandem window from a web view (a pane change by keyboard).
pub fn focus_window(hwnd: isize) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    // SAFETY: a plain handle; a handle that is gone makes the call fail harmlessly.
    unsafe { SetFocus(hwnd as *mut std::ffi::c_void) };
}

/// The WebView2 Runtime's version: the fixed-version folder's when one is given, else the
/// Evergreen runtime the system has. Err is an HRESULT: `Runtime::Missing`.
pub fn runtime_version(fixed: Option<&Path>) -> Result<String, i32> {
    scrub_env();
    let folder = fixed.map(|p| wide(&p.to_string_lossy()));
    let folder_ptr = folder.as_ref().map_or(PCWSTR::null(), |w| PCWSTR(w.as_ptr()));
    let mut out = PWSTR::null();
    // SAFETY: `folder` outlives the call; `out` is a plain out pointer the crate's helper frees.
    unsafe { GetAvailableCoreWebView2BrowserVersionString(folder_ptr, &mut out) }.map_err(|e| e.code().0)?;
    Ok(take_pwstr(out))
}

// --- Folders and the environment's inputs ---

/// `%LOCALAPPDATA%` from the shell's known folder (never the environment variable, which a parent
/// can redirect to a share), judged local (preflight P3).
pub fn local_appdata() -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath};
    let mut out: *mut u16 = std::ptr::null_mut();
    // SAFETY: `out` receives a CoTaskMem string that is freed below.
    let hr = unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, std::ptr::null_mut(), &mut out) };
    if hr < 0 || out.is_null() {
        return None;
    }
    let mut n = 0;
    // SAFETY: `out` is NUL-terminated.
    while unsafe { *out.add(n) } != 0 {
        n += 1;
    }
    let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(out, n) });
    unsafe { CoTaskMemFree(out as *const c_void) };
    let p = PathBuf::from(text);
    crate::localfs::judged(&p).then_some(p)
}

/// Makes `folder` (judged local, links never followed) with a protected owner-only access list: the
/// user's profile is not readable by any other account (preflight P3). The list is set when the
/// folder is new, and again (off this thread) only when a check finds it widened.
pub fn prepare_folder(folder: &Path) -> Result<(), String> {
    let judged = crate::localfs::LocalPath::judge(folder).ok_or_else(|| format!("{} is on a share or behind a link", folder.display()))?;
    let existed = crate::localfs::local_exists(judged.path());
    crate::localfs::create_dir_all(&judged).map_err(|e| format!("{}: {e}", folder.display()))?;
    if !existed {
        // New: the list is set once, before anything is in it (the cheap case).
        return protect_dir_owner_only(judged.path());
    }
    if crate::dynamic_profiles_trust::open(&judged, 0).is_ok_and(|o| o.dir && o.trust.is_ok()) {
        return Ok(());
    }
    // Setting a protected list walks the whole profile: a repair never runs on the UI thread. This
    // call fails (the caller retries on the next request) and a worker does it, one at a time.
    static REPAIRING: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    let dir = judged.path().to_path_buf();
    {
        let mut busy = REPAIRING.lock().unwrap_or_else(|e| e.into_inner());
        if !busy.contains(&dir) {
            busy.push(dir.clone());
            let worker = dir.clone();
            std::thread::spawn(move || {
                let _ = protect_dir_owner_only(&worker);
                REPAIRING.lock().unwrap_or_else(|e| e.into_inner()).retain(|p| *p != worker);
            });
        }
    }
    Err(format!("{}: its access list is being repaired; try again in a moment", folder.display()))
}

/// Every folder `protect_dir_owner_only` was asked to protect (tests count the calls).
#[cfg(test)]
static PROTECTED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// A protected DACL on a folder: full control for the current user only, inherited by what is made
/// inside it.
pub fn protect_dir_owner_only(dir: &Path) -> Result<(), String> {
    #[cfg(test)]
    PROTECTED.lock().unwrap().push(dir.to_path_buf());
    use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW};
    use windows_sys::Win32::Security::{ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR};
    let sid = crate::winpipe::user_sid()?;
    let sddl = wide(&format!("D:P(A;OICI;FA;;;{sid})"));
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: plain calls with NUL-terminated strings; the descriptor is freed below.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) == 0 {
            return Err(format!("the folder's access list could not be built: {}", std::io::Error::last_os_error()));
        }
        let (mut present, mut defaulted) = (0i32, 0i32);
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let got = GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted);
        let result = if got == 0 || present == 0 || dacl.is_null() {
            Err("the folder's access list is empty".to_string())
        } else {
            let mut path = wide(&dir.to_string_lossy());
            let err = SetNamedSecurityInfoW(path.as_mut_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, std::ptr::null_mut(), std::ptr::null_mut(), dacl, std::ptr::null());
            if err == 0 { Ok(()) } else { Err(format!("{}: the access list was refused ({})", dir.display(), std::io::Error::from_raw_os_error(err as i32))) }
        };
        LocalFree(sd as HLOCAL);
        result
    }
}

/// Removes the loader's override variables from Tandem's own environment before the first
/// environment is made (preflight P1): the loader lets them override the arguments, so an inherited
/// one could point the runtime at a planted folder.
pub fn scrub_env() {
    for name in [
        "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
        "WEBVIEW2_USER_DATA_FOLDER",
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "WEBVIEW2_RELEASE_CHANNEL_PREFERENCE",
        "WEBVIEW2_PIPE_FOR_SCRIPT_DEBUGGER",
        "WEBVIEW2_WAIT_FOR_SCRIPT_DEBUGGER",
    ] {
        // SAFETY: the Windows environment is process-wide and its calls are thread-safe; nothing reads these names from another thread.
        unsafe { std::env::remove_var(name) };
    }
}

/// Whether a process with this id is still running.
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: plain calls; the handle is closed.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            // Access denied means it exists; anything else means it is gone.
            return std::io::Error::last_os_error().raw_os_error() == Some(5);
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        !ok || code == STILL_ACTIVE
    }
}

/// Removes this run's Dev Null folder and every one whose process is gone (preflight P4), from
/// `Tmp` under the WebView2 folder. Pure file removal, no COM: it runs from the one process
/// teardown and at the first environment of a run. A folder a browser process still holds stays
/// until the next launch's sweep.
pub fn cleanup_files() {
    let Some(base) = local_appdata() else { return };
    // Saved Instant Replay movies are page pictures: they do not outlive the run that made them (P16).
    if let Some(dir) = crate::localfs::LocalPath::judge(crate::web_replay::replay_folder(&base)) {
        if let Ok(entries) = crate::localfs::entries(&dir) {
            for e in entries.iter().filter(|e| !e.stat.is_dir) {
                // A link is removed as a link, never followed.
                let _ = e.unlink();
            }
        }
    }
    let tmp = webhost::tmp_folder(&base);
    let Some(dir) = crate::localfs::LocalPath::judge(&tmp) else { return };
    let Ok(entries) = crate::localfs::entries(&dir) else { return };
    let names: Vec<String> = entries.iter().filter(|e| e.stat.is_dir).filter_map(|e| e.name.to_str().map(String::from)).collect();
    for name in webhost::stale_dev_null(&names, std::process::id(), &process_alive) {
        if let Some(e) = entries.iter().find(|e| e.name.to_str() == Some(name.as_str())) {
            // A link is removed as a link, never followed; a folder goes with its contents.
            if let Some(p) = e.local() {
                if e.stat.is_reparse { let _ = e.unlink(); } else { let _ = crate::localfs::remove_dir_all(&p); }
            }
        }
    }
}

/// Checks a `BrowserPluginPathHint` folder (preflight B2): the text and the one path judge
/// (`webhost::refused_hint`), then the disk: a folder (never a link) whose owner is the person,
/// Administrators or TrustedInstaller with no write for anyone else, holding `msedgewebview2.exe` and
/// `msedge.dll`, and an `msedgewebview2.exe` with a valid Authenticode signature from Microsoft.
/// Err is the reason, for one banner; the folder is then not used, and the runtime counts as
/// Missing (no silent fall back to the Evergreen runtime).
pub fn fixed_runtime_check(hint: &str) -> Result<PathBuf, String> {
    let judge = |p: &Path| crate::localfs::LocalPath::judge(p).is_some();
    let text = webhost::refused_hint(hint, &judge).map_err(|e| e.to_string())?;
    let folder = crate::localfs::LocalPath::judge(&text).ok_or("the folder is on a share or behind a link")?;
    let opened = crate::dynamic_profiles_trust::open(&folder, 0).map_err(|e| format!("the folder could not be examined ({e})"))?;
    if !opened.dir {
        return Err("it is not a folder".into());
    }
    opened.trust.map_err(|why| format!("the folder is not trusted: {why}"))?;
    let exe = folder.join("msedgewebview2.exe").ok_or("the folder name is not usable")?;
    let dll = folder.join("msedge.dll").ok_or("the folder name is not usable")?;
    if !crate::localfs::is_file(&exe) || !crate::localfs::is_file(&dll) {
        return Err("the folder holds no msedgewebview2.exe and msedge.dll".into());
    }
    signed_by_microsoft(exe.path()).map_err(|why| format!("msedgewebview2.exe is not signed by Microsoft ({why})"))?;
    Ok(folder.into_path_buf())
}

/// A valid Authenticode signature (WinVerifyTrust, no network revocation fetch) whose signer is
/// Microsoft Corporation.
fn signed_by_microsoft(path: &Path) -> Result<(), String> {
    use windows_sys::Win32::Security::Cryptography::{
        CERT_CONTEXT, CERT_FIND_SUBJECT_CERT, CERT_INFO, CERT_NAME_SIMPLE_DISPLAY_TYPE, CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED, CERT_QUERY_FORMAT_FLAG_BINARY, CERT_QUERY_OBJECT_FILE,
        CMSG_SIGNER_INFO_PARAM, CertCloseStore, CertFindCertificateInStore, CertFreeCertificateContext, CertGetNameStringW, CryptMsgClose, CryptMsgGetParam, CryptQueryObject, PKCS_7_ASN_ENCODING,
        X509_ASN_ENCODING,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WinVerifyTrust,
    };
    let wpath = wide(&path.to_string_lossy());
    // The guid of WINTRUST_ACTION_GENERIC_VERIFY_V2.
    let mut action = windows_sys::core::GUID { data1: 0x00aac56b, data2: 0xcd44, data3: 0x11d0, data4: [0x8c, 0xc2, 0x00, 0xc0, 0x4f, 0xc2, 0x95, 0xee] };
    let mut file = WINTRUST_FILE_INFO { cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32, pcwszFilePath: wpath.as_ptr(), hFile: std::ptr::null_mut(), pgKnownSubject: std::ptr::null_mut() };
    let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
    data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
    data.dwUIChoice = WTD_UI_NONE;
    data.fdwRevocationChecks = WTD_REVOKE_NONE;
    data.dwUnionChoice = WTD_CHOICE_FILE;
    data.Anonymous = WINTRUST_DATA_0 { pFile: &mut file };
    data.dwProvFlags = WTD_CACHE_ONLY_URL_RETRIEVAL;
    data.dwStateAction = WTD_STATEACTION_VERIFY;
    // SAFETY: `data` and `file` outlive both calls; INVALID_HANDLE_VALUE means no window.
    let status = unsafe { WinVerifyTrust(-1isize as _, &mut action, &mut data as *mut _ as *mut c_void) };
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    unsafe { WinVerifyTrust(-1isize as _, &mut action, &mut data as *mut _ as *mut c_void) };
    if status != 0 {
        return Err(format!("signature check 0x{status:08x}"));
    }
    // The signer's name, from the embedded signature.
    // SAFETY: plain out parameters; every handle and context opened below is closed.
    unsafe {
        let (mut store, mut msg): (*mut c_void, *mut c_void) = (std::ptr::null_mut(), std::ptr::null_mut());
        if CryptQueryObject(CERT_QUERY_OBJECT_FILE, wpath.as_ptr() as *const c_void, CERT_QUERY_CONTENT_FLAG_PKCS7_SIGNED_EMBED, CERT_QUERY_FORMAT_FLAG_BINARY, 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), &mut store, &mut msg, std::ptr::null_mut()) == 0 {
            return Err("no embedded signature".into());
        }
        let mut name = None;
        let mut size = 0u32;
        if CryptMsgGetParam(msg, CMSG_SIGNER_INFO_PARAM, 0, std::ptr::null_mut(), &mut size) != 0 {
            let mut buf = vec![0u8; size as usize];
            if CryptMsgGetParam(msg, CMSG_SIGNER_INFO_PARAM, 0, buf.as_mut_ptr() as *mut c_void, &mut size) != 0 {
                let signer = &*(buf.as_ptr() as *const windows_sys::Win32::Security::Cryptography::CMSG_SIGNER_INFO);
                let mut info: CERT_INFO = std::mem::zeroed();
                info.Issuer = signer.Issuer;
                info.SerialNumber = signer.SerialNumber;
                let ctx: *const CERT_CONTEXT = CertFindCertificateInStore(store, X509_ASN_ENCODING | PKCS_7_ASN_ENCODING, 0, CERT_FIND_SUBJECT_CERT, &info as *const _ as *const c_void, std::ptr::null());
                if !ctx.is_null() {
                    let mut text = [0u16; 256];
                    let n = CertGetNameStringW(ctx, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, std::ptr::null(), text.as_mut_ptr(), text.len() as u32);
                    if n > 1 {
                        name = Some(String::from_utf16_lossy(&text[..(n as usize - 1)]));
                    }
                    CertFreeCertificateContext(ctx);
                }
            }
        }
        CertCloseStore(store, 0);
        CryptMsgClose(msg);
        match name.as_deref() {
            Some("Microsoft Corporation") => Ok(()),
            Some(other) => Err(format!("signed by {other}")),
            None => Err("the signer is unknown".into()),
        }
    }
}

// --- Environment ---

/// One `ICoreWebView2Environment` over one user data folder. Cloning shares the COM object.
#[derive(Clone)]
pub struct Env {
    pub store: Store,
    env: ICoreWebView2Environment,
}

impl Env {
    /// Creates the environment for `store` over `folder` (made owner-only first). Asynchronous:
    /// `done` runs on the UI thread when the loop pumps messages (or at once, with an Err, when
    /// nothing can be created). A failure is posted by the caller and retried only on the next
    /// request.
    pub fn create(store: Store, folder: &Path, fixed: Option<&Path>, opts: &EnvOptions, done: Box<dyn FnOnce(Result<Env, String>)>) {
        // `done` runs once: from the completion handler, or here when nothing can be started.
        let done: Rc<RefCell<Option<Box<dyn FnOnce(Result<Env, String>)>>>> = Rc::new(RefCell::new(Some(done)));
        let finish = |done: &Rc<RefCell<Option<Box<dyn FnOnce(Result<Env, String>)>>>>, r: Result<Env, String>| {
            let f = done.borrow_mut().take();
            if let Some(f) = f {
                f(r);
            }
        };
        // The one argument that may reach the browser is the proxy's, exactly as `proxy_args` makes it.
        if !crate::adblock::proxy_args_ok(&opts.args) {
            return finish(&done, Err("the environment arguments were refused".to_string()));
        }
        scrub_env();
        if let Err(e) = prepare_folder(folder) {
            return finish(&done, Err(e));
        }
        let options = CoreWebView2EnvironmentOptions::default();
        // SAFETY: the setters write fields of an object no other thread has yet.
        unsafe {
            options.set_additional_browser_arguments(opts.args.clone());
            // No second process shares this profile.
            options.set_exclusive_user_data_folder_access(true);
            // The system's Stable runtime only (the crate's default also searches Beta, Dev and Canary).
            options.set_release_channels(COREWEBVIEW2_RELEASE_CHANNELS_STABLE);
            let regs: Vec<Option<ICoreWebView2CustomSchemeRegistration>> = opts
                .custom_schemes
                .iter()
                .map(|s| {
                    // Not secure, no authority component, no allowed origins (preflight P8).
                    let reg = CoreWebView2CustomSchemeRegistration::new(s.clone());
                    Some(reg.into())
                })
                .collect();
            options.set_scheme_registrations(regs);
        }
        let options: ICoreWebView2EnvironmentOptions = options.into();
        let (done_in, store_copy) = (done.clone(), store);
        let handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |hr, env| guard(|| {
            let result = match (hr, env) {
                (Ok(()), Some(env)) => Ok(Env { store: store_copy, env }),
                (Err(e), _) => Err(format!("the WebView2 environment could not be created: {e}")),
                (Ok(()), None) => Err("the WebView2 environment was not created".to_string()),
            };
            let f = done_in.borrow_mut().take();
            if let Some(f) = f {
                f(result);
            }
            Ok(())
        })));
        let wfolder = fixed.map(|p| wide(&p.to_string_lossy()));
        let wdata = wide(&folder.to_string_lossy());
        // SAFETY: the strings outlive the call; the loader takes its own references on the two objects.
        let hr = unsafe {
            CreateCoreWebView2EnvironmentWithOptions(wfolder.as_ref().map_or(PCWSTR::null(), |w| PCWSTR(w.as_ptr())), PCWSTR(wdata.as_ptr()), &options, &handler)
        };
        if let Err(e) = hr {
            // The handler is never called now: say so here.
            finish(&done, Err(format!("the loader refused to start the environment (0x{:08x})", e.code().0 as u32)));
        }
    }
}

// --- Views ---

/// How one view is made. Always built with `..Default::default()`: later plans add fields.
#[derive(Clone, Debug, Default)]
pub struct ViewSetup {
    /// `WebUserAgent`'s setting: Some(setting) applies `webhost::user_agent(setting, engine)` (an
    /// empty setting keeps the engine's agent with "Tandem" appended); None leaves the engine's own.
    pub user_agent: Option<String>,
    /// Scripts run at the start of every document, in every frame.
    pub scripts: Vec<String>,
    pub devtools: bool,
    /// `NewWindowRequested` posts `Event::NewWindow` for a user-initiated http, https or `iterm2-about:` request;
    /// either way the request is handled and no popup window ever appears.
    pub new_windows: bool,
    /// Request filters (URI patterns, all contexts); every matching request is posted as `Event::Request` with a
    /// deferral and waits for `answer_request`.
    pub resource_filters: Vec<String>,
    pub transparent: bool,
    pub scale: Option<f64>,
    /// `IsInPrivateModeEnabled` on this view's controller options (R13xy-3).
    pub in_private: bool,
    /// The view's NavigationStarting handler asks `navigation_allowed` and cancels a refused
    /// navigation. Every view sets it (preflight B4c).
    pub guard_local_schemes: bool,
    /// Only these schemes may load (compared ignoring case); None allows any the guard allows.
    pub allowed_schemes: Option<Vec<String>>,
    /// `IsWebMessageEnabled`: true only for popover HTML, tool views and `iterm2-about:` pages (preflight P5).
    pub web_message: bool,
    /// The page's favicon is posted as `Event::Favicon` (browser panes): WebView2's own fetch,
    /// bounded and judged by `webhost::favicon_ok` before it is queued.
    pub favicon: bool,
    /// The page's right-click menu is the app's to decorate: `ContextMenuRequested` posts
    /// `Event::ContextMenu` and waits for `answer_context_menu` (browser panes).
    pub context_menu: bool,
}

#[derive(Clone)]
struct Core {
    ctrl: ICoreWebView2Controller,
    view: ICoreWebView2,
}

struct Pending {
    deferral: ICoreWebView2Deferral,
    args: ICoreWebView2WebResourceRequestedEventArgs,
}

/// A context menu held back until the app answers (`answer_context_menu`).
struct PendingMenu {
    deferral: ICoreWebView2Deferral,
    args: ICoreWebView2ContextMenuRequestedEventArgs,
}

/// What a handler posts through: the view's id, the shared sink and the view's gate.
#[derive(Clone)]
struct Poster {
    view: ViewId,
    tx: Sink,
    gate: std::sync::Arc<ViewGate>,
    last_log: Rc<RefCell<Option<Instant>>>,
}

impl Poster {
    fn post(&self, ev: Event) {
        match self.gate.admit(&ev) {
            Ok(charge) => {
                let _ = self.tx.send(Posted::event(ev, Some(charge)));
            }
            Err(()) => {
                let mut last = self.last_log.borrow_mut();
                if last.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
                    *last = Some(Instant::now());
                    let dropped = self.gate.take_dropped();
                    crate::dlog!("webhost: view {} dropped {} page events", self.view, dropped);
                }
            }
        }
    }
}

struct Inner {
    id: ViewId,
    poster: Poster,
    env: Env,
    setup: ViewSetup,
    core: Option<Core>,
    queued: Vec<Box<dyn FnOnce(&Core)>>,
    /// NavigationStarting's URI by NavigationId, for `Navigated` (at most 64 kept).
    nav_uris: VecDeque<(u64, String)>,
    /// The URI of the navigation `navigate_tandem` started, until the first NavigationStarting
    /// that matches takes it (or 30 s pass): preflight P19.
    tandem_uri: Option<(String, Instant)>,
    new_windows: webhost::NewWindowLimiter,
    pending: HashMap<u64, Pending>,
    next_token: u64,
    last_bounds: Option<(i32, i32, i32, i32)>,
    visible: Option<bool>,
    /// One DevTools event receiver per event name (`devtools_subscribe`), with its handler's token.
    devtools: HashMap<String, (ICoreWebView2DevToolsProtocolEventReceiver, i64)>,
    /// What the app is told of the DevTools events (final review I4): only what it acts on, the
    /// lifecycle held behind one wake.
    devtools_filter: browser_world::EventFilter,
    /// The page world's iframes while `watch_frames` is on (at most `MAX_FRAMES`), by WebView2's
    /// frame id; the COM pointer lives only here and goes on `Destroyed`.
    frames: HashMap<u64, ICoreWebView2Frame2>,
    /// The view's `FrameCreated` registration, removed in `shut`.
    frame_watch: Option<(ICoreWebView2_4, i64)>,
    /// Menus waiting for an answer, by token (at most `webhost::MAX_PENDING_MENUS`).
    menus: HashMap<u64, PendingMenu>,
    /// Asked inside `AcceleratorKeyPressed` (Task 6).
    key_gate: webhost::KeyGate,
}

/// One WebView2 controller. Dropping it closes the controller; `close` says so.
pub struct WebView {
    inner: Rc<RefCell<Inner>>,
}

const MAX_NAV_URIS: usize = 64;
const MAX_PENDING_REQUESTS: usize = 256;
/// Page-world frames tracked at once (a page that makes frames without end cannot grow the table).
const MAX_FRAMES: usize = 512;

fn local_judge(p: &Path) -> bool {
    crate::localfs::LocalPath::judge(p).is_some()
}

fn rect_of(b: (i32, i32, i32, i32)) -> RECT {
    RECT { left: b.0, top: b.1, right: b.2, bottom: b.3 }
}

impl WebView {
    /// Starts creating a view in `parent_hwnd` (a winit window's HWND). Asynchronous: `Event::Created`
    /// or `Event::Failed` follows; calls made before then are kept and run when the view exists.
    pub fn create(env: &Env, parent_hwnd: isize, id: ViewId, bounds: (i32, i32, i32, i32), setup: ViewSetup, sink: Sink) -> WebView {
        let poster = Poster { view: id, tx: sink, gate: ViewGate::new(), last_log: Rc::new(RefCell::new(None)) };
        let inner = Rc::new(RefCell::new(Inner {
            id,
            poster: poster.clone(),
            env: env.clone(),
            setup: setup.clone(),
            core: None,
            queued: Vec::new(),
            nav_uris: VecDeque::new(),
            tandem_uri: None,
            new_windows: webhost::NewWindowLimiter::default(),
            pending: HashMap::new(),
            next_token: 1,
            last_bounds: None,
            visible: None,
            devtools: HashMap::new(),
            devtools_filter: Default::default(),
            frames: HashMap::new(),
            frame_watch: None,
            menus: HashMap::new(),
            key_gate: webhost::KeyGate::default(),
        }));
        let weak = Rc::downgrade(&inner);
        // Every view is known to the process teardown (`close_all`), whatever owns it.
        LIVE.with(|l| {
            if let Ok(mut l) = l.try_borrow_mut() {
                l.retain(|w| w.strong_count() > 0);
                l.push(weak.clone());
            }
        });
        let fail_poster = poster.clone();
        let handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |hr, ctrl| guard(|| {
            let Some(inner) = weak.upgrade() else { return Ok(()) };
            match (hr, ctrl) {
                (Ok(()), Some(ctrl)) => match unsafe { ctrl.CoreWebView2() } {
                    Ok(view) => {
                        let core = Core { ctrl, view };
                        match configure(&inner, &core, bounds) {
                            Ok(()) => {
                                let queued = {
                                    let mut i = inner.borrow_mut();
                                    i.core = Some(core.clone());
                                    std::mem::take(&mut i.queued)
                                };
                                inner.borrow().poster.post(Event::Created(id));
                                for q in queued {
                                    q(&core);
                                }
                            }
                            Err(e) => poster_failed(&inner, format!("the web view could not be set up: {e}")),
                        }
                    }
                    Err(e) => poster_failed(&inner, format!("the web view has no core: {e}")),
                },
                (Err(e), _) => poster_failed(&inner, format!("the web view could not be created: {e}")),
                (Ok(()), None) => poster_failed(&inner, "the web view was not created".to_string()),
            }
            Ok(())
        })));
        let hwnd = HWND(parent_hwnd as *mut c_void);
        let started = (|| -> Result<(), String> {
            if setup.in_private {
                let e10 = env.env.cast::<ICoreWebView2Environment10>().map_err(|e| format!("InPrivate views need a newer WebView2 Runtime ({e})"))?;
                let options = unsafe { e10.CreateCoreWebView2ControllerOptions() }.map_err(|e| e.to_string())?;
                unsafe { options.SetIsInPrivateModeEnabled(true) }.map_err(|e| e.to_string())?;
                unsafe { e10.CreateCoreWebView2ControllerWithOptions(hwnd, &options, &handler) }.map_err(|e| e.to_string())
            } else {
                unsafe { env.env.CreateCoreWebView2Controller(hwnd, &handler) }.map_err(|e| e.to_string())
            }
        })();
        if let Err(e) = started {
            fail_poster.post(Event::Failed(id, e));
        }
        WebView { inner }
    }

    /// Runs `f` now, or when the view exists. The borrow is released before `f` runs (a COM call
    /// can re-enter a handler).
    fn with(&self, f: impl FnOnce(&Core) + 'static) {
        let core = self.inner.borrow().core.clone();
        match core {
            Some(c) => f(&c),
            None => self.inner.borrow_mut().queued.push(Box::new(f)),
        }
    }

    pub fn navigate(&self, url: &str) {
        let w = wide(url);
        self.with(move |c| {
            let _ = unsafe { c.view.Navigate(PCWSTR(w.as_ptr())) };
        });
    }

    /// A navigation Tandem itself starts (never for a URL from a page, a link or a model): the
    /// URL is remembered so the guarded NavigationStarting passes `by_tandem` for exactly that
    /// URI, once.
    pub fn navigate_tandem(&self, url: &str) {
        self.inner.borrow_mut().tandem_uri = Some((url.to_string(), Instant::now()));
        self.navigate(url);
    }

    /// Sets the physical rectangle (left, top, right, bottom) in the parent's client coordinates;
    /// a COM call only when it changed (preflight P24).
    pub fn set_bounds(&self, b: (i32, i32, i32, i32)) {
        {
            let mut i = self.inner.borrow_mut();
            if i.last_bounds == Some(b) {
                return;
            }
            i.last_bounds = Some(b);
        }
        self.with(move |c| {
            let _ = unsafe { c.ctrl.SetBounds(rect_of(b)) };
        });
    }

    pub fn set_visible(&self, on: bool) {
        {
            let mut i = self.inner.borrow_mut();
            if i.visible == Some(on) {
                return;
            }
            i.visible = Some(on);
        }
        self.with(move |c| {
            let _ = unsafe { c.ctrl.SetIsVisible(on) };
        });
    }

    /// Moves the view to another window (a pane that changed windows).
    pub fn set_parent(&self, hwnd: isize) {
        self.with(move |c| {
            let _ = unsafe { c.ctrl.SetParentWindow(HWND(hwnd as *mut c_void)) };
            let _ = unsafe { c.ctrl.NotifyParentWindowPositionChanged() };
        });
    }

    /// The table the accelerator handler asks before it marks a key handled.
    pub fn set_key_gate(&self, gate: webhost::KeyGate) {
        self.inner.borrow_mut().key_gate = gate;
    }

    pub fn focus(&self) {
        self.with(|c| {
            let _ = unsafe { c.ctrl.MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC) };
        });
    }

    /// Runs `js` in the page; `Event::ScriptDone { token, .. }` carries the JSON result or the error.
    pub fn execute_script(&self, js: &str, token: u64) {
        let w = wide(js);
        let (poster, id) = {
            let i = self.inner.borrow();
            (i.poster.clone(), i.id)
        };
        self.with(move |c| {
            let p = poster.clone();
            let handler = ExecuteScriptCompletedHandler::create(Box::new(move |hr, json| guard(|| {
                let json = hr.map(|_| json).map_err(|e| e.to_string());
                p.post(Event::ScriptDone { view: id, token, json });
                Ok(())
            })));
            if let Err(e) = unsafe { c.view.ExecuteScript(PCWSTR(w.as_ptr()), &handler) } {
                poster.post(Event::ScriptDone { view: id, token, json: Err(e.to_string()) });
            }
        });
    }

    /// `ICoreWebView2::CallDevToolsProtocolMethod`: `Event::ScriptDone { token, .. }` carries the
    /// result JSON or the error text. The DevTools protocol is reached only from Rust (no debugging port).
    pub fn devtools_call(&self, method: &str, params_json: &str, token: u64) {
        self.devtools_call_in(None, method, params_json, token);
    }

    /// `ICoreWebView2_11::CallDevToolsProtocolMethodForSession`: the same, in a child session
    /// (a cross-site frame attached with `Target.setAutoAttach`).
    pub fn devtools_call_for_session(&self, session: &str, method: &str, params_json: &str, token: u64) {
        self.devtools_call_in(Some(session.to_string()), method, params_json, token);
    }

    fn devtools_call_in(&self, session: Option<String>, method: &str, params_json: &str, token: u64) {
        // The one DevTools sender: a closed set of methods (P22).
        if !browser_world::method_allowed(method) {
            let i = self.inner.borrow();
            i.poster.post(Event::ScriptDone { view: i.id, token, json: Err("that DevTools method is not allowed".into()) });
            return;
        }
        let (wm, wp) = (wide(method), wide(params_json));
        let ws = session.as_deref().map(wide);
        let (poster, id) = {
            let i = self.inner.borrow();
            (i.poster.clone(), i.id)
        };
        self.with(move |c| {
            let p = poster.clone();
            let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |hr, json| guard(|| {
                let json = hr.map(|_| json).map_err(|e| e.to_string());
                p.post(Event::ScriptDone { view: id, token, json });
                Ok(())
            })));
            let started = match &ws {
                None => unsafe { c.view.CallDevToolsProtocolMethod(PCWSTR(wm.as_ptr()), PCWSTR(wp.as_ptr()), &handler) },
                Some(ws) => match c.view.cast::<ICoreWebView2_11>() {
                    Ok(v11) => unsafe { v11.CallDevToolsProtocolMethodForSession(PCWSTR(ws.as_ptr()), PCWSTR(wm.as_ptr()), PCWSTR(wp.as_ptr()), &handler) },
                    Err(e) => Err(e),
                },
            };
            if let Err(e) = started {
                poster.post(Event::ScriptDone { view: id, token, json: Err(e.to_string()) });
            }
        });
    }

    /// The lifecycle events the view's filter holds, oldest first (after an `Event::DevToolsHeld`,
    /// and before a binding call is routed, so a context is known before its first call).
    pub fn take_devtools_held(&self) -> Vec<(Option<String>, String, String)> {
        self.inner.borrow_mut().devtools_filter.take()
    }

    /// Posts `Event::DevTools` for every `event` the page's target (and its attached children)
    /// raises that the app acts on (`browser_world::EventFilter`: a binding call as it is, the
    /// lifecycle behind one `Event::DevToolsHeld`). One receiver per event name per view; it is
    /// dropped with the view.
    pub fn devtools_subscribe(&self, event: &str) {
        if self.inner.borrow().devtools.contains_key(event) {
            return;
        }
        let name = event.to_string();
        let wname = wide(event);
        let weak = Rc::downgrade(&self.inner);
        self.with(move |c| {
            let Some(inner) = weak.upgrade() else { return };
            let Ok(receiver) = (unsafe { c.view.GetDevToolsProtocolEventReceiver(PCWSTR(wname.as_ptr())) }) else { return };
            let (weak2, ev_name) = (Rc::downgrade(&inner), name.clone());
            let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_sender, args| guard(|| {
                let (Some(inner), Some(args)) = (weak2.upgrade(), args) else { return Ok(()) };
                let Some(json) = pwstr_out(|p| unsafe { args.ParameterObjectAsJson(p) }) else { return Ok(()) };
                if json.len() > webhost::devtools_json_cap() {
                    inner.borrow().poster.gate.note_dropped();
                    return Ok(());
                }
                // An empty session id means the view's own target.
                let session = args.cast::<ICoreWebView2DevToolsProtocolEventReceivedEventArgs2>().ok().and_then(|a| pwstr_out(|p| unsafe { a.SessionId(p) })).filter(|s| !s.is_empty());
                // Only what the app acts on reaches the queue; the lifecycle waits behind one wake.
                let offer = inner.borrow_mut().devtools_filter.offer(session.as_deref(), &ev_name, &json);
                let i = inner.borrow();
                match offer {
                    browser_world::Offer::Post => i.poster.post(Event::DevTools { view: i.id, session, name: ev_name.clone(), json }),
                    browser_world::Offer::Wake => i.poster.post(Event::DevToolsHeld { view: i.id }),
                    browser_world::Offer::Hold => {}
                    browser_world::Offer::Drop => i.poster.gate.note_dropped(),
                }
                Ok(())
            })));
            let mut token = 0i64;
            if unsafe { receiver.add_DevToolsProtocolEventReceived(&handler, &mut token) }.is_ok() {
                inner.borrow_mut().devtools.insert(name, (receiver, token));
            }
        });
    }

    /// Tracks the page world's iframes: `Event::FrameCreated` and `FrameGone` for every frame (nested
    /// ones too), `Event::FrameMessage` for what each posts, and `execute_script_in_frame` for each.
    /// Asked for only by a consumer of the page-world frames (nothing in Task 1 needs it, P24); twice
    /// is safe. A runtime without `ICoreWebView2_4` leaves the view with no frame events.
    pub fn watch_frames(&self) {
        if self.inner.borrow().frame_watch.is_some() {
            return;
        }
        let weak = Rc::downgrade(&self.inner);
        self.with(move |c| {
            let Some(inner) = weak.upgrade() else { return };
            if inner.borrow().frame_watch.is_some() {
                return;
            }
            let Ok(v4) = c.view.cast::<ICoreWebView2_4>() else { return };
            let weak2 = Rc::downgrade(&inner);
            let created = FrameCreatedEventHandler::create(Box::new(move |_sender, args| guard(|| {
                let (Some(inner), Some(args)) = (weak2.upgrade(), args) else { return Ok(()) };
                if let Ok(f) = unsafe { args.Frame() } {
                    track_frame(&inner, f);
                }
                Ok(())
            })));
            let mut token = 0i64;
            if unsafe { v4.add_FrameCreated(&created, &mut token) }.is_ok() {
                inner.borrow_mut().frame_watch = Some((v4, token));
            }
        });
    }

    /// Runs `js` in the PAGE world of one frame (`ICoreWebView2Frame2::ExecuteScript`);
    /// `Event::ScriptDone { token, .. }` answers. Never the Tandem world.
    pub fn execute_script_in_frame(&self, frame: u64, js: &str, token: u64) {
        let w = wide(js);
        let (poster, id, weak) = {
            let i = self.inner.borrow();
            (i.poster.clone(), i.id, Rc::downgrade(&self.inner))
        };
        self.with(move |_c| {
            let f2 = weak.upgrade().and_then(|i| i.borrow().frames.get(&frame).cloned());
            let Some(f2) = f2 else {
                return poster.post(Event::ScriptDone { view: id, token, json: Err("no such frame".into()) });
            };
            let p = poster.clone();
            let handler = ExecuteScriptCompletedHandler::create(Box::new(move |hr, json| guard(|| {
                let json = hr.map(|_| json).map_err(|e| e.to_string());
                p.post(Event::ScriptDone { view: id, token, json });
                Ok(())
            })));
            if let Err(e) = unsafe { f2.ExecuteScript(PCWSTR(w.as_ptr()), &handler) } {
                poster.post(Event::ScriptDone { view: id, token, json: Err(e.to_string()) });
            }
        });
    }

    /// Posts a JSON message to the page's `window.chrome.webview` listeners.
    pub fn post_json(&self, json: &str) {
        let w = wide(json);
        self.with(move |c| {
            let _ = unsafe { c.view.PostWebMessageAsJson(PCWSTR(w.as_ptr())) };
        });
    }

    pub fn go_back(&self) {
        self.with(|c| {
            let _ = unsafe { c.view.GoBack() };
        });
    }

    pub fn go_forward(&self) {
        self.with(|c| {
            let _ = unsafe { c.view.GoForward() };
        });
    }

    pub fn reload(&self) {
        self.with(|c| {
            let _ = unsafe { c.view.Reload() };
        });
    }

    pub fn stop(&self) {
        self.with(|c| {
            let _ = unsafe { c.view.Stop() };
        });
    }

    pub fn set_zoom(&self, factor: f64) {
        self.with(move |c| {
            let _ = unsafe { c.ctrl.SetZoomFactor(factor) };
        });
    }

    /// Captures the page as a PNG; `Event::ScriptDone { token, json: Ok(base64) }` follows (at most 16 MiB).
    pub fn capture_png(&self, token: u64) {
        self.capture(token, false);
    }

    /// Captures the page as a JPEG (`jpeg`) or a PNG, the same way: the encoding runs in the browser
    /// process, not on Tandem's thread, and `Event::ScriptDone` carries the base64 bytes.
    pub fn capture(&self, token: u64, jpeg: bool) {
        let (poster, id) = {
            let i = self.inner.borrow();
            (i.poster.clone(), i.id)
        };
        self.with(move |c| {
            use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
            use windows::Win32::System::Com::{IStream, STREAM_SEEK_SET};
            let stream: windows::core::Result<IStream> = unsafe { CreateStreamOnHGlobal(Default::default(), true) };
            let Ok(stream) = stream else {
                return poster.post(Event::ScriptDone { view: id, token, json: Err("no stream for the capture".into()) });
            };
            let (p, s) = (poster.clone(), stream.clone());
            let handler = CapturePreviewCompletedHandler::create(Box::new(move |hr| guard(|| {
                let json = hr.map_err(|e| e.to_string()).and_then(|_| {
                    unsafe { s.Seek(0, STREAM_SEEK_SET, None) }.map_err(|e| e.to_string())?;
                    let mut out: Vec<u8> = Vec::new();
                    let mut buf = vec![0u8; 64 * 1024];
                    loop {
                        let mut read = 0u32;
                        let hr = unsafe { s.Read(buf.as_mut_ptr() as *mut c_void, buf.len() as u32, Some(&mut read as *mut u32)) };
                        if hr.is_err() {
                            return Err("the capture could not be read".to_string());
                        }
                        if read == 0 {
                            break;
                        }
                        out.extend_from_slice(&buf[..read as usize]);
                        if out.len() > 16 * 1024 * 1024 {
                            return Err("the capture is too large".to_string());
                        }
                    }
                    use base64::Engine as _;
                    Ok(base64::engine::general_purpose::STANDARD.encode(&out))
                });
                p.post(Event::ScriptDone { view: id, token, json });
                Ok(())
            })));
            if let Err(e) = unsafe { c.view.CapturePreview(if jpeg { COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG } else { COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG }, &stream, &handler) } {
                poster.post(Event::ScriptDone { view: id, token, json: Err(e.to_string()) });
            }
        });
    }

    /// Completes the deferral taken when `Event::Request { token, .. }` was posted.
    pub fn answer_request(&self, token: u64, answer: Answer) {
        let (pending, env) = {
            let mut i = self.inner.borrow_mut();
            (i.pending.remove(&token), i.env.clone())
        };
        let Some(p) = pending else { return };
        match answer {
            Answer::Continue => {}
            Answer::Block => {
                if let Ok(resp) = unsafe { env.env.CreateWebResourceResponse(None::<&windows::Win32::System::Com::IStream>, 403, windows::core::w!("Blocked"), windows::core::w!("")) } {
                    let _ = unsafe { p.args.SetResponse(&resp) };
                }
            }
            Answer::Respond { status, mime, body, headers } => {
                use windows::Win32::UI::Shell::SHCreateMemStream;
                let stream = unsafe { SHCreateMemStream(Some(&body)) };
                let head = wide(&webhost::response_headers(&mime, &headers));
                let reason = windows::core::w!("OK");
                if let Ok(resp) = unsafe { env.env.CreateWebResourceResponse(stream.as_ref(), i32::from(status), reason, PCWSTR(head.as_ptr())) } {
                    let _ = unsafe { p.args.SetResponse(&resp) };
                }
            }
        }
        let _ = unsafe { p.deferral.Complete() };
    }

    /// The system print dialog for the page (iTerm2's print panel).
    pub fn print_ui(&self) {
        self.with(|c| {
            if let Ok(v16) = c.view.cast::<ICoreWebView2_16>() {
                let _ = unsafe { v16.ShowPrintUI(COREWEBVIEW2_PRINT_DIALOG_KIND_SYSTEM) };
            }
        });
    }

    /// Completes the deferral taken when `Event::ContextMenu { token, .. }` was posted: WebView2's own
    /// items minus `REMOVED_NATIVE`, the entries before `Native` in front of them and the rest behind
    /// (no `Native` entry: all in front). An empty list shows the native menu unchanged. A chosen
    /// custom item posts `Event::ContextMenuPicked`. Separators the removal leaves at an end or in a
    /// pair are taken out.
    pub fn answer_context_menu(&self, token: u64, entries: Vec<webhost::MenuEntry>) {
        let (pending, env, poster, id) = {
            let mut i = self.inner.borrow_mut();
            (i.menus.remove(&token), i.env.clone(), i.poster.clone(), i.id)
        };
        let Some(p) = pending else { return };
        if !entries.is_empty() {
            let _ = decorate_menu(&p.args, &env, &poster, id, token, &entries);
        }
        #[cfg(test)]
        LAST_MENU.with(|m| *m.borrow_mut() = menu_labels(&p.args));
        let _ = unsafe { p.deferral.Complete() };
    }

    /// Closes the view (the same as dropping it).
    pub fn close(self) {}
}

impl Drop for WebView {
    fn drop(&mut self) {
        shut(&self.inner);
    }
}

thread_local! {
    /// Every view made on this (the UI) thread, for the process teardown.
    static LIVE: RefCell<Vec<std::rc::Weak<RefCell<Inner>>>> = const { RefCell::new(Vec::new()) };
}

/// Completes a view's pending requests with Continue and closes its controller (its browser
/// process lets go of the user data folder). Twice is safe: the second finds nothing.
fn shut(inner: &Rc<RefCell<Inner>>) {
    let Ok(mut i) = inner.try_borrow_mut() else { return };
    let (pending, core, receivers) = (std::mem::take(&mut i.pending), i.core.take(), std::mem::take(&mut i.devtools));
    let (frames, watch) = (std::mem::take(&mut i.frames), i.frame_watch.take());
    let menus = std::mem::take(&mut i.menus);
    drop(i);
    // A menu still waiting shows WebView2's own.
    for (_, m) in menus {
        let _ = unsafe { m.deferral.Complete() };
    }
    drop(frames);
    if let Some((v4, token)) = watch {
        let _ = unsafe { v4.remove_FrameCreated(token) };
    }
    for (_, (receiver, token)) in receivers {
        let _ = unsafe { receiver.remove_DevToolsProtocolEventReceived(token) };
    }
    for (_, p) in pending {
        let _ = unsafe { p.deferral.Complete() };
    }
    if let Some(c) = core {
        let _ = unsafe { c.ctrl.Close() };
    }
}

/// The process teardown's part (13w's one `teardown_process`, every way out): every live web view
/// on this thread is closed before the Dev Null folder is removed, so no browser process still
/// holds it. The owners' later drops find nothing to do. Returns how many were closed.
pub fn close_all() -> usize {
    let views = LIVE.with(|l| l.try_borrow_mut().map(|mut l| std::mem::take(&mut *l)).unwrap_or_default());
    let mut closed = 0;
    for w in views {
        if let Some(inner) = w.upgrade() {
            shut(&inner);
            closed += 1;
        }
    }
    closed
}

fn poster_failed(inner: &Rc<RefCell<Inner>>, why: String) {
    let i = inner.borrow();
    i.poster.post(Event::Failed(i.id, why));
}

/// Applies the setup and registers every handler on a new controller.
fn configure(inner: &Rc<RefCell<Inner>>, core: &Core, bounds: (i32, i32, i32, i32)) -> Result<(), String> {
    let setup = inner.borrow().setup.clone();
    let (view, ctrl) = (&core.view, &core.ctrl);
    let e = |what: &str, err: windows::core::Error| format!("{what}: {err}");
    unsafe {
        ctrl.SetBounds(rect_of(bounds)).map_err(|x| e("bounds", x))?;
        ctrl.SetIsVisible(true).map_err(|x| e("visible", x))?;
        let s = view.Settings().map_err(|x| e("settings", x))?;
        // No host objects for pages, ever (preflight P5): a page reaches the host only by `postMessage`.
        s.SetAreHostObjectsAllowed(false).map_err(|x| e("host objects", x))?;
        s.SetIsWebMessageEnabled(setup.web_message).map_err(|x| e("web message", x))?;
        s.SetAreDevToolsEnabled(setup.devtools).map_err(|x| e("devtools", x))?;
        // Autofill and password saving belong to 13x7.
        if let Ok(s4) = s.cast::<ICoreWebView2Settings4>() {
            let _ = s4.SetIsGeneralAutofillEnabled(false);
            let _ = s4.SetIsPasswordAutosaveEnabled(false);
        }
        if let Some(setting) = &setup.user_agent {
            if let Ok(s2) = s.cast::<ICoreWebView2Settings2>() {
                let engine = pwstr_out(|p| s2.UserAgent(p)).unwrap_or_default();
                let ua = wide(&webhost::user_agent(setting, &engine));
                let _ = s2.SetUserAgent(PCWSTR(ua.as_ptr()));
            }
        }
        if setup.transparent {
            if let Ok(c2) = ctrl.cast::<ICoreWebView2Controller2>() {
                let _ = c2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR { A: 0, R: 0, G: 0, B: 0 });
            }
        }
        if let Some(scale) = setup.scale {
            if let Ok(c3) = ctrl.cast::<ICoreWebView2Controller3>() {
                let _ = c3.SetRasterizationScale(scale);
            }
        }
    }
    for script in &setup.scripts {
        let w = wide(script);
        let handler = webview2_com::AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(|_hr, _id| guard(|| Ok(()))));
        unsafe { view.AddScriptToExecuteOnDocumentCreated(PCWSTR(w.as_ptr()), &handler) }.map_err(|x| e("script", x))?;
    }
    let mut token = 0i64;
    register_navigation(inner, view, &mut token).map_err(|x| e("navigation", x))?;
    register_page_events(inner, view, &mut token).map_err(|x| e("page events", x))?;
    register_policy(inner, view, &mut token).map_err(|x| e("policy", x))?;
    register_controller_events(inner, ctrl, &mut token).map_err(|x| e("focus", x))?;
    if setup.context_menu {
        register_context_menu(inner, view, &mut token).map_err(|x| e("context menu", x))?;
    }
    if !setup.resource_filters.is_empty() {
        register_requests(inner, view, &setup.resource_filters, &mut token).map_err(|x| e("requests", x))?;
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// The last answered menu's items for the ignored runtime test: "---" for a separator, else
    /// "name:label" (WebView2 names a custom item by its label), or the label when there is no name.
    static LAST_MENU: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn menu_labels(args: &ICoreWebView2ContextMenuRequestedEventArgs) -> Vec<String> {
    let Ok(list) = (unsafe { args.MenuItems() }) else { return Vec::new() };
    let mut n = 0u32;
    let _ = unsafe { list.Count(&mut n) };
    (0..n)
        .filter_map(|i| unsafe { list.GetValueAtIndex(i) }.ok())
        .map(|it| {
            let (kind, name) = menu_item_info(&it);
            if kind == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR {
                "---".to_string()
            } else {
                let label = pwstr_out(|p| unsafe { it.Label(p) }).unwrap_or_default();
                if name.is_empty() { label } else { format!("{name}:{label}") }
            }
        })
        .collect()
}

/// The kind of an item of WebView2's menu, and its `Name`.
fn menu_item_info(item: &ICoreWebView2ContextMenuItem) -> (COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND, String) {
    let mut kind = COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND;
    let _ = unsafe { item.Kind(&mut kind) };
    (kind, pwstr_out(|p| unsafe { item.Name(p) }).unwrap_or_default())
}

/// Builds one custom item (or separator) for `decorate_menu`.
fn make_menu_item(env9: &ICoreWebView2Environment9, entry: &webhost::MenuEntry, poster: &Poster, view: ViewId, token: u64) -> Option<ICoreWebView2ContextMenuItem> {
    use windows::Win32::System::Com::IStream;
    use windows::Win32::UI::Shell::SHCreateMemStream;
    match entry {
        webhost::MenuEntry::Separator => {
            let empty = wide("");
            unsafe { env9.CreateContextMenuItem(PCWSTR(empty.as_ptr()), None::<&IStream>, COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR) }.ok()
        }
        webhost::MenuEntry::Custom { id, label, enabled, icon_png } => {
            let w = wide(label);
            let icon = icon_png.as_ref().and_then(|b| unsafe { SHCreateMemStream(Some(b)) });
            let item = unsafe { env9.CreateContextMenuItem(PCWSTR(w.as_ptr()), icon.as_ref(), COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND) }.ok()?;
            if !*enabled {
                let _ = unsafe { item.SetIsEnabled(false) };
            }
            let (poster, id) = (poster.clone(), *id);
            let handler = webview2_com::CustomItemSelectedEventHandler::create(Box::new(move |_sender, _| guard(|| {
                poster.post(Event::ContextMenuPicked { view, token, id });
                Ok(())
            })));
            let mut t = 0i64;
            let _ = unsafe { item.add_CustomItemSelected(&handler, &mut t) };
            Some(item)
        }
        webhost::MenuEntry::Native => None,
    }
}

/// Edits the menu of `args` in place (see `WebView::answer_context_menu`).
fn decorate_menu(args: &ICoreWebView2ContextMenuRequestedEventArgs, env: &Env, poster: &Poster, view: ViewId, token: u64, entries: &[webhost::MenuEntry]) -> windows::core::Result<()> {
    let env9 = env.env.cast::<ICoreWebView2Environment9>()?;
    let list = unsafe { args.MenuItems() }?;
    // WebView2's own items without the ones Tandem replaces, then without stray separators.
    let count = |l: &ICoreWebView2ContextMenuItemCollection| {
        let mut n = 0u32;
        let _ = unsafe { l.Count(&mut n) };
        n
    };
    let mut at = count(&list);
    while at > 0 {
        at -= 1;
        if let Ok(item) = unsafe { list.GetValueAtIndex(at) } {
            if webhost::REMOVED_NATIVE.contains(&menu_item_info(&item).1.as_str()) {
                let _ = unsafe { list.RemoveValueAtIndex(at) };
            }
        }
    }
    let mut at = count(&list);
    while at > 0 {
        at -= 1;
        let sep = |i: u32| unsafe { list.GetValueAtIndex(i) }.ok().is_some_and(|it| menu_item_info(&it).0 == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR);
        let n = count(&list);
        if sep(at) && (at == 0 || at + 1 == n || sep(at - 1)) {
            let _ = unsafe { list.RemoveValueAtIndex(at) };
        }
    }
    let split = entries.iter().position(|e| matches!(e, webhost::MenuEntry::Native)).unwrap_or(entries.len());
    let (front, back) = entries.split_at(split);
    let mut index = 0u32;
    for e in front {
        if let Some(item) = make_menu_item(&env9, e, poster, view, token) {
            unsafe { list.InsertValueAtIndex(index, &item) }?;
            index += 1;
        }
    }
    for e in back.iter().skip(1) {
        if let Some(item) = make_menu_item(&env9, e, poster, view, token) {
            let n = count(&list);
            unsafe { list.InsertValueAtIndex(n, &item) }?;
        }
    }
    Ok(())
}

/// The menu target of `args`, read once (`MenuTarget::capped` bounds it before it is queued).
fn menu_target(args: &ICoreWebView2ContextMenuRequestedEventArgs) -> Option<webhost::MenuTarget> {
    let t = unsafe { args.ContextMenuTarget() }.ok()?;
    let mut kind = COREWEBVIEW2_CONTEXT_MENU_TARGET_KIND_PAGE;
    let _ = unsafe { t.Kind(&mut kind) };
    let opt = |has: unsafe fn(&ICoreWebView2ContextMenuTarget, *mut BOOL) -> windows::core::Result<()>, get: &dyn Fn() -> Option<String>| {
        let mut b = BOOL(0);
        if unsafe { has(&t, &mut b) }.is_ok() && b.as_bool() { get() } else { None }
    };
    let mut at = windows::Win32::Foundation::POINT::default();
    let _ = unsafe { args.Location(&mut at) };
    Some(webhost::MenuTarget {
        kind: kind.0,
        editable: bool_out(|b| unsafe { t.IsEditable(b) }),
        main_frame: bool_out(|b| unsafe { t.IsRequestedForMainFrame(b) }),
        page_uri: pwstr_out(|p| unsafe { t.PageUri(p) }).unwrap_or_default(),
        frame_uri: pwstr_out(|p| unsafe { t.FrameUri(p) }).unwrap_or_default(),
        link: opt(ICoreWebView2ContextMenuTarget::HasLinkUri, &|| pwstr_out(|p| unsafe { t.LinkUri(p) })),
        link_text: opt(ICoreWebView2ContextMenuTarget::HasLinkText, &|| pwstr_out(|p| unsafe { t.LinkText(p) })),
        source: opt(ICoreWebView2ContextMenuTarget::HasSourceUri, &|| pwstr_out(|p| unsafe { t.SourceUri(p) })),
        selection: opt(ICoreWebView2ContextMenuTarget::HasSelection, &|| pwstr_out(|p| unsafe { t.SelectionText(p) })),
        x: at.x,
        y: at.y,
    })
}

/// `ContextMenuRequested` (browser panes): takes a deferral, posts the target and returns at once; the
/// app answers through `answer_context_menu`. Over `MAX_PENDING_MENUS` waiting, WebView2's own menu shows.
fn register_context_menu(inner: &Rc<RefCell<Inner>>, view: &ICoreWebView2, token: &mut i64) -> windows::core::Result<()> {
    let v11 = view.cast::<ICoreWebView2_11>()?;
    let weak = Rc::downgrade(inner);
    let handler = webview2_com::ContextMenuRequestedEventHandler::create(Box::new(move |_sender, args| guard(|| {
        let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
        if inner.borrow().menus.len() >= webhost::MAX_PENDING_MENUS {
            return Ok(());
        }
        let Some(target) = menu_target(&args) else { return Ok(()) };
        let Ok(deferral) = (unsafe { args.GetDeferral() }) else { return Ok(()) };
        let mut i = inner.borrow_mut();
        let t = i.next_token;
        i.next_token += 1;
        i.menus.insert(t, PendingMenu { deferral, args });
        let id = i.id;
        i.poster.post(Event::ContextMenu { view: id, token: t, target: target.capped() });
        Ok(())
    })));
    unsafe { v11.add_ContextMenuRequested(&handler, token) }
}

fn source_of(view: &ICoreWebView2) -> Option<String> {
    pwstr_out(|p| unsafe { view.Source(p) }).map(|s| webhost::cap_url(&s).0)
}

fn register_navigation(inner: &Rc<RefCell<Inner>>, view: &ICoreWebView2, token: &mut i64) -> windows::core::Result<()> {
    // ONE NavigationStarting handler: later plans add their post to it.
    let weak = Rc::downgrade(inner);
    let starting = NavigationStartingEventHandler::create(Box::new(move |sender, args| guard(|| {
        let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
        let uri = pwstr_out(|p| unsafe { args.Uri(p) }).unwrap_or_default();
        let (user, redirect) = (bool_out(|b| unsafe { args.IsUserInitiated(b) }), bool_out(|b| unsafe { args.IsRedirected(b) }));
        let mut nav = 0u64;
        let _ = unsafe { args.NavigationId(&mut nav) };
        let (policy, by_tandem) = {
            let mut i = inner.borrow_mut();
            let (capped, _) = webhost::cap_url(&uri);
            i.nav_uris.push_back((nav, capped));
            while i.nav_uris.len() > MAX_NAV_URIS {
                i.nav_uris.pop_front();
            }
            // Tandem's own load: consumed by the first matching NavigationStarting (P19).
            let mut by_tandem = false;
            if let Some((url, at)) = i.tandem_uri.clone() {
                if at.elapsed() > Duration::from_secs(30) {
                    i.tandem_uri = None;
                } else if webhost::same_navigation_uri(&url, &uri) {
                    i.tandem_uri = None;
                    by_tandem = true;
                }
            }
            (webhost::NavPolicy { guard_local_schemes: i.setup.guard_local_schemes, allowed_schemes: i.setup.allowed_schemes.clone() }, by_tandem)
        };
        // The one verdict (webhost::nav_verdict): scheme list, then the local-scheme guard.
        let source = sender.as_ref().and_then(source_of);
        let info = webhost::NavInfo { target: &uri, source: source.as_deref(), user, redirect, by_tandem };
        let allowed = webhost::nav_verdict(&info, &policy, &|u| webhost::file_url_local(u, &local_judge));
        if !allowed {
            let _ = unsafe { args.SetCancel(true) };
            if webhost::refused_click(&info, allowed) {
                let i = inner.borrow();
                i.poster.post(Event::Refused { view: i.id, url: webhost::cap_url(&uri).0, user });
            }
            return Ok(());
        }
        let i = inner.borrow();
        // The single page-load-start event (ruling Q2): the kind needs a newer runtime (-1 without).
        let mut kind = COREWEBVIEW2_NAVIGATION_KIND(-1);
        if let Ok(a3) = args.cast::<ICoreWebView2NavigationStartingEventArgs3>() {
            let _ = unsafe { a3.NavigationKind(&mut kind) };
        }
        i.poster.post(Event::Starting { view: i.id, url: webhost::cap_url(&uri).0, kind: kind.0, user, redirect, nav });
        i.poster.post(Event::Loading(i.id, true));
        Ok(())
    })));
    unsafe { view.add_NavigationStarting(&starting, token) }?;

    let weak = Rc::downgrade(inner);
    let completed = NavigationCompletedEventHandler::create(Box::new(move |_sender, args| guard(|| {
        let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
        let ok = bool_out(|b| unsafe { args.IsSuccess(b) });
        let mut nav = 0u64;
        let _ = unsafe { args.NavigationId(&mut nav) };
        let mut status = 0i32;
        if ok {
            if let Ok(a2) = args.cast::<ICoreWebView2NavigationCompletedEventArgs2>() {
                let _ = unsafe { a2.HttpStatusCode(&mut status) };
            }
        } else {
            let mut web = COREWEBVIEW2_WEB_ERROR_STATUS(0);
            let _ = unsafe { args.WebErrorStatus(&mut web) };
            status = web.0;
        }
        let mut i = inner.borrow_mut();
        let at = i.nav_uris.iter().position(|(n, _)| *n == nav);
        let url = at.and_then(|p| i.nav_uris.remove(p)).map(|(_, u)| u).unwrap_or_default();
        let id = i.id;
        i.poster.post(Event::Navigated { view: id, ok, status, url, nav });
        i.poster.post(Event::Loading(id, false));
        Ok(())
    })));
    unsafe { view.add_NavigationCompleted(&completed, token) }?;

    let weak = Rc::downgrade(inner);
    let source = SourceChangedEventHandler::create(Box::new(move |sender, _| guard(|| {
        let Some(inner) = weak.upgrade() else { return Ok(()) };
        if let Some(s) = sender.as_ref().and_then(source_of) {
            let i = inner.borrow();
            i.poster.post(Event::Source(i.id, s));
        }
        Ok(())
    })));
    unsafe { view.add_SourceChanged(&source, token) }?;

    let weak = Rc::downgrade(inner);
    let title = DocumentTitleChangedEventHandler::create(Box::new(move |sender, _| guard(|| {
        let Some(inner) = weak.upgrade() else { return Ok(()) };
        if let Some(t) = sender.and_then(|v| pwstr_out(|p| unsafe { v.DocumentTitle(p) })) {
            let i = inner.borrow();
            i.poster.post(Event::Title(i.id, webhost::clean_title(&t)));
        }
        Ok(())
    })));
    unsafe { view.add_DocumentTitleChanged(&title, token) }?;

    let weak = Rc::downgrade(inner);
    let history = HistoryChangedEventHandler::create(Box::new(move |sender, _| guard(|| {
        let (Some(inner), Some(v)) = (weak.upgrade(), sender) else { return Ok(()) };
        let (back, forward) = (bool_out(|b| unsafe { v.CanGoBack(b) }), bool_out(|b| unsafe { v.CanGoForward(b) }));
        let i = inner.borrow();
        i.poster.post(Event::History { view: i.id, back, forward });
        Ok(())
    })));
    unsafe { view.add_HistoryChanged(&history, token) }?;
    Ok(())
}

/// The one reader of a posted web message (a view's or a frame's): size-capped before it is
/// queued, its source reported (the posting document's URL, capped).
fn message_post(inner: &Rc<RefCell<Inner>>, frame: Option<u64>, args: &ICoreWebView2WebMessageReceivedEventArgs) {
    let Some(json) = pwstr_out(|p| unsafe { args.WebMessageAsJson(p) }) else { return };
    if json.len() > webhost::MAX_MESSAGE {
        inner.borrow().poster.gate.note_dropped();
        return;
    }
    let source = pwstr_out(|p| unsafe { args.Source(p) }).map(|s| webhost::cap_url(&s).0).unwrap_or_default();
    let i = inner.borrow();
    match frame {
        None => i.poster.post(Event::Message { view: i.id, json, source }),
        Some(frame) => i.poster.post(Event::FrameMessage { view: i.id, frame, json, source }),
    }
}

/// Starts tracking `frame` (and, through `ICoreWebView2Frame7`, its children): its id, its messages and
/// its end. Bounded by `MAX_FRAMES`; the pointer is kept only until `Destroyed`.
fn track_frame(inner: &Rc<RefCell<Inner>>, frame: ICoreWebView2Frame) {
    let mut id = 0u32;
    let Ok(f5) = frame.cast::<ICoreWebView2Frame5>() else { return };
    if unsafe { f5.FrameId(&mut id) }.is_err() {
        return;
    }
    let id = u64::from(id);
    if inner.borrow().frames.len() >= MAX_FRAMES && !inner.borrow().frames.contains_key(&id) {
        return;
    }
    let name = pwstr_out(|p| unsafe { frame.Name(p) }).unwrap_or_default();
    let name: String = crate::inert_files::display_text(&name, 256, false);
    let mut token = 0i64;
    if let Ok(f2) = frame.cast::<ICoreWebView2Frame2>() {
        let weak = Rc::downgrade(inner);
        let message = FrameWebMessageReceivedEventHandler::create(Box::new(move |_sender, args| guard(|| {
            let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
            message_post(&inner, Some(id), &args);
            Ok(())
        })));
        let _ = unsafe { f2.add_WebMessageReceived(&message, &mut token) };
        inner.borrow_mut().frames.insert(id, f2);
    }
    let weak = Rc::downgrade(inner);
    let gone = FrameDestroyedEventHandler::create(Box::new(move |_sender, _| guard(|| {
        let Some(inner) = weak.upgrade() else { return Ok(()) };
        let existed = inner.borrow_mut().frames.remove(&id).is_some();
        if existed {
            let i = inner.borrow();
            i.poster.post(Event::FrameGone { view: i.id, frame: id });
        }
        Ok(())
    })));
    let _ = unsafe { frame.add_Destroyed(&gone, &mut token) };
    // Nested frames: a frame's own children are announced on the frame (`ICoreWebView2Frame7`).
    if let Ok(f7) = frame.cast::<ICoreWebView2Frame7>() {
        let weak = Rc::downgrade(inner);
        let child = FrameChildFrameCreatedEventHandler::create(Box::new(move |_sender, args| guard(|| {
            let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
            if let Ok(f) = unsafe { args.Frame() } {
                track_frame(&inner, f);
            }
            Ok(())
        })));
        let _ = unsafe { f7.add_FrameCreated(&child, &mut token) };
    }
    let i = inner.borrow();
    i.poster.post(Event::FrameCreated { view: i.id, frame: id, name });
}

fn register_page_events(inner: &Rc<RefCell<Inner>>, view: &ICoreWebView2, token: &mut i64) -> windows::core::Result<()> {
    // The view's WebMessageReceived handler (the frames' is `watch_frames`'; both read a message only
    // through `message_post`). A page's message is untrusted: size-capped there, its source
    // reported, its shape judged by the consumer (`webhost::invoke_request`, `logger_message`).
    let weak = Rc::downgrade(inner);
    let message = WebMessageReceivedEventHandler::create(Box::new(move |_sender, args| guard(|| {
        let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
        message_post(&inner, None, &args);
        Ok(())
    })));
    unsafe { view.add_WebMessageReceived(&message, token) }?;

    // Every new-window request is handled: no popup window ever appears (iTerm2's tool views
    // refuse automatic windows).
    let weak = Rc::downgrade(inner);
    let window = NewWindowRequestedEventHandler::create(Box::new(move |_sender, args| guard(|| {
        let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
        let _ = unsafe { args.SetHandled(true) };
        let user = bool_out(|b| unsafe { args.IsUserInitiated(b) });
        let uri = pwstr_out(|p| unsafe { args.Uri(p) }).unwrap_or_default();
        let mut i = inner.borrow_mut();
        if !i.setup.new_windows || !webhost::new_window_scheme_ok(&uri) {
            return Ok(());
        }
        let pending = i.poster.gate.new_windows_pending();
        if i.new_windows.allow(Instant::now(), user, pending) {
            let id = i.id;
            i.poster.post(Event::NewWindow { view: id, url: webhost::cap_url(&uri).0 });
        }
        Ok(())
    })));
    unsafe { view.add_NewWindowRequested(&window, token) }?;

    // The favicon (final review I6): WebView2 fetches it with the page (Tandem fetches nothing);
    // GetFavicon hands it over as PNG, read with the 256 KiB bound and posted only when it decodes
    // within 256 by 256 (B5: the cap is the host's, not the consumer's). A newer runtime only.
    let setup_favicon = inner.borrow().setup.favicon;
    if setup_favicon {
        if let Ok(v15) = view.cast::<ICoreWebView2_15>() {
            let weak = Rc::downgrade(inner);
            let changed = FaviconChangedEventHandler::create(Box::new(move |sender, _| guard(|| {
                let (Some(inner), Some(v)) = (weak.upgrade(), sender) else { return Ok(()) };
                let Ok(v15) = v.cast::<ICoreWebView2_15>() else { return Ok(()) };
                let weak = Rc::downgrade(&inner);
                let done = GetFaviconCompletedHandler::create(Box::new(move |hr, stream| guard(|| {
                    let (Some(inner), Some(stream)) = (weak.upgrade(), stream) else { return Ok(()) };
                    if hr.is_err() {
                        return Ok(());
                    }
                    let Some(bytes) = read_stream(&stream, webhost::MAX_FAVICON) else { return Ok(()) };
                    if !webhost::favicon_ok(&bytes) {
                        return Ok(());
                    }
                    let i = inner.borrow();
                    i.poster.post(Event::Favicon(i.id, bytes));
                    Ok(())
                })));
                let _ = unsafe { v15.GetFavicon(COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG, &done) };
                Ok(())
            })));
            unsafe { v15.add_FaviconChanged(&changed, token) }?;
        }
    }
    Ok(())
}

/// A COM stream's bytes from its start; None past `max` bytes or on a read error.
fn read_stream(s: &windows::Win32::System::Com::IStream, max: usize) -> Option<Vec<u8>> {
    use windows::Win32::System::Com::STREAM_SEEK_SET;
    unsafe { s.Seek(0, STREAM_SEEK_SET, None) }.ok()?;
    let mut out: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let mut read = 0u32;
        // SAFETY: the buffer outlives the call and `read` says how much of it was filled.
        let hr = unsafe { s.Read(buf.as_mut_ptr() as *mut c_void, buf.len() as u32, Some(&mut read as *mut u32)) };
        if hr.is_err() {
            return None;
        }
        if read == 0 {
            return Some(out);
        }
        out.extend_from_slice(&buf[..read as usize]);
        if out.len() > max {
            return None;
        }
    }
}

fn register_policy(inner: &Rc<RefCell<Inner>>, view: &ICoreWebView2, token: &mut i64) -> windows::core::Result<()> {
    // Permissions are denied (preflight P5): 13x6 asks the person as iTerm2 does.
    let denied = PermissionRequestedEventHandler::create(Box::new(|_sender, args| guard(|| {
        if let Some(args) = args {
            let _ = unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY) };
        }
        Ok(())
    })));
    unsafe { view.add_PermissionRequested(&denied, token) }?;

    // A failed process is reported with its kind; the app acts on the kind (`webhost::failure_action`).
    let weak = Rc::downgrade(inner);
    let failed = ProcessFailedEventHandler::create(Box::new(move |_sender, args| guard(|| {
        let Some(inner) = weak.upgrade() else { return Ok(()) };
        let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND(-1);
        if let Some(a) = args {
            let _ = unsafe { a.ProcessFailedKind(&mut kind) };
        }
        let i = inner.borrow();
        i.poster.post(Event::ProcessFailed { view: i.id, kind: kind.0 });
        Ok(())
    })));
    unsafe { view.add_ProcessFailed(&failed, token) }?;

    // An external protocol (ms-msdt:, search-ms:, mailto:, any registered handler) is always
    // cancelled here; the app opens it only through `semantic::guard_open` (preflight B4).
    if let Ok(v18) = view.cast::<ICoreWebView2_18>() {
        let weak = Rc::downgrade(inner);
        let external = LaunchingExternalUriSchemeEventHandler::create(Box::new(move |_sender, args| guard(|| {
            let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
            let _ = unsafe { args.SetCancel(true) };
            let url = pwstr_out(|p| unsafe { args.Uri(p) }).unwrap_or_default();
            let user = bool_out(|b| unsafe { args.IsUserInitiated(b) });
            let i = inner.borrow();
            i.poster.post(Event::ExternalUri { view: i.id, url: webhost::cap_url(&url).0, user });
            Ok(())
        })));
        unsafe { v18.add_LaunchingExternalUriScheme(&external, token) }?;
    } else {
        crate::dlog!("webhost: this WebView2 Runtime cannot cancel external protocol launches");
    }
    Ok(())
}

fn register_controller_events(inner: &Rc<RefCell<Inner>>, ctrl: &ICoreWebView2Controller, token: &mut i64) -> windows::core::Result<()> {
    for gained in [true, false] {
        let weak = Rc::downgrade(inner);
        let handler = FocusChangedEventHandler::create(Box::new(move |_sender, _| guard(|| {
            let Some(inner) = weak.upgrade() else { return Ok(()) };
            let i = inner.borrow();
            i.poster.post(Event::Focus(i.id, gained));
            Ok(())
        })));
        if gained {
            unsafe { ctrl.add_GotFocus(&handler, token) }?;
        } else {
            unsafe { ctrl.add_LostFocus(&handler, token) }?;
        }
    }
    // Every key with its modifiers; the app decides what a Tandem shortcut is (marking a key
    // handled needs a synchronous answer, which later plans add).
    let weak = Rc::downgrade(inner);
    let keys = AcceleratorKeyPressedEventHandler::create(Box::new(move |_sender, args| guard(|| {
        let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
        let mut kind = COREWEBVIEW2_KEY_EVENT_KIND(-1);
        let _ = unsafe { args.KeyEventKind(&mut kind) };
        if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN {
            return Ok(());
        }
        let mut vk = 0u32;
        let _ = unsafe { args.VirtualKey(&mut vk) };
        let (poster, id, gate) = {
            let i = inner.borrow();
            (i.poster.clone(), i.id, i.key_gate.clone())
        };
        let mods = modifier_bits();
        // The gate is read here, synchronously, and holds no App borrow: a key Tandem takes never
        // reaches the page.
        let taken = gate.takes(vk, mods);
        if taken {
            let _ = unsafe { args.SetHandled(true) };
        }
        poster.post(Event::Key { view: id, vk, mods, taken });
        Ok(())
    })));
    unsafe { ctrl.add_AcceleratorKeyPressed(&keys, token) }
}

/// The modifier keys held now: 1 Shift, 2 Control, 4 Alt, 8 Windows.
fn modifier_bits() -> u8 {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};
    let down = |vk: u16| unsafe { GetKeyState(i32::from(vk)) } < 0;
    u8::from(down(VK_SHIFT)) | (u8::from(down(VK_CONTROL)) << 1) | (u8::from(down(VK_MENU)) << 2) | (u8::from(down(VK_LWIN) || down(VK_RWIN)) << 3)
}

fn register_requests(inner: &Rc<RefCell<Inner>>, view: &ICoreWebView2, filters: &[String], token: &mut i64) -> windows::core::Result<()> {
    // `WebResourceRequested` sees every matching request: take a deferral, post, return at once; the app
    // completes it through `answer_request`. Over 256 pending, the request simply continues.
    let weak = Rc::downgrade(inner);
    let handler = WebResourceRequestedEventHandler::create(Box::new(move |sender, args| guard(|| {
        let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
        if inner.borrow().pending.len() >= MAX_PENDING_REQUESTS {
            return Ok(());
        }
        let Ok(deferral) = (unsafe { args.GetDeferral() }) else { return Ok(()) };
        let url = unsafe { args.Request() }.ok().and_then(|r| pwstr_out(|p| unsafe { r.Uri(p) })).unwrap_or_default();
        let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT(0);
        let _ = unsafe { args.ResourceContext(&mut context) };
        let top = sender.as_ref().and_then(source_of).unwrap_or_default();
        let mut i = inner.borrow_mut();
        let t = i.next_token;
        i.next_token += 1;
        i.pending.insert(t, Pending { deferral, args });
        let id = i.id;
        i.poster.post(Event::Request { view: id, token: t, url: webhost::cap_url(&url).0, context: context.0, top_url: top });
        Ok(())
    })));
    unsafe { view.add_WebResourceRequested(&handler, token) }?;
    for f in filters {
        let w = wide(f);
        unsafe { view.AddWebResourceRequestedFilter(PCWSTR(w.as_ptr()), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL) }?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, IsWindow, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW};

    /// 13w seam S4: WebView2 and 13w's taskbar overlay share the STA winit made on the UI thread.
    /// No web view file initializes or uninitializes COM in live code; the taskbar initializes an
    /// apartment only on its own worker thread and never uninitializes one.
    #[test]
    fn webhost_win_never_initializes_com() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let live = |f: &str| std::fs::read_to_string(root.join(f)).unwrap().split("#[cfg(test)]\nmod tests").next().unwrap().to_string();
        for f in ["webhost_win.rs", "webhost.rs", "web_popover.rs", "svg_raster_win.rs", "app/webhost_app.rs", "app/browser_app.rs", "app/web_popover_app.rs", "app/tool_web_ui.rs", "taskbar_win.rs"] {
            let code = live(f);
            for bad in ["CoUninitialize", "OleUninitialize", "OleInitialize("] {
                assert!(!code.contains(bad), "{f} calls {bad}");
            }
            if f != "taskbar_win.rs" {
                assert!(!code.contains("CoInitializeEx("), "{f} initializes COM; the UI thread's STA is winit's");
            }
        }
        let t = live("taskbar_win.rs");
        assert_eq!(t.matches("CoInitializeEx(").count(), 1);
        let worker = &t[t.find("fn worker(").unwrap()..];
        assert!(worker[..worker.find("\n}\n").unwrap()].contains("CoInitializeEx("), "only the worker thread makes an apartment");
    }

    /// 13w merge (13v2/13w/browser): every window hook is a comctl32 subclass with its own id, so
    /// the hooks on one window form one chain (each passes messages on and takes itself off at
    /// WM_NCDESTROY); the web view code adds no hook and never replaces a window procedure.
    #[test]
    fn every_window_hook_is_one_subclass_chain() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let id = regex::Regex::new(r"const \w*SUBCLASS_ID: usize = (0x[0-9A-Fa-f_]+);").unwrap();
        let (mut ids, mut dirs) = (Vec::new(), vec![root.clone()]);
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                    continue;
                }
                if p.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap();
                let code = text.split("#[cfg(test)]\nmod tests").next().unwrap();
                assert!(!code.contains("GWLP_WNDPROC"), "{p:?} replaces a window procedure; use SetWindowSubclass");
                if code.contains("SetWindowSubclass(") {
                    assert!(code.contains("DefSubclassProc(") && code.contains("RemoveWindowSubclass("), "{p:?}: a subclass passes messages on and takes itself off");
                    let found: Vec<String> = id.captures_iter(code).map(|c| c[1].replace('_', "").to_ascii_lowercase()).collect();
                    assert!(!found.is_empty(), "{p:?}: a subclass has its own id constant");
                    ids.extend(found);
                }
            }
        }
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "two hooks share a subclass id: one would replace the other");
        assert!(n >= 11, "{n}");
        for f in ["webhost_win.rs", "app/browser_app.rs", "app/web_popover_app.rs", "app/webhost_app.rs"] {
            let text = std::fs::read_to_string(root.join(f)).unwrap();
            assert!(!text.split("#[cfg(test)]\nmod tests").next().unwrap().contains("SetWindowSubclass("), "{f}: the web view host adds no window hook");
        }
    }

    /// The 13w merge: the one process teardown covers web views. Every view registers at create;
    /// `teardown_process` closes them before it removes the Dev Null folder; twice is safe.
    #[test]
    fn the_process_teardown_closes_every_web_view() {
        assert_eq!(close_all(), 0, "nothing live on a fresh thread");
        assert_eq!(close_all(), 0, "twice is safe");
        let this = include_str!("webhost_win.rs");
        let create = &this[this.find("pub fn create(env: &Env").unwrap()..];
        assert!(create[..create.find("\n    }\n").unwrap()].contains("l.push(weak.clone())"), "every view registers");
        let m = include_str!("app/mod.rs");
        let t = &m[m.find("pub(crate) fn teardown_process()").unwrap()..];
        let t = &t[..t.find("\n}\n").unwrap()];
        let close = t.find("crate::webhost_win::close_all()").expect("teardown closes the views");
        assert!(close < t.find("crate::webhost_win::cleanup_files()").unwrap(), "views close before their folder goes");
        assert!(close > t.find("crate::dlog::stop_at_exit()").unwrap());
    }

    /// 13x3 Task 1 review I2: the find probes can fail. The forged-message probe runs with new
    /// windows on, turns navigation shortcuts on and keys at them, and passes a fixed wrong guess; the
    /// forged-click probe posts from a cross-origin frame and checks the Tandem world for its marker.
    #[test]
    fn the_find_probes_can_fail() {
        let this = include_str!("webhost_win.rs");
        let body = |name: &str| {
            let at = this.find(&format!("fn {name}()")).unwrap_or_else(|| panic!("the probe {name}"));
            let rest = &this[at..];
            rest[..rest.find("\n    }\n").unwrap()].to_string()
        };
        let f = body("find_messages_forged_by_a_page");
        assert!(f.contains("Probe::start_with(") && f.contains("true)"), "new windows on");
        assert!(f.contains("\"startNavigationShortcuts\"") && f.contains("\"selectionAction\": \"open\""), "shortcuts started");
        assert!(f.contains("Input.dispatchKeyEvent"), "a real key is the control that opens a window");
        assert!(f.contains("hostile_page(&body, \"g\")") && !f.contains("hostile_page(&body, &secret)"), "a fixed wrong guess");
        let c = body("find_forged_click_cannot_reach_the_client");
        assert!(c.contains("FORGING_CHILD") && c.contains("__pwned"), "from the child frame, with a marker");
        assert!(this.contains("const FORGING_CHILD: &str = \"https://b.tandem-probe.invalid/") && this.contains("const PAGE: &str = \"https://probe.tandem-probe.invalid/\""), "the child is of another origin");
        assert!(f.contains("FORGING_CHILD") && f.contains("frames[0].postMessage({forge: 'click'"), "the B7 forgeries come from the child too");
        assert!(c.contains("p.eval(Some(k)") && c.contains("probePwned"), "the marker and the Client message are checked");
        let s = include_str!("../scripts/probe-desktop.ps1");
        assert!(s.contains("webhost_win::probe::"), "the default filter names the probes");
    }

    #[test]
    fn view_setup_defaults_are_the_closed_ones() {
        let s = ViewSetup::default();
        assert!(!s.in_private && !s.guard_local_schemes && !s.web_message && !s.new_windows && !s.devtools);
        assert!(s.scripts.is_empty() && s.resource_filters.is_empty() && s.user_agent.is_none() && s.allowed_schemes.is_none());
    }

    /// Preflight P5: no host object reaches a page, anywhere in Tandem.
    #[test]
    fn no_host_objects_for_pages() {
        let this = include_str!("webhost_win.rs");
        let code: String = this.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        let code = code.split("#[cfg(test)]\nmod tests").next().unwrap_or("");
        assert!(!code.contains("AddHostObjectToScript"), "a host object");
        assert!(code.contains("SetAreHostObjectsAllowed(false)"));
        // Downloads stay WebView2's: no handler and no setting that changes the Mark of the Web.
        for banned in ["DownloadStarting", "SetCancel(true)).and", "IsDefaultDownloadDialog", "DownloadOperation"] {
            assert!(!code.contains(banned), "{banned}");
        }
        // The one NavigationStarting handler, and two message constructors (the view's and the frames',
        // 13x2 B8a) that both read a message only through `message_post`.
        assert_eq!(code.matches("NavigationStartingEventHandler::create").count(), 1);
        assert_eq!(code.matches("WebMessageReceivedEventHandler::create").count(), 2);
        assert_eq!(code.matches("args.WebMessageAsJson(").count(), 1, "one reader: message_post");
        assert_eq!(code.matches("message_post(&inner").count(), 2);
    }

    /// Final review Minor 3: no `NavigateToString` and no "the next data: navigation is Tandem's"
    /// flag remain (program content is served by `LocalDoc`; the flag had no expiry).
    #[test]
    fn no_string_navigation_remains() {
        let src = include_str!("webhost_win.rs");
        let live = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        assert!(!live.contains("NavigateToString") && !live.contains("tandem_data") && !live.contains("fn navigate_to_string"));
    }

    /// Final review I6: the favicon is WebView2's own (FaviconChanged, then GetFavicon as PNG; Tandem
    /// fetches nothing), read with the 256 KiB bound and posted only when `favicon_ok` passes (B5
    /// caps in the host). Only views that ask for it (browser panes) register the handler.
    #[test]
    fn the_favicon_comes_from_the_view_and_is_bounded_before_posting() {
        let src = include_str!("webhost_win.rs");
        let live = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        let events = &live[live.find("fn register_page_events").unwrap()..live.find("fn register_policy").unwrap()];
        let at = events.find("add_FaviconChanged(").expect("the handler is registered");
        assert!(events[..at].contains("if setup_favicon {"), "only for views that show a favicon");
        let get = events.find("GetFavicon(COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG").expect("the view's own favicon, as PNG");
        let read = events.find("read_stream(&stream, webhost::MAX_FAVICON)").expect("read with the bound");
        let ok = events.find("webhost::favicon_ok(&bytes)").expect("judged in the host");
        let post = events.find("Event::Favicon(").unwrap();
        // The completion handler (read, judge, post) is made before the call that runs it.
        assert!(read < ok && ok < post && post < get);
        assert!(!live.contains("ai_transport") && !live.contains("WinHttp"), "no Tandem network fetch");
        let browser = include_str!("app/browser_app.rs");
        assert!(browser.contains("        favicon: true,"), "browser panes ask for it");
        assert!(!include_str!("app/tool_web_ui.rs").contains("favicon: true"));
        // Shown in the toolbar and, for a profile whose icon is Automatic, in the tab (`PS:6665`).
        let tabs = include_str!("app/tab_groups_ui.rs");
        let icon = &tabs[tabs.find("pub(super) fn tab_icon_for").unwrap()..];
        assert!(icon[..icon.find("let key = IconKey").unwrap()].contains("return self.browser_tab_icon(pane);"));
        assert!(browser.contains("pub(super) fn browser_tab_icon(&self, pane: PaneId) -> Option<scene::TabImage>"));
    }

    /// Final review round 1, item 1 (class fix): every COM callback body runs inside `guard`, so a
    /// panic becomes E_FAIL and never unwinds into COM.
    #[test]
    fn every_com_callback_goes_through_the_unwind_guard() {
        let this = include_str!("webhost_win.rs");
        let code = this.split("#[cfg(test)]\nmod tests").next().unwrap_or("");
        let mut seen = 0;
        let mut rest = code;
        while let Some(i) = rest.find("Handler::create(Box::new(") {
            let tail = &rest[i + "Handler::create(Box::new(".len()..];
            let params_end = tail.match_indices('|').nth(1).map(|(k, _)| k).expect("closure params");
            let body = tail[params_end + 1..].trim_start();
            assert!(body.starts_with("guard(||"), "a COM callback runs outside the unwind guard: {}", &tail[..params_end + 1]);
            seen += 1;
            rest = tail;
        }
        assert!(seen >= 15, "the callbacks are seen ({seen})");
    }

    #[test]
    fn a_panicking_callback_becomes_e_fail() {
        let r = guard(|| -> windows::core::Result<()> { panic!("boom") });
        assert_eq!(r.unwrap_err().code(), windows::Win32::Foundation::E_FAIL);
        assert!(guard(|| Ok(())).is_ok());
    }

    /// Item 3: the NavigationStarting handler asks `webhost::nav_verdict` and nothing else decides.
    #[test]
    fn the_navigation_handler_asks_the_one_verdict() {
        let this = include_str!("webhost_win.rs");
        let code = this.split("#[cfg(test)]\nmod tests").next().unwrap_or("");
        assert_eq!(code.matches("webhost::nav_verdict(").count(), 1);
        assert!(!code.contains("navigation_allowed("), "the handler judges through nav_verdict only");
    }

    /// Final review round 1, item 2: the protected DACL is set when the folder is made, not on every
    /// environment (setting it walks the whole profile). An existing folder that fails the check is
    /// repaired on a worker thread, and this call fails so the UI thread never waits for it.
    #[test]
    fn the_owner_only_acl_is_set_once() {
        let base = std::env::temp_dir().join(format!("tandem-wv2-acl-{}", std::process::id()));
        let folder = base.join("a").join("Default");
        let calls = || PROTECTED.lock().unwrap().iter().filter(|p| p.starts_with(&base)).count();
        prepare_folder(&folder).unwrap();
        assert_eq!(calls(), 1, "a new folder is protected once");
        prepare_folder(&folder).unwrap();
        prepare_folder(&folder).unwrap();
        assert_eq!(calls(), 1, "a folder that passes the check is never protected again");
        // Someone widens the access list: the next call fails and a worker repairs it.
        let out = std::process::Command::new("icacls").arg(&folder).args(["/grant", "*S-1-5-32-545:(OI)(CI)(M)"]).output().unwrap();
        assert!(out.status.success(), "icacls: {}", String::from_utf8_lossy(&out.stdout));
        let first = prepare_folder(&folder);
        assert!(first.is_err(), "a widened folder is not used until repaired");
        let end = Instant::now() + Duration::from_secs(20);
        while calls() < 2 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(calls(), 2, "the repair ran once, on a worker");
        let mut ok = false;
        while !ok && Instant::now() < end {
            ok = prepare_folder(&folder).is_ok();
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(ok, "the repaired folder is accepted");
        assert_eq!(calls(), 2);
        let _ = crate::localfs::remove_dir_all(&crate::localfs::LocalPath::judge(&base).unwrap());
    }

    /// Later plans add fields to `ViewSetup`: it is only ever built with `..Default::default()`.
    #[test]
    fn view_setup_is_built_with_defaults() {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let mut seen = 0;
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            for (at, _) in text.match_indices("ViewSetup {") {
                let before = &text[..at];
                if before.ends_with("struct ") || before.ends_with("impl ") || before.ends_with("for ") || before.ends_with("-> ") {
                    continue;
                }
                let line = text[..at].rsplit('\n').next().unwrap_or("");
                if line.trim_start().starts_with("//") || line.contains("\"") {
                    continue;
                }
                let body = &text[at..(at + 600).min(text.len())];
                let close = body.find('}').unwrap_or(body.len());
                assert!(body[..close].contains("..Default::default()"), "{}: a ViewSetup literal without ..Default::default()", f.display());
                seen += 1;
            }
        }
        assert!(seen >= 1, "the test's own literals are seen");
    }

    #[test]
    fn scrub_removes_the_override_variables() {
        // SAFETY: a test-only name set and removed in this test; no other test reads it.
        unsafe { std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", "C:\\planted") };
        scrub_env();
        assert!(std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_none());
    }

    /// Preflight P3: the folder is made owner-only and protected.
    #[test]
    fn folders_are_made_owner_only() {
        let base = std::env::temp_dir().join(format!("tandem-wv2-{}", std::process::id()));
        let folder = base.join("a").join("Default");
        prepare_folder(&folder).unwrap();
        let lp = crate::localfs::LocalPath::judge(&folder).unwrap();
        let opened = crate::dynamic_profiles_trust::open(&lp, 0).unwrap();
        assert!(opened.dir);
        opened.trust.expect("the folder passes the owner-only rule");
        let _ = crate::localfs::remove_dir_all(&crate::localfs::LocalPath::judge(&base).unwrap());
    }

    #[test]
    fn a_missing_runtime_is_an_error_code() {
        // The runtime may or may not be installed; either way the call answers and never panics.
        match runtime_version(None) {
            Ok(v) => assert!(!v.is_empty()),
            Err(code) => assert!(code != 0),
        }
        assert_eq!(webhost::runtime_outcome(Err(-2147024894)), webhost::Runtime::Missing);
    }

    #[test]
    fn a_planted_hint_is_refused() {
        for hint in ["", "C:\\x\\..\\y", "\\\\srv\\wv2", "relative", "C:\\definitely\\not\\here"] {
            assert!(fixed_runtime_check(hint).is_err(), "{hint}");
        }
    }

    /// The harness of an ignored test: a thread with no winit loop gets its own apartment, a raw
    /// never-shown window and a bounded message pump.
    fn pump_until(rx: &mpsc::Receiver<Posted>, secs: u64, mut want: impl FnMut(&Event) -> bool) -> Option<Event> {
        let mut skipped: Vec<String> = Vec::new();
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            let mut msg = MSG::default();
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            while let Ok(p) = rx.try_recv() {
                if let Some(e) = p.event {
                    if want(&e) {
                        return Some(e);
                    }
                    skipped.push(format!("{e:?}"));
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !skipped.is_empty() {
            println!("events seen while waiting: {skipped:#?}");
        }
        None
    }

    /// Needs a real WebView2 Runtime and the loader: `scripts/xt test --lib webhost -- --ignored`.
    /// Creates only a never-shown window; with no runtime it prints a note and passes.
    #[test]
    #[ignore]
    fn a_view_loads_a_page() {
        if runtime_version(None).is_err() {
            println!("WebView2 Runtime not installed; skipped");
            return;
        }
        // A view's browser process must reach the parent window's desktop: a session 0 process (a
        // service, an SSH command) cannot host one, so the test needs an interactive session.
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
        unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            unsafe { DefWindowProcW(h, m, w, l) }
        }
        let class = windows::core::w!("TandemWebView2Test");
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), lpszClassName: class, ..Default::default() };
        unsafe { RegisterClassW(&wc) };
        let hwnd = unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), class, windows::core::w!("tandem-wv2-test"), WS_OVERLAPPEDWINDOW, 0, 0, 200, 200, None, None, None, None) }.expect("a window");
        assert!(unsafe { IsWindow(Some(hwnd)) }.as_bool());
        let folder = std::env::temp_dir().join(format!("tandem-wv2-ui-{}", std::process::id()));
        let (tx, rx) = mpsc::channel::<Posted>();
        let env_cell: Rc<RefCell<Option<Result<Env, String>>>> = Rc::new(RefCell::new(None));
        let cell = env_cell.clone();
        Env::create(Store::Default, &folder, None, &EnvOptions::default(), Box::new(move |r| *cell.borrow_mut() = Some(r)));
        let end = Instant::now() + Duration::from_secs(20);
        while env_cell.borrow().is_none() && Instant::now() < end {
            pump_until(&rx, 1, |_| false);
        }
        let env = env_cell.borrow_mut().take().expect("an environment within 20 s").expect("the environment");
        let setup = ViewSetup { guard_local_schemes: true, web_message: true, resource_filters: vec!["https://tandem.test/*".into()], ..Default::default() };
        let view = WebView::create(&env, hwnd.0 as isize, 1, (0, 0, 200, 200), setup, tx.clone());
        assert!(pump_until(&rx, 20, |e| matches!(e, Event::Created(1))).is_some(), "Created");
        // Tandem's own page, served from memory (no NavigateToString: final review Minor 3).
        view.navigate_tandem("https://tandem.test/t");
        let Some(Event::Request { token, .. }) = pump_until(&rx, 10, |e| matches!(e, Event::Request { view: 1, .. })) else { panic!("a request for the page") };
        view.answer_request(token, Answer::Respond { status: 200, mime: "text/html".into(), body: b"<title>t</title><body>x</body>".to_vec(), headers: vec![] });
        let title = pump_until(&rx, 10, |e| matches!(e, Event::Title(1, t) if t == "t"));
        assert!(title.is_some(), "Title");
        view.execute_script("1+1", 7);
        assert_eq!(pump_until(&rx, 10, |e| matches!(e, Event::ScriptDone { token: 7, .. })), Some(Event::ScriptDone { view: 1, token: 7, json: Ok("2".into()) }));
        // A page that tries to leave for a file: URL goes nowhere (Chromium may refuse it before the guard; the guard itself is proved by `webhost::nav_verdict_guard_cancels_local_navigations_from_a_page`).
        view.execute_script("location.href = 'file:///C:/Windows/'; 0", 8);
        assert!(pump_until(&rx, 5, |e| matches!(e, Event::Navigated { url, .. } if url.starts_with("file:"))).is_none(), "no navigation to file:");
        // A navigation answered from memory finishes with its URL and a non-zero id.
        let view2 = WebView::create(&env, hwnd.0 as isize, 2, (0, 0, 200, 200), ViewSetup { resource_filters: vec!["https://tandem.test/*".into()], guard_local_schemes: true, ..Default::default() }, tx.clone());
        assert!(pump_until(&rx, 20, |e| matches!(e, Event::Created(2))).is_some());
        view2.navigate_tandem("https://tandem.test/page");
        let req = pump_until(&rx, 10, |e| matches!(e, Event::Request { view: 2, url, .. } if url.starts_with("https://tandem.test/page")));
        let Some(Event::Request { token, .. }) = req else { panic!("a request for the page") };
        view2.answer_request(token, Answer::Respond { status: 200, mime: "text/html".into(), body: b"<title>mem</title>".to_vec(), headers: vec![] });
        let done = pump_until(&rx, 10, |e| matches!(e, Event::Navigated { view: 2, .. }));
        match done {
            Some(Event::Navigated { ok, url, nav, .. }) => assert!(ok && nav != 0 && url.starts_with("https://tandem.test/page"), "{ok} {url} {nav}"),
            other => panic!("Navigated: {other:?}"),
        }
        drop(view);
        drop(view2);
        drop(env);
        let _ = unsafe { DestroyWindow(hwnd) };
        // Best effort: the browser process may still hold the folder for a moment.
        if let Some(lp) = crate::localfs::LocalPath::judge(&folder) {
            let _ = crate::localfs::remove_dir_all(&lp);
        }
    }

    /// 13x2 Task 3: a right-click's menu gets Tandem's items in order, and `saveAs` and `print` are
    /// gone. A right click is synthesized with DevTools input; when WebView2 raises no
    /// `ContextMenuRequested` for synthesized input, the test prints that and passes. Run in a console
    /// session (see the probe module); never shown.
    #[test]
    #[ignore]
    fn menu_gets_our_items() {
        if runtime_version(None).is_err() {
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
        unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            unsafe { DefWindowProcW(h, m, w, l) }
        }
        let class = windows::core::w!("TandemMenuTest");
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), lpszClassName: class, ..Default::default() };
        unsafe { RegisterClassW(&wc) };
        let hwnd = unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), class, windows::core::w!("tandem-menu-test"), WS_OVERLAPPEDWINDOW, 0, 0, 300, 300, None, None, None, None) }.expect("a window");
        let folder = std::env::temp_dir().join(format!("tandem-wv2-menu-{}", std::process::id()));
        let (tx, rx) = mpsc::channel::<Posted>();
        let env_cell: Rc<RefCell<Option<Result<Env, String>>>> = Rc::new(RefCell::new(None));
        let cell = env_cell.clone();
        Env::create(Store::Default, &folder, None, &EnvOptions::default(), Box::new(move |r| *cell.borrow_mut() = Some(r)));
        let end = Instant::now() + Duration::from_secs(20);
        while env_cell.borrow().is_none() && Instant::now() < end {
            pump_until(&rx, 1, |_| false);
        }
        let env = env_cell.borrow_mut().take().expect("an environment within 20 s").expect("the environment");
        let setup = ViewSetup { guard_local_schemes: true, context_menu: true, resource_filters: vec!["https://tandem.invalid/*".into()], ..Default::default() };
        let view = WebView::create(&env, hwnd.0 as isize, 1, (0, 0, 300, 300), setup, tx.clone());
        assert!(pump_until(&rx, 20, |e| matches!(e, Event::Created(1))).is_some(), "Created");
        view.navigate_tandem("https://tandem.invalid/m");
        let Some(Event::Request { token, .. }) = pump_until(&rx, 10, |e| matches!(e, Event::Request { view: 1, .. })) else { panic!("a request for the page") };
        view.answer_request(token, Answer::Respond { status: 200, mime: "text/html".into(), body: b"<body style='margin:0'>hello</body>".to_vec(), headers: vec![] });
        assert!(pump_until(&rx, 10, |e| matches!(e, Event::Navigated { view: 1, .. })).is_some());
        for kind in ["mousePressed", "mouseReleased"] {
            view.devtools_call("Input.dispatchMouseEvent", &format!(r#"{{"type":"{kind}","x":20,"y":20,"button":"right","clickCount":1}}"#), 50);
        }
        match pump_until(&rx, 5, |e| matches!(e, Event::ContextMenu { .. })) {
            None => println!("WebView2 raised no ContextMenuRequested for synthesized input; nothing to check"),
            Some(Event::ContextMenu { token, .. }) => {
                use webhost::MenuEntry::*;
                view.answer_context_menu(token, vec![Custom { id: 1, label: "First".into(), enabled: true, icon_png: None }, Separator, Native, Separator, Custom { id: 4, label: "Last".into(), enabled: true, icon_png: None }]);
                let labels = LAST_MENU.with(|m| m.borrow().clone());
                println!("menu: {labels:?}");
                // WebView2 names a custom item by its label (the 2026-10-07 run), so it reads "name:label".
                assert_eq!(labels.first().map(String::as_str), Some("First:First"));
                assert_eq!(labels.last().map(String::as_str), Some("Last:Last"));
                assert!(!labels.iter().any(|l| l.starts_with("saveAs:") || l.starts_with("print:")), "{labels:?}");
                assert!(labels.windows(2).all(|w| w[0] != "---" || w[1] != "---"), "{labels:?}");
            }
            Some(other) => panic!("{other:?}"),
        }
        drop(view);
        drop(env);
        let _ = unsafe { DestroyWindow(hwnd) };
        if let Some(lp) = crate::localfs::LocalPath::judge(&folder) {
            let _ = crate::localfs::remove_dir_all(&lp);
        }
    }
}

/// Task 0 of 13x2: the isolated-world probe. Windows only, `#[ignore]`d, and run in a console
/// session (a session 0 process cannot host WebView2): every test makes one never-shown window,
/// one view on a fresh environment in a temporary folder, serves every page from memory (hosts
/// under `.invalid`) and fails on any request it did not serve. A skip is printed as such and is
/// never a pass.
#[cfg(test)]
mod probe {
    use super::*;
    use crate::browser_world::{self, ClientWorld, Contexts, Setup};
    use serde_json::{Value, json};
    use std::sync::mpsc;
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, TranslateMessage, WNDCLASSW, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW};

    const HOVER: &str = include_str!("../resources/browser/hover-link-detector.js");
    const GRAPH: &str = include_str!("../resources/browser/graph-discovery.js");

    /// The hostile page's first script: saves what it needs, then replaces the builtins a Client
    /// script might use with recorders, plants `toJSON` and a secret getter on `Object.prototype`,
    /// records the page's view of its own window, and holds the forgery functions the tests call.
    const HOSTILE: &str = r#"<script>
(function () {
  const keys = Object.keys, names = Object.getOwnPropertyNames, syms = Object.getOwnPropertySymbols;
  const origStringify = JSON.stringify, push = Array.prototype.push;
  const post = window.chrome.webview.postMessage.bind(window.chrome.webview);
  // `counts` has no prototype: 'toString' and 'toJSON' are counted, not read from Object.prototype.
  const R = [], seen = new Set(), counts = Object.create(null), ids = [];
  let busy = false;
  // The page's own answers to the harness, serialized with its `toJSON` plant lifted for the call:
  // the plant is on `Object.prototype`, so it would answer for the page's own objects too and every
  // answer would read {"hacked":true} (the 2026-10-07 desktop run).
  function out(x) {
    const d = Object.getOwnPropertyDescriptor(Object.prototype, 'toJSON');
    if (d) delete Object.prototype.toJSON;
    try { return origStringify(x); } finally { if (d) Object.defineProperty(Object.prototype, 'toJSON', d); }
  }
  function ser(x, d) {
    const t = typeof x;
    if (t === 'string') return x;
    if (t !== 'object' || x === null) { try { return t + ':' + x; } catch (e) { return t; } }
    if (d > 3) return '[deep]';
    let s = '{', ks;
    try { ks = keys(x); } catch (e) { return '[opaque]'; }
    for (let i = 0; i < ks.length && i < 200; i++) { let v; try { v = x[ks[i]]; } catch (e) { v = 'throws'; } s += ks[i] + '=' + ser(v, d + 1) + ';'; }
    return s + '}';
  }
  function rec(tag, self, args) {
    counts[tag] = (counts[tag] || 0) + 1;
    const s = tag + '|' + ser(self, 0) + '|' + ser(args, 0);
    if (!seen.has(s) && R.length < 2000) { seen.add(s); push.call(R, s); }
  }
  function wrap(o, n) {
    const orig = o[n];
    o[n] = function (...a) {
      if (busy) return orig.apply(this, a);
      busy = true;
      try { rec(n, this, a); } catch (e) {} finally { busy = false; }
      return orig.apply(this, a);
    };
  }
  wrap(Array.prototype, 'slice'); wrap(Array.prototype, 'join'); wrap(Promise.prototype, 'then');
  wrap(JSON, 'stringify'); wrap(Function.prototype, 'toString'); wrap(String.prototype, 'indexOf');
  Object.defineProperty(Object.prototype, 'toJSON', { value: function () { counts.toJSON = (counts.toJSON || 0) + 1; return { hacked: true }; }, configurable: true, writable: true });
  Object.defineProperty(Object.prototype, 'sessionSecret', { get: function () { counts.getter = (counts.getter || 0) + 1; return undefined; }, set: function (v) { counts.setter = (counts.setter || 0) + 1; rec('sessionSecretSet', null, [v]); }, configurable: true });
  try { window.chrome.webview.postMessage = function (m) { rec('postMessage', null, [m]); return post(m); }; } catch (e) {}
  window.addEventListener('message', function (e) {
    rec('message', null, [e.data]);
    try { const f = e.data && e.data.payload && e.data.payload.graph && e.data.payload.graph.frameId; if (typeof f === 'string') push.call(ids, f); } catch (x) {}
  }, true);
  setInterval(function () {
    let a = '', n = names(window);
    for (let i = 0; i < n.length; i++) a += n[i] + ',';
    rec('globals', null, [a]);
    let b = '', sy = syms(window);
    for (let i = 0; i < sy.length; i++) b += String(sy[i]) + ',';
    rec('symbols', null, [b]);
    rec('html', null, [document.documentElement.outerHTML]);
    let c = '', all = document.getElementsByTagName('*');
    for (let i = 0; i < all.length; i++) { const at = all[i].attributes; for (let j = 0; j < at.length; j++) c += at[j].name + '=' + at[j].value + ';'; }
    rec('attrs', null, [c]);
  }, 50);
  window.__report = function () { return out(R); };
  window.__counts = function () {
    const o = {}, n = ['slice', 'join', 'then', 'stringify', 'toString', 'indexOf', 'toJSON', 'getter', 'setter'];
    for (let i = 0; i < n.length; i++) o[n[i]] = counts[n[i]] || 0;
    return out(o);
  };
  window.__forge = function () {
    post({ h: 'probeEcho', b: { v: 2, sessionSecret: '{{GUESS}}' } });
    post({ h: 'iTermSelectionMonitor', b: { selection: 'x', sessionSecret: '{{GUESS}}' } });
    post({ handler: 'iterm2-about:history', body: { action: 'clearAllHistory' } });
    const r = {};
    try { r.client = typeof window.__tandemClient; } catch (e) { r.client = 'threw'; }
    try { r.webkit = typeof (window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.probeEcho && window.webkit.messageHandlers.probeEcho.postMessage({})); } catch (e) { r.webkit = 'threw'; }
    window.postMessage({ namespace: 'IFRAME_GRAPH_DISCOVERY', type: 'CHILD_REPORT', requestId: 'r', payload: { graph: {} } }, '*');
    return out(r);
  };
  window.__forge4 = function (extra) {
    const r = {};
    try { const d = window[Symbol.for('iTermGraphDiscovery')]; r.api = typeof (d && d.evaluateInFrame && d.evaluateInFrame('x', "document.title='pwned'", function () {})); } catch (e) { r.api = 'threw'; }
    r.global = typeof window.graphDiscoveryEvaluateInFrame;
    // Every pair of ids the page has or was handed (the world's own graph ids among them), as target
    // and as sender, from the page and from its child frame.
    const targets = ids.concat(['x', 'guess'], JSON.parse(extra));
    for (let i = 0; i < targets.length; i++) {
      for (let j = 0; j < targets.length; j++) {
        post({ h: 'iTermGraphDiscovery', b: { type: 'EVAL_IN_FRAME', target: targets[i], sender: targets[j], requestID: 'r', code: "document.title='pwned'" } });
        try { window.frames[0].__fire(targets[i], targets[j]); } catch (e) {}
      }
    }
    return out(r);
  };
})();
</script>"#;

    const CHILD_SRCDOC: &str = "&lt;title&gt;child&lt;/title&gt;&lt;p&gt;c&lt;/p&gt;&lt;script&gt;window.__fire=function(t,s){try{chrome.webview.postMessage({h:'iTermGraphDiscovery',b:{type:'EVAL_IN_FRAME',target:t,sender:s,requestID:'r',code:&quot;document.title='pwned'&quot;}})}catch(e){}}&lt;/script&gt;";

    fn hostile_page(body: &str, guess: &str) -> String {
        format!("<!doctype html><html><head><title>hostile</title>{}</head><body>{body}</body></html>", HOSTILE.replace("{{GUESS}}", guess))
    }

    fn hex32() -> String {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).expect("random");
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// A Client script as Task 1's `wrap` injects it: the binding captured in a closure before the
    /// body, `window.webkit.messageHandlers.<h>.postMessage(b)` posting `{h, b}` through it.
    fn wrapped(code: &str) -> String {
        crate::browser_js::wrap(&crate::browser_js::UserScript { id: "probe", code: code.to_string(), inject: crate::browser_js::Inject::DocumentStart, main_frame_only: false, world: crate::browser_js::World::Client })
    }

    fn client_sources(secret: &str, load_echo: bool) -> Vec<String> {
        let mut v = vec![wrapped(&HOVER.replace("{{SECRET}}", secret)), wrapped(GRAPH)];
        if load_echo {
            v.push(wrapped("window.webkit.messageHandlers.probeEcho.postMessage({where: location.href});"));
        }
        v
    }

    #[derive(Clone, Debug, PartialEq)]
    enum Via {
        Client,
        Page,
    }

    #[derive(Clone, Debug)]
    struct Routed {
        via: Via,
        frame: Option<u64>,
        /// The posting document's URL (Page) or the context's origin (Client).
        from: String,
        body: String,
    }

    impl Routed {
        /// A Client message is `{h, b}` as the probe's wrapper makes it.
        fn client(&self) -> Option<(String, Value)> {
            let v: Value = serde_json::from_str(&self.body).ok()?;
            Some((v.get("h")?.as_str()?.to_string(), v.get("b")?.clone()))
        }
    }

    struct Probe {
        hwnd: HWND,
        folder: PathBuf,
        view: Option<WebView>,
        env: Option<Env>,
        rx: mpsc::Receiver<Posted>,
        pages: HashMap<String, String>,
        not_served: Vec<String>,
        contexts: Contexts,
        setup: Setup,
        sources: Vec<String>,
        next_token: u64,
        setup_token: Option<u64>,
        sessions: HashMap<String, VecDeque<(&'static str, Value)>>,
        session_tokens: HashMap<u64, String>,
        answers: HashMap<u64, Result<String, String>>,
        /// Every DevTools call and every ExecuteScript the harness issued, with its text.
        log: Vec<String>,
        routed: Vec<Routed>,
        /// `Runtime.bindingCalled` events `Contexts` did not accept as Tandem's.
        rejected_bindings: usize,
        attached: Vec<(String, String)>,
        navigated: Vec<(String, bool)>,
        failed: Vec<String>,
        created: bool,
        /// WebView2's own frame ids (`watch_frames`).
        frames: Vec<u64>,
        /// The production frame graph (`browser_frames::Frames`), when a test turns it on: every
        /// message goes through the app's own rule, Client from the binding and Page from the page.
        graph: Option<crate::browser_frames::Frames>,
        /// A graph evaluation's DevTools token, and the graph's own token for it.
        graph_tokens: HashMap<u64, u64>,
        /// Evaluations the graph planned for a Page-world message (production plans none).
        page_graph_acts: usize,
        /// Page-initiated windows the host let through (`Event::NewWindow`: user-initiated only).
        new_windows: Vec<String>,
    }

    impl Probe {
        /// None (with a printed SKIPPED line) when the runtime is missing or this is session 0.
        fn start(pages: &[(&str, String)], sources: Vec<String>) -> Option<Probe> {
            Probe::start_with(pages, sources, false)
        }

        /// `start`, with `new_windows` as the view's setup (the browser pane's is on): a page's
        /// `window.open` then reaches the host's limiter, and an allowed one is `Event::NewWindow`.
        fn start_with(pages: &[(&str, String)], sources: Vec<String>, new_windows: bool) -> Option<Probe> {
            if runtime_version(None).is_err() {
                println!("SKIPPED (not a pass): WebView2 Runtime not installed; skipped");
                return None;
            }
            let mut session = 0u32;
            // SAFETY: plain out parameter.
            unsafe { windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(std::process::id(), &mut session) };
            println!("probe: process session {session}");
            if session == 0 {
                println!("SKIPPED (not a pass): session 0 cannot host WebView2; skipped");
                return None;
            }
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
                unsafe { DefWindowProcW(h, m, w, l) }
            }
            let class = windows::core::w!("TandemWorldProbe");
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), lpszClassName: class, ..Default::default() };
            unsafe { RegisterClassW(&wc) };
            // Created, never shown: nothing appears on the desktop.
            let hwnd = unsafe { CreateWindowExW(WS_EX_TOOLWINDOW, class, windows::core::w!("tandem-world-probe"), WS_OVERLAPPEDWINDOW, 0, 0, 400, 300, None, None, None, None) }.expect("a window");
            let folder = std::env::temp_dir().join(format!("tandem-probe-{}-{}", std::process::id(), hex32()));
            let (tx, rx) = mpsc::channel::<Posted>();
            let mut p = Probe {
                hwnd,
                folder: folder.clone(),
                view: None,
                env: None,
                rx,
                pages: pages.iter().map(|(u, b)| (u.to_string(), b.clone())).collect(),
                not_served: Vec::new(),
                contexts: Contexts::default(),
                setup: Setup::unchecked(&sources),
                sources,
                next_token: 1,
                setup_token: None,
                sessions: HashMap::new(),
                session_tokens: HashMap::new(),
                answers: HashMap::new(),
                log: Vec::new(),
                routed: Vec::new(),
                rejected_bindings: 0,
                attached: Vec::new(),
                navigated: Vec::new(),
                failed: Vec::new(),
                created: false,
                frames: Vec::new(),
                graph: None,
                graph_tokens: HashMap::new(),
                page_graph_acts: 0,
                new_windows: Vec::new(),
            };
            let cell: Rc<RefCell<Option<Result<Env, String>>>> = Rc::new(RefCell::new(None));
            let c2 = cell.clone();
            Env::create(Store::Default, &folder, None, &EnvOptions::default(), Box::new(move |r| *c2.borrow_mut() = Some(r)));
            assert!(p.wait(20, |_| cell.borrow().is_some()), "an environment within 20 s");
            let env = cell.borrow_mut().take().unwrap().expect("the environment");
            let setup = ViewSetup { guard_local_schemes: true, web_message: true, new_windows, resource_filters: vec!["*".into()], ..Default::default() };
            let view = WebView::create(&env, hwnd.0 as isize, 1, (0, 0, 400, 300), setup, tx);
            p.env = Some(env);
            p.view = Some(view);
            assert!(p.wait(20, |p| p.created), "Created within 20 s");
            for name in browser_world::EVENTS {
                p.view.as_ref().unwrap().devtools_subscribe(name);
            }
            p.step_setup();
            assert!(p.wait(15, |p| *p.setup.state() != ClientWorld::Setting), "the setup finished within 15 s: {:?}", p.log.iter().map(|l| l.chars().take(60).collect::<String>()).collect::<Vec<_>>());
            assert_eq!(*p.setup.state(), ClientWorld::Ready, "the isolated world is unavailable: {:?}", p.setup.state());
            Some(p)
        }

        fn view(&self) -> &WebView {
            self.view.as_ref().expect("the view")
        }

        fn token(&mut self) -> u64 {
            self.next_token += 1;
            self.next_token
        }

        /// One DevTools call; its answer arrives as `Event::ScriptDone` with the returned token.
        fn call(&mut self, session: Option<&str>, method: &str, params: &Value) -> u64 {
            let (t, ps) = (self.token(), params.to_string());
            self.log.push(format!("{method} {}{ps}", session.map(|s| format!("[{s}] ")).unwrap_or_default()));
            match session {
                None => self.view().devtools_call(method, &ps, t),
                Some(s) => self.view().devtools_call_for_session(s, method, &ps, t),
            }
            t
        }

        fn call_wait(&mut self, session: Option<&str>, method: &str, params: &Value) -> Result<String, String> {
            let t = self.call(session, method, params);
            if !self.wait(10, |p| p.answers.contains_key(&t)) {
                return Err(format!("{method}: no answer in 10 s"));
            }
            self.answers.remove(&t).unwrap()
        }

        /// `Runtime.evaluate` in a frame's Tandem world (None is the main frame).
        fn eval(&mut self, frame: Option<u64>, expr: &str) -> Result<Value, String> {
            let (session, unique) = self.contexts.unique_for(frame).ok_or("no Tandem context for that frame")?;
            let r = self.call_wait(session.as_deref(), "Runtime.evaluate", &browser_world::evaluate_params(&unique, expr))?;
            browser_world::evaluate_result(&r)
        }

        /// A script in the page's own world (`ExecuteScript`); the answer is the result's JSON.
        fn page_eval(&mut self, js: &str) -> Result<String, String> {
            let t = self.token();
            self.log.push(format!("ExecuteScript {js}"));
            self.view().execute_script(js, t);
            if !self.wait(10, |p| p.answers.contains_key(&t)) {
                return Err("ExecuteScript: no answer in 10 s".into());
            }
            self.answers.remove(&t).unwrap()
        }

        fn load(&mut self, url: &str) {
            let n = self.navigated.len();
            self.view().navigate_tandem(url);
            assert!(self.wait(15, |p| p.navigated.len() > n), "Navigated within 15 s");
            assert!(self.navigated[n].1, "the navigation to {url} failed");
            assert!(self.wait(5, |p| browser_world::world_usable(p.setup.state(), &p.contexts)), "the main frame has a Tandem context after loading {url}");
        }

        fn step_setup(&mut self) {
            if let Some((m, params)) = self.setup.next_call() {
                let t = self.call(None, m, &params);
                self.setup_token = Some(t);
            }
        }

        fn step_session(&mut self, sid: &str) {
            let next = self.sessions.get_mut(sid).and_then(|q| q.pop_front());
            match next {
                Some((m, params)) => {
                    let t = self.call(Some(sid), m, &params);
                    self.session_tokens.insert(t, sid.to_string());
                }
                None => {
                    self.sessions.remove(sid);
                }
            }
        }

        fn attach(&mut self, sid: &str, ty: &str) {
            self.attached.push((sid.to_string(), ty.to_string()));
            // An iframe gets the world; any other target is only resumed (it would wait for ever).
            let frame = matches!(browser_world::attached(&json!({"sessionId": sid, "targetInfo": {"type": ty}}).to_string(), self.sessions.len()), browser_world::Attach::Frame { .. });
            let q: VecDeque<_> = if frame { browser_world::session_calls(&self.sources).into() } else { vec![("Runtime.runIfWaitingForDebugger", json!({}))].into() };
            self.sessions.insert(sid.to_string(), q);
            self.step_session(sid);
        }

        fn handle(&mut self, e: Event) {
            match e {
                Event::Created(_) => self.created = true,
                Event::Failed(_, why) => self.failed.push(why),
                Event::ProcessFailed { kind, .. } => self.failed.push(format!("process failed {kind}")),
                Event::Navigated { ok, url, .. } => self.navigated.push((url, ok)),
                Event::Request { token, url, .. } => {
                    let key = url.split(['#', '?']).next().unwrap_or("").to_string();
                    let answer = match self.pages.get(&key) {
                        Some(body) => Answer::Respond { status: 200, mime: "text/html".into(), body: body.clone().into_bytes(), headers: vec![] },
                        None => {
                            if !key.ends_with("/favicon.ico") {
                                self.not_served.push(url);
                            }
                            Answer::Respond { status: 404, mime: "text/plain".into(), body: b"not found".to_vec(), headers: vec![] }
                        }
                    };
                    self.view().answer_request(token, answer);
                }
                Event::Message { json, source, .. } => {
                    self.graph_page(&json, None, &source);
                    self.routed.push(Routed { via: Via::Page, frame: None, from: source, body: json });
                }
                Event::FrameCreated { frame, .. } => self.frames.push(frame),
                Event::NewWindow { url, .. } => self.new_windows.push(url),
                Event::FrameMessage { frame, json, source, .. } => {
                    self.graph_page(&json, Some(frame), &source);
                    self.routed.push(Routed { via: Via::Page, frame: Some(frame), from: source, body: json });
                }
                Event::ScriptDone { token, json, .. } => {
                    if self.setup_token == Some(token) {
                        self.setup_token = None;
                        self.setup.answered(json);
                        if let Some(m) = self.setup.main_frame() {
                            self.contexts.set_main_frame(m);
                        }
                        self.step_setup();
                    } else if let Some(g) = self.graph_tokens.remove(&token) {
                        let value = json.and_then(|t| browser_world::evaluate_result(&t));
                        let acts = self.graph.as_mut().map(|graph| graph.answered(g, &value).1).unwrap_or_default();
                        self.graph_run(acts);
                    } else if let Some(sid) = self.session_tokens.remove(&token) {
                        if json.is_err() {
                            // The frame then has no Tandem world, but it is still resumed.
                            if let Some(q) = self.sessions.get_mut(&sid) {
                                let last = q.pop_back();
                                q.clear();
                                q.extend(last);
                            }
                        }
                        self.step_session(&sid);
                    } else {
                        self.answers.insert(token, json);
                    }
                }
                Event::DevTools { session, name, json, .. } => {
                    self.devtools_held();
                    self.devtools(session, &name, &json);
                }
                Event::DevToolsHeld { .. } => self.devtools_held(),
                _ => {}
            }
        }

        /// A Page-world message as the app routes it (`browser_script_event`): Page, to the same graph.
        fn graph_page(&mut self, json: &str, frame: Option<u64>, source: &str) {
            let Some(routed) = crate::browser_js::route(json, crate::browser_js::World::Page, frame, source) else { return };
            if routed.handler != crate::browser_frames::HANDLER {
                return;
            }
            let acts = self.graph.as_mut().map(|g| g.handle(&routed, true, &mut crate::browser_js::new_secret)).unwrap_or_default();
            self.page_graph_acts += acts.len();
            self.graph_run(acts);
        }

        /// What the graph planned, evaluated as the app does (`graph_run`): in the Tandem world of the
        /// frame it names, the answer back to the graph. A frame with no Tandem context gets nothing.
        fn graph_run(&mut self, acts: Vec<crate::browser_frames::Act>) {
            for act in acts {
                let Some((session, unique)) = self.contexts.unique_for(act.frame) else { continue };
                let t = self.call(session.as_deref(), "Runtime.evaluate", &browser_world::evaluate_params(&unique, &crate::browser_js::make_safe(&act.code)));
                self.graph_tokens.insert(t, act.token);
            }
        }

        /// The graph ids the world's `REGISTER_FRAME` messages named, with the frame each came from.
        fn graph_ids(&self) -> Vec<(Option<u64>, String)> {
            self.clients(crate::browser_frames::HANDLER).into_iter().filter(|(_, b)| b["type"] == "REGISTER_FRAME").filter_map(|(f, b)| b["frameId"].as_str().map(|id| (f, id.to_string()))).collect()
        }

        fn devtools_held(&mut self) {
            for (session, name, json) in self.view().take_devtools_held() {
                self.devtools(session, &name, &json);
            }
        }

        fn devtools(&mut self, session: Option<String>, name: &str, json: &str) {
            let s = session.as_deref();
            match name {
                "Runtime.executionContextCreated" => {
                    self.contexts.created(s, json);
                }
                "Runtime.executionContextDestroyed" => self.contexts.destroyed(s, json),
                "Runtime.executionContextsCleared" => self.contexts.cleared(s),
                "Page.frameNavigated" => self.contexts.navigated(s, json),
                "Runtime.bindingCalled" => match self.contexts.binding_called(s, json) {
                    Some((frame, origin, payload)) => {
                        // As the app routes it (`browser_devtools_event`): Client, and the graph's
                        // messages to the production `Frames`.
                        if let Some(routed) = crate::browser_js::route(&payload, crate::browser_js::World::Client, frame, &origin) {
                            if routed.handler == crate::browser_frames::HANDLER {
                                let acts = self.graph.as_mut().map(|g| g.handle(&routed, true, &mut crate::browser_js::new_secret)).unwrap_or_default();
                                self.graph_run(acts);
                            }
                        }
                        self.routed.push(Routed { via: Via::Client, frame, from: origin, body: payload });
                    }
                    None => self.rejected_bindings += 1,
                },
                "Target.attachedToTarget" => {
                    if let Ok(v) = serde_json::from_str::<Value>(json) {
                        let sid = v.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
                        let ty = v.pointer("/targetInfo/type").and_then(Value::as_str).unwrap_or("").to_string();
                        if !sid.is_empty() {
                            self.attach(&sid, &ty);
                        }
                    }
                }
                "Target.detachedFromTarget" => {
                    if let Some(sid) = serde_json::from_str::<Value>(json).ok().and_then(|v| v.get("sessionId").and_then(Value::as_str).map(str::to_string)) {
                        self.contexts.detached(&sid);
                        self.sessions.remove(&sid);
                    }
                }
                _ => {}
            }
        }

        /// Pumps the thread's messages and the view's events until `cond` holds or `secs` pass.
        fn wait(&mut self, secs: u64, mut cond: impl FnMut(&Probe) -> bool) -> bool {
            let end = Instant::now() + Duration::from_secs(secs);
            loop {
                let mut msg = MSG::default();
                unsafe {
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                while let Ok(p) = self.rx.try_recv() {
                    if let Some(e) = p.event {
                        self.handle(e);
                    }
                }
                assert!(self.failed.is_empty(), "the view failed: {:?}", self.failed);
                if cond(self) {
                    return true;
                }
                if Instant::now() >= end {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// Lets the page and the world run for `ms` (a bounded pump, never an open wait).
        fn settle(&mut self, ms: u64) {
            let end = Instant::now() + Duration::from_millis(ms);
            self.wait(ms / 1000 + 1, |_| Instant::now() >= end);
        }

        fn clients(&self, handler: &str) -> Vec<(Option<u64>, Value)> {
            self.routed.iter().filter(|r| r.via == Via::Client).filter_map(|r| r.client().filter(|(h, _)| h == handler).map(|(_, b)| (r.frame, b))).collect()
        }

        /// What every test ends with: every request was answered from memory and the world never
        /// had a binding call from a context it does not know.
        fn finish(&self) {
            assert!(self.not_served.is_empty(), "requests not served from memory: {:?}", self.not_served);
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            self.view.take();
            self.env.take();
            let _ = unsafe { DestroyWindow(self.hwnd) };
            // Best effort: the browser process may still hold the folder for a moment.
            if let Some(lp) = crate::localfs::LocalPath::judge(&self.folder) {
                let _ = crate::localfs::remove_dir_all(&lp);
            }
        }
    }

    const PAGE: &str = "https://probe.tandem-probe.invalid/";

    /// The echo as the app evaluates a posting body in the Tandem world (`browser_js_call`, `call_js`):
    /// the world has no global `window.webkit` (Task 1's bridge is closure-local, so the page can never
    /// replace it), and `call_js` gives the body the same rewrite and binding capture `wrap` does.
    fn echo(secret: &str) -> String {
        crate::browser_js::call_js(&format!("window.webkit.messageHandlers.probeEcho.postMessage({{v: 1, sessionSecret: {}}});", crate::browser_js::js_string(secret)), &Value::Null).expect("the echo")
    }

    /// The text Task 5's `clear_script` will produce, written out here.
    fn clear_hover(secret: &str) -> String {
        format!("(function() {{window.iTermHoverLinkHandler?.clearHover('{secret}');}})();")
    }

    /// The 2026-10-07 desktop run (probe-fix-report.md): the hostile page's answers to the harness
    /// are its own data, never its `toJSON` plant's `{"hacked":true}`, and its counters cannot read
    /// `Object.prototype`; the echo is evaluated as the app evaluates a posting body (the world has
    /// no global `window.webkit`). Pure: no view is made.
    #[test]
    fn probe_page_answers_and_echo_are_real() {
        assert_eq!(HOSTILE.matches("origStringify(").count(), 1, "called only by out(), {} answers called it directly", HOSTILE.matches("return origStringify(").count());
        assert!(HOSTILE.contains("try { return origStringify(x); } finally {"), "out() lifts the plant for its own call");
        for f in ["window.__report = function () { return out(R); };", "    return out(o);\n"] {
            assert!(HOSTILE.contains(f), "{f}");
        }
        assert_eq!(HOSTILE.matches("    return out(r);\n").count(), 2, "both forgeries answer through out()");
        assert!(HOSTILE.contains("counts = Object.create(null)"));
        let e = echo("abc");
        assert!(!e.contains("window.webkit") && e.contains("const __tandemPost = globalThis.__tandemClient;"), "{e}");
        assert!(e.contains("__tandemWebkit.messageHandlers.probeEcho.postMessage({v: 1, sessionSecret: \"abc\"});"), "{e}");
    }

    #[test]
    #[ignore]
    fn probe_page_cannot_read_the_secret() {
        let secret = hex32();
        // P9: a child frame is part of the page, so the parent's page world, with its capture-phase
        // `message` recorder, sees every DOM message the world's scripts post to a parent. The secret
        // must be in none of them (the view secret never goes through `postMessage`).
        let body = "<a href=\"https://x.invalid/\">link</a><iframe srcdoc=\"&lt;p&gt;child&lt;/p&gt;&lt;a href=&quot;https://y.invalid/&quot;&gt;l&lt;/a&gt;\"></iframe>";
        let Some(mut p) = Probe::start(&[(PAGE, hostile_page(body, "g"))], client_sources(&secret, false)) else { return };
        p.load(PAGE);
        assert!(p.wait(10, |p| p.contexts.frames().len() == 1), "the child frame has a world, so its reports are posted");
        assert_eq!(p.eval(None, "typeof globalThis.__tandemClient"), Ok(json!("function")), "the world is live: the binding");
        assert_eq!(p.eval(None, "typeof window.iTermHoverLinkHandler"), Ok(json!("object")), "the world is live: hover");
        assert_eq!(p.eval(None, "typeof window[Symbol.for('iTermGraphDiscovery')]"), Ok(json!("object")), "the world is live: graph discovery");
        p.eval(None, &clear_hover(&secret)).expect("clearHover");
        p.eval(None, &echo(&secret)).expect("the echo");
        assert!(p.wait(5, |p| !p.clients("probeEcho").is_empty()), "the echo reaches the router");
        // The page's own world sees none of it.
        assert_eq!(p.page_eval("typeof window.__tandemClient"), Ok("\"undefined\"".into()));
        assert_eq!(p.page_eval("typeof window.iTermHoverLinkHandler"), Ok("\"undefined\"".into()));
        assert_eq!(p.page_eval("typeof window[Symbol.for('iTermGraphDiscovery')]"), Ok("\"undefined\"".into()));
        p.settle(500);
        let report = p.page_eval("window.__report()").expect("the report");
        assert!(report.len() > 100, "the page recorded something: {}", report.len());
        assert!(!report.contains(&secret), "the secret reached the page");
        assert!(p.routed.iter().filter(|r| r.via == Via::Page).all(|r| !r.body.contains(&secret)));
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    #[test]
    #[ignore]
    fn probe_page_builtins_do_not_reach_the_world() {
        let secret = hex32();
        let Some(mut p) = Probe::start(&[(PAGE, hostile_page("<p>x</p>", "g"))], client_sources(&secret, false)) else { return };
        p.load(PAGE);
        // Control: the recorders are live in the page's own world.
        let c0 = p.page_eval("window.__counts()").unwrap();
        p.page_eval("[1, 2, 3].slice(1).join(',') + JSON.stringify({a: 1})").unwrap();
        let c1 = p.page_eval("window.__counts()").unwrap();
        assert_ne!(c0, c1, "the recorders count the page's own calls");
        p.settle(300);
        let before = p.page_eval("window.__counts()").unwrap();
        p.eval(None, &clear_hover(&secret)).expect("clearHover");
        p.eval(None, &echo(&secret)).expect("the echo");
        let got = p.eval(None, "(async () => { const a = 21; return await Promise.resolve(a).then(x => x * 2); })()");
        assert_eq!(got, Ok(json!(42)), "the world's own Promise, JSON and Object are intact");
        assert!(p.wait(5, |p| !p.clients("probeEcho").is_empty()));
        p.settle(300);
        let after = p.page_eval("window.__counts()").unwrap();
        assert_eq!(before, after, "the world's calls reached the page's builtins");
        let echoes = p.clients("probeEcho");
        assert_eq!(echoes.len(), 1);
        assert_eq!(echoes[0].1, json!({"v": 1, "sessionSecret": secret}), "the page's toJSON had no effect");
        p.finish();
    }

    #[test]
    #[ignore]
    fn probe_page_cannot_fake_a_client_message() {
        let secret = hex32();
        let guess = hex32();
        let Some(mut p) = Probe::start(&[(PAGE, hostile_page("<p>x</p>", &guess))], client_sources(&secret, false)) else { return };
        p.load(PAGE);
        p.eval(None, &echo(&secret)).expect("the echo");
        let forged = p.page_eval("window.__forge()").expect("the forgery ran");
        // ExecuteScript answers with JSON: a string result arrives quoted, so decode it first.
        let forged = serde_json::from_str::<String>(&forged).unwrap_or(forged);
        assert!(forged.contains("\"client\":\"undefined\""), "the binding does not exist in the page world: {forged}");
        assert!(!forged.contains("threw"), "{forged}");
        p.settle(1000);
        let pages: Vec<&Routed> = p.routed.iter().filter(|r| r.via == Via::Page).collect();
        assert!(pages.len() >= 3, "the page's three messages arrived: {pages:?}");
        assert!(pages.iter().all(|r| r.from.starts_with(PAGE)), "every page message names the page as its source: {pages:?}");
        for r in p.routed.iter().filter(|r| r.via == Via::Client) {
            let (h, b) = r.client().expect("a client message is {h, b}");
            assert!(h == "probeEcho" || h == "iTermGraphDiscovery", "a client message from the page: {h}");
            assert!(!r.body.contains(&guess), "the page's guess reached the world's channel");
            if h == "probeEcho" {
                assert_eq!(b, json!({"v": 1, "sessionSecret": secret}), "only the world's own echo");
            }
        }
        assert_eq!(p.clients("probeEcho").len(), 1);
        assert_eq!(p.rejected_bindings, 0, "no binding call came from a context the table does not know");
        p.finish();
    }

    /// The page cannot make Tandem evaluate anything in any frame. The harness runs the app's own
    /// rule: the production `Frames` gets every Client message from the binding and every Page message
    /// from the page, and evaluates what it plans in the Tandem world. The page forges `EVAL_IN_FRAME`
    /// from itself and from its child frame with every pair of ids it has, the world's real graph ids
    /// among them; nothing is planned for a Page message and no forged code is evaluated. The control:
    /// the world's own `evaluateInFrame` still answers, across frames, through the same path.
    #[test]
    #[ignore]
    fn probe_page_cannot_reach_evaluate_in_frame() {
        let secret = hex32();
        let body = format!("<iframe srcdoc=\"{CHILD_SRCDOC}\"></iframe>");
        let Some(mut p) = Probe::start(&[(PAGE, hostile_page(&body, "g"))], client_sources(&secret, false)) else { return };
        p.graph = Some(crate::browser_frames::Frames::default());
        p.view().watch_frames();
        p.load(PAGE);
        assert!(p.wait(10, |p| p.contexts.frames().len() == 1), "the iframe has a Tandem world");
        let k = p.contexts.frames()[0];
        assert!(p.wait(10, |p| p.graph_ids().iter().any(|(f, _)| f.is_none()) && p.graph_ids().iter().any(|(f, _)| *f == Some(k))), "both frames registered with the graph: {:?}", p.graph_ids());
        let graph_ids: Vec<String> = p.graph_ids().into_iter().map(|(_, id)| id).collect();
        let main_id = p.graph_ids().into_iter().find(|(f, _)| f.is_none()).unwrap().1;
        let child_id = p.graph_ids().into_iter().find(|(f, _)| *f == Some(k)).unwrap().1;
        let mut extra = p.call_wait(None, "Page.getFrameTree", &json!({})).ok().and_then(|t| browser_world::frame_ids(&t)).map(|(_, all)| all).unwrap_or_default();
        extra.extend(graph_ids);
        let as_js_string = serde_json::to_string(&serde_json::to_string(&extra).unwrap()).unwrap();
        let r = p.page_eval(&format!("window.__forge4({as_js_string})")).expect("the forgery ran");
        let r: Value = serde_json::from_str::<String>(&r).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        assert_eq!(r, json!({"api": "undefined", "global": "undefined"}), "the page world has no graph API");
        p.settle(1000);
        let forged = p.routed.iter().filter(|r| r.via == Via::Page && r.body.contains("EVAL_IN_FRAME")).count();
        assert!(forged > 0, "the page's forgeries reached the host (as the page's)");
        assert_eq!(p.page_graph_acts, 0, "the graph planned nothing for a Page message");
        assert_eq!(p.eval(Some(k), "document.title"), Ok(json!("child")), "the iframe's title");
        assert_eq!(p.page_eval("document.title"), Ok("\"hostile\"".into()), "the page's title");
        assert!(p.log.iter().filter(|l| l.starts_with("Runtime.evaluate") || l.starts_with("ExecuteScript")).all(|l| !l.contains("pwned")), "a forged EVAL_IN_FRAME was evaluated");
        // Control: the path is live for the world itself, both ways across frames.
        let ask = |target: &str| format!("new Promise(function (res) {{ window[Symbol.for('iTermGraphDiscovery')].evaluateInFrame({}, 'document.title', res, 5000); }})", crate::browser_js::js_string(target));
        assert_eq!(p.eval(Some(k), &ask(&main_id)), Ok(json!("hostile")), "the child's world asks the main frame");
        assert_eq!(p.eval(None, &ask(&child_id)), Ok(json!("child")), "the main world asks the child frame");
        assert_eq!(p.page_graph_acts, 0);
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    /// 13x3 Task 3 (frame graph for find): a page with a `srcdoc` iframe registers both frames with the
    /// production `Frames` within 5 s, and a rediscovery (`REQUEST_REPORT`) completes both phases:
    /// `prepareForReport` then `report` in each frame, ending with the main frame's graph naming the
    /// child by the id it registered. The forged-message half of the brief is
    /// `probe_page_cannot_reach_evaluate_in_frame` (one test, one path). Needs the console session.
    #[test]
    #[ignore]
    fn graph_reaches_a_srcdoc_iframe() {
        let secret = hex32();
        let page = "<!doctype html><html><head><title>top</title></head><body><iframe srcdoc=\"&lt;p&gt;x&lt;/p&gt;\"></iframe></body></html>".to_string();
        let Some(mut p) = Probe::start(&[(PAGE, page)], client_sources(&secret, false)) else { return };
        p.graph = Some(crate::browser_frames::Frames::default());
        p.view().watch_frames();
        p.load(PAGE);
        assert!(p.wait(5, |p| p.graph_ids().len() == 2), "two frames registered within 5 s: {:?}", p.graph_ids());
        let ids = p.graph_ids();
        let main_id = ids.iter().find(|(f, _)| f.is_none()).expect("the main frame registered").1.clone();
        let child = ids.iter().find(|(f, _)| f.is_some()).expect("the iframe registered").clone();
        assert_eq!(p.graph.as_ref().unwrap().lookup(&child.1), Some(child.0), "the graph resolves the child's id to its frame");
        let ask = "new Promise(function (res) { window[Symbol.for('iTermGraphDiscovery')].rediscover(res, 5000); })";
        let before = p.log.len();
        let g = p.eval(None, ask).expect("the rediscovery completes");
        assert_eq!(g["frameId"], json!(main_id), "{g}");
        assert_eq!(g["children"].as_array().map(Vec::len), Some(1), "{g}");
        assert_eq!(g["children"][0]["frameId"], json!(child.1), "the main frame's graph names the child by its registered id: {g}");
        let evals: Vec<&String> = p.log[before..].iter().filter(|l| l.starts_with("Runtime.evaluate")).collect();
        let phase = |name: &str| evals.iter().filter(|l| l.contains(name)).count();
        assert_eq!(phase("prepareForReport"), 2, "phase one reached both frames: {evals:?}");
        assert_eq!(phase(".report("), 2, "phase two reached both frames: {evals:?}");
        let last_prepare = evals.iter().rposition(|l| l.contains("prepareForReport")).unwrap();
        let first_report = evals.iter().position(|l| l.contains(".report(")).unwrap();
        assert!(last_prepare < first_report, "reports start after every frame prepared");
        assert_eq!(p.page_graph_acts, 0);
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    #[test]
    #[ignore]
    fn probe_world_reaches_every_frame() {
        let secret = hex32();
        let (top, child) = ("https://a.tandem-probe.invalid/", "https://b.tandem-probe.invalid/child.html");
        let nested = "&lt;iframe srcdoc=&quot;&amp;lt;p&amp;gt;nested&amp;lt;/p&amp;gt;&quot;&gt;&lt;/iframe&gt;&lt;p&gt;mid&lt;/p&gt;";
        let top_page = format!(
            "<!doctype html><html><head><title>top</title></head><body><iframe srcdoc=\"{nested}\"></iframe><iframe src=\"{child}\"></iframe><script>\n\
             window.__worker = 'pending'; try {{ const w = new Worker(URL.createObjectURL(new Blob(['postMessage(\"ran\")'], {{ type: 'text/javascript' }}))); w.onmessage = function (m) {{ window.__worker = m.data; }}; w.onerror = function () {{ window.__worker = 'error'; }}; }} catch (x) {{ window.__worker = 'threw'; }}\n\
             const e = new Error('x'); Object.defineProperty(e, 'stack', {{ get() {{ window.__stackRead = true; return ''; }} }}); console.debug(e);\n\
             setTimeout(function () {{ const f = document.createElement('iframe'); f.srcdoc = '<p>late</p>'; document.body.appendChild(f); }}, 300);\n\
             </script></body></html>"
        );
        let Some(mut p) = Probe::start(&[(top, top_page), (child, "<!doctype html><title>child</title><p>b</p>".to_string())], client_sources(&secret, true)) else { return };
        p.load(top);
        // B11: a worker is auto-attached and waits for a debugger; the host resumes it, so it runs.
        let mut worker_ran = false;
        for _ in 0..30 {
            if p.page_eval("String(window.__worker)").is_ok_and(|v| v.contains("ran")) {
                worker_ran = true;
                break;
            }
            p.settle(500);
        }
        assert!(worker_ran, "a page's worker runs: {:?}", p.page_eval("String(window.__worker)"));
        let frames_ok = p.wait(15, |p| p.contexts.frames().len() == 4);
        let seen = format!("attached {:?}; frames {:?}; rejected {}", p.attached, p.contexts.frames(), p.rejected_bindings);
        assert!(frames_ok, "every frame has a Tandem world: {seen}");
        assert!(p.wait(10, |p| {
            let mut f: Vec<Option<u64>> = p.clients("probeEcho").into_iter().map(|(f, _)| f).collect();
            f.sort();
            f.dedup();
            f.len() == 5
        }), "each of the five frames posted its echo through the binding: {seen}");
        let mut hrefs = Vec::new();
        let mut cross_on_session = false;
        let mut keys: Vec<Option<u64>> = vec![None];
        keys.extend(p.contexts.frames().into_iter().map(Some));
        for k in keys {
            let h = p.eval(k, "location.href").expect("location.href in a frame's world");
            let h = h.as_str().unwrap_or("").to_string();
            if h == child {
                cross_on_session = p.contexts.context_for(k).is_some_and(|(s, _)| s.is_some());
            }
            hrefs.push(h);
        }
        hrefs.sort();
        let mut want = vec!["about:srcdoc", "about:srcdoc", "about:srcdoc", child, top];
        want.sort();
        assert_eq!(hrefs, want, "{hrefs:?}");
        // The runtime may keep a cross-site frame in-process (the 2026-10-07 console run attached no
        // iframe target for *.invalid hosts). The world reaching it is asserted above by its href;
        // when the frame IS out of process, its world must be on the child session.
        if p.attached.iter().any(|(_, kind)| kind == "iframe") {
            assert!(cross_on_session, "the cross-site frame's world is on its child session: {seen}");
        } else {
            eprintln!("probe: the cross-site frame ran in-process; its world was reached ({seen})");
        }
        p.settle(500);
        let stack = p.page_eval("String(window.__stackRead === true)").unwrap_or_default();
        println!("Runtime.enable detectable: {}", if stack.contains("true") { "yes" } else { "no" });
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    /// Task 5: a pointer move over a link raises the hover script's `iTermHoverLink` message in
    /// the Tandem world, routed as Client with the link's URL; both hosts are under `.invalid` so no
    /// preconnect can leave the machine. Needs the console session (skipped elsewhere, never a pass).
    #[test]
    #[ignore]
    fn hover_reaches_the_router() {
        let secret = hex32();
        let url = "https://hover.invalid/x";
        let page_html = format!("<!doctype html><html><body style=\"margin:0\"><a href=\"{url}\" style=\"display:block;width:200px;height:50px\">link</a><a href=\"#a\">frag</a></body></html>");
        let Some(mut p) = Probe::start(&[(PAGE, page_html)], vec![wrapped(&HOVER.replace("{{SECRET}}", &secret))]) else { return };
        p.load(PAGE);
        p.call_wait(None, "Input.dispatchMouseEvent", &json!({"type": "mouseMoved", "x": 20, "y": 20})).expect("the move");
        assert!(p.wait(10, |p| !p.clients("iTermHoverLink").is_empty()), "the move over the link reaches the router");
        let (_, body) = p.clients("iTermHoverLink").into_iter().next().unwrap();
        let hover = crate::browser_tip::hover_of(&body).expect("a hover message");
        match hover {
            crate::browser_tip::Hover::Show { url: u, rect } => {
                assert_eq!(u, url);
                assert!(rect.2 >= 100.0 && rect.3 >= 20.0, "the link's rectangle: {rect:?}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(body.get("sessionSecret").and_then(Value::as_str), Some(secret.as_str()));
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    /// Review Focus 1 (I3, I8, M11): a copied script's `window.webkit` post reaches the router as the
    /// Tandem world's, a page's own post as the page's, in every frame; a page that replaces
    /// `window.chrome.webview.postMessage` sees nothing of the world's traffic; `window.webkit` is
    /// undefined for an https page; a local page's form from a web source is not a local page's; a
    /// call into the world answers. (An `iterm2-about:` page needs the registered scheme: the
    /// desktop run of the app covers it.)
    #[test]
    #[ignore]
    fn bridge_round_trip() {
        let nested = "&lt;iframe srcdoc=&quot;&amp;lt;p&amp;gt;nested&amp;lt;/p&amp;gt;&quot;&gt;&lt;/iframe&gt;&lt;p&gt;mid&lt;/p&gt;";
        let page_html = format!(
            "<!doctype html><html><head><title>rt</title></head><body><script>\n\
             window.chrome.webview.postMessage({{h: 'x', b: 1}});\n\
             window.chrome.webview.postMessage({{handler: 'iterm2-about:history', body: {{action: 'clearAllHistory'}}}});\n\
             window.chrome.webview.postMessage = function () {{ window.__saw = true; }};\n\
             setTimeout(function () {{ const f = document.createElement('iframe'); f.srcdoc = '<p>late</p>'; document.body.appendChild(f); }}, 300);\n\
             </script><iframe srcdoc=\"{nested}\"></iframe></body></html>"
        );
        let url = "https://a.tandem-probe.invalid/";
        let sources = vec![wrapped("window.addEventListener('load', function () { window.webkit.messageHandlers.x.postMessage({a: 1}); });")];
        let Some(mut p) = Probe::start(&[(url, page_html)], sources) else { return };
        p.view().watch_frames();
        p.load(url);
        // The Tandem world's message, from every frame, as the world's; the page's, as the page's.
        assert!(p.wait(15, |p| p.clients("x").len() >= 4), "the world posted from the page and its three frames: {:?}", p.clients("x"));
        for (_, body) in p.clients("x") {
            assert_eq!(body, json!({"a": 1}));
        }
        let mut keys: Vec<Option<u64>> = p.clients("x").into_iter().map(|(f, _)| f).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 4, "one per frame, each with its own key: {keys:?}");
        let page: Vec<crate::browser_js::Routed> = p.routed.iter().filter(|r| r.via == Via::Page && r.frame.is_none()).filter_map(|r| crate::browser_js::route(&r.body, crate::browser_js::World::Page, r.frame, &r.from)).collect();
        assert!(page.iter().any(|r| r.handler == "x" && r.world == crate::browser_js::World::Page && r.body == json!(1)), "{page:?}");
        let local = page.iter().find(|r| r.handler == "iterm2-about:history").expect("the local-page form arrived");
        assert!(!crate::browser_js::local_page_ok(local), "from a web source it is not a local page's");
        assert_eq!(p.page_eval("String(window.__saw === true)"), Ok("\"false\"".into()), "the page's replacement saw none of the world's traffic");
        assert_eq!(p.page_eval("typeof window.webkit"), Ok("\"undefined\"".into()));
        assert!(p.wait(10, |p| p.frames.len() >= 3), "FrameCreated for each frame, the nested one and the late one included: {:?}", p.frames);
        let expr = crate::browser_js::call_js("return a * 2;", &json!({"a": 21})).unwrap();
        assert_eq!(p.eval(None, &expr), Ok(json!(42)));
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    /// Task 6: `insert-text.js` types into a focused field and a `contenteditable`, answers false with
    /// nothing focused, and the DevTools fallback then changes nothing. Needs the console session.
    #[test]
    #[ignore]
    fn send_text_reaches_a_field() {
        let page_html = "<!doctype html><html><body><textarea id=t autofocus></textarea><div id=c contenteditable=true></div></body></html>";
        let Some(mut p) = Probe::start(&[(PAGE, page_html.to_string())], Vec::new()) else { return };
        p.load(PAGE);
        let focused = (0..50).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            p.page_eval("document.activeElement.id") == Ok("\"t\"".into())
        });
        assert!(focused, "the field has the focus");
        assert_eq!(p.page_eval(&crate::browser_keys::insert_script("hi\n")), Ok("true".into()));
        assert_eq!(p.page_eval("document.getElementById('t').value"), Ok("\"hi\\n\"".into()));
        p.page_eval("document.getElementById('c').focus()").unwrap();
        assert_eq!(p.page_eval(&crate::browser_keys::insert_script("yo")), Ok("true".into()));
        assert_eq!(p.page_eval("document.getElementById('c').textContent"), Ok("\"yo\"".into()));
        // Nothing focused and no caret: a blurred contenteditable keeps the page's selection, and
        // Chromium's `Input.insertText` types at that caret (the 2026-10-07 run: "yono"; see the rulings).
        p.page_eval("document.activeElement.blur(); window.getSelection().removeAllRanges()").unwrap();
        assert_eq!(p.page_eval("document.activeElement === document.body && window.getSelection().rangeCount === 0"), Ok("true".into()), "nothing focused, no caret");
        assert_eq!(p.page_eval(&crate::browser_keys::insert_script("no")), Ok("false".into()), "nothing focused: the script says so");
        p.call_wait(None, "Input.insertText", &json!({"text": "no"})).expect("the fallback runs");
        assert_eq!(p.page_eval("document.getElementById('t').value + document.getElementById('c').textContent"), Ok("\"hi\\nyo\"".into()), "the page is unchanged");
        p.finish();
    }

    /// Task 8 (review I1, I2, I3): a receiving view takes a replay. Its focused field gets the text once
    /// and one line break from Enter; a masked field, a password field and a focus on the host of a
    /// closed shadow root are refused by the receiving guard, so nothing is replayed there. (One
    /// probe view is the receiver; the sender's side is `browser_keys`' pure model.) Needs the
    /// console session; `Emulation.setFocusEmulationEnabled` is not used: if an unfocused target
    /// drops the input, this test shows it and the decision is revisited (m13x2-rulings.txt).
    #[test]
    #[ignore]
    fn broadcast_types_into_a_second_view() {
        let page_html = "<!doctype html><html><body><textarea id=t autofocus></textarea><input id=m type=text style=\"-webkit-text-security:disc\"><input id=p type=password><div id=h></div><script>const r=document.getElementById('h').attachShadow({mode:'closed'});const i=document.createElement('input');r.appendChild(i);window.__ci=i;</script></body></html>";
        let Some(mut p) = Probe::start(&[(PAGE, page_html.to_string())], Vec::new()) else { return };
        p.load(PAGE);
        let focused = (0..50).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            p.page_eval("document.activeElement.id") == Ok("\"t\"".into())
        });
        assert!(focused, "the field has the focus");
        let guard = crate::browser_js::iife(&crate::browser_keys::guard_body());
        assert_eq!(p.eval(None, &guard), Ok(json!(false)), "a plain textarea may receive");
        let text = crate::browser_keys::BroadcastEvent::Insert("hi".into());
        let enter = crate::browser_keys::BroadcastEvent::Key { key: "Enter".into(), code: "Enter".into(), key_code: 13, mods: 0 };
        for e in [&text, &enter] {
            for (method, params) in crate::browser_keys::replay_calls(e) {
                p.call_wait(None, method, &params).expect("the replay call runs");
            }
        }
        assert_eq!(p.page_eval("document.getElementById('t').value"), Ok("\"hi\\n\"".into()), "the text once, one line break");
        // Fields that must refuse.
        for focus in ["document.getElementById('m').focus()", "document.getElementById('p').focus()", "window.__ci.focus()"] {
            p.page_eval(focus).unwrap();
            assert_eq!(p.eval(None, &guard), Ok(json!(true)), "{focus}");
        }
        p.finish();
    }

    /// Task 7: the page's Smart Selection scripts in Tandem's world: the text around a point, the
    /// matcher's counts, and the selection `select-text.js` makes. (`browser_smart_select` is these
    /// calls in order; the App has no test constructor.) Needs the console session.
    #[test]
    #[ignore]
    fn smart_selection_selects_in_a_page() {
        let page_html = "<!doctype html><html><body style=\"margin:0\"><p id=p>write to <span id=m>bob@example.com</span> today</p></body></html>";
        let Some(mut p) = Probe::start(&[(PAGE, page_html.to_string())], Vec::new()) else { return };
        p.load(PAGE);
        let center = p.eval(None, "(() => { const r = document.getElementById('m').getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; })()").expect("the center");
        let (x, y) = (center[0].as_f64().unwrap(), center[1].as_f64().unwrap());
        let fill = |name: &str, extra: &[(&str, String)]| {
            let (xs, ys) = (crate::browser_pointer::num(x), crate::browser_pointer::num(y));
            let mut subs: Vec<(&str, &str)> = vec![("X", &xs), ("Y", &ys)];
            subs.extend(extra.iter().map(|(k, v)| (*k, v.as_str())));
            crate::browser_js::fill_template(crate::browser_js::resource(name).unwrap(), &subs)
        };
        let text = p.eval(None, &crate::browser_js::make_safe(&fill("extract-text-at-point.js", &[("RADIUS", "2".into())]))).expect("the text");
        let (before, after) = (text["before"].as_str().unwrap().to_string(), text["after"].as_str().unwrap().to_string());
        let rule = crate::smartsel::Rule { regex: r"\S+@\S+".into(), precision: 1000.0, notes: String::new(), actions: Vec::new() };
        let m = crate::browser_smart::first_match(&before, &after, &[rule], false).expect("the address matches");
        let select = fill("select-text.js", &[("BEFORE_COUNT", m.before.to_string()), ("AFTER_COUNT", m.after.to_string())]);
        p.eval(None, &crate::browser_js::make_safe(&select)).expect("the selection");
        assert_eq!(p.page_eval("window.getSelection().toString()"), Ok("\"bob@example.com\"".into()));
        p.finish();
    }

    /// Task 7 (I7): a page's synthetic press, release and auxclick run no binding; a real middle press
    /// through `Input.dispatchMouseEvent` (trusted) reaches the router once as down and once as up, and
    /// the page never sees it. Needs the console session.
    #[test]
    #[ignore]
    fn synthetic_click_runs_nothing() {
        let secret = hex32();
        let page_html = "<!doctype html><html><body style=\"margin:0\"><div id=t style=\"width:300px;height:200px\"></div><script>window.__saw = []; for (const n of ['mousedown','mouseup','auxclick']) { window.addEventListener(n, e => window.__saw.push([n, e.isTrusted]), true); }</script></body></html>";
        let code = crate::browser_pointer::user_script(&secret, &["Button,2,1,,".to_string()], true, false).code;
        let Some(mut p) = Probe::start(&[(PAGE, page_html.to_string())], vec![wrapped(&code)]) else { return };
        p.load(PAGE);
        p.page_eval("(() => { const el = document.getElementById('t'); for (const n of ['mousedown','mouseup','auxclick']) { el.dispatchEvent(new MouseEvent(n, {button: 1, bubbles: true, detail: 1})); } })()").expect("the page dispatches");
        p.settle(500);
        assert!(p.clients(crate::browser_pointer::GATE_HANDLER).is_empty(), "a synthetic press reaches nothing");
        assert_eq!(p.page_eval("window.__saw.length"), Ok("3".into()), "the page saw its own events");
        p.page_eval("window.__saw = []").unwrap();
        for kind in ["mousePressed", "mouseReleased"] {
            p.call_wait(None, "Input.dispatchMouseEvent", &json!({"type": kind, "x": 20, "y": 20, "button": "middle", "clickCount": 1})).expect("the real press");
        }
        assert!(p.wait(10, |p| p.clients(crate::browser_pointer::GATE_HANDLER).len() >= 2), "the real press reaches the router");
        let kinds: Vec<String> = p.clients(crate::browser_pointer::GATE_HANDLER).iter().map(|(_, b)| b["type"].as_str().unwrap_or("").to_string()).collect();
        assert_eq!(kinds, vec!["down".to_string(), "up".to_string()]);
        let body = &p.clients(crate::browser_pointer::GATE_HANDLER)[0].1;
        assert_eq!(crate::browser_pointer::press_of(body), Some(crate::browser_pointer::Press::Down { button: 2, clicks: 1, mods: crate::pointer::Mods::default(), x: 20.0, y: 20.0 }));
        assert_eq!(p.page_eval("window.__saw.length"), Ok("0".into()), "the page never saw the taken press");
        p.finish();
    }

    /// The Client sources of a find test: the always-present graph script (find-api talks to it) and
    /// the find script itself, wrapped as the app registers it.
    fn find_sources(secret: &str) -> Vec<String> {
        let mut v = vec![wrapped(GRAPH)];
        v.push(crate::browser_js::wrap(&crate::browser_find::user_script(secret).expect("the find script")));
        v
    }

    fn find_start(p: &mut Probe, secret: &str, term: &str) -> Result<Value, String> {
        find_command(p, secret, "startFind", json!({"searchTerm": term, "contextLength": 10, "searchMode": "caseInsensitive"}))
    }

    /// One find command as the app sends it (`COMMAND_JS` in the main frame's Tandem world).
    fn find_command(p: &mut Probe, secret: &str, action: &str, extra: Value) -> Result<Value, String> {
        let cmd = crate::browser_find::command(action, extra, "main_probe", secret);
        let expr = crate::browser_js::call_js(crate::browser_find::COMMAND_JS, &json!({"command": cmd})).expect("the command");
        p.eval(None, &expr)
    }

    /// How many navigation bubbles the main frame shows, read in the Tandem world.
    fn bubbles(p: &mut Probe) -> usize {
        p.eval(None, "document.querySelectorAll('[data-find-nav-bubble=\"true\"]').length").ok().and_then(|v| v.as_u64()).unwrap_or(0) as usize
    }

    /// Waits up to `secs` for the bubble count to satisfy `ok` (each read is an evaluation).
    fn wait_bubbles(p: &mut Probe, secs: u64, ok: impl Fn(usize) -> bool) -> bool {
        for _ in 0..secs * 4 {
            if ok(bubbles(p)) {
                return true;
            }
            p.settle(250);
        }
        ok(bubbles(p))
    }

    /// The cross-origin child of the find forgery probes: it posts what its parent tells it to as
    /// find's own window messages, so the forgeries come from a frame of another origin.
    const FORGING_CHILD: &str = "https://b.tandem-probe.invalid/child.html";
    const FORGING_CHILD_HTML: &str = r#"<!doctype html><html><head><title>child</title></head><body><p>alpha in a child</p><script>
window.addEventListener('message', function (e) {
  const d = e.data;
  if (!d || typeof d !== 'object') return;
  if (d.forge === 'click') parent.postMessage({type: 'iTermGlobalClick', clickData: {offset: 0, textContent: 'alpha', coordinates: {x: d.x, y: 0}}}, '*');
  if (d.forge === 'iframe') parent.postMessage({type: 'iTermIframeClick', frameId: 'guess', clickLocation: {coordinates: [0, 0]}, framePath: ['guess']}, '*');
});
</script></body></html>"#;

    /// 13x3 Task 1 (B2): a long page's find result reaches the router whole in its count and within
    /// the message limit, because the Client bridge cuts it. Needs the console session.
    #[test]
    #[ignore]
    fn find_result_is_capped_in_the_bridge() {
        let secret = hex32();
        let body = "<p>".to_string() + &"word ".repeat(6000) + "</p>";
        let sources = find_sources(&secret);
        let size: usize = sources.iter().map(String::len).sum();
        println!("find sources: {size} bytes (P15: one addScriptToEvaluateOnNewDocument per script)");
        let Some(mut p) = Probe::start(&[(PAGE, hostile_page(&body, "g"))], sources) else { return };
        p.graph = Some(crate::browser_frames::Frames::default());
        p.load(PAGE);
        find_start(&mut p, &secret, "word").expect("startFind runs in the Tandem world");
        assert!(p.wait(20, |p| p.clients(crate::browser_find::HANDLER).iter().any(|(_, b)| b["action"] == "resultsUpdated")), "a results message arrived: {:?}", p.log.last());
        let routed: Vec<&Routed> = p.routed.iter().filter(|r| r.via == Via::Client && r.client().is_some_and(|(h, _)| h == crate::browser_find::HANDLER)).collect();
        let big = routed.iter().find(|r| r.client().is_some_and(|(_, b)| b["action"] == "resultsUpdated")).expect("resultsUpdated");
        assert!(big.body.len() <= crate::browser_js::CAP_BUDGET, "the message is (decoded, within the escaped weight budget, so within ViewGate::admit)  {} bytes", big.body.len());
        let (_, b) = big.client().unwrap();
        assert_eq!(b["data"]["totalMatches"], 6000, "the count is kept whole");
        assert!(b["data"]["matchIdentifiers"].as_array().is_some_and(|a| !a.is_empty() && a.len() < 6000));
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    /// 13x3 Task 4: the find engine highlights across frames. A page `<p>alpha beta Alpha</p>` with an
    /// iframe `<p>alpha</p>`: "alpha" (case insensitive) reports 3 matches within 5 s and exactly one
    /// current highlight; "Alpha" (case sensitive) reports 1 and one current highlight; `clearFind`
    /// leaves no highlight. The highlight nodes are read in the page's world (they are page DOM, as
    /// in iTerm2). Needs the console session.
    #[test]
    #[ignore]
    fn find_highlights_across_frames() {
        let secret = hex32();
        let page = "<!doctype html><html><head><title>t</title></head><body><p>alpha beta Alpha</p><iframe srcdoc=\"&lt;p&gt;alpha&lt;/p&gt;\"></iframe></body></html>".to_string();
        let Some(mut p) = Probe::start(&[(PAGE, page)], find_sources(&secret)) else { return };
        p.graph = Some(crate::browser_frames::Frames::default());
        p.view().watch_frames();
        p.load(PAGE);
        assert!(p.wait(5, |p| p.graph_ids().len() == 2), "both frames registered: {:?}", p.graph_ids());
        let reports = |p: &Probe, total: u64| p.clients(crate::browser_find::HANDLER).iter().filter(|(_, b)| b["action"] == "resultsUpdated" && b["data"]["totalMatches"] == total).count();
        let count = |p: &mut Probe, sel: &str| p.page_eval(&format!("document.querySelectorAll('{sel}').length")).expect("a count");
        find_command(&mut p, &secret, "startFind", json!({"searchTerm": "alpha", "contextLength": 0, "searchMode": "caseInsensitive"})).expect("startFind");
        assert!(p.wait(5, |p| reports(p, 3) > 0), "three matches reported within 5 s: {:?}", p.clients(crate::browser_find::HANDLER));
        assert_eq!(count(&mut p, ".iterm-find-highlight-current"), "1");
        find_command(&mut p, &secret, "startFind", json!({"searchTerm": "Alpha", "contextLength": 0, "searchMode": "caseSensitive"})).expect("startFind");
        assert!(p.wait(5, |p| reports(p, 1) > 0), "one match reported");
        assert_eq!(count(&mut p, ".iterm-find-highlight-current"), "1");
        find_command(&mut p, &secret, "clearFind", json!({})).expect("clearFind");
        p.settle(500);
        assert_eq!(count(&mut p, ".iterm-find-highlight"), "0", "clearFind leaves no highlight");
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    /// 13x3 Task 5: Find URLs on a page. The temporary search for the default pattern (as
    /// `browser_find::Model::temporary` sends it) is followed by the open shortcuts: a page with two
    /// links as text holds two navigation bubbles within 5 s (`FN:192-218`, counted by their
    /// attribute); with the pattern `(?i)https?://\S+`, which the page's RegExp rejects (M4), it
    /// holds none. Needs the console session.
    #[test]
    #[ignore]
    fn find_urls_show_bubbles() {
        let secret = hex32();
        let page = "<!doctype html><html><head><title>u</title></head><body><p>see https://a.example/x and https://a.example/y now</p></body></html>".to_string();
        let default = "https?://([a-z0-9A-Z]+(:[a-zA-Z0-9]+)?@)?[a-z0-9A-Z\\-]+(\\.[a-z0-9A-Z\\-]+)*((:[0-9]+)?)(/[a-zA-Z0-9;:/\\.\\-_+%~?&amp;@=#\\(\\)]*)?";
        let Some(mut p) = Probe::start(&[(PAGE, page)], find_sources(&secret)) else { return };
        p.graph = Some(crate::browser_frames::Frames::default());
        p.load(PAGE);
        // The model's own commands, in order, each answered before the next.
        let mut m = crate::browser_find::Model::new("probe", "probeglobal");
        // Each run has its own instance id, so one run's reports are never read as the next's.
        let run = |p: &mut Probe, pattern: &str, uuid: &str, m: &mut crate::browser_find::Model| {
            let want = format!("main_{uuid}");
            let cmds = m.temporary(pattern, &secret).expect("a pattern");
            for c in cmds {
                let expr = crate::browser_js::call_js(crate::browser_find::COMMAND_JS, &json!({"command": c})).expect("the command");
                p.eval(None, &expr).expect("the command runs");
            }
            assert!(p.wait(10, |p| p.clients(crate::browser_find::HANDLER).iter().any(|(_, b)| b["action"] == "resultsUpdated" && b["data"]["instanceId"] == want.as_str())), "the full result arrived");
            for (_, b) in p.clients(crate::browser_find::HANDLER) {
                m.on_message(&b, &secret);
            }
            m.shortcuts_after_complete(&secret)
        };
        let after = run(&mut p, default, "probe", &mut m).expect("something matched, so the shortcuts are owed");
        for c in after {
            let expr = crate::browser_js::call_js(crate::browser_find::COMMAND_JS, &json!({"command": c})).expect("the command");
            p.eval(None, &expr).expect("the shortcuts start");
        }
        assert!(wait_bubbles(&mut p, 5, |n| n == 2), "two navigation bubbles within 5 s: {}", bubbles(&mut p));
        find_command(&mut p, &secret, "clearNavigationShortcuts", json!({})).expect("clear");
        find_command(&mut p, &secret, "clearFind", json!({})).expect("clearFind");
        p.settle(500);
        // The inline flag: valid for 13s's validator, a SyntaxError for the page's RegExp, so literal text.
        let mut m = crate::browser_find::Model::new("probe2", "probe2global");
        let owed = run(&mut p, "(?i)https?://\\S+", "probe2", &mut m);
        assert!(owed.is_none(), "nothing matched as literal text, so no shortcuts are owed");
        p.settle(1000);
        assert_eq!(bubbles(&mut p), 0, "no bubbles for a pattern the page cannot run");
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    /// 13x3 Task 1 review I3: Find Globally's context (40) on a CJK and accented page with
    /// thousands of matches. The cap weighs the escaped DevTools event, so the result is admitted
    /// (a UTF-8 weight let it reach about 100 KB raw and the gate dropped it whole). Needs the
    /// console session.
    #[test]
    #[ignore]
    fn find_result_is_capped_for_wide_text() {
        let secret = hex32();
        // Entities, so the page's bytes are ASCII whatever charset the response is read with.
        let body = "<p>".to_string() + &"&#x691c;&#x7d22; &#xe9;&#x301;&#x8a9e;&#x1f600; ".repeat(3000) + "</p>";
        let page = format!("<!doctype html><html><head><title>wide</title></head><body>{body}</body></html>");
        let Some(mut p) = Probe::start(&[(PAGE, page)], find_sources(&secret)) else { return };
        p.graph = Some(crate::browser_frames::Frames::default());
        p.load(PAGE);
        find_command(&mut p, &secret, "startFind", json!({"searchTerm": "\u{691c}\u{7d22}", "contextLength": 40, "searchMode": "caseSensitive"})).expect("startFind runs in the Tandem world");
        assert!(p.wait(20, |p| p.clients(crate::browser_find::HANDLER).iter().any(|(_, b)| b["action"] == "resultsUpdated" && b["data"]["totalMatches"] == 3000)), "the full result arrived, its count whole");
        let (_, b) = p.clients(crate::browser_find::HANDLER).into_iter().find(|(_, b)| b["action"] == "resultsUpdated" && b["data"]["totalMatches"] == 3000).unwrap();
        let ids = b["data"]["matchIdentifiers"].as_array().map_or(0, Vec::len);
        let ctx = b["data"]["contexts"].as_array().map_or(0, Vec::len);
        assert!(ids > 0 && ids < 3000 && ctx == ids, "a prefix of identifiers and contexts: {ids} and {ctx}");
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    /// 13x3 Task 1 (B7, m13x2 B9 for the find scripts; made able to fail after the Task 1 review):
    /// window messages a page forges or reads. The page and a cross-origin child forge
    /// `iTermGlobalClick` and `iTermIframeClick`; neither carries the view secret. With new windows
    /// on (as a browser pane has them) and navigation shortcuts on, a synthetic key at a bubble's
    /// label reaches find-nav (the bubbles go) and opens nothing, because the host lets only a
    /// user-initiated window through; a real key, the control, does open one. Needs the console session.
    #[test]
    #[ignore]
    fn find_messages_forged_by_a_page() {
        let secret = hex32();
        let body = format!("<p>https://x.invalid/one alpha</p><iframe src=\"{FORGING_CHILD}\"></iframe>");
        let Some(mut p) = Probe::start_with(&[(PAGE, hostile_page(&body, "g")), (FORGING_CHILD, FORGING_CHILD_HTML.to_string())], find_sources(&secret), true) else { return };
        p.graph = Some(crate::browser_frames::Frames::default());
        p.load(PAGE);
        assert!(p.wait(10, |p| p.contexts.frames().len() == 1), "the child has a Tandem world");
        find_start(&mut p, &secret, "https://x.invalid").expect("startFind runs in the Tandem world");
        assert!(p.wait(20, |p| p.clients(crate::browser_find::HANDLER).iter().any(|(_, b)| b["action"] == "resultsUpdated")), "a results message arrived");
        let before = p.clients(crate::browser_find::HANDLER).len();
        // Forged window messages, from the page itself and from the cross-origin child.
        p.page_eval("window.postMessage({type: 'iTermGlobalClick', clickData: {offset: 1, textContent: 'alpha', coordinates: {x: 1, y: 1}}}, '*'); window.postMessage({type: 'iTermIframeClick', frameId: 'guess', clickLocation: {coordinates: [0, 0]}, framePath: ['guess']}, '*'); frames[0].postMessage({forge: 'click', x: 1}, '*'); frames[0].postMessage({forge: 'iframe'}, '*'); 1").expect("the forgery runs");
        p.settle(500);
        // Navigation shortcuts on, as the person's command turns them on.
        find_command(&mut p, &secret, "startNavigationShortcuts", json!({"selectionAction": "open"})).expect("the shortcuts start");
        assert!(wait_bubbles(&mut p, 10, |n| n > 0), "the bubbles show");
        let label = p.eval(None, "document.querySelector('[data-find-nav-bubble=\"true\"]').textContent").expect("a bubble's label");
        let label = label.as_str().expect("a label").to_lowercase();
        // A synthetic key at the label: find-nav ignores it (R13x3-2), so the bubbles stay.
        let keys = serde_json::to_string(&label.chars().map(String::from).collect::<Vec<_>>()).unwrap();
        p.page_eval(&format!("for (const k of {keys}) {{ document.dispatchEvent(new KeyboardEvent('keydown', {{key: k, bubbles: true}})); }} 1")).expect("the synthetic keys run");
        p.settle(1500);
        assert!(bubbles(&mut p) > 0, "the synthetic key picked a bubble (find-nav took an untrusted key)");
        // 1. The page recorded every message it could see: none holds the secret or its field name.
        let report = p.page_eval("window.__report()").expect("the page report");
        assert!(!report.contains(&secret) && !report.contains("sessionSecret"), "the view secret reached the page world");
        // 2. The pick asked for a window, and the host let none through: no NewWindow.
        assert!(p.new_windows.is_empty(), "a page-initiated window got through: {:?}", p.new_windows);
        let after: Vec<String> = p.clients(crate::browser_find::HANDLER).iter().skip(before).filter_map(|(_, b)| b["action"].as_str().map(str::to_string)).collect();
        assert!(after.iter().all(|a| a == "currentChanged" || a == "resultsUpdated"), "only find's own reports followed: {after:?}");
        // 3. Control: the same pick by a real key is user-initiated, and that window does reach the host.
        find_command(&mut p, &secret, "startNavigationShortcuts", json!({"selectionAction": "open"})).expect("the shortcuts start again");
        assert!(wait_bubbles(&mut p, 10, |n| n > 0), "the bubbles show again");
        let label = p.eval(None, "document.querySelector('[data-find-nav-bubble=\"true\"]').textContent").expect("a bubble's label");
        let label = label.as_str().expect("a label").to_lowercase();
        p.view().focus();
        for c in label.chars() {
            let (key, upper) = (c.to_string(), c.to_ascii_uppercase());
            for kind in ["keyDown", "keyUp"] {
                let code = if c.is_ascii_digit() { format!("Digit{c}") } else { format!("Key{upper}") };
                let mut k = json!({"type": kind, "key": key, "code": code, "windowsVirtualKeyCode": upper as u32});
                if kind == "keyDown" {
                    k["text"] = json!(key);
                }
                p.call_wait(None, "Input.dispatchKeyEvent", &k).expect("the real key");
            }
        }
        assert!(p.wait(10, |p| !p.new_windows.is_empty()), "control: a real key's pick opens a window through the host");
        // 4. The engine still answers a real command after the forgeries.
        find_command(&mut p, &secret, "findNext", json!({})).expect("findNext still runs");
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    /// 13x3 Task 1 review C1 (R13x3-1): a cross-origin frame forges `iTermGlobalClick` with
    /// coordinates that are script text. The main frame's find script stores it and, at the next
    /// find, builds the script it has evaluated in that frame's Tandem world: the text must never
    /// run there (no marker, no Client message, never in an evaluation). Control: a numeric forgery
    /// does reach the frame as `handleClickInFrame(5, 0, ...)`. Needs the console session.
    #[test]
    #[ignore]
    fn find_forged_click_cannot_reach_the_client() {
        const PAYLOAD: &str = "0) || (globalThis.__pwned = 1, globalThis.__tandemClient && globalThis.__tandemClient(JSON.stringify({h: 'probePwned', b: {}})), 0) || (0";
        let secret = hex32();
        let body = format!("<p>alpha beta alpha</p><iframe src=\"{FORGING_CHILD}\"></iframe>");
        let Some(mut p) = Probe::start(&[(PAGE, hostile_page(&body, "g")), (FORGING_CHILD, FORGING_CHILD_HTML.to_string())], find_sources(&secret)) else { return };
        p.graph = Some(crate::browser_frames::Frames::default());
        p.load(PAGE);
        assert!(p.wait(10, |p| p.contexts.frames().len() == 1), "the child has a Tandem world");
        let k = p.contexts.frames()[0];
        p.page_eval(&format!("frames[0].postMessage({{forge: 'click', x: {}}}, '*'); 1", crate::browser_js::js_string(PAYLOAD))).expect("the forgery runs");
        p.settle(500);
        find_start(&mut p, &secret, "alpha").expect("startFind runs in the Tandem world");
        assert!(p.wait(20, |p| p.clients(crate::browser_find::HANDLER).iter().any(|(_, b)| b["action"] == "resultsUpdated")), "a results message arrived");
        p.settle(1500);
        assert!(p.clients("probePwned").is_empty(), "the forged text posted a Client message");
        assert!(p.log.iter().filter(|l| l.starts_with("Runtime.evaluate")).all(|l| !l.contains("__pwned")), "the forged text was evaluated");
        assert_eq!(p.eval(Some(k), "typeof globalThis.__pwned"), Ok(json!("undefined")), "the child's Tandem world has no marker");
        assert_eq!(p.eval(None, "typeof globalThis.__pwned"), Ok(json!("undefined")), "the main Tandem world has no marker");
        // Control: a numeric forgery is the path that is live, so the assertions above mean something.
        p.page_eval("frames[0].postMessage({forge: 'click', x: 5}, '*'); 1").expect("the numeric forgery runs");
        p.settle(500);
        find_start(&mut p, &secret, "beta").expect("startFind runs again");
        assert!(p.wait(20, |p| p.log.iter().any(|l| l.starts_with("Runtime.evaluate") && l.contains("handleClickInFrame(5, 0, "))), "control: a numeric click reaches the child's world");
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    // --- 13x3 Task 9: copy mode on a real page ---

    fn copy_sources(secret: &str) -> Vec<String> {
        let mut v = vec![wrapped(GRAPH)];
        v.push(crate::browser_js::wrap(&crate::browser_copymode::user_script(secret).expect("the copy mode script")));
        v.push(crate::browser_js::wrap(&crate::browser_copymode::keys_script(secret).expect("the copy mode keys script")));
        v
    }

    /// What `browser_copy_mode_set(pane, true)` does: the page's cursor on, then the key switch in
    /// every live Tandem world (the main frame and each iframe's).
    fn copy_on(p: &mut Probe, secret: &str) {
        use crate::browser_copymode as cm;
        p.eval(None, &cm::enable_js(secret)).expect("enable runs in the Tandem world");
        copy_keys_switch(p, secret, true);
    }

    /// What `browser_world_update(pane, keys_on_js)` does: every live Tandem world gets the switch.
    fn copy_keys_switch(p: &mut Probe, secret: &str, on: bool) {
        let js = crate::browser_copymode::keys_on_js(secret, on);
        p.eval(None, &js).expect("the switch in the main frame");
        for f in p.contexts.frames() {
            p.eval(Some(f), &js).expect("the switch in a frame");
        }
    }

    /// What `browser_copy_mode_set(pane, false)` does.
    fn copy_off(p: &mut Probe, secret: &str) {
        p.eval(None, &crate::browser_copymode::disable_js(secret)).expect("disable runs");
        copy_keys_switch(p, secret, false);
    }

    /// One real key from the browser (`Input.dispatchKeyEvent` is trusted).
    fn real_key(p: &mut Probe, c: char) {
        let (key, upper) = (c.to_string(), c.to_ascii_uppercase());
        for kind in ["keyDown", "keyUp"] {
            let mut k = json!({"type": kind, "key": key, "code": format!("Key{upper}"), "windowsVirtualKeyCode": upper as u32});
            if kind == "keyDown" {
                k["text"] = json!(key);
            }
            p.call_wait(None, "Input.dispatchKeyEvent", &k).expect("the real key");
        }
    }

    /// A real key through the world's listener and the model, as `browser_copy_mode_key` and
    /// `browser_copy_mode_apply` take it. The page-side steps run in the main frame's Tandem world;
    /// Copy and Leave are returned for the test to finish, as the app finishes them natively.
    fn copy_press(p: &mut Probe, m: &mut crate::browser_copymode::Model, secret: &str, c: char) -> crate::browser_copymode::Step {
        use crate::browser_copymode as cm;
        let n = p.clients(cm::KEY_HANDLER).len();
        real_key(p, c);
        assert!(p.wait(5, |p| p.clients(cm::KEY_HANDLER).len() > n), "key {c} reached the world's listener");
        let (_, body) = p.clients(cm::KEY_HANDLER).pop().unwrap();
        let (key, ctrl, alt) = cm::key_of(&body).expect("a typed key body");
        let name = cm::key_name(&key, ctrl, alt).expect("a named key");
        let step = m.key(&name, secret);
        if let cm::Step::Script(js) = &step {
            p.eval(None, js).expect("the step runs in the Tandem world");
        }
        step
    }

    fn cursor_display(p: &mut Probe) -> String {
        p.eval(None, "(document.getElementById('iterm-copy-mode-cursor') || {style: {display: 'absent'}}).style.display").ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
    }

    fn selection(p: &mut Probe) -> String {
        p.eval(None, "window.getSelection().toString()").ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
    }

    /// 13x3 Task 9, Review Focus 8: copy mode follows iTerm2 on a page `<p>hello world</p>`. Enabling
    /// shows the cursor (`CMC:37-48`); "v", "w", "w" select from "hello"; "y" is the Copy step (the
    /// text the app's one reader, `browser_copy_selection`, puts on the clipboard) and leaving hides
    /// the cursor; a second enable and "q" hides the cursor and keeps the selection (`CMI:75-88`).
    /// The probe does not write the person's clipboard: the write is `set_clipboard_text`, pinned by
    /// the app's source tests. Needs the console session.
    #[test]
    #[ignore]
    fn browser_copy_mode_follows_iterm2() {
        use crate::browser_copymode as cm;
        let secret = hex32();
        let page = "<!doctype html><html><head><title>c</title></head><body><p>hello world</p></body></html>".to_string();
        let Some(mut p) = Probe::start(&[(PAGE, page)], copy_sources(&secret)) else { return };
        p.load(PAGE);
        p.view().focus();
        assert_eq!(cursor_display(&mut p), "absent", "no cursor before copy mode");
        copy_on(&mut p, &secret);
        assert_ne!(cursor_display(&mut p), "none", "the cursor is displayed");
        let mut m = cm::Model::default();
        for c in ['v', 'w', 'w'] {
            copy_press(&mut p, &mut m, &secret, c);
        }
        let sel = selection(&mut p);
        assert!(sel.starts_with("hello") && !sel.is_empty(), "the selection starts at hello: {sel:?}");
        assert!(m.selecting);
        // y: the Copy step, reading what the app's reader reads, capped as it caps.
        assert_eq!(copy_press(&mut p, &mut m, &secret, 'y'), cm::Step::Copy);
        let copied = crate::browser_js::store_selection(&selection(&mut p));
        assert!(copied.starts_with("hello"), "what the clipboard would hold: {copied:?}");
        copy_off(&mut p, &secret);
        assert_eq!(cursor_display(&mut p), "none", "the cursor is hidden after the copy");
        // Second enable, "q": the cursor hides, the selection stays, selecting stops.
        let keep = selection(&mut p);
        copy_on(&mut p, &secret);
        assert_ne!(cursor_display(&mut p), "none");
        let mut m = cm::Model::default();
        let step = copy_press(&mut p, &mut m, &secret, 'q');
        assert_eq!(step, cm::Step::Leave { stop_selecting: true });
        p.eval(None, cm::STOP_SELECTING_JS).expect("stop selecting");
        copy_off(&mut p, &secret);
        assert_eq!(cursor_display(&mut p), "none", "q hides the cursor");
        assert_eq!(selection(&mut p), keep, "q keeps the selection");
        // With the switch off a real key reaches the page, not copy mode.
        let n = p.clients(cm::KEY_HANDLER).len();
        real_key(&mut p, 'j');
        p.settle(500);
        assert_eq!(p.clients(cm::KEY_HANDLER).len(), n, "the listener is silent while copy mode is off");
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    /// 13x3 Task 9 (R13x23-3, C1): a page cannot hijack copy mode. Its first script assigns
    /// `window.iTerm2CopyMode` with a recorder for every method and a recording `selecting`, and
    /// tries to flip every global that looks like a key switch; copy mode still drives the real
    /// cursor, the recorder is never called, the page's recorders never hold the secret, and the
    /// page's `window.iTerm2CopyMode` is its own. The same page with no copy mode script (a view
    /// whose isolated world is unavailable gets none, `browser_user_scripts`): the recorder is never
    /// called either. Needs the console session.
    #[test]
    #[ignore]
    fn page_cannot_hijack_copy_mode() {
        use crate::browser_copymode as cm;
        const FIRST: &str = r#"<script>
(function () {
  window.__leaks = [];
  const leak = function () { window.__leaks.push(Array.prototype.slice.call(arguments).join('|')); };
  const names = ['enable', 'disable', 'moveBackwardWord', 'moveForwardWord', 'moveBackwardBigWord', 'moveForwardBigWord', 'moveLeft', 'moveRight', 'moveUp', 'moveDown', 'moveToStartOfNextLine', 'pageUp', 'pageDown', 'pageUpHalfScreen', 'pageDownHalfScreen', 'previousMark', 'nextMark', 'moveToStart', 'moveToEnd', 'moveToStartOfIndentation', 'moveToBottomOfVisibleArea', 'moveToMiddleOfVisibleArea', 'moveToTopOfVisibleArea', 'moveToStartOfLine', 'moveToEndOfLine', 'swap', 'scrollUp', 'scrollDown', 'scrollCursorIntoView', 'copySelection', 'getState'];
  const mine = {};
  for (const n of names) mine[n] = function (s) { leak(n, s); };
  Object.defineProperty(mine, 'selecting', { get: function () { return false; }, set: function (v) { leak('selecting', v); } });
  Object.defineProperty(mine, 'mode', { get: function () { return 0; }, set: function (v) { leak('mode', v); } });
  window.iTerm2CopyMode = mine;
  window.__mine = mine;
  // Every global that looks like a key switch: replaced by a recorder, in the page's world.
  for (const k of ['__tandemCopyModeKeys', 'tandemCopyModeKeys', 'copyModeKeys', 'iTerm2CopyModeKeys']) {
    window[k] = function () { leak(k, Array.prototype.slice.call(arguments).join(',')); };
  }
  for (const k of Object.getOwnPropertyNames(window)) {
    if (/key|copy|switch/i.test(k) && typeof window[k] === 'function' && k !== 'leak') {
      try { const f = window[k]; window[k] = function () { leak(k, 'called'); return f.apply(this, arguments); }; } catch (e) {}
    }
  }
})();
</script>"#;
        let page = format!("<!doctype html><html><head><title>h</title>{FIRST}{}</head><body><p>one two</p><p>three four</p></body></html>", HOSTILE.replace("{{GUESS}}", "g"));
        // With the isolated world: the real cursor moves and copies.
        {
            let secret = hex32();
            let Some(mut p) = Probe::start(&[(PAGE, page.clone())], copy_sources(&secret)) else { return };
            p.load(PAGE);
            p.view().focus();
            copy_on(&mut p, &secret);
            let top = |p: &mut Probe| p.eval(None, "parseFloat(document.getElementById('iterm-copy-mode-cursor').style.top) || 0").ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
            let before = top(&mut p);
            let mut m = cm::Model::default();
            copy_press(&mut p, &mut m, &secret, 'j');
            assert!(top(&mut p) > before, "j moved the real cursor down: {before} -> {}", top(&mut p));
            copy_press(&mut p, &mut m, &secret, 'v');
            copy_press(&mut p, &mut m, &secret, 'l');
            copy_press(&mut p, &mut m, &secret, 'l');
            assert_eq!(copy_press(&mut p, &mut m, &secret, 'y'), cm::Step::Copy);
            assert!(!selection(&mut p).is_empty(), "y has a selection to copy");
            assert_eq!(p.page_eval("window.__leaks.length").as_deref(), Ok("0"), "the page's recorder was never called: {:?}", p.page_eval("window.__leaks.join(';')"));
            assert_eq!(p.page_eval("String(window.iTerm2CopyMode === window.__mine)").as_deref(), Ok("\"true\""), "the page world holds the page's own object");
            assert_eq!(p.eval(None, "String(window.iTerm2CopyMode === window.__mine)"), Ok(json!("false")), "the Tandem world holds Tandem's");
            assert_eq!(p.eval(None, "typeof window.iTerm2CopyMode.getState"), Ok(json!("function")));
            let report = p.page_eval("window.__report()").expect("the page report");
            assert!(!report.contains(&secret) && !report.contains("sessionSecret"), "the view secret reached the page world");
            assert_eq!(p.rejected_bindings, 0);
            p.finish();
        }
        // No isolated world, so no copy mode script (and the menu item is off): nothing leaks.
        {
            let secret = hex32();
            let Some(mut p) = Probe::start(&[(PAGE, page)], vec![wrapped(GRAPH)]) else { return };
            p.load(PAGE);
            p.view().focus();
            assert_eq!(p.eval(None, "typeof window.iTerm2CopyMode"), Ok(json!("undefined")), "no copy mode script in the Tandem world");
            let _ = &secret;
            real_key(&mut p, 'j');
            p.settle(500);
            assert!(p.clients(cm::KEY_HANDLER).is_empty(), "no key listener, so nothing is reported");
            assert_eq!(p.page_eval("window.__leaks.length").as_deref(), Ok("0"), "the recorder was never called");
            p.finish();
        }
    }

    /// 13x3 Task 9 (I4): keys in iframes and untrusted ones. With focus inside a `srcdoc` iframe and
    /// copy mode on, a trusted "j" is reported by the iframe's world (a frame message) and the
    /// iframe page's own `keydown` listener sees nothing; a page's synthetic "y" in either frame
    /// changes nothing; an iframe added after copy mode started captures keys too once the new
    /// context is told (`browser_copy_mode_new_context`). Needs the console session.
    #[test]
    #[ignore]
    fn copy_mode_keys_in_iframes_and_untrusted() {
        use crate::browser_copymode as cm;
        let secret = hex32();
        const SRC: &str = "&lt;body tabindex=&quot;0&quot;&gt;&lt;p&gt;inside frame&lt;/p&gt;&lt;script&gt;window.__seen=0;document.addEventListener('keydown',function(){window.__seen++});window.addEventListener('keydown',function(){window.__seen++});&lt;/script&gt;&lt;/body&gt;";
        let page = format!("<!doctype html><html><head><title>i</title></head><body><p>outer text</p><iframe id=\"f1\" srcdoc=\"{SRC}\"></iframe></body></html>");
        let Some(mut p) = Probe::start(&[(PAGE, page)], copy_sources(&secret)) else { return };
        p.load(PAGE);
        assert!(p.wait(10, |p| p.contexts.frames().len() == 1), "the iframe has a Tandem world");
        p.view().focus();
        copy_on(&mut p, &secret);
        p.page_eval("document.getElementById('f1').contentWindow.focus(); document.getElementById('f1').contentDocument.body.focus(); 1").expect("focus moves into the iframe");
        // A trusted j with focus in the iframe: reported from the frame, the frame's page sees nothing.
        let n = p.clients(cm::KEY_HANDLER).len();
        real_key(&mut p, 'j');
        assert!(p.wait(5, |p| p.clients(cm::KEY_HANDLER).len() > n), "the iframe's world reported the key");
        let (frame, body) = p.clients(cm::KEY_HANDLER).pop().unwrap();
        assert!(frame.is_some(), "it came from the iframe's world");
        assert_eq!(cm::key_of(&body).map(|k| k.0), Some("j".to_string()));
        assert_eq!(p.page_eval("document.getElementById('f1').contentWindow.__seen").as_deref(), Ok("0"), "the iframe page's own listeners saw no key");
        // A page's synthetic y, in the iframe and in the top frame: nothing is reported, copy mode stays on.
        let n = p.clients(cm::KEY_HANDLER).len();
        p.page_eval("const e = () => new KeyboardEvent('keydown', {key: 'y', bubbles: true}); document.dispatchEvent(e()); document.getElementById('f1').contentDocument.dispatchEvent(e()); document.getElementById('f1').contentWindow.dispatchEvent(e()); window.dispatchEvent(e()); 1").expect("the synthetic keys run");
        p.settle(500);
        assert_eq!(p.clients(cm::KEY_HANDLER).len(), n, "synthetic keys are not reported");
        assert_ne!(cursor_display(&mut p), "none", "copy mode is still on");
        assert_eq!(selection(&mut p), "", "nothing was copied or selected");
        // An iframe added after copy mode started: its world takes keys once it is told.
        p.page_eval("const f = document.createElement('iframe'); f.id = 'f2'; f.srcdoc = '<body tabindex=0><p>late</p></body>'; document.body.appendChild(f); 1").expect("an iframe is added");
        assert!(p.wait(10, |p| p.contexts.frames().len() == 2), "the new iframe has a Tandem world");
        let js = cm::keys_on_js(&secret, true);
        for f in p.contexts.frames() {
            p.eval(Some(f), &js).expect("the new context is told");
        }
        p.page_eval("document.getElementById('f2').contentWindow.focus(); document.getElementById('f2').contentDocument.body.focus(); 1").expect("focus moves into the new iframe");
        let n = p.clients(cm::KEY_HANDLER).len();
        real_key(&mut p, 'j');
        assert!(p.wait(5, |p| p.clients(cm::KEY_HANDLER).len() > n), "the late iframe reported the key");
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }
}
