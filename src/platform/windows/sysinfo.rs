//! The machine's facts, read from the registry.

use super::super::sysinfo::Machine;

/// Reads a text value of `HKEY_LOCAL_MACHINE` (a normal user may read these).
fn hklm_string(key: &str, name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (key, name) = (wide(key), wide(name));
    let mut buf = [0u16; 256];
    let mut bytes = std::mem::size_of_val(&buf) as u32;
    // SAFETY: both names are NUL-terminated; `buf` has `bytes` bytes.
    let rc = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut bytes) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    let text = String::from_utf16_lossy(&buf[..end]).trim().to_string();
    (!text.is_empty()).then_some(text)
}

pub fn machine() -> Machine {
    Machine {
        cpu: hklm_string(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString").unwrap_or_default(),
        product: hklm_string(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "ProductName").unwrap_or_else(|| "Windows".into()),
        build: hklm_string(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuild").unwrap_or_default(),
    }
}
