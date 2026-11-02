//! Windows Hello through `UserConsentVerifier` (for this window, `IUserConsentVerifierInterop`),
//! or the Windows credential prompt for the signed-in user, checked with `LogonUserW`. The typed
//! password is unpacked into a stack buffer, used once and wiped with the packed buffer.
use std::sync::atomic::{compiler_fence, Ordering};

use super::super::user_auth::Answer;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::Security::Credentials::{CredUIPromptForWindowsCredentialsW, CredUnPackAuthenticationBufferW, CREDUIWIN_ENUMERATE_CURRENT_USER, CREDUI_INFOW, CRED_PACK_PROTECTED_CREDENTIALS};
use windows_sys::Win32::Security::{LogonUserW, LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT};
use windows_sys::Win32::System::Com::CoTaskMemFree;

pub const CAPTION: &str = "Termurion";
pub const MESSAGE: &str = "Termurion wants to open the password manager.";
const ERROR_CANCELLED: u32 = 1223;
const ERROR_LOGON_FAILURE: u32 = 1326;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn wipe(b: &mut [u16]) {
    for c in b.iter_mut() {
        unsafe { std::ptr::write_volatile(c, 0) };
    }
    compiler_fence(Ordering::SeqCst);
}

/// iTerm2's reasons (`P:320-359`) where Windows has the same case, else Windows' own text.
pub fn reason(code: u32) -> String {
    match code {
        ERROR_LOGON_FAILURE => "valid credentials weren't supplied.".to_string(),
        ERROR_CANCELLED => "password entry was cancelled.".to_string(),
        _ => format!("{}.", std::io::Error::from_raw_os_error(code as i32).to_string().trim_end_matches(['.', ' ', '\r', '\n'])),
    }
}

/// "DOMAIN\user" in the user field splits into its parts when the domain came back empty;
/// "user@domain" stays whole for `LogonUserW` with no domain.
pub fn split_user(user: &[u16], domain: &[u16]) -> (Vec<u16>, Option<Vec<u16>>) {
    let user = &user[..user.iter().position(|c| *c == 0).unwrap_or(user.len())];
    let domain = &domain[..domain.iter().position(|c| *c == 0).unwrap_or(domain.len())];
    let z = |s: &[u16]| s.iter().copied().chain(Some(0)).collect::<Vec<u16>>();
    if !domain.is_empty() {
        return (z(user), Some(z(domain)));
    }
    match user.iter().position(|c| *c == u16::from(b'\\')) {
        Some(i) => (z(&user[i + 1..]), Some(z(&user[..i]))),
        None => (z(user), None),
    }
}

/// Windows Hello's answer (`UserConsentVerificationResult`): None where the device or its
/// setup is missing, so the password prompt is used instead.
pub fn hello_answer(code: i32) -> Option<Answer> {
    Some(match code {
        0 => Answer::Granted,
        6 => Answer::Canceled,
        1 | 2 => return None,
        3 => Answer::Failed("authentication is disabled by policy.".to_string()),
        4 => Answer::Failed("the authentication device is busy.".to_string()),
        5 => Answer::Failed("there were too many failed attempts.".to_string()),
        _ => Answer::Failed("Windows Hello reported an error.".to_string()),
    })
}

/// Waits for a Windows Runtime operation, handling this thread's messages meanwhile, so
/// the Hello dialog works. Termurion's own windows are frozen until it returns: winit holds
/// every event while its handler is on the stack, so they neither repaint nor take input
/// (as with the credential prompt and the file pickers; accepted, checklist "modal calls").
fn wait<T: windows::core::RuntimeType + 'static>(op: &windows_future::IAsyncOperation<T>) -> Option<T> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT};
    while op.Status().ok()? == windows_future::AsyncStatus::Started {
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        if unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        } else {
            unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 50, QS_ALLINPUT) };
        }
    }
    op.GetResults().ok()
}

/// Windows Hello over `owner`; None when it is not available (or could not be asked).
pub fn hello(owner: isize) -> Option<Answer> {
    use windows::Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability};
    use windows::Win32::System::WinRT::IUserConsentVerifierInterop;
    let available = wait(&UserConsentVerifier::CheckAvailabilityAsync().ok()?)?;
    if available != UserConsentVerifierAvailability::Available {
        return None;
    }
    let interop = windows::core::factory::<UserConsentVerifier, IUserConsentVerifierInterop>().ok()?;
    let hwnd = windows::Win32::Foundation::HWND(owner as _);
    let op: windows_future::IAsyncOperation<UserConsentVerificationResult> = unsafe { interop.RequestVerificationForWindowAsync(hwnd, &windows::core::HSTRING::from(MESSAGE)) }.ok()?;
    hello_answer(wait(&op)?.0)
}

pub fn prompt(owner: isize) -> Answer {
    let caption = wide(CAPTION);
    let message = wide(MESSAGE);
    let info = CREDUI_INFOW {
        cbSize: std::mem::size_of::<CREDUI_INFOW>() as u32,
        hwndParent: owner as _,
        pszMessageText: message.as_ptr(),
        pszCaptionText: caption.as_ptr(),
        hbmBanner: std::ptr::null_mut(),
    };
    let mut package = 0u32;
    let mut out: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut out_len = 0u32;
    let mut save = 0;
    let r = unsafe { CredUIPromptForWindowsCredentialsW(&info, 0, &mut package, std::ptr::null(), 0, &mut out, &mut out_len, &mut save, CREDUIWIN_ENUMERATE_CURRENT_USER) };
    if r == ERROR_CANCELLED {
        return Answer::Canceled;
    }
    if r != 0 {
        return Answer::Failed(reason(r));
    }
    let mut user = [0u16; 514];
    let mut domain = [0u16; 338];
    let mut password = [0u16; 514];
    let (mut ul, mut dl, mut pl) = (user.len() as u32, domain.len() as u32, password.len() as u32);
    let unpacked = unsafe { CredUnPackAuthenticationBufferW(CRED_PACK_PROTECTED_CREDENTIALS, out, out_len, user.as_mut_ptr(), &mut ul, domain.as_mut_ptr(), &mut dl, password.as_mut_ptr(), &mut pl) } != 0;
    let unpack_error = unsafe { GetLastError() };
    unsafe {
        let bytes = std::slice::from_raw_parts_mut(out as *mut u8, out_len as usize);
        for b in bytes.iter_mut() {
            std::ptr::write_volatile(b, 0);
        }
        compiler_fence(Ordering::SeqCst);
        CoTaskMemFree(out);
    }
    let answer = if !unpacked {
        Answer::Failed(reason(unpack_error))
    } else {
        let (mut u, mut d) = split_user(&user, &domain);
        let mut token: HANDLE = std::ptr::null_mut();
        let domain_ptr = d.as_ref().map_or(std::ptr::null(), |d| d.as_ptr());
        let ok = unsafe { LogonUserW(u.as_ptr(), domain_ptr, password.as_ptr(), LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, &mut token) } != 0;
        let error = unsafe { GetLastError() };
        wipe(&mut u);
        if let Some(d) = d.as_mut() {
            wipe(d);
        }
        if ok {
            unsafe { CloseHandle(token) };
            Answer::Granted
        } else {
            Answer::Failed(reason(error))
        }
    };
    wipe(&mut password);
    wipe(&mut user);
    wipe(&mut domain);
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authentication_texts_follow_iterm2() {
        assert_eq!(CAPTION, "Termurion");
        assert_eq!(MESSAGE, "Termurion wants to open the password manager.");
        assert_eq!(reason(1326), "valid credentials weren't supplied.");
        assert_eq!(reason(1223), "password entry was cancelled.");
        assert!(reason(5).ends_with('.'));
        let w = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        assert_eq!(split_user(&w("BOX\\bob"), &w("")), (w("bob"), Some(w("BOX"))));
        assert_eq!(split_user(&w("bob@example.com"), &w("")), (w("bob@example.com"), None));
        assert_eq!(split_user(&w("bob"), &w("CORP")), (w("bob"), Some(w("CORP"))));
        // Windows Hello: verified, canceled, and a missing device or setup falls back.
        assert!(matches!(hello_answer(0), Some(Answer::Granted)));
        assert!(matches!(hello_answer(6), Some(Answer::Canceled)));
        assert!(hello_answer(1).is_none() && hello_answer(2).is_none());
        assert!(matches!(hello_answer(5), Some(Answer::Failed(r)) if r == "there were too many failed attempts."));
    }
}
