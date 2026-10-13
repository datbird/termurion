//! iTerm2's menu hints and menu tips: the split items' direction glyphs
//! (`AppKit/iTermApplicationDelegate.m:392-401`, `ShowHintsInSplitPaneMenuItems`), the table of
//! tips shown beside a highlighted item (`AppKit/iTermApplicationDelegate.swift:24-196`,
//! `MainMenu/MenuItemTipController.swift`), and the lookup from a menu item's shown label and its
//! parent's label to iTerm2's XIB identifier (Win32 menu items do not carry muda's ids). The
//! window side is `menu_tip_win`.
//!
//! The tip texts are iTerm2's with "iTerm2" as "Termurion", "the macOS Keychain" as "Windows
//! Credential Manager", "Keychain" as "Credential Manager", "macOS-native" as "native",
//! "/tmp/debuglog.txt" as "%TEMP%\debuglog.txt" (with the folder 13w's debug log falls back to) and
//! the Markdown marks removed. iTerm2's tip
//! images show macOS screenshots and are left out (Decision).

use std::time::Duration;

use crate::menus;

/// iTerm2's page of copy mode commands (`PseudoTerminal.m:10717-10722`).
pub const COPY_MODE_URL: &str = "https://iterm2.com/documentation-copymode.html";

/// The glyph before the two horizontal-split items (`AD:392-401`): a line with a combining box.
const HORIZONTAL_HINT: &str = "\u{2500}\u{20de} ";
/// The glyph before the two vertical-split items.
const VERTICAL_HINT: &str = "\u{2502}\u{20de} ";

/// A split item's label with its direction hint when `on` (`ShowHintsInSplitPaneMenuItems` was on
/// at launch): `splitHorizontally:` and `openSplitHorizontallySheet:` get the horizontal glyph, the
/// two vertical ones the vertical glyph. Any other selector is unchanged.
pub fn split_hint(selector: &str, label: &str, on: bool) -> String {
    let hint = match selector {
        "splitHorizontally:" | "openSplitHorizontallySheet:" => HORIZONTAL_HINT,
        "splitVertically:" | "openSplitVerticallySheet:" => VERTICAL_HINT,
        _ => return label.to_string(),
    };
    if on { format!("{hint}{label}") } else { label.to_string() }
}

/// A menu item's label as the Win32 menu holds it, back to the shown label: the text before the
/// tab (the chord), "&&" as "&", a split hint prefix removed.
pub fn shown_from_menu_text(raw: &str) -> String {
    let label = raw.split('\t').next().unwrap_or("").replace("&&", "&");
    let label = label.strip_prefix(HORIZONTAL_HINT).or_else(|| label.strip_prefix(VERTICAL_HINT)).unwrap_or(&label);
    label.to_string()
}

/// One tip: the identifier of the menu item it belongs to and its text.
pub struct MenuTip {
    pub identifier: &'static str,
    pub text: &'static str,
}

/// iTerm2's 69 tips (`ADS:24-196`), in its order.
pub const TIPS: &[MenuTip] = &[
    MenuTip { identifier: "Toolbelt", text: "The Toolbelt provides a versatile, dockable sidebar that offers quick access to frequently used features and information. It supports multiple panels that can be displayed simultaneously, including clipboard history, recently opened directories, command history, a scratchpad for notes, and more." },
    MenuTip { identifier: "Show Toolbelt", text: "The Toolbelt provides a versatile, dockable sidebar that offers quick access to frequently used features and information. It supports multiple panels that can be displayed simultaneously, including clipboard history, recently opened directories, command history, a scratchpad for notes, and more." },
    MenuTip { identifier: "Split Vertically with Current Profile", text: "Splits the current session vertically, placing a new session in the right half. The new session inherits the profile of the current session, including any changes made in Session > Edit Session." },
    MenuTip { identifier: "Split Horizontally with Current Profile", text: "Splits the current session horizontally, placing a new session in the bottom half. The new session inherits the profile of the current session, including any changes made in Session > Edit Session." },
    MenuTip { identifier: "Split Vertically\u{2026}", text: "Prompts you to select a profile and then splits the current session vertically, placing the new session in the right half." },
    MenuTip { identifier: "Split Horizontally\u{2026}", text: "Prompts you to select a profile and then splits the current session horizontally, placing the new session in the bottom half." },
    MenuTip { identifier: "tmux.Dashboard", text: "The tmux Dashboard helps you switch between tmux sessions, show and hide windows, and administer other features of tmux without needing to use tmux\u{2019}s commands." },
    MenuTip { identifier: "Paste Special.Advanced Paste\u{2026}", text: "Advanced Paste lets you edit text before pasting, remove control characters, convert tabs, base64-encode, and perform regular expression substitutions. It also lets you fine-tune how quickly pasted text is sent." },
    MenuTip { identifier: "Render Selection Natively", text: "Render Natively shows a nicely formatted, syntax-highlighted rendition of a document. For example, Markdown renders beautifully. It also allows for horizontal scrolling, making it a convenient way to view log files." },
    MenuTip { identifier: "Paste Special.Warn Before Multi-Line Paste", text: "You\u{2019}ll be prompted any time you paste text containing a newline. See also Limit Multi-Line Paste Warning to Shell Prompt." },
    MenuTip { identifier: "Paste Special.Limit Multi-Line Paste Warning to Shell Prompt", text: "This is effective only when Warn Before Multi-Line Paste is enabled. It also requires Shell Integration. When enabled, it suppresses confirmation when pasting text containing a newline if you are not at a shell prompt." },
    MenuTip { identifier: "Paste Special.Warn Before Pasting One Line Ending in a Newline at Shell Prompt", text: "If enabled, you\u{2019}ll be prompted to confirm that you wish to send a newline when pasting a single line of text ending in a newline. Shell Integration is required." },
    MenuTip { identifier: "Engage Artificial Intelligence", text: "When selected at a shell prompt (provided Shell Integration is installed) or in the Composer, it sends the current command to the configured AI system along with a prompt for it to generate a command. If no input is provided, you\u{2019}ll be asked to give instructions. The generated command goes into the Composer." },
    MenuTip { identifier: "Explain Output with AI", text: "This is meant to be used at the shell prompt after executing a command. It requires Shell Integration. The output of the preceding (or selected) command is sent to AI, which annotates the output and opens a chat window for further discussion." },
    MenuTip { identifier: "Edit.Snippets", text: "Snippets are pieces of text that you save to reuse later. They\u{2019}re great for frequently used commands, hard-to-remember directories, and much more." },
    MenuTip { identifier: "Edit.Actions", text: "Actions are saved instructions for Termurion. For example, you could create an action that opens a new window and then creates a split pane." },
    MenuTip { identifier: "Set Default Width", text: "Records the current width of the toolbelt for use in newly created windows." },
    MenuTip { identifier: "Toolbelt.Actions", text: "Actions are saved instructions for Termurion. For example, you could create an action that opens a new window and then creates a split pane." },
    MenuTip { identifier: "Selection Respects Soft Boundaries", text: "When enabled, dividers rendered by programs like vim or emacs are detected, and text selection wraps around them." },
    MenuTip { identifier: "Find.Filter", text: "Filter allows you to hide any lines that do not match a search query, which can be a substring or regular expression. It updates live as new text arrives." },
    MenuTip { identifier: "Marks and Annotations.Set Mark", text: "A Mark appears as a blue triangle in the left margin. You can easily navigate among marks using Jump to Mark, Next Mark, and Previous Mark. If Shell Integration is enabled, a Mark is automatically added at each shell prompt." },
    MenuTip { identifier: "Set Named Mark", text: "A Named Mark appears as a blue triangle in the left margin. In addition to being easy to navigate with Next Mark and Previous Mark, you can also find Named Marks in the Toolbelt\u{2019}s Named Marks tool." },
    MenuTip { identifier: "Toolbelt.Named Marks", text: "A Named Mark appears as a blue triangle in the left margin. In addition to being easy to navigate with Next Mark and Previous Mark, you can also find Named Marks in this Toolbelt tool." },
    MenuTip { identifier: "Fold Selected Lines", text: "Fold lets you collapse multiple lines into a single line to hide distracting text. You can always unfold it by clicking the arrow in the margin, or right-clicking and choosing Unfold." },
    MenuTip { identifier: "Toolbelt.Captured Output", text: "Captured Output works in conjunction with a Trigger to detect interesting text in the terminal and make it easy to find. The Toolbelt tool shows a list of captured text. You can click to navigate to it or double-click to enter a programmable command. This is useful for finding errors in the output of a build command, for example. It requires Shell Integration." },
    MenuTip { identifier: "Toolbelt.Codecierge", text: "Codecierge uses AI to help you achieve a goal. Tell it what you want to do, and it can watch your terminal to interpret output and suggest commands. Shell Integration is required." },
    MenuTip { identifier: "Toolbelt.Command History", text: "If Shell Integration is installed, Command History shows a searchable list of recently run commands on the current host." },
    MenuTip { identifier: "Toolbelt.Notes", text: "Notes is a single, persistent notepad in your Toolbelt. It\u{2019}s useful for keeping track of what you\u{2019}re doing or composing messages." },
    MenuTip { identifier: "Toolbelt.Paste History", text: "Paste History shows text that you have copied and pasted in Termurion. You can configure it to be saved long term." },
    MenuTip { identifier: "Toolbelt.Profiles", text: "Shows a list of your profiles so you can create new sessions easily." },
    MenuTip { identifier: "Toolbelt.Recent Directories", text: "Shows your most used directories, sorted by a combination of frequency and recency of use. Requires Shell Integration." },
    MenuTip { identifier: "Toolbelt.Snippets", text: "Snippets are pieces of text that you save to reuse later. They\u{2019}re great for frequently used commands, hard-to-remember directories, and much more." },
    MenuTip { identifier: "Zoom In on Selection", text: "Hides everything except the lines of selected text to remove distractions." },
    MenuTip { identifier: "Find Cursor", text: "Highlights the location of the cursor and unhides it if it is currently hidden." },
    MenuTip { identifier: "Show Annotations", text: "Annotations are inline markup. When closed, they appear as a yellow underline; when open, they look like yellow stickies where you can write memos about content in the terminal window." },
    MenuTip { identifier: "Composer", text: "The Composer is a window within the terminal where you can edit text using native controls. It does syntax highlighting, command and filename completion, even over SSH (provided you use SSH Integration). If AI features are enabled, you can also get AI-powered suggestions. You can even have multiple cursors! When you're ready, you can send the whole buffer or just a line at a time to your shell." },
    MenuTip { identifier: "Auto Composer", text: "Auto Composer replaces your shell prompt with a native text field. It does syntax highlighting and command and filename completion. You can also enable AI-powered suggestions. Shell Integration is required." },
    MenuTip { identifier: "Open Quickly", text: "Open Quickly provides quick access to many common actions. You can use it to find a session by typing its name, directory, hostname, or recent command. You can also use it to switch profiles or create a new window by typing the name of a profile. Restore an arrangement by entering its name. Press / to get tips for quick commands." },
    MenuTip { identifier: "Start Instant Replay", text: "Instant Replay lets you review recent terminal history. It\u{2019}s handy if something just disappeared from the screen and it isn\u{2019}t in scrollback history." },
    MenuTip { identifier: "Run Coprocess\u{2026}", text: "A Coprocess is a program that automates interactions in the terminal. Input to the terminal is redirected to stdin of the coprocess, and its output is sent back to the terminal as though the coprocess were typing." },
    MenuTip { identifier: "Stop Coprocess", text: "Stops the active coprocess. Input to the terminal is no longer redirected to the coprocess, and its output ceases." },
    MenuTip { identifier: "Triggers", text: "Triggers are actions the terminal performs automatically when text matching a regular expression is received. For example, you can highlight text or display an alert." },
    MenuTip { identifier: "Terminal State.Literal Mode", text: "When enabled, control characters are displayed visually rather than being interpreted as usual." },
    MenuTip { identifier: "Terminal State.Report Modifiers with CSI u", text: "This mode is generally not recommended. Disambiguate Escape is a more modern approach." },
    MenuTip { identifier: "Bury Session", text: "Buried sessions are hidden in the Buried Sessions menu below and do not appear in any window. These are particularly useful for the session where you initiate tmux integration by running tmux -CC." },
    MenuTip { identifier: "Open Interactive Window", text: "The Python REPL opens a window running a special Python interpreter that lets you experiment with Termurion\u{2019}s Python API. You can use await at the top level of the interpreter." },
    MenuTip { identifier: "Manage Dependencies", text: "Opens a UI where you can add, update, or remove pip dependencies of a Python API script." },
    MenuTip { identifier: "Install Python Runtime", text: "Termurion\u{2019}s Python Runtime is a large binary package (hundreds of MBs) that enables the Python API by installing a pre-built Python environment that scripts can use." },
    MenuTip { identifier: "Import Script", text: "Use Import to install scripts others have shared with you. These scripts have the .its extension." },
    MenuTip { identifier: "Export Script", text: "If you want to share Python API scripts, you can export them to an .its file. If you have a code signing certificate and private key in your Credential Manager, you can also sign the .its file." },
    MenuTip { identifier: "Script Console", text: "View errors and low-level communication between Python API scripts and Termurion here.\n\nThe Inspector can be accessed from the Console. It allows you to browse variables in sessions, tabs, and windows." },
    MenuTip { identifier: "Arrangements", text: "Window Arrangements are a saved record of one or more windows, their tabs, and split panes, including how each pane is configured. They do not include content. They\u{2019}re a quick way to create a working environment with multiple sessions in various configurations." },
    MenuTip { identifier: "Password Manager", text: "The Password Manager helps you keep track of your passwords securely. By default, it stores them in Windows Credential Manager, but it can also use 1Password or LastPass." },
    MenuTip { identifier: "AI Chats", text: "AI Chats opens a chat window where you can interact with AI. It can optionally view and control the terminal if you grant it permission." },
    MenuTip { identifier: "Pin Hotkey Window", text: "A pinned Hotkey Window does not close automatically when it loses keyboard focus." },
    MenuTip { identifier: "GPU Renderer Availability", text: "Checks whether the GPU Renderer is currently being used in the active session. This is sometimes useful for debugging." },
    MenuTip { identifier: "Secure Keyboard Entry", text: "Secure Keyboard Entry prevents other programs from intercepting your keystrokes in the terminal. However, it also breaks some functionality: other programs cannot activate their windows while this is enabled. For example, the open command will still open an app, but it won\u{2019}t be activated." },
    MenuTip { identifier: "Install Shell Integration", text: "Shell Integration consists of shell scripts that run when you log in. They inform Termurion of where your shell prompt is. This enables dozens of useful features such as command history, directory history, AI features, and more." },
    MenuTip { identifier: "Toggle Debug Logging", text: "Debug logs are saved in memory while this setting is enabled and written to %TEMP%\\debuglog.txt (%LOCALAPPDATA%\\Termurion\\Logs\\debuglog.txt when %TEMP% is not a local folder) when you turn it off. Memory use is capped at about 200MB; if the log grows past that, the oldest entries are discarded so the most recent activity is always kept." },
    MenuTip { identifier: "Broadcast Input.Broadcast Input to All Panes in All Tabs", text: "When enabled, anything you type in this window is sent to all sessions in this window." },
    MenuTip { identifier: "Broadcast Input.Broadcast Input to All Panes in Current Tab", text: "When enabled, anything you type in this tab is sent to all sessions in this tab." },
    MenuTip { identifier: "Broadcast Input.Toggle Broadcast Input to Current Session", text: "Adds or removes this session from the set of sessions in this window that have broadcast enabled. When you type in a session with broadcast enabled, the keystrokes are sent to all other sessions in the same window that have broadcast enabled." },
    MenuTip { identifier: "Broadcast Input.Show Background Pattern Indicator", text: "When enabled, prominent red lines are drawn in the background to indicate that text you type is being broadcast to other sessions." },
    MenuTip { identifier: "Broadcast Input.Current Session is Broadcast Source", text: "When enabled, typing in this session is broadcast to other sessions in the same broadcast domain. Typing in other sessions sends input only to those sessions." },
    MenuTip { identifier: "Lock Size", text: "Locked windows resist being resized. This can be useful when Windows moves your windows around when connecting or disconnecting displays." },
    MenuTip { identifier: "Lock Layout", text: "When a window\u{2019}s layout is locked, its tabs and panes can\u{2019}t be added, closed, reordered, dragged, or moved to another window, so a stray click or drag can\u{2019}t rearrange it. Resizing panes, opening new windows, and closing the window still work. This is an alternative to Lock Size; turning one on turns the other off." },
    MenuTip { identifier: "Notify on Status Change", text: "When enabled, the next time any session in this window changes its status (such as waiting, idle, or busy) an alert is shown and this setting turns itself back off. This is the same toggle as the bell button in the Session Status toolbelt tool, which must be open for this to be available." },
    MenuTip { identifier: "Toggle Buffer Input", text: "While Buffer Input is turned on, keyboard input is stored in a buffer. It will be sent when Buffer Input is turned off. You can also configure a trigger to change the Buffer Input setting." },
    MenuTip { identifier: "Toolbelt.Session Status", text: "The Session Status tool shows the status of sessions across all tabs. Statuses can be set by the Set Tab Status trigger or by programs using a control sequence. Each entry shows the session name, a colored indicator dot, status text, and a keyboard shortcut to jump to that session." },
];

/// Drops the Markdown marks iTerm2's tip texts carry: bold `**` and code backticks.
pub fn plain(markdown: &str) -> String {
    markdown.replace("**", "").replace('`', "")
}

/// The tip for a menu item's identifier.
pub fn tip_for(identifier: &str) -> Option<&'static str> {
    TIPS.iter().find(|t| t.identifier == identifier).map(|t| t.text)
}

/// The XIB identifier of the menu item with this shown label under this parent (the parent's shown
/// label; "" for a top-level menu), by `menu_text`'s rules. An item under View > Toolbelt that is
/// one of the toolbelt's tools is "Toolbelt.<label>", the identifier iTerm2 gives its dynamic
/// Toolbelt items (`append_toolbelt_menu` builds Termurion's). A submenu's own item counts, as
/// iTerm2 shows tips on "Toolbelt", "Edit.Snippets" and the like.
pub fn identifier_for(label: &str, parent: &str) -> Option<String> {
    // Only this build's items: an item `hidden_in_menu` removes has no tip, even if some other
    // menu (a submenu built elsewhere) showed the same label.
    let found = menus::shown_items().find(|i| {
        !i.separator
            && menus::shown_label(i.label()) == label
            && match i.path.len() {
                0 => false,
                1 => parent.is_empty(),
                n => menus::shown_label(&i.path[n - 2]) == parent,
            }
    });
    if let Some(i) = found.filter(|i| !i.identifier.is_empty()) {
        return Some(i.identifier.clone());
    }
    (parent == "Toolbelt" && crate::toolbelt::all_tool_names(&[]).iter().any(|t| t == label)).then(|| format!("Toolbelt.{label}"))
}

/// What the tip window does when a menu item is highlighted (`MenuItemTipController.swift:
/// 270-289`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// No tip for this item: hide any tip and start nothing.
    Hide,
    /// A tip is showing and the next item has one: replace it at once.
    ShowNow,
    /// Nothing showing yet: start the delay timer; a new highlight restarts it.
    Wait,
}

/// The step for a highlight, given whether a tip is showing and whether the item has one.
pub fn on_highlight(showing: bool, has_tip: bool) -> Step {
    match (has_tip, showing) {
        (false, _) => Step::Hide,
        (true, true) => Step::ShowNow,
        (true, false) => Step::Wait,
    }
}

/// `MenuTipDelay` as a delay: iTerm2's 1.0 s default, a bad value (NaN, negative) as the default,
/// and a huge one ("set it to a big number if you don't like the tips") capped at a day.
pub fn delay_of(seconds: f64) -> Duration {
    if seconds.is_finite() && seconds >= 0.0 { Duration::from_secs_f64(seconds.min(86400.0)) } else { Duration::from_secs(1) }
}

/// Where the tip window goes (`MenuItemTipController.swift:calculateTipPosition`): 8 px right of
/// the item's right edge, pulled left to stay inside the work area, level with the item's top and
/// pulled up to stay above the work area's bottom. Rectangles are (left, top, right, bottom).
pub fn tip_origin(item: (i32, i32, i32, i32), size: (i32, i32), work: (i32, i32, i32, i32)) -> (i32, i32) {
    let x = (item.2 + 8).min(work.2 - size.0 - 8).max(work.0);
    let y = item.1.min(work.3 - size.1).max(work.1);
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `AD:392-401`.
    #[test]
    fn split_hints_follow_iterm2() {
        assert_eq!(split_hint("splitHorizontally:", "Split Horizontally with Current Profile", true), "\u{2500}\u{20de} Split Horizontally with Current Profile");
        assert_eq!(split_hint("openSplitVerticallySheet:", "Split Vertically\u{2026}", true), "\u{2502}\u{20de} Split Vertically\u{2026}");
        assert_eq!(split_hint("openSplitHorizontallySheet:", "Split Horizontally\u{2026}", true), "\u{2500}\u{20de} Split Horizontally\u{2026}");
        assert_eq!(split_hint("splitVertically:", "Split Vertically with Current Profile", true), "\u{2502}\u{20de} Split Vertically with Current Profile");
        assert_eq!(split_hint("splitVertically:", "Split Vertically with Current Profile", false), "Split Vertically with Current Profile");
        assert_eq!(split_hint("newWindow:", "New Window", true), "New Window");
        // The menu text goes back to the shown label.
        assert_eq!(shown_from_menu_text("\u{2502}\u{20de} Split Vertically\u{2026}\tCtrl+Shift+D"), "Split Vertically\u{2026}");
        assert_eq!(shown_from_menu_text("Work && Play"), "Work & Play");
    }

    /// The 13v2 x 13w seam: a menu tip follows the one `hidden_in_menu` filter. No tip names an item
    /// this build hides, and the label lookup never answers a hidden item's identifier (Capture GPU
    /// Frame in release, Check Settings Control Truncation always).
    #[test]
    fn menu_tips_skip_hidden_items() {
        for i in menus::items().iter().filter(|i| !i.separator && menus::hidden_in_menu(&i.selector, cfg!(debug_assertions))) {
            assert!(tip_for(&i.identifier).is_none(), "{} is hidden but has a tip", i.identifier);
            assert_eq!(identifier_for(&menus::shown_label(i.label()), "Termurion"), None, "{}", i.identifier);
        }
        assert_eq!(identifier_for("Check Settings Control Truncation", "Termurion"), None);
        assert_eq!(identifier_for("Capture GPU Frame", "Termurion").is_some(), cfg!(debug_assertions));
    }

    /// The 13v2 x 13w seam: the Toggle Debug Logging tip names the file 13w's debug log writes
    /// (`dlog::FILE_NAME` in `%TEMP%`) and the folder used when `%TEMP%` is not a local folder
    /// (`dlog::log_dirs_with`).
    #[test]
    fn debug_log_tip_names_the_real_file() {
        let tip = tip_for("Toggle Debug Logging").unwrap();
        assert!(tip.contains(&format!("%TEMP%\\{}", crate::dlog::FILE_NAME)), "{tip}");
        assert!(tip.contains("%LOCALAPPDATA%\\Termurion\\Logs"), "{tip}");
        #[cfg(windows)]
        {
            let temp = std::env::temp_dir();
            let local = std::env::var("LOCALAPPDATA").unwrap();
            let t = temp.display().to_string();
            let dirs = crate::dlog::log_dirs_with(&|k| match k {
                "TEMP" => Some(t.clone()),
                "LOCALAPPDATA" => Some(local.clone()),
                _ => None,
            });
            assert_eq!(dirs.first().map(|d| d.path().to_path_buf()), Some(temp.clone()), "a local TEMP is the first folder");
            let fallback = std::path::Path::new(&local).join("Termurion").join("Logs");
            assert_eq!(dirs.last().map(|d| d.path().to_path_buf()), Some(fallback.clone()));
            let no_temp = crate::dlog::log_dirs_with(&|k| (k == "LOCALAPPDATA").then(|| local.clone()));
            assert_eq!(no_temp.iter().map(|d| d.path().to_path_buf()).collect::<Vec<_>>(), [fallback]);
        }
    }

    /// `ADS:24-196`: 69 tips; 58 name an item of the menu file by its identifier and 11 are the
    /// toolbelt's tools.
    #[test]
    fn menu_tips_follow_iterm2() {
        assert_eq!(TIPS.len(), 69);
        let in_menus = |id: &str| menus::shown_items().any(|i| !i.separator && !i.identifier.is_empty() && i.identifier == id);
        let (tools, items): (Vec<&MenuTip>, Vec<&MenuTip>) = TIPS.iter().partition(|t| t.identifier.starts_with("Toolbelt."));
        assert_eq!(items.len(), 58);
        for t in &items {
            assert!(in_menus(t.identifier), "{} names no menu item", t.identifier);
        }
        let tool_names: Vec<&str> = tools.iter().map(|t| &t.identifier["Toolbelt.".len()..]).collect();
        assert_eq!(tool_names, ["Actions", "Named Marks", "Captured Output", "Codecierge", "Command History", "Notes", "Paste History", "Profiles", "Recent Directories", "Snippets", "Session Status"]);
        for n in &tool_names {
            assert!(crate::toolbelt::all_tool_names(&[]).iter().any(|t| t == n), "{n} is no toolbelt tool");
        }
        for id in ["Pin Hotkey Window", "Split Vertically\u{2026}", "Broadcast Input.Show Background Pattern Indicator"] {
            assert!(tip_for(id).is_some(), "{id}");
        }
        // No identifier twice.
        let mut seen = std::collections::HashSet::new();
        assert!(TIPS.iter().all(|t| seen.insert(t.identifier)));
        // The lookup by label and parent.
        assert_eq!(identifier_for("Split Vertically with Current Profile", "Shell").as_deref(), Some("Split Vertically with Current Profile"));
        assert_eq!(identifier_for("Notes", "Toolbelt").as_deref(), Some("Toolbelt.Notes"));
        assert_eq!(identifier_for("Snippets", "Toolbelt").as_deref(), Some("Toolbelt.Snippets"));
        assert_eq!(identifier_for("Snippets", "Edit").as_deref(), Some("Edit.Snippets"), "the Edit submenu, not the tool");
        assert_eq!(identifier_for("Show Toolbelt", "Toolbelt").as_deref(), Some("Show Toolbelt"));
        assert_eq!(identifier_for("Toolbelt", "View").as_deref(), Some("Toolbelt"), "a submenu's own item");
        assert_eq!(identifier_for("Notes", "Edit"), None);
        assert_eq!(identifier_for("No Such Item", "Shell"), None);
        // Every tip is reachable by some item's label and parent.
        for t in TIPS.iter().filter(|t| !t.identifier.starts_with("Toolbelt.")) {
            let item = menus::shown_items().find(|i| !i.separator && i.identifier == t.identifier).unwrap();
            let parent = if item.path.len() > 1 { menus::shown_label(&item.path[item.path.len() - 2]) } else { String::new() };
            assert_eq!(identifier_for(&menus::shown_label(item.label()), &parent).as_deref(), Some(t.identifier), "{}", t.identifier);
        }
        // Termurion's words.
        assert!(tip_for("Toggle Debug Logging").unwrap().contains("%TEMP%\\debuglog.txt"));
        assert!(!tip_for("Toggle Debug Logging").unwrap().contains("/tmp"));
        assert!(tip_for("Password Manager").unwrap().contains("Windows Credential Manager"));
        assert_eq!(plain("Use **Import** to `x`"), "Use Import to x");
        for t in TIPS {
            assert!(!t.text.contains("iTerm2") && !t.text.to_lowercase().contains("keychain") && !t.text.contains("macOS"), "{}: {}", t.identifier, t.text);
            assert_eq!(plain(t.text), t.text, "{}: Markdown marks remain", t.identifier);
            assert!(!t.text.contains('\u{2014}') && !t.text.is_empty(), "{}", t.identifier);
        }
        // The one menu path the two split tips quote is a real Termurion item.
        let quoted = TIPS.iter().filter(|t| t.text.contains(" > ")).count();
        assert_eq!(quoted, 2, "a new quoted menu path needs a check here");
        assert!(menus::items().iter().any(|i| i.path == ["Session", "Edit Session..."]), "Session > Edit Session");
        // iTerm2 also says to unfold with Edit > Unfold in Selection, which Termurion's Edit menu lacks.
        assert!(tip_for("Fold Selected Lines").is_some_and(|t| !t.contains("Unfold in Selection")));
        // The right-click Unfold is Termurion's context menu item (semantic_ui.rs `CtxItem::Unfold`), so the clause stays.
        assert!(tip_for("Fold Selected Lines").is_some_and(|t| t.ends_with("by clicking the arrow in the margin, or right-clicking and choosing Unfold.")));
        assert_eq!(COPY_MODE_URL, "https://iterm2.com/documentation-copymode.html");
    }

    #[test]
    fn tips_stay_in_the_work_area() {
        let work = (0, 0, 1000, 800);
        assert_eq!(tip_origin((10, 100, 200, 120), (250, 60), work), (208, 100), "beside the item");
        assert_eq!(tip_origin((10, 100, 900, 120), (250, 60), work), (742, 100), "pulled left");
        assert_eq!(tip_origin((10, 790, 200, 810), (250, 60), work), (208, 740), "pulled up");
        assert_eq!(tip_origin((10, -50, 200, -30), (250, 60), work), (208, 0), "never above the area");
    }

    /// The tip window's rule (`MenuItemTipController.swift:270-289`).
    #[test]
    fn highlight_follows_iterm2() {
        assert_eq!(on_highlight(false, true), Step::Wait);
        assert_eq!(on_highlight(true, true), Step::ShowNow);
        assert_eq!(on_highlight(true, false), Step::Hide);
        assert_eq!(on_highlight(false, false), Step::Hide);
        assert_eq!(delay_of(1.0), Duration::from_secs(1));
        assert_eq!(delay_of(0.25), Duration::from_millis(250));
        assert_eq!(delay_of(f64::NAN), Duration::from_secs(1));
        assert_eq!(delay_of(-3.0), Duration::from_secs(1));
        assert_eq!(delay_of(1e12), Duration::from_secs(86400));
    }
}
