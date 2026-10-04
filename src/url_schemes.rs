//! Which profile an `ssh:` link opens: iTerm2's `URLHandlersByGuid` (`iTermLaunchServices.m`), a
//! table of scheme to profile Guid kept app-wide in the globals (not in the profile), and the
//! Windows side of being the handler. Only the person's tick plus an OK to the not-default
//! question writes the registry, only under `HKCU`, never a `UserChoice` key (Windows protects
//! it) and never `HKLM`. The registry sits behind `SchemeRegistry` so tests use a fake.

use std::path::Path;

/// The schemes the "URL schemes:" popup lists (milestone 14 does `ssh` only).
pub const SCHEMES: [&str; 1] = ["ssh"];
/// iTerm2's key for the table (`LS`); a globals value with no settings row.
pub const HANDLERS_KEY: &str = "URLHandlersByGuid";

pub const LABEL: &str = "URL schemes:";
pub const SELECT: &str = "Select URL Schemes\u{2026}";
pub const NO_HANDLER: &str = "There is currently no handler.";

pub fn not_default(scheme: &str) -> String {
    format!("Tandem is not the default handler for {scheme}. Would you like to set Tandem as the default handler?")
}

pub fn current_handler(name: &str) -> String {
    format!("The current handler is: {name}")
}

/// Tandem's own: shown when Windows' protected user choice keeps another app as the handler.
pub fn protected(scheme: &str) -> String {
    format!("Windows keeps another app as the handler for {scheme} links. Choose Tandem for {scheme} in Settings > Apps > Default apps.")
}

/// What Windows says about a scheme's handler, and the one write Tandem makes.
pub trait SchemeRegistry {
    /// The effective handler's command line (`shell\open\command`), None when there is none.
    fn handler_command(&self, scheme: &str) -> Option<String>;
    /// The effective handler's friendly name, None when it has none.
    fn handler_name(&self, scheme: &str) -> Option<String>;
    /// The ProgId of the protected user choice for the scheme (read only).
    fn user_choice(&self, scheme: &str) -> Option<String>;
    /// Registers Tandem as a handler for the user (`HKCU` only). Runs only after the person's OK.
    fn register(&mut self, scheme: &str, exe: &Path) -> Result<(), String>;
}

/// The protocol handler's command: the URL is one quoted argument (`tandem.exe` accepts only
/// exactly `[exe, "--url", url]`).
pub fn tandem_command(exe: &Path) -> String {
    format!("\"{}\" --url \"%1\"", exe.display())
}

/// Whether Tandem is the effective handler: its command as written, the path case-insensitive.
pub fn is_default(reg: &dyn SchemeRegistry, scheme: &str, exe: &Path) -> bool {
    reg.handler_command(scheme).is_some_and(|c| c.trim().eq_ignore_ascii_case(&tandem_command(exe)))
}

pub fn guid_for(table: &toml::Table, scheme: &str) -> Option<String> {
    table.get(scheme).and_then(|v| v.as_str()).map(str::to_string)
}

pub fn bind(table: &mut toml::Table, scheme: &str, guid: &str) {
    table.insert(scheme.to_string(), toml::Value::String(guid.to_string()));
}

/// Unbinding only removes the entry (iTerm2 leaves the Launch Services handler); Tandem stays
/// registered.
pub fn unbind(table: &mut toml::Table, scheme: &str) {
    table.remove(scheme);
}

/// The profile a scheme opens, only while Tandem is the default handler and the profile exists
/// (`profileForScheme`).
pub fn profile_for_scheme(table: &toml::Table, reg: &dyn SchemeRegistry, scheme: &str, exe: &Path, profile_exists: &dyn Fn(&str) -> bool) -> Option<String> {
    guid_for(table, scheme).filter(|g| is_default(reg, scheme, exe) && profile_exists(g))
}

/// Drops bindings whose profile no longer exists (iTerm2's load-time filter); true when the
/// table changed and must be saved. Run when the globals load, when a profile is deleted and
/// after an import (an imported iTerm2 table holds iTerm2's Guids).
pub fn prune(table: &mut toml::Table, profile_exists: &dyn Fn(&str) -> bool) -> bool {
    let before = table.len();
    table.retain(|_, v| v.as_str().is_some_and(profile_exists));
    table.len() != before
}

/// The control's check: bound to this profile and Tandem is the default.
pub fn handles(table: &toml::Table, reg: &dyn SchemeRegistry, scheme: &str, guid: &str, exe: &Path) -> bool {
    guid_for(table, scheme).as_deref() == Some(guid) && is_default(reg, scheme, exe)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    Unbind,
    Bind,
    /// Tandem is not the handler: ask (heading `text`, body `info`) before registering.
    Ask { text: String, info: String },
}

/// The program of a command line, for naming a handler that has no friendly name.
fn command_program(command: &str) -> String {
    let c = command.trim();
    match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or("").to_string(),
        None => c.split_whitespace().next().unwrap_or("").to_string(),
    }
}

/// What choosing `scheme` in a profile's menu does (iTerm2's toggle).
pub fn pick(table: &toml::Table, reg: &dyn SchemeRegistry, scheme: &str, guid: &str, exe: &Path) -> Pick {
    if handles(table, reg, scheme, guid, exe) {
        return Pick::Unbind;
    }
    if is_default(reg, scheme, exe) {
        return Pick::Bind;
    }
    // A handler that exists is always named (friendly name, else its program), so its command is
    // never replaced unseen; "no handler" only when there is none.
    let info = match reg.handler_command(scheme) {
        None => NO_HANDLER.to_string(),
        Some(command) => {
            let name = reg.handler_name(scheme).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| command_program(&command));
            current_handler(&name)
        }
    };
    Pick::Ask { text: not_default(scheme), info }
}

/// The OK of the not-default question: registers, then binds only when Tandem really is the
/// default now. Err is the text to show (Windows kept another handler, or a write failed) and
/// nothing is bound.
pub fn register_and_bind(table: &mut toml::Table, reg: &mut dyn SchemeRegistry, scheme: &str, guid: &str, exe: &Path) -> Result<(), String> {
    reg.register(scheme, exe).map_err(|e| format!("Could not register Tandem for {scheme} links: {e}"))?;
    if !is_default(reg, scheme, exe) {
        return Err(protected(scheme));
    }
    bind(table, scheme, guid);
    Ok(())
}

/// Prunes the globals' table against the profiles that exist; true when it changed.
pub fn prune_globals(globals: &mut crate::globals::Globals, store: &crate::store::Store) -> bool {
    let mut table = globals.table(HANDLERS_KEY);
    let guids: std::collections::HashSet<String> = store.profiles.iter().map(|p| p.guid()).collect();
    if !prune(&mut table, &|g| guids.contains(g)) {
        return false;
    }
    globals.set(HANDLERS_KEY, toml::Value::Table(table));
    true
}

/// The registry the app uses: Windows', and none elsewhere.
pub fn system() -> Box<dyn SchemeRegistry> {
    #[cfg(windows)]
    {
        Box::new(WinRegistry)
    }
    #[cfg(not(windows))]
    {
        Box::new(NoRegistry)
    }
}

#[cfg(not(windows))]
pub struct NoRegistry;

#[cfg(not(windows))]
impl SchemeRegistry for NoRegistry {
    fn handler_command(&self, _: &str) -> Option<String> {
        None
    }
    fn handler_name(&self, _: &str) -> Option<String> {
        None
    }
    fn user_choice(&self, _: &str) -> Option<String> {
        None
    }
    fn register(&mut self, _: &str, _: &Path) -> Result<(), String> {
        Err("not Windows".into())
    }
}

#[cfg(windows)]
pub struct WinRegistry;

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegGetValueW, RegSetValueExW};
    #[cfg(test)]
    use windows_sys::Win32::System::Registry::RegDeleteTreeW;

    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Writes a string value (`name` None is the key's default) under `HKCU\<subkey>`, creating
    /// the key. Only ever `HKEY_CURRENT_USER`.
    pub fn set(subkey: &str, name: Option<&str>, value: &str) -> Result<(), String> {
        let key = wide(subkey);
        let mut h: HKEY = std::ptr::null_mut();
        let rc = unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, std::ptr::null(), &mut h, std::ptr::null_mut()) };
        if rc != ERROR_SUCCESS {
            return Err(format!("HKCU\\{subkey}: error {rc}"));
        }
        let n = name.map(wide);
        let v = wide(value);
        let rc = unsafe { RegSetValueExW(h, n.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()), 0, REG_SZ, v.as_ptr() as *const u8, (v.len() * 2) as u32) };
        unsafe { RegCloseKey(h) };
        if rc != ERROR_SUCCESS {
            return Err(format!("HKCU\\{subkey}: error {rc}"));
        }
        Ok(())
    }

    /// Reads a string value under `HKCU\<subkey>`.
    pub fn get(subkey: &str, name: Option<&str>) -> Option<String> {
        let key = wide(subkey);
        let n = name.map(wide);
        let mut buf = vec![0u16; 1024];
        let mut bytes = (buf.len() * 2) as u32;
        let rc = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), n.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut _, &mut bytes) };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    /// Removes `HKCU\<subkey>` and everything under it (tests' throwaway key only).
    #[cfg(test)]
    pub fn delete_tree(subkey: &str) {
        let key = wide(subkey);
        unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr()) };
    }

}

#[cfg(windows)]
impl WinRegistry {
    /// Writes the protocol registration under `HKCU\<root>`: the ProgId `Tandem.<scheme>` first,
    /// then the scheme key, its command key last, so a failed write leaves no half handler.
    pub fn register_under(root: &str, scheme: &str, exe: &Path) -> Result<(), String> {
        let command = tandem_command(exe);
        let progid = format!("{root}\\Tandem.{scheme}");
        win::set(&progid, None, &format!("URL:{scheme} Protocol"))?;
        win::set(&format!("{progid}\\shell\\open\\command"), None, &command)?;
        let key = format!("{root}\\{scheme}");
        win::set(&key, None, &format!("URL:{scheme} Protocol"))?;
        win::set(&key, Some("URL Protocol"), "")?;
        win::set(&format!("{key}\\shell\\open\\command"), None, &command)
    }

    /// Reads back what `register_under` wrote: (scheme default, scheme command, ProgId command).
    pub fn read_under(root: &str, scheme: &str) -> (Option<String>, Option<String>, Option<String>) {
        (win::get(&format!("{root}\\{scheme}"), None), win::get(&format!("{root}\\{scheme}\\shell\\open\\command"), None), win::get(&format!("{root}\\Tandem.{scheme}\\shell\\open\\command"), None))
    }

    fn assoc(scheme: &str, what: windows_sys::Win32::UI::Shell::ASSOCSTR) -> Option<String> {
        use windows_sys::Win32::UI::Shell::{ASSOCF_IS_PROTOCOL, AssocQueryStringW};
        let a = win::wide(scheme);
        let verb = win::wide("open");
        let mut buf = vec![0u16; 2048];
        let mut len = buf.len() as u32;
        let hr = unsafe { AssocQueryStringW(ASSOCF_IS_PROTOCOL as _, what, a.as_ptr(), verb.as_ptr(), buf.as_mut_ptr(), &mut len) };
        if hr != 0 {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let s = String::from_utf16_lossy(&buf[..end]);
        (!s.trim().is_empty()).then_some(s)
    }
}

#[cfg(windows)]
impl SchemeRegistry for WinRegistry {
    fn handler_command(&self, scheme: &str) -> Option<String> {
        use windows_sys::Win32::UI::Shell::ASSOCSTR_COMMAND;
        Self::assoc(scheme, ASSOCSTR_COMMAND)
    }

    fn handler_name(&self, scheme: &str) -> Option<String> {
        use windows_sys::Win32::UI::Shell::ASSOCSTR_FRIENDLYAPPNAME;
        Self::assoc(scheme, ASSOCSTR_FRIENDLYAPPNAME)
    }

    fn user_choice(&self, scheme: &str) -> Option<String> {
        win::get(&format!("Software\\Microsoft\\Windows\\Shell\\Associations\\UrlAssociations\\{scheme}\\UserChoice"), Some("ProgId"))
    }

    fn register(&mut self, scheme: &str, exe: &Path) -> Result<(), String> {
        // Listed in Default apps first; the protocol handler itself last.
        win::set("Software\\Tandem\\Capabilities", Some("ApplicationName"), "Tandem")?;
        win::set("Software\\Tandem\\Capabilities", Some("ApplicationDescription"), "Tandem terminal")?;
        win::set("Software\\Tandem\\Capabilities\\URLAssociations", Some(scheme), &format!("Tandem.{scheme}"))?;
        win::set("Software\\RegisteredApplications", Some("Tandem"), "Software\\Tandem\\Capabilities")?;
        Self::register_under("Software\\Classes", scheme, exe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[derive(Default)]
    struct Fake {
        command: Option<String>,
        name: Option<String>,
        choice: Option<String>,
        registered: Vec<(String, PathBuf)>,
        fail: bool,
    }

    impl SchemeRegistry for Fake {
        fn handler_command(&self, _: &str) -> Option<String> {
            self.command.clone()
        }
        fn handler_name(&self, _: &str) -> Option<String> {
            self.name.clone()
        }
        fn user_choice(&self, _: &str) -> Option<String> {
            self.choice.clone()
        }
        fn register(&mut self, scheme: &str, exe: &Path) -> Result<(), String> {
            if self.fail {
                return Err("denied".into());
            }
            self.registered.push((scheme.to_string(), exe.to_path_buf()));
            // Windows keeps a protected user choice over a new registration.
            if self.choice.is_none() {
                self.command = Some(tandem_command(exe));
            }
            Ok(())
        }
    }

    fn exe() -> PathBuf {
        PathBuf::from(r"C:\T\tandem.exe")
    }

    fn ours() -> Fake {
        Fake { command: Some(tandem_command(&exe())), ..Fake::default() }
    }

    #[test]
    fn scheme_binding_follows_iterm2() {
        assert_eq!(tandem_command(&exe()), "\"C:\\T\\tandem.exe\" --url \"%1\"");
        assert!(is_default(&ours(), "ssh", &exe()));
        let upper = Fake { command: Some("\"c:\\t\\TANDEM.EXE\" --url \"%1\"".into()), ..Fake::default() };
        assert!(is_default(&upper, "ssh", &exe()));
        assert!(!is_default(&Fake { command: Some("putty.exe %1".into()), ..Fake::default() }, "ssh", &exe()));
        assert!(!is_default(&Fake::default(), "ssh", &exe()));
        let mut t = toml::Table::new();
        bind(&mut t, "ssh", "G1");
        assert_eq!(guid_for(&t, "ssh").as_deref(), Some("G1"));
        let reg = ours();
        let exists = |g: &str| g == "G1";
        assert_eq!(profile_for_scheme(&t, &reg, "ssh", &exe(), &exists).as_deref(), Some("G1"));
        assert_eq!(profile_for_scheme(&t, &Fake::default(), "ssh", &exe(), &exists), None, "not the default");
        assert!(handles(&t, &reg, "ssh", "G1", &exe()));
        assert!(!handles(&t, &reg, "ssh", "G2", &exe()));
        assert!(!handles(&t, &Fake::default(), "ssh", "G1", &exe()));
        // A deleted profile: no profile, and prune drops the binding once.
        let gone = |_: &str| false;
        assert_eq!(profile_for_scheme(&t, &reg, "ssh", &exe(), &gone), None);
        assert!(prune(&mut t, &gone));
        assert!(!prune(&mut t, &gone));
        assert_eq!(guid_for(&t, "ssh"), None);
        bind(&mut t, "ssh", "G1");
        assert!(!prune(&mut t, &exists), "a binding to a live profile stays");
        t.insert("odd".into(), toml::Value::Integer(3));
        assert!(prune(&mut t, &exists), "a non-string entry is dropped");
        unbind(&mut t, "ssh");
        assert!(t.is_empty());
        // pick
        bind(&mut t, "ssh", "G1");
        assert_eq!(pick(&t, &reg, "ssh", "G1", &exe()), Pick::Unbind);
        assert_eq!(pick(&t, &reg, "ssh", "G2", &exe()), Pick::Bind);
        assert_eq!(pick(&toml::Table::new(), &reg, "ssh", "G1", &exe()), Pick::Bind);
        let putty = Fake { command: Some("\"C:\\Program Files\\PuTTY\\putty.exe\" %1".into()), name: Some("PuTTY".into()), ..Fake::default() };
        assert_eq!(
            pick(&t, &putty, "ssh", "G1", &exe()),
            Pick::Ask {
                text: "Tandem is not the default handler for ssh. Would you like to set Tandem as the default handler?".into(),
                info: "The current handler is: PuTTY".into()
            }
        );
        // P15: a handler with no friendly name is named by its program, never "no handler".
        let nameless = Fake { command: Some("\"C:\\Tools\\sshx.exe\" %1".into()), ..Fake::default() };
        assert_eq!(pick(&t, &nameless, "ssh", "G1", &exe()), Pick::Ask { text: not_default("ssh"), info: "The current handler is: C:\\Tools\\sshx.exe".into() });
        let bare = Fake { command: Some("sshx %1".into()), ..Fake::default() };
        assert!(matches!(pick(&t, &bare, "ssh", "G1", &exe()), Pick::Ask { info, .. } if info == "The current handler is: sshx"));
        assert_eq!(pick(&t, &Fake::default(), "ssh", "G1", &exe()), Pick::Ask { text: not_default("ssh"), info: "There is currently no handler.".into() });
        assert_eq!((LABEL, SELECT), ("URL schemes:", "Select URL Schemes\u{2026}"));
        assert_eq!(SCHEMES, ["ssh"]);
        assert_eq!(HANDLERS_KEY, "URLHandlersByGuid");
    }

    #[test]
    fn registration_follows_windows() {
        // OK registers, then binds because Tandem is now the handler.
        let mut reg = Fake::default();
        let mut t = toml::Table::new();
        assert_eq!(register_and_bind(&mut t, &mut reg, "ssh", "G1", &exe()), Ok(()));
        assert_eq!(reg.registered, vec![("ssh".to_string(), exe())]);
        assert_eq!(guid_for(&t, "ssh").as_deref(), Some("G1"));
        // Windows keeps another ProgId: reported with Tandem's text, nothing bound.
        let mut kept = Fake { command: Some("putty.exe %1".into()), choice: Some("PuTTY.ssh".into()), ..Fake::default() };
        let mut t = toml::Table::new();
        let err = register_and_bind(&mut t, &mut kept, "ssh", "G1", &exe()).unwrap_err();
        assert_eq!(err, "Windows keeps another app as the handler for ssh links. Choose Tandem for ssh in Settings > Apps > Default apps.");
        assert_eq!(err, protected("ssh"));
        assert!(!is_default(&kept, "ssh", &exe()));
        assert!(t.is_empty());
        // A failed write binds nothing.
        let mut bad = Fake { fail: true, ..Fake::default() };
        assert!(register_and_bind(&mut t, &mut bad, "ssh", "G1", &exe()).is_err());
        assert!(t.is_empty());
        // Cancel never reaches register: only register_and_bind calls it.
        assert_eq!(HANDLERS_KEY, "URLHandlersByGuid");
    }

    #[test]
    fn globals_table_is_pruned_against_the_store() {
        let path = std::env::temp_dir().join(format!("tandem-urlschemes-{}.toml", std::process::id()));
        let store = crate::store::Store::fresh(&path);
        let live = store.profiles[0].guid();
        let mut globals = crate::globals::Globals::empty(&path);
        let mut t = toml::Table::new();
        bind(&mut t, "ssh", "not-a-profile");
        bind(&mut t, "other", &live);
        globals.set(HANDLERS_KEY, toml::Value::Table(t));
        assert!(prune_globals(&mut globals, &store));
        let t = globals.table(HANDLERS_KEY);
        assert_eq!((guid_for(&t, "ssh"), guid_for(&t, "other")), (None, Some(live)));
        assert!(!prune_globals(&mut globals, &store));
    }

    #[cfg(windows)]
    #[test]
    fn registry_writes_under_hkcu() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = format!("Software\\Tandem\\test-{}-{nanos}", std::process::id());
        let result = WinRegistry::register_under(&root, "ssh", &exe());
        let read = WinRegistry::read_under(&root, "ssh");
        win::delete_tree(&root);
        assert_eq!(result, Ok(()));
        let command = tandem_command(&exe());
        assert_eq!(read, (Some("URL:ssh Protocol".to_string()), Some(command.clone()), Some(command)));
        assert_eq!(win::get(&root, None), None, "the throwaway key is gone");
    }
}
