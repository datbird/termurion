//! The parity audit: `docs/parity/01..03` say what the code does, and this test keeps them that
//! way. A row is "have" only with its code site or test named, "partial" says what is missing,
//! "missing" has an owner (docs/TWEAKS.md, "not planned", or a ruling) and "n/a" has its
//! Windows reason. Reading the markdown is the whole mechanism (13y2 Task 10), so a new table
//! shape in the parity files needs `rows` updated.

/// One table row of a parity file.
#[derive(Clone, Debug)]
pub struct Row {
    /// 1-based line number in the file.
    pub line: usize,
    pub cells: Vec<String>,
    /// The status word: "have", "partial", "missing" or "n/a" (anything else is kept as written).
    pub status: String,
    /// What follows the status: the rest of the status cell plus every later cell.
    pub note: String,
}

fn split_cells(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut prev = ' ';
    for ch in inner.chars() {
        if ch == '|' && prev != '\\' {
            cells.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(ch);
        }
        prev = ch;
    }
    cells.push(cur.trim().to_string());
    cells
}

/// The rows of every table that has a "Tandem status" column.
pub fn rows(md: &str) -> Vec<Row> {
    let lines: Vec<&str> = md.lines().collect();
    let mut out = Vec::new();
    let mut col: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        if !l.starts_with('|') {
            col = None;
            continue;
        }
        if lines.get(i + 1).is_some_and(|n| n.starts_with("|---")) {
            col = split_cells(l).iter().position(|c| c == "Tandem status");
            continue;
        }
        let Some(c) = col else { continue };
        if l.starts_with("|---") {
            continue;
        }
        let cells = split_cells(l);
        let cell = cells.get(c).cloned().unwrap_or_default();
        let stripped = cell.trim_start_matches('*');
        let word: String = stripped.chars().take_while(|ch| ch.is_ascii_alphabetic() || *ch == '/').collect();
        let status = word.to_lowercase();
        let mut note = stripped[word.len()..].trim_start_matches('*').to_string();
        for later in cells.iter().skip(c + 1) {
            note.push(' ');
            note.push_str(later);
        }
        out.push(Row { line: i + 1, cells, status, note: note.trim().to_string() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const F01: &str = include_str!("../docs/parity/01-profile-settings.md");
    const F02: &str = include_str!("../docs/parity/02-global-settings.md");
    const F03: &str = include_str!("../docs/parity/03-features-and-menus.md");

    /// Follow-up plans whose rows may still say "to come". Every 13x follow-up has landed (their
    /// rulings files exist), so the list is empty; `accepted_plans_are_unbuilt` fails a plan that
    /// is listed while its rulings file exists.
    const ACCEPTED_LATER: [&str; 0] = [];

    fn files() -> [(&'static str, Vec<Row>); 3] {
        [("01", rows(F01)), ("02", rows(F02)), ("03", rows(F03))]
    }

    fn find<'a>(rows: &'a [Row], c0: &str, c1: Option<&str>) -> Vec<&'a Row> {
        rows.iter().filter(|r| r.cells[0].eq_ignore_ascii_case(c0) && c1.is_none_or(|x| r.cells.get(1).is_some_and(|c| c.starts_with(x)))).collect()
    }

    /// Words that say a milestone is still to come.
    const STALE: [&str; 8] = [
        "comes in a later milestone",
        "effect comes later",
        "comes in milestone",
        "come with milestone",
        "comes with milestone",
        "until milestone",
        "follow-up plan",
        "later plans",
    ];

    #[test]
    fn rows_parse_the_three_table_shapes() {
        let md = "x\n| A | B | Tandem status | Note |\n|---|---|---|---|\n| a | b | have | cite |\n| c | d | **partial**, only x | n |\n\n| E | Tandem status |\n|---|---|\n| e | n/a, reason |\n";
        let r = rows(md);
        assert_eq!(r.len(), 3);
        assert_eq!((r[0].line, r[0].status.as_str(), r[0].note.as_str()), (4, "have", "cite"));
        assert_eq!((r[1].status.as_str(), r[1].note.as_str()), ("partial", ", only x n"));
        assert_eq!((r[2].status.as_str(), r[2].note.as_str()), ("n/a", ", reason"));
        assert_eq!(rows("| a | b |\n|---|---|\n| 1 | 2 |").len(), 0, "no status column, no rows");
    }

    #[test]
    fn accepted_plans_are_unbuilt() {
        for p in ACCEPTED_LATER {
            let ruled = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("docs/superpowers/rulings/m{p}-rulings.txt"));
            assert!(!ruled.exists(), "{p} has landed, so rows may not call it still to come");
        }
    }

    #[test]
    fn parity_files_follow_the_audit() {
        let mut bad = Vec::new();
        for (f, rs) in files() {
            let mut seen = std::collections::HashSet::new();
            for r in &rs {
                let at = format!("{f}:{} {}", r.line, r.cells[0]);
                if !["have", "partial", "missing", "n/a"].contains(&r.status.as_str()) {
                    bad.push(format!("{at}: status {:?}", r.status));
                    continue;
                }
                let note = r.note.to_lowercase();
                let lower = format!("{} {}", r.cells.get(2).map(|c| c.to_lowercase()).unwrap_or_default(), note);
                if r.status != "have" {
                    if let Some(i) = lower.find("(milestone ") {
                        let rest = &lower[i + 11..];
                        let end = rest.find(')').unwrap_or(0);
                        if end > 0 && rest[..end].chars().all(|c| c.is_ascii_alphanumeric()) {
                            bad.push(format!("{at}: {} row says \"(milestone {})\"", r.status, &rest[..end]));
                        }
                    }
                    for s in STALE {
                        if lower.contains(s) && !ACCEPTED_LATER.iter().any(|p| note.contains(p)) {
                            bad.push(format!("{at}: {} row says {s:?}", r.status));
                        }
                    }
                }
                match r.status.as_str() {
                    "missing" => {
                        if !(note.contains("tweaks.md") || note.contains("not planned") || note.contains("ruling")) {
                            bad.push(format!("{at}: missing without an owner"));
                        }
                    }
                    "partial" => {
                        const NAMES_A_GAP: [&str; 20] = [
                            "not ", "no ", "only", "missing", "lacks", "without", "except", "but ", "still", "instead", "cannot",
                            "never", "nothing", "does ", "pending", "until", "proof", "yet", "inert", "need",
                        ];
                        if !NAMES_A_GAP.iter().any(|w| note.contains(w)) || note.len() < 12 {
                            bad.push(format!("{at}: partial without a named gap"));
                        }
                    }
                    "n/a" => {
                        if r.note.trim_matches(|c: char| c == ',' || c.is_whitespace()).len() < 12 {
                            bad.push(format!("{at}: n/a without its Windows reason"));
                        }
                    }
                    _ => {}
                }
                // A title (plus the feature cell of a 5-column table) names one row.
                if f != "01" && r.cells.len() >= 5 && !seen.insert((r.cells[0].clone(), r.cells[1].clone())) {
                    bad.push(format!("{at}: duplicate row"));
                }
            }
        }
        assert!(bad.is_empty(), "{} rows fail the audit:\n{}", bad.len(), bad.join("\n"));
    }

    /// Section 0's corrections, by title (line numbers shift): (file, title, feature cell, status).
    const CORRECTED: [(&str, &str, Option<&str>, &str); 24] = [
        ("02", "Always open at least one terminal window at startup", None, "have"),
        ("02", "Shortcut to activate a window", None, "have"),
        ("02", "Alert on marks in offscreen sessions", None, "have"),
        ("02", "Alert on Next Mark", None, "have"),
        ("02", "openNewWindowAtStartup", None, "have"),
        ("02", "useVirtualKeyCodesForDetectingDigits", None, "have"),
        ("02", "noSyncSuppressCaptureOutputRequiresShellIntegrationWarning", None, "have"),
        ("02", "noSyncSuppressCaptureOutputToolNotVisibleWarning", None, "have"),
        ("02", "noSyncSuppressSendSignal", None, "have"),
        ("01", "Custom title function", None, "have"),
        ("01", "Terminal may resize window", None, "have"),
        ("01", "Delete sends Ctrl-H", None, "have"),
        ("01", "Capture Output", None, "have"),
        ("02", "minimalSplitPaneDividerProminence", None, "have"),
        ("02", "splitPaneSourceFillColor", None, "have"),
        ("02", "alternateMouseScrollStringForUp", None, "have"),
        ("02", "useMultiPassUnderlineRenderer", None, "have"),
        ("02", "typingClearsSelection", None, "have"),
        ("03", "Marks and navigation", Some("Mark rendering"), "have"),
        ("03", "Paste history", Some("Paste History popup"), "have"),
        ("03", "Paste history", Some("Erase paste history"), "have"),
        ("03", "Annotations", Some("Annotation from Named Mark"), "have"),
        ("03", "tmux integration", Some("Window to tab/window mapping"), "have"),
        ("03", "OSC 1337 ; ClearCapturedOutput", None, "have"),
    ];

    #[test]
    fn section_zero_rows_read_as_corrected() {
        let all = files();
        for (f, title, c1, want) in CORRECTED {
            let rs = &all.iter().find(|(n, _)| *n == f).unwrap().1;
            let hit = find(rs, title, c1);
            assert_eq!(hit.len(), 1, "{f} {title}: {} rows", hit.len());
            assert_eq!(hit[0].status, want, "{f}:{} {title}", hit[0].line);
        }
        // Rows that were written wrong.
        let r03 = &all[2].1;
        let soft = find(r03, "Edit > Selection Respects Soft Boundaries", None);
        assert!(soft[0].note.contains("divider characters"), "soft boundaries are dividers programs draw");
        let man = find(&all[1].1, "viewManPageCommand", None);
        assert!(man[0].cells[1].contains("man %@ \\|\\| sleep 3"), "the default is man %@ || sleep 3");
        let wg = find(&all[1].1, "Workgroups", None);
        assert!(wg[0].cells[2].contains("User-configured session groups"));
        // The Companion rows are n/a with the relay reason.
        for (f, title) in [("03", "iTerm2 > Companion Device Settings"), ("01", "Also Send to Companion"), ("02", "companionPairingAllowed")] {
            let rs = &all.iter().find(|(n, _)| *n == f).unwrap().1;
            let hit = find(rs, title, None);
            assert_eq!(hit.len(), 1, "{title}");
            assert_eq!(hit[0].status, "n/a", "{title}");
            assert!(hit[0].note.contains("proprietary iPhone pairing"), "{title}");
        }
        let adapter = find(r03, "Password manager", Some("Generic command-line adapter"));
        assert_eq!(adapter[0].status, "n/a");
        assert!(adapter[0].note.contains("not an iTerm2 feature"));
    }

    #[test]
    fn features_with_no_row_have_one() {
        let all = files();
        let has = |f: usize, needle: &str| all[f].1.iter().any(|r| r.cells[0].contains(needle) || r.cells.get(1).is_some_and(|c| c.contains(needle)));
        assert!(has(2, "SGR underline styles"), "SGR underline drawing");
        assert!(has(2, "Strikethrough"), "SGR strikethrough drawing");
        assert!(has(0, "IME composition"), "IME composition");
        assert!(has(0, "Window transparency"), "window transparency");
        assert!(has(2, "Multiple selections"), "multiple selections");
        assert!(has(2, "DEC private mode 2048"), "DEC mode 2048");
        assert!(has(2, "Browser form autofill") && has(2, "Browser password manager button"), "browser rows 13x added");
        assert!(has(2, "Live Dynamic Profiles folder"));
        let suite = find(&all[1].1, "-suite isolated instances", None);
        assert_eq!(suite.len(), 1, "one -suite row, not two");
        assert_eq!(suite[0].status, "missing");
        assert!(suite[0].note.contains("ruling"));
    }

    #[test]
    fn no_stale_later_stubs() {
        use crate::commands::Status;
        for code in 0..200 {
            if let Status::Later(n) = crate::keyaction::status(code) {
                panic!("key action {code} is Later({n})");
            }
        }
        for action in crate::pointer::ACTIONS.iter().map(|a| a.0) {
            let b = crate::pointer::PointerBinding::new(action, "");
            if let Status::Later(n) = crate::pointer::status(&b) {
                panic!("pointer action {action} is Later({n})");
            }
        }
        let mut stale = Vec::new();
        for i in crate::menus::items().iter().filter(|i| !i.separator && !i.submenu && !i.selector.is_empty()) {
            if let Status::Later(n) = crate::commands::status(i) {
                stale.push(format!("{} -> Later({n})", i.id()));
            }
        }
        assert!(stale.is_empty(), "stale stubs:\n{}", stale.join("\n"));
    }
}
