//! The page context menu (milestone 13x2, Task 3): a right-click on a browser pane's page is
//! answered with iTerm2's menu (`browser_ctx`), and a pick runs here. Web content is untrusted: the
//! menu's page-derived text is made inert in `browser_ctx::entries`, every URL a pick opens goes
//! through `browser_page_open`, a copy copies exactly the selection or the page title and nothing a
//! page chose to put there, and no item runs a program.

use super::*;
use crate::browser_ctx::{self, Built, Ctx, Item};
use crate::webhost::MenuTarget;

impl App {
    /// Which items exist: 13x2 builds Print; later plans set theirs.
    pub(super) fn browser_ctx_built(&self) -> Built {
        Built { print: true, ..Built::default() }
    }

    /// A right-click on pane `pane`'s page: the pane becomes the focused one (iTerm2's menu acts on
    /// the clicked session), the menu is built and WebView2 is answered. The helpers and Smart
    /// Selection actions are Task 4's and are empty until then.
    pub(super) fn browser_context_menu(&mut self, pane: PaneId, token: u64, target: MenuTarget) {
        let engine = browser_ctx::search_engine_name(&self.globals.string("SearchCommand"));
        let zoom = self.panes.get(&pane).and_then(|p| p.browser_io()).map_or(1.0, |io| io.zoom);
        let mut scale = 1.0;
        let mut multiple = false;
        self.in_pane_window(pane, |a| {
            if let Some(t) = a.tabs.iter_mut().find(|t| t.layout.panes().contains(&pane)) {
                t.focused = pane;
                multiple = t.layout.panes().len() > 1;
            }
            scale = f64::from(a.scale());
            a.redraw();
        });
        let ctx = browser_ctx::ctx_of(&target, scale, zoom);
        let built = self.browser_ctx_built();
        let items = browser_ctx::items(&ctx, &built, engine, multiple);
        let entries = browser_ctx::entries(&items, &ctx, engine);
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        bv.menu = Some((token, ctx, items));
        if let Some(v) = &bv.view {
            v.answer_context_menu(token, entries);
        }
    }

    /// A custom item of the menu with `token` was chosen (`id` is its position plus one).
    pub(super) fn browser_ctx_event(&mut self, pane: PaneId, token: u64, id: u32, event_loop: &ActiveEventLoop) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        if !bv.menu.as_ref().is_some_and(|(t, _, _)| *t == token) {
            return;
        }
        let Some((_, ctx, items)) = bv.menu.take() else { return };
        let Some(Some(item)) = (id as usize).checked_sub(1).and_then(|i| items.get(i)).copied() else { return };
        self.browser_ctx_picked(pane, item, &ctx, event_loop);
    }

    /// Runs one item. Every pick first makes the pane the focused one: the split and move calls act
    /// on the focused pane.
    pub(super) fn browser_ctx_picked(&mut self, pane: PaneId, item: Item, ctx: &Ctx, event_loop: &ActiveEventLoop) {
        let guid = self.panes.get(&pane).map(|p| p.profile.clone());
        let search = self.globals.string("SearchCommand");
        let (title, url) = {
            let io = self.panes.get(&pane).and_then(|p| p.browser_io());
            let last = self.browser_views.get(&pane).and_then(|b| b.last_title.clone());
            (last.or_else(|| io.map(|i| i.title.clone())).unwrap_or_default(), io.map(|i| i.url.clone()).unwrap_or_default())
        };
        self.in_pane_window(pane, |a| {
            if let Some(t) = a.tabs.iter_mut().find(|t| t.layout.panes().contains(&pane)) {
                t.focused = pane;
            }
            match item {
                Item::SplitVertically => a.split(Axis::Row, guid.as_deref()),
                Item::SplitHorizontally => a.split(Axis::Column, guid.as_deref()),
                Item::MoveToSplitPane => a.start_pane_move(pane, true, event_loop),
                Item::SwapWithSession => a.start_pane_move(pane, false, event_loop),
                Item::MoveToTab => a.move_session_to_tab(),
                Item::MoveToWindow => a.move_session_to_window(),
                Item::SearchWith => {
                    if let Some(sel) = &ctx.selection {
                        let target = crate::browser::search_url(&search, sel);
                        a.browser_page_open(pane, &target, super::browser_scripts_app::PageOpen::Tab, event_loop);
                    }
                }
                Item::Print => {
                    if let Some(v) = a.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                        v.print_ui();
                    }
                }
                Item::CopyPageTitle => {
                    if let Some(text) = browser_ctx::copy_title_text(&title, &url) {
                        a.set_clipboard_text(text.clone());
                        a.remember_paste(&text);
                    }
                }
                // Task 4 (helpers, Smart Selection actions) and the later plans' items.
                Item::Helper(_) | Item::SmartAction(_) | Item::AddNamedMark | Item::SavePageAs | Item::ViewSource | Item::RemoveElement | Item::Native => {}
            }
        });
    }
}

#[cfg(test)]
mod tests {
    fn prod() -> &'static str {
        let src = include_str!("browser_ctx_app.rs");
        &src[..src.find("#[cfg(test)]").unwrap()]
    }

    /// Every pick makes the pane the focused one before it acts; the page's URLs open only through
    /// the one funnel; nothing here starts a program.
    #[test]
    fn picks_focus_first_and_open_through_the_funnel() {
        let p = prod();
        let run = &p[p.find("pub(super) fn browser_ctx_picked").unwrap()..];
        assert!(run.find("t.focused = pane;").unwrap() < run.find("match item").unwrap());
        assert_eq!(run.matches("a.browser_page_open(").count(), 1);
        for banned in ["open_url_in_browser(", "open_link(", "execute(", "execute_with(", "Command::new(", "guard_open(", "shell_open("] {
            assert!(!p.contains(banned), "{banned}");
        }
        // Copy Page Title is the only copy and it copies the title or the URL, never the context's text.
        assert_eq!(run.matches("set_clipboard_text(").count(), 1);
    }

    /// A pick acts only for the menu that is current and an id that names an item.
    #[test]
    fn a_pick_must_match_the_current_menu() {
        let p = prod();
        let ev = &p[p.find("pub(super) fn browser_ctx_event").unwrap()..p.find("pub(super) fn browser_ctx_picked").unwrap()];
        assert!(ev.contains("*t == token") && ev.contains("checked_sub(1)"));
    }
}
