//! What Tandem keeps in the tmux session (iTerm2's session options: `@hidden` and
//! `@affinities`, hex with a prefix), and the questions for closing tmux tabs and windows.

use std::collections::BTreeSet;

/// iTerm2's encodedString: the prefix, then lowercase hex of the UTF-8.
pub fn encode_option(text: &str, prefix: &str) -> String {
    let hex: String = text.bytes().map(|b| format!("{b:02x}")).collect();
    format!("{prefix}{hex}")
}

/// A pane's `user.*` variables as iTerm2's `@uservars`: `key=value` pairs (a value ends at
/// its first NUL) joined by NUL, encoded with `u_`.
pub fn user_vars_value(vars: &std::collections::BTreeMap<String, String>) -> String {
    let pairs: Vec<String> = vars.iter().map(|(k, v)| format!("{k}={}", v.split('\0').next().unwrap_or(""))).collect();
    encode_option(&pairs.join("\0"), "u_")
}

/// `@uservars` back to variables: `u_` hex (or plain text), NUL-separated, each split at its
/// first `=`; parts without `=` are skipped.
pub fn parse_user_vars(value: &str) -> std::collections::BTreeMap<String, String> {
    crate::tmux_windows::decode_option(value, "u_").split('\0').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

pub fn user_vars_command(pane: u32, value: &str) -> String {
    format!("set -p -t %{pane} @uservars \"{value}\"")
}

/// A window's saved size (iTerm2's per-window `size=`): `{W, H}`, or `fs` for full screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WinSize {
    Size(i64, i64),
    FullScreen,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowSettings {
    pub size: Option<WinSize>,
}

/// `@per_window_settings` (plain): `terminal:key=value&key=value;terminal:...`.
pub fn per_window_value(windows: &[(String, WindowSettings)]) -> String {
    windows
        .iter()
        .filter_map(|(t, w)| {
            let size = match w.size? {
                WinSize::Size(x, y) => format!("{{{x}, {y}}}"),
                WinSize::FullScreen => "fs".into(),
            };
            Some(format!("{t}:size={size}"))
        })
        .collect::<Vec<_>>()
        .join(";")
}

pub fn parse_per_window(value: &str) -> std::collections::HashMap<String, WindowSettings> {
    let text = crate::tmux_windows::decode_option(value, "w_");
    let mut out = std::collections::HashMap::new();
    for part in text.split(';') {
        let Some((terminal, settings)) = part.split_once(':') else { continue };
        let mut w = WindowSettings::default();
        for kv in settings.split('&') {
            match kv.split_once('=') {
                Some(("size", "fs")) => w.size = Some(WinSize::FullScreen),
                Some(("size", v)) => {
                    let nums: Vec<f64> = v.trim_matches(['{', '}']).split(',').filter_map(|n| n.trim().parse().ok()).collect();
                    if let [x, y] = nums[..] {
                        if x > 0.0 && y > 0.0 {
                            w.size = Some(WinSize::Size(x as i64, y as i64));
                        }
                    }
                }
                _ => {}
            }
        }
        out.insert(terminal.to_string(), w);
    }
    out
}

/// `@per_tab_settings` (plain): `window:t=<base64 of the title override>;...`.
pub fn per_tab_value(tabs: &[(u32, String)]) -> String {
    use base64::Engine;
    tabs.iter().map(|(w, t)| format!("{w}:t={}", base64::engine::general_purpose::STANDARD.encode(t))).collect::<Vec<_>>().join(";")
}

pub fn parse_per_tab(value: &str) -> std::collections::HashMap<u32, String> {
    use base64::Engine;
    let text = crate::tmux_windows::decode_option(value, "T_");
    let mut out = std::collections::HashMap::new();
    for part in text.split(';') {
        let Some((w, settings)) = part.split_once(':') else { continue };
        let Ok(w) = w.parse::<u32>() else { continue };
        for kv in settings.split('&') {
            if let Some(("t", b64)) = kv.split_once('=') {
                let title = base64::engine::general_purpose::STANDARD.decode(b64).ok().map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
                if !title.is_empty() {
                    out.insert(w, title);
                }
            }
        }
    }
    out
}

/// `@origins` (plain): `w1,w2:x,y w3:x,y`.
pub fn origins_value(groups: &[(Vec<u32>, (i32, i32))]) -> String {
    groups
        .iter()
        .map(|(ws, (x, y))| format!("{}:{x},{y}", ws.iter().map(u32::to_string).collect::<Vec<_>>().join(",")))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse_origins(value: &str) -> std::collections::HashMap<u32, (i32, i32)> {
    let text = crate::tmux_windows::decode_option(value, "o_");
    let mut out = std::collections::HashMap::new();
    for group in text.split(' ') {
        let parts: Vec<&str> = group.split(':').collect();
        let [ids, point] = parts[..] else { continue };
        let xy: Vec<&str> = point.split(',').collect();
        let [x, y] = xy[..] else { continue };
        let (Ok(x), Ok(y)) = (x.trim().parse::<i32>(), y.trim().parse::<i32>()) else { continue };
        for id in ids.split(',').filter_map(|i| i.trim().parse::<u32>().ok()) {
            out.insert(id, (x, y));
        }
    }
    out
}

/// `@buried_indexes` (plain): `terminal:window=index,window=index terminal:...`.
pub fn buried_value(windows: &[(String, Vec<(u32, usize)>)]) -> String {
    windows
        .iter()
        .map(|(t, pairs)| format!("{t}:{}", pairs.iter().map(|(w, i)| format!("{w}={i}")).collect::<Vec<_>>().join(",")))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse_buried(value: &str) -> std::collections::HashMap<String, Vec<(u32, usize)>> {
    let text = crate::tmux_windows::decode_option(value, "b_");
    let mut out = std::collections::HashMap::new();
    for group in text.split(' ') {
        let Some((terminal, pairs)) = group.split_once(':') else { continue };
        let list: Vec<(u32, usize)> = pairs.split(',').filter_map(|p| p.split_once('=')).filter_map(|(w, i)| Some((w.parse().ok()?, i.parse().ok()?))).collect();
        out.insert(terminal.to_string(), list);
    }
    out
}

/// A tab color as `@tab_colors` writes it: `#rrggbb`, or `none`.
pub fn hex_string(color: Option<(u8, u8, u8)>) -> String {
    match color {
        Some((r, g, b)) => format!("#{r:02x}{g:02x}{b:02x}"),
        None => "none".into(),
    }
}

/// `@tab_colors` (plain): `pane=color` joined by spaces.
pub fn tab_colors_value(panes: &[(u32, Option<(u8, u8, u8)>)]) -> String {
    panes.iter().map(|(p, c)| format!("{p}={}", hex_string(*c))).collect::<Vec<_>>().join(" ")
}

/// `@tab_colors` back: each pane's color (None: `none`), and whether iTerm2's pure red
/// (`p3#ffff00000000`) was found.
pub fn parse_tab_colors(value: &str) -> (std::collections::HashMap<u32, Option<(u8, u8, u8)>>, bool) {
    let text = crate::tmux_windows::decode_option(value, "t_");
    let mut out = std::collections::HashMap::new();
    let mut red = false;
    for part in text.split(' ') {
        let Some((pane, color)) = part.split_once('=').filter(|(p, c)| !p.is_empty() && !c.is_empty()) else { continue };
        let Ok(pane) = pane.parse::<u32>() else { continue };
        red |= color == "p3#ffff00000000";
        out.insert(pane, parse_color(color));
    }
    (out, red)
}

/// `#rrggbb` or iTerm2's Display P3 `p3#rrrrggggbbbb` (16 bits a channel, shown as sRGB).
fn parse_color(text: &str) -> Option<(u8, u8, u8)> {
    let (hex, wide) = match text.strip_prefix("p3#") {
        Some(h) => (h, true),
        None => (text.strip_prefix('#')?, false),
    };
    let n = if wide { 4 } else { 2 };
    if hex.len() != n * 3 || !hex.is_ascii() {
        return None;
    }
    let ch = |i: usize| -> Option<u8> {
        let v = u32::from_str_radix(&hex[i * n..(i + 1) * n], 16).ok()?;
        Some(if wide { (v as f64 / 65535.0 * 255.0).round() as u8 } else { v as u8 })
    };
    Some((ch(0)?, ch(1)?, ch(2)?))
}

/// The pure red of an early iTerm2 3.6 bug (a pane whose saved color is it).
pub const PURE_RED: (u8, u8, u8) = (255, 0, 0);

/// `set -t $S @name "<encoded>"`: the prefix and hex, with iTerm2's quote escaping first
/// where it does it (origins, buried indexes, tab colors).
pub fn option_command(session: u32, name: &str, plain: &str) -> String {
    let (prefix, escape) = match name {
        "@per_window_settings" => ("w_", false),
        "@per_tab_settings" => ("T_", false),
        "@origins" => ("o_", true),
        "@buried_indexes" => ("b_", true),
        "@tab_colors" => ("t_", true),
        "@hotkeys" => ("h_", true),
        _ => ("", false),
    };
    let text = if escape { escape_quotes(plain) } else { plain.to_string() };
    format!("set -t ${session} {name} \"{}\"", encode_option(&text, prefix))
}

/// iTerm2's stringByEscapingQuotes: backslashes double, then quotes get a backslash.
pub fn escape_quotes(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn hidden_command(session: u32, hidden: &BTreeSet<u32>) -> String {
    let list: Vec<String> = hidden.iter().map(u32::to_string).collect();
    format!("set -t ${session} @hidden \"{}\"", encode_option(&list.join(","), "i_"))
}

/// One Tandem window's tmux windows (in tab order), its name (`pty-...`), and whether it
/// is full screen.
pub struct Group {
    pub windows: Vec<u32>,
    pub terminal: String,
    pub full_screen: bool,
}

/// The affinities text: each group `w1,w2,pty-...;options`, separated by spaces.
pub fn affinities_value(groups: &[Group]) -> String {
    groups
        .iter()
        .filter(|g| !g.windows.is_empty())
        .map(|g| {
            let mut siblings: Vec<String> = g.windows.iter().map(u32::to_string).collect();
            siblings.push(g.terminal.clone());
            let options = if g.full_screen { "WindowStyle=FullScreen" } else { "" };
            format!("{};{options}", siblings.join(","))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn affinities_command(session: u32, value: &str) -> String {
    format!("set -t ${session} @affinities \"{}\"", encode_option(&escape_quotes(value), "a_"))
}

/// Closing one tmux tab while the window has others of the same session:
/// (text, buttons), buttons in iTerm2's order.
pub fn close_tab_question() -> (&'static str, [&'static str; 3]) {
    ("Kill tmux window, terminating its jobs, or hide it? Hidden windows may be restored from the tmux dashboard.", ["Hide", "Cancel", "Kill"])
}

/// Closing a window (or its last tmux tab) with `n` attached tmux tabs; None for none.
pub fn close_window_question(n: usize) -> Option<(&'static str, [&'static str; 4])> {
    let text = match n {
        0 => return None,
        1 => "Kill window and its jobs, hide window from view, or detach from tmux session?\n\nHidden windows may be restored from the tmux dashboard.",
        _ => "Kill all tmux windows and their jobs, hide windows from view, or detach from tmux session?\n\nHidden windows may be restored from the tmux dashboard.",
    };
    Some((text, ["Hide", "Detach tmux Session", "Kill", "Cancel"]))
}

/// The bar id of a window question: `gw` 0 means every tmux session in the window;
/// `whole`: the window closes too (Close Window), else only the tmux tabs (Close Tab).
pub fn close_win_id(gw: u64, whole: bool) -> String {
    format!("tmux-close-win:{gw}:{}", u8::from(whole))
}

pub fn parse_close_win(id: &str) -> Option<(u64, bool)> {
    let (gw, whole) = id.strip_prefix("tmux-close-win:")?.split_once(':')?;
    Some((gw.parse().ok()?, whole == "1"))
}

/// Attaching to a session this Tandem already shows (iTerm2's tmuxDoubleAttachForSessionGUID):
/// (heading, text, buttons); only OK when the other connection is gone.
pub fn attach_warning(other: bool) -> (&'static str, &'static str, Vec<&'static str>) {
    let buttons = if other { vec!["OK", "Reveal", "Force Detach Other"] } else { vec!["OK"] };
    ("Cannot Attach", "This instance of Tandem is already attached to this session", buttons)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux_windows::{Affinities, decode_option, hidden};

    #[test]
    fn options_encode_round_trip() {
        assert_eq!(encode_option("1,5", "i_"), "i_312c35");
        assert_eq!(decode_option(&encode_option("h\u{e9}", "o_"), "o_"), "h\u{e9}");
        let set: BTreeSet<u32> = [5, 1].into();
        let cmd = hidden_command(2, &set);
        assert_eq!(cmd, "set -t $2 @hidden \"i_312c35\"");
        let value = cmd.split('"').nth(1).unwrap();
        assert_eq!(hidden(value), set, "8b reads what 8c writes");
        let groups = [Group { windows: vec![1, 2], terminal: "pty-A".into(), full_screen: false }, Group { windows: vec![3], terminal: "pty-B".into(), full_screen: true }, Group { windows: vec![], terminal: "pty-C".into(), full_screen: false }];
        let v = affinities_value(&groups);
        assert_eq!(v, "1,2,pty-A; 3,pty-B;WindowStyle=FullScreen");
        let cmd = affinities_command(0, &v);
        let a = Affinities::parse(cmd.split('"').nth(1).unwrap());
        assert!(a.class_of("1").contains("2") && a.class_of("3").contains("pty-B"));
        assert_eq!(escape_quotes("a\"b\\"), "a\\\"b\\\\");
    }

    #[test]
    fn close_bars_carry_their_scope() {
        assert_eq!(parse_close_win(&close_win_id(7, false)), Some((7, false)), "a tab's close closes only the tab");
        assert_eq!(parse_close_win(&close_win_id(0, true)), Some((0, true)), "a window's close: every session, the whole window");
        assert_eq!(parse_close_win("tmux-close-win:x"), None);
        assert_eq!(parse_close_win("other"), None);
    }

    #[test]
    fn close_questions_follow_iterm2() {
        assert_eq!(close_tab_question().1, ["Hide", "Cancel", "Kill"]);
        assert!(close_tab_question().0.starts_with("Kill tmux window, terminating its jobs, or hide it?"));
        assert_eq!(close_window_question(0), None);
        assert!(close_window_question(1).unwrap().0.starts_with("Kill window and its jobs"));
        assert!(close_window_question(2).unwrap().0.starts_with("Kill all tmux windows"));
        assert_eq!(close_window_question(1).unwrap().1, ["Hide", "Detach tmux Session", "Kill", "Cancel"]);
    }

    #[test]
    fn user_vars_round_trip() {
        let mut vars = std::collections::BTreeMap::new();
        vars.insert("user.a".to_string(), "x=y".to_string());
        vars.insert("user.b".to_string(), "\u{e9}".to_string());
        let v = user_vars_value(&vars);
        assert_eq!(v, format!("u_{}", "user.a=x=y\0user.b=\u{e9}".bytes().map(|b| format!("{b:02x}")).collect::<String>()));
        assert_eq!(parse_user_vars(&v), vars);
        assert_eq!(parse_user_vars("user.c=1\0junk")["user.c"], "1", "plain text, a part without = skipped");
        assert_eq!(parse_user_vars("user.c=1\0junk").len(), 1);
        assert_eq!(parse_user_vars(&format!("{v}f")), vars, "an odd trailing digit is ignored");
        let mut nul = std::collections::BTreeMap::new();
        nul.insert("user.n".to_string(), "a\0b".to_string());
        assert_eq!(parse_user_vars(&user_vars_value(&nul))["user.n"], "a", "a value ends at its NUL");
        assert_eq!(user_vars_command(3, "u_61"), "set -p -t %3 @uservars \"u_61\"");
        assert!(parse_user_vars("").is_empty());
    }

    fn hexed(prefix: &str, text: &str) -> String {
        encode_option(text, prefix)
    }

    #[test]
    fn per_window_settings_round_trip() {
        let v = per_window_value(&[("pty-1".into(), WindowSettings { size: Some(WinSize::Size(800, 600)) }), ("pty-2".into(), WindowSettings { size: Some(WinSize::FullScreen) })]);
        assert_eq!(v, "pty-1:size={800, 600};pty-2:size=fs");
        let m = parse_per_window(&hexed("w_", &v));
        assert_eq!(m["pty-1"].size, Some(WinSize::Size(800, 600)));
        assert_eq!(m["pty-2"].size, Some(WinSize::FullScreen));
        let iterm = parse_per_window("ABCD-1:toolbelt=1&size={1024.5, 700}");
        assert_eq!(iterm["ABCD-1"].size, Some(WinSize::Size(1024, 700)), "iTerm2's text with a fraction and a toolbelt");
        assert!(parse_per_window("").is_empty());
        assert_eq!(option_command(2, "@per_window_settings", "a\"b"), format!("set -t $2 @per_window_settings \"{}\"", hexed("w_", "a\"b")), "no quote escaping here");
    }

    #[test]
    fn per_tab_settings_round_trip() {
        let v = per_tab_value(&[(3, "logs".into()), (5, "a;b".into())]);
        assert_eq!(v, "3:t=bG9ncw==;5:t=YTti");
        let m = parse_per_tab(&hexed("T_", &v));
        assert_eq!((m[&3].as_str(), m[&5].as_str()), ("logs", "a;b"));
        assert!(parse_per_tab("3:t=").is_empty(), "an empty title is skipped");
    }

    #[test]
    fn origins_decode_like_iterm2() {
        let v = origins_value(&[(vec![1, 2], (100, -20)), (vec![3], (0, 0))]);
        assert_eq!(v, "1,2:100,-20 3:0,0");
        assert_eq!(option_command(1, "@origins", &v), format!("set -t $1 @origins \"{}\"", hexed("o_", &escape_quotes(&v))));
        let m = parse_origins(&hexed("o_", &v));
        assert_eq!((m[&1], m[&2], m[&3]), ((100, -20), (100, -20), (0, 0)));
        assert!(parse_origins("1:2 4:5,6,7").is_empty(), "malformed groups are skipped");
    }

    #[test]
    fn buried_indexes_round_trip() {
        let v = buried_value(&[("pty-1".into(), vec![(4, 0), (7, 2)])]);
        assert_eq!(v, "pty-1:4=0,7=2");
        let m = parse_buried(&hexed("b_", &escape_quotes(&v)));
        assert_eq!(m["pty-1"], vec![(4, 0), (7, 2)]);
        assert_eq!(parse_buried("pty-1:4=x,5=1")["pty-1"], vec![(5, 1)], "both numbers are needed");
    }

    #[test]
    fn tab_colors_decode_like_iterm2() {
        assert_eq!(hex_string(Some((0x12, 0xab, 0x00))), "#12ab00");
        assert_eq!(hex_string(None), "none");
        let v = tab_colors_value(&[(1, Some((255, 128, 0))), (2, None)]);
        assert_eq!(v, "1=#ff8000 2=none");
        let (m, red) = parse_tab_colors(&hexed("t_", &escape_quotes(&v)));
        assert_eq!((m[&1], m[&2], red), (Some((255, 128, 0)), None, false));
        let (m, red) = parse_tab_colors("3=p3#ffff00000000 4=p3#80008000ffff");
        assert!(red, "iTerm2's red bug");
        assert_eq!((m[&3], m[&4]), (Some(PURE_RED), Some((128, 128, 255))));
    }
}
