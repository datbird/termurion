//! What happens when a session ends (milestone 13q). This task holds the keys and the silence
//! keys of the short-lived warning; later tasks add the end action, the outcome and the texts.

/// The profile key of the end action (`ITAddressBookMgr.h:219`): 0 No Action, 1 Close, 2 Restart.
pub const END_ACTION_KEY: &str = "Close Sessions On End";

/// The prefix of the per-profile silence key of the short-lived warning (`iTermPreferences.m:1326`).
pub const SHORT_LIVED_PREFIX: &str = "NeverWarnAboutShortLivedSessions_";

/// The silence key for the profile whose saved Guid is `saved_guid`.
pub fn short_lived_key(saved_guid: &str) -> String {
    format!("{SHORT_LIVED_PREFIX}{saved_guid}")
}

/// The key that stores the answer of the silenced warning (`warnings::selection_key`).
pub fn short_lived_selection_key(saved_guid: &str) -> String {
    crate::warnings::selection_key(&short_lived_key(saved_guid))
}
