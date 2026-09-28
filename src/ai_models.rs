//! Manual AI models, the default model's endpoint, the default-model popup's writes and the
//! one-time migration of the legacy global custom headers. Pure; ports
//! `Settings/GeneralPreferencesViewController.m` (the manual model dictionary, `G:43-64`,
//! `G:3634-3781`, `G:2917-2995`, `G:3188-3236`), `AITerm/LLMMetadata.swift` (`LM:78-218`,
//! `LM:491-718`) and `AppLaunch/iTermMigrationHelper.m:458-509`.

use crate::ai_catalog::{Catalog, Feature};
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
    let custom_headers = if g.bool(ai::CUSTOM_HEADERS_ENABLED) { headers_of(g.get(ai::CUSTOM_HEADERS).as_ref()) } else { Vec::new() };
    vec![Endpoint { vendor: manual_vendor(api, &url, &name), model: name, url, api, custom_headers }]
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
}
