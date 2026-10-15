//! The What's New carousel (iTerm2's `iTermOnboardingWindowController`): a window that pages
//! through the features of a release, with "<" and ">" buttons and a dot for each page. It opens
//! once, on the first launch after an update. Pages exist only for features Termurion has;
//! iTerm2's Companion iPhone App page is not here (ruling R12, 01-profile-settings.md).

use crate::panel_ui::{Act, View};

/// `NoSyncOnboardingWindowHasBeenShown34` for this edition of the pages.
pub const SHOWN_KEY: &str = "NoSyncOnboardingWindowHasBeenShown1";
/// The version the last launch ran, to tell an update from a first install.
pub const LAST_VERSION_KEY: &str = "NoSyncLastLaunchVersion";

/// A piece of a page's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seg {
    Text(&'static str),
    /// The link's words and the route it asks for (`iterm2whatsnew:<route>`).
    Link(&'static str, &'static str),
}

pub struct Page {
    pub title: &'static str,
    pub body: &'static [Seg],
}

/// The pages, in iTerm2's order without Companion (`iTermOnboardingWindowController.xib:156-303`).
pub const PAGES: [Page; 4] = [
    Page {
        title: "Claude Code Integration",
        body: &[
            Seg::Text("The Claude Code integration organizes multiple related sessions\u{2014}such as code review and reading diffs\u{2014}into a customizable UI. It also exposes Claude\u{2019}s status so you can easily find instances that are blocked on your attention. "),
            Seg::Link("Try it now!", "claudecode"),
        ],
    },
    Page {
        title: "Screenshots",
        body: &[
            Seg::Text("The new Screenshots feature simplifies creating a PNG of your terminal content. It supports redactions and highlights. Use "),
            Seg::Link("Session > Make Screenshot", "screenshot"),
            Seg::Text(" to activate it."),
        ],
    },
    Page {
        title: "Orchestration",
        body: &[
            Seg::Text("A new AI chat mode, Orchestration, lets AI run the show for you. It can create windows, wait for tasks to complete (even in interactive apps), and the new automatic safety checker relieves you of having to respond to almost all permission prompts. "),
            Seg::Link("Try it now!", "orchestration"),
        ],
    },
    Page {
        title: "Tab Groups",
        body: &[Seg::Text("To help manage a large number of tabs, you can now combine tabs into collapsible groups. The tab bar now scrolls instead of showing an overflow menu, too.")],
    },
];

/// The heading (`:130`), with the running version.
pub fn heading(version: &str) -> String {
    format!("What\u{2019}s New in Version {version}")
}

pub const PREVIOUS: &str = "<";
pub const NEXT: &str = ">";
pub const DOT: &str = "\u{25cf}";

/// Whether this launch shows the carousel: a previous launch ran another version and the pages
/// were not shown yet (a first install, with no previous version, does not show them).
pub fn should_show(previous: Option<&str>, current: &str, shown: bool) -> bool {
    !shown && previous.is_some_and(|p| !p.is_empty() && p != current)
}

/// The page after a click on dot `i` while on page `at`: one step toward it, never a jump
/// (`pageIndicator:`).
pub fn step_toward(at: usize, i: usize) -> usize {
    match i.cmp(&at) {
        std::cmp::Ordering::Greater => at + 1,
        std::cmp::Ordering::Less => at.saturating_sub(1),
        std::cmp::Ordering::Equal => at,
    }
}

pub struct WhatsNewView {
    version: String,
    page: usize,
}

impl WhatsNewView {
    pub fn new(version: &str) -> WhatsNewView {
        WhatsNewView { version: version.to_string(), page: 0 }
    }
}

impl View for WhatsNewView {
    fn title(&self) -> String {
        heading(&self.version)
    }

    fn size(&self) -> (f64, f64) {
        (560.0, 300.0)
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Act> {
        let mut acts = Vec::new();
        ui.vertical_centered(|ui| ui.heading(heading(&self.version)));
        ui.add_space(12.0);
        let page = &PAGES[self.page.min(PAGES.len() - 1)];
        ui.vertical_centered(|ui| ui.label(egui::RichText::new(page.title).size(18.0).strong()));
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for seg in page.body {
                match seg {
                    Seg::Text(t) => {
                        ui.label(*t);
                    }
                    Seg::Link(label, route) => {
                        if ui.link(*label).clicked() {
                            acts.push(Act::WhatsNewLink((*route).to_string()));
                        }
                    }
                }
            }
        });
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.horizontal(|ui| {
                let width = ui.available_width();
                ui.add_space((width / 2.0 - 70.0).max(0.0));
                if ui.add_enabled(self.page > 0, egui::Button::new(PREVIOUS).frame(false)).clicked() {
                    self.page -= 1;
                }
                for i in 0..PAGES.len() {
                    let dot = egui::RichText::new(DOT).color(if i == self.page { ui.visuals().text_color() } else { ui.visuals().weak_text_color() });
                    if ui.add(egui::Button::new(dot).frame(false)).clicked() {
                        self.page = step_toward(self.page, i);
                    }
                }
                if ui.add_enabled(self.page + 1 < PAGES.len(), egui::Button::new(NEXT).frame(false)).clicked() {
                    self.page += 1;
                }
            });
        });
        acts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_follow_the_xib_without_companion() {
        let titles: Vec<&str> = PAGES.iter().map(|p| p.title).collect();
        assert_eq!(titles, ["Claude Code Integration", "Screenshots", "Orchestration", "Tab Groups"]);
        assert!(!titles.iter().any(|t| t.contains("Companion")), "R12");
        let all: String = PAGES.iter().flat_map(|p| p.body.iter()).map(|s| match s {
            Seg::Text(t) | Seg::Link(t, _) => *t,
        }).collect();
        assert!(!all.contains("iTerm2"), "the app is Termurion");
    }

    #[test]
    fn it_shows_once_after_an_update_never_on_a_first_install() {
        assert!(!should_show(None, "1.1", false), "a first install");
        assert!(should_show(Some("1.0"), "1.1", false));
        assert!(!should_show(Some("1.1"), "1.1", false), "the same version");
        assert!(!should_show(Some("1.0"), "1.1", true), "already shown");
    }

    #[test]
    fn a_dot_steps_one_page_toward_itself() {
        assert_eq!((step_toward(0, 3), step_toward(3, 0), step_toward(2, 2), step_toward(0, 0)), (1, 2, 2, 0));
    }
}
