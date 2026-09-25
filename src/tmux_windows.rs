//! Opening tmux windows (iTerm2's TmuxController openWindowsOfSize and
//! initialListWindowsResponse, and TmuxWindowOpener): the attach list, list-windows rows,
//! the session options iTerm2 keeps (hex with a prefix), hidden windows, affinities, which
//! windows open, where each one goes, and the per-pane capture list.

use std::collections::BTreeSet;

/// iTerm2's openWindowsOfSize list for session `$session`, in its order. The size is the
/// client size (at least 2x2; one row more on tmux 2.9 and 2.9a with the status bar).
pub fn open_list(session: u32, w: u32, h: u32, detailed_format: &str) -> Vec<String> {
    let s = session;
    vec![
        format!("show -v -q -t ${s} @iterm2_id"),
        format!("refresh-client -C {},{}", w.max(2), h.max(2)),
        format!("show -v -q -t ${s} @hidden"),
        format!("show -v -q -t ${s} @buried_indexes"),
        format!("show -v -q -t ${s} @affinities"),
        format!("show -v -q -t ${s} @per_window_settings"),
        format!("show -v -q -t ${s} @per_tab_settings"),
        format!("show -v -q -t ${s} @origins"),
        format!("show -v -q -t ${s} @hotkeys"),
        format!("show -v -q -t ${s} @tab_colors"),
        "list-sessions -F \"#{session_id} #{session_name}\"".into(),
        format!("list-windows -F {detailed_format}"),
    ]
}

/// The height sent for the client: tmux 2.9 and 2.9a count the status bar inside it.
pub fn client_height(h: u32, status_bar: bool, version: Option<f64>) -> u32 {
    let buggy = version.is_some_and(|v| v == 2.9 || v == 2.91);
    if buggy && status_bar { h + 1 } else { h }
}

/// iTerm2's listWindowsDetailedFormat: tab-joined, in double quotes.
pub fn detailed_format(version: f64) -> String {
    let mut parts = vec!["#{session_name}", "#{window_id}", "#{window_name}", "#{window_width}", "#{window_height}", "#{window_layout}", "#{window_flags}", "#{?window_active,1,0}"];
    if version >= 2.2 {
        parts.push("#{window_visible_layout}");
    }
    if version >= 2.4 {
        parts.push("#{pane-border-status}");
    }
    format!("\"{}\"", parts.join("\t"))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    pub session: String,
    pub id: u32,
    pub name: String,
    pub w: u32,
    pub h: u32,
    pub layout: String,
    pub flags: String,
    pub active: bool,
    pub visible_layout: Option<String>,
    pub border_status: Option<String>,
}

impl Window {
    /// tmux flags a zoomed window with Z; iTerm2 unzooms it on open.
    pub fn zoomed(&self) -> bool {
        self.flags.contains('Z')
    }
}

/// `@12` is 12 (iTerm2's windowIdFromString reads the digits after `@`).
pub fn window_id(s: &str) -> u32 {
    s.strip_prefix('@').unwrap_or(s).bytes().take_while(u8::is_ascii_digit).fold(0u32, |n, d| n.saturating_mul(10).saturating_add((d - b'0') as u32))
}

/// tmux writes `\` in window names as `\\`.
pub fn unescape_name(s: &str) -> String {
    s.replace("\\\\", "\\")
}

/// The detailed list-windows reply (iTerm2's TSVParser): rows with fewer fields than the
/// version asks for are dropped; `tab_bug` splits on a literal `\t`.
pub fn parse_windows(reply: &str, version: f64, tab_bug: bool) -> Vec<Window> {
    let fields = 8 + usize::from(version >= 2.2) + usize::from(version >= 2.4);
    let int = |s: &str| s.trim().parse::<u32>().unwrap_or(0);
    reply
        .split('\n')
        .filter_map(|row| {
            let mut f: Vec<&str> = row.split('\t').collect();
            if tab_bug && f.len() == 1 {
                f = row.split("\\t").collect();
            }
            if f.len() < fields {
                return None;
            }
            Some(Window {
                session: f[0].into(),
                id: window_id(f[1]),
                name: unescape_name(f[2]),
                w: int(f[3]),
                h: int(f[4]),
                layout: f[5].into(),
                flags: f[6].into(),
                active: f[7] == "1",
                visible_layout: (fields > 8).then(|| f[8].to_string()),
                border_status: (fields > 9).then(|| f[9].to_string()),
            })
        })
        .collect()
}

/// A session option iTerm2 wrote: `prefix` then hex of UTF-8; without the prefix, plain.
pub fn decode_option(value: &str, prefix: &str) -> String {
    let Some(hex) = value.strip_prefix(prefix) else { return value.to_string() };
    let bytes: Vec<u8> = hex.as_bytes().chunks(2).filter_map(|p| std::str::from_utf8(p).ok().and_then(|p| u8::from_str_radix(p, 16).ok())).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `@hidden`: window ids joined by commas.
pub fn hidden(value: &str) -> BTreeSet<u32> {
    let v = decode_option(value, "i_");
    if v.is_empty() {
        return BTreeSet::new();
    }
    v.split(',').map(|w| w.trim().bytes().take_while(u8::is_ascii_digit).fold(0u32, |n, d| n * 10 + (d - b'0') as u32)).collect()
}

/// iTerm2's affinities: equivalence classes of window ids (and `pty-GUID` names of native
/// windows), one class per space-separated group, `;options` after it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Affinities {
    classes: Vec<BTreeSet<String>>,
}

impl Affinities {
    pub fn parse(value: &str) -> Affinities {
        let mut a = Affinities::default();
        let v = decode_option(value, "a_");
        if v.is_empty() {
            return a;
        }
        for group in v.split(' ') {
            let siblings = group.split(';').next().unwrap_or("");
            let members: Vec<&str> = siblings.split(',').collect();
            let Some(last) = members.last() else { continue };
            // A window alone in its native window gets a placeholder partner, so its class
            // exists and the rule for unrecognized windows does not apply to it.
            let exemplar = if members.len() == 1 { format!("{last}_ph") } else { last.to_string() };
            for m in &members {
                if *m != exemplar {
                    a.set_equal(m, &exemplar);
                }
            }
        }
        a
    }

    pub fn set_equal(&mut self, x: &str, y: &str) {
        let ix = self.classes.iter().position(|c| c.contains(x));
        let iy = self.classes.iter().position(|c| c.contains(y));
        match (ix, iy) {
            (Some(i), Some(j)) if i == j => {}
            (Some(i), Some(j)) => {
                let moved = self.classes.remove(i.max(j));
                self.classes[i.min(j)].extend(moved);
            }
            (Some(i), None) => {
                self.classes[i].insert(y.into());
            }
            (None, Some(j)) => {
                self.classes[j].insert(x.into());
            }
            (None, None) => self.classes.push([x.to_string(), y.to_string()].into()),
        }
    }

    /// The class of `x`, `x` included; empty when it has none.
    pub fn class_of(&self, x: &str) -> BTreeSet<String> {
        self.classes.iter().find(|c| c.contains(x)).cloned().unwrap_or_default()
    }
}

/// `OpenTmuxWindowsIn`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenIn {
    NativeWindows,
    TabsInNewWindow,
    TabsInAttachingWindow,
}

impl OpenIn {
    pub fn from_pref(n: i64) -> OpenIn {
        match n {
            1 => OpenIn::TabsInNewWindow,
            2 => OpenIn::TabsInAttachingWindow,
            _ => OpenIn::NativeWindows,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Initial {
    pub open: Vec<Window>,
    /// Windows hidden because there were too many (iTerm2 saves them in `@hidden`).
    pub hide: Vec<u32>,
    /// A notification: (title, text).
    pub notify: Option<(&'static str, &'static str)>,
}

/// iTerm2's initialListWindowsResponse: skip windows saved hidden; with "tabs in a new
/// window" every window no class names joins one class; more than `limit` opens none.
pub fn initial(windows: Vec<Window>, hidden: &BTreeSet<u32>, aff: &mut Affinities, open_in: OpenIn, limit: i64) -> Initial {
    let mut open = Vec::new();
    let mut have_hidden = false;
    let mut new_class: Option<u32> = None;
    for w in windows {
        if hidden.contains(&w.id) {
            have_hidden = true;
            continue;
        }
        let id = w.id.to_string();
        if aff.class_of(&id).is_empty() && open_in == OpenIn::TabsInNewWindow {
            match new_class {
                None => new_class = Some(w.id),
                Some(first) => aff.set_equal(&id, &first.to_string()),
            }
        }
        open.push(w);
    }
    let text = "Use the tmux dashboard to select which to open.";
    if open.len() as i64 > limit {
        let hide = open.iter().map(|w| w.id).collect();
        return Initial { open: Vec::new(), hide, notify: Some(("Too many tmux windows!", text)) };
    }
    let notify = have_hidden.then_some(("Some tmux windows were hidden.", text));
    Initial { open, hide: Vec::new(), notify }
}

/// Where a tmux window opens (research 3.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// As a tab of the Tandem window that shows this tmux window.
    WithSibling(u32),
    /// As a tab of the window that holds the gateway.
    GatewayWindow,
    /// As a tab of the focused window, if it has no tmux tabs of another session.
    Focused,
    /// As a tab of the frontmost window showing this session.
    Topmost,
    NewWindow,
}

/// `open`: the tmux windows already showing; `prevent_tab`: the profile's "Prevent Opening
/// in a Tab"; `anonymous`: made by `tmux new-window`, not by Tandem;
/// `anonymous_location`: anonymousTmuxWindowsOpenInCurrentWindow (0 new, 1 focused, 2 topmost).
pub fn place(window: u32, aff: &Affinities, open: &[u32], prevent_tab: bool, initial: bool, anonymous: bool, open_in: OpenIn, anonymous_location: i64) -> Place {
    if !prevent_tab {
        let class = aff.class_of(&window.to_string());
        if let Some(sib) = open.iter().find(|o| **o != window && class.contains(&o.to_string())) {
            return Place::WithSibling(*sib);
        }
    }
    if initial && open_in == OpenIn::TabsInAttachingWindow {
        return Place::GatewayWindow;
    }
    if !initial && anonymous {
        match anonymous_location {
            1 => return Place::Focused,
            2 => return Place::Topmost,
            _ => {}
        }
    }
    Place::NewWindow
}

/// iTerm2's preferredTmuxClientName: user@host from shell integration, else the session's
/// title, else "tmux".
pub fn client_name(user: Option<&str>, host: Option<&str>, title: &str) -> String {
    if let (Some(u), Some(h)) = (user, host) {
        return format!("{u}@{h}");
    }
    match title.trim() {
        "" => "tmux".into(),
        t => t.into(),
    }
}

/// The gateway session's name (iTerm2's formattedName for a gateway).
pub fn gateway_title(prefix: &str, base: &str, client: &str) -> String {
    format!("[{prefix}{base} {client}]")
}

/// A window whose current session is a tmux pane: ` [client]` after the title, only when
/// the window number shows (iTerm2's PseudoTerminal title rule).
pub fn window_title(title: &str, show_number: bool, include_client: bool, client: Option<&str>) -> String {
    match client {
        Some(c) if show_number && include_client => format!("{title} [{c}]"),
        _ => title.to_string(),
    }
}

/// A tmux pane took focus in Tandem (iTerm2's selectPane): none on exactly tmux 2.9.
pub fn select_pane_command(pane: u32, min: Option<f64>) -> Option<String> {
    (min != Some(2.9)).then(|| format!("select-pane -t \"%{pane}\""))
}

/// The history kept for a pane: the client's rows or the profile's scrollback, whichever is
/// more; unlimited scrollback is 10,000,000.
pub fn max_history(rows: u32, scrollback: i64, unlimited: bool) -> u32 {
    let lines = if unlimited { 10_000_000 } else { scrollback.clamp(0, 10_000_000) as u32 };
    rows.max(lines)
}

/// One pane's attach commands (TmuxWindowOpener appendRequestsForWindowPane), in order.
pub fn capture_list(pane: u32, max_history: u32, version: Option<f64>, pause_mode: bool, state_format: &str) -> Vec<String> {
    let n = if version.is_some_and(|v| v >= 3.1) { "N" } else { "" };
    let mut out = vec![
        format!("capture-pane -peqJ{n} -t \"%{pane}\" -S -{max_history}"),
        format!("capture-pane -peqJ{n} -a -t \"%{pane}\" -S -{max_history}"),
        format!("list-panes -t \"%{pane}\" -F \"{state_format}\""),
        format!("capture-pane -p -P -C -t \"%{pane}\""),
    ];
    if pause_mode {
        out.push(format!("refresh-client -A '%{pane}:continue'"));
    }
    if version.is_some_and(|v| v >= 3.1) {
        out.push(format!("show-options -v -q -p -t %{pane} @uservars"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_list_follows_iterm2() {
        let f = detailed_format(3.4);
        assert_eq!(f, "\"#{session_name}\t#{window_id}\t#{window_name}\t#{window_width}\t#{window_height}\t#{window_layout}\t#{window_flags}\t#{?window_active,1,0}\t#{window_visible_layout}\t#{pane-border-status}\"");
        assert!(!detailed_format(2.1).contains("visible_layout"));
        assert!(detailed_format(2.3).contains("visible_layout") && !detailed_format(2.3).contains("border"));
        let l = open_list(3, 1, 30, &f);
        assert_eq!(l[0], "show -v -q -t $3 @iterm2_id");
        assert_eq!(l[1], "refresh-client -C 2,30", "at least 2x2");
        assert_eq!(&l[2..10], ["show -v -q -t $3 @hidden", "show -v -q -t $3 @buried_indexes", "show -v -q -t $3 @affinities", "show -v -q -t $3 @per_window_settings", "show -v -q -t $3 @per_tab_settings", "show -v -q -t $3 @origins", "show -v -q -t $3 @hotkeys", "show -v -q -t $3 @tab_colors"]);
        assert_eq!(l[10], "list-sessions -F \"#{session_id} #{session_name}\"");
        assert_eq!(l[11], format!("list-windows -F {f}"));
        assert_eq!((client_height(24, true, Some(2.91)), client_height(24, true, Some(3.4)), client_height(24, false, Some(2.9))), (25, 24, 24));
    }

    #[test]
    fn windows_parse_with_the_tab_bug() {
        let row = "main\t@3\tvi\\\\m\t80\t24\td67e,80x24,0,0,1\t*Z\t1\td67e,80x24,0,0,1\toff";
        let w = &parse_windows(&format!("{row}\nshort\t@4"), 3.4, false);
        assert_eq!(w.len(), 1, "short rows are dropped");
        assert_eq!((w[0].id, w[0].name.as_str(), w[0].w, w[0].h, w[0].active, w[0].zoomed()), (3, "vi\\m", 80, 24, true, true));
        assert_eq!(w[0].border_status.as_deref(), Some("off"));
        let bugged = row.replace('\t', "\\t");
        assert_eq!(parse_windows(&bugged, 3.4, true).len(), 1);
        assert!(parse_windows(&bugged, 3.4, false).is_empty());
        assert_eq!(parse_windows(row, 2.1, false)[0].visible_layout, None, "older formats have 8 fields");
        assert_eq!(window_id("@12"), 12);
    }

    #[test]
    fn options_decode() {
        assert_eq!(decode_option("i_312c35", "i_"), "1,5");
        assert_eq!(decode_option("1,5", "i_"), "1,5", "plain text without the prefix");
        assert_eq!(hidden("i_312c35"), [1, 5].into());
        assert!(hidden("").is_empty());
        let a = Affinities::parse("a_312c322033");
        assert_eq!(a.class_of("1"), ["1".to_string(), "2".into()].into());
        assert_eq!(a.class_of("3"), ["3".to_string(), "3_ph".into()].into(), "alone: a placeholder partner");
        let a = Affinities::parse("4,pty-ABC;WindowStyle=FullScreen");
        assert!(a.class_of("4").contains("pty-ABC"));
        assert!(Affinities::parse("").class_of("1").is_empty());
    }

    fn win(id: u32) -> Window {
        Window { id, ..Default::default() }
    }

    #[test]
    fn too_many_windows_open_none() {
        let windows: Vec<Window> = (0..11).map(win).collect();
        let r = initial(windows.clone(), &BTreeSet::new(), &mut Affinities::default(), OpenIn::NativeWindows, 10);
        assert!(r.open.is_empty());
        assert_eq!(r.hide.len(), 11);
        assert_eq!(r.notify, Some(("Too many tmux windows!", "Use the tmux dashboard to select which to open.")));
        let r = initial(windows[..10].to_vec(), &BTreeSet::new(), &mut Affinities::default(), OpenIn::NativeWindows, 10);
        assert_eq!((r.open.len(), r.notify), (10, None), "the limit itself opens");
        let r = initial(windows[..3].to_vec(), &[1].into(), &mut Affinities::default(), OpenIn::NativeWindows, 10);
        assert_eq!(r.open.iter().map(|w| w.id).collect::<Vec<_>>(), vec![0, 2]);
        assert_eq!(r.notify.map(|n| n.0), Some("Some tmux windows were hidden."));
    }

    #[test]
    fn placement_follows_iterm2() {
        let mut aff = Affinities::default();
        let r = initial((0..3).map(win).collect(), &BTreeSet::new(), &mut aff, OpenIn::TabsInNewWindow, 10);
        assert_eq!(r.open.len(), 3);
        assert_eq!(place(0, &aff, &[], false, true, false, OpenIn::TabsInNewWindow, 1), Place::NewWindow);
        assert_eq!(place(1, &aff, &[0], false, true, false, OpenIn::TabsInNewWindow, 1), Place::WithSibling(0), "unrecognized windows share one window");
        assert_eq!(place(1, &aff, &[0], true, true, false, OpenIn::TabsInNewWindow, 1), Place::NewWindow, "Prevent Opening in a Tab");
        let none = Affinities::default();
        assert_eq!(place(5, &none, &[0], false, true, false, OpenIn::NativeWindows, 1), Place::NewWindow);
        assert_eq!(place(5, &none, &[0], false, true, false, OpenIn::TabsInAttachingWindow, 1), Place::GatewayWindow);
        assert_eq!(place(5, &none, &[0], false, false, true, OpenIn::NativeWindows, 1), Place::Focused);
        assert_eq!(place(5, &none, &[0], false, false, true, OpenIn::NativeWindows, 2), Place::Topmost);
        assert_eq!(place(5, &none, &[0], false, false, true, OpenIn::NativeWindows, 0), Place::NewWindow);
        assert_eq!(place(5, &none, &[0], false, false, false, OpenIn::NativeWindows, 1), Place::NewWindow, "Tandem made it");
    }

    #[test]
    fn titles_follow_iterm2() {
        assert_eq!(client_name(Some("bob"), Some("box"), "zsh"), "bob@box");
        assert_eq!(client_name(None, Some("box"), "  vim  "), "vim", "no shell integration: the session title");
        assert_eq!(client_name(None, None, " "), "tmux");
        assert_eq!(gateway_title("\u{21a3} ", "tmux", "bob@box"), "[\u{21a3} tmux bob@box]");
        assert_eq!(window_title("1. \u{21a3} vi", true, true, Some("bob@box")), "1. \u{21a3} vi [bob@box]");
        assert_eq!(window_title("\u{21a3} vi", false, true, Some("bob@box")), "\u{21a3} vi", "only with the window number shown, as iTerm2");
        assert_eq!(window_title("1. vi", true, false, Some("bob@box")), "1. vi");
        assert_eq!(window_title("1. vi", true, true, None), "1. vi");
    }

    #[test]
    fn select_pane_skips_2_9() {
        assert_eq!(select_pane_command(4, Some(3.4)).as_deref(), Some("select-pane -t \"%4\""));
        assert_eq!(select_pane_command(4, Some(2.9)), None, "tmux 2.9 misbehaved");
        assert!(select_pane_command(4, Some(2.91)).is_some());
    }

    #[test]
    fn capture_list_by_version() {
        let l = capture_list(7, 1000, Some(3.4), false, "F");
        assert_eq!(l, ["capture-pane -peqJN -t \"%7\" -S -1000", "capture-pane -peqJN -a -t \"%7\" -S -1000", "list-panes -t \"%7\" -F \"F\"", "capture-pane -p -P -C -t \"%7\"", "show-options -v -q -p -t %7 @uservars"]);
        let l = capture_list(7, 24, Some(2.9), true, "F");
        assert_eq!(l[0], "capture-pane -peqJ -t \"%7\" -S -24");
        assert_eq!(l[4], "refresh-client -A '%7:continue'");
        assert_eq!(l.len(), 5);
        assert_eq!((max_history(24, 1000, false), max_history(50, 10, false), max_history(24, 0, true)), (1000, 50, 10_000_000));
    }
}
