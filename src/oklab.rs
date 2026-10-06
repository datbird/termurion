//! OKLab color math (Bjorn Ottosson's OKLab): the hue wheel's placement and the color preset
//! distance. Pure.

use crate::palette::Rgb;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OkLab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

fn linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn encode(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let v = if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (v * 255.0).round() as u8
}

pub fn from_srgb(c: Rgb) -> OkLab {
    let (r, g, b) = (linear(c.r), linear(c.g), linear(c.b));
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    OkLab {
        l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    }
}

impl OkLab {
    /// The color of a Display P3 color (gamma encoded, 0 to 1), through XYZ as iTerm2 does.
    pub fn from_p3(r: f64, g: f64, b: f64) -> OkLab {
        let lin = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        let (r, g, b) = (lin(r), lin(g), lin(b));
        // Linear Display P3 to XYZ (D65), then iTerm2's XYZ to LMS and LMS' to Lab matrices.
        let x = 0.486_570_948_6 * r + 0.265_667_693_2 * g + 0.198_217_285_2 * b;
        let y = 0.228_974_564_1 * r + 0.691_738_521_8 * g + 0.079_286_914_1 * b;
        let z = 0.045_113_381_9 * g + 1.043_944_368_9 * b;
        let l = (0.8189330101 * x + 0.3618667424 * y - 0.1288597137 * z).cbrt();
        let m = (0.0329845436 * x + 0.9293118715 * y + 0.0361456387 * z).cbrt();
        let s = (0.0482003018 * x + 0.2643662691 * y + 0.6338517070 * z).cbrt();
        OkLab {
            l: (0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s) as f32,
            a: (1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s) as f32,
            b: (0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s) as f32,
        }
    }

    /// The sRGB color, or None when it lies outside the sRGB gamut.
    pub fn to_srgb(&self) -> Option<Rgb> {
        let l = (self.l + 0.396_337_78 * self.a + 0.215_803_76 * self.b).powi(3);
        let m = (self.l - 0.105_561_346 * self.a - 0.063_854_17 * self.b).powi(3);
        let s = (self.l - 0.089_484_18 * self.a - 1.291_485_5 * self.b).powi(3);
        let r = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
        let g = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s;
        let b = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;
        // A hair of slack for float error at the gamut's edge.
        if [r, g, b].iter().any(|c| !(-0.002..=1.002).contains(c)) {
            return None;
        }
        Some(Rgb { r: encode(r), g: encode(g), b: encode(b) })
    }

    /// The angle of (a, b) in radians, from the a axis.
    pub fn hue(&self) -> f32 {
        self.b.atan2(self.a)
    }

    pub fn chroma(&self) -> f32 {
        self.a.hypot(self.b)
    }
}

/// The Euclidean distance between two colors in OKLab.
pub fn distance(a: OkLab, b: OkLab) -> f64 {
    let d = |x: f32, y: f32| (x as f64 - y as f64).powi(2);
    (d(a.l, b.l) + d(a.a, b.a) + d(a.b, b.b)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::rgb;

    #[test]
    fn oklab_follows_iterm2() {
        let white = from_srgb(rgb(255, 255, 255));
        assert!((white.l - 1.0).abs() < 1e-3 && white.a.abs() < 1e-3 && white.b.abs() < 1e-3, "{white:?}");
        let red = from_srgb(rgb(255, 0, 0));
        assert!((red.hue() - 0.5).abs() < 0.05, "{}", red.hue());
        assert!((red.chroma() - 0.26).abs() < 0.01, "{}", red.chroma());
        for c in [rgb(0, 0, 0), rgb(255, 255, 255), rgb(12, 200, 99), rgb(255, 0, 0), rgb(7, 7, 250)] {
            let back = from_srgb(c).to_srgb().expect("in gamut");
            assert!(back.r.abs_diff(c.r) <= 1 && back.g.abs_diff(c.g) <= 1 && back.b.abs_diff(c.b) <= 1, "{c:?} {back:?}");
        }
        let out = OkLab { l: 0.5, a: 0.4, b: 0.0 };
        assert_eq!(out.to_srgb(), None);
        assert!((out.chroma() - 0.4).abs() < 1e-6 && out.hue().abs() < 1e-6);
        // iTerm2 places colors through Display P3: the same OKLab as the direct sRGB path.
        for c in [rgb(0, 0, 0), rgb(255, 255, 255), rgb(12, 200, 99), rgb(255, 0, 0), rgb(7, 7, 250)] {
            let (r, g, b) = crate::import::srgb_to_p3(c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0);
            let (p, d) = (OkLab::from_p3(r, g, b), from_srgb(c));
            assert!((p.l - d.l).abs() < 2e-3 && (p.a - d.a).abs() < 2e-3 && (p.b - d.b).abs() < 2e-3, "{c:?} {p:?} {d:?}");
        }
        // Pure P3 green is wider than sRGB: more chroma than sRGB green.
        assert!(OkLab::from_p3(0.0, 1.0, 0.0).chroma() > from_srgb(rgb(0, 255, 0)).chroma());
        assert_eq!(distance(white, white), 0.0);
        let d = distance(OkLab { l: 0.0, a: 0.0, b: 0.0 }, OkLab { l: 0.3, a: 0.4, b: 0.0 });
        assert!((d - 0.5).abs() < 1e-6, "{d}");
    }
}
