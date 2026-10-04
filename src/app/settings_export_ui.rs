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
pub(super) const WAIT_PID_VAR: &str = "TANDEM_WAIT_PID";

/// Where the export reads from.
struct Sources<'a> {
    /// `%APPDATA%\Tandem`.
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

/// `deepCopyContentsOfDirectory` (`IE:1163-1208`): every entry of `src` but the hidden ones
/// into `dst`, folders recursively; at the top level the excluded names stay out too. Links and
/// junctions are not followed (Decision: nothing outside the folder is copied).
fn copy_contents(src: &Path, dst: &Path, top: bool) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for (name, path) in entries(src) {
        if name.starts_with('.') || (top && se::excluded(&name)) || is_reparse(&path) {
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

/// Where the staged script zips wait in `%APPDATA%\Tandem` for the app to import them.
pub(super) const IMPORT_SCRIPTS_DIR: &str = ".import-scripts";

/// `%APPDATA%\Tandem`, the folder holding the settings.
fn appdata_tandem() -> PathBuf {
    let settings = crate::globals::default_path();
    settings.parent().unwrap_or(&settings).to_path_buf()
}

/// An extracted export holding a link, a junction or a file with another hard link is refused
/// (11d's rule for script zips, `sa::unsafe_entry`), so nothing the import reads or copies can
/// reach outside the staging folder or onto a network path.
fn staged_links(staging: &Path) -> Result<(), String> {
    match crate::script_archive::unsafe_entry(staging, &super::scripts_app::stage_entries) {
        Some(entry) => Err(crate::script_archive::unsafe_entry_text(entry.strip_prefix(staging).unwrap_or(&entry))),
        None => Ok(()),
    }
}

/// Import's first phase (`Importer.importEntities`, `IE:81-110`): `file` extracted into a fresh
/// `<temp>\.tandem-import-<uuid>`, refused when it holds a link (`staged_links`), then the
/// marker in `appdata` naming it. On a failure nothing is left behind.
fn stage_import(file: &Path, temp: &Path, appdata: &Path) -> Result<(), String> {
    let staging = se::staging_dir(temp, &uuid::Uuid::new_v4().to_string());
    let staged = std::fs::create_dir_all(&staging)
        .map_err(|e| format!("{}: {e}", staging.display()))
        .and_then(|_| super::scripts_app::run_tar(se::extract_args(file, &staging)))
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
pub(super) fn finish_import(appdata_tandem: &Path) -> Vec<PathBuf> {
    let marker = appdata_tandem.join(se::MARKER);
    let text = std::fs::read_to_string(&marker).ok();
    let staging = match se::pending(text.as_deref(), &|p| p.is_dir()) {
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
    apply_user_defaults(&staging.join(Entity::UserDefaults.key()).join(se::USER_DEFAULTS_FILE), appdata_tandem);
    let support = staging.join(Entity::AppSupport.key());
    if support.is_dir() && !is_reparse(&support) {
        let _ = copy_contents(&support, appdata_tandem, true);
    }
    let zips = stage_scripts(&staging.join(Entity::Scripts.key()), &appdata_tandem.join(IMPORT_SCRIPTS_DIR));
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
        match stage_import(&file, &std::env::temp_dir(), &appdata_tandem()) {
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
    /// is left alone), then the list of every entry of `%APPDATA%\Tandem`
    /// and of that folder. A dry run logs each path and says so; otherwise the host's shells
    /// end, every path goes (links as links) and Tandem exits at once, saving nothing (iTerm2's
    /// `_exit(0)`). Credential Manager entries are not touched.
    pub(super) fn erase_confirmed(&mut self, dry_run: bool) {
        let scripts = se::scripts_to_erase(self.scripts_folder()).filter(|s| crate::scripts::ancestors_stay_local(s, &crate::scripts::read_link));
        let tandem = appdata_tandem();
        let listed = all_entries(&tandem).map_err(|e| format!("{}: {e}", tandem.display())).and_then(|own| {
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
        crate::notify::remove(0);
        std::process::exit(0);
    }

    /// Writes the archive at `out`: the settings and profiles as they are now, `%APPDATA%\Tandem`
    /// and the Scripts folder. A Scripts folder on a network path is not read.
    pub(super) fn export_to(&mut self, out: &Path) -> Result<(), ExportError> {
        self.flush_notes();
        let settings = crate::globals::default_path();
        let app_support = settings.parent().unwrap_or(&settings).to_path_buf();
        let scripts = self.scripts_folder();
        let local = crate::scripts::ancestors_stay_local(&scripts, &crate::scripts::read_link) && !crate::remote_prefs::is_unc(&scripts.to_string_lossy());
        let user_defaults = Snapshot::from_local(&self.globals.values, &self.store.default_guid, &self.store.file_profiles());
        let entry_local = |p: &Path| crate::scripts::ancestors_stay_local(p, &crate::scripts::read_link);
        export_into(out, &Sources { app_support: &app_support, scripts: local.then_some(scripts.as_path()), user_defaults: &user_defaults, local: &entry_local })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tandem-export-test-{name}-{}", crate::api_auth::fresh_key()));
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
        let appdata = root.join("Tandem");
        std::fs::create_dir_all(appdata.join("Scripts")).unwrap();
        std::fs::write(appdata.join("settings.toml"), "[settings]\nFocusFollowsMouse = true\nNoSyncX = true\n").unwrap();
        std::fs::write(appdata.join("profiles.toml"), "default = \"g\"\n").unwrap();
        std::fs::write(appdata.join("snippets.json"), "[]").unwrap();
        std::fs::write(appdata.join("host.log"), "log").unwrap();
        let snap = user_defaults(&appdata);
        let out = root.join("Tandem State.tandemexport");
        let scripts = appdata.join("Scripts");
        export_into(&out, &Sources { app_support: &appdata, scripts: Some(&scripts), user_defaults: &snap, local: &|_| true }).unwrap();
        assert!(out.is_file());

        let listing = tar(&["-tzf", &out.to_string_lossy()]);
        let names: Vec<&str> = listing.lines().map(|l| l.trim_end_matches('/')).collect();
        for want in ["user-defaults/UserDefaults.toml", "app-support/snippets.json", "python-runtimes/PythonRuntimes.json", "scripts"] {
            assert!(names.contains(&want), "{want} in {names:?}");
        }
        for never in ["app-support/host.log", "app-support/settings.toml", "app-support/profiles.toml", "app-support/Scripts"] {
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
        let appdata = root.join("Tandem");
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
        let out = root.join("out.tandemexport");
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
        let appdata = root.join("Tandem");
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
        let out = root.join("out.tandemexport");
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
        let tandem = root.join("Tandem");
        std::fs::create_dir_all(&tandem).unwrap();
        // The final review's Important 1: a custom settings folder is turned off by the import
        // (`IE:836-852`), so the next load does not replace what was imported.
        let local_settings = "[settings]\nFocusFollowsMouse = false\nNoSyncX = true\nLoadPrefsFromCustomFolder = true\nPrefsCustomFolder = 'D:\\s'\n";
        std::fs::write(tandem.join("settings.toml"), local_settings).unwrap();
        std::fs::write(tandem.join("snippets.json"), "old").unwrap();
        let staging = se::staging_dir(&root, "U");
        std::fs::create_dir_all(staging.join("user-defaults")).unwrap();
        std::fs::create_dir_all(staging.join("app-support")).unwrap();
        std::fs::create_dir_all(staging.join("scripts")).unwrap();
        let profile: std::collections::BTreeMap<String, toml::Value> = [("Guid".to_string(), toml::Value::String("gi".into())), ("Name".to_string(), toml::Value::String("Imported".into()))].into();
        let snap = Snapshot { settings: [("FocusFollowsMouse".to_string(), toml::Value::Boolean(true))].into(), default: "gi".into(), profiles: vec![profile] };
        std::fs::write(staging.join(r"user-defaults\UserDefaults.toml"), snap.to_toml()).unwrap();
        std::fs::write(staging.join(r"app-support\snippets.json"), "[]").unwrap();
        std::fs::write(staging.join(r"scripts\script U.zip"), "zip").unwrap();
        std::fs::write(tandem.join(se::MARKER), staging.to_string_lossy().as_bytes()).unwrap();

        let zips = finish_import(&tandem);
        assert_eq!(zips, [tandem.join(IMPORT_SCRIPTS_DIR).join("script U.zip")]);
        assert_eq!(std::fs::read_to_string(&zips[0]).unwrap(), "zip");
        let g = crate::globals::Globals::load(&tandem.join("settings.toml")).unwrap();
        assert_eq!(g.values.get("FocusFollowsMouse"), Some(&toml::Value::Boolean(true)));
        assert_eq!(g.values.get("NoSyncX"), Some(&toml::Value::Boolean(true)), "a key that does not sync stays");
        assert!(!g.bool(crate::remote_prefs::LOAD_KEY), "loading from a custom folder is off");
        assert!(!g.values.contains_key(crate::remote_prefs::FOLDER_KEY), "the custom folder is unset");
        let store = crate::store::Store::load(&tandem.join("profiles.toml")).unwrap();
        let names: Vec<String> = store.profiles.iter().map(|p| p.name()).collect();
        assert_eq!(names, ["Imported"]);
        assert_eq!(store.default_guid, "gi");
        assert_eq!(std::fs::read_to_string(tandem.join("snippets.json")).unwrap(), "[]");
        assert!(!tandem.join(se::MARKER).exists());
        assert!(!staging.exists());

        // A second start does nothing.
        std::fs::write(tandem.join("snippets.json"), "later").unwrap();
        assert!(finish_import(&tandem).is_empty());
        assert_eq!(std::fs::read_to_string(tandem.join("snippets.json")).unwrap(), "later");

        // A marker naming a folder that is gone is removed and nothing changes.
        let before = std::fs::read_to_string(tandem.join("settings.toml")).unwrap();
        std::fs::write(tandem.join(se::MARKER), se::staging_dir(&root, "gone").to_string_lossy().as_bytes()).unwrap();
        assert!(finish_import(&tandem).is_empty());
        assert!(!tandem.join(se::MARKER).exists());
        assert_eq!(std::fs::read_to_string(tandem.join("settings.toml")).unwrap(), before);
        assert_eq!(std::fs::read_to_string(tandem.join("snippets.json")).unwrap(), "later");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Controller ruling I5: Import All's User Defaults never bring the AI consent switches
    /// (`ai_settings::guards_consent`); the local values stay.
    #[test]
    fn finish_import_keeps_the_ai_consent_switches() {
        use crate::ai_settings as ai;
        let root = temp("ai");
        let tandem = root.join("Tandem");
        std::fs::create_dir_all(&tandem).unwrap();
        let kept = ai::PERMISSIONS[1].0;
        std::fs::write(tandem.join("settings.toml"), format!("[settings]\n{kept} = 2\n")).unwrap();
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
        std::fs::write(tandem.join(se::MARKER), staging.to_string_lossy().as_bytes()).unwrap();
        finish_import(&tandem);
        let g = crate::globals::Globals::load(&tandem.join("settings.toml")).unwrap();
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
        let tandem = root.join("Tandem");
        std::fs::create_dir_all(&tandem).unwrap();
        std::fs::write(tandem.join("profiles.toml"), "").unwrap();
        let staging = se::staging_dir(&root, "K");
        let support = staging.join("app-support");
        std::fs::create_dir_all(support.join(r"state")).unwrap();
        for name in ["python-api-consent", "settings.toml", "profiles.toml", ".hidden"] {
            std::fs::write(support.join(name), "from the archive").unwrap();
        }
        std::fs::write(support.join(r"state\s.json"), "{}").unwrap();
        std::fs::write(tandem.join(se::MARKER), staging.to_string_lossy().as_bytes()).unwrap();
        assert!(finish_import(&tandem).is_empty());
        assert!(!tandem.join("python-api-consent").exists());
        assert!(!tandem.join("settings.toml").exists());
        assert!(!tandem.join(".hidden").exists());
        assert_eq!(std::fs::read_to_string(tandem.join("profiles.toml")).unwrap(), "");
        assert!(tandem.join(r"state\s.json").is_file());
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
        let out = root.join("linked.tandemexport");
        tar(&["-czf", &out.to_string_lossy(), "-C", &src.to_string_lossy(), "app-support"]);
        let (to, temp_dir) = (root.join("To"), root.join("Temp"));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let refused = stage_import(&out, &temp_dir, &to).unwrap_err();
        assert!(refused.contains("is a link or has another name"), "{refused}");
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

    /// Beyond the plan: an export imported into another Tandem folder through the real tar.exe
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
        std::fs::write(scripts.join(r"AutoLaunch\auto.py"), "print(1)\n").unwrap();
        let snap = user_defaults(&from);
        let out = root.join("x.tandemexport");
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
        assert!(entries(&temp_dir).is_empty(), "the staging folder is gone");

        let bad = root.join("bad.tandemexport");
        std::fs::write(&bad, "not an archive").unwrap();
        let other = root.join("Other");
        assert!(stage_import(&bad, &temp_dir, &other).is_err());
        assert!(!other.join(se::MARKER).exists());
        assert!(entries(&temp_dir).is_empty(), "a failed extraction leaves nothing");
        let _ = std::fs::remove_dir_all(&root);
    }
}
