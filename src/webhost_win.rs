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
    /// The browser pane's permission sync gate (`perm_queue::ProfileQueue::gate`): while it reads
    /// false the main-frame NavigationStarting handler cancels and parks every navigation
    /// (`webhost::nav_parked`). None: always open.
    pub nav_gate: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
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
    /// The app decides this view's permissions (browser panes): a request Tandem asks about is
    /// posted as `Event::Permission` and held until `answer_permission`; a page's notification
    /// is `Event::NotificationShown`; the audio state is `Event::AudioPlaying`. Off: every request
    /// is denied on the spot (tool views, popovers, Quick Look, the icon rasterizer).
    pub permissions: bool,
    /// The view reports the HTTP method of its main document requests (`Event::DocumentMethod`,
    /// 13x5): a second `WebResourceRequested` handler on a Document-context filter, no deferral.
    pub track_document_method: bool,
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

/// A permission request held back until the app answers (`answer_permission`).
struct PendingPerm {
    deferral: ICoreWebView2Deferral,
    args: ICoreWebView2PermissionRequestedEventArgs,
}

/// A page's notification the app may still report on (`notification_report`).
struct HeldNote {
    note: ICoreWebView2Notification,
    close_token: i64,
    /// The page's close was posted once; later ones are not.
    closed_posted: bool,
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
    /// The (capped) URI and id of the last main-frame NavigationStarting, for the Document filter.
    last_main_nav: Option<(String, u64)>,
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
    /// Permission requests waiting for an answer, by token (at most `MAX_PENDING_PERMISSIONS`).
    perms: HashMap<u64, PendingPerm>,
    perm_limit: webhost::PermissionLimiter,
    /// Page notifications the app holds a token for (at most `MAX_HELD_NOTES`).
    notes: HashMap<u64, HeldNote>,
    note_limit: webhost::NotificationLimiter,
    /// The registrations `shut` removes.
    note_watch: Option<(ICoreWebView2_24, i64)>,
    audio_watch: Option<(ICoreWebView2_8, i64)>,
    /// What `set_muted` last asked for (the engine answers once the view exists).
    muted: bool,
}

/// What the app tells the view about a notification it holds (`notification_report`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteReport {
    /// Tandem showed it: `ReportShown`, the object stays held.
    Shown,
    /// It was closed (by the person, or the app took it away): `ReportClosed`, then released.
    Closed,
    /// Tandem will not show it: released without a report.
    Release,
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
            last_main_nav: None,
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
            perms: HashMap::new(),
            perm_limit: webhost::PermissionLimiter::default(),
            notes: HashMap::new(),
            note_limit: webhost::NotificationLimiter::default(),
            note_watch: None,
            audio_watch: None,
            muted: false,
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

    /// Completes the deferral taken when `Event::Permission { token, .. }` was posted. WebView2
    /// never stores the answer itself (`SavesInProfile` false) and this never writes the profile:
    /// a saved grant was mirrored by the app's write queue before it answered (`perm_queue`), so
    /// the page works at once (a `new Notification` right after the Allow). Only `STATE_ALLOW`
    /// allows. An unknown token (already answered, or the view dropped it) does nothing.
    pub fn answer_permission(&self, token: u64, answer: webhost::PermAnswer) {
        let pending = self.inner.borrow_mut().perms.remove(&token);
        if let Some(p) = pending {
            complete_permission(p, answer);
        }
    }

    /// Tells the view what happened to the notification held under `token` (see `NoteReport`).
    pub fn notification_report(&self, token: u64, r: NoteReport) {
        if r == NoteReport::Shown {
            if let Some(h) = self.inner.borrow().notes.get(&token) {
                let _ = unsafe { h.note.ReportShown() };
            }
            return;
        }
        let held = self.inner.borrow_mut().notes.remove(&token);
        let Some(h) = held else { return };
        let _ = unsafe { h.note.remove_CloseRequested(h.close_token) };
        if r == NoteReport::Closed {
            let _ = unsafe { h.note.ReportClosed() };
        }
    }

    /// Mutes or unmutes the whole view (`IsMuted`). The page keeps playing: `AudioPlaying` stays
    /// true while muted.
    pub fn set_muted(&self, on: bool) {
        self.inner.borrow_mut().muted = on;
        self.with(move |c| {
            if let Ok(v8) = c.view.cast::<ICoreWebView2_8>() {
                let _ = unsafe { v8.SetIsMuted(on) };
            }
        });
    }

    /// Whether the view is muted: the engine's answer once the view exists, else what was asked.
    pub fn is_muted(&self) -> bool {
        let (core, asked) = {
            let i = self.inner.borrow();
            (i.core.clone(), i.muted)
        };
        match core.and_then(|c| c.view.cast::<ICoreWebView2_8>().ok()) {
            Some(v8) => bool_out(|b| unsafe { v8.IsMuted(b) }),
            None => asked,
        }
    }

    /// Sets one profile permission state (`SetPermissionState`); `Event::PermissionWritten` with
    /// `seq` answers once, `ok` false when the runtime cannot set the profile or the call failed.
    /// The app's write queue (`perm_queue`) sends one at a time, so writes land in the order they
    /// were asked. A failure is logged with the kind and state (numbers), never the origin.
    pub fn set_permission_state(&self, seq: u64, kind: i32, origin: &str, state: i32) {
        let (poster, id) = {
            let i = self.inner.borrow();
            (i.poster.clone(), i.id)
        };
        let w = wide(origin);
        self.with(move |c| {
            let failed = |poster: &Poster| poster.post(Event::PermissionWritten { view: id, seq, ok: false });
            let Some(p4) = profile4(&c.view) else {
                crate::dlog!("webhost: this runtime has no profile permission states");
                return failed(&poster);
            };
            let p2 = poster.clone();
            let handler = webview2_com::SetPermissionStateCompletedHandler::create(Box::new(move |hr| guard(|| {
                if hr.is_err() {
                    crate::dlog!("webhost: setting permission kind {} to state {} failed", kind, state);
                }
                p2.post(Event::PermissionWritten { view: id, seq, ok: hr.is_ok() });
                Ok(())
            })));
            if unsafe { p4.SetPermissionState(COREWEBVIEW2_PERMISSION_KIND(kind), PCWSTR(w.as_ptr()), COREWEBVIEW2_PERMISSION_STATE(state), &handler) }.is_err() {
                crate::dlog!("webhost: setting permission kind {} to state {} failed", kind, state);
                failed(&poster);
            }
        });
    }

    /// Reads the profile's non-default permission states; `Event::PermissionStates` answers (at
    /// most `MAX_PERMISSION_STATES`). Always posts: `complete` is false when the runtime cannot read
    /// the profile, the read failed, an item could not be read or the list was cut.
    pub fn read_permission_states(&self) {
        let (poster, id) = {
            let i = self.inner.borrow();
            (i.poster.clone(), i.id)
        };
        self.with(move |c| {
            let incomplete = |poster: &Poster| poster.post(Event::PermissionStates { view: id, states: Vec::new(), complete: false });
            let Some(p4) = profile4(&c.view) else {
                crate::dlog!("webhost: this runtime has no profile permission states");
                return incomplete(&poster);
            };
            let p2 = poster.clone();
            let handler = webview2_com::GetNonDefaultPermissionSettingsCompletedHandler::create(Box::new(move |hr, list| guard(|| {
                if hr.is_err() {
                    crate::dlog!("webhost: reading the profile permission states failed");
                    p2.post(Event::PermissionStates { view: id, states: Vec::new(), complete: false });
                    return Ok(());
                }
                let mut states = Vec::new();
                let mut complete = true;
                if let Some(list) = list {
                    let mut n = 0u32;
                    complete &= unsafe { list.Count(&mut n) }.is_ok();
                    complete &= n as usize <= webhost::MAX_PERMISSION_STATES;
                    for k in 0..n.min(webhost::MAX_PERMISSION_STATES as u32) {
                        let Ok(item) = (unsafe { list.GetValueAtIndex(k) }) else {
                            complete = false;
                            continue;
                        };
                        let mut kind = COREWEBVIEW2_PERMISSION_KIND(0);
                        let mut state = COREWEBVIEW2_PERMISSION_STATE(0);
                        let origin = pwstr_out(|p| unsafe { item.PermissionOrigin(p) });
                        if origin.is_some() && unsafe { item.PermissionKind(&mut kind) }.is_ok() && unsafe { item.PermissionState(&mut state) }.is_ok() {
                            let (url, cut) = webhost::cap_url(&origin.unwrap_or_default());
                            if cut {
                                // A cut origin matches no key: skipped, and the read says it is partial.
                                complete = false;
                            } else {
                                states.push((kind.0, url, state.0));
                            }
                        } else {
                            complete = false;
                        }
                    }
                } else {
                    complete = false;
                }
                p2.post(Event::PermissionStates { view: id, states, complete });
                Ok(())
            })));
            if unsafe { p4.GetNonDefaultPermissionSettings(&handler) }.is_err() {
                crate::dlog!("webhost: reading the profile permission states failed");
                incomplete(&poster);
            }
        });
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

/// The view's profile as `ICoreWebView2Profile4` (None on an older runtime).
fn profile4(view: &ICoreWebView2) -> Option<ICoreWebView2Profile4> {
    let v13 = view.cast::<ICoreWebView2_13>().ok()?;
    unsafe { v13.Profile() }.ok()?.cast::<ICoreWebView2Profile4>().ok()
}

/// Sets the answer on a held request and completes its deferral. Only `STATE_ALLOW` allows; the
/// profile is never written here (a saved grant was mirrored by the app's write queue before it
/// answered, `perm_queue`), and WebView2 never stores the answer itself.
fn complete_permission(p: PendingPerm, answer: webhost::PermAnswer) {
    let allow = matches!(answer, webhost::PermAnswer::State { state, .. } if state == crate::browser_perms::STATE_ALLOW);
    let state = if allow { COREWEBVIEW2_PERMISSION_STATE_ALLOW } else { COREWEBVIEW2_PERMISSION_STATE_DENY };
    // SavesInProfile false on every answer, before the state and the completion.
    if let Ok(a3) = p.args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
        let _ = unsafe { a3.SetSavesInProfile(false) };
    }
    let _ = unsafe { p.args.SetState(state) };
    let _ = unsafe { p.deferral.Complete() };
}

/// Completes a view's pending requests with Continue and closes its controller (its browser
/// process lets go of the user data folder). Twice is safe: the second finds nothing.
fn shut(inner: &Rc<RefCell<Inner>>) {
    let Ok(mut i) = inner.try_borrow_mut() else { return };
    let (pending, core, receivers) = (std::mem::take(&mut i.pending), i.core.take(), std::mem::take(&mut i.devtools));
    let (frames, watch) = (std::mem::take(&mut i.frames), i.frame_watch.take());
    let menus = std::mem::take(&mut i.menus);
    let (perms, notes) = (std::mem::take(&mut i.perms), std::mem::take(&mut i.notes));
    let (note_watch, audio_watch) = (i.note_watch.take(), i.audio_watch.take());
    drop(i);
    // A permission still waiting is refused, never left pending: nothing grants by the view going away.
    for (_, p) in perms {
        complete_permission(p, webhost::PermAnswer::Deny);
    }
    for (_, h) in notes {
        let _ = unsafe { h.note.remove_CloseRequested(h.close_token) };
    }
    if let Some((v24, token)) = note_watch {
        let _ = unsafe { v24.remove_NotificationReceived(token) };
    }
    if let Some((v8, token)) = audio_watch {
        let _ = unsafe { v8.remove_IsDocumentPlayingAudioChanged(token) };
    }
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
    let workers_refused = register_worker_refusal(inner, view, &mut token);
    register_policy(inner, view, workers_refused, &mut token).map_err(|x| e("policy", x))?;
    if setup.permissions {
        register_notifications(inner, view);
        register_audio(inner, view);
    }
    register_controller_events(inner, ctrl, &mut token).map_err(|x| e("focus", x))?;
    if setup.context_menu {
        register_context_menu(inner, view, &mut token).map_err(|x| e("context menu", x))?;
    }
    if !setup.resource_filters.is_empty() {
        register_requests(inner, view, &setup.resource_filters, &mut token).map_err(|x| e("requests", x))?;
    }
    if setup.track_document_method {
        register_document_method(inner, view, &mut token).map_err(|x| e("document method", x))?;
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

/// Subframe navigations the frame handler cancelled (tests only): the probe's positive control that
/// the cancel itself ran, not some other block (final review minor 4).
#[cfg(test)]
static FRAME_CANCELS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn register_navigation(inner: &Rc<RefCell<Inner>>, view: &ICoreWebView2, token: &mut i64) -> windows::core::Result<()> {
    unsafe { view.add_NavigationStarting(&starting_handler(inner, false), token) }?;
    // The same handler body, in frame mode, for FrameNavigationStarting: a subframe may not go to an
    // `iterm2-about:` page, so only the main frame can request the View Source page.
    let mut frame_token = 0i64;
    unsafe { view.add_FrameNavigationStarting(&starting_handler(inner, true), &mut frame_token) }?;
    register_navigation_rest(inner, view, token)
}

/// ONE NavigationStarting handler (later plans add their post to it), built for the main frame
/// (`frame` false) or for a subframe (`frame` true, which only cancels `iterm2-about:` targets).
fn starting_handler(inner: &Rc<RefCell<Inner>>, frame: bool) -> ICoreWebView2NavigationStartingEventHandler {
    let weak = Rc::downgrade(inner);
    NavigationStartingEventHandler::create(Box::new(move |sender, args| guard(|| {
        let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
        let uri = pwstr_out(|p| unsafe { args.Uri(p) }).unwrap_or_default();
        if frame {
            if webhost::frame_nav_cancelled(&uri) {
                let _ = unsafe { args.SetCancel(true) };
                #[cfg(test)]
                FRAME_CANCELS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            return Ok(());
        }
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
        // The kind needs a newer runtime (-1 without).
        let mut kind = COREWEBVIEW2_NAVIGATION_KIND(-1);
        if let Ok(a3) = args.cast::<ICoreWebView2NavigationStartingEventArgs3>() {
            let _ = unsafe { a3.NavigationKind(&mut kind) };
        }
        // The permission sync gate (13x6), AFTER the verdict (a refused navigation is never parked):
        // a store whose profile was not synced from the table lets no page navigate (a stale profile
        // grant would work with no question). Cancelled; a web page's TARGET is posted with how it
        // was judged, for the app to judge again and load after the sync (`nav_gate`).
        let gate = i.setup.nav_gate.as_ref().map(|g| g.load(std::sync::atomic::Ordering::SeqCst));
        match webhost::nav_gate(gate, &uri, by_tandem, webhost::cap_url(&uri).1) {
            webhost::GateAct::Open => {}
            webhost::GateAct::Cancel => {
                let _ = unsafe { args.SetCancel(true) };
                return Ok(());
            }
            webhost::GateAct::Park => {
                let _ = unsafe { args.SetCancel(true) };
                i.poster.post(Event::Parked { view: i.id, url: webhost::cap_url(&uri).0, user, redirect, by_tandem, history: kind.0 == 1, source });
                return Ok(());
            }
        }
        // The per-navigation hooks (13x5), only for a navigation that passed the verdict and the sync
        // gate: the Document filter's method is matched to this start (a refused or parked one is not).
        drop(i);
        inner.borrow_mut().last_main_nav = Some((webhost::cap_url(&uri).0, nav));
        let i = inner.borrow();
        // The single page-load-start event (ruling Q2).
        i.poster.post(Event::Starting { view: i.id, url: webhost::cap_url(&uri).0, kind: kind.0, user, redirect, nav });
        i.poster.post(Event::Loading(i.id, true));
        Ok(())
    })))
}

fn register_navigation_rest(inner: &Rc<RefCell<Inner>>, view: &ICoreWebView2, token: &mut i64) -> windows::core::Result<()> {
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

/// `NotificationReceived` (needs `ICoreWebView2_24`): WebView2 shows none of its own; the app gets
/// the notification (within the host's limits) and reports on it. An older runtime keeps its own
/// toast (one debug line says so).
fn register_notifications(inner: &Rc<RefCell<Inner>>, view: &ICoreWebView2) {
    let Ok(v24) = view.cast::<ICoreWebView2_24>() else {
        return crate::dlog!("webhost: this runtime has no notification events");
    };
    let weak = Rc::downgrade(inner);
    let handler = webview2_com::NotificationReceivedEventHandler::create(Box::new(move |_sender, args| guard(|| {
        let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
        let _ = unsafe { args.SetHandled(true) };
        let Ok(note) = (unsafe { args.Notification() }) else { return Ok(()) };
        let Ok(mut i) = inner.try_borrow_mut() else { return Ok(()) };
        let held = i.notes.len();
        if !i.note_limit.allow(Instant::now(), held) {
            drop(i);
            let _ = unsafe { note.ReportClosed() };
            return Ok(());
        }
        let origin = pwstr_out(|p| unsafe { args.SenderOrigin(p) }).unwrap_or_default();
        let title = pwstr_out(|p| unsafe { note.Title(p) }).unwrap_or_default();
        let body = pwstr_out(|p| unsafe { note.Body(p) }).unwrap_or_default();
        let t = i.next_token;
        i.next_token += 1;
        let w2 = Rc::downgrade(&inner);
        let closed = webview2_com::NotificationCloseRequestedEventHandler::create(Box::new(move |_s, _a| guard(|| {
            let Some(inner) = w2.upgrade() else { return Ok(()) };
            let Ok(mut i) = inner.try_borrow_mut() else { return Ok(()) };
            let id = i.id;
            if let Some(h) = i.notes.get_mut(&t) {
                if !h.closed_posted {
                    h.closed_posted = true;
                    i.poster.post(Event::NotificationClosed { view: id, token: t });
                }
            }
            Ok(())
        })));
        let mut close_token = 0i64;
        if unsafe { note.add_CloseRequested(&closed, &mut close_token) }.is_err() {
            drop(i);
            let _ = unsafe { note.ReportClosed() };
            return Ok(());
        }
        i.notes.insert(t, HeldNote { note, close_token, closed_posted: false });
        let id = i.id;
        i.poster.post(Event::NotificationShown { view: id, token: t, origin: webhost::cap_url(&origin).0, title: webhost::cap_note_title(&title), body: webhost::cap_note_body(&body) });
        Ok(())
    })));
    let mut t = 0i64;
    if unsafe { v24.add_NotificationReceived(&handler, &mut t) }.is_ok() {
        inner.borrow_mut().note_watch = Some((v24, t));
    } else {
        crate::dlog!("webhost: the notification handler could not be added");
    }
}

/// `IsDocumentPlayingAudioChanged` (needs `ICoreWebView2_8`): posts the current value on each change.
fn register_audio(inner: &Rc<RefCell<Inner>>, view: &ICoreWebView2) {
    let Ok(v8) = view.cast::<ICoreWebView2_8>() else {
        return crate::dlog!("webhost: this runtime has no audio state events");
    };
    let weak = Rc::downgrade(inner);
    let handler = webview2_com::IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |sender, _args| guard(|| {
        let (Some(inner), Some(sender)) = (weak.upgrade(), sender) else { return Ok(()) };
        let Ok(v8) = sender.cast::<ICoreWebView2_8>() else { return Ok(()) };
        let playing = bool_out(|b| unsafe { v8.IsDocumentPlayingAudio(b) });
        let Ok(i) = inner.try_borrow() else { return Ok(()) };
        i.poster.post(Event::AudioPlaying { view: i.id, playing });
        Ok(())
    })));
    let mut t = 0i64;
    if unsafe { v8.add_IsDocumentPlayingAudioChanged(&handler, &mut t) }.is_ok() {
        inner.borrow_mut().audio_watch = Some((v8, t));
    } else {
        crate::dlog!("webhost: the audio state handler was not added");
    }
}

/// Ruling R13x6-SW (`webhost::refuse_worker_request`): every request a service worker makes, its
/// script fetch included, is answered 403 on the spot, so no worker installs and none can show a
/// persistent notification through WebView2's own toast. WebView2 raises a worker's requests on every
/// view with such a filter; each answers the same. True only when the refusal is in place (needs
/// `ICoreWebView2_22`); notifications are not offered otherwise.
fn register_worker_refusal(inner: &Rc<RefCell<Inner>>, view: &ICoreWebView2, token: &mut i64) -> bool {
    let Ok(v22) = view.cast::<ICoreWebView2_22>() else {
        crate::dlog!("webhost: this runtime cannot filter service worker requests; notifications are not offered");
        return false;
    };
    let weak = Rc::downgrade(inner);
    let handler = WebResourceRequestedEventHandler::create(Box::new(move |_sender, args| guard(|| {
        let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
        if !from_service_worker(&args) {
            return Ok(());
        }
        let env = match inner.try_borrow() {
            Ok(i) => i.env.clone(),
            Err(_) => return Ok(()),
        };
        if let Ok(resp) = unsafe { env.env.CreateWebResourceResponse(None::<&windows::Win32::System::Com::IStream>, 403, windows::core::w!("Blocked"), windows::core::w!("")) } {
            let _ = unsafe { args.SetResponse(&resp) };
        }
        Ok(())
    })));
    if unsafe { view.add_WebResourceRequested(&handler, token) }.is_err() {
        crate::dlog!("webhost: the service worker refusal could not be added; notifications are not offered");
        return false;
    }
    let all = windows::core::w!("*");
    if unsafe { v22.AddWebResourceRequestedFilterWithRequestSourceKinds(all, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_SERVICE_WORKER) }.is_err() {
        crate::dlog!("webhost: the service worker filter could not be added; notifications are not offered");
        return false;
    }
    true
}

/// A request a service worker made, or the fetch of a worker's script (`Service-Worker` header).
fn from_service_worker(args: &ICoreWebView2WebResourceRequestedEventArgs) -> bool {
    let mut kind = COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS(0);
    if let Ok(a2) = args.cast::<ICoreWebView2WebResourceRequestedEventArgs2>() {
        let _ = unsafe { a2.RequestedSourceKind(&mut kind) };
    }
    let header = unsafe { args.Request() }
        .and_then(|r| unsafe { r.Headers() })
        .is_ok_and(|h| bool_out(|b| unsafe { h.Contains(windows::core::w!("Service-Worker"), b) }));
    webhost::refuse_worker_request(kind.0, header)
}

fn register_policy(inner: &Rc<RefCell<Inner>>, view: &ICoreWebView2, workers_refused: bool, token: &mut i64) -> windows::core::Result<()> {
    // Permissions are denied unless the person allows (pre-flight B3). A view the app decides for
    // (`ViewSetup::permissions`) holds the requests Tandem asks about and denies the rest on the
    // spot; every other view denies everything.
    let ask = inner.borrow().setup.permissions;
    let notes_supported = webhost::notifications_offered(view.cast::<ICoreWebView2_24>().is_ok(), workers_refused);
    let weak = Rc::downgrade(inner);
    let handler = PermissionRequestedEventHandler::create(Box::new(move |_sender, args| guard(|| {
        let Some(args) = args else { return Ok(()) };
        let deny = || {
            // WebView2 keeps a decision by default (SavesInProfile is TRUE): never store one.
            if let Ok(a3) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
                let _ = unsafe { a3.SetSavesInProfile(false) };
            }
            let _ = unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY) };
        };
        deny();
        if !ask {
            return Ok(());
        }
        let mut kind = COREWEBVIEW2_PERMISSION_KIND(0);
        let _ = unsafe { args.PermissionKind(&mut kind) };
        // Kinds Tandem never asks about (and any it does not know) stay denied.
        if !matches!(crate::browser_perms::route_kind(kind.0), crate::browser_perms::KindRoute::Ask(_)) {
            return Ok(());
        }
        // Without notification events, or with service workers able to run, WebView2 would show its own
        // toast for a granted origin: refuse.
        if kind.0 == 4 && !notes_supported {
            return Ok(());
        }
        let Some(inner) = weak.upgrade() else { return Ok(()) };
        let Ok(mut i) = inner.try_borrow_mut() else { return Ok(()) };
        let pending = i.perms.len();
        if !i.perm_limit.allow(Instant::now(), pending) {
            return Ok(());
        }
        let Ok(deferral) = (unsafe { args.GetDeferral() }) else { return Ok(()) };
        let uri = pwstr_out(|p| unsafe { args.Uri(p) }).unwrap_or_default();
        let t = i.next_token;
        i.next_token += 1;
        i.perms.insert(t, PendingPerm { deferral, args: args.clone() });
        let id = i.id;
        i.poster.post(Event::Permission { view: id, token: t, kind: kind.0, uri: webhost::cap_url(&uri).0 });
        Ok(())
    })));
    unsafe { view.add_PermissionRequested(&handler, token) }?;

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
        // A service worker's request was refused by `register_worker_refusal`; the app never answers it.
        if from_service_worker(&args) {
            return Ok(());
        }
        if inner.borrow().pending.len() >= MAX_PENDING_REQUESTS {
            return Ok(());
        }
        let url = unsafe { args.Request() }.ok().and_then(|r| pwstr_out(|p| unsafe { r.Uri(p) })).unwrap_or_default();
        let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT(0);
        let _ = unsafe { args.ResourceContext(&mut context) };
        // Every handler sees every request any filter matched: a Document request that only the
        // method tracker's filter matched is not this handler's (B10), and takes no deferral.
        if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT && inner.borrow().setup.track_document_method && !inner.borrow().setup.resource_filters.iter().any(|f| webhost::filter_matches(f, &url)) {
            return Ok(());
        }
        let Ok(deferral) = (unsafe { args.GetDeferral() }) else { return Ok(()) };
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

/// The method tracker (13x5, B10): its own handler on a Document-context filter. A request counts
/// only through `browser_marks::document_method_applies` (this view's document request whose URI is
/// the last main-frame NavigationStarting's); it takes no deferral and leaves the request to go on.
fn register_document_method(inner: &Rc<RefCell<Inner>>, view: &ICoreWebView2, token: &mut i64) -> windows::core::Result<()> {
    let weak = Rc::downgrade(inner);
    let handler = WebResourceRequestedEventHandler::create(Box::new(move |_sender, args| guard(|| {
        let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
        let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT(0);
        let _ = unsafe { args.ResourceContext(&mut context) };
        let Ok(request) = (unsafe { args.Request() }) else { return Ok(()) };
        let uri = webhost::cap_url(&pwstr_out(|p| unsafe { request.Uri(p) }).unwrap_or_default()).0;
        let i = inner.borrow();
        let last = i.last_main_nav.as_ref();
        if !crate::browser_marks::document_method_applies(true, context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT, &uri, last.map(|(u, _)| u.as_str())) {
            return Ok(());
        }
        let method = webhost::bounded_method(&pwstr_out(|p| unsafe { request.Method(p) }).unwrap_or_default());
        // `post` only sends on a channel; nothing downstream may borrow `inner` mutably.
        i.poster.post(Event::DocumentMethod { view: i.id, nav: last.map_or(0, |(_, n)| *n), method });
        Ok(())
    })));
    unsafe { view.add_WebResourceRequested(&handler, token) }?;
    let w = wide("*");
    unsafe { view.AddWebResourceRequestedFilter(PCWSTR(w.as_ptr()), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT) }
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
        assert!(!s.track_document_method);
    }

    /// Pre-flight B3: every permission request is denied first, only the kinds Tandem asks about are
    /// held, a held one is limited, and a view going away refuses what it still holds.
    #[test]
    fn the_permission_handler_denies_first_and_holds_only_asked_kinds() {
        let this = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webhost_win.rs")).unwrap();
        let live = this.split("#[cfg(test)]\nmod tests").next().unwrap();
        let policy = &live[live.find("fn register_policy").unwrap()..live.find("// A failed process is reported").unwrap()];
        let deny = policy.find("deny();").expect("denied first");
        let ask = policy.find("route_kind").expect("only asked kinds");
        let hold = policy.find("GetDeferral").expect("a deferral");
        assert!(deny < ask && ask < hold, "deny, then the kind route, then the deferral");
        assert!(policy.contains("perm_limit.allow"), "limited");
        // Task 3 review I1: no answer lets WebView2 store a decision.
        assert!(policy[..deny].contains("SetSavesInProfile(false)"), "the on-the-spot denial never saves");
        let complete = &live[live.find("fn complete_permission").unwrap()..live.find("fn shut(").unwrap()];
        assert!(complete.contains("SetSavesInProfile(false)") && !complete.contains("SetSavesInProfile(saves)"), "every answer sets SavesInProfile false");
        assert!(!complete.contains("SetPermissionState") && !live.contains("fn set_state_then") && !live.contains("fn set_next_state"), "an answer never writes the profile: every write goes through the app's one queue (Task 5 fix)");
        let write = &live[live.find("pub fn set_permission_state(").unwrap()..live.find("pub fn read_permission_states").unwrap()];
        assert!(write.matches("PermissionWritten").count() >= 2 && write.contains("ok: hr.is_ok()") && write.contains("ok: false"), "every write answers once with its sequence");
        assert!(!live.contains("pub fn set_permission_states"), "no batch write outside the queue");
        let read = &live[live.find("pub fn read_permission_states").unwrap()..live.find("pub fn close(self)").unwrap()];
        assert!(read.matches("complete: false").count() + read.matches("complete = false").count() >= 4 && read.contains("complete &="), "an incomplete read says so (I2)");
        assert!(read.contains("webhost::cap_url(&origin.unwrap_or_default())") && read.contains("if cut {") && read.contains("complete = false;"), "a cut origin makes the read partial (re-review minor 3)");
        assert!(!live.contains("Untouched"), "no answer leaves a request to WebView2's own prompt");
        let shut = &live[live.find("fn shut(").unwrap()..live.find("pub fn close_all").unwrap()];
        assert!(shut.contains("PermAnswer::Deny"), "a view going away refuses its pending requests");
        let create = &live[live.find("if setup.permissions").unwrap()..];
        assert!(create[..120].contains("register_notifications") , "notifications only where the app decides");
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
        // ...registered for the main frame and, in frame mode, for subframes (they may not reach iterm2-about:).
        assert!(code.contains("add_FrameNavigationStarting(&starting_handler(inner, true)") && code.contains("webhost::frame_nav_cancelled(&uri)"));
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

    /// Merge of 13x5 and 13x6: the one NavigationStarting handler runs the verdict, then the permission
    /// sync gate (which parks only a judged http/https target), then 13x5's per-navigation hook (the
    /// Document filter's method is bound to a start that passed both), then the Starting post.
    #[test]
    fn the_navigation_handler_runs_verdict_then_gate_then_hooks() {
        let this = include_str!("webhost_win.rs");
        let code = this.split("#[cfg(test)]\nmod tests").next().unwrap_or("");
        let h = &code[code.find("fn starting_handler").unwrap()..code.find("fn register_navigation_rest").unwrap()];
        let verdict = h.find("webhost::nav_verdict(").unwrap();
        let gate = h.find("webhost::nav_gate(gate").unwrap();
        let hook = h.find("last_main_nav = Some(").unwrap();
        let start = h.find("Event::Starting {").unwrap();
        assert!(verdict < gate && gate < hook && hook < start, "verdict, gate, 13x5 hooks, Starting");
        assert_eq!(h.matches("last_main_nav = Some(").count(), 1, "a refused or parked navigation is never the Document filter's");
        assert_eq!(code.matches("last_main_nav = Some(").count(), 1);
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

    /// 13x5 Task 3: a view with `track_document_method` reports POST for a form submit and GET for a
    /// plain link click. Run by hand in a console session (`scripts/xt test --lib webhost -- --ignored`);
    /// with no runtime it prints a note and passes.
    #[test]
    #[ignore]
    fn document_method_is_reported() {
        if runtime_version(None).is_err() {
            println!("WebView2 Runtime not installed; skipped");
            return;
        }
        let mut session = 0u32;
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
        let class = windows::core::w!("TandemWebView2MethodTest");
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), lpszClassName: class, ..Default::default() };
        unsafe { RegisterClassW(&wc) };
        let hwnd = unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), class, windows::core::w!("tandem-wv2-method"), WS_OVERLAPPEDWINDOW, 0, 0, 200, 200, None, None, None, None) }.expect("a window");
        let folder = std::env::temp_dir().join(format!("tandem-wv2-method-{}", std::process::id()));
        let (tx, rx) = mpsc::channel::<Posted>();
        let env_cell: Rc<RefCell<Option<Result<Env, String>>>> = Rc::new(RefCell::new(None));
        let cell = env_cell.clone();
        Env::create(Store::Default, &folder, None, &EnvOptions::default(), Box::new(move |r| *cell.borrow_mut() = Some(r)));
        let end = Instant::now() + Duration::from_secs(20);
        while env_cell.borrow().is_none() && Instant::now() < end {
            pump_until(&rx, 1, |_| false);
        }
        let env = env_cell.borrow_mut().take().expect("an environment within 20 s").expect("the environment");
        let setup = ViewSetup { guard_local_schemes: true, resource_filters: vec!["https://tandem.test/*".into()], track_document_method: true, ..Default::default() };
        let view = WebView::create(&env, hwnd.0 as isize, 1, (0, 0, 200, 200), setup, tx.clone());
        assert!(pump_until(&rx, 20, |e| matches!(e, Event::Created(1))).is_some(), "Created");
        let page = b"<title>m</title><a id=l href='/next'>next</a><form method=post action='/post'><input name=a value=b></form>".to_vec();
        // The POST result holds an iframe loading the page's own URL with GET (R13x5-7, checklist A).
        let post_page = b"<title>p</title><a id=l href='/next'>next</a><iframe src='/post'></iframe>".to_vec();
        // Answers every request the filter holds (the first request for `first`'s URL with its body, the
        // rest with `page`); returns every DocumentMethod and every requested URL, pumping `extra`
        // seconds past the first method.
        let serve = |first: Option<(&str, &[u8])>, extra: u64| -> (Vec<(u64, String)>, Vec<String>) {
            let mut first = first;
            let mut end = Instant::now() + Duration::from_secs(10);
            let (mut methods, mut urls) = (Vec::new(), Vec::new());
            while Instant::now() < end {
                match pump_until(&rx, 1, |e| matches!(e, Event::Request { view: 1, .. } | Event::DocumentMethod { view: 1, .. })) {
                    Some(Event::Request { token, url, .. }) => {
                        let body = match first {
                            Some((u, b)) if url == u => {
                                first = None;
                                b.to_vec()
                            }
                            _ => page.clone(),
                        };
                        urls.push(url);
                        view.answer_request(token, Answer::Respond { status: 200, mime: "text/html".into(), body, headers: vec![] });
                    }
                    Some(Event::DocumentMethod { nav, method, .. }) => {
                        if methods.is_empty() {
                            end = Instant::now() + Duration::from_secs(extra);
                        }
                        methods.push((nav, method));
                    }
                    _ => {}
                }
            }
            (methods, urls)
        };
        view.navigate_tandem("https://tandem.test/start");
        let (first, _) = serve(None, 0);
        assert_eq!(first.first().map(|f| f.1.as_str()), Some("GET"), "the first load reports GET");
        let _ = pump_until(&rx, 10, |e| matches!(e, Event::Navigated { view: 1, .. }));
        view.execute_script("document.forms[0].submit()", 1);
        let (posted, urls) = serve(Some(("https://tandem.test/post", &post_page)), 3);
        assert_eq!(posted.first().map(|f| f.1.as_str()), Some("POST"), "a form submit reports POST: {posted:?}");
        // The control: the iframe really loaded the same URL (two requests for /post).
        assert_eq!(urls.iter().filter(|u| u.as_str() == "https://tandem.test/post").count(), 2, "the iframe loaded the page's own URL: {urls:?}");
        // Whatever the iframe's request reported, the app's state keeps the main document's POST.
        let nav = posted[0].0;
        assert!(posted.iter().all(|(n, _)| *n == nav), "one navigation: {posted:?}");
        let mut state = crate::browser_marks::MethodState::default();
        state.starting(nav);
        for (n, m) in &posted {
            state.document(*n, m);
        }
        assert_eq!(state.method(), "POST", "an iframe loading the same URL does not change the answer: {posted:?}");
        println!("document methods after the POST (the iframe's included): {posted:?}");
        let _ = pump_until(&rx, 10, |e| matches!(e, Event::Navigated { view: 1, .. }));
        view.execute_script("document.getElementById('l').click()", 2);
        let (linked, _) = serve(None, 0);
        assert_eq!(linked.first().map(|f| f.1.as_str()), Some("GET"), "a link click reports GET");
        drop(view);
        drop(env);
        let _ = unsafe { DestroyWindow(hwnd) };
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

    const CHILD_SRCDOC: &str = "&lt;title&gt;child&lt;/title&gt;&lt;p&gt;c&lt;/p&gt;&lt;script&gt;window.__types=function(){var d=window[Symbol.for('iTermGraphDiscovery')];return JSON.stringify({api:typeof(d&amp;&amp;d.evaluateInFrame&amp;&amp;d.evaluateInFrame('x','1',function(){})),global:typeof window.graphDiscoveryEvaluateInFrame})};window.__fire=function(t,s){try{chrome.webview.postMessage({h:'iTermGraphDiscovery',b:{type:'EVAL_IN_FRAME',target:t,sender:s,requestID:'r',code:&quot;document.title='pwned'&quot;}})}catch(e){}}&lt;/script&gt;";

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
        /// `starting <url>` and `request <context> <url>` in the order the host saw them.
        order: Vec<String>,
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
        /// Every `Runtime.executionContextCreated` event, in order, with its session.
        created_json: Vec<(Option<String>, String)>,
        /// 13x6 Task 3: the permission requests the host posted (token, kind, uri), the notifications
        /// (token, origin, title, body), the audio changes, and the profile state reads.
        perm_reqs: Vec<(u64, i32, String)>,
        notes: Vec<(u64, String, String, String)>,
        audio: Vec<bool>,
        state_reads: Vec<Vec<(i32, String, i32)>>,
        /// `PermissionWritten` answers: (seq, ok).
        writes: Vec<(u64, bool)>,
        /// Each `PermissionStates`'s `complete`.
        reads_complete: Vec<bool>,
    }

    impl Probe {
        /// None (with a printed SKIPPED line) when the runtime is missing or this is session 0.
        fn start(pages: &[(&str, String)], sources: Vec<String>) -> Option<Probe> {
            Probe::start_with(pages, sources, false)
        }

        /// `start`, with `new_windows` as the view's setup (the browser pane's is on): a page's
        /// `window.open` then reaches the host's limiter, and an allowed one is `Event::NewWindow`.
        fn start_with(pages: &[(&str, String)], sources: Vec<String>, new_windows: bool) -> Option<Probe> {
            Probe::start_full(pages, sources, new_windows, &[])
        }

        /// `start_with`, with the custom schemes the environment registers (`iterm2-about` for the
        /// local pages, as the app's environment does).
        fn start_full(pages: &[(&str, String)], sources: Vec<String>, new_windows: bool, schemes: &[&str]) -> Option<Probe> {
            Probe::start_perms(pages, sources, new_windows, schemes, false)
        }

        /// `start_full`, with `permissions` as the view's setup (the browser pane's is on): permission
        /// requests, notifications and the audio state reach the host's events.
        fn start_perms(pages: &[(&str, String)], sources: Vec<String>, new_windows: bool, schemes: &[&str], permissions: bool) -> Option<Probe> {
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
                order: Vec::new(),
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
                created_json: Vec::new(),
                perm_reqs: Vec::new(),
                notes: Vec::new(),
                audio: Vec::new(),
                state_reads: Vec::new(),
                writes: Vec::new(),
                reads_complete: Vec::new(),
            };
            let cell: Rc<RefCell<Option<Result<Env, String>>>> = Rc::new(RefCell::new(None));
            let c2 = cell.clone();
            let opts = EnvOptions { custom_schemes: schemes.iter().map(|s| s.to_string()).collect(), ..Default::default() };
            Env::create(Store::Default, &folder, None, &opts, Box::new(move |r| *c2.borrow_mut() = Some(r)));
            assert!(p.wait(20, |_| cell.borrow().is_some()), "an environment within 20 s");
            let env = cell.borrow_mut().take().unwrap().expect("the environment");
            let setup = ViewSetup { guard_local_schemes: true, web_message: true, new_windows, permissions, resource_filters: vec!["*".into()], ..Default::default() };
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
            self.call_wait_for(session, method, params, 10)
        }

        fn call_wait_for(&mut self, session: Option<&str>, method: &str, params: &Value, secs: u64) -> Result<String, String> {
            let t = self.call(session, method, params);
            if !self.wait(secs, |p| p.answers.contains_key(&t)) {
                return Err(format!("{method}: no answer in {secs} s"));
            }
            self.answers.remove(&t).unwrap()
        }

        /// `Runtime.evaluate` in a frame's Tandem world (None is the main frame).
        fn eval(&mut self, frame: Option<u64>, expr: &str) -> Result<Value, String> {
            self.eval_for(frame, expr, 10)
        }

        /// `eval` with its own deadline: iTerm2's find engine highlights match by match, each walking
        /// the segment's text nodes again (find-utils.js collectCurrentTextNodes), so its time grows
        /// with the square of the matches (measured 2026-10-09 in Chromium: 500 in 2 s, 1000 in 6 s,
        /// 6000 not done in 4 minutes). iTerm2 awaits the same command (`FM:230`).
        fn eval_for(&mut self, frame: Option<u64>, expr: &str, secs: u64) -> Result<Value, String> {
            let (session, unique) = self.contexts.unique_for(frame).ok_or("no Tandem context for that frame")?;
            let r = self.call_wait_for(session.as_deref(), "Runtime.evaluate", &browser_world::evaluate_params(&unique, expr), secs)?;
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

        /// `page_eval` of a script whose answer is a string, decoded: ExecuteScript's JSON writes `<`
        /// as `\u003C` (probe run 2026-10-10), so a raw comparison with markup can never match.
        fn page_string(&mut self, js: &str) -> Result<String, String> {
            let text = self.page_eval(js)?;
            serde_json::from_str::<String>(&text).map_err(|e| format!("not a string ({e}): {text}"))
        }

        #[track_caller]
        fn load(&mut self, url: &str) {
            if let Err(e) = self.try_load(url) {
                panic!("{e}");
            }
        }

        /// `load`, answering what failed instead of panicking (with the world's state, the
        /// contexts the probe holds and the host's event order, for a failure to be read).
        fn try_load(&mut self, url: &str) -> Result<(), String> {
            let n = self.navigated.len();
            self.view().navigate_tandem(url);
            if !self.wait(15, |p| p.navigated.len() > n) {
                return Err(format!("Navigated within 15 s: {:?}", self.order));
            }
            if !self.navigated[n].1 {
                return Err(format!("the navigation to {url} failed"));
            }
            if !self.wait(5, |p| browser_world::world_usable(p.setup.state(), &p.contexts)) {
                let created: Vec<String> = self.created_json.iter().map(|(s, j)| format!("{s:?} {}", j.chars().take(240).collect::<String>())).collect();
                return Err(format!("the main frame has a Tandem context after loading {url}: setup {:?}, contexts {:?}, navigated {:?}, created {created:?}", self.setup.state(), self.contexts, self.navigated));
            }
            Ok(())
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
                Event::Starting { url, redirect: false, .. } => self.order.push(format!("starting {url}")),
                Event::Request { token, url, context, .. } => {
                    self.order.push(format!("request {context} {url}"));
                    let key = url.split(['#', '?']).next().unwrap_or("").to_string();
                    let answer = match self.pages.get(&key) {
                        Some(body) => {
                            // A page by default; a stylesheet or an image by its extension (Save Page As).
                            let mime = if key.ends_with(".css") { "text/css" } else if key.ends_with(".png") { "image/png" } else if key.ends_with(".svg") { "image/svg+xml" } else if key.ends_with(".js") { "text/javascript" } else { "text/html" };
                            Answer::Respond { status: 200, mime: mime.into(), body: body.clone().into_bytes(), headers: vec![] }
                        }
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
                Event::Permission { token, kind, uri, .. } => self.perm_reqs.push((token, kind, uri)),
                Event::NotificationShown { token, origin, title, body, .. } => self.notes.push((token, origin, title, body)),
                Event::AudioPlaying { playing, .. } => self.audio.push(playing),
                Event::PermissionStates { states, complete, .. } => {
                    self.state_reads.push(states);
                    self.reads_complete.push(complete);
                }
                Event::PermissionWritten { seq, ok, .. } => self.writes.push((seq, ok)),
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
                    self.created_json.push((session.clone(), json.to_string()));
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

    /// What the hostile page recorded (`__report()`, the JSON array of its `tag|self|args` records)
    /// that shows the view secret reached it: any record holding the secret's value, or a message,
    /// post or `sessionSecret` assignment the page received that names the field. Its `html`,
    /// `attrs` and `globals` records hold the page's own source, and HOSTILE itself names
    /// `sessionSecret` (its getter plant and its forgery), so the field name alone in those records
    /// is the page reading itself (the 2026-10-09 run's page_cannot_hijack_copy_mode failure).
    /// The offending record, cut to 300 characters, or None.
    fn page_saw_secret(report: &str, secret: &str) -> Option<String> {
        let decoded = serde_json::from_str::<String>(report).unwrap_or_else(|_| report.to_string());
        let Ok(records) = serde_json::from_str::<Vec<String>>(&decoded) else {
            return Some(format!("the report is not the page's record list: {}", decoded.chars().take(300).collect::<String>()));
        };
        let received = |r: &str| ["message|", "postMessage|", "sessionSecretSet|"].iter().any(|t| r.starts_with(t));
        records.into_iter().find(|r| r.contains(secret) || (received(r) && r.contains("sessionSecret"))).map(|r| r.chars().take(300).collect())
    }

    /// The 2026-10-09 run: HOSTILE's own source names `sessionSecret` and its `html` recorder keeps
    /// the page's outerHTML, so the old check (the field name anywhere in the report) failed on every
    /// run whatever Tandem did. The check now tells the page reading itself from a leak. Pure.
    #[test]
    fn the_secret_check_tells_a_leak_from_the_page_itself() {
        assert!(HOSTILE.contains("'sessionSecret'") && HOSTILE.contains("rec('html', null, [document.documentElement.outerHTML])"), "the page records its own source");
        let own = serde_json::to_string(&vec![format!("html|null|{{0=<html>{HOSTILE}</html>;}}")]).unwrap();
        assert!(own.contains("sessionSecret"), "the old check would fail here");
        assert_eq!(page_saw_secret(&own, "0123abcd"), None, "the page's own source is not a leak");
        let as_executed = serde_json::to_string(&own).unwrap();
        assert_eq!(page_saw_secret(&as_executed, "0123abcd"), None, "ExecuteScript's quoted answer decodes first");
        let msg = serde_json::to_string(&vec!["message|null|{0={sessionSecret=x;};}".to_string()]).unwrap();
        assert!(page_saw_secret(&msg, "0123abcd").is_some(), "a received message naming the field");
        let set = serde_json::to_string(&vec!["sessionSecretSet|null|{0=x;}".to_string()]).unwrap();
        assert!(page_saw_secret(&set, "0123abcd").is_some(), "an assignment through the plant");
        let value = serde_json::to_string(&vec!["attrs|null|{0=data-x=0123abcd;}".to_string()]).unwrap();
        assert!(page_saw_secret(&value, "0123abcd").is_some(), "the value anywhere");
        assert!(page_saw_secret("not json", "0123abcd").is_some(), "an unreadable report fails");
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
        // The same check from the child frame's page world (Task 3 review, minor 1).
        let c = p.page_eval("window.frames[0].__types()").expect("the child's check ran");
        let c: Value = serde_json::from_str::<String>(&c).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        assert_eq!(c, json!({"api": "undefined", "global": "undefined"}), "the child's page world has no graph API either");
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
        assert_eq!(p.graph.as_ref().unwrap().lookup(&main_id), Some(None), "and the main frame's id to the main frame");
        // Let the load's own discovery (the page's mutation observer) finish before this one starts.
        p.settle(1000);
        let ask = "new Promise(function (res) { window[Symbol.for('iTermGraphDiscovery')].rediscover(res, 5000); })";
        let before = p.log.len();
        let asked_before = p.clients(crate::browser_frames::HANDLER).into_iter().filter(|(_, b)| b["type"] == "REQUEST_REPORT").count();
        let g = p.eval(None, ask).expect("the rediscovery completes");
        assert_eq!(g["frameId"], json!(main_id), "{g}");
        assert_eq!(g["children"].as_array().map(Vec::len), Some(1), "{g}");
        assert_eq!(g["children"][0]["frameId"], json!(child.1), "the main frame's graph names the child by its registered id: {g}");
        // Only this rediscovery's two phases: the lines that name its request id.
        let asked: Vec<String> = p.clients(crate::browser_frames::HANDLER).into_iter().filter(|(_, b)| b["type"] == "REQUEST_REPORT").filter_map(|(_, b)| b["requestId"].as_str().map(str::to_string)).skip(asked_before).collect();
        assert_eq!(asked.len(), 1, "exactly this call's report request: {asked:?}");
        let rid = asked[0].clone();
        let evals: Vec<&String> = p.log[before..].iter().filter(|l| l.starts_with("Runtime.evaluate") && l.contains(&rid)).collect();
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

    /// A page Readability accepts: a title, and enough paragraphs to pass its character threshold.
    fn article_page(title: &str, extra_body: &str) -> String {
        article_page_with_head(title, extra_body, "")
    }

    /// `article_page` with `head` markup first in the page's `<head>` (a hostile page's script).
    fn article_page_with_head(title: &str, extra_body: &str, head: &str) -> String {
        let para = "Tandem reads the article text of a page and shows it alone, in a quiet column, so that a person can read it without the clutter around it. ";
        let body: String = (0..12).map(|i| format!("<p>Paragraph {i}. {para}{para}</p>")).collect();
        format!("<!doctype html><html><head>{head}<title>{title}</title></head><body><nav>menu</nav><article>{body}{extra_body}</article><footer>footer</footer></body></html>")
    }

    /// What `App::browser_js_call` does with a body: the isolated-world runner's expression.
    fn reader_run(p: &mut Probe, body: &str) -> Result<Value, String> {
        let expr = crate::browser_js::call_js(body, &Value::Null)?;
        p.eval(None, &expr)
    }

    fn reader_load(p: &mut Probe) {
        for b in crate::browser_reader::loader_bodies().expect("the loaders assemble") {
            assert_eq!(reader_run(p, &b), Ok(json!(true)), "a loader answers true");
        }
    }

    fn reader_actions(p: &Probe) -> Vec<String> {
        p.clients(crate::browser_reader::HANDLER).iter().filter_map(|(_, b)| b["action"].as_str().map(str::to_string)).collect()
    }

    /// 13x4 Task 1 (Review Focus 1, B6, B7): the loaders run in the isolated world, Reader Mode enters
    /// and posts "entered" as a Client message, a page's synthetic Escape does not leave it, a real
    /// Escape does and posts "exited"; Remove Distractions loads, enters and exits. Needs the
    /// console session.
    #[test]
    #[ignore]
    fn reader_mode_runs_in_a_page() {
        let Some(mut p) = Probe::start(&[(PAGE, article_page("An article", ""))], Vec::new()) else { return };
        p.load(PAGE);
        reader_load(&mut p);
        assert_eq!(p.page_eval("typeof window.iTermReaderMode + typeof window.Readability"), Ok("\"undefinedundefined\"".into()), "the reader objects exist only in the isolated world");
        assert_eq!(reader_run(&mut p, &crate::browser_reader::enter_reader_body()), Ok(json!(true)));
        assert_eq!(p.page_eval("document.body.classList.contains('reader-mode-active')"), Ok("true".into()));
        assert!(p.wait(10, |p| reader_actions(p).contains(&"entered".to_string())), "entered arrives as a Client message");
        // A page's synthetic Escape leaves nothing (B6), whoever the target is.
        p.page_eval("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}))").expect("the page's Escape");
        p.settle(300);
        assert_eq!(p.page_eval("document.body.classList.contains('reader-mode-active')"), Ok("true".into()), "synthetic Escape is ignored");
        assert!(!reader_actions(&p).contains(&"exited".to_string()));
        for kind in ["rawKeyDown", "keyUp"] {
            p.call_wait(None, "Input.dispatchKeyEvent", &json!({"type": kind, "key": "Escape", "code": "Escape", "windowsVirtualKeyCode": 27})).expect("the real Escape");
        }
        assert!(p.wait(10, |p| reader_actions(p).contains(&"exited".to_string())), "a real Escape leaves and posts exited");
        assert_eq!(p.page_eval("document.body.classList.contains('reader-mode-active')"), Ok("false".into()));
        // Enter and exit through the bodies.
        assert_eq!(reader_run(&mut p, &crate::browser_reader::enter_reader_body()), Ok(json!(true)));
        assert_eq!(reader_run(&mut p, &crate::browser_reader::exit_reader_body()), Ok(json!(true)));
        // Distraction removal.
        let [load, enter] = crate::browser_reader::distraction_bodies();
        assert_eq!(reader_run(&mut p, &load), Ok(json!(true)));
        assert_eq!(reader_run(&mut p, &enter), Ok(json!(true)));
        assert_eq!(reader_run(&mut p, &crate::browser_reader::exit_distraction_body()), Ok(json!(true)));
        p.finish();
    }

    /// What `App::browser_js_call` does with a `browser_marks::Call`: the isolated-world runner's
    /// expression, the typed arguments declared as constants.
    fn marks_run(p: &mut Probe, call: &crate::browser_marks::Call) -> Result<Value, String> {
        let expr = crate::browser_js::call_js(&call.body, &call.args)?;
        p.eval(None, &expr)
    }

    fn marks_body(p: &mut Probe, body: &str) -> Result<Value, String> {
        marks_run(p, &crate::browser_marks::Call { body: body.to_string(), args: Value::Null })
    }

    /// 13x5 Task 1 (B2, B3, B4): the Marks scripts run once in the isolated world. The point script
    /// answers the XPath under a point; the navigate script finds a hostile XPath without breaking out
    /// (the value is a typed argument); the annotations show, a page's synthetic Delete and Save
    /// clicks post nothing (the box only ever REQUESTS a rename or delete; the name is typed and the
    /// delete confirmed in a Tandem window, so no page-reachable press changes a mark); a page's own
    /// element with the old container id survives a second show; page CSS cannot displace the host;
    /// the names are not in the page's text; the position update answers; the layout monitor
    /// starts only in the isolated world. A trusted press cannot be synthesized here. Needs the
    /// console session.
    #[test]
    #[ignore]
    fn marks_scripts_run_in_a_page() {
        use crate::browser_marks as bm;
        let page = "<!doctype html><html><body style=\"margin:0\"><div style=\"height:80px\"></div><p id=\"target\" style=\"margin:0;height:40px\">Hello marked world</p><div style=\"height:2000px\"></div></body></html>".to_string();
        let Some(mut p) = Probe::start(&[(PAGE, page)], Vec::new()) else { return };
        p.load(PAGE);
        let at = marks_body(&mut p, &bm::xpath_script(10, 90).expect("the script")).expect("the point script");
        let loc = bm::Location::from_json(&at).expect("a location");
        // get-xpath-at-point.js names an element that has an id by its id (`//*[@id='...']`).
        assert_eq!((loc.xpath.as_str(), at["elementTag"].as_str(), loc.text_fragment.as_deref()), ("//*[@id='target']", Some("p"), Some("Hello marked world")), "{at}");
        // The monitor starts before any position update (the update calls it; task-1 review).
        assert!(marks_body(&mut p, &format!("return {}", bm::LAYOUT_MONITOR_JS.trim())).is_ok());
        let nav = bm::navigate_call(&loc).expect("navigate");
        assert!(matches!(marks_run(&mut p, &nav), Ok(Value::Bool(_))));
        let hostile = bm::Location { xpath: "//p[@id=\"x\\\"]</script>');window.__pwned=1;('\"]".into(), text_fragment: Some("\");window.__pwned=1;(\"".into()), ..loc.clone() };
        let nav = bm::navigate_call(&hostile).expect("navigate");
        assert!(marks_run(&mut p, &nav).is_ok());
        assert_eq!(p.page_eval("typeof window.__pwned"), Ok("\"undefined\"".into()), "a hostile XPath or fragment runs nothing");
        // Annotations.
        let marks = vec![loc.js_dict(Some("first"), "g1")];
        let show = bm::show_call(&marks, "mark-secret").expect("a call");
        assert_eq!(marks_run(&mut p, &show), Ok(json!(true)));
        // The annotations are in a closed shadow root: the page sees only the host.
        assert_eq!(p.page_eval("document.querySelectorAll('.iterm-mark-annotation').length"), Ok("0".into()));
        assert_eq!(p.page_eval("document.getElementById('iterm-mark-annotations') === null"), Ok("true".into()), "the host carries no id a page could use");
        assert_eq!(p.page_eval("document.body.lastElementChild.shadowRoot === null"), Ok("true".into()), "the root is closed");
        assert_eq!(p.page_eval("typeof window.iTermLayoutChangeMonitor + typeof window.tandemMarkRoot + typeof window.tandemMarkHost"), Ok("\"undefinedundefinedundefined\"".into()));
        assert_eq!(p.page_eval("document.body.innerText.includes('first')"), Ok("false".into()), "the name is not in the page's text");
        assert_eq!(p.page_eval("window.find('first')"), Ok("false".into()), "nor findable by the page");
        // A select-all in the page world (R13x5-6, R13x5-42, checklist A): neither the Selection API's
        // select-all nor a range over the body picks the name up. The control: the page's own text is
        // in the same selection, so a selection that read nothing cannot pass.
        assert_eq!(p.page_string("(document.execCommand('selectAll'), window.getSelection().toString())").map(|t| (t.contains("Hello marked world"), t.contains("first"))), Ok((true, false)), "select-all reads the page, not the name");
        assert_eq!(p.page_string("(function(){ var s = getSelection(); s.removeAllRanges(); var r = document.createRange(); r.selectNodeContents(document.body); s.addRange(r); return s.toString(); })()").map(|t| (t.contains("Hello marked world"), t.contains("first"))), Ok((true, false)), "a body range reads the page, not the name");
        p.page_eval("getSelection().removeAllRanges(); 0").expect("clear the selection");
        // Page CSS cannot displace the host (all: initial and inline important rules).
        p.page_eval("document.head.insertAdjacentHTML('beforeend', '<style>div { position: relative !important; transform: translateY(900px) !important; opacity: 0 !important; }</style>'); 0").expect("style");
        assert_eq!(p.page_eval("(function(){ var r = document.body.lastElementChild.getBoundingClientRect(); return r.top + ':' + getComputedStyle(document.body.lastElementChild).opacity; })()"), Ok("\"0:1\"".into()), "page rules do not move or hide the host");
        // A page element with the old container id is not removed by a second show, and the old host is.
        p.page_eval("var d = document.createElement('div'); d.id = 'iterm-mark-annotations'; document.body.prepend(d); 0").expect("decoy");
        assert_eq!(marks_run(&mut p, &show), Ok(json!(true)));
        assert_eq!(p.page_eval("document.getElementById('iterm-mark-annotations') !== null"), Ok("true".into()), "the page's own element stays");
        assert_eq!(marks_body(&mut p, "return window.tandemMarkRoot.querySelectorAll('.iterm-mark-annotation').length"), Ok(json!(1)), "one box: the old host went by Tandem's reference");
        assert_eq!(marks_body(&mut p, "return window.tandemMarkRoot.querySelectorAll('.iterm-mark-annotation button').length"), Ok(json!(3)));
        // The isolated world's own synthetic clicks are untrusted too.
        marks_body(&mut p, "window.tandemMarkRoot.querySelectorAll('button').forEach(b => b.click()); return true").expect("clicks");
        p.settle(300);
        assert!(p.clients(bm::MESSAGE_HANDLER).is_empty(), "an untrusted Delete or Save posts nothing");
        assert_eq!(marks_body(&mut p, "return window.tandemMarkRoot.querySelectorAll('.iterm-mark-annotation').length"), Ok(json!(1)), "and removes nothing");
        // The layout monitor (running already); positions now answer.
        assert_eq!(p.page_eval("typeof window.iTermLayoutChangeMonitor"), Ok("\"undefined\"".into()), "the monitor lives in the isolated world only");
        assert_eq!(marks_body(&mut p, &bm::update_positions_script("sec").expect("the script")).map(|v| v.is_array()), Ok(true));
        assert_eq!(p.page_eval("typeof window.iTermLayoutChangeMonitor"), Ok("\"undefined\"".into()), "the monitor lives in the isolated world only");
        assert!(marks_body(&mut p, bm::DISABLE_MONITOR_JS).is_ok());
        assert!(marks_body(&mut p, bm::ENABLE_MONITOR_JS).is_ok());
        p.finish();
    }

    /// 13x5 Task 4: the layout monitor posts `layoutChange` through the isolated-world binding after a
    /// page mutation, stays silent while disabled (`safelyModifyDOM`), and the page world never sees the
    /// monitor; synthetic Rename and Delete presses by the page post nothing. Needs the console session.
    #[test]
    #[ignore]
    fn annotations_post_renames() {
        use crate::browser_marks as bm;
        let page = "<!doctype html><html><body><p id=\"t\">Hello marked world</p></body></html>".to_string();
        let Some(mut p) = Probe::start(&[(PAGE, page)], Vec::new()) else { return };
        p.load(PAGE);
        assert!(marks_body(&mut p, &bm::monitor_body().unwrap()).is_ok());
        assert_eq!(p.page_eval("typeof window.iTermLayoutChangeMonitor"), Ok("\"undefined\"".into()));
        // iTerm2's monitor counts only a visible change: an added element over 10 px high or a text node
        // over 20 characters (layout-change-monitor.js). An empty div is not one, and stays silent.
        const EMPTY: &str = "document.body.appendChild(document.createElement('div')); 0";
        const TALL: &str = "var d = document.createElement('div'); d.style.height = '50px'; document.body.appendChild(d); 0";
        p.page_eval(EMPTY).expect("mutate");
        p.settle(900);
        assert!(p.clients(bm::LAYOUT_HANDLER).is_empty(), "an invisible mutation posts nothing");
        p.page_eval(TALL).expect("mutate");
        assert!(p.wait(5, |p| !p.clients(bm::LAYOUT_HANDLER).is_empty()), "a visible mutation posts a layout change");
        // The monitor disabled itself to post; the host brings it back (`marks_layout_message`).
        assert!(marks_body(&mut p, bm::ENABLE_MONITOR_JS).is_ok());
        assert!(marks_body(&mut p, bm::DISABLE_MONITOR_JS).is_ok());
        let seen = p.clients(bm::LAYOUT_HANDLER).len();
        p.page_eval(TALL).expect("mutate");
        p.settle(900);
        assert_eq!(p.clients(bm::LAYOUT_HANDLER).len(), seen, "silent while disabled");
        assert!(marks_body(&mut p, bm::ENABLE_MONITOR_JS).is_ok());
        // The control for "silent": enabled again, the same mutation posts.
        p.page_eval(TALL).expect("mutate");
        assert!(p.wait(5, |p| p.clients(bm::LAYOUT_HANDLER).len() > seen), "enabled again, a visible mutation posts");
        assert!(marks_body(&mut p, bm::ENABLE_MONITOR_JS).is_ok());
        let loc = bm::Location { xpath: "//p".into(), offset_y: 0, scroll_y: 0, text_fragment: None, y: 1 };
        let show = bm::show_call(&[loc.js_dict(Some("x"), "g1")], "mark-secret").expect("a call");
        assert_eq!(marks_run(&mut p, &show), Ok(json!(true)));
        marks_body(&mut p, "window.tandemMarkRoot.querySelectorAll('button').forEach(b => b.click()); return true").expect("clicks");
        p.settle(300);
        assert!(p.clients(bm::MESSAGE_HANDLER).is_empty(), "an untrusted press requests nothing");
        p.finish();
    }

    /// The page of the trigger probes: one paragraph of text, or many words for the cap.
    fn trigger_page(head: &str, body: &str) -> String {
        format!("<!doctype html><html><head><title>t</title>{head}</head><body>{body}</body></html>")
    }

    /// A trigger list holding one enabled Page Content Highlight (URL `tandem-probe`, content `regex`).
    fn trigger_list(regex: &str) -> Vec<crate::triggers::Trigger> {
        let mut t = crate::triggers::Trigger { regex: "tandem-probe\\.invalid".into(), action: "iTerm2SharedARC.HighlightBrowserTrigger".into(), parameter: "#ff0000;".into(), instant: false, enabled: true, name: String::new(), job: String::new(), match_type: 2, extra: toml::Table::new() };
        t.extra.insert("contentregex".into(), regex.into());
        vec![t]
    }

    /// The probe plays the app's part for `iTerm2Trigger` messages: a request is answered with
    /// `setTriggers` (the typed call, in the isolated world), then the match event is decoded and the
    /// effect of the first match is run.
    fn trigger_serve(p: &mut Probe, l: &crate::browser_triggers::Loaded) -> Vec<crate::browser_triggers::Match> {
        use crate::browser_triggers as bt;
        assert!(p.wait(15, |p| p.clients(bt::MESSAGE_HANDLER).iter().any(|(_, b)| b.get("requestTriggers").is_some())), "the engine asks for its triggers");
        marks_run(p, &bt::set_triggers_call(l)).expect("setTriggers runs in the isolated world");
        assert!(p.wait(15, |p| p.clients(bt::MESSAGE_HANDLER).iter().any(|(_, b)| b.get("matchEvent").is_some())), "a match event arrives");
        let mut all = Vec::new();
        for (_, body) in p.clients(bt::MESSAGE_HANDLER) {
            if let bt::Message::Matches(m) = bt::message_of(&body, &l.secret) {
                all.extend(m);
            }
        }
        all
    }

    /// 13x5 Task 9: a Page Content Highlight fires in a page. The engine is a Client user script
    /// (document start), the request and the match arrive as Client messages, the highlight runs in the
    /// isolated world and leaves a styled span in the shared DOM, and `window.iTerm2Triggers` exists only
    /// in the isolated world. Needs the console session.
    #[test]
    #[ignore]
    fn triggers_fire_in_a_page() {
        use crate::browser_triggers as bt;
        let l = bt::load(&trigger_list("wor(ld)"), "TRIG-SECRET-13x5".into());
        let code = bt::user_script(&l.secret).expect("the script");
        let Some(mut p) = Probe::start(&[(PAGE, trigger_page("", "<p>hello world</p>"))], vec![wrapped(&code)]) else { return };
        p.load(PAGE);
        let matches = trigger_serve(&mut p, &l);
        let m = matches.iter().find(|m| m.content_captures.as_deref() == Some(&["world".to_string(), "ld".to_string()])).expect("the content match");
        let (_, t) = l.triggers.iter().find(|(i, _)| *i == m.identifier).expect("a loaded trigger");
        let e = bt::effect(t, m, &t.parameter, &[]).expect("a highlight");
        let bt::Effect::Highlight { match_id, text, background } = e else { panic!("{e:?}") };
        marks_run(&mut p, &bt::highlight_call(&match_id, text.as_deref(), background.as_deref())).expect("highlightText");
        assert_eq!(p.page_eval("getComputedStyle(document.querySelector('span[data-iterm2-highlight]')).color"), Ok("\"rgb(255, 0, 0)\"".into()));
        assert_eq!(p.page_eval("typeof window.iTerm2Triggers"), Ok("\"undefined\"".into()), "the engine lives in the isolated world only");
        p.finish();
    }

    /// 13x5 Task 9 review I3: the engine on an `iterm2-about:` page asks for its triggers like any page;
    /// the message the host would act on is refused by its source, and the list is never sent there
    /// after a profile edit (`resend_allowed`). Needs the console session.
    #[test]
    #[ignore]
    fn triggers_skip_local_pages() {
        use crate::browser_triggers as bt;
        const LOCAL: &str = "iterm2-about:history";
        let l = bt::load(&trigger_list("wor(ld)"), "TRIG-SECRET-13x5".into());
        let code = bt::user_script(&l.secret).expect("the script");
        let pages = [(LOCAL, trigger_page("", "<p>hello world</p>"))];
        let Some(mut p) = Probe::start_full(&pages, vec![wrapped(&code)], false, &["iterm2-about"]) else { return };
        p.load(LOCAL);
        assert!(p.wait(15, |p| p.clients(bt::MESSAGE_HANDLER).iter().any(|(_, b)| b.get("requestTriggers").is_some())), "the engine asks");
        let sources: Vec<String> = p.routed.iter().filter(|r| r.via == Via::Client).map(|r| r.from.clone()).collect();
        assert!(!sources.is_empty());
        // A Client message's source is its isolated context's origin, and Chromium gives an
        // `iterm2-about:` document an opaque one ("://", probe run 2026-10-10b), so the source names no
        // local page. The pane's URL carries the rule (B5): `accepted` refuses on it, as the app calls
        // it (`browser_trigger_message` passes the pane's URL), and `marks_page` refuses a local URL
        // before any Marks message is read. The control: the same source is heard on a web page.
        for s in &sources {
            assert!(!crate::browser::local_source_ok(s), "a Client source is an origin, not a local URL: {s:?}");
            assert!(!bt::accepted(crate::browser_js::World::Client, None, s, LOCAL), "source {s:?} on the local page must be refused");
            assert!(bt::accepted(crate::browser_js::World::Client, None, s, "https://other.example/"), "the control: {s:?} on a web page is heard");
        }
        assert!(crate::browser_marks::is_local_page(LOCAL), "the Marks gate refuses the page by its URL");
        assert!(!bt::resend_allowed(LOCAL));
        p.finish();
    }

    /// 13x5 Task 9 (B8): a page with thousands of matches posts at most 64 of them, each capture cut,
    /// under the message limit. Needs the console session.
    #[test]
    #[ignore]
    fn many_matches_are_capped() {
        use crate::browser_triggers as bt;
        let l = bt::load(&trigger_list("ab+"), "TRIG-SECRET-13x5".into());
        let code = bt::user_script(&l.secret).expect("the script");
        let body = format!("<p>{}</p>", "abbb ".repeat(5000));
        let Some(mut p) = Probe::start(&[(PAGE, trigger_page("", &body))], vec![wrapped(&code)]) else { return };
        p.load(PAGE);
        let matches = trigger_serve(&mut p, &l);
        assert!(!matches.is_empty() && matches.len() <= bt::MAX_MATCHES * 2, "{} matches", matches.len());
        // The page-side cap, read before `message_of` (which cuts to 64 itself): every raw matchEvent
        // the engine posted holds at most 64 matches (final review M5).
        let raw: Vec<usize> = p.clients(bt::MESSAGE_HANDLER).iter().filter_map(|(_, b)| b.get("matchEvent").and_then(Value::as_str).map(str::to_string)).map(|t| serde_json::from_str::<Value>(&t).expect("a matchEvent is JSON")["matches"].as_array().expect("a matches array").len()).collect();
        assert!(!raw.is_empty() && raw.iter().all(|n| *n <= bt::MAX_MATCHES), "raw matches per event: {raw:?}");
        for r in p.routed.iter().filter(|r| r.via == Via::Client) {
            assert!(r.body.len() <= crate::webhost::MAX_MESSAGE);
        }
        p.finish();
    }

    /// 13x5 Task 9 (Review Focus 11, review C1): a page that replaced `Function.prototype.bind` and
    /// `chrome.webview.postMessage` before anything ran sees neither the trigger secret nor the marks
    /// secret after the engine asked, a trigger fired and a mark was shown and pressed; neither
    /// `window.iTerm2Triggers` nor `window.iTermLayoutChangeMonitor` exists in the page world. Needs the
    /// console session.
    #[test]
    #[ignore]
    fn page_never_sees_the_marks_or_trigger_secrets() {
        use crate::browser_marks as bm;
        use crate::browser_triggers as bt;
        let spy = "<script>(function(){var send=window.chrome.webview.postMessage.bind(window.chrome.webview);var seen=function(v){try{send({h:\"spy\",b:String(v)});}catch(e){}};var ob=Function.prototype.bind;Function.prototype.bind=function(){for(var i=0;i<arguments.length;i++){seen(arguments[i]);}seen(this);return ob.apply(this,arguments);};window.chrome.webview.postMessage=function(m){seen(JSON.stringify(m));};})();</script>";
        let l = bt::load(&trigger_list("wor(ld)"), "TRIG-SECRET-13x5".into());
        let code = bt::user_script(&l.secret).expect("the script");
        let body = "<p>hello world</p><p>two</p><p>three</p><p>four</p><p>five</p>";
        let Some(mut p) = Probe::start(&[(PAGE, trigger_page(spy, body))], vec![wrapped(&code)]) else { return };
        p.load(PAGE);
        let matches = trigger_serve(&mut p, &l);
        let m = matches.iter().find(|m| m.match_id.is_some()).expect("a content match");
        let (_, t) = l.triggers.iter().find(|(i, _)| *i == m.identifier).expect("loaded");
        let Some(bt::Effect::Highlight { match_id, text, background }) = bt::effect(t, m, &t.parameter, &[]) else { panic!("the trigger is a highlight") };
        marks_run(&mut p, &bt::highlight_call(&match_id, text.as_deref(), background.as_deref())).expect("highlightText");
        let loc = bm::Location { xpath: "//p[1]".into(), offset_y: 0, scroll_y: 0, text_fragment: None, y: 1 };
        let show = bm::show_call(&[loc.js_dict(Some("m"), "g1")], "MARKS-SECRET-13x5").expect("a call");
        marks_run(&mut p, &show).expect("annotations");
        marks_body(&mut p, "window.tandemMarkRoot.querySelectorAll('button').forEach(b => b.click()); return true").expect("clicks");
        p.settle(500);
        // Tandem's code runs in the isolated world, which has its own `Function.prototype` and
        // `chrome.webview`, so the spy normally reports nothing. The proof it is alive and posting is the
        // page's own call, made last (the 13x4 precedent; a silent spy proves nothing).
        assert!(!p.routed.iter().any(|r| r.via == Via::Page && r.body.contains("\"spy\"")), "Tandem called nothing the page replaced");
        p.page_eval("window.chrome.webview.postMessage({control: 'spy-alive'}); 0").expect("the control post");
        assert!(p.wait(5, |p| p.routed.iter().any(|r| r.via == Via::Page && r.body.contains("\"spy\"") && r.body.contains("spy-alive"))), "the spy is alive: the page's own post went through it");
        for r in p.routed.iter().filter(|r| r.via == Via::Page) {
            assert!(!r.body.contains("TRIG-SECRET-13x5") && !r.body.contains("MARKS-SECRET-13x5"), "a page-world message holds a secret: {}", r.body);
        }
        // Nothing Tandem wrote into the shared DOM holds either secret.
        assert_eq!(p.page_eval("document.documentElement.outerHTML.includes('SECRET-13x5')"), Ok("false".into()), "no secret in the page's DOM");
        assert_eq!(p.page_eval("typeof window.iTerm2Triggers + typeof window.iTermLayoutChangeMonitor"), Ok("\"undefinedundefined\"".into()));
        p.finish();
    }

    /// 13x4 Task 1 (B9): Readability keeps event-handler attributes, and the title goes in as markup;
    /// both run in the PAGE world. From there the reader's objects, the Tandem bridge and the view's
    /// secrets are out of reach, and no Client message results. Needs the console session.
    #[test]
    #[ignore]
    fn reader_article_handlers_stay_in_the_page_world() {
        let probe = "window.__hits = (window.__hits || []).concat([[typeof window.iTermReaderMode, typeof window.Readability, typeof window.__tandemClient, (window.chrome && window.chrome.webview) ? (() => { try { window.chrome.webview.postMessage({handler: 'readerMode', body: {action: 'distractionRemovalEntered'}}); } catch (e) {} return 'webview'; })() : 'none']])";
        let handler = probe.replace('"', "&quot;");
        // The image path is served (a non-image body, so the decode fails and onerror still fires).
        let img = format!("<p><img src=\"{PAGE}x\" onerror=\"{handler}\"><a href=\"javascript:{handler}\">link</a></p>");
        // Readability may split a title on ": ", so the title's copy has none.
        let title = format!("&lt;img src=x onerror=&quot;{}&quot;&gt;", handler.replace(": ", ":"));
        let image = format!("{PAGE}x");
        let Some(mut p) = Probe::start(&[(PAGE, article_page(&title, &img)), (image.as_str(), "not an image".to_string())], Vec::new()) else { return };
        p.load(PAGE);
        reader_load(&mut p);
        // The original page's own onerror fires at load; only growth after entering counts.
        p.settle(500);
        // A failed read fails the probe: it never counts as 0 hits.
        let hit_count = |p: &mut Probe| p.page_eval("(window.__hits || []).length").expect("the hit count").trim().parse::<usize>().expect("the hit count is a number");
        let before = hit_count(&mut p);
        assert_eq!(reader_run(&mut p, &crate::browser_reader::enter_reader_body()), Ok(json!(true)));
        let mut grew = false;
        for _ in 0..20 {
            if hit_count(&mut p) > before {
                grew = true;
                break;
            }
            p.settle(250);
        }
        assert!(grew, "the reader-inserted copy of the handler ran in the page world ({before} hits before)");
        let hits = p.page_eval("JSON.stringify(window.__hits)").expect("the hits");
        let hits: Vec<Vec<String>> = serde_json::from_str(&serde_json::from_str::<String>(&hits).unwrap()).unwrap();
        for h in &hits {
            assert_eq!(&h[..3], &["undefined", "undefined", "undefined"], "a handler sees no reader object, no Readability, no client binding");
            assert_eq!(h[3], "webview", "chrome.webview is present in the page world by design, so the forged post was really sent");
        }
        // No handler ran in the isolated world: its own window.__hits was never written.
        assert_eq!(reader_run(&mut p, "return typeof window.__hits"), Ok(json!("undefined")), "no handler ran in the isolated world");
        p.settle(500);
        assert_eq!(reader_actions(&p), vec!["entered".to_string()], "a page-world post is no Client message: nothing but the reader's own");
        assert!(p.routed.iter().any(|r| r.via == Via::Page && r.body.contains("distractionRemovalEntered")), "the forged post arrived as a page message");
        assert!(p.routed.iter().filter(|r| r.via == Via::Client).all(|r| r.client().map_or(true, |(h, _)| h == crate::browser_reader::HANDLER)));
        p.finish();
    }

    /// 13x4 Review Focus 6 (B7): a hostile page replaces `Function.prototype.bind`,
    /// `JSON.stringify` and `chrome.webview.postMessage` before anything runs, then the reader, the
    /// plain text and Remove Distractions run (twice) with a canary secret in the isolated world. The
    /// spy sees no canary, no result token and no session secret, and the reader objects are not in
    /// the page world. The spy is proved alive by the page's own post. Needs the console session.
    #[test]
    #[ignore]
    fn page_never_sees_the_view_secret() {
        let canary = hex32();
        let spy = r#"(function () {
            window.__spy = [];
            let busy = false;
            const text = (v) => { try { return typeof v === 'string' ? v : String(v) + ' ' + JSON.stringify(v); } catch (e) { return '?'; } };
            const log = (k, v) => { if (busy) { return; } busy = true; try { window.__spy.push(k + ':' + text(v)); } finally { busy = false; } };
            const ob = Function.prototype.bind;
            Function.prototype.bind = function () { log('bind', Array.prototype.slice.call(arguments).map(text).join(',')); return ob.apply(this, arguments); };
            const os = JSON.stringify;
            JSON.stringify = function (v) { log('stringify', v); return os.apply(this, arguments); };
            try {
                const w = window.chrome.webview, op = w.postMessage;
                w.postMessage = function (m) { log('post', m); return op.apply(this, arguments); };
            } catch (e) { window.__spy.push('postMessage not replaceable'); }
            window.addEventListener('message', (e) => log('message', e.data), true);
        })();"#.to_string();
        // The spy is the page's first script, in the page world (`hostile_page`); `sources` all run in
        // the isolated world, so only the canary goes there.
        let sources = vec![wrapped(&format!("window.__canary = '{canary}';"))];
        let Some(mut p) = Probe::start(&[(PAGE, article_page_with_head("An article", "", &format!("<script>{spy}</script>")))], sources) else { return };
        p.load(PAGE);
        reader_load(&mut p);
        let plain = reader_run(&mut p, &crate::browser_reader::plain_text_body()).expect("the plain text");
        assert!(plain.as_str().is_some_and(|t| t.chars().count() > 10), "the article's text came back: {plain}");
        assert_eq!(reader_run(&mut p, "return window.__canary"), Ok(json!(canary)), "the canary is in the isolated world");
        assert_eq!(reader_run(&mut p, &crate::browser_reader::enter_reader_body()), Ok(json!(true)));
        p.page_eval("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape', bubbles: true}))").expect("the page's Escape");
        p.settle(300);
        for kind in ["rawKeyDown", "keyUp"] {
            p.call_wait(None, "Input.dispatchKeyEvent", &json!({"type": kind, "key": "Escape", "code": "Escape", "windowsVirtualKeyCode": 27})).expect("the real Escape");
        }
        assert!(p.wait(10, |p| reader_actions(p).contains(&"exited".to_string())), "a real Escape leaves");
        // Task 4: the markdown, with and without chrome, runs the same way.
        for skip in [false, true] {
            let md = reader_run(&mut p, &crate::browser_reader::markdown_body(skip)).expect("the markdown");
            assert!(md.as_str().is_some_and(|t| t.contains("Paragraph 0")), "markdown came back: {md}");
        }
        let [load, enter] = crate::browser_reader::distraction_bodies();
        for _ in 0..2 {
            assert_eq!(reader_run(&mut p, &load), Ok(json!(true)));
            assert_eq!(reader_run(&mut p, &enter), Ok(json!(true)));
            assert_eq!(reader_run(&mut p, &crate::browser_reader::exit_distraction_body()), Ok(json!(true)));
        }
        assert_eq!(p.page_eval("typeof window.iTermReaderMode + typeof window.Readability + typeof window.iTermDistractionRemoval"), Ok("\"undefinedundefinedundefined\"".into()));
        // Task 7: View Source's two reads (the DevTools original, then the page-world DOM body) run
        // with the canary in the isolated world; the spy sees neither.
        let original = view_source_cached(&mut p, &format!("{PAGE}#frag")).expect("the cached source");
        assert!(original.contains("Paragraph 0"), "the original came back");
        let dom = p.page_eval(&crate::browser_js::page_call_script(crate::view_source::DOM_BODY, &json!({})).expect("the DOM script")).expect("the DOM text");
        assert!(dom.contains("Paragraph 0"), "the DOM text came back");
        // Proof the spy works: the page's own post is in its log.
        p.page_eval("window.chrome.webview.postMessage({control: 'spy-alive'})").expect("the control post");
        let log = p.page_eval("window.__spy.join('\\n')").expect("the spy log");
        assert!(log.contains("spy-alive"), "the spy is alive (a log that cannot fail proves nothing): {log}");
        assert!(!log.contains("postMessage not replaceable"), "the spy replaced postMessage: {log}");
        for bad in [canary.as_str(), "__tandemResult", "sessionSecret", "__tandemPost"] {
            assert!(!log.contains(bad), "the page saw {bad}");
        }
        p.settle(300);
        p.finish();
    }

    /// Task 4 (B7): turndown's markdown of a page, with and without chrome; the plain text has single
    /// spaces only. Needs the console session.
    #[test]
    #[ignore]
    fn markdown_is_turndowns() {
        let page = "<!doctype html><html><body><nav>Menu</nav><main><h1>Title</h1><p>a <b>b</b></p><img src=\"x.png\"><pre><code>let x;</code></pre></main><footer>F</footer></body></html>";
        // The page's image is served too (probe run 2026-10-10: an unserved x.png failed `finish`).
        let Some(mut p) = Probe::start(&[(PAGE, page.to_string()), ("https://probe.tandem-probe.invalid/x.png", "x".to_string())], Vec::new()) else { return };
        p.load(PAGE);
        let skip = reader_run(&mut p, &crate::browser_reader::markdown_body(true)).expect("the markdown");
        let skip = skip.as_str().expect("a string").to_string();
        assert!(skip.starts_with("# Title\n\na **b**"), "{skip}");
        assert!(skip.contains("```\nlet x;\n```"), "{skip}");
        assert!(!skip.contains("Menu") && !skip.contains("x.png") && !skip.trim_end().ends_with('F'), "{skip}");
        let all = reader_run(&mut p, &crate::browser_reader::markdown_body(false)).expect("the markdown");
        let all = all.as_str().expect("a string");
        assert!(all.contains("Menu") && all.trim_end().ends_with('F'), "{all}");
        assert!(!all.contains("x.png"), "media is removed");
        p.finish();
        let para = "word ".repeat(80);
        let five: String = (0..5).map(|_| format!("<p>{para}</p>")).collect();
        let Some(mut p) = Probe::start(&[(PAGE, format!("<!doctype html><html><head><title>t</title></head><body><article>{five}</article></body></html>"))], Vec::new()) else { return };
        p.load(PAGE);
        reader_load(&mut p);
        let plain = reader_run(&mut p, &crate::browser_reader::plain_text_body()).expect("the plain text");
        let plain = plain.as_str().expect("a string");
        assert!(plain.chars().count() > 10 && !plain.contains("  ") && !plain.contains('\n'), "single spaces only: {plain}");
        p.finish();
    }

    /// Task 3 findings I1 (B7): Remove Element hides the overlay under a point and nothing else: not
    /// the backdrop (no sweep), not the article. A point on the article removes nothing; a non-finite
    /// point is 0,0. The body posts nothing, and a page's synthetic click removes nothing. Needs the
    /// console session.
    #[test]
    #[ignore]
    fn remove_element_runs_in_a_page() {
        let extra = "<div id=backdrop style=\"position:fixed;left:0;top:0;width:100vw;height:100vh;background:rgba(0,0,0,.4);z-index:50\"></div><div id=overlay style=\"position:fixed;left:100px;top:100px;width:300px;height:200px;background:white;z-index:100\">Subscribe</div>";
        let Some(mut p) = Probe::start(&[(PAGE, article_page("An article", extra))], Vec::new()) else { return };
        p.load(PAGE);
        let shown = |p: &mut Probe, id: &str| p.page_eval(&format!("getComputedStyle(document.getElementById('{id}')).display")).expect("computed display");
        // Review minor: the leg can fail. A page listener counts the events it sees, and the whole
        // DOM is compared before and after: any removal (or any mutation) by a synthetic click or
        // contextmenu event, or by a trusted right press through Input.dispatchMouseEvent, fails it.
        // (A native ContextMenuRequested is not raised for synthesized input in this harness, so the
        // real menu pick stays the app's, covered by `remove_element_is_picked`.)
        p.page_eval("window.__ctx = 0; document.addEventListener('contextmenu', () => { window.__ctx++; }, true); 1").expect("the counter");
        let snapshot = |p: &mut Probe| p.page_eval("document.documentElement.outerHTML + '|' + getComputedStyle(document.getElementById('overlay')).display + getComputedStyle(document.getElementById('backdrop')).display").expect("a snapshot");
        let before = snapshot(&mut p);
        p.page_eval("document.getElementById('overlay').dispatchEvent(new MouseEvent('contextmenu', {bubbles: true, clientX: 150, clientY: 150})); document.getElementById('overlay').click(); 1").expect("synthetic");
        assert_eq!(p.page_eval("window.__ctx"), Ok("1".into()), "the page saw its own event, so the leg is not vacuous");
        for kind in ["mousePressed", "mouseReleased"] {
            p.call_wait(None, "Input.dispatchMouseEvent", &json!({"type": kind, "x": 150, "y": 150, "button": "right", "clickCount": 1})).expect("a trusted right press");
        }
        p.settle(200);
        println!("contextmenu events the page saw (1 synthetic, then the trusted press if raised): {:?}", p.page_eval("window.__ctx"));
        assert_eq!(snapshot(&mut p), before, "a synthetic or trusted click changes nothing");
        assert_ne!(shown(&mut p, "overlay"), "\"none\"", "a click removes nothing");
        assert!(p.routed.iter().all(|r| r.via != Via::Client), "and posts no Client message");
        // A point inside the overlay.
        assert_eq!(reader_run(&mut p, &crate::browser_reader::remove_element_body(150.0, 150.0).expect("the body")), Ok(json!(true)));
        assert_eq!(shown(&mut p, "overlay"), "\"none\"", "the overlay under the point is hidden");
        assert_ne!(shown(&mut p, "backdrop"), "\"none\"", "no backdrop sweep");
        let article = |p: &mut Probe| p.page_eval("getComputedStyle(document.querySelector('article')).display").expect("the article");
        assert_ne!(article(&mut p), "\"none\"", "the article stays");
        // Non-finite points are 0,0 (iTerm2 parity). With the overlay gone, the full-viewport backdrop
        // is what lies there, and removing the overlay under a point removes it too: it is hidden. The
        // article stays. (The finding's "a point inside the article removes nothing" is not achievable
        // on this page: every article point is under the backdrop, and on a page without one a point
        // on a paragraph hides that paragraph, because the search stops below the main container.)
        assert_eq!(reader_run(&mut p, &crate::browser_reader::remove_element_body(f64::NAN, f64::NAN).expect("the body")), Ok(json!(true)));
        assert_eq!(shown(&mut p, "backdrop"), "\"none\"", "the backdrop at 0,0 is what a non-finite point removes");
        assert_ne!(article(&mut p), "\"none\"", "a non-finite point removes no article");
        assert!(reader_actions(&p).is_empty(), "the body posts nothing");
        assert!(p.routed.iter().all(|r| r.via != Via::Client), "no Client message at all");
        p.finish();
    }

    /// 13x4 Task 5 (B1): the two DevTools reads, through the one sender, on a page answered from
    /// memory at a URL with a fragment: the tree's main frame URL has no fragment and the content of
    /// that frame and URL is the served HTML, with no network request. Needs the console session.
    #[test]
    #[ignore]
    fn devtools_reads_a_page() {
        const TEST_PAGE: &str = "https://tandem.test/";
        let html = "<!doctype html><html><head><title>t</title></head><body>served from memory</body></html>";
        let Some(mut p) = Probe::start(&[(TEST_PAGE, html.to_string())], Vec::new()) else { return };
        p.load(&format!("{TEST_PAGE}#frag"));
        let tree = p.call_wait(None, "Page.getResourceTree", &json!({})).expect("the tree");
        let tree: Value = serde_json::from_str(&tree).expect("tree JSON");
        let (frame, url) = crate::devtools::main_frame(&tree).expect("a main frame");
        assert_eq!(url, TEST_PAGE, "the frame's own URL carries no fragment");
        assert_eq!(crate::devtools::frame_for(&tree, &url).as_deref(), Some(frame.as_str()));
        let body = p.call_wait(None, "Page.getResourceContent", &crate::devtools::resource_content_params(&frame, &url)).expect("the content");
        let body: Value = serde_json::from_str(&body).expect("content JSON");
        let bytes = crate::devtools::content_bytes(&body).expect("content bytes");
        assert_eq!(String::from_utf8(bytes).unwrap(), html, "the served HTML");
        assert!(p.not_served.is_empty(), "no request was made for the read: {:?}", p.not_served);
        p.finish();
    }

    /// View Source's cached read, as the app's flow runs it: the tree, then the main frame's content.
    fn view_source_cached(p: &mut Probe, page_url: &str) -> Option<String> {
        use crate::view_source::{Next, Stage};
        let mut next = crate::view_source::start(crate::view_source::Choice::Cached);
        for _ in 0..4 {
            match next {
                Next::Read { method, params, stage } => {
                    assert!(crate::devtools::refusal(method, &params).is_none());
                    let r = p.call_wait(None, method, &params).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).ok_or_else(|| "no answer".to_string());
                    next = if stage == Stage::Tree { crate::view_source::after_tree(r) } else { crate::view_source::after_content(r, page_url) };
                }
                Next::Show(pending) => return Some(pending.raw),
                _ => return None,
            }
        }
        None
    }

    /// 13x4 Task 7: View Source shows the page as it was served, not as a script changed it. A page
    /// answered from memory at `https://tandem.test/`, its DOM changed afterwards: the cached read has
    /// the original paragraph, the DOM read the change; the source page built from the cached text
    /// holds the original in `.source-code` and the page URL in `.url`. Needs the console session.
    #[test]
    #[ignore]
    fn view_source_reads_the_original() {
        const TEST_PAGE: &str = "https://tandem.test/";
        let html = "<html><head><title>T</title></head><body><p class=\"k\">x</p></body></html>";
        let Some(mut p) = Probe::start(&[(TEST_PAGE, html.to_string())], Vec::new()) else { return };
        p.load(TEST_PAGE);
        p.page_eval("document.querySelector('p.k').textContent = 'changed'; 1").expect("the change");
        let cached = view_source_cached(&mut p, TEST_PAGE).expect("the cached source");
        assert!(cached.contains("<p class=\"k\">x</p>"), "the original: {cached}");
        let dom = p.page_string(&crate::browser_js::page_call_script(crate::view_source::DOM_BODY, &json!({})).expect("the DOM script")).expect("the DOM text");
        assert!(dom.contains("<p class=\"k\">changed</p>") && !dom.contains("<p class=\"k\">x</p>"), "the DOM differs: {dom}");
        p.finish();
        let css = crate::browser_js::resource("iterm-browser-common.css").unwrap_or_default();
        let page = crate::view_source::page(&cached, TEST_PAGE, css, "N").expect("the page");
        let Some(mut p) = Probe::start(&[(PAGE, page)], Vec::new()) else { return };
        p.load(PAGE);
        p.settle(500);
        let text = p.page_string("document.querySelector('.source-code').textContent").expect("the text");
        assert!(text.contains("<p class=\"k\">x</p>"), "{text}");
        assert_eq!(p.page_string("String(document.querySelector('p.url').textContent)"), Ok("https://tandem.test/".into()));
        p.finish();
    }

    /// 13x4 Task 7 I1 and the subframe ruling: the two facts View Source arming rests on, through the
    /// real `iterm2-about:` scheme. The main frame's `Starting` for `iterm2-about:source` is posted
    /// before its document `Request` (and the page it is answered with shows), and an iframe a page
    /// adds that points at `iterm2-about:source` produces neither: the frame navigation is cancelled
    /// in `FrameNavigationStarting`. Needs the console session.
    #[test]
    #[ignore]
    fn view_source_arming_facts() {
        const TEST_PAGE: &str = "https://tandem.test/";
        const FRAME_PAGE: &str = "https://tandem.test/f";
        const SOURCE: &str = "iterm2-about:source";
        let pages = [(TEST_PAGE, "<html><body><p>x</p></body></html>".to_string()), (FRAME_PAGE, "<html><body>frame</body></html>".to_string()), (SOURCE, "<html><body><p id=\"s\">SOURCE-SHOWN</p></body></html>".to_string())];
        // One world script, as the app always has (its canary): with none, Chromium made no named
        // world for the document after the iterm2-about round trip (probe run 2026-10-10b: no new
        // isolated context; the binding alone does not make the world). A browser pane never runs
        // without one, so the load after the source page must have the world back.
        let Some(mut p) = Probe::start_full(&pages, vec![wrapped("window.__tandemProbeWorld = 1;")], false, &["iterm2-about"]) else { return };
        // The facts below need no Tandem world; whether each load has one is recorded and asserted
        // last, so one run shows every fact (probe run 2026-10-10 stopped at a load).
        let first = p.try_load(TEST_PAGE);
        // The main frame: Starting first, then the document Request, and the page shows.
        p.order.clear();
        let n = p.navigated.len();
        p.view().navigate_tandem(SOURCE);
        assert!(p.wait(15, |p| p.navigated.len() > n), "Navigated within 15 s: {:?}", p.order);
        let at = |p: &Probe, what: &str| p.order.iter().position(|o| o == what);
        let (s, r) = (at(&p, &format!("starting {SOURCE}")), at(&p, &format!("request {} {SOURCE}", crate::browser_pages::CONTEXT_DOCUMENT)));
        assert!(s.is_some() && r.is_some() && s < r, "Starting before the document Request: {:?}", p.order);
        p.settle(300);
        assert_eq!(p.page_eval("document.getElementById('s') && document.getElementById('s').textContent"), Ok("\"SOURCE-SHOWN\"".into()));
        let back = p.try_load(TEST_PAGE);
        // A subframe to iterm2-about: is cancelled by the frame handler (the counter is the positive
        // control: an iframe blocked for any other reason would also send no request, final review
        // minor 4), and no request reaches the host.
        p.order.clear();
        let cancels = FRAME_CANCELS.load(std::sync::atomic::Ordering::SeqCst);
        p.page_eval("var f = document.createElement('iframe'); f.src = 'iterm2-about:source'; document.body.appendChild(f); 1").expect("the iframe");
        assert!(p.wait(5, |_| FRAME_CANCELS.load(std::sync::atomic::Ordering::SeqCst) > cancels), "the frame handler cancelled the subframe navigation: {:?}", p.order);
        p.settle(1000);
        assert!(!p.order.iter().any(|o| o.contains(SOURCE)), "a subframe reached the host: {:?}", p.order);
        // Control: a subframe's request is seen by the host when nothing cancels it.
        p.page_eval(&format!("var g = document.createElement('iframe'); g.src = '{FRAME_PAGE}'; document.body.appendChild(g); 1")).expect("the control iframe");
        assert!(p.wait(5, |p| p.order.iter().any(|o| o.starts_with("request ") && o.ends_with(FRAME_PAGE))), "the control subframe's request reached the host: {:?}", p.order);
        p.finish();
        assert_eq!(first, Ok(()), "the first load");
        assert_eq!(back, Ok(()), "the load after the source page");
    }

    /// 13x6 Task 9: the permissions page's buttons (adapted at load time: no inline handler) act on
    /// a real click and on nothing a script made up. A synthetic `click()` posts no revoke; a trusted
    /// press at the button (DevTools input) posts one, as the page's own message. Needs the console
    /// session (probe-desktop.ps1 runs it; this build has not).
    #[test]
    #[ignore]
    fn permissions_page_buttons_need_a_trusted_click() {
        const PAGE: &str = "iterm2-about:permissions";
        let (html, _) = crate::browser_pages::document("permissions", "probe-nonce").expect("the adapted page");
        let pages = [(PAGE, html)];
        let sources = vec![crate::browser_js::wrap(&crate::browser_js::local_page_script()), wrapped("window.__tandemProbeWorld = 1;")];
        let Some(mut p) = Probe::start_full(&pages, sources, false, &["iterm2-about"]) else { return };
        assert_eq!(p.try_load(PAGE), Ok(()));
        p.page_eval("window.confirm = function () { return true; }; window.onPermissionsLoaded({permissions: [{origin: 'https://a.b', permissionType: 'camera', decision: 'granted', createdAt: 1, updatedAt: 1}], hasMore: false}); 1").expect("a row");
        let revokes = |p: &Probe| p.routed.iter().filter(|r| r.via == Via::Page).filter_map(|r| crate::browser_js::route(&r.body, crate::browser_js::World::Page, r.frame, &r.from)).filter(|r| r.handler == "iterm2-about:permissions" && r.body.get("action").and_then(Value::as_str) == Some("revokePermission")).count();
        p.page_eval("document.querySelector('.revoke-button').click(); 1").expect("a synthetic click");
        p.settle(800);
        assert_eq!(revokes(&p), 0, "a click a script made posts nothing");
        let rect = p.page_eval("(function(){var r=document.querySelector('.revoke-button').getBoundingClientRect();return JSON.stringify([r.x+r.width/2,r.y+r.height/2]);})()").expect("the button");
        let xy: Vec<f64> = serde_json::from_str(&serde_json::from_str::<String>(&rect).unwrap()).unwrap();
        for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
            p.call_wait(None, "Input.dispatchMouseEvent", &json!({"type": kind, "x": xy[0], "y": xy[1], "button": "left", "clickCount": 1})).expect("a trusted press");
        }
        assert!(p.wait(10, |p| revokes(p) == 1), "the trusted click posts the revoke");
        p.finish();
    }

    /// 13x4 Task 6: the View Source page, with a hostile page's source in it, runs its copied
    /// highlighter and linkifier and the source stays text: no element, handler or script comes out
    /// of it, `window.__pwn` is never set, and only parsed http(s) URLs become links. The probe
    /// serves the page with no policy header, so only the escaping and the page script are tested
    /// here (the policy and the nonce are `browser_pages` and `view_source` unit tests). Needs the
    /// console session.
    #[test]
    #[ignore]
    fn view_source_shows_the_page() {
        let raw = "<a href=\"JAVASCRIPT:window.__pwn=1\">j</a> <img src=x onerror=\"window.__pwn=1\"> </script><script nonce=\"FAKE\">window.__pwn=1</script> <a href=\"http://x.test/ onmouseover=window.__pwn=1\">h</a> <a href=\"/rel?a=1\">r</a> <a href=\"data:text/html,<b>\">d</a> {{NONCE}}";
        let css = crate::browser_js::resource("iterm-browser-common.css").unwrap_or_default();
        let html = crate::view_source::page(raw, "https://tandem.test/orig?x='1'&y=\\", css, "REALNONCE").expect("the page");
        let Some(mut p) = Probe::start(&[(PAGE, html)], Vec::new()) else { return };
        p.load(PAGE);
        p.settle(500);
        let text = p.page_string("document.querySelector('.source-code').textContent").expect("the text");
        assert_eq!(text, raw, "the source is shown as text, whole");
        assert_eq!(p.page_eval("String(window.__pwn === undefined)"), Ok("\"true\"".into()), "nothing in the source ran");
        assert_eq!(p.page_eval("String(document.querySelectorAll('.source-code img, .source-code script, .source-code [onerror], .source-code [onmouseover]').length)"), Ok("\"0\"".into()), "no element of the source's own");
        assert_eq!(p.page_eval("String(document.querySelectorAll('script').length)"), Ok("\"1\"".into()), "only the template's script");
        let links = p.page_eval("JSON.stringify([...document.querySelectorAll('a.source-link')].map(a => a.getAttribute('href')))").expect("links");
        let links: Vec<String> = serde_json::from_str::<String>(&links).ok().and_then(|s| serde_json::from_str(&s).ok()).expect("a list of links");
        assert!(!links.is_empty() && links.iter().all(|l| l.starts_with("http://") || l.starts_with("https://")), "only web links: {links:?}");
        assert!(links.iter().any(|l| l == "https://tandem.test/rel?a=1"), "a relative link resolves against the viewed page: {links:?}");
        assert!(links.iter().all(|l| !l.contains(' ')), "no link holds a space: {links:?}");
        assert_eq!(p.page_string("String(document.querySelector('p.url').textContent)"), Ok("https://tandem.test/orig?x='1'&y=\\".into()), "the visible URL");
        p.finish();
    }

    /// 13x4 Task 10 (R13x45-2): Save Page As writes a folder. A page answered from memory at
    /// `https://tandem.test/` references `s.css` (which has `url(bg.svg)`) and `i.svg`; the real saver
    /// scripts run in the page world, the resources are read from the page's own loaded copy through
    /// the two DevTools reads, and the job's steps are carried out as the app does (create-new through
    /// `inert_files::write_new_checked`, `localfs::create_dir`). The folder holds an `index.html` whose
    /// `<style>` has `resources/bg.svg` and whose `<img>` is `resources/i.svg`, and the two files hold
    /// exactly the served bytes. The images are real SVG: Chromium keeps no content for an image it
    /// cannot decode ("Content unavailable. Resource was not cached", measured 2026-10-10), so the
    /// first run's text-as-PNG bytes were never readable. Needs the console session.
    #[test]
    #[ignore]
    fn save_page_writes_a_folder() {
        use crate::page_saver::{Job, Step};
        const TEST_PAGE: &str = "https://tandem.test/";
        let html = "<html><head><title>T</title><link rel=\"stylesheet\" href=\"s.css\"></head><body><img src=\"i.svg\"><p>hi</p></body></html>";
        const SVG_I: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2\" height=\"2\"><desc>IMAGE-I</desc></svg>";
        const SVG_BG: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2\" height=\"2\"><desc>IMAGE-BG</desc></svg>";
        let pages = [(TEST_PAGE, html.to_string()), ("https://tandem.test/s.css", "body { background: url(bg.svg); }".to_string()), ("https://tandem.test/i.svg", SVG_I.to_string()), ("https://tandem.test/bg.svg", SVG_BG.to_string())];
        let Some(mut p) = Probe::start(&pages, Vec::new()) else { return };
        p.load(TEST_PAGE);
        p.settle(800);
        let root = std::env::temp_dir().join(format!("tandem-save-probe-{}", hex32()));
        std::fs::create_dir_all(&root).expect("a temp root");
        let folder = root.join("site");
        let mut job = Job::new(folder.clone());
        job.bind_epoch(1);
        let mut queue: std::collections::VecDeque<Step> = job.start().into();
        let mut tree: Option<Value> = None;
        let mut done = None;
        // What each read answered, named in a failure.
        let mut fetched: Vec<String> = Vec::new();
        let mut read_budget = crate::devtools::Budget::with_limits(crate::page_saver::MAX_REPLY_BYTES, crate::page_saver::MAX_READ_TOTAL);
        let page_world = |p: &mut Probe, body: &str| -> Value {
            let script = crate::browser_js::page_call_script(body, &json!({})).expect("the script");
            let text = p.page_eval(&script).expect("the page answers");
            serde_json::from_str(&text).unwrap_or(Value::Null)
        };
        while let Some(step) = queue.pop_front() {
            assert!(job.permits(&step), "{step:?}");
            match step {
                Step::MakeDir(d) => {
                    if crate::localfs::judge_io(&d).and_then(|j| crate::localfs::create_dir(&j)).is_err() {
                        queue.extend(job.on_make_dir_failed(&d));
                    } else {
                        queue.extend(job.on_made(&d));
                    }
                }
                Step::Extract => {
                    let v = page_world(&mut p, &crate::page_saver::extract_body());
                    let urls = v.as_array().map(|a| a.iter().filter_map(|u| u.as_str().map(str::to_string)).collect()).unwrap_or_default();
                    queue.extend(job.on_urls(urls));
                }
                Step::Fetch(u) => {
                    if tree.is_none() {
                        tree = Some(p.call_wait(None, "Page.getResourceTree", &json!({})).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({})));
                    }
                    let t = tree.as_ref().unwrap();
                    let frame = crate::devtools::frame_for(t, &u);
                    let reply = frame.as_ref().map(|f| p.call_wait(None, "Page.getResourceContent", &crate::devtools::resource_content_params(f, &u)));
                    let got = reply.as_ref().and_then(|r| r.as_ref().ok()).and_then(|r| {
                        // As the app reads it: the saver's own caps (`Saving.budget`).
                        let bytes = read_budget.take(&serde_json::from_str(r).ok()?).ok()?;
                        Some((bytes, crate::devtools::resource_mime(t, &u)))
                    });
                    fetched.push(format!("{u}: frame {frame:?}, {}", match (&reply, &got) {
                        (_, Some((b, m))) => format!("{} bytes {m:?}", b.len()),
                        (Some(Err(e)), None) => format!("error {e}"),
                        _ => "no content".to_string(),
                    }));
                    queue.extend(job.on_resource(&u, got, &|_| false));
                }
                Step::Write { path, bytes } => {
                    if crate::inert_files::write_new_checked(&path, &bytes, Some(TEST_PAGE)).is_err() {
                        queue.extend(job.on_write_failed(&path));
                    }
                }
                Step::AddAttributes(_) => {
                    page_world(&mut p, &job.attributes_body());
                    queue.extend(job.on_attributes_added());
                }
                Step::GetHtml => {
                    let v = page_world(&mut p, &crate::page_saver::clone_body());
                    queue.extend(job.on_html(v.as_str().map(str::to_string)));
                }
                Step::Done(path) => done = Some(Ok(path)),
                Step::Failed(e) => done = Some(Err(e)),
                Step::Remove(_) | Step::RemoveDir(_) => {}
            }
        }
        p.finish();
        assert_eq!(done, Some(Ok(folder.join("index.html"))), "the save finished");
        let index = std::fs::read_to_string(folder.join("index.html")).expect("index.html");
        assert!(index.contains("resources/i.svg"), "the img points at the saved file: {index}; reads {fetched:?}");
        assert!(index.contains("<style") && index.contains("resources/bg.svg"), "the stylesheet is inlined with its url rewritten: {index}; reads {fetched:?}");
        assert_eq!(std::fs::read(folder.join("resources").join("i.svg")).expect("i.svg"), SVG_I.as_bytes());
        assert_eq!(std::fs::read(folder.join("resources").join("bg.svg")).expect("bg.svg"), SVG_BG.as_bytes());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Main bug from the 13x3 probe review: Chromium's Windows editing behavior extends forward by
    /// word to the start of the next word, so the iTerm2 script selected "hello " (trailing space).
    /// The assembled script (`browser_keys::extend_script`, load-time patch) selects "hello", then
    /// "hello world" by a second extension, and a backward extension is unchanged. Needs the console
    /// session.
    #[test]
    #[ignore]
    fn extend_word_forward_stops_before_the_space() {
        let page_html = "<!doctype html><html><body><p id=p>hello world again</p></body></html>";
        let Some(mut p) = Probe::start(&[(PAGE, page_html.to_string())], Vec::new()) else { return };
        p.load(PAGE);
        p.page_eval("(() => { const t = document.getElementById('p').firstChild; const r = document.createRange(); r.setStart(t, 0); r.setEnd(t, 0); const s = window.getSelection(); s.removeAllRanges(); s.addRange(r); })()").expect("the caret");
        let forward = crate::browser_keys::extend_script(false, true, 1).expect("the script assembles");
        p.eval(None, &crate::browser_js::make_safe(&forward)).expect("the first extension");
        assert_eq!(p.page_eval("window.getSelection().toString()"), Ok("\"hello\"".into()), "no trailing space");
        p.eval(None, &crate::browser_js::make_safe(&forward)).expect("the second extension");
        assert_eq!(p.page_eval("window.getSelection().toString()"), Ok("\"hello world\"".into()));
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
        find_command_for(p, secret, action, extra, 10)
    }

    /// `find_command` with its own deadline (`Probe::eval_for`: a page with hundreds of matches).
    fn find_command_for(p: &mut Probe, secret: &str, action: &str, extra: Value, secs: u64) -> Result<Value, String> {
        let cmd = crate::browser_find::command(action, extra, "main_probe", secret);
        let expr = crate::browser_js::call_js(crate::browser_find::COMMAND_JS, &json!({"command": cmd})).expect("the command");
        p.eval_for(None, &expr, secs)
    }

    /// The cap probes' page sizes: enough matches that the cap cuts the identifiers (about 430 of
    /// "word" with context 10, about 110 of the wide text with context 40, measured in Chromium), few
    /// enough that iTerm2's quadratic highlighting ends well inside `CAP_FIND_SECS` (the 2026-10-09
    /// run used 6000 and 3000: no answer in 10 s, and minutes in Chromium).
    const CAP_WORDS: usize = 600;
    const CAP_WIDE: usize = 600;
    const CAP_FIND_SECS: u64 = 120;

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
        let body = "<p>".to_string() + &"word ".repeat(CAP_WORDS) + "</p>";
        let sources = find_sources(&secret);
        let size: usize = sources.iter().map(String::len).sum();
        println!("find sources: {size} bytes (P15: one addScriptToEvaluateOnNewDocument per script)");
        let Some(mut p) = Probe::start(&[(PAGE, hostile_page(&body, "g"))], sources) else { return };
        p.graph = Some(crate::browser_frames::Frames::default());
        p.load(PAGE);
        find_command_for(&mut p, &secret, "startFind", json!({"searchTerm": "word", "contextLength": 10, "searchMode": "caseInsensitive"}), CAP_FIND_SECS).expect("startFind runs in the Tandem world");
        assert!(p.wait(20, |p| p.clients(crate::browser_find::HANDLER).iter().any(|(_, b)| b["action"] == "resultsUpdated")), "a results message arrived: {:?}", p.log.last());
        let routed: Vec<&Routed> = p.routed.iter().filter(|r| r.via == Via::Client && r.client().is_some_and(|(h, _)| h == crate::browser_find::HANDLER)).collect();
        let big = routed.iter().find(|r| r.client().is_some_and(|(_, b)| b["action"] == "resultsUpdated")).expect("resultsUpdated");
        assert!(big.body.len() <= crate::browser_js::CAP_BUDGET, "the message is (decoded, within the escaped weight budget, so within ViewGate::admit) {} bytes", big.body.len());
        let (_, b) = big.client().unwrap();
        assert_eq!(b["data"]["totalMatches"], CAP_WORDS, "the count is kept whole");
        assert!(b["data"]["matchIdentifiers"].as_array().is_some_and(|a| !a.is_empty() && a.len() < CAP_WORDS), "the cap cut the identifiers: {:?}", b["data"]["matchIdentifiers"].as_array().map(Vec::len));
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
        // The state that differs if the page had run the pattern: its full result holds matches.
        let want = "main_probe2";
        let total = p.clients(crate::browser_find::HANDLER).iter().rev().find(|(_, b)| b["action"] == "resultsUpdated" && b["data"]["instanceId"] == want).map(|(_, b)| b["data"]["totalMatches"].clone());
        assert_eq!(total, Some(json!(0)), "the page found nothing for a pattern it cannot run");
        // And ask for the open shortcuts anyway: over zero results the engine shows no bubble. (Had
        // the pattern run, this is where the two links' bubbles would appear.)
        for c in m.navigation_shortcuts("open", &secret) {
            let expr = crate::browser_js::call_js(crate::browser_find::COMMAND_JS, &json!({"command": c})).expect("the command");
            p.eval(None, &expr).expect("the shortcuts command runs");
        }
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
        let body = "<p>".to_string() + &"&#x691c;&#x7d22; &#xe9;&#x301;&#x8a9e;&#x1f600; ".repeat(CAP_WIDE) + "</p>";
        let page = format!("<!doctype html><html><head><title>wide</title></head><body>{body}</body></html>");
        let Some(mut p) = Probe::start(&[(PAGE, page)], find_sources(&secret)) else { return };
        p.graph = Some(crate::browser_frames::Frames::default());
        p.load(PAGE);
        find_command_for(&mut p, &secret, "startFind", json!({"searchTerm": "\u{691c}\u{7d22}", "contextLength": 40, "searchMode": "caseSensitive"}), CAP_FIND_SECS).expect("startFind runs in the Tandem world");
        assert!(p.wait(20, |p| p.clients(crate::browser_find::HANDLER).iter().any(|(_, b)| b["action"] == "resultsUpdated" && b["data"]["totalMatches"] == CAP_WIDE)), "the full result arrived, its count whole");
        let (_, b) = p.clients(crate::browser_find::HANDLER).into_iter().find(|(_, b)| b["action"] == "resultsUpdated" && b["data"]["totalMatches"] == CAP_WIDE).unwrap();
        let ids = b["data"]["matchIdentifiers"].as_array().map_or(0, Vec::len);
        let ctx = b["data"]["contexts"].as_array().map_or(0, Vec::len);
        assert!(ids > 0 && ids < CAP_WIDE && ctx == ids, "a prefix of identifiers and contexts: {ids} and {ctx}");
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    /// 13x3 Task 1 (B7, m13x2 B9 for the find scripts; made able to fail after the Task 1 review):
    /// window messages a page forges or reads. The page and a cross-origin child forge
    /// `iTermGlobalClick` and `iTermIframeClick`; neither carries the view secret. With new windows
    /// on (as a browser pane has them) and navigation shortcuts on, a synthetic key at a bubble's
    /// label is ignored by find-nav (R13x3-2: the bubbles stay and nothing opens); a real key, the
    /// control, is user-initiated and does open a window through the host. Needs the console session.
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
        p.settle(1000);
        let before = p.clients(crate::browser_find::HANDLER).len();
        // Forged window messages, from the page itself and from the cross-origin child.
        p.page_eval("window.postMessage({type: 'iTermGlobalClick', clickData: {offset: 1, textContent: 'alpha', coordinates: {x: 1, y: 1}}}, '*'); window.postMessage({type: 'iTermIframeClick', frameId: 'guess', clickLocation: {coordinates: [0, 0]}, framePath: ['guess']}, '*'); frames[0].postMessage({forge: 'click', x: 1}, '*'); frames[0].postMessage({forge: 'iframe'}, '*'); 1").expect("the forgery runs");
        p.settle(500);
        // Forged messages make the engine report nothing: its report count is unchanged.
        assert_eq!(p.clients(crate::browser_find::HANDLER).len(), before, "a forged window message made the find engine report");
        // Navigation shortcuts on, as the person's command turns them on.
        find_command(&mut p, &secret, "startNavigationShortcuts", json!({"selectionAction": "open"})).expect("the shortcuts start");
        assert!(wait_bubbles(&mut p, 10, |n| n > 0), "the bubbles show");
        let label = p.eval(None, "document.querySelector('[data-find-nav-bubble=\"true\"]').textContent").expect("a bubble's label");
        let label = label.as_str().expect("a label").to_lowercase();
        // Count the page-side window.open calls the Tandem world makes (find-engine opens a link with
        // window.open): the host's limiter drops a non-user window silently, so `new_windows` alone
        // could not tell an accepted synthetic key from an ignored one.
        p.eval(None, "window.__opens = 0; (function () { const o = window.open; window.open = function () { window.__opens += 1; return o.apply(window, arguments); }; })(); 1").expect("the open counter");
        // A synthetic key at the label: find-nav ignores it (R13x3-2), so the bubbles stay.
        let keys = serde_json::to_string(&label.chars().map(String::from).collect::<Vec<_>>()).unwrap();
        p.page_eval(&format!("for (const k of {keys}) {{ document.dispatchEvent(new KeyboardEvent('keydown', {{key: k, bubbles: true}})); }} 1")).expect("the synthetic keys run");
        p.settle(1500);
        assert!(bubbles(&mut p) > 0, "the synthetic key picked a bubble (find-nav took an untrusted key)");
        // 1. The page recorded every message it could see: none holds the secret or its field name.
        let report = p.page_eval("window.__report()").expect("the page report");
        if let Some(leak) = page_saw_secret(&report, &secret) {
            panic!("the view secret reached the page world: {leak}");
        }
        // 2. The synthetic key picked nothing (R13x3-2), so no window was asked for: no NewWindow. The
        //    host's own user-gesture gate on a find-nav open is exercised only by the control below.
        assert_eq!(p.eval(None, "window.__opens"), Ok(json!(0)), "find-nav took a synthetic key and tried to open a window");
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
        assert!(p.eval(None, "window.__opens").ok().and_then(|v| v.as_i64()).is_some_and(|n| n >= 1), "control: the counter sees the real pick");
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

    /// The copy mode cursor's display value when it is shown (`copy-mode-cursor.js` sets 'block').
    /// Asserted by value: "none", "absent" and "" (a failed read) all fail.
    const CURSOR_SHOWN: &str = "block";

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
        assert_eq!(cursor_display(&mut p), CURSOR_SHOWN, "the cursor is displayed");
        let mut m = cm::Model::default();
        for c in ['v', 'w', 'w'] {
            copy_press(&mut p, &mut m, &secret, c);
        }
        let sel = selection(&mut p);
        // As iTerm2 (macOS word movement): the first w stops at "world", the second finds no word
        // after it and stays (the 2026-10-09 run selected "" before FORWARD_WORD_TYPED).
        assert_eq!(sel, "hello ", "v w w selects from hello to the start of world");
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
        assert_eq!(cursor_display(&mut p), CURSOR_SHOWN, "the cursor is displayed again");
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

    /// 13x3 final wave (Task 8 minor 1): one key is one line, and Esc leaves. On four one-line
    /// paragraphs, a single trusted "j" moves the cursor to the next line and no further, and the listener reports it exactly once
    /// (one new KEY_HANDLER message after a settle, so a doubled report fails). The gate's own
    /// delivery path is the checklist's, not this probe's. A trusted Escape is reported by the listener as Leave. The gate's own path for Esc
    /// (WebView2's AcceleratorKeyPressed) cannot be raised from DevTools: that half stays with
    /// the person's check in the checklist. Needs the console session.
    #[test]
    #[ignore]
    fn copy_mode_one_key_is_one_line_and_escape_leaves() {
        use crate::browser_copymode as cm;
        let secret = hex32();
        let page = "<!doctype html><html><head><title>l</title></head><body><p>aaa</p><p>bbb</p><p>ccc</p><p>ddd</p></body></html>".to_string();
        let Some(mut p) = Probe::start(&[(PAGE, page)], copy_sources(&secret)) else { return };
        p.load(PAGE);
        p.view().focus();
        copy_on(&mut p, &secret);
        // The index of the paragraph whose line the cursor is on (nearest top).
        let line = |p: &mut Probe| -> i64 {
            p.eval(None, "(function () { const t = parseFloat(document.getElementById('iterm-copy-mode-cursor').style.top) - window.scrollY; const ps = Array.from(document.querySelectorAll('p')); let best = 0; ps.forEach(function (e, i) { if (Math.abs(e.getBoundingClientRect().top - t) < Math.abs(ps[best].getBoundingClientRect().top - t)) { best = i; } }); return best; })()")
                .ok()
                .and_then(|v| v.as_i64())
                .unwrap_or(-1)
        };
        let start = line(&mut p);
        assert!(start >= 0 && start < 3, "the cursor starts on a line with a next one: {start}");
        let mut m = cm::Model::default();
        let before = p.clients(cm::KEY_HANDLER).len();
        copy_press(&mut p, &mut m, &secret, 'j');
        // A key the listener reported twice would add a second message; the probe applies one step
        // per press, so the count (after a settle) is what shows a double delivery.
        p.settle(300);
        let reported = p.clients(cm::KEY_HANDLER);
        assert_eq!(reported.len(), before + 1, "one trusted j is reported exactly once: {:?}; key log {:?}", &reported[before.min(reported.len())..], p.log.iter().filter(|l| l.starts_with("Input.dispatchKeyEvent")).collect::<Vec<_>>());
        assert_eq!(line(&mut p), start + 1, "one j is one line");
        copy_press(&mut p, &mut m, &secret, 'k');
        assert_eq!(line(&mut p), start, "one k is one line back");
        // Escape: the listener reports it and the model leaves.
        let n = p.clients(cm::KEY_HANDLER).len();
        for kind in ["keyDown", "keyUp"] {
            p.call_wait(None, "Input.dispatchKeyEvent", &json!({"type": kind, "key": "Escape", "code": "Escape", "windowsVirtualKeyCode": 27})).expect("the real Escape");
        }
        assert!(p.wait(5, |p| p.clients(cm::KEY_HANDLER).len() > n), "Escape reached the world's listener");
        let (_, body) = p.clients(cm::KEY_HANDLER).pop().unwrap();
        let (key, ctrl, alt) = cm::key_of(&body).expect("a typed key body");
        let name = cm::key_name(&key, ctrl, alt).expect("a named key");
        assert_eq!(name, "Esc");
        // Esc is Quit, as "q" is (iTermCopyModeKeyBindings.dict:30-31, `"\U001B" = ("Quit")`), and
        // Quit stops selecting (iTermCopyModeHandler.m:437-440); ExitCopyMode (C-c, C-g) does not.
        assert_eq!(cm::Model::default().key(&name, &secret), cm::Step::Leave { stop_selecting: true });
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
            if let Some(leak) = page_saw_secret(&report, &secret) {
                panic!("the view secret reached the page world: {leak}");
            }
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
            // A state that differs if the page had started copy mode: the page's own keydown
            // listener sees the key (copy mode would have swallowed it), no cursor exists and
            // nothing is selected.
            p.page_eval("window.__keys = 0; document.addEventListener('keydown', function () { window.__keys++; }); 1").expect("the page listens");
            real_key(&mut p, 'j');
            p.settle(500);
            assert_eq!(p.page_eval("String(window.__keys)").as_deref(), Ok("\"1\""), "the key reached the page, so copy mode never started");
            assert_eq!(cursor_display(&mut p), "absent", "no copy mode cursor was made");
            assert_eq!(selection(&mut p), "", "nothing was selected");
            assert!(p.clients(cm::KEY_HANDLER).is_empty(), "no key listener, so nothing is reported");
            assert_eq!(p.page_eval("window.__leaks.length").as_deref(), Ok("0"), "the recorder was never called");
            p.finish();
        }
    }

    /// 13x3 Task 9 (I4): keys in iframes and untrusted ones. With focus inside a `srcdoc` iframe and
    /// copy mode on, a trusted "j" is reported by the iframe's world (a frame message) and the
    /// iframe page's own `keydown` listener sees nothing; a page's synthetic "y" in either frame
    /// changes nothing; an iframe added after copy mode started captures keys too once the new
    /// context is told, with the parameters `browser_copy_mode_new_context` composes (the shared
    /// `new_context_params`; the app's wiring around it is source-pinned). Needs the console session.
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
        assert_eq!(cursor_display(&mut p), CURSOR_SHOWN, "copy mode is still on");
        assert_eq!(selection(&mut p), "", "nothing was copied or selected");
        // An iframe added after copy mode started: its world takes keys once it is told.
        let seen_before = p.created_json.len();
        p.page_eval("const f = document.createElement('iframe'); f.id = 'f2'; f.srcdoc = '<body tabindex=0><p>late</p></body>'; document.body.appendChild(f); 1").expect("an iframe is added");
        assert!(p.wait(10, |p| p.contexts.frames().len() == 2), "the new iframe has a Tandem world");
        // The app's own composition (`browser_copy_mode_new_context` calls the same function) for each
        // context the page made after copy mode started; the probe does not send keys_on_js itself.
        let late: Vec<(Option<String>, serde_json::Value)> = p.created_json[seen_before..].iter().filter_map(|(s, j)| cm::new_context_params(&secret, j).map(|v| (s.clone(), v))).collect();
        assert!(!late.is_empty(), "the late iframe's Tandem world was announced");
        for (session, params) in &late {
            p.call_wait(session.as_deref(), "Runtime.evaluate", params).expect("the new context is told");
        }
        p.page_eval("document.getElementById('f2').contentWindow.focus(); document.getElementById('f2').contentDocument.body.focus(); 1").expect("focus moves into the new iframe");
        let n = p.clients(cm::KEY_HANDLER).len();
        real_key(&mut p, 'j');
        assert!(p.wait(5, |p| p.clients(cm::KEY_HANDLER).len() > n), "the late iframe reported the key");
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }

    // --- 13x6 Task 3: permissions, notifications and audio (runtime facts the bindings cannot confirm) ---

    const PERM_HOST: &str = "https://perm.tandem-probe.invalid";
    const PERM_PAGE: &str = "https://perm.tandem-probe.invalid/";

    /// A page that asks for notification permission on demand (`ask()`), recording the answer.
    fn perm_page(extra: &str) -> String {
        format!("<!doctype html><title>perm</title><script>window.__r = 'none'; function ask() {{ return Notification.requestPermission().then(function (r) {{ window.__r = r; }}); }}</script>{extra}")
    }

    /// A 0.5 s 440 Hz tone as a WAV data URI (8 kHz, mono, 16 bit).
    fn tone_wav_uri() -> String {
        use base64::Engine as _;
        let (rate, n) = (8000u32, 4000u32);
        let mut d: Vec<u8> = Vec::new();
        d.extend_from_slice(b"RIFF");
        d.extend_from_slice(&(36 + n * 2).to_le_bytes());
        d.extend_from_slice(b"WAVEfmt ");
        d.extend_from_slice(&16u32.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&rate.to_le_bytes());
        d.extend_from_slice(&(rate * 2).to_le_bytes());
        d.extend_from_slice(&2u16.to_le_bytes());
        d.extend_from_slice(&16u16.to_le_bytes());
        d.extend_from_slice(b"data");
        d.extend_from_slice(&(n * 2).to_le_bytes());
        for i in 0..n {
            let v = ((i as f64 * 440.0 * std::f64::consts::TAU / f64::from(rate)).sin() * 0.3 * 32767.0) as i16;
            d.extend_from_slice(&v.to_le_bytes());
        }
        format!("data:audio/wav;base64,{}", base64::engine::general_purpose::STANDARD.encode(&d))
    }

    impl Probe {
        /// Waits for the next permission request after `seen` and returns it.
        #[track_caller]
        fn next_permission(&mut self, seen: usize) -> (u64, i32, String) {
            assert!(self.wait(10, |p| p.perm_reqs.len() > seen), "a permission request within 10 s");
            self.perm_reqs[seen].clone()
        }

        /// Whether `page_string(js)` (or, with `raw`, `page_eval(js)`) becomes `want` within `secs`.
        fn page_becomes(&mut self, secs: u64, js: &str, raw: bool, want: &str) -> bool {
            let end = Instant::now() + Duration::from_secs(secs);
            loop {
                let got = if raw { self.page_eval(js) } else { self.page_string(js) };
                if got.as_deref() == Ok(want) {
                    return true;
                }
                if Instant::now() >= end {
                    return false;
                }
                self.settle(200);
            }
        }
    }

    /// The request arrives with its kind and the requester's URI; `saves` decides whether the profile
    /// keeps the answer; resetting the profile state and reading it back follow the same calls the app uses.
    #[test]
    #[ignore]
    fn permission_request_reaches_the_app() {
        use crate::browser_perms::{STATE_ALLOW, STATE_DEFAULT};
        use crate::webhost::PermAnswer;
        let Some(mut p) = Probe::start_perms(&[(PERM_PAGE, perm_page(""))], Vec::new(), false, &[], true) else { return };
        p.load(PERM_PAGE);
        p.page_eval("ask(); 1").expect("the page asks");
        let (token, kind, uri) = p.next_permission(0);
        assert_eq!(kind, 4, "notifications");
        assert!(uri.starts_with(PERM_HOST), "the requester's URI: {uri}");
        p.view().answer_permission(token, PermAnswer::State { state: STATE_ALLOW, saves: false });
        assert!(p.page_becomes(10, "window.__r", false, "granted"), "the promise resolves granted");
        // Not saved: the same profile sees the default after a fresh load.
        p.load(PERM_PAGE);
        assert_eq!(p.page_string("Notification.permission"), Ok("default".into()), "saves:false leaves the profile alone");
        // Saved: the app's queue writes the profile first, then answers; it survives a reload.
        p.page_eval("ask(); 1").expect("asks again");
        let (token, ..) = p.next_permission(1);
        p.view().set_permission_state(1, 4, PERM_HOST, STATE_ALLOW);
        assert!(p.wait(10, |p| p.writes.contains(&(1, true))), "the write answered: {:?}", p.writes);
        p.view().answer_permission(token, PermAnswer::State { state: STATE_ALLOW, saves: true });
        assert!(p.page_becomes(10, "window.__r", false, "granted"));
        p.load(PERM_PAGE);
        assert_eq!(p.page_string("Notification.permission"), Ok("granted".into()), "the mirrored grant is kept by the profile");
        // Put back to default through the profile, and read the non-default states.
        p.view().set_permission_state(2, 4, PERM_HOST, STATE_DEFAULT);
        assert!(p.wait(10, |p| p.writes.contains(&(2, true))), "the reset answered: {:?}", p.writes);
        assert!(p.page_becomes(10, "Notification.permission", false, "default"), "the state went back to default");
        p.view().read_permission_states();
        assert!(p.wait(10, |p| !p.state_reads.is_empty()), "PermissionStates within 10 s");
        assert!(p.state_reads.iter().flatten().all(|(k, o, _)| !(*k == 4 && o.starts_with(PERM_HOST))), "no kind-4 entry for the origin: {:?}", p.state_reads);
        p.finish();
    }

    /// Review I1 and ruling A: a saved Allow is mirrored into the profile (SetPermissionState)
    /// BEFORE the request completes, so a `new Notification` in the `then` is raised at once; an
    /// unsaved answer mirrors nothing and raises nothing.
    #[test]
    #[ignore]
    fn saved_grant_is_live_before_the_promise_resolves() {
        use crate::browser_perms::STATE_ALLOW;
        use crate::webhost::PermAnswer;
        const OTHER: &str = "https://other.tandem-probe.invalid/";
        let then = "<script>function go() { return Notification.requestPermission().then(function () { new Notification('T'); }); }</script>";
        let Some(mut p) = Probe::start_perms(&[(PERM_PAGE, perm_page(then)), (OTHER, perm_page(then))], Vec::new(), false, &[], true) else { return };
        p.load(PERM_PAGE);
        p.page_eval("go(); 1").expect("the page asks");
        let (token, ..) = p.next_permission(0);
        // The app's order (Task 5 fix): the queue's write, its completion, then the answer.
        p.view().set_permission_state(1, 4, PERM_HOST, STATE_ALLOW);
        assert!(p.wait(10, |p| p.writes.contains(&(1, true))), "the write answered: {:?}", p.writes);
        p.view().answer_permission(token, PermAnswer::State { state: STATE_ALLOW, saves: true });
        assert!(p.wait(10, |p| p.notes.iter().any(|n| n.2 == "T")), "the notification was raised: {:?}", p.notes);
        // Ruling A: WebView2 stored nothing itself; the grant was mirrored with SetPermissionState before the request completed.
        p.view().read_permission_states();
        assert!(p.wait(10, |p| p.state_reads.iter().flatten().any(|(k, o, st)| *k == 4 && o.starts_with(PERM_HOST) && *st == 1)), "the mirrored grant is in the profile: {:?}", p.state_reads);
        let t = p.notes[0].0;
        p.view().notification_report(t, NoteReport::Release);
        let before = p.notes.len();
        p.load(OTHER);
        p.page_eval("go(); 1").expect("the other origin asks");
        let (token, ..) = p.next_permission(1);
        p.view().answer_permission(token, PermAnswer::State { state: STATE_ALLOW, saves: false });
        p.settle(3000);
        assert_eq!(p.notes.len(), before, "an unsaved answer raises nothing: {:?}", p.notes);
        p.finish();
    }

    /// Task 5 fix (one write queue): every `set_permission_state` answers once with its own
    /// sequence number, so the queue can send the next only after the last landed; an Allow then
    /// a reset of the same entry leaves the profile at the reset.
    #[test]
    #[ignore]
    fn profile_writes_answer_once_with_their_sequence() {
        use crate::browser_perms::{STATE_ALLOW, STATE_DEFAULT};
        let Some(mut p) = Probe::start_perms(&[(PERM_PAGE, perm_page(""))], Vec::new(), false, &[], true) else { return };
        p.load(PERM_PAGE);
        p.view().set_permission_state(11, 4, PERM_HOST, STATE_ALLOW);
        assert!(p.wait(10, |p| p.writes.contains(&(11, true))), "the grant answered: {:?}", p.writes);
        assert!(p.page_becomes(10, "Notification.permission", false, "granted"), "live at once");
        p.view().set_permission_state(12, 4, PERM_HOST, STATE_DEFAULT);
        assert!(p.wait(10, |p| p.writes.contains(&(12, true))), "the reset answered: {:?}", p.writes);
        assert!(p.page_becomes(10, "Notification.permission", false, "default"), "the reset is the last word");
        p.settle(500);
        assert_eq!(p.writes, vec![(11, true), (12, true)], "each write answers exactly once");
        p.finish();
    }

    /// Task 5 fix (sync done only on a complete answer): a read of a profile the probe has just
    /// written answers once, complete, and holds the entry; the start-up sync trusts only such an
    /// answer.
    #[test]
    #[ignore]
    fn a_profile_read_says_it_is_complete() {
        use crate::browser_perms::{STATE_ALLOW, STATE_DEFAULT};
        let Some(mut p) = Probe::start_perms(&[(PERM_PAGE, perm_page(""))], Vec::new(), false, &[], true) else { return };
        p.load(PERM_PAGE);
        p.view().set_permission_state(21, 3, PERM_HOST, STATE_ALLOW);
        assert!(p.wait(10, |p| p.writes.contains(&(21, true))), "written: {:?}", p.writes);
        p.view().read_permission_states();
        assert!(p.wait(10, |p| !p.state_reads.is_empty()), "PermissionStates within 10 s");
        assert_eq!(p.reads_complete, vec![true], "a plain profile reads complete");
        assert!(p.state_reads[0].iter().any(|(k, o, st)| *k == 3 && o.starts_with(PERM_HOST) && *st == STATE_ALLOW), "{:?}", p.state_reads);
        p.view().set_permission_state(22, 3, PERM_HOST, STATE_DEFAULT);
        assert!(p.wait(10, |p| p.writes.contains(&(22, true))));
        p.finish();
    }

    /// Task 5 fix (no first page before the sync): the hazard the gate closes. A grant the profile
    /// holds is honored by the first page with NO `PermissionRequested`, so a stale grant from an
    /// earlier run works until the sync has rewritten the profile from the table; after a reset
    /// the page sees the default and must ask.
    #[test]
    #[ignore]
    fn a_profile_grant_is_honored_with_no_request() {
        use crate::browser_perms::{STATE_ALLOW, STATE_DEFAULT};
        let Some(mut p) = Probe::start_perms(&[(PERM_PAGE, perm_page(""))], Vec::new(), false, &[], true) else { return };
        p.load("about:blank");
        p.view().set_permission_state(31, 4, PERM_HOST, STATE_ALLOW);
        assert!(p.wait(10, |p| p.writes.contains(&(31, true))), "written: {:?}", p.writes);
        p.load(PERM_PAGE);
        assert_eq!(p.page_string("Notification.permission"), Ok("granted".into()), "the first page has the stale grant");
        assert!(p.perm_reqs.is_empty(), "and nothing asked Tandem: {:?}", p.perm_reqs);
        p.view().set_permission_state(32, 4, PERM_HOST, STATE_DEFAULT);
        assert!(p.wait(10, |p| p.writes.contains(&(32, true))));
        p.load(PERM_PAGE);
        assert_eq!(p.page_string("Notification.permission"), Ok("default".into()), "synced: the page must ask");
        p.finish();
    }

    /// Task 5 fix (revoke = reset confirmed, then reload): once the reset's `PermissionWritten`
    /// came back, a reload sees the default state; the app reloads only then.
    #[test]
    #[ignore]
    fn a_reload_after_the_confirmed_reset_sees_the_default() {
        use crate::browser_perms::{STATE_ALLOW, STATE_DEFAULT};
        let Some(mut p) = Probe::start_perms(&[(PERM_PAGE, perm_page(""))], Vec::new(), false, &[], true) else { return };
        p.view().set_permission_state(41, 4, PERM_HOST, STATE_ALLOW);
        assert!(p.wait(10, |p| p.writes.contains(&(41, true))));
        p.load(PERM_PAGE);
        assert_eq!(p.page_string("Notification.permission"), Ok("granted".into()));
        p.view().set_permission_state(42, 4, PERM_HOST, STATE_DEFAULT);
        assert!(p.wait(10, |p| p.writes.contains(&(42, true))), "the reset is confirmed first");
        p.view().reload();
        assert!(p.page_becomes(10, "Notification.permission", false, "default"), "the reloaded page has no grant");
        p.finish();
    }

    /// Task 5 (P-sweep): the revocation sweep script, run as the app runs it (isolated world,
    /// `return (...)`), reads the frames' and resources' origins of a hostile page: a cross-origin
    /// iframe, 5000 resource entries and a page that overrides `performance.getEntriesByType`.
    /// The answer is bounded by `frame_origins` and a reload follows only a match.
    #[test]
    #[ignore]
    fn revocation_reload_sweep_reads_the_frames_origins() {
        use crate::browser_perms::{check_iframe_origins_js, frame_origins, page_uses_origin, sweep_reloads};
        const FRAME: &str = "https://frame.tandem-probe.invalid/";
        let hostile = perm_page(&format!("<iframe src=\"{FRAME}\"></iframe><script>performance.getEntriesByType = function () {{ return []; }}; Object.defineProperty(window.location, 'origin', {{get: function () {{ return 'https://forged.invalid'; }}}});</script>"));
        let Some(mut p) = Probe::start_perms(&[(PERM_PAGE, hostile), (FRAME, "<!doctype html><title>f</title>".to_string())], Vec::new(), false, &[], true) else { return };
        p.load(PERM_PAGE);
        p.settle(500);
        let body = format!("return ({});", check_iframe_origins_js().trim_end().trim_end_matches(';'));
        let expr = crate::browser_js::call_js(&body, &Value::Null).expect("the sweep assembles");
        let got = p.eval(None, &expr).expect("the sweep runs in the isolated world");
        let list = frame_origins(&got).expect("a bounded array of strings");
        assert!(list.len() <= 256);
        assert!(list.iter().any(|o| o == "https://frame.tandem-probe.invalid"), "the cross-origin iframe is listed: {list:?}");
        assert!(page_uses_origin(PERM_PAGE, Some(&list), "https://frame.tandem-probe.invalid"), "a match reloads");
        assert!(!page_uses_origin(PERM_PAGE, Some(&list), "https://other.tandem-probe.invalid"), "no match, no reload");
        assert!(!page_uses_origin("https://c.d/", None, "https://frame.tandem-probe.invalid"), "a failed script compares the main origin only");
        // 5000 performance entries that are not resources: the answer keeps its shape and its frame.
        p.page_eval("for (var i = 0; i < 5000; i++) { performance.mark('m' + i); } 1").expect("marks");
        let again = p.eval(None, &expr).expect("still answers");
        let kept = frame_origins(&again);
        assert!(kept.as_ref().is_some_and(|l| l.iter().any(|o| o == "https://frame.tandem-probe.invalid")), "marks add no origin and the frame stays listed: {kept:?}");
        // Task 5 fix: 300 distinct resource origins (images the probe answers 404) push the answer over
        // the cap; it then reloads the pane (fail closed), never reads as "this origin is not used".
        // The page raises its resource timing buffer first (the default 250 would keep it under the cap).
        p.page_eval("performance.setResourceTimingBufferSize(1000); for (var i = 0; i < 300; i++) { var im = new Image(); im.src = 'https://h' + i + '.tandem-probe.invalid/x.png'; } 1").expect("images");
        p.settle(1500);
        let crowd = p.eval(None, &expr).expect("still answers");
        let other = "https://other.tandem-probe.invalid";
        assert!(frame_origins(&crowd).is_none(), "over 256 origins is unacceptable: {}", crowd.as_array().map_or(0, Vec::len));
        assert!(sweep_reloads(PERM_PAGE, Ok(&crowd), other), "an oversized answer reloads even for an origin it does not name");
        assert!(sweep_reloads(PERM_PAGE, Ok(&crowd), "https://frame.tandem-probe.invalid"), "the iframe's origin still reloads");
        p.finish();
    }

    /// Pre-flight B3: kinds Tandem never asks about are denied by the host and never posted, and the
    /// page sees the refusal.
    #[test]
    #[ignore]
    fn unasked_kinds_are_denied_by_the_host() {
        let page = perm_page("<script>window.__c = 'pending'; navigator.requestMIDIAccess({ sysex: true }).then(function () { window.__c = 'ok'; }, function (e) { window.__c = 'denied:' + e.name; });</script>");
        let Some(mut p) = Probe::start_perms(&[(PERM_PAGE, page)], Vec::new(), false, &[], true) else { return };
        p.load(PERM_PAGE);
        p.settle(2000);
        let got = p.page_string("window.__c");
        // A permission refusal, not any rejection (a MIDI "NotSupportedError" would pass otherwise).
        assert!(got.as_deref().is_ok_and(|c| c == "denied:NotAllowedError" || c == "denied:SecurityError"), "the MIDI request was refused by permission: {got:?}");
        assert!(p.perm_reqs.is_empty(), "a kind Tandem does not ask about never reaches the app: {:?}", p.perm_reqs);
        assert_eq!(p.page_eval("1"), Ok("1".into()), "the view still answers");
        p.finish();
    }

    /// Pre-flight B2: a request unanswered is bounded (at most 8 pending), a notification flood holds
    /// at most 16 objects, and the view stays responsive.
    #[test]
    #[ignore]
    fn permission_and_notification_floods_are_bounded() {
        use crate::browser_perms::STATE_ALLOW;
        use crate::webhost::{MAX_HELD_NOTES, MAX_PENDING_PERMISSIONS, PermAnswer};
        let page = perm_page("<script>for (var i = 0; i < 5000; i++) { try { navigator.geolocation.getCurrentPosition(function () {}, function () {}); Notification.requestPermission(); } catch (e) {} }</script>");
        let Some(mut p) = Probe::start_perms(&[(PERM_PAGE, page)], Vec::new(), false, &[], true) else { return };
        p.load(PERM_PAGE);
        p.settle(3000);
        assert!(p.perm_reqs.len() <= MAX_PENDING_PERMISSIONS + crate::webhost::PERMISSIONS_PER_SECOND * 4, "bounded: {}", p.perm_reqs.len());
        assert_eq!(p.page_eval("1"), Ok("1".into()), "still responsive");
        // Grant notifications, then flood new Notification() without ever reporting on them.
        let notif: Vec<(u64, i32)> = p.perm_reqs.iter().filter(|r| r.1 == 4).map(|r| (r.0, r.1)).collect();
        let (token, _) = notif.first().copied().unwrap_or_else(|| panic!("a notification request arrived: {:?}", p.perm_reqs));
        // The app's order (ruling A): mirror the grant into the profile, wait for the write, then answer.
        // Without the mirror an Allow may raise no notification at all and the cap below would hold with 0.
        p.view().set_permission_state(51, 4, PERM_HOST, STATE_ALLOW);
        assert!(p.wait(10, |p| p.writes.contains(&(51, true))), "the write answered: {:?}", p.writes);
        p.view().answer_permission(token, PermAnswer::State { state: STATE_ALLOW, saves: true });
        p.settle(500);
        p.page_eval("for (var i = 0; i < 5000; i++) { try { new Notification('n' + i); } catch (e) {} } 1").expect("the flood runs");
        p.settle(3000);
        // Review I2: the flood is held AT the cap, so a grant that raised nothing fails here.
        assert_eq!(p.notes.len(), MAX_HELD_NOTES, "the flood fills the cap of {MAX_HELD_NOTES} and no more: {}", p.notes.len());
        assert_eq!(p.page_eval("1"), Ok("1".into()), "still responsive after the flood");
        p.finish();
    }

    /// Final review M5 (ruling R13x6-SW): WebView2 raises `NotificationReceived` for non-persistent
    /// notifications only, so a service worker's `showNotification` would reach WebView2's own toast.
    /// For a granted origin (mirrored as the app does), the worker's script fetch is refused, the
    /// registration fails, no registration exists, no notification reaches Tandem and the probe's
    /// server never saw the script request.
    #[test]
    #[ignore]
    fn service_worker_notifications_are_refused() {
        use crate::browser_perms::{STATE_ALLOW, STATE_DEFAULT};
        const SW: &str = "https://perm.tandem-probe.invalid/sw.js";
        let worker = "self.addEventListener('install', function () { self.registration.showNotification('W'); });".to_string();
        let page = perm_page("<script>window.__sw = 'pending'; window.__regs = 'pending'; function reg() { return navigator.serviceWorker.register('/sw.js').then(function (r) { window.__sw = 'registered'; return r.showNotification('P').then(function () { window.__sw = 'shown'; }, function (e) { window.__sw = 'refused:' + e.name; }); }, function (e) { window.__sw = 'denied:' + e.name; }).then(function () { return navigator.serviceWorker.getRegistrations(); }).then(function (l) { window.__regs = String(l.length); }); }</script>");
        let Some(mut p) = Probe::start_perms(&[(PERM_PAGE, page), (SW, worker)], Vec::new(), false, &[], true) else { return };
        p.view().set_permission_state(61, 4, PERM_HOST, STATE_ALLOW);
        assert!(p.wait(10, |p| p.writes.contains(&(61, true))), "the grant is mirrored: {:?}", p.writes);
        p.load(PERM_PAGE);
        assert_eq!(p.page_string("Notification.permission"), Ok("granted".into()), "the origin is granted, as after Allow");
        p.page_eval("reg(); 1").expect("the page registers a worker");
        assert!(p.page_becomes(15, "window.__regs", false, "0"), "no registration exists: {:?} / {:?}", p.page_string("window.__sw"), p.page_string("window.__regs"));
        let got = p.page_string("window.__sw");
        assert!(got.as_deref().is_ok_and(|c| c.starts_with("denied:")), "the worker never installs: {got:?}");
        p.settle(1000);
        assert!(p.notes.is_empty(), "no notification at all: {:?}", p.notes);
        assert!(!p.order.iter().any(|o| o.ends_with("/sw.js")), "the script fetch never reached the server: {:?}", p.order);
        p.view().set_permission_state(62, 4, PERM_HOST, STATE_DEFAULT);
        assert!(p.wait(10, |p| p.writes.contains(&(62, true))));
        p.finish();
    }

    /// A request from an iframe is attributed to the frame's URI, not the page's.
    #[test]
    #[ignore]
    fn a_frame_request_names_the_frame() {
        const CHILD: &str = "https://child.tandem-probe.invalid/c.html";
        let page = perm_page(&format!("<iframe allow=\"geolocation\" src=\"{CHILD}\"></iframe>"));
        let child = "<!doctype html><script>navigator.geolocation.getCurrentPosition(function () {}, function () {});</script>".to_string();
        let Some(mut p) = Probe::start_perms(&[(PERM_PAGE, page), (CHILD, child)], Vec::new(), false, &[], true) else { return };
        p.load(PERM_PAGE);
        let (token, kind, uri) = p.next_permission(0);
        assert_eq!(kind, 3, "geolocation");
        assert!(uri.starts_with("https://child.tandem-probe.invalid/"), "the frame's URI: {uri}");
        p.view().answer_permission(token, crate::webhost::PermAnswer::Deny);
        p.finish();
    }

    /// Review M12: a playing document is reported even while muted, and muting does not end it.
    #[test]
    #[ignore]
    fn audio_state_reaches_the_app() {
        let audio = format!("<audio id=a loop src=\"{}\"></audio>", tone_wav_uri());
        let Some(mut p) = Probe::start_perms(&[(PERM_PAGE, perm_page(&audio))], Vec::new(), false, &[], true) else { return };
        p.load(PERM_PAGE);
        p.page_eval("var a = document.getElementById('a'); a.muted = true; a.play(); a.muted = false; 1").expect("the page plays");
        assert!(p.page_becomes(10, "!document.getElementById('a').paused", true, "true"), "the audio element is playing");
        assert!(p.wait(10, |p| p.audio.contains(&true)), "AudioPlaying true arrives: {:?}", p.audio);
        let n = p.audio.len();
        p.view().set_muted(true);
        assert!(p.view().is_muted(), "muted");
        p.settle(2000);
        assert!(!p.audio[n..].contains(&false), "muting does not report silence: {:?}", &p.audio[n..]);
        p.finish();
    }

    /// Review M12: only a document-changing navigation raises `Event::Starting` (Task 7 relies on it).
    #[test]
    #[ignore]
    fn same_document_navigation_starts_nothing() {
        let other = "https://perm.tandem-probe.invalid/other";
        let Some(mut p) = Probe::start_perms(&[(PERM_PAGE, perm_page("")), (other, perm_page(""))], Vec::new(), false, &[], true) else { return };
        p.load(PERM_PAGE);
        let starts = |p: &Probe| p.order.iter().filter(|o| o.starts_with("starting ")).count();
        let n = starts(&p);
        p.page_eval("location.hash = '#frag'; 1").expect("hash");
        p.page_eval("history.pushState({}, '', '/next'); 1").expect("pushState");
        p.settle(2000);
        assert_eq!(starts(&p), n, "a fragment and a pushState start nothing: {:?}", p.order);
        p.page_eval("location.href = '/other'; 1").expect("href");
        assert!(p.wait(10, |p| starts(p) > n), "a real navigation starts one: {:?}", p.order);
        p.finish();
    }

    /// Task 7: the view's mute is the person's: a page's own `muted = false`, a new play call and a
    /// same-document navigation do not lift it, and muting still reports the document as playing.
    #[test]
    #[ignore]
    fn a_page_cannot_unmute_itself() {
        let audio = format!("<audio id=a loop src=\"{}\"></audio>", tone_wav_uri());
        let Some(mut p) = Probe::start_perms(&[(PERM_PAGE, perm_page(&audio))], Vec::new(), false, &[], true) else { return };
        p.view().set_muted(true);
        p.load(PERM_PAGE);
        p.page_eval("var a = document.getElementById('a'); a.muted = false; a.volume = 1; a.play(); 1").expect("the page plays");
        assert!(p.wait(10, |p| p.audio.contains(&true)), "AudioPlaying true arrives: {:?}", p.audio);
        p.page_eval("var a = document.getElementById('a'); a.muted = false; a.play(); history.pushState({}, '', '/n'); 1").expect("the page tries again");
        p.settle(500);
        assert!(p.view().is_muted(), "the page cannot lift the view's mute");
        p.finish();
    }
}
