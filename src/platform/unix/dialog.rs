//! Message boxes on Unix: `rfd::MessageDialog`. A native handle cannot be an owner here, so every
//! box is its own.

use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

use super::super::dialog::{Answer, Buttons, Kind, buttons};

pub fn message(_owner: Option<isize>, title: &str, body: &str, kind: Kind) -> Answer {
    let level = match kind {
        Kind::Info | Kind::AskYesNo | Kind::AskOkCancel => MessageLevel::Info,
        Kind::Warning | Kind::ConfirmOkCancel | Kind::ConfirmOkCancelOnTop => MessageLevel::Warning,
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

/// There is no disclosure dialog on Unix; the caller asks with a plain box.
pub fn confirm_with_disclosure(_owner: Option<isize>, _topmost: bool, _title: &str, _text: &str, _prompt: &str, _detail: &str) -> Option<bool> {
    None
}
