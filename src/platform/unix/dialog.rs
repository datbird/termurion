//! Message boxes on Unix: `rfd::MessageDialog`. A native handle cannot be an owner here, so every
//! box is its own. Without zenity on Linux nothing can be drawn: the text goes to stderr instead.

use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

use super::super::dialog::{Answer, Buttons, Kind, buttons};

/// Whether a program can draw a box. macOS has its own; on Linux rfd shells out to zenity and,
/// when that fails, quietly answers Cancel, so look for it once and say so when it is missing.
fn available() -> bool {
    static FOUND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| {
        if cfg!(target_os = "macos") {
            return true;
        }
        let found = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| crate::localfs::judge(d.join("zenity")).is_some_and(|p| crate::localfs::is_file(&p))));
        if !found {
            eprintln!("Termurion: message boxes are not available on Linux yet (install zenity); their text goes to stderr.");
            crate::dlog!("Message boxes are not available on Linux yet: install zenity");
        }
        found
    })
}

/// Draws the box and waits for the person.
fn show(title: &str, body: &str, kind: Kind) -> Answer {
    let level = match kind {
        Kind::Info | Kind::AskYesNo | Kind::AskOkCancel => MessageLevel::Info,
        Kind::Warning | Kind::ConfirmOkCancel | Kind::ConfirmClose | Kind::ConfirmOkCancelOnTop => MessageLevel::Warning,
        Kind::Error => MessageLevel::Error,
    };
    let row = match buttons(kind) {
        Buttons::Ok => MessageButtons::Ok,
        Buttons::OkCancel => MessageButtons::OkCancel,
        Buttons::YesNo => MessageButtons::YesNo,
    };
    let result = MessageDialog::new().set_title(title).set_description(body).set_level(level).set_buttons(row).show();
    match result {
        MessageDialogResult::Ok => Answer::Ok,
        MessageDialogResult::Yes => Answer::Yes,
        MessageDialogResult::No => Answer::No,
        _ => Answer::Cancel,
    }
}

/// What a question answers when no box can be drawn. A close or quit question says OK so the
/// window can still be closed; the others (an erase, a force detach) take the answer that changes
/// nothing.
fn safe_answer(kind: Kind) -> Answer {
    match kind {
        Kind::ConfirmClose | Kind::ConfirmOkCancelOnTop => Answer::Ok,
        Kind::AskYesNo => Answer::No,
        Kind::AskOkCancel | Kind::ConfirmOkCancel => Answer::Cancel,
        Kind::Info | Kind::Warning | Kind::Error => Answer::Ok,
    }
}

/// The text always goes to stderr and the debug log, drawn or not. An OK-only box is drawn on its
/// own thread so the window keeps running; a question is asked in place because the caller needs
/// the answer, and an Error box too because it is a fatal start message and the process ends right
/// after. With no dialog program a question takes its safe answer.
pub fn message(_owner: Option<isize>, title: &str, body: &str, kind: Kind) -> Answer {
    eprintln!("{title}: {body}");
    crate::dlog!("A message box was asked for; the words went to stderr");
    if !available() {
        if buttons(kind) != Buttons::Ok {
            crate::dlog!("No message box program, so a question took its safe answer");
        }
        return safe_answer(kind);
    }
    if buttons(kind) == Buttons::Ok && kind != Kind::Error {
        let (title, body) = (title.to_string(), body.to_string());
        std::thread::spawn(move || {
            show(&title, &body, kind);
        });
        return Answer::Ok;
    }
    show(title, body, kind)
}

/// There is no disclosure dialog on Unix; the caller asks with a plain box.
pub fn confirm_with_disclosure(_owner: Option<isize>, _topmost: bool, _title: &str, _text: &str, _prompt: &str, _detail: &str) -> Option<bool> {
    None
}

pub fn choose_color(_hwnd: isize, _initial: Option<crate::palette::Rgb>, _swatches: &[crate::palette::Rgb]) -> Result<Option<crate::palette::Rgb>, super::super::Unsupported> {
    Err(super::super::Unsupported { feature: "The color dialog" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_with_no_box_never_traps_a_close() {
        assert_eq!(safe_answer(Kind::ConfirmClose), Answer::Ok);
        assert_eq!(safe_answer(Kind::ConfirmOkCancelOnTop), Answer::Ok);
        assert_eq!(safe_answer(Kind::AskYesNo), Answer::No);
        assert_eq!(safe_answer(Kind::AskOkCancel), Answer::Cancel);
    }

    #[test]
    fn a_destructive_question_with_no_box_changes_nothing() {
        assert_eq!(safe_answer(Kind::ConfirmOkCancel), Answer::Cancel);
    }
}
