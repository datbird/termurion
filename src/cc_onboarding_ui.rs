//! The Claude Code integration's setup window (`OB:26-65,714-1182`): an egui window over
//! `EguiHost`. First the "Before You Start" sheet, then one page per step with Back, the
//! step's own button (Enable, Install, Show) and Next (Close on the last). Each step's line
//! carries a check mark, a play or a ring glyph (Segoe Fluent Icons). The window only asks:
//! the app does each step and marks it done.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::claude_code::{self, Step};
use crate::egui_host::EguiHost;

/// What the window asks of the app.
#[derive(Clone, Debug, PartialEq)]
pub enum OnboardAct {
    /// The current step's button: do it.
    DoIt(Step),
    /// "Learn more about the Claude Code integration".
    Learn,
    /// Close on the last step, the window's close box, or Ctrl+W.
    Closed,
}

/// The mark in front of a step's line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Done,
    Current,
    Other,
}

const CHECK: char = '\u{E73E}';
const PLAY: char = '\u{E768}';
const RING: char = '\u{E91F}';

impl Mark {
    fn glyph(self) -> char {
        match self {
            Mark::Done => CHECK,
            Mark::Current => PLAY,
            Mark::Other => RING,
        }
    }
}

/// Which button the Return key presses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enter {
    Next,
    DoIt,
}

/// Where the setup is: the sheet, then the step page, and which steps are done.
#[derive(Clone, Debug)]
pub struct Flow {
    pub steps: Vec<Step>,
    pub done: Vec<Step>,
    pub current: usize,
    pub intro: bool,
}

impl Flow {
    pub fn new(steps: Vec<Step>) -> Flow {
        Flow { steps, done: Vec::new(), current: 0, intro: true }
    }

    pub fn step(&self) -> Step {
        self.steps[self.current]
    }

    pub fn is_done(&self, step: Step) -> bool {
        self.done.contains(&step)
    }

    pub fn mark_done(&mut self, step: Step) {
        if !self.is_done(step) {
            self.done.push(step);
        }
    }

    fn is_last(&self) -> bool {
        self.current + 1 == self.steps.len()
    }

    pub fn back_enabled(&self) -> bool {
        self.current > 0
    }

    /// Next, or Close on the last step.
    pub fn next_label(&self) -> &'static str {
        if self.is_last() { claude_code::CLOSE } else { claude_code::NEXT }
    }

    /// Show Toolbelt is optional, so Next is always on there (`OB:1166-1172`).
    pub fn next_enabled(&self) -> bool {
        self.is_last() || self.is_done(self.step()) || self.step() == Step::ShowToolbelt
    }

    pub fn enter(&self) -> Enter {
        if self.is_done(self.step()) { Enter::Next } else { Enter::DoIt }
    }

    pub fn mark(&self, i: usize) -> Mark {
        if self.is_done(self.steps[i]) {
            Mark::Done
        } else if i == self.current {
            Mark::Current
        } else {
            Mark::Other
        }
    }

    pub fn back(&mut self) {
        self.current = self.current.saturating_sub(1);
    }

    /// Next: the following step; true when it was the last (the window closes).
    pub fn next(&mut self) -> bool {
        if self.is_last() {
            return true;
        }
        self.current += 1;
        false
    }
}

pub struct OnboardingWindow {
    host: EguiHost,
    flow: Flow,
    settings: std::path::PathBuf,
}

impl OnboardingWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        steps: Vec<Step>,
        settings: &std::path::Path,
        dark: bool,
    ) -> Result<OnboardingWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, claude_code::PANEL_TITLE, 800.0, 380.0)?;
        host.follow_theme(Some(dark));
        crate::settings_keys::add_icon_font(host.ctx());
        Ok(OnboardingWindow { host, flow: Flow::new(steps), settings: settings.to_path_buf() })
    }

    pub fn id(&self) -> WindowId {
        self.host.id()
    }

    pub fn focus(&self) {
        self.host.focus();
    }

    /// A step the app did (or found already done): its line shows the check mark.
    pub fn mark_done(&mut self, step: Step) {
        if !self.flow.is_done(step) {
            self.flow.mark_done(step);
            self.host.request_redraw();
        }
    }

    /// Whether a step is marked done.
    pub fn is_done(&self, step: Step) -> bool {
        self.flow.is_done(step)
    }

    pub fn event(&mut self, e: &WindowEvent) -> bool {
        self.host.event(e)
    }

    /// Draws the window; returns what it asks of the app.
    pub fn redraw(&mut self) -> Vec<OnboardAct> {
        let mut acts = Vec::new();
        let Self { host, flow, settings } = self;
        host.paint(|ui| {
            if ui.input(|i| i.key_pressed(egui::Key::W) && i.modifiers.command) {
                acts.push(OnboardAct::Closed);
            }
            egui::CentralPanel::default().show(ui, |ui| {
                if flow.intro {
                    intro(ui, flow, settings, &mut acts);
                } else {
                    page(ui, flow, &mut acts);
                }
            });
        });
        acts
    }
}

fn intro(ui: &mut egui::Ui, flow: &mut Flow, settings: &std::path::Path, acts: &mut Vec<OnboardAct>) {
    ui.heading(claude_code::INTRO_TITLE);
    ui.add_space(8.0);
    ui.label(claude_code::INTRO_LEAD);
    ui.add_space(8.0);
    ui.label(egui::RichText::new(claude_code::WHAT_CHANGES).strong());
    ui.label(claude_code::bullets(settings));
    ui.add_space(8.0);
    if ui.link(claude_code::LEARN_MORE).clicked() {
        acts.push(OnboardAct::Learn);
    }
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
        let go = ui.button(claude_code::CONTINUE).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
        if go {
            flow.intro = false;
        }
    });
}

fn page(ui: &mut egui::Ui, flow: &mut Flow, acts: &mut Vec<OnboardAct>) {
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        ui.horizontal(|ui| {
            if ui.add_enabled(flow.back_enabled(), egui::Button::new(claude_code::BACK)).clicked() {
                flow.back();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                let next = ui.add_enabled(flow.next_enabled(), egui::Button::new(flow.next_label())).clicked() || (enter && flow.enter() == Enter::Next && flow.next_enabled());
                let do_it = ui.button(flow.step().button()).clicked() || (enter && flow.enter() == Enter::DoIt);
                if do_it {
                    acts.push(OnboardAct::DoIt(flow.step()));
                }
                if next && flow.next() {
                    acts.push(OnboardAct::Closed);
                }
            });
        });
        ui.separator();
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(220.0);
                    for (i, step) in flow.steps.clone().into_iter().enumerate() {
                        let mark = flow.mark(i);
                        ui.horizontal(|ui| {
                            let glyph = crate::settings_keys::icon_text(ui.ctx(), mark.glyph()).unwrap_or_else(|| egui::RichText::new(match mark {
                                Mark::Done => "\u{2713}",
                                Mark::Current => "\u{25B6}",
                                Mark::Other => "\u{25CB}",
                            }));
                            ui.label(glyph);
                            let title = egui::RichText::new(step.title());
                            ui.label(if mark == Mark::Current { title.strong() } else { title });
                        });
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.heading(flow.step().title());
                    ui.add_space(6.0);
                    ui.label(flow.step().description());
                });
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `OB:1154-1182`: Back is off on the first step, Next (Close on the last) is on when the
    /// step is done or is Show Toolbelt, and Return presses Next once the step is done, else
    /// the step's own button.
    #[test]
    fn onboarding_flow_follows_iterm2() {
        let mut f = Flow::new(vec![Step::EnablePythonApi, Step::InstallHook, Step::ShowToolbelt]);
        assert!(f.intro, "the sheet comes first");
        assert_eq!(f.step(), Step::EnablePythonApi);
        assert!(!f.back_enabled());
        assert_eq!((f.next_label(), f.next_enabled(), f.enter()), ("Next", false, Enter::DoIt));
        assert_eq!((f.mark(0), f.mark(1), f.mark(2)), (Mark::Current, Mark::Other, Mark::Other));
        f.mark_done(Step::EnablePythonApi);
        assert_eq!((f.next_enabled(), f.enter(), f.mark(0)), (true, Enter::Next, Mark::Done));
        assert!(!f.next());
        assert_eq!(f.step(), Step::InstallHook);
        assert!(f.back_enabled());
        assert!(!f.next_enabled());
        assert_eq!((f.mark(0), f.mark(1)), (Mark::Done, Mark::Current));
        f.back();
        assert_eq!(f.step(), Step::EnablePythonApi);
        f.back();
        assert_eq!(f.step(), Step::EnablePythonApi, "Back stops at the first step");
        f.next();
        f.mark_done(Step::InstallHook);
        assert!(!f.next());
        // Show Toolbelt: Close, always on; Return is the step's button until it is done.
        assert_eq!((f.step(), f.next_label(), f.next_enabled(), f.enter()), (Step::ShowToolbelt, "Close", true, Enter::DoIt));
        f.mark_done(Step::ShowToolbelt);
        assert_eq!(f.enter(), Enter::Next);
        assert!(f.next(), "Close ends it");
        // With the API on there is no API step.
        let g = Flow::new(claude_code::steps(true, true));
        assert_eq!(g.step(), Step::InstallHook);
        assert!(!g.back_enabled());
    }

    #[test]
    fn a_step_is_marked_done_once() {
        let mut f = Flow::new(vec![Step::InstallHook]);
        f.mark_done(Step::InstallHook);
        f.mark_done(Step::InstallHook);
        assert_eq!(f.done, [Step::InstallHook]);
        assert_eq!((f.next_label(), f.mark(0)), ("Close", Mark::Done));
    }
}
