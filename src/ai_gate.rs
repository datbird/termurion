//! The gate every AI feature asks first (`AITerm/iTermAITermGatekeeper.swift`): the
//! administrator's switch, then "Enable generative AI features", then an endpoint to send to,
//! then the person's consent for that endpoint's host. The consent replaces iTerm2's plugin
//! check (ruling 1, ruling 12a-1) and is kept per normalized host in `CONSENT`. Pure; the
//! alerts are shown by `app::ai_app`.

use toml::Value;

use crate::ai_catalog::Catalog;
use crate::ai_model::Discovered;
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
    /// The model has nothing Termurion can send to (an imported Apple Intelligence vendor, an
    /// out-of-range vendor, a manual model without a URL).
    NoEndpoint,
    /// The person has not allowed requests to this host yet.
    NeedsConsent { provider: String, host: String },
}

/// The gate's order for the default model: admin, EnableAI, endpoint, consent (Decision:
/// consent follows EnableAI because the host it names depends on settings). `found` is the
/// discovered Ollama models, so a dynamic default is gated on the host it is sent to.
pub fn check(g: &Globals, catalog: &Catalog, found: &Discovered) -> Check {
    check_endpoint(g, default_endpoint(g, catalog, found).as_ref())
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
    /// The first button turns on automatic sharing of terminal contents (Task 8).
    AutoProvide,
    /// The first button turns on the command safety check, whose one-time question holds the
    /// command that raised it (Task 10, ruling R12cd-3).
    SafetyNag,
    /// The second button deletes these chats (the list's Delete, `CWC:844-872`); the first is
    /// Cancel, which a close and Escape press.
    DeleteChats(Vec<String>),
    /// Confirm Change (`CVC:1150-1204`): the answer sets the chat's permission for the category
    /// in that session, only while the chat is still linked to it. The first button also grants
    /// consent to automatic sending; a close answers "Ask Each Time" (ruling F2).
    ConfirmChange { chat: String, guid: String, category: crate::ai_permissions::Category },
    /// The Privacy Warning before streaming (`CVC:1335-1357`): only OK starts it.
    PrivacyWarning { chat: String, guid: String },
    /// The drop question (`IN:507-528`): only the first button (Attach) attaches `supported`;
    /// it also inserts the paths of the others in `all`. Any other answer, a close included
    /// (ruling F2), inserts every path in `all` as text. Applies only while `chat` is shown.
    DropChoice { chat: String, supported: Vec<std::path::PathBuf>, all: Vec<std::path::PathBuf> },
    /// Danger! (`CG:837-850`): ticking "Run commands automatically" in this Codecierge session
    /// (its guid). Only OK ticks the box; Cancel, a close and Escape leave it unticked and turn
    /// auto-run off at once if the session runs.
    CodeciergeDanger(String),
    /// The Privacy Warning before Start (`CG:455`): only OK remembers it and starts this session.
    CodeciergePrivacy(String),
    /// The usage warning (`CG:233-266`): the second button (Stop Codecierge) stops this session;
    /// Keep Going and a close leave it running. Remembered answers act the same way.
    CodeciergeUsage(String),
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
    /// Set only by `alert`: this is one of the gate's alerts (`is_gate`), whatever its words.
    pub gate: bool,
    /// The label of the button a window close (X, Alt+F4) and Escape answer, when the alert
    /// names one (`prompt_ui::escape_index`). None keeps the button labeled "Cancel", else the
    /// first. A consent question with no "Cancel" button names its refusing button here, so
    /// closing it can never grant.
    pub close: Option<&'static str>,
}

/// The warning spec of an alert that has a silence key, as Suppressed Alerts lists it (a
/// permanent silence, buttons as labels), so an answer the silence gives on its own is recorded
/// (ruling F3, 12c ruling 14). None for an alert with no key.
pub fn silence_spec(alert: &Alert) -> Option<crate::warnings::Spec> {
    let id = alert.silence.clone()?;
    Some(crate::warnings::Spec { id: Some(id), kind: crate::warnings::Kind::Permanent, heading: alert.heading.clone(), text: alert.text.clone(), labels: alert.buttons.iter().map(|b| b.to_string()).collect(), cancel: None, never_remember: Vec::new() })
}

pub const FEATURE_UNAVAILABLE: &str = "Feature Unavailable";
pub const ADMIN_OFF: &str = "Generative AI features have been disabled. Check with your system administrator.";
pub const MUST_ENABLE: &str = "You must enable AI features in settings before you can use this feature.";
pub const REVEAL: &str = "Reveal";
pub const NO_ENDPOINT: &str = "The selected AI model has no endpoint Termurion can reach. Choose another model in Settings > General > AI.";
pub const ALLOW: &str = "Allow";
const OK: &str = "OK";
const CANCEL: &str = "Cancel";

pub fn consent_heading(provider: &str) -> String {
    format!("Send AI Requests to {provider}?")
}

pub fn consent_text(provider: &str, host: &str) -> String {
    format!(
        "To use this feature, Termurion sends your request, and any terminal content you choose to share, to {provider} at {host}. You will always be prompted before terminal content is shared. Allow requests to {host}?"
    )
}

/// The alert the gate shows for a refusal (`GK:23-99`); None for Pass.
pub fn alert(c: &Check) -> Option<Alert> {
    let plain = |text: &str, buttons: Vec<&'static str>, then| Alert { heading: FEATURE_UNAVAILABLE.to_string(), text: text.to_string(), buttons, silence: None, then, gate: true, close: None };
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
            gate: true,
            close: None,
        }),
    }
}

/// "Share Terminal Contents Automatically?" (`ChatAgent.swift:1125-1137`): Turn On, Not Now.
/// Not a gate alert. A close (X, Alt+F4) and Escape answer Not Now, never Turn On (ruling: a
/// consent question is refused by a close, as 13b's are); the alert names its refusing button
/// itself because it has no button labeled "Cancel".
pub fn auto_provide_alert() -> Alert {
    use crate::ai_context::{CONSENT_HEADING, CONSENT_TEXT, NOT_NOW, TURN_ON};
    Alert { heading: CONSENT_HEADING.to_string(), text: CONSENT_TEXT.to_string(), buttons: vec![TURN_ON, NOT_NOW], silence: None, then: Then::AutoProvide, gate: false, close: Some(NOT_NOW) }
}

/// Whether an alert is one of the gate's (so `App::ai_check` queues at most one at a time),
/// not another AI feature's alert on the same queue, even one with the same words.
pub fn is_gate(a: &Alert) -> bool {
    a.gate
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
        assert_eq!(check(&g, cat, &Discovered::default()), Check::AdminOff);
        grant(&mut g, "api.openai.com");
        assert_eq!(check(&g, cat, &Discovered::default()), Check::AdminOff, "the admin switch wins whatever else is set");
        g.set(VENDOR, Value::Integer(5));
        assert_eq!(check(&g, cat, &Discovered::default()), Check::AdminOff);

        let mut g = open();
        g.set(ENABLE_AI, Value::Boolean(false));
        assert_eq!(check(&g, cat, &Discovered::default()), Check::AiOff);
        g.set(VENDOR, Value::Integer(5));
        assert_eq!(check(&g, cat, &Discovered::default()), Check::AiOff);

        let mut g = open();
        g.set(VENDOR, Value::Integer(5));
        assert_eq!(check(&g, cat, &Discovered::default()), Check::NoEndpoint, "Apple Intelligence has no endpoint on Windows");
        g.set(VENDOR, Value::Integer(2));
        assert_eq!(check(&g, cat, &Discovered::default()), needs("OpenAI", "api.openai.com"));
        grant(&mut g, "api.openai.com");
        assert_eq!(check(&g, cat, &Discovered::default()), Check::Pass);
        select_provider(&mut g, Vendor::Anthropic, cat);
        assert_eq!(check(&g, cat, &Discovered::default()), needs("Anthropic", "api.anthropic.com"), "a new provider asks again");

        let t: toml::Table = r#"m = {name="llama3", url="http://localhost:11434/api/chat", api=5}"#.parse().expect("test TOML parses");
        let m = ManualModel::from_table(t["m"].as_table().expect("a table")).expect("a manual model");
        crate::ai_models::write(&mut g, std::slice::from_ref(&m));
        select_manual(&mut g, &m);
        assert_eq!(check(&g, cat, &Discovered::default()), needs("Ollama", "localhost"), "a localhost model asks too");

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
        assert_eq!(check(&g, cat, &Discovered::default()), needs("OpenAI", "api.openai.com"), "other hosts never let the endpoint's pass");
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
                then: Then::Nothing,
                gate: true,
                close: None
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
                then: Then::Reveal,
                gate: true,
                close: None
            }
        );
        let a = alert(&Check::NoEndpoint).expect("an alert");
        assert_eq!(
            a,
            Alert {
                heading: "Feature Unavailable".into(),
                text: "The selected AI model has no endpoint Termurion can reach. Choose another model in Settings > General > AI.".into(),
                buttons: vec!["Reveal", "Cancel"],
                silence: None,
                then: Then::Reveal,
                gate: true,
                close: None
            }
        );
        let a = alert(&needs("OpenAI", "api.openai.com")).expect("an alert");
        assert_eq!(
            a,
            Alert {
                heading: "Send AI Requests to OpenAI?".into(),
                text: "To use this feature, Termurion sends your request, and any terminal content you choose to share, to OpenAI at api.openai.com. You will always be prompted before terminal content is shared. Allow requests to api.openai.com?".into(),
                buttons: vec!["Allow", "Cancel"],
                silence: None,
                then: Then::Grant("api.openai.com".into()),
                gate: true,
                close: None
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
        let other = Alert { heading: "AI Error".into(), text: "x".into(), buttons: vec!["OK"], silence: None, then: Then::Nothing, gate: false, close: None };
        assert!(!is_gate(&other));
        let reveal_elsewhere = Alert { then: Then::Reveal, ..other.clone() };
        assert!(!is_gate(&reveal_elsewhere), "a later plan's alert may reveal Settings too");
        // Only the gate's own alerts are tagged; the same words from another producer are not.
        for c in [Check::AdminOff, Check::AiOff, Check::NoEndpoint] {
            let same_words = Alert { gate: false, ..alert(&c).expect("an alert") };
            assert!(!is_gate(&same_words), "{c:?}");
        }
        let grant_elsewhere = Alert { then: Then::Grant("h".into()), ..other };
        assert!(!is_gate(&grant_elsewhere));
    }

    #[test]
    fn auto_provide_alert_follows_iterm2() {
        let a = auto_provide_alert();
        assert_eq!(a.heading, crate::ai_context::CONSENT_HEADING);
        assert_eq!(a.text, crate::ai_context::CONSENT_TEXT);
        assert_eq!(a.buttons, vec!["Turn On", "Not Now"]);
        assert_eq!((a.then.clone(), a.silence.clone(), a.gate), (Then::AutoProvide, None, false));
        assert_eq!(a.close, Some("Not Now"));
        assert!(a.buttons.contains(&a.close.expect("a close label")), "the close label is one of the buttons");
    }

    /// The consent's close answer is Not Now (never Turn On), by an explicit label, and the
    /// gate's own alerts keep their Cancel.
    #[cfg(windows)]
    #[test]
    fn closing_the_auto_provide_question_answers_not_now() {
        let a = auto_provide_alert();
        let i = crate::prompt_ui::close_index_with(&a.buttons, a.close);
        assert_eq!(a.buttons[i], "Not Now");
        assert_ne!(i, 0, "the first button turns sharing on");
        assert_eq!(crate::prompt_ui::escape_index(&a.buttons, a.close), Some(i), "Escape answers the same");
        let g = alert(&needs("OpenAI", "api.openai.com")).expect("the consent alert");
        assert_eq!(g.close, None);
        assert_eq!(g.buttons[crate::prompt_ui::close_index_with(&g.buttons, g.close)], CANCEL, "the gate's close answer is unchanged");
    }

    /// Milestone 13b's rule for consent questions: closing one with the title bar X or Alt+F4
    /// refuses it. The AI alert window answers a close with `close_index`, which must land on
    /// Cancel, never on Allow (the first button, which grants).
    #[cfg(windows)]
    #[test]
    fn closing_the_consent_refuses_it() {
        let a = alert(&needs("OpenAI", "api.openai.com")).expect("the consent alert");
        assert!(matches!(a.then, Then::Grant(_)));
        let i = crate::prompt_ui::close_index(&a.buttons);
        assert_ne!(i, 0, "the first button grants");
        assert_eq!(a.buttons[i], CANCEL);
    }

    /// An alert with a silence key is listed in Suppressed Alerts when the silence answers it, and
    /// Unsuppress forgets the answer (ruling F3).
    #[test]
    fn a_silenced_ai_alert_is_listed_in_suppressed_alerts() {
        use crate::warnings::{self, Catalog};
        let id = crate::chat_input::DROP_SILENCE;
        let spec = silence_spec(&crate::chat_input::drop_alert("c", vec![], vec![])).unwrap();
        assert_eq!((spec.id.as_deref(), spec.kind, spec.labels.clone()), (Some(id), warnings::Kind::Permanent, vec!["Attach".to_string(), "Insert Path".to_string()]));
        assert_eq!(silence_spec(&crate::chat_menu::help_alert()), None, "no key, nothing to list");

        let mut g = open();
        g.set(id, Value::Boolean(true));
        g.set(&warnings::selection_key(id), Value::Integer(0));
        let mut catalog = Catalog::default();
        catalog.record(&spec, &spec.labels[0], warnings::episode(&g, id, 5.0), 5.0);
        let listed = catalog.current(&g, 6.0);
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].id.as_str(), listed[0].label.as_str(), listed[0].count), (id, "Attach", 1));
        catalog.unsuppress(&mut g, id);
        assert!(catalog.current(&g, 7.0).is_empty());
        assert!(g.get(id).is_none() && g.get(&warnings::selection_key(id)).is_none(), "the drop question asks again");
    }
}
