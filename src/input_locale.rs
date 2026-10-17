//! iTerm2's Force keyboard (`InputSourceForcer.swift`), done the Windows way: while a window of
//! this process is in front, the chosen keyboard layout is activated for Termurion's own input
//! thread; the person's layout is remembered at activation and restored when Termurion is no longer
//! in front and at exit. The decisions are pure; the Win32 side only lists and activates layouts.

use crate::globals::Globals;

/// The Force keyboard settings.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Prefs {
    pub force: bool,
    pub once: bool,
    pub layout: String,
}

impl Prefs {
    pub fn from_globals(g: &Globals) -> Prefs {
        Prefs { force: g.bool("ForceKeyboard"), once: g.bool("ForceKeyboardOncePerSession"), layout: g.string("KeyboardLocale") }
    }
}

/// What the app does with the keyboard layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Activate(String),
    Nothing,
}

/// The forcer's memory (`InputSourceForcer`: `systemLocale`, `active`, `haveChangedSinceBecomingActive`).
#[derive(Debug, Default)]
pub struct Forcer {
    system: Option<String>,
    active: bool,
    changed_since_active: bool,
    /// Termurion has changed the layout and not yet put the person's back.
    forced: bool,
}

/// Whether Termurion as an app became active (Some(true)) or resigned (Some(false)): a move between
/// two of its own windows changes nothing (iTerm2 keys on app activation).
pub fn activation_change(was_any: bool, now_any: bool) -> Option<bool> {
    (was_any != now_any).then_some(now_any)
}

/// The layout id of an HKL: its high and low words as 8 hex digits.
pub fn layout_id(hkl: isize) -> String {
    format!("{:04X}{:04X}", (hkl >> 16) & 0xFFFF, hkl & 0xFFFF)
}

/// The names the layout popup shows: sorted by name, a name more than one layout has gets its id.
pub fn display_names(raw: &[(String, String)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = raw
        .iter()
        .map(|(name, id)| {
            let repeats = raw.iter().filter(|(n, _)| n == name).count() > 1;
            (if repeats { format!("{name} ({id})") } else { name.clone() }, id.clone())
        })
        .collect();
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then_with(|| a.1.cmp(&b.1)));
    out
}

impl Forcer {
    /// The layout the setting asks for; an empty one follows the person's (`desiredLocale ?? systemLocale`).
    fn desired(&self, prefs: &Prefs) -> Option<String> {
        if prefs.layout.is_empty() { self.system.clone() } else { Some(prefs.layout.clone()) }
    }

    fn force(&mut self, prefs: &Prefs) -> Step {
        match self.desired(prefs) {
            Some(id) => {
                self.forced = true;
                Step::Activate(id)
            }
            None => Step::Nothing,
        }
    }

    /// The person's layout comes back, when Termurion changed it.
    fn restore(&mut self) -> Step {
        match (self.forced, self.system.clone()) {
            (true, Some(id)) => {
                self.forced = false;
                Step::Activate(id)
            }
            _ => Step::Nothing,
        }
    }

    /// Termurion came to the front; `current` is the person's layout.
    pub fn became_active(&mut self, prefs: &Prefs, current: &str) -> Step {
        // Once per session leaves its layout in place after a resign: then `current` is the forced
        // one, and the person's layout stays the one to put back.
        if !self.forced {
            self.system = Some(current.to_string());
        }
        self.active = true;
        self.changed_since_active = false;
        if !prefs.force || prefs.once { Step::Nothing } else { self.force(prefs) }
    }

    /// Termurion is no longer in front.
    pub fn resigned(&mut self, prefs: &Prefs) -> Step {
        if !prefs.force {
            return Step::Nothing;
        }
        self.active = false;
        // Once per session leaves the layout in place (until Termurion ends).
        if prefs.once { Step::Nothing } else { self.restore() }
    }

    /// The person changed the layout while Termurion is in front: forced again, once.
    pub fn layout_changed(&mut self, prefs: &Prefs) -> Step {
        if !self.active || !prefs.force || prefs.once || self.changed_since_active {
            return Step::Nothing;
        }
        self.changed_since_active = true;
        self.force(prefs)
    }

    /// A setting changed.
    pub fn prefs_changed(&mut self, prefs: &Prefs) -> Step {
        if !self.active {
            return Step::Nothing;
        }
        if !prefs.force {
            return self.restore();
        }
        if prefs.once { Step::Nothing } else { self.force(prefs) }
    }

    /// A session got the keyboard; `needs` is its flag. Returns the step and the flag's new value.
    pub fn session_focused(&mut self, prefs: &Prefs, needs: bool) -> (Step, bool) {
        if !needs || !prefs.force || !prefs.once {
            return (Step::Nothing, needs);
        }
        let step = if prefs.layout.is_empty() { Step::Nothing } else { self.force(prefs) };
        (step, false)
    }

    /// Whether Termurion is in front as far as the forcer knows.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The person's layout while Termurion has changed it (what a panic must put back).
    pub fn to_restore(&self) -> Option<String> {
        if self.forced { self.system.clone() } else { None }
    }

    /// Termurion is ending: the person's layout comes back whatever the mode.
    pub fn exiting(&mut self) -> Step {
        self.active = false;
        self.restore()
    }
}

/// The installed layouts as (display name, id), by name; empty off Windows.
pub fn layouts() -> Vec<(String, String)> {
    #[cfg(windows)]
    return win::layouts();
    #[cfg(not(windows))]
    Vec::new()
}

/// `layouts` kept for a few seconds, so a settings page can ask every frame.
pub fn layouts_cached() -> Vec<(String, String)> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, Vec<(String, String)>)>> = Mutex::new(None);
    let Ok(mut c) = CACHE.lock() else { return layouts() };
    if let Some((at, list)) = c.as_ref()
        && at.elapsed() < Duration::from_secs(5) {
            return list.clone();
        }
    let list = layouts();
    *c = Some((Instant::now(), list.clone()));
    list
}

/// The layout id of Termurion's own input now (the person's, before anything is forced).
pub fn current() -> String {
    #[cfg(windows)]
    return win::current();
    #[cfg(not(windows))]
    String::new()
}

/// Activates the installed layout with this id from Termurion's input thread (the UI thread). Under
/// Windows' default input mode (one input method for every app) this changes the desktop's
/// layout, other apps' included, until it is restored: on resign and at exit, so with "Only force
/// once per session" it stays for other apps until Termurion exits. With "Let me use a different
/// input method for each app window" it is Termurion's alone. An id not installed logs and does
/// nothing (`InputSourceForcer.swift:180-199`).
pub fn activate(id: &str) -> bool {
    #[cfg(windows)]
    return win::activate(id);
    #[cfg(not(windows))]
    {
        let _ = id;
        false
    }
}

/// The layout the person had, kept for a panic: the hook puts it back (set while Termurion has changed it).
#[cfg(windows)]
static PANIC_RESTORE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Arms (Some) or disarms (None) the panic restore.
pub fn arm_restore(id: Option<String>) {
    #[cfg(windows)]
    if let Ok(mut r) = PANIC_RESTORE.lock() {
        *r = id;
    }
    #[cfg(not(windows))]
    let _ = id;
}

/// Called by the panic hook on the main (UI) thread: the person's layout comes back.
pub fn restore_after_panic() {
    #[cfg(windows)]
    if let Ok(mut r) = PANIC_RESTORE.try_lock()
        && let Some(id) = r.take() {
            win::activate(&id);
        }
}

#[cfg(windows)]
mod win {
    use super::{display_names, layout_id};
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ActivateKeyboardLayout, GetKeyboardLayout, GetKeyboardLayoutList, HKL};

    const ROOT: &str = "SYSTEM\\CurrentControlSet\\Control\\Keyboard Layouts";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn installed() -> Vec<HKL> {
        let n = unsafe { GetKeyboardLayoutList(0, std::ptr::null_mut()) };
        if n <= 0 {
            return Vec::new();
        }
        let mut list: Vec<HKL> = vec![std::ptr::null_mut(); n as usize];
        let got = unsafe { GetKeyboardLayoutList(n, list.as_mut_ptr()) };
        list.truncate(got.max(0) as usize);
        list
    }

    pub fn current() -> String {
        layout_id(unsafe { GetKeyboardLayout(0) } as isize)
    }

    pub fn activate(id: &str) -> bool {
        let Some(hkl) = installed().into_iter().find(|h| layout_id(*h as isize).eq_ignore_ascii_case(id)) else {
            eprintln!("input_locale: no installed keyboard layout {id}");
            return false;
        };
        // Flag 0: the calling thread's layout, which under the default input mode is the
        // desktop's too (restored on resign and at exit).
        !unsafe { ActivateKeyboardLayout(hkl, 0) }.is_null()
    }

    fn string_value(key: HKEY, name: &str) -> Option<String> {
        let name = wide(name);
        let mut bytes = 0u32;
        let rc = unsafe { RegGetValueW(key, std::ptr::null(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut bytes) };
        if rc != 0 || bytes < 2 || bytes > 4096 {
            return None;
        }
        let mut buf = vec![0u16; bytes as usize / 2 + 1];
        let mut got = bytes;
        let rc = unsafe { RegGetValueW(key, std::ptr::null(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut got) };
        if rc != 0 {
            return None;
        }
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        let text = String::from_utf16_lossy(&buf[..end]);
        (!text.is_empty()).then_some(text)
    }

    fn open(path: &str) -> Option<HKEY> {
        let p = wide(path);
        let mut key: HKEY = std::ptr::null_mut();
        let rc = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, p.as_ptr(), 0, KEY_READ, &mut key) };
        (rc == 0).then_some(key)
    }

    fn layout_text(klid: &str) -> Option<String> {
        let key = open(&format!("{ROOT}\\{klid}"))?;
        let text = string_value(key, "Layout Text");
        unsafe { RegCloseKey(key) };
        text
    }

    /// The name of a layout with a "Layout Id" (high word F...): the key of that language whose Layout Id matches.
    fn text_by_layout_id(hkl: isize) -> Option<String> {
        let (lang, want) = ((hkl & 0xFFFF) as u16, (hkl >> 16) & 0x0FFF);
        let root = open(ROOT)?;
        let mut found = None;
        for i in 0..2048u32 {
            let mut name = [0u16; 16];
            let mut len = name.len() as u32;
            let rc = unsafe { RegEnumKeyExW(root, i, name.as_mut_ptr(), &mut len, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
            if rc != 0 {
                break;
            }
            let klid = String::from_utf16_lossy(&name[..len as usize]);
            if klid.len() != 8 || u16::from_str_radix(&klid[4..], 16).ok() != Some(lang) {
                continue;
            }
            let Some(key) = open(&format!("{ROOT}\\{klid}")) else { continue };
            let id = string_value(key, "Layout Id").and_then(|t| i64::from_str_radix(&t, 16).ok());
            let text = string_value(key, "Layout Text");
            unsafe { RegCloseKey(key) };
            if id == Some(want as i64) {
                found = text;
                break;
            }
        }
        unsafe { RegCloseKey(root) };
        found
    }

    /// The language's own name, when the registry has no Layout Text.
    fn locale_name(lang: u16) -> Option<String> {
        use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LCIDToLocaleName, LOCALE_SLOCALIZEDDISPLAYNAME};
        let mut tag = [0u16; 85];
        let n = unsafe { LCIDToLocaleName(lang as u32, tag.as_mut_ptr(), tag.len() as i32, 0) };
        if n <= 0 {
            return None;
        }
        let mut out = [0u16; 128];
        let m = unsafe { GetLocaleInfoEx(tag.as_ptr(), LOCALE_SLOCALIZEDDISPLAYNAME, out.as_mut_ptr(), out.len() as i32) };
        (m > 1).then(|| String::from_utf16_lossy(&out[..(m as usize - 1)]))
    }

    pub fn layouts() -> Vec<(String, String)> {
        let raw: Vec<(String, String)> = installed()
            .into_iter()
            .map(|h| {
                let hkl = h as isize;
                let id = layout_id(hkl);
                let name = layout_text(&id)
                    .or_else(|| layout_text(&format!("0000{}", &id[4..])).filter(|_| (hkl >> 16) & 0xFFFF == hkl & 0xFFFF))
                    .or_else(|| if (hkl >> 28) & 0xF == 0xF { text_by_layout_id(hkl) } else { None })
                    .or_else(|| locale_name((hkl & 0xFFFF) as u16))
                    .unwrap_or_else(|| id.clone());
                (name, id)
            })
            .collect();
        display_names(&raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(force: bool, once: bool, layout: &str) -> Prefs {
        Prefs { force, once, layout: layout.into() }
    }
    fn act(s: &str) -> Step {
        Step::Activate(s.into())
    }

    #[test]
    fn forced_layout_follows_iterm2() {
        let p = prefs(true, false, "00000407");
        let mut f = Forcer::default();
        assert_eq!(f.became_active(&p, "00000409"), act("00000407"));
        assert_eq!(f.resigned(&p), act("00000409"));
        // The person's layout is remembered again at the next activation.
        assert_eq!(f.became_active(&p, "0000040C"), act("00000407"));
        assert_eq!(f.resigned(&p), act("0000040C"));

        // Once per session: nothing on activation or resignation; the first focus forces once.
        let o = prefs(true, true, "00000407");
        let mut f = Forcer::default();
        assert_eq!(f.became_active(&o, "00000409"), Step::Nothing);
        assert_eq!(f.session_focused(&o, true), (act("00000407"), false));
        assert_eq!(f.session_focused(&o, false), (Step::Nothing, false));
        assert_eq!(f.resigned(&o), Step::Nothing);
        // Sustained mode leaves the session flag alone.
        assert_eq!(f.session_focused(&p, true), (Step::Nothing, true));

        // Force off: every step is nothing.
        let off = prefs(false, false, "00000407");
        let mut f = Forcer::default();
        assert_eq!(f.became_active(&off, "00000409"), Step::Nothing);
        assert_eq!(f.layout_changed(&off), Step::Nothing);
        assert_eq!(f.prefs_changed(&off), Step::Nothing);
        assert_eq!(f.session_focused(&off, true), (Step::Nothing, true));
        assert_eq!(f.resigned(&off), Step::Nothing);

        // An empty layout follows the person's own.
        let e = prefs(true, false, "");
        let mut f = Forcer::default();
        assert_eq!(f.became_active(&e, "00000409"), act("00000409"));

        // A layout change after activation is overridden once, then left alone until the next activation.
        let mut f = Forcer::default();
        f.became_active(&p, "00000409");
        assert_eq!(f.layout_changed(&p), act("00000407"));
        assert_eq!(f.layout_changed(&p), Step::Nothing);
        f.resigned(&p);
        f.became_active(&p, "00000409");
        assert_eq!(f.layout_changed(&p), act("00000407"));
        // Not while Termurion is not in front.
        let mut f = Forcer::default();
        assert_eq!(f.layout_changed(&p), Step::Nothing);
    }

    #[test]
    fn prefs_changes_follow_iterm2() {
        let p = prefs(true, false, "00000407");
        let mut f = Forcer::default();
        f.became_active(&prefs(false, false, "00000407"), "00000409");
        // Turning Force keyboard on while active forces at once.
        assert_eq!(f.prefs_changed(&p), act("00000407"));
        // Turning it off puts the person's layout back (Termurion changed it).
        assert_eq!(f.prefs_changed(&prefs(false, false, "00000407")), act("00000409"));
        assert_eq!(f.prefs_changed(&prefs(false, false, "00000407")), Step::Nothing);
        // Once per session does not sustain.
        assert_eq!(f.prefs_changed(&prefs(true, true, "00000407")), Step::Nothing);
        // Not active: nothing.
        let mut idle = Forcer::default();
        assert_eq!(idle.prefs_changed(&p), Step::Nothing);
    }

    #[test]
    fn layout_restored_at_exit() {
        let p = prefs(true, false, "00000407");
        let mut f = Forcer::default();
        f.became_active(&p, "00000409");
        assert_eq!(f.exiting(), act("00000409"));
        assert_eq!(f.exiting(), Step::Nothing);
        // Once per session never restores on resign, but exit puts the layout back.
        let o = prefs(true, true, "00000407");
        let mut f = Forcer::default();
        f.became_active(&o, "00000409");
        f.session_focused(&o, true);
        assert_eq!(f.resigned(&o), Step::Nothing);
        assert_eq!(f.exiting(), act("00000409"));
        // Once per session, leave and return: the forced layout is current now, but the
        // person's layout is still the one to put back.
        let mut f = Forcer::default();
        f.became_active(&o, "00000409");
        f.session_focused(&o, true);
        f.resigned(&o);
        assert_eq!(f.became_active(&o, "00000407"), Step::Nothing);
        assert_eq!(f.to_restore(), Some("00000409".to_string()));
        assert_eq!(f.exiting(), act("00000409"));
        // Never forced: nothing to put back.
        let mut f = Forcer::default();
        f.became_active(&prefs(false, false, ""), "00000409");
        assert_eq!(f.exiting(), Step::Nothing);
    }

    #[test]
    fn activation_follows_the_process() {
        // Terminal window to Settings: Termurion stays in front, no step.
        assert_eq!(activation_change(true, true), None);
        assert_eq!(activation_change(false, false), None);
        assert_eq!(activation_change(false, true), Some(true));
        assert_eq!(activation_change(true, false), Some(false));
    }

    #[test]
    fn layout_names_follow_windows() {
        let raw = [("US".to_string(), "04090409".to_string()), ("US".to_string(), "F0010409".to_string()), ("German".to_string(), "04070407".to_string())];
        let names = display_names(&raw);
        let want: Vec<(String, String)> = vec![("German".into(), "04070407".into()), ("US (04090409)".into(), "04090409".into()), ("US (F0010409)".into(), "F0010409".into())];
        assert_eq!(names, want);
        // Unique names are unchanged and sorted.
        let raw = [("US-International".to_string(), "00020409".to_string()), ("German".to_string(), "04070407".to_string())];
        assert_eq!(display_names(&raw), vec![("German".to_string(), "04070407".to_string()), ("US-International".to_string(), "00020409".to_string())]);
        assert_eq!(layout_id(0x0409_0409), "04090409");
        assert_eq!(layout_id(0xF020_0409u32 as i32 as isize), "F0200409");
        assert_eq!(layout_id(0xF020_0409), "F0200409");
    }
}
