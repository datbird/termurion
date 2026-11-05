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
        })
    }
}

/// Answers PAM's questions with the password: `data` is the password, NUL ended. Anything that
/// is not a password prompt or a plain message is refused.
unsafe extern "C" fn converse(count: c_int, msgs: *mut *const Message, out: *mut *mut Response, data: *mut c_void) -> c_int {
    if count <= 0 || count > 32 || msgs.is_null() || out.is_null() || data.is_null() {
        return PAM_CONV_ERR;
    }
    // SAFETY: PAM hands over `count` messages; the array is calloc'd for PAM to free.
    unsafe {
        let n = count as usize;
        let replies = libc::calloc(n, std::mem::size_of::<Response>()) as *mut Response;
        if replies.is_null() {
            return PAM_CONV_ERR;
        }
        for i in 0..n {
            let style = (**msgs.add(i)).style;
            match style {
                PAM_PROMPT_ECHO_OFF | PAM_PROMPT_ECHO_ON => {
                    let copy = libc::strdup(data as *const c_char);
                    if copy.is_null() {
                        free_replies(replies, i);
                        return PAM_CONV_ERR;
                    }
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

/// Checks `password` for `user` with PAM. Takes as long as the system's failure delay on a wrong one.
pub fn check(user: &str, password: &Secret) -> Result<(), String> {
    if !service_present() {
        return Err("this system has no PAM login service to check the password with.".to_string());
    }
    let pam = pam()?;
    let user = CString::new(user).map_err(|_| "the account name is not valid.".to_string())?;
    let mut pw = CString::new(password.expose()).map_err(|_| WRONG.to_string())?.into_bytes_with_nul();
    let conv = Conv { conv: converse, data: pw.as_mut_ptr() as *mut c_void };
    let mut handle: *mut c_void = std::ptr::null_mut();
    // SAFETY: the signatures match libpam's; `conv` and `pw` outlive every call that can use them.
    let result = unsafe {
        let rc = (pam.start)(SERVICE.as_ptr() as *const c_char, user.as_ptr(), &conv, &mut handle);
        if rc != PAM_SUCCESS {
            Err(reason(None, rc, pam))
        } else {
            let rc = (pam.authenticate)(handle, PAM_SILENT | PAM_DISALLOW_NULL_AUTHTOK);
            let r = if rc == PAM_SUCCESS { Ok(()) } else { Err(reason(Some(handle), rc, pam)) };
            (pam.end)(handle, rc);
            r
        }
    };
    wipe(&mut pw);
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

    #[test]
    fn a_wrong_password_fails() {
        if !service_present() || pam().is_err() {
            println!("skipped: no PAM login service or libpam here");
            return;
        }
        let user = account_name().expect("the current account");
        let bad = Secret::new(format!("not-the-password-{}", random_tail()));
        assert_eq!(check(&user, &bad), Err(WRONG.to_string()));
    }

    #[test]
    fn a_missing_user_is_not_a_grant() {
        if !service_present() || pam().is_err() {
            println!("skipped: no PAM login service or libpam here");
            return;
        }
        let bad = Secret::new(format!("not-the-password-{}", random_tail()));
        assert!(check("termurion-no-such-account", &bad).is_err());
    }

    #[test]
    fn a_password_with_a_nul_is_refused() {
        assert!(check("anyone", &Secret::new("a\0b".to_string())).is_err());
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
