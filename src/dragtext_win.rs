//! Dragging selected text out of Tandem: an OLE drag source offering CF_UNICODETEXT.
//! `ole32.dll` is a load-time import of the `windows` crate (a KnownDLL), not a runtime load.

use windows::core::{implement, Result, BOOL, HRESULT};
use windows::Win32::Foundation::{GlobalFree, DATA_S_SAMEFORMATETC, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, DV_E_FORMATETC, E_NOTIMPL, E_OUTOFMEMORY, OLE_E_ADVISENOTSUPPORTED, S_OK};
use windows::Win32::System::Com::{IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize, CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::SHCreateStdEnumFmtEtc;

/// The one format offered.
fn unicode_format() -> FORMATETC {
    FORMATETC { cfFormat: CF_UNICODETEXT.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0 as u32, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
}

/// Whether a request names CF_UNICODETEXT in global memory.
fn wants_text(f: &FORMATETC) -> bool {
    f.cfFormat == CF_UNICODETEXT.0 && f.dwAspect == DVASPECT_CONTENT.0 as u32 && f.tymed & TYMED_HGLOBAL.0 as u32 != 0
}

#[implement(IDataObject)]
struct TextData {
    /// UTF-16 with the terminating NUL.
    units: Vec<u16>,
}

impl IDataObject_Impl for TextData_Impl {
    fn GetData(&self, format: *const FORMATETC) -> Result<STGMEDIUM> {
        let f = unsafe { format.as_ref() }.ok_or(DV_E_FORMATETC)?;
        if !wants_text(f) {
            return Err(DV_E_FORMATETC.into());
        }
        let bytes = self.units.len() * 2;
        unsafe {
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes)?;
            let p = GlobalLock(h) as *mut u16;
            if p.is_null() {
                let _ = GlobalFree(Some(h));
                return Err(E_OUTOFMEMORY.into());
            }
            std::ptr::copy_nonoverlapping(self.units.as_ptr(), p, self.units.len());
            let _ = GlobalUnlock(h);
            Ok(STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: h }, pUnkForRelease: std::mem::ManuallyDrop::new(None) })
        }
    }

    fn GetDataHere(&self, _format: *const FORMATETC, _medium: *mut STGMEDIUM) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn QueryGetData(&self, format: *const FORMATETC) -> HRESULT {
        match unsafe { format.as_ref() } {
            Some(f) if wants_text(f) => S_OK,
            _ => DV_E_FORMATETC,
        }
    }

    fn GetCanonicalFormatEtc(&self, _format: *const FORMATETC, out: *mut FORMATETC) -> HRESULT {
        if let Some(o) = unsafe { out.as_mut() } {
            o.ptd = std::ptr::null_mut();
        }
        DATA_S_SAMEFORMATETC
    }

    fn SetData(&self, _format: *const FORMATETC, _medium: *const STGMEDIUM, _release: BOOL) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn EnumFormatEtc(&self, direction: u32) -> Result<IEnumFORMATETC> {
        // DATADIR_GET is 1; a target cannot set data here.
        if direction != 1 {
            return Err(E_NOTIMPL.into());
        }
        unsafe { SHCreateStdEnumFmtEtc(&[unicode_format()]) }
    }

    fn DAdvise(&self, _format: *const FORMATETC, _flags: u32, _sink: windows::core::Ref<'_, windows::Win32::System::Com::IAdviseSink>) -> Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _connection: u32) -> Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

/// A data object that serves `text` as CF_UNICODETEXT and nothing else.
pub fn text_data_object(text: &str) -> IDataObject {
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    TextData { units }.into()
}

/// What the drag source tells OLE on each pointer step: Escape cancels, the left button
/// coming up drops, else the drag goes on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Continue,
    Drop,
    Cancel,
}

pub fn drag_step(escape: bool, left_down: bool) -> Step {
    if escape {
        Step::Cancel
    } else if !left_down {
        Step::Drop
    } else {
        Step::Continue
    }
}

#[implement(IDropSource)]
struct Source;

impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        match drag_step(escape.as_bool(), keys.0 & MK_LBUTTON.0 != 0) {
            Step::Continue => S_OK,
            Step::Drop => DRAGDROP_S_DROP,
            Step::Cancel => DRAGDROP_S_CANCEL,
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Drags `text` out of the window; blocks in `DoDragDrop` until the drop ends on the UI
/// thread (it runs a modal loop and swallows the release). True when a target took it.
/// `OleInitialize` runs once per thread; a failure (the thread is already in another
/// apartment) leaves the drag to the loop's own initialization.
pub fn drag_text(text: &str) -> bool {
    thread_local! {
        static OLE: bool = unsafe { OleInitialize(None) }.is_ok();
    }
    OLE.with(|_| ());
    let data = text_data_object(text);
    let source: IDropSource = Source.into();
    let mut effect = DROPEFFECT_NONE;
    let r = unsafe { DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect) };
    r == DRAGDROP_S_DROP && effect != DROPEFFECT_NONE
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{DV_E_FORMATETC, S_OK};
    use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::{ReleaseStgMedium, CF_HDROP, CF_UNICODETEXT};

    fn format(cf: u16) -> FORMATETC {
        FORMATETC { cfFormat: cf, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0 as u32, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
    }

    #[test]
    fn data_object_serves_unicode_text() {
        let data = text_data_object("h\u{e9}");
        unsafe {
            assert_eq!(data.QueryGetData(&format(CF_UNICODETEXT.0)), S_OK);
            assert_eq!(data.QueryGetData(&format(CF_HDROP.0)), DV_E_FORMATETC);
            assert!(data.GetData(&format(CF_HDROP.0)).is_err());
            let mut medium = data.GetData(&format(CF_UNICODETEXT.0)).unwrap();
            let h = medium.u.hGlobal;
            let size = GlobalSize(h);
            let p = GlobalLock(h) as *const u16;
            let units = std::slice::from_raw_parts(p, size / 2).to_vec();
            let _ = GlobalUnlock(h);
            ReleaseStgMedium(&mut medium);
            assert_eq!(units, vec![0x68, 0xe9, 0]);
        }
        assert_eq!(drag_step(true, true), Step::Cancel);
        assert_eq!(drag_step(false, false), Step::Drop);
        assert_eq!(drag_step(false, true), Step::Continue);
    }
}
