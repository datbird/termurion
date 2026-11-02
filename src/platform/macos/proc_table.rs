//! The macOS process table, from libproc and sysctl.

use std::ffi::c_void;

use crate::platform::proc_table::Row;
use crate::platform::unix::proc_table::join_argv;

/// `SZOMB` from `<sys/proc.h>`: exited, not yet reaped.
const ZOMBIE: u32 = 5;

fn bsdinfo(pid: u32) -> Option<libc::proc_bsdinfo> {
    // SAFETY: a plain C struct that the call fills; the size is checked on return.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let got = libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast::<c_void>(), size);
        (got == size && info.pbi_pid == pid).then_some(info)
    }
}

fn c_name(raw: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = raw.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The command name: the long one (`pbi_name`) when the kernel has it, else the short one.
fn name_of(info: &libc::proc_bsdinfo) -> String {
    let long = c_name(&info.pbi_name);
    if long.is_empty() { c_name(&info.pbi_comm) } else { long }
}

/// Every live process. A zombie is skipped; a process whose group is not its terminal's
/// foreground group (or that has no terminal) is background.
pub fn rows() -> Result<Vec<Row>, String> {
    // SAFETY: the first call only counts; the second fills a buffer sized in bytes.
    let pids: Vec<i32> = unsafe {
        let count = libc::proc_listallpids(std::ptr::null_mut(), 0);
        if count <= 0 {
            return Err(format!("proc_listallpids failed: {}", std::io::Error::last_os_error()));
        }
        // Room for processes born between the two calls.
        let mut buf = vec![0i32; count as usize + 64];
        let got = libc::proc_listallpids(buf.as_mut_ptr().cast::<c_void>(), (buf.len() * std::mem::size_of::<i32>()) as libc::c_int);
        if got <= 0 {
            return Err(format!("proc_listallpids failed: {}", std::io::Error::last_os_error()));
        }
        buf.truncate(got as usize);
        buf
    };
    let mut out = Vec::with_capacity(pids.len());
    for pid in pids.into_iter().filter(|&p| p > 0) {
        // The process may end between the listing and the read.
        let Some(info) = bsdinfo(pid as u32) else { continue };
        if info.pbi_status == ZOMBIE {
            continue;
        }
        out.push(Row { pid: pid as u32, ppid: info.pbi_ppid, name: name_of(&info), background: info.pbi_pgid != info.e_tpgid });
    }
    Ok(out)
}

/// A process seen once: its start time (microseconds) is fixed at open.
pub struct Opened {
    pid: u32,
    start: u64,
}

pub fn open(pid: u32) -> Option<Opened> {
    let info = bsdinfo(pid).filter(|i| i.pbi_status != ZOMBIE)?;
    Some(Opened { pid, start: info.pbi_start_tvsec.saturating_mul(1_000_000).saturating_add(info.pbi_start_tvusec) })
}

impl Opened {
    pub fn created(&self) -> u64 {
        self.start
    }

    pub fn image(&self) -> Option<String> {
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer is as long as the size passed.
        let n = unsafe { libc::proc_pidpath(self.pid as libc::c_int, buf.as_mut_ptr().cast::<c_void>(), buf.len() as u32) };
        (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
    }

    pub fn cmdline(&self) -> Option<String> {
        let args = parse_procargs(&procargs(self.pid)?);
        (!args.is_empty()).then(|| join_argv(&args))
    }
}

/// The `KERN_PROCARGS2` block of `pid`: argc, the program path, then argv and the environment.
fn procargs(pid: u32) -> Option<Vec<u8>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    // The size can grow between the two calls; one retry covers it.
    for _ in 0..2 {
        // SAFETY: sysctl with a mib of three and a buffer of the size it reports.
        unsafe {
            let mut size = 0usize;
            if libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) != 0 || size == 0 {
                return None;
            }
            let mut buf = vec![0u8; size + 256];
            let mut len = buf.len();
            if libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast::<c_void>(), &mut len, std::ptr::null_mut(), 0) == 0 {
                buf.truncate(len);
                return Some(buf);
            }
        }
    }
    None
}

/// The argv words of a `KERN_PROCARGS2` block: a native-endian i32 argc, the program path and
/// its NUL padding, then argc NUL-ended strings (the environment after them is not read).
pub fn parse_procargs(block: &[u8]) -> Vec<String> {
    let Some(head) = block.get(..4) else { return Vec::new() };
    let argc = i32::from_ne_bytes([head[0], head[1], head[2], head[3]]).max(0) as usize;
    let rest = &block[4..];
    // Past the path and its NUL, then the NUL padding to the next multiple of 8 (counted from the
    // path's start, as the kernel lays it out). Only that far, so an empty argv[0] stays an argument.
    let end = rest.iter().position(|&b| b == 0).map_or(rest.len(), |i| i + 1);
    let at = end.next_multiple_of(8).min(rest.len());
    rest[at..].split(|&b| b == 0).take(argc).map(|w| String::from_utf8_lossy(w).into_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procargs_blocks_parse() {
        let mut b = 3i32.to_ne_bytes().to_vec();
        b.extend_from_slice(b"/bin/sh\0sh\0-c\0sleep 5\0HOME=/x\0");
        assert_eq!(parse_procargs(&b), vec!["sh", "-c", "sleep 5"]);
        let mut e = 3i32.to_ne_bytes().to_vec();
        e.extend_from_slice(b"/bin/sh\0\0-c\0x\0HOME=/x\0");
        assert_eq!(parse_procargs(&e), vec!["", "-c", "x"], "an empty argv[0] keeps its place");
        assert!(parse_procargs(b"").is_empty());
        assert!(parse_procargs(&0i32.to_ne_bytes()).is_empty());
    }

    #[test]
    fn a_real_empty_argv0_keeps_its_place() {
        use std::os::unix::process::CommandExt;
        // Paths of several lengths, so the padding after each is tried.
        let dir = std::env::temp_dir().join(format!("termurion-argv-{}-with-a-long-folder-name", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let long = dir.join("sleep");
        let _ = std::os::unix::fs::symlink("/bin/sleep", &long);
        for exe in ["/bin/sleep", long.to_str().unwrap()] {
            for (a0, want) in [("", " 30"), ("s", "s 30"), ("a b", "\"a b\" 30")] {
                let mut child = std::process::Command::new(exe).arg0(a0).arg("30").spawn().unwrap();
                let line = open(child.id()).and_then(|o| o.cmdline());
                let _ = child.kill();
                let _ = child.wait();
                assert_eq!(line.as_deref(), Some(want), "{exe} with argv0 {a0:?}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn our_own_row_and_facts_are_there() {
        let me = std::process::id();
        let all = rows().unwrap();
        let mine = all.iter().find(|r| r.pid == me).expect("our row");
        assert_eq!(mine.ppid, std::os::unix::process::parent_id());
        let o = open(me).unwrap();
        assert!(o.created() > 0);
        assert!(o.image().unwrap().starts_with('/'));
        assert!(o.cmdline().is_some_and(|c| !c.is_empty()));
        assert!(open(4_000_000).is_none());
    }
}
