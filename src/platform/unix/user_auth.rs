//! No Hello and no credential prompt on Unix yet: the check fails with a reason the alert shows.

use super::super::user_auth::Answer;
use super::super::OS_NAME;

/// There is no Hello here; the caller falls to the prompt.
pub fn hello(_owner: isize) -> Option<Answer> {
    None
}

pub fn prompt(_owner: isize) -> Answer {
    Answer::Failed(format!("the password prompt is not available on {OS_NAME} yet."))
}
