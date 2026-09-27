//! it2api's side of the utilities (11e Task 7). Only the Python finder is here so far: the
//! Python runtime (`script_runtime`) uses it to find an installed Python 3; 11e Task 7 adds
//! the script, its texts and the `it2api` entry point.

use std::path::Path;

/// The Pythons it2api may run, best first: the first `py.exe` on PATH as `py -3`, then each
/// `python.exe` and `python3.exe` on PATH in PATH order, skipping the WindowsApps aliases
/// (they open the Store instead of running Python).
pub fn python_candidates(path_var: &str, is_file: &dyn Fn(&Path) -> bool) -> Vec<Vec<String>> {
    let dirs: Vec<&str> = path_var.split(';').map(str::trim).filter(|d| !d.is_empty()).collect();
    let mut out = Vec::new();
    if let Some(py) = dirs.iter().map(|d| Path::new(d).join("py.exe")).find(|p| is_file(p)) {
        out.push(vec![py.to_string_lossy().into_owned(), "-3".to_string()]);
    }
    for dir in &dirs {
        for name in ["python.exe", "python3.exe"] {
            let exe = Path::new(dir).join(name);
            let text = exe.to_string_lossy().into_owned();
            if !text.to_ascii_lowercase().contains("\\windowsapps\\") && is_file(&exe) {
                out.push(vec![text]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn python_follows_windows() {
        let path = r"C:\Users\u\AppData\Local\Microsoft\WindowsApps;C:\Py312;C:\Windows";
        let files = [r"C:\Users\u\AppData\Local\Microsoft\WindowsApps\python.exe", r"C:\Py312\python.exe", r"C:\Windows\py.exe"];
        let is_file = |p: &Path| files.iter().any(|f| Path::new(f) == p);
        assert_eq!(
            python_candidates(path, &is_file),
            vec![vec![r"C:\Windows\py.exe".to_string(), "-3".to_string()], vec![r"C:\Py312\python.exe".to_string()]]
        );
        assert!(python_candidates(path, &|_: &Path| false).is_empty());
    }
}
