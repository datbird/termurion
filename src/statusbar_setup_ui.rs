//! iTerm2's status bar setup window (`iTermStatusBarSetupViewController`): the Status Bar
//! Component Menu (the palette), Active Components (drag from the menu to add, drag within
//! to reorder, drag out or Delete to remove), Configure Component (a sheet with one control
//! per knob), Auto-Rainbow, Advanced (colors, font, and the three checkboxes), help, Cancel,
//! and OK. Every change goes to the profile at once (the bar redraws behind the window);
//! Cancel puts back the layout the window opened with.
//!
//! Citations use `.superpowers/research/m9-facts.md` section 1's abbreviations (`SVC` =
//! `iTermStatusBarSetupViewController.m`, `DCV` = the destination collection view
//! controller, `KVC` = the knobs view controller, `SXIB` = the setup xib).

use egui::{Color32, Id, Pos2, Rect, RichText, Sense, Stroke};
use toml::Value;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::statusbar::{self, Advanced, Algorithm, Knob, KnobKind, Layout, Rainbow};

/// What a frame of the window asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupEvent {
    None,
    /// The layout changed: write it to the profile now.
    Changed,
    /// OK: keep the layout.
    Ok,
    /// Cancel (or the window closed): put back the layout it opened with.
    Cancel,
}

/// iTerm2's help page for the setup window (`SVC:331-336`).
pub const HELP_URL: &str = "https://iterm2.com/status-bar-layout";

/// The Auto-Rainbow popup's items (`SXIB:198-207`).
const RAINBOW_ITEMS: [(Rainbow, &str); 4] = [(Rainbow::Disabled, "Disabled"), (Rainbow::Light, "Light Colors"), (Rainbow::Dark, "Dark Colors"), (Rainbow::Automatic, "Automatic")];

/// A drag in progress: from the palette (a class) or from Active Components (an index).
#[derive(Clone, Debug, PartialEq)]
enum Drag {
    Palette(&'static str),
    Active(usize),
}

/// The Configure Component sheet: the knobs of one component and their values being edited.
struct KnobSheet {
    index: usize,
    knobs: Vec<Knob>,
    values: Vec<Option<Value>>,
    /// A Double knob's text as typed.
    texts: Vec<String>,
    /// An Action knob's editor is showing.
    action_open: Vec<bool>,
}

/// The Advanced sheet: a working copy committed when it closes (`SVC:338-356`).
struct AdvancedSheet {
    advanced: Advanced,
    /// The font field, while "Change" is showing it.
    font_edit: Option<String>,
}

pub struct StatusBarSetupWindow {
    host: EguiHost,
    pub layout: Layout,
    selected: Option<usize>,
    /// For Automatic auto-rainbow: the profile's background is dark (`PSV:654-662`).
    dark_background: bool,
    /// Every profile, for an Action knob's profile parameter.
    profiles: Vec<(String, String)>,
    drag: Option<Drag>,
    sheet: Option<KnobSheet>,
    advanced: Option<AdvancedSheet>,
    /// The Configure Component sheet opens on the first frame (the bar's "Configure").
    configure_first: bool,
}

/// A `{r, g, b, a}` table as an egui color.
fn color32(v: &Value) -> Option<Color32> {
    let t = v.as_table()?;
    let c = |k: &str| t.get(k).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
    let byte = |f: f64| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
    Some(Color32::from_rgba_unmultiplied(byte(c("r")?), byte(c("g")?), byte(c("b")?), byte(c("a").unwrap_or(1.0))))
}

fn color_value(c: Color32) -> Value {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    let mut t = toml::Table::new();
    for (k, v) in [("r", r), ("g", g), ("b", b), ("a", a)] {
        t.insert(k.into(), Value::Float(v as f64 / 255.0));
    }
    Value::Table(t)
}

fn rgb32(c: crate::palette::Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

/// A color well that allows "no color" (CPKColorWell with `noColorAllowed`). True on change.
fn color_well(ui: &mut egui::Ui, value: &mut Option<Value>, alpha: bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut c = value.as_ref().and_then(color32).unwrap_or(Color32::from_gray(128));
        let a = if alpha { egui::color_picker::Alpha::OnlyBlend } else { egui::color_picker::Alpha::Opaque };
        if egui::color_picker::color_edit_button_srgba(ui, &mut c, a).changed() {
            *value = Some(color_value(c));
            changed = true;
        }
        if value.is_some() {
            if ui.small_button("No Color").clicked() {
                *value = None;
                changed = true;
            }
        } else {
            ui.weak("No color");
        }
    });
    changed
}

/// Where a drop at `pos` goes among `rects` (items laid out left to right in wrapped
/// rows): before the first item whose row is below the pointer, or that is in the
/// pointer's row with its middle to the right of it; else last. Always "drop before"
/// (`DCV:318`).
pub fn drop_index(rects: &[Rect], pos: Pos2) -> usize {
    for (i, r) in rects.iter().enumerate() {
        if pos.y < r.top() || (pos.y <= r.bottom() && pos.x < r.center().x) {
            return i;
        }
    }
    rects.len()
}

/// A double as the numeric knob shows it: "∞" and "-∞" for the infinities
/// (`iTermStatusBarKnobNumericViewController.m:27-40`).
pub fn format_double(v: f64) -> String {
    if v == f64::INFINITY {
        "∞".into()
    } else if v == f64::NEG_INFINITY {
        "-∞".into()
    } else {
        format!("{v}")
    }
}

/// Typed text as a double: "∞" and "-∞" (and "inf"), else the number, else 0 (as
/// `it_localizedDoubleValue` reads text that is not a number).
pub fn parse_double(s: &str) -> f64 {
    match s.trim() {
        "∞" | "inf" | "+∞" => f64::INFINITY,
        "-∞" | "-inf" => f64::NEG_INFINITY,
        t => t.parse().unwrap_or(0.0),
    }
}

fn as_f64(v: &Value) -> Option<f64> {
    v.as_float().or_else(|| v.as_integer().map(|i| i as f64)).or_else(|| v.as_str().map(parse_double))
}

/// The font label in the Advanced sheet (`SVC:270-280`): "System Font", or "12pt Name".
pub fn font_label(font: Option<&str>) -> String {
    match font.filter(|f| !f.trim().is_empty()) {
        None => "System Font".into(),
        Some(f) => {
            let spec = crate::profile::parse_font(f);
            format!("{}pt {}", spec.size_pt, spec.family)
        }
    }
}

/// A font string to start from when a font control is turned on.
const START_FONT: &str = "Segoe UI 12";

impl StatusBarSetupWindow {
    /// Opens the window on `layout`. With `select`, that component is selected; with
    /// `configure` too, its Configure Component sheet opens (the bar's "Configure").
    #[allow(clippy::too_many_arguments)]
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, title: &str, layout: Layout, dark_background: bool, profiles: Vec<(String, String)>, select: Option<usize>, configure: bool) -> Result<StatusBarSetupWindow, String> {
        // iTerm2's content view is 918 x 526 (`SXIB:28`).
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, title, 918.0, 526.0)?;
        add_symbol_font(host.ctx());
        let selected = select.filter(|&i| i < layout.components.len());
        Ok(StatusBarSetupWindow { host, layout, selected, dark_background, profiles, drag: None, sheet: None, advanced: None, configure_first: configure && selected.is_some() })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    pub fn event(&mut self, event: &WindowEvent) -> bool {
        self.host.event(event)
    }

    /// Selects a component and opens its Configure Component sheet, when it has knobs
    /// (`DCV:190-205`).
    pub fn configure(&mut self, index: usize) {
        if index < self.layout.components.len() {
            self.selected = Some(index);
            self.open_sheet(index);
            self.host.request_redraw();
        }
    }

    fn open_sheet(&mut self, index: usize) {
        let algorithm = self.layout.advanced.algorithm;
        let Some(c) = self.layout.components.get(index) else { return };
        let knobs = statusbar::knobs(&c.class, algorithm);
        if knobs.is_empty() {
            return;
        }
        // Each knob's saved value, else its default (`KVC:66-69`).
        let values: Vec<Option<Value>> = knobs.iter().map(|k| c.knobs.get(&k.key).cloned().or_else(|| k.default.clone())).collect();
        let texts = knobs.iter().zip(&values).map(|(k, v)| if k.kind == KnobKind::Double { v.as_ref().and_then(as_f64).map(format_double).unwrap_or_default() } else { String::new() }).collect();
        let action_open = vec![false; knobs.len()];
        self.sheet = Some(KnobSheet { index, knobs, values, texts, action_open });
    }

    /// After a change: the setup window's auto-rainbow colors every component, and each
    /// component's copy of the advanced configuration follows the layout's (iTerm2 builds
    /// the layout again from its dictionary, `L:122-123`).
    fn settle(&mut self) {
        statusbar::apply_rainbow(&mut self.layout, self.dark_background);
        statusbar::sync_advanced_copies(&mut self.layout);
    }

    pub fn redraw(&mut self) -> SetupEvent {
        if std::mem::take(&mut self.configure_first)
            && let Some(i) = self.selected
        {
            self.open_sheet(i);
        }
        let mut out = SetupEvent::None;
        let mut changed = false;
        let algorithm = self.layout.advanced.algorithm;
        let adv = self.layout.advanced.clone();
        let default_fg = adv.default_text_color.as_ref().and_then(color32).unwrap_or(rgb32(crate::scene::STATUS_BAR_TEXT));
        let layout_bg = adv.background_color.as_ref().and_then(color32);
        let default_bg = layout_bg.unwrap_or(rgb32(crate::scene::STATUS_BAR_BG));
        // The exemplar's own live data (`statusbar::exemplar`): the OS lookups a Clock,
        // User Name or Current Directory component's exemplar needs. `clock_text` is the
        // Clock's already-formatted text; Task 2 builds the formatter, so until then the
        // palette and Active Components show no sample time for it.
        let user = std::env::var("USERNAME").unwrap_or_default();
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        let clock_text = String::new();
        let sheet_open = self.sheet.is_some() || self.advanced.is_some();
        let mut open_sheet_for: Option<usize> = None;
        let mut open_advanced = false;
        let mut delete: Option<usize> = None;
        let mut insert: Option<(usize, &'static str)> = None;
        let mut moved: Option<(usize, usize)> = None;
        let mut rainbow = adv.rainbow;
        let (layout, selected, drag, profiles) = (&self.layout, &mut self.selected, &mut self.drag, &self.profiles);
        let sheet = &mut self.sheet;
        let advanced = &mut self.advanced;
        let mut sheet_done: Option<bool> = None;
        let mut advanced_done = false;
        self.host.paint(|ui| {
            let ctx = ui.ctx().clone();
            // A popup (the Auto-Rainbow list) open as the frame starts takes this frame's
            // Escape and Return: its own code closes it before the keys are read below.
            let popup_open = egui::Popup::is_any_open(&ctx);
            let drop_color = ui.visuals().selection.stroke.color;
            let pointer = ctx.input(|i| i.pointer.interact_pos());
            let released = ctx.input(|i| i.pointer.any_released());
            let mut active_area = Rect::NOTHING;
            let mut active_rects: Vec<Rect> = Vec::new();
            egui::CentralPanel::default().show(ui, |ui| {
                ui.add_enabled_ui(!sheet_open, |ui| {
                    ui.strong("Status Bar Component Menu");
                    ui.small("Drag a component from the menu to Active Components to add it to your status bar.");
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_min_size(egui::vec2(ui.available_width(), 150.0));
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                            for class in statusbar::PALETTE {
                                let Some(info) = statusbar::info_for(class, algorithm) else { continue };
                                // The palette shows the exemplar with the layout's colors and
                                // the short description below it (`SVC:381-397`); a generic
                                // instance's default knobs, since nothing is placed yet.
                                let exemplar = statusbar::exemplar(class, &statusbar::default_knobs(class), &clock_text, &user, &home, algorithm);
                                let r = element(ui, Id::new(("palette", class)), &exemplar, Some(info.short), default_bg, default_fg, false);
                                let r = r.on_hover_text(info.detail);
                                if r.drag_started() {
                                    *drag = Some(Drag::Palette(class));
                                }
                            }
                        });
                    });
                    ui.add_space(6.0);
                    ui.strong("Active Components");
                    let group = egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_min_size(egui::vec2(ui.available_width(), 150.0));
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                            for (i, c) in layout.components.iter().enumerate() {
                                let info = statusbar::info_for(&c.class, algorithm);
                                // A class Tandem does not show yet (milestone 9i's five)
                                // keeps its place under its class name.
                                let unknown = short_class(&c.class);
                                let exemplar = match info {
                                    Some(_) => statusbar::exemplar(&c.class, &c.knobs, &clock_text, &user, &home, algorithm),
                                    None => unknown.clone(),
                                };
                                let bg = c.background_color().as_ref().and_then(color32).or(layout_bg).unwrap_or(default_bg);
                                let fg = if info.is_some() { c.text_color().as_ref().and_then(color32).unwrap_or(default_fg) } else { Color32::GRAY };
                                // Destination items show only the exemplar (`DCV:221-238`).
                                let r = element(ui, Id::new(("active", i)), &exemplar, None, bg, fg, *selected == Some(i));
                                let r = match info {
                                    Some(info) => r.on_hover_text(info.detail),
                                    None => r.on_hover_text(format!("{}: shown in a later milestone", c.class)),
                                };
                                if r.clicked() {
                                    *selected = Some(i);
                                }
                                if r.double_clicked() {
                                    open_sheet_for = Some(i);
                                }
                                if r.drag_started() {
                                    *selected = Some(i);
                                    *drag = Some(Drag::Active(i));
                                }
                                active_rects.push(r.rect);
                            }
                        });
                    });
                    active_area = group.response.rect;
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let has_knobs = selected.and_then(|i| layout.components.get(i)).is_some_and(|c| !statusbar::knobs(&c.class, algorithm).is_empty());
                        if ui.add_enabled(has_knobs, egui::Button::new("Configure Component")).clicked() {
                            open_sheet_for = *selected;
                        }
                        ui.add_space(12.0);
                        ui.label("Auto-Rainbow:");
                        let shown = RAINBOW_ITEMS.iter().find(|(r, _)| *r == rainbow).map_or("Disabled", |(_, n)| n);
                        egui::ComboBox::from_id_salt("rainbow").selected_text(shown).show_ui(ui, |ui| {
                            for (k, (style, name)) in RAINBOW_ITEMS.iter().enumerate() {
                                ui.selectable_value(&mut rainbow, *style, *name);
                                if k == 0 {
                                    ui.separator();
                                }
                            }
                        });
                        if ui.button("?").on_hover_text(HELP_URL).clicked() {
                            ctx.open_url(egui::OpenUrl::new_tab(HELP_URL));
                        }
                        if ui.button("Advanced...").clicked() {
                            open_advanced = true;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("OK").clicked() {
                                out = SetupEvent::Ok;
                            }
                            if ui.button("Cancel").clicked() {
                                out = SetupEvent::Cancel;
                            }
                        });
                    });
                });
            });

            // Dragging: the item follows the pointer; over Active Components a line shows
            // where it goes. A release there drops (always before); an Active item released
            // anywhere else is removed (`DCV:278-289`).
            if let (Some(d), Some(pos)) = (drag.clone(), pointer) {
                let label = match &d {
                    Drag::Palette(class) => statusbar::exemplar(class, &statusbar::default_knobs(class), &clock_text, &user, &home, algorithm),
                    Drag::Active(i) => layout
                        .components
                        .get(*i)
                        .map(|c| match statusbar::info_for(&c.class, algorithm) {
                            Some(_) => statusbar::exemplar(&c.class, &c.knobs, &clock_text, &user, &home, algorithm),
                            None => short_class(&c.class),
                        })
                        .unwrap_or_default(),
                };
                egui::Area::new(Id::new("setup-drag")).order(egui::Order::Tooltip).fixed_pos(pos + egui::vec2(8.0, 8.0)).interactable(false).show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(label).monospace().size(14.0)).selectable(false));
                    });
                });
                let over = active_area.contains(pos);
                if over {
                    let at = drop_index(&active_rects, pos);
                    let x = match active_rects.get(at) {
                        Some(r) => Rect::from_min_max(egui::pos2(r.left() - 5.0, r.top()), egui::pos2(r.left() - 3.0, r.bottom())),
                        None => active_rects.last().map_or(Rect::from_min_size(active_area.min + egui::vec2(8.0, 8.0), egui::vec2(2.0, 24.0)), |r| Rect::from_min_max(egui::pos2(r.right() + 3.0, r.top()), egui::pos2(r.right() + 5.0, r.bottom()))),
                    };
                    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("setup-drop"))).rect_filled(x, 1.0, drop_color);
                }
                if released {
                    match d {
                        Drag::Palette(class) if over => insert = Some((drop_index(&active_rects, pos), class)),
                        Drag::Active(from) if over => moved = Some((from, drop_index(&active_rects, pos))),
                        Drag::Active(from) => delete = Some(from),
                        Drag::Palette(_) => {}
                    }
                    *drag = None;
                }
                ctx.request_repaint();
            } else if released {
                *drag = None;
            }

            // Delete or Backspace removes the selected component (`DCV:90-117`); Escape
            // is Cancel and Return is OK (`SXIB:127-152`); none of them while a popup is
            // open, whose Escape closes only the popup.
            let popup_open = popup_open || egui::Popup::is_any_open(&ctx);
            if !sheet_open && !popup_open && !ctx.egui_wants_keyboard_input() {
                if ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) {
                    delete = *selected;
                }
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    out = SetupEvent::Cancel;
                }
                if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    out = SetupEvent::Ok;
                }
            }

            if let Some(s) = sheet.as_mut() {
                sheet_done = knob_sheet(&ctx, s, profiles);
            }
            if let Some(a) = advanced.as_mut() {
                advanced_done = advanced_sheet(&ctx, a);
            }
        });

        if let Some(i) = open_sheet_for {
            self.open_sheet(i);
        }
        if open_advanced {
            self.advanced = Some(AdvancedSheet { advanced: self.layout.advanced.clone(), font_edit: None });
        }
        if let Some((at, class)) = insert {
            let at = statusbar::insert_component(&mut self.layout, at, class);
            self.selected = Some(at);
            changed = true;
        }
        if let Some((from, to)) = moved {
            let before = self.layout.components.clone();
            let at = statusbar::move_component(&mut self.layout, from, to);
            self.selected = Some(at);
            changed |= self.layout.components != before;
        }
        if let Some(i) = delete.filter(|&i| i < self.layout.components.len()) {
            self.layout.components.remove(i);
            self.selected = None;
            self.sheet = None;
            changed = true;
        }
        if rainbow != self.layout.advanced.rainbow {
            self.layout.advanced.rainbow = rainbow;
            changed = true;
        }
        match sheet_done {
            Some(true) => {
                let s = self.sheet.take().unwrap();
                let values: toml::Table = s.knobs.iter().zip(s.values).filter_map(|(k, v)| v.map(|v| (k.key.clone(), v))).collect();
                statusbar::set_knob_values(&mut self.layout, s.index, values);
                changed = true;
            }
            Some(false) => self.sheet = None,
            None => {}
        }
        if advanced_done {
            let a = self.advanced.take().unwrap();
            self.layout.advanced = a.advanced;
            changed = true;
        }
        if changed {
            self.settle();
            self.host.request_redraw();
            if out == SetupEvent::None {
                out = SetupEvent::Changed;
            }
        }
        out
    }
}

/// `iTermStatusBarFooComponent` as "Foo", for a class Tandem does not show yet.
fn short_class(class: &str) -> String {
    let s = class.strip_prefix("iTermStatusBar").unwrap_or(class);
    s.strip_suffix("Component").unwrap_or(s).to_string()
}

/// One element box: a rounded box (corner radius 4, `SCI`) with the exemplar, and the
/// caption below it in the palette. Click to select, drag to move.
fn element(ui: &mut egui::Ui, id: Id, exemplar: &str, caption: Option<&str>, bg: Color32, fg: Color32, selected: bool) -> egui::Response {
    let stroke = if selected { Stroke::new(2.0, ui.visuals().selection.stroke.color) } else { ui.visuals().widgets.noninteractive.bg_stroke };
    let inner = ui.vertical(|ui| {
        let r = egui::Frame::new().fill(bg).corner_radius(4.0).stroke(stroke).inner_margin(egui::Margin::symmetric(10, 4)).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(if exemplar.is_empty() { " " } else { exemplar }).monospace().size(14.0).color(fg)).selectable(false));
        });
        if let Some(c) = caption {
            ui.add(egui::Label::new(RichText::new(c).small()).selectable(false));
        }
        r.response.rect
    });
    let rect = inner.response.rect.union(inner.inner);
    ui.interact(rect, id, Sense::click_and_drag())
}

/// The Configure Component sheet (`CCW`, `KVC`): the knobs top to bottom, labels on a
/// common column, Cancel (Escape) and OK (Return). Some(true) on OK, Some(false) on Cancel.
fn knob_sheet(ctx: &egui::Context, s: &mut KnobSheet, profiles: &[(String, String)]) -> Option<bool> {
    let mut out = None;
    egui::Modal::new(Id::new("configure-component")).show(ctx, |ui| {
        ui.set_min_width(480.0 - 16.0);
        egui::Grid::new("knobs").num_columns(2).spacing(egui::vec2(8.0, 5.0)).show(ui, |ui| {
            for i in 0..s.knobs.len() {
                let k = s.knobs[i].clone();
                match k.kind {
                    // A checkbox titled with the label (`iTermStatusBarKnobCheckboxViewController`).
                    KnobKind::Checkbox => {
                        ui.label("");
                        let mut on = s.values[i].as_ref().and_then(Value::as_bool).unwrap_or(false);
                        if ui.checkbox(&mut on, &k.label).changed() {
                            s.values[i] = Some(Value::Boolean(on));
                        }
                    }
                    KnobKind::Text | KnobKind::Invocation => {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(&k.label));
                        let mut text = s.values[i].as_ref().and_then(Value::as_str).unwrap_or("").to_string();
                        let r = ui.add(egui::TextEdit::singleline(&mut text).hint_text(k.placeholder.clone().unwrap_or_default()).desired_width(260.0));
                        if r.changed() {
                            s.values[i] = Some(Value::String(text));
                        }
                        // Return in a text knob is OK (`iTermStatusBarKnobTextViewController.m:132-135`).
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            out = Some(true);
                        }
                    }
                    // A text field and a stepper (`iTermStatusBarKnobNumericViewController`).
                    KnobKind::Double => {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(&k.label));
                        ui.horizontal(|ui| {
                            let r = ui.add(egui::TextEdit::singleline(&mut s.texts[i]).hint_text(k.placeholder.clone().unwrap_or_default()).desired_width(90.0));
                            if r.changed() {
                                s.values[i] = Some(Value::Float(parse_double(&s.texts[i])));
                            }
                            if r.lost_focus() {
                                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                    out = Some(true);
                                }
                                s.texts[i] = s.values[i].as_ref().and_then(as_f64).map(format_double).unwrap_or_default();
                            }
                            for (label, step) in [("-", -1.0), ("+", 1.0)] {
                                if ui.small_button(label).clicked() {
                                    let v = s.values[i].as_ref().and_then(as_f64).unwrap_or(0.0) + step;
                                    s.values[i] = Some(Value::Float(v));
                                    s.texts[i] = format_double(v);
                                }
                            }
                        });
                    }
                    KnobKind::Color => {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(&k.label));
                        color_well(ui, &mut s.values[i], true);
                    }
                    // "Configure Action..." opens the key action editor without a key
                    // (`iTermStatusBarKnobActionViewController.m:40-58`).
                    KnobKind::Action => {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(&k.label));
                        ui.vertical(|ui| {
                            if ui.button("Configure Action...").clicked() {
                                s.action_open[i] = !s.action_open[i];
                            }
                            if s.action_open[i] {
                                let mut a = s.values[i].as_ref().and_then(crate::user_actions::UserAction::from_value).unwrap_or(crate::user_actions::UserAction { action: -1, title: String::new(), parameter: String::new(), escaping: crate::keyaction::Escaping::Common, apply_mode: 0, version: crate::user_actions::VERSION });
                                let mut b = a.binding();
                                let salt = format!("knob-action-{i}");
                                let mut edited = false;
                                ui.horizontal(|ui| {
                                    ui.label("Title:");
                                    edited |= ui.add(egui::TextEdit::singleline(&mut a.title).desired_width(240.0)).changed();
                                });
                                if crate::settings_keys::action_combo(ui, &format!("{salt}-action"), &mut b.action, false) {
                                    b.text.clear();
                                    edited = true;
                                }
                                edited |= crate::settings_keys::param_editor(ui, &format!("{salt}-param"), &mut b, profiles);
                                edited |= crate::settings_keys::apply_mode_combo(ui, &format!("{salt}-apply"), &mut b);
                                if edited {
                                    let na = crate::user_actions::UserAction { action: b.action, title: a.title.clone(), parameter: b.text.clone(), escaping: b.escaping(), apply_mode: b.apply_mode, version: crate::user_actions::VERSION };
                                    s.values[i] = Some(na.to_value());
                                }
                            }
                        });
                    }
                    // A checkbox titled with the label; on, a font picker; off stores ""
                    // (use the layout's font) (`StatusBarKnobFontViewController.swift`).
                    KnobKind::Font => {
                        ui.label("");
                        ui.vertical(|ui| {
                            let mut font = s.values[i].as_ref().and_then(Value::as_str).unwrap_or("").to_string();
                            let mut on = !font.is_empty();
                            if ui.checkbox(&mut on, &k.label).changed() {
                                font = if on { START_FONT.to_string() } else { String::new() };
                                s.values[i] = Some(Value::String(font.clone()));
                            }
                            if on {
                                ui.add_space(8.0);
                                if ui.add(egui::TextEdit::singleline(&mut font).desired_width(200.0)).on_hover_text("Font name and size, such as \"Segoe UI 12\". The size is used; the family stays the prose font.").changed() {
                                    s.values[i] = Some(Value::String(font));
                                }
                            }
                        });
                    }
                }
                ui.end_row();
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("OK").clicked() {
                    out = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    out = Some(false);
                }
            });
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            out = Some(false);
        }
        if !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            out = Some(true);
        }
    });
    out
}

/// The Advanced sheet (`SXIB:225-340`). True when OK closes it.
fn advanced_sheet(ctx: &egui::Context, a: &mut AdvancedSheet) -> bool {
    let mut done = false;
    egui::Modal::new(Id::new("advanced")).show(ctx, |ui| {
        ui.set_min_width(468.0 - 16.0);
        let adv = &mut a.advanced;
        egui::Grid::new("advanced-grid").num_columns(2).spacing(egui::vec2(8.0, 6.0)).show(ui, |ui| {
            ui.label("Separator Color:");
            color_well(ui, &mut adv.separator_color, true);
            ui.end_row();
            ui.label("Background Color:");
            color_well(ui, &mut adv.background_color, false);
            ui.end_row();
            ui.label("Default Text Color:");
            color_well(ui, &mut adv.default_text_color, false);
            ui.end_row();
            ui.label("Font:");
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(font_label(adv.font.as_deref()));
                    if ui.small_button("Change").clicked() {
                        a.font_edit = Some(adv.font.clone().unwrap_or_else(|| START_FONT.to_string()));
                    }
                });
                let mut reset = false;
                if let Some(edit) = a.font_edit.as_mut() {
                    ui.horizontal(|ui| {
                        if ui.add(egui::TextEdit::singleline(edit).desired_width(180.0)).on_hover_text("Font name and size, such as \"Segoe UI 12\". The size is used; the family stays the prose font.").changed() {
                            let f = edit.trim().to_string();
                            adv.font = (!f.is_empty()).then_some(f);
                        }
                        reset = ui.button("Reset to System Font").clicked();
                    });
                }
                if reset {
                    adv.font = None;
                    a.font_edit = None;
                }
            });
            ui.end_row();
        });
        ui.add_space(6.0);
        let mut tight = adv.algorithm == Algorithm::TightlyPacked;
        let tight_tip = "The tight packing algorithm allows more status bar components to be visible at a time but they tend to jump around more.";
        if ui.checkbox(&mut tight, "Prefer tight packing to stable positioning").on_hover_text(tight_tip).changed() {
            adv.algorithm = if tight { Algorithm::TightlyPacked } else { Algorithm::Stable };
        }
        // iTerm2's xib gives this checkbox the tight-packing tooltip (`SXIB:304-307`).
        ui.checkbox(&mut adv.remove_empty, "Remove components with empty values").on_hover_text(tight_tip);
        ui.checkbox(&mut adv.edge_separator, "Draw separator between status bar and terminal").on_hover_text("Draws a thin line between the status bar and the terminal content.");
        ui.separator();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("OK").clicked() {
                done = true;
            }
        });
        if !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            done = true;
        }
    });
    done
}

/// Adds a system font with the exemplars' symbols (box drawing, the wave dash) as egui's
/// last fallback, when Windows has one.
fn add_symbol_font(ctx: &egui::Context) {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    for path in ["C:\\Windows\\Fonts\\msgothic.ttc", "C:\\Windows\\Fonts\\YuGothR.ttc", "C:\\Windows\\Fonts\\seguisym.ttf"] {
        if let Ok(bytes) = std::fs::read(path) {
            let families = [egui::FontFamily::Proportional, egui::FontFamily::Monospace].into_iter().map(|family| InsertFontFamily { family, priority: FontPriority::Lowest }).collect();
            ctx.add_font(FontInsert::new("status-bar-symbols", egui::FontData::from_owned(bytes), families));
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_controls_follow_iterm2() {
        // Drops go before the item under the pointer, rows wrapping.
        let r = |x: f32, y: f32| Rect::from_min_size(egui::pos2(x, y), egui::vec2(40.0, 20.0));
        let rects = [r(0.0, 0.0), r(50.0, 0.0), r(0.0, 30.0)];
        assert_eq!(drop_index(&rects, egui::pos2(5.0, 10.0)), 0);
        assert_eq!(drop_index(&rects, egui::pos2(30.0, 10.0)), 1);
        assert_eq!(drop_index(&rects, egui::pos2(95.0, 10.0)), 2, "right of the first row: before the next row's first");
        assert_eq!(drop_index(&rects, egui::pos2(35.0, 40.0)), 3);
        assert_eq!(drop_index(&[], egui::pos2(0.0, 0.0)), 0);

        // The numeric knob shows and reads the infinities.
        assert_eq!(format_double(f64::INFINITY), "∞");
        assert_eq!(format_double(f64::NEG_INFINITY), "-∞");
        assert_eq!(format_double(5.0), "5");
        assert_eq!(format_double(0.01), "0.01");
        assert_eq!(parse_double("∞"), f64::INFINITY);
        assert_eq!(parse_double(" -∞ "), f64::NEG_INFINITY);
        assert_eq!(parse_double("2.5"), 2.5);
        assert_eq!(parse_double("abc"), 0.0);

        assert_eq!(font_label(None), "System Font");
        assert_eq!(font_label(Some("")), "System Font");
        assert_eq!(font_label(Some("Menlo 14")), "14pt Menlo");
        assert_eq!(short_class("iTermStatusBarClockComponent"), "Clock");

        let v = color_value(Color32::from_rgba_unmultiplied(255, 0, 128, 255));
        assert_eq!(color32(&v), Some(Color32::from_rgba_unmultiplied(255, 0, 128, 255)));
    }
}
