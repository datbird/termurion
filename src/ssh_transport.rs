//! How ssh.exe is connected to a pane for SSH integration (milestone 14a, ruling 1): the shipped
//! ConPTY in passthrough mode, then normal ConPTY, then plain pipes, in that order, then the pipes
//! investigation's overlapped named pipes. Task 1's probe (`ssh_probe_win`) measured which one
//! carries iTerm2's protocol intact; `CHOSEN` is its answer.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Transport {
    /// The shipped ConPTY created with `PSEUDOCONSOLE_PASSTHROUGH_MODE`.
    Passthrough,
    /// The shipped ConPTY with no flags, as every other pane.
    Conpty,
    /// ssh.exe on anonymous pipes, with no console (no resize).
    Pipes,
    /// ssh.exe on overlapped named pipes both ways, in a console with no window (no resize). The
    /// pipes investigation: OpenSSH for Windows stalls on anonymous (synchronous) pipes.
    NamedPipes,
}

/// Ruling 1's order, then the pipes investigation's variant: the first that passes is used.
pub const ORDER: [Transport; 4] = [Transport::Passthrough, Transport::Conpty, Transport::Pipes, Transport::NamedPipes];

/// `PSEUDOCONSOLE_PASSTHROUGH_MODE` in Windows Terminal's conpty.h.
pub const PASSTHROUGH_FLAG: u32 = 0x8;

/// The flags `CreatePseudoConsole` takes for `t`; None for pipes (no pseudoconsole).
pub fn conpty_flags(t: Transport) -> Option<u32> {
    match t {
        Transport::Passthrough => Some(PASSTHROUGH_FLAG),
        Transport::Conpty => Some(0),
        Transport::Pipes | Transport::NamedPipes => None,
    }
}

pub fn label(t: Transport) -> &'static str {
    match t {
        Transport::Passthrough => "passthrough",
        Transport::Conpty => "conpty",
        Transport::Pipes => "pipes",
        Transport::NamedPipes => "named-pipes",
    }
}

/// The probe's choice (tests/fixtures/ssh, `ssh_probe` `probe_fixtures_follow_the_ruling`): both
/// ConPTY modes turn invalid UTF-8 into U+FFFD, anonymous pipes stall ssh.exe, and overlapped named
/// pipes carry everything byte for byte (no resize: the remote pty stays 0x0).
pub const CHOSEN: Transport = Transport::NamedPipes;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transports_follow_the_ruling() {
        assert_eq!(ORDER, [Transport::Passthrough, Transport::Conpty, Transport::Pipes, Transport::NamedPipes]);
        assert_eq!(PASSTHROUGH_FLAG, 8);
        assert_eq!(ORDER.map(conpty_flags), [Some(8), Some(0), None, None]);
        assert_eq!(ORDER.map(label), ["passthrough", "conpty", "pipes", "named-pipes"]);
    }
}
