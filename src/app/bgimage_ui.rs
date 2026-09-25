//! Profile background images in the window: which image each pane shows (its own, or the
//! active pane's across the tab when panes share one), loading, and folder rotation.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;
use crate::bgimage::{self, Deck, Loaded, Mode, Piece};

/// One folder rotation: its order, the image showing, and when the next one is due.
pub(super) struct Rotator {
    deck: Deck,
    current: Option<PathBuf>,
    next_at: Instant,
}

/// What one pane draws under its cells.
pub(super) struct PaneBackground {
    pub pane: PaneId,
    pub rect: Rect,
    pub pieces: Vec<Piece>,
    pub image: Arc<Loaded>,
    pub bg: alacritty_terminal::vte::ansi::Rgb,
    pub blend: f32,
}

impl App {
    /// A profile's image, mode, and blend now; None without an image.
    fn background_of(&mut self, pane: PaneId) -> Option<(Arc<Loaded>, Mode, f32)> {
        let p = self.profile_of(pane)?;
        let mode = Mode::from_int(p.int("Background Image Mode"));
        let blend = p.float("Blend") as f32;
        let path = if p.int("Background Image Source Mode") == 1 {
            let folder = PathBuf::from(expand_home(&p.string("Background Image Folder Location")));
            let interval = (p.int("Background Image Folder Interval").max(1) as u64).max(bgimage::MIN_INTERVAL_SECS);
            if folder.as_os_str().is_empty() {
                return None;
            }
            self.rotated((folder, interval))?
        } else {
            let loc = p.string("Background Image Location");
            if loc.is_empty() {
                return None;
            }
            PathBuf::from(expand_home(&loc))
        };
        let image = self.background_image(&path)?;
        Some((image, mode, blend))
    }

    /// A loaded image, by path; a missing or broken file is remembered as None, so the
    /// pane shows its background color without trying every frame.
    fn background_image(&mut self, path: &std::path::Path) -> Option<Arc<Loaded>> {
        if let Some(hit) = self.bg_images.get(path) {
            return hit.clone();
        }
        let loaded = bgimage::load(path).map(Arc::new);
        // A few images at most stay decoded (each can be 100 MB).
        if self.bg_images.len() >= BG_CACHE {
            self.bg_images.clear();
        }
        self.bg_images.insert(path.to_path_buf(), loaded.clone());
        loaded
    }

    /// The image a folder rotation shows now, starting the rotation if it is new.
    fn rotated(&mut self, key: (PathBuf, u64)) -> Option<PathBuf> {
        let r = self.rotators.entry(key.clone()).or_insert_with(|| {
            let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
            let mut deck = Deck::new(seed);
            let current = deck.next(&bgimage::folder_images(&key.0));
            Rotator { deck, current, next_at: Instant::now() + Duration::from_secs(key.1) }
        });
        r.current.clone()
    }

    /// Advances rotations that are due; says when the next one is.
    pub(super) fn tick_rotations(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let mut changed = false;
        for ((folder, secs), r) in self.rotators.iter_mut() {
            if now >= r.next_at {
                r.current = r.deck.next(&bgimage::folder_images(folder));
                r.next_at = now + Duration::from_secs(*secs);
                changed = true;
            }
        }
        if changed {
            // Images a rotation showed before are no longer needed.
            let showing: Vec<PathBuf> = self.rotators.values().filter_map(|r| r.current.clone()).collect();
            let folders: Vec<PathBuf> = self.rotators.keys().map(|(f, _)| f.clone()).collect();
            self.bg_images.retain(|p, _| showing.contains(p) || !p.parent().is_some_and(|d| folders.iter().any(|f| f == d)));
            self.redraw();
        }
        self.rotators.values().map(|r| r.next_at).min()
    }

    /// What each visible pane of the active tab draws under its cells.
    pub(super) fn pane_backgrounds(&mut self) -> Vec<PaneBackground> {
        let Some(tab) = self.tabs.get(self.active) else { return Vec::new() };
        let visible = self.visible(tab);
        let focused = tab.focused;
        let per_pane = self.globals.bool("PerPaneBackgroundImage");
        let content = self.content_area();
        let shared = if per_pane { None } else { self.background_of(focused) };
        let mut out = Vec::new();
        for (id, rect) in visible {
            let (image, mode, blend, container) = if per_pane {
                let Some((image, mode, blend)) = self.background_of(id) else { continue };
                (image, mode, blend, rect)
            } else {
                let Some((image, mode, blend)) = shared.clone() else { continue };
                (image, mode, blend, content)
            };
            let Some(bg) = self.panes.get(&id).map(|p| p.screen.palette.bg) else { continue };
            let pieces = bgimage::quads(mode, image.size, container, rect);
            out.push(PaneBackground { pane: id, rect, pieces, image, bg, blend });
        }
        out
    }
}

/// How many background images stay decoded.
const BG_CACHE: usize = 8;

/// `~` at the start of a path is the user's home folder.
fn expand_home(path: &str) -> String {
    match (path.strip_prefix('~'), std::env::var("USERPROFILE")) {
        (Some(rest), Ok(home)) => format!("{home}{rest}"),
        _ => path.to_string(),
    }
}
