//! iTerm2's synthetic sessions: Filter shows only the lines that match, and Zoom In on
//! Selection shows only the selected lines. Both are read-only copies of the text.

use crate::find;
use crate::screen::{Look, Screen};

/// The logical lines that match `pattern`, oldest first. An empty pattern keeps them all.
pub fn filter_lines(screen: &Screen, pattern: &str) -> Result<Vec<String>, String> {
    filter_lines_in(screen, pattern, None)
}

/// `filter_lines` over buffer rows `rows` only (the selected command's); None reads them all.
pub fn filter_lines_in(screen: &Screen, pattern: &str, rows: Option<(usize, usize)>) -> Result<Vec<String>, String> {
    let re = regex::Regex::new(pattern).map_err(|e| e.to_string())?;
    let texts = match rows {
        Some((first, last)) => find::logical_texts_over(screen, first, last),
        None => find::logical_texts(screen),
    };
    Ok(texts.into_iter().filter(|l| pattern.is_empty() || re.is_match(l)).collect())
}

/// The logical lines over buffer rows `first..=last`.
pub fn zoom_lines(screen: &Screen, first: usize, last: usize) -> Vec<String> {
    find::logical_texts_over(screen, first, last)
}

/// The lines `new` adds after `old`, when `new` only grew at the end.
pub fn appended<'a>(old: &[String], new: &'a [String]) -> Option<&'a [String]> {
    (new.len() >= old.len() && new[..old.len()] == *old).then(|| &new[old.len()..])
}

/// A screen showing `lines`, scrolled to the bottom, with room for all of them.
pub fn view(lines: &[String], like: &Screen) -> Screen {
    use alacritty_terminal::grid::Dimensions;
    let (cols, rows) = (like.term().columns(), like.term().screen_lines());
    // Room for every line, and for Filter to add more as output arrives.
    let wrap = 1 + lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) / cols.max(1);
    let look = Look { scrollback: lines.len() * wrap + rows + 10_000, ..Look::default() };
    let mut s = Screen::with_look(cols, rows, like.palette.clone(), look);
    // No cursor: the view is read-only.
    s.feed(b"\x1b[?25l");
    s.feed(lines.join("\r\n").as_bytes());
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(text: &str, scrollback: usize) -> Screen {
        let mut s = Screen::with_scrollback(20, 4, scrollback);
        s.feed(text.replace('\n', "\r\n").as_bytes());
        s
    }

    #[test]
    fn filter_keeps_whole_matching_lines() {
        let s = screen("apple pie\nbanana\n..................apple wraps\ncherry", 100);
        assert_eq!(filter_lines(&s, "apple").unwrap(), vec!["apple pie", "..................apple wraps"]);
        assert_eq!(filter_lines(&s, "").unwrap().len(), 4);
        assert!(filter_lines(&s, "(").is_err());
    }

    #[test]
    fn filtering_a_large_buffer_is_quick() {
        let n = 100_000;
        let mut s = Screen::with_scrollback(80, 24, n);
        let mut text = String::new();
        for i in 0..n {
            text.push_str(&format!("line {i} {}\r\n", if i % 1000 == 0 { "needle" } else { "hay" }));
        }
        s.feed(text.as_bytes());
        let t = std::time::Instant::now();
        let hits = filter_lines(&s, "needle").unwrap();
        let took = t.elapsed();
        assert_eq!(hits.len(), 100);
        assert!(took < std::time::Duration::from_millis(500), "{took:?}");
    }

    #[test]
    fn zoom_copies_the_selected_lines() {
        let s = screen("one\ntwo\nthree\nfour", 100);
        assert_eq!(zoom_lines(&s, 1, 2), vec!["two", "three"]);
        let v = view(&zoom_lines(&s, 1, 2), &s);
        assert_eq!(v.all_text(), "two\nthree\n");
        assert_eq!(v.term().grid().display_offset(), 0);
        let many: Vec<String> = (0..50).map(|i| format!("l{i}")).collect();
        let v = view(&many, &s);
        assert!(v.all_text().starts_with("l0\nl1\n"), "the view keeps every line");
    }

    #[test]
    fn new_output_appends_to_the_filter_view() {
        let v = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let old = v(&["a1", "a2"]);
        assert_eq!(appended(&old, &v(&["a1", "a2", "a3"])), Some(&v(&["a3"])[..]));
        assert_eq!(appended(&old, &v(&["a1", "a2"])), Some(&[][..]));
        assert_eq!(appended(&old, &v(&["a2", "a3"])), None);
        assert_eq!(appended(&old, &v(&["a1"])), None);
    }
}
