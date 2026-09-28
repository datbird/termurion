//! The framer half of the Conductor (`CR`, milestone 14c): iTerm2 splits
//! `Conductor+SSHCommandRunning.swift` from `Conductor.swift` the same way.

use crate::conductor::{Command, Conductor, Handler};
use crate::framer_script::framer_code;

impl Conductor {
    /// The framer path after python is found (`CD:2130-2161`). Task 5 completes it.
    pub fn frame(&mut self) {
        self.exec_framer();
    }

    /// execFramer (`CD:2044-2063`): `runpython` with the framer as its program, written on
    /// success; the version is set at once, before any reply.
    pub(crate) fn exec_framer(&mut self) {
        let code = framer_code(self.depth());
        self.send(Command::RunPython(code.clone()), Handler::WriteOnSuccess(code), false);
        self.framer_version = Some(2);
    }

    /// The framer protocol version, None until the framer is launched.
    pub fn framer_version(&self) -> Option<u8> {
        self.framer_version
    }
}

#[cfg(test)]
mod tests {
    use crate::conductor::{python_status, Command, Conductor, Effect, Setup};
    use crate::conductor_parser::Token;
    use crate::framer_script::framer_code;

    fn feed(c: &mut Conductor, tokens: &[Token]) {
        for t in tokens {
            match t {
                Token::Begin(id) => c.handle_begin(id, 0),
                Token::Line(l) => c.handle_line(l, 0),
                Token::End { id, status, kind } => c.handle_end(id, kind, *status, 0),
                _ => unreachable!(),
            }
        }
    }

    fn end(id: &str, status: u8, kind: &str) -> Token {
        Token::End { id: id.into(), status, kind: kind.into() }
    }

    /// A conductor driven through the recorded getshell reply to the pythonversion step
    /// (F31), its writes taken; the recorded pythonversion reply then goes in.
    fn at_runpython() -> Conductor {
        let mut c = Conductor::new(Setup { client_unique_id: "A".into(), ..Setup::default() }, None, "P");
        c.start();
        feed(&mut c, &[Token::Begin("1.1".into()), Token::Line("/bin/sh".into()), Token::Line("\n/home/u".into()), end("1.1", 0, "r")]);
        assert_eq!(c.in_flight(), Some(&Command::PythonVersion));
        c.take_effects();
        feed(&mut c, &[Token::Begin("2.1".into()), Token::Line("Python 3.11.4".into()), end("2.1", 0, "r")]);
        c
    }

    #[test]
    fn exec_framer_follows_iterm2() {
        let mut c = at_runpython();
        assert_eq!(c.take_effects(), [Effect::Write(b"\x03PcnVucHl0aG9u\n\n".to_vec())]);
        assert_eq!(c.framer_version(), Some(2));
        assert_eq!(c.in_flight(), Some(&Command::RunPython(framer_code(0))));
        feed(&mut c, &[Token::Begin("3.1".into()), Token::Line("31337".into()), end("3.1", 0, "0")]);
        assert_eq!(c.take_effects(), [Effect::Write((framer_code(0) + "\nEOF\n\n").into_bytes())]);
    }

    #[test]
    fn python_failure_follows_iterm2() {
        let mut c = at_runpython();
        c.take_effects();
        feed(&mut c, &[Token::Begin("3.1".into()), Token::Line("31337".into()), end("3.1", 1, "0")]);
        assert_eq!(c.take_effects(), [Effect::SessionWrite(b"exec_login_shell\n".to_vec()), Effect::Abort(python_status(1))]);
        assert_eq!(python_status(1), "Status 1 when running python code");
    }
}
