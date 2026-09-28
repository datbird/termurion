//! The bundled AI model catalog: iTerm2's `ai-models.json`, decoded fail-safe as
//! `AITerm/AIModelCatalog.swift` does, with the default, recommended and economy models. Pure;
//! Tandem never downloads a newer catalog (ruling 6).

use crate::ai_settings::{Api, Vendor};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// iTerm2's catalog, byte for byte.
pub const BUNDLED_JSON: &str = include_str!("../resources/ai/ai-models.json");

/// A model capability (`AIMetadata.Model.Feature`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    FunctionCalling,
    Streaming,
    HostedFileSearch,
    HostedWebSearch,
    HostedCodeInterpreter,
    ConfigurableThinking,
    Vision,
}

/// iTerm2's `VectorStoreConfig`, stored as `AIVectorStore`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VectorStore {
    Disabled,
    OpenAI,
}

impl VectorStore {
    pub fn raw(self) -> i64 {
        self as i64
    }

    pub fn from_raw(v: i64) -> Option<VectorStore> {
        match v {
            0 => Some(VectorStore::Disabled),
            1 => Some(VectorStore::OpenAI),
            _ => None,
        }
    }
}

/// A reasoning effort (`AITerm/AIReasoningTypes.swift:19-26`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

impl Effort {
    /// The wire string (`AIReasoningEffort`'s raw value).
    pub fn as_str(self) -> &'static str {
        match self {
            Effort::None => "none",
            Effort::Minimal => "minimal",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
        }
    }
}

/// A Responses API service tier (`AITerm/AIReasoningTypes.swift:28-33`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceTier {
    Auto,
    Default,
    Flex,
    Priority,
}

impl ServiceTier {
    /// The wire string (`AIServiceTier`'s raw value).
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceTier::Auto => "auto",
            ServiceTier::Default => "default",
            ServiceTier::Flex => "flex",
            ServiceTier::Priority => "priority",
        }
    }
}

/// One catalog model (`AIMetadata.Model`).
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub name: String,
    pub vendor: Vendor,
    pub api: Api,
    pub url: String,
    pub context_window_tokens: i64,
    pub max_response_tokens: i64,
    pub features: BTreeSet<Feature>,
    pub vector_store: VectorStore,
    pub supports_temperature: bool,
    pub recommended: bool,
    pub fixture_exempt: bool,
    pub economy_model: Option<String>,
    pub thinking_off_effort: Effort,
    pub thinking_on_effort: Effort,
    pub reasoning_efforts: Vec<Effort>,
    pub service_tiers: Vec<ServiceTier>,
    /// The model id sent on the wire when it differs from `name` (a disambiguated dynamic
    /// Ollama tag, `AM:210-218`); None for every catalog entry.
    pub wire_model_name: Option<String>,
    /// Headers merged into every request to this model (`AM:220-225`), in order; empty for
    /// every catalog entry.
    pub custom_headers: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Catalog {
    pub version: i64,
    pub models: Vec<Model>,
}

impl Catalog {
    /// The first entry (`AM:148`); a parsed catalog is never empty.
    pub fn default_model(&self) -> &Model {
        &self.models[0]
    }

    /// The vendor's flagged entry, else its first (`CAT:121-128`).
    pub fn recommended(&self, v: Vendor) -> Option<&Model> {
        let mut of_vendor = self.models.iter().filter(|m| m.vendor == v);
        let first = of_vendor.clone().next();
        of_vendor.find(|m| m.recommended).or(first)
    }

    /// The first entry with this name.
    pub fn named(&self, name: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.name == name)
    }

    /// The cheaper model for low-stakes work (`AM:269-301`): the model's own pointer, else its
    /// catalog twin's by name. The result keeps the model's url and api, so a key configured for
    /// a proxy never goes to the vendor's public host.
    pub fn economy_for(&self, model: &Model) -> Option<Model> {
        let name = model.economy_model.clone().or_else(|| self.named(&model.name).and_then(|m| m.economy_model.clone()))?;
        let mut economy = self.named(&name)?.clone();
        economy.url = model.url.clone();
        economy.api = model.api;
        Some(economy)
    }
}

/// The file as Swift's `Decodable` sees it (`CAT:196-217`): a missing or mistyped required field
/// fails the whole file; enum-like values stay strings so an unknown one is skipped later.
#[derive(Deserialize)]
struct CatalogDto {
    version: i64,
    models: Vec<ModelDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelDto {
    name: String,
    vendor: String,
    api: String,
    url: String,
    context_window_tokens: i64,
    max_response_tokens: i64,
    features: Vec<String>,
    vector_store: Option<String>,
    supports_temperature: Option<bool>,
    recommended: Option<bool>,
    fixture_exempt: Option<bool>,
    economy_model: Option<String>,
    thinking_off_effort: Option<String>,
    thinking_on_effort: Option<String>,
    reasoning_efforts: Option<Vec<String>>,
    service_tiers: Option<Vec<String>>,
}

impl ModelDto {
    /// `CAT:219-245`: None when the api or vendor is unknown to this build.
    fn to_model(&self) -> Option<Model> {
        let api = api_case(&self.api)?;
        let vendor = vendor_case(&self.vendor)?;
        Some(Model {
            name: self.name.clone(),
            vendor,
            api,
            url: self.url.clone(),
            context_window_tokens: self.context_window_tokens,
            max_response_tokens: self.max_response_tokens,
            features: self.features.iter().filter_map(|f| feature_case(f)).collect(),
            vector_store: if self.vector_store.as_deref() == Some("openAI") { VectorStore::OpenAI } else { VectorStore::Disabled },
            supports_temperature: self.supports_temperature.unwrap_or(true),
            recommended: self.recommended.unwrap_or(false),
            fixture_exempt: self.fixture_exempt.unwrap_or(false),
            economy_model: self.economy_model.clone(),
            thinking_off_effort: self.thinking_off_effort.as_deref().and_then(effort_case).unwrap_or(Effort::Low),
            thinking_on_effort: self.thinking_on_effort.as_deref().and_then(effort_case).unwrap_or(Effort::Medium),
            reasoning_efforts: self.reasoning_efforts.iter().flatten().filter_map(|e| effort_case(e)).collect(),
            service_tiers: self.service_tiers.iter().flatten().filter_map(|t| tier_case(t)).collect(),
            wire_model_name: None,
            custom_headers: Vec::new(),
        })
    }
}

/// `CAT:261-274`.
fn api_case(s: &str) -> Option<Api> {
    Some(match s {
        "completions" => Api::Completions,
        "chatCompletions" => Api::ChatCompletions,
        "responses" => Api::Responses,
        "gemini" => Api::Gemini,
        "earlyO1" => Api::EarlyO1,
        "llama" => Api::Llama,
        "deepSeek" => Api::DeepSeek,
        "anthropic" => Api::Anthropic,
        "appleIntelligence" => Api::AppleIntelligence,
        _ => return None,
    })
}

/// `CAT:276-286`.
fn vendor_case(s: &str) -> Option<Vendor> {
    Some(match s {
        "deepSeek" => Vendor::DeepSeek,
        "gemini" => Vendor::Gemini,
        "openAI" => Vendor::OpenAI,
        "llama" => Vendor::Llama,
        "anthropic" => Vendor::Anthropic,
        "apple" => Vendor::Apple,
        _ => return None,
    })
}

/// `CAT:288-301`; others are ignored.
fn feature_case(s: &str) -> Option<Feature> {
    Some(match s {
        "functionCalling" => Feature::FunctionCalling,
        "streaming" => Feature::Streaming,
        "hostedFileSearch" => Feature::HostedFileSearch,
        "hostedWebSearch" => Feature::HostedWebSearch,
        "hostedCodeInterpreter" => Feature::HostedCodeInterpreter,
        "configurableThinking" => Feature::ConfigurableThinking,
        "vision" => Feature::Vision,
        _ => return None,
    })
}

/// `AIReasoningEffort`'s raw values.
fn effort_case(s: &str) -> Option<Effort> {
    Some(match s {
        "none" => Effort::None,
        "minimal" => Effort::Minimal,
        "low" => Effort::Low,
        "medium" => Effort::Medium,
        "high" => Effort::High,
        "xhigh" => Effort::Xhigh,
        _ => return None,
    })
}

/// `AIServiceTier`'s raw values.
fn tier_case(s: &str) -> Option<ServiceTier> {
    Some(match s {
        "auto" => ServiceTier::Auto,
        "default" => ServiceTier::Default,
        "flex" => ServiceTier::Flex,
        "priority" => ServiceTier::Priority,
        _ => return None,
    })
}

/// Decodes a catalog (`CAT:135-149`): entries this build cannot represent are skipped, and a
/// catalog with none left is a failed load.
pub fn parse(json: &str) -> Option<Catalog> {
    let dto: CatalogDto = serde_json::from_str(json).ok()?;
    let models: Vec<Model> = dto.models.iter().filter_map(ModelDto::to_model).collect();
    if models.is_empty() {
        return None;
    }
    Some(Catalog { version: dto.version, models })
}

/// Checks a candidate catalog (`CAT:165-193`) and answers its version: it decodes, has a usable
/// entry, every economy pointer names another entry of the same vendor, and no vendor has two
/// recommended entries. The checks use the declared strings, so unknown entries take part.
pub fn validate(json: &str) -> Option<i64> {
    let dto: CatalogDto = serde_json::from_str(json).ok()?;
    if !dto.models.iter().any(|m| m.to_model().is_some()) {
        return None;
    }
    let mut vendor_by_name: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for m in &dto.models {
        vendor_by_name.entry(m.name.as_str()).or_insert(m.vendor.as_str());
    }
    for m in &dto.models {
        let Some(economy) = m.economy_model.as_deref() else { continue };
        if economy == m.name || vendor_by_name.get(economy) != Some(&m.vendor.as_str()) {
            return None;
        }
    }
    let mut recommended = std::collections::HashSet::new();
    for m in dto.models.iter().filter(|m| m.recommended == Some(true)) {
        if !recommended.insert(m.vendor.as_str()) {
            return None;
        }
    }
    Some(dto.version)
}

/// The bundled catalog, decoded once.
pub fn bundled() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| parse(BUNDLED_JSON).expect("resources/ai/ai-models.json must parse; ai_catalog tests check it"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// A catalog entry with every required field.
    fn entry(name: &str, vendor: &str) -> Value {
        json!({
            "name": name,
            "vendor": vendor,
            "api": "anthropic",
            "url": "https://api.anthropic.com/v1/messages",
            "contextWindowTokens": 1000,
            "maxResponseTokens": 100,
            "features": []
        })
    }

    fn catalog(models: Vec<Value>) -> String {
        json!({ "version": 7, "models": models }).to_string()
    }

    #[test]
    fn bundled_catalog_is_iterm2s() {
        use sha2::{Digest, Sha256};
        assert_eq!(BUNDLED_JSON.len(), 18762);
        let digest: String = Sha256::digest(BUNDLED_JSON.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(digest, "6b0202d571b5696177a37619a794c7e8cca1e9d3ff59d05d04ce9963d914b0b3");
    }

    #[test]
    fn catalog_decodes_like_iterm2() {
        use Feature::*;
        let c = bundled();
        assert_eq!(c.version, 3);
        assert_eq!(c.models.len(), 40);
        let count = |v: Vendor| c.models.iter().filter(|m| m.vendor == v).count();
        assert_eq!(
            [count(Vendor::OpenAI), count(Vendor::Gemini), count(Vendor::Anthropic), count(Vendor::DeepSeek), count(Vendor::Apple), count(Vendor::Llama)],
            [20, 10, 7, 2, 1, 0]
        );
        let openai: Vec<&str> = c.models.iter().filter(|m| m.vendor == Vendor::OpenAI).map(|m| m.name.as_str()).collect();
        assert_eq!(
            openai,
            [
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gpt-5.5",
                "gpt-5.5-pro",
                "gpt-5.4",
                "gpt-5.3-codex",
                "gpt-5.2",
                "gpt-5.2-pro",
                "gpt-5.1",
                "gpt-5",
                "gpt-5-mini",
                "gpt-5-nano",
                "gpt-4.1",
                "gpt-4o",
                "gpt-4o-mini",
                "gpt-4.1-mini",
                "o3",
                "o3-pro",
                "o4-mini"
            ]
        );

        let d = c.default_model();
        assert_eq!(d.name, "gpt-5.6-sol");
        assert_eq!(d.vendor, Vendor::OpenAI);
        assert_eq!(d.api, Api::Responses);
        assert_eq!(d.url, "https://api.openai.com/v1/responses");
        assert_eq!((d.context_window_tokens, d.max_response_tokens), (1050000, 128000));
        assert_eq!(d.features, BTreeSet::from([FunctionCalling, HostedFileSearch, HostedWebSearch, Streaming, HostedCodeInterpreter, ConfigurableThinking]));
        assert_eq!(d.vector_store, VectorStore::Disabled);
        assert!(d.supports_temperature);
        assert!(d.recommended);
        assert!(d.fixture_exempt);
        assert_eq!(d.economy_model.as_deref(), Some("gpt-5.6-luna"));
        // thinkingOnEffort is absent: the default, Medium.
        assert_eq!((d.thinking_off_effort, d.thinking_on_effort), (Effort::None, Effort::Medium));
        assert_eq!(d.reasoning_efforts, [Effort::None, Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh]);
        assert_eq!(d.service_tiers, [ServiceTier::Auto, ServiceTier::Default, ServiceTier::Priority, ServiceTier::Flex]);

        let pro = c.named("gpt-5.5-pro").unwrap();
        assert_eq!((pro.thinking_off_effort, pro.thinking_on_effort), (Effort::Medium, Effort::High));
        assert_eq!(pro.reasoning_efforts, [Effort::Medium, Effort::High, Effort::Xhigh]);
        assert_eq!(pro.service_tiers, [ServiceTier::Auto, ServiceTier::Default, ServiceTier::Flex]);

        let four_o = c.named("gpt-4o").unwrap();
        assert!(four_o.reasoning_efforts.is_empty());
        assert!(four_o.service_tiers.is_empty());
        assert_eq!((four_o.thinking_off_effort, four_o.thinking_on_effort), (Effort::Low, Effort::Medium));

        assert_eq!(c.named("gpt-5").unwrap().vector_store, VectorStore::OpenAI);
        assert!(!c.named("claude-opus-4-8").unwrap().supports_temperature);
        assert!(c.named("claude-opus-4-6").unwrap().supports_temperature);

        let apple = c.named("apple-on-device").unwrap();
        assert_eq!(apple.vendor, Vendor::Apple);
        assert_eq!(apple.api, Api::AppleIntelligence);
        assert_eq!(apple.url, "");
        assert_eq!((apple.context_window_tokens, apple.max_response_tokens), (8192, 1024));
        assert!(apple.features.is_empty());

        let mini = c.named("gpt-4o-mini").unwrap();
        assert_eq!((mini.context_window_tokens, mini.max_response_tokens), (128000, 16384));
        assert_eq!(c.named("gemini-3.7-flash").unwrap().url, "https://generativelanguage.googleapis.com/v1beta/models/{{MODEL}}");
        assert_eq!(c.named("no-such-model"), None);

        assert_eq!(VectorStore::Disabled.raw(), 0);
        assert_eq!(VectorStore::OpenAI.raw(), 1);
        assert_eq!(VectorStore::from_raw(1), Some(VectorStore::OpenAI));
        assert_eq!(VectorStore::from_raw(0), Some(VectorStore::Disabled));
        assert_eq!(VectorStore::from_raw(2), None);
    }

    #[test]
    fn recommended_follows_iterm2() {
        let c = bundled();
        let name = |v: Vendor| c.recommended(v).map(|m| m.name.as_str());
        assert_eq!(name(Vendor::OpenAI), Some("gpt-5.6-sol"));
        assert_eq!(name(Vendor::Anthropic), Some("claude-opus-4-8"));
        assert_eq!(name(Vendor::Gemini), Some("gemini-3.7-flash"));
        assert_eq!(name(Vendor::DeepSeek), Some("deepseek-v4-flash"));
        assert_eq!(name(Vendor::Apple), Some("apple-on-device"));
        assert_eq!(name(Vendor::Llama), None);

        // No flagged entry: the vendor's first.
        let two = parse(&catalog(vec![entry("a", "anthropic"), entry("b", "anthropic")])).unwrap();
        assert_eq!(two.recommended(Vendor::Anthropic).map(|m| m.name.as_str()), Some("a"));
        // A flagged entry wins over an earlier one.
        let mut b = entry("b", "anthropic");
        b["recommended"] = json!(true);
        let flagged = parse(&catalog(vec![entry("a", "anthropic"), b])).unwrap();
        assert_eq!(flagged.recommended(Vendor::Anthropic).map(|m| m.name.as_str()), Some("b"));
    }

    #[test]
    fn economy_keeps_the_transport() {
        let c = bundled();
        let haiku = c.economy_for(c.named("claude-opus-4-8").unwrap()).unwrap();
        assert_eq!(haiku.name, "claude-haiku-4-5");
        assert_eq!(haiku.url, "https://api.anthropic.com/v1/messages");
        assert_eq!(haiku.api, Api::Anthropic);
        assert_eq!((haiku.context_window_tokens, haiku.max_response_tokens), (200000, 64000));

        // A catalog model pointed at a proxy keeps the proxy.
        let mut proxied = c.named("gpt-5").unwrap().clone();
        proxied.url = "https://proxy.example/v1/chat".to_string();
        proxied.api = Api::ChatCompletions;
        let e = c.economy_for(&proxied).unwrap();
        assert_eq!(e.name, "gpt-5-mini");
        assert_eq!(e.url, "https://proxy.example/v1/chat");
        assert_eq!(e.api, Api::ChatCompletions);
        assert_eq!(e.context_window_tokens, 400000);

        // No pointer and no catalog twin.
        let mut other = c.named("gpt-5").unwrap().clone();
        other.name = "my-proxy".to_string();
        other.economy_model = None;
        assert_eq!(c.economy_for(&other), None);
        // The economy model has none of its own.
        assert_eq!(c.economy_for(c.named("gpt-5-mini").unwrap()), None);
        // A manual model with a catalog name uses its twin's pointer.
        let mut manual = c.named("gpt-5").unwrap().clone();
        manual.economy_model = None;
        assert_eq!(c.economy_for(&manual).map(|m| m.name), Some("gpt-5-mini".to_string()));
        // A pointer to a missing model gives none.
        manual.economy_model = Some("gone".to_string());
        assert_eq!(c.economy_for(&manual), None);
    }

    #[test]
    fn catalog_models_have_no_wire_extras() {
        for m in &bundled().models {
            assert_eq!(m.wire_model_name, None, "{}", m.name);
            assert!(m.custom_headers.is_empty(), "{}", m.name);
        }
    }

    #[test]
    fn fail_safe_decoding_follows_iterm2() {
        let valid = entry("valid", "anthropic");
        let mut future_api = entry("future", "anthropic");
        future_api["api"] = json!("futureApi");
        let mistral = entry("mistral", "mistral");
        let mut odd = entry("odd", "anthropic");
        odd["features"] = json!(["streaming", "teleport"]);
        odd["vectorStore"] = json!("weird");
        odd["thinkingOffEffort"] = json!("ultra");
        odd["reasoningEfforts"] = json!(["low", "ultra"]);
        odd["serviceTiers"] = json!(["auto", "gold"]);
        let mut vision = entry("seeing", "gemini");
        vision["features"] = json!(["vision"]);
        let c = parse(&catalog(vec![valid.clone(), future_api.clone(), mistral.clone(), odd, vision])).unwrap();
        assert_eq!(c.version, 7);
        let names: Vec<&str> = c.models.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["valid", "odd", "seeing"]);
        let odd = c.named("odd").unwrap();
        assert_eq!(odd.features, BTreeSet::from([Feature::Streaming]));
        assert_eq!(odd.vector_store, VectorStore::Disabled);
        assert_eq!(odd.thinking_off_effort, Effort::Low);
        assert_eq!(odd.reasoning_efforts, [Effort::Low]);
        assert_eq!(odd.service_tiers, [ServiceTier::Auto]);
        assert_eq!(c.named("seeing").unwrap().features, BTreeSet::from([Feature::Vision]));
        // Optional fields absent: iTerm2's defaults.
        let v = c.named("valid").unwrap();
        assert!(v.supports_temperature && !v.recommended && !v.fixture_exempt);
        assert_eq!(v.economy_model, None);

        // Only skipped entries: a failed load.
        assert_eq!(parse(&catalog(vec![future_api, mistral])), None);
        // A missing required field fails the whole file.
        for field in ["name", "url", "features", "contextWindowTokens"] {
            let mut broken = entry("broken", "anthropic");
            broken.as_object_mut().unwrap().remove(field);
            assert_eq!(parse(&catalog(vec![valid.clone(), broken])), None, "{field}");
        }
        assert_eq!(parse("x"), None);
    }

    #[test]
    fn validate_follows_iterm2() {
        assert_eq!(validate(BUNDLED_JSON), Some(3));
        let with_economy = |economy: &str| {
            let mut a = entry("a", "anthropic");
            a["economyModel"] = json!(economy);
            catalog(vec![a, entry("b", "anthropic"), entry("g", "gemini")])
        };
        assert_eq!(validate(&with_economy("b")), Some(7));
        assert_eq!(validate(&with_economy("a")), None);
        assert_eq!(validate(&with_economy("g")), None);
        assert_eq!(validate(&with_economy("missing")), None);

        let mut a = entry("a", "anthropic");
        a["recommended"] = json!(true);
        let mut b = entry("b", "anthropic");
        b["recommended"] = json!(true);
        assert_eq!(validate(&catalog(vec![a.clone(), b])), None);
        let mut g = entry("g", "gemini");
        g["recommended"] = json!(true);
        assert_eq!(validate(&catalog(vec![a, g])), Some(7));

        assert_eq!(validate(&catalog(vec![entry("m", "mistral"), entry("a", "anthropic")])), Some(7));
        assert_eq!(validate(&catalog(vec![entry("m", "mistral")])), None);
        assert_eq!(validate("x"), None);
    }

    #[test]
    fn wire_strings_round_trip() {
        for e in [Effort::None, Effort::Minimal, Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh] {
            assert_eq!(effort_case(e.as_str()), Some(e));
        }
        for t in [ServiceTier::Auto, ServiceTier::Default, ServiceTier::Flex, ServiceTier::Priority] {
            assert_eq!(tier_case(t.as_str()), Some(t));
        }
        assert_eq!((Effort::Xhigh.as_str(), ServiceTier::Priority.as_str()), ("xhigh", "priority"));
    }
}
