//! The hue and chroma wheel (iTerm2's HueVisualizationView): the twelve ANSI colors placed by OKLab
//! hue and chroma, dragged to change hue and chroma while lightness stays. Geometry is pure.

use crate::oklab::OkLab;
use crate::palette::Rgb;

/// The colors the wheel shows: ANSI 1 to 6 and 9 to 14 (black, white and their brights are skipped).
pub const KEYS: [&str; 12] = [
    "Ansi 1 Color", "Ansi 2 Color", "Ansi 3 Color", "Ansi 4 Color", "Ansi 5 Color", "Ansi 6 Color",
    "Ansi 9 Color", "Ansi 10 Color", "Ansi 11 Color", "Ansi 12 Color", "Ansi 13 Color", "Ansi 14 Color",
];
pub const POINT_RADIUS: f32 = 4.0;
/// iTerm2 checked every P3 color: 0.4 chroma is the rim.
pub const MAX_CHROMA: f32 = 0.4;
pub const CIRCLES: usize = 8;
pub const RADII: usize = 8;

/// A color's place in OKLab. iTerm2 reads a color in Display P3; sRGB goes in, converted the same way.
fn lab_of(c: Rgb) -> OkLab {
    let (r, g, b) = crate::import::srgb_to_p3(c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0);
    OkLab::from_p3(r, g, b)
}

/// Where a point of radius `point` sits: at its chroma out from the center plus the point radius,
/// at its hue (`rect(entry:pointRadius:)`). egui's y grows down, so the angle is mirrored.
fn center_with(center: (f32, f32), radius: f32, c: OkLab, point: f32) -> (f32, f32) {
    let r = c.chroma() / MAX_CHROMA * radius + point;
    let hue = c.hue();
    (center.0 + hue.cos() * r, center.1 - hue.sin() * r)
}

pub fn point_center(center: (f32, f32), radius: f32, c: OkLab) -> (f32, f32) {
    center_with(center, radius, c, POINT_RADIUS)
}

/// The chroma and hue of a position (`chromaAndHue(for:)`): the inverse of `point_center`.
pub fn chroma_hue(center: (f32, f32), radius: f32, p: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = (p.0 - center.0, center.1 - p.1);
    let distance = dx.hypot(dy);
    ((distance - POINT_RADIUS) / radius * MAX_CHROMA, dy.atan2(dx))
}

/// The color a drag to (chroma, hue) makes of `from`: a and b from the new chroma and hue, the
/// lightness kept. Tandem stores sRGB, so a spot beyond the sRGB gamut gets the most chroma that
/// fits at that lightness and hue (iTerm2 keeps the wider P3 color).
pub fn dragged(from: Rgb, chroma: f32, hue: f32) -> Rgb {
    let l = lab_of(from).l;
    let at = |t: f32| OkLab { l, a: t * chroma * hue.cos(), b: t * chroma * hue.sin() }.to_srgb();
    if let Some(c) = at(1.0) {
        return c;
    }
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    for _ in 0..14 {
        let mid = (lo + hi) / 2.0;
        if at(mid).is_some() { lo = mid } else { hi = mid }
    }
    at(lo).unwrap_or(from)
}

/// Draws the wheel in a square of the available room and handles a drag. `colors` are the keys
/// and their current colors; `drag` is the drag in progress (key, the point's original center,
/// where the pointer started). Returns the dragged key and its new color when a drag changed it.
pub fn wheel(ui: &mut egui::Ui, colors: &[(String, Rgb)], drag: &mut Option<(String, (f32, f32), (f32, f32))>) -> Option<(String, Rgb)> {
    let side = ui.available_width().min(ui.available_height()).max(0.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click_and_drag());
    let radius = side / 2.0;
    let center = (rect.center().x, rect.center().y);
    let pos = |p: (f32, f32)| egui::pos2(p.0, p.1);
    let mut changed = None;
    if radius > 0.0 {
        let labs: Vec<OkLab> = colors.iter().map(|(_, c)| lab_of(*c)).collect();
        if response.drag_started() {
            if let Some(p) = ui.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos()) {
                *drag = colors.iter().zip(&labs).rev().find_map(|((key, _), lab)| {
                    let at = point_center(center, radius, *lab);
                    ((at.0 - p.x).hypot(at.1 - p.y) <= POINT_RADIUS).then(|| (key.clone(), at, (p.x, p.y)))
                });
            }
        }
        if response.dragged() {
            if let (Some((key, original, start)), Some(p)) = (drag.as_ref(), response.interact_pointer_pos()) {
                let moved = (original.0 + p.x - start.0, original.1 + p.y - start.1);
                let (chroma, hue) = chroma_hue(center, radius, moved);
                if let Some((_, from)) = colors.iter().find(|(k, _)| k == key) {
                    let now = dragged(*from, chroma, hue);
                    if now != *from {
                        changed = Some((key.clone(), now));
                    }
                }
            }
        }
        if response.drag_stopped() || (!response.dragged() && !response.drag_started()) {
            *drag = None;
        }
        let painter = ui.painter_at(rect);
        let gray = egui::Stroke::new(1.0, egui::Color32::from_gray(128));
        for i in 0..CIRCLES {
            let r = radius * (i + 1) as f32 / CIRCLES as f32;
            let stroke = if i == CIRCLES - 1 { egui::Stroke::new(1.0, egui::Color32::BLACK) } else { gray };
            painter.circle_stroke(pos(center), r, stroke);
        }
        for i in 0..RADII {
            let angle = std::f32::consts::TAU * i as f32 / RADII as f32;
            painter.line_segment([pos(center), egui::pos2(center.0 + radius * angle.cos(), center.1 - radius * angle.sin())], gray);
        }
        let dragging = drag.as_ref().map(|d| d.0.as_str());
        // The dragged point is drawn last, 2 larger, with a white outline.
        let order = colors.iter().zip(&labs).filter(|((k, _), _)| Some(k.as_str()) != dragging).chain(colors.iter().zip(&labs).filter(|((k, _), _)| Some(k.as_str()) == dragging));
        for ((key, color), lab) in order {
            let big = Some(key.as_str()) == dragging;
            let point = if big { POINT_RADIUS + 2.0 } else { POINT_RADIUS };
            let at = center_with(center, radius, *lab, point);
            let outline = if big { egui::Color32::WHITE } else { egui::Color32::BLACK };
            painter.circle(pos(at), point, egui::Color32::from_rgb(color.r, color.g, color.b), egui::Stroke::new(1.0, outline));
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

    fn near(p: (f32, f32), x: f32, y: f32) -> bool {
        (p.0 - x).abs() < 1e-3 && (p.1 - y).abs() < 1e-3
    }

    #[test]
    fn wheel_geometry_follows_iterm2() {
        let (c, r) = ((100.0, 100.0), 100.0);
        let gray = OkLab { l: 0.5, a: 0.0, b: 0.0 };
        assert!(near(point_center(c, r, gray), 104.0, 100.0), "{:?}", point_center(c, r, gray));
        let red_hue = OkLab { l: 0.5, a: 0.2, b: 0.0 };
        assert!(near(point_center(c, r, red_hue), 154.0, 100.0), "{:?}", point_center(c, r, red_hue));
        // egui's y grows down, so the angle is mirrored: a quarter turn up is smaller y.
        let up = OkLab { l: 0.5, a: 0.0, b: 0.2 };
        assert!(near(point_center(c, r, up), 100.0, 100.0 - 54.0), "{:?}", point_center(c, r, up));
        // chroma_hue inverts point_center.
        for lab in [red_hue, up, OkLab { l: 0.6, a: -0.1, b: -0.15 }] {
            let (chroma, hue) = chroma_hue(c, r, point_center(c, r, lab));
            assert!((chroma - lab.chroma()).abs() < 1e-4 && (hue - lab.hue()).abs() < 1e-4, "{chroma} {hue}");
        }
        // A drag moving the red point keeps its lightness and changes its hue.
        let red = rgb(205, 0, 0);
        let lab = crate::oklab::from_srgb(red);
        let start = point_center(c, r, lab);
        let (chroma, hue) = chroma_hue(c, r, (start.0 + 5.0, start.1 - 25.0));
        let moved = crate::oklab::from_srgb(dragged(red, chroma, hue));
        assert!((moved.l - lab.l).abs() < 0.03, "{} {}", moved.l, lab.l);
        assert!((moved.hue() - lab.hue()).abs() > 0.05, "{} {}", moved.hue(), lab.hue());
        // Far past the rim: lightness and hue survive (the color is pulled into the gamut).
        let (chroma, hue) = chroma_hue(c, r, (start.0 + 5.0, start.1 - 90.0));
        let far = crate::oklab::from_srgb(dragged(red, chroma, hue));
        assert!((far.l - lab.l).abs() < 0.03 && far.chroma() <= chroma + 0.01, "{far:?}");
        assert_eq!(KEYS.len(), 12);
        assert_eq!((CIRCLES, RADII, POINT_RADIUS, MAX_CHROMA), (8, 8, 4.0, 0.4));
    }
}
