//! How ssh.exe is connected to a pane for SSH integration (milestone 14a, ruling 1): the shipped
//! ConPTY in passthrough mode, then normal ConPTY, then plain pipes, in that order. Task 1's probe
//! (`ssh_probe_win`) measured which one carries iTerm2's protocol intact; `CHOSEN` is its answer.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Transport {
    /// The shipped ConPTY created with `PSEUDOCONSOLE_PASSTHROUGH_MODE`.
    Passthrough,
    /// The shipped ConPTY with no flags, as every other pane.
    Conpty,
    /// ssh.exe on plain pipes, with no console (no resize).
    Pipes,
}

/// Ruling 1's order: the first that passes the probe is used.
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

/// The probe's choice (tests/fixtures/ssh, `ssh_probe` `probe_fixtures_follow_the_ruling`).
/// PLACEHOLDER: the probe chose none (both ConPTY modes turn invalid UTF-8 into U+FFFD, pipes
/// cannot start ssh.exe), so this waits for Robert's ruling (ruling 1) before Task 2 uses it.
pub const CHOSEN: Transport = Transport::Conpty;

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
