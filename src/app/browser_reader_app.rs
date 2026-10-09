//! Reader Mode and Remove Distractions in a browser pane (milestone 13x4). Task 1 holds the message
//! handler only: the reader scripts run in Tandem's isolated world and post their state through the
//! `readerMode` handler, which arrives as a Client message from the world's binding and nowhere else.

use super::*;
use crate::browser_js::{Routed, World};

impl App {
    /// One `readerMode` message. Advisory: only the isolated world's, only with a proven world, and
    /// only the typed `action` is read (the page's text it may carry, the title, is never read, logged
    /// or shown). A page-world message is dropped before it is read.
    pub(super) fn browser_reader_message(&mut self, pane: PaneId, r: &Routed) {
        if r.world != World::Client || !self.client_open(pane) {
            return;
        }
        let Some(action) = r.body.get("action").and_then(|a| a.as_str()) else { return };
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.reader.on_message(action);
        }
    }
}

#[cfg(test)]
mod tests {
    fn prod(src: &str) -> &str {
        src.split("\n#[cfg(test)]").next().unwrap_or(src)
    }

    fn between<'a>(src: &'a str, from: &str, to: &str) -> &'a str {
        let a = src.find(from).unwrap_or_else(|| panic!("missing {from}"));
        let b = a + src[a..].find(to).unwrap_or_else(|| panic!("missing {to}"));
        &src[a..b]
    }

    /// B10: the `readerMode` handler is a listed Client handler: world checked, gate asked, and the
    /// router has its arm before the drop. B8: the reader state dies with the isolated world.
    #[test]
    fn reader_message_is_a_gated_client_handler() {
        let me = prod(include_str!("browser_reader_app.rs"));
        let msg = between(me, "pub(super) fn browser_reader_message", "\n    }\n");
        assert!(msg.contains("r.world != World::Client") && msg.contains("!self.client_open(pane)"));
        assert!(msg.find("World::Client").unwrap() < msg.find("\"action\"").unwrap(), "a page-world message is dropped before it is read");
        assert!(!msg.contains("title") && !msg.contains("rlog!"), "the page's title is never read or logged");
        let router = prod(include_str!("browser_scripts_app.rs"));
        let arm = "h if h == crate::browser_reader::HANDLER => self.browser_reader_message(pane, &r),";
        assert!(router.contains(arm) && router.find(arm).unwrap() < router.find("browser message dropped").unwrap());
        assert!(include_str!("browser_scripts_app.rs").contains("\"browser_copy_mode_key\", \"browser_reader_message\"]"), "listed in the router test");
    }

    #[test]
    fn the_reader_state_dies_with_the_world() {
        let r = prod(include_str!("browser_scripts_app.rs"));
        assert!(r.contains("bv.copy_mode = None;\n            bv.reader.reset();\n            dones"), "view close");
        let restart = between(r, "fn browser_world_restart", "pub(super) fn browser_world_tick");
        assert!(restart.contains("bv.reader.reset()"), "restart");
        let tick = between(r, "pub(super) fn browser_world_tick", "let late =");
        assert!(tick.contains("bv.reader.reset()"), "lost after a process failure");
        let starting = between(r, "Event::Starting { redirect: false, .. } => {", "self.browser_copy_mode_set(pane, false)");
        assert!(starting.contains("bv.reader.reset()"), "a navigation start");
        let created = between(r, "\"Runtime.executionContextCreated\" => {", "\"Runtime.executionContextDestroyed\"");
        assert!(created.contains("bv.reader.reset()") && created.contains("is_main_context"), "the main frame's new Tandem context");
    }
}
