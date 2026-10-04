//! Composer AI suggestions (milestone 12e Task 5): the pure half of iTerm2's `AICompletion`
//! (`sources/AITerm/AICompletion.swift`): the prompt, the line parser that reconciles the
//! model's mode with what was typed, the `{{FILE}}` expansion, and the merge with the file and
//! history items of milestone 9l (`LargeComposerViewController.m` `setEarlyResult:` and
//! `didFindCompletions:`). Nothing here sends a request; `app::composer_ui` does, through
//! `App::ai_one_shot`, which asks the gate right before sending.

use crate::composer_suggest::{self as suggest, Item, ItemKind};
use crate::shell_history::Use;

/// The placeholder the model uses for a file name (`AC:7`).
pub const FILE_PLACEHOLDER: &str = "{{FILE}}";
/// The most suggestions the prompt asks for (`AC:195`).
pub const MAX_SUGGESTIONS: usize = 4;
/// How many previous commands the prompt carries (`PSS:1111-1154`).
pub const HISTORY_COUNT: usize = 10;

/// What a request knows about the line (`SuggestionRequest`, `SR:49-60`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    /// Everything before the caret.
    pub full_prefix: String,
    /// Everything after the caret.
    pub full_suffix: String,
    pub working_directory: Option<String>,
    /// The caret is in the first word of the command.
    pub executable: bool,
}

/// The prompt and the sanitized prefix the parser compares with (`AC:179-226`): the full prefix
/// with newlines as spaces; the word before the caret is not used.
pub fn prompt(inputs: &Inputs, history: &[String], files: &[String]) -> (String, String) {
    let mut lines: Vec<String> = vec![
        "You are offering command line completion suggestions and descriptions.".into(),
        "The user is editing a shell command line. For the text they have entered so far, decide whether it is the beginning of a shell command that should be completed, or a natural-language description of what they want to do that should be turned into a command.".into(),
        "Respond with one suggestion per line.".into(),
        "Each line must consist of a mode, a tab, a 2-5 word description, a tab, and finally the suggestion.".into(),
        "The mode is the word `append` when the suggestion finishes the command the user has begun typing, or `replace` when the suggestion is a complete command that should take the place of what the user typed.".into(),
        format!("Return no more than {MAX_SUGGESTIONS} lines. You may return fewer or none rather than poor quality suggestions."),
        "For example, if the text so far was `git re` then you might respond with `append\tRewrite commit history\tgit rebase` and `append\tRevert a commit\tgit revert`.".into(),
        "Use `append` when the text so far looks like the start of a valid command. For an `append` suggestion, the text after the final tab must begin with the literal text the user has entered so far, because only the portion beyond what they typed will be appended.".into(),
        "Use `replace` when the text reads as an English request rather than a command. For example, if the text so far was `delete every .log file in this folder` you might respond with `replace\tRemove log files\trm ./*.log`. A `replace` suggestion does not need to begin with what the user typed; it will replace the entire line.".into(),
        "You can suggest more than one additional word when you can tell from context that it would be a good suggestion. For example, if the user frequently runs `grep ERROR <file> | sort | uniq` and you see that the text so far is `grep ERROR myfile.txt` your response could include `append\tCount unique errors\tgrep ERROR myfile.txt | sort | uniq`".into(),
    ];
    if let Some(cwd) = &inputs.working_directory {
        lines.push(format!("The current working directory is {cwd}"));
    }
    if inputs.executable {
        lines.push("The user appears to be entering the name of a command to execute, not one of its arguments, so completions should be for executable commands likely to be present on the system.".into());
    }
    lines.push("Each suggestion is either appended after what has been typed (mode `append`) or replaces the whole line (mode `replace`), according to its mode.".into());
    if !history.is_empty() {
        lines.push("Previously run commands in chronological order are:".into());
        lines.extend(history.iter().map(|c| c.replace('\n', " ")));
        lines.push("(end of command list)".into());
        lines.push("Your suggestions should take recently run commands into consideration: if possible, suggest a command that builds on what the user has begun.".into());
        lines.push(String::new());
    }
    lines.push("Do not make up or invent file or directory names.".into());
    lines.push(format!("If a suggestion would end with a filename, use {FILE_PLACEHOLDER} instead."));
    lines.push("Do not invent other kinds of placeholders such as {{MESSAGE}} or {{DIRECTORY}}. Use plain English, like \"message\", instead.".into());
    let values: Vec<&str> = files.iter().map(String::as_str).collect();
    let lcp = suggest::longest_common_prefix(&values);
    if !lcp.is_empty() {
        lines.push(format!("Instead, your suggestion may end with the longest common prefix of valid filename completions, which is: {lcp}"));
    }
    let sanitized = inputs.full_prefix.replace('\n', " ");
    lines.push("The text the user has entered so far is:".into());
    lines.push(sanitized.clone());
    if !inputs.full_suffix.trim_start().is_empty() {
        lines.push(String::new());
        lines.push("Additionally, there is some text after the cursor. Use this to come up with better suggestions, assuming it will be used after what you suggest or that your suggestion will replace some of it. It is:".into());
        lines.push(inputs.full_suffix.replace('\n', " "));
    }
    (lines.join("\n"), sanitized)
}

/// The longest suffix of `prefix` that `suggestion` starts with (`removingPrefixThatIsLongestSuffix`).
fn overlap<'a>(suggestion: &'a str, prefix: &str) -> &'a str {
    let mut starts: Vec<usize> = prefix.char_indices().map(|(i, _)| i).collect();
    starts.push(prefix.len());
    for start in starts {
        let suffix = &prefix[start..];
        if let Some(rest) = suggestion.strip_prefix(suffix) {
            return rest;
        }
    }
    suggestion
}

/// One line of the model's answer, `mode<TAB>description<TAB>suggestion` (`AC:118-176`). The
/// declared mode only breaks the tie where the suggestion overlaps a partly typed word.
pub fn item_from_line(line: &str, prefix: &str) -> Option<Item> {
    let parts: Vec<&str> = line.split('\t').collect();
    let (mode, description, suggestion) = match parts.as_slice() {
        [m, d, s] => (m.trim_matches([' ', '\t']).to_lowercase(), *d, *s),
        [d, s] => (String::new(), *d, *s),
        _ => return None,
    };
    let make = |value: &str, kind: ItemKind| Item { value: value.to_string(), detail: description.to_string(), kind, replacement: kind == ItemKind::AiReplacement };
    if let Some(rest) = suggestion.strip_prefix(prefix) {
        // Restating the prefix adds nothing.
        return (!rest.is_empty()).then(|| make(rest, ItemKind::AiSuggestion));
    }
    let truncated = overlap(suggestion, prefix);
    let overlap_len = suggestion.chars().count() - truncated.chars().count();
    let trailing = prefix.split(char::is_whitespace).next_back().unwrap_or("");
    if overlap_len >= 2 && overlap_len == trailing.chars().count() && mode != "replace" {
        return Some(make(truncated, ItemKind::AiSuggestion));
    }
    if suggestion.trim_matches([' ', '\t']).is_empty() {
        return None;
    }
    Some(make(suggestion, ItemKind::AiReplacement))
}

/// Every item of the model's answer, with `{{FILE}}` expanded over `files` and the files
/// appended when nothing expanded (`AC:49-95`).
pub fn items_from_response(content: &str, prefix: &str, files: &[Item]) -> Vec<Item> {
    items_from_response_with(content, prefix, files, files)
}

/// `items_from_response` with the files to expand (the unescaped values the prompt carried) apart
/// from the files to append (the escaped items the popup shows).
pub fn items_from_response_with(content: &str, prefix: &str, expand: &[Item], append: &[Item]) -> Vec<Item> {
    let other = regex::Regex::new(r"\{\{([A-Z0-9_]+)\}\}").expect("a fixed pattern");
    let mut expanded = false;
    let mut items: Vec<Item> = Vec::new();
    for found in content.split('\n').filter_map(|l| item_from_line(l, prefix)) {
        if found.value.contains(FILE_PLACEHOLDER) {
            expanded = true;
            let mut sorted: Vec<&Item> = expand.iter().collect();
            sorted.sort_by(|a, b| a.value.cmp(&b.value));
            // The file goes into the surrounding text, so `rm {{FILE}}` stays a command.
            items.extend(sorted.into_iter().map(|f| Item { value: found.value.replace(FILE_PLACEHOLDER, &f.value), ..found.clone() }));
        } else if found.value.contains("{{") {
            let value = other.replace_all(&found.value, "($1)").into_owned();
            items.push(Item { value, ..found });
        } else {
            items.push(found);
        }
    }
    if !append.is_empty() && !expanded {
        items.extend(append.iter().cloned());
    }
    let mut seen = std::collections::HashSet::new();
    items.retain(|i| seen.insert(i.value.clone()));
    items
}

/// The last `HISTORY_COUNT` commands, oldest first (`PSS:1111-1154`); `uses` is oldest first.
pub fn history_for_prompt(uses: &[(String, Use)]) -> Vec<String> {
    let skip = uses.len().saturating_sub(HISTORY_COUNT);
    uses[skip..].iter().map(|(c, _)| c.clone()).collect()
}

/// The item shown before the model answers (`LC:715-739` `setEarlyResult:`): the longest common
/// prefix of the files and the history as a File item, else the last history item.
pub fn early_item(files: &[Item], history: &[Item], typed: &str) -> Option<Item> {
    let values: Vec<&str> = files.iter().chain(history).map(|i| i.value.as_str()).collect();
    if values.is_empty() {
        return None;
    }
    let lcp = suggest::longest_common_prefix(&values);
    if !lcp.is_empty() {
        return Some(Item { detail: format!("{typed}{lcp}"), value: lcp, kind: ItemKind::File, replacement: false });
    }
    history.last().cloned()
}

/// `[first] + items without first's value`, or nothing when the model gave nothing (`PSS:1111-1154`).
pub fn with_first(first: Option<Item>, items: Vec<Item>) -> Vec<Item> {
    if items.is_empty() {
        return Vec::new();
    }
    match first {
        Some(f) => {
            let mut all: Vec<Item> = items.into_iter().filter(|i| i.value != f.value).collect();
            all.insert(0, f);
            all
        }
        None => items,
    }
}

/// The popup's rows (AI items, then history) and the ghost text (`LC:790-803`, suggestion-only
/// false): never from a replacement. Values are never re-quoted.
pub fn shown(ai_items: &[Item], history: &[Item]) -> (Vec<Item>, Option<String>) {
    let rows: Vec<Item> = ai_items.iter().chain(history).cloned().collect();
    let ghost = suggest::ghost(&rows, false);
    (rows, ghost)
}

/// The accessory's "all text sent to AI" label (`LCX:98`, `LC:127-131`).
pub fn warning_shows(ai_completions: bool, generative_ai_allowed: bool) -> bool {
    ai_completions && generative_ai_allowed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::composer::{send_tip_shows, AI_WARNING};

    fn item(value: &str, kind: ItemKind) -> Item {
        Item { value: value.into(), detail: String::new(), kind, replacement: kind == ItemKind::AiReplacement }
    }
    fn file(v: &str) -> Item {
        item(v, ItemKind::File)
    }
    fn hist(v: &str) -> Item {
        item(v, ItemKind::History)
    }
    fn sugg(v: &str) -> Item {
        item(v, ItemKind::AiSuggestion)
    }
    fn repl(v: &str) -> Item {
        item(v, ItemKind::AiReplacement)
    }

    /// The fixed lines of `AC:183-224`, copied from the source (`\t` are real tabs).
    const HEAD: [&str; 10] = [
        "You are offering command line completion suggestions and descriptions.",
        "The user is editing a shell command line. For the text they have entered so far, decide whether it is the beginning of a shell command that should be completed, or a natural-language description of what they want to do that should be turned into a command.",
        "Respond with one suggestion per line.",
        "Each line must consist of a mode, a tab, a 2-5 word description, a tab, and finally the suggestion.",
        "The mode is the word `append` when the suggestion finishes the command the user has begun typing, or `replace` when the suggestion is a complete command that should take the place of what the user typed.",
        "Return no more than 4 lines. You may return fewer or none rather than poor quality suggestions.",
        "For example, if the text so far was `git re` then you might respond with `append\tRewrite commit history\tgit rebase` and `append\tRevert a commit\tgit revert`.",
        "Use `append` when the text so far looks like the start of a valid command. For an `append` suggestion, the text after the final tab must begin with the literal text the user has entered so far, because only the portion beyond what they typed will be appended.",
        "Use `replace` when the text reads as an English request rather than a command. For example, if the text so far was `delete every .log file in this folder` you might respond with `replace\tRemove log files\trm ./*.log`. A `replace` suggestion does not need to begin with what the user typed; it will replace the entire line.",
        "You can suggest more than one additional word when you can tell from context that it would be a good suggestion. For example, if the user frequently runs `grep ERROR <file> | sort | uniq` and you see that the text so far is `grep ERROR myfile.txt` your response could include `append\tCount unique errors\tgrep ERROR myfile.txt | sort | uniq`",
    ];
    const MODE_LINE: &str = "Each suggestion is either appended after what has been typed (mode `append`) or replaces the whole line (mode `replace`), according to its mode.";
    const EXE_LINE: &str = "The user appears to be entering the name of a command to execute, not one of its arguments, so completions should be for executable commands likely to be present on the system.";
    const NO_INVENT: [&str; 3] = [
        "Do not make up or invent file or directory names.",
        "If a suggestion would end with a filename, use {{FILE}} instead.",
        "Do not invent other kinds of placeholders such as {{MESSAGE}} or {{DIRECTORY}}. Use plain English, like \"message\", instead.",
    ];
    const ENTERED: &str = "The text the user has entered so far is:";

    fn inputs(prefix: &str, suffix: &str, cwd: Option<&str>, executable: bool) -> Inputs {
        Inputs { full_prefix: prefix.into(), full_suffix: suffix.into(), working_directory: cwd.map(String::from), executable }
    }

    #[test]
    fn ai_prompt_follows_iterm2() {
        // The base case: cwd, no executable, history, common prefix or suffix.
        let (p, prefix) = prompt(&inputs("git re", "", Some("C:\\src"), false), &[], &[]);
        let mut want: Vec<String> = HEAD.iter().map(|s| s.to_string()).collect();
        want.push("The current working directory is C:\\src".into());
        want.push(MODE_LINE.into());
        want.extend(NO_INVENT.iter().map(|s| s.to_string()));
        want.push(ENTERED.into());
        want.push("git re".into());
        assert_eq!(p, want.join("\n"));
        assert_eq!(prefix, "git re");
        assert!(p.contains(&format!("Return no more than {MAX_SUGGESTIONS} lines.")));

        // No cwd, no line.
        let (p, _) = prompt(&inputs("x", "", None, false), &[], &[]);
        assert!(!p.contains("The current working directory"));

        // The executable line follows the cwd line.
        let (p, _) = prompt(&inputs("gi", "", Some("C:\\src"), true), &[], &[]);
        assert!(p.contains(&format!("The current working directory is C:\\src\n{EXE_LINE}\n{MODE_LINE}\n")));

        // History: the block, a blank line, then the do-not-invent lines.
        let hist = ["cargo build".to_string(), "git status".to_string()];
        let (p, _) = prompt(&inputs("git re", "", None, false), &hist, &[]);
        let block = "Previously run commands in chronological order are:\ncargo build\ngit status\n(end of command list)\nYour suggestions should take recently run commands into consideration: if possible, suggest a command that builds on what the user has begun.\n\nDo not make up or invent file or directory names.";
        assert!(p.contains(&format!("{MODE_LINE}\n{block}")), "{p}");
        // A newline inside a history command becomes a space.
        let (p, _) = prompt(&inputs("a", "", None, false), &["a\nb".to_string()], &[]);
        assert!(p.contains("\na b\n(end of command list)"));

        // The common prefix of the files.
        let files = ["readme.md".to_string(), "readme.txt".to_string()];
        let (p, _) = prompt(&inputs("cat re", "", None, false), &[], &files);
        assert!(p.contains(&format!("{}\nInstead, your suggestion may end with the longest common prefix of valid filename completions, which is: readme.\n{ENTERED}", NO_INVENT[2])));
        // No common prefix: no line.
        let (p, _) = prompt(&inputs("cat re", "", None, false), &[], &["a".to_string(), "b".to_string()]);
        assert!(!p.contains("Instead, your suggestion"));

        // A newline in the prefix is a space, in the prompt and in the returned prefix.
        let (p, prefix) = prompt(&inputs("a\nb", "", None, false), &[], &[]);
        assert_eq!(prefix, "a b");
        assert!(p.ends_with(&format!("{ENTERED}\na b")));

        // A suffix adds the block; whitespace only adds nothing.
        let (p, _) = prompt(&inputs("git ", " --all", None, false), &[], &[]);
        assert!(p.ends_with(&format!("{ENTERED}\ngit \n\nAdditionally, there is some text after the cursor. Use this to come up with better suggestions, assuming it will be used after what you suggest or that your suggestion will replace some of it. It is:\n --all")));
        let (p, _) = prompt(&inputs("git ", " \n\t", None, false), &[], &[]);
        assert!(p.ends_with(&format!("{ENTERED}\ngit ")));
    }

    #[test]
    fn ai_lines_follow_iterm2() {
        let one = |line: &str, prefix: &str| item_from_line(line, prefix);
        let i = one("append\tRewrite history\tgit rebase", "git re").unwrap();
        assert_eq!((i.value.as_str(), i.detail.as_str(), i.kind, i.replacement), ("base", "Rewrite history", ItemKind::AiSuggestion, false));
        // A full-prefix match wins over replace.
        let i = one("replace\tRevert\tgit revert", "git re").unwrap();
        assert_eq!((i.value.as_str(), i.kind), ("vert", ItemKind::AiSuggestion));
        assert_eq!(one("append\tSame\tgit re", "git re"), None);
        // The overlap covers the trailing token.
        let i = one("append\tRebase\trebase", "git reba").unwrap();
        assert_eq!((i.value.as_str(), i.kind), ("se", ItemKind::AiSuggestion));
        let i = one("replace\tRebase\trebase", "git reba").unwrap();
        assert_eq!((i.value.as_str(), i.kind, i.replacement), ("rebase", ItemKind::AiReplacement, true));
        // A short overlap is a replacement.
        let i = one("\tList\texa -l", "list files as a table").unwrap();
        assert_eq!((i.value.as_str(), i.kind), ("exa -l", ItemKind::AiReplacement));
        // Two fields carry no mode.
        let i = one("Only two\tls", "l").unwrap();
        assert_eq!((i.value.as_str(), i.kind), ("s", ItemKind::AiSuggestion));
        assert_eq!(one("one field", "x"), None);
        assert_eq!(one("replace\tBlank\t   ", "x"), None);
    }

    #[test]
    fn ai_expansion_follows_iterm2() {
        let files = [file("b.log"), file("a.log")];
        let got = items_from_response("replace\tDelete\trm {{FILE}}\nappend\tMsg\tgit commit -m {{MESSAGE}}", "delete logs", &files);
        let values: Vec<(&str, ItemKind)> = got.iter().map(|i| (i.value.as_str(), i.kind)).collect();
        assert_eq!(values, [("rm a.log", ItemKind::AiReplacement), ("rm b.log", ItemKind::AiReplacement), ("git commit -m (MESSAGE)", ItemKind::AiReplacement)]);
        assert_eq!(got[0].detail, "Delete");
        // Not expanded: the files follow the answer's items.
        let got = items_from_response("replace\tList\tls -la", "list", &files);
        let values: Vec<&str> = got.iter().map(|i| i.value.as_str()).collect();
        assert_eq!(values, ["ls -la", "b.log", "a.log"]);
        // Duplicates by value, the first kept.
        let got = items_from_response("replace\tOne\tls\nreplace\tTwo\tls", "list", &[]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].detail, "One");
        // The expansion's files and the appended files can differ (unescaped against escaped).
        let got = items_from_response_with("replace\tOpen\tcat {{FILE}}", "show", &[file("my file")], &[file("my\\ file")]);
        assert_eq!(got.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(), ["cat my file"]);
        let got = items_from_response_with("replace\tList\tls", "show", &[file("my file")], &[file("my\\ file")]);
        assert_eq!(got.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(), ["ls", "my\\ file"]);
        // A response with no usable line is just the files.
        assert_eq!(items_from_response("nonsense", "x", &files).len(), 2);

        // The last ten uses, oldest first.
        let uses: Vec<(String, Use)> = (0..12).map(|n| (format!("c{n}"), Use { time: n as f64, dir: None, code: None })).collect();
        let h = history_for_prompt(&uses);
        assert_eq!(h.len(), HISTORY_COUNT);
        assert_eq!((h[0].as_str(), h[9].as_str()), ("c2", "c11"));
        assert!(history_for_prompt(&[]).is_empty());
    }

    #[test]
    fn ai_merge_follows_iterm2() {
        // The early item: the common prefix as a File item with the typed text in its detail.
        let e = early_item(&[file("rc"), file("rc.d")], &[], "ls ").unwrap();
        assert_eq!((e.value.as_str(), e.kind, e.detail.as_str()), ("rc", ItemKind::File, "ls rc"));
        // No common prefix: the last history item.
        let e = early_item(&[file("a"), file("b")], &[hist("ls -la"), hist("ls")], "").unwrap();
        assert_eq!((e.value.as_str(), e.kind), ("ls", ItemKind::History));
        assert_eq!(early_item(&[], &[], "x"), None);
        assert_eq!(early_item(&[file("a"), file("b")], &[], "x"), None);

        // The first is pinned; nothing from the model means nothing.
        assert!(with_first(Some(file("x")), vec![]).is_empty());
        let (x, y) = (file("x"), sugg("y"));
        assert_eq!(with_first(Some(x.clone()), vec![x.clone(), y.clone()]), vec![x.clone(), y.clone()]);
        assert_eq!(with_first(Some(x.clone()), vec![y.clone()]), vec![x.clone(), y.clone()]);
        assert_eq!(with_first(None, vec![y.clone()]), vec![y]);

        // Rows are AI items then history; replacements never become ghost text.
        let (rows, ghost) = shown(&[repl("rm a.log"), sugg("base")], &[hist("git rebase -i")]);
        assert_eq!(rows, vec![repl("rm a.log"), sugg("base"), hist("git rebase -i")]);
        assert_eq!(ghost.as_deref(), Some("base"));
        let (rows, ghost) = shown(&[repl("rm a.log")], &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(ghost, None);
        // No re-quoting: an AI value and a quoted file value come back unchanged.
        let (rows, _) = shown(&[sugg("a b | c"), file("my\\ file")], &[]);
        assert_eq!((rows[0].value.as_str(), rows[1].value.as_str()), ("a b | c", "my\\ file"));
    }

    #[test]
    fn ai_warning_follows_iterm2() {
        assert!(warning_shows(true, true));
        assert!(!warning_shows(true, false));
        assert!(!warning_shows(false, true));
        assert!(!send_tip_shows(417.0, true));
        assert!(send_tip_shows(417.0, false));
        assert_eq!(AI_WARNING, "AI completion enabled: all text sent to AI");
    }
}
