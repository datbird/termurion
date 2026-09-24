//! Loads the ConPTY functions, from the shipped conpty.dll or from Windows.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Console::{
    COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole,
};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::core::HRESULT;

pub type CreateFn = unsafe extern "system" fn(COORD, HANDLE, HANDLE, u32, *mut HPCON) -> HRESULT;
pub type ResizeFn = unsafe extern "system" fn(HPCON, COORD) -> HRESULT;
pub type CloseFn = unsafe extern "system" fn(HPCON);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// conpty.dll and OpenConsole.exe in this folder.
    Shipped(PathBuf),
    /// The copy built into Windows.
    Inbox,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Source::Shipped(_) => "shipped",
            Source::Inbox => "inbox",
        }
    }
}

#[derive(Clone)]
pub struct Api {
    pub source: Source,
    pub create: CreateFn,
    pub resize: ResizeFn,
    pub close: CloseFn,
}

pub fn load(want: &Source) -> Result<Api, String> {
    match want {
        Source::Inbox => Ok(Api {
            source: Source::Inbox,
            create: CreatePseudoConsole,
            resize: ResizePseudoConsole,
            close: ClosePseudoConsole,
        }),
        Source::Shipped(dir) => load_shipped(dir),
    }
}

fn load_shipped(dir: &Path) -> Result<Api, String> {
    let dll = dir.join("conpty.dll");
    // conpty.dll starts OpenConsole.exe from its own folder. Without the exe, ConPTY
    // quietly uses the Windows copy, so a missing exe is an error here.
    for file in [dll.clone(), dir.join("OpenConsole.exe")] {
        if !file.is_file() {
            return Err(format!("{} not found", file.display()));
        }
    }
    let wide: Vec<u16> = dll.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let module = LoadLibraryW(wide.as_ptr());
        if module.is_null() {
            return Err(format!("LoadLibraryW({}) failed: {}", dll.display(), std::io::Error::last_os_error()));
        }
        let get = |name: &str| {
            let cname = format!("{name}\0");
            GetProcAddress(module, cname.as_ptr()).ok_or_else(|| format!("{} has no {name}", dll.display()))
        };
        let create = get("ConptyCreatePseudoConsole")?;
        let resize = get("ConptyResizePseudoConsole")?;
        let close = get("ConptyClosePseudoConsole")?;
        type Raw = unsafe extern "system" fn() -> isize;
        Ok(Api {
            source: Source::Shipped(dir.to_path_buf()),
            create: std::mem::transmute::<Raw, CreateFn>(create),
            resize: std::mem::transmute::<Raw, ResizeFn>(resize),
            close: std::mem::transmute::<Raw, CloseFn>(close),
        })
    }
}

/// The repo's vendor\conpty folder, filled by scripts\fetch-conpty.ps1.
pub fn dev_vendor_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor").join("conpty")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tandem-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn inbox_loads() {
        assert_eq!(load(&Source::Inbox).unwrap().source, Source::Inbox);
    }

    #[test]
    fn shipped_loads_from_vendor() {
        let dir = dev_vendor_dir();
        assert_eq!(load(&Source::Shipped(dir.clone())).unwrap().source, Source::Shipped(dir));
    }

    #[test]
    fn missing_dll_is_an_error() {
        let err = load(&Source::Shipped(scratch("nodll"))).err().unwrap();
        assert!(err.contains("conpty.dll not found"), "{err}");
    }

    #[test]
    fn missing_openconsole_is_an_error() {
        let dir = scratch("noexe");
        std::fs::copy(dev_vendor_dir().join("conpty.dll"), dir.join("conpty.dll")).unwrap();
        let err = load(&Source::Shipped(dir)).err().unwrap();
        assert!(err.contains("OpenConsole.exe not found"), "{err}");
    }

    #[test]
    fn labels() {
        assert_eq!(Source::Inbox.label(), "inbox");
        assert_eq!(Source::Shipped(PathBuf::new()).label(), "shipped");
    }
}
