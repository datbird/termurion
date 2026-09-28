//! The profile's `SSH` dictionary and the Configure SSH sheet's rules (`SC:10-55`,
//! `SC:100-291`): SSH Integration, the environment variables and files to copy to the remote
//! host, and the path to ssh.

use crate::profile::Profile;
use toml::Value;

/// The profile key holding the dictionary.
pub const KEY: &str = "SSH";

pub const SSH_INTEGRATION: &str = "SSH Integration";
pub const ENV_LABEL: &str = "Environment Variables to Copy to Remote Host:";
pub const FILES_LABEL: &str = "Files to Copy to Remote Host:";
pub const FILE_COLUMN: &str = "File or Folder";
pub const DEST_COLUMN: &str = "Destination";
pub const PATH_LABEL: &str = "Path to SSH:";
/// iTerm2's placeholder is `/usr/bin/ssh`; Windows' OpenSSH lives here.
pub const PATH_PLACEHOLDER: &str = "C:\\Windows\\System32\\OpenSSH\\ssh.exe";
pub const BROWSE: &str = "Browse";
pub const SELECT_TITLE: &str = "Select SSH Binary";
pub const NOT_EXECUTABLE: &str = "Selected file must be an executable.";
pub const DEST_LABEL: &str = "Destination folder on remote host:";
pub const HELP_URL: &str = "https://gitlab.com/gnachman/iterm2/-/wikis/SSH-Integration";
/// The button beside the command type (`PreferencePanel.xib:2107`).
pub const CONFIGURE: &str = "Configure\u{2026}";
/// Tandem's: iTerm2's sheet has no title.
pub const SHEET_TITLE: &str = "Configure SSH";
/// Tandem's: a Windows file dialog picks files or a folder, not both.
pub const ADD_FILES: &str = "Files\u{2026}";
pub const ADD_FOLDER: &str = "Folder\u{2026}";

/// One profile's SSH settings (`SC:10-14`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshConfig {
    pub ssh_integration: bool,
    pub environment_variables_to_copy: Vec<String>,
    /// (file or folder, destination folder on the remote host).
    pub files_to_copy: Vec<(String, String)>,
    pub path_to_ssh: Option<String>,
}

impl Default for SshConfig {
    fn default() -> Self {
        SshConfig { ssh_integration: true, environment_variables_to_copy: Vec::new(), files_to_copy: Vec::new(), path_to_ssh: None }
    }
}

/// Every item a string, or None (Swift's `as? [String]` fails as a whole).
fn all_strings(v: &Value) -> Option<Vec<String>> {
    v.as_array()?.iter().map(|x| x.as_str().map(str::to_string)).collect()
}

/// Reads the dictionary (`SC:36-41`); anything missing or of the wrong type is its default.
/// A `filesToCopy` entry with fewer than two strings is skipped (iTerm2 would crash).
pub fn from_value(v: Option<&Value>) -> SshConfig {
    let Some(t) = v.and_then(Value::as_table) else { return SshConfig::default() };
    let files = t.get("filesToCopy").and_then(Value::as_array).map(|a| a.iter().map(all_strings).collect::<Option<Vec<_>>>());
    SshConfig {
        ssh_integration: t.get("sshIntegration").and_then(Value::as_bool).unwrap_or(true),
        environment_variables_to_copy: t.get("environmentVariablesToCopy").and_then(all_strings).unwrap_or_default(),
        files_to_copy: files.flatten().unwrap_or_default().into_iter().filter(|e| e.len() >= 2).map(|e| (e[0].clone(), e[1].clone())).collect(),
        path_to_ssh: t.get("pathToSSH").and_then(Value::as_str).map(str::to_string),
    }
}

/// The profile's `SSH` dictionary.
pub fn from_profile(p: &Profile) -> SshConfig {
    from_value(p.get(KEY).as_ref())
}

impl SshConfig {
    /// The dictionary (`SC:23-34`): `pathToSSH` only when set.
    pub fn to_value(&self) -> Value {
        let strings = |v: &[String]| Value::Array(v.iter().cloned().map(Value::String).collect());
        let mut t = toml::Table::new();
        t.insert("sshIntegration".into(), Value::Boolean(self.ssh_integration));
        t.insert("environmentVariablesToCopy".into(), strings(&self.environment_variables_to_copy));
        t.insert("filesToCopy".into(), Value::Array(self.files_to_copy.iter().map(|(a, b)| strings(&[a.clone(), b.clone()])).collect()));
        if let Some(p) = &self.path_to_ssh {
            t.insert("pathToSSH".into(), Value::String(p.clone()));
        }
        Value::Table(t)
    }
}

/// The byte length of `home` at the start of `path`, compared ignoring case, when it ends
/// there at a folder boundary.
fn home_prefix(path: &str, home: &str) -> Option<usize> {
    let home = home.trim_end_matches(['\\', '/']);
    if home.is_empty() {
        return None;
    }
    let mut chars = path.char_indices();
    for h in home.chars() {
        let (_, c) = chars.next()?;
        if !c.to_lowercase().eq(h.to_lowercase()) {
            return None;
        }
    }
    let end = chars.next().map_or(path.len(), |(i, _)| i);
    (end == path.len() || path[end..].starts_with(['\\', '/'])).then_some(end)
}

/// One row from the Add dialog (`SC:132-179`): a source under `home` becomes `~` plus the
/// rest; an empty destination is `~`.
pub fn add_file(source: &str, destination: &str, home: &str) -> (String, String) {
    let source = match home_prefix(source, home) {
        Some(n) => format!("~{}", &source[n..]),
        None => source.to_string(),
    };
    let destination = if destination.is_empty() { "~" } else { destination };
    (source, destination.to_string())
}

/// The variables field's names: split on whitespace and commas.
pub fn env_names(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// The variables field's text for `names`.
pub fn env_text(names: &[String]) -> String {
    names.join(" ")
}

/// The stored path: none when the field is empty (`SC:261-265`).
pub fn ok_path(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_string())
}

/// Whether Browse may take `path` (`SC:194-204`): a folder or a file that is not a program is
/// refused.
pub fn browse_ok(path: &std::path::Path) -> Result<(), &'static str> {
    let exe = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    if path.is_dir() || !exe { Err(NOT_EXECUTABLE) } else { Ok(()) }
}

/// What is enabled (`SC:117-123`): the variables field and files table follow SSH
/// Integration; the remove button needs a selected row.
pub fn controls_enabled(c: &SshConfig, selected: Option<usize>) -> (bool, bool) {
    (c.ssh_integration, selected.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn table(text: &str) -> Value {
        let t: toml::Table = toml::from_str(&format!("SSH = {text}")).unwrap();
        t["SSH"].clone()
    }

    /// `SC:10-55`: the four keys, their defaults, and wrong-typed values read as defaults.
    #[test]
    fn ssh_config_follows_iterm2() {
        assert_eq!(from_value(None), SshConfig::default());
        assert_eq!(SshConfig::default(), SshConfig { ssh_integration: true, environment_variables_to_copy: vec![], files_to_copy: vec![], path_to_ssh: None });
        let full = table(r#"{ sshIntegration = false, environmentVariablesToCopy = ["LANG", "EDITOR"], filesToCopy = [['~\.vimrc', "~"]], pathToSSH = 'C:\ssh.exe' }"#);
        let c = from_value(Some(&full));
        assert_eq!(c, SshConfig { ssh_integration: false, environment_variables_to_copy: strings(&["LANG", "EDITOR"]), files_to_copy: vec![("~\\.vimrc".into(), "~".into())], path_to_ssh: Some("C:\\ssh.exe".into()) });
        assert_eq!(c.to_value(), full);
        let d = SshConfig::default().to_value();
        let t = d.as_table().unwrap();
        let mut keys: Vec<&str> = t.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["environmentVariablesToCopy", "filesToCopy", "sshIntegration"], "no pathToSSH when unset");
        assert_eq!(t["sshIntegration"], Value::Boolean(true));
        assert_eq!(t["environmentVariablesToCopy"], Value::Array(vec![]));
        assert_eq!(t["filesToCopy"], Value::Array(vec![]));
        assert_eq!(from_value(Some(&table(r#"{ filesToCopy = "x" }"#))).files_to_copy, vec![]);
        assert_eq!(from_value(Some(&table(r#"{ filesToCopy = [["a"]] }"#))).files_to_copy, vec![], "a short entry is skipped");
        assert!(from_value(Some(&table(r#"{ sshIntegration = "yes" }"#))).ssh_integration, "a wrong type reads as the default");
        assert_eq!(from_value(Some(&Value::String("x".into()))), SshConfig::default(), "not a table");
    }

    /// `SC:132-179`: a source under home is rewritten to `~` plus the rest; an empty
    /// destination is `~`.
    #[test]
    fn add_file_follows_iterm2() {
        let home = r"C:\Users\u";
        let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(add_file(r"C:\Users\u\proj\a.txt", "~", home), pair(r"~\proj\a.txt", "~"));
        assert_eq!(add_file(r"C:\Users\u\proj\a.txt", "", home), pair(r"~\proj\a.txt", "~"));
        assert_eq!(add_file(r"c:\users\U\b", "~/x", home), pair(r"~\b", "~/x"), "home matches ignoring case");
        assert_eq!(add_file(r"C:\Users\u2\c", "~", home), pair(r"C:\Users\u2\c", "~"), "only at a folder boundary");
        assert_eq!(add_file(r"D:\data\b.txt", "/opt/x", home), pair(r"D:\data\b.txt", "/opt/x"));
        assert_eq!(add_file(r"C:\Users\u", "", home), pair("~", "~"));
    }

    /// `SC:100-123`, `SC:183-269`: the variables field, the path, Browse, and what is enabled.
    #[test]
    fn sheet_rules_follow_iterm2() {
        assert_eq!(env_names("LANG EDITOR,  TZ"), strings(&["LANG", "EDITOR", "TZ"]));
        assert_eq!(env_names(""), Vec::<String>::new());
        assert_eq!(env_text(&strings(&["A", "B"])), "A B");
        assert_eq!(ok_path(""), None);
        assert_eq!(ok_path("C:\\s.exe"), Some("C:\\s.exe".to_string()));
        let dir = std::env::temp_dir().join(format!("tandem-ssh-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("x.EXE");
        let txt = dir.join("x.txt");
        std::fs::write(&exe, b"").unwrap();
        std::fs::write(&txt, b"").unwrap();
        assert_eq!(browse_ok(&exe), Ok(()));
        assert_eq!(browse_ok(&txt), Err(NOT_EXECUTABLE));
        assert_eq!(browse_ok(&dir), Err(NOT_EXECUTABLE));
        std::fs::remove_dir_all(&dir).unwrap();
        let on = SshConfig::default();
        let off = SshConfig { ssh_integration: false, ..SshConfig::default() };
        assert_eq!(controls_enabled(&on, None), (true, false));
        assert_eq!(controls_enabled(&on, Some(0)), (true, true));
        assert_eq!(controls_enabled(&off, Some(0)), (false, true));
    }

    /// `SX` and `SC:183-291`, with Windows' ssh.exe as the placeholder.
    #[test]
    fn sheet_texts_follow_iterm2() {
        assert_eq!(SSH_INTEGRATION, "SSH Integration");
        assert_eq!(ENV_LABEL, "Environment Variables to Copy to Remote Host:");
        assert_eq!(FILES_LABEL, "Files to Copy to Remote Host:");
        assert_eq!(FILE_COLUMN, "File or Folder");
        assert_eq!(DEST_COLUMN, "Destination");
        assert_eq!(PATH_LABEL, "Path to SSH:");
        assert_eq!(PATH_PLACEHOLDER, r"C:\Windows\System32\OpenSSH\ssh.exe");
        assert_eq!(BROWSE, "Browse");
        assert_eq!(SELECT_TITLE, "Select SSH Binary");
        assert_eq!(NOT_EXECUTABLE, "Selected file must be an executable.");
        assert_eq!(DEST_LABEL, "Destination folder on remote host:");
        assert_eq!(HELP_URL, "https://gitlab.com/gnachman/iterm2/-/wikis/SSH-Integration");
        assert_eq!(CONFIGURE, "Configure\u{2026}");
        assert_eq!(SHEET_TITLE, "Configure SSH");
        assert_eq!(ADD_FILES, "Files\u{2026}");
        assert_eq!(ADD_FOLDER, "Folder\u{2026}");
    }
}
