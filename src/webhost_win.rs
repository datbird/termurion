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
    AcceleratorKeyPressedEventHandler, CallDevToolsProtocolMethodCompletedHandler, CapturePreviewCompletedHandler, DevToolsProtocolEventReceivedEventHandler, CoreWebView2CustomSchemeRegistration, CoreWebView2EnvironmentOptions, CreateCoreWebView2ControllerCompletedHandler,
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
}

/// One WebView2 controller. Dropping it closes the controller; `close` says so.
pub struct WebView {
    inner: Rc<RefCell<Inner>>,
}

const MAX_NAV_URIS: usize = 64;
const MAX_PENDING_REQUESTS: usize = 256;

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

    /// Posts `Event::DevTools` for every `event` the page's target (and its attached children)
    /// raises. One receiver per event name per view; it is dropped with the view.
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
                let i = inner.borrow();
                i.poster.post(Event::DevTools { view: i.id, session, name: ev_name.clone(), json });
                Ok(())
            })));
            let mut token = 0i64;
            if unsafe { receiver.add_DevToolsProtocolEventReceived(&handler, &mut token) }.is_ok() {
                inner.borrow_mut().devtools.insert(name, (receiver, token));
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
    drop(i);
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
    if !setup.resource_filters.is_empty() {
        register_requests(inner, view, &setup.resource_filters, &mut token).map_err(|x| e("requests", x))?;
    }
    Ok(())
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

fn register_page_events(inner: &Rc<RefCell<Inner>>, view: &ICoreWebView2, token: &mut i64) -> windows::core::Result<()> {
    // ONE WebMessageReceived handler. A page's message is untrusted: size-capped here, its source
    // reported, its shape judged by the consumer (`webhost::invoke_request`, `logger_message`).
    let weak = Rc::downgrade(inner);
    let message = WebMessageReceivedEventHandler::create(Box::new(move |_sender, args| guard(|| {
        let (Some(inner), Some(args)) = (weak.upgrade(), args) else { return Ok(()) };
        let Some(json) = pwstr_out(|p| unsafe { args.WebMessageAsJson(p) }) else { return Ok(()) };
        if json.len() > webhost::MAX_MESSAGE {
            return Ok(());
        }
        let source = pwstr_out(|p| unsafe { args.Source(p) }).map(|s| webhost::cap_url(&s).0).unwrap_or_default();
        let i = inner.borrow();
        i.poster.post(Event::Message { view: i.id, json, source });
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
        let i = inner.borrow();
        i.poster.post(Event::Key { view: i.id, vk, mods: modifier_bits() });
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
        // The one NavigationStarting and one WebMessageReceived handler.
        assert_eq!(code.matches("NavigationStartingEventHandler::create").count(), 1);
        assert_eq!(code.matches("WebMessageReceivedEventHandler::create").count(), 1);
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
  const R = [], seen = new Set(), counts = {}, ids = [];
  let busy = false;
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
  window.__report = function () { return origStringify(R); };
  window.__counts = function () {
    const o = {}, n = ['slice', 'join', 'then', 'stringify', 'toString', 'indexOf', 'toJSON', 'getter', 'setter'];
    for (let i = 0; i < n.length; i++) o[n[i]] = counts[n[i]] || 0;
    return origStringify(o);
  };
  window.__forge = function () {
    post({ h: 'probeEcho', b: { v: 2, sessionSecret: '{{GUESS}}' } });
    post({ h: 'iTermSelectionMonitor', b: { selection: 'x', sessionSecret: '{{GUESS}}' } });
    post({ handler: 'iterm2-about:history', body: { action: 'clearAllHistory' } });
    const r = {};
    try { r.client = typeof window.__tandemClient; } catch (e) { r.client = 'threw'; }
    try { r.webkit = typeof (window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.probeEcho && window.webkit.messageHandlers.probeEcho.postMessage({})); } catch (e) { r.webkit = 'threw'; }
    window.postMessage({ namespace: 'IFRAME_GRAPH_DISCOVERY', type: 'CHILD_REPORT', requestId: 'r', payload: { graph: {} } }, '*');
    return origStringify(r);
  };
  window.__forge4 = function (extra) {
    const r = {};
    try { const d = window[Symbol.for('iTermGraphDiscovery')]; r.api = typeof (d && d.evaluateInFrame && d.evaluateInFrame('x', "document.title='pwned'", function () {})); } catch (e) { r.api = 'threw'; }
    r.global = typeof window.graphDiscoveryEvaluateInFrame;
    const targets = ids.concat(['x', 'guess'], JSON.parse(extra));
    for (let i = 0; i < targets.length; i++) {
      post({ h: 'iTermGraphDiscovery', b: { type: 'EVAL_IN_FRAME', target: targets[i], sender: 'x', requestID: 'r', code: "document.title='pwned'" } });
      try { window.frames[0].__fire(targets[i]); } catch (e) {}
    }
    return origStringify(r);
  };
})();
</script>"#;

    const CHILD_SRCDOC: &str = "&lt;title&gt;child&lt;/title&gt;&lt;p&gt;c&lt;/p&gt;&lt;script&gt;window.__fire=function(t){try{chrome.webview.postMessage({h:'iTermGraphDiscovery',b:{type:'EVAL_IN_FRAME',target:t,sender:'x',requestID:'r',code:&quot;document.title='pwned'&quot;}})}catch(e){}}&lt;/script&gt;";

    fn hostile_page(body: &str, guess: &str) -> String {
        format!("<!doctype html><html><head><title>hostile</title>{}</head><body>{body}</body></html>", HOSTILE.replace("{{GUESS}}", guess))
    }

    fn hex32() -> String {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).expect("random");
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// The probe's minimal stand-in for Task 1's `wrap`: the binding is captured into a closure
    /// before anything else runs, and `window.webkit.messageHandlers.<h>.postMessage(b)` becomes
    /// `__tandemClient(JSON.stringify({h, b}))`; both exist only in the isolated world.
    fn client_sources(secret: &str, load_echo: bool) -> Vec<String> {
        let wrap = |s: &str| {
            format!(
                "(function() {{ const __post = globalThis.{}; if (typeof __post !== 'function') {{ return; }} if (!window.webkit) {{ Object.defineProperty(window, 'webkit', {{ value: {{ messageHandlers: new Proxy({{}}, {{ get: (_, h) => ({{ postMessage: (b) => __post(JSON.stringify({{h: String(h), b}})) }}) }}) }} }}); }}\n{s}\n}})();",
                browser_world::BINDING
            )
        };
        let mut v = vec![wrap(&HOVER.replace("{{SECRET}}", secret)), wrap(GRAPH)];
        if load_echo {
            v.push(wrap("window.webkit.messageHandlers.probeEcho.postMessage({where: location.href});"));
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
    }

    impl Probe {
        /// None (with a printed SKIPPED line) when the runtime is missing or this is session 0.
        fn start(pages: &[(&str, String)], sources: Vec<String>) -> Option<Probe> {
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
            };
            let cell: Rc<RefCell<Option<Result<Env, String>>>> = Rc::new(RefCell::new(None));
            let c2 = cell.clone();
            Env::create(Store::Default, &folder, None, &EnvOptions::default(), Box::new(move |r| *c2.borrow_mut() = Some(r)));
            assert!(p.wait(20, |_| cell.borrow().is_some()), "an environment within 20 s");
            let env = cell.borrow_mut().take().unwrap().expect("the environment");
            let setup = ViewSetup { guard_local_schemes: true, web_message: true, resource_filters: vec!["*".into()], ..Default::default() };
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
                Event::Message { json, source, .. } => self.routed.push(Routed { via: Via::Page, frame: None, from: source, body: json }),
                Event::ScriptDone { token, json, .. } => {
                    if self.setup_token == Some(token) {
                        self.setup_token = None;
                        self.setup.answered(json);
                        if let Some(m) = self.setup.main_frame() {
                            self.contexts.set_main_frame(m);
                        }
                        self.step_setup();
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
                Event::DevTools { session, name, json, .. } => self.devtools(session, &name, &json),
                _ => {}
            }
        }

        fn devtools(&mut self, session: Option<String>, name: &str, json: &str) {
            let s = session.as_deref();
            match name {
                "Runtime.executionContextCreated" => self.contexts.created(s, json),
                "Runtime.executionContextDestroyed" => self.contexts.destroyed(s, json),
                "Runtime.executionContextsCleared" => self.contexts.cleared(s),
                "Page.frameNavigated" => self.contexts.navigated(s, json),
                "Runtime.bindingCalled" => match self.contexts.binding_called(s, json) {
                    Some((frame, origin, payload)) => {
                        let r = Routed { via: Via::Client, frame, from: origin, body: payload };
                        // The probe's stand-in for Task 2's rule: EVAL_IN_FRAME is answered only from the
                        // world, in the frame that sent it.
                        if let Some((h, b)) = r.client() {
                            if h == "iTermGraphDiscovery" && b.get("type").and_then(Value::as_str) == Some("EVAL_IN_FRAME") {
                                if let Some(code) = b.get("code").and_then(Value::as_str) {
                                    let _ = self.eval(frame, code);
                                }
                            }
                        }
                        self.routed.push(r);
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

    fn echo(secret: &str) -> String {
        format!("window.webkit.messageHandlers.probeEcho.postMessage({{v: 1, sessionSecret: \"{secret}\"}})")
    }

    /// The text Task 5's `clear_script` will produce, written out here.
    fn clear_hover(secret: &str) -> String {
        format!("(function() {{window.iTermHoverLinkHandler?.clearHover('{secret}');}})();")
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

    #[test]
    #[ignore]
    fn probe_page_cannot_reach_evaluate_in_frame() {
        let secret = hex32();
        let body = format!("<iframe srcdoc=\"{CHILD_SRCDOC}\"></iframe>");
        let Some(mut p) = Probe::start(&[(PAGE, hostile_page(&body, "g"))], client_sources(&secret, false)) else { return };
        p.load(PAGE);
        assert!(p.wait(10, |p| p.contexts.frames().len() == 1), "the iframe has a Tandem world");
        let frame_ids = p.call_wait(None, "Page.getFrameTree", &json!({})).ok().and_then(|t| browser_world::frame_ids(&t)).map(|(_, all)| all).unwrap_or_default();
        let as_js_string = serde_json::to_string(&serde_json::to_string(&frame_ids).unwrap()).unwrap();
        let r = p.page_eval(&format!("window.__forge4({as_js_string})")).expect("the forgery ran");
        assert!(r.contains("undefined"), "{r}");
        p.settle(1000);
        let k = p.contexts.frames()[0];
        assert_eq!(p.eval(Some(k), "document.title"), Ok(json!("child")), "the iframe's title");
        assert!(p.log.iter().filter(|l| l.starts_with("Runtime.evaluate") || l.starts_with("ExecuteScript")).all(|l| !l.contains("pwned")), "a forged EVAL_IN_FRAME was evaluated");
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
        assert_eq!(hrefs, vec!["about:srcdoc", "about:srcdoc", "about:srcdoc", child, top], "{hrefs:?}");
        assert!(cross_on_session, "the cross-site frame's world is on its child session: {seen}");
        p.settle(500);
        let stack = p.page_eval("String(window.__stackRead === true)").unwrap_or_default();
        println!("Runtime.enable detectable: {}", if stack.contains("true") { "yes" } else { "no" });
        assert_eq!(p.rejected_bindings, 0);
        p.finish();
    }
}
