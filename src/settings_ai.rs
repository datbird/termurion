//! Settings > General > AI, drawn inside the settings window (egui): its three sub-tabs and
//! the state they keep. Not `#[cfg(windows)]`, so its pure helpers test anywhere;
//! `SettingsWindow::show_ai` lives in `settings_ui.rs`.

use toml::Value;

use crate::ai_catalog::{self, Catalog, Model};
use crate::ai_discovery::{self as ad, Outcome, Probe};
use crate::ai_gate;
use crate::ai_model::{self, Discovered};
use crate::ai_keys::{self, KeyStore, Keys};
use crate::ai_models::{self as am, default_endpoint, manual_vendor, EditorForm, ManualModel};
use crate::ai_policy::{self, KeyPolicy};
use crate::ai_prompts::{self as ap, Prompt};
use crate::ai_test_conn::{self as tc, Tester};
use crate::ai_settings::{self as ai, Api, Vendor};
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

/// Work the settings window asks of the App, which alone holds the discovery cache, the
/// transport, the loop's wake, the AI alert queue and the key prompt (ruling R12ab-3): the App
/// takes it after the frame and answers in `State::probe`, `State::tester` or `State::test_key`.
#[derive(Clone, Debug, PartialEq)]
pub enum AiRequest {
    /// Refresh one Ollama server's models now.
    Discover { endpoint: String, headers: Vec<(String, String)>, force: bool },
    /// Test Connection needs this vendor's key for this endpoint (`App::ask_ai_key`); the answer
    /// arrives in `State::test_key`.
    AskKey(Vendor, am::Endpoint),
    /// Show this alert on the AI alert queue (Test Connection's consent, `App::ask_ai`).
    Alert(ai_gate::Alert),
    /// Build the tester for this probe into `State::tester`.
    BuildTester(tc::Probe),
}

/// The Ollama probe sheet (`G:3055-3104`): the default in effect before Ollama was picked,
/// restored when the probe fails or is cancelled, and the frame it opened in.
struct ProbeSheet {
    prior: DefaultSnapshot,
    opened: u64,
}

/// What the AI section keeps between frames.
#[derive(Default)]
pub struct State {
    pub sub: SubTab,
    /// The section was revealed (the gate's Reveal, Codecierge's Reveal Setting): the next
    /// frame scrolls it into view.
    pub reveal: bool,
    /// The "Manage AI API Keys" dialog while it is open.
    keys: Option<KeyDialog>,
    /// A failed key write's text, for the settings window's banner.
    error: Option<String>,
    /// The same text, marked with the warning glyph beside "Set API Key\u{2026}" until the
    /// dialog opens again.
    key_failure: Option<String>,
    /// The Manual AI Models sheet while it is open.
    sheet: Option<Sheet>,
    /// The model editor, over the sheet, while it is open.
    editor: Option<Editor>,
    /// The prompt the Prompts sub-tab edits.
    prompt: Prompt,
    /// The prompt help, while it is open: the frame it opened in.
    help: Option<u64>,
    /// A request for the App, taken after the frame (ruling R12ab-3).
    pub pending: Option<AiRequest>,
    /// The App's answer to a `Discover` request, read by the probe sheet or the editor's
    /// Fetch/Refresh Models, then taken.
    pub probe: Option<Outcome>,
    /// The discovered Ollama models of every server asked, which the App copies in when they
    /// change: the model popup's dynamic entries, the Regular and Budget pickers, and the
    /// editor's popups read it.
    pub found: Discovered,
    /// The Ollama probe sheet while it is open.
    probing: Option<ProbeSheet>,
    /// "Could Not Reach Ollama" after a failed probe: its text and the frame it opened in.
    tell: Option<(String, u64)>,
    /// The editor's Test Connection once the App built it; the editor's frames drive it.
    pub tester: Option<Tester>,
    /// The key prompt's answer for the running test (None: refused), taken by the next frame.
    pub test_key: Option<Option<Secret>>,
}

impl State {
    /// The last failed key write's text, clearing it.
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    /// A probe, a model fetch or a connection test is running: the window keeps drawing so its
    /// spinner turns and the answer shows at once.
    /// A test waiting for the key prompt is not: the prompt's answer asks for a redraw.
    pub fn busy(&self) -> bool {
        let waits_for_key = self.tester.as_ref().is_some_and(|t| t.needs_key().is_some());
        self.probing.is_some()
            || (!waits_for_key && (self.tester.is_some() || self.editor.as_ref().is_some_and(|e| e.testing)))
            || self.editor.as_ref().is_some_and(|e| e.fetching.is_some())
    }
}

/// The key dialog's fields, one per `Vendor::KEY_ORDER` vendor: what each was prefilled with
/// and what it holds now. Both are dropped (and zeroed) when the dialog closes.
struct KeyDialog {
    initial: Vec<Secret>,
    now: Vec<Secret>,
    /// The first field takes the focus on the dialog's first frame.
    focus: bool,
    /// The frame it opened in, where Return and Escape do nothing.
    opened: u64,
}

/// The Manual AI Models sheet (`G:282-560`): the rows it edits, the selected row, and whether
/// the legacy headers notice is still showing in front of it (`G:4005-4030`).
struct Sheet {
    models: Vec<ManualModel>,
    selected: Option<usize>,
    notice: bool,
    /// The frame it opened in, where Return and Escape do nothing.
    opened: u64,
}

/// The model editor (`G:645-1640`): the form, the model it started from, the row it edits
/// (None adds), the selected header row, and Save's refusal while it shows.
struct Editor {
    form: EditorForm,
    base: ManualModel,
    editing: Option<usize>,
    header: Option<usize>,
    failure: Option<String>,
    /// The Model field takes the focus on the first frame; a new header row's name on the next.
    focus_name: bool,
    focus_header: Option<usize>,
    /// The frame it opened in, where Return and Escape do nothing.
    opened: u64,
    /// Fetch or Refresh Models is waiting on the App: whether it was Refresh (auto-discovery),
    /// and the URL and headers it asked.
    fetching: Option<(bool, String, Vec<(String, String)>)>,
    /// The request Fetch or Refresh Models made, for `State::pending`.
    request: Option<AiRequest>,
    /// Fetch Models' names, offered to fill the Model field.
    menu: Option<Vec<String>>,
    /// An alert over the editor (title, text) and the frame it opened in.
    alert: Option<(String, String, u64)>,
    /// The URL the auto-discovery Model popup was last built for; a change clears the choice.
    popup_url: Option<String>,
    /// Test Connection runs: the button reads "Testing\u{2026}" and is disabled.
    testing: bool,
}

impl Editor {
    fn new(base: ManualModel, editing: Option<usize>, catalog: &Catalog, opened: u64) -> Editor {
        Editor {
            form: EditorForm::from_model(&base, catalog),
            base,
            editing,
            header: None,
            failure: None,
            focus_name: true,
            focus_header: None,
            opened,
            fetching: None,
            request: None,
            menu: None,
            alert: None,
            popup_url: None,
            testing: false,
        }
    }

    /// Fetch or Refresh Models' answer (`G:1355-1425`): Refresh reports the count or that the
    /// server was not reached; Fetch offers the names or says why there are none.
    fn take_answer(&mut self, o: Outcome, found: &Discovered, now: u64) {
        let Some((dynamic, url, headers)) = self.fetching.take() else { return };
        let (title, text) = if dynamic {
            if o.failed {
                (ad::REACH_SERVER_TITLE.to_string(), ad::REACH_SERVER_BODY.to_string())
            } else {
                (ad::REFRESHED_TITLE.to_string(), ad::refreshed_body(o.count))
            }
        } else {
            match ad::fetched_names(&o, &ad::names_at(found, &url, &headers)) {
                Ok(names) => {
                    self.menu = Some(names);
                    return;
                }
                Err(msg) => (ad::FETCH_FAILED_TITLE.to_string(), msg),
            }
        };
        self.alert = Some((title, text, now));
    }
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
pub const RESET: &str = "Reset";

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

/// The model editor's key hint (`G:975-1008`), from the form as a request would read it: a local
/// endpoint never receives a key; a vendor with an enterable key authorizes with it; else none.
pub fn editor_key_hint(form: &EditorForm) -> String {
    let api = Api::from_raw_or_chat(form.api);
    let url = form.url.trim();
    if ai_policy::key_policy(url, api) == KeyPolicy::LocalEndpoint {
        return SELF_HOSTED.to_string();
    }
    let v = manual_vendor(api, url, &form.name);
    if v.has_enterable_key() {
        authorizes(v.provider_name())
    } else {
        NO_KEY_USED.to_string()
    }
}

/// The preset popup's title for a tag (`G:696-717`): Custom, a catalog preset's name, or a
/// provider preset's.
pub fn preset_title(tag: i64, catalog: &Catalog) -> String {
    if tag >= am::PROVIDER_PRESET_BASE {
        let p = usize::try_from(tag - am::PROVIDER_PRESET_BASE).ok().and_then(|i| am::PROVIDER_PRESETS.get(i));
        return p.map_or(am::CUSTOM, |p| p.name).to_string();
    }
    let m = usize::try_from(tag).ok().and_then(|i| am::catalog_presets(catalog).get(i).map(|m| m.name.clone()));
    m.unwrap_or_else(|| am::CUSTOM.to_string())
}

/// The one-vendor key prompt's heading and its field's hint (ruling 12e-1).
pub fn key_prompt(v: Vendor) -> (String, String) {
    (KEYS_HEADING.to_string(), key_placeholder(v.provider_name()))
}

/// The default-model popup's item identifiers (`G:2663-2686`).
pub const PROVIDER_PREFIX: &str = "provider:";
pub const MANUAL_PREFIX: &str = "manual:";

/// The default-model popup's items as (identifier, title) (`G:2649-2661`, `G:2859-2890`): the
/// providers, then the manual models as a request resolves them, a dynamic entry expanded to
/// its discovered models (`MD:177-184`), which the popup lists beneath a separator.
pub fn model_items(g: &Globals, catalog: &Catalog, found: &Discovered) -> (Vec<(String, String)>, Vec<(String, String)>) {
    let providers = Vendor::POPUP_ORDER.iter().map(|v| (format!("{PROVIDER_PREFIX}{}", v.raw()), v.provider_name().to_string())).collect();
    let manuals = ai_model::manual_models(g, catalog, found).iter().map(|m| (format!("{MANUAL_PREFIX}{}", m.name), manual_title(&m.name, m.vendor.provider_name()))).collect();
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
pub fn shown_identifier(g: &Globals, catalog: &Catalog, found: &Discovered) -> String {
    let stored = stored_identifier(g);
    let (providers, manuals) = model_items(g, catalog, found);
    if providers.iter().chain(&manuals).any(|(id, _)| *id == stored) {
        return stored;
    }
    providers.into_iter().next().map(|(id, _)| id).unwrap_or(stored)
}

/// The resolved manual model an identifier names, among the popup's.
fn manual_named(g: &Globals, identifier: &str, catalog: &Catalog, found: &Discovered) -> Option<Model> {
    let name = identifier.strip_prefix(MANUAL_PREFIX)?;
    ai_model::manual_models(g, catalog, found).into_iter().find(|m| m.name == name)
}

/// A person picked an item (`G:3129-3160`): a provider becomes the default with its
/// recommended model; a dynamic entry's discovered model with its own settings, by its
/// resolved name (`G:2944-2970`); another manual model with its configuration's.
pub fn pick(g: &mut Globals, identifier: &str, catalog: &Catalog, found: &Discovered) {
    if let Some(raw) = identifier.strip_prefix(PROVIDER_PREFIX) {
        if let Some(v) = raw.parse().ok().and_then(Vendor::from_raw) {
            am::select_provider(g, v, catalog);
        }
        return;
    }
    let Some(m) = manual_named(g, identifier, catalog, found) else { return };
    if m.wire_model_name.is_some() {
        am::select_model(g, &m);
    } else if let Some(c) = am::settings_manual_models(g).into_iter().find(|c| c.name == m.name) {
        am::select_manual(g, &c);
    }
}

/// A person picked an item in the popup: `pick`, and when that made the Ollama vendor the
/// default, the probe of the local server with the default to restore if it fails or is
/// cancelled (`G:3113-3127`). Only this interactive pick probes, never a load or a
/// programmatic change.
pub fn pick_interactive(g: &mut Globals, identifier: &str, catalog: &Catalog, found: &Discovered) -> Option<(AiRequest, DefaultSnapshot)> {
    let prior = default_snapshot(g);
    pick(g, identifier, catalog, found);
    let ollama = identifier == format!("{PROVIDER_PREFIX}{}", Vendor::Llama.raw());
    ollama.then(|| (AiRequest::Discover { endpoint: am::OLLAMA_ENDPOINT.to_string(), headers: Vec::new(), force: true }, prior))
}

/// The default-model settings a failed Ollama probe restores (`G:3167-3178`).
#[derive(Clone, Debug, PartialEq)]
pub struct DefaultSnapshot {
    use_recommended: bool,
    vendor: i64,
    model: String,
}

pub fn default_snapshot(g: &Globals) -> DefaultSnapshot {
    DefaultSnapshot { use_recommended: g.bool(ai::USE_RECOMMENDED), vendor: g.int(ai::VENDOR), model: g.string(ai::MODEL) }
}

/// Puts the snapshot back, then the model's settings follow it (`aiModelDidChange`).
pub fn restore_default(g: &mut Globals, s: &DefaultSnapshot, catalog: &Catalog) {
    g.set(ai::USE_RECOMMENDED, Value::Boolean(s.use_recommended));
    g.set(ai::VENDOR, Value::Integer(s.vendor));
    g.set(ai::MODEL, Value::String(s.model.clone()));
    am::model_did_change(g, catalog);
}

/// The Ollama vendor's Regular and Budget pickers, in place of the key hint while it is the
/// default (`G:2762-2833`): the default endpoint's discovered tags sorted, Budget's first item
/// "Same as regular model"; with none, one disabled "No models found" item in each. The shown
/// items are the stored picks, else the first item; showing writes nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct OllamaPickers {
    pub regular: Vec<String>,
    pub budget: Vec<String>,
    pub regular_at: usize,
    pub budget_at: usize,
    pub enabled: bool,
}

pub const REGULAR_LABEL: &str = "Regular:";
pub const BUDGET_LABEL: &str = "Budget:";

pub fn ollama_pickers(g: &Globals, found: &Discovered) -> OllamaPickers {
    let mut names: Vec<String> = found.default_models().iter().map(|m| m.name.clone()).collect();
    names.sort();
    if names.is_empty() {
        let none = vec![ad::NO_MODELS_FOUND.to_string()];
        return OllamaPickers { regular: none.clone(), budget: none, regular_at: 0, budget_at: 0, enabled: false };
    }
    let at = |key: &str| {
        let chosen = g.string(key);
        names.iter().position(|n| !chosen.is_empty() && *n == chosen)
    };
    let regular_at = at(ai::OLLAMA_REGULAR).unwrap_or(0);
    let budget_at = at(ai::OLLAMA_ECONOMY).map_or(0, |i| i + 1);
    let budget = std::iter::once(ad::SAME_AS_REGULAR.to_string()).chain(names.iter().cloned()).collect();
    OllamaPickers { regular: names, budget, regular_at, budget_at, enabled: true }
}

/// Regular's item `i` was picked (`G:2815-2818`).
pub fn pick_regular(g: &mut Globals, p: &OllamaPickers, i: usize) {
    if let Some(name) = p.regular.get(i).filter(|_| p.enabled) {
        g.set(ai::OLLAMA_REGULAR, Value::String(name.clone()));
    }
}

/// Budget's item `i` was picked: "Same as regular model" stores "" (`G:2820-2828`).
pub fn pick_budget(g: &mut Globals, p: &OllamaPickers, i: usize) {
    if !p.enabled {
        return;
    }
    let name = if i == 0 { String::new() } else { p.budget.get(i).cloned().unwrap_or_default() };
    g.set(ai::OLLAMA_ECONOMY, Value::String(name));
}

/// The line under the popup saying which key authorizes the shown model (`G:3306-3358`).
/// None for the Ollama vendor, whose Regular and Budget pickers take the space; a manual model
/// at a local endpoint never receives a key; otherwise the vendor's key, set or not, or no key
/// at all.
pub fn key_hint(g: &Globals, shown: &str, catalog: &Catalog, found: &Discovered, key_set: &dyn Fn(Vendor) -> bool) -> Option<String> {
    let manual = manual_named(g, shown, catalog, found);
    let vendor = match (shown.strip_prefix(PROVIDER_PREFIX), &manual) {
        (Some(raw), _) => raw.parse().ok().and_then(Vendor::from_raw),
        (None, Some(m)) => Some(m.vendor),
        (None, None) => None,
    }
    .or_else(|| Vendor::from_raw(g.int(ai::VENDOR)));
    if manual.is_none() && vendor == Some(Vendor::Llama) {
        return None;
    }
    if manual.as_ref().is_some_and(|m| ai_policy::key_policy(&m.url, m.api) == KeyPolicy::LocalEndpoint) {
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

/// Where the default model's requests go and whether the person allowed them (ruling 1), with
/// the discovered Ollama models, as the request resolves it.
pub fn requests_text(g: &Globals, catalog: &Catalog, found: &Discovered) -> String {
    let Some(e) = default_endpoint(g, catalog, found) else { return ai_gate::NO_ENDPOINT.to_string() };
    let Some(host) = ai_policy::host_of(&e.url) else { return ai_gate::NO_ENDPOINT.to_string() };
    let provider = e.vendor.provider_name();
    if ai_gate::consented(g, &host) {
        format!("Requests go to {host} ({provider}). You allowed them.")
    } else {
        format!("Requests go to {host} ({provider}). Tandem asks before the first one.")
    }
}

/// Test Connection pressed (`G:1460-1510`): the form's trimmed URL and name (in auto-discovery
/// mode the popup's choice, else the first discovered tag), `missing` first (Err is its text for
/// a "Missing Information" alert), then the consent gate for the probe's own endpoint: anything
/// but Pass asks the App to show the gate's alert, and the test does not start (after Allow the
/// person presses again, Decision 16); otherwise the App builds the tester.
pub fn test_click(g: &Globals, form: &EditorForm, found: &Discovered) -> Result<AiRequest, (&'static str, &'static str)> {
    let url = form.url.trim().to_string();
    let headers = form.named_headers();
    let cached = if form.dynamic { ad::names_at(found, &url, &headers).into_iter().next() } else { None };
    let chosen = if form.dynamic { form.selected.trim() } else { form.name.trim() };
    if let Some(text) = tc::missing(chosen, &url, form.dynamic, cached.as_deref()) {
        return Err((tc::MISSING_TITLE, text));
    }
    // A URL with no host sends nothing, so it needs no gate (whose refusal would name the
    // default model): "The URL is not valid." at once, as iTerm2's test ends.
    if crate::ai_policy::host_of(&url).is_none() {
        return Err((tc::FAILED_TITLE, tc::BAD_URL));
    }
    let name = if chosen.is_empty() { cached.unwrap_or_default() } else { chosen.to_string() };
    let probe = tc::Probe { name, url, api: Api::from_raw_or_chat(form.api), function_calling: form.function_calling, supports_temperature: form.temperature, headers };
    Ok(match ai_gate::alert(&ai_gate::check_endpoint(g, Some(&tc::endpoint(&probe)))) {
        Some(alert) => AiRequest::Alert(alert),
        None => AiRequest::BuildTester(probe),
    })
}

/// Drives the editor's Test Connection each frame (`G:1511-1540`): it starts with the vendor's
/// stored key, asks the App for a key when one is needed, takes the prompt's answer, polls, and
/// shows the outcome over the editor ("Connection Succeeded" or "Connection Failed"; nothing
/// when the key prompt was refused). Closing the editor drops the test.
fn drive_test(st: &mut State, g: &Globals, now: u64) {
    let Some(t) = st.tester.as_mut() else {
        // A key answered after its test ended is never used by a later one.
        st.test_key = None;
        return;
    };
    let Some(e) = st.editor.as_mut() else {
        st.tester = None;
        st.test_key = None;
        return;
    };
    let outcome = if !t.started() {
        // One request for the App per frame: a start that needs a key waits for a free slot.
        if st.pending.is_some() {
            return;
        }
        let endpoint = tc::endpoint(t.probe());
        // The gate was checked when the test was pressed; it is asked again just before the
        // test's request is sent (12b final Minor 2), and a closed gate ends the test unsent.
        let Some(proof) = crate::chat_gate::pass(g, &endpoint) else {
            st.tester = None;
            e.testing = false;
            return;
        };
        let stored = keys().get(endpoint.vendor, &mut *ai_keys::system_store());
        let o = t.start(proof, stored.as_ref());
        if let Some(v) = t.needs_key() {
            st.pending = Some(AiRequest::AskKey(v, tc::endpoint(t.probe())));
        }
        o
    } else if t.needs_key().is_some() {
        let Some(proof) = crate::chat_gate::pass(g, &tc::endpoint(t.probe())) else {
            st.tester = None;
            st.test_key = None;
            e.testing = false;
            return;
        };
        st.test_key.take().and_then(|k| t.key_given(proof, k))
    } else {
        t.poll()
    };
    let Some(outcome) = outcome else { return };
    st.tester = None;
    e.testing = false;
    match outcome {
        tc::Outcome::Canceled => {}
        tc::Outcome::Success(text) => e.alert = Some((tc::SUCCEEDED_TITLE.to_string(), text, now)),
        tc::Outcome::Failure(text) => e.alert = Some((tc::FAILED_TITLE.to_string(), text, now)),
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
fn open_key_dialog(opened: u64) -> KeyDialog {
    let mut store = ai_keys::system_store();
    let mut cache = keys();
    let initial: Vec<Secret> = Vendor::KEY_ORDER.iter().map(|v| cache.get(*v, &mut *store).unwrap_or_else(|| Secret::new(String::new()))).collect();
    let now = initial.iter().map(Secret::duplicate).collect();
    KeyDialog { initial, now, focus: true, opened }
}

/// OK: writes the changed fields. A failed write keeps the key for the session (`Keys::set`)
/// and becomes the settings window's error text, one line per failed vendor (Tandem's; iTerm2
/// logs each failure, `KC:257-275`).
fn save_keys(d: &KeyDialog, cache: &mut Keys, store: &mut dyn KeyStore) -> Option<String> {
    let failures: Vec<String> = changed_fields(&d.initial, &d.now)
        .into_iter()
        .filter_map(|i| {
            let v = Vendor::KEY_ORDER[i];
            cache.set(v, &d.now[i], store).err().map(|e| format!("Could not save the {} API key: {}", v.provider_name(), e))
        })
        .collect();
    (!failures.is_empty()).then(|| failures.join("\n"))
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
        // A key field keeps the focus on Return, which is still OK there.
        let fields: Vec<egui::Id> = (0..Vendor::KEY_ORDER.len()).map(|i| egui::Id::new(("ai-key", i))).collect();
        let (enter, escape) = modal_keys(ui, d.opened, &fields);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ok = ui.add(egui::Button::new("OK").min_size(egui::vec2(76.0, 24.0))).clicked();
            let cancel = ui.add(egui::Button::new("Cancel").min_size(egui::vec2(76.0, 24.0))).clicked();
            if ok || enter {
                *error = save_keys(d, &mut keys(), &mut *ai_keys::system_store());
                done = true;
            } else if cancel || escape {
                done = true;
            }
        });
    });
    done
}

/// A failed key write beside the button that opens the dialog: the warning glyph (when the icon
/// font loaded) and the text, in the warning color.
fn failure_mark(ui: &mut egui::Ui, text: &str) {
    let color = ui.visuals().warn_fg_color;
    if crate::settings_keys::has_icon_font(ui.ctx()) {
        ui.label(egui::RichText::new(crate::scripts::WARNING_GLYPH.to_string()).family(crate::settings_keys::icon_family()).color(color));
    }
    ui.add(egui::Label::new(egui::RichText::new(text).color(color)).wrap());
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
            ui.label(requests_text(g, catalog, &st.found));
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
        ui.horizontal(|ui| {
            if ui.add_enabled(on.rest, egui::Button::new(SET_API_KEY)).clicked() {
                st.key_failure = None;
                st.keys = Some(open_key_dialog(ui.ctx().cumulative_frame_nr()));
            }
            if let Some(f) = &st.key_failure {
                failure_mark(ui, f);
            }
        });
        ui.end_row();

        row_label(ui, MODEL_LABEL);
        ui.vertical(|ui| {
            let shown = shown_identifier(g, catalog, &st.found);
            let (providers, manuals) = model_items(g, catalog, &st.found);
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
                if ui.add_enabled(on.rest, egui::Button::new(MANAGE_MODELS)).clicked() {
                    let (sheet, noted) = open_sheet(g, ui.ctx().cumulative_frame_nr());
                    st.sheet = Some(sheet);
                    st.editor = None;
                    changed |= noted;
                }
            });
            if let Some(id) = picked.filter(|id| *id != shown) {
                if let Some((request, prior)) = pick_interactive(g, &id, catalog, &st.found) {
                    st.probe = None;
                    st.pending = Some(request);
                    st.probing = Some(ProbeSheet { prior, opened: ui.ctx().cumulative_frame_nr() });
                }
                changed = true;
            }
            if ai_model::ollama_vendor_is_default(g) {
                changed |= ollama_picker_rows(ui, g, &st.found, on.rest);
            } else {
                let shown = shown_identifier(g, catalog, &st.found);
                let store = std::cell::RefCell::new(ai_keys::system_store());
                let key_set = |v: Vendor| keys().is_set(v, &mut **store.borrow_mut());
                if let Some(hint) = key_hint(g, &shown, catalog, &st.found, &key_set) {
                    ui.add(egui::Label::new(egui::RichText::new(hint.as_str()).weak()).truncate()).on_hover_text(hint);
                }
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
            st.key_failure = st.error.clone();
        }
    }
    // The App's answer goes to whoever asked: the probe sheet, else the editor's fetch.
    let now = ui.ctx().cumulative_frame_nr();
    if st.probing.is_none() {
        if let Some(o) = st.probe.take() {
            if let Some(e) = st.editor.as_mut() {
                e.take_answer(o, &st.found, now);
            }
        }
    }
    if let Some(sh) = st.sheet.as_mut() {
        let (closed, sheet_changed) = sheet_modal(ui.ctx(), g, sh, &mut st.editor, catalog, &st.found);
        changed |= sheet_changed;
        if closed {
            st.sheet = None;
            st.editor = None;
        }
    }
    if let Some(request) = st.editor.as_mut().and_then(|e| e.request.take()) {
        if matches!(request, AiRequest::Discover { .. }) {
            st.probe = None;
        }
        st.pending = Some(request);
    }
    drive_test(st, g, now);
    if let Some(p) = &st.probing {
        let canceled = probe_modal(ui.ctx(), p.opened);
        let answer = if canceled { Some(Probe::Restore) } else { st.probe.take().map(|o| ad::probe_outcome(&o, false)) };
        if let Some(answer) = answer {
            let prior = st.probing.take().map(|p| p.prior);
            match (answer, prior) {
                (Probe::Keep, _) | (_, None) => {}
                (Probe::Restore, Some(prior)) => restore_default(g, &prior, catalog),
                (Probe::RestoreAndTell(text), Some(prior)) => {
                    restore_default(g, &prior, catalog);
                    st.tell = Some((text, now));
                }
            }
            changed = true;
        }
    }
    if let Some((text, opened)) = &st.tell {
        if notice_modal(ui.ctx(), "ai-ollama-unreachable", ad::UNREACHABLE_TITLE, text, now != *opened) {
            st.tell = None;
        }
    }
    changed
}

/// The Regular and Budget pickers under the model popup while the Ollama vendor is the default
/// (`G:2762-2828`). True when a pick changed a setting.
fn ollama_picker_rows(ui: &mut egui::Ui, g: &mut Globals, found: &Discovered, enabled: bool) -> bool {
    let p = ollama_pickers(g, found);
    let mut changed = false;
    for (label, items, at, id) in [(REGULAR_LABEL, &p.regular, p.regular_at, "ai-ollama-regular"), (BUDGET_LABEL, &p.budget, p.budget_at, "ai-ollama-budget")] {
        ui.horizontal(|ui| {
            ui.label(label);
            let mut picked = None;
            ui.add_enabled_ui(enabled && p.enabled, |ui| {
                egui::ComboBox::from_id_salt(id).width(260.0).selected_text(items[at].as_str()).show_ui(ui, |ui| {
                    for (i, item) in items.iter().enumerate() {
                        if ui.selectable_label(i == at, item.as_str()).clicked() {
                            picked = Some(i);
                        }
                    }
                });
            });
            if let Some(i) = picked.filter(|i| *i != at) {
                if id == "ai-ollama-regular" {
                    pick_regular(g, &p, i);
                } else {
                    pick_budget(g, &p, i);
                }
                changed = true;
            }
        });
    }
    changed
}

/// The Ollama probe sheet (`G:3055-3104`): the title, the text, a spinner and Cancel (Return or
/// Escape). True when cancelled.
fn probe_modal(ctx: &egui::Context, opened: u64) -> bool {
    let mut cancel = false;
    egui::Modal::new(egui::Id::new("ai-ollama-probe")).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.strong(ad::PROBE_TITLE);
        ui.add_space(4.0);
        ui.add(egui::Label::new(ad::PROBE_BODY).wrap());
        ui.add_space(8.0);
        ui.add(egui::Spinner::new().size(18.0));
        ui.add_space(8.0);
        let (enter, escape) = modal_keys(ui, opened, &[]);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new(am::CANCEL).min_size(egui::vec2(76.0, 24.0))).clicked() || enter || escape {
                cancel = true;
            }
        });
    });
    cancel
}

/// "Manage AI Models\u{2026}" (`G:4032-4046`): the sheet over the configurations (else the
/// legacy model), the first row selected; the legacy headers notice first when it is due, which
/// sets its flag. True when the flag was set.
fn open_sheet(g: &mut Globals, opened: u64) -> (Sheet, bool) {
    let notice = am::legacy_notice_due(g);
    if notice {
        g.set(am::LEGACY_NOTICE_FLAG, Value::Boolean(true));
    }
    let models = am::sheet_models(g);
    let selected = (!models.is_empty()).then_some(0);
    (Sheet { models, selected, notice, opened }, notice)
}

/// Return pressed without modifiers (`fresh_return`), and Escape pressed.
fn enter_escape(ui: &egui::Ui) -> (bool, bool) {
    ui.input(|i| (fresh_return(&i.events), i.key_pressed(egui::Key::Escape)))
}

/// A press of Return without modifiers that is not a key repeat, so a held Return acts on a
/// modal once and never saves it again when the repeat starts.
pub fn fresh_return(events: &[egui::Event]) -> bool {
    events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Enter, pressed: true, repeat: false, modifiers, .. } if modifiers.is_none()))
}

/// Whether Return and Escape act as a modal's default buttons. Neither acts in the frame the
/// modal opened, where the key that opened it is still down (egui turns Return or Space on a
/// focused button into a click). Return does not act while a button, checkbox or menu has
/// the focus, since egui clicks that widget with it instead.
pub fn default_keys(opening: bool, enter: bool, escape: bool, focus_takes_return: bool) -> (bool, bool) {
    (enter && !opening && !focus_takes_return, escape && !opening)
}

/// `default_keys` for a modal drawn in `ui` that opened in frame `opened`. `keeps` names the
/// fields that keep the focus on Return (the key fields) and still let it act; egui's text
/// fields give the focus up on Return, so they need no entry.
pub fn modal_keys(ui: &egui::Ui, opened: u64, keeps: &[egui::Id]) -> (bool, bool) {
    let (enter, escape) = enter_escape(ui);
    let ctx = ui.ctx();
    let focus_takes_return = ctx.memory(|m| m.focused()).filter(|id| !keeps.contains(id)).and_then(|id| ctx.read_response(id)).is_some_and(|r| r.enabled() && r.sense.senses_click());
    default_keys(ctx.cumulative_frame_nr() == opened, enter, escape, focus_takes_return)
}

/// An alert over the settings window: the heading, the text and OK (Return or Escape). True
/// when it closed.
fn notice_modal(ctx: &egui::Context, id: &str, heading: &str, text: &str, keys: bool) -> bool {
    let mut done = false;
    egui::Modal::new(egui::Id::new(id)).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.strong(heading);
        ui.add_space(4.0);
        ui.add(egui::Label::new(text).wrap());
        ui.add_space(10.0);
        let (enter, escape) = enter_escape(ui);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new("OK").min_size(egui::vec2(76.0, 24.0))).clicked() || (keys && (enter || escape)) {
                done = true;
            }
        });
    });
    done
}

/// One of the sheet's segment buttons: a Segoe Fluent Icons glyph (the tooltip's word when the
/// icon font is missing) with iTerm2's tooltip.
fn segment(ui: &mut egui::Ui, icons: bool, glyph: char, tip: &str, enabled: bool) -> bool {
    let text = if icons { egui::RichText::new(glyph.to_string()).family(crate::settings_keys::icon_family()) } else { egui::RichText::new(tip) };
    ui.add_enabled(enabled, egui::Button::new(text).min_size(egui::vec2(34.0, 24.0))).on_hover_text(tip).on_disabled_hover_text(tip).clicked()
}

/// What a sheet button asked for.
enum SheetAct {
    Add,
    Delete(usize),
    Edit(usize),
    Duplicate(usize),
    Default(usize),
    Economy(usize),
}

/// The sheet's columns: Model, API, Endpoint (`G:316-320`).
const COLUMNS: [f32; 3] = [260.0, 130.0, 150.0];

/// The Manual AI Models sheet and, over it, the model editor. Answers (closed, changed).
fn sheet_modal(ctx: &egui::Context, g: &mut Globals, sh: &mut Sheet, editor: &mut Option<Editor>, catalog: &Catalog, found: &Discovered) -> (bool, bool) {
    if sh.notice {
        if notice_modal(ctx, "ai-legacy-headers", am::LEGACY_HEADING, am::LEGACY_TEXT, ctx.cumulative_frame_nr() != sh.opened) {
            sh.notice = false;
        }
        return (false, false);
    }
    let default_name = am::default_manual_name(g);
    let rows = am::sheet_rows(&sh.models, default_name.as_deref(), &am::economy_name(g));
    let enabled = am::segments_enabled(&sh.models, sh.selected, default_name.as_deref());
    let mut act = None;
    let mut done = false;
    let editing = editor.is_some();
    egui::Modal::new(egui::Id::new("ai-manual-models")).show(ctx, |ui| {
        let width: f32 = COLUMNS.iter().sum();
        ui.set_width(width);
        ui.strong(am::SHEET_TITLE);
        ui.add_space(8.0);
        let (head, _) = ui.allocate_exact_size(egui::vec2(width, 20.0), egui::Sense::hover());
        let strong = ui.visuals().strong_text_color();
        let mut x = head.left() + 4.0;
        for (title, w) in [am::COLUMN_MODEL, am::COLUMN_API, am::COLUMN_ENDPOINT].into_iter().zip(COLUMNS) {
            ui.painter().text(egui::pos2(x, head.center().y), egui::Align2::LEFT_CENTER, title, egui::FontId::proportional(13.0), strong);
            x += w;
        }
        egui::ScrollArea::vertical().id_salt("ai-manual-models-rows").max_height(200.0).auto_shrink([false, true]).show(ui, |ui| {
            for (i, row) in rows.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 24.0), egui::Sense::click());
                let painter = ui.painter_at(rect);
                if sh.selected == Some(i) {
                    painter.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
                } else if i % 2 == 1 {
                    painter.rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
                }
                let color = ui.visuals().text_color();
                let mut x = rect.left() + 4.0;
                for (text, w) in row.iter().zip(COLUMNS) {
                    let cell = egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x + w - 8.0, rect.bottom()));
                    ui.painter_at(cell).text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(13.0), color);
                    x += w;
                }
                if resp.clicked() {
                    sh.selected = Some(i);
                }
                if resp.double_clicked() {
                    act = Some(SheetAct::Edit(i));
                }
            }
        });
        ui.add_space(8.0);
        let (enter, _) = modal_keys(ui, sh.opened, &[]);
        ui.horizontal(|ui| {
            let icons = crate::settings_keys::has_icon_font(ui.ctx());
            let row = sh.selected.unwrap_or(0);
            if segment(ui, icons, '\u{E710}', am::ADD, enabled[0]) {
                act = Some(SheetAct::Add);
            }
            if segment(ui, icons, '\u{E738}', am::DELETE, enabled[1]) {
                act = Some(SheetAct::Delete(row));
            }
            ui.add_space(12.0);
            if segment(ui, icons, '\u{E70F}', am::EDIT, enabled[2]) {
                act = Some(SheetAct::Edit(row));
            }
            if segment(ui, icons, '\u{E8C8}', am::DUPLICATE, enabled[3]) {
                act = Some(SheetAct::Duplicate(row));
            }
            if segment(ui, icons, '\u{E734}', am::TOGGLE_DEFAULT, enabled[4]) {
                act = Some(SheetAct::Default(row));
            }
            let economy = egui::Button::new(am::ECONOMY).min_size(egui::vec2(34.0, 24.0));
            if ui.add_enabled(enabled[5], economy).on_hover_text(am::TOGGLE_ECONOMY).on_disabled_hover_text(am::TOGGLE_ECONOMY).clicked() {
                act = Some(SheetAct::Economy(row));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new("OK").min_size(egui::vec2(90.0, 30.0))).clicked() || (enter && !editing && act.is_none()) {
                    done = true;
                }
            });
        });
    });
    let mut changed = false;
    let now = ctx.cumulative_frame_nr();
    match act {
        Some(SheetAct::Add) => *editor = Some(Editor::new(am::default_base(g), None, catalog, now)),
        Some(SheetAct::Edit(i)) if i < sh.models.len() => *editor = Some(Editor::new(sh.models[i].clone(), Some(i), catalog, now)),
        Some(SheetAct::Duplicate(i)) if i < sh.models.len() => *editor = Some(Editor::new(am::duplicate_base(&sh.models[i]), None, catalog, now)),
        Some(SheetAct::Delete(i)) => {
            sh.selected = am::delete_row(g, &mut sh.models, i, catalog);
            changed = true;
        }
        Some(SheetAct::Default(i)) => {
            am::toggle_default(g, &sh.models, i, catalog);
            changed = true;
        }
        Some(SheetAct::Economy(i)) => {
            am::toggle_economy(g, &sh.models, i);
            changed = true;
        }
        _ => {}
    }
    if let Some(e) = editor.as_mut() {
        match editor_modal(ctx, g, e, &sh.models, catalog, found) {
            Some(Some(m)) => {
                sh.selected = Some(am::save_edit(g, &mut sh.models, e.editing, m, catalog));
                *editor = None;
                changed = true;
            }
            Some(None) => *editor = None,
            None => {}
        }
    }
    (done, changed)
}

/// A right-aligned label in the editor's first column (`G:672-677`).
fn editor_label(ui: &mut egui::Ui, text: &str) {
    ui.allocate_ui_with_layout(egui::vec2(150.0, 20.0), egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(text));
}

/// The editor's field column width (`G:662-664`).
const FIELD_WIDTH: f32 = 400.0;

/// The model editor (`G:645-1640`). None while open; Some(None) on Cancel; Some(Some(model))
/// when Save passed its checks. Test Connection hands its request to the App (`test_click`).
fn editor_modal(ctx: &egui::Context, g: &Globals, e: &mut Editor, models: &[ManualModel], catalog: &Catalog, found: &Discovered) -> Option<Option<ManualModel>> {
    let mut out = None;
    let blocked = e.failure.is_some() || e.alert.is_some() || e.menu.is_some();
    let now = ctx.cumulative_frame_nr();
    egui::Modal::new(egui::Id::new("ai-model-editor")).show(ctx, |ui| {
        ui.set_width(150.0 + 12.0 + FIELD_WIDTH);
        ui.strong(if e.editing.is_some() { am::EDIT_TITLE } else { am::ADD_TITLE });
        ui.add_space(10.0);
        let form = &mut e.form;
        egui::Grid::new("ai-model-editor-grid").num_columns(2).spacing(egui::vec2(12.0, 6.0)).show(ui, |ui| {
            editor_label(ui, am::PRESET_LABEL);
            let mut picked = None;
            egui::ComboBox::from_id_salt("ai-model-preset").width(FIELD_WIDTH).selected_text(preset_title(form.preset, catalog)).show_ui(ui, |ui| {
                if ui.selectable_label(form.preset == am::CUSTOM_PRESET, am::CUSTOM).clicked() {
                    picked = Some(am::CUSTOM_PRESET);
                }
                ui.separator();
                for (i, m) in am::catalog_presets(catalog).iter().enumerate() {
                    let tag = i as i64;
                    if ui.selectable_label(form.preset == tag, m.name.as_str()).clicked() {
                        picked = Some(tag);
                    }
                }
                ui.separator();
                for (i, p) in am::PROVIDER_PRESETS.iter().enumerate() {
                    let tag = am::PROVIDER_PRESET_BASE + i as i64;
                    if ui.selectable_label(form.preset == tag, p.name).clicked() {
                        picked = Some(tag);
                    }
                }
            });
            if let Some(tag) = picked {
                form.preset = tag;
                if tag >= am::PROVIDER_PRESET_BASE {
                    if let Some(p) = usize::try_from(tag - am::PROVIDER_PRESET_BASE).ok().and_then(|i| am::PROVIDER_PRESETS.get(i)) {
                        am::apply_provider_preset(form, p);
                    }
                } else if let Some(m) = usize::try_from(tag).ok().and_then(|i| am::catalog_presets(catalog).get(i).copied()) {
                    am::apply_catalog_preset(form, m);
                }
            }
            ui.end_row();

            editor_label(ui, am::EDITOR_MODEL_LABEL);
            ui.horizontal(|ui| {
                if form.dynamic {
                    // Auto-discovery replaces the Model field with the discovered models of the
                    // URL; a new URL clears a choice made for the old server (`G:1026-1070`).
                    let url = form.url.trim().to_string();
                    if e.popup_url.as_ref().is_some_and(|u| *u != url) {
                        form.selected.clear();
                    }
                    e.popup_url = Some(url.clone());
                    let items = am::dynamic_popup(&ad::names_at(found, &url, &form.named_headers()), &form.selected);
                    let title = items.iter().find(|(_, v)| v.as_deref() == Some(form.selected.as_str())).map_or(ad::ALL_INSTALLED, |(t, _)| t.as_str()).to_string();
                    egui::ComboBox::from_id_salt("ai-model-dynamic").width(FIELD_WIDTH - 144.0).selected_text(title).show_ui(ui, |ui| {
                        for (i, (t, v)) in items.iter().enumerate() {
                            if i == 1 {
                                ui.separator();
                            }
                            match v {
                                Some(v) => {
                                    if ui.selectable_label(*v == form.selected, t.as_str()).clicked() {
                                        form.selected = v.clone();
                                    }
                                }
                                None => {
                                    ui.add_enabled(false, egui::Button::selectable(false, t.as_str()));
                                }
                            }
                        }
                    });
                    e.focus_name = false;
                } else {
                    let name = ui.add(egui::TextEdit::singleline(&mut form.name).hint_text(form.name_placeholder.as_str()).desired_width(FIELD_WIDTH - 136.0));
                    if std::mem::take(&mut e.focus_name) {
                        name.request_focus();
                    }
                }
                let fetch = if e.fetching.is_some() {
                    ad::FETCHING
                } else if form.dynamic {
                    am::REFRESH_MODELS
                } else {
                    am::FETCH_MODELS
                };
                if ui.add_enabled(e.fetching.is_none(), egui::Button::new(fetch).min_size(egui::vec2(130.0, 0.0))).clicked() {
                    // `G:1347-1425`: the URL is required; the App asks the server, forced so a
                    // background refresh cannot swallow this one.
                    let url = form.url.trim().to_string();
                    if url.is_empty() {
                        e.alert = Some((ad::MISSING_URL_TITLE.to_string(), ad::MISSING_URL_BODY.to_string(), now));
                    } else {
                        let headers = form.named_headers();
                        e.request = Some(AiRequest::Discover { endpoint: url.clone(), headers: headers.clone(), force: true });
                        e.fetching = Some((form.dynamic, url, headers));
                    }
                }
            });
            ui.end_row();

            editor_label(ui, am::URL_LABEL);
            ui.add(egui::TextEdit::singleline(&mut form.url).desired_width(FIELD_WIDTH));
            ui.end_row();

            editor_label(ui, am::API_LABEL);
            ui.add_enabled_ui(!form.dynamic, |ui| {
                egui::ComboBox::from_id_salt("ai-model-api").width(FIELD_WIDTH).selected_text(Api::from_raw_or_chat(form.api).title()).show_ui(ui, |ui| {
                    for api in Api::EDITOR_ORDER {
                        ui.selectable_value(&mut form.api, api.raw(), api.title());
                    }
                });
            });
            ui.end_row();

            ui.label("");
            ui.vertical(|ui| {
                let mut discover = form.dynamic;
                if ui.checkbox(&mut discover, am::DISCOVER).changed() {
                    form.set_dynamic(discover);
                }
                let hint = editor_key_hint(form);
                ui.add(egui::Label::new(egui::RichText::new(hint.as_str()).small().weak()).wrap()).on_hover_text(hint);
            });
            ui.end_row();

            editor_label(ui, am::CONTEXT_LABEL);
            ui.add_enabled(!form.dynamic, egui::TextEdit::singleline(&mut form.context).desired_width(120.0));
            ui.end_row();
            editor_label(ui, am::RESPONSE_LABEL);
            ui.add_enabled(!form.dynamic, egui::TextEdit::singleline(&mut form.response).desired_width(120.0));
            ui.end_row();

            editor_label(ui, am::VECTOR_STORE_LABEL);
            let store = if form.vector_store == 1 { am::VECTOR_OPENAI } else { am::VECTOR_DISABLED };
            egui::ComboBox::from_id_salt("ai-model-vector-store").width(FIELD_WIDTH).selected_text(store).show_ui(ui, |ui| {
                ui.selectable_value(&mut form.vector_store, 0, am::VECTOR_DISABLED);
                ui.selectable_value(&mut form.vector_store, 1, am::VECTOR_OPENAI);
            });
            ui.end_row();

            ui.label("");
            ui.vertical(|ui| {
                for (i, (title, _)) in am::FEATURES.iter().enumerate() {
                    ui.add_enabled(!form.dynamic, egui::Checkbox::new(form.feature_mut(i), *title));
                }
                ui.add_enabled(!form.dynamic, egui::Checkbox::new(&mut form.thinking, am::THINKING)).on_hover_text(am::THINKING_TIP).on_disabled_hover_text(am::THINKING_TIP);
                ui.checkbox(&mut form.temperature, am::TEMPERATURE).on_hover_text(am::TEMPERATURE_TIP);
            });
            ui.end_row();

            editor_label(ui, am::HEADERS_LABEL);
            ui.vertical(|ui| {
                egui::ScrollArea::vertical().id_salt("ai-model-headers").max_height(120.0).auto_shrink([false, true]).show(ui, |ui| {
                    egui::Grid::new("ai-model-headers-grid").num_columns(2).striped(true).show(ui, |ui| {
                        ui.strong(am::HEADER_COLUMN);
                        ui.strong(am::VALUE_COLUMN);
                        ui.end_row();
                        let focus = e.focus_header.take();
                        for (i, (n, v)) in form.headers.iter_mut().enumerate() {
                            let rn = ui.add(egui::TextEdit::singleline(n).desired_width(170.0));
                            let rv = ui.add(egui::TextEdit::singleline(v).desired_width(FIELD_WIDTH - 170.0 - 40.0));
                            if focus == Some(i) {
                                rn.request_focus();
                            }
                            if rn.has_focus() || rv.has_focus() {
                                e.header = Some(i);
                            }
                            ui.end_row();
                        }
                    });
                });
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new("+").min_size(egui::vec2(34.0, 22.0))).clicked() {
                        form.headers.push((String::new(), String::new()));
                        e.header = Some(form.headers.len() - 1);
                        e.focus_header = e.header;
                    }
                    let selected = e.header.filter(|i| *i < form.headers.len());
                    if ui.add_enabled(selected.is_some(), egui::Button::new("\u{2212}").min_size(egui::vec2(34.0, 22.0))).clicked() {
                        if let Some(i) = selected {
                            form.headers.remove(i);
                            e.header = None;
                        }
                    }
                });
            });
            ui.end_row();
        });
        ui.add_space(12.0);
        let (enter, escape) = modal_keys(ui, e.opened, &[]);
        ui.horizontal(|ui| {
            // `G:1497-1510`: disabled with a spinner while the test runs.
            let title = if e.testing { tc::TESTING } else { am::TEST_CONNECTION };
            if ui.add_enabled(!e.testing, egui::Button::new(title).min_size(egui::vec2(150.0, 30.0))).clicked() {
                match test_click(g, &e.form, found) {
                    Err((title, text)) => e.alert = Some((title.to_string(), text.to_string(), now)),
                    Ok(request) => {
                        e.testing = matches!(request, AiRequest::BuildTester(_));
                        e.request = Some(request);
                    }
                }
            }
            if e.testing {
                ui.add(egui::Spinner::new().size(16.0));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // iTerm2 titles the button Add while adding (`G:927-929`).
                let title = if e.editing.is_some() { am::SAVE } else { am::ADD };
                let save = ui.add(egui::Button::new(title).min_size(egui::vec2(100.0, 30.0))).clicked();
                let cancel = ui.add(egui::Button::new(am::CANCEL).min_size(egui::vec2(100.0, 30.0))).clicked();
                if cancel || (escape && !blocked) {
                    out = Some(None);
                } else if save || (enter && !blocked) {
                    let taken = |name: &str| models.iter().enumerate().any(|(i, m)| Some(i) != e.editing && m.name == name);
                    match e.form.validate(&e.base, &taken) {
                        Ok(m) => out = Some(Some(m)),
                        Err(msg) => e.failure = Some(msg),
                    }
                }
            });
        });
    });
    if let Some(msg) = e.failure.clone() {
        if notice_modal(ctx, "ai-model-invalid", am::INVALID_TITLE, &msg, blocked) {
            e.failure = None;
        }
    }
    if let Some((title, text, opened)) = e.alert.clone() {
        if notice_modal(ctx, "ai-model-fetch", &title, &text, now != opened) {
            e.alert = None;
        }
    }
    if let Some(names) = e.menu.clone() {
        if let Some(picked) = names_menu(ctx, &names) {
            e.menu = None;
            if let Some(name) = picked {
                e.form.name = name;
            }
        }
    }
    out
}

/// Fetch Models' names (`G:1427-1445`): picking one fills the Model field. None while open;
/// Some(None) when dismissed.
fn names_menu(ctx: &egui::Context, names: &[String]) -> Option<Option<String>> {
    let mut out = None;
    let shown = egui::Modal::new(egui::Id::new("ai-model-names")).show(ctx, |ui| {
        ui.set_width(300.0);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for n in names {
                if ui.selectable_label(false, n.as_str()).clicked() {
                    out = Some(Some(n.clone()));
                }
            }
        });
    });
    // A click outside or Escape dismisses it, as a menu.
    if out.is_none() && shown.should_close() {
        out = Some(None);
    }
    out
}

/// The Prompts sub-tab (`X:7471-7580`): the cost note, the prompt selector with the
/// required-variable warning, Reset and "?", and the selected prompt's text, editable while AI
/// is allowed (`G:3285-3297`). True when a setting changed.
fn prompts(ui: &mut egui::Ui, g: &mut Globals, st: &mut State) -> bool {
    let allowed = ai::allowed(g);
    let mut changed = false;
    ui.label(ap::COST_NOTE);
    ui.add_space(4.0);
    let key = st.prompt.key();
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("ai-prompt-selector").selected_text(st.prompt.title()).width(300.0).show_ui(ui, |ui| {
            for (i, group) in ap::SELECTOR.iter().enumerate() {
                if i > 0 {
                    ui.separator();
                }
                for p in group.iter() {
                    if ui.selectable_label(st.prompt == *p, p.title()).clicked() {
                        st.prompt = *p;
                    }
                }
            }
        });
        if let Some(w) = ap::warning(st.prompt, &g.string(st.prompt.key())) {
            let color = ui.visuals().warn_fg_color;
            let mark = if crate::settings_keys::has_icon_font(ui.ctx()) {
                egui::RichText::new(crate::scripts::WARNING_GLYPH.to_string()).family(crate::settings_keys::icon_family())
            } else {
                egui::RichText::new("!").strong()
            };
            ui.label(mark.color(color)).on_hover_text(w);
        }
        if ui.add_enabled(allowed, egui::Button::new(RESET)).clicked() {
            g.set(key, Value::String(ap::default_text(key).unwrap_or_default().to_string()));
            changed = true;
        }
        if ui.button("?").clicked() {
            st.help = Some(ui.ctx().cumulative_frame_nr());
        }
    });
    // The selector may have changed the prompt this frame.
    let key = st.prompt.key();
    let mut text = g.string(key);
    egui::ScrollArea::vertical().id_salt("ai-prompt-text").max_height(320.0).show(ui, |ui| {
        let mut edit = |buffer: &mut dyn egui::TextBuffer| egui::TextEdit::multiline(buffer).id(prompt_text_id(key)).desired_width(f32::INFINITY).desired_rows(14).show(ui).response.changed();
        if allowed {
            if edit(&mut text) {
                g.set(key, Value::String(text.clone()));
                changed = true;
            }
        } else {
            edit(&mut text.as_str());
        }
    });
    if let Some(opened) = st.help {
        if help_modal(ui.ctx(), opened) {
            st.help = None;
        }
    }
    changed
}

/// The prompt text box's id: one per prompt, so each keeps its own undo history and Undo never
/// writes one prompt's text into another's setting.
fn prompt_text_id(key: &str) -> egui::Id {
    egui::Id::new(("ai-prompt-text", key))
}

/// The "?" button's help (`G:4173-4181`): the Markdown as plain text in a scrollable modal
/// with OK (Return or Escape). True when it closed.
fn help_modal(ctx: &egui::Context, opened: u64) -> bool {
    let mut done = false;
    egui::Modal::new(egui::Id::new("ai-prompt-help")).show(ctx, |ui| {
        ui.set_width(480.0);
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            ui.add(egui::Label::new(crate::clippings::plain(ap::HELP_MD)).wrap());
        });
        ui.add_space(10.0);
        let (enter, escape) = modal_keys(ui, opened, &[]);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(egui::Button::new("OK").min_size(egui::vec2(76.0, 24.0))).clicked() || enter || escape {
                done = true;
            }
        });
    });
    done
}

/// One row of the Features sub-tab (`X:7583-7866`).
#[derive(Clone, Debug, PartialEq)]
pub enum FeatureRow {
    Heading(&'static str),
    Note(&'static str),
    /// A default chat permission's popup.
    Permission { key: &'static str, label: &'static str },
    /// The AI completion checkbox.
    Completion { key: &'static str, label: &'static str },
}

/// The Features sub-tab, top to bottom (iTerm2's "Permissons" typo kept).
pub fn features_layout() -> Vec<FeatureRow> {
    let mut rows = vec![FeatureRow::Heading("Chat Permissons:"), FeatureRow::Note("These settings define the default permissions for new chats.")];
    rows.extend(ai::PERMISSIONS.iter().map(|&(key, label)| FeatureRow::Permission { key, label }));
    rows.push(FeatureRow::Heading("AI Completion:"));
    rows.push(FeatureRow::Completion { key: ai::AI_COMPLETIONS, label: "Offer AI command suggestion in Composer & Auto Composer" });
    rows.push(FeatureRow::Note("When enabled, everything you type in the Composer will be shared with your AI provider."));
    rows
}

/// The Features sub-tab: every row always enabled, as iTerm2's popups and checkbox have no
/// enabling rule. True when a setting changed.
fn features(ui: &mut egui::Ui, g: &mut Globals) -> bool {
    let mut changed = false;
    for row in features_layout() {
        match row {
            FeatureRow::Heading(text) => {
                ui.add_space(4.0);
                ui.strong(text);
            }
            FeatureRow::Note(text) => {
                ui.weak(text);
            }
            FeatureRow::Permission { key, .. } => {
                if let Some(s) = crate::schema::find_global(key) {
                    changed |= crate::settings_keys::global_control(ui, g, s, true);
                }
            }
            FeatureRow::Completion { key, label } => changed |= bool_box(ui, g, key, label, true),
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
        SubTab::Prompts => prompts(ui, g, st),
        SubTab::Features => features(ui, g),
    };
    ui.add_space(8.0);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_catalog::bundled;
    use crate::ai_models::ManualModel;
    use crate::ai_settings::{Api, Vendor, ENABLE_AI, ADMIN, MODEL, URL, USE_RECOMMENDED, VENDOR};
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
        let none = Discovered::default();
        let (providers, manuals) = model_items(&g, cat, &none);
        assert_eq!(providers, pairs(&[("provider:2", "OpenAI"), ("provider:4", "Anthropic"), ("provider:1", "Gemini"), ("provider:0", "DeepSeek"), ("provider:3", "Ollama")]));
        assert_eq!(manuals, pairs(&[("manual:llama3", "Manual: llama3 \u{2014} Ollama"), ("manual:b", "Manual: b \u{2014} OpenAI")]), "a dynamic entry lists nothing before discovery");

        assert_eq!(stored_identifier(&g), "provider:2");
        pick(&mut g, "manual:b", cat, &none);
        assert_eq!(stored_identifier(&g), "manual:b");
        assert!(!g.bool(USE_RECOMMENDED), "select_manual ran");
        assert_eq!(g.string(URL), B_URL, "select_manual ran, not model_did_change");
        assert_eq!(g.int(crate::ai_settings::API), 2);

        pick(&mut g, "provider:4", cat, &none);
        assert_eq!(stored_identifier(&g), "provider:4");
        assert!(g.bool(USE_RECOMMENDED));
        assert_eq!(g.int(VENDOR), Vendor::Anthropic.raw());
        let recommended = cat.recommended(Vendor::Anthropic).expect("Anthropic has a recommended model");
        assert_eq!(g.string(MODEL), recommended.name, "select_provider(Anthropic) ran");

        // A dynamic entry lists the models discovered at its server, and picking one makes it
        // the default by its resolved name, which the request resolves to the same server.
        let server = "http://server.example:11434/api/chat";
        let mut m = crate::ai_model::pending_ollama_model();
        m.name = "qwen3".into();
        m.url = server.into();
        m.features.insert(crate::ai_catalog::Feature::FunctionCalling);
        let found = Discovered { servers: vec![crate::ai_model::Server { endpoint: server.into(), headers: Vec::new(), models: vec![m] }] };
        let (_, manuals) = model_items(&g, cat, &found);
        assert_eq!(manuals.last(), Some(&("manual:qwen3".to_string(), "Manual: qwen3 \u{2014} Ollama".to_string())));
        pick(&mut g, "manual:qwen3", cat, &found);
        assert_eq!(stored_identifier(&g), "manual:qwen3");
        assert_eq!(shown_identifier(&g, cat, &found), "manual:qwen3");
        assert_eq!(shown_identifier(&g, cat, &none), "provider:2", "undiscovered, the default shows the first provider");
        assert!(!g.bool(USE_RECOMMENDED));
        assert_eq!((g.string(URL), g.int(crate::ai_settings::API)), (server.to_string(), Api::Llama.raw()));
        assert!(g.bool(crate::ai_settings::FEATURE_FUNCTION_CALLING) && g.bool(crate::ai_settings::FEATURE_STREAMING));
        let resolved = crate::ai_model::configured(&g, cat, &found).expect("a model");
        assert_eq!((resolved.name.as_str(), resolved.url.as_str()), ("qwen3", server));
        assert_eq!(key_hint(&g, "manual:qwen3", cat, &found, &|_| false).as_deref(), Some("No API key is used."));
        // Settings, the gate and the request name the same host for a dynamic default, even with
        // other usable entries ahead of it.
        assert_eq!(requests_text(&g, cat, &found), "Requests go to server.example (Ollama). Tandem asks before the first one.");
        g.set(crate::ai_settings::ENABLE_AI, Value::Boolean(true));
        assert_eq!(crate::ai_gate::check(&g, cat, &found), crate::ai_gate::Check::NeedsConsent { provider: "Ollama".into(), host: "server.example".into() });
        assert_eq!(crate::ai_gate::check_endpoint(&g, Some(&crate::ai_model::endpoint(&resolved))), crate::ai_gate::check(&g, cat, &found));
    }

    #[test]
    fn ollama_pickers_follow_iterm2() {
        let cat = bundled();
        let none = Discovered::default();
        let mut g = with_models(&[]);
        let p = ollama_pickers(&g, &none);
        assert_eq!(p, OllamaPickers { regular: vec![crate::ai_discovery::NO_MODELS_FOUND.into()], budget: vec![crate::ai_discovery::NO_MODELS_FOUND.into()], regular_at: 0, budget_at: 0, enabled: false });
        pick_regular(&mut g, &p, 0);
        pick_budget(&mut g, &p, 0);
        assert_eq!((g.string(crate::ai_settings::OLLAMA_REGULAR), g.string(crate::ai_settings::OLLAMA_ECONOMY)), (String::new(), String::new()), "a disabled picker writes nothing");

        let tags = |names: &[&str]| {
            let models = names
                .iter()
                .map(|n| {
                    let mut m = crate::ai_model::pending_ollama_model();
                    m.name = n.to_string();
                    m
                })
                .collect();
            Discovered { servers: vec![crate::ai_model::Server { endpoint: crate::ai_models::OLLAMA_ENDPOINT.into(), headers: Vec::new(), models }] }
        };
        let found = tags(&["qwen3:8b", "llama3.3"]);
        let p = ollama_pickers(&g, &found);
        assert_eq!(p.regular, ["llama3.3", "qwen3:8b"], "sorted");
        assert_eq!(p.budget, ["Same as regular model", "llama3.3", "qwen3:8b"]);
        assert_eq!((p.regular_at, p.budget_at, p.enabled), (0, 0, true));
        assert_eq!(g.string(crate::ai_settings::OLLAMA_REGULAR), "", "showing the first item writes nothing");
        pick_regular(&mut g, &p, 1);
        pick_budget(&mut g, &p, 1);
        assert_eq!((g.string(crate::ai_settings::OLLAMA_REGULAR), g.string(crate::ai_settings::OLLAMA_ECONOMY)), ("qwen3:8b".to_string(), "llama3.3".to_string()));
        let p = ollama_pickers(&g, &found);
        assert_eq!((p.regular_at, p.budget_at), (1, 1));
        pick_budget(&mut g, &p, 0);
        assert_eq!(g.string(crate::ai_settings::OLLAMA_ECONOMY), "", "Same as regular model stores empty");
        // A pick the server no longer has shows the first item.
        g.set(crate::ai_settings::OLLAMA_REGULAR, Value::String("gone".into()));
        assert_eq!(ollama_pickers(&g, &found).regular_at, 0);
        // Tags of another server are not the vendor's.
        let mut other = tags(&["a"]);
        other.servers[0].endpoint = "http://gpu:11434/api/chat".into();
        assert!(!ollama_pickers(&g, &other).enabled);
        assert_eq!((REGULAR_LABEL, BUDGET_LABEL), ("Regular:", "Budget:"));
        let _ = cat;
    }

    #[test]
    fn ollama_probe_follows_iterm2() {
        let cat = bundled();
        let none = Discovered::default();
        let mut g = with_models(&[manual("b", B_URL, 2)]);
        assert_eq!(pick_interactive(&mut g, "provider:4", cat, &none), None, "only Ollama probes");
        pick(&mut g, "manual:b", cat, &none);
        let before = default_snapshot(&g);
        let (request, prior) = pick_interactive(&mut g, "provider:3", cat, &none).expect("picking Ollama probes");
        assert_eq!(request, AiRequest::Discover { endpoint: "http://localhost:11434/api/chat".into(), headers: Vec::new(), force: true });
        assert_eq!(prior, before);
        assert!(crate::ai_model::ollama_vendor_is_default(&g), "the pick is committed first");
        restore_default(&mut g, &prior, cat);
        assert_eq!(default_snapshot(&g), before);
        assert_eq!(stored_identifier(&g), "manual:b");
        assert!(!crate::ai_model::ollama_vendor_is_default(&g));
    }

    #[test]
    fn editor_fetch_answers_follow_iterm2() {
        let cat = bundled();
        let url = "http://gpu.test:11434/api/chat";
        let mut e = Editor::new(manual("m", url, 5), None, cat, 0);
        let ok = |count| Outcome { count, failed: false, error: None };
        e.fetching = Some((true, url.into(), Vec::new()));
        e.take_answer(ok(3), &Discovered::default(), 7);
        assert_eq!(e.alert, Some(("Models Refreshed".into(), crate::ai_discovery::refreshed_body(3), 7)));
        e.fetching = Some((true, url.into(), Vec::new()));
        e.take_answer(Outcome { count: 0, failed: true, error: None }, &Discovered::default(), 8);
        assert_eq!(e.alert.as_ref().map(|a| a.0.as_str()), Some("Could Not Reach Server"));
        // Fetch Models lists what the cache now holds for that server and headers.
        let mut m = crate::ai_model::pending_ollama_model();
        m.name = "phi4".into();
        m.url = url.into();
        let found = Discovered { servers: vec![crate::ai_model::Server { endpoint: url.into(), headers: Vec::new(), models: vec![m] }] };
        e.alert = None;
        e.fetching = Some((false, url.into(), Vec::new()));
        e.take_answer(ok(1), &found, 9);
        assert_eq!((e.menu.clone(), e.alert.clone()), (Some(vec!["phi4".to_string()]), None));
        e.menu = None;
        e.fetching = Some((false, url.into(), Vec::new()));
        e.take_answer(ok(0), &Discovered::default(), 10);
        assert_eq!(e.alert, Some(("Could Not Fetch Models".into(), crate::ai_discovery::NO_MODELS.into(), 10)));
        e.fetching = Some((false, url.into(), Vec::new()));
        e.take_answer(Outcome { count: 0, failed: true, error: Some("Could not reach Ollama at gpu.test: x".into()) }, &found, 11);
        assert_eq!(e.alert.as_ref().map(|a| a.1.as_str()), Some("Could not reach Ollama at gpu.test: x"));
        // An answer nobody waits for is dropped.
        e.alert = None;
        e.take_answer(ok(2), &found, 12);
        assert_eq!((e.alert.clone(), e.menu.clone()), (None, None));
        assert!(!State::default().busy());
    }

    /// Task 10 re-review: a running test keeps the window drawing, but not while it waits for
    /// the key prompt, whose answer asks for a redraw.
    #[test]
    fn busy_while_a_test_runs_but_not_while_it_waits_for_a_key() {
        use crate::ai_transport::{Options, Proxy, Recorded};
        let probe = crate::ai_test_conn::Probe {
            name: "gpt-5".into(),
            url: "https://api.openai.com/v1/responses".into(),
            api: Api::Responses,
            function_calling: false,
            supports_temperature: true,
            headers: Vec::new(),
        };
        let wire = crate::ai_turn::Wire { token_limit: 128000, response_limit: 8192, zdr: false, ollama_num_ctx: 0, ollama_keep_alive: "30m".into() };
        let options = Options { idle: std::time::Duration::from_secs(5), proxy: Proxy::Off };
        let mut s = State::default();
        s.tester = Some(Tester::new(probe, wire, std::sync::Arc::new(Recorded::new(Vec::new())), options, std::sync::Arc::new(|| {})));
        assert!(s.busy(), "a built test draws until it answers");
        assert_eq!(s.tester.as_mut().map(|t| t.start(crate::chat_gate::Passed::for_test(), None)), Some(None));
        assert_eq!(s.tester.as_ref().and_then(Tester::needs_key), Some(Vendor::OpenAI));
        assert!(!s.busy(), "waiting for the key does not repaint");
        let mut e = Editor::new(manual("m", "https://api.openai.com/v1/responses", 0), None, bundled(), 0);
        e.testing = true;
        s.editor = Some(e);
        assert!(!s.busy(), "the editor's testing flag waits for the key too");
        s.tester = None;
        assert!(s.busy(), "a test the App has not built yet draws");
    }

    #[test]
    fn unmatched_default_writes_nothing() {
        let cat = bundled();
        let mut g = with_models(&[manual("llama3", LLAMA3_URL, 5)]);
        g.set(USE_RECOMMENDED, Value::Boolean(false));
        g.set(MODEL, Value::String("gone".into()));
        let before = g.values.clone();
        assert_eq!(stored_identifier(&g), "manual:gone");
        assert_eq!(shown_identifier(&g, cat, &Discovered::default()), "provider:2");
        assert_eq!(g.values, before);

        for vendor in [5, 9] {
            let mut g = with_models(&[]);
            g.set(VENDOR, Value::Integer(vendor));
            let before = g.values.clone();
            assert_eq!(shown_identifier(&g, cat, &Discovered::default()), "provider:2", "AIVendor {vendor}");
            assert_eq!(g.values, before, "AIVendor {vendor}");
        }

        let mut g = with_models(&[manual("llama3", LLAMA3_URL, 5)]);
        pick(&mut g, "manual:llama3", cat, &Discovered::default());
        assert_eq!(shown_identifier(&g, cat, &Discovered::default()), "manual:llama3", "a matching default shows itself");
    }

    #[test]
    fn key_hint_follows_iterm2() {
        let cat = bundled();
        let g = with_models(&[manual("llama3", LLAMA3_URL, 5), manual("b", B_URL, 2), manual("remote", "https://example.com/v1", 5)]);
        let anthropic_set = |v: Vendor| v == Vendor::Anthropic;
        let none_set = |_: Vendor| false;
        assert_eq!(key_hint(&g, "provider:4", cat, &Discovered::default(), &anthropic_set).as_deref(), Some("Authorizes with your Anthropic API key."));
        assert_eq!(key_hint(&g, "provider:4", cat, &Discovered::default(), &none_set).as_deref(), Some("No Anthropic API key is set yet."));
        assert_eq!(key_hint(&g, "provider:3", cat, &Discovered::default(), &none_set), None, "12b puts the Regular and Budget pickers there");
        assert_eq!(key_hint(&g, "manual:llama3", cat, &Discovered::default(), &none_set).as_deref(), Some("No API key is sent to a local endpoint. If it requires authentication, use a custom header."));
        assert_eq!(key_hint(&g, "manual:b", cat, &Discovered::default(), &anthropic_set).as_deref(), Some("No OpenAI API key is set yet."));
        assert_eq!(key_hint(&g, "manual:remote", cat, &Discovered::default(), &none_set).as_deref(), Some("No API key is used."));
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

    /// A store whose writes fail, for `save_keys`.
    struct FailingStore;

    impl KeyStore for FailingStore {
        fn read(&mut self, _target: &str) -> Result<Option<Secret>, String> {
            Ok(None)
        }
        fn write(&mut self, target: &str, _user: &str, _key: &Secret) -> Result<(), String> {
            Err(format!("denied {target}"))
        }
        fn delete(&mut self, target: &str) -> Result<(), String> {
            Err(format!("denied {target}"))
        }
    }

    #[test]
    fn failed_key_writes_are_all_reported() {
        let d = KeyDialog { initial: secrets(&["", "", "", ""]), now: secrets(&["sk-1", "", "sk-ant-1", ""]), focus: false, opened: 0 };
        let mut cache = Keys::default();
        assert_eq!(
            save_keys(&d, &mut cache, &mut FailingStore).as_deref(),
            Some("Could not save the OpenAI API key: denied TandemAI/OpenAI\nCould not save the Gemini API key: denied TandemAI/Gemini"),
            "every failed vendor is reported, not only the last"
        );
        assert!(cache.is_set(Vendor::OpenAI, &mut FailingStore), "the key stays for the session");
        let unchanged = KeyDialog { initial: secrets(&["a"]), now: secrets(&["a"]), focus: false, opened: 0 };
        assert_eq!(save_keys(&unchanged, &mut cache, &mut FailingStore), None);
    }

    #[test]
    fn editor_key_hint_follows_iterm2() {
        let form = |url: &str, api: Api, name: &str| {
            let mut f = EditorForm::from_model(&manual("m", "u", 1), bundled());
            f.url = url.into();
            f.api = api.raw();
            f.name = name.into();
            f
        };
        assert_eq!(editor_key_hint(&form(" http://localhost:1234/v1/chat/completions", Api::ChatCompletions, "m")), SELF_HOSTED, "the URL is trimmed first");
        assert_eq!(editor_key_hint(&form("https://api.anthropic.com/v1/messages", Api::Anthropic, "m")), "Authorizes with your Anthropic API key.");
        assert_eq!(editor_key_hint(&form("https://openrouter.ai/api/v1/chat/completions", Api::ChatCompletions, "gemini-pro")), "Authorizes with your Gemini API key.");
        assert_eq!(editor_key_hint(&form("https://ollama.example.com/api/chat", Api::Llama, "m")), NO_KEY_USED);
    }

    #[test]
    fn preset_titles_follow_iterm2() {
        let cat = bundled();
        assert_eq!(preset_title(crate::ai_models::CUSTOM_PRESET, cat), "Custom");
        assert_eq!(preset_title(0, cat), crate::ai_models::catalog_presets(cat)[0].name);
        assert_eq!(preset_title(crate::ai_models::PROVIDER_PRESET_BASE + 3, cat), "OpenRouter");
        assert_eq!(preset_title(crate::ai_models::PROVIDER_PRESET_BASE + 7, cat), "Custom");
        assert_eq!(preset_title(999, cat), "Custom");
    }

    #[test]
    fn sheet_opens_like_iterm2() {
        let mut g = with_models(&[]);
        g.set(crate::ai_settings::CUSTOM_HEADERS_ENABLED, Value::Boolean(true));
        g.set(crate::ai_settings::CUSTOM_HEADERS, Value::Array(vec![Value::Table(toml::toml! { name = "X-Key" value = "1" })]));
        let (sheet, noted) = open_sheet(&mut g, 7);
        assert!(noted && sheet.notice, "the legacy headers notice comes first");
        assert_eq!(sheet.opened, 7, "Return and Escape wait for the next frame");
        assert!(g.bool(crate::ai_models::LEGACY_NOTICE_FLAG));
        assert_eq!((sheet.models.len(), sheet.selected), (0, None));
        let (sheet, noted) = open_sheet(&mut g, 7);
        assert!(!noted && !sheet.notice, "once");

        let mut g = with_models(&[manual("llama3", LLAMA3_URL, 5), manual("b", B_URL, 2)]);
        let (sheet, noted) = open_sheet(&mut g, 7);
        assert!(!noted && !sheet.notice);
        assert_eq!(sheet.models.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["llama3", "b"]);
        assert_eq!(sheet.selected, Some(0));
    }

    #[test]
    fn modal_keys_wait_for_the_next_frame() {
        assert_eq!(default_keys(true, true, true, false), (false, false), "the key that opened a modal does not act in it");
        assert_eq!(default_keys(false, true, true, false), (true, true));
        assert_eq!(default_keys(false, true, false, true), (false, false), "Return clicks the focused button, not the default one");
        assert_eq!(default_keys(false, false, true, true), (false, true), "Escape still cancels");
    }

    #[test]
    fn held_return_acts_once() {
        let key = |repeat: bool, modifiers: egui::Modifiers| egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat, modifiers };
        assert!(fresh_return(&[key(false, egui::Modifiers::NONE)]));
        assert!(!fresh_return(&[key(true, egui::Modifiers::NONE)]), "a key repeat of a held Return does not act");
        assert!(!fresh_return(&[key(false, egui::Modifiers::SHIFT)]), "Shift+Return is not Return");
        assert!(!fresh_return(&[egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE }]), "a release is not a press");
        assert!(fresh_return(&[key(true, egui::Modifiers::NONE), key(false, egui::Modifiers::NONE)]), "a fresh press in the same frame still acts");
    }

    #[test]
    fn each_prompt_has_its_own_editor() {
        let ids: Vec<egui::Id> = ap::SELECTOR.iter().flat_map(|group| group.iter()).map(|p| prompt_text_id(p.key())).chain([prompt_text_id(ap::CODE_REVIEW_KEY)]).collect();
        for (i, a) in ids.iter().enumerate() {
            assert!(ids[i + 1..].iter().all(|b| b != a), "prompt {i} shares its editor, and with it undo, with another prompt");
        }
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
        assert_eq!(requests_text(&g, cat, &Discovered::default()), "Requests go to api.openai.com (OpenAI). Tandem asks before the first one.");
        crate::ai_gate::grant(&mut g, "api.openai.com");
        assert_eq!(requests_text(&g, cat, &Discovered::default()), "Requests go to api.openai.com (OpenAI). You allowed them.");
        g.set(VENDOR, Value::Integer(5));
        assert_eq!(requests_text(&g, cat, &Discovered::default()), crate::ai_gate::NO_ENDPOINT);
    }

    #[test]
    fn features_rows_follow_iterm2() {
        use FeatureRow::*;
        assert_eq!(
            features_layout(),
            [
                Heading("Chat Permissons:"),
                Note("These settings define the default permissions for new chats."),
                Permission { key: "AIPermissionCheckTerminalState", label: "Check Terminal State" },
                Permission { key: "AIPermissionRunCommands", label: "Run Commands" },
                Permission { key: "AIPermissionViewHistory", label: "View Contents" },
                Permission { key: "AIPermissionWriteToClipboard", label: "Write to Clipboard" },
                Permission { key: "AIPermissionTypeForYou", label: "Control Terminal" },
                Permission { key: "AIPermissionViewManpages", label: "View Manpages" },
                Permission { key: "AIPermissionWriteToFilesystem", label: "Write to Filesystem" },
                Permission { key: "AIPermissionActInWebBrowser", label: "Act in Web Browser" },
                Heading("AI Completion:"),
                Completion { key: "AICompletions", label: "Offer AI command suggestion in Composer & Auto Composer" },
                Note("When enabled, everything you type in the Composer will be shared with your AI provider."),
            ]
        );
        assert_eq!(RESET, "Reset");
    }

    #[test]
    fn key_prompt_follows_iterm2() {
        assert_eq!(key_prompt(Vendor::Anthropic), ("Manage AI API Keys".to_string(), "Anthropic API key".to_string()));
        assert_eq!(key_prompt(Vendor::OpenAI).1, "OpenAI API key");
        let typed = Secret::new(" \t ".to_string());
        assert_eq!(crate::ai_keys::normalized(typed.expose()), None, "a blank key writes nothing and continues with None");
    }

    #[test]
    fn test_click_follows_iterm2() {
        let cat = bundled();
        let mut g = with_models(&[]);
        g.set(crate::ai_settings::ENABLE_AI, Value::Boolean(true));
        let none = Discovered::default();
        let mut form = EditorForm::from_model(&manual("m", " http://localhost:1234/v1/chat/completions ", 1), cat);
        form.name = " ".into();
        assert_eq!(test_click(&g, &form, &none), Err((tc::MISSING_TITLE, tc::MISSING_TEXT)));
        form.name = "  qwen3 ".into();
        form.headers = pairs(&[("X-Team", "blue"), ("  ", "dropped")]);
        form.temperature = false;
        form.function_calling = true;
        // Consent first: the gate's alert for the probe's own host, and no test.
        let Ok(AiRequest::Alert(alert)) = test_click(&g, &form, &none) else { panic!("the consent alert") };
        assert_eq!(alert.then, crate::ai_gate::Then::Grant("localhost".into()));
        crate::ai_gate::grant(&mut g, "localhost");
        let probe = tc::Probe {
            name: "qwen3".into(),
            url: "http://localhost:1234/v1/chat/completions".into(),
            api: Api::ChatCompletions,
            function_calling: true,
            supports_temperature: false,
            headers: pairs(&[("X-Team", "blue")]),
        };
        assert_eq!(test_click(&g, &form, &none), Ok(AiRequest::BuildTester(probe)));

        // Auto-discovery: the popup's choice, else the first discovered tag.
        let url = "http://localhost:1234/v1/chat/completions";
        form.set_dynamic(true);
        assert_eq!(test_click(&g, &form, &none), Err((tc::MISSING_TITLE, tc::MISSING_DISCOVERED)));
        let mut m = crate::ai_model::pending_ollama_model();
        m.name = "llama3.3".into();
        m.url = url.into();
        let found = Discovered { servers: vec![crate::ai_model::Server { endpoint: url.into(), headers: pairs(&[("X-Team", "blue")]), models: vec![m] }] };
        let Ok(AiRequest::BuildTester(p)) = test_click(&g, &form, &found) else { panic!("a test") };
        assert_eq!((p.name.as_str(), p.api), ("llama3.3", Api::Llama));
        form.selected = "qwen3:8b".into();
        let Ok(AiRequest::BuildTester(p)) = test_click(&g, &form, &found) else { panic!("a test") };
        assert_eq!(p.name, "qwen3:8b");
        form.url = " ".into();
        assert_eq!(test_click(&g, &form, &found), Err((tc::MISSING_TITLE, tc::MISSING_TEXT)));

        // A URL with no host fails at once with "The URL is not valid.", before any gate.
        form.set_dynamic(false);
        for url in ["not a url", "http:///x"] {
            form.url = url.into();
            assert_eq!(test_click(&g, &form, &none), Err((tc::FAILED_TITLE, tc::BAD_URL)), "{url}");
        }
    }
}
