//! Portholes: a document shown rendered in place of grid rows (iTerm2's "render natively").

pub mod detect;
pub mod render;
pub mod store;

/// Where a link in a rendered document goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Url(String),
    File(String),
}

/// A markdown link's target: web and mail links; `file://` URLs, drive paths, and paths
/// relative to the porthole's folder, when they are written as a drive path. Other schemes are
/// refused. This only sorts the text: a file target is opened only after `LocalPath::judge`
/// (porthole_ui `open_link_target`), the one gate that refuses mapped drives and links to shares.
pub fn link_target(link: &str, base_dir: Option<&str>) -> Option<LinkTarget> {
    let link = link.trim();
    let windows = |p: &str| p.replace('/', "\\");
    if let Some(rest) = link.get(..7).filter(|p| p.eq_ignore_ascii_case("file://")).map(|_| &link[7..]) {
        let path = windows(rest.trim_start_matches('/'));
        return drive_path(&path).then_some(LinkTarget::File(path));
    }
    if let Some((scheme, rest)) = link.split_once(':') {
        if scheme.len() > 1 && scheme.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)) {
            let ok = matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "ftp" | "mailto") && !rest.is_empty();
            return ok.then(|| LinkTarget::Url(link.to_string()));
        }
    }
    let path = windows(link);
    if drive_path(&path) {
        return Some(LinkTarget::File(path));
    }
    if path.starts_with('\\') {
        return None;
    }
    let base = base_dir.filter(|b| drive_path(b))?;
    let joined = format!("{}\\{}", base.trim_end_matches(['\\', '/']), path);
    drive_path(&joined).then_some(LinkTarget::File(joined))
}

/// A path written from a drive letter (`C:\...`, `c:/...`) that is no network spelling: the
/// shape of a file link, not a judgement of where it leads.
fn drive_path(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') && !crate::scripts::is_network_path(path)
}

/// What a link click does: open it, or, for a file Windows would run (a program, a
/// script), show it in Explorer instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkAction {
    Open,
    Reveal,
}

pub fn link_action(target: &LinkTarget) -> LinkAction {
    match target {
        LinkTarget::File(f) if crate::semantic::runnable(f) => LinkAction::Reveal,
        _ => LinkAction::Open,
    }
}

/// The question a link click asks, and the setting that silences it for good.
pub fn link_prompt(target: &LinkTarget) -> (String, &'static str) {
    match target {
        LinkTarget::Url(u) => (format!("Open URL {u}?"), "NoSyncOpenURLFromMarkdownLink"),
        LinkTarget::File(f) => (format!("Open file at {f}?"), "NoSyncOpenFileFromMarkdownLink"),
    }
}

/// Copy with Control Sequences for rendered text: each run in its colors and styles.
pub fn sgr(rows: &[(Vec<render::Run>, bool)]) -> String {
    let mut out = String::new();
    for (runs, hard) in rows {
        for r in runs {
            let mut codes = format!("\x1b[0;38;2;{};{};{}", r.color.r, r.color.g, r.color.b);
            if r.bold {
                codes.push_str(";1");
            }
            if r.italic {
                codes.push_str(";3");
            }
            if r.underline {
                codes.push_str(";4");
            }
            out.push_str(&codes);
            out.push('m');
            out.push_str(&r.text);
        }
        if *hard {
            out.push_str("\x1b[0m\n");
        }
    }
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Render Selection Natively is on for one selection that is not being dragged and does not
/// cover a porthole.
pub fn selection_can_render(selections: usize, dragging: bool, overlaps: bool) -> bool {
    selections == 1 && !dragging && !overlaps
}

/// The whole rows a selection from `start` to `end` (line, column) renders: a selection
/// ending at column 0 of a later line does not take that line (iTerm2 took it).
pub fn selection_rows(start: (i32, usize), end: (i32, usize)) -> (i32, i32) {
    let last = if end.1 == 0 && end.0 > start.0 { end.0 - 1 } else { end.0 };
    (start.0, last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::Screen;
    use base64::Engine;

    fn b64(s: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(s)
    }

    #[test]
    fn render_selection_enablement() {
        assert!(selection_can_render(1, false, false));
        assert!(!selection_can_render(0, false, false), "nothing selected");
        assert!(!selection_can_render(1, true, false), "still dragging");
        assert!(!selection_can_render(1, false, true), "over a porthole");
    }

    #[test]
    fn selection_ending_at_column_0() {
        assert_eq!(selection_rows((2, 5), (4, 0)), (2, 3));
        assert_eq!(selection_rows((2, 5), (2, 0)), (2, 2));
        assert_eq!(selection_rows((1, 0), (3, 7)), (1, 3));
    }

    #[test]
    fn block_render_makes_a_porthole() {
        let mut s = Screen::new(30, 10);
        s.feed(b"\x1b]1337;Block=id=r;attr=start;type=markdown\x07# Hi\r\ntext\r\n\x1b]1337;Block=id=r;attr=end;render=1\x07");
        assert_eq!(s.portholes.len(), 1);
        let p = s.portholes.values().next().unwrap();
        assert_eq!(p.text.trim_end(), "# Hi\ntext");
        assert_eq!(p.detected.language, Some(detect::Language::Markdown));
        s.feed(b"\x1b]1337;Block=id=q;attr=start\x07plain\r\n\x1b]1337;Block=id=q;attr=end\x07");
        assert_eq!(s.portholes.len(), 1, "no render=1, no porthole");
    }

    #[test]
    fn file_text_needs_utf8() {
        let mut s = Screen::new(30, 10);
        s.feed(format!("\x1b]1337;File=inline=1;type=text/plain:{}\x07", b64(&[0xff, 0xfe, 0x00])).as_bytes());
        s.allow_inline();
        assert!(s.portholes.is_empty(), "not UTF-8: no porthole");
        assert!(!s.images.by_id.is_empty(), "the broken image shows");
        s.feed(format!("\x1b]1337;File=inline=1;name={}:{}\x07", b64(b"notes.md"), b64(b"hello\nworld")).as_bytes());
        s.allow_inline();
        assert_eq!(s.portholes.len(), 1);
        let p = s.portholes.values().next().unwrap();
        assert_eq!(p.text.trim_end(), "hello\nworld");
        assert_eq!(p.filename.as_deref(), Some("notes.md"));
        assert_eq!(p.detected.language, Some(detect::Language::Markdown), "from the file name");
        s.feed(format!("\x1b]1337;File=inline=0;name={}:{}\x07", b64(b"a.txt"), b64(b"x")).as_bytes());
        assert_eq!(s.portholes.len(), 1, "inline=0 downloads");
    }

    #[test]
    fn smells_like_image_follows_iterm2() {
        use crate::inline_file::smells_like_image;
        assert!(smells_like_image(None, None));
        assert!(!smells_like_image(None, Some("a.md")), "a known text file name");
        assert!(smells_like_image(None, Some("photo.png")));
        assert!(!smells_like_image(Some(".py"), None));
        assert!(smells_like_image(Some("image/png"), None));
        assert!(smells_like_image(Some("video/mp4"), None));
        assert!(!smells_like_image(Some("text/plain"), None));
        assert!(!smells_like_image(Some("Markdown"), None));
        assert!(!smells_like_image(Some(".zzz"), None), "anything else is tried as text");
    }

    #[test]
    fn links_resolve_and_ask() {
        assert_eq!(link_target("https://x.org/a", None), Some(LinkTarget::Url("https://x.org/a".into())));
        assert_eq!(link_target("mailto:a@b.c", None), Some(LinkTarget::Url("mailto:a@b.c".into())));
        assert_eq!(link_target("javascript:alert(1)", None), None, "other schemes are refused");
        assert_eq!(link_target("notes.md", Some("C:\\docs")), Some(LinkTarget::File("C:\\docs\\notes.md".into())));
        assert_eq!(link_target("sub/notes.md", Some("C:\\docs\\")), Some(LinkTarget::File("C:\\docs\\sub\\notes.md".into())));
        assert_eq!(link_target("file:///C:/x/y.txt", None), Some(LinkTarget::File("C:\\x\\y.txt".into())));
        assert_eq!(link_target("\\\\server\\share\\x", None), None, "network paths leak the NTLM hash");
        assert_eq!(link_target("notes.md", Some("/home/me")), None, "a WSL folder is not a Windows path");
        assert_eq!(link_target("notes.md", None), None);
        assert_eq!(link_prompt(&LinkTarget::Url("https://x.org".into())), ("Open URL https://x.org?".to_string(), "NoSyncOpenURLFromMarkdownLink"));
        assert_eq!(link_prompt(&LinkTarget::File("C:\\a".into())), ("Open file at C:\\a?".to_string(), "NoSyncOpenFileFromMarkdownLink"));
    }

    #[test]
    fn block_copy_uses_raw_rows() {
        let mut s = Screen::new(30, 10);
        s.feed(b"\x1b]1337;Block=id=r;attr=start\x07{\"a\": 1}\r\n\x1b]1337;Block=id=r;attr=end;render=1\x07");
        assert_eq!(s.portholes.len(), 1);
        assert!(s.block_text("r").expect("the block still has rows").starts_with("{\"a\": 1}\n"), "{:?}", s.block_text("r"));
    }

    #[test]
    fn selection_is_exclusive() {
        let mut s = Screen::new(30, 10);
        s.feed(b"# T\r\nx\r\n$ ");
        let id = s.render_range_estimated(0, 1, store::Source::default()).unwrap();
        s.term_mut().selection = Some(alacritty_terminal::selection::Selection::new(
            alacritty_terminal::selection::SelectionType::Simple,
            alacritty_terminal::index::Point::new(alacritty_terminal::index::Line(0), alacritty_terminal::index::Column(0)),
            alacritty_terminal::index::Side::Left,
        ));
        let a = render::Pos { line: 0, col: 0 };
        let b = render::Pos { line: 0, col: 1 };
        s.select_in_porthole(id, a, b);
        assert!(s.term().selection.is_none(), "a porthole selection clears the terminal's");
        assert_eq!(s.porthole_selection, Some((id, a, b)));
        s.select_in_porthole(99, b, a);
        assert_eq!(s.porthole_selection, Some((99, b, a)), "one porthole selection at a time");
        s.clear_porthole_selection();
        assert_eq!(s.porthole_selection, None);
    }

    #[test]
    fn non_ascii_links_do_not_crash() {
        assert_eq!(link_target("résumé.md", Some("C:\\d")), Some(LinkTarget::File("C:\\d\\résumé.md".into())));
        assert_eq!(link_target("日本語.md", Some("C:\\d")), Some(LinkTarget::File("C:\\d\\日本語.md".into())));
        assert_eq!(link_target("fé", None), None);
    }

    #[test]
    fn runnable_files_are_revealed() {
        assert_eq!(link_action(&LinkTarget::File("C:\\x\\setup.bat".into())), LinkAction::Reveal);
        assert_eq!(link_action(&LinkTarget::File("C:\\x\\tool.EXE".into())), LinkAction::Reveal);
        assert_eq!(link_action(&LinkTarget::File("C:\\x\\notes.md".into())), LinkAction::Open);
        assert_eq!(link_action(&LinkTarget::Url("https://x.org/a.exe".into())), LinkAction::Open);
        // A stream suffix does not hide the program (findings r2 N1).
        assert_eq!(link_action(&LinkTarget::File("C:\\x\\a.exe::$DATA".into())), LinkAction::Reveal);
        assert_eq!(link_action(&LinkTarget::File("C:\\x\\a.cmd:s".into())), LinkAction::Reveal);
    }

    #[test]
    fn older_porthole_selection_loses_to_the_terminals() {
        let mut s = Screen::new(30, 10);
        s.feed(b"# T\r\nx\r\n$ ");
        let id = s.render_range_estimated(0, 1, store::Source::default()).unwrap();
        let a = render::Pos { line: 0, col: 0 };
        s.select_in_porthole(id, a, render::Pos { line: 0, col: 1 });
        assert!(s.porthole_copy_selection().is_some());
        s.term_mut().selection = Some(alacritty_terminal::selection::Selection::new(
            alacritty_terminal::selection::SelectionType::Simple,
            alacritty_terminal::index::Point::new(alacritty_terminal::index::Line(2), alacritty_terminal::index::Column(0)),
            alacritty_terminal::index::Side::Left,
        ));
        assert_eq!(s.porthole_copy_selection(), None, "the terminal's selection is the newer one");
    }
}
