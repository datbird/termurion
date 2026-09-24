//! One terminal pane: a shell in a pty, and its screen.

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::TermMode;

use crate::conpty::Api;
use crate::layout::PaneId;
use crate::pty::{self, Notify, Pty, Read};
use crate::screen::Screen;

/// The bytes to send for a paste. Newlines become carriage returns, as a typed
/// Enter would. Bracketed paste wraps the text so the app knows it was pasted,
/// and removes every ESC from it, so no end marker can form inside and a paste
/// cannot break out and run commands.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if !bracketed {
        return text.into_bytes();
    }
    format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes()
}

pub struct Pane {
    pub id: PaneId,
    pub screen: Screen,
    pty: Option<Pty>,
    pub exited: Option<u32>,
    pub error: Option<String>,
    /// The Guid of the profile this pane runs.
    pub profile: String,
}

impl Pane {
    pub fn start(id: PaneId, api: &Api, cmdline: &str, cwd: Option<&std::path::Path>, cols: u16, rows: u16, notify: Notify) -> Result<Pane, String> {
        let (cols, rows) = (cols.max(2), rows.max(1));
        let pty = pty::spawn_notify(api, cmdline, cwd, cols, rows, notify)?;
        Ok(Pane { id, screen: Screen::new(cols as usize, rows as usize), pty: Some(pty), exited: None, error: None, profile: String::new() })
    }

    /// Feeds all waiting output into the screen. Returns true if anything changed.
    pub fn pump(&mut self) -> bool {
        let Some(pty) = &self.pty else { return false };
        let mut changed = false;
        while let Read::Data(chunk) = pty.try_read() {
            changed = true;
            let reply = self.screen.feed(&chunk);
            if !reply.is_empty() {
                if let Err(e) = pty.write(&reply) {
                    self.error = Some(e);
                }
            }
        }
        if self.exited.is_none() {
            if let Some(code) = pty.exit_code() {
                self.exited = Some(code);
                changed = true;
            }
        }
        changed
    }

    pub fn input(&mut self, bytes: &[u8]) {
        if let Some(pty) = &self.pty {
            if let Err(e) = pty.write(bytes) {
                self.error = Some(e);
            }
        }
    }

    pub fn paste(&mut self, text: &str) {
        let bracketed = self.screen.term().mode().contains(TermMode::BRACKETED_PASTE);
        self.input(&paste_bytes(text, bracketed));
    }

    /// Resizes the screen and the pty. A pane squeezed to nothing keeps 2x1.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(2), rows.max(1));
        if (self.screen.term().columns(), self.screen.term().screen_lines()) == (cols as usize, rows as usize) {
            return;
        }
        self.screen.resize(cols as usize, rows as usize);
        if let Some(pty) = &self.pty {
            if let Err(e) = pty.resize(cols, rows) {
                self.error = Some(e);
            }
        }
    }

    /// iTerm2's Clear to Last Mark: clears from after the last mark above the prompt, then
    /// clears ConPTY's buffer too (keeping the cursor row), so both agree.
    pub fn clear_to_last_mark(&mut self) {
        let lines: Vec<i32> = self.screen.marks_in_order().iter().map(|(l, _)| *l).collect();
        let cursor = self.screen.term().grid().cursor.point.line.0;
        let start = crate::marks::clear_start(&lines, self.screen.current_prompt_line(), cursor);
        self.screen.clear_from(start);
        self.screen.term_mut().selection = None;
        if let Some(pty) = &self.pty {
            if let Err(e) = pty.clear() {
                self.error = Some(e);
            }
        }
    }

    /// iTerm2's Clear Buffer: the screen and ConPTY both clear, keeping the cursor line.
    pub fn clear(&mut self) {
        self.screen.clear(false);
        if let Some(pty) = &self.pty {
            if let Err(e) = pty.clear() {
                self.error = Some(e);
            }
        }
    }

    pub fn scroll(&mut self, scroll: Scroll) {
        self.screen.term_mut().scroll_display(scroll);
    }
}

impl Drop for Pane {
    /// Closing a pty can take about 2 s on the Windows copy of ConPTY (milestone 0),
    /// so it happens on its own thread.
    fn drop(&mut self) {
        if let Some(pty) = self.pty.take() {
            std::thread::spawn(move || drop(pty));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conpty::{self, Source, dev_vendor_dir};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn start(cmd: &str) -> Pane {
        let api = conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap();
        Pane::start(1, &api, cmd, None, 80, 24, Arc::new(|| {})).unwrap()
    }

    fn pump_until(p: &mut Pane, done: impl Fn(&Pane) -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            p.pump();
            if done(p) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn clear_buffer_keeps_conpty_in_step() {
        let mut p = start("cmd.exe");
        assert!(pump_until(&mut p, |p| p.screen.text().contains('>')), "no prompt");
        p.input(b"for /l %i in (1,1,30) do @echo L%i\r");
        assert!(pump_until(&mut p, |p| {
            let t = p.screen.all_text();
            t.contains("L30\n") && t.trim_end().ends_with('>')
        }), "no output");
        p.clear();
        assert!(!p.screen.all_text().contains("L29"));
        // ConPTY must clear too, or it keeps drawing where its own cursor was: near the
        // bottom of the screen.
        // The clear reaches ConPTY on its signal pipe, not in order with typed input, so
        // give it a moment as a person would.
        std::thread::sleep(Duration::from_millis(300));
        p.input(b"echo MARK\r");
        assert!(pump_until(&mut p, |p| p.screen.all_text().matches("MARK").count() >= 2), "no echo");
        let rows: Vec<usize> = p.screen.text().lines().enumerate().filter(|(_, l)| l.contains("MARK")).map(|(i, _)| i).collect();
        assert!(rows.iter().all(|&r| r < 4), "MARK on rows {rows:?}:\n{}", p.screen.all_text());
        assert!(p.error.is_none(), "{:?}", p.error);
    }

    #[test]
    fn the_pwsh_script_emits_prompt_marks() {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/shell_integration/tandem_shell_integration.ps1");
        // Windows PowerShell 5.1 is on every Windows machine, and is the older of the two
        // PowerShells the script supports.
        let cmd = format!("powershell.exe -NoProfile -NoLogo -Command \". '{}'; prompt; Start-Sleep -Milliseconds 300\"", script.display());
        let mut p = start(&cmd);
        assert!(pump_until(&mut p, |p| p.exited.is_some()), "pwsh did not finish");
        p.pump();
        assert_eq!(p.screen.marks_in_order().len(), 1, "{}", p.screen.all_text());
        assert!(p.screen.shell.cwd.as_deref().is_some_and(|c| c.contains(':')), "{:?}", p.screen.shell.cwd);
        assert_eq!(p.screen.shell.shell.as_deref(), Some("powershell"));
    }

    #[test]
    fn clear_to_last_mark_keeps_conpty_in_step() {
        let mut p = start("cmd.exe");
        assert!(pump_until(&mut p, |p| p.screen.text().contains('>')), "no prompt");
        p.input(b"for /l %i in (1,1,30) do @echo L%i\r");
        assert!(pump_until(&mut p, |p| {
            let t = p.screen.all_text();
            t.contains("L30\n") && t.trim_end().ends_with('>')
        }), "no output");
        let top = -(p.screen.term().grid().history_size() as i32);
        let line = (top..24).find(|l| p.screen.line_text(*l) == "L20").expect("L20 row");
        p.screen.set_manual_mark(line, None);
        p.clear_to_last_mark();
        let t = p.screen.all_text();
        assert!(t.contains("L20\n") && !t.contains("L21"), "{t}");
        std::thread::sleep(Duration::from_millis(300));
        p.input(b"echo MARK\r");
        assert!(pump_until(&mut p, |p| p.screen.all_text().matches("MARK").count() >= 2), "no echo");
        let rows: Vec<usize> = p.screen.text().lines().enumerate().filter(|(_, l)| l.contains("MARK")).map(|(i, _)| i).collect();
        assert!(rows.iter().all(|&r| r < 4), "MARK on rows {rows:?}:\n{}", p.screen.all_text());
    }

    #[test]
    fn plain_paste_uses_carriage_returns() {
        assert_eq!(paste_bytes("a\r\nb\nc", false), b"a\rb\rc");
    }

    #[test]
    fn bracketed_paste_wraps() {
        assert_eq!(paste_bytes("hi", true), b"\x1b[200~hi\x1b[201~");
    }

    #[test]
    fn bracketed_paste_strips_the_end_marker() {
        assert_eq!(paste_bytes("a\x1b[201~rm -rf x\n", true), b"\x1b[200~a[201~rm -rf x\r\x1b[201~");
    }

    #[test]
    fn bracketed_paste_strips_a_nested_end_marker() {
        // One pass of removal would join the halves into a live end marker.
        assert_eq!(paste_bytes("\x1b[20\x1b[201~1~rm -rf x", true), b"\x1b[200~[20[201~1~rm -rf x\x1b[201~");
    }

    #[test]
    fn pump_shows_output() {
        let mut p = start("cmd.exe /q /k echo pane-ok");
        assert!(pump_until(&mut p, |p| p.screen.text().contains("pane-ok")), "{}", p.screen.text());
    }

    #[test]
    fn exit_code_is_reported() {
        let mut p = start("cmd.exe /c exit 4");
        assert!(pump_until(&mut p, |p| p.exited.is_some()));
        assert_eq!(p.exited, Some(4));
    }

    #[test]
    fn resize_to_zero_keeps_a_minimum() {
        let mut p = start("cmd.exe /q /k");
        p.resize(0, 0);
        assert_eq!(p.error, None);
        assert_eq!((p.screen.term().columns(), p.screen.term().screen_lines()), (2, 1));
    }

    #[test]
    fn drop_does_not_block() {
        let p = start("cmd.exe /q /k");
        let t = Instant::now();
        drop(p);
        assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
    }
}
