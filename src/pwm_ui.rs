//! The password manager window (`W`, `WX`): the search field, the accounts table, the bottom
//! row of buttons, the gear pull-down, the row menu, and, modal inside the window, the New
//! Account / Edit Account panel, the alerts and the busy scrim. The window is an egui host
//! window owned by its terminal window (iTerm2 shows a sheet), so it stays above it. It holds
//! only what is being typed; everything else comes from `PwmView` each redraw, and what the
//! user does comes back as `PwmAct`s. The rules (titles, enabling, Tab, double-click, the gear
//! validation) are `pwm.rs`'s, not re-derived here.
//!
//! Secrets: a typed or fetched password lives in a `pwm::Secret` edited in place by
//! `secret_field` (egui's `TextEdit` keeps plain copies of its text in its undo history), and
//! leaves only in a `PwmAct` (whose `Debug` is redacted). A revealed password is drawn as text,
//! so egui lays it out for the frames it shows; nothing logs or prints it.
//!
//! `disablePasswordManagerAnimations` has nothing to turn off: egui windows do not animate open
//! and the busy scrim appears at once. The shake of a refused panel is iTerm2's `it_shakeNo`,
//! which that setting does not cover.

use std::time::Instant;

use winit::event::{ElementState, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::Key;
use winit::window::{Window, WindowId};

use crate::egui_host::EguiHost;
use crate::pwm::{self, Account, Buttons, Entry, RowItem, Secret};
pub use crate::pwm::{AccountPanel, Alert, Gear, GearItem};

/// What the window shows, built by the app at each redraw.
pub struct PwmView<'a> {
    /// Every account, sorted (`pwm::sorted`); the window filters them by `filter` itself, and
    /// every index in `selected` and in a `PwmAct` is an index into this list.
    pub accounts: &'a [Account],
    pub filter: &'a str,
    pub selected: Option<usize>,
    pub buttons: Buttons,
    /// `pwm::Busy::shown`.
    pub busy: bool,
    pub gear: Gear,
    pub panel: Option<&'a AccountPanel>,
    pub alert: Option<&'a Alert>,
}

/// What the user asked for. Indexes are into `PwmView::accounts`. `Debug` is safe: every
/// password is a `Secret`.
#[derive(Debug)]
pub enum PwmAct {
    Filter(String),
    Select(usize),
    EnterPassword,
    EnterUserName,
    Close,
    Add,
    Remove,
    Edit(usize),
    TwoFactor(String),
    Broadcast(bool),
    Gear(GearItem),
    AppendOtp(usize),
    Reveal(usize),
    CopyPassword(usize),
    PanelOk { name: String, user: String, password: Secret, toggle: Option<bool> },
    PanelCancel,
    /// The dice: the app answers with `PwmWindow::fill_password`.
    Generate { alnum: bool },
    AlertButton(usize),
    /// OK on an adapter Settings sheet: every field as (key, value).
    Settings(Vec<(String, Secret)>),
    CancelBusy,
}

/// The New Account / Edit Account fields being typed, for the panel with `serial`.
struct PanelFields {
    serial: u64,
    name: String,
    user: String,
    password: Secret,
    toggle: bool,
    first: bool,
}

/// One adapter Settings value being typed.
enum SettingValue {
    Plain(String),
    /// The value and whether Show is on.
    Secret(Secret, bool),
}

/// How long a refused panel shakes.
const SHAKE_SECS: f32 = 0.4;
/// The table's header and row heights (`WX:64-92`: 17-point rows and 2 points apart).
const HEADER_H: f32 = 17.0;
const ROW_H: f32 = 19.0;
/// The xib's column widths, scaled to the table's width.
const COLUMNS: [f32; 3] = [383.0, 101.0, 212.0];
/// The Settings sheet's width and label column (`W:1363-1366`).
const SETTINGS_WIDTH: f32 = 560.0;
const SETTINGS_LABEL: f32 = 90.0;
const ALERT_WIDTH: f32 = 420.0;

const SEARCH_ID: &str = "pwm-search";
const TABLE_ID: &str = "pwm-table";
const TWO_FACTOR_ID: &str = "pwm-2fa";
const PANEL_NAME_ID: &str = "pwm-panel-name";
const PANEL_PASSWORD_ID: &str = "pwm-panel-password";

pub struct PwmWindow {
    host: EguiHost,
    search: String,
    two_factor: String,
    broadcast: bool,
    panel: Option<PanelFields>,
    settings: Option<Vec<SettingValue>>,
    shake: Option<Instant>,
    focus_password: bool,
    /// The search field takes the focus once the window is authenticated.
    focus_search: bool,
    win_down: bool,
    /// Win chords (b, u, c) waiting for the next redraw.
    chords: Vec<char>,
}

impl PwmWindow {
    /// A window over `owner` (the terminal window's HWND), centered on it under its title bar.
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        owner: isize,
    ) -> Result<PwmWindow, String> {
        use winit::platform::windows::WindowAttributesExtWindows;
        let (w, h) = pwm::WINDOW_SIZE;
        let attrs = Window::default_attributes()
            .with_title(pwm::TITLE_WINDOW)
            .with_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64))
            .with_min_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64))
            .with_owner_window(owner)
            .with_visible(false);
        let host = EguiHost::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        let dark = host.window.theme().is_none_or(|t| t == winit::window::Theme::Dark);
        host.ctx().set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
        if let Some(at) = sheet_origin(owner, host.window.outer_size()) {
            host.window.set_outer_position(at);
        }
        host.window.set_visible(true);
        host.focus();
        Ok(PwmWindow {
            host,
            search: String::new(),
            two_factor: String::new(),
            broadcast: false,
            panel: None,
            settings: None,
            shake: None,
            focus_password: false,
            focus_search: true,
            win_down: false,
            chords: Vec::new(),
        })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    /// The window's Win32 handle (for the Win chord hook).
    pub fn hwnd(&self) -> Option<isize> {
        self.host.hwnd()
    }

    pub fn request_redraw(&self) {
        self.host.request_redraw();
    }

    /// A Win chord for the window (Win+B, Win+U, Win+C), from the low-level key hook: Windows
    /// keeps those chords for itself unless the hook takes them.
    pub fn win_chord(&mut self, key: char) {
        self.chords.push(key.to_ascii_lowercase());
        self.host.request_redraw();
    }

    /// Puts `p` in the panel's password field: the Edit panel's fetched password, or the dice's.
    /// The field's old value is wiped.
    pub fn fill_password(&mut self, p: Secret) {
        if let Some(f) = self.panel.as_mut() {
            f.password = p;
        }
        self.host.request_redraw();
    }

    /// The app refused the panel's OK: it shakes (`it_shakeNo`), and with `focus_password` the
    /// password field takes the focus (Add without a required password).
    pub fn shake(&mut self, focus_password: bool) {
        self.shake = Some(Instant::now());
        self.focus_password = focus_password;
        self.host.request_redraw();
    }

    /// Feeds a window event to egui. True when the window should redraw.
    pub fn event(&mut self, e: &WindowEvent) -> bool {
        match e {
            WindowEvent::ModifiersChanged(m) => self.win_down = m.state().super_key(),
            WindowEvent::KeyboardInput { event, .. } if self.win_down && event.state == ElementState::Pressed && !event.repeat => {
                if let Key::Character(c) = &event.logical_key {
                    let c = c.to_ascii_lowercase();
                    if matches!(c.as_str(), "b" | "u" | "c") {
                        self.chords.extend(c.chars());
                        self.host.request_redraw();
                    }
                }
            }
            WindowEvent::Focused(false) => self.win_down = false,
            _ => {}
        }
        self.host.event(e)
    }

    /// Draws the window from `v`; returns what the user did.
    pub fn redraw(&mut self, v: &PwmView) -> Vec<PwmAct> {
        let mut acts = Vec::new();
        self.sync(v);
        let shaking = self.shake.map(|t| t.elapsed().as_secs_f32()).filter(|s| *s < SHAKE_SECS);
        if shaking.is_none() {
            self.shake = None;
        }
        let chords = std::mem::take(&mut self.chords);
        let Self { host, search, two_factor, broadcast, panel, settings, focus_password, focus_search, .. } = self;
        host.paint(|ui| {
            let ctx = ui.ctx().clone();
            let modal = v.busy || v.panel.is_some() || v.alert.is_some();
            let rows: Vec<usize> = (0..v.accounts.len()).filter(|&i| pwm::matches(&v.accounts[i], search)).collect();
            if !modal {
                keys(&ctx, v, &rows, &chords, &mut acts);
                for c in &chords {
                    if *c == 'b' && v.buttons.broadcast_on {
                        *broadcast = !*broadcast;
                        acts.push(PwmAct::Broadcast(*broadcast));
                    }
                }
            }
            if *focus_search && v.gear.authenticated {
                ctx.memory_mut(|m| m.request_focus(egui::Id::new(SEARCH_ID)));
                *focus_search = false;
            }
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(ui.visuals().panel_fill).inner_margin(20)).show(ui, |ui| {
                main(ui, v, &rows, search, two_factor, broadcast, &mut acts);
            });
            if let (Some(p), Some(f)) = (v.panel, panel.as_mut()) {
                panel_modal(&ctx, p, f, shaking, focus_password, &mut acts);
            }
            if let Some(a) = v.alert {
                alert_modal(&ctx, a, settings, &mut acts);
            }
            if v.busy {
                busy_modal(&ctx, &mut acts);
            }
        });
        // Closing clears the 2FA field (`W:2339-2344`).
        if acts.iter().any(|a| matches!(a, PwmAct::Close)) {
            self.two_factor.clear();
        }
        let fetching = v.panel.is_some_and(|p| p.fetching);
        if v.busy || fetching || shaking.is_some() {
            self.host.request_redraw();
        }
        acts
    }

    /// Follows the view: a new panel reseeds the typed fields, a closed one drops them (their
    /// password wiped), and so for a Settings sheet's values.
    fn sync(&mut self, v: &PwmView) {
        match v.panel {
            Some(p) if self.panel.as_ref().is_some_and(|f| f.serial == p.serial) => {}
            Some(p) => {
                self.panel = Some(PanelFields {
                    serial: p.serial,
                    name: p.name.clone(),
                    user: p.user.clone(),
                    password: Secret::new(String::new()),
                    toggle: p.toggle.as_ref().is_some_and(|t| t.default_value),
                    first: true,
                });
                self.shake = None;
                self.focus_password = false;
            }
            None => self.panel = None,
        }
        match v.alert {
            Some(a) if !a.fields.is_empty() => {
                if self.settings.is_none() {
                    self.settings = Some(
                        a.fields
                            .iter()
                            .map(|(f, val)| if f.is_secret { SettingValue::Secret(val.duplicate(), false) } else { SettingValue::Plain(val.expose().to_string()) })
                            .collect(),
                    );
                }
            }
            _ => self.settings = None,
        }
        if self.search != v.filter {
            self.search = v.filter.to_string();
        }
    }
}

/// The owner's client area's top, centered across it: where a sheet would drop from.
fn sheet_origin(owner: isize, size: winit::dpi::PhysicalSize<u32>) -> Option<winit::dpi::PhysicalPosition<i32>> {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if unsafe { GetClientRect(owner as _, &mut rect) } == 0 {
        return None;
    }
    let mut top_left = POINT { x: 0, y: 0 };
    if unsafe { ClientToScreen(owner as _, &mut top_left) } == 0 {
        return None;
    }
    let x = top_left.x + (rect.right - rect.left - size.width as i32) / 2;
    Some(winit::dpi::PhysicalPosition::new(x, top_left.y))
}

/// The default button's action: Enter User Name when that is its title, else Enter Password
/// (which enters the user name first in send-both mode; `performDefaultAction:`).
fn default_act(b: &Buttons) -> PwmAct {
    if b.default_title == pwm::TITLE_ENTER_USER_NAME {
        PwmAct::EnterUserName
    } else {
        PwmAct::EnterPassword
    }
}

fn has_focus(ctx: &egui::Context, id: &str) -> bool {
    ctx.memory(|m| m.has_focus(egui::Id::new(id)))
}

/// A text field with a non-empty selection keeps Ctrl+C for itself.
fn field_selection(ctx: &egui::Context, id: &str) -> bool {
    let id = egui::Id::new(id);
    ctx.memory(|m| m.has_focus(id)) && egui::TextEdit::load_state(ctx, id).and_then(|s| s.cursor.char_range()).is_some_and(|r| !r.is_empty())
}

/// The window's keys with no panel, alert or scrim up: Return (the default button), Esc
/// (Close), Up and Down from the search field or the table, Delete and Backspace in the table
/// ("-"), Tab to the 2FA field (`tab_to_two_factor`), Ctrl+C and Win+C (copy the selected
/// row's password), Win+U (Enter User Name). Win+B is the caller's (it flips the check box).
fn keys(ctx: &egui::Context, v: &PwmView, rows: &[usize], chords: &[char], acts: &mut Vec<PwmAct>) {
    use egui::{Key, Modifiers};
    let b = &v.buttons;
    let search = has_focus(ctx, SEARCH_ID);
    let table = has_focus(ctx, TABLE_ID);
    let tab = ctx.input(|i| i.key_pressed(Key::Tab) && i.modifiers.is_none());
    if tab && b.two_factor_on && pwm::tab_to_two_factor(table, search, rows.len()) {
        ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab));
        ctx.memory_mut(|m| {
            m.move_focus(egui::FocusDirection::None);
            m.request_focus(egui::Id::new(TWO_FACTOR_ID));
        });
    }
    if search || table {
        let up = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp));
        let down = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown));
        let at = v.selected.and_then(|s| rows.iter().position(|&r| r == s));
        let next = match (up, down, at) {
            (true, _, Some(p)) => Some(p.saturating_sub(1)),
            (_, true, Some(p)) => Some((p + 1).min(rows.len().saturating_sub(1))),
            (true, _, None) if !rows.is_empty() => Some(rows.len() - 1),
            (_, true, None) if !rows.is_empty() => Some(0),
            _ => None,
        };
        if let Some(p) = next {
            if Some(rows[p]) != v.selected {
                acts.push(PwmAct::Select(rows[p]));
            }
        }
    }
    if table && b.remove_on {
        let del = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace));
        if del {
            acts.push(PwmAct::Remove);
        }
    }
    if ctx.input(|i| i.key_pressed(Key::Enter) && i.modifiers.is_none()) && b.default_on {
        acts.push(default_act(b));
    }
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        acts.push(PwmAct::Close);
    }
    let copy = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)))
        && !field_selection(ctx, SEARCH_ID)
        && !field_selection(ctx, TWO_FACTOR_ID);
    if copy || chords.contains(&'c') {
        if let Some(s) = v.selected {
            acts.push(PwmAct::CopyPassword(s));
        }
    }
    if chords.contains(&'u') && b.secondary_shown && b.secondary_on {
        acts.push(PwmAct::EnterUserName);
    }
}

/// The search row, the table and the bottom row.
fn main(ui: &mut egui::Ui, v: &PwmView, rows: &[usize], search: &mut String, two_factor: &mut String, broadcast: &mut bool, acts: &mut Vec<PwmAct>) {
    let b = &v.buttons;
    ui.horizontal(|ui| {
        let filter = egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, tab: false, escape: false };
        let r = ui.add(
            egui::TextEdit::singleline(search)
                .id(egui::Id::new(SEARCH_ID))
                .hint_text(pwm::PLACEHOLDER_SEARCH)
                .event_filter(filter)
                .return_key(None)
                .desired_width(ui.available_width() - 36.0),
        );
        if r.changed() {
            acts.push(PwmAct::Filter(search.clone()));
            // One remaining row is selected (`W:2370-2372`).
            let now: Vec<usize> = (0..v.accounts.len()).filter(|&i| pwm::matches(&v.accounts[i], search)).collect();
            if now.len() == 1 && v.selected != Some(now[0]) {
                acts.push(PwmAct::Select(now[0]));
            }
        }
        gear_button(ui, &v.gear, acts);
    });
    ui.add_space(8.0);
    let bottom = 34.0;
    let table_h = (ui.available_height() - bottom).max(HEADER_H + ROW_H);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), table_h), egui::Sense::hover());
    table(ui, rect, v, rows, acts);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let small = egui::vec2(25.0, 22.0);
        if ui.add_enabled(b.add_on, egui::Button::new(pwm::TITLE_ADD).min_size(small)).clicked() {
            acts.push(PwmAct::Add);
        }
        if ui.add_enabled(b.remove_on, egui::Button::new(pwm::TITLE_REMOVE).min_size(small)).clicked() {
            acts.push(PwmAct::Remove);
        }
        if ui.add_enabled(b.edit_on, egui::Button::new(pwm::TITLE_EDIT).min_size(egui::vec2(95.0, 22.0))).clicked() {
            if let Some(s) = v.selected {
                acts.push(PwmAct::Edit(s));
            }
        }
        let r = ui
            .add_enabled(
                b.two_factor_on,
                egui::TextEdit::singleline(two_factor).id(egui::Id::new(TWO_FACTOR_ID)).hint_text(pwm::PLACEHOLDER_TWO_FACTOR).return_key(None).desired_width(100.0),
            )
            .on_hover_text(pwm::TOOLTIP_TWO_FACTOR);
        if r.changed() {
            acts.push(PwmAct::TwoFactor(two_factor.clone()));
        }
        if ui.add_enabled(b.broadcast_on, egui::Checkbox::new(broadcast, pwm::TITLE_BROADCAST)).changed() {
            acts.push(PwmAct::Broadcast(*broadcast));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let wide = egui::vec2(0.0, 26.0);
            if ui.add_enabled(b.default_on, egui::Button::new(b.default_title).min_size(wide)).clicked() {
                acts.push(default_act(b));
            }
            if b.secondary_shown {
                let r = ui
                    .add_enabled(b.secondary_on, egui::Button::new(pwm::TITLE_ENTER_USER_NAME).min_size(wide))
                    .on_hover_text(pwm::TOOLTIP_ENTER_USER_NAME)
                    .on_disabled_hover_text(pwm::TOOLTIP_ENTER_USER_NAME);
                if r.clicked() {
                    acts.push(PwmAct::EnterUserName);
                }
            }
            if ui.add(egui::Button::new(pwm::TITLE_CLOSE).min_size(wide)).clicked() {
                acts.push(PwmAct::Close);
            }
        });
    });
}

/// A menu entry with a check mark column.
fn menu_entry<T: Clone>(ui: &mut egui::Ui, e: &Entry<T>) -> Option<T> {
    use egui::AtomExt;
    match e {
        Entry::Separator => {
            ui.separator();
            None
        }
        Entry::Item { title, checked, enabled, act } => {
            let mark = egui::RichText::new(if *checked { "\u{2713}" } else { "" }).atom_size(egui::vec2(14.0, 14.0));
            let clicked = ui.add_enabled(*enabled, egui::Button::new((mark, title.as_str())).frame_when_inactive(false)).clicked();
            if clicked {
                ui.close();
                return Some(act.clone());
            }
            None
        }
    }
}

/// The gear pull-down (`WX:243-314`), validated by `pwm::gear_menu`.
fn gear_button(ui: &mut egui::Ui, g: &Gear, acts: &mut Vec<PwmAct>) {
    ui.menu_button("\u{2699}", |ui| {
        ui.set_min_width(330.0);
        for e in pwm::gear_menu(g) {
            if let Some(item) = menu_entry(ui, &e) {
                acts.push(PwmAct::Gear(item));
            }
        }
    });
}

/// The accounts table: the column headers, alternating rows, single selection, a double-click
/// by `pwm::double_click`, and the row menu on a right click.
fn table(ui: &mut egui::Ui, rect: egui::Rect, v: &PwmView, rows: &[usize], acts: &mut Vec<PwmAct>) {
    let table_id = egui::Id::new(TABLE_ID);
    let focus = ui.interact(rect, table_id, egui::Sense::click());
    if focus.clicked() {
        focus.request_focus();
    }
    if focus.has_focus() {
        let filter = egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, tab: false, escape: false };
        ui.memory_mut(|m| m.set_focus_lock_filter(table_id, filter));
    }
    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
    painter.rect_stroke(rect, 0.0, visuals.widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
    let total: f32 = COLUMNS.iter().sum();
    let widths: Vec<f32> = COLUMNS.iter().map(|w| w / total * rect.width()).collect();
    let col_x = |c: usize| rect.left() + widths[..c].iter().sum::<f32>();
    let text = visuals.text_color();
    let font = egui::TextStyle::Body.resolve(ui.style());
    let header = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), HEADER_H));
    painter.rect_filled(header, 0.0, visuals.faint_bg_color);
    for (c, title) in [pwm::COLUMN_ACCOUNT_NAME, pwm::COLUMN_USER_NAME, pwm::COLUMN_PASSWORD].into_iter().enumerate() {
        let cell = egui::Rect::from_min_size(egui::pos2(col_x(c), rect.top()), egui::vec2(widths[c], HEADER_H));
        ui.painter_at(cell).text(cell.left_center() + egui::vec2(5.0, 0.0), egui::Align2::LEFT_CENTER, title, font.clone(), visuals.weak_text_color());
        if c > 0 {
            painter.vline(cell.left(), header.y_range(), visuals.widgets.noninteractive.bg_stroke);
        }
    }
    let body = egui::Rect::from_min_max(egui::pos2(rect.left(), header.bottom()), rect.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body.shrink2(egui::vec2(1.0, 0.0))));
    child.set_clip_rect(body);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(&mut child, ROW_H, rows.len(), |ui, range| {
        for n in range {
            let i = rows[n];
            let a = &v.accounts[i];
            let (row, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), egui::Sense::click());
            let selected = v.selected == Some(i);
            let fill = if selected {
                Some(visuals.selection.bg_fill)
            } else if n % 2 == 1 {
                Some(visuals.faint_bg_color)
            } else {
                None
            };
            if let Some(f) = fill {
                ui.painter().rect_filled(row, 0.0, f);
            }
            let color = if selected { visuals.selection.stroke.color } else { text };
            let name = pwm::row_name(a);
            for (c, s) in [name.as_str(), a.user_name.as_str(), pwm::PASSWORD_CELL].into_iter().enumerate() {
                let cell = egui::Rect::from_min_size(egui::pos2(col_x(c), row.top()), egui::vec2(widths[c], ROW_H));
                ui.painter_at(cell.shrink2(egui::vec2(4.0, 0.0))).text(cell.left_center() + egui::vec2(5.0, 0.0), egui::Align2::LEFT_CENTER, s, font.clone(), color);
            }
            if resp.clicked() || resp.secondary_clicked() {
                ui.memory_mut(|m| m.request_focus(table_id));
            }
            if resp.clicked() && !selected {
                acts.push(PwmAct::Select(i));
            }
            if resp.double_clicked() {
                let x = resp.interact_pointer_pos().map_or(0.0, |p| p.x);
                let column = (0..3).rev().find(|&c| x >= col_x(c)).unwrap_or(0);
                match pwm::double_click(column) {
                    pwm::Double::Edit => acts.push(PwmAct::Edit(i)),
                    pwm::Double::Default => acts.push(default_act(&v.buttons)),
                }
            }
            resp.context_menu(|ui| {
                for e in pwm::row_menu(a) {
                    match menu_entry(ui, &e) {
                        Some(RowItem::AppendOtp) => acts.push(PwmAct::AppendOtp(i)),
                        Some(RowItem::Reveal) => acts.push(PwmAct::Reveal(i)),
                        Some(RowItem::Copy) => acts.push(PwmAct::CopyPassword(i)),
                        None => {}
                    }
                }
            });
        }
    });
}

/// A password field that edits a `Secret` in place: typing and pasting append, Backspace
/// deletes the last character (Ctrl+Backspace all of it), and it shows a bullet per character,
/// or the text itself when `shown`. It never copies to the clipboard.
fn secret_field(ui: &mut egui::Ui, id: egui::Id, s: &mut Secret, enabled: bool, hint: Option<&str>, width: f32, shown: bool) -> egui::Response {
    let h = ui.spacing().interact_size.y;
    let (_, rect) = ui.allocate_space(egui::vec2(width, h));
    let resp = ui.interact(rect, id, if enabled { egui::Sense::click() } else { egui::Sense::hover() });
    if resp.clicked() {
        resp.request_focus();
    }
    let focused = enabled && resp.has_focus();
    if focused {
        ui.input(|i| {
            for e in &i.events {
                match e {
                    egui::Event::Text(t) | egui::Event::Paste(t) => s.push_str(t),
                    egui::Event::Key { key: egui::Key::Backspace, pressed: true, modifiers, .. } => {
                        if modifiers.command {
                            s.clear();
                        } else {
                            s.pop();
                        }
                    }
                    _ => {}
                }
            }
        });
        ui.output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                purpose: egui::IMEPurpose::Password,
                rect,
                cursor_rect: rect,
                should_interrupt_composition: false,
            });
        });
    }
    let v = ui.visuals();
    let stroke = if focused { v.selection.stroke } else { v.widgets.inactive.bg_stroke };
    let painter = ui.painter_at(rect);
    painter.rect(rect, 2.0, v.extreme_bg_color, stroke, egui::StrokeKind::Inside);
    let font = egui::TextStyle::Body.resolve(ui.style());
    let at = rect.left_center() + egui::vec2(4.0, 0.0);
    let end = if s.is_empty() {
        if let Some(hint) = hint {
            painter.text(at, egui::Align2::LEFT_CENTER, hint, font.clone(), v.weak_text_color());
        }
        at.x
    } else {
        let color = if enabled { v.text_color() } else { v.weak_text_color() };
        let r = if shown {
            painter.text(at, egui::Align2::LEFT_CENTER, s.expose(), font.clone(), color)
        } else {
            painter.text(at, egui::Align2::LEFT_CENTER, "\u{2022}".repeat(s.expose().chars().count()), font.clone(), color)
        };
        r.right()
    };
    if focused {
        painter.vline(end + 1.0, rect.shrink(4.0).y_range(), egui::Stroke::new(1.0, v.text_color()));
    }
    resp
}

/// The New Account / Edit Account panel (`W:710-792`): 330 wide, labels on the right of a
/// common column, the toggle row with its note when adding with a source that has one, a
/// spinner in the password field's place while an edit fetches, OK (Return; off without a
/// name or while fetching) and Cancel (Esc).
fn panel_modal(ctx: &egui::Context, p: &AccountPanel, f: &mut PanelFields, shaking: Option<f32>, focus_password: &mut bool, acts: &mut Vec<PwmAct>) {
    let dx = shaking.map_or(0.0, |t| 12.0 * (t / SHAKE_SECS * 3.0 * std::f32::consts::TAU).sin() * (1.0 - t / SHAKE_SECS));
    let area = egui::Modal::default_area(egui::Id::new("pwm-panel")).anchor(egui::Align2::CENTER_CENTER, egui::vec2(dx, 0.0));
    egui::Modal::new(egui::Id::new("pwm-panel")).area(area).show(ctx, |ui| {
        let content = p.width() - 2.0 * 18.0;
        ui.set_width(content);
        ui.set_min_height(p.height() - 60.0);
        ui.strong(p.title());
        ui.add_space(6.0);
        let label = |ui: &mut egui::Ui, s: &str| {
            ui.allocate_ui_with_layout(egui::vec2(74.0, 20.0), egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(s));
        };
        egui::Grid::new("pwm-panel-grid").num_columns(2).spacing(egui::vec2(6.0, 6.0)).show(ui, |ui| {
            label(ui, pwm::LABEL_ACCOUNT);
            let r = ui.add(egui::TextEdit::singleline(&mut f.name).id(egui::Id::new(PANEL_NAME_ID)).hint_text(pwm::PLACEHOLDER_REQUIRED).return_key(None).desired_width(content - 80.0));
            if f.first {
                r.request_focus();
                f.first = false;
            }
            ui.end_row();
            label(ui, pwm::LABEL_USER_NAME);
            ui.add(egui::TextEdit::singleline(&mut f.user).return_key(None).desired_width(content - 80.0));
            ui.end_row();
            label(ui, pwm::LABEL_PASSWORD);
            ui.horizontal(|ui| {
                let field_w = content - 80.0 - 30.0;
                if p.fetching {
                    ui.allocate_ui(egui::vec2(field_w, 20.0), |ui| ui.add(egui::Spinner::new().size(16.0)));
                } else {
                    let id = egui::Id::new(PANEL_PASSWORD_ID);
                    secret_field(ui, id, &mut f.password, p.password_on, p.placeholder, field_w, false);
                    if std::mem::take(focus_password) {
                        ui.memory_mut(|m| m.request_focus(id));
                    }
                }
                let dice = ui.add_enabled(p.dice_on(), egui::Button::new("\u{1F3B2}")).on_hover_text(pwm::TOOLTIP_GENERATE_PASSWORD);
                if dice.clicked() {
                    acts.push(PwmAct::Generate { alnum: ui.input(|i| i.modifiers.alt) });
                }
            });
            ui.end_row();
            if let Some(t) = &p.toggle {
                ui.label("");
                ui.vertical(|ui| {
                    ui.checkbox(&mut f.toggle, t.label.as_str());
                    if let Some(note) = t.note.as_deref().filter(|n| !n.is_empty()) {
                        ui.add(egui::Label::new(egui::RichText::new(note).small().weak()).wrap());
                    }
                });
                ui.end_row();
            }
        });
        ui.add_space(10.0);
        let ok_on = p.ok_on(&f.name);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ok = ui.add_enabled(ok_on, egui::Button::new(pwm::TITLE_OK).min_size(egui::vec2(53.0, 24.0))).clicked();
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.is_none());
            if (ok || enter) && ok_on {
                acts.push(PwmAct::PanelOk {
                    name: f.name.clone(),
                    user: f.user.clone(),
                    password: f.password.duplicate(),
                    toggle: p.toggle.as_ref().map(|_| f.toggle),
                });
            }
            let cancel = ui.add(egui::Button::new(pwm::TITLE_CANCEL).min_size(egui::vec2(76.0, 24.0))).clicked();
            if cancel || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                acts.push(PwmAct::PanelCancel);
            }
        });
    });
}

/// An alert (`NSAlert`): the heading, the text, Reveal's password, an adapter Settings sheet's
/// rows (a Show toggle beside each secret field, the note under it), and the buttons with the
/// first on the right. Return is the first button; Esc is a button titled Cancel.
fn alert_modal(ctx: &egui::Context, a: &Alert, settings: &mut Option<Vec<SettingValue>>, acts: &mut Vec<PwmAct>) {
    egui::Modal::new(egui::Id::new("pwm-alert")).show(ctx, |ui| {
        let width = if a.fields.is_empty() { ALERT_WIDTH } else { SETTINGS_WIDTH };
        ui.set_width(width);
        ui.strong(a.heading.as_str());
        if !a.text.is_empty() {
            ui.add_space(4.0);
            ui.add(egui::Label::new(a.text.as_str()).wrap());
        }
        if let Some(s) = &a.secret {
            ui.add_space(4.0);
            ui.add(egui::Label::new(egui::RichText::new(s.expose()).monospace()).selectable(false));
        }
        if let (false, Some(values)) = (a.fields.is_empty(), settings.as_mut()) {
            ui.add_space(12.0);
            for (n, ((field, _), value)) in a.fields.iter().zip(values.iter_mut()).enumerate() {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(egui::vec2(SETTINGS_LABEL, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| ui.label(field.label.as_str()));
                    let hint = field.placeholder.as_deref().unwrap_or("");
                    match value {
                        SettingValue::Plain(s) => {
                            ui.add(egui::TextEdit::singleline(s).hint_text(hint).return_key(None).desired_width(width - SETTINGS_LABEL - 8.0));
                        }
                        SettingValue::Secret(s, shown) => {
                            let id = egui::Id::new(("pwm-setting", n));
                            secret_field(ui, id, s, true, Some(hint), width - SETTINGS_LABEL - 8.0 - 60.0, *shown);
                            ui.toggle_value(shown, pwm::TITLE_SHOW);
                        }
                    }
                });
                if let Some(note) = field.note.as_deref() {
                    ui.horizontal(|ui| {
                        ui.add_space(SETTINGS_LABEL + 8.0);
                        ui.label(egui::RichText::new(note).small().weak());
                    });
                }
                ui.add_space(6.0);
            }
        }
        ui.add_space(12.0);
        let mut chosen = None;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (n, title) in a.buttons.iter().enumerate() {
                if ui.add(egui::Button::new(title.as_str()).min_size(egui::vec2(76.0, 24.0))).clicked() {
                    chosen = Some(n);
                }
            }
        });
        if ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.is_none()) && !a.buttons.is_empty() {
            chosen = chosen.or(Some(0));
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            chosen = chosen.or(a.buttons.iter().position(|b| b == pwm::TITLE_CANCEL));
        }
        match (chosen, settings.as_ref()) {
            (Some(0), Some(values)) if !a.fields.is_empty() => {
                let out = a
                    .fields
                    .iter()
                    .zip(values)
                    .map(|((f, _), v)| {
                        let value = match v {
                            SettingValue::Plain(s) => Secret::new(s.clone()),
                            SettingValue::Secret(s, _) => s.duplicate(),
                        };
                        (f.key.clone(), value)
                    })
                    .collect();
                acts.push(PwmAct::Settings(out));
            }
            (Some(n), _) => acts.push(PwmAct::AlertButton(n)),
            (None, _) => {}
        }
    });
}

/// The busy scrim (`WX:340-361`): "Working\u{2026}", a spinner and Cancel (Esc) over the window.
fn busy_modal(ctx: &egui::Context, acts: &mut Vec<PwmAct>) {
    egui::Modal::new(egui::Id::new("pwm-busy")).show(ctx, |ui| {
        ui.set_min_width(231.0 - 16.0);
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(16.0));
            ui.label(pwm::BUSY_WORKING);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(pwm::TITLE_CANCEL).min_size(egui::vec2(76.0, 24.0))).clicked() {
                    acts.push(PwmAct::CancelBusy);
                }
            });
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            acts.push(PwmAct::CancelBusy);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acts_never_print_secrets() {
        let act = PwmAct::PanelOk { name: "github".into(), user: "me".into(), password: Secret::new("hunter2".into()), toggle: Some(true) };
        let s = format!("{act:?}");
        assert!(s.contains("github") && s.contains("Secret(<redacted>)") && !s.contains("hunter2"));
        let act = PwmAct::Settings(vec![("master".into(), Secret::new("s3cret".into()))]);
        assert!(!format!("{act:?}").contains("s3cret"));
    }
}
