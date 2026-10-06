//! The UI Automation provider (13v2 Task 9): Narrator, NVDA and any other UI Automation client
//! read the terminal through a Pane element (the window) holding one Text element per pane
//! (`TermControlUiaProvider` and `UiaTextRange` are the Windows Terminal precedent; iTerm2's
//! text area is `iTermTextViewAccessibilityHelper`).
//!
//! Threading: provider methods run on UI Automation's threads (free threaded) and read only
//! the last `WindowSnapshot` the window thread published, an `Arc` replaced whole under a short
//! mutex and never held across a COM call. Anything that changes Tandem (`Select`,
//! `ScrollIntoView`) is handed to `send`, which posts it to the window thread. Events are raised
//! from the window thread only. Every COM method body runs in `guard`: a panic inside an
//! `extern "system"` method aborts the process, so it becomes `E_FAIL`. A pane that is no longer
//! in the snapshot answers `UIA_E_ELEMENTNOTAVAILABLE`; ranges clamp to the current text.
//! Counts and lengths a client passes are bounded (`FindText`, `Move*`, `GetText`).
//!
//! Positions are UTF-16 offsets into the pane's `access::Text`; `rect` is (left, top, right,
//! bottom) in screen pixels, `origin` the screen position of the cell at text row
//! `first_visible_row`, column 0.

#![allow(non_upper_case_globals)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use windows::core::{implement, AsImpl, Error, Interface, Ref, Result, BOOL, BSTR, HRESULT, IUnknown};
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_OUTOFMEMORY, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayDestroy, SafeArrayPutElement};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_I4, VT_R8, VT_UNKNOWN};
use windows::Win32::UI::Accessibility::*;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::WM_GETOBJECT;

use crate::access::{self, Unit};

/// Subclass id ("uias").
const SUBCLASS_ID: usize = 0x7561_6973;
/// The longest `FindText` needle, in UTF-16 units.
const MAX_NEEDLE: usize = 1024;
/// The most unit comparisons one `FindText` makes.
const FIND_BUDGET: usize = 20_000_000;
/// The longest announcement handed to a client, in characters.
const MAX_ANNOUNCE_CHARS: usize = 100_000;

/// One pane as the provider sees it (screen coordinates).
#[derive(Clone, Debug, Default)]
pub struct PaneSnapshot {
    pub id: u64,
    pub text: access::Text,
    /// The selection as text offsets (empty when `.0 == .1`).
    pub selection: (usize, usize),
    pub cursor: usize,
    /// (left, top, right, bottom).
    pub rect: (i32, i32, i32, i32),
    /// One cell's width and height in pixels.
    pub cell: (f32, f32),
    /// Where text row `first_visible_row`, column 0 sits on screen.
    pub origin: (i32, i32),
    pub first_visible_row: usize,
    pub visible_rows: usize,
    pub focused: bool,
    /// The pane is at a password prompt (13v2 B5/P12): the text is only the prompt, nothing is
    /// announced, and `Select` and `ScrollIntoView` are refused.
    pub secure: bool,
}

#[derive(Clone, Debug, Default)]
pub struct WindowSnapshot {
    pub title: String,
    /// (left, top, right, bottom).
    pub rect: (i32, i32, i32, i32),
    pub panes: Vec<PaneSnapshot>,
}

/// What a client asks of Tandem; posted to the window thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Select { pane: u64, start: usize, end: usize },
    ScrollIntoView { pane: u64, offset: usize },
}

/// Every COM method body runs in this: a panic becomes `E_FAIL`.
pub(crate) fn guard<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => Err(Error::from_hresult(E_FAIL)),
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn hr(code: u32) -> Error {
    Error::from_hresult(HRESULT(code as i32))
}

fn unavailable() -> Error {
    hr(UIA_E_ELEMENTNOTAVAILABLE)
}

fn none<T>() -> Result<T> {
    Err(Error::empty())
}

// ---------------------------------------------------------------------------------------------
// Pure range arithmetic (over `access::Text`)

fn is_lead(u: u16) -> bool {
    (0xD800..0xDC00).contains(&u)
}

fn is_trail(u: u16) -> bool {
    (0xDC00..0xE000).contains(&u)
}

/// A range clamped to the text: both ends inside it, never between the halves of a surrogate
/// pair, start not after end.
fn normalize(t: &access::Text, s: usize, e: usize) -> (usize, usize) {
    let len = t.text.len();
    let floor = |o: usize| if o > 0 && o < len && is_trail(t.text[o]) && is_lead(t.text[o - 1]) { o - 1 } else { o };
    let ceil = |o: usize| if o > 0 && o < len && is_trail(t.text[o]) && is_lead(t.text[o - 1]) { o + 1 } else { o };
    let s = floor(s.min(len));
    let e = ceil(e.min(len)).max(s);
    (s, e)
}

fn unit_of(u: TextUnit) -> Result<Unit> {
    Ok(match u {
        TextUnit_Character => Unit::Character,
        TextUnit_Format | TextUnit_Word => Unit::Word,
        TextUnit_Paragraph | TextUnit_Line => Unit::Line,
        TextUnit_Page | TextUnit_Document => Unit::Document,
        _ => return Err(Error::from_hresult(E_INVALIDARG)),
    })
}

fn fold(u: u16) -> u16 {
    char::from_u32(u as u32)
        .and_then(|c| {
            let mut l = c.to_lowercase();
            let f = l.next()?;
            (l.next().is_none() && (f as u32) <= 0xFFFF).then_some(f as u16)
        })
        .unwrap_or(u)
}

/// The first (or last, `backward`) occurrence of `needle` inside `t[lo..hi]`; at most
/// `FIND_BUDGET` comparisons, then not found.
fn find(t: &[u16], lo: usize, hi: usize, needle: &[u16], backward: bool, nocase: bool) -> Option<(usize, usize)> {
    let n = needle.len();
    let hi = hi.min(t.len());
    if n == 0 || n > MAX_NEEDLE || lo >= hi || n > hi - lo {
        return None;
    }
    let same = |a: u16, b: u16| if nocase { fold(a) == fold(b) } else { a == b };
    let mut budget = FIND_BUDGET;
    let last = hi - n;
    let mut i = if backward { last } else { lo };
    loop {
        let inside_pair = i > 0 && i < t.len() && is_trail(t[i]) && is_lead(t[i - 1]);
        if !inside_pair {
            let mut k = 0;
            while k < n && same(t[i + k], needle[k]) {
                k += 1;
            }
            budget = budget.saturating_sub(k + 1);
            if k == n {
                return Some((i, i + n));
            }
            if budget == 0 {
                return None;
            }
        }
        if backward {
            if i == lo {
                return None;
            }
            i -= 1;
        } else {
            if i == last {
                return None;
            }
            i += 1;
        }
    }
}

/// One rectangle (left, top, width, height) per visible row the range touches; a degenerate
/// range gives one cell-wide rectangle at its position when that row is visible.
fn rects(p: &PaneSnapshot, s: usize, e: usize) -> Vec<[f64; 4]> {
    let t = &p.text;
    let (cw, ch) = (p.cell.0 as f64, p.cell.1 as f64);
    let rows = t.breaks.len();
    let row_y = |r: usize| (p.origin.1 as f64) + (r as f64 - p.first_visible_row as f64) * ch;
    let visible = |r: usize| r >= p.first_visible_row && r - p.first_visible_row < p.visible_rows;
    let mut out = Vec::new();
    if s >= e {
        let (mut r, mut c) = access::cell_of(t, s);
        if r >= rows && rows > 0 {
            r = rows - 1;
            c = t.cells.last().map_or(0, |x| x.1 + 1);
        }
        if visible(r) {
            out.push([p.origin.0 as f64 + c as f64 * cw, row_y(r), cw, ch]);
        }
        return out;
    }
    let first = access::line_of(t, s);
    let last = access::line_of(t, e - 1);
    for r in first..=last.min(rows.saturating_sub(1)) {
        if out.len() >= p.visible_rows || !visible(r) {
            continue;
        }
        let (rs, re) = access::line_range(t, r);
        let (a, b) = (s.max(rs), e.min(re));
        if a >= b {
            continue;
        }
        let c0 = t.cells[a].1;
        let c1 = t.cells[b - 1].1 + 1;
        out.push([p.origin.0 as f64 + c0 as f64 * cw, row_y(r), (c1 - c0) as f64 * cw, ch]);
    }
    out
}

/// The offsets of the visible rows.
fn visible_range(p: &PaneSnapshot) -> (usize, usize) {
    let t = &p.text;
    let n = t.breaks.len();
    if n == 0 || p.first_visible_row >= n || p.visible_rows == 0 {
        return (t.text.len(), t.text.len());
    }
    let last = (p.first_visible_row + p.visible_rows).min(n) - 1;
    (access::line_range(t, p.first_visible_row).0, access::line_range(t, last).1)
}

// ---------------------------------------------------------------------------------------------
// SAFEARRAY helpers

fn vector(vt: VARENUM, n: usize) -> Result<*mut SAFEARRAY> {
    // SAFETY: a plain allocation; null is checked.
    let psa = unsafe { SafeArrayCreateVector(vt, 0, n as u32) };
    if psa.is_null() {
        Err(Error::from_hresult(E_OUTOFMEMORY))
    } else {
        Ok(psa)
    }
}

fn fill(psa: *mut SAFEARRAY, n: usize, mut put: impl FnMut(usize) -> *const c_void) -> Result<*mut SAFEARRAY> {
    for i in 0..n {
        let idx = i as i32;
        // SAFETY: psa has n elements; the pointer put gives is valid for this element's type.
        if let Err(e) = unsafe { SafeArrayPutElement(psa, &idx, put(i)) } {
            // SAFETY: freeing the array just made.
            let _ = unsafe { SafeArrayDestroy(psa) };
            return Err(e);
        }
    }
    Ok(psa)
}

fn ranges_array(items: &[ITextRangeProvider]) -> Result<*mut SAFEARRAY> {
    let psa = vector(VT_UNKNOWN, items.len())?;
    // For VT_UNKNOWN the element is the interface pointer itself (AddRef'd by the array).
    fill(psa, items.len(), |i| items[i].as_raw() as *const c_void)
}

fn doubles_array(v: &[f64]) -> Result<*mut SAFEARRAY> {
    let psa = vector(VT_R8, v.len())?;
    fill(psa, v.len(), |i| &v[i] as *const f64 as *const c_void)
}

fn ints_array(v: &[i32]) -> Result<*mut SAFEARRAY> {
    let psa = vector(VT_I4, v.len())?;
    fill(psa, v.len(), |i| &v[i] as *const i32 as *const c_void)
}

fn not_supported() -> Result<VARIANT> {
    // SAFETY: returns a process-wide singleton.
    let u: IUnknown = unsafe { UiaGetReservedNotSupportedValue()? };
    Ok(VARIANT::from(u))
}

// ---------------------------------------------------------------------------------------------
// Shared state and the provider objects

struct Shared {
    hwnd: isize,
    snap: Mutex<Arc<WindowSnapshot>>,
    send: Box<dyn Fn(Request) + Send + Sync>,
    /// The root and the pane providers handed out; each holds an `Arc<Shared>`, so `Host::drop`
    /// empties both to break the cycle.
    root: Mutex<Option<IRawElementProviderSimple>>,
    panes: Mutex<HashMap<u64, IRawElementProviderSimple>>,
}

/// The vtable of `RangeProvider`'s interface, recorded when the first range is made: a range a
/// client hands back is ours only if its vtable is this one.
static RANGE_VTBL: OnceLock<usize> = OnceLock::new();

impl Shared {
    fn snap(&self) -> Arc<WindowSnapshot> {
        lock(&self.snap).clone()
    }

    /// The snapshot and the index of pane `id` in it.
    fn pane(&self, id: u64) -> Result<(Arc<WindowSnapshot>, usize)> {
        let snap = self.snap();
        let i = snap.panes.iter().position(|p| p.id == id).ok_or_else(unavailable)?;
        Ok((snap, i))
    }

    fn root_provider(&self) -> Result<IRawElementProviderSimple> {
        lock(&self.root).clone().ok_or_else(unavailable)
    }

    fn pane_provider(self: &Arc<Self>, id: u64) -> IRawElementProviderSimple {
        let mut cache = lock(&self.panes);
        cache.entry(id).or_insert_with(|| PaneProvider { shared: self.clone(), id }.into()).clone()
    }

    fn pane_fragment(self: &Arc<Self>, id: u64) -> Result<IRawElementProviderFragment> {
        self.pane_provider(id).cast()
    }

    fn range(self: &Arc<Self>, pane: u64, s: usize, e: usize) -> ITextRangeProvider {
        let r: ITextRangeProvider = RangeProvider { shared: self.clone(), pane, r: Mutex::new((s, e)) }.into();
        // SAFETY: the first word of a COM interface pointer is its vtable pointer.
        RANGE_VTBL.get_or_init(|| unsafe { *(r.as_raw() as *const usize) });
        r
    }

    fn send(&self, r: Request) {
        (self.send)(r);
    }
}

#[implement(IRawElementProviderSimple, IRawElementProviderFragment, IRawElementProviderFragmentRoot)]
struct RootProvider {
    shared: Arc<Shared>,
}

impl IRawElementProviderSimple_Impl for RootProvider_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }

    fn GetPatternProvider(&self, _id: UIA_PATTERN_ID) -> Result<IUnknown> {
        none()
    }

    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        guard(|| {
            Ok(match id {
                UIA_ControlTypePropertyId => VARIANT::from(UIA_PaneControlTypeId.0),
                UIA_NamePropertyId => VARIANT::from(self.shared.snap().title.as_str()),
                UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId | UIA_IsEnabledPropertyId => VARIANT::from(true),
                UIA_FrameworkIdPropertyId => VARIANT::from("Tandem"),
                _ => VARIANT::default(),
            })
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        guard(|| {
            // SAFETY: asks UI Automation for the window's own provider.
            unsafe { UiaHostProviderFromHwnd(HWND(self.shared.hwnd as *mut c_void)) }
        })
    }
}

impl IRawElementProviderFragment_Impl for RootProvider_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        guard(|| {
            let snap = self.shared.snap();
            let pick = match direction {
                NavigateDirection_FirstChild => snap.panes.first(),
                NavigateDirection_LastChild => snap.panes.last(),
                _ => None,
            };
            match pick {
                Some(p) => self.shared.pane_fragment(p.id),
                None => none(),
            }
        })
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        // The window's own provider supplies the root's runtime id.
        Ok(std::ptr::null_mut())
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        guard(|| {
            let r = self.shared.snap().rect;
            Ok(UiaRect { left: r.0 as f64, top: r.1 as f64, width: (r.2 - r.0).max(0) as f64, height: (r.3 - r.1).max(0) as f64 })
        })
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        Ok(())
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        guard(|| self.shared.root_provider()?.cast())
    }
}

impl IRawElementProviderFragmentRoot_Impl for RootProvider_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        guard(|| {
            let snap = self.shared.snap();
            let hit = snap.panes.iter().find(|p| x >= p.rect.0 as f64 && x < p.rect.2 as f64 && y >= p.rect.1 as f64 && y < p.rect.3 as f64);
            match hit {
                Some(p) => self.shared.pane_fragment(p.id),
                None => none(),
            }
        })
    }

    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        guard(|| {
            let snap = self.shared.snap();
            match snap.panes.iter().find(|p| p.focused) {
                Some(p) => self.shared.pane_fragment(p.id),
                None => none(),
            }
        })
    }
}

#[implement(IRawElementProviderSimple, IRawElementProviderFragment, ITextProvider)]
struct PaneProvider {
    shared: Arc<Shared>,
    id: u64,
}

impl IRawElementProviderSimple_Impl for PaneProvider_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }

    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<IUnknown> {
        guard(|| {
            self.shared.pane(self.id)?;
            if id == UIA_TextPatternId {
                let p: ITextProvider = self.shared.pane_provider(self.id).cast()?;
                p.cast()
            } else {
                none()
            }
        })
    }

    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        guard(|| {
            let (snap, i) = self.shared.pane(self.id)?;
            let p = &snap.panes[i];
            Ok(match id {
                UIA_ControlTypePropertyId => VARIANT::from(UIA_TextControlTypeId.0),
                UIA_NamePropertyId => VARIANT::from("shell"),
                UIA_AutomationIdPropertyId => VARIANT::from(format!("TandemPane{}", p.id).as_str()),
                UIA_HasKeyboardFocusPropertyId => VARIANT::from(p.focused),
                UIA_IsKeyboardFocusablePropertyId | UIA_IsControlElementPropertyId | UIA_IsContentElementPropertyId | UIA_IsEnabledPropertyId => VARIANT::from(true),
                UIA_FrameworkIdPropertyId => VARIANT::from("Tandem"),
                _ => VARIANT::default(),
            })
        })
    }

    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        none()
    }
}

impl IRawElementProviderFragment_Impl for PaneProvider_Impl {
    fn Navigate(&self, direction: NavigateDirection) -> Result<IRawElementProviderFragment> {
        guard(|| {
            let (snap, i) = self.shared.pane(self.id)?;
            match direction {
                NavigateDirection_Parent => self.shared.root_provider()?.cast(),
                NavigateDirection_NextSibling => match snap.panes.get(i + 1) {
                    Some(p) => self.shared.pane_fragment(p.id),
                    None => none(),
                },
                NavigateDirection_PreviousSibling => match i.checked_sub(1).and_then(|j| snap.panes.get(j)) {
                    Some(p) => self.shared.pane_fragment(p.id),
                    None => none(),
                },
                _ => none(),
            }
        })
    }

    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        guard(|| {
            self.shared.pane(self.id)?;
            ints_array(&[UiaAppendRuntimeId as i32, self.id as u32 as i32, (self.id >> 32) as u32 as i32])
        })
    }

    fn BoundingRectangle(&self) -> Result<UiaRect> {
        guard(|| {
            let (snap, i) = self.shared.pane(self.id)?;
            let r = snap.panes[i].rect;
            Ok(UiaRect { left: r.0 as f64, top: r.1 as f64, width: (r.2 - r.0).max(0) as f64, height: (r.3 - r.1).max(0) as f64 })
        })
    }

    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }

    fn SetFocus(&self) -> Result<()> {
        guard(|| self.shared.pane(self.id).map(|_| ()))
    }

    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        guard(|| self.shared.root_provider()?.cast())
    }
}

impl ITextProvider_Impl for PaneProvider_Impl {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        guard(|| {
            let (snap, i) = self.shared.pane(self.id)?;
            let p = &snap.panes[i];
            let (s, e) = if p.selection.0 == p.selection.1 { (p.cursor, p.cursor) } else { p.selection };
            let (s, e) = normalize(&p.text, s, e);
            ranges_array(&[self.shared.range(self.id, s, e)])
        })
    }

    fn GetVisibleRanges(&self) -> Result<*mut SAFEARRAY> {
        guard(|| {
            let (snap, i) = self.shared.pane(self.id)?;
            let (s, e) = visible_range(&snap.panes[i]);
            ranges_array(&[self.shared.range(self.id, s, e)])
        })
    }

    fn RangeFromChild(&self, _child: Ref<IRawElementProviderSimple>) -> Result<ITextRangeProvider> {
        // The text has no children.
        Err(Error::from_hresult(E_INVALIDARG))
    }

    fn RangeFromPoint(&self, point: &UiaPoint) -> Result<ITextRangeProvider> {
        guard(|| {
            let (snap, i) = self.shared.pane(self.id)?;
            let p = &snap.panes[i];
            let t = &p.text;
            let (cw, ch) = (p.cell.0 as f64, p.cell.1 as f64);
            if !(cw > 0.0 && ch > 0.0) || !point.x.is_finite() || !point.y.is_finite() {
                return Ok(self.shared.range(self.id, 0, 0));
            }
            let col = ((point.x - p.origin.0 as f64) / cw).floor().clamp(0.0, 1.0e6) as usize;
            let rel = ((point.y - p.origin.1 as f64) / ch).floor().clamp(-1.0e6, 1.0e6) as i64;
            let row = (p.first_visible_row as i64 + rel).clamp(0, t.breaks.len().saturating_sub(1) as i64) as usize;
            let o = access::offset_of(t, row, col);
            let (s, e) = access::expand(t, o, Unit::Character);
            Ok(self.shared.range(self.id, s, e))
        })
    }

    fn DocumentRange(&self) -> Result<ITextRangeProvider> {
        guard(|| {
            let (snap, i) = self.shared.pane(self.id)?;
            Ok(self.shared.range(self.id, 0, snap.panes[i].text.text.len()))
        })
    }

    fn SupportedTextSelection(&self) -> Result<SupportedTextSelection> {
        Ok(SupportedTextSelection_Single)
    }
}

#[implement(ITextRangeProvider)]
struct RangeProvider {
    shared: Arc<Shared>,
    pane: u64,
    r: Mutex<(usize, usize)>,
}

impl RangeProvider {
    /// The current snapshot, this range's pane in it and the range clamped to its text.
    fn view(&self) -> Result<(Arc<WindowSnapshot>, usize, (usize, usize))> {
        let (snap, i) = self.shared.pane(self.pane)?;
        let (s, e) = *lock(&self.r);
        let r = normalize(&snap.panes[i].text, s, e);
        Ok((snap, i, r))
    }

    fn set(&self, r: (usize, usize)) {
        *lock(&self.r) = r;
    }

    /// Another range of this provider (one this module made), as its pane and clamped offsets.
    fn peer(&self, other: &Ref<ITextRangeProvider>) -> Result<(usize, usize)> {
        let i = other.ok()?;
        let vt = RANGE_VTBL.get().ok_or_else(|| Error::from_hresult(E_INVALIDARG))?;
        // SAFETY: the first word of a COM interface pointer is its vtable pointer.
        if unsafe { *(i.as_raw() as *const usize) } != *vt {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        // SAFETY: the vtable is RangeProvider's, so the object is one.
        let o: &RangeProvider = unsafe { i.as_impl() };
        if o.pane != self.pane {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        let (snap, idx) = self.shared.pane(self.pane)?;
        let (s, e) = *lock(&o.r);
        Ok(normalize(&snap.panes[idx].text, s, e))
    }
}

fn endpoint_of(r: (usize, usize), e: TextPatternRangeEndpoint) -> usize {
    if e == TextPatternRangeEndpoint_Start {
        r.0
    } else {
        r.1
    }
}

/// `r` with one endpoint set to `o`; the other follows when they would cross.
fn with_endpoint(r: (usize, usize), e: TextPatternRangeEndpoint, o: usize) -> (usize, usize) {
    if e == TextPatternRangeEndpoint_Start {
        (o, r.1.max(o))
    } else {
        (r.0.min(o), o)
    }
}

impl ITextRangeProvider_Impl for RangeProvider_Impl {
    fn Clone(&self) -> Result<ITextRangeProvider> {
        guard(|| {
            let (_, _, (s, e)) = self.view()?;
            Ok(self.shared.range(self.pane, s, e))
        })
    }

    fn Compare(&self, range: Ref<ITextRangeProvider>) -> Result<BOOL> {
        guard(|| {
            let (_, _, mine) = self.view()?;
            Ok(BOOL::from(self.peer(&range)? == mine))
        })
    }

    fn CompareEndpoints(&self, endpoint: TextPatternRangeEndpoint, target: Ref<ITextRangeProvider>, targetendpoint: TextPatternRangeEndpoint) -> Result<i32> {
        guard(|| {
            let (_, _, mine) = self.view()?;
            let theirs = self.peer(&target)?;
            Ok(match endpoint_of(mine, endpoint).cmp(&endpoint_of(theirs, targetendpoint)) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            })
        })
    }

    fn ExpandToEnclosingUnit(&self, unit: TextUnit) -> Result<()> {
        guard(|| {
            let u = unit_of(unit)?;
            let (snap, i, (s, _)) = self.view()?;
            self.set(access::expand(&snap.panes[i].text, s, u));
            Ok(())
        })
    }

    fn FindAttribute(&self, _id: UIA_TEXTATTRIBUTE_ID, _val: &VARIANT, _backward: BOOL) -> Result<ITextRangeProvider> {
        none()
    }

    fn FindText(&self, text: &BSTR, backward: BOOL, ignorecase: BOOL) -> Result<ITextRangeProvider> {
        guard(|| {
            let needle: &[u16] = text;
            if needle.is_empty() || needle.len() > MAX_NEEDLE {
                return none();
            }
            let (snap, i, (s, e)) = self.view()?;
            match find(&snap.panes[i].text.text, s, e, needle, backward.as_bool(), ignorecase.as_bool()) {
                Some((a, b)) => Ok(self.shared.range(self.pane, a, b)),
                None => none(),
            }
        })
    }

    fn GetAttributeValue(&self, id: UIA_TEXTATTRIBUTE_ID) -> Result<VARIANT> {
        guard(|| {
            self.view()?;
            if id == UIA_IsReadOnlyAttributeId {
                Ok(VARIANT::from(true))
            } else {
                not_supported()
            }
        })
    }

    fn GetBoundingRectangles(&self) -> Result<*mut SAFEARRAY> {
        guard(|| {
            let (snap, i, (s, e)) = self.view()?;
            let flat: Vec<f64> = rects(&snap.panes[i], s, e).into_iter().flatten().collect();
            doubles_array(&flat)
        })
    }

    fn GetEnclosingElement(&self) -> Result<IRawElementProviderSimple> {
        guard(|| {
            self.view()?;
            Ok(self.shared.pane_provider(self.pane))
        })
    }

    fn GetText(&self, maxlength: i32) -> Result<BSTR> {
        guard(|| {
            if maxlength < -1 {
                return Err(Error::from_hresult(E_INVALIDARG));
            }
            let (snap, i, (s, e)) = self.view()?;
            let t = &snap.panes[i].text.text;
            let mut take = if maxlength < 0 { e - s } else { (e - s).min(maxlength as usize) };
            if take > 0 && take < e - s && is_lead(t[s + take - 1]) {
                take -= 1;
            }
            Ok(BSTR::from_wide(&t[s..s + take]))
        })
    }

    fn Move(&self, unit: TextUnit, count: i32) -> Result<i32> {
        guard(|| {
            let u = unit_of(unit)?;
            let (snap, i, (s, e)) = self.view()?;
            let t = &snap.panes[i].text;
            if count == 0 {
                return Ok(0);
            }
            if s == e {
                let (o, m) = access::move_by(t, s, u, count);
                self.set((o, o));
                return Ok(m);
            }
            // A range spanning text moves as the unit it starts in (UI Automation: it is first
            // normalized to whole units), and ends as that unit does.
            let (us, _) = access::expand(t, s, u);
            let (o, m) = access::move_by(t, us, u, count);
            if m == 0 {
                return Ok(0);
            }
            let (ns, ne) = if o >= t.text.len() { (o, o) } else { access::expand(t, o, u) };
            self.set((ns, ne));
            Ok(m)
        })
    }

    fn MoveEndpointByUnit(&self, endpoint: TextPatternRangeEndpoint, unit: TextUnit, count: i32) -> Result<i32> {
        guard(|| {
            let u = unit_of(unit)?;
            let (snap, i, r) = self.view()?;
            let (o, m) = access::move_by(&snap.panes[i].text, endpoint_of(r, endpoint), u, count);
            self.set(with_endpoint(r, endpoint, o));
            Ok(m)
        })
    }

    fn MoveEndpointByRange(&self, endpoint: TextPatternRangeEndpoint, target: Ref<ITextRangeProvider>, targetendpoint: TextPatternRangeEndpoint) -> Result<()> {
        guard(|| {
            let (_, _, r) = self.view()?;
            let theirs = self.peer(&target)?;
            self.set(with_endpoint(r, endpoint, endpoint_of(theirs, targetendpoint)));
            Ok(())
        })
    }

    fn Select(&self) -> Result<()> {
        guard(|| {
            let (snap, i, (s, e)) = self.view()?;
            if snap.panes[i].secure {
                return Err(hr(UIA_E_INVALIDOPERATION));
            }
            self.shared.send(Request::Select { pane: self.pane, start: s, end: e });
            Ok(())
        })
    }

    fn AddToSelection(&self) -> Result<()> {
        guard(|| {
            self.view()?;
            Err(hr(UIA_E_INVALIDOPERATION))
        })
    }

    fn RemoveFromSelection(&self) -> Result<()> {
        guard(|| {
            self.view()?;
            Err(hr(UIA_E_INVALIDOPERATION))
        })
    }

    fn ScrollIntoView(&self, aligntotop: BOOL) -> Result<()> {
        guard(|| {
            let (snap, i, (s, e)) = self.view()?;
            if snap.panes[i].secure {
                return Err(hr(UIA_E_INVALIDOPERATION));
            }
            self.shared.send(Request::ScrollIntoView { pane: self.pane, offset: if aligntotop.as_bool() { s } else { e } });
            Ok(())
        })
    }

    fn GetChildren(&self) -> Result<*mut SAFEARRAY> {
        guard(|| {
            self.view()?;
            ranges_array(&[])
        })
    }
}

// ---------------------------------------------------------------------------------------------
// The host: the window subclass and the events

/// The provider tree of one window. Create and drop it on the window's thread.
pub struct Host {
    shared: Arc<Shared>,
}

unsafe extern "system" fn proc(hwnd: windows_sys::Win32::Foundation::HWND, msg: u32, wparam: usize, lparam: isize, _id: usize, data: usize) -> isize {
    if msg == WM_GETOBJECT && lparam as i32 == UiaRootObjectId {
        let r = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: data is the Arc<Shared> the Host keeps alive for as long as this subclass is installed.
            let shared = unsafe { &*(data as *const Shared) };
            let root = lock(&shared.root).clone()?;
            // SAFETY: handing the root provider to UI Automation for this window.
            Some(unsafe { UiaReturnRawElementProvider(HWND(hwnd), WPARAM(wparam), LPARAM(lparam), &root) }.0)
        }));
        if let Ok(Some(v)) = r {
            return v;
        }
    }
    // SAFETY: forwarding to the next handler.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

impl Host {
    /// Subclasses `hwnd`: `WM_GETOBJECT` for `UiaRootObjectId` returns the root provider.
    /// `send` is called on UI Automation's threads and must only post.
    pub fn install(hwnd: isize, send: Box<dyn Fn(Request) + Send + Sync>) -> std::result::Result<Host, String> {
        let shared = Arc::new(Shared { hwnd, snap: Mutex::new(Arc::default()), send, root: Mutex::new(None), panes: Mutex::new(HashMap::new()) });
        let root: IRawElementProviderSimple = RootProvider { shared: shared.clone() }.into();
        *lock(&shared.root) = Some(root);
        // SAFETY: hwnd is a live window of this thread; the refdata outlives the subclass (Drop removes it first).
        let ok = unsafe { SetWindowSubclass(hwnd as _, Some(proc), SUBCLASS_ID, Arc::as_ptr(&shared) as usize) };
        if ok == 0 {
            lock(&shared.root).take();
            return Err("SetWindowSubclass failed".into());
        }
        Ok(Host { shared })
    }

    /// Replaces the snapshot; providers of panes that are gone are disconnected.
    pub fn publish(&self, snap: WindowSnapshot) {
        let new = Arc::new(snap);
        let gone: Vec<IRawElementProviderSimple> = {
            let mut cache = lock(&self.shared.panes);
            let ids: Vec<u64> = cache.keys().filter(|id| !new.panes.iter().any(|p| p.id == **id)).copied().collect();
            ids.iter().filter_map(|id| cache.remove(id)).collect()
        };
        *lock(&self.shared.snap) = new;
        for p in &gone {
            // SAFETY: disconnecting a provider this host made.
            let _ = unsafe { UiaDisconnectProvider(p) };
        }
    }

    /// The root provider (what `WM_GETOBJECT` returns).
    pub fn root_provider(&self) -> IRawElementProviderSimple {
        lock(&self.shared.root).clone().expect("the host is live")
    }

    /// The provider events for `pane` go to: none while the pane is missing or at a password
    /// prompt (B5: nothing about a secure pane is raised).
    pub fn deliverable(&self, pane: u64) -> Option<IRawElementProviderSimple> {
        let (snap, i) = self.shared.pane(pane).ok()?;
        if snap.panes[i].secure {
            return None;
        }
        Some(self.shared.pane_provider(pane))
    }

    /// Raises a tracker event for a pane; a no-op while no client listens.
    pub fn raise(&self, pane: u64, e: &access::Event) {
        if !Self::listening() {
            return;
        }
        let Some(p) = self.deliverable(pane) else { return };
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let text = |s: &str| BSTR::from(s.chars().take(MAX_ANNOUNCE_CHARS).collect::<String>().as_str());
            // SAFETY: raising events for a provider this host made; no lock is held.
            unsafe {
                let _ = match e {
                    access::Event::TextChanged => UiaRaiseAutomationEvent(&p, UIA_Text_TextChangedEventId),
                    access::Event::SelectionChanged => UiaRaiseAutomationEvent(&p, UIA_Text_TextSelectionChangedEventId),
                    access::Event::Announce(s) => UiaRaiseNotificationEvent(&p, NotificationKind_Other, NotificationProcessing_ImportantAll, &text(s), &BSTR::from("TandemOutput")),
                    access::Event::AnnounceDeleted(s) => UiaRaiseNotificationEvent(&p, NotificationKind_ItemRemoved, NotificationProcessing_ImportantAll, &text(s), &BSTR::from("TandemDeleted")),
                };
            }
        }));
    }

    /// The pane took keyboard focus.
    pub fn raise_focus(&self, pane: u64) {
        if !Self::listening() {
            return;
        }
        if let Some(p) = self.deliverable(pane) {
            // SAFETY: as in `raise`.
            let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
                let _ = UiaRaiseAutomationEvent(&p, UIA_AutomationFocusChangedEventId);
            }));
        }
    }

    /// Whether any UI Automation client listens; the snapshot is built only then.
    pub fn listening() -> bool {
        // SAFETY: a plain query.
        unsafe { UiaClientsAreListening() }.as_bool()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let root = lock(&self.shared.root).take();
        let panes: Vec<IRawElementProviderSimple> = lock(&self.shared.panes).drain().map(|(_, p)| p).collect();
        // SAFETY: disconnecting this host's providers, releasing the window's one, then removing the subclass.
        unsafe {
            for p in &panes {
                let _ = UiaDisconnectProvider(p);
            }
            if let Some(r) = &root {
                let _ = UiaDisconnectProvider(r);
            }
            let _ = UiaReturnRawElementProvider(HWND(self.shared.hwnd as *mut c_void), WPARAM(0), LPARAM(0), None::<&IRawElementProviderSimple>);
            RemoveWindowSubclass(self.shared.hwnd as _, Some(proc), SUBCLASS_ID);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_void;
    use std::sync::{Arc, Mutex};
    use windows::core::{Interface, BSTR, IUnknown};
    use windows::Win32::Foundation::{E_FAIL, HWND};
    use windows::Win32::System::Com::SAFEARRAY;
    use windows::Win32::System::Ole::{SafeArrayDestroy, SafeArrayGetElement, SafeArrayGetUBound};
    use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_I4, VT_UNKNOWN};
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow};

    struct Win(isize);
    impl Win {
        fn new() -> Win {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            // SAFETY: a never-shown STATIC window owned by this test's thread.
            let h = unsafe { CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null()) };
            assert!(!h.is_null(), "no test window");
            Win(h as isize)
        }
    }
    impl Drop for Win {
        fn drop(&mut self) {
            // SAFETY: destroying the window this struct made.
            unsafe { DestroyWindow(self.0 as _) };
        }
    }

    fn text_of(rows: &[&str]) -> access::Text {
        let r = access::Rows { rows: rows.iter().map(|s| (s.chars().map(|c| c.to_string()).collect(), false)).collect(), cursor: (rows.len() - 1, 0), first_line: 0 };
        access::build(&r)
    }

    fn pane(id: u64, rows: &[&str]) -> PaneSnapshot {
        PaneSnapshot { id, text: text_of(rows), cursor: 0, rect: (100, 200, 600, 600), cell: (10.0, 20.0), origin: (110, 210), first_visible_row: 0, visible_rows: 20, focused: true, ..Default::default() }
    }

    fn win_snap(panes: Vec<PaneSnapshot>) -> WindowSnapshot {
        WindowSnapshot { title: "Tandem".into(), rect: (100, 200, 600, 600), panes }
    }

    type Sent = Arc<Mutex<Vec<Request>>>;

    fn host(w: &Win, snap: WindowSnapshot) -> (Host, Sent) {
        let sent: Sent = Arc::default();
        let s2 = sent.clone();
        let h = Host::install(w.0, Box::new(move |r| s2.lock().unwrap().push(r))).expect("install");
        h.publish(snap);
        (h, sent)
    }

    fn variant_i32(v: &VARIANT) -> i32 {
        // SAFETY: the tag is checked first.
        unsafe {
            assert_eq!(v.Anonymous.Anonymous.vt, VT_I4);
            v.Anonymous.Anonymous.Anonymous.lVal
        }
    }

    fn variant_bool(v: &VARIANT) -> bool {
        // SAFETY: the tag is checked first.
        unsafe {
            assert_eq!(v.Anonymous.Anonymous.vt, VT_BOOL);
            v.Anonymous.Anonymous.Anonymous.boolVal.0 != 0
        }
    }

    fn variant_str(v: &VARIANT) -> String {
        BSTR::try_from(v).unwrap().to_string()
    }

    fn first_pane(root: &IRawElementProviderSimple) -> IRawElementProviderSimple {
        let frag: IRawElementProviderFragment = root.cast().unwrap();
        // SAFETY: plain COM calls on objects of this process.
        unsafe { frag.Navigate(NavigateDirection_FirstChild).unwrap().cast().unwrap() }
    }

    /// The elements of a SAFEARRAY of interface pointers, then frees it.
    fn ranges_of(psa: *mut SAFEARRAY) -> Vec<ITextRangeProvider> {
        assert!(!psa.is_null());
        let mut out = Vec::new();
        // SAFETY: psa is a vector this test owns until SafeArrayDestroy.
        unsafe {
            let ub = SafeArrayGetUBound(psa, 1).unwrap();
            for i in 0..=ub {
                let mut p: Option<IUnknown> = None;
                SafeArrayGetElement(psa, &i, &mut p as *mut _ as *mut c_void).unwrap();
                out.push(p.unwrap().cast().unwrap());
            }
            SafeArrayDestroy(psa).unwrap();
        }
        out
    }

    fn doubles_of(psa: *mut SAFEARRAY) -> Vec<f64> {
        assert!(!psa.is_null());
        let mut out = Vec::new();
        // SAFETY: psa is a vector this test owns until SafeArrayDestroy.
        unsafe {
            if let Ok(ub) = SafeArrayGetUBound(psa, 1) {
                for i in 0..=ub {
                    let mut d = 0f64;
                    SafeArrayGetElement(psa, &i, &mut d as *mut f64 as *mut c_void).unwrap();
                    out.push(d);
                }
            }
            SafeArrayDestroy(psa).unwrap();
        }
        out
    }

    fn text_provider(root: &IRawElementProviderSimple) -> ITextProvider {
        let p = first_pane(root);
        // SAFETY: plain COM call.
        let u = unsafe { p.GetPatternProvider(UIA_TextPatternId).unwrap() };
        u.cast().unwrap()
    }

    fn get_text(r: &ITextRangeProvider, max: i32) -> String {
        // SAFETY: plain COM call.
        unsafe { r.GetText(max).unwrap().to_string() }
    }

    /// A range of the document: `start..end` through the endpoint moves.
    fn range(tp: &ITextProvider, start: i32, end_back: i32) -> ITextRangeProvider {
        // SAFETY: plain COM calls.
        unsafe {
            let r = tp.DocumentRange().unwrap();
            r.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, start).unwrap();
            if end_back != 0 {
                r.MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, end_back).unwrap();
            }
            r
        }
    }

    #[test]
    fn provider_answers_from_the_snapshot() {
        let w = Win::new();
        let (h, sent) = host(&w, win_snap(vec![pane(7, &["ab", "cd"])]));
        let root = h.root_provider();
        // SAFETY: plain COM calls on objects of this process.
        unsafe {
            assert_eq!(variant_i32(&root.GetPropertyValue(UIA_ControlTypePropertyId).unwrap()), UIA_PaneControlTypeId.0);
            let p = first_pane(&root);
            assert_eq!(variant_i32(&p.GetPropertyValue(UIA_ControlTypePropertyId).unwrap()), UIA_TextControlTypeId.0);
            assert_eq!(variant_str(&p.GetPropertyValue(UIA_NamePropertyId).unwrap()), "shell");
            let tp = text_provider(&root);
            assert_eq!(tp.SupportedTextSelection().unwrap(), SupportedTextSelection_Single);
            let doc = tp.DocumentRange().unwrap();
            assert_eq!(get_text(&doc, -1), "ab\ncd\n");
            assert_eq!(get_text(&doc, 1), "a");
            assert_eq!(doc.CompareEndpoints(TextPatternRangeEndpoint_Start, &doc, TextPatternRangeEndpoint_Start).unwrap(), 0);
            assert_eq!(doc.CompareEndpoints(TextPatternRangeEndpoint_End, &doc, TextPatternRangeEndpoint_End).unwrap(), 0);
            assert_eq!(doc.CompareEndpoints(TextPatternRangeEndpoint_Start, &doc, TextPatternRangeEndpoint_End).unwrap(), -1);
            // a range expanded by Line at offset 3
            let line = range(&tp, 3, 0);
            line.ExpandToEnclosingUnit(TextUnit_Line).unwrap();
            assert_eq!(get_text(&line, -1), "cd\n");
            // an empty selection is one degenerate range at the cursor
            let sel = ranges_of(tp.GetSelection().unwrap());
            assert_eq!(sel.len(), 1);
            assert_eq!(get_text(&sel[0], -1), "");
            assert_eq!(sel[0].CompareEndpoints(TextPatternRangeEndpoint_Start, &doc, TextPatternRangeEndpoint_Start).unwrap(), 0);
            // Select "cd"
            let cd = range(&tp, 3, -1);
            assert_eq!(get_text(&cd, -1), "cd");
            cd.Select().unwrap();
            assert_eq!(sent.lock().unwrap().clone(), vec![Request::Select { pane: 7, start: 3, end: 5 }]);
            // the rectangle of "cd": the second row, in screen coordinates
            let rects = doubles_of(cd.GetBoundingRectangles().unwrap());
            assert_eq!(rects, vec![110.0, 230.0, 20.0, 20.0]);
            // attributes
            let ro = doc.GetAttributeValue(UIA_IsReadOnlyAttributeId).unwrap();
            assert!(variant_bool(&ro));
            let other = doc.GetAttributeValue(UIA_FontNameAttributeId).unwrap();
            assert_eq!(other.Anonymous.Anonymous.vt, VT_UNKNOWN);
            assert!(doc.AddToSelection().is_err());
            assert!(doc.RemoveFromSelection().is_err());
            assert!(ranges_of(doc.GetChildren().unwrap()).is_empty());
            // enclosing element is the pane
            let enc = doc.GetEnclosingElement().unwrap();
            assert_eq!(variant_str(&enc.GetPropertyValue(UIA_NamePropertyId).unwrap()), "shell");
        }
    }

    #[test]
    fn ranges_survive_a_new_snapshot() {
        let w = Win::new();
        let (h, _s) = host(&w, win_snap(vec![pane(7, &["ab", "cd"])]));
        let root = h.root_provider();
        let tp = text_provider(&root);
        // SAFETY: plain COM call.
        let doc = unsafe { tp.DocumentRange().unwrap() };
        let tail = range(&tp, 3, -1);
        h.publish(win_snap(vec![pane(7, &["x"])]));
        assert_eq!(get_text(&doc, -1), "x\n");
        assert_eq!(get_text(&tail, -1), "");
    }

    #[test]
    fn a_closed_pane_is_not_available() {
        let w = Win::new();
        let (h, _s) = host(&w, win_snap(vec![pane(7, &["ab"]), pane(8, &["zz"])]));
        let root = h.root_provider();
        let tp = text_provider(&root);
        // SAFETY: plain COM call.
        let doc = unsafe { tp.DocumentRange().unwrap() };
        let p = first_pane(&root);
        h.publish(win_snap(vec![pane(8, &["zz"])]));
        // SAFETY: plain COM calls.
        unsafe {
            for e in [doc.GetText(-1).err().unwrap(), doc.Select().err().unwrap(), tp.DocumentRange().err().unwrap(), p.GetPropertyValue(UIA_NamePropertyId).err().unwrap()] {
                assert_eq!(e.code().0 as u32, UIA_E_ELEMENTNOTAVAILABLE);
            }
            assert_eq!(doc.ExpandToEnclosingUnit(TextUnit_Line).err().unwrap().code().0 as u32, UIA_E_ELEMENTNOTAVAILABLE);
        }
    }

    #[test]
    fn a_panicking_method_returns_e_fail() {
        let e = guard::<()>(|| panic!("boom")).unwrap_err();
        assert_eq!(e.code(), E_FAIL);
        assert_eq!(guard(|| Ok(5)).unwrap(), 5);
    }

    #[test]
    fn secure_pane_is_prompt_only_and_silent() {
        let w = Win::new();
        let mut p = pane(7, &["Password:"]);
        p.secure = true;
        let (h, sent) = host(&w, win_snap(vec![p]));
        assert!(h.deliverable(7).is_none(), "no event reaches a secure pane");
        let root = h.root_provider();
        let tp = text_provider(&root);
        // SAFETY: plain COM calls.
        let doc = unsafe { tp.DocumentRange().unwrap() };
        unsafe {
            assert_eq!(doc.Select().unwrap_err().code().0 as u32, UIA_E_INVALIDOPERATION);
            assert_eq!(doc.ScrollIntoView(true).unwrap_err().code().0 as u32, UIA_E_INVALIDOPERATION);
        }
        assert!(sent.lock().unwrap().is_empty());
        // raising anything is a no-op, never a crash
        h.raise(7, &access::Event::Announce("secret".into()));
        h.publish(win_snap(vec![pane(7, &["ab"])]));
        assert!(h.deliverable(7).is_some());
        assert!(h.deliverable(99).is_none());
    }

    #[test]
    fn find_text_follows_the_range() {
        let w = Win::new();
        let (h, _s) = host(&w, win_snap(vec![pane(7, &["Foo bar", "foo baz"])]));
        let tp = text_provider(&h.root_provider());
        // SAFETY: plain COM calls.
        let doc = unsafe { tp.DocumentRange().unwrap() };
        unsafe {
            let a = doc.FindText(&BSTR::from("foo"), false, false).unwrap();
            assert_eq!(a.CompareEndpoints(TextPatternRangeEndpoint_Start, &range(&tp, 8, 0), TextPatternRangeEndpoint_Start).unwrap(), 0);
            let b = doc.FindText(&BSTR::from("foo"), false, true).unwrap();
            assert_eq!(b.CompareEndpoints(TextPatternRangeEndpoint_Start, &doc, TextPatternRangeEndpoint_Start).unwrap(), 0);
            let c = doc.FindText(&BSTR::from("FOO"), true, true).unwrap();
            assert_eq!(c.CompareEndpoints(TextPatternRangeEndpoint_Start, &a, TextPatternRangeEndpoint_Start).unwrap(), 0);
            assert_eq!(get_text(&c, -1), "foo");
            assert!(doc.FindText(&BSTR::from("nope"), false, false).is_err());
            assert!(doc.FindText(&BSTR::new(), false, false).is_err());
            let long = BSTR::from("a".repeat(5000).as_str());
            assert!(doc.FindText(&long, false, false).is_err());
        }
    }

    #[test]
    fn huge_counts_are_bounded() {
        let w = Win::new();
        let (h, _s) = host(&w, win_snap(vec![pane(7, &["ab cd", "ef"])]));
        let tp = text_provider(&h.root_provider());
        // SAFETY: plain COM calls.
        let doc = unsafe { tp.DocumentRange().unwrap() };
        unsafe {
            let r = range(&tp, 1, -3);
            let n = r.MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, i32::MAX).unwrap();
            assert_eq!(n, 3);
            let r = tp.DocumentRange().unwrap();
            let r2 = r.Clone().unwrap();
            let moved = r2.Move(TextUnit_Word, i32::MIN).unwrap();
            assert!(moved <= 0);
            assert!(r.Compare(&doc).unwrap().as_bool());
            let cap = doc.GetText(i32::MAX).unwrap();
            assert_eq!(cap.to_string(), "ab cd\nef\n");
            assert!(doc.GetText(-2).is_err());
        }
    }

    #[test]
    fn navigation_and_points() {
        let w = Win::new();
        let mut p2 = pane(8, &["zz"]);
        p2.rect = (700, 200, 800, 300);
        p2.focused = false;
        let (h, _s) = host(&w, win_snap(vec![pane(7, &["ab"]), p2]));
        let root = h.root_provider();
        // SAFETY: plain COM calls.
        unsafe {
            let rf: IRawElementProviderFragment = root.cast().unwrap();
            let first: IRawElementProviderFragment = rf.Navigate(NavigateDirection_FirstChild).unwrap();
            let last = rf.Navigate(NavigateDirection_LastChild).unwrap();
            assert_eq!(first.Navigate(NavigateDirection_NextSibling).unwrap().as_raw(), last.as_raw());
            assert_eq!(last.Navigate(NavigateDirection_PreviousSibling).unwrap().as_raw(), first.as_raw());
            assert!(first.Navigate(NavigateDirection_PreviousSibling).is_err());
            assert_eq!(first.Navigate(NavigateDirection_Parent).unwrap().as_raw(), rf.as_raw());
            assert_eq!(first.FragmentRoot().unwrap().as_raw(), root.cast::<IRawElementProviderFragmentRoot>().unwrap().as_raw());
            let r = first.BoundingRectangle().unwrap();
            assert_eq!((r.left, r.top, r.width, r.height), (100.0, 200.0, 500.0, 400.0));
            let fr: IRawElementProviderFragmentRoot = root.cast().unwrap();
            assert_eq!(fr.ElementProviderFromPoint(750.0, 250.0).unwrap().as_raw(), last.as_raw());
            assert_eq!(fr.GetFocus().unwrap().as_raw(), first.as_raw());
            assert!(fr.ElementProviderFromPoint(5.0, 5.0).is_err());
            // the same element is the same object every time
            assert_eq!(rf.Navigate(NavigateDirection_FirstChild).unwrap().as_raw(), first.as_raw());
            let id = first.GetRuntimeId().unwrap();
            assert!(!id.is_null());
            assert_eq!(ranges_len(id), 3);
        }
    }

    fn ranges_len(psa: *mut SAFEARRAY) -> i32 {
        // SAFETY: psa is a vector from the provider.
        let n = unsafe { SafeArrayGetUBound(psa, 1).unwrap() + 1 };
        // SAFETY: freeing it.
        unsafe { SafeArrayDestroy(psa).unwrap() };
        n
    }

    #[test]
    fn scroll_and_visible_ranges() {
        let w = Win::new();
        let rows: Vec<String> = (0..10).map(|i| format!("l{i}")).collect();
        let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
        let mut p = pane(7, &refs);
        p.first_visible_row = 4;
        p.visible_rows = 3;
        let (h, sent) = host(&w, win_snap(vec![p]));
        let tp = text_provider(&h.root_provider());
        // SAFETY: plain COM calls.
        unsafe {
            let vis = ranges_of(tp.GetVisibleRanges().unwrap());
            assert_eq!(vis.len(), 1);
            assert_eq!(get_text(&vis[0], -1), "l4\nl5\nl6\n");
            let rects = doubles_of(vis[0].GetBoundingRectangles().unwrap());
            assert_eq!(rects.len(), 12);
            assert_eq!(rects[1], 210.0);
            let at = tp.RangeFromPoint(UiaPoint { x: 121.0, y: 231.0 }).unwrap();
            assert_eq!(get_text(&at, -1), "5");
            vis[0].ScrollIntoView(true).unwrap();
            assert_eq!(sent.lock().unwrap().clone(), vec![Request::ScrollIntoView { pane: 7, offset: 12 }]);
        }
    }

    #[test]
    fn the_host_unhooks_the_window() {
        let w = Win::new();
        let (h, _s) = host(&w, win_snap(vec![pane(7, &["ab"])]));
        let root = h.root_provider();
        drop(h);
        // a provider kept by a client after the window's host is gone answers, never crashes
        // SAFETY: plain COM call.
        let _ = unsafe { root.GetPropertyValue(UIA_ControlTypePropertyId) };
        let _ = HWND(std::ptr::null_mut());
    }
}
