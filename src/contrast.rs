//! Minimum contrast (iTerm2's `NSColor+iTerm.m` `getComponents:...withContrastAgainstComponents:`).

use crate::palette::Rgb;

pub const RED: f64 = 0.30;
pub const GREEN: f64 = 0.59;
pub const BLUE: f64 = 0.11;

/// `PerceivedBrightness` (`NC:36-40`): the weighted sum of the components as stored (not
/// linearized), each 0 to 1.
pub fn perceived_brightness(r: f64, g: f64, b: f64) -> f64 {
    RED * r + GREEN * g + BLUE * b
}

/// Moves `c` along the line to white (when it is dimmer than `target`) or black until its
/// perceived brightness is `target` (`getComponents:forColorWithRed:...perceivedBrightness:`).
pub fn with_brightness(c: [f64; 3], target: f64) -> [f64; 3] {
    let [r, g, b] = c;
    let k = if perceived_brightness(r, g, b) < target { 1.0 } else { 0.0 };
    let p = (target - perceived_brightness(r, g, b)) / perceived_brightness(k - r, k - g, k - b);
    // p is NaN or infinite when the color already is the extreme (division by 0).
    let p = if p.is_nan() { 0.0 } else { p.clamp(0.0, 1.0) };
    [p * k + (1.0 - p) * r, p * k + (1.0 - p) * g, p * k + (1.0 - p) * b]
}

/// `getComponents:forComponents:withContrastAgainstComponents:minimumContrast:` (`NC:694-744`).
pub fn with_min_contrast(text: [f64; 3], bg: [f64; 3], min: f64) -> [f64; 3] {
    let main = perceived_brightness(text[0], text[1], text[2]);
    let other = perceived_brightness(bg[0], bg[1], bg[2]);
    let diff = (main - other).abs();
    if diff >= min {
        return text;
    }
    let error = (diff - min).abs();
    let mut target = main;
    if main < other {
        // To increase contrast, go dimmer; when that runs below black, take the bright side
        // if it gives more contrast than black could.
        target -= error;
        if target < 0.0 {
            let alternative = other + min;
            if alternative.min(1.0) - other > other {
                target = alternative;
            }
        }
    } else {
        target += error;
        if target > 1.0 {
            let alternative = other - min;
            if other - alternative.max(0.0) > 1.0 - other {
                target = alternative;
            }
        }
    }
    with_brightness(text, target.clamp(0.0, 1.0))
}

/// A text color adjusted for `min` (0 to 1) against its background; 0 returns it unchanged.
pub fn apply(text: Rgb, bg: Rgb, min: f32) -> Rgb {
    if min <= 0.0 {
        return text;
    }
    let f = |c: Rgb| [c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0];
    let out = with_min_contrast(f(text), f(bg), min as f64);
    let u = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    if out == f(text) {
        return text;
    }
    Rgb { r: u(out[0]), g: u(out[1]), b: u(out[2]) }
}

/// Minimum Contrast at or above this shows the "Black & White Text Only" warning.
pub const WARNING_AT: f64 = 0.97;
pub const WARNING_TEXT: &str = "Black & White Text Only";

/// The Colors pane shows the warning (`PCP:459-463`) when the value is at least `WARNING_AT`.
pub fn warns(min: f64) -> bool {
    min >= WARNING_AT
}

/// CIE Lab (D65), iTerm2's `iTermLABColor`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lab {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

/// iTerm2's reference white (D65, 2 degrees; `NC:329-345`).
const WHITE: [f64; 3] = [0.95047, 1.0, 1.08883];

fn linearize(n: f64) -> f64 {
    if n > 0.04045 { ((n + 0.055) / 1.055).powf(2.4) } else { n / 12.92 }
}

/// `iTermCompressRGB`: linear light back to sRGB.
fn compress(l: f64) -> f64 {
    if l <= 0.003130 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 }
}

/// `iTermLABFromSRGB` (`NC:116-131`): components 0 to 1 as stored.
pub fn lab_from_srgb(c: [f64; 3]) -> Lab {
    let [r, g, b] = [linearize(c[0]), linearize(c[1]), linearize(c[2])];
    let xyz = [
        0.4124564 * r + 0.3575761 * g + 0.1804375 * b,
        0.2126729 * r + 0.7151522 * g + 0.0721750 * b,
        0.0193339 * r + 0.1191920 * g + 0.9503041 * b,
    ];
    let pivot = |v: f64| if v > 0.008856 { v.cbrt() } else { 7.787 * v + 16.0 / 116.0 };
    let [x, y, z] = [pivot(xyz[0] / WHITE[0]), pivot(xyz[1] / WHITE[1]), pivot(xyz[2] / WHITE[2])];
    Lab { l: 116.0 * y - 16.0, a: 500.0 * (x - y), b: 200.0 * (y - z) }
}

/// `iTermSRGBFromLAB` (`NC:177-197`): clamped to the gamut, components 0 to 1.
pub fn srgb_from_lab(lab: Lab) -> [f64; 3] {
    let y = (lab.l + 16.0) / 116.0;
    let unpivot = |v: f64| if v.powi(3) > 0.008856 { v.powi(3) } else { (v - 16.0 / 116.0) / 7.787 };
    let x = unpivot(lab.a / 500.0 + y) * WHITE[0];
    let z = unpivot(y - lab.b / 200.0) * WHITE[2];
    let y = unpivot(y) * WHITE[1];
    let lin = [
        x * 3.2406 + y * -1.5372 + z * -0.4986,
        x * -0.9689 + y * 1.8758 + z * 0.0415,
        x * 0.0557 + y * -0.2040 + z * 1.0570,
    ];
    lin.map(|v| compress(v.clamp(0.0, 1.0)))
}

/// `lerp_lab` of the harmonizer (`MS:6108-6117`).
pub fn lerp_lab(t: f64, a: Lab, b: Lab) -> Lab {
    Lab { l: a.l + t * (b.l - a.l), a: a.a + t * (b.a - a.a), b: a.b + t * (b.b - a.b) }
}

/// The harmonized 256-color table, indexes 16 to 255 (`MS:6119-6181`, from Jake Stewart's
/// gist): the 6x6x6 cube interpolated in Lab between the background, the eight base colors
/// and the foreground, then 24 grays from the background to the foreground. Termurion always
/// converts back to sRGB (the P3 ruling).
pub fn harmonized_256(base8: [Rgb; 8], bg: Rgb, fg: Rgb) -> [Rgb; 240] {
    let lab = |c: Rgb| lab_from_srgb([c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0]);
    let out = |l: Lab| {
        let u = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
        let [r, g, b] = srgb_from_lab(l);
        Rgb { r: u(r), g: u(g), b: u(b) }
    };
    let base = base8.map(lab);
    let (bg, fg) = (lab(bg), lab(fg));
    let mut table = [Rgb { r: 0, g: 0, b: 0 }; 240];
    let mut i = 0;
    for r in 0..6 {
        let t = r as f64 / 5.0;
        let c0 = lerp_lab(t, bg, base[1]);
        let c1 = lerp_lab(t, base[2], base[3]);
        let c2 = lerp_lab(t, base[4], base[5]);
        let c3 = lerp_lab(t, base[6], fg);
        for g in 0..6 {
            let t = g as f64 / 5.0;
            let c4 = lerp_lab(t, c0, c1);
            let c5 = lerp_lab(t, c2, c3);
            for b in 0..6 {
                table[i] = out(lerp_lab(b as f64 / 5.0, c4, c5));
                i += 1;
            }
        }
    }
    for k in 0..24 {
        table[i] = out(lerp_lab((k + 1) as f64 / 25.0, bg, fg));
        i += 1;
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 0.001)
    }

    #[test]
    fn contrast_follows_iterm2() {
        assert!((perceived_brightness(1.0, 1.0, 1.0) - 1.0).abs() < 1e-9);
        assert!((perceived_brightness(1.0, 0.0, 0.0) - 0.30).abs() < 1e-9);
        // Same color on itself: the brighter target 1.0 fits.
        assert!(near(with_min_contrast([0.5; 3], [0.5; 3], 0.5), [1.0; 3]));
        // Dimmer target -0.2 is below 0; the alternative 0.8 gives more contrast than black could.
        assert!(near(with_min_contrast([0.2; 3], [0.3; 3], 0.5), [0.8; 3]));
        assert!(near(with_min_contrast([0.9; 3], [0.95; 3], 0.5), [0.45; 3]));
        assert!(near(with_min_contrast([0.0, 0.0, 1.0], [0.0; 3], 0.0), [0.0, 0.0, 1.0]));
        assert!(near(with_min_contrast([1.0, 0.0, 0.0], [0.0; 3], 0.6), [1.0, 0.4286, 0.4286]));
        // The brighter target overflows; the darker side offers 0.502 against 0.498.
        assert_eq!(apply(rgb(0x80, 0x80, 0x80), rgb(0x80, 0x80, 0x80), 1.0), rgb(0, 0, 0));
        assert!(warns(0.97));
        assert!(!warns(0.96));
        assert_eq!(WARNING_TEXT, "Black & White Text Only");
        // Zero keeps the color; a color that already contrasts is kept.
        assert_eq!(apply(rgb(10, 20, 30), rgb(10, 20, 30), 0.0), rgb(10, 20, 30));
        assert_eq!(apply(rgb(255, 255, 255), rgb(0, 0, 0), 0.5), rgb(255, 255, 255));
    }

    #[test]
    fn brightness_agrees_with_the_tab_theme_copy() {
        for c in [rgb(0, 0, 0), rgb(255, 255, 255), rgb(200, 30, 90), rgb(12, 240, 100)] {
            let ours = perceived_brightness(c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0);
            assert!((ours - crate::tab_theme::perceived(c) as f64).abs() < 1e-4, "{c:?}");
        }
    }

    #[test]
    fn lab_follows_iterm2() {
        let white = lab_from_srgb([1.0; 3]);
        assert!((white.l - 100.0).abs() < 0.01 && white.a.abs() < 0.01 && white.b.abs() < 0.01, "{white:?}");
        let black = lab_from_srgb([0.0; 3]);
        assert!(black.l.abs() < 1e-9 && black.a.abs() < 1e-9 && black.b.abs() < 1e-9, "{black:?}");
        assert!(near_by(srgb_from_lab(lab_from_srgb([0.2, 0.4, 0.6])), [0.2, 0.4, 0.6], 0.002));
        let mid = lerp_lab(0.5, Lab { l: 0.0, a: 0.0, b: 0.0 }, Lab { l: 100.0, a: 0.0, b: 0.0 });
        assert_eq!(mid, Lab { l: 50.0, a: 0.0, b: 0.0 });
    }

    fn near_by(a: [f64; 3], b: [f64; 3], eps: f64) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < eps)
    }

    fn within_one(a: Rgb, b: Rgb) -> bool {
        a.r.abs_diff(b.r) <= 1 && a.g.abs_diff(b.g) <= 1 && a.b.abs_diff(b.b) <= 1
    }

    fn rgb_of(c: [f64; 3]) -> Rgb {
        let u = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
        rgb(u(c[0]), u(c[1]), u(c[2]))
    }

    #[test]
    fn harmony_follows_iterm2() {
        let base8 = [rgb(0, 0, 0), rgb(187, 0, 0), rgb(0, 187, 0), rgb(187, 187, 0), rgb(0, 0, 187), rgb(187, 0, 187), rgb(0, 187, 187), rgb(187, 187, 187)];
        let (bg, fg) = (rgb(0, 0, 0), rgb(255, 255, 255));
        let t = harmonized_256(base8, bg, fg);
        let at = |i: usize| t[i - 16];
        assert!(within_one(at(16), bg), "{:?}", at(16));
        assert!(within_one(at(231), fg), "{:?}", at(231));
        assert!(within_one(at(196), base8[1]), "{:?}", at(196));
        let f = |c: Rgb| [c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0];
        let gray = |t: f64| rgb_of(srgb_from_lab(lerp_lab(t, lab_from_srgb(f(bg)), lab_from_srgb(f(fg)))));
        assert!(within_one(at(232), gray(1.0 / 25.0)), "{:?} vs {:?}", at(232), gray(1.0 / 25.0));
        assert!(within_one(at(255), gray(24.0 / 25.0)), "{:?}", at(255));
        // (0,5,0) is green and (0,0,5) is blue: the other corners of the r = 0 face.
        assert!(within_one(at(46), base8[2]), "{:?}", at(46));
        assert!(within_one(at(21), base8[4]), "{:?}", at(21));
    }
}
