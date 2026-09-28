//! How ssh.exe is connected to a pane for SSH integration (milestone 14a, ruling 1): the shipped
//! ConPTY in passthrough mode, then normal ConPTY, then plain pipes, in that order. Task 1's probe
//! (`ssh_probe_win`) measured which one carries iTerm2's protocol intact; `CHOSEN` is the
//! transport ruled on those measurements. The pipes investigation's overlapped named pipes are
//! test-only evidence for the ruling (`ssh_probe_win`), not a transport.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Transport {
    /// The shipped ConPTY created with `PSEUDOCONSOLE_PASSTHROUGH_MODE`.
    Passthrough,
    /// The shipped ConPTY with no flags, as every other pane.
    Conpty,
    /// ssh.exe on anonymous pipes, with no console (no resize).
    Pipes,
}

/// Ruling 1's order: the first that passes is used.
pub const ORDER: [Transport; 3] = [Transport::Passthrough, Transport::Conpty, Transport::Pipes];

/// `PSEUDOCONSOLE_PASSTHROUGH_MODE` in Windows Terminal's conpty.h.
pub const PASSTHROUGH_FLAG: u32 = 0x8;

/// The flags `CreatePseudoConsole` takes for `t`; None for pipes (no pseudoconsole).
pub fn conpty_flags(t: Transport) -> Option<u32> {
    match t {
        Transport::Passthrough => Some(PASSTHROUGH_FLAG),
        Transport::Conpty => Some(0),
        Transport::Pipes => None,
    }
}

pub fn label(t: Transport) -> &'static str {
    match t {
        Transport::Passthrough => "passthrough",
        Transport::Conpty => "conpty",
        Transport::Pipes => "pipes",
    }
}

/// The transport SSH integration uses: the shipped ConPTY in passthrough mode (ruling 1, decided
/// by Robert on the probe's fixtures). It carries everything byte for byte except invalid UTF-8,
/// which arrives as U+FFFD (accepted); named pipes were byte-exact but leave the remote pty 0x0
/// with no resize, so they are not used.
pub const CHOSEN: Transport = Transport::Passthrough;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transports_follow_the_ruling() {
        assert_eq!(ORDER, [Transport::Passthrough, Transport::Conpty, Transport::Pipes]);
        assert_eq!(PASSTHROUGH_FLAG, 8);
        assert_eq!(ORDER.map(conpty_flags), [Some(8), Some(0), None]);
        assert_eq!(ORDER.map(label), ["passthrough", "conpty", "pipes"]);
    }
}
