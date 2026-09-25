//! Sixel graphics: `ESC P <params> q <data> ESC \`, decoded to RGBA. iTerm2 hands sixel to
//! libsixel; Tandem decodes it here. Pixels come out opaque, as iTerm2 flattens them.

/// Larger images are refused.
pub const MAX_SIDE: usize = crate::images::MAX_SIDE as usize;
/// And at most this many pixels in all.
pub const MAX_PIXELS: usize = 50_000_000;

/// The VT340's 16 default colors, in percent.
pub const VT340: [(u8, u8, u8); 16] = [
    (0, 0, 0), (20, 20, 80), (80, 13, 13), (20, 80, 20), (80, 20, 80), (20, 80, 80), (80, 80, 20), (53, 53, 53),
    (26, 26, 26), (33, 33, 60), (60, 26, 26), (33, 60, 33), (60, 33, 60), (33, 60, 60), (60, 60, 33), (80, 80, 80),
];

/// Decodes a sixel body (the bytes after `q`): the width, height, and RGBA pixels.
pub fn decode(data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let pct = |v: u32| (v.min(100) * 255 / 100) as u8;
    let mut palette: Vec<[u8; 3]> = (0..256).map(|i| VT340.get(i).map_or([0, 0, 0], |&(r, g, b)| [pct(r as u32), pct(g as u32), pct(b as u32)])).collect();
    let mut rows: Vec<Vec<u32>> = Vec::new();
    let (mut x, mut band, mut color) = (0usize, 0usize, 0usize);
    let (mut max_x, mut max_y) = (0usize, 0usize);
    let mut raster = (0usize, 0usize);
    let mut i = 0;
    // A run of numbers separated by ';', read from `i` on.
    let numbers = |i: &mut usize| -> Vec<u32> {
        let mut out = vec![0u32];
        let mut any = false;
        while *i < data.len() {
            match data[*i] {
                d @ b'0'..=b'9' => {
                    let last = out.last_mut().unwrap_or_else(|| unreachable!());
                    *last = last.saturating_mul(10).saturating_add((d - b'0') as u32);
                    any = true;
                }
                b';' => out.push(0),
                _ => break,
            }
            *i += 1;
        }
        if any || out.len() > 1 { out } else { Vec::new() }
    };
    while i < data.len() {
        let b = data[i];
        i += 1;
        let mut count = 1usize;
        let sixel = match b {
            b'"' => {
                let p = numbers(&mut i);
                if p.len() >= 4 {
                    raster = (p[2] as usize, p[3] as usize);
                    if raster.0 > MAX_SIDE || raster.1 > MAX_SIDE {
                        return Err("The sixel image is too large.".into());
                    }
                }
                continue;
            }
            b'#' => {
                let p = numbers(&mut i);
                if let Some(&n) = p.first() {
                    color = (n as usize).min(255);
                    if p.len() >= 5 {
                        palette[color] = match p[1] {
                            1 => hls(p[2], p[3], p[4]),
                            _ => [pct(p[2]), pct(p[3]), pct(p[4])],
                        };
                    }
                }
                continue;
            }
            b'!' => {
                count = numbers(&mut i).first().copied().unwrap_or(1).max(1) as usize;
                match data.get(i) {
                    Some(&c @ 0x3f..=0x7e) => {
                        i += 1;
                        c
                    }
                    _ => continue,
                }
            }
            b'$' => {
                x = 0;
                continue;
            }
            b'-' => {
                x = 0;
                band += 1;
                if band * 6 >= MAX_SIDE {
                    return Err("The sixel image is too tall.".into());
                }
                continue;
            }
            c @ 0x3f..=0x7e => c,
            _ => continue,
        };
        if x + count > MAX_SIDE || (x + count) * (band * 6 + 6) > MAX_PIXELS {
            return Err("The sixel image is too large.".into());
        }
        let bits = sixel - 0x3f;
        let [r, g, bl] = palette[color];
        let rgba = u32::from_le_bytes([r, g, bl, 0xff]);
        for dy in 0..6 {
            if bits & (1 << dy) == 0 {
                continue;
            }
            let y = band * 6 + dy;
            if rows.len() <= y {
                rows.resize_with(y + 1, Vec::new);
            }
            let row = &mut rows[y];
            if row.len() < x + count {
                row.resize(x + count, 0);
            }
            row[x..x + count].fill(rgba);
        }
        x += count;
        max_x = max_x.max(x);
        max_y = max_y.max(band * 6 + 6);
    }
    let (w, h) = (max_x.max(raster.0), max_y.max(raster.1));
    if w == 0 || h == 0 {
        return Err("The sixel image has no pixels.".into());
    }
    if w * h > MAX_PIXELS {
        return Err("The sixel image is too large.".into());
    }
    // Pixels no sixel set are opaque black, as iTerm2 shows them.
    let mut out = vec![0u8; w * h * 4];
    for px in out.chunks_exact_mut(4) {
        px[3] = 0xff;
    }
    for (y, row) in rows.iter().enumerate().take(h) {
        for (x, &p) in row.iter().enumerate().take(w) {
            if p != 0 {
                let i = (y * w + x) * 4;
                out[i..i + 4].copy_from_slice(&p.to_le_bytes());
            }
        }
    }
    Ok((w as u32, h as u32, out))
}

/// A DEC HLS color (hue 0 is blue, 120 red, 240 green; lightness and saturation in
/// percent) as RGB.
fn hls(h: u32, l: u32, s: u32) -> [u8; 3] {
    let h = ((h + 240) % 360) as f32;
    let (l, s) = (l.min(100) as f32 / 100.0, s.min(100) as f32 / 100.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [to(r), to(g), to(b)]
}

impl crate::screen::Screen {
    /// A sixel image lands at the cursor; afterwards the cursor keeps the last row and
    /// goes back to its column (iTerm2 with DECSDM off).
    pub fn sixel(&mut self, data: &[u8]) {
        use alacritty_terminal::grid::Dimensions;
        let Ok((w, h, px)) = decode(data) else { return };
        let (cw, ch) = self.cell_px();
        let grid = (self.term().columns(), self.term().screen_lines());
        let col = self.term().grid().cursor.point.column;
        let (cols, rows) = crate::images::size_in_cells((w, h), crate::images::Dim::Auto, crate::images::Dim::Auto, true, (cw as f32, ch as f32), grid, col.0);
        let frame = crate::images::Frame { rgba: std::sync::Arc::new(px), delay_ms: 0 };
        let image = crate::images::Image { width_px: w, height_px: h, cols, rows, frames: vec![frame], broken: false, keep_aspect: true, uid: 0 };
        self.place_image(image, true);
        self.term_mut().grid_mut().cursor.point.column = col;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(img: &(u32, u32, Vec<u8>), x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.0 + x) * 4) as usize;
        [img.2[i], img.2[i + 1], img.2[i + 2], img.2[i + 3]]
    }

    #[test]
    fn decodes_a_small_sixel() {
        let img = decode(b"#1;2;100;0;0#1~~").unwrap();
        assert_eq!((img.0, img.1), (2, 6));
        assert_eq!(px(&img, 1, 5), [255, 0, 0, 255]);
        let hls = decode(b"#3;1;120;50;100#3~").unwrap();
        assert_eq!(px(&hls, 0, 0), [255, 0, 0, 255], "DEC hue 120 is red");
    }

    #[test]
    fn repeat_and_newline() {
        let img = decode(b"#1!5~-!3~$#2!2~").unwrap();
        assert_eq!((img.0, img.1), (5, 12));
        assert_eq!(px(&img, 4, 0)[3], 255);
        assert_eq!(px(&img, 1, 11), [204, 33, 33, 255], "the second band overdraws with VT340 red");
        assert_eq!(px(&img, 4, 11), [0, 0, 0, 255], "unset pixels are opaque black");
    }

    #[test]
    fn raster_attributes_set_the_size() {
        let img = decode(b"\"1;1;10;12#1~").unwrap();
        assert_eq!((img.0, img.1), (10, 12));
    }

    #[test]
    fn bad_sixel_is_harmless() {
        assert!(decode(b"!999999999~").map_or(true, |i| i.0 as usize <= MAX_SIDE));
        assert!(decode(b"\"1;1;99999;99999~").is_err());
        let _ = decode(b"#999;2;x;y;z#-5!-~#");
        let _ = decode(&[0xff, 0x00, b'#', b'!']);
        assert!(decode(b"").is_err(), "no pixels");
        let tall: Vec<u8> = std::iter::repeat_n(b"~-".as_slice(), 2000).flatten().copied().collect();
        assert!(decode(&tall).is_err(), "more than MAX_SIDE rows");
    }

    #[test]
    fn a_sixel_lands_at_the_cursor() {
        let mut s = crate::screen::Screen::new(20, 6);
        s.set_cell_px(2, 3);
        s.feed(b"ab\x1bPq#1;2;100;0;0#1~~~~\x1b\\");
        use alacritty_terminal::index::{Column, Line};
        assert!(s.term().grid()[Line(0)][Column(2)].image().is_some());
        assert!(s.term().grid()[Line(1)][Column(3)].image().is_some());
        assert_eq!(s.term().grid().cursor.point.column.0, 2, "the cursor column comes back");
    }
}
