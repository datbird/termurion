//! The password manager's gate on macOS: LocalAuthentication, as iTerm2 does
//! (`PasswordManagerDataSourceProvider.swift:261-288`). Touch ID where there is one, else the
//! login password. The reply block runs on another thread, so it only sends the answer down a channel.

use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_foundation::{NSError, NSString};
use objc2_local_authentication::{LAContext, LAError, LAPolicy};

use super::super::user_auth::Answer;
use super::super::OS_NAME;

/// How long the prompt may stay open before it is given up.
const WAIT: Duration = Duration::from_secs(300);

fn why(e: &NSError) -> String {
    e.localizedDescription().to_string()
}

/// What an evaluation's reply means: iTerm2 shows no alert for a system or app cancel.
fn answer_for(ok: bool, code: Option<isize>, reason: String) -> Answer {
    if ok {
        Answer::Granted
    } else if matches!(code, Some(c) if c == LAError::SystemCancel.0 || c == LAError::AppCancel.0) {
        Answer::Canceled
    } else {
        Answer::Failed(reason)
    }
}

/// Always answers (never None): macOS always has the system prompt.
pub fn hello(_owner: isize) -> Option<Answer> {
    let answer = evaluate("open the password manager", WAIT);
    // The prompt took the focus; the window about to open must come to the front, not open behind.
    if matches!(answer, Answer::Granted)
        && let Some(mtm) = objc2::MainThreadMarker::new()
    {
        objc2_app_kit::NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }
    Some(answer)
}

fn evaluate(reason: &str, wait: Duration) -> Answer {
    // SAFETY: a plain LAContext, used from this thread only; the reply block is the only thing
    // that runs elsewhere and it touches nothing but the channel.
    unsafe {
        let ctx: Retained<LAContext> = LAContext::new();
        if let Err(e) = ctx.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication) {
            return Answer::Failed(why(&e));
        }
        let (tx, rx) = channel::<Answer>();
        let reply = RcBlock::new(move |ok: Bool, err: *mut NSError| {
            // The system passes a valid NSError, or null on success.
            let (code, text) = match err.as_ref() {
                Some(e) => (Some(e.code()), why(e)),
                None => (None, String::new()),
            };
            let _ = tx.send(answer_for(ok.as_bool(), code, text));
        });
        ctx.evaluatePolicy_localizedReason_reply(LAPolicy::DeviceOwnerAuthentication, &NSString::from_str(reason), &reply);
        match rx.recv_timeout(wait) {
            Ok(a) => a,
            Err(RecvTimeoutError::Timeout) => {
                ctx.invalidate();
                Answer::Failed("the request timed out.".to_string())
            }
            Err(RecvTimeoutError::Disconnected) => Answer::Failed("the request ended without an answer.".to_string()),
        }
    }
}

/// Never reached on macOS: `hello` always answers.
pub fn prompt(_owner: isize) -> Answer {
    Answer::Failed(format!("the password prompt is not available on {OS_NAME} yet."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_map_like_iterm2() {
        assert!(matches!(answer_for(true, None, String::new()), Answer::Granted));
        assert!(matches!(answer_for(false, Some(LAError::SystemCancel.0), "x".into()), Answer::Canceled));
        assert!(matches!(answer_for(false, Some(LAError::AppCancel.0), "x".into()), Answer::Canceled));
        // A user cancel is a failure with its text: that is the alert the brief wants.
        assert!(matches!(answer_for(false, Some(LAError::UserCancel.0), "canceled".into()), Answer::Failed(t) if t == "canceled"));
        assert!(matches!(answer_for(false, None, "no".into()), Answer::Failed(_)));
    }
}
