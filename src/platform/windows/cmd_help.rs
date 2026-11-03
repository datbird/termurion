//! Reading a command's help through PowerShell's `Get-Help`.

use crate::ai_actions::powershell_in;

/// Runs System32's powershell.exe hidden with `args` and answers its output (stdout, then stderr), or
/// iTerm2's error form (`PS:940`). Ends the process after ten seconds. Blocking: call it only
/// on a worker thread.
pub fn get_help(args: &[String]) -> String {
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Some(system) = crate::script_provision::system_directory() else { return "Error running Get-Help: the System32 folder is unknown.".to_string() };
    let mut child = match std::process::Command::new(powershell_in(&system)).args(args).creation_flags(CREATE_NO_WINDOW).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return format!("Error running Get-Help: {e}"),
    };
    let drain = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut b);
            }
            b
        })
    };
    let out = drain(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < limit => std::thread::sleep(std::time::Duration::from_millis(25)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return "Error running Get-Help: it took longer than 10 seconds.".to_string();
            }
            Err(e) => return format!("Error running Get-Help: {e}"),
        }
    }
    let mut text = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    text.push_str(&String::from_utf8_lossy(&err.join().unwrap_or_default()));
    text
}
