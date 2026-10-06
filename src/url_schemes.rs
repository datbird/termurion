//! Which profile an `ssh:` link opens: iTerm2's `URLHandlersByGuid` (`iTermLaunchServices.m`), a
//! table of scheme to profile Guid kept app-wide in the globals (not in the profile), and the
//! Windows side of being the handler. Only the person's tick plus an OK to the not-default
//! question writes the registry, only under `HKCU`, never a `UserChoice` key (Windows protects
//! it) and never `HKLM`. The registry sits behind `SchemeRegistry` so tests use a fake.

use std::path::Path;

/// The schemes the "URL schemes:" popup lists and Tandem may register for: iTerm2's
/// `CFBundleURLTypes` narrowed to the ones a Tandem link opens (P8). http, https, mailto, news and
/// the rest wait for a Browser profile: registering them would make Tandem a candidate default
/// browser or mail program whose handler opens an unrelated terminal tab.
pub const SCHEMES: [&str; 3] = ["ftp", "ssh", "telnet"];
/// Windows' Default apps page for Tandem, opened when Windows keeps another handler (a UserChoice
/// is hash protected: a program cannot set it, the person can).
pub const DEFAULT_APPS_URI: &str = "ms-settings:defaultapps?registeredAppUser=Tandem";
/// iTerm2's key for the table (`LS`); a globals value with no settings row.
pub const HANDLERS_KEY: &str = "URLHandlersByGuid";

pub const LABEL: &str = "URL schemes:";
pub const SELECT: &str = "Select URL Schemes\u{2026}";
pub const NO_HANDLER: &str = "There is currently no handler.";

pub fn not_default(scheme: &str) -> String {
    format!("Tandem is not the default handler for {scheme}. Would you like to set Tandem as the default handler?")
}

const ANOTHER_PROGRAM: &str = "another program";

pub fn replaces(program: &str) -> String {
    format!("Tandem will replace this program's registration: {program}")
}

/// Tandem's own: Windows keeps the user's choice, so Tandem's entry waits for the person.
pub fn not_effective(scheme: &str) -> String {
    format!("Windows keeps another app as the handler for {scheme} links, so Tandem's entry will not take effect until you choose Tandem in Settings > Apps > Default apps.")
}

pub fn current_handler(name: &str) -> String {
    format!("The current handler is: {name}")
}

/// Tandem's own: shown when Windows' protected user choice keeps another app as the handler.
pub fn protected(scheme: &str) -> String {
    format!("Windows keeps another app as the handler for {scheme} links. Choose Tandem for {scheme} in Settings > Apps > Default apps.")
}

/// Whether `scheme` is one Tandem lists (the one gate before any registry write or delete).
pub fn is_listed(scheme: &str) -> bool {
    SCHEMES.contains(&scheme)
}

/// The exe path inside a command written by `tandem_command`, None for any other shape.
pub fn command_argument_shape(command: &str) -> Option<String> {
    let rest = command.trim().strip_prefix('"')?;
    let (program, tail) = rest.split_once('"')?;
    (tail == " --url \"%1\"" && !program.is_empty()).then(|| program.to_string())
}

/// A path that may sit inside the quoted command: absolute, no quote, no control character.
fn exe_ok(exe: &Path) -> bool {
    let s = exe.to_string_lossy();
    exe.is_absolute() && !s.contains('"') && !s.chars().any(char::is_control)
}

fn progid(scheme: &str) -> String {
    format!("Software\\Classes\\Tandem.{scheme}")
}

const CAPABILITIES: &str = "Software\\Tandem\\Capabilities";
const ASSOCIATIONS: &str = "Software\\Tandem\\Capabilities\\URLAssociations";
const REGISTERED_APPS: &str = "Software\\RegisteredApplications";
/// Where the exe path each OK registered is kept, one value per scheme (Tandem's own key).
const RECORDED: &str = "Software\\Tandem\\Registered";

/// Every value Tandem writes to be listed as a handler of `scheme` (key, name, value; `""` is a key's
/// default): its ProgId, its capabilities and its RegisteredApplications entry. Empty for a scheme
/// Tandem does not list. The scheme key `Software\Classes\<scheme>` is not here: it is Tandem's only
/// after the person's OK (`WinRegistry::register_under`).
pub fn registry_entries(exe: &Path, scheme: &str) -> Vec<(String, String, String)> {
    if !is_listed(scheme) || !exe_ok(exe) {
        return Vec::new();
    }
    let e = |k: &str, n: &str, v: String| (k.to_string(), n.to_string(), v);
    let id = progid(scheme);
    vec![
        e(CAPABILITIES, "ApplicationName", "Tandem".into()),
        e(CAPABILITIES, "ApplicationDescription", "Tandem terminal".into()),
        e(ASSOCIATIONS, scheme, format!("Tandem.{scheme}")),
        e(REGISTERED_APPS, "Tandem", CAPABILITIES.into()),
        e(&id, "", format!("URL:{scheme} Protocol")),
        e(&id, "URL Protocol", String::new()),
        e(&format!("{id}\\shell\\open\\command"), "", tandem_command(exe)),
    ]
}

/// Writes Tandem's own entries for `scheme` through the registry seam; the number written (0 when
/// they are all there already). Refuses a scheme Tandem does not list and an exe path that could not
/// sit inside the quoted command.
pub fn register_with(r: &mut dyn crate::reg::Registry, scheme: &str, exe: &Path) -> Result<usize, String> {
    if !is_listed(scheme) {
        return Err(format!("Tandem does not register for {scheme} links."));
    }
    if !exe_ok(exe) {
        return Err("Tandem's own path cannot be registered.".to_string());
    }
    crate::reg::apply(r, &registry_entries(exe, scheme))
}

/// Takes back what `register_with` (and, when its command is Tandem's, the scheme key) wrote for
/// `scheme`; another program's keys are never touched. The listing goes with the last scheme.
pub fn unregister_with(r: &mut dyn crate::reg::Registry, scheme: &str, exe: &Path) -> Result<(), String> {
    if !is_listed(scheme) {
        return Err(format!("Tandem does not register for {scheme} links."));
    }
    r.delete_tree(&progid(scheme))?;
    r.delete_value(ASSOCIATIONS, scheme)?;
    r.delete_value(RECORDED, scheme)?;
    let class = format!("Software\\Classes\\{scheme}\\shell\\open\\command");
    if let Some(c) = r.get(&class, "") {
        let ours = is_tandem_command(&c) || c.trim().eq_ignore_ascii_case(&tandem_command(exe));
        if ours {
            r.delete_scheme_class(scheme)?;
        }
    }
    if !SCHEMES.iter().any(|s| r.get(ASSOCIATIONS, s).is_some()) {
        r.delete_value(REGISTERED_APPS, "Tandem")?;
        r.delete_tree(CAPABILITIES)?;
    }
    Ok(())
}

/// A command `tandem_command` wrote for some `tandem.exe`.
fn is_tandem_command(command: &str) -> bool {
    command_argument_shape(command).is_some_and(|p| Path::new(&p).file_name().is_some_and(|f| f.eq_ignore_ascii_case("tandem.exe")))
}

/// The bound schemes whose registration the person's OK wrote at a path that is gone (the exe
/// moved): the exe path recorded at that OK (`record_exe`) is not this exe, `HKCU\Software\Classes\
/// <scheme>` still holds exactly the command written for that path (never the effective handler,
/// which may be HKLM's or a UserChoice ProgId), and `gone` says the recorded exe no longer exists.
/// Another live copy, a machine-wide registration and a binding imported with no OK recorded on this
/// machine are never listed, so a repair takes nothing the person did not give.
pub fn stale_schemes(table: &toml::Table, reg: &dyn SchemeRegistry, exe: &Path, gone: &dyn Fn(&Path) -> bool) -> Vec<String> {
    let now = tandem_command(exe);
    let stale = |s: &str| {
        let Some(recorded) = reg.recorded_exe(s) else { return false };
        let recorded = Path::new(&recorded);
        let then = tandem_command(recorded);
        exe_ok(recorded) && then != now && reg.classes_command(s).is_some_and(|c| c.trim() == then) && gone(recorded)
    };
    SCHEMES.iter().filter(|s| guid_for(table, s).is_some()).filter(|s| stale(s)).map(|s| s.to_string()).collect()
}

/// Rewrites Tandem's own entries for the schemes `stale_schemes` names and records the new path;
/// the ones repaired.
pub fn repair(table: &toml::Table, reg: &mut dyn SchemeRegistry, exe: &Path, gone: &dyn Fn(&Path) -> bool) -> Vec<String> {
    stale_schemes(table, reg, exe, gone).into_iter().filter(|s| reg.register(s, exe).and_then(|()| reg.record_exe(s, exe)).is_ok()).collect()
}

/// Startup: rewrites Tandem's own registrations for the schemes the person bound when the exe moved.
/// Registers nothing new and never asks (it only restores what the person's earlier OK created).
/// The recorded exe path is saved state, so it is judged before it is probed: a path the judge
/// refuses counts as still there, and nothing is rewritten.
pub fn repair_at_start(globals: &crate::globals::Globals) {
    let Ok(exe) = std::env::current_exe() else { return };
    let table = globals.table(HANDLERS_KEY);
    if table.is_empty() {
        return;
    }
    let mut reg = system();
    repair(&table, reg.as_mut(), &exe, &|p| crate::localfs::judged(p) && !crate::localfs::local_exists(p));
}

/// The items of the "Select URL Schemes..." popup: the title unchecked, then each scheme checked
/// when this profile is bound to it and Tandem is the default (`PGP:1045-1051`).
pub fn row_items(table: &toml::Table, reg: &dyn SchemeRegistry, guid: &str, exe: &Path) -> Vec<(String, bool)> {
    std::iter::once((SELECT.to_string(), false)).chain(SCHEMES.iter().map(|s| (s.to_string(), handles(table, reg, s, guid, exe)))).collect()
}

/// What Windows says about a scheme's handler, and the one write Tandem makes.
pub trait SchemeRegistry {
    /// The effective handler's command line (`shell\open\command`), None when there is none.
    fn handler_command(&self, scheme: &str) -> Option<String>;
    /// The effective handler's friendly name, None when it has none.
    fn handler_name(&self, scheme: &str) -> Option<String>;
    /// The ProgId of the protected user choice for the scheme (read only).
    fn user_choice(&self, scheme: &str) -> Option<String>;
    /// The command of `HKCU\Software\Classes\<scheme>\shell\open\command`, the entry Tandem's OK
    /// overwrites: None when there is no such entry, Some("") when it has no readable command
    /// (a `DelegateExecute` verb).
    fn classes_command(&self, scheme: &str) -> Option<String>;
    /// Registers Tandem as a handler for the user (`HKCU` only). Runs only after the person's OK.
    fn register(&mut self, scheme: &str, exe: &Path) -> Result<(), String>;
    /// The exe path the person's OK registered for `scheme` (`HKCU\Software\Tandem\Registered`, a
    /// key no settings import carries), None when no OK on this machine did.
    fn recorded_exe(&self, _scheme: &str) -> Option<String> {
        None
    }
    /// Keeps `exe` as the path registered for `scheme`; written only with a registration the
    /// person's OK made, or its repair.
    fn record_exe(&mut self, _scheme: &str, _exe: &Path) -> Result<(), String> {
        Ok(())
    }
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
    Pick::Ask { text: not_default(scheme), info: ask_info(reg, scheme) }
}

/// The question's detail. Every handler the OK touches is named, so none is replaced unseen: the
/// effective handler (friendly name, else its program, else "another program" when one exists
/// whose command cannot be read), the `HKCU\Classes` entry the OK overwrites when it is another
/// program, and, when Windows keeps a protected choice, that Tandem's entry will not take effect.
/// "No handler" only when there is none at all.
fn ask_info(reg: &dyn SchemeRegistry, scheme: &str) -> String {
    let command = reg.handler_command(scheme);
    let classes = reg.classes_command(scheme);
    let choice = reg.user_choice(scheme).filter(|c| !c.eq_ignore_ascii_case(&format!("Tandem.{scheme}")));
    let name = reg.handler_name(scheme).filter(|n| !n.trim().is_empty());
    let effective_program = command.as_deref().map(command_program).filter(|p| !p.is_empty());
    let mut lines = Vec::new();
    match (&name, &effective_program) {
        (Some(n), _) => lines.push(current_handler(n)),
        (None, Some(p)) => lines.push(current_handler(p)),
        // Something handles the scheme but its command cannot be read.
        (None, None) if command.is_some() || classes.is_some() || choice.is_some() => lines.push(current_handler(ANOTHER_PROGRAM)),
        (None, None) => lines.push(NO_HANDLER.to_string()),
    }
    if let Some(entry) = &classes {
        let program = command_program(entry);
        let ours = Path::new(&program).file_name().is_some_and(|f| f.eq_ignore_ascii_case("tandem.exe"));
        let same = effective_program.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(&program));
        // An entry with no readable command is named only when another handler is named above.
        let hidden = program.is_empty() && effective_program.is_none() && name.is_none();
        if !ours && !same && !hidden {
            lines.push(replaces(if program.is_empty() { ANOTHER_PROGRAM } else { &program }));
        }
    }
    if choice.is_some() {
        lines.push(not_effective(scheme));
    }
    lines.join(" ")
}

/// The OK of the not-default question: registers, then binds only when Tandem really is the
/// default now. Err is the text to show (Windows kept another handler, or a write failed) and
/// nothing is bound.
pub fn register_and_bind(table: &mut toml::Table, reg: &mut dyn SchemeRegistry, scheme: &str, guid: &str, exe: &Path) -> Result<(), String> {
    if !is_listed(scheme) {
        return Err(format!("Tandem does not register for {scheme} links."));
    }
    reg.register(scheme, exe).map_err(|e| format!("Could not register Tandem for {scheme} links: {e}"))?;
    reg.record_exe(scheme, exe).map_err(|e| format!("Could not register Tandem for {scheme} links: {e}"))?;
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
    fn classes_command(&self, _: &str) -> Option<String> {
        None
    }
    fn register(&mut self, _: &str, _: &Path) -> Result<(), String> {
        Err("not Windows".into())
    }
}

#[cfg(windows)]
pub struct WinRegistry;

#[cfg(windows)]
pub(crate) mod win {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegGetValueW, RegSetValueExW};
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

    /// Whether `HKCU\<subkey>` exists.
    pub fn key_exists(subkey: &str) -> bool {
        use windows_sys::Win32::System::Registry::{KEY_READ, RegOpenKeyExW};
        let key = wide(subkey);
        let mut h: HKEY = std::ptr::null_mut();
        let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_READ, &mut h) };
        if rc == ERROR_SUCCESS {
            unsafe { RegCloseKey(h) };
        }
        rc == ERROR_SUCCESS
    }

    /// Removes the string value `name` (None: the key's default) of `HKCU\<subkey>` when present.
    pub fn delete_value(subkey: &str, name: Option<&str>) {
        use windows_sys::Win32::System::Registry::{KEY_SET_VALUE, RegDeleteValueW, RegOpenKeyExW};
        let key = wide(subkey);
        let mut h: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_SET_VALUE, &mut h) } != ERROR_SUCCESS {
            return;
        }
        let n = name.map(wide);
        unsafe {
            RegDeleteValueW(h, n.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()));
            RegCloseKey(h);
        }
    }

    /// Removes `HKCU\<subkey>` and everything under it: a key Tandem's own registration leaves
    /// stale, and the tests' throwaway key.
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
        // What a program that held these keys before left would still route the link to it (a
        // `DelegateExecute` verb, a DDE launch, another default verb): only these three go.
        for k in [&progid, &key] {
            win::delete_value(&format!("{k}\\shell\\open\\command"), Some("DelegateExecute"));
            win::delete_tree(&format!("{k}\\shell\\open\\ddeexec"));
            win::delete_value(&format!("{k}\\shell"), None);
        }
        win::set(&format!("{key}\\shell\\open\\command"), None, &command)
    }

    /// Reads back what `register_under` wrote: (scheme default, scheme command, ProgId command).
    pub fn read_under(root: &str, scheme: &str) -> (Option<String>, Option<String>, Option<String>) {
        (win::get(&format!("{root}\\{scheme}"), None), win::get(&format!("{root}\\{scheme}\\shell\\open\\command"), None), win::get(&format!("{root}\\Tandem.{scheme}\\shell\\open\\command"), None))
    }

    /// Tells the shell the file and protocol associations changed, so Default apps lists Tandem
    /// (documented for every new handler registration).
    fn notify_assoc_changed() {
        use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as _, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
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

    fn classes_command(&self, scheme: &str) -> Option<String> {
        let key = format!("Software\\Classes\\{scheme}\\shell\\open\\command");
        win::get(&key, None).or_else(|| win::key_exists(&key).then(String::new))
    }

    fn recorded_exe(&self, scheme: &str) -> Option<String> {
        is_listed(scheme).then(|| win::get(RECORDED, Some(scheme))).flatten()
    }

    fn record_exe(&mut self, scheme: &str, exe: &Path) -> Result<(), String> {
        if !is_listed(scheme) || !exe_ok(exe) {
            return Err(format!("Tandem does not register for {scheme} links."));
        }
        win::set(RECORDED, Some(scheme), &exe.to_string_lossy())
    }

    fn register(&mut self, scheme: &str, exe: &Path) -> Result<(), String> {
        // Listed in Default apps first (the seam writes only what differs); the protocol handler last.
        register_with(&mut crate::reg::HkcuRegistry, scheme, exe)?;
        Self::register_under("Software\\Classes", scheme, exe)?;
        Self::notify_assoc_changed();
        Ok(())
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
        classes: Option<String>,
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
        fn classes_command(&self, _: &str) -> Option<String> {
            self.classes.clone()
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
        assert_eq!(SCHEMES, ["ftp", "ssh", "telnet"]);
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

    /// Consent (task 2 fix round 1): the question names the effective handler AND the
    /// `HKCU\Classes` entry the OK replaces, and says so when Windows keeps another choice.
    #[test]
    fn the_question_names_every_handler_it_replaces() {
        let mut t = toml::Table::new();
        bind(&mut t, "ssh", "G1");
        let info = |reg: &Fake| match pick(&t, reg, "ssh", "G1", &exe()) {
            Pick::Ask { info, .. } => info,
            other => panic!("{other:?}"),
        };
        // The effective handler is PuTTY and HKCU\Classes\ssh holds another program's entry.
        let two = Fake { command: Some("putty.exe %1".into()), name: Some("PuTTY".into()), classes: Some("\"C:\\Tools\\sshx.exe\" %1".into()), ..Fake::default() };
        let text = info(&two);
        assert!(text.contains("The current handler is: PuTTY"), "{text}");
        assert!(text.contains(&replaces("C:\\Tools\\sshx.exe")), "{text}");
        // Protected: the same, and Tandem's entry will not take effect until chosen in Windows.
        let kept = Fake { choice: Some("PuTTY.ssh".into()), ..two };
        let text = info(&kept);
        assert!(text.contains(&replaces("C:\\Tools\\sshx.exe")) && text.contains(&not_effective("ssh")), "{text}");
        // The same program in both places is named once, as the handler.
        let same = Fake { command: Some("\"C:\\Tools\\sshx.exe\" %1".into()), classes: Some("\"C:\\Tools\\sshx.exe\" %1".into()), ..Fake::default() };
        assert!(!info(&same).contains(&replaces("C:\\Tools\\sshx.exe")), "{}", info(&same));
        // A handler whose command cannot be read (packaged app, DelegateExecute) is "another program",
        // never "no handler".
        let packaged = Fake { classes: Some(String::new()), ..Fake::default() };
        let text = info(&packaged);
        assert!(text.contains("The current handler is: another program") && !text.contains(NO_HANDLER), "{text}");
        let chosen = Fake { choice: Some("AppX123".into()), ..Fake::default() };
        assert!(info(&chosen).contains("The current handler is: another program"));
        // Nothing anywhere: "no handler".
        assert_eq!(info(&Fake::default()), NO_HANDLER);
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

    /// Final review: Windows documents SHChangeNotify(SHCNE_ASSOCCHANGED) after a handler is
    /// registered, so the shell and Default apps see Tandem (the call is the last thing
    /// `register` does, after the protocol handler key).
    #[test]
    fn a_successful_register_tells_the_shell() {
        // The anchors are built so this test's own text is not what is searched.
        let src = include_str!("url_schemes.rs");
        let real = src.split(&format!("impl SchemeRegistry for Win{}", "Registry {")).nth(1).expect("the Windows impl");
        let body = real.split(&format!("fn reg{}", "ister(")).nth(1).and_then(|r| r.split("\n    }\n").next()).expect("register");
        let handler = body.find("register_under(").expect("writes the handler");
        let notify = body.find(&format!("notify_assoc{}", "_changed()")).expect("tells the shell");
        assert!(notify > handler, "after the writes succeed");
    }

    #[cfg(windows)]
    #[test]
    fn registry_writes_under_hkcu() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = format!("Software\\Tandem\\test-{}-{nanos}", std::process::id());
        // A previous program's leftovers under the keys Tandem writes.
        win::set(&format!("{root}\\ssh\\shell\\open\\command"), Some("DelegateExecute"), "{GUID}").unwrap();
        win::set(&format!("{root}\\ssh\\shell\\open\\ddeexec"), None, "[open]").unwrap();
        win::set(&format!("{root}\\ssh\\shell"), None, "other").unwrap();
        let result = WinRegistry::register_under(&root, "ssh", &exe());
        let leftovers = (
            win::get(&format!("{root}\\ssh\\shell\\open\\command"), Some("DelegateExecute")),
            win::key_exists(&format!("{root}\\ssh\\shell\\open\\ddeexec")),
            win::get(&format!("{root}\\ssh\\shell"), None),
        );
        let read = WinRegistry::read_under(&root, "ssh");
        win::delete_tree(&root);
        assert_eq!(result, Ok(()));
        assert_eq!(leftovers, (None, false, None), "DelegateExecute, ddeexec and a default verb are cleared");
        let command = tandem_command(&exe());
        assert_eq!(read, (Some("URL:ssh Protocol".to_string()), Some(command.clone()), Some(command)));
        assert_eq!(win::get(&root, None), None, "the throwaway key is gone");
    }
}

#[cfg(test)]
mod registration_tests {
    use super::*;
    use crate::reg::{FakeRegistry, Registry};
    use std::path::PathBuf;

    fn exe() -> PathBuf {
        PathBuf::from(r"C:\T\tandem.exe")
    }

    #[test]
    fn schemes_follow_iterm2() {
        // P8: iTerm2's list narrowed to the schemes Tandem opens; the rest wait for a Browser profile.
        assert_eq!(SCHEMES, ["ftp", "ssh", "telnet"]);
        assert!(SCHEMES.windows(2).all(|w| w[0] < w[1]), "sorted as iTerm2's popup");
        assert!(SCHEMES.iter().all(|s| crate::url_launch::EXTERNAL_SCHEMES.contains(s)), "only what a link can open");
        assert_eq!(DEFAULT_APPS_URI, "ms-settings:defaultapps?registeredAppUser=Tandem");
        assert_eq!(LABEL, "URL schemes:");
    }

    #[test]
    fn registration_follows_windows_entries() {
        let entries = registry_entries(&exe(), "ssh");
        let has = |k: &str, n: &str, v: &str| entries.iter().any(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()) == (k, n, v));
        assert!(has("Software\\Classes\\Tandem.ssh", "", "URL:ssh Protocol"));
        assert!(has("Software\\Classes\\Tandem.ssh", "URL Protocol", ""));
        assert!(has("Software\\Classes\\Tandem.ssh\\shell\\open\\command", "", "\"C:\\T\\tandem.exe\" --url \"%1\""));
        assert!(has("Software\\Tandem\\Capabilities", "ApplicationName", "Tandem"));
        assert!(has("Software\\Tandem\\Capabilities\\URLAssociations", "ssh", "Tandem.ssh"));
        assert!(has("Software\\RegisteredApplications", "Tandem", "Software\\Tandem\\Capabilities"));
        // Every key is Tandem's own: the ProgId, its software key and its RegisteredApplications value.
        assert!(entries.iter().all(|(k, _, _)| crate::reg::deletable(k) || k == "Software\\RegisteredApplications"));
        assert_eq!(command_argument_shape(&tandem_command(&exe())), Some(r"C:\T\tandem.exe".to_string()));
        // Applied twice to a fake: the second pass writes nothing (idempotent).
        let mut r = FakeRegistry::default();
        assert_eq!(register_with(&mut r, "ssh", &exe()), Ok(entries.len()));
        assert_eq!(register_with(&mut r, "ssh", &exe()), Ok(0));
        assert_eq!(r.values.keys().filter(|(k, _)| k.to_ascii_lowercase().starts_with("software\\classes\\ssh")).count(), 0, "the scheme key itself is not written here");
    }

    #[test]
    fn a_system_scheme_is_never_registered() {
        let mut r = FakeRegistry::default();
        for s in ["http", "https", "file", "mailto", "ms-settings", "", "ssh\\x", "ssh/..", "ftp ", "Tandem.ssh"] {
            assert!(registry_entries(&exe(), s).is_empty(), "{s:?}");
            assert!(register_with(&mut r, s, &exe()).is_err(), "{s:?}");
            assert!(unregister_with(&mut r, s, &exe()).is_err(), "{s:?}");
        }
        assert!(r.values.is_empty(), "nothing was written");
        // The binding path refuses too, before any registry call.
        let mut t = toml::Table::new();
        let mut fake = FakeScheme::default();
        assert!(register_and_bind(&mut t, &mut fake, "http", "G1", &exe()).is_err());
        assert!(fake.registered.is_empty() && t.is_empty());
        // An exe path that could break out of the quoted command is refused.
        assert!(register_with(&mut r, "ssh", Path::new("C:\\T\\ta\"ndem.exe")).is_err());
        assert!(register_with(&mut r, "ssh", Path::new("tandem.exe")).is_err(), "absolute paths only");
    }

    #[test]
    fn unregistering_removes_only_what_tandem_wrote() {
        let mut r = FakeRegistry::default();
        register_with(&mut r, "ssh", &exe()).unwrap();
        register_with(&mut r, "telnet", &exe()).unwrap();
        // Someone else's keys beside Tandem's.
        r.set("Software\\Classes\\http\\shell\\open\\command", "", "firefox.exe %1").unwrap();
        r.set("Software\\Classes\\Tandemx", "", "x").unwrap();
        r.set("Software\\RegisteredApplications", "Other", "Software\\Other\\Capabilities").unwrap();
        // The scheme key Tandem took after the question holds Tandem's command; another program's stays.
        r.set("Software\\Classes\\ssh\\shell\\open\\command", "", &tandem_command(&exe())).unwrap();
        r.set("Software\\Classes\\telnet\\shell\\open\\command", "", "putty.exe %1").unwrap();
        unregister_with(&mut r, "ssh", &exe()).unwrap();
        assert!(r.get("Software\\Classes\\Tandem.ssh\\shell\\open\\command", "").is_none());
        assert!(r.get("Software\\Tandem\\Capabilities\\URLAssociations", "ssh").is_none());
        assert!(r.get("Software\\Classes\\ssh\\shell\\open\\command", "").is_none(), "Tandem's own scheme key goes");
        assert!(r.get("Software\\Tandem\\Capabilities\\URLAssociations", "telnet").is_some(), "another scheme stays");
        assert!(r.get("Software\\RegisteredApplications", "Tandem").is_some(), "still needed by telnet");
        unregister_with(&mut r, "telnet", &exe()).unwrap();
        assert_eq!(r.get("Software\\Classes\\telnet\\shell\\open\\command", "").as_deref(), Some("putty.exe %1"), "another program's entry is never touched");
        assert!(r.get("Software\\RegisteredApplications", "Tandem").is_none(), "the last scheme removes the listing");
        assert!(r.get("Software\\Tandem\\Capabilities", "ApplicationName").is_none());
        assert_eq!(r.get("Software\\RegisteredApplications", "Other").as_deref(), Some("Software\\Other\\Capabilities"));
        assert_eq!(r.get("Software\\Classes\\http\\shell\\open\\command", "").as_deref(), Some("firefox.exe %1"));
        assert_eq!(r.get("Software\\Classes\\Tandemx", "").as_deref(), Some("x"));
        // The fake refuses a delete outside Tandem's keys as the real writer does.
        assert!(r.delete_value("Software\\Classes\\http\\shell\\open\\command", "").is_err());
        assert!(r.delete_value("Software\\RegisteredApplications", "Other").is_err());
        assert!(r.delete_scheme_class("http").is_err());
    }

    /// A registry that tells the effective handler (`command`, which may come from HKLM or a
    /// UserChoice ProgId) apart from the HKCU scheme key (`classes`), and keeps the exe path the
    /// person's OK registered (`recorded`).
    #[derive(Default)]
    struct FakeScheme {
        command: Option<String>,
        classes: Option<String>,
        recorded: Option<String>,
        registered: Vec<String>,
    }

    impl SchemeRegistry for FakeScheme {
        fn handler_command(&self, _: &str) -> Option<String> {
            self.command.clone()
        }
        fn handler_name(&self, _: &str) -> Option<String> {
            None
        }
        fn user_choice(&self, _: &str) -> Option<String> {
            None
        }
        fn classes_command(&self, _: &str) -> Option<String> {
            self.classes.clone()
        }
        fn register(&mut self, scheme: &str, exe: &Path) -> Result<(), String> {
            self.registered.push(scheme.to_string());
            self.command = Some(tandem_command(exe));
            self.classes = Some(tandem_command(exe));
            Ok(())
        }
        fn recorded_exe(&self, _: &str) -> Option<String> {
            self.recorded.clone()
        }
        fn record_exe(&mut self, _: &str, exe: &Path) -> Result<(), String> {
            self.recorded = Some(exe.to_string_lossy().into_owned());
            Ok(())
        }
    }

    /// Task 4 fix round 1, item 1: the startup repair rewrites a scheme only when HKCU still holds
    /// exactly the exe path the person's OK registered AND that exe is gone. Another live copy, an
    /// HKLM-only registration and a binding imported without an OK on this machine are left alone.
    #[test]
    fn a_moved_exe_is_repaired_only_for_what_the_ok_registered() {
        let old = PathBuf::from(r"D:\Old\tandem.exe");
        let old_s = old.to_string_lossy().into_owned();
        let mut t = toml::Table::new();
        bind(&mut t, "ssh", "G1");
        let asked = std::cell::RefCell::new(Vec::new());
        let gone = |p: &Path| {
            asked.borrow_mut().push(p.to_path_buf());
            true
        };
        let alive = |_: &Path| false;
        // The person's OK registered D:\Old, HKCU still holds it, and D:\Old is gone: rewritten,
        // and the record follows the new path.
        let mut moved = FakeScheme { command: Some(tandem_command(&old)), classes: Some(tandem_command(&old)), recorded: Some(old_s.clone()), ..FakeScheme::default() };
        assert_eq!(stale_schemes(&t, &moved, &exe(), &gone), vec!["ssh".to_string()]);
        assert_eq!(asked.borrow().as_slice(), [old.clone()], "only the recorded path is probed");
        assert_eq!(repair(&t, &mut moved, &exe(), &gone), vec!["ssh".to_string()]);
        assert_eq!(moved.registered, ["ssh"]);
        assert_eq!(moved.recorded.as_deref(), Some(r"C:\T\tandem.exe"));
        assert!(stale_schemes(&t, &moved, &exe(), &gone).is_empty(), "now current");
        // Two live copies: the other copy still exists, so its registration stays.
        let mut live = FakeScheme { command: Some(tandem_command(&old)), classes: Some(tandem_command(&old)), recorded: Some(old_s.clone()), ..FakeScheme::default() };
        assert!(repair(&t, &mut live, &exe(), &alive).is_empty());
        assert!(live.registered.is_empty() && live.recorded.as_deref() == Some(old_s.as_str()));
        // A machine-wide (HKLM) Tandem registration: the effective handler is Tandem's old path but
        // HKCU holds nothing, so nothing is written.
        let mut hklm = FakeScheme { command: Some(tandem_command(&old)), recorded: Some(old_s.clone()), ..FakeScheme::default() };
        assert!(repair(&t, &mut hklm, &exe(), &gone).is_empty());
        assert!(hklm.registered.is_empty());
        // An imported table (bound, HKCU holds a Tandem command) with no OK recorded on this machine.
        let mut imported = FakeScheme { command: Some(tandem_command(&old)), classes: Some(tandem_command(&old)), ..FakeScheme::default() };
        assert!(repair(&t, &mut imported, &exe(), &gone).is_empty());
        assert!(imported.registered.is_empty());
        // HKCU holds another Tandem path than the recorded one (or a look-alike): not what the OK wrote.
        let other_tandem = PathBuf::from(r"E:\Else\tandem.exe");
        let mut differs = FakeScheme { classes: Some(tandem_command(&other_tandem)), recorded: Some(old_s.clone()), ..FakeScheme::default() };
        assert!(repair(&t, &mut differs, &exe(), &gone).is_empty());
        for cmd in ["\"D:\\Old\\tandem.exe\" --url \"%1\" --x", "putty.exe %1", ""] {
            let mut f = FakeScheme { classes: Some(cmd.into()), recorded: Some(old_s.clone()), ..FakeScheme::default() };
            assert!(repair(&t, &mut f, &exe(), &gone).is_empty(), "{cmd}");
        }
        // An unbound scheme, or a binding for a scheme Tandem does not list, is never repaired.
        let full = || FakeScheme { classes: Some(tandem_command(&old)), recorded: Some(old_s.clone()), ..FakeScheme::default() };
        assert!(stale_schemes(&toml::Table::new(), &full(), &exe(), &gone).is_empty());
        let mut odd = toml::Table::new();
        bind(&mut odd, "http", "G1");
        assert!(stale_schemes(&odd, &full(), &exe(), &gone).is_empty());
    }

    /// The person's OK records the exe path it registered (the only evidence a later repair trusts).
    #[test]
    fn the_ok_records_the_registered_exe() {
        let mut reg = FakeScheme::default();
        let mut t = toml::Table::new();
        register_and_bind(&mut t, &mut reg, "ssh", "G1", &exe()).unwrap();
        assert_eq!(reg.recorded.as_deref(), Some(r"C:\T\tandem.exe"));
    }

    #[test]
    fn row_items_follow_iterm2() {
        let mut t = toml::Table::new();
        bind(&mut t, "ssh", "G1");
        let reg = FakeScheme { command: Some(tandem_command(&exe())), ..FakeScheme::default() };
        let items = row_items(&t, &reg, "G1", &exe());
        assert_eq!(items[0], (SELECT.to_string(), false));
        assert_eq!(items[1..], [("ftp".to_string(), false), ("ssh".to_string(), true), ("telnet".to_string(), false)]);
        // Bound but Tandem is not the default: unchecked.
        let off = row_items(&t, &FakeScheme::default(), "G1", &exe());
        assert!(off.iter().all(|(_, on)| !on));
        // Bound to another profile: unchecked for this one.
        assert!(!row_items(&t, &reg, "G2", &exe()).iter().any(|(_, on)| *on));
    }
}
