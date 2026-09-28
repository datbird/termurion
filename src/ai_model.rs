//! Which model a request goes to: the model and vendor resolved from the settings, 12a's
//! catalog and the discovered Ollama tags, with the provider display names, the attachment
//! rules and the token arithmetic. Pure; ports `AITerm/LLMMetadata.swift` (`MD:78-259`,
//! `MD:399-407`, `MD:491-609`) and `AITerm/LLMProvider.swift` (`P:72-411`). The host tests,
//! the vendor of a manual model and the key's vendor are 12a's (`ai_policy`, `ai_models`).

use crate::ai_catalog::{Catalog, Effort, Feature, Model, VectorStore};
use crate::ai_models::{self, ManualModel, OLLAMA_ENDPOINT, OLLAMA_PENDING};
use crate::ai_policy;
use crate::ai_settings::{self as ai, Api, Vendor};
use crate::globals::Globals;
use std::collections::{BTreeSet, HashMap};

/// The context window of an Ollama model that reports none (`OllamaModelDiscovery.defaultContextWindow`).
pub const DEFAULT_CONTEXT: i64 = 32768;
/// The display name of a provider no host or API names (`P:101-103`).
pub const UNKNOWN_PLATFORM: &str = "Unknown Platform";

/// A model with iTerm2's `AIMetadata.Model` defaults for everything not given.
fn model(name: &str, url: &str, api: Api, vendor: Vendor, context: i64, response: i64, features: BTreeSet<Feature>) -> Model {
    Model {
        name: name.to_string(),
        vendor,
        api,
        url: url.to_string(),
        context_window_tokens: context,
        max_response_tokens: response,
        features,
        vector_store: VectorStore::Disabled,
        supports_temperature: true,
        recommended: false,
        fixture_exempt: false,
        economy_model: None,
        thinking_off_effort: Effort::Low,
        thinking_on_effort: Effort::Medium,
        reasoning_efforts: Vec::new(),
        service_tiers: Vec::new(),
        wire_model_name: None,
        custom_headers: Vec::new(),
    }
}

pub fn has(m: &Model, f: Feature) -> bool {
    m.features.contains(&f)
}

/// A manual model as a request sees it (`MD:538-613`), for an entry `ai_models::usable` accepts.
/// A manual model named like a catalog entry is usually that entry behind a proxy, so the
/// catalog twin supplies the temperature and thinking defaults and always its efforts and tiers.
pub fn from_manual(m: &ManualModel, catalog: &Catalog) -> Model {
    let api = m.api();
    let twin = catalog.named(&m.name);
    let mut features = BTreeSet::new();
    for (on, f) in [
        (m.function_calling, Feature::FunctionCalling),
        (m.hosted_web_search, Feature::HostedWebSearch),
        (m.hosted_file_search, Feature::HostedFileSearch),
        (m.streaming, Feature::Streaming),
        (m.hosted_code_interpreter, Feature::HostedCodeInterpreter),
        (m.vision, Feature::Vision),
    ] {
        if on {
            features.insert(f);
        }
    }
    if m.configurable_thinking.unwrap_or_else(|| twin.is_some_and(|t| has(t, Feature::ConfigurableThinking))) {
        features.insert(Feature::ConfigurableThinking);
    }
    let vendor = ai_models::manual_vendor(api, &m.url, &m.name);
    let mut out = model(&m.name, &m.url, api, vendor, m.context_window_tokens, m.max_response_tokens, features);
    out.vector_store = VectorStore::from_raw(m.vector_store).unwrap_or(VectorStore::Disabled);
    out.supports_temperature = m.supports_temperature.unwrap_or_else(|| twin.is_none_or(|t| t.supports_temperature));
    if let Some(t) = twin {
        out.reasoning_efforts = t.reasoning_efforts.clone();
        out.service_tiers = t.service_tiers.clone();
        out.thinking_off_effort = t.thinking_off_effort;
        out.thinking_on_effort = t.thinking_on_effort;
    }
    out.custom_headers = m.custom_headers.clone();
    out
}

/// The single model the settings described before manual model lists (`MD:491-536`); None
/// without an `AitermURL`. It inherits the legacy global headers.
pub fn legacy_model(g: &Globals) -> Option<Model> {
    let url = g.string(ai::URL);
    if url.is_empty() {
        return None;
    }
    let mut features = BTreeSet::new();
    for (key, f) in [
        (ai::FEATURE_FUNCTION_CALLING, Feature::FunctionCalling),
        (ai::FEATURE_WEB_SEARCH, Feature::HostedWebSearch),
        (ai::FEATURE_FILE_SEARCH, Feature::HostedFileSearch),
        (ai::FEATURE_STREAMING, Feature::Streaming),
        (ai::FEATURE_CODE_INTERPRETER, Feature::HostedCodeInterpreter),
    ] {
        if g.bool(key) {
            features.insert(f);
        }
    }
    let name = g.string(ai::MODEL);
    let api = Api::from_raw(g.int(ai::API)).unwrap_or(Api::ChatCompletions);
    let vendor = ai_models::manual_vendor(api, &url, &name);
    let mut m = model(&name, &url, api, vendor, g.int(ai::TOKEN_LIMIT), g.int(ai::RESPONSE_TOKEN_LIMIT), features);
    m.vector_store = VectorStore::from_raw(g.int(ai::VECTOR_STORE)).unwrap_or(VectorStore::Disabled);
    m.custom_headers = ai_models::legacy_headers(g);
    Some(m)
}

/// The Ollama tags discovery found, per server (Task 9 fills it).
#[derive(Clone, Debug, Default)]
pub struct Discovered {
    pub servers: Vec<Server>,
}

/// One discovery bucket: an endpoint reached with a set of headers, and its models.
#[derive(Clone, Debug)]
pub struct Server {
    pub endpoint: String,
    pub headers: Vec<(String, String)>,
    pub models: Vec<Model>,
}

impl Discovered {
    /// The built-in Ollama vendor's models: those at the default endpoint, reached without
    /// headers (`MD:344-346`).
    pub fn default_models(&self) -> &[Model] {
        self.servers.iter().find(|s| s.endpoint == OLLAMA_ENDPOINT && s.headers.is_empty()).map_or(&[], |s| &s.models)
    }
}

/// The manual models a request can use (`MD:163-174`, `MD:261-289`): the configured entries in
/// stored order, a dynamic entry expanded to the models discovered at its server, colliding
/// dynamic names qualified; else (unless the recommended model is in use) the legacy model.
pub fn manual_models(g: &Globals, catalog: &Catalog, found: &Discovered) -> Vec<Model> {
    let configured: Vec<Model> = ai_models::read(g)
        .iter()
        .flat_map(|m| {
            if m.dynamic_models {
                expand_dynamic(m, found)
            } else {
                ai_models::usable(std::slice::from_ref(m)).into_iter().map(|m| from_manual(m, catalog)).collect()
            }
        })
        .collect();
    let built_in: Vec<String> = found.default_models().iter().map(|m| m.name.clone()).collect();
    let configured = disambiguate(configured, &built_in);
    if !configured.is_empty() || g.bool(ai::USE_RECOMMENDED) {
        return configured;
    }
    legacy_model(g).into_iter().collect()
}

/// A dynamic Ollama entry's models (`MD:442-489`): those discovered at its URL with its headers,
/// each carrying the headers and its tag as the wire name; only the chosen tag when the entry
/// names one, and while that tag is not discovered a stand-in with streaming, vision and
/// function calling so an attachment or a tool is never silently dropped.
pub fn expand_dynamic(m: &ManualModel, found: &Discovered) -> Vec<Model> {
    if m.url.trim().is_empty() {
        return Vec::new();
    }
    let discovered: Vec<Model> = crate::ai_discovery::models_at(found, &m.url, &m.custom_headers)
        .iter()
        .map(|d| {
            let mut d = d.clone();
            d.custom_headers = m.custom_headers.clone();
            d.wire_model_name = Some(d.name.clone());
            d
        })
        .collect();
    let selected = m.dynamic_selected_model.as_deref().unwrap_or("").trim();
    if selected.is_empty() {
        return discovered;
    }
    if let Some(d) = discovered.into_iter().find(|d| d.name == selected) {
        return vec![d];
    }
    let mut stand_in = model(selected, &m.url, Api::Llama, Vendor::Llama, DEFAULT_CONTEXT, DEFAULT_CONTEXT, BTreeSet::from([Feature::Streaming, Feature::Vision, Feature::FunctionCalling]));
    stand_in.custom_headers = m.custom_headers.clone();
    stand_in.wire_model_name = Some(selected.to_string());
    vec![stand_in]
}

/// Unique names for dynamic models (`MD:307-349`), which carry a wire name: one whose name
/// clashes with another model or with a built-in default-endpoint tag (`built_in`) becomes
/// "tag (scheme://host:port)"; one still clashing, "tag (url)", then "tag (url #2)" for a
/// repeated URL. The wire name stays the tag; other models are never renamed.
pub fn disambiguate(models: Vec<Model>, built_in: &[String]) -> Vec<Model> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for name in built_in.iter().collect::<BTreeSet<_>>() {
        *counts.entry(name.clone()).or_default() += 1;
    }
    for m in &models {
        *counts.entry(m.name.clone()).or_default() += 1;
    }
    let qualified: Vec<Model> = models
        .into_iter()
        .map(|mut m| {
            if m.wire_model_name.is_some() && counts[&m.name] > 1 {
                m.name = format!("{} ({})", effective_name(&m), crate::ai_discovery::server_label(&m.url));
            }
            m
        })
        .collect();
    let mut again: HashMap<String, usize> = HashMap::new();
    for m in &qualified {
        *again.entry(m.name.clone()).or_default() += 1;
    }
    let mut occurrences: HashMap<String, usize> = HashMap::new();
    qualified
        .into_iter()
        .map(|mut m| {
            if m.wire_model_name.is_some() && again[&m.name] > 1 {
                let n = occurrences.entry(m.url.clone()).or_default();
                *n += 1;
                m.name = if *n == 1 { format!("{} ({})", effective_name(&m), m.url) } else { format!("{} ({} #{})", effective_name(&m), m.url, n) };
            }
            m
        })
        .collect()
}

/// The stand-in while Ollama discovery is pending (`MD:399-407`), so the built-in Ollama
/// vendor never crosses to a cloud vendor.
pub fn pending_ollama_model() -> Model {
    model(OLLAMA_PENDING, OLLAMA_ENDPOINT, Api::Llama, Vendor::Llama, DEFAULT_CONTEXT, DEFAULT_CONTEXT, BTreeSet::from([Feature::Streaming]))
}

/// A vendor's recommended model (`MD:126-161`). Ollama: the Regular pick when discovered (the
/// placeholder while it is missing, never another tag), else the lexicographically first
/// discovered tag, else the placeholder. Apple answers its catalog entry, which no picker offers.
pub fn recommended(vendor: Vendor, g: &Globals, catalog: &Catalog, found: &Discovered) -> Option<Model> {
    if vendor != Vendor::Llama {
        return catalog.recommended(vendor).cloned();
    }
    let discovered = found.default_models();
    let chosen = g.string(ai::OLLAMA_REGULAR);
    let pick = if chosen.is_empty() { discovered.iter().min_by(|a, b| a.name.cmp(&b.name)) } else { discovered.iter().find(|m| m.name == chosen) };
    Some(pick.cloned().unwrap_or_else(pending_ollama_model))
}

/// The default model for new chats (`MD:203-216`): the vendor's recommended model, else the
/// manual model named by `AiModel`, else the first manual model.
pub fn configured(g: &Globals, catalog: &Catalog, found: &Discovered) -> Option<Model> {
    if g.bool(ai::USE_RECOMMENDED) {
        if let Some(v) = Vendor::from_raw(g.int(ai::VENDOR)) {
            return recommended(v, g, catalog, found);
        }
    }
    let mut models = manual_models(g, catalog, found);
    let name = g.string(ai::MODEL);
    let pick = models.iter().position(|m| m.name == name).unwrap_or(0);
    (pick < models.len()).then(|| models.swap_remove(pick))
}

/// Where a model's requests go: its vendor, name, URL, API and custom headers. The consent gate
/// checks this endpoint of the model a request actually sends, so the host it allowed is the host
/// the transport connects to (12a's `default_endpoint` is this of `configured`).
pub fn endpoint(m: &Model) -> ai_models::Endpoint {
    ai_models::Endpoint { vendor: m.vendor, model: m.name.clone(), url: m.url.clone(), api: m.api, custom_headers: m.custom_headers.clone() }
}

/// A pinned or persisted model name (`MD:194-201`): a manual model wins over the catalog entry of
/// the same name, then the catalog, then the tags discovered at the default Ollama endpoint.
pub fn named(name: &str, g: &Globals, catalog: &Catalog, found: &Discovered) -> Option<Model> {
    if name.is_empty() {
        return None;
    }
    manual_models(g, catalog, found)
        .into_iter()
        .find(|m| m.name == name)
        .or_else(|| catalog.named(name).cloned())
        .or_else(|| found.default_models().iter().find(|m| m.name == name).cloned())
}

/// The built-in Ollama vendor is the default for new chats (`MD:376-379`).
pub fn ollama_vendor_is_default(g: &Globals) -> bool {
    g.bool(ai::USE_RECOMMENDED) && Vendor::from_raw(g.int(ai::VENDOR)) == Some(Vendor::Llama)
}

/// The cheaper model for frequent low-stakes work (`MD:228-259`); None leaves the caller on the
/// configured model. With the Ollama vendor the Budget pick alone decides (a designated manual
/// economy model survives the switch to Ollama and must not shadow it); else the designated
/// manual model; else the configured model's catalog economy variant on its own url and api.
pub fn economy(g: &Globals, catalog: &Catalog, found: &Discovered) -> Option<Model> {
    if ollama_vendor_is_default(g) {
        let chosen = g.string(ai::OLLAMA_ECONOMY);
        if chosen.is_empty() {
            return None;
        }
        return found.default_models().iter().find(|m| m.name == chosen).cloned();
    }
    let designated = g.string(ai::ECONOMY_MODEL);
    if !designated.is_empty() {
        if let Some(m) = manual_models(g, catalog, found).into_iter().find(|m| m.name == designated) {
            return Some(m);
        }
    }
    catalog.economy_for(&configured(g, catalog, found)?)
}

/// The provider's name in "Error from %@" (`P:72-104`). iTerm2 tests the host of the request
/// URL; the Gemini `{{MODEL}}` substitution changes only the path, so the model URL's host is
/// the same.
pub fn display_name(m: &Model) -> &'static str {
    let url = m.url.as_str();
    if ai_policy::host_is_openai(url) {
        "OpenAI"
    } else if ai_policy::host_is_google(url) {
        "Google"
    } else if ai_policy::host_is_azure(url) {
        "Azure"
    } else if ai_policy::host_is_deepseek(url) {
        "Deep Seek"
    } else if ai_policy::host_is_anthropic(url) {
        "Anthropic"
    } else if m.api == Api::Llama {
        // The native /api/chat dialect is named for the runner, whatever model it serves.
        "Ollama"
    } else if m.name.contains("llama") {
        "Llama"
    } else {
        UNKNOWN_PLATFORM
    }
}

/// Whether the request body names the model (`P:106-116`): not for Azure, whose deployment is in
/// the URL, and not for Gemini, whose model is in the path.
pub fn dynamic_models_supported(m: &Model) -> bool {
    !ai_policy::host_is_azure(&m.url) && m.api != Api::Gemini
}

/// `P:129-138`: Responses only, and never under zero data retention (`zdr` is
/// `AIZeroDataRetention`), whose organizations cannot keep responses on the server.
pub fn supports_previous_response_id(m: &Model, zdr: bool) -> bool {
    !zdr && m.api == Api::Responses
}

/// The model id on the wire (`AM:218`).
pub fn effective_name(m: &Model) -> &str {
    m.wire_model_name.as_deref().unwrap_or(&m.name)
}

/// iTerm2's token estimate (`AM:437-439`): UTF-8 bytes / 2.
pub fn tokens(s: &str) -> i64 {
    (s.len() / 2) as i64
}

/// The response token budget (`P:303-317`): the model's maximum response, capped by what the
/// token limit leaves after the query (every message's content joined with "\n") and the
/// functions' JSON ("" when there are none or the model has no function calling). It can be
/// zero or below.
pub fn max_tokens(m: &Model, token_limit: i64, query: &str, functions_json: &str) -> i64 {
    m.max_response_tokens.min(token_limit - tokens(query) - tokens(functions_json))
}

/// `P:383-411`.
pub fn mime_is_textual(mime: &str) -> bool {
    mime.starts_with("text/")
        || ["application/json", "application/javascript", "application/ecmascript", "application/xml", "message/rfc822", "application/x-sql"].contains(&mime)
        || mime.ends_with("+xml")
        || mime.starts_with("application/x-tex")
}

/// OpenAI's Responses API with a vector store: binaries go to the store, PDFs and images inline.
fn openai_responses_with_store(m: &Model) -> bool {
    ai_policy::host_is_openai(&m.url) && m.api == Api::Responses && m.vector_store != VectorStore::Disabled
}

/// Whether the request format can carry an attachment of this type (`P:179-279`): text always;
/// PDFs, images, audio and video where the vendor's serializer has a block for them (images on
/// Ollama only with vision); other binaries only by upload.
pub fn accepts(m: &Model, mime: &str) -> bool {
    let url = m.url.as_str();
    let (openai, anthropic, google) = (ai_policy::host_is_openai(url), ai_policy::host_is_anthropic(url), ai_policy::host_is_google(url));
    if mime_is_textual(mime) {
        true
    } else if mime == "application/pdf" {
        openai || anthropic || google
    } else if mime.starts_with("image/") {
        if m.api == Api::Llama {
            has(m, Feature::Vision)
        } else {
            anthropic || google || openai
        }
    } else if mime.starts_with("audio/") {
        // OpenAI's input_audio block takes wav and mp3 only, and only on Chat Completions.
        google || (openai && m.api == Api::ChatCompletions && ["audio/wav", "audio/x-wav", "audio/mpeg", "audio/mp3"].contains(&mime))
    } else if mime.starts_with("video/") {
        google
    } else {
        should_upload(m, mime)
    }
}

/// Whether an attachment goes to the vector store (`P:144-163`): a binary that is not an image,
/// audio or video, on OpenAI's Responses API with a vector store.
pub fn should_upload(m: &Model, mime: &str) -> bool {
    if mime_is_textual(mime) || mime.starts_with("image/") || mime.starts_with("audio/") || mime.starts_with("video/") {
        return false;
    }
    openai_responses_with_store(m)
}

/// Whether a binary attachment is sent inline as base64 rather than uploaded (`P:281-297`): PDFs
/// and images on OpenAI's Responses API with a vector store.
pub fn inline_base64(m: &Model, mime: &str) -> bool {
    if should_upload(m, mime) {
        return false;
    }
    openai_responses_with_store(m) && (mime == "application/pdf" || mime.starts_with("image/"))
}

/// A test model: 128000 context and 8192 response tokens, streaming and function calling, the
/// vendor 12a's manual classification gives.
#[cfg(test)]
pub fn fixture(name: &str, url: &str, api: Api) -> Model {
    model(name, url, api, ai_models::manual_vendor(api, url, name), 128000, 8192, BTreeSet::from([Feature::Streaming, Feature::FunctionCalling]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_catalog::{bundled, Effort, Feature, VectorStore};
    use crate::ai_models::{self, OLLAMA_ENDPOINT, OLLAMA_PENDING};
    use crate::ai_settings::{self as ai, Api, Vendor};
    use crate::globals::Globals;
    use std::collections::BTreeSet;
    use toml::{Table, Value};

    fn value(toml_literal: &str) -> Value {
        let t: Table = format!("v = {toml_literal}").parse().expect("test TOML parses");
        t["v"].clone()
    }

    fn empty() -> Globals {
        Globals::empty(&std::env::temp_dir().join("tandem-ai-model-test.toml"))
    }

    fn globals_with(configs: &str) -> Globals {
        let mut g = empty();
        g.set(ai::MANUAL_MODELS, value(configs));
        g
    }

    fn manual_mode(configs: &str) -> Globals {
        let mut g = globals_with(configs);
        g.set(ai::USE_RECOMMENDED, Value::Boolean(false));
        g
    }

    fn ollama_default() -> Globals {
        let mut g = empty();
        g.set(ai::VENDOR, Value::Integer(Vendor::Llama.raw()));
        g
    }

    /// Tags as discovery reports them at one endpoint.
    fn found_at(endpoint: &str, headers: Vec<(String, String)>, names: &[&str]) -> Discovered {
        let models = names
            .iter()
            .map(|n| {
                let mut m = pending_ollama_model();
                m.name = n.to_string();
                m.url = endpoint.to_string();
                m
            })
            .collect();
        Discovered { servers: vec![Server { endpoint: endpoint.to_string(), headers, models }] }
    }

    fn found(names: &[&str]) -> Discovered {
        found_at(OLLAMA_ENDPOINT, Vec::new(), names)
    }

    fn name_of(m: Option<Model>) -> Option<String> {
        m.map(|m| m.name)
    }

    #[test]
    fn agrees_with_12a() {
        let c = bundled();
        let none = Discovered::default();
        let mut cases = vec![empty()];
        for v in [0, 1, 2, 4, 3, 5, 9] {
            let mut g = empty();
            g.set(ai::VENDOR, Value::Integer(v));
            cases.push(g);
        }
        // An Ollama Regular pick that discovery has not found.
        let mut pick = empty();
        pick.set(ai::VENDOR, Value::Integer(3));
        pick.set(ai::OLLAMA_REGULAR, Value::String("qwen3".into()));
        cases.push(pick);
        cases.push(manual_mode(r#"[{name="a", url="https://a.test/v1", api=2}, {name="b", url="https://h.test/v1/chat/completions", customHeaders=[{name="X-A", value="1"}]}]"#));
        cases.last_mut().unwrap().set(ai::MODEL, Value::String("b".into()));
        // `AiModel` names a dynamic entry: it is not a model, so the first usable entry is picked.
        cases.push(manual_mode(r#"[{name="d", url="http://d.test:11434", dynamicModels=true}, {name="a", url="https://a.test/v1", api=2}]"#));
        cases.last_mut().unwrap().set(ai::MODEL, Value::String("d".into()));
        let mut legacy = empty();
        legacy.set(ai::USE_RECOMMENDED, Value::Boolean(false));
        legacy.set(ai::URL, Value::String("https://my.host/v1/chat/completions".into()));
        legacy.set(ai::MODEL, Value::String("m".into()));
        cases.push(legacy);
        for (i, g) in cases.iter().enumerate() {
            let m = configured(g, c, &none);
            let e = ai_models::default_endpoint(g, c, &none);
            assert_eq!(e, m.as_ref().filter(|m| !m.url.is_empty()).map(endpoint), "case {i}: one pick for 12a and the request");
            if let Some(e) = &e {
                assert_eq!(e.vendor, ai_models::effective_vendor(g, c, &none), "case {i}");
                assert_eq!(ai_policy::host_of(&e.url), ai_policy::host_of(&m.as_ref().unwrap().url), "case {i}");
            }
        }
        // The manual case really carries its header, the dynamic case picks "a", and the
        // undiscovered pick waits on the placeholder at the default endpoint.
        let n = cases.len();
        assert_eq!(configured(&cases[n - 3], c, &none).unwrap().custom_headers, [("X-A".to_string(), "1".to_string())]);
        assert_eq!(name_of(configured(&cases[n - 2], c, &none)).as_deref(), Some("a"));
        let e = ai_models::default_endpoint(&cases[n - 4], c, &none).unwrap();
        assert_eq!((e.model.as_str(), e.url.as_str(), e.vendor), (OLLAMA_PENDING, OLLAMA_ENDPOINT, Vendor::Llama));
        // Apple Intelligence: the catalog entry, which has nowhere to send (the gate's NoEndpoint,
        // the turn's APPLE_UNAVAILABLE).
        let apple = configured(&cases[6], c, &none).unwrap();
        assert_eq!((apple.name.as_str(), apple.api, apple.url.as_str()), ("apple-on-device", Api::AppleIntelligence, ""));
        assert_eq!(ai_models::default_endpoint(&cases[6], c, &none), None);
        assert_eq!(configured(&cases[7], c, &none), None, "an out-of-range vendor finds no manual model");
    }

    #[test]
    fn endpoint_is_where_the_model_goes() {
        let c = bundled();
        let mut models: Vec<Model> = c.models.clone();
        models.push(pending_ollama_model());
        models.extend(manual_models(&globals_with(r#"[{name="p", url="http://user:pw@[::1]:8080/v1", customHeaders=[{name="X-A", value="1"}]}]"#), c, &Discovered::default()));
        for m in &models {
            let e = endpoint(m);
            assert_eq!((e.vendor, &e.model, &e.url, e.api, &e.custom_headers), (m.vendor, &m.name, &m.url, m.api, &m.custom_headers), "{}", m.name);
            // Gemini's request URL substitutes the model into the path only, so the host the
            // gate checks is the host the request goes to.
            let sent = m.url.replace("{{MODEL}}", effective_name(m));
            assert_eq!(ai_policy::host_of(&e.url), ai_policy::host_of(&sent), "{}", m.name);
        }
    }

    #[test]
    fn manual_models_follow_iterm2() {
        let c = bundled();
        let none = Discovered::default();
        let g = globals_with(r#"[{name="m", url="http://h/v1"}]"#);
        let ms = manual_models(&g, c, &none);
        assert_eq!(ms.len(), 1);
        let m = &ms[0];
        assert_eq!((m.name.as_str(), m.url.as_str(), m.api), ("m", "http://h/v1", Api::ChatCompletions));
        assert_eq!((m.context_window_tokens, m.max_response_tokens), (8192, 8192));
        assert!(m.features.is_empty());
        assert_eq!(m.vector_store, VectorStore::Disabled);
        assert!(m.supports_temperature);
        assert_eq!(m.vendor, Vendor::OpenAI);
        assert_eq!((m.wire_model_name.as_deref(), m.custom_headers.len()), (None, 0));

        // The catalog twin supplies the defaults and always its efforts and tiers.
        let twin = c.named("claude-opus-4-8").unwrap();
        let opus = |extra: &str| manual_models(&globals_with(&format!(r#"[{{name="claude-opus-4-8", url="https://p.test/v1/messages", api=7{extra}}}]"#)), c, &none).remove(0);
        let plain = opus("");
        assert!(!plain.supports_temperature);
        assert!(opus(", supportsTemperature=true").supports_temperature);
        assert_eq!(plain.reasoning_efforts, twin.reasoning_efforts);
        assert_eq!(plain.service_tiers, twin.service_tiers);
        assert_eq!((plain.thinking_off_effort, plain.thinking_on_effort), (twin.thinking_off_effort, twin.thinking_on_effort));
        assert!(!has(&plain, Feature::ConfigurableThinking));
        assert!(has(&opus(", configurableThinking=true"), Feature::ConfigurableThinking));
        // A twin with thinking, efforts and tiers.
        let luna = |extra: &str| manual_models(&globals_with(&format!(r#"[{{name="gpt-5.6-luna", url="https://p.test/v1/responses", api=2{extra}}}]"#)), c, &none).remove(0);
        let l = luna("");
        assert!(has(&l, Feature::ConfigurableThinking));
        assert!(!has(&luna(", configurableThinking=false"), Feature::ConfigurableThinking));
        assert_eq!(l.reasoning_efforts, [Effort::None, Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh]);
        assert_eq!(l.thinking_off_effort, Effort::None);
        assert_eq!(l.service_tiers, c.named("gpt-5.6-luna").unwrap().service_tiers);
        // No twin: iTerm2's defaults.
        assert_eq!((m.thinking_off_effort, m.thinking_on_effort), (Effort::Low, Effort::Medium));
        assert!(m.reasoning_efforts.is_empty() && m.service_tiers.is_empty());

        // Blank names and URLs are not listed; order and headers are kept.
        let g = globals_with(
            r#"[{name="  ", url="http://h/v1"}, {name="x", url=""}, {name="b", url="http://b/v1", customHeaders=[{name="X-B", value="2"}, {name="X-A", value="1"}]}, {name="a", url="http://a/v1", streaming=true, vision=true, vectorStore=1}]"#,
        );
        let ms = manual_models(&g, c, &none);
        let names: Vec<&str> = ms.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["b", "a"]);
        assert_eq!(ms[0].custom_headers, [("X-B".to_string(), "2".to_string()), ("X-A".to_string(), "1".to_string())]);
        assert_eq!(ms[1].features, BTreeSet::from([Feature::Streaming, Feature::Vision]));
        assert_eq!(ms[1].vector_store, VectorStore::OpenAI);

        // from_manual is what manual_models applies.
        let raw = ai_models::read(&g);
        assert_eq!(from_manual(&raw[2], c), ms[0]);
    }

    #[test]
    fn legacy_model_follows_iterm2() {
        let c = bundled();
        let none = Discovered::default();
        let mut g = empty();
        g.set(ai::USE_RECOMMENDED, Value::Boolean(false));
        g.set(ai::URL, Value::String("https://h.test/v1/chat/completions".into()));
        let m = legacy_model(&g).expect("a legacy model");
        assert_eq!(m.name, "gpt-4o-mini");
        assert_eq!(m.url, "https://h.test/v1/chat/completions");
        assert_eq!(m.api, Api::Responses);
        assert_eq!((m.context_window_tokens, m.max_response_tokens), (128000, 8192));
        assert!(m.features.is_empty());
        assert!(m.custom_headers.is_empty());
        assert_eq!(m.vendor, Vendor::OpenAI);
        assert_eq!(manual_models(&g, c, &none), [m.clone()]);

        g.set(ai::FEATURE_STREAMING, Value::Boolean(true));
        g.set(ai::FEATURE_FUNCTION_CALLING, Value::Boolean(true));
        assert_eq!(legacy_model(&g).unwrap().features, BTreeSet::from([Feature::Streaming, Feature::FunctionCalling]));

        g.set(ai::CUSTOM_HEADERS, value(r#"[{name="X-A", value="1"}]"#));
        assert!(legacy_model(&g).unwrap().custom_headers.is_empty(), "headers need AICustomHeadersEnabled");
        g.set(ai::CUSTOM_HEADERS_ENABLED, Value::Boolean(true));
        assert_eq!(legacy_model(&g).unwrap().custom_headers, [("X-A".to_string(), "1".to_string())]);

        // A configured model wins over the legacy one.
        g.set(ai::MANUAL_MODELS, value(r#"[{name="c", url="http://c/v1"}]"#));
        assert_eq!(manual_models(&g, c, &none).iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["c"]);
        g.set(ai::MANUAL_MODELS, value("[]"));
        assert_eq!(manual_models(&g, c, &none).len(), 1);

        g.set(ai::URL, Value::String(String::new()));
        assert_eq!(legacy_model(&g), None);

        let mut g = empty();
        g.set(ai::URL, Value::String("https://h.test/v1/chat/completions".into()));
        assert!(manual_models(&g, c, &none).is_empty());
    }

    #[test]
    fn resolution_follows_iterm2() {
        let c = bundled();
        let none = Discovered::default();
        let name = |g: &Globals, f: &Discovered| name_of(configured(g, c, f));
        assert_eq!(name(&empty(), &none).as_deref(), Some("gpt-5.6-sol"));
        for (v, expected) in [(4, "claude-opus-4-8"), (0, "deepseek-v4-flash"), (1, "gemini-3.7-flash")] {
            let mut g = empty();
            g.set(ai::VENDOR, Value::Integer(v));
            assert_eq!(name(&g, &none).as_deref(), Some(expected), "vendor {v}");
        }

        let g = ollama_default();
        let pending = configured(&g, c, &none).unwrap();
        assert_eq!(pending, pending_ollama_model());
        assert_eq!((pending.name.as_str(), pending.url.as_str()), (OLLAMA_PENDING, OLLAMA_ENDPOINT));
        assert_eq!((pending.api, pending.vendor), (Api::Llama, Vendor::Llama));
        assert_eq!((pending.context_window_tokens, pending.max_response_tokens), (DEFAULT_CONTEXT, DEFAULT_CONTEXT));
        assert_eq!(pending.features, BTreeSet::from([Feature::Streaming]));

        let tags = found(&["qwen3:8b", "llama3.3"]);
        assert_eq!(name(&g, &tags).as_deref(), Some("llama3.3"));
        let mut chosen = ollama_default();
        chosen.set(ai::OLLAMA_REGULAR, Value::String("qwen3:8b".into()));
        assert_eq!(name(&chosen, &tags).as_deref(), Some("qwen3:8b"));
        chosen.set(ai::OLLAMA_REGULAR, Value::String("gone:1b".into()));
        assert_eq!(configured(&chosen, c, &tags), Some(pending_ollama_model()));
        // Tags of another server, or of the default endpoint reached with headers, are not the vendor's.
        assert_eq!(name(&g, &found_at("http://gpu:11434/api/chat", Vec::new(), &["a"])).as_deref(), Some(OLLAMA_PENDING));
        assert_eq!(name(&g, &found_at(OLLAMA_ENDPOINT, vec![("X".into(), "1".into())], &["a"])).as_deref(), Some(OLLAMA_PENDING));

        let mut g = manual_mode(r#"[{name="a", url="http://a/v1"}, {name="b", url="http://b/v1"}]"#);
        g.set(ai::MODEL, Value::String("b".into()));
        assert_eq!(name(&g, &none).as_deref(), Some("b"));
        g.set(ai::MODEL, Value::String("zzz".into()));
        assert_eq!(name(&g, &none).as_deref(), Some("a"));
        assert_eq!(name(&manual_mode("[]"), &none), None);

        // An out-of-range vendor in recommended mode falls through to the manual models.
        let mut g = globals_with(r#"[{name="a", url="http://a/v1"}]"#);
        g.set(ai::VENDOR, Value::Integer(9));
        assert_eq!(name(&g, &none).as_deref(), Some("a"));
        assert!(ollama_vendor_is_default(&ollama_default()));
        assert!(!ollama_vendor_is_default(&empty()));
        let mut g = ollama_default();
        g.set(ai::USE_RECOMMENDED, Value::Boolean(false));
        assert!(!ollama_vendor_is_default(&g));
    }

    #[test]
    fn dynamic_entries_follow_iterm2() {
        let c = bundled();
        let gpu = "http://gpu.test:11434/api/chat";
        let auth = vec![("Authorization".to_string(), "Bearer t".to_string())];
        let entry = |url: &str, extra: &str| format!(r#"{{name="Ollama (auto): x", url="{url}", dynamicModels=true, customHeaders=[{{name="Authorization", value="Bearer t"}}]{extra}}}"#);
        let one = |extra: &str| ai_models::read(&globals_with(&format!("[{}]", entry(gpu, extra)))).remove(0);
        let tags = found_at(gpu, auth.clone(), &["llama3.3", "phi4"]);
        let all = expand_dynamic(&one(""), &tags);
        assert_eq!(all.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["llama3.3", "phi4"]);
        for m in &all {
            assert_eq!(m.wire_model_name.as_deref(), Some(m.name.as_str()));
            assert_eq!((m.url.as_str(), m.api, m.vendor, &m.custom_headers), (gpu, Api::Llama, Vendor::Llama, &auth));
            // The gate checks the host the request goes to.
            assert_eq!(endpoint(m).url, gpu);
        }
        assert_eq!(name_of(expand_dynamic(&one(r#", dynamicSelectedModel="phi4""#), &tags).pop()).as_deref(), Some("phi4"));
        assert_eq!(expand_dynamic(&one(r#", dynamicSelectedModel="phi4""#), &tags).len(), 1);
        let gone = expand_dynamic(&one(r#", dynamicSelectedModel=" gone ""#), &tags);
        assert_eq!(gone.len(), 1);
        let g0 = &gone[0];
        assert_eq!((g0.name.as_str(), g0.url.as_str(), g0.wire_model_name.as_deref(), &g0.custom_headers), ("gone", gpu, Some("gone"), &auth));
        assert_eq!((g0.context_window_tokens, g0.max_response_tokens, g0.api, g0.vendor), (DEFAULT_CONTEXT, DEFAULT_CONTEXT, Api::Llama, Vendor::Llama));
        assert_eq!(g0.features, BTreeSet::from([Feature::Streaming, Feature::Vision, Feature::FunctionCalling]));
        // Nothing discovered, or discovered with other headers: a whole-server entry has no models.
        assert!(expand_dynamic(&one(""), &Discovered::default()).is_empty());
        assert!(expand_dynamic(&one(""), &found_at(gpu, Vec::new(), &["llama3.3"])).is_empty());
        let mut blank = one("");
        blank.url = " ".into();
        assert!(expand_dynamic(&blank, &tags).is_empty());

        // Collisions are qualified by server, then by URL, then by occurrence; the wire name stays.
        let names = |entries: &[String], found: Discovered| {
            let g = globals_with(&format!("[{}]", entries.join(", ")));
            manual_models(&g, c, &found).into_iter().map(|m| (m.name.clone(), effective_name(&m).to_string())).collect::<Vec<_>>()
        };
        let cpu = "http://cpu.test:11434/api/chat";
        let two = |a: &str, b: &str, headers_b: Vec<(String, String)>, extra_b: &str| {
            let mut f = found_at(a, auth.clone(), &["llama3.3", "phi4"]);
            f.servers.extend(found_at(b, headers_b, &["llama3.3"]).servers);
            names(&[entry(a, ""), entry(b, extra_b)], f)
        };
        let pair = |n: &str| (n.to_string(), "llama3.3".to_string());
        let phi = ("phi4".to_string(), "phi4".to_string());
        assert_eq!(two(gpu, cpu, auth.clone(), ""), [pair("llama3.3 (http://gpu.test:11434)"), phi.clone(), pair("llama3.3 (http://cpu.test:11434)")]);
        let v1 = "http://gpu.test:11434/v1/chat/completions";
        assert_eq!(two(gpu, v1, auth.clone(), ""), [pair(&format!("llama3.3 ({gpu})")), phi.clone(), pair(&format!("llama3.3 ({v1})"))]);
        // The same URL with other headers is another bucket.
        let other = vec![("Authorization".to_string(), "Bearer u".to_string())];
        let same_url = {
            let mut f = found_at(gpu, auth.clone(), &["llama3.3", "phi4"]);
            f.servers.extend(found_at(gpu, other.clone(), &["llama3.3"]).servers);
            let second = format!(r#"{{name="Ollama (auto): y", url="{gpu}", dynamicModels=true, customHeaders=[{{name="Authorization", value="Bearer u"}}]}}"#);
            names(&[entry(gpu, ""), second], f)
        };
        assert_eq!(same_url, [pair(&format!("llama3.3 ({gpu})")), phi.clone(), pair(&format!("llama3.3 ({gpu} #2)"))]);
        // A tag that clashes only with the built-in default endpoint's is qualified; the built-in
        // keeps its clean name and resolves by it.
        let mut f = found_at(gpu, auth.clone(), &["llama3.3", "phi4"]);
        f.servers.extend(found(&["llama3.3"]).servers);
        assert_eq!(names(&[entry(gpu, "")], f.clone()), [pair("llama3.3 (http://gpu.test:11434)"), phi.clone()]);
        let g = globals_with(&format!("[{}]", entry(gpu, "")));
        assert_eq!(named("llama3.3", &g, c, &f).map(|m| m.url), Some(OLLAMA_ENDPOINT.to_string()));
        assert_eq!(named("llama3.3 (http://gpu.test:11434)", &g, c, &f).map(|m| m.url), Some(gpu.to_string()));
        assert_eq!(disambiguate(Vec::new(), &["x".to_string()]), Vec::<Model>::new());
        // A non-dynamic model is never renamed, even when it clashes.
        let plain = fixture("llama3.3", "http://h/v1", Api::ChatCompletions);
        assert_eq!(disambiguate(vec![plain.clone()], &["llama3.3".to_string()]), [plain]);

        // `AiModel` naming an expanded model picks it, so the request and the gate see its host.
        let mut g = manual_mode(&format!("[{}]", entry(gpu, "")));
        g.set(ai::MODEL, Value::String("phi4".into()));
        let m = configured(&g, c, &tags).expect("a model");
        assert_eq!((m.name.as_str(), endpoint(&m).url.as_str()), ("phi4", gpu));
    }

    #[test]
    fn named_follows_iterm2() {
        let c = bundled();
        let g = globals_with(r#"[{name="gpt-5.6-sol", url="https://proxy.test/v1/responses", api=2}]"#);
        let tags = found(&["llama3.3"]);
        let sol = named("gpt-5.6-sol", &g, c, &tags).unwrap();
        assert_eq!(sol.url, "https://proxy.test/v1/responses");
        let haiku = named("claude-haiku-4-5", &g, c, &tags).unwrap();
        assert_eq!(&haiku, c.named("claude-haiku-4-5").unwrap());
        let local = named("llama3.3", &g, c, &tags).unwrap();
        assert_eq!((local.url.as_str(), local.api), (OLLAMA_ENDPOINT, Api::Llama));
        assert_eq!(named("", &g, c, &tags), None);
        assert_eq!(named("nope", &g, c, &tags), None);
    }

    #[test]
    fn economy_follows_iterm2() {
        let c = bundled();
        let tags = found(&["qwen3:8b", "llama3.3"]);
        let mut g = ollama_default();
        g.set(ai::MANUAL_MODELS, value(r#"[{name="a", url="http://a/v1"}, {name="b", url="http://b/v1"}]"#));
        g.set(ai::ECONOMY_MODEL, Value::String("b".into()));
        assert_eq!(economy(&g, c, &tags), None);
        g.set(ai::OLLAMA_ECONOMY, Value::String("llama3.3".into()));
        assert_eq!(name_of(economy(&g, c, &tags)).as_deref(), Some("llama3.3"));
        g.set(ai::OLLAMA_ECONOMY, Value::String("gone".into()));
        assert_eq!(economy(&g, c, &tags), None);

        let mut g = manual_mode(r#"[{name="a", url="http://a/v1"}, {name="b", url="http://b/v1"}]"#);
        g.set(ai::ECONOMY_MODEL, Value::String("b".into()));
        let b = economy(&g, c, &tags).unwrap();
        assert_eq!((b.name.as_str(), b.url.as_str()), ("b", "http://b/v1"));

        let g = manual_mode(r#"[{name="gpt-5.6-sol", url="https://proxy.test/v1/responses", api=2}]"#);
        let e = economy(&g, c, &tags).unwrap();
        assert_eq!((e.name.as_str(), e.url.as_str(), e.api), ("gpt-5.6-luna", "https://proxy.test/v1/responses", Api::Responses));
        // A designated name that is gone falls back to the configured model's economy variant.
        let mut g = g;
        g.set(ai::ECONOMY_MODEL, Value::String("gone".into()));
        assert_eq!(name_of(economy(&g, c, &tags)).as_deref(), Some("gpt-5.6-luna"));
    }

    #[test]
    fn display_names_follow_iterm2() {
        let cases = [
            ("m", "https://api.openai.com/v1/responses", Api::Responses, "OpenAI"),
            ("gemini-3.7-flash", "https://generativelanguage.googleapis.com/v1beta/models/{{MODEL}}", Api::Gemini, "Google"),
            ("m", "https://me.openai.azure.com/x", Api::ChatCompletions, "Azure"),
            ("m", "https://api.deepseek.com/chat/completions", Api::DeepSeek, "Deep Seek"),
            ("m", "https://API.Anthropic.com/v1/messages", Api::Anthropic, "Anthropic"),
            ("qwen3", OLLAMA_ENDPOINT, Api::Llama, "Ollama"),
            ("llama-3-8b", "http://localhost:1234/v1/chat/completions", Api::ChatCompletions, "Llama"),
            ("qwen3", "http://localhost:1234/v1/chat/completions", Api::ChatCompletions, UNKNOWN_PLATFORM),
        ];
        for (name, url, api, expected) in cases {
            assert_eq!(display_name(&fixture(name, url, api)), expected, "{url}");
        }
        assert_eq!(UNKNOWN_PLATFORM, "Unknown Platform");
    }

    #[test]
    fn capabilities_follow_iterm2() {
        assert!(!dynamic_models_supported(&fixture("m", "https://me.openai.azure.com/x", Api::ChatCompletions)));
        assert!(!dynamic_models_supported(&fixture("m", "https://generativelanguage.googleapis.com/v1beta/models/{{MODEL}}", Api::Gemini)));
        assert!(dynamic_models_supported(&fixture("m", "https://api.openai.com/v1/responses", Api::Responses)));
        assert!(dynamic_models_supported(&fixture("m", OLLAMA_ENDPOINT, Api::Llama)));
        let responses = fixture("m", "https://api.openai.com/v1/responses", Api::Responses);
        assert!(supports_previous_response_id(&responses, false));
        assert!(!supports_previous_response_id(&responses, true));
        assert!(!supports_previous_response_id(&fixture("m", "https://api.openai.com/v1/chat/completions", Api::ChatCompletions), false));
        let mut m = fixture("llama3.3 (http://gpu:11434)", "http://gpu:11434/api/chat", Api::Llama);
        assert_eq!(effective_name(&m), "llama3.3 (http://gpu:11434)");
        m.wire_model_name = Some("llama3.3".into());
        assert_eq!(effective_name(&m), "llama3.3");
    }

    #[test]
    fn fixture_follows_the_brief() {
        let m = fixture("m", "https://api.anthropic.com/v1/messages", Api::ChatCompletions);
        assert_eq!((m.context_window_tokens, m.max_response_tokens), (128000, 8192));
        assert_eq!(m.features, BTreeSet::from([Feature::Streaming, Feature::FunctionCalling]));
        assert_eq!(m.vector_store, VectorStore::Disabled);
        assert!(m.supports_temperature);
        assert_eq!(m.vendor, Vendor::Anthropic);
        assert_eq!((m.thinking_off_effort, m.thinking_on_effort), (Effort::Low, Effort::Medium));
        assert!(m.reasoning_efforts.is_empty() && m.service_tiers.is_empty());
        assert_eq!((m.wire_model_name, m.custom_headers.len()), (None, 0));
    }

    #[test]
    fn max_tokens_follows_iterm2() {
        assert_eq!(tokens("h\u{e9}llo"), 3);
        assert_eq!(tokens(""), 0);
        let m = fixture("m", "https://api.openai.com/v1/responses", Api::Responses);
        assert_eq!(max_tokens(&m, 128000, "S\nhi", ""), 8192);
        assert_eq!(max_tokens(&m, 10, "0123456789abcdef", ""), 2);
        assert_eq!(max_tokens(&m, 10, "0123456789abcdef", "0123"), 0);
        assert_eq!(max_tokens(&m, 1, "0123456789abcdef", "0123"), -9);
    }

    #[test]
    fn attachments_follow_iterm2() {
        for mime in ["text/x-rust", "application/json", "application/javascript", "application/ecmascript", "image/svg+xml", "application/xml", "message/rfc822", "application/x-sql", "application/x-tex"] {
            assert!(mime_is_textual(mime), "{mime}");
        }
        for mime in ["application/pdf", "image/png"] {
            assert!(!mime_is_textual(mime), "{mime}");
        }

        let mut responses = fixture("m", "https://api.openai.com/v1/responses", Api::Responses);
        assert!(accepts(&responses, "application/pdf"));
        assert!(accepts(&responses, "image/png"));
        assert!(accepts(&responses, "text/plain"));
        assert!(!accepts(&responses, "audio/wav"));
        assert!(!accepts(&responses, "video/mp4"));
        assert!(!accepts(&responses, "application/zip"));
        assert!(!inline_base64(&responses, "application/pdf"));
        assert!(!inline_base64(&responses, "image/png"));
        responses.vector_store = VectorStore::OpenAI;
        assert!(accepts(&responses, "application/zip"));
        assert!(should_upload(&responses, "application/zip"));
        assert!(!should_upload(&responses, "image/png"));
        assert!(!should_upload(&responses, "text/plain"));
        // The source uploads a PDF here (P:144-163 has no PDF exclusion), so its inline PDF
        // branch (P:290-292) is unreachable; the brief's "true" follows that comment, not the code.
        assert!(should_upload(&responses, "application/pdf"));
        assert!(!inline_base64(&responses, "application/pdf"));
        assert!(inline_base64(&responses, "image/png"));
        assert!(!inline_base64(&responses, "application/zip"));

        let chat = fixture("m", "https://api.openai.com/v1/chat/completions", Api::ChatCompletions);
        assert!(accepts(&chat, "audio/mp3"));
        assert!(!accepts(&chat, "audio/ogg"));
        let gemini = fixture("m", "https://generativelanguage.googleapis.com/v1beta/models/{{MODEL}}", Api::Gemini);
        assert!(accepts(&gemini, "video/mp4"));
        assert!(accepts(&gemini, "audio/ogg"));
        let mut llama = fixture("m", OLLAMA_ENDPOINT, Api::Llama);
        assert!(!accepts(&llama, "image/png"));
        llama.features.insert(Feature::Vision);
        assert!(accepts(&llama, "image/png"));
        let local = fixture("m", "http://localhost:1234/v1/chat/completions", Api::ChatCompletions);
        assert!(!accepts(&local, "application/pdf"));
        assert!(!accepts(&local, "image/png"));
    }
}
