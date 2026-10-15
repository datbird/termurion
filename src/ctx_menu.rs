//! The terminal's right-click menu as data: iTerm2's `menuAtCoord:` and `contextMenuWithEvent:`
//! (`sources/ContextMenu/iTermTextViewContextMenuHelper.m`, "CMH"), the window's additions
//! (`PseudoTerminal menuForEvent:menu:`, "PT") and the session's (`PTYSession
//! textViewAddContextMenuItems:`, "PS"). Pure code: `app::semantic_ui` fills a `Ctx` from the
//! pane, shows the rows and runs the picks. The order, the separators and the conditions are
//! iTerm2's; the titles are iTerm2's own (`…` and all). iTerm2 shows no shortcut text here.
//!
//! Ids are `CTX_ID` followed by an action name (`cmd.<menu id>` runs a menu command on the
//! menu's pane).

use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor};

/// Menu item ids of the right-click menu's own items.
pub const CTX_ID: &str = "termurion.ctx.";

/// One row of the menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Item { id: String, label: String, enabled: bool, checked: Option<bool>, swatch: Option<(u8, u8, u8)> },
    Sub { label: String, rows: Vec<Row> },
    Sep,
}

/// A plain item.
pub fn item(action: &str, label: &str, enabled: bool) -> Row {
    Row::Item { id: format!("{CTX_ID}{action}"), label: label.to_string(), enabled, checked: None, swatch: None }
}

/// A check item.
pub fn check(action: &str, label: &str, enabled: bool, on: bool) -> Row {
    Row::Item { id: format!("{CTX_ID}{action}"), label: label.to_string(), enabled, checked: Some(on), swatch: None }
}

/// A menu command (by menu item id) run on the menu's pane, shown under `label`.
pub fn command(menu_id: &str, label: &str, enabled: bool) -> Row {
    item(&format!("cmd.{menu_id}"), label, enabled)
}

/// What an inline image offers (`CMH:385-420`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImageFacts {
    pub broken: bool,
    pub animated: bool,
    pub paused: bool,
}

/// The fold item on the clicked line (`CMH:244-267`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Fold {
    #[default]
    None,
    Fold,
    Unfold,
}

/// What a replacement does (`SelectionReplacement.swift:Kind`, in iTerm2's order).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Replace {
    Json,
    Base64Decode,
    Base64Encode,
}

/// The selection helper rows in the order the menu shows them (`CMH:434-505`), whatever a row is.
#[derive(Clone, Debug, PartialEq)]
pub struct Groups<T> {
    pub synonyms: Vec<T>,
    pub color: Option<T>,
    pub preview: Option<T>,
    pub numeric: Vec<T>,
    pub copy: Vec<T>,
}

impl<T> Default for Groups<T> {
    fn default() -> Self {
        Groups { synonyms: Vec::new(), color: None, preview: None, numeric: Vec::new(), copy: Vec::new() }
    }
}

impl<T> Groups<T> {
    /// Every row, in the order the menu numbers them (`helper.<n>`).
    pub fn into_flat(self) -> Vec<T> {
        let mut v = self.synonyms;
        v.extend(self.color);
        v.extend(self.preview);
        v.extend(self.numeric);
        v.extend(self.copy);
        v
    }

    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Groups<U> {
        Groups { synonyms: self.synonyms.into_iter().map(&mut f).collect(), color: self.color.map(&mut f), preview: self.preview.map(&mut f), numeric: self.numeric.into_iter().map(&mut f).collect(), copy: self.copy.into_iter().map(&mut f).collect() }
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        self.synonyms.retain(&mut keep);
        self.numeric.retain(&mut keep);
        self.copy.retain(&mut keep);
        if self.color.as_ref().is_some_and(|c| !keep(c)) {
            self.color = None;
        }
        if self.preview.as_ref().is_some_and(|c| !keep(c)) {
            self.preview = None;
        }
    }
}

/// A helper row: its label, whether it copies anything, and a swatch.
#[derive(Clone, Debug, PartialEq)]
pub struct HelperRow {
    pub label: String,
    pub enabled: bool,
    pub swatch: Option<(u8, u8, u8)>,
}

/// Everything the menu depends on.
#[derive(Clone, Debug, Default)]
pub struct Ctx {
    /// There is a click point (false for the title bar's menu, `titleBarMenu`).
    pub at_point: bool,
    /// The mark on the clicked line has a command (`Command Info`, `CMH:224-242`).
    pub command_info: bool,
    /// The mark on the clicked line is named.
    pub mark_name: Option<String>,
    pub fold: Fold,
    /// The Profiles submenus of New Window and New Tab, and the tabs of Select (empty: none).
    pub new_window: Vec<Row>,
    pub new_tab: Vec<Row>,
    pub select: Vec<Row>,
    pub image: Option<ImageFacts>,
    /// The selection has text.
    pub has_selection: bool,
    /// The selection text when it is short (`CMH:436`).
    pub short: Option<String>,
    /// The selection is reasonable (`CMH:452`).
    pub reasonable: bool,
    pub helpers: Groups<HelperRow>,
    pub replacements: Vec<Replace>,
    /// One selected cell, described (`CMH:507-523`).
    pub char_description: Option<String>,
    /// The Download item: its title and whether it can run (`CMH:528-548`, `:271-286`).
    pub scp_title: String,
    pub scp_enabled: bool,
    /// The selection is a URL that can open here: the two split items (`CMH:551-556`).
    pub locally_openable: bool,
    pub quick_look: bool,
    /// Smart Selection actions for the selection: title and whether each can run.
    pub actions: Vec<(String, bool)>,
    /// The script providers' items: title and unique id.
    pub providers: Vec<(String, String)>,
    pub layout_locked: bool,
    pub pane_locked: bool,
    pub tab_has_many: bool,
    pub all_locked: bool,
    pub any_locked: bool,
    pub move_to_tab_ok: bool,
    pub move_to_window_ok: bool,
    pub pasteable: bool,
    /// A hyperlink under the click: Copy Link Address; else a detected URL: Copy URL.
    pub link: Link,
    /// A hidden annotation lies under the click (`Reveal Annotation`, `CMH:355-364`).
    pub annotation_hidden: bool,
    pub has_coprocess: bool,
    pub restartable: bool,
    pub can_bury: bool,
    /// The tmux pause item: None for a pane that is not in a pause-mode tmux session.
    pub tmux_paused: Option<bool>,
    pub terminal_state: TerminalState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Link {
    #[default]
    None,
    Address,
    Url,
}

/// The checks of the Terminal State submenu, by title.
#[derive(Clone, Debug, Default)]
pub struct TerminalState {
    pub on: Vec<String>,
}

impl TerminalState {
    fn is(&self, title: &str) -> bool {
        self.on.iter().any(|t| t == title)
    }
}

/// The Terminal State submenu (`CMH:736-785`): titles, separators as `""`, and the Emulation
/// Level submenu last.
pub const TERMINAL_STATE: [&str; 24] = [
    "Alternate Screen",
    "",
    "Focus Reporting",
    "Mouse Reporting",
    "Paste Bracketing",
    "",
    "Application Cursor",
    "Application Keypad",
    "",
    "Standard Key Reporting Mode",
    "modifyOtherKeys Mode 1",
    "modifyOtherKeys Mode 2",
    "CSI u Mode",
    "Raw Key Reporting Mode",
    "",
    "Disambiguate Escape",
    "Report All Event Types",
    "Report Alternate Keys",
    "Report All Keys as Escape Codes",
    "Report Associated Text",
    "",
    "Literal Controls",
    "",
    "Emulation Level",
];

/// The Emulation Level submenu (`CMH:787-810`).
pub const LEVELS: [&str; 5] = ["VT100", "VT200", "VT300", "VT400", "VT500"];

fn sep(rows: &mut Vec<Row>) {
    rows.push(Row::Sep);
}

/// The menu (`menuAtCoord:`, `CMH:376-813`, with `contextMenuWithEvent:`'s additions at the top).
pub fn build(c: &Ctx) -> Vec<Row> {
    let mut m: Vec<Row> = Vec::new();
    match c.image {
        Some(img) => image_rows(&mut m, img),
        None => main_rows(&mut m, c),
    }
    // contextMenuWithEvent: each inserts at index 0, so the last one inserted is the top (`CMH:208-267`).
    let mut top: Vec<Row> = Vec::new();
    if c.command_info {
        top.push(item("cmdinfo", "Command Info", true));
        sep(&mut top);
    }
    if let Some(name) = &c.mark_name {
        top.push(Row::Item { id: format!("{CTX_ID}markname"), label: name.clone(), enabled: false, checked: None, swatch: None });
        top.push(item("removemark", "Remove Named Mark", true));
        sep(&mut top);
    }
    match c.fold {
        Fold::Unfold => {
            top.push(item("unfold", "Unfold", true));
            sep(&mut top);
        }
        Fold::Fold => {
            top.push(item("fold", "Fold", true));
            sep(&mut top);
        }
        Fold::None => {}
    }
    top.extend(m);
    top
}

/// `CMH:385-427`.
fn image_rows(m: &mut Vec<Row>, img: ImageFacts) {
    let (save, copy, open) = if img.broken { ("Save File As\u{2026}", "Copy File", "Open File") } else { ("Save Image As\u{2026}", "Copy Image", "Open Image") };
    m.push(item("img.save", save, true));
    m.push(item("img.copy", copy, true));
    m.push(item("img.open", open, true));
    m.push(item("img.inspect", "Inspect", true));
    if img.animated || img.paused {
        m.push(item("img.animation", if img.paused { "Resume Animating" } else { "Stop Animating" }, true));
    }
}

fn main_rows(m: &mut Vec<Row>, c: &Ctx) {
    // `PT:7200-7266` (via `amend:`, `CMH:728`) puts New Window, New Tab, Select and a line at the top.
    m.push(Row::Sub { label: "New Window".into(), rows: c.new_window.clone() });
    m.push(Row::Sub { label: "New Tab".into(), rows: c.new_tab.clone() });
    if !c.select.is_empty() {
        m.push(Row::Sub { label: "Select".into(), rows: c.select.clone() });
    }
    sep(m);
    let sel = c.has_selection;
    // CMH:437-505: the helpers of a short selection, then of a reasonable one.
    let mut need_sep = false;
    let mut index = 0;
    let mut helper = |m: &mut Vec<Row>, h: &HelperRow| {
        m.push(Row::Item { id: format!("{CTX_ID}helper.{index}"), label: h.label.clone(), enabled: h.enabled, checked: None, swatch: h.swatch });
        index += 1;
    };
    if c.short.is_some() {
        for h in &c.helpers.synonyms {
            helper(m, h);
        }
        need_sep |= !c.helpers.synonyms.is_empty();
        if let Some(h) = &c.helpers.color {
            helper(m, h);
            need_sep = true;
        }
        if let Some(h) = &c.helpers.preview {
            helper(m, h);
            need_sep = true;
        }
    }
    if c.reasonable {
        for h in &c.helpers.numeric {
            helper(m, h);
        }
        // addMenuItemsToCopyBase64 always adds Copy Base64-Encoded, so the line is always needed (`CMH:464-470`).
        for h in &c.helpers.copy {
            helper(m, h);
        }
        need_sep = true;
        if !c.replacements.is_empty() {
            sep(m);
        }
        for r in &c.replacements {
            let (action, label) = match r {
                Replace::Json => ("Edit > Replace Selection > Replace with Pretty-Printed JSON", "Replace with Pretty-Printed JSON"),
                Replace::Base64Decode => ("Edit > Replace Selection > Replace with Base 64-Decoded Value", "Replace with Base64-Decoded Value"),
                Replace::Base64Encode => ("Edit > Replace Selection > Replace with Base 64-Encoded Value", "Replace with Base64-Encoded Value"),
            };
            m.push(command(action, label, true));
            need_sep = true;
        }
    }
    if need_sep {
        sep(m);
    }
    // CMH:507-523.
    if let Some(d) = &c.char_description {
        m.push(Row::Item { id: format!("{CTX_ID}chardesc"), label: d.clone(), enabled: false, checked: None, swatch: None });
    }
    // CMH:528-563.
    m.push(item("scp", &c.scp_title, c.scp_enabled));
    if c.short.is_some() {
        m.push(item("openurl", "Open Selection as URL", sel));
        if c.locally_openable {
            m.push(item("openurlv", "Open URL in Vertical Split Pane", sel && !c.layout_locked));
            m.push(item("openurlh", "Open URL in Horizontal Split Pane", sel && !c.layout_locked));
            sep(m);
        }
        if c.quick_look {
            m.push(item("quicklook", "Quick Look Link", sel));
        }
    }
    m.push(item("searchweb", "Search the Web for Selection", sel));
    m.push(item("mail", "Send Email to Selected Address", sel));
    m.push(command("Session > Triggers > Add Trigger...", "Add Trigger\u{2026}", sel));
    sep(m);
    // CMH:570-580.
    if !c.actions.is_empty() {
        for (i, (title, ok)) in c.actions.iter().enumerate() {
            m.push(item(&format!("action.{i}"), title, *ok));
        }
        sep(m);
    }
    if !c.providers.is_empty() {
        // addAPIProvidedMenuItems adds its own line first, then the caller adds one after.
        sep(m);
        for (title, id) in &c.providers {
            m.push(item(&format!("provider.{id}"), title, true));
        }
        sep(m);
    }
    // CMH:583-594.
    m.push(command("Shell > Split Vertically with Current Profile", "Split Pane Vertically", !c.layout_locked));
    m.push(command("Shell > Split Horizontally with Current Profile", "Split Pane Horizontally", !c.layout_locked));
    sep(m);
    // CMH:596-607.
    let may_move = !c.pane_locked && !c.layout_locked;
    m.push(command("Session > Move Session > Move Session to Split Pane", "Move Session to Split Pane", may_move));
    if c.tab_has_many {
        m.push(command("Session > Move Session > Move Session to Tab", "Move Session to Tab", c.move_to_tab_ok));
    }
    m.push(command("Session > Move Session > Move Session to Window", "Move Session to Window", c.move_to_window_ok));
    m.push(item("swappane", "Swap With Session\u{2026}", may_move));
    sep(m);
    // CMH:612-640.
    m.push(command("Edit > Copy", "Copy", sel));
    match c.link {
        Link::Address if c.at_point => m.push(item("copylink", "Copy Link Address", true)),
        Link::Url if c.at_point => m.push(item("copylink", "Copy URL", true)),
        _ => {}
    }
    m.push(command("Edit > Paste", "Paste", c.pasteable));
    m.push(command("Shell > Save Selected Text...", "Save", sel));
    sep(m);
    // CMH:646-657.
    m.push(command("Edit > Select All", "Select All", true));
    m.push(item("sendselection", "Send Selection", sel));
    m.push(item("snippet", "Save Selection as Snippet", sel));
    m.push(command("Edit > Clear Buffer", "Clear Buffer", true));
    m.push(command("Edit > Marks and Annotations > Annotate Selection", "Annotate Selection", sel));
    m.push(item("reveal", "Reveal Annotation", c.at_point && c.annotation_hidden));
    sep(m);
    m.push(command("Session > Edit Session...", "Edit Session\u{2026}", true));
    sep(m);
    m.push(item("togglebroadcast", "Toggle Broadcasting Input", true));
    m.push(check("lockpane", "Lock Pane", true, c.pane_locked));
    if c.tab_has_many {
        if c.all_locked {
            m.push(item("unlockall", "Unlock All Panes in Tab", true));
        } else {
            m.push(item("lockall", "Lock All Panes in Tab", true));
            if c.any_locked {
                m.push(item("unlockall", "Unlock All Panes in Tab", true));
            }
        }
    }
    if c.has_coprocess {
        m.push(item("stopcoprocess", "Stop Coprocess", true));
    }
    sep(m);
    m.push(command("Shell > Close", "Close", !c.layout_locked));
    m.push(command("Session > Restart Session", "Restart", c.restartable));
    sep(m);
    m.push(command("Session > Bury Session", "Bury", c.can_bury));
    sep(m);
    // Terminal State.
    let mut state: Vec<Row> = Vec::new();
    for title in TERMINAL_STATE {
        if title.is_empty() {
            sep(&mut state);
        } else if title == "Emulation Level" {
            let levels = LEVELS.iter().map(|l| Row::Item { id: format!("{CTX_ID}cmd.Session > Terminal State > Emulation Level > {l}"), label: (*l).to_string(), enabled: true, checked: Some(c.terminal_state.is(l)), swatch: None }).collect();
            state.push(Row::Sub { label: title.into(), rows: levels });
        } else {
            state.push(Row::Item { id: format!("{CTX_ID}cmd.Session > Terminal State > {title}"), label: title.to_string(), enabled: true, checked: Some(c.terminal_state.is(title)), swatch: None });
        }
    }
    m.push(Row::Sub { label: "Terminal State".into(), rows: state });
    // PS:14841-14856.
    if let Some(paused) = c.tmux_paused {
        sep(m);
        m.push(command("Shell > tmux > Pause Pane", if paused { "Unpause tmux Pane" } else { "Pause tmux Pane" }, true));
    }
}

/// The Profiles of New Window (`action` "newwin") or New Tab ("newtab") (`PT:7234-7262`, `iTermController
/// addBookmarksToMenu:`): a profile by guid, a tag folder as a submenu, and "Open All" after a line
/// for the folder at `path`. The profile's shortcut is not shown: iTerm2 shows none in this menu.
pub fn profile_rows(nodes: &[crate::profile_menu::Node], action: &str) -> Vec<Row> {
    use crate::profile_menu::Node;
    let mut out = Vec::new();
    for node in nodes {
        match node {
            Node::Profile { guid, name, .. } => out.push(item(&format!("{action}:{guid}"), name, true)),
            Node::Folder { name, children } => out.push(Row::Sub { label: name.clone(), rows: profile_rows(children, action) }),
            Node::OpenAll { path } => {
                out.push(Row::Sep);
                out.push(item(&format!("{action}all:{path}"), "Open All", true));
            }
            Node::More => out.push(Row::Item { id: format!("{CTX_ID}more"), label: "More profiles are in Settings".into(), enabled: false, checked: None, swatch: None }),
        }
    }
    out
}

/// The margin menu on the timestamps (`CMH:139-158`): Set Baseline when the clicked time is not the
/// baseline, Disable when there is one.
pub fn timestamp_rows(baseline: f64, clicked: f64) -> Vec<Row> {
    let mut m = Vec::new();
    if baseline != clicked {
        m.push(item("tsbaseline", "Set Baseline for Relative Timestamps", true));
    }
    if baseline != 0.0 {
        m.push(item("tsdisable", "Disable Relative Timestamps", true));
    }
    m
}

/// The menu as text lines for tests: `label`, `label (disabled)`, `[x] label`, `----`, and a
/// submenu's rows indented under `label >`.
pub fn outline(rows: &[Row]) -> Vec<String> {
    fn go(rows: &[Row], depth: usize, out: &mut Vec<String>) {
        let pad = "  ".repeat(depth);
        for r in rows {
            match r {
                Row::Sep => out.push(format!("{pad}----")),
                Row::Item { label, enabled, checked, .. } => {
                    let mark = match checked {
                        Some(true) => "[x] ",
                        Some(false) => "[ ] ",
                        None => "",
                    };
                    out.push(format!("{pad}{mark}{label}{}", if *enabled { "" } else { " (disabled)" }));
                }
                Row::Sub { label, rows } => {
                    out.push(format!("{pad}{label} >"));
                    go(rows, depth + 1, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    go(rows, 0, &mut out);
    out
}

fn color_name(color: Color, fg: bool) -> String {
    const NAMES: [&str; 8] = ["Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White"];
    let indexed = |n: u8| -> String {
        match n {
            0..=15 => format!("{}{}", if n > 7 { "ANSI Bright " } else { "" }, NAMES[(n & 7) as usize]),
            16..=231 => {
                let (r, g, b) = ((n - 16) / 36, ((n - 16) % 36) / 6, (n - 16) % 6);
                format!("8bit({r}/5,{g}/5,{b}/5)")
            }
            _ => format!("gray{}/22", n - 232),
        }
    };
    match color {
        Color::Named(NamedColor::Foreground) | Color::Named(NamedColor::BrightForeground) => if fg { "Default fg" } else { "Default bg" }.to_string(),
        Color::Named(NamedColor::Background) => if fg { "Default bg" } else { "Default fg" }.to_string(),
        Color::Named(NamedColor::Cursor) => "Cursor".to_string(),
        Color::Named(n) => {
            let i = n as usize;
            if i < 16 {
                indexed(i as u8)
            } else {
                "Invalid".to_string()
            }
        }
        Color::Indexed(n) => indexed(n),
        Color::Spec(c) => format!("24bit({},{},{})", c.r, c.g, c.b),
    }
}

/// A selected cell's description (`ScreenCharDescription`, `ScreenChar.m:761-831`), with the cell's
/// hyperlink and underline color as `humanReadableDescription` names them (`iTermExternalAttributeIndex.m:739-760`).
/// Termurion reads no right-to-left status from a cell, so the direction is "Dir-Unk" (iTerm2 says that
/// with bidi off).
pub fn char_description(cell: &Cell) -> Option<String> {
    if cell.image().is_some() {
        return None;
    }
    let f = cell.flags;
    let mut attrs: Vec<&str> = Vec::new();
    for (flag, name) in [(Flags::BOLD, "Bold"), (Flags::DIM, "Faint"), (Flags::ITALIC, "Italic"), (Flags::HIDDEN, "Invisible")] {
        if f.contains(flag) {
            attrs.push(name);
        }
    }
    for (flag, name) in [
        (Flags::UNDERLINE, "Underline"),
        (Flags::UNDERCURL, "Curly-Underline"),
        (Flags::DOUBLE_UNDERLINE, "Double-Underline"),
        (Flags::DOTTED_UNDERLINE, "Dotted-Underline"),
        (Flags::DASHED_UNDERLINE, "Dashed-Underline"),
    ] {
        if f.contains(flag) {
            attrs.push(name);
        }
    }
    if f.contains(Flags::STRIKEOUT) {
        attrs.push("Strike");
    }
    if f.contains(Flags::INVERSE) {
        attrs.push("Inverse");
    }
    attrs.push("Dir-Unk");
    let mut text = format!("fg={} bg={}; {}", color_name(cell.fg, true), color_name(cell.bg, false), attrs.join(" "));
    let mut ea: Vec<String> = Vec::new();
    if let Some(c) = cell.underline_color() {
        ea.push(format!("underline={}", color_name(c, true)));
    }
    if cell.hyperlink().is_some() {
        ea.push("URL".to_string());
    }
    if !ea.is_empty() {
        text.push_str("; ");
        text.push_str(&ea.join(","));
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(c: &Ctx) -> Vec<String> {
        outline(&build(c))
    }

    fn cmds(rows: &[Row]) -> Vec<String> {
        let mut out = Vec::new();
        fn go(rows: &[Row], out: &mut Vec<String>) {
            for r in rows {
                match r {
                    Row::Item { id, .. } => {
                        if let Some(menu) = id.strip_prefix(&format!("{CTX_ID}cmd.")) {
                            out.push(menu.to_string());
                        }
                    }
                    Row::Sub { rows, .. } => go(rows, out),
                    Row::Sep => {}
                }
            }
        }
        go(rows, &mut out);
        out
    }

    /// The menu of a pane with nothing selected, in a tab of one pane (`CMH:522-813`, `PT:7203-7266`). Every item iTerm2
    /// builds is there, greyed when its validation says no (`CMH:312-376`).
    fn plain() -> Ctx {
        Ctx {
            at_point: true,
            scp_title: "Download with scp".into(),
            move_to_window_ok: false,
            pasteable: true,
            can_bury: true,
            restartable: true,
            ..Ctx::default()
        }
    }

    /// The Terminal State submenu as iTerm2 builds it (`CMH:736-810`), nothing checked.
    fn terminal_state_lines() -> Vec<String> {
        let mut v: Vec<String> = vec!["Terminal State >".into()];
        for t in [
            "[ ] Alternate Screen", "----", "[ ] Focus Reporting", "[ ] Mouse Reporting", "[ ] Paste Bracketing", "----", "[ ] Application Cursor", "[ ] Application Keypad", "----",
            "[ ] Standard Key Reporting Mode", "[ ] modifyOtherKeys Mode 1", "[ ] modifyOtherKeys Mode 2", "[ ] CSI u Mode", "[ ] Raw Key Reporting Mode", "----",
            "[ ] Disambiguate Escape", "[ ] Report All Event Types", "[ ] Report Alternate Keys", "[ ] Report All Keys as Escape Codes", "[ ] Report Associated Text", "----",
            "[ ] Literal Controls", "----", "Emulation Level >",
        ] {
            v.push(format!("  {t}"));
        }
        for l in ["VT100", "VT200", "VT300", "VT400", "VT500"] {
            v.push(format!("    [ ] {l}"));
        }
        v
    }

    /// No selection, no mark: the whole fixed menu, top to bottom. Each group cites iTerm2.
    #[test]
    fn no_selection_follows_iterm2() {
        let mut want: Vec<String> = Vec::new();
        let mut add = |lines: &[&str]| want.extend(lines.iter().map(|s| s.to_string()));
        // PT:7211-7222 (menuForEvent:menu:, called by CMH:728, inserts at the top): New Window, New Tab, a line.
        add(&["New Window >", "New Tab >", "----"]);
        // CMH:548 the download item (greyed with no selection, CMH:271-286); no short selection: no Open Selection as URL (CMH:549).
        add(&["Download with scp (disabled)"]);
        // CMH:560 Search the Web for Selection, CMH:562 Send Email, CMH:563 Add Trigger, greyed with no selection (CMH:343-350); CMH:566 a line.
        add(&["Search the Web for Selection (disabled)", "Send Email to Selected Address (disabled)", "Add Trigger\u{2026} (disabled)", "----"]);
        // CMH:583-584 the splits (enabled: the layout is not locked, CMH:297-300), CMH:592 a line.
        add(&["Split Pane Vertically", "Split Pane Horizontally", "----"]);
        // CMH:596-606 the move group: no Move Session to Tab with one pane (CMH:597), Move Session to Window needs a second session (PT:13884); CMH:608 a line.
        add(&["Move Session to Split Pane", "Move Session to Window (disabled)", "Swap With Session\u{2026}", "----"]);
        // CMH:612-639 Copy (greyed with no selection, PTYTextView.m:540-545), Paste, Save (PTYTextView.m:526-527); CMH:642 a line.
        add(&["Copy (disabled)", "Paste", "Save (disabled)", "----"]);
        // CMH:646-657 Select All, Send Selection, Save Selection as Snippet, Clear Buffer, Annotate Selection, Reveal Annotation (CMH:355-363); CMH:660 a line.
        add(&["Select All", "Send Selection (disabled)", "Save Selection as Snippet (disabled)", "Clear Buffer", "Annotate Selection (disabled)", "Reveal Annotation (disabled)", "----"]);
        // CMH:663 Edit Session, a line, CMH:669 Toggle Broadcasting Input, CMH:673-678 Lock Pane; CMH:721 a line.
        add(&["Edit Session\u{2026}", "----", "Toggle Broadcasting Input", "[ ] Lock Pane", "----"]);
        // CMH:725-726 Close, Restart (CMH:287-289); CMH:730 a line, CMH:732 Bury (CMH:329-330); CMH:735 a line.
        add(&["Close", "Restart", "----", "Bury", "----"]);
        // CMH:736-810 Terminal State.
        want.extend(terminal_state_lines());
        assert_eq!(lines(&plain()), want);
    }

    /// A short selection of a word: the short-selection items (`CMH:535-562`), a locally openable URL's splits and Quick Look.
    #[test]
    fn a_selected_url_follows_iterm2() {
        let mut c = plain();
        c.has_selection = true;
        c.short = Some("http://example.com".into());
        c.reasonable = true;
        c.locally_openable = true;
        c.quick_look = true;
        c.scp_enabled = false;
        c.helpers.copy = vec![HelperRow { label: "Copy Base64-Encoded".into(), enabled: true, swatch: None }];
        let got = lines(&c);
        let from = got.iter().position(|l| l == "Copy Base64-Encoded").unwrap();
        // CMH:464-470: the base64 copy closes the helpers with a line (CMH:502-504), then the download item (CMH:548) and the web items (CMH:549-563).
        assert_eq!(
            &got[from..from + 13],
            [
                "Copy Base64-Encoded",
                "----",
                "Download with scp (disabled)",
                "Open Selection as URL",
                "Open URL in Vertical Split Pane",
                "Open URL in Horizontal Split Pane",
                "----",
                "Quick Look Link",
                "Search the Web for Selection",
                "Send Email to Selected Address",
                "Add Trigger\u{2026}",
                "----",
                "Split Pane Vertically",
            ]
        );
        // Copy, Save and the selection items work with a selection.
        assert!(got.contains(&"Copy".to_string()) && got.contains(&"Save".to_string()) && got.contains(&"Send Selection".to_string()));
        // A locked layout greys the URL splits (CMH:336-341) and the layout items (CMH:291-300).
        c.layout_locked = true;
        let locked = lines(&c);
        assert!(locked.contains(&"Open URL in Vertical Split Pane (disabled)".to_string()));
        assert!(locked.contains(&"Split Pane Vertically (disabled)".to_string()) && locked.contains(&"Close (disabled)".to_string()));
        assert!(locked.contains(&"Swap With Session\u{2026} (disabled)".to_string()));
    }

    /// The helpers: synonyms, color, Base64 preview of a short selection, then the numeric rows,
    /// the Base64 copies and the replacements of a reasonable one, each group closed by one line (`CMH:435-498`).
    #[test]
    fn helpers_and_replacements_follow_iterm2() {
        let row = |t: &str| HelperRow { label: t.into(), enabled: true, swatch: None };
        let mut c = plain();
        c.has_selection = true;
        c.short = Some("255".into());
        c.reasonable = true;
        c.helpers = Groups { synonyms: vec![row("255 = 0xff")], color: Some(HelperRow { swatch: Some((1, 2, 3)), ..row("#010203") }), preview: Some(row("Base64: x")), numeric: vec![row("4 UTF-8 bytes: abcd")], copy: vec![row("Copy Base64-Decoded"), row("Copy Base64-Encoded")] };
        c.replacements = vec![Replace::Json, Replace::Base64Decode, Replace::Base64Encode];
        let got = lines(&c);
        let at = got.iter().position(|l| l == "255 = 0xff").unwrap();
        assert_eq!(
            &got[at..at + 11],
            [
                "255 = 0xff",
                "#010203",
                "Base64: x",
                "4 UTF-8 bytes: abcd",
                "Copy Base64-Decoded",
                "Copy Base64-Encoded",
                // CMH:473-475 a line before the replacements, CMH:484-498 their titles.
                "----",
                "Replace with Pretty-Printed JSON",
                "Replace with Base64-Decoded Value",
                "Replace with Base64-Encoded Value",
                "----",
            ]
        );
        // The numbering the pick reads is the order shown.
        let ids: Vec<String> = build(&c).iter().filter_map(|r| if let Row::Item { id, .. } = r { id.strip_prefix(&format!("{CTX_ID}helper.")).map(str::to_string) } else { None }).collect();
        assert_eq!(ids, ["0", "1", "2", "3", "4", "5"]);
        // Without a short selection the synonyms, the color and the preview are not offered (CMH:436).
        c.short = None;
        let long = lines(&c);
        assert!(!long.contains(&"255 = 0xff".to_string()) && !long.contains(&"#010203".to_string()));
        assert!(long.contains(&"4 UTF-8 bytes: abcd".to_string()));
    }

    /// The marks and folds above the New Window group (`CMH:208-267`): Command Info on top, then the
    /// mark's name with Remove Named Mark, then Fold or Unfold.
    #[test]
    fn marks_and_folds_stack_above_new_window() {
        let mut c = plain();
        c.command_info = true;
        c.mark_name = Some("deploy".into());
        c.fold = Fold::Fold;
        let got = lines(&c);
        assert_eq!(&got[..9], ["Command Info", "----", "deploy (disabled)", "Remove Named Mark", "----", "Fold", "----", "New Window >", "New Tab >"]);
        c.fold = Fold::Unfold;
        c.command_info = false;
        c.mark_name = None;
        assert_eq!(&lines(&c)[..4], ["Unfold", "----", "New Window >", "New Tab >"]);
    }

    /// Several panes and tabs: Move Session to Tab, Select, and the tab-wide lock items (`CMH:597-602`, `CMH:682-712`, `PT:7222-7248`).
    #[test]
    fn several_panes_and_tabs_follow_iterm2() {
        let mut c = plain();
        c.tab_has_many = true;
        c.move_to_tab_ok = true;
        c.move_to_window_ok = true;
        c.select = vec![item("selecttab:0", "zsh #1", true)];
        c.pane_locked = true;
        c.any_locked = true;
        let got = lines(&c);
        assert_eq!(&got[..4], ["New Window >", "New Tab >", "Select >", "  zsh #1"]);
        assert!(got.contains(&"Move Session to Tab".to_string()));
        // Lock Pane is checked; not all are locked: Lock All, and Unlock All behind Option, which a popup shows (CMH:696-711).
        let at = got.iter().position(|l| l == "[x] Lock Pane").unwrap();
        assert_eq!(&got[at..at + 3], ["[x] Lock Pane", "Lock All Panes in Tab", "Unlock All Panes in Tab"]);
        // A locked pane cannot move (CMH:291-295).
        assert!(got.contains(&"Move Session to Split Pane (disabled)".to_string()));
        // All locked: only Unlock (CMH:688-695).
        c.all_locked = true;
        let all = lines(&c);
        let at = all.iter().position(|l| l == "[x] Lock Pane").unwrap();
        assert_eq!(&all[at..at + 2], ["[x] Lock Pane", "Unlock All Panes in Tab"]);
        assert!(!all.contains(&"Lock All Panes in Tab".to_string()));
    }

    /// A link under the click, a running coprocess, a tmux pane in pause mode, annotations and providers.
    #[test]
    fn link_coprocess_tmux_annotation_and_providers() {
        let mut c = plain();
        c.link = Link::Address;
        c.has_coprocess = true;
        c.tmux_paused = Some(false);
        c.annotation_hidden = true;
        c.providers = vec![("Mine".into(), "id1".into())];
        c.actions = vec![("Open \"x\"".into(), true)];
        let got = lines(&c);
        // CMH:612-636 Copy Link Address follows Copy (a plain URL says Copy URL, CMH:630).
        let at = got.iter().position(|l| l == "Copy (disabled)").unwrap();
        assert_eq!(&got[at..at + 3], ["Copy (disabled)", "Copy Link Address", "Paste"]);
        c.link = Link::Url;
        assert!(lines(&c).contains(&"Copy URL".to_string()));
        // CMH:570-580 the smart selection actions with a line, then the providers (CMH:898-912) inside their own line and one more after.
        let at = got.iter().position(|l| l == "Open \"x\"").unwrap();
        assert_eq!(&got[at..at + 6], ["Open \"x\"", "----", "----", "Mine", "----", "Split Pane Vertically"]);
        // CMH:657 Reveal Annotation works when the click is on a hidden annotation (CMH:355-364).
        assert!(got.contains(&"Reveal Annotation".to_string()));
        // CMH:717-718 Stop Coprocess follows the lock items; PS:14841-14856 the tmux item closes the menu.
        let at = got.iter().position(|l| l == "Stop Coprocess").unwrap();
        assert_eq!(&got[at - 1..at + 1], ["[ ] Lock Pane", "Stop Coprocess"]);
        assert_eq!(&got[got.len() - 2..], ["----", "Pause tmux Pane"]);
        c.tmux_paused = Some(true);
        assert_eq!(lines(&c).last().unwrap(), "Unpause tmux Pane");
        // The title bar's menu has no click point: no link item (CMH:617), no annotation (CMH:356-358).
        c.at_point = false;
        let bar = lines(&c);
        assert!(!bar.contains(&"Copy URL".to_string()) && bar.contains(&"Reveal Annotation (disabled)".to_string()));
    }

    /// An image (`CMH:385-427`): its own short menu, a broken one says File, an animated one offers to stop.
    #[test]
    fn an_image_follows_iterm2() {
        let mut c = plain();
        c.image = Some(ImageFacts::default());
        assert_eq!(lines(&c), ["Save Image As\u{2026}", "Copy Image", "Open Image", "Inspect"]);
        c.image = Some(ImageFacts { animated: true, ..ImageFacts::default() });
        assert_eq!(lines(&c).last().unwrap(), "Stop Animating");
        c.image = Some(ImageFacts { animated: true, paused: true, broken: false });
        assert_eq!(lines(&c).last().unwrap(), "Resume Animating");
        c.image = Some(ImageFacts { broken: true, ..ImageFacts::default() });
        assert_eq!(lines(&c), ["Save File As\u{2026}", "Copy File", "Open File", "Inspect"]);
        // The marks above still apply to an image menu (CMH:208-267 insert into whatever menu was built).
        c.command_info = true;
        assert_eq!(&lines(&c)[..2], ["Command Info", "----"]);
    }

    #[test]
    fn terminal_state_checks_and_titles() {
        let mut c = plain();
        c.terminal_state.on = vec!["Alternate Screen".into(), "VT200".into(), "Literal Controls".into()];
        let got = lines(&c);
        assert!(got.contains(&"  [x] Alternate Screen".to_string()) && got.contains(&"  [x] Literal Controls".to_string()) && got.contains(&"    [x] VT200".to_string()));
        assert!(got.contains(&"    [ ] VT100".to_string()));
    }

    #[test]
    fn the_timestamp_menu_follows_iterm2() {
        // CMH:139-158: Set Baseline when the clicked time differs from the baseline; Disable when there is one.
        assert_eq!(outline(&timestamp_rows(0.0, 5.0)), ["Set Baseline for Relative Timestamps"]);
        assert_eq!(outline(&timestamp_rows(5.0, 7.0)), ["Set Baseline for Relative Timestamps", "Disable Relative Timestamps"]);
        assert_eq!(outline(&timestamp_rows(5.0, 5.0)), ["Disable Relative Timestamps"]);
    }

    /// Every menu command the menu runs is an item of the main menu, and no title carries shortcut text or three dots.
    #[test]
    fn commands_exist_and_titles_are_plain() {
        let mut c = plain();
        c.has_selection = true;
        c.short = Some("x".into());
        c.reasonable = true;
        c.tab_has_many = true;
        c.tmux_paused = Some(false);
        c.replacements = vec![Replace::Json, Replace::Base64Decode, Replace::Base64Encode];
        let rows = build(&c);
        for id in cmds(&rows) {
            assert!(crate::menus::by_id(&id).is_some(), "{id}");
        }
        for l in outline(&rows) {
            assert!(!l.contains('\t') && !l.contains("..."), "{l}");
        }
    }

    #[test]
    fn a_cell_is_described_as_iterm2_does() {
        let mut cell = Cell::default();
        assert_eq!(char_description(&cell).unwrap(), "fg=Default fg bg=Default bg; Dir-Unk");
        cell.flags.insert(Flags::BOLD | Flags::ITALIC);
        cell.fg = Color::Indexed(9);
        cell.bg = Color::Indexed(200);
        assert_eq!(char_description(&cell).unwrap(), "fg=ANSI Bright Red bg=8bit(4/5,0/5,2/5); Bold Italic Dir-Unk");
        cell.fg = Color::Spec(alacritty_terminal::vte::ansi::Rgb { r: 1, g: 2, b: 3 });
        cell.bg = Color::Indexed(240);
        assert_eq!(char_description(&cell).unwrap(), "fg=24bit(1,2,3) bg=gray8/22; Bold Italic Dir-Unk");
    }
}
