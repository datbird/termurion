//! Scripts > Manage > Export's chooser (iTerm2's `iTermScriptChooser`, an open panel in the
//! Scripts folder with its accessory view): the Scripts folder's tree with a checkbox for
//! each script and for the AutoLaunch folder (the panel's validator allows both,
//! `M:683-690`), "Offer to launch automatically during installation", OK and Cancel.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use crate::egui_host::EguiHost;
use crate::script_archive as sa;
use crate::scripts::Node;

/// What the person did in the chooser.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportEvent {
    None,
    Export { paths: Vec<PathBuf>, autolaunch: bool },
    Cancel,
}

/// One line of the tree: its depth, its title, and its path when it can be chosen (a script,
/// or the AutoLaunch folder).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub depth: usize,
    pub label: String,
    pub choosable: Option<PathBuf>,
}

/// The tree's lines in the scan's order: folders then their contents, indented.
pub fn rows(tree: &[Node], autolaunch: &Path) -> Vec<Row> {
    fn walk(nodes: &[Node], depth: usize, autolaunch: &Path, out: &mut Vec<Row>) {
        for n in nodes {
            match n {
                Node::Script { name, path, .. } => out.push(Row { depth, label: name.clone(), choosable: Some(path.clone()) }),
                Node::Folder { name, path, children } => {
                    out.push(Row { depth, label: name.clone(), choosable: (path == autolaunch).then(|| path.clone()) });
                    walk(children, depth + 1, autolaunch, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, 0, autolaunch, &mut out);
    out
}

pub struct ExportWindow {
    host: EguiHost,
    rows: Vec<Row>,
    chosen: BTreeSet<PathBuf>,
    /// "Offer to launch automatically during installation", off at first
    /// (`autoLaunchByDefault:NO`).
    autolaunch: bool,
}

impl ExportWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn open(event_loop: &ActiveEventLoop, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device, queue: &wgpu::Queue, tree: &[Node], autolaunch_folder: &Path) -> Result<ExportWindow, String> {
        let host = EguiHost::open(event_loop, instance, adapter, device, queue, sa::EXPORT_TITLE, 480.0, 420.0)?;
        Ok(ExportWindow { host, rows: rows(tree, autolaunch_folder), chosen: BTreeSet::new(), autolaunch: false })
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

    pub fn redraw(&mut self) -> ExportEvent {
        let mut out = ExportEvent::None;
        let (rows, chosen, autolaunch) = (&self.rows, &mut self.chosen, &mut self.autolaunch);
        let mut changed = false;
        self.host.paint(|ui| {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                out = ExportEvent::Cancel;
            }
            egui::Panel::bottom("buttons").show(ui, |ui| {
                ui.add_space(4.0);
                ui.checkbox(autolaunch, sa::OFFER_AUTOLAUNCH);
                ui.add_space(4.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(!chosen.is_empty(), egui::Button::new(crate::api_auth::OK)).clicked() {
                        out = ExportEvent::Export { paths: chosen.iter().cloned().collect(), autolaunch: *autolaunch };
                    }
                    if ui.button(crate::api_auth::CANCEL).clicked() {
                        out = ExportEvent::Cancel;
                    }
                });
                ui.add_space(4.0);
            });
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for r in rows {
                        ui.horizontal(|ui| {
                            ui.add_space(r.depth as f32 * 18.0);
                            match &r.choosable {
                                Some(p) => {
                                    let mut on = chosen.contains(p);
                                    if ui.checkbox(&mut on, &r.label).changed() {
                                        changed = true;
                                        if on {
                                            chosen.insert(p.clone());
                                        } else {
                                            chosen.remove(p);
                                        }
                                    }
                                }
                                None => {
                                    ui.label(&r.label);
                                }
                            }
                        });
                    }
                });
            });
        });
        if changed {
            self.host.request_redraw();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_rows_follow_the_chooser() {
        let script = |p: &str, full: bool| Node::Script { name: Path::new(p).file_name().unwrap().to_string_lossy().into_owned(), path: PathBuf::from(p), full_environment: full };
        let folder = |p: &str, children| Node::Folder { name: Path::new(p).file_name().unwrap().to_string_lossy().into_owned(), path: PathBuf::from(p), children };
        let tree = vec![folder(r"C:\S\AutoLaunch", vec![script(r"C:\S\AutoLaunch\run.py", false)]), folder(r"C:\S\zeta", vec![script(r"C:\S\zeta\z.py", false)]), script(r"C:\S\Full", true)];
        let row = |depth, label: &str, p: Option<&str>| Row { depth, label: label.into(), choosable: p.map(PathBuf::from) };
        assert_eq!(
            rows(&tree, Path::new(r"C:\S\AutoLaunch")),
            vec![
                row(0, "AutoLaunch", Some(r"C:\S\AutoLaunch")),
                row(1, "run.py", Some(r"C:\S\AutoLaunch\run.py")),
                // A plain folder is only a heading.
                row(0, "zeta", None),
                row(1, "z.py", Some(r"C:\S\zeta\z.py")),
                row(0, "Full", Some(r"C:\S\Full")),
            ]
        );
    }
}
