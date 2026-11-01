//! The local clock and the alert sound.

pub fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

pub fn beep() {
    unsafe { windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(windows_sys::Win32::UI::WindowsAndMessaging::MB_OK) };
}

/// Unix seconds as local time (Windows' time zone rules, so summer time is right):
/// year, month, day, weekday (0 is Sunday), hour, minute, second. None when the time does not convert.
pub fn local_at(secs: u64) -> Option<(u16, u16, u16, u16, u16, u16, u16)> {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    // FILETIME counts 100 ns steps from 1601.
    let ticks = (secs + 11_644_473_600) * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut here: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let ok = unsafe { FileTimeToSystemTime(&ft, &mut utc) != 0 && SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut here) != 0 };
    ok.then_some((here.wYear, here.wMonth, here.wDay, here.wDayOfWeek, here.wHour, here.wMinute, here.wSecond))
}

pub fn beep_error() {
    unsafe { windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONHAND) };
}

/// This thread's kernel plus user time.
pub fn thread_cpu_time() -> std::time::Duration {
    use windows_sys::Win32::Foundation::FILETIME;
    let z = || FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut a, mut b, mut k, mut u) = (z(), z(), z(), z());
    unsafe { windows_sys::Win32::System::Threading::GetThreadTimes(windows_sys::Win32::System::Threading::GetCurrentThread(), &mut a, &mut b, &mut k, &mut u) };
    let t = |f: FILETIME| (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64;
    std::time::Duration::from_nanos((t(k) + t(u)) * 100)
}

type Local = (u16, u16, u16, u16, u16, u16, u16);

/// A local time as the SYSTEMTIME the formatting calls read.
fn systemtime(p: Local) -> windows_sys::Win32::Foundation::SYSTEMTIME {
    windows_sys::Win32::Foundation::SYSTEMTIME { wYear: p.0, wMonth: p.1, wDay: p.2, wDayOfWeek: p.3, wHour: p.4, wMinute: p.5, wSecond: p.6, wMilliseconds: 0 }
}

/// A formatting call's UTF-16 buffer as text; a call that failed (0) or gave nothing is "".
fn wide_text(buf: &[u16], n: i32) -> String {
    if n > 1 {
        String::from_utf16_lossy(&buf[..n as usize - 1])
    } else {
        String::new()
    }
}

/// The locale's short date (Windows' `DATE_SHORTDATE`) of a local date.
pub fn short_date(p: Local) -> String {
    use windows_sys::Win32::Globalization::{DATE_SHORTDATE, GetDateFormatEx};
    let (st, mut buf) = (systemtime(p), [0u16; 128]);
    let n = unsafe { GetDateFormatEx(std::ptr::null(), DATE_SHORTDATE, &st, std::ptr::null(), buf.as_mut_ptr(), buf.len() as i32, std::ptr::null()) };
    wide_text(&buf, n)
}

/// The locale's time without seconds (`TIME_NOSECONDS`) of a local time.
pub fn short_time(p: Local) -> String {
    use windows_sys::Win32::Globalization::{GetTimeFormatEx, TIME_NOSECONDS};
    let (st, mut buf) = (systemtime(p), [0u16; 128]);
    let n = unsafe { GetTimeFormatEx(std::ptr::null(), TIME_NOSECONDS, &st, std::ptr::null(), buf.as_mut_ptr(), buf.len() as i32) };
    wide_text(&buf, n)
}

/// The locale's time with seconds (`GetTimeFormatEx` with no flags) of a local time.
pub fn long_time(p: Local) -> String {
    use windows_sys::Win32::Globalization::GetTimeFormatEx;
    let (st, mut buf) = (systemtime(p), [0u16; 128]);
    let n = unsafe { GetTimeFormatEx(std::ptr::null(), 0, &st, std::ptr::null(), buf.as_mut_ptr(), buf.len() as i32) };
    wide_text(&buf, n)
}
