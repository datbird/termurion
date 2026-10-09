//! The page context menu's model (milestone 13x2, Task 3): iTerm2's browser menu
//! (`iTermBrowserContextMenuHelper.decorate`), as the items to show, their texts and what the
//! WebView2 host is told. Pure: no web view and no App. Web content is untrusted: a menu text that
//! comes from a page (a helper title, a Smart Selection action title) passes `display_text` and is
//! cut before it is shown, and an item that copies text copies exactly the selection.

use crate::webhost::{MenuEntry, MenuTarget};

/// The longest menu label a page's text may make (P15).
pub const MAX_LABEL: usize = 120;

/// What a right-click landed on, in the terms of iTerm2's menu helper: the link, the image, the
/// trimmed selection, whether a field is being edited, and where (page coordinates).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ctx {
    pub link: Option<String>,
    pub image: Option<String>,
    pub selection: Option<String>,
    pub editable: bool,
    pub page_url: String,
    pub x: f64,
    pub y: f64,
    /// The titles of the selection helpers and of the Smart Selection actions (Task 4 fills them).
    pub smart_actions: Vec<String>,
    pub helpers: Vec<String>,
    /// What each helper copies, and each Smart Selection action with its captures, as they were
    /// when the menu was built (the items are positions into these).
    pub helper_data: Vec<crate::ctx_help::Helper>,
    pub smart_data: Vec<SmartPick>,
}

/// A Smart Selection action offered in the menu, with the captures of the match that offered it.
#[derive(Clone, Debug, PartialEq)]
pub struct SmartPick {
    pub action: crate::smartsel::RuleAction,
    pub captures: Vec<String>,
}

/// "Run command?" (`PSB:301-307`).
pub const RUN_HEADING: &str = "Run command?";
/// "Open file?" (`PSB:336-342`).
pub const OPEN_FILE_HEADING: &str = "Open file?";

/// The text of the ask before a command runs.
pub fn run_text(command: &str) -> String {
    format!("OK to run:\n{command}")
}

/// The text of the ask before a file opens.
pub fn open_file_text(file: &str) -> String {
    format!("OK to open this file?\n{file}")
}

/// The longest command or file name the ask shows whole.
pub const MAX_SHOWN: usize = 400;

/// Whether `text` shows in an ask exactly as it is: no hidden character and no cut. What the person
/// cannot read in full, a page's text does not get to run or open (a page can pad a line).
pub fn shown_whole(text: &str) -> bool {
    crate::inert_files::display_text(text, MAX_SHOWN, false) == text
}

/// What a Smart Selection action does from a page (`BWV:554-602`), decided before anything runs.
#[derive(Clone, Debug, PartialEq)]
pub enum SmartPlan {
    Copy(String),
    OpenFile(String),
    OpenUrl(String),
    /// A command line built by the one cmd.exe builder, to run after the ask.
    Run(String),
    /// Nothing runs, and why (the builder's refusal).
    Refused(String),
    /// "runCommand:", "runCoprocess:", "sendText:" and unknown actions do nothing (`BWV:573-575`).
    Nothing,
}

/// The plan of one action with the captures of a page's text. The parameter is built by
/// `smartsel::parameter` (no interpolation for a page; the host is the page's, the folder and user
/// empty), so a value a command line would read as syntax refuses the whole action (B5).
pub fn smart_plan(a: &crate::smartsel::RuleAction, captures: &[String], page_host: &str) -> SmartPlan {
    if !matches!(a.action.as_str(), "copy:" | "openFile:" | "openUrl:" | "runCommandInWindow:") {
        return SmartPlan::Nothing;
    }
    let eval = |_: &str, _: &[Option<&str>], _: bool| Err("A page's Smart Selection action does not use interpolated strings.".to_string());
    match crate::smartsel::parameter(a, captures, false, &eval, "", page_host, "") {
        Err(why) => SmartPlan::Refused(why),
        Ok(p) => match a.action.as_str() {
            "copy:" => SmartPlan::Copy(p),
            "openFile:" => SmartPlan::OpenFile(p),
            "openUrl:" => SmartPlan::OpenUrl(p),
            _ => SmartPlan::Run(p),
        },
    }
}

/// The menu's Smart Selection items for a selection (`BCM:101-110`, `:194-212`): one per action of
/// each rule that matches it, in rule order, titled with the action's own title. A selection of
/// `MAX_CUSTOM_ACTIONS_TEXT` characters or more gets none.
pub fn smart_entries(selection: &str, rules: &[crate::smartsel::Rule]) -> (Vec<String>, Vec<SmartPick>) {
    let mut titles = Vec::new();
    let mut picks = Vec::new();
    if selection.chars().count() >= crate::ctx_help::MAX_CUSTOM_ACTIONS_TEXT {
        return (titles, picks);
    }
    for m in crate::browser_smart::all_matches(rules, selection) {
        for action in rules.get(m.rule).map(|r| r.actions.as_slice()).unwrap_or_default() {
            titles.push(action.title.clone());
            picks.push(SmartPick { action: action.clone(), captures: m.captures.clone() });
        }
    }
    (titles, picks)
}

/// One item of the menu. `Native` marks where WebView2's own items go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Helper(usize),
    SmartAction(usize),
    SplitVertically,
    SplitHorizontally,
    MoveToSplitPane,
    MoveToTab,
    MoveToWindow,
    SwapWithSession,
    Native,
    SearchWith,
    AddNamedMark,
    SavePageAs,
    Print,
    CopyPageTitle,
    ViewSource,
    RemoveElement,
}

/// Which of the later plans' items exist. An item that is not built is never offered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Built {
    pub named_marks: bool,
    pub save_page: bool,
    pub view_source: bool,
    pub remove_element: bool,
    pub print: bool,
}

/// iTerm2's menu (`BCM:60-192`) as a list: None is a separator. The helpers and their separator, the
/// Smart Selection actions and theirs, the split and move items (Move to Tab and Window only when the
/// tab has more sessions), Swap With Session, a separator, the native items (then Search with the
/// engine, for a selection; WebView2 has no web search item to replace, so it follows the native
/// block), a separator, Add Named Mark, a separator, Save Page As, Print, Copy Page Title, a
/// separator, View Source and Remove Element. Unbuilt items are left out and separators collapse:
/// none first, none last, never two together.
pub fn items(ctx: &Ctx, built: &Built, search_engine: Option<&str>, multiple_sessions: bool) -> Vec<Option<Item>> {
    let mut v: Vec<Option<Item>> = Vec::new();
    if !ctx.helpers.is_empty() {
        v.extend((0..ctx.helpers.len()).map(|i| Some(Item::Helper(i))));
        v.push(None);
    }
    if !ctx.smart_actions.is_empty() {
        v.extend((0..ctx.smart_actions.len()).map(|i| Some(Item::SmartAction(i))));
        v.push(None);
    }
    v.extend([Some(Item::SplitVertically), Some(Item::SplitHorizontally), Some(Item::MoveToSplitPane)]);
    if multiple_sessions {
        v.extend([Some(Item::MoveToTab), Some(Item::MoveToWindow)]);
    }
    v.extend([Some(Item::SwapWithSession), None, Some(Item::Native)]);
    if search_engine.is_some() && ctx.selection.is_some() {
        v.push(Some(Item::SearchWith));
    }
    v.push(None);
    if built.named_marks {
        v.push(Some(Item::AddNamedMark));
    }
    v.push(None);
    if built.save_page {
        v.push(Some(Item::SavePageAs));
    }
    if built.print {
        v.push(Some(Item::Print));
    }
    v.push(Some(Item::CopyPageTitle));
    v.push(None);
    if built.view_source {
        v.push(Some(Item::ViewSource));
    }
    if built.remove_element {
        v.push(Some(Item::RemoveElement));
    }
    let mut out: Vec<Option<Item>> = Vec::with_capacity(v.len());
    for i in v {
        if i.is_none() && out.last().is_none_or(|l| l.is_none()) {
            continue;
        }
        out.push(i);
    }
    while out.last().is_some_and(|l| l.is_none()) {
        out.pop();
    }
    out
}

/// An item's text: iTerm2's, with "Tandem" and the Windows words where they differ. A helper's or a
/// Smart Selection action's title is the text the caller gave (made inert in `entries`).
pub fn title(item: Item, ctx: &Ctx, search_engine: Option<&str>) -> String {
    match item {
        Item::Helper(i) => ctx.helpers.get(i).cloned().unwrap_or_default(),
        Item::SmartAction(i) => ctx.smart_actions.get(i).cloned().unwrap_or_default(),
        Item::SplitVertically => "Split Pane Vertically".into(),
        Item::SplitHorizontally => "Split Pane Horizontally".into(),
        Item::MoveToSplitPane => "Move Browser to Split Pane".into(),
        Item::MoveToTab => "Move Browser to Tab".into(),
        Item::MoveToWindow => "Move Browser to Window".into(),
        Item::SwapWithSession => "Swap With Session\u{2026}".into(),
        Item::Native => String::new(),
        Item::SearchWith => format!("Search with {}", search_engine.unwrap_or_default()),
        Item::AddNamedMark => "Add Named Mark\u{2026}".into(),
        Item::SavePageAs => "Save Page As\u{2026}".into(),
        Item::Print => "Print\u{2026}".into(),
        Item::CopyPageTitle => "Copy Page Title".into(),
        Item::ViewSource => "View Source".into(),
        Item::RemoveElement => "Remove Element".into(),
    }
}

/// The search engine the `SearchCommand` names (`BM:962-1016`): the last two labels of the template's
/// host, one of iTerm2's 20 engines. A template with no scheme, or a host of one label, has none.
pub fn search_engine_name(search_command: &str) -> Option<&'static str> {
    let rest = search_command.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let domain = format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
    Some(match domain.as_str() {
        "google.com" => "Google",
        "bing.com" => "Bing",
        "baidu.com" => "Baidu",
        "yandex.com" => "Yandex",
        "duckduckgo.com" => "DuckDuckGo",
        "ecosia.org" => "Ecosia",
        "startpage.com" => "Startpage",
        "searxng.org" => "SearXNG",
        "swisscows.ch" => "Swisscows",
        "dogpile.com" => "Dogpile",
        "metacrawler.com" => "MetaCrawler",
        "mojeek.com" => "Mojeek",
        "kagi.com" => "Kagi",
        "qwant.com" => "Qwant",
        "excite.com" => "Excite",
        "hotbot.com" => "HotBot",
        "lycos.com" => "Lycos",
        "sogou.com" => "Sogou",
        "youdao.com" => "Youdao",
        "webcrawler.com" => "WebCrawler",
        _ => return None,
    })
}

/// The context of a right-click. The location is in physical pixels of the view: dividing by the
/// window's scale and the page's zoom gives page coordinates. The selection is trimmed the way
/// iTerm2 reads it (whitespace and newlines off both ends; blank is none); the image is the target's
/// source for an image only.
pub fn ctx_of(t: &MenuTarget, scale: f64, zoom: f64) -> Ctx {
    let div = |v: i32| {
        let d = scale * zoom;
        if d.is_finite() && d > 0.0 { f64::from(v) / d } else { f64::from(v) }
    };
    Ctx {
        link: t.link.clone().filter(|l| !l.is_empty()),
        image: if t.kind == 1 { t.source.clone().filter(|s| !s.is_empty()) } else { None },
        selection: t.selection.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string),
        editable: t.editable,
        page_url: t.page_uri.clone(),
        x: div(t.x),
        y: div(t.y),
        ..Ctx::default()
    }
}

/// What Copy Page Title copies (`BWV:919-927`): the title, else the URL, else nothing.
pub fn copy_title_text(title: &str, url: &str) -> Option<String> {
    let s = if title.is_empty() { url } else { title };
    if s.is_empty() { None } else { Some(s.to_string()) }
}

/// A label a page's text made, safe to show (P15): hidden characters out (`display_text`), cut to
/// `MAX_LABEL`, "&" doubled so no page text sets an access key; one that comes out empty shows its first character's code point. Empty stays empty.
fn safe_label(raw: &str) -> String {
    let clean = crate::inert_files::display_text(raw, MAX_LABEL, false);
    if clean.trim().is_empty() {
        return raw.chars().next().map(|c| format!("U+{:04X}", c as u32)).unwrap_or_default();
    }
    // WebView2's menus read "&" as the access-key marker: "&&" shows one.
    clean.replace('&', "&&")
}

/// A color helper's swatch (`CMU:343-350`): 24 by 24 pixels of the color with a 1-pixel border, light
/// gray on a dark color and dark gray on a light one, as PNG.
pub fn swatch_png(rgb: (u8, u8, u8)) -> Vec<u8> {
    let luma = (0.299 * f64::from(rgb.0) + 0.587 * f64::from(rgb.1) + 0.114 * f64::from(rgb.2)) / 255.0;
    let edge = if luma < 0.5 { 204 } else { 51 };
    let mut img = image::RgbaImage::new(24, 24);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let border = x == 0 || y == 0 || x == 23 || y == 23;
        *p = if border { image::Rgba([edge, edge, edge, 255]) } else { image::Rgba([rgb.0, rgb.1, rgb.2, 255]) };
    }
    let mut out = Vec::new();
    let _ = image::DynamicImage::ImageRgba8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png);
    out
}

/// The entries for `WebView::answer_context_menu`: each custom item with its id (its position plus
/// one), `Native` where WebView2's own items go. Every label passes `safe_label`; an item with no
/// label at all (a helper index the list does not have) is left out.
pub fn entries(items: &[Option<Item>], ctx: &Ctx, search_engine: Option<&str>) -> Vec<MenuEntry> {
    let mut out = Vec::with_capacity(items.len());
    for (at, item) in items.iter().enumerate() {
        match item {
            None => out.push(MenuEntry::Separator),
            Some(Item::Native) => out.push(MenuEntry::Native),
            Some(i) => {
                let label = safe_label(&title(*i, ctx, search_engine));
                if label.is_empty() {
                    continue;
                }
                // A helper that copies nothing (a color, a byte count) is shown, not offered; a
                // color has its swatch beside the label.
                let helper = if let Item::Helper(n) = i { ctx.helper_data.get(*n) } else { None };
                let enabled = helper.is_none_or(|h| h.copy.is_some());
                let icon_png = helper.and_then(|h| h.color).map(swatch_png);
                out.push(MenuEntry::Custom { id: at as u32 + 1, label, enabled, icon_png });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webhost::{MenuEntry, MenuTarget};

    fn ctx() -> Ctx {
        Ctx::default()
    }

    fn with_selection() -> Ctx {
        Ctx { selection: Some("abc".into()), helpers: vec!["h0".into(), "h1".into()], smart_actions: vec!["s0".into()], ..Ctx::default() }
    }

    /// Review Focus 2 (`BCM:60-192`).
    #[test]
    fn menu_follows_iterm2() {
        use Item::*;
        let base = items(&ctx(), &Built::default(), None, false);
        assert_eq!(base, vec![Some(SplitVertically), Some(SplitHorizontally), Some(MoveToSplitPane), Some(SwapWithSession), None, Some(Native), None, Some(CopyPageTitle)]);
        let multi = items(&ctx(), &Built::default(), None, true);
        assert_eq!(&multi[..6], &[Some(SplitVertically), Some(SplitHorizontally), Some(MoveToSplitPane), Some(MoveToTab), Some(MoveToWindow), Some(SwapWithSession)]);
        let print = items(&ctx(), &Built { print: true, ..Built::default() }, None, false);
        assert_eq!(&print[print.len() - 3..], &[None, Some(Print), Some(CopyPageTitle)]);
        let all = Built { named_marks: true, save_page: true, view_source: true, remove_element: true, print: true };
        let full = items(&ctx(), &all, None, false);
        assert_eq!(&full[full.len() - 9..], &[None, Some(AddNamedMark), None, Some(SavePageAs), Some(Print), Some(CopyPageTitle), None, Some(ViewSource), Some(RemoveElement)]);
        // The native block is first after the split and move items (iTerm2 inserts them at the top).
        let sel = items(&with_selection(), &Built::default(), None, false);
        assert_eq!(&sel[..6], &[Some(Helper(0)), Some(Helper(1)), None, Some(SmartAction(0)), None, Some(SplitVertically)]);
        // Search with <engine>: only with an engine and a selection, right after the native items.
        let search = items(&with_selection(), &Built::default(), Some("Google"), false);
        let at = search.iter().position(|i| *i == Some(Native)).unwrap();
        assert_eq!(search[at + 1], Some(SearchWith));
        assert!(!items(&with_selection(), &Built::default(), None, false).contains(&Some(SearchWith)));
        assert!(!items(&ctx(), &Built::default(), Some("Google"), false).contains(&Some(SearchWith)));
        // No two separators meet and none starts or ends the list, in every combination.
        for bits in 0..64u32 {
            let built = Built { named_marks: bits & 1 != 0, save_page: bits & 2 != 0, view_source: bits & 4 != 0, remove_element: bits & 8 != 0, print: bits & 16 != 0 };
            for c in [ctx(), with_selection()] {
                let v = items(&c, &built, if bits & 32 != 0 { Some("Bing") } else { None }, bits % 2 == 0);
                assert!(v.first().is_some_and(|i| i.is_some()) && v.last().is_some_and(|i| i.is_some()), "{v:?}");
                assert!(v.windows(2).all(|w| w[0].is_some() || w[1].is_some()), "{v:?}");
            }
        }
    }

    #[test]
    fn titles_follow_iterm2() {
        use Item::*;
        let c = Ctx { helpers: vec!["Hex".into()], smart_actions: vec!["Open".into()], ..Ctx::default() };
        let t = |i| title(i, &c, Some("Google"));
        assert_eq!(t(SplitVertically), "Split Pane Vertically");
        assert_eq!(t(SplitHorizontally), "Split Pane Horizontally");
        assert_eq!(t(MoveToSplitPane), "Move Browser to Split Pane");
        assert_eq!(t(MoveToTab), "Move Browser to Tab");
        assert_eq!(t(MoveToWindow), "Move Browser to Window");
        assert_eq!(t(SwapWithSession), "Swap With Session\u{2026}");
        assert_eq!(t(SearchWith), "Search with Google");
        assert_eq!(t(AddNamedMark), "Add Named Mark\u{2026}");
        assert_eq!(t(SavePageAs), "Save Page As\u{2026}");
        assert_eq!(t(Print), "Print\u{2026}");
        assert_eq!(t(CopyPageTitle), "Copy Page Title");
        assert_eq!(t(ViewSource), "View Source");
        assert_eq!(t(RemoveElement), "Remove Element");
        assert_eq!(t(Helper(0)), "Hex");
        assert_eq!(t(SmartAction(0)), "Open");
        assert_eq!(t(Helper(9)), "");
    }

    #[test]
    fn search_engine_names_follow_iterm2() {
        assert_eq!(search_engine_name("https://google.com/search?q=%@"), Some("Google"));
        assert_eq!(search_engine_name("https://www.bing.com/search?q=%@"), Some("Bing"));
        assert_eq!(search_engine_name("https://duckduckgo.com/?q=%@"), Some("DuckDuckGo"));
        assert_eq!(search_engine_name("https://kagi.com/search?q=%@"), Some("Kagi"));
        assert_eq!(search_engine_name("https://search.example.org/?q=%@"), None);
        assert_eq!(search_engine_name("localhost"), None);
        assert_eq!(search_engine_name("https://localhost/?q=%@"), None);
        let all = [
            ("google.com", "Google"), ("bing.com", "Bing"), ("baidu.com", "Baidu"), ("yandex.com", "Yandex"), ("duckduckgo.com", "DuckDuckGo"), ("ecosia.org", "Ecosia"), ("startpage.com", "Startpage"),
            ("searxng.org", "SearXNG"), ("swisscows.ch", "Swisscows"), ("dogpile.com", "Dogpile"), ("metacrawler.com", "MetaCrawler"), ("mojeek.com", "Mojeek"), ("kagi.com", "Kagi"), ("qwant.com", "Qwant"),
            ("excite.com", "Excite"), ("hotbot.com", "HotBot"), ("lycos.com", "Lycos"), ("sogou.com", "Sogou"), ("youdao.com", "Youdao"), ("webcrawler.com", "WebCrawler"),
        ];
        assert_eq!(all.len(), 20);
        for (domain, name) in all {
            assert_eq!(search_engine_name(&format!("https://www.{domain}/s?q=%@")), Some(name), "{domain}");
            assert_eq!(search_engine_name(&format!("https://{}:8080/s?q=%@", domain.to_uppercase())), Some(name), "{domain} with a port, in capitals");
        }
        // A host that only ends in a known domain's text is not that engine.
        assert_eq!(search_engine_name("https://notgoogle.com/?q=%@"), None);
        assert_eq!(search_engine_name("https://google.com.evil.example/?q=%@"), None);
        // The userinfo is not the host.
        assert_eq!(search_engine_name("https://google.com@evil.example/?q=%@"), None);
    }

    #[test]
    fn copy_title_follows_iterm2() {
        assert_eq!(copy_title_text("T", "u"), Some("T".into()));
        assert_eq!(copy_title_text("", "https://x"), Some("https://x".into()));
        assert_eq!(copy_title_text("", ""), None);
    }

    #[test]
    fn ctx_of_follows_iterm2() {
        let t = MenuTarget { kind: 1, source: Some("https://i/p.png".into()), selection: Some("  a b \n".into()), x: 300, y: 150, page_uri: "https://p/".into(), ..MenuTarget::default() };
        assert_eq!(ctx_of(&t, 1.5, 2.0), Ctx { image: Some("https://i/p.png".into()), selection: Some("a b".into()), x: 100.0, y: 50.0, page_url: "https://p/".into(), ..Ctx::default() });
        // A source is an image only for an image target; a blank selection is none; a link is kept.
        let t = MenuTarget { kind: 0, source: Some("https://i/p.png".into()), selection: Some(" \n ".into()), link: Some("https://l/".into()), editable: true, ..MenuTarget::default() };
        let c = ctx_of(&t, 1.0, 1.0);
        assert_eq!((c.image, c.selection, c.link, c.editable), (None, None, Some("https://l/".into()), true));
        // A zero scale or zoom never divides by zero.
        let c = ctx_of(&MenuTarget { x: 10, y: 20, ..MenuTarget::default() }, 0.0, 0.0);
        assert!(c.x.is_finite() && c.y.is_finite());
    }

    #[test]
    fn entries_follow_the_items() {
        use Item::*;
        let e = entries(&[Some(SplitVertically), None, Some(Native)], &ctx(), None);
        assert_eq!(e, vec![MenuEntry::Custom { id: 1, label: "Split Pane Vertically".into(), enabled: true, icon_png: None }, MenuEntry::Separator, MenuEntry::Native]);
    }

    /// P15: a label a page chose is made inert and cut; the ids are positions.
    #[test]
    fn page_labels_are_inert_and_bounded() {
        use Item::*;
        let evil = format!("a\u{202e}b\u{200b}c{}", "x".repeat(500));
        let c = Ctx { helpers: vec![evil.clone(), "\u{200b}".into()], smart_actions: vec![evil], ..Ctx::default() };
        let e = entries(&[Some(Helper(0)), Some(Helper(1)), Some(SmartAction(0)), Some(Native)], &c, None);
        let labels: Vec<&String> = e.iter().filter_map(|m| if let MenuEntry::Custom { label, .. } = m { Some(label) } else { None }).collect();
        assert_eq!(labels.len(), 3);
        for l in &labels {
            assert!(!l.contains('\u{202e}') && !l.contains('\u{200b}'), "{l:?}");
            assert!(l.chars().count() <= 120, "{}", l.len());
            assert!(!l.is_empty());
        }
        assert!(matches!(e[2], MenuEntry::Custom { id: 3, .. }));
    }

    /// Task 3 review minor 1: WebView2's menus read "&" as the access-key marker, so a label a page
    /// chose shows its ampersands as "&&" and no page can give its item an access key.
    #[test]
    fn page_labels_escape_the_access_key_marker() {
        use Item::*;
        let c = Ctx { helpers: vec!["a&b".into(), "&&x&".into(), "\u{200b}&".into()], smart_actions: vec!["R&D".into()], helper_data: vec![crate::ctx_help::Helper { title: String::new(), copy: Some(crate::ctx_help::Copy::Text("t".into())), color: None }; 3], ..Ctx::default() };
        let e = entries(&[Some(Helper(0)), Some(Helper(1)), Some(Helper(2)), Some(SmartAction(0))], &c, None);
        let labels: Vec<String> = e.iter().filter_map(|m| if let MenuEntry::Custom { label, .. } = m { Some(label.clone()) } else { None }).collect();
        assert_eq!(labels, vec!["a&&b".to_string(), "&&&&x&&".into(), "&&".into(), "R&&D".into()]);
        // The fixed items have none.
        assert!(!entries(&[Some(SplitVertically)], &Ctx::default(), None).iter().any(|m| matches!(m, MenuEntry::Custom { label, .. } if label.contains('&'))));
    }

    fn act(action: &str, parameter: &str) -> crate::smartsel::RuleAction {
        crate::smartsel::RuleAction { title: "t".into(), action: action.into(), parameter: parameter.into() }
    }

    /// `BWV:554-602`, `PSB:300-346` and the one command path (B5): what each action does with the
    /// captures a page's text gave.
    #[test]
    fn smart_actions_follow_iterm2() {
        assert_eq!(RUN_HEADING, "Run command?");
        assert_eq!(OPEN_FILE_HEADING, "Open file?");
        assert_eq!(run_text("ls"), "OK to run:\nls");
        assert_eq!(open_file_text("C:\\a.txt"), "OK to open this file?\nC:\\a.txt");
        let caps = |c: &str| vec![c.to_string(), c.to_string()];
        let plan = |a: &str, p: &str, c: &str| smart_plan(&act(a, p), &caps(c), "example.com");
        assert_eq!(plan("copy:", "", "abc"), SmartPlan::Copy("abc".into()));
        assert_eq!(plan("copy:", "x\\1", "abc"), SmartPlan::Copy("xabc".into()));
        assert_eq!(plan("openFile:", "\\1", "C:\\a.txt"), SmartPlan::OpenFile("C:\\a.txt".into()));
        assert_eq!(plan("openUrl:", "https://\\h/\\1", "a b"), SmartPlan::OpenUrl("https://example.com/a b".into()));
        assert_eq!(plan("runCommandInWindow:", "echo \\1", "hi"), SmartPlan::Run("echo \"hi\"".into()));
        // Not supported in a page (BWV:573-575).
        for a in ["runCommand:", "runCoprocess:", "sendText:", "bogus:"] {
            assert_eq!(plan(a, "x", "y"), SmartPlan::Nothing, "{a}");
        }
        // Page text that a command line would read as syntax runs nothing (13p-r, B5).
        for bad in ["x%CMDCMDLINE%&calc", "a^b", "a|b", "a<b", "a>b", "a\"b", "a\u{7}b", "a!b", "a(b", "a)b", "a\u{201c}b", "a\u{ff02}b", "a\\", "a`b"] {
            assert_eq!(plan("runCommandInWindow:", "echo \\1", bad), SmartPlan::Refused(crate::semantic::CMD_REFUSED.into()), "{bad:?}");
        }
        assert_eq!(plan("runCommandInWindow:", "echo ok", "x&y"), SmartPlan::Run("echo ok".into()), "only a substituted value is judged");
    }

    /// P15: an action's title is page-independent text from the profile, still made inert; the
    /// menu's data is a snapshot taken with the menu.
    #[test]
    fn color_swatches_follow_iterm2() {
        let px = |rgb: (u8, u8, u8)| {
            let img = image::load_from_memory(&swatch_png(rgb)).unwrap().to_rgba8();
            assert_eq!(img.dimensions(), (24, 24));
            (img.get_pixel(0, 5).0, img.get_pixel(12, 12).0)
        };
        assert_eq!(px((0, 0, 0)), ([204, 204, 204, 255], [0, 0, 0, 255]), "light gray on a dark color");
        assert_eq!(px((255, 128, 0)), ([51, 51, 51, 255], [255, 128, 0, 255]), "dark gray on a light one");
        let c = Ctx { helpers: vec!["#ff8000".into(), "5 hex bytes".into(), "255 = 0xff".into()], helper_data: vec![crate::ctx_help::color("#ff8000").unwrap(), crate::ctx_help::Helper { title: "5 hex bytes".into(), copy: None, color: None }, crate::ctx_help::hex_or_decimal("255").unwrap()], ..Ctx::default() };
        let e = entries(&[Some(Item::Helper(0)), Some(Item::Helper(1)), Some(Item::Helper(2))], &c, None);
        let flags: Vec<(bool, bool)> = e.iter().map(|m| if let MenuEntry::Custom { enabled, icon_png, .. } = m { (*enabled, icon_png.is_some()) } else { (false, false) }).collect();
        assert_eq!(flags, vec![(false, true), (false, false), (true, false)]);
    }

    #[test]
    fn what_the_person_cannot_read_does_not_run() {
        assert!(shown_whole("echo \"hi\""));
        assert!(!shown_whole("echo hi\u{202e}"));
        assert!(!shown_whole(&format!("echo {}", " ".repeat(500))));
        assert!(!shown_whole("a\nb"));
    }

    #[test]
    fn smart_menu_entries_follow_the_rules() {
        let rules = vec![crate::smartsel::Rule { regex: r"\d+".into(), precision: 1.0, notes: String::new(), actions: vec![act("copy:", ""), act("openUrl:", "u")] }, crate::smartsel::Rule { regex: "zzz".into(), precision: 1.0, notes: String::new(), actions: vec![act("copy:", "")] }];
        let (titles, picks) = smart_entries("go 42", &rules);
        assert_eq!(titles, vec!["t".to_string(), "t".to_string()]);
        assert_eq!(picks.len(), 2);
        assert_eq!(picks[0].captures, vec!["42".to_string()]);
        // Over 400 characters: no actions at all (CMH:39).
        assert!(smart_entries(&"1".repeat(400), &rules).0.is_empty());
        assert!(!smart_entries(&"1".repeat(399), &rules).0.is_empty());
    }
}
