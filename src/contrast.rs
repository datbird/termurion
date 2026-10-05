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
}
