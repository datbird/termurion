//! The Windows privacy values, from the registry.

use super::super::privacy::Privacy;

pub fn read() -> Privacy {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let dword = |root: HKEY, key: &str, name: &str| -> Option<u32> {
        let (key, name) = (wide(key), wide(name));
        let (mut data, mut bytes) = (0u32, 4u32);
        let rc = unsafe { RegGetValueW(root, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut data as *mut u32 as *mut _, &mut bytes) };
        (rc == ERROR_SUCCESS).then_some(data)
    };
    let string = |root: HKEY, key: &str, name: &str| -> Option<String> {
        let (key, name) = (wide(key), wide(name));
        let mut buf = vec![0u16; 64];
        let mut bytes = (buf.len() * 2) as u32;
        let rc = unsafe { RegGetValueW(root, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut _, &mut bytes) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    };
    const STORE: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\location";
    Privacy {
        toast: dword(HKEY_CURRENT_USER, "Software\\Microsoft\\Windows\\CurrentVersion\\PushNotifications", "ToastEnabled"),
        machine: string(HKEY_LOCAL_MACHINE, STORE, "Value"),
        user: string(HKEY_CURRENT_USER, STORE, "Value"),
        non_packaged: string(HKEY_CURRENT_USER, &format!("{STORE}\\NonPackaged"), "Value"),
    }
}
