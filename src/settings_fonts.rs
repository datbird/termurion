//! Profiles > Text's font block, as iTerm2 lays it out (`ProfilesTextPreferencesViewController`
//! and its `BFPCompositeView` font pickers): "Font:" with a family menu, a size and the spacings,
//! the Anti-aliased and ligatures switches under it with their hints, "Use a different font for
//! non-ASCII text", and the non-ASCII picker. Every profile has its own fonts.

use toml::Value;

use crate::profile::Profile;
use crate::schema::{self, Setting};

/// iTerm2 keeps the spacings between 50 and 200 percent (`saveChangesFromFontPicker`).
pub const SPACING_MIN: f64 = 50.0;
pub const SPACING_MAX: f64 = 200.0;

/// The hint beside the ligatures switch while it may slow drawing (`PreferencePanel.xib`; its
/// sign is the warning icon).
pub const LIGATURES_HINT: &str = "This may reduce performance";
/// The hint beside Anti-aliased for a font that asks for it.
pub const RECOMMENDED_HINT: &str = "(recommended for this font)";

/// A font as the profile stores it: the family, then the size in points.
pub fn font_string(family: &str, size_pt: f32) -> String {
    if size_pt.fract() == 0.0 { format!("{family} {}", size_pt as u32) } else { format!("{family} {size_pt}") }
}

/// The families containing `needle`, ignoring case; all of them for an empty needle.
pub fn filtered<'a>(families: &'a [String], needle: &str) -> Vec<&'a String> {
    let n = needle.trim().to_lowercase();
    families.iter().filter(|f| n.is_empty() || f.to_lowercase().contains(&n)).collect()
}

/// Whether the ligatures hint shows for the font named `family` (`shouldShowASCIILigaturesWarning`):
/// not with right-to-left support on (it has no ligatures), not for a font that never has them
/// (`fonts::blacklisted`), and only while the switch is on.
pub fn ligatures_hint(family: &str, switch_on: bool, bidi: bool) -> bool {
    switch_on && !bidi && !crate::fonts::blacklisted(&family.replace(' ', ""))
}

fn setting(key: &str) -> &'static Setting {
    schema::find(key).unwrap_or_else(|| panic!("schema row {key} is missing"))
}

/// A switch drawn by the generic profile control.
fn check(ui: &mut egui::Ui, profile: &mut Profile, key: &str) -> bool {
    crate::settings_ui::control(ui, profile, setting(key), false)
}

/// One font picker: its label, the family menu with a search box, and the size.
fn picker(ui: &mut egui::Ui, profile: &mut Profile, key: &str, families: &[String]) -> bool {
    let s = setting(key);
    let spec = profile.font(key);
    let mut size = spec.size_pt;
    let mut picked: Option<String> = None;
    let mut size_changed = false;
    let mut rect = None;
    ui.horizontal(|ui| {
        ui.label(&s.label);
        if families.is_empty() {
            // The installed fonts are not known yet: type the family.
            let mut family = spec.family.clone();
            let r = ui.add(egui::TextEdit::singleline(&mut family).desired_width(220.0));
            rect = Some(r.rect);
            if r.changed() {
                picked = Some(family);
            }
        } else {
            let filter_id = egui::Id::new(("font-filter", key));
            let r = egui::ComboBox::from_id_salt((key, "family")).selected_text(spec.family.clone()).width(220.0).show_ui(ui, |ui| {
                let mut filter: String = ui.data_mut(|d| d.get_temp::<String>(filter_id)).unwrap_or_default();
                ui.add(egui::TextEdit::singleline(&mut filter).hint_text("Search fonts").desired_width(200.0));
                let list = filtered(families, &filter);
                let row = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
                egui::ScrollArea::vertical().max_height(260.0).show_rows(ui, row, list.len(), |ui, range| {
                    for f in &list[range] {
                        if ui.selectable_label(f.eq_ignore_ascii_case(&spec.family), f.as_str()).clicked() {
                            picked = Some((*f).clone());
                        }
                    }
                });
                ui.data_mut(|d| d.insert_temp(filter_id, filter));
            });
            rect = Some(r.response.rect);
        }
        let d = ui.add(egui::DragValue::new(&mut size).range(4.0..=128.0).speed(0.25).suffix(" pt"));
        size_changed = d.changed();
    })
    .response
    .on_hover_text(&s.help);
    if let Some(r) = rect.filter(|_| crate::settings_keys::indicate_non_default() && !crate::settings_ui::profile_is_default(profile, s, key)) {
        crate::settings_keys::non_default_bar(ui, r, ui.is_enabled());
    }
    if picked.is_some() || size_changed {
        let family = picked.unwrap_or(spec.family);
        profile.set(key, Value::String(font_string(&family, size)));
        return true;
    }
    false
}

/// The two spacing fields under the ASCII picker (iTerm2 shows them as icons with a percent).
fn spacings(ui: &mut egui::Ui, profile: &mut Profile) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        for key in ["Horizontal Spacing", "Vertical Spacing"] {
            let s = setting(key);
            ui.label(&s.label);
            let mut pct = (profile.float(key) * 100.0).clamp(SPACING_MIN, SPACING_MAX);
            let r = ui.add(egui::DragValue::new(&mut pct).range(SPACING_MIN..=SPACING_MAX).speed(1.0).suffix("%")).on_hover_text(&s.help);
            if r.changed() {
                profile.set(key, Value::Float(pct / 100.0));
                changed = true;
            }
        }
    });
    changed
}

/// An Anti-aliased or ligatures switch, with its hint after it.
fn switch_with_hint(ui: &mut egui::Ui, profile: &mut Profile, key: &str, hint: Option<(&'static str, bool)>) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        changed = check(ui, profile, key);
        if let Some((text, warning)) = hint {
            if warning {
                if let Some(g) = crate::settings_keys::icon_text(ui.ctx(), crate::scripts::WARNING_GLYPH) {
                    ui.label(g.color(ui.visuals().warn_fg_color));
                }
            }
            ui.label(egui::RichText::new(text).small());
        }
    });
    changed
}

/// The font block. `families` are the installed families (empty until the app has listed them),
/// `bidi` is the right-to-left switch, and `recommended(family)` whether the font asks for
/// anti-aliasing.
pub fn group(ui: &mut egui::Ui, profile: &mut Profile, families: &[String], bidi: bool, recommended: &dyn Fn(&str, f32) -> bool) -> bool {
    let mut changed = picker(ui, profile, "Normal Font", families);
    changed |= spacings(ui, profile);
    let normal = profile.font("Normal Font");
    let ascii = normal.family.clone();
    let rec = recommended(&ascii, normal.size_pt).then_some((RECOMMENDED_HINT, false));
    changed |= switch_with_hint(ui, profile, "ASCII Anti Aliased", rec);
    let lig = ligatures_hint(&ascii, profile.bool("ASCII Ligatures"), bidi).then_some((LIGATURES_HINT, true));
    changed |= switch_with_hint(ui, profile, "ASCII Ligatures", lig);
    changed |= check(ui, profile, "Use Non-ASCII Font");
    if profile.bool("Use Non-ASCII Font") {
        changed |= picker(ui, profile, "Non Ascii Font", families);
        let non_ascii = profile.font("Non Ascii Font");
        let other = non_ascii.family.clone();
        let rec = recommended(&other, non_ascii.size_pt).then_some((RECOMMENDED_HINT, false));
        changed |= switch_with_hint(ui, profile, "Non-ASCII Anti Aliased", rec);
        let lig = ligatures_hint(&other, profile.bool("Non-ASCII Ligatures"), bidi).then_some((LIGATURES_HINT, true));
        changed |= switch_with_hint(ui, profile, "Non-ASCII Ligatures", lig);
    }
    changed
}

/// The size in pixels per em a font of `size_pt` points is drawn at (96 pixels to the inch), the
/// key the app answers the "(recommended for this font)" question under.
pub fn ppem(size_pt: f32) -> u16 {
    (size_pt * 96.0 / 72.0).round().clamp(1.0, 4096.0) as u16
}

/// The question "does `family` at `size_pt` ask for anti-aliasing?" as the hint table keys it.
pub fn hint_key(family: &str, size_pt: f32) -> String {
    format!("{family}@{}", ppem(size_pt))
}

/// The keys `group` draws; the profile loop skips them.
pub const GROUPED: [&str; 9] = ["Normal Font", "Non Ascii Font", "Use Non-ASCII Font", "Horizontal Spacing", "Vertical Spacing", "ASCII Anti Aliased", "Non-ASCII Anti Aliased", "ASCII Ligatures", "Non-ASCII Ligatures"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_font_is_its_family_and_size() {
        assert_eq!(font_string("Cascadia Mono", 12.0), "Cascadia Mono 12");
        assert_eq!(font_string("Consolas", 11.5), "Consolas 11.5");
        assert_eq!(crate::profile::parse_font(&font_string("Cascadia Mono", 12.0)).family, "Cascadia Mono");
    }

    #[test]
    fn the_family_search_ignores_case() {
        let all: Vec<String> = ["Cascadia Mono", "Consolas", "JetBrains Mono"].map(String::from).to_vec();
        assert_eq!(filtered(&all, "mono"), vec![&all[0], &all[2]]);
        assert_eq!(filtered(&all, "").len(), 3);
        assert!(filtered(&all, "zzz").is_empty());
    }

    #[test]
    fn the_ligatures_hint_follows_iterm2() {
        assert!(ligatures_hint("Cascadia Code", true, false));
        assert!(!ligatures_hint("Cascadia Code", false, false), "the switch is off");
        assert!(!ligatures_hint("Cascadia Code", true, true), "right-to-left support has no ligatures");
        assert!(!ligatures_hint("Courier New", true, false), "a font on iTerm2's blacklist never has them");
    }

    #[test]
    fn the_block_draws_every_grouped_key() {
        for key in GROUPED {
            assert!(schema::find(key).is_some(), "{key}");
        }
    }
}
