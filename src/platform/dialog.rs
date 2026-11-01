//! Message boxes. Windows runs them through `MessageBoxW` (with the watcher that keeps one in
//! front); Unix shows a `rfd::MessageDialog`.

#[cfg(unix)]
use super::unix::dialog as imp;
#[cfg(windows)]
use super::windows::dialog as imp;

/// The disclosure's prompt (`QuitAlert.WhyPrompt`); its lead-in is `close_rules::quit_parts`'.
pub const WHY_PROMPT: &str = "Why am I being prompted?";

/// What a box says and which buttons it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// OK, information icon.
    Info,
    /// OK, warning icon.
    Warning,
    /// OK, error icon.
    Error,
    /// Yes and No, question icon.
    AskYesNo,
    /// OK and Cancel, question icon.
    AskOkCancel,
    /// OK and Cancel, warning icon.
    ConfirmOkCancel,
    /// OK and Cancel, warning icon, on top of everything and in front (a box of a thread that owns
    /// no window).
    ConfirmOkCancelOnTop,
}

/// The button row a `Kind` shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Buttons {
    Ok,
    OkCancel,
    YesNo,
}

/// What the person pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Ok,
    Cancel,
    Yes,
    No,
}

/// The buttons of a kind, shared by both backends.
pub fn buttons(kind: Kind) -> Buttons {
    match kind {
        Kind::Info | Kind::Warning | Kind::Error => Buttons::Ok,
        Kind::AskYesNo => Buttons::YesNo,
        Kind::AskOkCancel | Kind::ConfirmOkCancel | Kind::ConfirmOkCancelOnTop => Buttons::OkCancel,
    }
}

/// Shows a box and waits for the answer. `owner` is the native handle of the window the box belongs
/// to (it is disabled while the box is open), or None for a box of its own. A box closed without a
/// choice answers the negative one (Cancel, or No).
pub fn message(owner: Option<isize>, title: &str, body: &str, kind: Kind) -> Answer {
    imp::message(owner, title, body, kind)
}

/// Whether an answer is the affirmative one of its button row (OK or Yes).
pub fn accepted(answer: Answer) -> bool {
    matches!(answer, Answer::Ok | Answer::Yes)
}

/// The quit question with a collapsed "why" disclosure. None when this OS has no such dialog; the
/// caller then asks with a plain box. True for OK.
pub fn confirm_with_disclosure(owner: Option<isize>, topmost: bool, title: &str, text: &str, prompt: &str, detail: &str) -> Option<bool> {
    imp::confirm_with_disclosure(owner, topmost, title, text, prompt, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_answer_maps_buttons() {
        assert_eq!(buttons(Kind::Info), Buttons::Ok);
        assert_eq!(buttons(Kind::Warning), Buttons::Ok);
        assert_eq!(buttons(Kind::Error), Buttons::Ok);
        assert_eq!(buttons(Kind::AskYesNo), Buttons::YesNo);
        assert_eq!(buttons(Kind::AskOkCancel), Buttons::OkCancel);
        assert_eq!(buttons(Kind::ConfirmOkCancel), Buttons::OkCancel);
        assert_eq!(buttons(Kind::ConfirmOkCancelOnTop), Buttons::OkCancel);
        assert!(accepted(Answer::Ok) && accepted(Answer::Yes));
        assert!(!accepted(Answer::Cancel) && !accepted(Answer::No));
    }
}
