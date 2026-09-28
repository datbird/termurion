//! The Configure SSH sheet (`SC`, `SX`), opened from Profiles > General when the command
//! type is SSH (`PG:860-870`); OK writes the profile's `SSH` dictionary (`PG:971-986`).

use crate::profile::Profile;
use crate::ssh_config::{self as cfg, SshConfig};

const WARN: egui::Color32 = egui::Color32::from_rgb(0xD0, 0x8A, 0x00);
/// Browse's folder when the path field names none (iTerm2's `/usr/bin`).
const SSH_FOLDER: &str = "C:\\Windows\\System32\\OpenSSH";

/// The open sheet for one profile.
pub struct Sheet {
    pub guid: String,
    cfg: SshConfig,
    env_text: String,
    path_text: String,
    selected: Option<usize>,
    /// Paths chosen by Files or Folder, and the destination field, until Add or Cancel.
    adding: Option<(Vec<String>, String)>,
    /// Browse refused the chosen file.
    error: Option<&'static str>,
}

pub enum Outcome {
    Open,
    Cancel,
    Ok(SshConfig),
}

impl Sheet {
    /// The sheet loaded from the profile (`SC:100-108`).
    pub fn open(guid: &str, p: &Profile) -> Sheet {
        let c = cfg::from_profile(p);
        Sheet { guid: guid.to_string(), env_text: cfg::env_text(&c.environment_variables_to_copy), path_text: c.path_to_ssh.clone().unwrap_or_default(), cfg: c, selected: None, adding: None, error: None }
    }

    /// The settings OK stores: the table, the variables field's names and the path.
    fn finish(&self) -> SshConfig {
        SshConfig { environment_variables_to_copy: cfg::env_names(&self.env_text), path_to_ssh: cfg::ok_path(&self.path_text), ..self.cfg.clone() }
    }
}

/// Browse starts in the path field's folder when it names one.
fn browse_dir(path_text: &str) -> std::path::PathBuf {
    browse_dir_with(path_text, &std::path::Path::is_dir)
}

/// `browse_dir` with the folder test passed in. Only an absolute local folder is looked at
/// (`prompted::local_path_ok`, a verbatim `\\?\C:\` path in its plain form): a relative, rooted
/// or drive-relative one would resolve against Tandem's working directory, which can be a
/// share. A network or device folder, or one reached through a link to a network path
/// (`util_cli::may_open`), is never looked at either: the field can come from an import or a
/// settings folder, and probing `\\host` sends credentials.
fn browse_dir_with(path_text: &str, is_dir: &dyn Fn(&std::path::Path) -> bool) -> std::path::PathBuf {
    std::path::Path::new(path_text)
        .parent()
        .filter(|d| crate::prompted::local_path_ok(crate::util_cli::local_form(&d.to_string_lossy())) && crate::util_cli::may_open(d) && is_dir(d))
        .map_or_else(|| SSH_FOLDER.into(), std::path::Path::to_path_buf)
}

fn show(ctx: &egui::Context, sh: &mut Sheet, open_url: &mut Option<String>, home: &str) -> Outcome {
    let mut out = Outcome::Open;
    egui::Window::new(cfg::SHEET_TITLE).collapsible(false).resizable(false).default_width(520.0).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.checkbox(&mut sh.cfg.ssh_integration, cfg::SSH_INTEGRATION);
            if ui.button("?").clicked() {
                *open_url = Some(cfg::HELP_URL.to_string());
            }
        });
        let (fields_on, remove_on) = cfg::controls_enabled(&sh.cfg, sh.selected);
        ui.label(cfg::ENV_LABEL);
        ui.add_enabled(fields_on, egui::TextEdit::singleline(&mut sh.env_text).desired_width(f32::INFINITY));
        ui.add_space(6.0);
        ui.label(cfg::FILES_LABEL);
        ui.add_enabled_ui(fields_on, |ui| {
            egui::Grid::new("ssh-files").num_columns(3).striped(true).show(ui, |ui| {
                ui.label("");
                ui.strong(cfg::FILE_COLUMN);
                ui.strong(cfg::DEST_COLUMN);
                ui.end_row();
                for (i, (src, dst)) in sh.cfg.files_to_copy.iter_mut().enumerate() {
                    let mut picked = ui.selectable_label(sh.selected == Some(i), format!("{}", i + 1)).clicked();
                    let a = ui.add(egui::TextEdit::singleline(src).desired_width(300.0));
                    let b = ui.add(egui::TextEdit::singleline(dst).desired_width(140.0));
                    picked |= a.gained_focus() || b.gained_focus();
                    if picked {
                        sh.selected = Some(i);
                    }
                    ui.end_row();
                }
            });
        });
        ui.horizontal(|ui| {
            ui.menu_button("+", |ui| {
                if ui.button(cfg::ADD_FILES).clicked() {
                    ui.close();
                    if let Some(paths) = rfd::FileDialog::new().set_directory(home).pick_files() {
                        sh.adding = Some((paths.iter().map(|p| p.to_string_lossy().into_owned()).collect(), "~".into()));
                    }
                }
                if ui.button(cfg::ADD_FOLDER).clicked() {
                    ui.close();
                    if let Some(path) = rfd::FileDialog::new().set_directory(home).pick_folder() {
                        sh.adding = Some((vec![path.to_string_lossy().into_owned()], "~".into()));
                    }
                }
            });
            if ui.add_enabled(remove_on, egui::Button::new("\u{2212}")).clicked() {
                if let Some(i) = sh.selected.take().filter(|&i| i < sh.cfg.files_to_copy.len()) {
                    sh.cfg.files_to_copy.remove(i);
                }
            }
        });
        let mut done = None;
        if let Some((paths, dest)) = &mut sh.adding {
            ui.horizontal(|ui| {
                ui.label(cfg::DEST_LABEL);
                ui.text_edit_singleline(dest);
                if ui.button("Add").clicked() {
                    done = Some(paths.iter().map(|p| cfg::add_file(p, dest, home)).collect::<Vec<_>>());
                }
                if ui.button("Cancel").clicked() {
                    done = Some(Vec::new());
                }
            });
        }
        if let Some(rows) = done {
            sh.cfg.files_to_copy.extend(rows);
            sh.adding = None;
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(cfg::PATH_LABEL);
            if ui.add(egui::TextEdit::singleline(&mut sh.path_text).hint_text(cfg::PATH_PLACEHOLDER).desired_width(300.0)).changed() {
                sh.error = None;
            }
            if ui.button(cfg::BROWSE).clicked() {
                let dialog = rfd::FileDialog::new().set_title(cfg::SELECT_TITLE).add_filter("Programs", &["exe"]).set_directory(browse_dir(&sh.path_text));
                if let Some(path) = dialog.pick_file() {
                    match cfg::browse_ok(&path) {
                        Ok(()) => {
                            sh.path_text = path.to_string_lossy().into_owned();
                            sh.error = None;
                        }
                        Err(e) => sh.error = Some(e),
                    }
                }
            }
        });
        if let Some(e) = sh.error {
            ui.colored_label(WARN, e);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    out = Outcome::Cancel;
                }
                if ui.button("OK").clicked() {
                    out = Outcome::Ok(sh.finish());
                }
            });
        });
    });
    out
}

/// Profiles > General's `SSH` row: "Configure\u{2026}" for the SSH command type only
/// (`PG:860-870`), and the sheet while it is open for this profile. True when the profile
/// changed.
pub fn row(ui: &mut egui::Ui, guid: &str, p: &mut Profile, sheet: &mut Option<Sheet>, open_url: &mut Option<String>, home: &str) -> bool {
    if p.string("Custom Command") == "SSH" && ui.button(cfg::CONFIGURE).clicked() {
        *sheet = Some(Sheet::open(guid, p));
    }
    let Some(sh) = sheet.as_mut().filter(|s| s.guid == guid) else { return false };
    match show(ui.ctx(), sh, open_url, home) {
        Outcome::Open => false,
        Outcome::Cancel => {
            *sheet = None;
            false
        }
        Outcome::Ok(c) => {
            p.set(cfg::KEY, c.to_value());
            *sheet = None;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OK stores the table as edited, the variables field's names and an empty path as none.
    #[test]
    fn sheet_ok_reads_the_fields() {
        let mut p = Profile::default();
        let stored = SshConfig { ssh_integration: false, environment_variables_to_copy: vec!["LANG".into()], files_to_copy: vec![("~\\a".into(), "~".into())], path_to_ssh: Some("C:\\s.exe".into()) };
        p.set(cfg::KEY, stored.to_value());
        let mut sh = Sheet::open("g", &p);
        assert_eq!(sh.finish(), stored);
        assert_eq!(sh.env_text, "LANG");
        sh.env_text = "A, B".into();
        sh.path_text.clear();
        assert_eq!(sh.finish(), SshConfig { environment_variables_to_copy: vec!["A".into(), "B".into()], path_to_ssh: None, ..stored });
    }

    /// A network or device path in the field (typed, imported or loaded from a settings
    /// folder) gives OpenSSH's folder without the path being looked at (invariant 5); a
    /// local folder is looked at and used.
    #[test]
    fn browse_never_probes_a_network_path() {
        let probed = std::cell::RefCell::new(Vec::new());
        let is_dir = |d: &std::path::Path| {
            probed.borrow_mut().push(d.to_path_buf());
            true
        };
        for p in ["\\\\server\\share\\ssh.exe", "//server/share/ssh.exe", "\\\\?\\UNC\\server\\share\\ssh.exe", "\\\\.\\C:\\Windows\\ssh.exe", "\\??\\UNC\\server\\share\\ssh.exe", "/\\server\\share\\ssh.exe"] {
            assert_eq!(browse_dir_with(p, &is_dir), std::path::PathBuf::from(SSH_FOLDER), "{p}");
        }
        assert_eq!(*probed.borrow(), Vec::<std::path::PathBuf>::new());
        assert_eq!(browse_dir_with("", &is_dir), std::path::PathBuf::from(SSH_FOLDER));
        assert_eq!(browse_dir_with("ssh.exe", &is_dir), std::path::PathBuf::from(SSH_FOLDER));
        assert_eq!(browse_dir_with("C:\\Tools\\ssh.exe", &is_dir), std::path::PathBuf::from("C:\\Tools"));
        assert_eq!(*probed.borrow(), vec![std::path::PathBuf::from("C:\\Tools")]);
        assert_eq!(browse_dir_with("C:\\Tools\\ssh.exe", &|_| false), std::path::PathBuf::from(SSH_FOLDER));
    }

    /// A relative, rooted or drive-relative folder would be resolved against Tandem's working
    /// directory (which can be a share), so only an absolute local folder is ever looked at.
    #[test]
    fn browse_looks_only_at_an_absolute_local_folder() {
        let probed = std::cell::RefCell::new(Vec::new());
        let is_dir = |d: &std::path::Path| {
            probed.borrow_mut().push(d.to_path_buf());
            true
        };
        for p in ["Tools\\ssh.exe", "..\\ssh.exe", ".\\bin\\ssh.exe", "\\Tools\\ssh.exe", "/Tools/ssh.exe", "C:Tools\\ssh.exe", "C:..\\ssh.exe"] {
            assert_eq!(browse_dir_with(p, &is_dir), std::path::PathBuf::from(SSH_FOLDER), "{p}");
        }
        assert_eq!(*probed.borrow(), Vec::<std::path::PathBuf>::new());
        assert_eq!(browse_dir_with("C:/Tools/ssh.exe", &is_dir), std::path::PathBuf::from("C:/Tools"));
        assert_eq!(browse_dir_with("\\\\?\\C:\\Tools\\ssh.exe", &is_dir), std::path::PathBuf::from("\\\\?\\C:\\Tools"));
        assert_eq!(*probed.borrow(), vec![std::path::PathBuf::from("C:/Tools"), std::path::PathBuf::from("\\\\?\\C:\\Tools")]);
    }
}
