//! The framer half of the Conductor (`CR`, milestone 14c): iTerm2 splits
//! `Conductor+SSHCommandRunning.swift` from `Conductor.swift` the same way.

use crate::conductor::{Command, Conductor, Handler};
use crate::framer_script::framer_code;

/// How many `%it2` frames a conductor keeps.
const IT2_INBOX_MAX: usize = 16;
/// The most of one `%it2` frame it keeps (F30).
pub(crate) const IT2_ENTRY_MAX: usize = 64 * 1024;

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

    /// handleTerminate (`CD:2544-2550`): a process of a deeper conductor is the parent's.
    /// Task 4 gives the body.
    pub fn handle_terminate(&mut self, pid: i32, rc: i32, depth: i32) {
        if depth != self.depth() {
            self.via_parent(|parent| parent.handle_terminate(pid, rc, depth));
            return;
        }
        #[cfg(test)]
        self.heard.push(format!("terminate {pid} {rc}"));
    }

    /// handleSideChannelOutput (`CD:2582-2591`): routed by depth like the rest. Task 4 gives
    /// the body.
    pub fn handle_side(&mut self, text: &str, pid: i32, channel: i32, depth: i32) {
        if depth != self.depth() {
            self.via_parent(|parent| parent.handle_side(text, pid, channel, depth));
            return;
        }
        #[cfg(test)]
        self.heard.push(format!("side {text} {pid} {channel}"));
    }

    /// handleIT2 (`CD:2593-2600`): a `%it2` frame is kept by the conductor that owns its
    /// depth, the newest 16, each cut at 64 KiB (F30). It raises no prompt and writes nothing
    /// (F29): no proxy reads it.
    pub fn handle_it2(&mut self, text: &str, depth: i32) {
        if depth != self.depth() {
            self.via_parent(|parent| parent.handle_it2(text, depth));
            return;
        }
        let mut end = text.len().min(IT2_ENTRY_MAX);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.it2_inbox.push(text[..end].to_string());
        if self.it2_inbox.len() > IT2_INBOX_MAX {
            self.it2_inbox.remove(0);
        }
    }

    /// The framer protocol version, None until the framer is launched.
    pub fn framer_version(&self) -> Option<u8> {
        self.framer_version
    }
}

#[cfg(test)]
mod tests {
    use super::IT2_ENTRY_MAX;
    use crate::conductor::{python_status, Command, Conductor, Effect, Handler, Setup, State};
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

    /// A framing parent (pid 100, depth 0) with a poll in flight and a framing child (pid 200,
    /// depth 1) with a poll in flight, the child the session's innermost conductor.
    fn nest() -> Conductor {
        let mut parent = Conductor::new(Setup { client_unique_id: "A".into(), ..Setup::default() }, None, "P");
        parent.framed_pid = Some(100);
        parent.send(Command::FramerPoll, Handler::FireAndForget, false);
        parent.send(Command::FramerPoll, Handler::FireAndForget, false);
        let mut child = Conductor::new(Setup { client_unique_id: "B".into(), ..Setup::default() }, Some(Box::new(parent)), "V");
        child.framed_pid = Some(200);
        child.send(Command::FramerPoll, Handler::FireAndForget, false);
        assert_eq!((child.depth(), child.parent.as_ref().unwrap().depth()), (1, 0));
        child.take_effects();
        child
    }

    fn parent_of(c: &Conductor) -> &Conductor {
        c.parent.as_ref().unwrap()
    }

    #[test]
    fn depth_routing_follows_iterm2() {
        // A line of depth 0 reaches the parent's in-flight command, depth 1 the child's.
        let mut c = nest();
        assert!(matches!(parent_of(&c).state(), State::WillExecute(_)) && matches!(c.state(), State::WillExecute(_)));
        c.handle_line("L", 0);
        assert!(matches!(parent_of(&c).state(), State::Executing(..)), "the parent heard it");
        assert!(matches!(c.state(), State::WillExecute(_)), "the child did not");
        c.handle_line("L", 1);
        assert!(matches!(c.state(), State::Executing(..)));
        // An end of depth 0 goes to the parent, which writes its next command; the app drains
        // only the innermost conductor, so the child's take_effects has it.
        c.handle_end("f", "f", 0, 0);
        assert_eq!(parent_of(&c).in_flight(), Some(&Command::FramerPoll), "the parent's second poll");
        assert!(matches!(c.state(), State::Executing(..)), "the child's is still in flight");
        assert!(!parent_of(&c).has_effects());
        assert_eq!(c.take_effects(), [Effect::Write(b"cG9sbA==\n\n".to_vec())]);
        assert_eq!(c.take_effects(), []);
        c.handle_end("f", "f", 0, 1);
        assert_eq!(c.state(), &State::Ground);
        // A non-framing child whose in-flight command is not a framer command passes an end
        // of kind "f" up and handles kind "r" itself (CD:2482-2491).
        let plain = || {
            let mut parent = Conductor::new(Setup { client_unique_id: "A".into(), ..Setup::default() }, None, "P");
            parent.framed_pid = Some(100);
            parent.send(Command::FramerPoll, Handler::FireAndForget, false);
            let mut child = Conductor::new(Setup { client_unique_id: "B".into(), ..Setup::default() }, Some(Box::new(parent)), "V");
            child.send(Command::Cd("/a".into()), Handler::FireAndForget, false);
            child.take_effects();
            child
        };
        let mut c = plain();
        assert!(!c.framing() && c.in_flight() == Some(&Command::Cd("/a".into())));
        c.handle_end("f", "f", 0, 0);
        assert_eq!(parent_of(&c).in_flight(), None, "the parent took the framer end");
        assert_eq!(c.in_flight(), Some(&Command::Cd("/a".into())));
        let mut c = plain();
        c.handle_end("r", "r", 0, 1);
        assert_eq!(c.in_flight(), None, "an r end is the child's own");
        assert_eq!(parent_of(&c).in_flight(), Some(&Command::FramerPoll));
    }

    #[test]
    fn routing_of_the_rest_follows_iterm2() {
        let mut c = nest();
        c.handle_terminate(7, 0, 0);
        c.handle_side("x", 7, 1, 0);
        assert_eq!(c.heard, Vec::<String>::new());
        assert_eq!(parent_of(&c).heard, ["terminate 7 0", "side x 7 1"]);
        c.handle_terminate(8, 3, 1);
        c.handle_side("y", 8, 2, 1);
        assert_eq!(c.heard, ["terminate 8 3", "side y 8 2"]);
        assert_eq!(parent_of(&c).heard.len(), 2);
    }

    #[test]
    fn it2_frames_are_kept_and_never_acted_on() {
        let mut c = nest();
        c.handle_it2("3 open", 0);
        assert_eq!(parent_of(&c).it2_inbox, ["3 open"]);
        assert!(c.it2_inbox.is_empty());
        c.handle_it2("3 open", 1);
        assert_eq!(c.it2_inbox, ["3 open"]);
        // No prompt, no write, no state change (F29): nothing is queued for the app.
        assert_eq!(c.take_effects(), []);
        assert!(matches!(c.state(), State::WillExecute(_)));
        // Twenty entries keep the newest sixteen.
        for n in 0..20 {
            c.handle_it2(&format!("{n} e"), 1);
        }
        assert_eq!(c.it2_inbox.len(), 16);
        assert_eq!((c.it2_inbox[0].as_str(), c.it2_inbox[15].as_str()), ("4 e", "19 e"));
        // An entry is cut at 64 KiB, on a character boundary (F30).
        c.handle_it2(&"\u{e9}".repeat(100_000), 1);
        let last = c.it2_inbox.last().unwrap();
        assert!(last.len() <= IT2_ENTRY_MAX && last.len() > IT2_ENTRY_MAX - 4, "{}", last.len());
        assert!(last.chars().all(|ch| ch == '\u{e9}'));
        assert!(c.it2_inbox.iter().map(String::len).sum::<usize>() <= 16 * IT2_ENTRY_MAX);
    }
}
