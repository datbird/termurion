//! tmux sizes from Termurion (iTerm2's TmuxController window sizing): in fixed-size mode one
//! client size for every window (the smallest tmux tab), set with `@iterm2_size` and
//! `refresh-client -C`, then a short list-windows for the new layouts; in variable-size mode
//! (tmux 2.9 and later) one size per window.

use std::collections::HashMap;

/// Variable window sizes: tmux 2.9 or later and tmuxVariableWindowSizesSupported.
pub fn variable(min: Option<f64>, supported: bool) -> bool {
    supported && min.is_some_and(|v| v >= 2.9)
}

/// Fixed-size mode: the smallest tab in each direction; None when a size is 0 (a window
/// closing reports that).
pub fn fixed_client_size(tabs: &[(u32, u32)]) -> Option<(u32, u32)> {
    let w = tabs.iter().map(|t| t.0).min()?;
    let h = tabs.iter().map(|t| t.1).min()?;
    (w > 0 && h > 0).then_some((w, h))
}

/// iTerm2's commandToListWindows (the short form used after a resize).
pub fn short_list(version: f64) -> String {
    if version >= 2.4 {
        "list-windows -F \"#{window_id} #{window_layout} #{window_flags} #{window_visible_layout} #{pane-border-status}\"".into()
    } else if version >= 2.2 {
        "list-windows -F \"#{window_id} #{window_layout} #{window_flags} #{window_visible_layout}\"".into()
    } else {
        "list-windows -F \"#{window_id} #{window_layout} #{window_flags}\"".into()
    }
}

/// iTerm2's setClientSize list: (command, errors tolerated).
pub fn client_size_commands(session: u32, w: u32, h: u32, status_bar: bool, version: Option<f64>) -> Vec<(String, bool)> {
    let height = crate::tmux_windows::client_height(h, status_bar, version);
    vec![
        (format!("set -t ${session} @iterm2_size {w},{h}"), false),
        (format!("refresh-client -C {w},{height}"), true),
        (short_list(version.unwrap_or(0.0)), false),
    ]
}

/// One row of the short list: window, layout, zoomed, visible layout, border status.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub window: u32,
    pub layout: String,
    pub zoomed: Option<bool>,
    pub visible: Option<String>,
    pub border: Option<String>,
}

/// The short list-windows reply (iTerm2's regex; empty flags allowed).
pub fn parse_short(reply: &str) -> Vec<Row> {
    reply
        .split('\n')
        .filter_map(|l| {
            let rest = l.strip_prefix('@')?;
            let (id, rest) = rest.split_once(' ')?;
            let window: u32 = id.parse().ok().filter(|_| id.bytes().all(|b| b.is_ascii_digit()))?;
            let mut parts = rest.splitn(4, ' ');
            let layout = parts.next().filter(|s| !s.is_empty())?.to_string();
            let flags = parts.next();
            let visible = parts.next().filter(|s| !s.is_empty()).map(String::from);
            let border = parts.next().and_then(|s| s.split(' ').next()).filter(|s| !s.is_empty()).map(String::from);
            Some(Row { window, layout, zoomed: flags.map(|f| f.contains('Z')), visible, border })
        })
        .collect()
}

/// Variable-size mode: a command per window whose size changed (`refresh-client -C @W:WxH`
/// on 3.4, else `resize-window`). Keys that are not window ids (`pty-...`, `..._ph`) and
/// sizes outside 1 to 9999 are skipped; `last` remembers what was sent.
pub fn window_size_commands(sizes: &[(String, (u32, u32))], last: &mut HashMap<u32, (u32, u32)>, version: f64) -> Vec<String> {
    let mut out = Vec::new();
    for (key, (w, h)) in sizes {
        if key.starts_with("pty") || key.ends_with("_ph") {
            continue;
        }
        // 10000 is tmux's WINDOW_MAXIMUM.
        if *w < 1 || *h < 1 || *w >= 10000 || *h >= 10000 {
            continue;
        }
        let window = crate::tmux_windows::window_id(key);
        if last.get(&window) == Some(&(*w, *h)) {
            continue;
        }
        last.insert(window, (*w, *h));
        out.push(if version >= 3.4 { format!("refresh-client -C @{window}:{w}x{h}") } else { format!("resize-window -x {w} -y {h} -t @{window}") });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_size_is_the_minimum() {
        assert_eq!(fixed_client_size(&[(80, 24), (100, 20)]), Some((80, 20)));
        assert_eq!(fixed_client_size(&[(80, 24), (0, 20)]), None, "a closing window reports 0");
        assert_eq!(fixed_client_size(&[]), None);
        assert!(variable(Some(2.9), true) && !variable(Some(2.8), true) && !variable(Some(3.4), false));
        let c = client_size_commands(2, 80, 24, true, Some(2.91));
        assert_eq!(c[0], ("set -t $2 @iterm2_size 80,24".into(), false));
        assert_eq!(c[1], ("refresh-client -C 80,25".into(), true), "the 2.9 status bar fix");
        assert_eq!(c[2].0, "list-windows -F \"#{window_id} #{window_layout} #{window_flags} #{window_visible_layout} #{pane-border-status}\"");
        assert_eq!(short_list(2.1), "list-windows -F \"#{window_id} #{window_layout} #{window_flags}\"");
    }

    #[test]
    fn variable_sizes_by_version() {
        let mut last = HashMap::new();
        assert_eq!(window_size_commands(&[("3".into(), (80, 24))], &mut last, 3.4), ["refresh-client -C @3:80x24"]);
        let mut last = HashMap::new();
        assert_eq!(window_size_commands(&[("3".into(), (80, 24))], &mut last, 3.3), ["resize-window -x 80 -y 24 -t @3"]);
    }

    #[test]
    fn sizes_skip_unchanged_and_out_of_range() {
        let mut last = HashMap::new();
        let sizes = vec![("1".to_string(), (80, 24)), ("pty-ABC".into(), (80, 24)), ("2_ph".into(), (80, 24)), ("4".into(), (10000, 24)), ("5".into(), (0, 24))];
        assert_eq!(window_size_commands(&sizes, &mut last, 3.4), ["refresh-client -C @1:80x24"]);
        assert!(window_size_commands(&sizes, &mut last, 3.4).is_empty(), "unchanged");
        assert_eq!(window_size_commands(&[("1".into(), (81, 24))], &mut last, 3.4).len(), 1);
    }

    #[test]
    fn short_list_rows_parse() {
        let rows = parse_short("@0 d67e,80x24,0,0,0 * d67e,80x24,0,0,0 off\n@1 b25d,80x24,0,0,1  b25d,80x24,0,0,1 top\n@2 x\nbogus");
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0].zoomed, rows[0].border.as_deref()), (Some(false), Some("off")));
        assert_eq!((rows[1].visible.as_deref(), rows[1].border.as_deref()), (Some("b25d,80x24,0,0,1"), Some("top")), "empty flags keep the rest");
        assert_eq!((rows[2].window, rows[2].zoomed), (2, None));
    }
}
