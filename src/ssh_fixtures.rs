//! Recorded-stream builders for the SSH tests (milestone 14c): byte for byte the formats
//! framer.py and conductor.sh print, and the ones Tandem's `conductor::encode` writes. Test
//! only: nothing here starts ssh, a shell, Python or a socket.

use base64::Engine;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// framer.py's `send_esc` (`F:107-112`): `ESC ] 134 ; : <payload> ESC \`.
pub fn osc134(payload: &str) -> Vec<u8> {
    format!("\x1b]134;:{payload}\x1b\\").into_bytes()
}

/// conductor.sh's `write` (`C:378-380`): `ESC ] 135 ; : <payload> ESC \`.
pub fn osc135(payload: &str) -> Vec<u8> {
    format!("\x1b]135;:{payload}\x1b\\").into_bytes()
}

/// framer.py's `print_output` (`F:1458-1466`): the block header, the raw data, the end.
pub fn output(id: &str, pid: i32, channel: i32, depth: i32, data: &[u8]) -> Vec<u8> {
    let mut out = osc134(&format!("%output {id} {pid} {channel} {depth}"));
    out.extend_from_slice(data);
    out.extend(osc134(&format!("%end {id}")));
    out
}

/// A framer command's reply (`F:1475-1491`): begin, one payload per line, end with the
/// status and the type (`f` while it has processes, else `r`).
pub fn reply(id: &str, lines: &[&str], status: u8, kind: char) -> Vec<u8> {
    let mut out = osc134(&format!("begin {id}"));
    for line in lines {
        out.extend(osc134(line));
    }
    out.extend(osc134(&format!("end {id} {status} {kind}")));
    out
}

/// A pre-framer (conductor.sh) reply: begin, the lines, `end <b> 0 r` (`C:396-419`).
pub fn pre_reply(boundary: &str, lines: &[&str]) -> Vec<u8> {
    let mut out = osc135(&format!("begin {boundary}"));
    for line in lines {
        out.extend(osc135(line));
    }
    out.extend(osc135(&format!("end {boundary} 0 r")));
    out
}

/// The autopoll block (`F:350-356`): the header, the raw rows, the end.
pub fn autopoll(id: &str, body: &str) -> Vec<u8> {
    let mut out = osc134(&format!("%autopoll {id}"));
    out.extend_from_slice(body.as_bytes());
    out.extend(osc134(&format!("%end {id}")));
    out
}

/// A `%notif` line; `text` is the whole payload ("%notif tty -echo").
pub fn notif(text: &str) -> Vec<u8> {
    osc134(text)
}

/// `%terminate <pid> <rc>` (`F:1493-1509`).
pub fn terminate(pid: i32, rc: i32) -> Vec<u8> {
    osc134(&format!("%terminate {pid} {rc}"))
}

/// The DCS hook and its init line (`C:41-53`); an empty token is "none".
pub fn dcs_hook(token: &str, unique: &str, bool_b64: &str, sshargs_b64: &str) -> Vec<u8> {
    let token = if token.is_empty() { "none" } else { token };
    format!("\x1bP2000p{token} {unique} {bool_b64} - {sshargs_b64}\n").into_bytes()
}

/// The in-band recovery block (`F:1300-1321`): a login process with its saved keys, the
/// other processes, and the end.
pub fn recovery(login: i32, keys: &[(&str, &str)], others: &[i32]) -> Vec<u8> {
    let mut out = osc134("begin-recovery");
    out.extend(osc134("recovery: version 2"));
    out.extend(osc134(&format!("recovery: login {login}")));
    for (key, value) in keys {
        out.extend(osc134(&format!("recovery: {key} {value}")));
    }
    for pid in others {
        out.extend(osc134(&format!("recovery: process {pid}")));
    }
    out.extend(osc134("end-recovery"));
    out
}

/// What `conductor::encode` plus `write`'s newline send for a framer command: each line
/// base64, joined with "\n", cut into 128-character chunks with `\` after all but the last,
/// joined with "\n", then "\n\n" (`CD:2871-2879`, `String+iTerm.swift:508-523`).
pub fn framer_cmd(lines: &[&str]) -> Vec<u8> {
    let text = lines.iter().map(|l| b64(l.as_bytes())).collect::<Vec<_>>().join("\n");
    let pieces: Vec<&str> = text.as_bytes().chunks(128).map(|c| std::str::from_utf8(c).unwrap_or_default()).collect();
    let last = pieces.len().saturating_sub(1);
    let chunks: Vec<String> = pieces.iter().enumerate().map(|(i, p)| if i < last { format!("{p}\\") } else { p.to_string() }).collect();
    (chunks.join("\n") + "\n\n").into_bytes()
}

/// A pre-framer command as written: the same chunks with the line prefix in front of each,
/// then "\n\n".
pub fn pre_cmd(prefix: &str, lines: &[&str]) -> Vec<u8> {
    let text = lines.iter().map(|l| b64(l.as_bytes())).collect::<Vec<_>>().join("\n");
    let chunks: Vec<String> = text.as_bytes().chunks(128).map(|c| format!("{prefix}{}", std::str::from_utf8(c).unwrap_or_default())).collect();
    (chunks.join("\n") + "\n\n").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conductor::{encode, Command};

    #[test]
    fn fixtures_follow_framer() {
        assert_eq!(osc134("x"), b"\x1b]134;:x\x1b\\");
        assert_eq!(osc135("x"), b"\x1b]135;:x\x1b\\");
        assert_eq!(framer_cmd(&["send", "4242", "bHMN"]), b"c2VuZA==\nNDI0Mg==\nYkhNTg==\n\n");
        let send = Command::FramerSend { data: b"ls\r".to_vec(), pid: 4242 };
        let lines: Vec<String> = send.string_value().split('\n').map(String::from).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert_eq!(framer_cmd(&refs), (encode(&send, "") + "\n").into_bytes());
        let long = Command::FramerExecPythonStatements("a".repeat(200));
        let lines: Vec<String> = long.string_value().split('\n').map(String::from).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let cmd = framer_cmd(&refs);
        assert_eq!(cmd, (encode(&long, "") + "\n").into_bytes());
        let text = String::from_utf8(cmd).unwrap();
        // Three chunks; the first holds the newline between the two base64 lines.
        assert_eq!(text.lines().filter(|l| !l.is_empty()).count(), 4);
        assert_eq!(text.lines().filter(|l| l.ends_with('\\')).count(), 2);
        // A pre-framer command: the prefix in front of each chunk.
        let cd = Command::Cd("/tmp".into());
        let lines: Vec<String> = cd.string_value().split('\n').map(String::from).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert_eq!(pre_cmd("P.", &refs), (encode(&cd, "P.") + "\n").into_bytes());
        assert_eq!(pre_reply("2.1", &["a"]), [osc135("begin 2.1"), osc135("a"), osc135("end 2.1 0 r")].concat());
        assert_eq!(reply("5", &["1"], 0, 'f'), [osc134("begin 5"), osc134("1"), osc134("end 5 0 f")].concat());
        assert_eq!(terminate(4, 0), osc134("%terminate 4 0"));
        assert_eq!(dcs_hook("", "u", "YQ==", "Yg=="), b"\x1bP2000pnone u YQ== - Yg==\n");
        assert_eq!(output("7", 1, -1, 0, b"d"), [osc134("%output 7 1 -1 0"), b"d".to_vec(), osc134("%end 7")].concat());
        assert_eq!(autopoll("9", "r\n"), [osc134("%autopoll 9"), b"r\n".to_vec(), osc134("%end 9")].concat());
        assert_eq!(
            recovery(5, &[("k", "v")], &[6]),
            [osc134("begin-recovery"), osc134("recovery: version 2"), osc134("recovery: login 5"), osc134("recovery: k v"), osc134("recovery: process 6"), osc134("end-recovery")].concat()
        );
    }
}
