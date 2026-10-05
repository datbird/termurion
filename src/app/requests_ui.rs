//! What output asks of the window: focus, attention, frames, the cursor guide, the tab color.

use std::time::{Duration, Instant};

use winit::dpi::{PhysicalPosition, PhysicalSize};

use super::*;
use crate::chrome::ResizePermission;
use crate::osc1337::{self, Attention};
use crate::screen::Request;

/// How long the pointer rests on a tab before its tooltip shows.
pub(super) const TIP_DELAY: Duration = Duration::from_millis(500);

/// The Tab Color picker's no-color entry (iTerm2's "Default" swatch, index 0 of
/// `ColorsMenuItemView`).
pub(super) const NO_COLOR: &str = "Default";

/// The Tab Color picker's entry that opens Windows' color dialog.
pub(super) const CHOOSE_COLOR: &str = "Choose Color\u{2026}";

/// The session values a picked tab color writes (`PTYSession setTabColorByUI:`): "Use Tab
/// Color" and "Tab Color", or "Use Tab Color" off for none. With `separate` (the profile's
/// "Use Separate Colors for Light and Dark Mode") the Dark and Light variants are written
/// too, so the pick shows in both modes (`allTabColorKeysForBaseKey:`).
pub(super) fn tab_color_values(color: Option<crate::palette::Rgb>, separate: bool) -> Vec<(String, Option<toml::Value>)> {
    let keys = |base: &str| -> Vec<String> {
        if separate { vec![base.to_string(), format!("{base} (Dark)"), format!("{base} (Light)")] } else { vec![base.to_string()] }
    };
    let mut values: Vec<(String, Option<toml::Value>)> = keys("Use Tab Color").into_iter().map(|k| (k, Some(toml::Value::Boolean(color.is_some())))).collect();
    if let Some(c) = color {
        let v = crate::settings_ui::value_of_rgb([c.r, c.g, c.b]);
        values.extend(keys("Tab Color").into_iter().map(|k| (k, Some(v.clone()))));
    }
    values
}

/// The tooltip of a tab (`PT:8701-8723`): "Name", "Profile" and "Command" lines, the name
/// the tab's label (`tab_label`) without HTML when `html`, the profile the saved original's
/// name for a session copy (`original_name`) else the profile's own, and "None" for no
/// command. None when `disabled`.
pub(super) fn tip_text(disabled: bool, label: &str, html: bool, profile_name: &str, original_name: Option<&str>, command: &str) -> Option<String> {
    if disabled {
        return None;
    }
    let name = crate::tab_layout::plain_title(label, html);
    Some(crate::tab_layout::tooltip(&name, original_name.unwrap_or(profile_name), Some(command).filter(|c| !c.is_empty())))
}

/// A tab's label as iTerm2 keeps it (`PTYTab labelForActiveSession`, `TAB:735-751`): the
/// title, a line break and the subtitle, the break kept when the subtitle is empty.
pub(super) fn tab_label(title: &str, subtitle: &str) -> String {
    format!("{title}\n{subtitle}")
}

/// A session profile's own name, and its saved original's name when it is a session copy
/// whose original still exists (iTerm2 looks the Guid up in the saved profiles, then the
/// copy's "Original Guid").
fn tip_profile_names(store: &crate::store::Store, guid: &str) -> (String, Option<String>) {
    let own = store.get(guid).map(|p| p.name()).unwrap_or_default();
    let shared = store.shared_guid(guid);
    let original = (shared != guid).then(|| store.get(&shared)).flatten().map(|p| p.name());
    (own, original)
}

/// The sessions that take a title a program set in `from` (`tabDidSetWindowTitle:to:`,
/// `PT:15578-15584`): every other pane of its window when SeparateWindowTitlePerTab
/// (`separate`) is off, else none.
pub(super) fn share_targets(panes_in_window: &[PaneId], from: PaneId, separate: bool) -> Vec<PaneId> {
    if !crate::wintitle::shares_title(separate) {
        return Vec::new();
    }
    panes_in_window.iter().copied().filter(|&p| p != from).collect()
}

/// How long RequestAttention's pane flash and fireworks last.
pub(super) const FLASH: Duration = Duration::from_millis(300);
pub(super) const FIREWORKS: Duration = Duration::from_millis(1200);

impl App {
    pub(super) fn window_request(&mut self, pane: PaneId, r: Request) {
        match r {
            Request::Trigger(t) => self.trigger_request(pane, t),
            Request::Bell => self.ring_bell(pane),
            Request::Notify { title, message } => self.terminal_alert(pane, title, message),
            Request::WindowTitle(title) => self.share_window_title(pane, title),
            Request::SetBadge(t) => self.set_badge(pane, Some(t)),
            Request::BadgeRefused => self.set_badge(pane, None),
            Request::HostChanged(left_on) => self.host_changed(pane, left_on),
            Request::HostMoved { previous, current, mode, title } => self.host_moved(pane, previous, current, mode, title),
            Request::ShellIntegrationReported { version, shell } => self.shell_integration_reported(pane, version, shell),
            // Progress Bar Changed event triggers arrive in task 5 of milestone 6b.
            Request::Progress(_) => {}
            Request::CommandEnded { code, mark } => {
                self.stream_command(pane, code, mark);
                self.codecierge_command_exited(pane, mark, code);
            }
            Request::CursorGuide(on) => {
                self.show_guide.insert(pane, on);
                self.redraw();
            }
            Request::Attention(a) => self.attention(pane, a),
            Request::Disinter => self.disinter(pane),
            Request::StealFocus => {
                let allow = files_ui::ask("A control sequence attempted to activate a session. Allow it?");
                self.steal_asked = Some(Instant::now());
                if allow {
                    self.reveal(pane);
                }
            }
            Request::ReportCellSize => {
                let scale = self.scale() as f64;
                let (w, h) = self.pane_cell(pane);
                let reply = format!(
                    "\x1b]1337;ReportCellSize={};{};{}\x1b\\",
                    osc1337::number(h as f64 / scale),
                    osc1337::number(w as f64 / scale),
                    osc1337::number(scale)
                );
                self.reply(pane, reply.as_bytes());
            }
            Request::ReportWindowFrame => {
                let Some((x, y, w, h)) = self.window_points() else { return };
                let n = osc1337::number;
                let reply = format!("\x1b]1337;WindowFrame={};{};{};{}\x1b\\", n(x), n(y), n(w), n(h));
                self.reply(pane, reply.as_bytes());
            }
            Request::ReportScreenFrames => {
                let scale = self.scale() as f64;
                let primary = primary_height();
                let n = osc1337::number;
                let frames: Vec<String> = work_areas()
                    .into_iter()
                    .map(|(x, y, w, h)| {
                        let (x, y, w, h) = osc1337::to_points(x, y, w, h, primary, scale);
                        format!("{},{},{},{}", n(x), n(y), n(w), n(h))
                    })
                    .collect();
                let reply = format!("\x1b]1337;ScreenFrames={}\x1b\\", frames.join(";"));
                self.reply(pane, reply.as_bytes());
            }
            Request::SetWindowFrame(x, y, w, h) => self.set_window_frame(pane, x, y, w, h),
            Request::TabColor(c) => {
                match c {
                    Some(c) => self.tab_colors.insert(pane, c),
                    None => self.tab_colors.remove(&pane),
                };
                self.redraw();
            }
            Request::SetProfile(name) => self.set_profile_request(pane, name),
            Request::SetProfileProperty(pairs) => self.set_property_request(pane, pairs),
            Request::SetBackgroundImageFile(value) => self.set_background_request(pane, value),
            Request::OpenUrl(value) => self.open_url_request(pane, value),
            Request::ReportVariable(value) => self.report_variable_request(pane, value),
            Request::RequestUpload(value) => self.upload_request(pane, value),
            Request::SshWillBegin | Request::SshSendConductor(_) | Request::SshEnd(_) => self.ssh_request(pane, r),
            Request::ClipboardWrite(_) | Request::ClipboardRead(_) => {}
        }
    }

    fn reply(&mut self, pane: PaneId, bytes: &[u8]) {
        if let Some(p) = self.panes.get_mut(&pane) {
            p.reply(bytes);
        }
    }

    fn attention(&mut self, pane: PaneId, a: Attention) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{FLASHW_STOP, FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx};
        let flash = |flags: u32, count: u32, hwnd: isize| {
            let info = FLASHWINFO { cbSize: std::mem::size_of::<FLASHWINFO>() as u32, hwnd: hwnd as _, dwFlags: flags, uCount: count, dwTimeout: 0 };
            unsafe { FlashWindowEx(&info) };
        };
        match a {
            Attention::Once | Attention::Start | Attention::Stop => {
                let Some(hwnd) = self.hwnd() else { return };
                match a {
                    Attention::Once => flash(FLASHW_TRAY, 1, hwnd),
                    Attention::Start => flash(FLASHW_TRAY | FLASHW_TIMERNOFG, 0, hwnd),
                    _ => flash(FLASHW_STOP, 0, hwnd),
                }
            }
            Attention::Flash => {
                self.flashes.insert(pane, Instant::now());
                self.redraw();
            }
            Attention::Fireworks => {
                let at = self.panes.get(&pane).map(|p| {
                    let g = p.own().term().grid();
                    (g.cursor.point.column.0, (g.cursor.point.line.0 + g.display_offset() as i32).max(0) as usize)
                });
                if let Some(at) = at {
                    self.fireworks.insert(pane, (Instant::now(), at));
                    self.redraw();
                }
            }
        }
    }

    /// StealFocus: the pane's tab comes forward, the pane takes focus, and the window comes
    /// to the front (Windows may only flash its taskbar button instead).
    fn reveal(&mut self, pane: PaneId) {
        if let Some(t) = self.tabs.iter().position(|t| t.layout.panes().contains(&pane)) {
            self.select_tab(t);
            self.tabs[t].focused = pane;
        }
        self.bring_forward();
        if let Some(hwnd) = self.hwnd() {
            use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
            if unsafe { SetForegroundWindow(hwnd as _) } == 0 {
                self.attention(pane, Attention::Once);
            }
        }
        self.relayout();
    }

    /// The window's frame in AppKit points (y up from the bottom of the primary monitor).
    fn window_points(&self) -> Option<(f64, f64, f64, f64)> {
        let g = self.gpu.as_ref()?;
        let pos = g.window.outer_position().ok()?;
        let size = g.window.outer_size();
        Some(osc1337::to_points(pos.x as f64, pos.y as f64, size.width as f64, size.height as f64, primary_height(), g.window.scale_factor()))
    }

    /// A program in `pane` set or reset its title: with SeparateWindowTitlePerTab off every
    /// other session of the window (the current one, entered for the request) takes it.
    /// `set_title` asks nothing back, so two panes never answer each other.
    fn share_window_title(&mut self, pane: PaneId, title: Option<String>) {
        let window: Vec<PaneId> = self.tabs.iter().flat_map(|t| t.layout.panes()).collect();
        let targets = share_targets(&window, pane, self.globals.bool("SeparateWindowTitlePerTab"));
        if targets.is_empty() {
            return;
        }
        for id in targets {
            if let Some(p) = self.panes.get_mut(&id) {
                p.own_mut().set_title(title.clone());
            }
        }
        self.redraw();
    }

    fn set_window_frame(&mut self, pane: PaneId, x: f64, y: f64, w: f64, h: f64) {
        // Only a pane the permission allows: the focused pane of the focused window when
        // the profile refuses unfocused sessions (`PS:15717-15726`).
        let focused = self.focused && self.focused_id() == Some(pane);
        let permission = self.profile_of(pane).map_or(ResizePermission::Allowed, |p| {
            crate::chrome::resize_permission(p.bool("Disable Window Resizing"), p.bool("Disable Window Resizing by Unfocused Sessions"), focused)
        });
        let Some(g) = &self.gpu else { return };
        if permission != ResizePermission::Allowed || g.window.fullscreen().is_some() {
            return;
        }
        let (x, y, w, h) = osc1337::from_points(x, y, w, h, primary_height(), g.window.scale_factor());
        // The frame includes the title bar and borders; the inner size leaves them out.
        let (outer, inner) = (g.window.outer_size(), g.window.inner_size());
        let (dw, dh) = (outer.width.saturating_sub(inner.width) as f64, outer.height.saturating_sub(inner.height) as f64);
        g.window.set_outer_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
        let _ = super::caption_ui::request_inner_size(&g.window, PhysicalSize::new((w - dw).max(1.0).round() as u32, (h - dh).max(1.0).round() as u32));
    }

    /// A tab's title, subtitle (the status joined in), colors, icon, spinner, number, pin,
    /// group and its focused session's icon picture for the tab bar.
    pub(super) fn tab_look(&self, t: &Tab) -> scene::TabLook {
        use crate::tabstatus::{self, xterm_color};
        // Edit Tab Title's (or the profile's) title evaluated; an empty one shows the session's.
        let title = self.tmux_tab_label(t.focused).or_else(|| self.tab_title_text(t).filter(|t| !t.is_empty())).unwrap_or_else(|| self.session_title(t.focused));
        let panes = t.layout.panes();
        // The focused pane's status when it has one, else the best by the user's priority
        // patterns, the first on a tie (`PTYTab.m:7747-7801`).
        // A pane has a status when it has text or an indicator (`hasActiveStatus`,
        // `iTermSessionTabStatus.swift:178`), not a color or detail alone.
        let status_of = |id: &PaneId| self.panes.get(id).map(|p| &p.own().tab_status).filter(|s| s.is_active());
        let st: Option<&tabstatus::Status> = status_of(&t.focused).or_else(|| {
            let patterns = self.status_patterns();
            panes.iter().filter_map(status_of).enumerate().min_by_key(|(i, s)| (crate::status_tool::priority(s.status.as_deref(), &patterns), *i)).map(|(_, s)| s)
        });
        // The status text in its display form (`localizedStatusForDisplay`) joins the subtitle
        // line (`TAB:740-751`); it no longer prefixes the title.
        let status = st.and_then(|s| s.status.as_deref().filter(|t| !t.is_empty())).map(crate::status_tool::display_status);
        let subtitle = crate::tab_layout::subtitle_text(&self.subtitle_text(t.focused), status.as_deref(), self.globals.bool(crate::status_tool::SUBTITLE_KEY));
        let dot = st.and_then(|s| s.indicator.as_deref()).and_then(xterm_color);
        // A program's (or tmux's saved) tab color, else the profile's.
        let color = self.pane_tab_color(t.focused);
        let dead = self.panes.get(&t.focused).is_some_and(|p| p.exited.is_some());
        let bell = panes.iter().any(|id| self.bell_icons.contains(id));
        let (new_output, idle) = self.activity_icons(&panes);
        let icon = scene::tab_icon(dot.is_some(), dead, bell, new_output, idle);
        let index = self.tabs.iter().position(|x| x.id == t.id);
        let foreground = index == Some(self.active);
        let processing = activity_ui::tab_processing(self.any_processing(&panes), foreground, self.globals.bool("HideActivityIndicator"));
        scene::TabLook { title, subtitle, dot, color, icon, processing, number: index.map_or(0, |i| i + 1), close_pressed: false, dragged: false, pinned: t.pinned, group: t.group.as_ref().map(|g| (g.name.clone(), g.color)), hidden: t.group.as_ref().is_some_and(|g| g.collapsed), icon_image: self.tab_icon_for(t.focused) }
    }

    /// Colors every pane of the tab (None: no color), as iTerm2's Tab Color does for each
    /// session of the tab (`PT:14655-14674`). The pick replaces a program's tab color for
    /// those panes, which would otherwise win over the profile's.
    pub(super) fn set_tab_color_for(&mut self, tab: usize, color: Option<crate::palette::Rgb>) {
        let Some(t) = self.tabs.get(tab) else { return };
        for pane in t.layout.panes() {
            self.tab_colors.remove(&pane);
            let separate = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).is_some_and(|p| p.bool("Use Separate Colors for Light and Dark Mode"));
            self.set_session_values(pane, &tab_color_values(color, separate));
        }
    }

    /// A tab's tooltip text (`PT tabView:toolTipForTabViewItem:`): its title without HTML, its
    /// focused session's profile and the command it started with. None with
    /// `DisableTabBarTooltips` or no such tab.
    pub(super) fn tab_tip_text(&self, tab: usize) -> Option<String> {
        let t = self.tabs.get(tab)?;
        let pane = self.panes.get(&t.focused)?;
        let (own, original) = tip_profile_names(&self.store, &pane.profile);
        let look = self.tab_look(t);
        tip_text(self.globals.bool("DisableTabBarTooltips"), &tab_label(&look.title, &look.subtitle), self.globals.bool("HTMLTabTitles"), &own, original.as_deref(), &pane.command)
    }

    /// A session's subtitle: its profile's Subtitle over the session's variables, as the badge.
    fn subtitle_text(&self, pane: PaneId) -> String {
        let Some(p) = self.panes.get(&pane) else { return String::new() };
        let template = self.store.get(&p.profile).map(|pr| pr.string("Subtitle")).unwrap_or_default();
        let s = p.own();
        crate::badge::text(&template, &|n| s.trigger_var(n, 0))
    }

    /// Forgets pane flashes and fireworks that are over.
    pub(super) fn prune_attention(&mut self) {
        let now = Instant::now();
        self.flashes.retain(|_, t| now - *t < FLASH);
        self.fireworks.retain(|_, (t, _)| now - *t < FIREWORKS);
    }

    /// Draws RequestAttention's pane flash and fireworks; says when to draw again.
    pub(super) fn draw_attention(&self, frame: &mut Frame, pane: PaneId, inner: Rect, cell: (f32, f32)) -> Option<Instant> {
        let now = Instant::now();
        let mut next = None;
        if let Some(t) = self.flashes.get(&pane).copied() {
            let age = now - t;
            if age < FLASH {
                scene::pane_flash(frame, inner, 1.0 - age.as_secs_f32() / FLASH.as_secs_f32());
                next = Some(now + Duration::from_millis(16));
            }
        }
        if let Some((t, (col, row))) = self.fireworks.get(&pane).copied() {
            let age = now - t;
            if age < FIREWORKS {
                let center = (inner.x + (col as f32 + 0.5) * cell.0, inner.y + (row as f32 + 0.5) * cell.1);
                scene::fireworks(frame, center, age.as_secs_f32() / FIREWORKS.as_secs_f32(), cell);
                next = Some(now + Duration::from_millis(16));
            }
        }
        next
    }
}

/// The primary monitor's height in pixels (it sits at the virtual screen's origin).
fn primary_height() -> f64 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CYSCREEN};
    unsafe { GetSystemMetrics(SM_CYSCREEN) as f64 }
}

/// Each monitor's work area (without the taskbar) in virtual-screen pixels.
pub(super) fn work_areas() -> Vec<(f64, f64, f64, f64)> {
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> windows_sys::Win32::Foundation::BOOL {
        let out = unsafe { &mut *(data as *mut Vec<(f64, f64, f64, f64)>) };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(m, &mut info) } != 0 {
            let r = info.rcWork;
            out.push((r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64));
        }
        1
    }
    let mut out: Vec<(f64, f64, f64, f64)> = Vec::new();
    unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(each), &mut out as *mut _ as LPARAM) };
    out
}

#[cfg(test)]
mod tests {
    use super::tip_text;

    /// With SeparateWindowTitlePerTab off every other session of the window takes a
    /// program's title (`PT:15578-15584`).
    #[test]
    fn shared_window_title_follows_iterm2() {
        use super::share_targets;
        assert_eq!(share_targets(&[1, 2, 3], 2, false), vec![1, 3]);
        assert_eq!(share_targets(&[1, 2, 3], 2, true), Vec::<u64>::new());
    }

    #[test]
    fn tab_color_follows_iterm2() {
        use super::{CHOOSE_COLOR, NO_COLOR, tab_color_values};
        use crate::settings_ui::value_of_rgb;
        let c = crate::palette::rgb(0x5f, 0xa3, 0xf8);
        let v = value_of_rgb([0x5f, 0xa3, 0xf8]);
        assert_eq!(tab_color_values(Some(c), false), vec![("Use Tab Color".to_string(), Some(toml::Value::Boolean(true))), ("Tab Color".to_string(), Some(v.clone()))]);
        assert_eq!(tab_color_values(None, false), vec![("Use Tab Color".to_string(), Some(toml::Value::Boolean(false)))]);
        // With separate light and dark colors the pick goes to both modes too
        // (`PTYSession setTabColorByUI:`, `allTabColorKeysForBaseKey:`).
        let t = |k: &str| (k.to_string(), Some(toml::Value::Boolean(true)));
        let col = |k: &str| (k.to_string(), Some(v.clone()));
        assert_eq!(tab_color_values(Some(c), true), vec![t("Use Tab Color"), t("Use Tab Color (Dark)"), t("Use Tab Color (Light)"), col("Tab Color"), col("Tab Color (Dark)"), col("Tab Color (Light)")]);
        let f = |k: &str| (k.to_string(), Some(toml::Value::Boolean(false)));
        assert_eq!(tab_color_values(None, true), vec![f("Use Tab Color"), f("Use Tab Color (Dark)"), f("Use Tab Color (Light)")]);
        assert_eq!(NO_COLOR, "Default");
        assert_eq!(CHOOSE_COLOR, "Choose Color\u{2026}");
    }

    #[test]
    fn tooltips_follow_iterm2() {
        use super::tab_label;
        // The name is the tab's label: the title, a line break and the subtitle, the line
        // blank when there is no subtitle (`TAB:735-751`).
        assert_eq!(tab_label("zsh", "~/src"), "zsh\n~/src");
        assert_eq!(tab_label("zsh", ""), "zsh\n");
        // `DisableTabBarTooltips` shows none (`PT:8701-8703`).
        assert_eq!(tip_text(true, "zsh\n", false, "Default", None, "zsh -l"), None);
        // The name, the profile and the command, one per line.
        assert_eq!(tip_text(false, "zsh\n", false, "Default", None, "zsh -l").as_deref(), Some("Name: zsh\n\nProfile: Default\nCommand: zsh -l"));
        assert_eq!(tip_text(false, "zsh\n~/src", false, "Default", None, "zsh -l").as_deref(), Some("Name: zsh\n~/src\nProfile: Default\nCommand: zsh -l"));
        // The label's HTML is removed when HTML titles are on, kept as typed when off.
        assert_eq!(tip_text(false, "<b>x</b>\n", true, "Default", None, "cmd").as_deref(), Some("Name: x\n\nProfile: Default\nCommand: cmd"));
        assert_eq!(tip_text(false, "<b>x</b>\n", false, "Default", None, "cmd").as_deref(), Some("Name: <b>x</b>\n\nProfile: Default\nCommand: cmd"));
        // A session copy names its saved original; a copy whose original is gone its own name.
        assert_eq!(tip_text(false, "t\n", false, "Default (copy)", Some("Default"), "cmd").as_deref(), Some("Name: t\n\nProfile: Default\nCommand: cmd"));
        assert_eq!(tip_text(false, "t\n", false, "Mine", None, "cmd").as_deref(), Some("Name: t\n\nProfile: Mine\nCommand: cmd"));
        // No command reads "None".
        assert_eq!(tip_text(false, "t\n", false, "Default", None, "").as_deref(), Some("Name: t\n\nProfile: Default\nCommand: None"));
    }

    #[test]
    fn tooltip_profile_names_follow_iterm2() {
        use crate::profile::Profile;
        use crate::session_profile::{ORIGINAL_GUID, divorce};
        let mut saved = Profile::default();
        saved.set("Guid", toml::Value::String("saved".into()));
        saved.set("Name", toml::Value::String("Work".into()));
        let mut store = crate::store::Store::fresh(std::path::Path::new("profiles-test.toml"));
        store.add(saved.clone());
        // A saved profile: its own name.
        assert_eq!(super::tip_profile_names(&store, "saved"), ("Work".to_string(), None));
        // A session copy: its original's name.
        let mut copy = divorce(&saved, "copy".into());
        copy.profile.set("Name", toml::Value::String("Work edited".into()));
        store.sessions.push(copy);
        assert_eq!(super::tip_profile_names(&store, "copy"), ("Work edited".to_string(), Some("Work".to_string())));
        // A copy whose original is gone: its own name.
        let mut orphan = divorce(&saved, "orphan".into());
        orphan.profile.set(ORIGINAL_GUID, toml::Value::String("gone".into()));
        store.sessions.push(orphan);
        assert_eq!(super::tip_profile_names(&store, "orphan"), ("Work".to_string(), None));
        // An unknown Guid: no name.
        assert_eq!(super::tip_profile_names(&store, "nothing"), (String::new(), None));
    }
}
