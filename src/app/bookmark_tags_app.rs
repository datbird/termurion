//! Edit Bookmark on the app side (13x8, Task 8): Add Bookmark opens the window for tags, Save applies
//! the difference through the one browser database, Delete asks first. The window is opened only
//! from the person's own menu action (never a page event), takes the focus only then, and is closed
//! by a navigation of the pane that owns it (`BVC:1095-1096`) or the pane going away. Dev Null panes
//! never open it (their bookmarks are not kept, so there is nothing to tag).

use super::*;
use crate::bookmark_tag_ui::{self as ui, TagAct, TagEditor};
use crate::browser_db::Db;

impl App {
    /// Asks for the Edit Bookmark window for the pane's page; it opens on the next pass of the event
    /// loop (which owns the `ActiveEventLoop`). Only one at a time (`BVC:1444`): while one shows, the
    /// request only brings it forward.
    pub(super) fn open_bookmark_editor(&mut self, pane: PaneId, url: &str, title: Option<&str>) {
        if self.pane_dev_null(pane) {
            return;
        }
        if let Some(w) = &self.bookmark_editor {
            w.focus();
            return;
        }
        self.bookmark_editor_req = Some((pane, url.to_string(), title.map(str::to_string)));
    }

    /// The pass that opens a requested window.
    pub(super) fn bookmark_editor_tick(&mut self, event_loop: &ActiveEventLoop) {
        let Some((pane, url, title)) = self.bookmark_editor_req.take() else { return };
        if self.bookmark_editor.is_some() || self.pane_dev_null(pane) || !self.panes.contains_key(&pane) {
            return;
        }
        let Some((tags, all)) = self.browser_db(false).map(|db| (db.tags_for(&url), db.all_tags())) else { return };
        let Some(g) = self.gpu.as_ref() else { return };
        let (pos, size) = (g.window.outer_position().unwrap_or_default(), g.window.outer_size());
        let center = (f64::from(pos.x) + f64::from(size.width) / 2.0, f64::from(pos.y) + f64::from(size.height) / 2.0);
        match TagEditor::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, pane, url, title, tags, all, center) {
            Ok(w) => {
                w.focus();
                self.bookmark_editor = Some(w);
            }
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Events for the Edit Bookmark window. False for other windows.
    pub(super) fn bookmark_editor_event(&mut self, window: WindowId, e: &WindowEvent) -> bool {
        let Some(w) = self.bookmark_editor.as_mut().filter(|w| w.id() == window) else { return false };
        match w.event(e) {
            None => {}
            Some(TagAct::Cancel) => self.bookmark_editor = None,
            Some(TagAct::Save(tags)) => {
                if let Some(w) = self.bookmark_editor.take() {
                    self.bookmark_tags_save(w.pane(), &w.url(), &tags);
                }
            }
            Some(TagAct::Delete) => self.bookmark_delete_ask(),
        }
        true
    }

    /// The editor's window closes with a navigation of its pane or the pane itself.
    pub(super) fn bookmark_editor_close_for(&mut self, pane: PaneId) {
        if self.bookmark_editor.as_ref().is_some_and(|w| w.pane() == pane) {
            self.bookmark_editor = None;
        }
        if self.bookmark_editor_req.as_ref().is_some_and(|r| r.0 == pane) {
            self.bookmark_editor_req = None;
        }
    }

    /// Save: tags no longer in the field are removed, new ones added (`TE:193-208`).
    fn bookmark_tags_save(&mut self, _pane: PaneId, url: &str, tags: &[String]) {
        let Some(db) = self.browser_db(false) else { return };
        apply_tags(db, url, tags);
    }

    /// Delete Bookmark: asks first (`TE:219-234`); one question at a time.
    fn bookmark_delete_ask(&mut self) {
        let Some((pane, url)) = self.bookmark_editor.as_ref().map(|w| (w.pane(), w.url())) else { return };
        if std::mem::replace(&mut self.bookmark_delete_asking, true) {
            return;
        }
        let ask = super::scripts_alerts::ask(ui::DELETE, ui::DELETE_CONFIRM, &[ui::DELETE_BUTTON, crate::api_auth::CANCEL], None, crate::warnings::Kind::Persistent, super::scripts_alerts::Then::BookmarkDelete { pane, url });
        self.ask_scripts(ask);
    }

    /// The Delete question answered (0 is Delete). It acts only on the bookmark the open editor is
    /// for; a closed question or a gone window deletes nothing.
    pub(super) fn bookmark_delete_answered(&mut self, pane: PaneId, url: &str, index: usize) {
        self.bookmark_delete_asking = false;
        let same = self.bookmark_editor.as_ref().is_some_and(|w| w.pane() == pane && w.url() == url);
        if index != 0 || !same {
            return;
        }
        let removed = self.browser_db(false).is_some_and(|db| db.remove_bookmark(url));
        self.bookmark_editor = None;
        if removed {
            self.show_toast(ui::DELETED_TOAST);
        }
    }
}

/// The tags to keep, applied as the difference from the bookmark's own (removals first).
pub(super) fn apply_tags(db: &Db, url: &str, keep: &[String]) {
    let (removed, added) = ui::tag_diff(&db.tags_for(url), keep);
    for t in removed {
        db.remove_tag(url, &t);
    }
    for t in added {
        db.add_tag(url, &t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prod() -> &'static str {
        let src = include_str!("bookmark_tags_app.rs");
        &src[..src.find("#[cfg(test)]").unwrap()]
    }

    #[test]
    fn saving_applies_the_difference() {
        let db = Db::open_in_memory();
        db.add_bookmark("u", None, 1);
        db.add_tag("u", "a");
        db.add_tag("u", "b");
        apply_tags(&db, "u", &["b".to_string(), "c".to_string()]);
        assert_eq!(db.tags_for("u"), ["b", "c"]);
    }

    /// Add Bookmark on an unbookmarked page adds it and opens Edit Bookmark; on a bookmarked page
    /// removes it and toasts (`BVC:984-998`); the editor needs a person's action, a navigation of its
    /// pane closes it, Dev Null never opens it, and Delete asks first and acts on the editor's own bookmark.
    #[test]
    fn bookmark_page_tags_follow_iterm2() {
        let toggle = include_str!("browser_db_app.rs");
        let toggle = &toggle[toggle.find("pub(super) fn browser_toggle_bookmark").unwrap()..toggle.find("/// A request for an `iterm2-about:` URL").unwrap()];
        assert!(toggle.contains("crate::bookmark_tag_ui::REMOVED_TOAST") && toggle.contains("self.open_bookmark_editor(pane, &url, Some(&title))"));
        assert!(toggle.contains("dev_null"), "Dev Null returns first");
        let p = prod();
        assert!(p.contains("if self.pane_dev_null(pane)") && p.contains("self.panes.contains_key(&pane)"));
        assert!(p.contains("index != 0 || !same"), "a stale or closed question deletes nothing");
        let app = include_str!("browser_app.rs");
        assert!(app.contains("self.bookmark_editor_close_for(pane)"), "a navigation of the pane closes the editor");
        let m = include_str!("mod.rs");
        assert!(m.contains("self.bookmark_editor_event(id, &event)") && m.contains("self.bookmark_editor_tick(event_loop)"));
        let alerts = include_str!("scripts_alerts.rs");
        assert!(alerts.contains("Then::BookmarkDelete { .. } => Answer::Button(1)"), "a closed question is Cancel");
        // Nothing here is a page's: no message reaches the editor, nothing is logged.
        assert!(!p.contains("local_page_message") && !p.contains("rlog!") && !p.contains("dlog!"));
    }
}
