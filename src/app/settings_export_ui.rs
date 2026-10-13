//! Export, Import and Erase All Settings and Data, the app side (`IE:19-329`, `GP:4084-4108`).
//! Export: the save panel, one folder per entity in a temporary folder, the gzip tar, and the
//! reveal or the failure alert. Import: the open panel, the warning, the archive extracted and
//! named by a marker, the restart, and at the next start (before anything loads) the settings,
//! the files and the scripts applied. Erase: the offer to export first, the confirmation, and
//! every file removed (or, in a dry run, logged). The model is `crate::settings_export`.

use std::path::{Path, PathBuf};

use winit::event_loop::ActiveEventLoop;

use super::App;
use super::scripts_alerts::{ask, Then};
use crate::warnings::Kind;
use crate::globals::Globals;
use crate::remote_prefs::Snapshot;
use crate::script_runtime as rt;
use crate::settings_export::{self as se, Entity, ExportError, Pending, Requirement};
use crate::store::Store;

/// `revealExportedSettingsAndData` (`ASM:558`).
const REVEAL_KEY: &str = "RevealExportedSettingsAndData";
/// `dryRunEraseAllSettingsAndData` (`ASM:559`).
const DRY_RUN_KEY: &str = "DryRunEraseAllSettingsAndData";
/// The variable naming the process a relaunched Tandem waits for (`IE:898-911`).
pub(super) const WAIT_PID_VAR: &str = "TERMURION_WAIT_PID";

/// Where the export reads from.
struct Sources<'a> {
    /// `%APPDATA%\Termurion`.
    app_support: &'a Path,
    /// The Scripts folder; None when it is on a network path (never read).
    scripts: Option<&'a Path>,
    /// The syncable settings and the profiles.
    user_defaults: &'a Snapshot,
    /// Whether a path in the Scripts folder is read: false for one that is, or is reached
    /// through a link or junction to, a network path (`scripts::ancestors_stay_local`).
    local: &'a dyn Fn(&Path) -> bool,
}

/// The entries of the Scripts folder and of its AutoLaunch folder that are read, AutoLaunch's
/// flagged: an entry that is not `local`, or every entry of an AutoLaunch folder that is not,
/// is skipped before anything probes it (as 11d's scan does).
fn script_entries(scripts: &Path, local: &dyn Fn(&Path) -> bool) -> Vec<(String, PathBuf, bool)> {
    let autolaunch = scripts.join("AutoLaunch");
    let root = entries(scripts).into_iter().map(|(n, p)| (n, p, false));
    let auto = if local(&autolaunch) { entries(&autolaunch) } else { Vec::new() };
    root.chain(auto.into_iter().map(|(n, p)| (n, p, true))).filter(|(_, p, _)| local(p)).collect()
}

/// A folder's entries by name, sorted; none when it cannot be read.
fn entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).collect()).unwrap_or_default();
    v.sort();
    v
}

/// A symbolic link, a junction or any other reparse point (`FILE_ATTRIBUTE_REPARSE_POINT`).
fn is_reparse(p: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    p.symlink_metadata().is_ok_and(|m| m.file_attributes() & 0x400 != 0)
}

/// The name the existing entry at `p` really has (its final path's last part, the reparse point
/// itself never followed); None when there is none.
fn final_name(p: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, GetFinalPathNameByHandleW};
    let f = std::fs::OpenOptions::new().access_mode(0).custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).open(p).ok()?;
    let mut buf = vec![0u16; 1024];
    // SAFETY: an open handle and a buffer of the length given.
    let n = unsafe { GetFinalPathNameByHandleW(f.as_raw_handle() as _, buf.as_mut_ptr(), buf.len() as u32, 0) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    let path = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]));
    Some(path.file_name()?.to_string_lossy().into_owned())
}

/// Whether the archive's entry `name` may be copied to `dest`: never a name Import keeps out
/// (`se::kept_from_import`: hidden, excluded, or one Windows reads as another entry), and never
/// onto an existing entry whose real name is another (an alias the rule did not foresee) or is
/// itself kept out (Task 9 findings r2), and never onto an existing link, junction or other reparse
/// point, which the copy would follow (Task 9 re-review r2).
fn importable(name: &str, dest: &Path, top: bool) -> bool {
    if se::kept_from_import(name, top) || is_reparse(dest) {
        return false;
    }
    match final_name(dest) {
        Some(real) => real.eq_ignore_ascii_case(name) && !se::kept_from_import(&real, top),
        None => !dest.exists() && dest.symlink_metadata().is_err(),
    }
}

/// `deepCopyContentsOfDirectory` (`IE:1163-1208`): every entry of `src` but the hidden ones
/// into `dst`, folders recursively; at the top level the excluded names stay out too, and at
/// every level a name that is (or reaches) another entry (`importable`). Links and junctions are
/// not followed (Decision: nothing outside the folder is copied).
fn copy_contents(src: &Path, dst: &Path, top: bool) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for (name, path) in entries(src) {
        if !importable(&name, &dst.join(&name), top) || is_reparse(&path) {
            continue;
        }
        if path.is_dir() {
            copy_contents(&path, &dst.join(&name), false)?;
        } else {
            std::fs::copy(&path, dst.join(&name))?;
        }
    }
    Ok(())
}

/// `PythonRuntimesImporterExporter.export` (`IE:701-743`): the `setup.cfg` of every folder at
/// the Scripts root and in AutoLaunch, each requirement once; a `.py` file at those levels is
/// a basic script. A `setup.cfg` without `python_requires` needs the default version.
fn runtime_requirements(scripts: &Path, local: &dyn Fn(&Path) -> bool) -> (Vec<Requirement>, bool) {
    let (mut requirements, mut have_basic) = (Vec::<Requirement>::new(), false);
    for (name, path, _) in script_entries(scripts, local) {
        if path.is_file() {
            have_basic |= name.ends_with(".py");
            continue;
        }
        let Some(cfg) = std::fs::read_to_string(path.join("setup.cfg")).ok().and_then(|t| rt::SetupCfg::parse(&t)) else { continue };
        let r = Requirement { minimum_runtime: cfg.environment_version, python_version: cfg.python_version.unwrap_or_else(|| rt::DEFAULT_PYTHON.to_string()) };
        if !requirements.contains(&r) {
            requirements.push(r);
        }
    }
    (requirements, have_basic)
}

/// `ScriptsImporterExporter.export` (`IE:995-1053`): each script at the Scripts root but the
/// hidden entries and AutoLaunch, then each in AutoLaunch, as an unsigned zip; anything that is
/// not a script is skipped.
fn export_scripts(scripts: &Path, dest: &Path, local: &dyn Fn(&Path) -> bool) -> Result<(), String> {
    let wanted = |(n, p, auto): (String, PathBuf, bool)| (auto || !(n.starts_with('.') || n.eq_ignore_ascii_case("autolaunch"))).then_some((p, auto));
    for (path, autolaunch) in script_entries(scripts, local).into_iter().filter_map(wanted) {
        if !super::scripts_app::is_exportable_script(&path) {
            continue;
        }
        let uuid = uuid::Uuid::new_v4().to_string().to_uppercase();
        super::scripts_app::export_script_to(&path, &dest.join(se::script_zip_name(autolaunch, &uuid)), autolaunch)?;
    }
    Ok(())
}

/// One entity into its folder under `temp` (`IE:569-597`).
fn export_entity(entity: Entity, temp: &Path, src: &Sources) -> Result<(), ExportError> {
    let dest = temp.join(entity.key());
    std::fs::create_dir_all(&dest).map_err(|e| ExportError::Folder(e.to_string()))?;
    let save = |file: &str, text: String| std::fs::write(dest.join(file), text).map_err(|e| ExportError::Save(e.to_string()));
    match entity {
        Entity::PythonRuntimes => {
            let (requirements, have_basic) = src.scripts.map(|s| runtime_requirements(s, src.local)).unwrap_or_default();
            save(se::RUNTIMES_FILE, se::runtimes_json(&requirements, have_basic))
        }
        Entity::UserDefaults => save(se::USER_DEFAULTS_FILE, src.user_defaults.to_toml()),
        Entity::AppSupport => {
            // A missing folder exports nothing (`IE:626-628`).
            if !src.app_support.is_dir() {
                return Ok(());
            }
            copy_contents(src.app_support, &dest, true).map_err(|e| ExportError::Copy(e.to_string()))
        }
        Entity::Scripts => src.scripts.map_or(Ok(()), |s| export_scripts(s, &dest, src.local).map_err(ExportError::Script)),
    }
}

/// `Exporter.export(to:)` (`IE:551-560`): the entities in a fresh `%TEMP%\<uuid>`, then the
/// archive at `out`. The temporary folder is removed either way.
fn export_into(out: &Path, src: &Sources) -> Result<(), ExportError> {
    let temp = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&temp).map_err(|e| ExportError::TempDir(e.to_string()))?;
    let result = se::ENTITIES.iter().try_for_each(|&e| export_entity(e, &temp, src)).and_then(|_| {
        super::scripts_app::run_tar(se::archive_args(out, &temp)).map_err(|e| {
            let _ = std::fs::remove_file(out);
            ExportError::Archive(e)
        })
    });
    let _ = std::fs::remove_dir_all(&temp);
    result
}

/// Where the staged script zips wait in `%APPDATA%\Termurion` for the app to import them.
pub(super) const IMPORT_SCRIPTS_DIR: &str = ".import-scripts";

/// `%APPDATA%\Termurion`, the folder holding the settings.
fn appdata_termurion() -> PathBuf {
    let settings = crate::globals::default_path();
    settings.parent().unwrap_or(&settings).to_path_buf()
}

/// An extracted export holding a link, a junction or a file with another hard link is refused
/// (11d's rule for script zips, `sa::unsafe_entry`), so nothing the import reads or copies can
/// reach outside the staging folder or onto a network path.
fn staged_links(staging: &Path) -> Result<(), String> {
    if let Some(entry) = alias_entry(staging) {
        let shown = entry.strip_prefix(staging).unwrap_or(&entry).display().to_string();
        return Err(crate::zip_check::refused(&format!("\u{201c}{shown}\u{201d} has a name Windows reads as another item")));
    }
    match crate::script_archive::unsafe_entry(staging, &super::scripts_app::stage_entries) {
        Some(entry) => Err(crate::script_archive::unsafe_entry_text(entry.strip_prefix(staging).unwrap_or(&entry))),
        None => Ok(()),
    }
}

/// The first entry under `dir` whose name Windows may read as another entry (`se::aliased`: a
/// trailing dot or space, a stream, a device name, an 8.3 short name), walked without following
/// anything (Task 9 findings r2: such a name could reach a protected file of `%APPDATA%\Termurion`).
fn alias_entry(dir: &Path) -> Option<PathBuf> {
    for (name, stat) in super::scripts_app::stage_entries(dir) {
        let path = dir.join(&name);
        if se::aliased(&name) {
            return Some(path);
        }
        if stat == crate::script_archive::Stat::Dir {
            if let Some(found) = alias_entry(&path) {
                return Some(found);
            }
        }
    }
    None
}

/// Import's first phase (`Importer.importEntities`, `IE:81-110`): `file` read ONCE into memory, every
/// entry checked and then written from those same bytes by Tandem's own walk (`extract::tgz`, the
/// rule every Tandem extraction uses: plain files and folders only, no link of any kind, no name
/// that leaves the folder or that Windows reads as another, Task 9 findings r3; no tar.exe, so no
/// second parser and no reopen by path, final findings r2) into a fresh
/// `<temp>\.termurion-import-<uuid>`, refused when it holds a link after all (`staged_links`), then
/// the marker in `appdata` naming it. On a failure nothing is left behind.
fn stage_import(file: &Path, temp: &Path, appdata: &Path) -> Result<(), String> {
    // The archive came from outside: judged before it is read (as a script zip is), read once.
    let archive = crate::extract::read_whole(file, crate::extract::MAX_ARCHIVE)?;
    let staging = se::staging_dir(temp, &uuid::Uuid::new_v4().to_string());
    let staged = crate::extract::tgz(&archive, &staging, crate::zip_check::Limits::DEFAULT, &se::aliased)
        .and_then(|_| staged_links(&staging))
        .and_then(|_| std::fs::create_dir_all(appdata).map_err(|e| format!("{}: {e}", appdata.display())))
        .and_then(|_| std::fs::write(appdata.join(se::MARKER), staging.to_string_lossy().as_bytes()).map_err(|e| format!("{}: {e}", appdata.join(se::MARKER).display())));
    if staged.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    staged
}

/// The staged User Defaults entity applied to the files on disk, as a custom folder's settings
/// are at launch: the syncable settings replace the local ones in `settings.toml`, and the
/// profiles and the default replace `profiles.toml`. Loading from a custom folder or URL is
/// then turned off and its location unset (`IE:836-852`), or the load that follows at this
/// same start would replace what was imported. A file that cannot be read is never
/// overwritten (as at launch); an entity that is missing or empty changes nothing.
fn apply_user_defaults(file: &Path, appdata: &Path) {
    let Some(snap) = std::fs::read_to_string(file).ok().and_then(|t| Snapshot::parse(&t)) else { return };
    let (Ok(mut g), Ok(mut store)) = (Globals::load(&appdata.join("settings.toml")), Store::load(&appdata.join("profiles.toml"))) else { return };
    super::prefs_ui::apply_snapshot(&snap, &mut g, &mut store);
    g.set(crate::remote_prefs::LOAD_KEY, toml::Value::Boolean(false));
    g.unset(crate::remote_prefs::FOLDER_KEY);
    let _ = g.save();
    let _ = store.save();
}

/// The staged script zips moved from `from` to `to`, in name order; where they are now.
fn stage_scripts(from: &Path, to: &Path) -> Vec<PathBuf> {
    let mut moved = Vec::new();
    for (name, path) in entries(from) {
        if name.starts_with('.') || !path.is_file() || std::fs::create_dir_all(to).is_err() {
            continue;
        }
        let dest = to.join(&name);
        let ok = std::fs::rename(&path, &dest).is_ok() || std::fs::copy(&path, &dest).is_ok();
        if ok {
            moved.push(dest);
        }
    }
    moved
}

/// Import's final phase (`finishImporting`, `IE:112-120`), at start before the settings and
/// profiles load, so it edits the files: with a marker naming a staged export, the marker goes
/// first (an import that stops half way is never applied twice), then the settings and
/// profiles, the Application Support files (hidden and excluded names kept out, links not
/// followed: the Python API consent never comes from an archive), and the script zips moved to
/// `.import-scripts`; the staging folder goes last. A stale marker is removed and nothing
/// changes; a staging folder that now holds a link (`staged_links`) is removed unapplied.
/// Returns the zips for the app to import once it runs. The Python Runtimes entity installs
/// nothing: each script is provisioned on its first launch (`IE:788-793`, under uv).
pub(super) fn finish_import(appdata_termurion: &Path) -> Vec<PathBuf> {
    let marker = appdata_termurion.join(se::MARKER);
    let text = std::fs::read_to_string(&marker).ok();
    let staging = match se::pending(text.as_deref(), &|p| crate::localfs::is_local_dir(p)) {
        Pending::None => return Vec::new(),
        Pending::Stale => {
            let _ = std::fs::remove_file(&marker);
            return Vec::new();
        }
        Pending::Apply(staging) => staging,
    };
    if std::fs::remove_file(&marker).is_err() {
        return Vec::new();
    }
    // Checked again at start: the staging folder sat in %TEMP% across the restart.
    if staged_links(&staging).is_err() {
        let _ = std::fs::remove_dir_all(&staging);
        return Vec::new();
    }
    apply_user_defaults(&staging.join(Entity::UserDefaults.key()).join(se::USER_DEFAULTS_FILE), appdata_termurion);
    let support = staging.join(Entity::AppSupport.key());
    if support.is_dir() && !is_reparse(&support) {
        let carries_state = support.join("state").is_file() && !is_reparse(&support.join("state"));
        let _ = copy_contents(&support, appdata_termurion, true);
        // An imported state is another person's or machine's document: its restore starts no hidden member.
        if carries_state {
            let _ = std::fs::write(appdata_termurion.join(se::STATE_IMPORTED), b"");
        }
    }
    let zips = stage_scripts(&staging.join(Entity::Scripts.key()), &appdata_termurion.join(IMPORT_SCRIPTS_DIR));
    let _ = std::fs::remove_dir_all(&staging);
    zips
}

/// Waits up to 30 seconds for the process `pid` to end (`IE:898-911`, the relaunch waiting for
/// the old Tandem); at once when it cannot be opened (already gone).
pub(super) fn wait_for_pid(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    // SAFETY: a handle opened here, waited on and closed here.
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return;
        }
        WaitForSingleObject(h, 30_000);
        CloseHandle(h);
    }
}

/// Starts Tandem again (the current executable) telling it to wait for this process to end.
pub(super) fn start_relaunched() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe).env(WAIT_PID_VAR, std::process::id().to_string()).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Every entry of `dir`, hidden ones included; empty when it does not exist.
fn all_entries(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    match std::fs::read_dir(dir) {
        Ok(rd) => {
            let mut v = rd.map(|e| e.map(|e| e.path())).collect::<std::io::Result<Vec<_>>>()?;
            v.sort();
            Ok(v)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// One line in the debug output (`OutputDebugStringW`, iTerm2's `NSLog`).
fn debug_line(text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated wide string that outlives the call.
    unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringW(wide.as_ptr()) };
}

impl App {
    /// General > Settings' Export All Settings and Data (`performExportFlow`, `IE:19-69`): the
    /// save panel (Cancel does nothing), the export, then the file revealed in Explorer or
    /// "Problem Exporting" with the reason (`GP:4084-4086`, persistent).
    pub(super) fn export_all(&mut self) {
        self.export_flow(se::PROBLEM_EXPORTING);
    }

    /// `performExportFlow`: the save panel and the export. None: canceled. Some(false): failed,
    /// and the reason is queued under `failed_heading`. Some(true): written (and revealed).
    fn export_flow(&mut self, failed_heading: &str) -> Option<bool> {
        let dialog = rfd::FileDialog::new().set_title(se::PANEL_TITLE).set_file_name(se::DEFAULT_NAME).add_filter(se::FILTER_NAME, &[se::EXTENSION]);
        let mut out = dialog.save_file()?;
        // A name typed without the extension gets it, as the save panel's content type does.
        if out.extension().is_none() {
            out.set_extension(se::EXTENSION);
        }
        match self.export_to(&out) {
            Ok(()) => {
                if self.globals.bool(REVEAL_KEY) {
                    self.reveal_path(&out);
                }
                Some(true)
            }
            Err(e) => {
                self.alert_ok(failed_heading, e.message(), Then::ExportFailed);
                Some(false)
            }
        }
    }

    /// A persistent alert with OK alone.
    fn alert_ok(&mut self, heading: &str, text: String, then: Then) {
        self.ask_scripts(ask(heading, &text, &[crate::api_auth::OK], None, Kind::Persistent, then));
    }

    /// General > Settings' Import All Settings and Data (`importAll`, `IE:81-110`): the open
    /// panel (Cancel does nothing), then "Importing Settings and Data" (OK, Cancel; persistent).
    pub(super) fn import_all(&mut self) {
        let Some(file) = rfd::FileDialog::new().add_filter(se::FILTER_NAME, &[se::EXTENSION]).pick_file() else { return };
        let buttons = [crate::api_auth::OK, crate::api_auth::CANCEL];
        self.ask_scripts(ask(se::IMPORT_HEADING, se::IMPORT_BODY, &buttons, None, Kind::Persistent, Then::ImportConfirm(file)));
    }

    /// OK in "Importing Settings and Data": the archive is extracted and named for the next
    /// start, then Tandem quits (no quit confirmation; sessions are kept as after any quit) and
    /// starts again. A failure says "Problem Importing" with the reason.
    pub(super) fn import_confirmed(&mut self, file: PathBuf) {
        match stage_import(&file, &std::env::temp_dir(), &appdata_termurion()) {
            Ok(()) => self.relaunch_due = true,
            Err(e) => self.alert_ok(se::PROBLEM_IMPORTING, e, Then::Nothing),
        }
    }

    /// The quit path without its confirmation, for the restart after an import (`NSApp
    /// relaunch`); `run()` starts the new Tandem once the event loop has ended.
    pub(super) fn relaunch(&mut self, event_loop: &ActiveEventLoop) {
        self.relaunch_on_exit = true;
        self.quit_confirmed(event_loop);
    }

    /// At start: the zips `finish_import` left are imported one by one as iTerm2's import does
    /// with `avoidUI` (no questions, a script of the same name replaced, AutoLaunch from the
    /// name), and `.import-scripts` goes once the last has finished.
    pub(super) fn import_restored_scripts(&mut self) {
        let zips = std::mem::take(&mut self.restored_scripts);
        let Some(dir) = zips.first().and_then(|z| z.parent()).map(Path::to_path_buf) else { return };
        self.restored_dir = Some(dir);
        for zip in zips {
            let autolaunch = zip.file_name().is_some_and(|n| n.to_string_lossy().starts_with("autolaunch"));
            self.import_quietly(zip, autolaunch);
        }
    }

    /// The import queue is empty: `.import-scripts` goes.
    pub(super) fn restored_scripts_done(&mut self) {
        if let Some(dir) = self.restored_dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// General > Settings' Erase All Settings and Data (`eraseAll`, `IE:122-222`): first the
    /// offer to export (Export First, Skip Export, Cancel; persistent).
    pub(super) fn erase_all(&mut self) {
        let buttons = [se::EXPORT_FIRST, se::SKIP_EXPORT, crate::api_auth::CANCEL];
        self.ask_scripts(ask(se::ERASE_HEADING, se::EXPORT_FIRST_BODY, &buttons, None, Kind::Persistent, Then::EraseExportFirst));
    }

    /// The answer to the offer to export: Export First runs the export (canceled: the erase
    /// stops; failed: "Problem Exporting Settings and Data" and it stops); then, as after Skip
    /// Export, the confirmation, with Cancel first so Enter cancels.
    pub(super) fn erase_export_answered(&mut self, index: usize) {
        match index {
            0 if self.export_flow(se::PROBLEM_EXPORTING_ERASE) != Some(true) => return,
            0 | 1 => {}
            _ => return,
        }
        let dry_run = self.globals.bool(DRY_RUN_KEY);
        let (heading, text, action) = if dry_run {
            (se::DRY_RUN_HEADING, se::DRY_RUN_BODY.to_string(), se::DRY_RUN_ACTION)
        } else {
            (se::ERASE_CONFIRM_HEADING, se::erase_confirm_body(), se::ERASE_ACTION)
        };
        let buttons = [crate::api_auth::CANCEL, action];
        self.ask_scripts(ask(heading, &text, &buttons, None, Kind::Persistent, Then::EraseConfirm { dry_run }));
    }

    /// The second button of the confirmation (`Eraser`, `IE:224-329`): the Scripts folder is
    /// resolved first as the one Tandem uses (`scripts_folder`, iTerm2's `scriptsPath`; its
    /// settings are erased with the rest; one on, or reached through a link to, a network path
    /// is left alone), then the list of every entry of `%APPDATA%\Termurion`
    /// and of that folder. A dry run logs each path and says so; otherwise the host's shells
    /// end, every path goes (links as links) and Tandem exits at once, saving nothing (iTerm2's
    /// `_exit(0)`). Credential Manager entries are not touched.
    pub(super) fn erase_confirmed(&mut self, dry_run: bool) {
        let scripts = se::scripts_to_erase(self.scripts_folder()).filter(|s| crate::scripts::ancestors_stay_local(s, &crate::netpath::read_reparse));
        let termurion = appdata_termurion();
        let listed = all_entries(&termurion).map_err(|e| format!("{}: {e}", termurion.display())).and_then(|own| {
            let scripts = match &scripts {
                Some(s) => Some((s.as_path(), all_entries(s).map_err(|e| format!("{}: {e}", s.display()))?)),
                None => None,
            };
            Ok(se::erase_list(&own, scripts))
        });
        let list = match listed {
            Ok(list) => list,
            Err(e) => return self.alert_ok(se::PROBLEM_ERASING, e, Then::Nothing),
        };
        if dry_run {
            for path in &list {
                debug_line(&se::dry_run_line(path));
            }
            return self.alert_ok(se::DRY_RUN_DONE_HEADING, se::DRY_RUN_DONE_BODY.to_string(), Then::Nothing);
        }
        self.leave_host(false);
        for path in &list {
            super::scripts_app::remove_path(path);
        }
        // Every exit tears the process down: the keyboard hook, the layout, the icon.
        super::teardown_process();
        std::process::exit(0);
    }

    /// Writes the archive at `out`: the settings and profiles as they are now, `%APPDATA%\Termurion`
    /// and the Scripts folder. A Scripts folder on a network path is not read.
    pub(super) fn export_to(&mut self, out: &Path) -> Result<(), ExportError> {
        self.flush_notes();
        let settings = crate::globals::default_path();
        let app_support = settings.parent().unwrap_or(&settings).to_path_buf();
        let scripts = self.scripts_folder();
        let local = crate::scripts::ancestors_stay_local(&scripts, &crate::netpath::read_reparse) && !crate::remote_prefs::is_unc(&scripts.to_string_lossy());
        let user_defaults = Snapshot::from_local(&self.globals.values, &self.store.saved_default_guid(), &self.store.file_profiles());
        let entry_local = |p: &Path| crate::scripts::ancestors_stay_local(p, &crate::netpath::read_reparse);
        export_into(out, &Sources { app_support: &app_support, scripts: local.then_some(scripts.as_path()), user_defaults: &user_defaults, local: &entry_local })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("termurion-export-test-{name}-{}", crate::api_auth::fresh_key()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tar(args: &[&str]) -> String {
        let out = std::process::Command::new(crate::script_provision::RealRunner::system_tool("tar.exe").unwrap()).args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).replace('\\', "/")
    }

    /// The local settings as `run()` would load them from `settings.toml`.
    fn user_defaults(app_support: &Path) -> Snapshot {
        let g = crate::globals::Globals::load(&app_support.join("settings.toml")).unwrap();
        Snapshot::from_local(&g.values, "", &[])
    }

    #[test]
    fn export_writes_the_archive() {
        let root = temp("archive");
        let appdata = root.join("Termurion");
        std::fs::create_dir_all(appdata.join("Scripts")).unwrap();
        std::fs::write(appdata.join("settings.toml"), "[settings]\nFocusFollowsMouse = true\nNoSyncX = true\n").unwrap();
        std::fs::write(appdata.join("profiles.toml"), "default = \"g\"\n").unwrap();
        std::fs::write(appdata.join("snippets.json"), "[]").unwrap();
        std::fs::write(appdata.join("host.log"), "log").unwrap();
        std::fs::write(appdata.join(crate::workgroup_restore::KEY_FILE), [1u8; 32]).unwrap();
        let snap = user_defaults(&appdata);
        let out = root.join("Tandem State.termurionexport");
        let scripts = appdata.join("Scripts");
        export_into(&out, &Sources { app_support: &appdata, scripts: Some(&scripts), user_defaults: &snap, local: &|_| true }).unwrap();
        assert!(out.is_file());

        let listing = tar(&["-tzf", &out.to_string_lossy()]);
        let names: Vec<&str> = listing.lines().map(|l| l.trim_end_matches('/')).collect();
        for want in ["user-defaults/UserDefaults.toml", "app-support/snippets.json", "python-runtimes/PythonRuntimes.json", "scripts"] {
            assert!(names.contains(&want), "{want} in {names:?}");
        }
        for never in ["app-support/host.log", "app-support/settings.toml", "app-support/profiles.toml", "app-support/Scripts", "app-support/workgroup-approval-key"] {
            assert!(!names.contains(&never), "{never} in {names:?}");
        }

        let x = root.join("x");
        std::fs::create_dir_all(&x).unwrap();
        tar(&["-xzf", &out.to_string_lossy(), "-C", &x.to_string_lossy()]);
        let back = Snapshot::parse(&std::fs::read_to_string(x.join(r"user-defaults\UserDefaults.toml")).unwrap()).unwrap();
        assert_eq!(back.settings.get("FocusFollowsMouse"), Some(&toml::Value::Boolean(true)));
        assert!(!back.settings.contains_key("NoSyncX"));
        assert_eq!(std::fs::read_to_string(x.join(r"python-runtimes\PythonRuntimes.json")).unwrap(), r#"{"requirements":[],"haveAnyBasicScripts":false}"#);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Beyond the plan: scripts at the root and in AutoLaunch become zips, a full environment's
    /// `setup.cfg` is a requirement, a basic script is noted, a hidden entry and a plain folder
    /// are skipped, and exclusions apply only at the top level.
    #[test]
    fn export_carries_scripts_and_folders() {
        let root = temp("scripts");
        let appdata = root.join("Termurion");
        let scripts = appdata.join("Scripts");
        std::fs::create_dir_all(scripts.join("AutoLaunch")).unwrap();
        std::fs::create_dir_all(scripts.join("plain")).unwrap();
        std::fs::create_dir_all(appdata.join(r"state\Scripts")).unwrap();
        std::fs::create_dir_all(appdata.join(".hidden")).unwrap();
        std::fs::write(appdata.join("settings.toml"), "").unwrap();
        std::fs::write(appdata.join(r"state\Scripts\keep.txt"), "k").unwrap();
        std::fs::write(scripts.join("tool.py"), "print(1)\n").unwrap();
        std::fs::write(scripts.join(".secret.py"), "print(2)\n").unwrap();
        std::fs::write(scripts.join(r"AutoLaunch\auto.py"), "print(3)\n").unwrap();
        let full = scripts.join("Full");
        std::fs::create_dir_all(full.join("Full")).unwrap();
        std::fs::write(full.join("setup.cfg"), rt::setup_cfg_text("Full", &[], true, "3.11", 3)).unwrap();
        std::fs::write(full.join(rt::MARKER), "{}").unwrap();
        std::fs::write(full.join(r"Full\Full.py"), "print(4)\n").unwrap();
        let snap = user_defaults(&appdata);
        let out = root.join("out.termurionexport");
        export_into(&out, &Sources { app_support: &appdata, scripts: Some(&scripts), user_defaults: &snap, local: &|_| true }).unwrap();

        let x = root.join("x");
        std::fs::create_dir_all(&x).unwrap();
        tar(&["-xzf", &out.to_string_lossy(), "-C", &x.to_string_lossy()]);
        let zips: Vec<String> = entries(&x.join("scripts")).into_iter().map(|(n, _)| n).collect();
        assert_eq!(zips.len(), 3, "{zips:?}");
        assert_eq!(zips.iter().filter(|n| n.starts_with("autolaunch ") && n.ends_with(".zip")).count(), 1, "{zips:?}");
        assert_eq!(zips.iter().filter(|n| n.starts_with("script ") && n.ends_with(".zip")).count(), 2, "{zips:?}");
        assert_eq!(
            std::fs::read_to_string(x.join(r"python-runtimes\PythonRuntimes.json")).unwrap(),
            r#"{"requirements":[{"minimumRuntime":3,"pythonVersion":"3.11"}],"haveAnyBasicScripts":true}"#
        );
        assert!(x.join(r"app-support\state\Scripts\keep.txt").is_file(), "a nested Scripts folder is kept");
        assert!(!x.join(r"app-support\.hidden").exists());
        assert!(!x.join(r"app-support\Scripts").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Task 9 review: an entry of the Scripts folder or an AutoLaunch folder that is not local
    /// (a link or junction to a network path) is never read: neither zipped nor counted.
    #[test]
    fn export_skips_scripts_that_are_not_local() {
        let root = temp("links");
        let appdata = root.join("Termurion");
        let scripts = appdata.join("Scripts");
        std::fs::create_dir_all(scripts.join("AutoLaunch")).unwrap();
        std::fs::write(scripts.join("kept.py"), "print(1)\n").unwrap();
        std::fs::write(scripts.join("remote.py"), "print(2)\n").unwrap();
        std::fs::write(scripts.join(r"AutoLaunch\auto.py"), "print(3)\n").unwrap();
        let full = scripts.join("Far");
        std::fs::create_dir_all(full.join("Far")).unwrap();
        std::fs::write(full.join("setup.cfg"), rt::setup_cfg_text("Far", &[], true, "3.11", 3)).unwrap();
        std::fs::write(full.join(rt::MARKER), "{}").unwrap();
        std::fs::write(full.join(r"Far\Far.py"), "print(4)\n").unwrap();
        let snap = user_defaults(&appdata);
        let (remote, far, auto) = (scripts.join("remote.py"), full.clone(), scripts.join("AutoLaunch"));
        let asked = std::cell::RefCell::new(Vec::new());
        let local = |p: &Path| {
            asked.borrow_mut().push(p.to_path_buf());
            p != remote && p != far && p != auto
        };
        let out = root.join("out.termurionexport");
        export_into(&out, &Sources { app_support: &appdata, scripts: Some(&scripts), user_defaults: &snap, local: &local }).unwrap();
        assert!(asked.borrow().contains(&auto), "AutoLaunch is checked before it is listed");

        let x = root.join("x");
        std::fs::create_dir_all(&x).unwrap();
        tar(&["-xzf", &out.to_string_lossy(), "-C", &x.to_string_lossy()]);
        let zips: Vec<String> = entries(&x.join("scripts")).into_iter().map(|(n, _)| n).collect();
        assert_eq!(zips.len(), 1, "only kept.py: {zips:?}");
        assert!(zips[0].starts_with("script "), "{zips:?}");
        assert_eq!(std::fs::read_to_string(x.join(r"python-runtimes\PythonRuntimes.json")).unwrap(), r#"{"requirements":[],"haveAnyBasicScripts":true}"#);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn finish_import_applies_once() {
        let root = temp("finish");
        let termurion = root.join("Termurion");
        std::fs::create_dir_all(&termurion).unwrap();
        // The final review's Important 1: a custom settings folder is turned off by the import
        // (`IE:836-852`), so the next load does not replace what was imported.
        let local_settings = "[settings]\nFocusFollowsMouse = false\nNoSyncX = true\nLoadPrefsFromCustomFolder = true\nPrefsCustomFolder = 'D:\\s'\n";
        std::fs::write(termurion.join("settings.toml"), local_settings).unwrap();
        std::fs::write(termurion.join("snippets.json"), "old").unwrap();
        let staging = se::staging_dir(&root, "U");
        std::fs::create_dir_all(staging.join("user-defaults")).unwrap();
        std::fs::create_dir_all(staging.join("app-support")).unwrap();
        std::fs::create_dir_all(staging.join("scripts")).unwrap();
        let profile: std::collections::BTreeMap<String, toml::Value> = [("Guid".to_string(), toml::Value::String("gi".into())), ("Name".to_string(), toml::Value::String("Imported".into()))].into();
        let snap = Snapshot { settings: [("FocusFollowsMouse".to_string(), toml::Value::Boolean(true))].into(), default: "gi".into(), profiles: vec![profile] };
        std::fs::write(staging.join(r"user-defaults\UserDefaults.toml"), snap.to_toml()).unwrap();
        std::fs::write(staging.join(r"app-support\snippets.json"), "[]").unwrap();
        std::fs::write(staging.join(r"scripts\script U.zip"), "zip").unwrap();
        std::fs::write(termurion.join(se::MARKER), staging.to_string_lossy().as_bytes()).unwrap();

        let zips = finish_import(&termurion);
        assert_eq!(zips, [termurion.join(IMPORT_SCRIPTS_DIR).join("script U.zip")]);
        assert_eq!(std::fs::read_to_string(&zips[0]).unwrap(), "zip");
        let g = crate::globals::Globals::load(&termurion.join("settings.toml")).unwrap();
        assert_eq!(g.values.get("FocusFollowsMouse"), Some(&toml::Value::Boolean(true)));
        assert_eq!(g.values.get("NoSyncX"), Some(&toml::Value::Boolean(true)), "a key that does not sync stays");
        assert!(!g.bool(crate::remote_prefs::LOAD_KEY), "loading from a custom folder is off");
        assert!(!g.values.contains_key(crate::remote_prefs::FOLDER_KEY), "the custom folder is unset");
        let store = crate::store::Store::load(&termurion.join("profiles.toml")).unwrap();
        let names: Vec<String> = store.profiles.iter().map(|p| p.name()).collect();
        assert_eq!(names, ["Imported"]);
        assert_eq!(store.default_guid, "gi");
        assert_eq!(std::fs::read_to_string(termurion.join("snippets.json")).unwrap(), "[]");
        assert!(!termurion.join(se::MARKER).exists());
        assert!(!staging.exists());

        // A second start does nothing.
        std::fs::write(termurion.join("snippets.json"), "later").unwrap();
        assert!(finish_import(&termurion).is_empty());
        assert_eq!(std::fs::read_to_string(termurion.join("snippets.json")).unwrap(), "later");

        // A marker naming a folder that is gone is removed and nothing changes.
        let before = std::fs::read_to_string(termurion.join("settings.toml")).unwrap();
        std::fs::write(termurion.join(se::MARKER), se::staging_dir(&root, "gone").to_string_lossy().as_bytes()).unwrap();
        assert!(finish_import(&termurion).is_empty());
        assert!(!termurion.join(se::MARKER).exists());
        assert_eq!(std::fs::read_to_string(termurion.join("settings.toml")).unwrap(), before);
        assert_eq!(std::fs::read_to_string(termurion.join("snippets.json")).unwrap(), "later");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Controller ruling I5: Import All's User Defaults never bring the AI consent switches
    /// (`ai_settings::guards_consent`); the local values stay.
    /// Carried pass I2: an imported state file is marked, so its restore starts no hidden member; an import
    /// without one writes no mark, and the mark is never exported.
    #[test]
    fn finish_import_marks_an_imported_state() {
        let root = temp("finish-state");
        let termurion = root.join("Termurion");
        std::fs::create_dir_all(&termurion).unwrap();
        let staging = se::staging_dir(&root, "S");
        std::fs::create_dir_all(staging.join("app-support")).unwrap();
        std::fs::write(staging.join(r"app-support\state"), "x").unwrap();
        std::fs::write(termurion.join(se::MARKER), staging.to_string_lossy().as_bytes()).unwrap();
        finish_import(&termurion);
        assert!(termurion.join(se::STATE_IMPORTED).exists(), "the state came from an import");
        assert!(se::excluded(se::STATE_IMPORTED), "and the mark is never exported");
        let _ = std::fs::remove_file(termurion.join(se::STATE_IMPORTED));
        let staging = se::staging_dir(&root, "T");
        std::fs::create_dir_all(staging.join("app-support")).unwrap();
        std::fs::write(staging.join(r"app-support\snippets.json"), "[]").unwrap();
        std::fs::write(termurion.join(se::MARKER), staging.to_string_lossy().as_bytes()).unwrap();
        finish_import(&termurion);
        assert!(!termurion.join(se::STATE_IMPORTED).exists(), "no state, no mark");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn finish_import_keeps_the_ai_consent_switches() {
        use crate::ai_settings as ai;
        let root = temp("ai");
        let termurion = root.join("Termurion");
        std::fs::create_dir_all(&termurion).unwrap();
        let kept = ai::PERMISSIONS[1].0;
        std::fs::write(termurion.join("settings.toml"), format!("[settings]\n{kept} = 2\n")).unwrap();
        let staging = se::staging_dir(&root, "A");
        std::fs::create_dir_all(staging.join("user-defaults")).unwrap();
        let mut settings: std::collections::BTreeMap<String, toml::Value> = [("FocusFollowsMouse".to_string(), toml::Value::Boolean(true))].into();
        for key in [ai::ADMIN, ai::ENABLE_AI, ai::AI_COMPLETIONS] {
            settings.insert(key.into(), toml::Value::Boolean(key != ai::ADMIN));
        }
        for (key, _) in ai::PERMISSIONS {
            settings.insert(key.into(), toml::Value::Integer(1));
        }
        let snap = Snapshot { settings, ..Default::default() };
        std::fs::write(staging.join(r"user-defaults\UserDefaults.toml"), snap.to_toml()).unwrap();
        std::fs::write(termurion.join(se::MARKER), staging.to_string_lossy().as_bytes()).unwrap();
        finish_import(&termurion);
        let g = crate::globals::Globals::load(&termurion.join("settings.toml")).unwrap();
        assert_eq!(g.values.get("FocusFollowsMouse"), Some(&toml::Value::Boolean(true)), "the import applied");
        for key in [ai::ADMIN, ai::ENABLE_AI, ai::AI_COMPLETIONS] {
            assert!(!g.values.contains_key(key), "{key} came from the import");
        }
        for (key, _) in ai::PERMISSIONS {
            let want = (key == kept).then_some(toml::Value::Integer(2));
            assert_eq!(g.values.get(key), want.as_ref(), "{key}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Beyond the plan: the archive's app-support never replaces what Tandem keeps out of an
    /// export (the settings files, the Python API consent, the marker), nor a hidden entry.
    #[test]
    fn finish_import_keeps_what_export_leaves_out() {
        let root = temp("keep");
        let termurion = root.join("Termurion");
        std::fs::create_dir_all(&termurion).unwrap();
        std::fs::write(termurion.join("profiles.toml"), "").unwrap();
        let staging = se::staging_dir(&root, "K");
        let support = staging.join("app-support");
        std::fs::create_dir_all(support.join(r"state")).unwrap();
        for name in ["python-api-consent", "settings.toml", "profiles.toml", ".hidden", crate::workgroup_restore::KEY_FILE] {
            std::fs::write(support.join(name), "from the archive").unwrap();
        }
        std::fs::write(support.join(r"state\s.json"), "{}").unwrap();
        std::fs::write(termurion.join(se::MARKER), staging.to_string_lossy().as_bytes()).unwrap();
        assert!(finish_import(&termurion).is_empty());
        assert!(!termurion.join("python-api-consent").exists());
        assert!(!termurion.join("settings.toml").exists());
        assert!(!termurion.join(".hidden").exists());
        assert!(!termurion.join(crate::workgroup_restore::KEY_FILE).exists(), "an archive never brings the approval key");
        assert_eq!(std::fs::read_to_string(termurion.join("profiles.toml")).unwrap(), "");
        assert!(termurion.join(r"state\s.json").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Task 9 findings r2 (Critical), on the real file system: an archive's Application Support
    /// entry that Windows reads as a protected entry (a trailing dot or space beside the excluded
    /// name itself, an 8.3 short name, a case Windows folds) never replaces or enters it. Each
    /// alias is its own archive, so one cannot hide another's short name.
    #[test]
    fn import_never_reaches_a_protected_entry_through_an_alias() {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let verbatim = |p: &Path| PathBuf::from(format!(r"\\?\{}", p.display()));
        let short_name = |p: &Path| -> Option<String> {
            let wide: Vec<u16> = p.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut buf = vec![0u16; 1024];
            // SAFETY: a NUL-terminated path and a buffer of the length given.
            let n = unsafe { windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
            let short = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n.min(buf.len())]));
            let name = short.file_name()?.to_string_lossy().into_owned();
            (n > 0 && name != p.file_name()?.to_string_lossy()).then_some(name)
        };
        let root = temp("alias");
        let termurion = root.join("Termurion");
        std::fs::create_dir_all(&termurion).unwrap();
        let files = [crate::workgroup_restore::KEY_FILE, "python-api-consent", "settings.toml", "profiles.toml", "host.log"];
        let dirs = ["Scripts", "uv", IMPORT_SCRIPTS_DIR];
        for f in files {
            std::fs::write(termurion.join(f), format!("ours: {f}")).unwrap();
        }
        for d in dirs {
            std::fs::create_dir_all(termurion.join(d)).unwrap();
        }
        // (the staged name, whether it is a folder, whether the archive also holds the excluded name itself)
        let mut cases: Vec<(String, bool, bool)> = Vec::new();
        for f in files {
            for tail in [".", " ", ". ", " ."] {
                cases.push((format!("{f}{tail}"), false, true));
            }
            match short_name(&termurion.join(f)) {
                Some(s) => {
                    // The final-path check sees through the alias by itself too.
                    assert_eq!(final_name(&termurion.join(&s)).as_deref(), Some(f), "{s}");
                    assert!(!importable("anything", &termurion.join(&s), false), "{s}");
                    cases.push((s, false, false));
                }
                None => eprintln!("no 8.3 name for {f} on this volume"),
            }
        }
        for d in dirs {
            cases.push((format!("{d}."), true, false));
            cases.push((format!("{d} "), true, false));
            if let Some(s) = short_name(&termurion.join(d)) {
                cases.push((s, true, false));
            }
        }
        cases.push(("\u{17f}cripts".into(), true, false));
        let before = entries(&termurion).into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert!(cases.iter().any(|(n, _, _)| n.contains('~')), "8.3 names are on for this volume: {cases:?}");
        for (i, (name, dir, literal)) in cases.iter().enumerate() {
            let staging = se::staging_dir(&root, &format!("A{i}"));
            let support = staging.join("app-support");
            std::fs::create_dir_all(&support).unwrap();
            if *dir {
                std::fs::create_dir_all(verbatim(&support.join(name))).unwrap();
                std::fs::write(verbatim(&support.join(name).join("evil.py")), "from the archive").unwrap();
            } else {
                std::fs::write(verbatim(&support.join(name)), "from the archive").unwrap();
            }
            if *literal {
                let plain = name.trim_end_matches(['.', ' ']);
                std::fs::write(support.join(plain), "from the archive").unwrap();
            }
            std::fs::write(support.join("notes.txt"), "kept").unwrap();
            std::fs::write(termurion.join(se::MARKER), staging.to_string_lossy().as_bytes()).unwrap();
            assert!(finish_import(&termurion).is_empty());
            for f in files {
                assert_eq!(std::fs::read_to_string(termurion.join(f)).unwrap(), format!("ours: {f}"), "{name:?} reached {f}");
            }
            for d in dirs {
                assert!(entries(&termurion.join(d)).is_empty(), "{name:?} reached {d}");
            }
            if se::aliased(name) {
                assert!(!termurion.join("notes.txt").exists(), "{name:?}: an archive holding an alias is refused whole");
            } else {
                assert_eq!(std::fs::read_to_string(termurion.join("notes.txt")).unwrap(), "kept", "{name:?}: the rest is still imported");
                std::fs::remove_file(termurion.join("notes.txt")).unwrap();
            }
            assert_eq!(entries(&termurion).into_iter().map(|(n, _)| n).collect::<Vec<_>>(), before, "{name:?} made a new entry");
        }
        let _ = std::fs::remove_dir_all(verbatim(&root));
    }

    /// Task 9 findings r3, on the real file system: a hard-link entry whose target leaves the
    /// staging folder (absolute with either slash, or "..") never writes there, whether the link
    /// carries data or a file of the same name follows it. Every entry is checked before anything
    /// is written (since final findings r2 by Tandem itself, no tar.exe) and the archive is refused
    /// whole.
    #[test]
    fn import_checks_every_entry_before_tar_runs() {
        use crate::zip_check::tests::{pax, tar_entry, tgz};
        let root = temp("hardlink");
        let canary = root.join("canary.txt");
        let (to, temp_dir) = (root.join("To"), root.join("Temp"));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let back = canary.to_string_lossy().into_owned();
        let targets = [back.replace('\\', "/"), back, "../../canary.txt".to_string()];
        let mut seen = Vec::new();
        for target in &targets {
            for follow in [false, true] {
                std::fs::write(&canary, "ours").unwrap();
                let name = "app-support/evil.txt";
                let mut blocks = tar_entry("user-defaults/", b'5', b"", "");
                let data: &[u8] = if follow { b"" } else { b"PWNED" };
                let link = if target.len() < 100 { tar_entry(name, b'1', data, target) } else { [pax(&[("linkpath", target)]), tar_entry(name, b'1', data, "")].concat() };
                blocks.extend(link);
                if follow {
                    blocks.extend(tar_entry(name, b'0', b"PWNED", ""));
                }
                let file = root.join("evil.termurionexport");
                std::fs::write(&file, tgz(&blocks)).unwrap();
                let got = stage_import(&file, &temp_dir, &to);
                let after = std::fs::read_to_string(&canary).unwrap_or_default();
                seen.push((target.clone(), follow, after, got, entries(&temp_dir).len(), to.join(se::MARKER).exists()));
            }
        }
        for (target, follow, after, got, left, marker) in &seen {
            let refused = got.as_ref().err().is_some_and(|e| e.starts_with("The archive is not safe to extract") && e.contains("is a link"));
            assert!(after == "ours" && refused && *left == 0 && !marker, "{target} (a file follows: {follow}): canary {after:?}, {got:?}, left {left}; all: {seen:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Task 9 re-review r2, new minor 2: an existing entry of `%APPDATA%\Termurion` that is itself a
    /// link or junction is never written through: the import skips it.
    #[test]
    fn import_never_writes_through_a_link_at_the_destination() {
        let root = temp("destlink");
        let (to, outside) = (root.join("Termurion"), root.join("Outside"));
        std::fs::create_dir_all(&to).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let made = std::process::Command::new("cmd.exe").args(["/d", "/c", "mklink", "/J"]).arg(to.join("notes")).arg(&outside).output().unwrap();
        assert!(made.status.success(), "{made:?}");
        let staging = se::staging_dir(&root, "D");
        std::fs::create_dir_all(staging.join(r"app-support\notes")).unwrap();
        std::fs::write(staging.join(r"app-support\notes\x.txt"), "from the archive").unwrap();
        std::fs::write(staging.join(r"app-support\kept.txt"), "kept").unwrap();
        std::fs::write(to.join(se::MARKER), staging.to_string_lossy().as_bytes()).unwrap();
        assert!(finish_import(&to).is_empty());
        assert!(!outside.join("x.txt").exists(), "written through the junction");
        assert_eq!(std::fs::read_to_string(to.join("kept.txt")).unwrap(), "kept", "the rest is imported");
        let _ = std::fs::remove_dir(to.join("notes"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn import_refuses_links() {
        // The Task 10 review's Important 1 (invariant 5): an export holding a link, a junction
        // or a hard-linked file is refused at staging, leaving nothing, and a staging folder
        // that gained one before the restart is removed unapplied.
        let root = temp("links");
        let src = root.join("Src");
        std::fs::create_dir_all(src.join("app-support")).unwrap();
        std::fs::write(src.join(r"app-support\a.txt"), "a").unwrap();
        std::fs::hard_link(src.join(r"app-support\a.txt"), src.join(r"app-support\b.txt")).unwrap();
        let out = root.join("linked.termurionexport");
        tar(&["-czf", &out.to_string_lossy(), "-C", &src.to_string_lossy(), "app-support"]);
        let (to, temp_dir) = (root.join("To"), root.join("Temp"));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let refused = stage_import(&out, &temp_dir, &to).unwrap_err();
        assert!(refused.contains("is a link"), "{refused}");
        assert!(!to.join(se::MARKER).exists());
        assert!(entries(&temp_dir).is_empty(), "a refused archive leaves nothing");

        // A junction (no rights needed) planted in a staged folder before the next start.
        let staging = se::staging_dir(&temp_dir, "U");
        std::fs::create_dir_all(staging.join("user-defaults")).unwrap();
        std::fs::write(staging.join(r"user-defaults\UserDefaults.toml"), "[settings]\nFocusFollowsMouse = true\n").unwrap();
        assert_eq!(staged_links(&staging), Ok(()));
        let outside = root.join("Outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("x.txt"), "x").unwrap();
        let junction = staging.join("app-support");
        let made = std::process::Command::new("cmd.exe").args(["/d", "/c", "mklink", "/J"]).arg(&junction).arg(&outside).output().unwrap();
        assert!(made.status.success(), "{made:?}");
        assert!(staged_links(&staging).is_err());
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(to.join(se::MARKER), staging.to_string_lossy().as_bytes()).unwrap();
        assert!(finish_import(&to).is_empty());
        assert!(!to.join(se::MARKER).exists());
        assert!(!to.join("settings.toml").exists(), "nothing was applied");
        assert!(!staging.exists(), "the staging folder is gone");
        assert!(outside.join("x.txt").exists(), "the junction's target is untouched");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Beyond the plan: an export written by the real tar.exe imported into another Tandem folder
    /// (the first phase names the staging folder in the marker; the next start applies it),
    /// and a file that is not an archive fails with nothing left behind.
    #[test]
    fn import_round_trips_an_export() {
        let root = temp("round");
        let from = root.join("From");
        let scripts = from.join("Scripts");
        std::fs::create_dir_all(scripts.join("AutoLaunch")).unwrap();
        std::fs::write(from.join("settings.toml"), "[settings]\nFocusFollowsMouse = true\n").unwrap();
        std::fs::write(from.join("snippets.json"), "[1]").unwrap();
        // A long name with a letter past ASCII: bsdtar writes a pax header for it, and the entry
        // check and the extraction take it.
        let long = format!("{}\u{e9}.json", "n".repeat(120));
        std::fs::write(from.join(&long), "[2]").unwrap();
        std::fs::write(scripts.join(r"AutoLaunch\auto.py"), "print(1)\n").unwrap();
        let snap = user_defaults(&from);
        let out = root.join("x.termurionexport");
        export_into(&out, &Sources { app_support: &from, scripts: Some(&scripts), user_defaults: &snap, local: &|_| true }).unwrap();

        let (to, temp_dir) = (root.join("To"), root.join("Temp"));
        std::fs::create_dir_all(&temp_dir).unwrap();
        stage_import(&out, &temp_dir, &to).unwrap();
        let marker = std::fs::read_to_string(to.join(se::MARKER)).unwrap();
        assert!(matches!(se::pending(Some(&marker), &|p| p.is_dir()), Pending::Apply(p) if p.parent() == Some(temp_dir.as_path())));
        let zips = finish_import(&to);
        assert_eq!(zips.len(), 1, "{zips:?}");
        assert!(zips[0].file_name().unwrap().to_string_lossy().starts_with("autolaunch "), "{zips:?}");
        let g = Globals::load(&to.join("settings.toml")).unwrap();
        assert_eq!(g.values.get("FocusFollowsMouse"), Some(&toml::Value::Boolean(true)));
        assert_eq!(std::fs::read_to_string(to.join("snippets.json")).unwrap(), "[1]");
        assert_eq!(std::fs::read_to_string(to.join(&long)).unwrap(), "[2]");
        assert!(entries(&temp_dir).is_empty(), "the staging folder is gone");

        let bad = root.join("bad.termurionexport");
        std::fs::write(&bad, "not an archive").unwrap();
        let other = root.join("Other");
        assert!(stage_import(&bad, &temp_dir, &other).is_err());
        assert!(!other.join(se::MARKER).exists());
        assert!(entries(&temp_dir).is_empty(), "a failed extraction leaves nothing");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every name under `dir`, relative, folders with a trailing "/" (nothing followed).
    fn tree_names(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![(String::new(), dir.to_path_buf())];
        while let Some((rel, d)) = stack.pop() {
            for (name, path) in entries(&d) {
                let shown = format!("{rel}{name}");
                if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                    out.push(format!("{shown}/"));
                    stack.push((format!("{shown}/"), path));
                } else {
                    out.push(shown);
                }
            }
        }
        out.sort();
        out
    }

    /// A raw header block's checksum written again with `spell` (the octal value as text).
    fn reseal(block: &mut [u8], spell: &dyn Fn(u32) -> String) {
        block[148..156].copy_from_slice(b"        ");
        let sum: u32 = block[..512].iter().map(|&b| u32::from(b)).sum();
        let text = spell(sum);
        block[148..156].fill(0);
        block[148..148 + text.len()].copy_from_slice(text.as_bytes());
    }

    /// Final findings r2 1(a), on the real file system: a number field the tar crate reads one way and
    /// libarchive another (a leading "+" in the size or the checksum) never lets an entry the check did
    /// not see reach the staging folder. The second entry's data block is itself a header naming
    /// `hidden.url`, a type the check refuses; the archive is refused, or holds only the checked entries.
    #[test]
    fn import_never_extracts_an_entry_the_check_did_not_see() {
        use crate::zip_check::tests::{tar_entry, tgz};
        let root = temp("numeric");
        let (to, temp_dir) = (root.join("To"), root.join("Temp"));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let hidden = tar_entry("app-support/hidden.url", b'0', b"", "");
        let plus_size = |b: &mut Vec<u8>| {
            b[124..136].copy_from_slice(b"+0000001000\0");
            reseal(b, &|s| format!("{s:06o}\0 "));
        };
        let plus_sum = |b: &mut Vec<u8>| reseal(b, &|s| format!("+0{s:05o}\0"));
        let mut seen = Vec::new();
        for (what, edit) in [("size", &plus_size as &dyn Fn(&mut Vec<u8>)), ("checksum", &plus_sum)] {
            let mut second = tar_entry("app-support/a.txt", b'0', &hidden[..512], "");
            edit(&mut second);
            let blocks = [tar_entry("user-defaults/", b'5', b"", ""), second].concat();
            let file = root.join(format!("{what}.termurionexport"));
            std::fs::write(&file, tgz(&blocks)).unwrap();
            let got = stage_import(&file, &temp_dir, &to);
            let names = tree_names(&temp_dir);
            seen.push((what, got.clone(), names.clone()));
            let _ = std::fs::remove_file(to.join(se::MARKER));
            for (_, p) in entries(&temp_dir) {
                let _ = std::fs::remove_dir_all(p);
            }
        }
        for (what, got, names) in &seen {
            assert!(!names.iter().any(|n| n.ends_with("hidden.url")), "{what}: an unchecked entry was extracted: {got:?} {names:?}; all: {seen:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Final findings r2 1(c): a gzip tar whose first entry's name is bzip2's magic is read as the gzip tar
    /// it is (no second filter is guessed on the decompressed bytes), so the entry the check saw is the one
    /// extracted.
    #[test]
    fn import_guesses_no_second_filter() {
        use crate::zip_check::tests::{tar_entry, tgz};
        let root = temp("bzip2");
        let (to, temp_dir) = (root.join("To"), root.join("Temp"));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let blocks = [tar_entry("BZh91AY&SY", b'0', b"plain", ""), tar_entry("user-defaults/", b'5', b"", "")].concat();
        let file = root.join("magic.termurionexport");
        std::fs::write(&file, tgz(&blocks)).unwrap();
        let got = stage_import(&file, &temp_dir, &to);
        assert_eq!(got, Ok(()), "the checked archive is the extracted one");
        let staging = PathBuf::from(std::fs::read_to_string(to.join(se::MARKER)).unwrap());
        assert_eq!(tree_names(&staging), ["BZh91AY&SY", "user-defaults/"]);
        assert_eq!(std::fs::read(staging.join("BZh91AY&SY")).unwrap(), b"plain");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Final findings r2 1(b): the archive is read once. While another writer swaps the file between a
    /// checked archive and one the check refuses (a `.url` entry), no staging folder ever holds the
    /// refused archive's entry: what is extracted is the bytes that were checked.
    #[test]
    fn import_extracts_the_bytes_it_checked() {
        use crate::zip_check::tests::{tar_entry, tgz};
        let root = temp("swap");
        let (to, temp_dir) = (root.join("To"), root.join("Temp"));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let good = tgz(&[tar_entry("user-defaults/", b'5', b"", ""), tar_entry("app-support/a.txt", b'0', b"a", "")].concat());
        let evil = tgz(&[tar_entry("user-defaults/", b'5', b"", ""), tar_entry("app-support/swapped.url", b'0', b"a", "")].concat());
        let file = root.join("swap.termurionexport");
        std::fs::write(&file, &good).unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = {
            let (stop, file, good, evil) = (stop.clone(), file.clone(), good.clone(), evil.clone());
            std::thread::spawn(move || {
                let mut flip = false;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = std::fs::write(&file, if flip { &evil } else { &good });
                    flip = !flip;
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            })
        };
        let started = std::time::Instant::now();
        let mut leaked = None;
        let mut runs = 0;
        while started.elapsed() < std::time::Duration::from_secs(8) && leaked.is_none() {
            runs += 1;
            let _ = stage_import(&file, &temp_dir, &to);
            let names = tree_names(&temp_dir);
            if names.iter().any(|n| n.ends_with("swapped.url")) {
                leaked = Some(names);
            }
            let _ = std::fs::remove_file(to.join(se::MARKER));
            for (_, p) in entries(&temp_dir) {
                let _ = std::fs::remove_dir_all(p);
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        writer.join().unwrap();
        assert_eq!(leaked, None, "an entry the check refused was extracted ({runs} runs)");
        let _ = std::fs::remove_dir_all(&root);
    }
}
