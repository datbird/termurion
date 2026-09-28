//! Settings > General > AI, drawn inside the settings window (egui): its three sub-tabs and
//! the state they keep. Not `#[cfg(windows)]`, so its pure helpers test anywhere;
//! `SettingsWindow::show_ai` lives in `settings_ui.rs`.

use toml::Value;

use crate::ai_catalog::{self, Catalog};
use crate::ai_gate;
use crate::ai_keys::{self, KeyStore, Keys};
use crate::ai_models::{self, default_endpoint, manual_vendor, ManualModel};
use crate::ai_policy::{self, KeyPolicy};
use crate::ai_settings::{self as ai, Vendor};
use crate::globals::Globals;
use crate::pwm::Secret;

/// The AI section's sub-tabs (`X:7171-7866`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubTab {
    #[default]
    General,
    Prompts,
    Features,
}

/// What the AI section keeps between frames. Tasks 8 and 9 add private fields.
#[derive(Default)]
pub struct State {
    pub sub: SubTab,
    /// The section was revealed (the gate's Reveal, Codecierge's Reveal Setting): the next
    /// frame scrolls it into view.
    pub reveal: bool,
    /// The "Manage AI API Keys" dialog while it is open.
    keys: Option<KeyDialog>,
    /// A failed key write's text, for the settings window to show.
    error: Option<String>,
}

impl State {
    /// The last failed key write's text, clearing it.
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
}

/// The key dialog's fields, one per `Vendor::KEY_ORDER` vendor: what each was prefilled with
/// and what it holds now. Both are dropped (and zeroed) when the dialog closes.
struct KeyDialog {
    initial: Vec<Secret>,
    now: Vec<Secret>,
    /// The first field takes the focus on the dialog's first frame.
    focus: bool,
}

// The General sub-tab's texts (`X:7171-7466`, `G:2859-3358`, `G:3547-3626`), Tandem's where
// iTerm2's name a plugin or the macOS Keychain.
pub const REQUESTS_LABEL: &str = "Requests:";
pub const REQUESTS_NOTE: &str = "To ensure privacy, Tandem asks before it first sends AI requests to a provider.";
pub const FORGET_HOSTS: &str = "Forget Allowed Hosts";
pub const CONSENT_LABEL: &str = "Consent:";
pub const ENABLE: &str = "Enable generative AI features";
pub const NO_THIRD_PARTIES: &str = "No data is sent to third parties without your explicit permission.";
pub const ALWAYS_PROMPTED: &str = "You will always be prompted before data is shared.";
pub const API_KEY_LABEL: &str = "API Key:";
pub const SET_API_KEY: &str = "Set API Key\u{2026}";
pub const MODEL_LABEL: &str = "AI Model:";
pub const MANAGE_MODELS: &str = "Manage AI Models\u{2026}";
pub const TIMEOUT_LABEL: &str = "Timeout:";
pub const SECONDS: &str = "seconds";
pub const SAFETY: &str = "Use AI to check safety of commands before running them";
pub const ZDR: &str = "Enable \u{201c}zero data retention\u{201d} (send whole conversation every time)";
pub const AUTO_UPDATE: &str = "Automatically update available AI models from live online database";
pub const UPDATE_NOW: &str = "Update Now";
pub const UPDATES_OFF: &str = "Tandem ships its AI model list with each release; online updates are not available.";
pub const KEYS_HEADING: &str = "Manage AI API Keys";
pub const KEYS_TEXT: &str = "Keys are stored securely in the Windows Credential Manager.";
pub const NO_KEY_USED: &str = "No API key is used.";
pub const SELF_HOSTED: &str = "No API key is sent to a local endpoint. If it requires authentication, use a custom header.";
const LATER: &str = "Comes in milestone 12.";

pub fn key_placeholder(provider: &str) -> String {
    format!("{provider} API key")
}

pub fn authorizes(provider: &str) -> String {
    format!("Authorizes with your {provider} API key.")
}

pub fn no_key_yet(provider: &str) -> String {
    format!("No {provider} API key is set yet.")
}

pub fn manual_title(model: &str, provider: &str) -> String {
    format!("Manual: {model} \u{2014} {provider}")
}

/// The one-vendor key prompt's heading and its field's hint (ruling 12e-1).
pub fn key_prompt(v: Vendor) -> (String, String) {
    (KEYS_HEADING.to_string(), key_placeholder(v.provider_name()))
}

/// The default-model popup's item identifiers (`G:2663-2686`).
pub const PROVIDER_PREFIX: &str = "provider:";
pub const MANUAL_PREFIX: &str = "manual:";

/// The default-model popup's items as (identifier, title) (`G:2649-2661`, `G:2859-2890`): the
/// providers, then the manual models, which the popup lists beneath a separator.
pub fn model_items(g: &Globals, _catalog: &Catalog) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let providers = Vendor::POPUP_ORDER.iter().map(|v| (format!("{PROVIDER_PREFIX}{}", v.raw()), v.provider_name().to_string())).collect();
    let manuals = ai_models::settings_manual_models(g)
        .iter()
        .map(|m| (format!("{MANUAL_PREFIX}{}", m.name), manual_title(&m.name, manual_vendor(m.api(), &m.url, &m.name).provider_name())))
        .collect();
    (providers, manuals)
}

/// The identifier of the stored default (`G:2863-2870`).
pub fn stored_identifier(g: &Globals) -> String {
    if g.bool(ai::USE_RECOMMENDED) {
        format!("{PROVIDER_PREFIX}{}", g.int(ai::VENDOR))
    } else {
        format!("{MANUAL_PREFIX}{}", g.string(ai::MODEL))
    }
}

/// The item the popup shows: the stored default when an item matches it, else the first
/// provider's. Nothing is written: only a person's pick changes the default (`G:2893-2907`).
pub fn shown_identifier(g: &Globals, catalog: &Catalog) -> String {
    let stored = stored_identifier(g);
    let (providers, manuals) = model_items(g, catalog);
    if providers.iter().chain(&manuals).any(|(id, _)| *id == stored) {
        return stored;
    }
    providers.into_iter().next().map(|(id, _)| id).unwrap_or(stored)
}

/// The manual model an identifier names, among the popup's.
fn manual_named(g: &Globals, identifier: &str) -> Option<ManualModel> {
    let name = identifier.strip_prefix(MANUAL_PREFIX)?;
    ai_models::settings_manual_models(g).into_iter().find(|m| m.name == name)
}

/// A person picked an item (`G:3129-3160`): a provider becomes the default with its
/// recommended model, a manual model with its own settings. Choosing Ollama writes the vendor
/// without probing the server (ruling 12a-4; the probe comes with 12b).
pub fn pick(g: &mut Globals, identifier: &str, catalog: &Catalog) {
    if let Some(raw) = identifier.strip_prefix(PROVIDER_PREFIX) {
        if let Some(v) = raw.parse().ok().and_then(Vendor::from_raw) {
            ai_models::select_provider(g, v, catalog);
        }
    } else if let Some(m) = manual_named(g, identifier) {
        ai_models::select_manual(g, &m);
    }
}

/// The line under the popup saying which key authorizes the shown model (`G:3306-3358`).
/// None for the Ollama vendor, whose Regular and Budget pickers take the space (12b); a
/// manual model at a local endpoint never receives a key; otherwise the vendor's key, set or
/// not, or no key at all.
pub fn key_hint(g: &Globals, shown: &str, _catalog: &Catalog, key_set: &dyn Fn(Vendor) -> bool) -> Option<String> {
    let manual = manual_named(g, shown);
    let vendor = match (shown.strip_prefix(PROVIDER_PREFIX), &manual) {
        (Some(raw), _) => raw.parse().ok().and_then(Vendor::from_raw),
        (None, Some(m)) => Some(manual_vendor(m.api(), &m.url, &m.name)),
        (None, None) => None,
    }
    .or_else(|| Vendor::from_raw(g.int(ai::VENDOR)));
    if manual.is_none() && vendor == Some(Vendor::Llama) {
        return None;
    }
    if manual.as_ref().is_some_and(|m| ai_policy::key_policy(&m.url, m.api()) == KeyPolicy::LocalEndpoint) {
        return Some(SELF_HOSTED.to_string());
    }
    Some(match vendor.filter(|v| v.has_enterable_key()) {
        Some(v) if key_set(v) => authorizes(v.provider_name()),
        Some(v) => no_key_yet(v.provider_name()),
        None => NO_KEY_USED.to_string(),
    })
}

/// Which General controls take input (`G:3285-3297`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Enabled {
    /// "Enable generative AI features": the administrator's switch.
    pub enable: bool,
    /// The key, the model, the safety check: AI features are allowed.
    pub rest: bool,
}

pub fn enabled(g: &Globals) -> Enabled {
    Enabled { enable: g.bool(ai::ADMIN), rest: ai::allowed(g) }
}

/// Where the default model's requests go and whether the person allowed them (ruling 1).
pub fn requests_text(g: &Globals, catalog: &Catalog) -> String {
    let Some(e) = default_endpoint(g, catalog) else { return ai_gate::NO_ENDPOINT.to_string() };
    let Some(host) = ai_policy::host_of(&e.url) else { return ai_gate::NO_ENDPOINT.to_string() };
    let provider = e.vendor.provider_name();
    if ai_gate::consented(g, &host) {
        format!("Requests go to {host} ({provider}). You allowed them.")
    } else {
        format!("Requests go to {host} ({provider}). Tandem asks before the first one.")
    }
}

/// The key dialog's fields the person changed (`G:3606-3618`): only those are written, so a
/// field that prefilled blank because its read failed never erases the stored key.
pub fn changed_fields(initial: &[Secret], now: &[Secret]) -> Vec<usize> {
    now.iter().enumerate().filter(|(i, s)| initial.get(*i).map_or("", |k| k.expose()) != s.expose()).map(|(i, _)| i).collect()
}

/// The app's key cache, whatever a panicking holder left in it.
fn keys() -> std::sync::MutexGuard<'static, Keys> {
    ai_keys::shared().lock().unwrap_or_else(|e| e.into_inner())
}

/// Opens the key dialog: each vendor's key is read once (a read that fails prefills blank).
/// Credential Manager calls run on the UI thread; they never prompt (Decision).
fn open_key_dialog() -> KeyDialog {
    let mut store = ai_keys::system_store();
    let mut cache = keys();
    let initial: Vec<Secret> = Vendor::KEY_ORDER.iter().map(|v| cache.get(*v, &mut *store).unwrap_or_else(|| Secret::new(String::new()))).collect();
    let now = initial.iter().map(Secret::duplicate).collect();
    KeyDialog { initial, now, focus: true }
}

/// OK: writes the changed fields. A failed write keeps the key for the session (`Keys::set`)
/// and becomes the settings window's error text (Tandem's; iTerm2 only logs).
fn save_keys(d: &KeyDialog, store: &mut dyn KeyStore) -> Option<String> {
    let mut error = None;
    let mut cache = keys();
    for i in changed_fields(&d.initial, &d.now) {
        let v = Vendor::KEY_ORDER[i];
        if let Err(e) = cache.set(v, &d.now[i], store) {
            error = Some(format!("Could not save the {} API key: {}", v.provider_name(), e));
        }
    }
    error
}

/// The key dialog (`G:3547-3626`): one masked field per vendor, OK (Enter) and Cancel (Escape).
/// Returns true when it closed.
fn key_dialog(ctx: &egui::Context, d: &mut KeyDialog, error: &mut Option<String>) -> bool {
    let mut done = false;
    egui::Modal::new(egui::Id::new("ai-keys")).show(ctx, |ui| {
        ui.set_width(90.0 + 14.0 + 516.0);
        ui.strong(KEYS_HEADING);
        ui.add_space(4.0);
        ui.label(KEYS_TEXT);
        ui.add_space(10.0);
        for (i, (v, s)) in Vendor::KEY_ORDER.iter().zip(d.now.iter_mut()).enumerate() {
            ui.horizontal(|ui| {
                let name = v.provider_name();
                ui.allocate_ui_with_layout(egui::vec2(90.0, 22.0), egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(name));
                ui.add_space(8.0);
                let id = egui::Id::new(("ai-key", i));
                crate::secret_field::secret_field(ui, id, s, true, Some(&key_placeholder(name)), 516.0, false);
                if i == 0 && std::mem::take(&mut d.focus) {
                    ui.memory_mut(|m| m.request_focus(id));
                }
            });
            ui.add_space(6.0);
        }
        ui.add_space(8.0);
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter) && i.modifiers.is_none(), i.key_pressed(egui::Key::Escape)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ok = ui.add(egui::Button::new("OK").min_size(egui::vec2(76.0, 24.0))).clicked();
            let cancel = ui.add(egui::Button::new("Cancel").min_size(egui::vec2(76.0, 24.0))).clicked();
            if ok || enter {
                *error = save_keys(d, &mut *ai_keys::system_store());
                done = true;
            } else if cancel || escape {
                done = true;
            }
        });
    });
    done
}

/// A right-aligned label in the General sub-tab's first column.
fn row_label(ui: &mut egui::Ui, text: &str) {
    ui.allocate_ui_with_layout(egui::vec2(90.0, 20.0), egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(text));
}

/// A checkbox on a stored boolean. True when it changed.
fn bool_box(ui: &mut egui::Ui, g: &mut Globals, key: &str, label: &str, on: bool) -> bool {
    let mut b = g.bool(key);
    let changed = ui.add_enabled(on, egui::Checkbox::new(&mut b, label)).changed();
    if changed {
        g.set(key, Value::Boolean(b));
    }
    changed
}

/// The General sub-tab (`X:7171-7466`). True when a setting changed.
fn general(ui: &mut egui::Ui, g: &mut Globals, st: &mut State) -> bool {
    let catalog = ai_catalog::bundled();
    let on = enabled(g);
    let mut changed = false;
    egui::Grid::new("ai-general").num_columns(2).spacing(egui::vec2(8.0, 10.0)).show(ui, |ui| {
        row_label(ui, REQUESTS_LABEL);
        ui.vertical(|ui| {
            ui.label(requests_text(g, catalog));
            ui.weak(REQUESTS_NOTE);
            let any = matches!(g.values.get(ai::CONSENT), Some(Value::Array(a)) if !a.is_empty());
            if ui.add_enabled(any, egui::Button::new(FORGET_HOSTS)).clicked() {
                ai_gate::forget_all(g);
                changed = true;
            }
        });
        ui.end_row();

        row_label(ui, CONSENT_LABEL);
        ui.vertical(|ui| {
            changed |= bool_box(ui, g, ai::ENABLE_AI, ENABLE, on.enable);
            ui.weak(NO_THIRD_PARTIES);
            ui.weak(ALWAYS_PROMPTED);
        });
        ui.end_row();

        row_label(ui, API_KEY_LABEL);
        if ui.add_enabled(on.rest, egui::Button::new(SET_API_KEY)).clicked() {
            st.keys = Some(open_key_dialog());
        }
        ui.end_row();

        row_label(ui, MODEL_LABEL);
        ui.vertical(|ui| {
            let shown = shown_identifier(g, catalog);
            let (providers, manuals) = model_items(g, catalog);
            let title = providers.iter().chain(&manuals).find(|(id, _)| *id == shown).map_or(String::new(), |(_, t)| t.clone());
            let mut picked = None;
            ui.horizontal(|ui| {
                ui.add_enabled_ui(on.rest, |ui| {
                    egui::ComboBox::from_id_salt("ai-default-model").selected_text(title).show_ui(ui, |ui| {
                        for (id, t) in &providers {
                            if ui.selectable_label(*id == shown, t.as_str()).clicked() {
                                picked = Some(id.clone());
                            }
                        }
                        if !manuals.is_empty() {
                            ui.separator();
                            for (id, t) in &manuals {
                                if ui.selectable_label(*id == shown, t.as_str()).clicked() {
                                    picked = Some(id.clone());
                                }
                            }
                        }
                    });
                });
                ui.add_enabled(false, egui::Button::new(MANAGE_MODELS)).on_disabled_hover_text(LATER);
            });
            if let Some(id) = picked.filter(|id| *id != shown) {
                pick(g, &id, catalog);
                changed = true;
            }
            let shown = shown_identifier(g, catalog);
            let store = std::cell::RefCell::new(ai_keys::system_store());
            let key_set = |v: Vendor| keys().is_set(v, &mut **store.borrow_mut());
            if let Some(hint) = key_hint(g, &shown, catalog, &key_set) {
                ui.add(egui::Label::new(egui::RichText::new(hint.as_str()).weak()).truncate()).on_hover_text(hint);
            }
        });
        ui.end_row();

        row_label(ui, TIMEOUT_LABEL);
        ui.horizontal(|ui| {
            let mut t = g.int(ai::TIMEOUT);
            if ui.add(egui::DragValue::new(&mut t).range(0..=i64::from(i32::MAX))).changed() {
                g.set(ai::TIMEOUT, Value::Integer(t));
                changed = true;
            }
            ui.label(SECONDS);
        });
        ui.end_row();

        ui.label("");
        ui.vertical(|ui| {
            changed |= bool_box(ui, g, ai::SAFETY_CHECK, SAFETY, on.rest);
            changed |= bool_box(ui, g, ai::ZERO_DATA_RETENTION, ZDR, true);
            ui.horizontal(|ui| {
                let mut updates = g.bool(ai::MODEL_UPDATES);
                ui.add_enabled(false, egui::Checkbox::new(&mut updates, AUTO_UPDATE)).on_disabled_hover_text(UPDATES_OFF);
                ui.add_enabled(false, egui::Button::new(UPDATE_NOW)).on_disabled_hover_text(UPDATES_OFF);
            });
        });
        ui.end_row();
    });
    if let Some(d) = st.keys.as_mut() {
        if key_dialog(ui.ctx(), d, &mut st.error) {
            st.keys = None;
        }
    }
    changed
}

/// Settings > General > AI: the heading, the sub-tabs, and the chosen sub-tab. True when a
/// setting changed.
pub fn section(ui: &mut egui::Ui, g: &mut Globals, st: &mut State) -> bool {
    let heading = ui.strong("AI");
    if std::mem::take(&mut st.reveal) {
        heading.scroll_to_me(Some(egui::Align::TOP));
    }
    ui.horizontal(|ui| {
        for (sub, name) in [(SubTab::General, "General"), (SubTab::Prompts, "Prompts"), (SubTab::Features, "Features")] {
            if ui.selectable_label(st.sub == sub, name).clicked() {
                st.sub = sub;
            }
        }
    });
    ui.add_space(6.0);
    let changed = match st.sub {
        SubTab::General => general(ui, g, st),
        SubTab::Prompts | SubTab::Features => {
            ui.weak(LATER);
            false
        }
    };
    ui.add_space(8.0);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_catalog::bundled;
    use crate::ai_models::ManualModel;
    use crate::ai_settings::{Vendor, ENABLE_AI, ADMIN, MODEL, URL, USE_RECOMMENDED, VENDOR};
    use crate::globals::Globals;
    use crate::pwm::Secret;
    use std::path::Path;
    use toml::Value;

    fn manual(name: &str, url: &str, api: i64) -> ManualModel {
        let mut t = toml::Table::new();
        t.insert("name".into(), Value::String(name.into()));
        t.insert("url".into(), Value::String(url.into()));
        t.insert("api".into(), Value::Integer(api));
        ManualModel::from_table(&t).expect("a manual model")
    }

    fn with_models(models: &[ManualModel]) -> Globals {
        let mut g = Globals::empty(Path::new("settings-ai-test.toml"));
        crate::ai_models::write(&mut g, models);
        g
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    fn secrets(list: &[&str]) -> Vec<Secret> {
        list.iter().map(|s| Secret::new(s.to_string())).collect()
    }

    const LLAMA3_URL: &str = "http://localhost:11434/api/chat";
    const B_URL: &str = "https://api.example.com/v1/responses";

    #[test]
    fn model_popup_follows_iterm2() {
        let cat = bundled();
        let mut dynamic = manual("everything", "http://server.example:11434/api/chat", 5);
        dynamic.dynamic_models = true;
        let mut g = with_models(&[manual("llama3", LLAMA3_URL, 5), manual("b", B_URL, 2), dynamic]);
        let (providers, manuals) = model_items(&g, cat);
        assert_eq!(providers, pairs(&[("provider:2", "OpenAI"), ("provider:4", "Anthropic"), ("provider:1", "Gemini"), ("provider:0", "DeepSeek"), ("provider:3", "Ollama")]));
        assert_eq!(manuals, pairs(&[("manual:llama3", "Manual: llama3 \u{2014} Ollama"), ("manual:b", "Manual: b \u{2014} OpenAI")]), "a dynamic entry is not listed until 12b");

        assert_eq!(stored_identifier(&g), "provider:2");
        pick(&mut g, "manual:b", cat);
        assert_eq!(stored_identifier(&g), "manual:b");
        assert!(!g.bool(USE_RECOMMENDED), "select_manual ran");
        assert_eq!(g.string(URL), B_URL, "select_manual ran, not model_did_change");
        assert_eq!(g.int(crate::ai_settings::API), 2);

        pick(&mut g, "provider:4", cat);
        assert_eq!(stored_identifier(&g), "provider:4");
        assert!(g.bool(USE_RECOMMENDED));
        assert_eq!(g.int(VENDOR), Vendor::Anthropic.raw());
        let recommended = cat.recommended(Vendor::Anthropic).expect("Anthropic has a recommended model");
        assert_eq!(g.string(MODEL), recommended.name, "select_provider(Anthropic) ran");
    }

    #[test]
    fn unmatched_default_writes_nothing() {
        let cat = bundled();
        let mut g = with_models(&[manual("llama3", LLAMA3_URL, 5)]);
        g.set(USE_RECOMMENDED, Value::Boolean(false));
        g.set(MODEL, Value::String("gone".into()));
        let before = g.values.clone();
        assert_eq!(stored_identifier(&g), "manual:gone");
        assert_eq!(shown_identifier(&g, cat), "provider:2");
        assert_eq!(g.values, before);

        for vendor in [5, 9] {
            let mut g = with_models(&[]);
            g.set(VENDOR, Value::Integer(vendor));
            let before = g.values.clone();
            assert_eq!(shown_identifier(&g, cat), "provider:2", "AIVendor {vendor}");
            assert_eq!(g.values, before, "AIVendor {vendor}");
        }

        let mut g = with_models(&[manual("llama3", LLAMA3_URL, 5)]);
        pick(&mut g, "manual:llama3", cat);
        assert_eq!(shown_identifier(&g, cat), "manual:llama3", "a matching default shows itself");
    }

    #[test]
    fn key_hint_follows_iterm2() {
        let cat = bundled();
        let g = with_models(&[manual("llama3", LLAMA3_URL, 5), manual("b", B_URL, 2), manual("remote", "https://example.com/v1", 5)]);
        let anthropic_set = |v: Vendor| v == Vendor::Anthropic;
        let none_set = |_: Vendor| false;
        assert_eq!(key_hint(&g, "provider:4", cat, &anthropic_set).as_deref(), Some("Authorizes with your Anthropic API key."));
        assert_eq!(key_hint(&g, "provider:4", cat, &none_set).as_deref(), Some("No Anthropic API key is set yet."));
        assert_eq!(key_hint(&g, "provider:3", cat, &none_set), None, "12b puts the Regular and Budget pickers there");
        assert_eq!(key_hint(&g, "manual:llama3", cat, &none_set).as_deref(), Some("No API key is sent to a local endpoint. If it requires authentication, use a custom header."));
        assert_eq!(key_hint(&g, "manual:b", cat, &anthropic_set).as_deref(), Some("No OpenAI API key is set yet."));
        assert_eq!(key_hint(&g, "manual:remote", cat, &none_set).as_deref(), Some("No API key is used."));
    }

    #[test]
    fn enabled_rules_follow_iterm2() {
        let mut g = with_models(&[]);
        assert_eq!(enabled(&g), Enabled { enable: true, rest: false });
        g.set(ENABLE_AI, Value::Boolean(true));
        assert_eq!(enabled(&g), Enabled { enable: true, rest: true });
        g.set(ADMIN, Value::Boolean(false));
        assert_eq!(enabled(&g), Enabled { enable: false, rest: false });
    }

    #[test]
    fn key_dialog_writes_only_changes() {
        assert_eq!(changed_fields(&secrets(&["a", "", "c", ""]), &secrets(&["a", "", "d", "e"])), vec![2, 3]);
        assert_eq!(changed_fields(&secrets(&[""]), &secrets(&[""])), Vec::<usize>::new(), "a blank left blank after a failed read writes nothing");
        assert_eq!(changed_fields(&secrets(&["a"]), &secrets(&["a "])), vec![0], "a change, which Keys::set then trims");
    }

    #[test]
    fn general_texts_follow_iterm2() {
        assert_eq!(REQUESTS_LABEL, "Requests:");
        assert_eq!(REQUESTS_NOTE, "To ensure privacy, Tandem asks before it first sends AI requests to a provider.");
        assert_eq!(FORGET_HOSTS, "Forget Allowed Hosts");
        assert_eq!(CONSENT_LABEL, "Consent:");
        assert_eq!(ENABLE, "Enable generative AI features");
        assert_eq!(NO_THIRD_PARTIES, "No data is sent to third parties without your explicit permission.");
        assert_eq!(ALWAYS_PROMPTED, "You will always be prompted before data is shared.");
        assert_eq!(API_KEY_LABEL, "API Key:");
        assert_eq!(SET_API_KEY, "Set API Key\u{2026}");
        assert_eq!(MODEL_LABEL, "AI Model:");
        assert_eq!(MANAGE_MODELS, "Manage AI Models\u{2026}");
        assert_eq!(TIMEOUT_LABEL, "Timeout:");
        assert_eq!(SECONDS, "seconds");
        assert_eq!(SAFETY, "Use AI to check safety of commands before running them");
        assert_eq!(ZDR, "Enable \u{201c}zero data retention\u{201d} (send whole conversation every time)");
        assert_eq!(AUTO_UPDATE, "Automatically update available AI models from live online database");
        assert_eq!(UPDATE_NOW, "Update Now");
        assert_eq!(UPDATES_OFF, "Tandem ships its AI model list with each release; online updates are not available.");
        assert_eq!(KEYS_HEADING, "Manage AI API Keys");
        assert_eq!(KEYS_TEXT, "Keys are stored securely in the Windows Credential Manager.");
        assert_eq!(key_placeholder("Gemini"), "Gemini API key");
        assert_eq!(authorizes("OpenAI"), "Authorizes with your OpenAI API key.");
        assert_eq!(no_key_yet("OpenAI"), "No OpenAI API key is set yet.");
        assert_eq!(NO_KEY_USED, "No API key is used.");
        assert_eq!(SELF_HOSTED, "No API key is sent to a local endpoint. If it requires authentication, use a custom header.");
        assert_eq!(manual_title("m", "Ollama"), "Manual: m \u{2014} Ollama");
        assert_eq!(PROVIDER_PREFIX, "provider:");
        assert_eq!(MANUAL_PREFIX, "manual:");

        let cat = bundled();
        let mut g = with_models(&[]);
        assert_eq!(requests_text(&g, cat), "Requests go to api.openai.com (OpenAI). Tandem asks before the first one.");
        crate::ai_gate::grant(&mut g, "api.openai.com");
        assert_eq!(requests_text(&g, cat), "Requests go to api.openai.com (OpenAI). You allowed them.");
        g.set(VENDOR, Value::Integer(5));
        assert_eq!(requests_text(&g, cat), crate::ai_gate::NO_ENDPOINT);
    }

    #[test]
    fn key_prompt_follows_iterm2() {
        assert_eq!(key_prompt(Vendor::Anthropic), ("Manage AI API Keys".to_string(), "Anthropic API key".to_string()));
        assert_eq!(key_prompt(Vendor::OpenAI).1, "OpenAI API key");
        let typed = Secret::new(" \t ".to_string());
        assert_eq!(crate::ai_keys::normalized(typed.expose()), None, "a blank key writes nothing and continues with None");
    }
}
