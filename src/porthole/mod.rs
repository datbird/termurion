//! Portholes: a document shown rendered in place of grid rows (iTerm2's "render natively").

pub mod detect;
pub mod render;
pub mod store;

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
        assert!(s.portholes.is_empty(), "not UTF-8: no porthole");
        assert!(!s.images.by_id.is_empty(), "the broken image shows");
        s.feed(format!("\x1b]1337;File=inline=1;name={}:{}\x07", b64(b"notes.md"), b64(b"hello\nworld")).as_bytes());
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
}
