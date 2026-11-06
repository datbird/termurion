//! The password manager's gate on Linux: the account password, checked by PAM under the `login`
//! service (authenticate only, no account or session calls). libpam is opened at run time, so
//! nothing links against it and a machine without PAM answers with a reason instead of failing to
//! start. The password goes to PAM through the conversation callback and nowhere else.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::OnceLock;

use super::super::user_auth::{Answer, WRONG};
use crate::pwm::Secret;

const SERVICE: &[u8] = b"login\0";
const SERVICE_FILES: [&str; 2] = ["/etc/pam.d/login", "/usr/lib/pam.d/login"];

const PAM_SUCCESS: c_int = 0;
const PAM_AUTH_ERR: c_int = 7;
const PAM_CONV_ERR: c_int = 19;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
#[cfg(test)]
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;
const PAM_SILENT: c_int = 0x8000;
const PAM_DISALLOW_NULL_AUTHTOK: c_int = 0x1;

#[repr(C)]
struct Message {
    style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct Response {
    resp: *mut c_char,
    retcode: c_int,
}

type ConvFn = unsafe extern "C" fn(c_int, *mut *const Message, *mut *mut Response, *mut c_void) -> c_int;

#[repr(C)]
struct Conv {
    conv: ConvFn,
    data: *mut c_void,
}

struct Pam {
    start: unsafe extern "C" fn(*const c_char, *const c_char, *const Conv, *mut *mut c_void) -> c_int,
    authenticate: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    end: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    strerror: unsafe extern "C" fn(*mut c_void, c_int) -> *const c_char,
    /// Linux-PAM 1.4 and later; used by the tests to point at a scratch service folder.
    start_confdir: Option<unsafe extern "C" fn(*const c_char, *const c_char, *const Conv, *const c_char, *mut *mut c_void) -> c_int>,
}

/// The library is opened once and kept for the life of the process.
fn pam() -> Result<&'static Pam, String> {
    static PAM: OnceLock<Result<Pam, String>> = OnceLock::new();
    PAM.get_or_init(load).as_ref().map_err(|e| e.clone())
}

fn load() -> Result<Pam, String> {
    const MISSING: &str = "PAM is not installed on this system (libpam.so.0 was not found).";
    // SAFETY: plain dlopen/dlsym of fixed names; each symbol is cast to its documented signature.
    unsafe {
        let lib = libc::dlopen(c"libpam.so.0".as_ptr(), libc::RTLD_NOW);
        if lib.is_null() {
            return Err(MISSING.to_string());
        }
        let sym = |name: &CStr| -> Result<*mut c_void, String> {
            let p = libc::dlsym(lib, name.as_ptr());
            if p.is_null() { Err(MISSING.to_string()) } else { Ok(p) }
        };
        Ok(Pam {
            start: std::mem::transmute::<*mut c_void, _>(sym(c"pam_start")?),
            authenticate: std::mem::transmute::<*mut c_void, _>(sym(c"pam_authenticate")?),
            end: std::mem::transmute::<*mut c_void, _>(sym(c"pam_end")?),
            strerror: std::mem::transmute::<*mut c_void, _>(sym(c"pam_strerror")?),
            start_confdir: {
                let p = libc::dlsym(lib, c"pam_start_confdir".as_ptr());
                if p.is_null() { None } else { Some(std::mem::transmute::<*mut c_void, _>(p)) }
            },
        })
    }
}

/// What the conversation knows: the password (NUL ended) and whether it was already given.
struct Talk {
    password: Vec<u8>,
    given: bool,
}

impl Drop for Talk {
    // Every way out of `run` wipes the copy.
    fn drop(&mut self) {
        wipe(&mut self.password);
    }
}

/// Answers the first hidden prompt with the password. A visible prompt, a second hidden one or
/// anything unknown is refused, so the password goes to nothing that did not ask for it.
unsafe extern "C" fn converse(count: c_int, msgs: *mut *const Message, out: *mut *mut Response, data: *mut c_void) -> c_int {
    if count <= 0 || count > 32 || msgs.is_null() || out.is_null() || data.is_null() {
        return PAM_CONV_ERR;
    }
    // SAFETY: PAM hands over `count` messages; the array is calloc'd for PAM to free. `data` is our Talk.
    unsafe {
        let talk = &mut *(data as *mut Talk);
        let n = count as usize;
        let replies = libc::calloc(n, std::mem::size_of::<Response>()) as *mut Response;
        if replies.is_null() {
            return PAM_CONV_ERR;
        }
        for i in 0..n {
            let style = (**msgs.add(i)).style;
            match style {
                PAM_PROMPT_ECHO_OFF if !talk.given => {
                    let copy = libc::strdup(talk.password.as_ptr() as *const c_char);
                    if copy.is_null() {
                        free_replies(replies, i);
                        return PAM_CONV_ERR;
                    }
                    talk.given = true;
                    (*replies.add(i)).resp = copy;
                }
                PAM_ERROR_MSG | PAM_TEXT_INFO => {}
                _ => {
                    free_replies(replies, i);
                    return PAM_CONV_ERR;
                }
            }
        }
        *out = replies;
    }
    PAM_SUCCESS
}

/// Frees the answers made so far, wiping each password copy first.
unsafe fn free_replies(replies: *mut Response, upto: usize) {
    // SAFETY: `replies` holds at least `upto` entries, each null or a strdup'd string.
    unsafe {
        for i in 0..upto {
            let p = (*replies.add(i)).resp;
            if !p.is_null() {
                let len = libc::strlen(p);
                for k in 0..len {
                    std::ptr::write_volatile(p.add(k), 0);
                }
                libc::free(p as *mut c_void);
            }
        }
        libc::free(replies as *mut c_void);
    }
}

fn wipe(b: &mut [u8]) {
    for c in b.iter_mut() {
        // SAFETY: a plain write to a byte we own.
        unsafe { std::ptr::write_volatile(c, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Whether this system has a PAM `login` service to check against.
fn service_present() -> bool {
    SERVICE_FILES.iter().any(|f| std::path::Path::new(f).exists())
}

/// The account name the gate asks for: the one running Termurion.
pub fn account_name() -> Result<String, String> {
    // SAFETY: getpwuid_r into a buffer we own; the name is copied out before it goes away.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as c_char; 4096];
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let rc = libc::getpwuid_r(libc::getuid(), &mut pwd, buf.as_mut_ptr(), buf.len(), &mut found);
        if rc != 0 || found.is_null() {
            return Err("the account running Termurion has no entry in the password database.".to_string());
        }
        Ok(CStr::from_ptr(pwd.pw_name).to_string_lossy().into_owned())
    }
}

/// Whether a password can be checked here at all: a PAM `login` service and a loadable libpam.
/// Asked before the window opens, so the person is told why at once.
pub fn ready() -> Result<(), String> {
    if !service_present() {
        return Err("this system has no PAM login service to check the password with.".to_string());
    }
    pam().map(|_| ())
}

/// Checks `password` for `user` with PAM. Takes as long as the system's failure delay on a wrong one.
pub fn check(user: &str, password: &Secret) -> Result<(), String> {
    ready()?;
    run(pam()?, SERVICE, None, user, password)
}

/// One pam_start, pam_authenticate and pam_end for `service` (in `confdir` when given).
fn run(pam: &Pam, service: &[u8], confdir: Option<&CStr>, user: &str, password: &Secret) -> Result<(), String> {
    let user = CString::new(user).map_err(|_| "the account name is not valid.".to_string())?;
    let start_in = match confdir {
        Some(dir) => Some((dir, pam.start_confdir.ok_or_else(|| "this libpam cannot start from a scratch folder.".to_string())?)),
        None => None,
    };
    // A NUL can never be in a password; refuse before any copy is made.
    let bytes = password.expose().as_bytes();
    if bytes.contains(&0) {
        return Err(WRONG.to_string());
    }
    let mut talk = Talk { password: Vec::with_capacity(bytes.len() + 1), given: false };
    talk.password.extend_from_slice(bytes);
    talk.password.push(0);
    let conv = Conv { conv: converse, data: &mut talk as *mut Talk as *mut c_void };
    let mut handle: *mut c_void = std::ptr::null_mut();
    // SAFETY: the signatures match libpam's; `conv` and `talk` outlive every call that can use them.
    let result = unsafe {
        let rc = match start_in {
            Some((dir, f)) => f(service.as_ptr() as *const c_char, user.as_ptr(), &conv, dir.as_ptr(), &mut handle),
            None => (pam.start)(service.as_ptr() as *const c_char, user.as_ptr(), &conv, &mut handle),
        };
        if rc != PAM_SUCCESS {
            Err(reason(None, rc, pam))
        } else {
            let rc = (pam.authenticate)(handle, PAM_SILENT | PAM_DISALLOW_NULL_AUTHTOK);
            let r = if rc == PAM_SUCCESS { Ok(()) } else { Err(reason(Some(handle), rc, pam)) };
            (pam.end)(handle, rc);
            r
        }
    };
    drop(talk);
    result
}

/// The words for a PAM failure: Windows' for a wrong password, PAM's own text otherwise.
unsafe fn reason(handle: Option<*mut c_void>, rc: c_int, pam: &Pam) -> String {
    if rc == PAM_AUTH_ERR {
        return WRONG.to_string();
    }
    // SAFETY: pam_strerror accepts a null handle and returns a static string.
    let text = unsafe {
        let p = (pam.strerror)(handle.unwrap_or(std::ptr::null_mut()), rc);
        if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
    };
    let text = text.trim_end_matches(['.', ' ']).to_lowercase();
    if text.is_empty() { format!("PAM failed with code {rc}.") } else { format!("{text}.") }
}

/// Never reached with a biometric: there is no gate but the password.
pub fn hello(_owner: isize) -> Option<Answer> {
    None
}

/// The password is asked in Termurion's own window (the app drives `Gate` from it), so a caller
/// that cannot show that window gets a reason, never a silent grant.
pub fn prompt(_owner: isize) -> Answer {
    Answer::Failed("the password prompt needs Termurion's own window, which was not open.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::user_auth::{Gate, Next, THREE_WRONG, TRIES};

    fn random_tail() -> String {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        format!("{:x}{:x}", t, std::process::id())
    }

    /// The real account is only asked when the runner opts in: a wrong password is a line in the
    /// journal and a count against any lockout policy of the machine.
    fn real_account_allowed() -> bool {
        if std::env::var("TERMURION_PAM_TEST").as_deref() != Ok("1") {
            println!("skipped: set TERMURION_PAM_TEST=1 to check a wrong password against the real account");
            return false;
        }
        if ready().is_err() {
            println!("skipped: no PAM login service or libpam here");
            return false;
        }
        true
    }

    #[test]
    fn a_wrong_password_fails() {
        if !real_account_allowed() {
            return;
        }
        let user = account_name().expect("the current account");
        let bad = Secret::new(format!("not-the-password-{}", random_tail()));
        assert_eq!(check(&user, &bad), Err(WRONG.to_string()));
    }

    #[test]
    fn a_missing_user_is_not_a_grant() {
        if !real_account_allowed() {
            return;
        }
        let bad = Secret::new(format!("not-the-password-{}", random_tail()));
        assert!(check("termurion-no-such-account", &bad).is_err());
    }

    #[test]
    fn a_password_with_a_nul_is_refused() {
        assert!(run_nothing("a\0b").is_err());
    }

    fn run_nothing(pw: &str) -> Result<(), String> {
        match pam() {
            Ok(p) => run(p, b"termurion-test\0", None, "anyone", &Secret::new(pw.to_string())),
            Err(_) => Err(WRONG.to_string()),
        }
    }

    /// A scratch PAM service folder with one `auth` line using `module`; None (with the reason
    /// printed) where libpam or the module is missing.
    fn scratch_service(module: &str) -> Option<(std::path::PathBuf, &'static Pam)> {
        let p = match pam() {
            Ok(p) if p.start_confdir.is_some() => p,
            _ => {
                println!("skipped: this libpam has no pam_start_confdir");
                return None;
            }
        };
        let dirs = ["/lib/x86_64-linux-gnu/security", "/usr/lib/x86_64-linux-gnu/security", "/lib64/security", "/usr/lib64/security", "/usr/lib/security", "/lib/security"];
        let Some(path) = dirs.iter().map(|d| std::path::Path::new(d).join(module)).find(|m| m.exists()) else {
            println!("skipped: {module} was not found");
            return None;
        };
        let dir = std::env::temp_dir().join(format!("termurion-pam-{}-{}", std::process::id(), random_tail()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("termurion-test"), format!("auth required {}\n", path.display())).unwrap();
        Some((dir, p))
    }

    fn through(module: &str) -> Option<Result<(), String>> {
        let (dir, p) = scratch_service(module)?;
        let confdir = CString::new(dir.to_str().unwrap()).unwrap();
        let r = run(p, b"termurion-test\0", Some(&confdir), "anyone", &Secret::new("whatever".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
        Some(r)
    }

    /// The whole FFI path (start, conversation setup, authenticate, end) against a scratch service.
    #[test]
    fn pam_deny_fails_and_pam_permit_grants() {
        if let Some(r) = through("pam_deny.so") {
            assert!(r.is_err(), "pam_deny granted");
        }
        if let Some(r) = through("pam_permit.so") {
            assert_eq!(r, Ok(()));
        }
    }

    /// A libpam with no pam_start_confdir: the scratch-folder start is refused before the password
    /// is copied anywhere (the copy is also wiped on every return, by Talk's Drop).
    #[test]
    fn an_old_libpam_is_refused_before_the_password_is_copied() {
        let Ok(p) = pam() else { return };
        let old = Pam { start: p.start, authenticate: p.authenticate, end: p.end, strerror: p.strerror, start_confdir: None };
        let r = run(&old, b"termurion-test\0", Some(c"/nonexistent"), "anyone", &Secret::new("x".into()));
        assert_eq!(r, Err("this libpam cannot start from a scratch folder.".to_string()));
    }

    fn message(style: c_int) -> Message {
        Message { style, msg: c"Password:".as_ptr() }
    }

    /// Calls the conversation as PAM would; the reply text when it answered with one.
    fn ask(talk: &mut Talk, style: c_int) -> Result<Option<String>, c_int> {
        let m = message(style);
        let mut list: [*const Message; 1] = [&m];
        let mut out: *mut Response = std::ptr::null_mut();
        // SAFETY: valid message list and Talk; the reply array is freed below.
        unsafe {
            let rc = converse(1, list.as_mut_ptr(), &mut out, talk as *mut Talk as *mut c_void);
            if rc != PAM_SUCCESS {
                return Err(rc);
            }
            let r = (*out).resp;
            let text = if r.is_null() { None } else { Some(CStr::from_ptr(r).to_string_lossy().into_owned()) };
            free_replies(out, 1);
            Ok(text)
        }
    }

    #[test]
    fn only_the_first_hidden_prompt_gets_the_password() {
        let mut talk = Talk { password: b"secret\0".to_vec(), given: false };
        assert_eq!(ask(&mut talk, PAM_PROMPT_ECHO_ON), Err(PAM_CONV_ERR), "a visible prompt never gets it");
        assert_eq!(ask(&mut talk, PAM_PROMPT_ECHO_OFF), Ok(Some("secret".to_string())));
        assert_eq!(ask(&mut talk, PAM_PROMPT_ECHO_OFF), Err(PAM_CONV_ERR), "a second hidden prompt (an OTP) gets nothing");
        assert_eq!(ask(&mut talk, PAM_TEXT_INFO), Ok(None));
    }

    /// The module never puts the password in a log line or a formatted string.
    #[test]
    fn the_password_is_never_logged() {
        let src = include_str!("user_auth.rs");
        let prod = &src[..src.find("#[cfg(test)]").unwrap()];
        for (n, line) in prod.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if code.contains("dlog!") || code.contains("println!") || code.contains("eprintln!") || code.contains("dbg!") {
                panic!("line {} logs: {line}", n + 1);
            }
            if code.contains("format!") || code.contains("write!") {
                assert!(!code.contains("pw") && !code.contains("password") && !code.contains("expose"), "line {} formats a password: {line}", n + 1);
            }
        }
    }

    /// A fake checker says wrong every time: the gate asks three times and never a fourth.
    #[test]
    fn three_failures_fail_the_gate() {
        let mut gate = Gate::default();
        let mut shown = 0;
        let answer = loop {
            shown += 1;
            assert!(shown <= TRIES, "a fourth prompt was shown");
            match gate.answer(Err(WRONG.to_string())) {
                Next::Again => {}
                Next::Done(a) => break a,
            }
        };
        assert_eq!(shown, 3);
        assert!(matches!(answer, Answer::Failed(t) if t == THREE_WRONG));
    }

    #[test]
    fn a_right_password_ends_the_gate_and_a_missing_pam_does_not_retry() {
        assert!(matches!(Gate::default().answer(Ok(())), Next::Done(Answer::Granted)));
        let mut g = Gate::default();
        assert!(matches!(g.answer(Err(WRONG.to_string())), Next::Again));
        assert!(matches!(g.answer(Err("PAM is not installed".to_string())), Next::Done(Answer::Failed(t)) if t == "PAM is not installed"));
    }
}
