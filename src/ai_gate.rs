//! The gate every AI feature asks first (`AITerm/iTermAITermGatekeeper.swift`): the
//! administrator's switch, then "Enable generative AI features", then an endpoint to send to,
//! then the person's consent for that endpoint's host. The consent replaces iTerm2's plugin
//! check (ruling 1, ruling 12a-1) and is kept per normalized host in `CONSENT`. Pure; the
//! alerts are shown by `app::ai_app`.

use toml::Value;

use crate::ai_catalog::Catalog;
use crate::ai_models::{default_endpoint, Endpoint};
use crate::ai_policy::host_of;
use crate::ai_settings::{ADMIN, CONSENT, ENABLE_AI};
use crate::globals::Globals;

/// What the gate decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    Pass,
    /// `GenerativeAIAllowed` is off.
    AdminOff,
    /// `EnableAI` is off.
    AiOff,
    /// The model has nothing Tandem can send to (an imported Apple Intelligence vendor, an
    /// out-of-range vendor, a manual model without a URL).
    NoEndpoint,
    /// The person has not allowed requests to this host yet.
    NeedsConsent { provider: String, host: String },
}

/// The gate's order for the default model: admin, EnableAI, endpoint, consent (Decision:
/// consent follows EnableAI because the host it names depends on settings).
pub fn check(g: &Globals, catalog: &Catalog) -> Check {
    check_endpoint(g, default_endpoint(g, catalog).as_ref())
}

/// The same order for an endpoint a feature chose itself (a pinned model, the economy model,
/// Test Connection; ruling R12ab-2).
pub fn check_endpoint(g: &Globals, endpoint: Option<&Endpoint>) -> Check {
    if !g.bool(ADMIN) {
        return Check::AdminOff;
    }
    if !g.bool(ENABLE_AI) {
        return Check::AiOff;
    }
    let Some(e) = endpoint else { return Check::NoEndpoint };
    let Some(host) = host_of(&e.url) else { return Check::NoEndpoint };
    if consented(g, &host) {
        Check::Pass
    } else {
        Check::NeedsConsent { provider: e.vendor.provider_name().to_string(), host }
    }
}

/// Lowercased with trailing dots removed, as `ai_policy::host_of` normalizes.
fn normalized(host: &str) -> String {
    host.to_lowercase().trim_end_matches('.').to_string()
}

/// The allowed hosts as stored: string entries only.
fn hosts(g: &Globals) -> Vec<String> {
    match g.values.get(CONSENT) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(normalized)).collect(),
        _ => Vec::new(),
    }
}

/// Whether the person allowed requests to `host` (compared normalized on both sides).
pub fn consented(g: &Globals, host: &str) -> bool {
    let host = normalized(host);
    !host.is_empty() && hosts(g).contains(&host)
}

/// Remembers that the person allowed requests to `host`, once.
pub fn grant(g: &mut Globals, host: &str) {
    let host = normalized(host);
    if host.is_empty() {
        return;
    }
    let mut list = Vec::new();
    for h in hosts(g).into_iter().chain(std::iter::once(host)) {
        if !list.contains(&h) {
            list.push(h);
        }
    }
    g.set(CONSENT, Value::Array(list.into_iter().map(Value::String).collect()));
}

/// Settings' "Forget Allowed Hosts": every host asks again.
pub fn forget_all(g: &mut Globals) {
    g.values.remove(CONSENT);
}

/// What an alert's answer does. Later plans (12c to 12f) add variants for their own alerts and
/// their arms in `App::ai_answered` (ruling 12c-7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Then {
    Nothing,
    /// The first button opens Settings > General > AI.
    Reveal,
    /// The first button allows requests to this host.
    Grant(String),
}

/// One AI alert for `App::ask_ai`. `silence` is a permanent-silence key (the `scripts_alerts`
/// pattern); every gate alert has None, since iTerm2's gate warnings are
/// `kiTermWarningTypePersistent` with no identifier (`GK:27-95`, `W:379-406`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub heading: String,
    pub text: String,
    pub buttons: Vec<&'static str>,
    pub silence: Option<String>,
    pub then: Then,
}

pub const FEATURE_UNAVAILABLE: &str = "Feature Unavailable";
pub const ADMIN_OFF: &str = "Generative AI features have been disabled. Check with your system administrator.";
pub const MUST_ENABLE: &str = "You must enable AI features in settings before you can use this feature.";
pub const REVEAL: &str = "Reveal";
pub const NO_ENDPOINT: &str = "The selected AI model has no endpoint Tandem can reach. Choose another model in Settings > General > AI.";
pub const ALLOW: &str = "Allow";
const OK: &str = "OK";
const CANCEL: &str = "Cancel";

pub fn consent_heading(provider: &str) -> String {
    format!("Send AI Requests to {provider}?")
}

pub fn consent_text(provider: &str, host: &str) -> String {
    format!(
        "To use this feature, Tandem sends your request, and any terminal content you choose to share, to {provider} at {host}. You will always be prompted before terminal content is shared. Allow requests to {host}?"
    )
}

/// The alert the gate shows for a refusal (`GK:23-99`); None for Pass.
pub fn alert(c: &Check) -> Option<Alert> {
    let plain = |text: &str, buttons: Vec<&'static str>, then| Alert { heading: FEATURE_UNAVAILABLE.to_string(), text: text.to_string(), buttons, silence: None, then };
    match c {
        Check::Pass => None,
        Check::AdminOff => Some(plain(ADMIN_OFF, vec![OK], Then::Nothing)),
        Check::AiOff => Some(plain(MUST_ENABLE, vec![REVEAL, CANCEL], Then::Reveal)),
        Check::NoEndpoint => Some(plain(NO_ENDPOINT, vec![REVEAL, CANCEL], Then::Reveal)),
        Check::NeedsConsent { provider, host } => Some(Alert {
            heading: consent_heading(provider),
            text: consent_text(provider, host),
            buttons: vec![ALLOW, CANCEL],
            silence: None,
            then: Then::Grant(host.clone()),
        }),
    }
}

/// Whether an alert is one of the gate's (so `App::ai_check` queues at most one at a time),
/// not another AI feature's alert on the same queue.
pub fn is_gate(a: &Alert) -> bool {
    match &a.then {
        Then::Grant(_) => true,
        _ => a.heading == FEATURE_UNAVAILABLE && [ADMIN_OFF, MUST_ENABLE, NO_ENDPOINT].contains(&a.text.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_catalog::bundled;
    use crate::ai_models::{select_manual, select_provider, ManualModel};
    use crate::ai_settings::{Vendor, VENDOR};
    use std::path::Path;

    fn open() -> Globals {
        let mut g = Globals::empty(Path::new("ai-gate-test.toml"));
        g.set(ENABLE_AI, Value::Boolean(true));
        g
    }

    fn needs(provider: &str, host: &str) -> Check {
        Check::NeedsConsent { provider: provider.to_string(), host: host.to_string() }
    }

    #[test]
    fn gate_follows_iterm2() {
        let cat = bundled();
        let mut g = open();
        g.set(ADMIN, Value::Boolean(false));
        assert_eq!(check(&g, cat), Check::AdminOff);
        grant(&mut g, "api.openai.com");
        assert_eq!(check(&g, cat), Check::AdminOff, "the admin switch wins whatever else is set");
        g.set(VENDOR, Value::Integer(5));
        assert_eq!(check(&g, cat), Check::AdminOff);

        let mut g = open();
        g.set(ENABLE_AI, Value::Boolean(false));
        assert_eq!(check(&g, cat), Check::AiOff);
        g.set(VENDOR, Value::Integer(5));
        assert_eq!(check(&g, cat), Check::AiOff);

        let mut g = open();
        g.set(VENDOR, Value::Integer(5));
        assert_eq!(check(&g, cat), Check::NoEndpoint, "Apple Intelligence has no endpoint on Windows");
        g.set(VENDOR, Value::Integer(2));
        assert_eq!(check(&g, cat), needs("OpenAI", "api.openai.com"));
        grant(&mut g, "api.openai.com");
        assert_eq!(check(&g, cat), Check::Pass);
        select_provider(&mut g, Vendor::Anthropic, cat);
        assert_eq!(check(&g, cat), needs("Anthropic", "api.anthropic.com"), "a new provider asks again");

        let t: toml::Table = r#"m = {name="llama3", url="http://localhost:11434/api/chat", api=5}"#.parse().expect("test TOML parses");
        let m = ManualModel::from_table(t["m"].as_table().expect("a table")).expect("a manual model");
        crate::ai_models::write(&mut g, std::slice::from_ref(&m));
        select_manual(&mut g, &m);
        assert_eq!(check(&g, cat), needs("Ollama", "localhost"), "a localhost model asks too");

        assert_eq!(check_endpoint(&g, None), Check::NoEndpoint);
        let mut closed = open();
        closed.set(ENABLE_AI, Value::Boolean(false));
        assert_eq!(check_endpoint(&closed, None), Check::AiOff, "the same order for a chosen endpoint");
    }

    #[test]
    fn consent_is_kept_per_host() {
        let cat = bundled();
        let mut g = open();
        grant(&mut g, "API.OpenAI.com.");
        assert_eq!(g.values.get(CONSENT), Some(&Value::Array(vec![Value::String("api.openai.com".into())])));
        grant(&mut g, "api.openai.com");
        assert_eq!(g.values.get(CONSENT), Some(&Value::Array(vec![Value::String("api.openai.com".into())])), "granting again keeps one entry");
        assert!(consented(&g, "api.openai.com"));
        assert!(!consented(&g, "api.anthropic.com"));
        forget_all(&mut g);
        assert_eq!(g.values.get(CONSENT), None);
        assert!(!consented(&g, "api.openai.com"));

        grant(&mut g, "api.anthropic.com");
        grant(&mut g, "localhost");
        assert_eq!(check(&g, cat), needs("OpenAI", "api.openai.com"), "other hosts never let the endpoint's pass");
    }

    #[test]
    fn gate_alerts_follow_iterm2() {
        assert_eq!(alert(&Check::Pass), None);
        let a = alert(&Check::AdminOff).expect("an alert");
        assert_eq!(
            a,
            Alert {
                heading: "Feature Unavailable".into(),
                text: "Generative AI features have been disabled. Check with your system administrator.".into(),
                buttons: vec!["OK"],
                silence: None,
                then: Then::Nothing
            }
        );
        let a = alert(&Check::AiOff).expect("an alert");
        assert_eq!(
            a,
            Alert {
                heading: "Feature Unavailable".into(),
                text: "You must enable AI features in settings before you can use this feature.".into(),
                buttons: vec!["Reveal", "Cancel"],
                silence: None,
                then: Then::Reveal
            }
        );
        let a = alert(&Check::NoEndpoint).expect("an alert");
        assert_eq!(
            a,
            Alert {
                heading: "Feature Unavailable".into(),
                text: "The selected AI model has no endpoint Tandem can reach. Choose another model in Settings > General > AI.".into(),
                buttons: vec!["Reveal", "Cancel"],
                silence: None,
                then: Then::Reveal
            }
        );
        let a = alert(&needs("OpenAI", "api.openai.com")).expect("an alert");
        assert_eq!(
            a,
            Alert {
                heading: "Send AI Requests to OpenAI?".into(),
                text: "To use this feature, Tandem sends your request, and any terminal content you choose to share, to OpenAI at api.openai.com. You will always be prompted before terminal content is shared. Allow requests to api.openai.com?".into(),
                buttons: vec!["Allow", "Cancel"],
                silence: None,
                then: Then::Grant("api.openai.com".into())
            }
        );
        for c in [Check::AdminOff, Check::AiOff, Check::NoEndpoint, needs("Anthropic", "api.anthropic.com")] {
            assert_eq!(alert(&c).expect("an alert").silence, None, "{c:?} is never silenceable");
        }
    }

    #[test]
    fn gate_alerts_are_told_apart() {
        for c in [Check::AdminOff, Check::AiOff, Check::NoEndpoint, needs("Anthropic", "api.anthropic.com")] {
            assert!(is_gate(&alert(&c).expect("an alert")), "{c:?}");
        }
        let other = Alert { heading: "AI Error".into(), text: "x".into(), buttons: vec!["OK"], silence: None, then: Then::Nothing };
        assert!(!is_gate(&other));
        let reveal_elsewhere = Alert { then: Then::Reveal, ..other };
        assert!(!is_gate(&reveal_elsewhere), "a later plan's alert may reveal Settings too");
    }
}
