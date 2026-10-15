//! Announcements iTerm2 queues on a session (`PTYSession.m`): a program that tried to resize the
//! window when the profile forbids it, and the offer to pretty-print a hard-to-read JSON
//! selection that was just copied. Pure: the texts, the stored keys and the JSON rule.

use crate::chrome::ResizePermission;

/// A program tried to resize or move the window and the profile says no (`PS:15864-15937`).
pub const RESIZE_ID: &str = "Resize Window Announcement";
pub const RESIZE_UNFOCUSED_ID: &str = "Resize Unfocused Window Announcement";
pub const RESIZE_TEXT: &str = "A program has tried to resize the window. Allow it?";
pub const RESIZE_UNFOCUSED_TEXT: &str = "A program has tried to resize the window while this session was not active. Allow it?";
/// "Don't Show This Again" is remembered under these.
pub const RESIZE_SUPPRESS_KEY: &str = "NoSyncSuppressPromptToEnableResizing";
pub const RESIZE_UNFOCUSED_SUPPRESS_KEY: &str = "NoSyncSuppressPromptToEnableUnfocusedResizing";
/// Allow Once, Open Settings, Don't Show This Again (a leading underscore marks the shortcut key).
pub const RESIZE_BUTTONS: [&str; 3] = ["_Allow Once", "_Open Settings", "Don\u{2019}t Show This Again"];

/// The announcement for a refused resize: its id, text and stored suppression key; None when allowed.
pub fn resize_announcement(p: ResizePermission) -> Option<(&'static str, &'static str, &'static str)> {
    match p {
        ResizePermission::Allowed => None,
        ResizePermission::Denied => Some((RESIZE_ID, RESIZE_TEXT, RESIZE_SUPPRESS_KEY)),
        ResizePermission::DeniedInactive => Some((RESIZE_UNFOCUSED_ID, RESIZE_UNFOCUSED_TEXT, RESIZE_UNFOCUSED_SUPPRESS_KEY)),
    }
}

/// The JSON offer (`NaggingController.m:895-918`): its id, text and buttons, and the key set once it has been made.
pub const JSON_ID: &str = "JSONPromotion";
pub const JSON_TEXT: &str = "That's a gnarly JSON blob you've got there! Termurion can replace this hard-to-read selection with a pretty-printed value.";
pub const JSON_BUTTONS: [&str; 2] = ["Try it Now", "Dismiss"];
pub const JSON_PROMOTED_KEY: &str = "NoSyncHavePromotedJSONPrettyPrint";
/// What Try it Now says after it works (`PTYSession.m:23943-23952`) and when the selection changed.
pub const JSON_DONE_HEADING: &str = "Done!";
pub const JSON_FEATURE_LOCATION: &str = "You can find this feature under Edit > Replace Selection > Replace with Pretty-Printed JSON if you want to use it again.";
pub const JSON_SELECTION_CHANGED: &str = "Looks like the selection changed and is no longer a valid JSON object.\nYou can find this feature under Edit > Replace Selection > Replace with Pretty-Printed JSON if you want to try again.";

/// `JSONPrettyPrinter.promoteIfJSON` (`JSONPrettyPrinter.swift:20-58`): a copy of 200 to 100,000
/// characters that starts (within 20 characters) with an optional run of spaces and `[` or `{`, is
/// valid JSON, would take ten lines or more pretty-printed, and has no more than a quarter of that many
/// line breaks now.
pub fn worth_promoting(text: &str) -> bool {
    let n = text.chars().count();
    if !(200..=100_000).contains(&n) {
        return false;
    }
    let head: String = text.chars().take(20).collect();
    if !head.trim_start_matches(' ').starts_with(['[', '{']) {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return false };
    let Ok(pretty) = serde_json::to_string_pretty(&value) else { return false };
    let desired = pretty.lines().count();
    let actual = text.matches('\n').count();
    desired >= 10 && actual <= desired / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_resize_names_its_announcement() {
        assert_eq!(resize_announcement(ResizePermission::Allowed), None);
        assert_eq!(resize_announcement(ResizePermission::Denied), Some((RESIZE_ID, RESIZE_TEXT, RESIZE_SUPPRESS_KEY)));
        assert_eq!(resize_announcement(ResizePermission::DeniedInactive).map(|a| a.2), Some("NoSyncSuppressPromptToEnableUnfocusedResizing"));
    }

    #[test]
    fn json_is_promoted_only_when_it_is_hard_to_read() {
        let one_line = format!("[{}]", (0..60).map(|i| format!("{{\"k\":{i}}}")).collect::<Vec<_>>().join(","));
        assert!(worth_promoting(&one_line));
        assert!(!worth_promoting("{\"a\":1}"), "too short");
        assert!(!worth_promoting(&format!("x{one_line}")), "does not start like JSON");
        assert!(!worth_promoting(&format!("[{}", "1,".repeat(200))), "not valid");
        let readable = serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(&one_line).unwrap()).unwrap();
        assert!(!worth_promoting(&readable), "already has its line breaks");
    }
}
