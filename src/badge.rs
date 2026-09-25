//! iTerm2's badge: the largest font size (4 to 100) at which the wrapped text fits within a
//! share of the pane, placed at the top right with margins.

use crate::layout::Rect;

/// Line height against the font size.
pub const LINE: f32 = 1.2;
const SMALLEST: f32 = 4.0;
const LARGEST: f32 = 100.0;
/// The most characters a badge shows; a longer text is cut (a badge is a label, and a huge
/// one would be measured word by word at every size).
pub const MAX_TEXT: usize = 300;

/// The profile's badge box: shares of the pane's width and height, and margins in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BadgePrefs {
    pub max_w: f32,
    pub max_h: f32,
    pub top: f32,
    pub right: f32,
}

/// A badge laid out: its font size, lines and their widths, and its box in the pane.
#[derive(Clone, Debug, PartialEq)]
pub struct BadgeLayout {
    pub size: f32,
    pub lines: Vec<String>,
    pub widths: Vec<f32>,
    pub rect: Rect,
}

/// Word wrap at a size; None when a word is wider than the box.
fn wrap(text: &str, width: f32, size: f32, max_lines: usize, measure: &dyn Fn(&str, f32) -> f32) -> Option<(Vec<String>, Vec<f32>)> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        if lines.len() >= max_lines {
            return None;
        }
        let mut line = String::new();
        for word in para.split_whitespace() {
            let next = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if measure(&next, size) <= width {
                line = next;
            } else if line.is_empty() {
                return None;
            } else {
                lines.push(std::mem::replace(&mut line, word.to_string()));
                if lines.len() >= max_lines || measure(&line, size) > width {
                    return None;
                }
            }
        }
        lines.push(line);
    }
    let widths = lines.iter().map(|l| measure(l, size)).collect();
    Some((lines, widths))
}

/// The badge for `text` in a pane of `pane` pixels: the largest whole size from 4 to 100 at
/// which the wrapped text fits the box (4 when none does). None for empty text.
pub fn layout(text: &str, pane: (f32, f32), prefs: BadgePrefs, measure: &dyn Fn(&str, f32) -> f32) -> Option<BadgeLayout> {
    let text = cut(text.trim());
    if text.is_empty() {
        return None;
    }
    let box_w = pane.0 * prefs.max_w.clamp(0.01, 1.0);
    let box_h = pane.1 * prefs.max_h.clamp(0.0, 1.0);
    // A hundredth of a pixel of slack for float rounding (100 x 1.2 is not exactly 120).
    let fits = |s: f32| wrap(text, box_w + 0.01, s, ((box_h + 0.01) / (s * LINE)) as usize, measure).filter(|(l, _)| l.len() as f32 * s * LINE <= box_h + 0.01);
    let (mut lo, mut hi) = (SMALLEST as i32, LARGEST as i32);
    let mut best = None;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        match fits(mid as f32) {
            Some(r) => {
                best = Some((mid as f32, r));
                lo = mid + 1;
            }
            None => hi = mid - 1,
        }
    }
    let (size, (lines, widths)) = best.unwrap_or_else(|| {
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        let widths = lines.iter().map(|l| measure(l, SMALLEST)).collect();
        (SMALLEST, (lines, widths))
    });
    let w = widths.iter().cloned().fold(0.0, f32::max);
    let h = lines.len() as f32 * size * LINE;
    let x = (pane.0 - prefs.right - w).max(0.0);
    Some(BadgeLayout { size, lines, widths, rect: Rect { x, y: prefs.top, w, h } })
}

/// The badge's text: the profile's Badge Text as an interpolated string; an error shows in
/// the badge.
pub fn text(template: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    if !template.contains("\\(") {
        return cut(template).to_string();
    }
    match crate::interp::evaluate(template, lookup, &[]) {
        Ok(v) => cut(&v).to_string(),
        Err(e) => cut(&format!("Error: {e}")).to_string(),
    }
}

fn cut(s: &str) -> &str {
    s.char_indices().nth(MAX_TEXT).map_or(s, |(i, _)| &s[..i])
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFS: BadgePrefs = BadgePrefs { max_w: 0.5, max_h: 0.2, top: 10.0, right: 10.0 };

    fn measure(t: &str, size: f32) -> f32 {
        t.chars().count() as f32 * 0.6 * size
    }

    #[test]
    fn badge_fits_the_box() {
        let l = layout("hello", (800.0, 600.0), PREFS, &measure).expect("a badge");
        assert_eq!(l.size, 100.0, "capped at 100");
        assert_eq!(l.lines, vec!["hello"]);
        assert_eq!((l.rect.x, l.rect.y), (800.0 - 10.0 - 300.0, 10.0), "right-aligned under the top margin");
        let long = "word ".repeat(40);
        let l = layout(long.trim(), (800.0, 600.0), PREFS, &measure).unwrap();
        assert!(l.size < 100.0 && l.lines.len() > 1, "{} {:?}", l.size, l.lines);
        assert!(l.rect.w <= 400.0 + 0.01 && l.rect.h <= 120.0 + 0.01, "{:?}", l.rect);
        assert_eq!(layout("", (800.0, 600.0), PREFS, &measure), None, "no badge for empty text");
        let tiny = layout("W", (20.0, 20.0), PREFS, &measure).unwrap();
        assert_eq!(tiny.size, 4.0, "never below 4");
        assert!(tiny.rect.x >= 0.0 && tiny.rect.y >= 0.0 && tiny.rect.x + tiny.rect.w <= 20.0 + 0.01);
    }

    #[test]
    fn badge_text_interpolates() {
        let vars = |n: &str| (n == "session.hostname").then(|| "box".to_string());
        assert_eq!(text("\\(session.hostname) build", &vars), "box build");
        assert_eq!(text("plain", &vars), "plain");
        assert!(text("\\(f())", &vars).starts_with("Error: "), "errors show in the badge");
    }

    #[test]
    fn set_badge_format_refuses_calls() {
        use base64::Engine;
        let b = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let mut s = crate::screen::Screen::new(20, 4);
        s.feed(format!("\x1b]1337;SetBadgeFormat={}\x07\x1b]1337;SetBadgeFormat={}\x07", b("\\(user.x)"), b("\\(f())")).as_bytes());
        let got: Vec<crate::screen::Request> = s.take_requests();
        assert_eq!(got, vec![crate::screen::Request::SetBadge("\\(user.x)".into()), crate::screen::Request::BadgeRefused]);
    }

    #[test]
    fn a_huge_badge_stays_fast_and_short() {
        let long = "word ".repeat(10_000);
        let t = std::time::Instant::now();
        let l = layout(&long, (800.0, 600.0), PREFS, &measure).unwrap();
        assert!(t.elapsed() < std::time::Duration::from_millis(500), "{:?}", t.elapsed());
        assert!(l.lines.iter().map(|x| x.chars().count()).sum::<usize>() <= MAX_TEXT);
        let vars = |_: &str| None;
        assert_eq!(text(&"x".repeat(5000), &vars).chars().count(), MAX_TEXT);
    }
}
