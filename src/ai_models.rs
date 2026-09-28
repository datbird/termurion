//! Manual AI models, the default model's endpoint, the default-model popup's writes, the Manual
//! AI Models sheet's actions, the model editor's form, presets and Save rules, and the one-time
//! migration of the legacy global custom headers. Pure; ports
//! `Settings/GeneralPreferencesViewController.m` (the manual model dictionary, `G:43-64`,
//! `G:3634-3781`, `G:2917-2995`, `G:3188-3236`, the sheet and editor `G:282-1640`,
//! `G:3811-4020`), `AITerm/LLMMetadata.swift` (`LM:78-218`, `LM:491-718`),
//! `AITerm/AIProviderPreset.swift` and `AppLaunch/iTermMigrationHelper.m:458-509`.

use crate::ai_catalog::{Catalog, Feature, Model, VectorStore};
use crate::ai_policy;
use crate::ai_settings::{self as ai, Api, Vendor};
use crate::globals::Globals;
use toml::{Table, Value};

/// The built-in Ollama vendor's local endpoint (`LM:357`).
pub const OLLAMA_ENDPOINT: &str = "http://localhost:11434/api/chat";
/// The stand-in model name while Ollama discovery is pending (`LM:401`); never stored.
pub const OLLAMA_PENDING: &str = "Ollama (discovering models\u{2026})";
/// Set once the legacy global headers were copied into the manual models.
pub const MIGRATION_FLAG: &str = "NoSyncAICustomHeadersPerModelMigrationDone";

// The manual model dictionary's keys (`G:43-64`).
const ID: &str = "id";
const NAME: &str = "name";
const URL: &str = "url";
const API: &str = "api";
const CONTEXT_WINDOW_TOKENS: &str = "contextWindowTokens";
const MAX_RESPONSE_TOKENS: &str = "maxResponseTokens";
const HOSTED_CODE_INTERPRETER: &str = "hostedCodeInterpreter";
const HOSTED_FILE_SEARCH: &str = "hostedFileSearch";
const HOSTED_WEB_SEARCH: &str = "hostedWebSearch";
const FUNCTION_CALLING: &str = "functionCalling";
const STREAMING: &str = "streaming";
const VISION: &str = "vision";
const VECTOR_STORE: &str = "vectorStore";
const SUPPORTS_TEMPERATURE: &str = "supportsTemperature";
const CONFIGURABLE_THINKING: &str = "configurableThinking";
const DYNAMIC_MODELS: &str = "dynamicModels";
const DYNAMIC_SELECTED_MODEL: &str = "dynamicSelectedModel";
const CUSTOM_HEADERS: &str = "customHeaders";
const KNOWN_KEYS: [&str; 18] = [
    ID,
    NAME,
    URL,
    API,
    CONTEXT_WINDOW_TOKENS,
    MAX_RESPONSE_TOKENS,
    HOSTED_CODE_INTERPRETER,
    HOSTED_FILE_SEARCH,
    HOSTED_WEB_SEARCH,
    FUNCTION_CALLING,
    STREAMING,
    VISION,
    VECTOR_STORE,
    SUPPORTS_TEMPERATURE,
    CONFIGURABLE_THINKING,
    DYNAMIC_MODELS,
    DYNAMIC_SELECTED_MODEL,
    CUSTOM_HEADERS,
];

/// iTerm2's token fallback for a manual model (`G:3776`, `LM:582`).
const FALLBACK_TOKENS: i64 = 8192;

/// The leading integer of a string as NSString `integerValue` reads it: leading whitespace, an
/// optional sign, then digits; 0 when there are none.
fn ns_integer_value(s: &str) -> i64 {
    let t = s.trim_start();
    let (negative, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let mut n: i64 = 0;
    for c in digits.bytes().take_while(u8::is_ascii_digit) {
        n = n.saturating_mul(10).saturating_add(i64::from(c - b'0'));
    }
    if negative {
        -n
    } else {
        n
    }
}

/// An integer field as `integerValue` reads an NSNumber or NSString; None for other types.
fn integer_of(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Integer(i) => Some(*i),
        Value::Float(f) => Some(*f as i64),
        Value::Boolean(b) => Some(i64::from(*b)),
        Value::String(s) => Some(ns_integer_value(s)),
        _ => None,
    }
}

/// A boolean field as `boolValue` reads an NSNumber or NSString; None for other types.
fn bool_of(v: Option<&Value>) -> Option<bool> {
    match v? {
        Value::Boolean(b) => Some(*b),
        Value::Integer(i) => Some(*i != 0),
        Value::Float(f) => Some(*f != 0.0),
        Value::String(s) => Some(matches!(s.trim_start().bytes().next(), Some(b'Y' | b'y' | b'T' | b't' | b'1'..=b'9'))),
        _ => None,
    }
}

/// `{name, value}` string entries; an entry whose name or value is not a string is dropped, a
/// missing value reads "".
fn headers_of(v: Option<&Value>) -> Vec<(String, String)> {
    let Some(Value::Array(list)) = v else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|h| {
            let t = h.as_table()?;
            let name = t.get("name")?.as_str()?;
            let value = match t.get("value") {
                None => "",
                Some(v) => v.as_str()?,
            };
            Some((name.to_string(), value.to_string()))
        })
        .collect()
}

fn headers_value(headers: &[(String, String)]) -> Value {
    Value::Array(
        headers
            .iter()
            .map(|(n, v)| {
                let mut t = Table::new();
                t.insert("name".into(), Value::String(n.clone()));
                t.insert("value".into(), Value::String(v.clone()));
                Value::Table(t)
            })
            .collect(),
    )
}

fn new_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()
}

/// One configured manual model (`G:43-64`). Keys Tandem does not know stay in `extra` so a round
/// trip through Settings keeps them.
#[derive(Clone, Debug, PartialEq)]
pub struct ManualModel {
    pub id: String,
    pub name: String,
    pub url: String,
    pub api: i64,
    pub context_window_tokens: i64,
    pub max_response_tokens: i64,
    pub hosted_code_interpreter: bool,
    pub hosted_file_search: bool,
    pub hosted_web_search: bool,
    pub function_calling: bool,
    pub streaming: bool,
    pub vision: bool,
    pub vector_store: i64,
    pub supports_temperature: Option<bool>,
    pub configurable_thinking: Option<bool>,
    pub dynamic_models: bool,
    pub dynamic_selected_model: Option<String>,
    pub custom_headers: Vec<(String, String)>,
    pub extra: Table,
}

impl ManualModel {
    /// None when the name or URL is not a string (`G:3712-3716`). A missing id reads "", so
    /// repeated reads agree (the sheet gives a new model its id).
    pub fn from_table(t: &Table) -> Option<ManualModel> {
        let name = t.get(NAME)?.as_str()?.to_string();
        let url = t.get(URL)?.as_str()?.to_string();
        let flag = |k: &str| bool_of(t.get(k)).unwrap_or(false);
        Some(ManualModel {
            id: t.get(ID).and_then(Value::as_str).unwrap_or_default().to_string(),
            name,
            url,
            api: integer_of(t.get(API)).unwrap_or(Api::ChatCompletions.raw()),
            context_window_tokens: integer_of(t.get(CONTEXT_WINDOW_TOKENS)).unwrap_or(FALLBACK_TOKENS),
            max_response_tokens: integer_of(t.get(MAX_RESPONSE_TOKENS)).unwrap_or(FALLBACK_TOKENS),
            hosted_code_interpreter: flag(HOSTED_CODE_INTERPRETER),
            hosted_file_search: flag(HOSTED_FILE_SEARCH),
            hosted_web_search: flag(HOSTED_WEB_SEARCH),
            function_calling: flag(FUNCTION_CALLING),
            streaming: flag(STREAMING),
            vision: flag(VISION),
            vector_store: integer_of(t.get(VECTOR_STORE)).unwrap_or(0),
            supports_temperature: bool_of(t.get(SUPPORTS_TEMPERATURE)),
            configurable_thinking: bool_of(t.get(CONFIGURABLE_THINKING)),
            dynamic_models: flag(DYNAMIC_MODELS),
            dynamic_selected_model: t.get(DYNAMIC_SELECTED_MODEL).and_then(Value::as_str).map(str::to_string),
            custom_headers: headers_of(t.get(CUSTOM_HEADERS)),
            extra: t.iter().filter(|(k, _)| !KNOWN_KEYS.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect(),
        })
    }

    pub fn to_table(&self) -> Table {
        let mut t = self.extra.clone();
        let mut put = |k: &str, v: Value| {
            t.insert(k.to_string(), v);
        };
        put(ID, Value::String(self.id.clone()));
        put(NAME, Value::String(self.name.clone()));
        put(URL, Value::String(self.url.clone()));
        put(API, Value::Integer(self.api));
        put(CONTEXT_WINDOW_TOKENS, Value::Integer(self.context_window_tokens));
        put(MAX_RESPONSE_TOKENS, Value::Integer(self.max_response_tokens));
        put(HOSTED_CODE_INTERPRETER, Value::Boolean(self.hosted_code_interpreter));
        put(HOSTED_FILE_SEARCH, Value::Boolean(self.hosted_file_search));
        put(HOSTED_WEB_SEARCH, Value::Boolean(self.hosted_web_search));
        put(FUNCTION_CALLING, Value::Boolean(self.function_calling));
        put(STREAMING, Value::Boolean(self.streaming));
        put(VISION, Value::Boolean(self.vision));
        put(VECTOR_STORE, Value::Integer(self.vector_store));
        if let Some(b) = self.supports_temperature {
            put(SUPPORTS_TEMPERATURE, Value::Boolean(b));
        }
        if let Some(b) = self.configurable_thinking {
            put(CONFIGURABLE_THINKING, Value::Boolean(b));
        }
        put(DYNAMIC_MODELS, Value::Boolean(self.dynamic_models));
        if let Some(s) = &self.dynamic_selected_model {
            put(DYNAMIC_SELECTED_MODEL, Value::String(s.clone()));
        }
        put(CUSTOM_HEADERS, headers_value(&self.custom_headers));
        t
    }

    /// An out-of-range raw value reads as Chat Completions.
    pub fn api(&self) -> Api {
        Api::from_raw_or_chat(self.api)
    }

    /// The Endpoint column: the URL's host, else the URL itself (`G:181-188`).
    pub fn endpoint_host(&self) -> String {
        if self.url.is_empty() {
            return String::new();
        }
        ai_policy::host_of(&self.url).unwrap_or_else(|| self.url.clone())
    }

    fn vendor(&self) -> Vendor {
        manual_vendor(self.api(), &self.url, &self.name)
    }
}

/// The stored configurations; an entry that is not a table, or whose name or URL is not a
/// string, is dropped (`G:3701-3720`).
pub fn read(g: &Globals) -> Vec<ManualModel> {
    match g.get(ai::MANUAL_MODELS) {
        Some(Value::Array(list)) => list.iter().filter_map(|v| ManualModel::from_table(v.as_table()?)).collect(),
        _ => Vec::new(),
    }
}

/// The single model the settings described before manual model lists (`G:3652-3671`).
fn legacy_model(g: &Globals) -> Option<ManualModel> {
    let url = g.string(ai::URL);
    if url.is_empty() {
        return None;
    }
    Some(ManualModel {
        id: new_id(),
        name: g.string(ai::MODEL),
        url,
        api: g.int(ai::API),
        context_window_tokens: g.int(ai::TOKEN_LIMIT),
        max_response_tokens: g.int(ai::RESPONSE_TOKEN_LIMIT),
        hosted_code_interpreter: g.bool(ai::FEATURE_CODE_INTERPRETER),
        hosted_file_search: g.bool(ai::FEATURE_FILE_SEARCH),
        hosted_web_search: g.bool(ai::FEATURE_WEB_SEARCH),
        function_calling: g.bool(ai::FEATURE_FUNCTION_CALLING),
        streaming: g.bool(ai::FEATURE_STREAMING),
        vision: false,
        vector_store: g.int(ai::VECTOR_STORE),
        supports_temperature: None,
        configurable_thinking: None,
        dynamic_models: false,
        dynamic_selected_model: None,
        custom_headers: Vec::new(),
        extra: Table::new(),
    })
}

/// The Manual AI Models sheet's rows: the configurations, else the legacy entry when the
/// vendor's recommended model is not in use (`G:3721-3727`).
pub fn sheet_models(g: &Globals) -> Vec<ManualModel> {
    let models = read(g);
    if !models.is_empty() || g.bool(ai::USE_RECOMMENDED) {
        return models;
    }
    legacy_model(g).into_iter().collect()
}

/// The default-model popup's manual models (`LM:163-184`, `settingsManualModels`): the usable
/// configurations, else (when the recommended model is not in use) the legacy model. 12b adds
/// the models a dynamic entry discovers.
pub fn settings_manual_models(g: &Globals) -> Vec<ManualModel> {
    let configured: Vec<ManualModel> = usable(&read(g)).into_iter().cloned().collect();
    if !configured.is_empty() || g.bool(ai::USE_RECOMMENDED) {
        return configured;
    }
    legacy_model(g).into_iter().collect()
}

/// The models a request can use (`LM:538-545`): not a dynamic entry, a name and URL that are not
/// blank. 12b adds the models a dynamic entry discovers.
pub fn usable(models: &[ManualModel]) -> Vec<&ManualModel> {
    models.iter().filter(|m| !m.dynamic_models && !m.name.trim().is_empty() && !m.url.trim().is_empty()).collect()
}

/// Stores the configurations, dropping an entry with an empty name or URL (`G:3730-3742`).
pub fn write(g: &mut Globals, models: &[ManualModel]) {
    let clean = models.iter().filter(|m| !m.name.is_empty() && !m.url.is_empty()).map(|m| Value::Table(m.to_table())).collect();
    g.set(ai::MANUAL_MODELS, Value::Array(clean));
}

/// A vendor guessed from a model name alone (`LM:639-653`).
pub fn vendor_for_model_name(name: &str) -> Option<Vendor> {
    let lower = name.to_lowercase();
    [("claude", Vendor::Anthropic), ("gemini", Vendor::Gemini), ("deepseek", Vendor::DeepSeek), ("llama", Vendor::Llama)]
        .into_iter()
        .find(|(needle, _)| lower.contains(needle))
        .map(|(_, v)| v)
}

/// The vendor of a manual model (`LM:655-718`): the API when it names one, then the model name,
/// then the host, else OpenAI.
pub fn manual_vendor(api: Api, url: &str, name: &str) -> Vendor {
    match api {
        Api::Anthropic => return Vendor::Anthropic,
        Api::DeepSeek => return Vendor::DeepSeek,
        Api::Gemini => return Vendor::Gemini,
        Api::Llama => return Vendor::Llama,
        Api::AppleIntelligence => return Vendor::Apple,
        Api::ChatCompletions | Api::Completions | Api::Responses | Api::EarlyO1 => {}
    }
    if let Some(v) = vendor_for_model_name(name) {
        return v;
    }
    if ai_policy::host_is_anthropic(url) {
        Vendor::Anthropic
    } else if ai_policy::host_is_google(url) {
        Vendor::Gemini
    } else if ai_policy::host_is_deepseek(url) {
        Vendor::DeepSeek
    } else {
        Vendor::OpenAI
    }
}

/// Where the default model's requests go.
#[derive(Clone, Debug, PartialEq)]
pub struct Endpoint {
    pub vendor: Vendor,
    pub model: String,
    pub url: String,
    pub api: Api,
    pub custom_headers: Vec<(String, String)>,
}

impl Endpoint {
    fn of_manual(m: &ManualModel) -> Endpoint {
        Endpoint { vendor: m.vendor(), model: m.name.clone(), url: m.url.clone(), api: m.api(), custom_headers: m.custom_headers.clone() }
    }
}

/// The request-time manual models (`LM:162-174`): the usable configurations, else (when the
/// recommended model is not in use) the legacy model, which inherits the legacy global headers
/// (`LM:491-535`).
fn request_manual_models(g: &Globals) -> Vec<Endpoint> {
    let configured: Vec<Endpoint> = usable(&read(g)).into_iter().map(Endpoint::of_manual).collect();
    if !configured.is_empty() || g.bool(ai::USE_RECOMMENDED) {
        return configured;
    }
    let url = g.string(ai::URL);
    if url.is_empty() {
        return Vec::new();
    }
    let name = g.string(ai::MODEL);
    let api = Api::from_raw(g.int(ai::API)).unwrap_or(Api::ChatCompletions);
    let custom_headers = legacy_headers(g);
    vec![Endpoint { vendor: manual_vendor(api, &url, &name), model: name, url, api, custom_headers }]
}

/// The legacy global custom headers the legacy manual model inherits (`LM:528-534`): the
/// stored list while `AICustomHeadersEnabled` is on, else none.
pub fn legacy_headers(g: &Globals) -> Vec<(String, String)> {
    if g.bool(ai::CUSTOM_HEADERS_ENABLED) {
        headers_of(g.get(ai::CUSTOM_HEADERS).as_ref())
    } else {
        Vec::new()
    }
}

/// The recommended model of a vendor (`LM:126-161`). Ollama's models are discovered (12b); until
/// then it is the chosen regular model, else the pending placeholder. Apple Intelligence has no
/// endpoint on Windows (ruling 7).
fn recommended_endpoint(g: &Globals, v: Vendor, catalog: &Catalog) -> Option<Endpoint> {
    match v {
        Vendor::OpenAI | Vendor::Anthropic | Vendor::Gemini | Vendor::DeepSeek => {
            let m = catalog.recommended(v)?;
            if m.url.is_empty() {
                return None;
            }
            Some(Endpoint { vendor: m.vendor, model: m.name.clone(), url: m.url.clone(), api: m.api, custom_headers: Vec::new() })
        }
        Vendor::Llama => {
            let chosen = g.string(ai::OLLAMA_REGULAR);
            let model = if chosen.is_empty() { OLLAMA_PENDING.to_string() } else { chosen };
            Some(Endpoint { vendor: Vendor::Llama, model, url: OLLAMA_ENDPOINT.to_string(), api: Api::Llama, custom_headers: Vec::new() })
        }
        Vendor::Apple => None,
    }
}

/// The default model for new chats (`LM:203-218`): the vendor's recommended model, else the
/// manual model named by `AiModel`, else the first manual model. None when there is nothing to
/// send to (the gate refuses with a reason).
pub fn default_endpoint(g: &Globals, catalog: &Catalog) -> Option<Endpoint> {
    if g.bool(ai::USE_RECOMMENDED) {
        if let Some(v) = Vendor::from_raw(g.int(ai::VENDOR)) {
            return recommended_endpoint(g, v, catalog);
        }
    }
    let mut models = request_manual_models(g);
    let name = g.string(ai::MODEL);
    let pick = models.iter().position(|m| m.model == name).unwrap_or(0);
    (pick < models.len()).then(|| models.swap_remove(pick))
}

/// The vendor whose key a request uses (`LM:78-89`).
pub fn effective_vendor(g: &Globals, catalog: &Catalog) -> Vendor {
    if g.bool(ai::USE_RECOMMENDED) {
        return Vendor::from_raw(g.int(ai::VENDOR)).unwrap_or(Vendor::OpenAI);
    }
    default_endpoint(g, catalog).map_or(Vendor::OpenAI, |e| e.vendor)
}

/// The default-model popup picked a provider (`G:2917-2922`): its recommended model's name is
/// copied, never Ollama's placeholder (`G:3188-3197`), then the model's settings follow.
pub fn select_provider(g: &mut Globals, v: Vendor, catalog: &Catalog) {
    g.set(ai::USE_RECOMMENDED, Value::Boolean(true));
    g.set(ai::VENDOR, Value::Integer(v.raw()));
    if let Some(e) = default_endpoint(g, catalog) {
        if e.model != OLLAMA_PENDING {
            g.set(ai::MODEL, Value::String(e.model));
        }
    }
    model_did_change(g, catalog);
}

/// iTerm2's API guess for a model name (`AM:402-424`).
fn api_for_model(catalog: &Catalog, name: &str, fallback: i64) -> i64 {
    if let Some(m) = catalog.named(name) {
        return m.api.raw();
    }
    let guess = [("gpt", Api::Responses), ("gemini", Api::Gemini), ("deepseek", Api::Completions), ("llama", Api::Completions), ("claude", Api::Anthropic)]
        .into_iter()
        .find(|(needle, _)| name.contains(needle));
    guess.map_or(fallback, |(_, a)| a.raw())
}

/// The stored model changed (`G:3199-3236`, `aiModelDidChange` without its `_lastModel` guard):
/// the API follows the model, and a catalog model also sets the token limits, the URL, the five
/// feature switches and the vector store.
pub fn model_did_change(g: &mut Globals, catalog: &Catalog) {
    let name = g.string(ai::MODEL);
    let api = api_for_model(catalog, &name, g.int(ai::API));
    if api != g.int(ai::API) {
        g.set(ai::API, Value::Integer(api));
    }
    let Some(m) = catalog.named(&name).cloned() else {
        return;
    };
    g.set(ai::TOKEN_LIMIT, Value::Integer(m.context_window_tokens));
    g.set(ai::RESPONSE_TOKEN_LIMIT, Value::Integer(m.max_response_tokens));
    g.set(ai::URL, Value::String(m.url.clone()));
    for (key, feature) in [
        (ai::FEATURE_CODE_INTERPRETER, Feature::HostedCodeInterpreter),
        (ai::FEATURE_FILE_SEARCH, Feature::HostedFileSearch),
        (ai::FEATURE_WEB_SEARCH, Feature::HostedWebSearch),
        (ai::FEATURE_FUNCTION_CALLING, Feature::FunctionCalling),
        (ai::FEATURE_STREAMING, Feature::Streaming),
    ] {
        g.set(key, Value::Boolean(m.features.contains(&feature)));
    }
    g.set(ai::VECTOR_STORE, Value::Integer(m.vector_store.raw()));
}

/// A manual model became the default (`G:2971-2995`, `G:3749-3781`). A dynamic entry never
/// does; the default stops being the economy model. `model_did_change` is not called, so a
/// same-named catalog model never overwrites the manual model's URL.
pub fn select_manual(g: &mut Globals, m: &ManualModel) {
    if m.dynamic_models {
        return;
    }
    if m.name == g.string(ai::ECONOMY_MODEL) {
        g.set(ai::ECONOMY_MODEL, Value::String(String::new()));
    }
    g.set(ai::USE_RECOMMENDED, Value::Boolean(false));
    g.set(ai::MODEL, Value::String(m.name.clone()));
    g.set(ai::URL, Value::String(m.url.clone()));
    g.set(ai::API, Value::Integer(m.api));
    g.set(ai::TOKEN_LIMIT, Value::Integer(m.context_window_tokens));
    g.set(ai::RESPONSE_TOKEN_LIMIT, Value::Integer(m.max_response_tokens));
    g.set(ai::FEATURE_CODE_INTERPRETER, Value::Boolean(m.hosted_code_interpreter));
    g.set(ai::FEATURE_FILE_SEARCH, Value::Boolean(m.hosted_file_search));
    g.set(ai::FEATURE_WEB_SEARCH, Value::Boolean(m.hosted_web_search));
    g.set(ai::FEATURE_FUNCTION_CALLING, Value::Boolean(m.function_calling));
    g.set(ai::FEATURE_STREAMING, Value::Boolean(m.streaming));
    g.set(ai::VECTOR_STORE, Value::Integer(m.vector_store));
}

/// Copies the legacy global custom headers into every manual model whose own headers are
/// missing or empty, once (`iTermMigrationHelper.m:458-509`). Entries are rewritten in place, so
/// ones Settings cannot read survive. The flag is set only after the configurations are written.
/// True when anything changed (the caller saves).
pub fn migrate_legacy_headers(g: &mut Globals) -> bool {
    if g.bool(MIGRATION_FLAG) {
        return false;
    }
    if g.bool(ai::CUSTOM_HEADERS_ENABLED) {
        let headers = match g.get(ai::CUSTOM_HEADERS) {
            Some(Value::Array(list)) => list
                .iter()
                .filter_map(|h| {
                    let t = h.as_table()?;
                    let name = t.get("name")?.as_str()?;
                    Some((name.to_string(), t.get("value").and_then(Value::as_str).unwrap_or("").to_string()))
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        if let (false, Some(Value::Array(configs))) = (headers.is_empty(), g.get(ai::MANUAL_MODELS)) {
            if !configs.is_empty() {
                let migrated = configs
                    .into_iter()
                    .map(|entry| match entry {
                        Value::Table(mut t) => {
                            let needs = match t.get(CUSTOM_HEADERS) {
                                None => true,
                                Some(Value::Array(a)) => a.is_empty(),
                                Some(_) => false,
                            };
                            if needs {
                                t.insert(CUSTOM_HEADERS.to_string(), headers_value(&headers));
                            }
                            Value::Table(t)
                        }
                        other => other,
                    })
                    .collect();
                g.set(ai::MANUAL_MODELS, Value::Array(migrated));
            }
        }
    }
    g.set(MIGRATION_FLAG, Value::Boolean(true));
    true
}

/// "Import from iTerm2..." may have replaced the manual models and the legacy headers, so the
/// migration runs again over them. It fills only missing or empty header lists, as iTerm2's does.
pub fn migrate_after_import(g: &mut Globals) -> bool {
    g.set(MIGRATION_FLAG, Value::Boolean(false));
    migrate_legacy_headers(g)
}

// The Manual AI Models sheet (`G:282-560`) and the model editor (`G:645-1000`), their texts.
pub const SHEET_TITLE: &str = "Manual AI Models";
pub const COLUMN_MODEL: &str = "Model";
pub const COLUMN_API: &str = "API";
pub const COLUMN_ENDPOINT: &str = "Endpoint";
pub const ADD: &str = "Add";
pub const DELETE: &str = "Delete";
pub const EDIT: &str = "Edit";
pub const DUPLICATE: &str = "Duplicate";
pub const TOGGLE_DEFAULT: &str = "Toggle Default";
/// The economy toggle's text; iTerm2 shows a leaf symbol (Decision: no leaf glyph is assumed).
pub const ECONOMY: &str = "Economy";
pub const TOGGLE_ECONOMY: &str = "Toggle Economy Model. A cheaper model used for frequent background jobs like command-safety checks and screen-idle detection.";
/// Marks the economy row, where iTerm2 draws a leaf described as "Economy model".
pub const ECONOMY_MARK: &str = "Economy model";
/// iTerm2's names for a model whose name is not a string (`G:398-400`, `G:3833`); Tandem's
/// reader drops such an entry, so neither is shown. An empty name stays empty, as in iTerm2.
pub const UNTITLED: &str = "Untitled model";
pub const MANUAL_MODEL: &str = "Manual model";
pub const LEGACY_HEADING: &str = "Custom Headers Are Now Set Per Model";
pub const LEGACY_TEXT: &str = "Your AI custom HTTP headers used to be a single global setting. They still apply to your current model, but adding or editing models here does not carry them over. Re-add the headers you need in each model\u{2019}s \u{201c}Custom headers\u{201d} section.";
pub const ADD_TITLE: &str = "Add Manual AI Model";
pub const EDIT_TITLE: &str = "Edit Manual AI Model";
pub const PRESET_LABEL: &str = "Preset:";
pub const CUSTOM: &str = "Custom";
pub const EDITOR_MODEL_LABEL: &str = "Model:";
pub const FETCH_MODELS: &str = "Fetch Models";
pub const REFRESH_MODELS: &str = "Refresh Models";
pub const URL_LABEL: &str = "URL:";
pub const API_LABEL: &str = "API:";
pub const DISCOVER: &str = "Discover installed models automatically (Ollama)";
pub const CONTEXT_LABEL: &str = "Context tokens:";
pub const RESPONSE_LABEL: &str = "Max response tokens:";
pub const VECTOR_STORE_LABEL: &str = "Vector store:";
pub const VECTOR_DISABLED: &str = "Disabled";
pub const VECTOR_OPENAI: &str = "OpenAI";
pub const THINKING: &str = "Configurable thinking";
pub const THINKING_TIP: &str = "Enable for reasoning models with a thinking mode, such as GPT-5, o-series, or DeepSeek models, so the chat\u{2019}s Think toggle appears.";
pub const TEMPERATURE: &str = "Supports temperature";
pub const TEMPERATURE_TIP: &str = "Uncheck for models that reject a temperature parameter, such as Anthropic Opus 4.7 and later.";
pub const HEADERS_LABEL: &str = "Custom headers:";
pub const HEADER_COLUMN: &str = "Header";
pub const VALUE_COLUMN: &str = "Value";
pub const TEST_CONNECTION: &str = "Test Connection";
pub const CANCEL: &str = "Cancel";
pub const SAVE: &str = "Save";
pub const INVALID_TITLE: &str = "Invalid Manual AI Model";
pub const URL_REQUIRED: &str = "URL is required.";
pub const MODEL_REQUIRED: &str = "Model is required.";
pub const CONTEXT_POSITIVE: &str = "Context tokens must be greater than zero.";
pub const RESPONSE_POSITIVE: &str = "Max response tokens must be greater than zero.";
pub const NAMES_UNIQUE: &str = "Manual model names must be unique.";

pub fn invalid_header_name(name: &str) -> String {
    format!("Custom header name \u{201c}{name}\u{201d} is not valid. Use only RFC 7230 token characters (letters, digits, and any of !#$%&'*+-.^_`|~).")
}

pub fn invalid_header_value(name: &str) -> String {
    format!("The value for custom header \u{201c}{name}\u{201d} must not contain newline or null characters.")
}

/// The name of the manual model that is the default for new chats; None while the vendor's
/// recommended model is (`G:2688-2693`).
pub fn default_manual_name(g: &Globals) -> Option<String> {
    (!g.bool(ai::USE_RECOMMENDED)).then(|| g.string(ai::MODEL))
}

/// The economy model's name, "" when there is none (`G:2695-2702`).
pub fn economy_name(g: &Globals) -> String {
    g.string(ai::ECONOMY_MODEL)
}

/// The sheet's rows as (Model, API, Endpoint) (`G:388-420`): a star marks the default, the
/// text "Economy model" the economy model; an out-of-range API reads Chat Completions.
pub fn sheet_rows(models: &[ManualModel], default_name: Option<&str>, economy_name: &str) -> Vec<[String; 3]> {
    models
        .iter()
        .map(|m| {
            let shown = m.name.as_str();
            let name = if default_name == Some(m.name.as_str()) {
                format!("\u{2605} {shown}")
            } else if !economy_name.is_empty() && m.name == economy_name {
                format!("{ECONOMY_MARK} {shown}")
            } else {
                shown.to_string()
            };
            [name, m.api().title().to_string(), m.endpoint_host()]
        })
        .collect()
}

/// Which of the sheet's buttons take input (`G:486-514`): Add, Delete, Edit, Duplicate, Toggle
/// Default, Toggle Economy. The default cannot also be the economy model; a dynamic entry is a
/// whole server, so it can be neither.
pub fn segments_enabled(models: &[ManualModel], selected: Option<usize>, default_name: Option<&str>) -> [bool; 6] {
    let Some(m) = selected.and_then(|i| models.get(i)) else {
        return [true, false, false, false, false, false];
    };
    let is_default = default_name == Some(m.name.as_str());
    [true, true, true, true, !m.dynamic_models, !m.dynamic_models && !is_default]
}

/// The vendor's recommended model becomes the default again (`G:3990-3997`). An out-of-range
/// vendor only turns the recommended model on, as iTerm2 stores the raw value it read.
fn select_stored_provider(g: &mut Globals, catalog: &Catalog) {
    match Vendor::from_raw(g.int(ai::VENDOR)) {
        Some(v) => select_provider(g, v, catalog),
        None => g.set(ai::USE_RECOMMENDED, Value::Boolean(true)),
    }
}

/// Delete (`G:3838-3862`): the default's deletion makes the row now at its index (else the
/// last) the default, or the vendor's recommended model when none is left; the economy model's
/// deletion clears it. Answers the row to select next.
pub fn delete_row(g: &mut Globals, models: &mut Vec<ManualModel>, row: usize, catalog: &Catalog) -> Option<usize> {
    if row >= models.len() {
        return models.len().checked_sub(1).map(|last| row.min(last));
    }
    let deleted = models.remove(row).name;
    let deleting_default = default_manual_name(g).as_deref() == Some(deleted.as_str());
    let economy = economy_name(g);
    let deleting_economy = !economy.is_empty() && economy == deleted;
    let next = models.len().checked_sub(1).map(|last| row.min(last));
    write(g, models);
    if deleting_economy {
        g.set(ai::ECONOMY_MODEL, Value::String(String::new()));
    }
    if deleting_default {
        match next {
            Some(i) => select_manual(g, &models[i]),
            None => select_stored_provider(g, catalog),
        }
    }
    next
}

/// Toggle Default (`G:3864-3884`): the default row falls back to the vendor's recommended
/// model; another row becomes the default (and stops being the economy model).
pub fn toggle_default(g: &mut Globals, models: &[ManualModel], row: usize, catalog: &Catalog) {
    let Some(m) = models.get(row) else { return };
    write(g, models);
    if default_manual_name(g).as_deref() == Some(m.name.as_str()) {
        select_stored_provider(g, catalog);
    } else {
        select_manual(g, m);
    }
}

/// Toggle Economy (`G:3886-3910`): set or cleared; never on the default row or a nameless one.
pub fn toggle_economy(g: &mut Globals, models: &[ManualModel], row: usize) {
    let Some(m) = models.get(row) else { return };
    write(g, models);
    if m.name.is_empty() {
        return;
    }
    if economy_name(g) == m.name {
        g.set(ai::ECONOMY_MODEL, Value::String(String::new()));
    } else if default_manual_name(g).as_deref() != Some(m.name.as_str()) {
        g.set(ai::ECONOMY_MODEL, Value::String(m.name.clone()));
    }
}

/// Duplicate's editor starts from a copy with a new id and the name "<name> copy"
/// (`G:3826-3836`).
pub fn duplicate_base(m: &ManualModel) -> ManualModel {
    ManualModel { id: new_id(), name: format!("{} copy", m.name), ..m.clone() }
}

/// Add's editor starts from the stored model settings (`G:3673-3698`): the current model name
/// (never the Ollama placeholder), URL and API, the token limits (8192 when not positive), the
/// feature switches and the vector store, with a fresh id.
pub fn default_base(g: &Globals) -> ManualModel {
    let positive = |key: &str| Some(g.int(key)).filter(|n| *n > 0).unwrap_or(FALLBACK_TOKENS);
    let seed = g.string(ai::MODEL);
    let name = if seed.is_empty() || seed == OLLAMA_PENDING { "gpt-4o-mini".to_string() } else { seed };
    ManualModel {
        id: new_id(),
        name,
        url: g.string(ai::URL),
        api: g.int(ai::API),
        context_window_tokens: positive(ai::TOKEN_LIMIT),
        max_response_tokens: positive(ai::RESPONSE_TOKEN_LIMIT),
        hosted_code_interpreter: g.bool(ai::FEATURE_CODE_INTERPRETER),
        hosted_file_search: g.bool(ai::FEATURE_FILE_SEARCH),
        hosted_web_search: g.bool(ai::FEATURE_WEB_SEARCH),
        function_calling: g.bool(ai::FEATURE_FUNCTION_CALLING),
        streaming: g.bool(ai::FEATURE_STREAMING),
        vision: false,
        vector_store: g.int(ai::VECTOR_STORE),
        supports_temperature: None,
        configurable_thinking: None,
        dynamic_models: false,
        dynamic_selected_model: None,
        custom_headers: Vec::new(),
        extra: Table::new(),
    }
}

/// The editor saved (`G:3935-3982`): an edit replaces its row, carrying the economy name to a
/// renamed economy model and reapplying a renamed or changed default; an add appends. Answers
/// the row to select.
pub fn save_edit(g: &mut Globals, models: &mut Vec<ManualModel>, editing: Option<usize>, result: ManualModel, _catalog: &Catalog) -> usize {
    let Some(i) = editing.filter(|i| *i < models.len()) else {
        models.push(result);
        write(g, models);
        return models.len() - 1;
    };
    let old = std::mem::replace(&mut models[i], result);
    let editing_default = default_manual_name(g).as_deref() == Some(old.name.as_str());
    let economy = economy_name(g);
    let editing_economy = !economy.is_empty() && economy == old.name;
    write(g, models);
    if editing_economy {
        g.set(ai::ECONOMY_MODEL, Value::String(models[i].name.clone()));
    }
    if editing_default {
        select_manual(g, &models[i]);
    }
    i
}

/// The preset popup's "Custom" item (`G:699-700`).
pub const CUSTOM_PRESET: i64 = -1;
/// A provider preset's popup tag is this plus its index (`G:89`); a catalog preset's is its index.
pub const PROVIDER_PRESET_BASE: i64 = 100_000;

/// A third-party gateway to start a manual model from (`AITerm/AIProviderPreset.swift:18-66`):
/// the endpoint, API and capabilities, with the model name left for the person.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProviderPreset {
    pub name: &'static str,
    pub url: &'static str,
    pub api: Api,
    pub context: i64,
    pub response: i64,
    pub function_calling: bool,
    pub streaming: bool,
    pub thinking: bool,
    pub dynamic: bool,
    pub vision: bool,
    pub placeholder: &'static str,
}

const fn gateway(name: &'static str, url: &'static str, api: Api, thinking: bool, dynamic: bool, placeholder: &'static str) -> ProviderPreset {
    ProviderPreset { name, url, api, context: 128_000, response: 16_384, function_calling: true, streaming: true, thinking, dynamic, vision: false, placeholder }
}

/// iTerm2's provider presets (`AITerm/AIProviderPreset.swift:73-125`).
pub const PROVIDER_PRESETS: [ProviderPreset; 7] = [
    gateway("Ollama (auto-discover)", "http://localhost:11434/api/chat", Api::Llama, true, true, "qwen3.5"),
    gateway("Ollama (native)", "http://localhost:11434/api/chat", Api::Llama, true, false, "qwen3.5"),
    gateway("Local OpenAI-compatible (LM Studio, vLLM, \u{2026})", "http://localhost:1234/v1/chat/completions", Api::ChatCompletions, false, false, "qwen2.5-7b-instruct"),
    gateway("OpenRouter", "https://openrouter.ai/api/v1/chat/completions", Api::ChatCompletions, false, false, "openai/gpt-4o"),
    gateway("z.ai (GLM)", "https://api.z.ai/api/paas/v4/chat/completions", Api::ChatCompletions, false, false, "glm-4.6"),
    gateway("Qwen (DashScope)", "https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions", Api::ChatCompletions, false, false, "qwen-plus"),
    gateway("MiniMax", "https://api.minimax.io/v1/text/chatcompletion_v2", Api::ChatCompletions, false, false, "MiniMax-Text-01"),
];

/// The catalog models offered as presets: all but Apple's on-device classifier (`AM:371-373`).
pub fn catalog_presets(catalog: &Catalog) -> Vec<&Model> {
    catalog.models.iter().filter(|m| m.vendor != Vendor::Apple).collect()
}

/// The editor's six feature checkboxes in iTerm2's order (`G:826-833`), with their keys.
pub const FEATURES: [(&str, &str); 6] = [
    ("Function calling", FUNCTION_CALLING),
    ("Streaming responses", STREAMING),
    ("Vision (image input)", VISION),
    ("Hosted web search", HOSTED_WEB_SEARCH),
    ("Hosted file search", HOSTED_FILE_SEARCH),
    ("Hosted code interpreter", HOSTED_CODE_INTERPRETER),
];

/// The model editor's fields as the person sees them (`G:645-1000`). The API, vector store and
/// preset are popup tags; the token counts are the text fields' strings.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorForm {
    pub preset: i64,
    pub name: String,
    pub name_placeholder: String,
    pub url: String,
    pub api: i64,
    pub context: String,
    pub response: String,
    pub vector_store: i64,
    pub function_calling: bool,
    pub streaming: bool,
    pub vision: bool,
    pub web_search: bool,
    pub file_search: bool,
    pub code_interpreter: bool,
    pub thinking: bool,
    pub temperature: bool,
    pub dynamic: bool,
    pub headers: Vec<(String, String)>,
}

impl EditorForm {
    /// The form for a model (`G:645-1000`). An API the popup does not offer reads Chat
    /// Completions, a vector store other than OpenAI reads Disabled; the thinking and
    /// temperature boxes take a stored value, else the same-named catalog model's (`G:617-630`).
    pub fn from_model(m: &ManualModel, catalog: &Catalog) -> EditorForm {
        let api = Api::from_raw(m.api).filter(|a| Api::EDITOR_ORDER.contains(a)).unwrap_or(Api::ChatCompletions);
        let twin = catalog.named(&m.name);
        EditorForm {
            preset: CUSTOM_PRESET,
            name: m.name.clone(),
            name_placeholder: String::new(),
            url: m.url.clone(),
            api: api.raw(),
            context: m.context_window_tokens.to_string(),
            response: m.max_response_tokens.to_string(),
            vector_store: if m.vector_store == VectorStore::OpenAI.raw() { VectorStore::OpenAI.raw() } else { VectorStore::Disabled.raw() },
            function_calling: m.function_calling,
            streaming: m.streaming,
            vision: m.vision,
            web_search: m.hosted_web_search,
            file_search: m.hosted_file_search,
            code_interpreter: m.hosted_code_interpreter,
            thinking: m.configurable_thinking.unwrap_or_else(|| twin.is_some_and(|t| t.features.contains(&Feature::ConfigurableThinking))),
            temperature: m.supports_temperature.unwrap_or_else(|| twin.is_none_or(|t| t.supports_temperature)),
            dynamic: m.dynamic_models,
            headers: m.custom_headers.clone(),
        }
    }

    /// The feature checkbox at `FEATURES[i]`.
    pub fn feature_mut(&mut self, i: usize) -> &mut bool {
        match i {
            0 => &mut self.function_calling,
            1 => &mut self.streaming,
            2 => &mut self.vision,
            3 => &mut self.web_search,
            4 => &mut self.file_search,
            _ => &mut self.code_interpreter,
        }
    }

    /// Save (`G:1545-1640`, the non-dynamic path; saving a dynamic entry is 12b's): the checks in
    /// iTerm2's order, then the model with the base's id (a new one when it has none), the
    /// trimmed name and URL, the boxes' values, and the headers with a name (trimmed). Keys
    /// Settings does not know are not carried, as iTerm2 builds a new dictionary.
    pub fn validate(&self, base: &ManualModel, taken: &dyn Fn(&str) -> bool) -> Result<ManualModel, String> {
        let name = self.name.trim();
        let url = self.url.trim();
        let context = ns_integer_value(&self.context);
        let response = ns_integer_value(&self.response);
        if url.is_empty() {
            return Err(URL_REQUIRED.to_string());
        }
        if name.is_empty() {
            return Err(MODEL_REQUIRED.to_string());
        }
        if context <= 0 {
            return Err(CONTEXT_POSITIVE.to_string());
        }
        if response <= 0 {
            return Err(RESPONSE_POSITIVE.to_string());
        }
        if taken(name) {
            return Err(NAMES_UNIQUE.to_string());
        }
        let mut headers = Vec::new();
        for (n, v) in &self.headers {
            let n = n.trim();
            if n.is_empty() {
                continue;
            }
            if !ai_policy::header_name_valid(n) {
                return Err(invalid_header_name(n));
            }
            if !ai_policy::header_value_valid(v) {
                return Err(invalid_header_value(n));
            }
            headers.push((n.to_string(), v.clone()));
        }
        Ok(ManualModel {
            id: if base.id.is_empty() { new_id() } else { base.id.clone() },
            name: name.to_string(),
            url: url.to_string(),
            api: self.api,
            context_window_tokens: context,
            max_response_tokens: response,
            hosted_code_interpreter: self.code_interpreter,
            hosted_file_search: self.file_search,
            hosted_web_search: self.web_search,
            function_calling: self.function_calling,
            streaming: self.streaming,
            vision: self.vision,
            vector_store: self.vector_store,
            supports_temperature: Some(self.temperature),
            configurable_thinking: Some(self.thinking),
            dynamic_models: false,
            dynamic_selected_model: None,
            custom_headers: headers,
            extra: Table::new(),
        })
    }
}

/// A catalog preset fills every field from the model, its name included (`G:1263-1292`).
pub fn apply_catalog_preset(form: &mut EditorForm, m: &Model) {
    form.name = m.name.clone();
    form.url = m.url.clone();
    form.api = m.api.raw();
    form.context = m.context_window_tokens.to_string();
    form.response = m.max_response_tokens.to_string();
    form.vector_store = m.vector_store.raw();
    form.function_calling = m.features.contains(&Feature::FunctionCalling);
    form.streaming = m.features.contains(&Feature::Streaming);
    form.vision = m.features.contains(&Feature::Vision);
    form.web_search = m.features.contains(&Feature::HostedWebSearch);
    form.file_search = m.features.contains(&Feature::HostedFileSearch);
    form.code_interpreter = m.features.contains(&Feature::HostedCodeInterpreter);
    form.thinking = m.features.contains(&Feature::ConfigurableThinking);
    form.temperature = m.supports_temperature;
    form.dynamic = false;
}

/// A provider preset fills the endpoint and capabilities and clears the model name, showing the
/// host's example as its placeholder (`G:1299-1330`).
pub fn apply_provider_preset(form: &mut EditorForm, p: &ProviderPreset) {
    form.name.clear();
    form.name_placeholder = p.placeholder.to_string();
    form.url = p.url.to_string();
    form.api = p.api.raw();
    form.context = p.context.to_string();
    form.response = p.response.to_string();
    form.vector_store = VectorStore::Disabled.raw();
    form.function_calling = p.function_calling;
    form.streaming = p.streaming;
    form.vision = p.vision;
    form.web_search = false;
    form.file_search = false;
    form.code_interpreter = false;
    form.thinking = p.thinking;
    form.dynamic = p.dynamic;
    form.temperature = true;
}

/// Set once the legacy headers notice was shown.
pub const LEGACY_NOTICE_FLAG: &str = "NoSyncAILegacyGlobalHeadersWarningShown";

/// Whether opening the sheet first shows the legacy headers notice (`G:4005-4020`): legacy
/// global headers are on and set, and no manual model is configured, so they apply only
/// through the legacy model, which the sheet's first edit replaces.
pub fn legacy_notice_due(g: &Globals) -> bool {
    let non_empty = |key: &str| matches!(g.get(key), Some(Value::Array(a)) if !a.is_empty());
    !g.bool(LEGACY_NOTICE_FLAG) && g.bool(ai::CUSTOM_HEADERS_ENABLED) && non_empty(ai::CUSTOM_HEADERS) && !non_empty(ai::MANUAL_MODELS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_catalog::bundled;
    use std::path::Path;

    const CONFIGS: &str = r#"[
        {id="a", name="llama3", url="http://localhost:11434/api/chat", api=5, contextWindowTokens=32768, maxResponseTokens=4096, streaming=true, customHeaders=[{name="Authorization", value="Bearer x"}]},
        {name=7, url="x"},
        {name="n", url=""},
        {name="d", url="http://h:11434", dynamicModels=true},
        {name="b", url="https://api.example.com/v1", api="2", functionCalling=1, vision="YES", future="kept"},
    ]"#;

    fn value(toml_literal: &str) -> Value {
        let t: Table = format!("v = {toml_literal}").parse().expect("test TOML parses");
        t["v"].clone()
    }

    fn empty() -> Globals {
        Globals::empty(Path::new("ai-models-test.toml"))
    }

    fn globals_with(configs: &str) -> Globals {
        let mut g = empty();
        g.set(ai::MANUAL_MODELS, value(configs));
        g
    }

    fn one(configs: &str) -> ManualModel {
        let g = globals_with(configs);
        read(&g).remove(0)
    }

    fn named<'a>(models: &'a [ManualModel], name: &str) -> &'a ManualModel {
        models.iter().find(|m| m.name == name).expect("model present")
    }

    #[test]
    fn manual_models_read_like_iterm2() {
        let g = globals_with(CONFIGS);
        let models = read(&g);
        let names: Vec<&str> = models.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["llama3", "n", "d", "b"]);

        let l = named(&models, "llama3");
        assert_eq!(l.id, "a");
        assert_eq!(l.url, "http://localhost:11434/api/chat");
        assert_eq!(l.api, 5);
        assert_eq!(l.api(), Api::Llama);
        assert_eq!((l.context_window_tokens, l.max_response_tokens), (32768, 4096));
        assert!(l.streaming);
        assert!(!l.hosted_code_interpreter && !l.hosted_file_search && !l.hosted_web_search && !l.function_calling && !l.vision && !l.dynamic_models);
        assert_eq!(l.vector_store, 0);
        assert_eq!(l.supports_temperature, None);
        assert_eq!(l.configurable_thinking, None);
        assert_eq!(l.dynamic_selected_model, None);
        assert_eq!(l.custom_headers, [("Authorization".to_string(), "Bearer x".to_string())]);
        assert!(l.extra.is_empty());

        let b = named(&models, "b");
        assert_eq!(b.api, 2, "a numeric string reads as NSString integerValue");
        assert_eq!(b.api(), Api::Responses);
        assert!(b.function_calling, "integer 1 reads as true");
        assert!(b.vision, "\"YES\" reads as NSString boolValue");
        assert!(!b.streaming);
        assert_eq!((b.context_window_tokens, b.max_response_tokens), (8192, 8192));
        assert_eq!(b.extra.get("future").and_then(|v| v.as_str()), Some("kept"));
        assert_eq!(b.extra.len(), 1);
        assert_eq!(b.id, "", "a missing id reads empty, so repeated reads agree");

        assert!(named(&models, "d").dynamic_models);
        // No `api` reads Chat Completions.
        assert_eq!(named(&models, "n").api, 1);
        assert_eq!(named(&models, "n").api(), Api::ChatCompletions);

        let usable_names: Vec<&str> = usable(&models).iter().map(|m| m.name.as_str()).collect();
        assert_eq!(usable_names, ["llama3", "b"]);
        let blank = one(r#"[{name="  ", url="u"}, {name="x", url=" \n"}]"#);
        assert!(usable(&[blank]).is_empty(), "a whitespace-only name is not usable");

        assert_eq!(l.endpoint_host(), "localhost");
        assert_eq!(one(r#"[{name="x", url="not a url"}]"#).endpoint_host(), "not a url");
        assert_eq!(one(r#"[{name="x", url=""}]"#).endpoint_host(), "");

        // Number and string readers follow NSNumber and NSString.
        let odd = one(r#"[{name="o", url="u", api=2.9, contextWindowTokens=true, maxResponseTokens=" 12abc", streaming=2, vision=" t", functionCalling="no", hostedWebSearch="0", hostedFileSearch=0.5, vectorStore=false, supportsTemperature="Y", configurableThinking=0, dynamicSelectedModel="qwen3"}]"#);
        assert_eq!(odd.api, 2, "a float truncates");
        assert_eq!(odd.context_window_tokens, 1, "a boolean reads 0 or 1");
        assert_eq!(odd.max_response_tokens, 12, "a string's leading integer");
        assert!(odd.streaming, "a non-zero integer is true");
        assert!(odd.vision, "leading whitespace, then t");
        assert!(!odd.function_calling);
        assert!(!odd.hosted_web_search, "\"0\" is false");
        assert!(odd.hosted_file_search, "a non-zero float is true");
        assert_eq!(odd.vector_store, 0);
        assert_eq!(odd.supports_temperature, Some(true));
        assert_eq!(odd.configurable_thinking, Some(false));
        assert_eq!(odd.dynamic_selected_model.as_deref(), Some("qwen3"));

        // Header entries: a non-string name or value drops the entry; a missing value reads "".
        let h = one(r#"[{name="h", url="u", customHeaders=[{name=5, value="v"}, {name="A", value=7}, {name="B"}, {name="C", value="3"}, "junk"]}]"#);
        assert_eq!(h.custom_headers, [("B".to_string(), String::new()), ("C".to_string(), "3".to_string())]);

        // The sheet: the configurations, else the legacy entry when not using the recommended model.
        let mut legacy = empty();
        legacy.set(ai::USE_RECOMMENDED, Value::Boolean(false));
        legacy.set(ai::URL, Value::String("https://my.host/v1/chat/completions".into()));
        legacy.set(ai::MODEL, Value::String("m".into()));
        legacy.set(ai::API, Value::Integer(1));
        legacy.set(ai::TOKEN_LIMIT, Value::Integer(4000));
        legacy.set(ai::RESPONSE_TOKEN_LIMIT, Value::Integer(500));
        legacy.set(ai::FEATURE_STREAMING, Value::Boolean(true));
        let sheet = sheet_models(&legacy);
        assert_eq!(sheet.len(), 1);
        assert_eq!(sheet[0].name, "m");
        assert_eq!(sheet[0].url, "https://my.host/v1/chat/completions");
        assert_eq!(sheet[0].api, 1);
        assert_eq!((sheet[0].context_window_tokens, sheet[0].max_response_tokens), (4000, 500));
        assert!(sheet[0].streaming && !sheet[0].function_calling);
        assert!(!sheet[0].id.is_empty());
        legacy.set(ai::USE_RECOMMENDED, Value::Boolean(true));
        assert!(sheet_models(&legacy).is_empty());
        assert_eq!(sheet_models(&g), models);
    }

    #[test]
    fn manual_models_round_trip() {
        let g = globals_with(CONFIGS);
        let models = read(&g);
        for name in ["llama3", "b"] {
            let m = named(&models, name);
            assert_eq!(ManualModel::from_table(&m.to_table()).as_ref(), Some(m), "{name}");
        }
        let t = named(&models, "llama3").to_table();
        for key in [
            "id",
            "name",
            "url",
            "api",
            "contextWindowTokens",
            "maxResponseTokens",
            "hostedCodeInterpreter",
            "hostedFileSearch",
            "hostedWebSearch",
            "functionCalling",
            "streaming",
            "vision",
            "vectorStore",
            "dynamicModels",
            "customHeaders",
        ] {
            assert!(t.contains_key(key), "{key}");
        }
        for key in ["supportsTemperature", "configurableThinking", "dynamicSelectedModel"] {
            assert!(!t.contains_key(key), "{key} is written only when set");
        }
        assert_eq!(t.len(), 15);
        assert_eq!(t["api"].as_integer(), Some(5));
        assert_eq!(t["streaming"].as_bool(), Some(true));
        assert_eq!(t["customHeaders"], value(r#"[{name="Authorization", value="Bearer x"}]"#));
        let tb = named(&models, "b").to_table();
        assert_eq!(tb["api"].as_integer(), Some(2));
        assert_eq!(tb["future"].as_str(), Some("kept"));

        let mut set = named(&models, "llama3").clone();
        set.supports_temperature = Some(false);
        set.configurable_thinking = Some(true);
        set.dynamic_selected_model = Some("qwen3".into());
        let ts = set.to_table();
        assert_eq!(ts["supportsTemperature"].as_bool(), Some(false));
        assert_eq!(ts["configurableThinking"].as_bool(), Some(true));
        assert_eq!(ts["dynamicSelectedModel"].as_str(), Some("qwen3"));
        assert_eq!(ManualModel::from_table(&ts), Some(set));

        let mut empty_name = named(&models, "llama3").clone();
        empty_name.name = String::new();
        let mut empty_url = named(&models, "llama3").clone();
        empty_url.name = "x".into();
        empty_url.url = String::new();
        let mut w = empty();
        write(&mut w, &[named(&models, "llama3").clone(), empty_name, empty_url]);
        assert_eq!(read(&w), [named(&models, "llama3").clone()]);
        let stored = w.get(ai::MANUAL_MODELS).unwrap();
        assert_eq!(stored.as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn manual_vendor_follows_iterm2() {
        for (api, v) in [
            (Api::Anthropic, Vendor::Anthropic),
            (Api::Gemini, Vendor::Gemini),
            (Api::Llama, Vendor::Llama),
            (Api::DeepSeek, Vendor::DeepSeek),
            (Api::AppleIntelligence, Vendor::Apple),
        ] {
            assert_eq!(manual_vendor(api, "https://api.openai.com/v1/responses", "gpt-4o"), v, "{api:?}");
        }
        for (name, v) in [("anthropic/claude-3.5", Vendor::Anthropic), ("my-gemini-proxy", Vendor::Gemini), ("DeepSeek-R1", Vendor::DeepSeek), ("llama3.1:8b", Vendor::Llama)] {
            assert_eq!(manual_vendor(Api::ChatCompletions, "https://api.openai.com/v1/chat/completions", name), v, "{name}");
            assert_eq!(vendor_for_model_name(name), Some(v), "{name}");
        }
        assert_eq!(vendor_for_model_name("m"), None);
        for (url, v) in [
            ("https://foo.anthropic.com/v1", Vendor::Anthropic),
            ("https://generativelanguage.googleapis.com/v1beta/openai", Vendor::Gemini),
            ("https://api.deepseek.com/x", Vendor::DeepSeek),
            ("https://x.openai.azure.com/o", Vendor::OpenAI),
            ("http://localhost:1234/v1/chat/completions", Vendor::OpenAI),
        ] {
            assert_eq!(manual_vendor(Api::ChatCompletions, url, "m"), v, "{url}");
        }
        assert_eq!(manual_vendor(Api::Responses, "https://foo.anthropic.com/v1", "m"), Vendor::Anthropic);
    }

    #[test]
    fn default_endpoint_follows_iterm2() {
        let cat = bundled();
        let mut g = empty();
        assert_eq!(
            default_endpoint(&g, cat),
            Some(Endpoint { vendor: Vendor::OpenAI, model: "gpt-5.6-sol".into(), url: "https://api.openai.com/v1/responses".into(), api: Api::Responses, custom_headers: vec![] })
        );
        assert_eq!(effective_vendor(&g, cat), Vendor::OpenAI);

        g.set(ai::VENDOR, Value::Integer(4));
        let e = default_endpoint(&g, cat).unwrap();
        assert_eq!((e.vendor, e.model.as_str(), e.url.as_str(), e.api), (Vendor::Anthropic, "claude-opus-4-8", "https://api.anthropic.com/v1/messages", Api::Anthropic));
        assert_eq!(effective_vendor(&g, cat), Vendor::Anthropic);

        g.set(ai::VENDOR, Value::Integer(3));
        assert_eq!(
            default_endpoint(&g, cat),
            Some(Endpoint { vendor: Vendor::Llama, model: OLLAMA_PENDING.into(), url: OLLAMA_ENDPOINT.into(), api: Api::Llama, custom_headers: vec![] })
        );
        g.set(ai::OLLAMA_REGULAR, Value::String("qwen3".into()));
        assert_eq!(default_endpoint(&g, cat).unwrap().model, "qwen3");
        assert_eq!(effective_vendor(&g, cat), Vendor::Llama);

        g.set(ai::VENDOR, Value::Integer(5));
        assert_eq!(default_endpoint(&g, cat), None, "Apple Intelligence is not available on Windows");
        assert_eq!(effective_vendor(&g, cat), Vendor::Apple);
        g.set(ai::VENDOR, Value::Integer(9));
        assert_eq!(default_endpoint(&g, cat), None, "an out-of-range vendor falls to the manual path and finds nothing");
        assert_eq!(effective_vendor(&g, cat), Vendor::OpenAI);

        let mut m = globals_with(CONFIGS);
        m.set(ai::USE_RECOMMENDED, Value::Boolean(false));
        m.set(ai::MODEL, Value::String("b".into()));
        let e = default_endpoint(&m, cat).unwrap();
        assert_eq!((e.vendor, e.model.as_str(), e.url.as_str(), e.api), (Vendor::OpenAI, "b", "https://api.example.com/v1", Api::Responses));
        assert!(e.custom_headers.is_empty());
        assert_eq!(effective_vendor(&m, cat), Vendor::OpenAI);
        m.set(ai::MODEL, Value::String("missing".into()));
        let e = default_endpoint(&m, cat).unwrap();
        assert_eq!((e.vendor, e.model.as_str(), e.api), (Vendor::Llama, "llama3", Api::Llama));
        assert_eq!(e.custom_headers, [("Authorization".to_string(), "Bearer x".to_string())]);
        assert_eq!(effective_vendor(&m, cat), Vendor::Llama);
        m.set(ai::MODEL, Value::String("d".into()));
        assert_eq!(default_endpoint(&m, cat).unwrap().model, "llama3", "a dynamic entry is not a default in 12a");

        let mut legacy = empty();
        legacy.set(ai::USE_RECOMMENDED, Value::Boolean(false));
        legacy.set(ai::URL, Value::String("https://my.host/v1/chat/completions".into()));
        legacy.set(ai::MODEL, Value::String("m".into()));
        legacy.set(ai::API, Value::Integer(1));
        legacy.set(ai::CUSTOM_HEADERS_ENABLED, Value::Boolean(true));
        legacy.set(ai::CUSTOM_HEADERS, value(r#"[{name="X-K", value="1"}]"#));
        assert_eq!(
            default_endpoint(&legacy, cat),
            Some(Endpoint {
                vendor: Vendor::OpenAI,
                model: "m".into(),
                url: "https://my.host/v1/chat/completions".into(),
                api: Api::ChatCompletions,
                custom_headers: vec![("X-K".into(), "1".into())]
            })
        );
        legacy.set(ai::CUSTOM_HEADERS_ENABLED, Value::Boolean(false));
        assert!(default_endpoint(&legacy, cat).unwrap().custom_headers.is_empty());
        legacy.set(ai::URL, Value::String(String::new()));
        assert_eq!(default_endpoint(&legacy, cat), None);
        assert_eq!(effective_vendor(&legacy, cat), Vendor::OpenAI);
    }

    #[test]
    fn select_provider_writes_like_iterm2() {
        let cat = bundled();
        let mut g = empty();
        g.set(ai::USE_RECOMMENDED, Value::Boolean(false));
        select_provider(&mut g, Vendor::Anthropic, cat);
        assert!(g.bool(ai::USE_RECOMMENDED));
        assert_eq!(g.int(ai::VENDOR), 4);
        assert_eq!(g.string(ai::MODEL), "claude-opus-4-8");
        assert_eq!(g.int(ai::API), 7);
        assert_eq!((g.int(ai::TOKEN_LIMIT), g.int(ai::RESPONSE_TOKEN_LIMIT)), (200000, 128000));
        assert_eq!(g.string(ai::URL), "https://api.anthropic.com/v1/messages");
        assert!(g.bool(ai::FEATURE_FUNCTION_CALLING) && g.bool(ai::FEATURE_STREAMING));
        assert!(!g.bool(ai::FEATURE_CODE_INTERPRETER) && !g.bool(ai::FEATURE_FILE_SEARCH) && !g.bool(ai::FEATURE_WEB_SEARCH));
        assert_eq!(g.int(ai::VECTOR_STORE), 0);

        select_provider(&mut g, Vendor::OpenAI, cat);
        assert_eq!(g.int(ai::VENDOR), 2);
        assert_eq!(g.string(ai::MODEL), "gpt-5.6-sol");
        assert_eq!(g.int(ai::API), 2);
        assert!(g.bool(ai::FEATURE_CODE_INTERPRETER) && g.bool(ai::FEATURE_FILE_SEARCH) && g.bool(ai::FEATURE_WEB_SEARCH));

        select_provider(&mut g, Vendor::Llama, cat);
        assert_eq!(g.int(ai::VENDOR), 3);
        assert_eq!(g.string(ai::MODEL), "gpt-5.6-sol", "the Ollama placeholder is never stored");

        let mut n = empty();
        n.set(ai::MODEL, Value::String("not-in-catalog".into()));
        let before = n.values.clone();
        model_did_change(&mut n, cat);
        assert_eq!(n.values, before, "a model the catalog does not know writes nothing");
        n.set(ai::MODEL, Value::String("my-claude".into()));
        model_did_change(&mut n, cat);
        assert_eq!(n.int(ai::API), 7, "iTerm2 guesses the API from the name");
    }

    #[test]
    fn select_manual_writes_like_iterm2() {
        let g0 = globals_with(CONFIGS);
        let models = read(&g0);
        let b = named(&models, "b");
        let mut g = empty();
        g.set(ai::ECONOMY_MODEL, Value::String("b".into()));
        g.set(ai::FEATURE_STREAMING, Value::Boolean(true));
        g.set(ai::VECTOR_STORE, Value::Integer(1));
        select_manual(&mut g, b);
        assert!(!g.bool(ai::USE_RECOMMENDED));
        assert_eq!(g.string(ai::MODEL), "b");
        assert_eq!(g.string(ai::URL), "https://api.example.com/v1");
        assert_eq!(g.int(ai::API), 2);
        assert_eq!((g.int(ai::TOKEN_LIMIT), g.int(ai::RESPONSE_TOKEN_LIMIT)), (8192, 8192));
        assert!(g.bool(ai::FEATURE_FUNCTION_CALLING));
        assert!(!g.bool(ai::FEATURE_STREAMING) && !g.bool(ai::FEATURE_CODE_INTERPRETER) && !g.bool(ai::FEATURE_FILE_SEARCH) && !g.bool(ai::FEATURE_WEB_SEARCH));
        assert_eq!(g.int(ai::VECTOR_STORE), 0);
        assert_eq!(g.string(ai::ECONOMY_MODEL), "", "a default cannot also be the economy model");

        // A dynamic entry has no single model name, so it never becomes the default (`G:2981`).
        let mut d = empty();
        let before = d.values.clone();
        select_manual(&mut d, named(&models, "d"));
        assert_eq!(d.values, before);
    }

    #[test]
    fn legacy_headers_migrate_once() {
        let configs = r#"[{name="a", url="u"}, {name="b", url="u", customHeaders=[{name="Z", value="2"}]}, {name="c", url="u", customHeaders=[]}]"#;
        let mut g = globals_with(configs);
        g.set(ai::CUSTOM_HEADERS_ENABLED, Value::Boolean(true));
        g.set(ai::CUSTOM_HEADERS, value(r#"[{name="X-Key", value="1"}, {name=5}, {name="Y", value=7}]"#));
        assert!(migrate_legacy_headers(&mut g));
        let models = read(&g);
        let migrated = vec![("X-Key".to_string(), "1".to_string()), ("Y".to_string(), String::new())];
        assert_eq!(named(&models, "a").custom_headers, migrated);
        assert_eq!(named(&models, "b").custom_headers, [("Z".to_string(), "2".to_string())]);
        assert_eq!(named(&models, "c").custom_headers, migrated);
        assert!(g.bool(MIGRATION_FLAG));
        let after = g.values.clone();
        assert!(!migrate_legacy_headers(&mut g));
        assert_eq!(g.values, after);

        let mut off = globals_with(configs);
        off.set(ai::CUSTOM_HEADERS, value(r#"[{name="X-Key", value="1"}]"#));
        let configs_before = off.get(ai::MANUAL_MODELS);
        assert!(migrate_legacy_headers(&mut off), "the flag was set");
        assert_eq!(off.get(ai::MANUAL_MODELS), configs_before);
        assert!(off.bool(MIGRATION_FLAG));

        let mut none = empty();
        none.set(ai::CUSTOM_HEADERS_ENABLED, Value::Boolean(true));
        none.set(ai::CUSTOM_HEADERS, value(r#"[{name="X-Key", value="1"}]"#));
        assert!(migrate_legacy_headers(&mut none));
        let mut expect: Vec<&str> = vec![ai::CUSTOM_HEADERS, ai::CUSTOM_HEADERS_ENABLED, MIGRATION_FLAG];
        expect.sort();
        assert_eq!(none.values.keys().map(String::as_str).collect::<Vec<_>>(), expect, "only the flag is added");

        // Imported settings run the migration again over the imported configurations.
        let mut imported = globals_with(configs);
        imported.set(MIGRATION_FLAG, Value::Boolean(true));
        imported.set(ai::CUSTOM_HEADERS_ENABLED, Value::Boolean(true));
        imported.set(ai::CUSTOM_HEADERS, value(r#"[{name="X-Key", value="1"}]"#));
        assert!(migrate_after_import(&mut imported));
        assert_eq!(named(&read(&imported), "a").custom_headers, [("X-Key".to_string(), "1".to_string())]);
        assert!(imported.bool(MIGRATION_FLAG));

        // An entry that is not a table survives unchanged; a table whose name is not a string
        // is still a dictionary to iTerm2, so it gets the headers and keeps everything else
        // (`iTermMigrationHelper.m:483-498`).
        let mut odd = globals_with(r#"["loose", 7, {name=7, url="u", extra="kept"}, {name="a", url="u"}]"#);
        odd.set(ai::CUSTOM_HEADERS_ENABLED, Value::Boolean(true));
        odd.set(ai::CUSTOM_HEADERS, value(r#"[{name="X-Key", value="1"}]"#));
        assert!(migrate_legacy_headers(&mut odd));
        let Some(Value::Array(after)) = odd.get(ai::MANUAL_MODELS) else { panic!("configurations kept") };
        assert_eq!(after.len(), 4);
        assert_eq!(after[0], Value::String("loose".into()));
        assert_eq!(after[1], Value::Integer(7));
        assert_eq!(after[2], value(r#"{name=7, url="u", extra="kept", customHeaders=[{name="X-Key", value="1"}]}"#));
        assert_eq!(named(&read(&odd), "a").custom_headers, [("X-Key".to_string(), "1".to_string())]);
    }

    fn model(name: &str, url: &str, api: i64) -> ManualModel {
        let mut t = Table::new();
        t.insert(NAME.into(), Value::String(name.into()));
        t.insert(URL.into(), Value::String(url.into()));
        t.insert(API.into(), Value::Integer(api));
        ManualModel::from_table(&t).expect("a manual model")
    }

    const LOCAL: &str = "http://localhost:11434/api/chat";
    const EXAMPLE: &str = "https://api.example.com/v1/responses";

    fn row(a: &str, b: &str, c: &str) -> [String; 3] {
        [a.to_string(), b.to_string(), c.to_string()]
    }

    #[test]
    fn sheet_rows_follow_iterm2() {
        let models = [model("llama3", LOCAL, 5), model("b", EXAMPLE, 2), model("n", "not a url", 99)];
        assert_eq!(
            sheet_rows(&models, Some("b"), "llama3"),
            [row("Economy model llama3", "Ollama", "localhost"), row("\u{2605} b", "Responses", "api.example.com"), row("n", "Chat Completions", "not a url")]
        );
        let rows = sheet_rows(&models, None, "");
        assert!(rows.iter().all(|r| !r[0].contains('\u{2605}')));
        assert_eq!(rows[0][0], "llama3");
        assert_eq!(sheet_rows(&[model("", "u", 1)], None, "")[0][0], "", "an empty name shows empty (G:398-400)");

        let mut g = empty();
        assert_eq!(default_manual_name(&g), None, "the recommended model is in use");
        assert_eq!(economy_name(&g), "");
        g.set(ai::USE_RECOMMENDED, Value::Boolean(false));
        g.set(ai::MODEL, Value::String("b".into()));
        g.set(ai::ECONOMY_MODEL, Value::String("llama3".into()));
        assert_eq!(default_manual_name(&g).as_deref(), Some("b"));
        assert_eq!(economy_name(&g), "llama3");
    }

    #[test]
    fn segments_follow_iterm2() {
        let mut dynamic = model("d", "http://h:11434/api/chat", 5);
        dynamic.dynamic_models = true;
        let models = [model("llama3", LOCAL, 5), model("b", EXAMPLE, 2), dynamic];
        assert_eq!(segments_enabled(&models, None, Some("b")), [true, false, false, false, false, false]);
        assert_eq!(segments_enabled(&models, Some(7), Some("b")), [true, false, false, false, false, false], "a stale selection is none");
        assert_eq!(segments_enabled(&models, Some(1), Some("b")), [true, true, true, true, true, false]);
        assert_eq!(segments_enabled(&models, Some(0), Some("b")), [true; 6]);
        assert_eq!(segments_enabled(&models, Some(2), Some("b")), [true, true, true, true, false, false]);
    }

    #[test]
    fn sheet_actions_follow_iterm2() {
        let cat = bundled();
        let setup = || {
            let models = vec![model("b", EXAMPLE, 2), model("llama3", LOCAL, 5), model("n", "https://n.example.com/v1", 1)];
            let mut g = empty();
            write(&mut g, &models);
            select_manual(&mut g, &models[0]);
            g.set(ai::ECONOMY_MODEL, Value::String("llama3".into()));
            (g, models)
        };
        let stored = |g: &Globals| read(g).iter().map(|m| m.name.clone()).collect::<Vec<_>>();

        // Deleting the default makes the row now at its index the default, which also stops it
        // being the economy model.
        let (mut g, mut models) = setup();
        assert_eq!(delete_row(&mut g, &mut models, 0, cat), Some(0));
        assert_eq!(stored(&g), ["llama3", "n"], "write ran");
        assert_eq!(default_manual_name(&g).as_deref(), Some("llama3"));
        assert_eq!(g.string(ai::URL), LOCAL, "select_manual ran");
        assert_eq!(economy_name(&g), "");

        // Deleting the last row: the new last row becomes the default.
        let (mut g, mut models) = setup();
        let b = models.remove(0);
        models.push(b);
        write(&mut g, &models);
        assert_eq!(delete_row(&mut g, &mut models, 2, cat), Some(1));
        assert_eq!(default_manual_name(&g).as_deref(), Some("n"));
        assert_eq!(economy_name(&g), "llama3");

        // Deleting the economy model clears it; the default stays.
        let (mut g, mut models) = setup();
        assert_eq!(delete_row(&mut g, &mut models, 1, cat), Some(1));
        assert_eq!(economy_name(&g), "");
        assert_eq!(default_manual_name(&g).as_deref(), Some("b"));

        // Deleting the only model falls back to the vendor's recommended model.
        let mut g = empty();
        g.set(ai::VENDOR, Value::Integer(Vendor::Anthropic.raw()));
        let mut only = vec![model("b", EXAMPLE, 2)];
        write(&mut g, &only);
        select_manual(&mut g, &only[0]);
        assert_eq!(delete_row(&mut g, &mut only, 0, cat), None);
        assert!(g.bool(ai::USE_RECOMMENDED));
        assert_eq!(g.string(ai::MODEL), "claude-opus-4-8", "select_provider(AIVendor) ran");
        assert!(read(&g).is_empty());

        // Toggle Default on the default row falls back to the provider.
        let (mut g, models) = setup();
        toggle_default(&mut g, &models, 0, cat);
        assert!(g.bool(ai::USE_RECOMMENDED));
        assert_eq!(default_manual_name(&g), None);
        assert_eq!(g.string(ai::MODEL), "gpt-5.6-sol");
        // On the economy row it makes it the default and clears the economy name.
        let (mut g, models) = setup();
        g.set(ai::MANUAL_MODELS, Value::Array(Vec::new()));
        toggle_default(&mut g, &models, 1, cat);
        assert_eq!(stored(&g), ["b", "llama3", "n"], "write ran");
        assert_eq!(default_manual_name(&g).as_deref(), Some("llama3"));
        assert_eq!(economy_name(&g), "");

        // Toggle Economy: not on the default, set on another row, cleared by a second toggle.
        let (mut g, models) = setup();
        g.set(ai::MANUAL_MODELS, Value::Array(Vec::new()));
        toggle_economy(&mut g, &models, 0);
        assert_eq!(stored(&g), ["b", "llama3", "n"], "write ran");
        assert_eq!(economy_name(&g), "llama3");
        toggle_economy(&mut g, &models, 2);
        assert_eq!(economy_name(&g), "n");
        toggle_economy(&mut g, &models, 2);
        assert_eq!(economy_name(&g), "");

        // Save: renaming the economy model carries the name.
        let (mut g, mut models) = setup();
        let mut renamed = models[1].clone();
        renamed.name = "llama3.1".into();
        assert_eq!(save_edit(&mut g, &mut models, Some(1), renamed, cat), 1);
        assert_eq!(stored(&g), ["b", "llama3.1", "n"]);
        assert_eq!(economy_name(&g), "llama3.1");
        // Renaming the default reapplies select_manual.
        let mut renamed = models[0].clone();
        renamed.name = "b2".into();
        renamed.url = "https://b2.example.com/v1".into();
        assert_eq!(save_edit(&mut g, &mut models, Some(0), renamed, cat), 0);
        assert_eq!(default_manual_name(&g).as_deref(), Some("b2"));
        assert_eq!(g.string(ai::URL), "https://b2.example.com/v1");
        // Editing another row changes neither name.
        let mut other = models[2].clone();
        other.api = 2;
        assert_eq!(save_edit(&mut g, &mut models, Some(2), other, cat), 2);
        assert_eq!(read(&g)[2].api, 2);
        assert_eq!((default_manual_name(&g).as_deref(), economy_name(&g).as_str()), (Some("b2"), "llama3.1"));
        // Adding appends and answers the new index.
        assert_eq!(save_edit(&mut g, &mut models, None, model("new", "https://x.example.com", 1), cat), 3);
        assert_eq!(stored(&g), ["b2", "llama3.1", "n", "new"]);
        assert_eq!(default_manual_name(&g).as_deref(), Some("b2"));
    }

    #[test]
    fn editor_presets_follow_iterm2() {
        let expected = [
            ("Ollama (auto-discover)", "http://localhost:11434/api/chat", "qwen3.5"),
            ("Ollama (native)", "http://localhost:11434/api/chat", "qwen3.5"),
            ("Local OpenAI-compatible (LM Studio, vLLM, \u{2026})", "http://localhost:1234/v1/chat/completions", "qwen2.5-7b-instruct"),
            ("OpenRouter", "https://openrouter.ai/api/v1/chat/completions", "openai/gpt-4o"),
            ("z.ai (GLM)", "https://api.z.ai/api/paas/v4/chat/completions", "glm-4.6"),
            ("Qwen (DashScope)", "https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions", "qwen-plus"),
            ("MiniMax", "https://api.minimax.io/v1/text/chatcompletion_v2", "MiniMax-Text-01"),
        ];
        assert_eq!(PROVIDER_PRESETS.map(|p| (p.name, p.url, p.placeholder)), expected);
        for (i, p) in PROVIDER_PRESETS.iter().enumerate() {
            let ollama = i < 2;
            assert_eq!(p.api, if ollama { Api::Llama } else { Api::ChatCompletions }, "{}", p.name);
            assert_eq!((p.context, p.response), (128000, 16384), "{}", p.name);
            assert!(p.function_calling && p.streaming, "{}", p.name);
            assert_eq!(p.thinking, ollama, "{}", p.name);
            assert_eq!(p.dynamic, i == 0, "{}", p.name);
            assert!(!p.vision, "{}", p.name);
        }

        let cat = bundled();
        let presets = catalog_presets(cat);
        assert_eq!(presets.len(), 39);
        assert!(presets.iter().all(|m| m.vendor != Vendor::Apple));

        let mut form = EditorForm::from_model(&model("x", "u", 5), cat);
        form.vision = true;
        form.temperature = true;
        form.dynamic = true;
        form.vector_store = 1;
        let opus = cat.named("claude-opus-4-8").expect("claude-opus-4-8 is in the catalog");
        apply_catalog_preset(&mut form, opus);
        assert_eq!((form.name.as_str(), form.url.as_str(), form.api), ("claude-opus-4-8", "https://api.anthropic.com/v1/messages", 7));
        assert_eq!((form.context.as_str(), form.response.as_str(), form.vector_store), ("200000", "128000", 0));
        assert!(form.function_calling && form.streaming);
        assert!(!form.vision && !form.web_search && !form.file_search && !form.code_interpreter);
        assert!(!form.thinking && !form.temperature && !form.dynamic);

        let open_router = &PROVIDER_PRESETS[3];
        apply_provider_preset(&mut form, open_router);
        assert_eq!((form.name.as_str(), form.name_placeholder.as_str()), ("", "openai/gpt-4o"));
        assert_eq!((form.url.as_str(), form.api), ("https://openrouter.ai/api/v1/chat/completions", 1));
        assert_eq!((form.context.as_str(), form.response.as_str(), form.vector_store), ("128000", "16384", 0));
        assert!(form.function_calling && form.streaming && !form.vision);
        assert!(!form.web_search && !form.file_search && !form.code_interpreter);
        assert!(form.temperature && !form.thinking && !form.dynamic);
        apply_provider_preset(&mut form, &PROVIDER_PRESETS[0]);
        assert!(form.dynamic && form.thinking);
        assert_eq!(form.api, Api::Llama.raw());

        // The form reads the model; the quirk boxes fall back to the same-named catalog model.
        let mut stored = model("claude-opus-4-8", "https://proxy.example.com/v1", 7);
        stored.context_window_tokens = 1000;
        stored.custom_headers = vec![("X-A".into(), "1".into())];
        let f = EditorForm::from_model(&stored, cat);
        assert_eq!((f.preset, f.name.as_str(), f.name_placeholder.as_str(), f.url.as_str(), f.api), (CUSTOM_PRESET, "claude-opus-4-8", "", "https://proxy.example.com/v1", 7));
        assert_eq!((f.context.as_str(), f.response.as_str()), ("1000", "8192"));
        assert!(!f.temperature, "the catalog's supportsTemperature");
        assert_eq!(f.headers, stored.custom_headers);
        stored.supports_temperature = Some(true);
        stored.configurable_thinking = Some(true);
        let f = EditorForm::from_model(&stored, cat);
        assert!(f.temperature && f.thinking, "a stored value wins");
        let f = EditorForm::from_model(&model("unknown", "u", 99), cat);
        assert!(f.temperature && !f.thinking, "unknown names support temperature, not thinking");
        assert_eq!(f.api, Api::ChatCompletions.raw(), "an API the popup does not offer reads Chat Completions");
    }

    #[test]
    fn editor_save_follows_iterm2() {
        let cat = bundled();
        let base = default_base(&empty());
        assert_eq!((base.name.as_str(), base.url.as_str(), base.api), ("gpt-4o-mini", "", 2));
        assert_eq!((base.context_window_tokens, base.max_response_tokens), (128000, 8192));
        assert!(!base.function_calling && !base.streaming && !base.hosted_web_search && !base.hosted_file_search && !base.hosted_code_interpreter);
        assert_eq!(base.vector_store, 0);
        assert!(!base.id.is_empty() && base.id != default_base(&empty()).id, "a fresh id");
        let mut g = empty();
        g.set(ai::MODEL, Value::String(OLLAMA_PENDING.into()));
        g.set(ai::TOKEN_LIMIT, Value::Integer(0));
        g.set(ai::FEATURE_STREAMING, Value::Boolean(true));
        let seeded = default_base(&g);
        assert_eq!((seeded.name.as_str(), seeded.context_window_tokens), ("gpt-4o-mini", 8192));
        assert!(seeded.streaming);

        let free = |_: &str| false;
        let good = || {
            let mut f = EditorForm::from_model(&base, cat);
            f.url = " https://api.example.com/v1/chat/completions \n".into();
            f.name = "  my-model ".into();
            f
        };
        let fails = |f: &EditorForm, taken: &dyn Fn(&str) -> bool| f.validate(&base, taken).err().unwrap_or_default();
        let mut f = good();
        f.url = "  ".into();
        assert_eq!(fails(&f, &free), "URL is required.");
        let mut f = good();
        f.name = String::new();
        assert_eq!(fails(&f, &free), "Model is required.");
        for bad in ["0", "x", "-5"] {
            let mut f = good();
            f.context = bad.into();
            assert_eq!(fails(&f, &free), "Context tokens must be greater than zero.", "{bad}");
        }
        let mut f = good();
        f.response = "0".into();
        assert_eq!(fails(&f, &free), "Max response tokens must be greater than zero.");
        assert_eq!(fails(&good(), &|n: &str| n == "my-model"), "Manual model names must be unique.", "the trimmed name is checked");
        let mut f = good();
        f.headers = vec![("Bad Name".into(), "v".into())];
        assert_eq!(
            fails(&f, &free),
            "Custom header name \u{201c}Bad Name\u{201d} is not valid. Use only RFC 7230 token characters (letters, digits, and any of !#$%&'*+-.^_`|~)."
        );
        let mut f = good();
        f.headers = vec![("X-A".into(), "a\nb".into())];
        assert_eq!(fails(&f, &free), "The value for custom header \u{201c}X-A\u{201d} must not contain newline or null characters.");

        let mut f = good();
        f.headers = vec![(" ".into(), "dropped\n".into()), (" X-A ".into(), " 1".into()), (String::new(), String::new())];
        f.context = "12abc".into();
        f.thinking = true;
        f.temperature = false;
        f.dynamic = true;
        f.vision = true;
        f.vector_store = 1;
        let m = f.validate(&base, &free).expect("valid");
        assert_eq!(m.id, base.id);
        assert_eq!((m.name.as_str(), m.url.as_str(), m.api), ("my-model", "https://api.example.com/v1/chat/completions", 2));
        assert_eq!((m.context_window_tokens, m.max_response_tokens, m.vector_store), (12, 8192, 1));
        assert_eq!((m.supports_temperature, m.configurable_thinking, m.dynamic_models, m.dynamic_selected_model.as_deref()), (Some(false), Some(true), false, None));
        assert!(m.vision && !m.streaming);
        assert_eq!(m.custom_headers, [("X-A".to_string(), " 1".to_string())], "blank names are dropped, names trimmed");
        assert!(m.extra.is_empty());

        let mut no_id = base.clone();
        no_id.id = String::new();
        let minted = good().validate(&no_id, &free).expect("valid").id;
        assert!(!minted.is_empty(), "a model read without an id gets one");

        let b = model("b", EXAMPLE, 2);
        let copy = duplicate_base(&b);
        assert_eq!(copy.name, "b copy");
        assert!(!copy.id.is_empty() && copy.id != b.id);
        assert_eq!((copy.url.as_str(), copy.api), (EXAMPLE, 2));
        assert_eq!(duplicate_base(&model("", "u", 1)).name, " copy", "an empty name is a string (G:3833)");
    }

    #[test]
    fn legacy_notice_follows_iterm2() {
        let due = || {
            let mut g = empty();
            g.set(ai::CUSTOM_HEADERS_ENABLED, Value::Boolean(true));
            g.set(ai::CUSTOM_HEADERS, value(r#"[{name="X-Key", value="1"}]"#));
            g
        };
        assert!(legacy_notice_due(&due()));
        let mut g = due();
        g.set(LEGACY_NOTICE_FLAG, Value::Boolean(true));
        assert!(!legacy_notice_due(&g));
        let mut g = due();
        g.set(ai::MANUAL_MODELS, value(r#"[{name="a", url="u"}]"#));
        assert!(!legacy_notice_due(&g));
        let mut g = due();
        g.set(ai::CUSTOM_HEADERS_ENABLED, Value::Boolean(false));
        assert!(!legacy_notice_due(&g));
        let mut g = due();
        g.set(ai::CUSTOM_HEADERS, Value::Array(Vec::new()));
        assert!(!legacy_notice_due(&g));
        assert_eq!(LEGACY_NOTICE_FLAG, "NoSyncAILegacyGlobalHeadersWarningShown");
    }

    #[test]
    fn sheet_and_editor_texts_follow_iterm2() {
        assert_eq!([SHEET_TITLE, COLUMN_MODEL, COLUMN_API, COLUMN_ENDPOINT], ["Manual AI Models", "Model", "API", "Endpoint"]);
        assert_eq!([ADD, DELETE, EDIT, DUPLICATE, TOGGLE_DEFAULT, ECONOMY], ["Add", "Delete", "Edit", "Duplicate", "Toggle Default", "Economy"]);
        assert_eq!(TOGGLE_ECONOMY, "Toggle Economy Model. A cheaper model used for frequent background jobs like command-safety checks and screen-idle detection.");
        assert_eq!([ECONOMY_MARK, UNTITLED, MANUAL_MODEL], ["Economy model", "Untitled model", "Manual model"]);
        assert_eq!(LEGACY_HEADING, "Custom Headers Are Now Set Per Model");
        assert_eq!(
            LEGACY_TEXT,
            "Your AI custom HTTP headers used to be a single global setting. They still apply to your current model, but adding or editing models here does not carry them over. Re-add the headers you need in each model\u{2019}s \u{201c}Custom headers\u{201d} section."
        );
        assert_eq!([ADD_TITLE, EDIT_TITLE, PRESET_LABEL, CUSTOM, EDITOR_MODEL_LABEL], ["Add Manual AI Model", "Edit Manual AI Model", "Preset:", "Custom", "Model:"]);
        assert_eq!([FETCH_MODELS, REFRESH_MODELS, URL_LABEL, API_LABEL], ["Fetch Models", "Refresh Models", "URL:", "API:"]);
        assert_eq!(DISCOVER, "Discover installed models automatically (Ollama)");
        assert_eq!([CONTEXT_LABEL, RESPONSE_LABEL, VECTOR_STORE_LABEL, VECTOR_DISABLED, VECTOR_OPENAI], ["Context tokens:", "Max response tokens:", "Vector store:", "Disabled", "OpenAI"]);
        assert_eq!(FEATURES.map(|(title, _)| title), ["Function calling", "Streaming responses", "Vision (image input)", "Hosted web search", "Hosted file search", "Hosted code interpreter"]);
        assert_eq!(THINKING, "Configurable thinking");
        assert_eq!(THINKING_TIP, "Enable for reasoning models with a thinking mode, such as GPT-5, o-series, or DeepSeek models, so the chat\u{2019}s Think toggle appears.");
        assert_eq!(TEMPERATURE, "Supports temperature");
        assert_eq!(TEMPERATURE_TIP, "Uncheck for models that reject a temperature parameter, such as Anthropic Opus 4.7 and later.");
        assert_eq!([HEADERS_LABEL, HEADER_COLUMN, VALUE_COLUMN], ["Custom headers:", "Header", "Value"]);
        assert_eq!([TEST_CONNECTION, CANCEL, SAVE, INVALID_TITLE], ["Test Connection", "Cancel", "Save", "Invalid Manual AI Model"]);
    }
}
