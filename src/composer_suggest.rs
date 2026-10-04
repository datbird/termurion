//! The Composer's suggestion model (milestone 9l Task 5): a plain-Rust port of the pieces of
//! iTerm2's large-composer autocomplete that decide *what* to suggest and *how* to show it,
//! kept OS-free so it runs on Linux; the actual PATH and file-system listing is gathered by
//! the caller (an injected lister on a worker thread, milestone 9l Task 6) and handed in as
//! plain `(name, is_dir)` pairs.
//!
//! iTerm2 sources (`~/gitrepos/iterm2-ref/sources`): `LCVC`
//! `StatusBar/Components/iTermStatusBarLargeComposerViewController.m` (`textDidChange:`
//! deciding whether to fetch at all, `historySuggestionsForPrefix:maxResults:removePrefix:`,
//! `fetchCompletionsForCommand:...` choosing PATH executables vs. the working directory,
//! `setEarlyResult:history:prefix:` and `didFindCompletions:...` picking the ghost text),
//! `CTV` `Composer/ComposerTextView.swift` (`substringUpToFirstSpace` truncating the ghost
//! when the caret is not at the end, `withoutDuplicates(by:)` deduplicating completions), `CW`
//! `CompletionsUI/CompletionsWindow.swift` (`singleLineForDisplay` folding an embedded
//! newline to the return glyph, `attributedString(font:prefix:suffix:)` bolding the typed
//! prefix in front of a completion's remainder).
//!
//! Reused rather than duplicated: `shell_history::{Store::commands, remainder, insertable_dir,
//! POWERS}` and `paste_history::ago` (project invariant; both are already iTerm2 ports in
//! their own right).

use std::ops::Range;

use crate::paste_history;
use crate::shell_history::{self, Store};

/// One suggestion's source (`iTermCompletionItemKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    History,
    File,
    Folder,
    Command,
    /// A model's completion of the typed text (`iTermCompletionItemKindAiSuggestion`).
    AiSuggestion,
    /// A model's full command that replaces the whole line (`.aiReplacement`); its item has
    /// `replacement: true`.
    AiReplacement,
}

/// One row the popup can show, or the source of ghost text.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// What the row appends after the typed text (`LCVC:733`, `733-739`: a completion's
    /// value is the remainder after the prefix the user already typed).
    pub value: String,
    /// The popup's detail column; falls back to `value` when there is nothing more specific
    /// (`CW:174`, `item.detail ?? item.value`).
    pub detail: String,
    pub kind: ItemKind,
    /// A full-line replacement (iTerm2's `.aiReplacement`): shown on its own in the popup,
    /// with no typed prefix in front of it and never folded into the ghost text
    /// (`CTV:169-172`, `LCVC:481-483`). Milestone 9l has no AI suggestions of its own, but the
    /// field is kept so a later milestone's replacement rows fit this same model.
    pub replacement: bool,
}

/// Most history items a lookup returns (`LCVC:645`, `maxResults:32`).
pub const HISTORY_MAX: usize = 32;
/// The file-completion limit while typing (`LCVC:634`, `explicit ? 256 : 64`).
pub const FILES_TYPING: usize = 64;
/// The file-completion limit for an explicit request (Tab).
pub const FILES_TAB: usize = 256;

/// The history item's detail format (`LCVC:487`, `LargeComposer.LastUsedFormat`); `%@` is
/// `paste_history::ago`'s relative time.
pub const LAST_USED: &str = "Last used %@";
/// The popup's placeholder row while a fetch is outstanding.
pub const THINKING: &str = "Thinking\u{2026}";

/// Whether a suggestion fetch should run at all (`LCVC:511-527`): never for an empty line
/// before the cursor, and never while the caret sits in the middle of a word (the line before
/// the cursor does not end in whitespace, and there is text after the cursor that does not
/// begin with whitespace either).
pub fn should_fetch(line_before: &str, after: &str) -> bool {
    if line_before.is_empty() {
        return false;
    }
    let ends_with_whitespace = line_before.chars().next_back().is_some_and(char::is_whitespace);
    let begins_with_whitespace = after.chars().next().is_some_and(char::is_whitespace);
    !(!ends_with_whitespace && !after.is_empty() && !begins_with_whitespace)
}

/// The text from the start of the caret's line to the caret (`LCVC:211-226`,
/// `lineBeforeCursor`): back to the character after the nearest newline before it, or the
/// start of `text` when there is none.
pub fn line_before_cursor(text: &str, caret: usize) -> &str {
    let mut caret = caret.min(text.len());
    while caret > 0 && !text.is_char_boundary(caret) {
        caret -= 1;
    }
    let start = text[..caret].rfind('\n').map(|i| i + 1).unwrap_or(0);
    &text[start..caret]
}

/// Whether `line`'s last word is the first word of the command, and that word's text so far
/// (`LCVC:634-636`): appending a placeholder character before splitting on whitespace means a
/// trailing space already starts a new (empty) word.
pub fn word_to_complete(line: &str) -> (bool, &str) {
    let with_placeholder = format!("{line}X");
    let on_first_word = with_placeholder.split_whitespace().count() < 2;
    let prefix: &str = if line.chars().next_back().is_some_and(char::is_whitespace) {
        ""
    } else if let Some((i, c)) = line.char_indices().rev().find(|(_, c)| c.is_whitespace()) {
        &line[i + c.len_utf8()..]
    } else {
        line
    };
    (on_first_word, prefix)
}

/// The host's command-history completions for `prefix`, newest/most-frecent first, at most
/// `max` (`LCVC:477-499`, `645-647`: `removePrefix:YES`, `maxResults:32`).
pub fn history(store: &Store, host: &str, prefix: &str, max: usize, now: f64) -> Vec<Item> {
    store
        .commands(host, prefix, now, shell_history::POWERS)
        .into_iter()
        .take(max)
        .map(|(command, time)| Item {
            value: shell_history::remainder(&command, prefix),
            detail: LAST_USED.replace("%@", &paste_history::ago(time, now)),
            kind: ItemKind::History,
            replacement: false,
        })
        .collect()
}

/// Quotes `text` for insertion into `shell`'s command line, reusing
/// `shell_history::insertable_dir`'s exact character sets and quoting per shell instead of
/// duplicating them. `insertable_dir` only takes the Windows-quoting branch when
/// `profile::local_path` recognizes its argument as a drive-letter path, so a bare file or
/// directory name (which never looks like one) is given a throwaway drive prefix to reach
/// that branch for `pwsh`/`cmd`, then the prefix is peeled back off (from just inside any
/// opening quote mark `insertable_dir` added). A POSIX shell needs no such trick: a bare name
/// already takes `insertable_dir`'s backslash-escaping branch correctly.
fn quote_name(text: &str, shell: Option<&str>) -> String {
    let lower = shell.unwrap_or("").to_ascii_lowercase();
    if !matches!(lower.as_str(), "pwsh" | "powershell" | "cmd") {
        return shell_history::insertable_dir(text, shell);
    }
    const PLACEHOLDER: &str = "Z:\\";
    let quoted = shell_history::insertable_dir(&format!("{PLACEHOLDER}{text}"), shell);
    if let Some(rest) = quoted.strip_prefix(PLACEHOLDER) {
        return rest.to_string();
    }
    let mut chars = quoted.chars();
    let open = chars.next().unwrap_or('\'');
    let rest = chars.as_str();
    let inner = rest.strip_prefix(PLACEHOLDER).unwrap_or(rest);
    format!("{open}{inner}")
}

/// File and directory completions from an already-gathered listing (`LCVC:645-704`): entries
/// whose name starts with `prefix` (case-insensitively), sorted, capped at `limit`, each
/// with the prefix removed and quoted for `shell` as `shell_history::insertable_dir` quotes a
/// path. No prefix means no completions (there is nothing yet to complete).
///
/// Decision: quoting is applied to the remainder (the text appended after what is already
/// typed), not to the whole entry name, so the model keeps appending rather than replacing
/// the word under the caret. Cost if wrong: a Windows shell (`pwsh`/`cmd`) whose quoted
/// remainder does not start where the typed prefix left off (rare: only when the character
/// that needs quoting sits inside the already-typed prefix) would insert text that needs the
/// whole word requoted by hand; a later task can special-case that by replacing the current
/// word instead of appending when this happens.
pub fn files(entries: &[(String, bool)], prefix: &str, limit: usize, shell: Option<&str>) -> Vec<Item> {
    if prefix.is_empty() {
        return Vec::new();
    }
    let lower_prefix = prefix.to_lowercase();
    let mut matched: Vec<&(String, bool)> = entries.iter().filter(|(name, _)| name.to_lowercase().starts_with(&lower_prefix)).collect();
    matched.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then_with(|| a.0.cmp(&b.0)));
    matched
        .into_iter()
        .take(limit)
        .map(|(name, is_dir)| {
            let remainder = shell_history::remainder(name, prefix);
            Item { value: quote_name(&remainder, shell), detail: name.clone(), kind: if *is_dir { ItemKind::Folder } else { ItemKind::File }, replacement: false }
        })
        .collect()
}

pub fn longest_common_prefix(values: &[&str]) -> String {
    let Some(first) = values.first() else { return String::new() };
    let mut len = first.len();
    for v in &values[1..] {
        let common = first.as_bytes().iter().zip(v.as_bytes()).take_while(|(a, b)| a == b).count();
        len = len.min(common);
    }
    while len > 0 && !first.is_char_boundary(len) {
        len -= 1;
    }
    first[..len].to_string()
}

/// The ghost text for a set of items (`LCVC:786-796`, `didFindCompletions:`): the longest
/// common prefix of every non-replacement item's value. Without `suggestion_only` (the local
/// fetch passes `!byUserRequest`, `PS:24528`, so only Tab clears it) an empty common prefix
/// falls back to the first inline item's value, and then the first filename item (any
/// non-history item, `inlineFilenameCompletions[0]`) overrides whatever was chosen. An empty
/// result is no ghost.
pub fn ghost(items: &[Item], suggestion_only: bool) -> Option<String> {
    let inline: Vec<&Item> = items.iter().filter(|i| !i.replacement).collect();
    let values: Vec<&str> = inline.iter().map(|i| i.value.as_str()).collect();
    let mut suggestion = longest_common_prefix(&values);
    if !suggestion_only {
        if suggestion.is_empty() {
            suggestion = inline.first().map(|i| i.value.clone()).unwrap_or_default();
        }
        if let Some(file) = inline.iter().find(|i| i.kind != ItemKind::History) {
            suggestion = file.value.clone();
        }
    }
    (!suggestion.is_empty()).then_some(suggestion)
}

/// The ghost text actually shown (`CTV:76`, `1199-1211`): the whole thing when the caret sits
/// at the end of the text, otherwise only up to (not including) its first space.
pub fn shown_ghost(ghost: &str, caret_at_end: bool) -> &str {
    if caret_at_end {
        return ghost;
    }
    match ghost.find(' ') {
        Some(i) => &ghost[..i],
        None => ghost,
    }
}

/// Drops items whose value, trimmed, repeats an earlier one, keeping the first occurrence
/// (`CTV:124-127`, `Array.withoutDuplicates(by:)`).
pub fn dedupe(items: Vec<Item>) -> Vec<Item> {
    let mut seen = std::collections::HashSet::new();
    items.into_iter().filter(|item| seen.insert(item.value.trim().to_string())).collect()
}

fn single_line(s: &str) -> String {
    s.replace("\r\n", "\u{21a9}").replace('\n', "\u{21a9}").replace('\r', "\u{21a9}")
}

/// A popup row's display text and the byte range of its bold typed-prefix part
/// (`CW:31-58`): the typed prefix (bold, empty for a full-line replacement) followed by the
/// item's value, an embedded newline in either part folded to the return glyph so the row
/// never spills onto the next line.
pub fn row_text(item: &Item, typed: &str) -> (String, Option<Range<usize>>) {
    let row_prefix = if item.replacement { "" } else { typed };
    let prefix_disp = single_line(row_prefix);
    let suffix_disp = single_line(&item.value);
    let bold = if prefix_disp.is_empty() { None } else { Some(0..prefix_disp.len()) };
    (format!("{prefix_disp}{suffix_disp}"), bold)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_follow_iterm2() {
        // should_fetch: never on an empty line before the cursor, never mid-word.
        assert!(should_fetch("git ", ""));
        assert!(should_fetch("gi", ""));
        assert!(!should_fetch("gi", "t x"), "typing inside the word \"git\"");
        assert!(should_fetch("gi", " x"), "a space already separates the word");
        assert!(!should_fetch("", ""));

        // line_before_cursor: back to the last newline, or the start of the text.
        assert_eq!(line_before_cursor("git status\ngit ", 15), "git ");
        assert_eq!(line_before_cursor("git ", 4), "git ");
        assert_eq!(line_before_cursor("git ", 2), "gi");

        // word_to_complete.
        assert_eq!(word_to_complete("git sta"), (false, "sta"));
        assert_eq!(word_to_complete("gi"), (true, "gi"));
        assert_eq!(word_to_complete("git "), (false, ""), "a trailing space starts a new word");

        // history: at most `max`, newest/most-frecent first, the prefix removed, the detail
        // formatted from paste_history::ago.
        let mut store = Store::default();
        let now = 100.0 * 86_400.0;
        store.record_command("h", "git status", None, now - 2.0 * 86_400.0);
        store.record_command("h", "git commit", None, now - 1.0);
        for i in 0..40 {
            store.record_command("h", &format!("git x{i}"), None, now - 3.0 * 86_400.0 - i as f64);
        }
        let items = history(&store, "h", "git", HISTORY_MAX, now);
        assert_eq!(items.len(), HISTORY_MAX, "capped at HISTORY_MAX even though 42 commands match");
        let status = items.iter().find(|i| i.value == " status").expect("git status is in the list");
        assert_eq!(status.detail, "Last used 2 days ago");
        assert_eq!(status.kind, ItemKind::History);
        assert!(!status.replacement);
        assert!(history(&store, "h", "nope", HISTORY_MAX, now).is_empty());

        // files: case-insensitive prefix match, prefix removed, sorted, none for an empty
        // prefix, capped at the limit, quoted per shell as insertable_dir quotes a path.
        let entries = vec![("readme.md".to_string(), false), ("README2.md".to_string(), false), ("recipes".to_string(), true), ("other.txt".to_string(), false)];
        assert!(files(&entries, "", 64, None).is_empty());
        let found = files(&entries, "RE", 64, None);
        assert_eq!(found.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(), vec!["adme.md", "ADME2.md", "cipes"], "case-insensitive match, sorted, prefix removed (case kept)");
        assert_eq!(found[2].kind, ItemKind::Folder);
        assert_eq!(found[0].kind, ItemKind::File);
        assert_eq!(files(&entries, "re", 2, None).len(), 2, "capped at the limit");
        let posix_space = files(&[("a nice file".to_string(), false)], "a", 64, Some("bash"));
        assert_eq!(posix_space[0].value, shell_history::insertable_dir(" nice file", Some("bash")), "POSIX: backslash-escaped like insertable_dir");
        let pwsh_space = files(&[("a nice file".to_string(), false)], "a", 64, Some("pwsh"));
        assert_eq!(pwsh_space[0].value, "' nice file'", "PowerShell: single-quoted like insertable_dir quotes a path with a space");
        let cmd_space = files(&[("a nice file".to_string(), false)], "a", 64, Some("cmd"));
        assert_eq!(cmd_space[0].value, "\" nice file\"", "cmd: double-quoted like insertable_dir quotes a path with a space");

        // ghost (`LCVC:786-796`, `didFindCompletions`): with `suggestion_only` (typing, the
        // local fetch's `!byUserRequest`) only the longest common prefix of the
        // non-replacement values; without it (Tab) an empty common prefix falls back to the
        // first inline item's value, and the first filename item wins outright; a
        // replacement item is never folded in.
        let common = vec![
            Item { value: "status".into(), detail: String::new(), kind: ItemKind::History, replacement: false },
            Item { value: "stash".into(), detail: String::new(), kind: ItemKind::History, replacement: false },
        ];
        assert_eq!(ghost(&common, true).as_deref(), Some("sta"));
        assert_eq!(ghost(&common, false).as_deref(), Some("sta"));
        let no_common = vec![
            Item { value: "abc".into(), detail: String::new(), kind: ItemKind::History, replacement: false },
            Item { value: "xyz".into(), detail: String::new(), kind: ItemKind::History, replacement: false },
        ];
        assert_eq!(ghost(&no_common, true), None, "typing: no common prefix, no ghost");
        assert_eq!(ghost(&no_common, false).as_deref(), Some("abc"), "Tab: the first inline value");
        let with_file = vec![
            Item { value: "adme2.md".into(), detail: String::new(), kind: ItemKind::File, replacement: false },
            Item { value: "adme.md".into(), detail: String::new(), kind: ItemKind::History, replacement: false },
        ];
        assert_eq!(ghost(&with_file, true).as_deref(), Some("adme"), "suggestion only: the common prefix still wins");
        assert_eq!(ghost(&with_file, false).as_deref(), Some("adme2.md"), "Tab: the first filename item outright");
        let ignores_replacement = vec![
            Item { value: "totally different command".into(), detail: String::new(), kind: ItemKind::Command, replacement: true },
            Item { value: "status".into(), detail: String::new(), kind: ItemKind::History, replacement: false },
        ];
        assert_eq!(ghost(&ignores_replacement, true).as_deref(), Some("status"));
        assert_eq!(ghost(&ignores_replacement, false).as_deref(), Some("status"));
        assert_eq!(ghost(&[], false), None);

        // shown_ghost: truncated to the first space unless the caret sits at the end.
        assert_eq!(shown_ghost("abc def", false), "abc");
        assert_eq!(shown_ghost("abc def", true), "abc def");
        assert_eq!(shown_ghost("abc", false), "abc", "no space to truncate at");

        // dedupe: trimmed values, first occurrence kept, order preserved.
        let dupes = vec![
            Item { value: "one".into(), detail: "a".into(), kind: ItemKind::File, replacement: false },
            Item { value: " one ".into(), detail: "b".into(), kind: ItemKind::File, replacement: false },
            Item { value: "two".into(), detail: "c".into(), kind: ItemKind::File, replacement: false },
        ];
        let deduped = dedupe(dupes);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].detail, "a", "the first occurrence is kept");
        assert_eq!(deduped[1].value, "two");

        // row_text: the typed prefix bolded, a newline in either part shown as U+21A9.
        let item = Item { value: "adme.md\nmore".into(), detail: String::new(), kind: ItemKind::File, replacement: false };
        let (text, bold) = row_text(&item, "re\n");
        assert_eq!(text, "re\u{21a9}adme.md\u{21a9}more");
        assert_eq!(bold, Some(0.."re\u{21a9}".len()));
        let replacement_item = Item { value: "git status".into(), detail: String::new(), kind: ItemKind::Command, replacement: true };
        let (text, bold) = row_text(&replacement_item, "re");
        assert_eq!(text, "git status", "a replacement shows on its own, with no typed prefix in front");
        assert_eq!(bold, None);
    }
}
