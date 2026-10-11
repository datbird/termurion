//! A small window that asks for one line of text (Set Named Mark's "Enter Mark Name").

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;

/// "2026-09-24 08:05:03": iTerm2's default mark name.
pub fn stamp(y: u16, mo: u16, d: u16, h: u16, mi: u16, s: u16) -> String {
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// The local time: year, month, day, hour, minute, second.
pub fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// The local time as a mark name.
pub fn local_stamp() -> String {
    let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    stamp(t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

pub enum PromptResult {
    Open,
    Cancel,
    Ok(String),
    /// The extra button (Save to File with Contents...).
    Extra(String),
}

pub struct PromptWindow {
    host: EguiHost,
    label: String,
    text: String,
    first: bool,
    /// Earlier entries to pick from (Run Coprocess's history).
    choices: Vec<String>,
    /// A checkbox: its label, its state, and the entries it starts checked for.
    check: Option<(String, bool, Vec<String>)>,
    /// OK with blank text beeps and keeps the window open.
    refuse_blank: bool,
    /// A button between OK and Cancel.
    extra: Option<String>,
    /// Gray text the empty field shows (NSTextField's placeholder).
    hint: String,
    /// A lone OK (NSAlert's default): no Cancel, and Escape does nothing.
    ok_only: bool,
    /// The default button's own label, when the caller names one ("Rename").
    ok_label: Option<String>,
    /// The text starts selected (a rename shows the old name selected).
    select_all: bool,
}

/// The default button's label: the caller's, else "OK".
fn ok_label_of(custom: Option<&str>) -> &str {
    custom.unwrap_or("OK")
}

impl PromptWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        title: &str,
        label: &str,
        initial: &str,
    ) -> Result<PromptWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, title, 420.0, 130.0)?;
        Ok(PromptWindow { host, label: label.into(), text: initial.into(), first: true, choices: Vec::new(), check: None, refuse_blank: false, extra: None, hint: String::new(), ok_only: false, ok_label: None, select_all: false })
    }

    /// Earlier entries, offered in a list under the text.
    pub fn with_choices(mut self, choices: Vec<String>) -> PromptWindow {
        self.choices = choices;
        self
    }

    /// A checkbox that starts checked when the text is one of `checked_for`, and follows
    /// the text as it changes.
    pub fn with_check(mut self, label: &str, checked_for: Vec<String>) -> PromptWindow {
        let on = checked_for.iter().any(|c| *c == self.text);
        self.check = Some((label.into(), on, checked_for));
        self
    }

    pub fn with_extra(mut self, label: &str) -> PromptWindow {
        self.extra = Some(label.into());
        self
    }

    /// The field's placeholder text.
    pub fn with_hint(mut self, hint: &str) -> PromptWindow {
        self.hint = hint.into();
        self
    }

    /// OK only, as an NSAlert with a text field has (`get_string`).
    pub fn ok_only(mut self) -> PromptWindow {
        self.ok_only = true;
        self
    }

    /// The default button's label (NSAlert's first button title).
    pub fn with_ok_label(mut self, label: &str) -> PromptWindow {
        self.ok_label = Some(label.into());
        self
    }

    /// The text starts selected, so typing replaces it.
    pub fn with_selected(mut self) -> PromptWindow {
        self.select_all = true;
        self
    }

    /// The default button's label now.
    pub fn ok_label(&self) -> &str {
        ok_label_of(self.ok_label.as_deref())
    }

    /// The field's text now.
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn refuse_blank(mut self) -> PromptWindow {
        self.refuse_blank = true;
        self
    }

    /// The checkbox's state.
    pub fn checked(&self) -> bool {
        self.check.as_ref().is_some_and(|c| c.1)
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

    /// Draws the window. Enter or OK gives the text; Escape or Cancel gives up.
    pub fn redraw(&mut self) -> PromptResult {
        let mut result = PromptResult::Open;
        let ok_label = ok_label_of(self.ok_label.as_deref()).to_string();
        let select_all = self.select_all;
        let (label, text, first, choices, check, refuse_blank, extra, hint, ok_only) = (&self.label, &mut self.text, &mut self.first, &self.choices, &mut self.check, self.refuse_blank, &self.extra, &self.hint, self.ok_only);
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label(label.as_str());
                let before = text.clone();
                let edit = egui::TextEdit::singleline(text).hint_text(hint.as_str()).desired_width(f32::INFINITY).show(ui);
                let r = edit.response;
                if *first {
                    r.request_focus();
                    if select_all {
                        let mut state = edit.state;
                        let end = egui::text::CCursor::new(text.chars().count());
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), end)));
                        state.store(ui.ctx(), r.id);
                    }
                    *first = false;
                }
                if !choices.is_empty() {
                    egui::ComboBox::from_id_salt("choices").selected_text("History").width(ui.available_width()).show_ui(ui, |ui| {
                        for c in choices {
                            if ui.selectable_label(false, c.as_str()).clicked() {
                                *text = c.clone();
                            }
                        }
                    });
                }
                if let Some((label, on, checked_for)) = check.as_mut() {
                    if *text != before {
                        *on = checked_for.iter().any(|c| c == text);
                    }
                    ui.checkbox(on, label.as_str());
                }
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let escape = !ok_only && ui.input(|i| i.key_pressed(egui::Key::Escape));
                ui.horizontal(|ui| {
                    if ui.button(ok_label.as_str()).clicked() || enter {
                        if refuse_blank && text.trim().is_empty() {
                            unsafe { windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(windows_sys::Win32::UI::WindowsAndMessaging::MB_OK) };
                        } else {
                            result = PromptResult::Ok(text.clone());
                        }
                    }
                    if let Some(x) = extra {
                        if ui.button(x.as_str()).clicked() {
                            result = PromptResult::Extra(text.clone());
                        }
                    }
                    if (!ok_only && ui.button("Cancel").clicked()) || escape {
                        result = PromptResult::Cancel;
                    }
                });
            });
        });
        result
    }
}

/// What a `ConfirmWindow` answered.
pub enum ConfirmResult {
    Open,
    Cancel,
    /// OK, and whether "Suppress this message permanently" was checked.
    Ok { suppress: bool },
    /// A button of an `actions` window, by its place in the labels, and the checkbox's state.
    Action { index: usize, suppress: bool },
}

/// The buttons Enter and Escape press in an `actions` window: Enter the first, Escape the one
/// labeled "Cancel", if any (`NSAlert`'s key equivalents, which `iTermWarning` keeps).
pub fn action_keys(labels: &[&str]) -> (usize, Option<usize>) {
    (0, labels.iter().position(|l| *l == "Cancel"))
}

/// The button closing an `actions` window answers: Escape's (Cancel), else the first (OK),
/// which only dismisses. iTerm2's alerts cannot be closed, so closing never acts.
pub fn close_index(labels: &[&str]) -> usize {
    action_keys(labels).1.unwrap_or(0)
}

/// The button Escape presses when the caller names its own close answer (`close`, a label):
/// that button, else `action_keys`' Cancel. A caller whose alert has no "Cancel" button, and
/// whose safe answer is another (a consent question's "Not Now"), names it here.
pub fn escape_index(labels: &[&str], close: Option<&str>) -> Option<usize> {
    close.and_then(|c| labels.iter().position(|l| *l == c)).or(action_keys(labels).1)
}

/// The button a closed window answers, given the caller's own close label: `escape_index`,
/// else the first. Only a caller that passes a close label changes it; `close_index` is the
/// answer for every other caller.
pub fn close_index_with(labels: &[&str], close: Option<&str>) -> usize {
    escape_index(labels, close).unwrap_or(0)
}

/// An `actions` window's buttons, left to right: the labels, then with `forget` "Permanently
/// Forget Saved Selection" (`iTermWarning.m:368-371`, the remembered mode's extra button).
pub fn button_labels(labels: &[&str], forget: bool) -> Vec<String> {
    let mut out: Vec<String> = labels.iter().map(|l| l.to_string()).collect();
    if forget {
        out.push(crate::warnings::FORGET.to_string());
    }
    out
}

/// The button Alt plus `letter` presses: the first whose shortcut is that letter, in either
/// case (`iTermWarning.m:296-326` can give two buttons one letter; AppKit presses the first).
pub fn shortcut_press(shortcuts: &[Option<(usize, char)>], letter: char) -> Option<usize> {
    let letter = letter.to_ascii_lowercase();
    shortcuts.iter().position(|s| s.is_some_and(|(_, c)| c == letter))
}

/// A button's label with the character at `at` (a char index) underlined, in the button font
/// (`alertsIndicateShortcuts`).
fn underlined(ui: &egui::Ui, label: &str, at: usize) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let color = ui.visuals().text_color();
    let plain = egui::TextFormat::simple(font.clone(), color);
    let marked = egui::TextFormat { underline: egui::Stroke::new(1.0, color), ..egui::TextFormat::simple(font, color) };
    let mut job = egui::text::LayoutJob::default();
    for (i, c) in label.chars().enumerate() {
        job.append(&c.to_string(), 0.0, if i == at { marked.clone() } else { plain.clone() });
    }
    job
}

/// Whether the person has seen to the end of a scrolled body: it fits, or the offset plus the view
/// reaches the content (a pixel of slack).
pub fn scrolled_to_end(offset: f32, view: f32, content: f32) -> bool {
    content <= view + 1.0 || offset + view + 1.0 >= content
}

/// Whether button `index` may be pressed: a gated button (the entry question's Enter) waits until the
/// body has been read; every other button is always open.
pub fn gate_open(gated: Option<usize>, read: bool, index: usize) -> bool {
    gated != Some(index) || read
}

/// A question window's height in logical points for `text`: its lines (wrapped near 60 characters),
/// at least the old 130 and at most 560 so a small screen still holds the buttons.
pub fn window_height(text: &str) -> f64 {
    let lines: usize = text.lines().map(|l| l.chars().count().div_ceil(60).max(1)).sum();
    (110.0 + lines as f64 * 16.0).clamp(130.0, 560.0)
}

/// How long a guarded window ignores input after it appears and after it regains focus, so a key
/// or click already in flight for the page behind it cannot answer it.
pub const GUARD_DELAY: std::time::Duration = std::time::Duration::from_millis(1000);

/// The arming delay of a window that a web page can raise (13x6 B1). Pure: the caller passes `now`.
#[derive(Clone, Copy, Debug)]
pub struct InputGuard {
    since: std::time::Instant,
}

impl InputGuard {
    pub fn new(now: std::time::Instant) -> InputGuard {
        InputGuard { since: now }
    }

    /// The window appeared or took the focus again: the delay starts over.
    pub fn refocus(&mut self, now: std::time::Instant) {
        self.since = now;
    }

    /// Whether input counts yet.
    pub fn armed(&self, now: std::time::Instant) -> bool {
        now.saturating_duration_since(self.since) >= GUARD_DELAY
    }

    /// Whether `event` is input that could answer a window: a key, a mouse button, a touch (a touch
    /// screen or pen), or text from an input method.
    pub fn is_answer_input(event: &WindowEvent) -> bool {
        matches!(event, WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } | WindowEvent::Touch(_) | WindowEvent::Ime(_))
    }

    /// Whether the guard drops `event` at `now`: input that could answer, before it is armed. Every
    /// guarded window asks this one rule (the permission prompt, the mark windows, the autofill
    /// question).
    pub fn drops(&self, event: &WindowEvent, now: std::time::Instant) -> bool {
        Self::is_answer_input(event) && !self.armed(now)
    }

    /// A window event: regaining the focus starts the delay over.
    pub fn window_event(&mut self, event: &WindowEvent, now: std::time::Instant) {
        if matches!(event, WindowEvent::Focused(true)) {
            self.refocus(now);
        }
    }
}

/// An OK and Cancel question with iTerm2's permanently silenceable checkbox
/// (`iTermWarning.m:400-406,517-520`: "Suppress this message permanently", remembered only on
/// OK). Enter is OK and Escape Cancel.
pub struct ConfirmWindow {
    host: EguiHost,
    text: String,
    suppress: bool,
    /// The "Suppress this message permanently" checkbox shows, and its label.
    checkbox: bool,
    checkbox_label: &'static str,
    /// The button that answers `Ok`, and the one that answers `Cancel` (none for a warning).
    ok: String,
    cancel: Option<String>,
    /// Enter answers the cancel button: an iTerm2 warning whose first action is Cancel makes
    /// it the default button.
    enter_cancels: bool,
    /// An `actions` window's buttons, left to right.
    actions: Vec<String>,
    /// Each action's Alt shortcut (its letter's char index and the letter), whether the letter
    /// is drawn underlined, and whether "Permanently Forget Saved Selection" follows the
    /// actions (it answers `Action { index: actions.len(), .. }`).
    shortcuts: Vec<Option<(usize, char)>>,
    underline: bool,
    forget: bool,
    /// The label of the button a close (X, Alt+F4) and Escape answer, when the caller names one
    /// (`escape_index`); None keeps `action_keys`' Cancel.
    close: Option<String>,
    /// The action that waits until the body has been scrolled to its end, and whether it has been.
    gated: Option<usize>,
    read: bool,
    /// A guarded window (raised by a web page): input is ignored for `GUARD_DELAY` after it appears
    /// and after it regains focus, Enter and Space press nothing, no button has the focus, and only
    /// a pointer click or an Alt access key answers.
    guard: Option<InputGuard>,
}

impl ConfirmWindow {
    /// The checkbox's label (`iTermWarning.m:400-406`).
    pub const SUPPRESS: &'static str = "Suppress this message permanently";

    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        heading: &str,
        text: &str,
    ) -> Result<ConfirmWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, heading, 480.0, window_height(text))?;
        Ok(ConfirmWindow { host, text: text.into(), suppress: false, checkbox: true, checkbox_label: Self::SUPPRESS, ok: "OK".into(), cancel: Some("Cancel".into()), enter_cancels: false, actions: Vec::new(), shortcuts: Vec::new(), underline: false, forget: false, close: None, gated: None, read: false, guard: None })
    }

    /// A warning that cannot be silenced: the text and an OK button (Enter; Escape closes
    /// it too).
    #[allow(clippy::too_many_arguments)]
    pub fn warning(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        heading: &str,
        text: &str,
    ) -> Result<ConfirmWindow, String> {
        let mut w = ConfirmWindow::open(event_loop, instance, adapter, device, queue, heading, text)?;
        w.checkbox = false;
        w.cancel = None;
        Ok(w)
    }

    /// A warning that cannot be silenced with two actions in iTerm2's order: `first` is the
    /// default button (Enter) and answers `Cancel`, as does Escape; `second` answers `Ok`
    /// ("Not all sessions at password prompt", the password prompt check).
    #[allow(clippy::too_many_arguments)]
    pub fn choice(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        heading: &str,
        text: &str,
        first: &str,
        second: &str,
    ) -> Result<ConfirmWindow, String> {
        let mut w = ConfirmWindow::open(event_loop, instance, adapter, device, queue, heading, text)?;
        w.checkbox = false;
        w.ok = second.into();
        w.cancel = Some(first.into());
        w.enter_cancels = true;
        Ok(w)
    }

    /// An `iTermWarning` with any number of actions: the text, one button per label, left to
    /// right in the labels' order (iTerm2's), and the checkbox only when `checkbox` names its
    /// label. A button answers `Action` with its index; Enter and Escape press the buttons
    /// `action_keys` names, and Alt plus a letter the button `shortcut_press` names, its letter
    /// underlined with `underline`. With `forget`, "Permanently Forget Saved Selection" follows
    /// the labels and answers the index `labels.len()`.
    #[allow(clippy::too_many_arguments)]
    pub fn actions(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        heading: &str,
        text: &str,
        labels: &[&str],
        checkbox: Option<&'static str>,
        shortcuts: &[Option<(usize, char)>],
        underline: bool,
        forget: bool,
    ) -> Result<ConfirmWindow, String> {
        let mut w = ConfirmWindow::open(event_loop, instance, adapter, device, queue, heading, text)?;
        w.checkbox = checkbox.is_some();
        w.checkbox_label = checkbox.unwrap_or(Self::SUPPRESS);
        w.cancel = None;
        w.actions = labels.iter().map(|l| l.to_string()).collect();
        w.shortcuts = shortcuts.to_vec();
        w.underline = underline;
        w.forget = forget;
        Ok(w)
    }

    /// The answer when the window is closed: Cancel, or for an `actions` window `close_index`'s
    /// button, with the checkbox not remembered.
    pub fn close_answer(&self) -> ConfirmResult {
        if self.actions.is_empty() {
            return ConfirmResult::Cancel;
        }
        let labels: Vec<&str> = self.actions.iter().map(String::as_str).collect();
        ConfirmResult::Action { index: close_index_with(&labels, self.close.as_deref()), suppress: false }
    }

    /// Names the button a close and Escape answer (`escape_index`), for an `actions` window
    /// whose refusing button is not labeled "Cancel".
    pub fn close_with(&mut self, label: Option<&str>) {
        self.close = label.map(str::to_string);
    }

    /// Action `index` (and its access key) is offered only after the body has been shown whole: it
    /// fits the window, or the person scrolled to its end.
    pub fn require_read(&mut self, index: usize) {
        self.gated = Some(index);
    }

    /// Guards the window against a stray key or click (see `guard`). Call right after opening.
    pub fn guard_input(&mut self) {
        self.guard = Some(InputGuard::new(std::time::Instant::now()));
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    /// A guarded window drops a key, click, touch or input-method event before its guard is armed.
    pub fn event(&mut self, event: &WindowEvent) -> bool {
        let now = std::time::Instant::now();
        if let Some(g) = self.guard.as_mut() {
            g.window_event(event, now);
            if g.drops(event, now) {
                return false;
            }
        }
        self.host.event(event)
    }

    /// Whether the window's guard drops `event` now (false for an unguarded window): for a caller
    /// that answers a key itself.
    pub fn input_guarded(&self, event: &WindowEvent) -> bool {
        self.guard.is_some_and(|g| g.drops(event, std::time::Instant::now()))
    }

    pub fn redraw(&mut self) -> ConfirmResult {
        let mut result = ConfirmResult::Open;
        let (text, suppress, checkbox, checkbox_label, ok, cancel, enter_cancels, actions) = (&self.text, &mut self.suppress, self.checkbox, self.checkbox_label, &self.ok, &self.cancel, self.enter_cancels, &self.actions);
        let (shortcuts, underline, forget, close) = (&self.shortcuts, self.underline, self.forget, &self.close);
        let (gated, read) = (self.gated, &mut self.read);
        let guard = self.guard;
        let guard_armed = guard.is_none_or(|g| g.armed(std::time::Instant::now()));
        self.host.paint(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                // The body scrolls inside the window and the buttons stay below it, so no text or
                // button is ever cut off.
                let reserve = 48.0 + if checkbox { 28.0 } else { 0.0 } + if gated.is_some() { 20.0 } else { 0.0 };
                let out = egui::ScrollArea::vertical().max_height((ui.available_height() - reserve).max(60.0)).auto_shrink([false, true]).show(ui, |ui| {
                    ui.label(text.as_str());
                });
                if gated.is_some() && scrolled_to_end(out.state.offset.y, out.inner_rect.height(), out.content_size.y) {
                    *read = true;
                }
                if gated.is_some() && !*read {
                    ui.label("Scroll to the end of the text to enable the button.");
                }
                ui.add_space(6.0);
                if checkbox {
                    ui.checkbox(suppress, checkbox_label);
                    ui.add_space(6.0);
                }
                let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
                // A guarded window has no default button and no focused widget (Enter and Space
                // would press it), and says why its buttons wait.
                let (enter, escape) = if guard.is_some() {
                    let ctx = ui.ctx().clone();
                    ctx.memory_mut(|m| {
                        if let Some(f) = m.focused() {
                            m.surrender_focus(f);
                        }
                    });
                    if !guard_armed {
                        ui.label("The buttons turn on in a moment.");
                    }
                    (false, false)
                } else {
                    (enter, escape)
                };
                ui.horizontal(|ui| {
                    if !actions.is_empty() {
                        let labels: Vec<&str> = actions.iter().map(String::as_str).collect();
                        let enter_index = action_keys(&labels).0;
                        let escape_index = escape_index(&labels, close.as_deref());
                        for (index, label) in button_labels(&labels, forget).iter().enumerate() {
                            let open = gate_open(gated, *read, index) && guard_armed;
                            let response = match shortcuts.get(index).copied().flatten() {
                                Some((at, _)) if underline => ui.add_enabled(open, egui::Button::new(underlined(ui, label, at))),
                                _ => ui.add_enabled(open, egui::Button::new(label.as_str())),
                            };
                            // A guarded window counts only a real pointer release over the button.
                            let clicked = response.clicked() && (guard.is_none() || (response.hovered() && ui.input(|i| i.pointer.primary_released())));
                            if clicked {
                                result = ConfirmResult::Action { index, suppress: *suppress };
                            }
                        }
                        // Alt plus a shortcut letter (iTerm2's Command plus the letter).
                        let pressed = ui.input(|i| {
                            i.events.iter().find_map(|e| match e {
                                egui::Event::Key { key, pressed: true, modifiers, .. } if modifiers.alt && !modifiers.ctrl => {
                                    let name = key.name();
                                    let mut chars = name.chars();
                                    match (chars.next(), chars.next()) {
                                        (Some(c), None) if c.is_ascii_alphabetic() => shortcut_press(shortcuts, c),
                                        _ => None,
                                    }
                                }
                                _ => None,
                            })
                        });
                        let pressed = pressed.filter(|i| gate_open(gated, *read, *i) && guard_armed);
                        if let Some(index) = pressed {
                            result = ConfirmResult::Action { index, suppress: *suppress };
                        } else if enter && gate_open(gated, *read, enter_index) {
                            result = ConfirmResult::Action { index: enter_index, suppress: *suppress };
                        } else if let (true, Some(index)) = (escape, escape_index) {
                            result = ConfirmResult::Action { index, suppress: *suppress };
                        }
                        return;
                    }
                    if enter_cancels {
                        if let Some(c) = cancel {
                            if ui.button(c.as_str()).clicked() || enter {
                                result = ConfirmResult::Cancel;
                            }
                        }
                        if ui.button(ok.as_str()).clicked() {
                            result = ConfirmResult::Ok { suppress: *suppress };
                        }
                        if escape {
                            result = ConfirmResult::Cancel;
                        }
                        return;
                    }
                    if ui.button(ok.as_str()).clicked() || enter {
                        result = ConfirmResult::Ok { suppress: *suppress };
                    }
                    if cancel.as_ref().is_some_and(|c| ui.button(c.as_str()).clicked()) || escape {
                        result = ConfirmResult::Cancel;
                    }
                });
            });
        });
        if guard.is_some() && !guard_armed {
            self.host.request_redraw();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guarded_window_waits_after_it_appears_and_after_it_regains_focus() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let mut g = InputGuard::new(t0);
        assert!(!g.armed(t0), "a key already in flight cannot answer it");
        assert!(!g.armed(t0 + GUARD_DELAY - Duration::from_millis(1)));
        assert!(g.armed(t0 + GUARD_DELAY));
        g.refocus(t0 + Duration::from_secs(5));
        assert!(!g.armed(t0 + Duration::from_secs(5)), "regaining focus starts it over");
        assert!(g.armed(t0 + Duration::from_secs(5) + GUARD_DELAY));
        let src = include_str!("prompt_ui.rs");
        let f = &src[src.find("pub fn redraw(&mut self) -> ConfirmResult").unwrap()..src.find("mod tests {").unwrap()];
        assert!(f.contains("guard_armed"), "buttons and access keys ask the guard");
        assert!(f.contains("(false, false)"), "a guarded window ignores Enter and Escape");
        assert!(f.contains("surrender_focus"), "no widget holds the focus for Enter or Space");
        assert!(f.contains("primary_released"), "only a pointer release counts");
    }

    /// The one guard covers a touch (a touch screen or pen) as it covers keys and clicks, and the
    /// window drops guarded input before egui sees it.
    #[test]
    fn the_guard_drops_touch_keys_clicks_and_ime_until_armed() {
        use std::time::{Duration, Instant};
        use winit::event::{Ime, Touch, TouchPhase};
        let t0 = Instant::now();
        let mut g = InputGuard::new(t0);
        let touch = WindowEvent::Touch(Touch { device_id: winit::event::DeviceId::dummy(), phase: TouchPhase::Started, location: winit::dpi::PhysicalPosition::new(1.0, 1.0), force: None, id: 1 });
        let mouse = WindowEvent::MouseInput { device_id: winit::event::DeviceId::dummy(), state: winit::event::ElementState::Pressed, button: winit::event::MouseButton::Left };
        for e in [&touch, &mouse, &WindowEvent::Ime(Ime::Enabled)] {
            assert!(g.drops(e, t0), "{e:?} is dropped while the guard waits");
            assert!(!g.drops(e, t0 + GUARD_DELAY), "{e:?} counts once armed");
        }
        assert!(!g.drops(&WindowEvent::Focused(true), t0), "focus and paint are not answers");
        assert!(!g.drops(&WindowEvent::RedrawRequested, t0));
        g.window_event(&WindowEvent::Focused(true), t0 + Duration::from_secs(4));
        assert!(g.drops(&touch, t0 + Duration::from_secs(4) + Duration::from_millis(999)), "refocus restarts it for a touch too");
        let src = include_str!("prompt_ui.rs");
        let ev = &src[src.find("pub fn event(&mut self, event: &WindowEvent) -> bool {\n        let now").unwrap()..];
        assert!(ev[..ev.find("self.host.event(event)").unwrap()].contains("if g.drops(event, now) {"));
    }

    /// A long body scrolls, the buttons never leave the window, and a gated button (the entry
    /// question's Enter) is offered only once the person has seen to the end of the text.
    #[test]
    fn a_long_question_scrolls_and_gates_its_button() {
        assert!(scrolled_to_end(0.0, 200.0, 150.0), "text that fits has been seen");
        assert!(!scrolled_to_end(0.0, 200.0, 900.0));
        assert!(!scrolled_to_end(600.0, 200.0, 900.0));
        assert!(scrolled_to_end(700.0, 200.0, 900.0));
        assert!(scrolled_to_end(699.5, 200.0, 900.0), "a pixel of slack");
        assert!(gate_open(None, false, 1), "no gate: open");
        assert!(gate_open(Some(1), true, 1));
        assert!(!gate_open(Some(1), false, 1), "the gated button waits");
        assert!(gate_open(Some(1), false, 0), "the other buttons never wait");
        assert!(window_height("short") <= window_height(&"a long line of command text ".repeat(100)));
        assert_eq!(window_height("short"), 130.0);
        assert_eq!(window_height(&"x\n".repeat(5000)), 560.0, "capped at what a small screen holds");
        let src = include_str!("prompt_ui.rs");
        let f = &src[src.find("pub fn redraw(&mut self) -> ConfirmResult").unwrap()..];
        assert!(f.contains("egui::ScrollArea::vertical()"), "the body scrolls");
        assert!(f.contains("gate_open(gated"), "the gated button and its access key ask the gate");
        assert!(f.contains("scrolled_to_end("));
    }

    #[test]
    fn ok_label_follows_the_caller() {
        assert_eq!(ok_label_of(Some("Rename")), "Rename");
        assert_eq!(ok_label_of(None), "OK");
    }

    #[test]
    fn action_keys_follow_iterm2() {
        assert_eq!(action_keys(&["OK", "Cancel", "More Info", "Always"]), (0, Some(1)));
        assert_eq!(action_keys(&["OK", "Reveal Preference"]), (0, None));
        assert_eq!(action_keys(&["OK"]), (0, None));
    }

    #[test]
    fn closing_is_harmless() {
        // Escape's Cancel, else the first (OK) button, which never acts.
        assert_eq!(close_index(&["OK", "Cancel", "More Info", "Always"]), 1);
        assert_eq!(close_index(&["OK", "Reveal Preference"]), 0);
        assert_eq!(close_index(&["OK", "Show in Explorer"]), 0);
        assert_eq!(close_index(&["OK"]), 0);
    }

    #[test]
    fn close_answer_is_named_by_the_caller() {
        // No close label: every existing caller keeps its answer (Cancel, else the first).
        for labels in [&["OK", "Cancel", "More Info", "Always"][..], &["OK", "Reveal Preference"], &["OK"], &["Allow", "Cancel"], &["Close", "Cancel"]] {
            assert_eq!(close_index_with(labels, None), close_index(labels), "{labels:?}");
            assert_eq!(escape_index(labels, None), action_keys(labels).1, "{labels:?}");
        }
        // A close label picks that button, whatever the labels are called.
        let consent = ["Turn On", "Not Now"];
        assert_eq!(close_index(&consent), 0, "without a close label the first button answers, which grants");
        assert_eq!(close_index_with(&consent, Some("Not Now")), 1);
        assert_eq!(escape_index(&consent, Some("Not Now")), Some(1));
        // The label wins over a button named Cancel, and a label not present falls back.
        assert_eq!(close_index_with(&["Yes", "Cancel", "No"], Some("No")), 2);
        assert_eq!(close_index_with(&consent, Some("Missing")), close_index(&consent));
    }

    #[test]
    fn confirm_buttons_follow_iterm2() {
        assert_eq!(button_labels(&["OK", "Cancel"], true), ["OK", "Cancel", "Permanently Forget Saved Selection"]);
        assert_eq!(button_labels(&["OK", "Cancel"], false), ["OK", "Cancel"]);
        assert_eq!(shortcut_press(&[None, Some((0, 'c')), None], 'c'), Some(1));
        assert_eq!(shortcut_press(&[None, Some((0, 'c')), None], 'C'), Some(1), "case-insensitive");
        assert_eq!(shortcut_press(&[None, Some((0, 'c')), None], 'x'), None);
        assert_eq!(shortcut_press(&[None, Some((0, 'c')), Some((2, 'c'))], 'c'), Some(1), "the first of two with one letter");
    }

    #[test]
    fn a_named_mark_defaults_to_the_time() {
        assert_eq!(stamp(2026, 9, 24, 8, 5, 3), "2026-09-24 08:05:03");
    }
}
