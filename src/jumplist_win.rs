//! The Jump List committed through COM (m13v1 Task 6): `ICustomDestinationList` for Tasks and the
//! custom categories, `IApplicationDestinations` for the "Remove Recent Profiles" clear. Each link
//! is an `IShellLinkW` to Tandem's own exe with `jumplist::args_string`'s arguments and
//! `PKEY_Title` set. Nothing here runs in a test: the app side hands `commit` to the pure
//! `jumplist` code as an injected writer.

use std::path::Path;

use windows::Win32::Storage::EnhancedStorage::PKEY_Title;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemAlloc, CoUninitialize};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{ApplicationDestinations, DestinationList, EnumerableObjectCollection, IApplicationDestinations, ICustomDestinationList, IShellLinkW, ShellLink};
use windows::core::{HSTRING, Interface, PWSTR};

use crate::jumplist::{Link, Model, args_string, split_args, without_removed};

/// A `VT_LPWSTR` property value holding `text` (a shell property store wants this type for a
/// title; `PROPVARIANT::from(&str)` makes a `VT_BSTR`). Freed by the value's own drop.
fn title_value(text: &str) -> windows::core::Result<PROPVARIANT> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    let memory = unsafe { CoTaskMemAlloc(bytes) } as *mut u16;
    if memory.is_null() {
        return Err(windows::core::Error::from(windows::Win32::Foundation::E_OUTOFMEMORY));
    }
    unsafe { std::ptr::copy_nonoverlapping(wide.as_ptr(), memory, wide.len()) };
    let mut value = PROPVARIANT::default();
    unsafe {
        let inner = &mut *value.Anonymous.Anonymous;
        inner.vt = VT_LPWSTR;
        inner.Anonymous.pwszVal = PWSTR(memory);
    }
    Ok(value)
}

/// One link: the exe, its arguments, the exe's icon and the title.
fn shell_link(link: &Link, exe: &Path) -> windows::core::Result<IShellLinkW> {
    let item: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)? };
    let path = HSTRING::from(exe.as_os_str());
    unsafe {
        item.SetPath(&path)?;
        item.SetArguments(&HSTRING::from(args_string(&link.args)))?;
        item.SetIconLocation(&path, 0)?;
    }
    let store: IPropertyStore = item.cast()?;
    let mut title = title_value(&link.title)?;
    let set = unsafe { store.SetValue(&PKEY_Title, &title) };
    let _ = unsafe { PropVariantClear(&mut title) };
    set?;
    unsafe { store.Commit()? };
    Ok(item)
}

fn collection(links: &[Link], exe: &Path) -> windows::core::Result<IObjectCollection> {
    let all: IObjectCollection = unsafe { CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)? };
    for l in links {
        let item = shell_link(l, exe)?;
        unsafe { all.AddObject(&item)? };
    }
    Ok(all)
}

/// The arguments of the links the person removed since the last commit (`BeginList`'s array).
fn removed_args(removed: &IObjectArray) -> Vec<Vec<String>> {
    let count = unsafe { removed.GetCount() }.unwrap_or(0).min(1024);
    let mut out = Vec::new();
    for i in 0..count {
        let Ok(link) = (unsafe { removed.GetAt::<IShellLinkW>(i) }) else { continue };
        let mut buf = vec![0u16; 32_768];
        if unsafe { link.GetArguments(&mut buf) }.is_ok() {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            out.push(split_args(&String::from_utf16_lossy(&buf[..end])));
        }
    }
    out
}

/// Replaces the Jump List with `m`: `BeginList`, the removed items dropped from the model (an
/// `AppendCategory` holding one fails the whole category), `AddUserTasks`, one `AppendCategory`
/// per category, `CommitList`; `AbortList` on every failure after `BeginList`. Returns the args
/// of the items the person had removed, which the caller keeps out of every later list.
pub fn commit(m: &Model, exe: &Path) -> Result<Vec<Vec<String>>, String> {
    // The UI thread is already an apartment (winit's OLE); this is for any other caller. Declared
    // first, so the COM objects below are released before the apartment reference goes.
    let _com = ComInit::new();
    let list: ICustomDestinationList = unsafe { CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER) }.map_err(|e| format!("Jump List: {e}"))?;
    let mut slots = 0u32;
    let removed_items: IObjectArray = unsafe { list.BeginList(&mut slots) }.map_err(|e| format!("Jump List: {e}"))?;
    let removed = removed_args(&removed_items);
    let fill = || -> windows::core::Result<()> {
        let m = without_removed(m.clone(), &removed);
        if !m.tasks.is_empty() {
            let tasks = collection(&m.tasks, exe)?;
            unsafe { list.AddUserTasks(&tasks.cast::<IObjectArray>()?)? };
        }
        for c in &m.categories {
            let links = collection(&c.links, exe)?;
            match unsafe { list.AppendCategory(&HSTRING::from(c.name.as_str()), &links.cast::<IObjectArray>()?) } {
                Ok(()) => {}
                // Windows' "Show recently opened items in Start, Jump Lists and File Explorer" is off:
                // custom categories are refused, the tasks (already added) and the commit still go.
                Err(e) if access_denied(e.code().0) => {
                    #[cfg(debug_assertions)]
                    eprintln!("Jump List: custom categories are off in Windows; committing the tasks only");
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        unsafe { list.CommitList() }
    };
    match fill() {
        Ok(()) => Ok(removed),
        Err(e) => {
            let _ = unsafe { list.AbortList() };
            Err(format!("Jump List: {e}"))
        }
    }
}

/// E_ACCESSDENIED (0x80070005): `AppendCategory` when Start_TrackDocs is 0 (13y2).
pub fn access_denied(code: i32) -> bool {
    code as u32 == 0x8007_0005
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_access_denied_skips_the_categories() {
        assert!(access_denied(0x8007_0005_u32 as i32));
        assert!(!access_denied(0x8000_4005_u32 as i32));
        // The commit still adds the tasks and commits when the categories are refused.
        let src = include_str!("jumplist_win.rs");
        let live = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        assert!(live.contains("Err(e) if access_denied(e.code().0)") && live.contains("Err(e) => return Err(e),"));
    }
}

/// One CoInitializeEx, balanced by CoUninitialize when it succeeded (S_OK or S_FALSE both count;
/// final review item 16, Task 6 M1: one apartment reference leaked per commit before).
struct ComInit(bool);

impl ComInit {
    fn new() -> ComInit {
        ComInit(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

/// Clears the destinations Windows keeps for Tandem (`IApplicationDestinations`).
pub fn clear_recent_destinations() -> Result<(), String> {
    let _com = ComInit::new();
    let all: IApplicationDestinations = unsafe { CoCreateInstance(&ApplicationDestinations, None, CLSCTX_INPROC_SERVER) }.map_err(|e| format!("Jump List: {e}"))?;
    unsafe { all.RemoveAllDestinations() }.map_err(|e| format!("Jump List: {e}"))
}
