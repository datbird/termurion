//! The AI gate for each request a chat sends (ruling R12cd-1): the endpoint a request really
//! goes to, and whether 12a's gate is open for it (the administrator's switch, "Enable
//! generative AI features" and the consent for that endpoint's host). Pure, so the App-free
//! chat engine and its tests can ask it; the App shows a refusal through
//! `App::ai_check_endpoint`, which runs the same check (iTerm2 checks
//! `iTermAITermGatekeeper.allowed` at every registration,
//! `AITermControllerRegistrationHelper.swift:11-24`).

use crate::ai_catalog::Model;
use crate::ai_gate::{check_endpoint, Check};
use crate::ai_models::Endpoint;
use crate::globals::Globals;

/// Where a request for this model goes: the fields 12a's `default_endpoint` fills, so consent
/// is asked for the host the request really goes to (a model pinned to another host included).
pub fn endpoint_of(m: &Model) -> Endpoint {
    crate::ai_model::endpoint(m)
}

/// Where the chat's vector-store jobs go: api.openai.com, whatever the chat model's URL.
pub fn files_endpoint(m: &Model) -> Endpoint {
    Endpoint { url: crate::ai_files::STORES_URL.to_string(), ..endpoint_of(m) }
}

/// Whether a request to `e` may be sent now; a refusal sends nothing.
pub fn gate_open(g: &Globals, e: &Endpoint) -> bool {
    check_endpoint(g, Some(e)) == Check::Pass
}

/// Proof that the gate for a request's own endpoint was open just before it is sent (12b final
/// Minor 2): each of the chat engine's sends takes one, and only `pass` makes one.
#[derive(Debug, PartialEq)]
pub struct Passed(());

impl Passed {
    /// A proof for a test that drives a transport with no gate in front of it.
    #[cfg(test)]
    pub fn for_test() -> Passed {
        Passed(())
    }
}

/// `gate_open` as a proof to spend on one send.
pub fn pass(g: &Globals, e: &Endpoint) -> Option<Passed> {
    gate_open(g, e).then_some(Passed(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_gate::grant;
    use crate::ai_models::Endpoint;
    use crate::ai_settings::{Api, ADMIN, ENABLE_AI};
    use crate::globals::Globals;
    use std::path::Path;
    use toml::Value;

    #[test]
    fn endpoint_follows_the_model() {
        let mut m = crate::ai_model::fixture("m", "https://pinned.test/v1/chat/completions", Api::ChatCompletions);
        assert_eq!(
            endpoint_of(&m),
            Endpoint { vendor: m.vendor, model: "m".into(), url: "https://pinned.test/v1/chat/completions".into(), api: Api::ChatCompletions, custom_headers: vec![] }
        );
        m.custom_headers = vec![("X-K".into(), "1".into())];
        assert_eq!(endpoint_of(&m).custom_headers, vec![("X-K".to_string(), "1".to_string())], "the model's headers are carried");
        let files = files_endpoint(&m);
        assert_eq!(files.url, crate::ai_files::STORES_URL);
        assert_eq!(Endpoint { url: endpoint_of(&m).url, ..files }, endpoint_of(&m), "only the URL differs");

        let e = endpoint_of(&m);
        let mut g = Globals::empty(Path::new("chat-gate-test.toml"));
        g.set(ENABLE_AI, Value::Boolean(false));
        assert!(!gate_open(&g, &e), "Enable generative AI features is off");
        g.set(ENABLE_AI, Value::Boolean(true));
        assert!(!gate_open(&g, &e), "no consent");
        grant(&mut g, "api.openai.com");
        assert!(!gate_open(&g, &e), "another host's consent never counts");
        grant(&mut g, "pinned.test");
        assert!(gate_open(&g, &e));
        g.set(ADMIN, Value::Boolean(false));
        assert!(!gate_open(&g, &e), "the admin switch wins");
    }
}
