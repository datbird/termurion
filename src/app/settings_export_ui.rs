//! Export All Settings and Data, the app side (`IE:19-69`, `IE:549-619`, `GP:4084-4086`): the
//! save panel, one folder per entity in a temporary folder, the gzip tar, and the reveal or the
//! failure alert. The model is `crate::settings_export`.

use std::path::{Path, PathBuf};

use super::App;
use super::scripts_alerts::{Ask, Then};
use crate::remote_prefs::Snapshot;
use crate::script_runtime as rt;
use crate::settings_export::{self as se, Entity, ExportError, Requirement};

/// `revealExportedSettingsAndData` (`ASM:558`).
const REVEAL_KEY: &str = "RevealExportedSettingsAndData";

/// Where the export reads from.
struct Sources<'a> {
    /// `%APPDATA%\Tandem`.
    app_support: &'a Path,
    /// The Scripts folder; None when it is on a network path (never read).
    scripts: Option<&'a Path>,
    /// The syncable settings and the profiles.
    user_defaults: &'a Snapshot,
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
fn runtime_requirements(scripts: &Path) -> (Vec<Requirement>, bool) {
    let (mut requirements, mut have_basic) = (Vec::<Requirement>::new(), false);
    for (name, path) in entries(scripts).into_iter().chain(entries(&scripts.join("AutoLaunch"))) {
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
fn export_scripts(scripts: &Path, dest: &Path) -> Result<(), String> {
    let root = entries(scripts).into_iter().filter(|(n, _)| !n.starts_with('.') && !n.eq_ignore_ascii_case("autolaunch")).map(|(_, p)| (p, false));
    let autolaunch = entries(&scripts.join("AutoLaunch")).into_iter().map(|(_, p)| (p, true));
    for (path, autolaunch) in root.chain(autolaunch).collect::<Vec<_>>() {
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
            let (requirements, have_basic) = src.scripts.map(runtime_requirements).unwrap_or_default();
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
        Entity::Scripts => src.scripts.map_or(Ok(()), |s| export_scripts(s, &dest).map_err(ExportError::Script)),
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

impl App {
    /// General > Settings' Export All Settings and Data (`performExportFlow`, `IE:19-69`): the
    /// save panel (Cancel does nothing), the export, then the file revealed in Explorer or
    /// "Problem Exporting" with the reason (`GP:4084-4086`, persistent).
    pub(super) fn export_all(&mut self) {
        let dialog = rfd::FileDialog::new().set_title(se::PANEL_TITLE).set_file_name(se::DEFAULT_NAME).add_filter(se::FILTER_NAME, &[se::EXTENSION]);
        let Some(mut out) = dialog.save_file() else { return };
        // A name typed without the extension gets it, as the save panel's content type does.
        if out.extension().is_none() {
            out.set_extension(se::EXTENSION);
        }
        match self.export_to(&out) {
            Ok(()) => {
                if self.globals.bool(REVEAL_KEY) {
                    self.reveal_path(&out);
                }
            }
            Err(e) => {
                let ask = Ask { heading: se::PROBLEM_EXPORTING.to_string(), text: e.message(), buttons: vec![crate::api_auth::OK.to_string()], silence: None, then: Then::ExportFailed };
                self.ask_scripts(ask);
            }
        }
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
        export_into(out, &Sources { app_support: &app_support, scripts: local.then_some(scripts.as_path()), user_defaults: &user_defaults })
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
        export_into(&out, &Sources { app_support: &appdata, scripts: Some(&scripts), user_defaults: &snap }).unwrap();
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
        export_into(&out, &Sources { app_support: &appdata, scripts: Some(&scripts), user_defaults: &snap }).unwrap();

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
}
