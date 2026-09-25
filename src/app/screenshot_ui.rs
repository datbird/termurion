//! Session > Make Screenshot... in the window: rendering a range of a frozen copy of the
//! pane into an image.

use super::*;
use crate::screenshot;

impl App {
    /// Renders buffer lines `first..=last` (0 is the oldest line) of `screen` as one RGBA
    /// image: the pane's full width with its side padding, no top or bottom margin, opaque,
    /// with the pane's background image laid out over the whole image's height.
    pub(super) fn render_lines(&mut self, pane: PaneId, screen: &crate::screen::Screen, first: usize, last: usize) -> Result<(u32, u32, Vec<u8>), String> {
        use alacritty_terminal::grid::Dimensions;
        let cell = self.cell();
        let pad = (PAD * self.scale()).round();
        let cols = screen.term().columns();
        let width = (cols as f32 * cell.0 + 2.0 * pad).ceil();
        let lines = last + 1 - first;
        let total_h = lines as f32 * cell.1;
        let history = screen.term().grid().history_size() as i32;
        let background = self.background_of(pane);
        // A batch must fit in one texture; iTerm2 renders 1000 lines at a time.
        let max_rows = ((8192.0 / cell.1).floor() as usize).clamp(1, 1000);
        let bg = screen.palette.bg;
        let mut out = Vec::with_capacity((width as usize) * (total_h as usize) * 4);
        for (b0, b1) in screenshot::batches(first, last, max_rows) {
            let count = b1 + 1 - b0;
            let batch = screenshot::slice(screen, b0 as i32 - history, count);
            let h = count as f32 * cell.1;
            let mut frame = Frame { clear: rgba(bg, 1.0), ..Default::default() };
            let area = Rect { x: 0.0, y: 0.0, w: width, h };
            if let Some((image, mode, blend)) = &background {
                // The image is laid out over the whole screenshot, and this batch shows its slice.
                let container = Rect { x: 0.0, y: -((b0 - first) as f32 * cell.1), w: width, h: total_h };
                let pieces = crate::bgimage::quads(*mode, image.size, container, area);
                scene::background(&mut frame, area, &pieces, image, bg, *blend);
            }
            scene::pane_with(&mut frame, &batch, Rect { x: pad, y: 0.0, w: cols as f32 * cell.0, h }, cell, false, false, None);
            let gpu = self.gpu.as_mut().ok_or("no window")?;
            out.extend(gpu.renderer.draw_offscreen(&frame, width as u32, h as u32)?);
        }
        Ok((width as u32, total_h as u32, out))
    }
}

/// An open Make Screenshot panel and the pane it shows a frozen copy in.
pub(super) struct Shot {
    pub window: crate::shot_ui::ShotWindow,
    pub pane: PaneId,
}

impl App {
    /// Session > Make Screenshot...: freezes a copy of the focused pane and opens the panel.
    pub(super) fn make_screenshot(&mut self, event_loop: &ActiveEventLoop) {
        use alacritty_terminal::grid::Dimensions;
        let Some(pane) = self.focused_id().filter(|id| self.panes.contains_key(id)) else {
            unsafe { windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(0) };
            return;
        };
        if let Some(old) = self.shot.take() {
            self.close_shot(old);
        }
        let p = self.panes.get_mut(&pane).expect("pane checked above");
        let snap = screenshot::snapshot(p.own());
        let g = snap.term().grid();
        let (history, rows) = (g.history_size(), g.screen_lines());
        let top = history - p.screen.term().grid().display_offset().min(history);
        let panel = screenshot::Panel::new(history + rows, (top, top + rows - 1));
        p.freeze(snap);
        let Some(g) = self.gpu.as_ref() else { return };
        match crate::shot_ui::ShotWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, panel) {
            Ok(window) => {
                self.place_shot_window(&window);
                self.shot = Some(Shot { window, pane });
                self.refresh_minimap();
                self.refresh_preview();
            }
            Err(e) => {
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.thaw();
                }
                self.banners.set("window", format!("Could not open the window: {e}"));
            }
        }
        self.redraw();
    }

    /// Beside the terminal window: 20 points to its right, else to its left, else at the
    /// monitor's right edge; centered vertically.
    fn place_shot_window(&self, w: &crate::shot_ui::ShotWindow) {
        let Some(g) = self.gpu.as_ref() else { return };
        let (Ok(pos), size) = (g.window.outer_position(), g.window.outer_size()) else { return };
        let panel = w.window().outer_size();
        let gap = (20.0 * g.window.scale_factor()) as i32;
        let (mx, mw) = g.window.current_monitor().map_or((0, i32::MAX), |m| (m.position().x, m.size().width as i32));
        let right = pos.x + size.width as i32 + gap;
        let left = pos.x - gap - panel.width as i32;
        let x = if right + panel.width as i32 <= mx + mw {
            right
        } else if left >= mx {
            left
        } else {
            mx + mw - panel.width as i32
        };
        let y = pos.y + (size.height as i32 - panel.height as i32) / 2;
        w.window().set_outer_position(winit::dpi::PhysicalPosition::new(x, y.max(0)));
    }

    /// Takes the frozen copy out of its pane for a moment (to draw from it), then puts it back.
    fn with_snapshot<T>(&mut self, f: impl FnOnce(&mut Self, PaneId, &crate::screen::Screen) -> T) -> Option<T> {
        let pane = self.shot.as_ref()?.pane;
        let p = self.panes.get_mut(&pane)?;
        let snap = std::mem::replace(&mut p.screen, crate::screen::Screen::new(1, 1));
        let out = f(self, pane, &snap);
        if let Some(p) = self.panes.get_mut(&pane) {
            p.screen = snap;
        }
        Some(out)
    }

    fn refresh_minimap(&mut self) {
        let drawn = self.with_snapshot(|app, pane, snap| {
            use alacritty_terminal::grid::Dimensions;
            let g = snap.term().grid();
            let total = g.history_size() + g.screen_lines();
            let max = app.gpu.as_ref().map_or(8192, |g| g.device.limits().max_texture_dimension_2d as usize);
            let rows = crate::shotimage::minimap_rows(total, app.cell().1.ceil() as usize, max);
            let lines = crate::shotimage::minimap_lines(total, rows);
            let picked = screenshot::pick(snap, &lines);
            let last = lines.len().max(1) - 1;
            app.render_lines(pane, &picked, 0, last)
        });
        match drawn {
            Some(Ok((w, h, px))) => {
                if let Some(s) = self.shot.as_mut() {
                    s.window.set_minimap(w, h, &px);
                }
            }
            Some(Err(e)) => self.banners.set("screenshot", format!("Could not draw the minimap: {e}")),
            None => {}
        }
    }

    /// Draws the chosen range (its first lines, when it is very tall) with the annotations.
    fn refresh_preview(&mut self) {
        let Some(shot) = self.shot.as_ref() else { return };
        let (start, end) = shot.window.panel.range;
        let cell = self.cell();
        // The preview is one texture: at most 8000 pixels tall.
        let cap = ((8000.0 / cell.1).floor() as usize).max(1);
        let last = end.min(start + cap - 1);
        match self.render_annotated(start, last) {
            Some(Ok((w, h, px))) => {
                let parts = self.shot_parts();
                if let Some(s) = self.shot.as_mut() {
                    s.window.parts = parts;
                    s.window.set_preview(w, h, &px);
                }
            }
            Some(Err(e)) => self.banners.set("screenshot", format!("Could not draw the screenshot: {e}")),
            None => {}
        }
    }

    /// How many files the chosen range saves as.
    fn shot_parts(&self) -> usize {
        let Some(shot) = self.shot.as_ref() else { return 1 };
        let (s, e) = shot.window.panel.range;
        let cell = self.cell();
        let cols = self.panes.get(&shot.pane).map_or(80, |p| { use alacritty_terminal::grid::Dimensions; p.screen.term().columns() });
        let width = (cols as f32 * cell.0 + 2.0 * (PAD * self.scale()).round()).ceil() as usize;
        screenshot::parts(e + 1 - s, cell.1.round().max(1.0) as usize, width, screenshot::MAX_PX).len().max(1)
    }

    /// Renders lines `first..=last` of the frozen copy with the panel's redactions and
    /// highlights.
    fn render_annotated(&mut self, first: usize, last: usize) -> Option<Result<(u32, u32, Vec<u8>), String>> {
        let panel = self.shot.as_ref()?.window.panel.clone();
        let cell = self.cell();
        let scale = self.scale();
        let pad = (PAD * scale).round();
        self.with_snapshot(|app, pane, snap| {
            let (w, h, mut px) = app.render_lines(pane, snap, first, last)?;
            let bg = snap.palette.bg;
            let dark = (0.299 * bg.r as f32 + 0.587 * bg.g as f32 + 0.114 * bg.b as f32) < 128.0;
            let (wu, hu) = (w as usize, h as usize);
            for a in panel.annotations.iter().filter(|a| a.kind == screenshot::Kind::Redact) {
                let rects = screenshot::rects(&a.spans, a.is_box, (first, last), cell, pad);
                match panel.method {
                    // The radius is in points, as iTerm2's.
                    screenshot::Method::Blur(r) => crate::shotimage::blur_rects(&mut px, wu, hu, &rects, r * scale),
                    screenshot::Method::Solid(c) => crate::shotimage::fill_rects(&mut px, wu, hu, &rects, [c[0], c[1], c[2], 255]),
                }
            }
            for a in panel.annotations.iter().filter(|a| a.kind == screenshot::Kind::Highlight) {
                let rects = screenshot::rects(&a.spans, a.is_box, (first, last), cell, pad);
                crate::shotimage::highlight(&mut px, wu, hu, &rects, dark);
            }
            Ok((w, h, px))
        })
    }

    /// Redact Selection or Highlight Selection: the frozen pane's selection becomes an
    /// annotation, and the selection clears.
    fn annotate_shot(&mut self, kind: screenshot::Kind) {
        let Some(pane) = self.shot.as_ref().map(|s| s.pane) else { return };
        let Some(p) = self.panes.get_mut(&pane) else { return };
        use alacritty_terminal::grid::Dimensions;
        let history = p.screen.term().grid().history_size() as i32;
        let (spans, is_box) = crate::styled_copy::selection_spans(&p.screen);
        let lines: Vec<String> = spans
            .iter()
            .map(|&(l, a, b)| {
                let row = &p.screen.term().grid()[alacritty_terminal::index::Line(l)];
                (a..=b).map(|c| row[alacritty_terminal::index::Column(c)].c).collect::<String>().trim_end().to_string()
            })
            .collect();
        let spans: Vec<(usize, usize, usize)> = spans.into_iter().map(|(l, a, b)| ((l + history) as usize, a, b)).collect();
        p.screen.term_mut().selection = None;
        if let Some(s) = self.shot.as_mut() {
            s.window.panel.annotate(kind, spans, is_box, &lines);
        }
        self.refresh_preview();
        self.redraw();
    }

    /// Renders the whole range, part by part, as PNG files named from `chosen` (see
    /// `screenshot::save_paths`). The first file's path.
    fn save_shot(&mut self, chosen: &std::path::Path) -> Result<std::path::PathBuf, String> {
        let (start, end) = self.shot.as_ref().ok_or("no screenshot")?.window.panel.range;
        let cell = self.cell();
        let cols = self.shot.as_ref().and_then(|s| self.panes.get(&s.pane)).map_or(80, |p| { use alacritty_terminal::grid::Dimensions; p.screen.term().columns() });
        let width = (cols as f32 * cell.0 + 2.0 * (PAD * self.scale()).round()).ceil() as usize;
        let parts = screenshot::parts(end + 1 - start, cell.1.round().max(1.0) as usize, width, screenshot::MAX_PX);
        let paths = screenshot::save_paths(chosen, parts.len());
        let mut first_path = None;
        for (part, path) in parts.into_iter().zip(paths) {
            let (w, h, px) = self.render_annotated(start + part.start, start + part.end - 1).ok_or("no screenshot")??;
            let img = image::RgbaImage::from_raw(w, h, px).ok_or("the image had the wrong size")?;
            img.save_with_format(&path, image::ImageFormat::Png).map_err(|e| format!("{}: {e}", path.display()))?;
            first_path.get_or_insert(path);
        }
        first_path.ok_or_else(|| "nothing to save".to_string())
    }

    /// Where Save puts screenshots: `screenshotSaveLocation` when it is a folder, else the
    /// Desktop.
    fn shot_folder(&self) -> Option<std::path::PathBuf> {
        let set = self.globals.string("screenshotSaveLocation");
        let set = match (set.strip_prefix('~'), std::env::var("USERPROFILE")) {
            (Some(rest), Ok(home)) => format!("{home}{rest}"),
            _ => set,
        };
        if !set.is_empty() && crate::prompted::local_path_ok(&set) && std::path::Path::new(&set).is_dir() {
            return Some(set.into());
        }
        desktop()
    }

    fn close_shot(&mut self, shot: Shot) {
        if let Some(p) = self.panes.get_mut(&shot.pane) {
            p.thaw();
        }
        self.redraw();
    }

    /// The panel's window events. True when the event was for it.
    pub(super) fn shot_event(&mut self, window: winit::window::WindowId, event: &WindowEvent) -> bool {
        use crate::shot_ui::ShotEvent;
        let Some(shot) = self.shot.as_mut() else { return false };
        if shot.window.id() != window {
            return false;
        }
        match event {
            WindowEvent::CloseRequested => {
                if let Some(s) = self.shot.take() {
                    self.close_shot(s);
                }
            }
            WindowEvent::RedrawRequested => {
                let pane = shot.pane;
                shot.window.can_annotate = self.panes.get(&pane).is_some_and(|p| p.screen.term().selection.as_ref().is_some_and(|s| !s.is_empty()));
                let Some(shot) = self.shot.as_mut() else { return true };
                let ev = shot.window.redraw();
                match ev {
                    ShotEvent::None => {}
                    ShotEvent::Changed => self.refresh_preview(),
                    ShotEvent::Annotate(kind) => self.annotate_shot(kind),
                    ShotEvent::Cancel => {
                        if let Some(s) = self.shot.take() {
                            self.close_shot(s);
                        }
                    }
                    ShotEvent::Copy => {
                        let (start, end) = shot.window.panel.range;
                        match self.render_annotated(start, end) {
                            Some(Ok((w, h, px))) => {
                                let data = arboard::ImageData { width: w as usize, height: h as usize, bytes: px.into() };
                                match self.clipboard.as_mut().map(|cb| cb.set_image(data)) {
                                    Some(Ok(())) => {
                                        if let Some(s) = self.shot.as_mut() {
                                            s.window.copied();
                                        }
                                    }
                                    Some(Err(e)) => self.banners.set("screenshot", format!("Could not copy the screenshot: {e}")),
                                    None => {}
                                }
                            }
                            Some(Err(e)) => self.banners.set("screenshot", format!("Could not draw the screenshot: {e}")),
                            None => {}
                        }
                    }
                    ShotEvent::Save | ShotEvent::SaveAs => {
                        let name = {
                            let t = crate::prompt_ui::local_time();
                            screenshot::file_name(t.0, t.1, t.2, t.3, t.4, t.5)
                        };
                        let base = if ev == ShotEvent::SaveAs {
                            rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_file_name(&name).save_file()
                        } else {
                            self.shot_folder().map(|d| d.join(&name))
                        };
                        let Some(base) = base else { return true };
                        match self.save_shot(&base) {
                            Ok(path) => {
                                let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
                                if let Some(s) = self.shot.take() {
                                    self.close_shot(s);
                                }
                            }
                            Err(e) => files_ui::message(&format!("Could not save the screenshot.\n\n{e}")),
                        }
                    }
                }
            }
            e => {
                shot.window.event(e);
            }
        }
        true
    }
}

/// The user's Desktop folder (where Windows keeps it, OneDrive included).
fn desktop() -> Option<std::path::PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, SHGetKnownFolderPath};
    let mut out: windows_sys::core::PWSTR = std::ptr::null_mut();
    let hr = unsafe { SHGetKnownFolderPath(&FOLDERID_Desktop, 0, std::ptr::null_mut(), &mut out) };
    if hr < 0 || out.is_null() {
        return None;
    }
    let len = (0..).take_while(|&i| unsafe { *out.add(i) } != 0).count();
    let path = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(out, len) });
    unsafe { CoTaskMemFree(out as _) };
    Some(path.into())
}
