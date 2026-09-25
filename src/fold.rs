//! iTerm2's folds: a run of lines collapses into one summary line in the buffer.

/// The summary line for a fold: the start of the first line, " …N lines… ", and the start
/// of the last, fitted to `cols` characters (just "…" when even the middle does not fit).
pub fn summary(first: &str, last: &str, n: usize, cols: usize) -> String {
    let mid = format!(" \u{2026}{n} lines\u{2026} ");
    let m = mid.chars().count();
    if m >= cols {
        return "\u{2026}".into();
    }
    let avail = cols - m;
    let (f, l) = (first.trim_end(), last.trim_end());
    let (fl, ll) = (f.chars().count(), l.chars().count());
    let (take_f, take_l) = if fl + ll <= avail { (fl, ll) } else {
        let half = avail / 2;
        let take_l = ll.min(half);
        ((avail - take_l).min(fl), take_l)
    };
    let head: String = f.chars().take(take_f).collect();
    let tail: String = l.chars().take(take_l).collect();
    format!("{head}{mid}{tail}").trim_end().to_string()
}

/// Fold All's caption: when the first command ran, the first and last commands (20
/// characters each), and the line count.
pub fn fold_all_caption(time: &str, first_cmd: &str, last_cmd: &str, n: usize, cols: usize) -> String {
    let cut = |s: &str| s.chars().take(20).collect::<String>();
    let head = format!("{time} {}", cut(first_cmd));
    let tail = if last_cmd == first_cmd { String::new() } else { cut(last_cmd) };
    summary(&head, &tail, n, cols)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_fits_the_width() {
        assert_eq!(summary("cargo build", "Finished", 12, 80), "cargo build …12 lines… Finished");
        let s = summary(&"a".repeat(100), &"b".repeat(100), 3, 40);
        assert_eq!(s.chars().count(), 40);
        assert!(s.starts_with("aaaa") && s.contains(" …3 lines… ") && s.ends_with("bbb"), "{s}");
        assert_eq!(summary("x", "y", 1000, 8), "…");
        assert_eq!(fold_all_caption("10:04:05", "cargo build --release --all", "ls", 90, 80), "10:04:05 cargo build --releas …90 lines… ls");
        assert_eq!(fold_all_caption("10:04:05", "ls", "ls", 9, 80), "10:04:05 ls …9 lines…");
    }
}
