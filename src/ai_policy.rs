//! Which API key a request carries and in which headers, the per-model custom headers, and the
//! advice shown when a self-hosted endpoint refuses a request that carried no key. Pure; ports
//! `AITerm/PrivateIPChecker.swift`, `LLMMetadata.swift:54-76`, `AITerm.swift:160-196`,
//! `LLM+Mac.swift:67-108`, `AICustomHeaders.swift` and `AIWithheldKeyAdvice.swift`.

use crate::ai_settings::Api;
use std::net::{Ipv4Addr, Ipv6Addr};

/// Lowercased with trailing dots removed, as `PrivateIPChecker.normalized` and
/// `URL.it_normalizedHost` agree (issue 13021).
fn normalized(host: &str) -> String {
    host.to_lowercase().trim_end_matches('.').to_string()
}

/// The host of a URL as Foundation's `URL.host` reads it, normalized; None when the text has no
/// `scheme://` authority, the authority names no host, or it holds a backslash, whitespace, a
/// control character or `%`, which Foundation's `URL(string:)` rejects and other parsers read
/// differently (`http://evil.example\@localhost/` must never name localhost).
pub fn host_of(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    if authority.chars().any(|c| c == '\\' || c == '%' || c.is_whitespace() || c.is_control()) {
        return None;
    }
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(inner) = hostport.strip_prefix('[') {
        &inner[..inner.find(']')?]
    } else {
        hostport.split_once(':').map_or(hostport, |(h, _)| h)
    };
    if host.is_empty() {
        return None;
    }
    Some(normalized(host))
}

fn normalized_host(url: &str) -> String {
    host_of(url).unwrap_or_default()
}

pub fn host_is_openai(url: &str) -> bool {
    normalized_host(url) == "api.openai.com"
}

pub fn host_is_google(url: &str) -> bool {
    normalized_host(url) == "generativelanguage.googleapis.com"
}

pub fn host_is_azure(url: &str) -> bool {
    normalized_host(url).ends_with(".azure.com")
}

pub fn host_is_deepseek(url: &str) -> bool {
    normalized_host(url).ends_with(".deepseek.com")
}

pub fn host_is_anthropic(url: &str) -> bool {
    normalized_host(url).ends_with(".anthropic.com")
}

/// localhost, `.local` (mDNS), 127/8, 10/8, 172.16/12, 192.168/16, 169.254/16, ::1, fe80::/10
/// and fc00::/7 (`PrivateIPChecker.swift:37-113`).
pub fn is_local_or_private(host: &str) -> bool {
    let host = normalized(host);
    if host == "localhost" || host.ends_with(".local") {
        return true;
    }
    if let Ok(v4) = host.parse::<Ipv4Addr>() {
        let [a, b, _, _] = v4.octets();
        return a == 127 || a == 10 || (a == 172 && (16..=31).contains(&b)) || (a == 192 && b == 168) || (a == 169 && b == 254);
    }
    if let Ok(v6) = host.parse::<Ipv6Addr>() {
        let o = v6.octets();
        return v6 == Ipv6Addr::LOCALHOST || (o[0] == 0xfe && (o[1] & 0xc0) == 0x80) || (o[0] & 0xfe) == 0xfc;
    }
    false
}

/// Why a request carries the stored vendor key or the placeholder (`AITerm.swift:174-196`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPolicy {
    VendorKey,
    /// Apple Intelligence runs on the device and builds no request.
    OnDevice,
    /// A local or private host never receives a vendor key (issue 12477).
    LocalEndpoint,
}

/// Stands in for the vendor key on endpoints that must never receive one; it goes on the wire.
pub const SELF_HOSTED_PLACEHOLDER: &str = "Placeholder for self-hosted";

/// A URL with no host (as Foundation parses it) gets the vendor key, as iTerm2's.
pub fn key_policy(url: &str, api: Api) -> KeyPolicy {
    if api == Api::AppleIntelligence {
        return KeyPolicy::OnDevice;
    }
    match host_of(url) {
        Some(host) if is_local_or_private(&host) => KeyPolicy::LocalEndpoint,
        _ => KeyPolicy::VendorKey,
    }
}

/// The headers that authorize a request with `key` (trimmed), in iTerm2's order.
pub fn auth_headers(url: &str, api: Api, key: &str) -> Vec<(String, String)> {
    let key = key.trim();
    match api {
        Api::ChatCompletions | Api::Completions | Api::EarlyO1 | Api::Responses | Api::DeepSeek => {
            if host_is_azure(url) {
                vec![("api-key".to_string(), key.to_string())]
            } else {
                vec![("Authorization".to_string(), format!("Bearer {key}"))]
            }
        }
        Api::Anthropic => vec![("x-api-key".to_string(), key.to_string()), ("anthropic-version".to_string(), "2023-06-01".to_string())],
        Api::Gemini | Api::Llama | Api::AppleIntelligence => Vec::new(),
    }
}

/// The names of the headers of `auth_headers` that carry the key, sorted; derived with a
/// sentinel key as iTerm2 does, so it cannot drift from what goes on the wire.
pub fn credential_header_names(url: &str, api: Api) -> Vec<String> {
    const SENTINEL: &str = "TandemCredentialProbe";
    let mut names: Vec<String> = auth_headers(url, api, SENTINEL).into_iter().filter(|(_, v)| v.contains(SENTINEL)).map(|(n, _)| n).collect();
    names.sort();
    names
}

/// Header names a reverse proxy or gateway plausibly authenticates with (`AIWithheldKeyAdvice.swift`).
pub const COMMON_CREDENTIAL_HEADERS: [&str; 6] = ["Authorization", "Proxy-Authorization", "X-Api-Key", "Api-Key", "X-Auth-Token", "X-Access-Token"];

/// An RFC 7230 token.
pub fn header_name_valid(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c))
}

/// No CR, LF or NUL.
pub fn header_value_valid(value: &str) -> bool {
    !value.contains(['\r', '\n', '\0'])
}

/// `base` with a model's custom headers merged in: an invalid name or value is skipped, and a
/// custom header replaces every earlier field whose name matches case-insensitively, so the
/// person's spelling wins (`AICustomHeaders.swift:28-57`).
pub fn merged_headers(base: &[(String, String)], custom: &[(String, String)]) -> Vec<(String, String)> {
    let mut out = base.to_vec();
    for (name, value) in custom {
        if !header_name_valid(name) || !header_value_valid(value) {
            continue;
        }
        out.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        out.push((name.clone(), value.clone()));
    }
    out
}

/// The advice naming the lane's credential header.
pub fn self_hosted_hint(header: &str) -> String {
    format!(
        "Tandem did not send an API key with this request. If this endpoint requires authentication, add the header \u{201c}{header}\u{201d} under \u{201c}Custom headers\u{201d}."
    )
}

/// The advice for a lane with no credential header of its own (Gemini, Ollama).
pub const SELF_HOSTED_HINT_GENERIC: &str =
    "Tandem did not send an API key with this request. If this endpoint requires authentication, add the header it expects under \u{201c}Custom headers\u{201d}.";

/// Advice for a request that carried no vendor key because its endpoint is local, or None when
/// the key was sent, the model runs on the device, or a custom header that `merged_headers`
/// would send already carries a plausible credential.
pub fn withheld_key_hint(url: &str, api: Api, custom: &[(String, String)]) -> Option<String> {
    if key_policy(url, api) != KeyPolicy::LocalEndpoint {
        return None;
    }
    let lane = credential_header_names(url, api);
    let authenticated = custom.iter().any(|(name, value)| {
        header_name_valid(name)
            && header_value_valid(value)
            && lane.iter().map(String::as_str).chain(COMMON_CREDENTIAL_HEADERS).any(|c| c.eq_ignore_ascii_case(name))
    });
    if authenticated {
        return None;
    }
    Some(lane.first().map_or_else(|| SELF_HOSTED_HINT_GENERIC.to_string(), |h| self_hosted_hint(h)))
}

/// Whether a server's refusal reads like one about credentials (the vendor's English).
pub fn looks_like_auth_failure(message: &str) -> bool {
    const NEEDLES: [&str; 10] =
        ["status 401", "status 403", "unauthorized", "unauthenticated", "forbidden", "authentication", "api key", "api-key", "access key", "access token"];
    let haystack = message.to_lowercase();
    NEEDLES.iter().any(|n| haystack.contains(n))
}

/// `message` with the advice after a blank line when the message or the transport's status
/// text reads like an auth failure the advice explains; the advice alone for an empty message.
pub fn amended(message: &str, status_text: &str, url: &str, api: Api, custom: &[(String, String)]) -> String {
    if !(looks_like_auth_failure(message) || looks_like_auth_failure(status_text)) {
        return message.to_string();
    }
    match withheld_key_hint(url, api, custom) {
        None => message.to_string(),
        Some(hint) if message.is_empty() => hint,
        Some(hint) => format!("{message}\n\n{hint}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect()
    }

    #[test]
    fn hosts_parse_like_foundation() {
        assert_eq!(host_of("http://[::1]:8080/x").as_deref(), Some("::1"));
        assert_eq!(host_of("https://API.OpenAI.com./v1").as_deref(), Some("api.openai.com"));
        assert_eq!(host_of("http://user:pw@192.168.0.2:1234/").as_deref(), Some("192.168.0.2"));
        assert_eq!(
            host_of("https://generativelanguage.googleapis.com/v1beta/models/{{MODEL}}").as_deref(),
            Some("generativelanguage.googleapis.com")
        );
        assert_eq!(host_of("http://h?x=1").as_deref(), Some("h"));
        assert_eq!(host_of("localhost:11434"), None);
        assert_eq!(host_of(""), None);
        assert_eq!(host_of("https:///path"), None);
        // Foundation's URL(string:) rejects these, so they name no host (the gate's NoEndpoint).
        assert_eq!(host_of("http://evil.example\\@localhost/v1/chat/completions"), None);
        assert_eq!(host_of("http://evil.example\\localhost/"), None);
        assert_eq!(host_of("http://evil example@localhost/"), None);
        assert_eq!(host_of("http://local host/"), None);
        assert_eq!(host_of("http://localhost\t/"), None);
        assert_eq!(host_of("http://localhost\n.evil.example/"), None);
        assert_eq!(host_of("http://evil\u{7f}@localhost/"), None);
        assert_eq!(host_of("http://evil.example%40@localhost/"), None);
        assert_eq!(host_of("http://%6cocalhost/"), None);
        // Only the authority is checked: a path, query or fragment may hold them.
        assert_eq!(host_of("http://localhost:11434/a%20b\\c d").as_deref(), Some("localhost"));
        assert_eq!(host_of("http://h?q=a b%20").as_deref(), Some("h"));

        assert!(host_is_openai("https://api.openai.com/v1/responses"));
        assert!(host_is_openai("https://API.OPENAI.COM/"));
        assert!(!host_is_openai("https://api.openai.com.evil.io/"));
        assert!(host_is_azure("https://x.openai.azure.com/o"));
        assert!(host_is_deepseek("https://api.deepseek.com/chat"));
        assert!(!host_is_deepseek("https://deepseek.com/"));
        assert!(host_is_anthropic("https://api.anthropic.com/v1/messages"));
        assert!(host_is_google("https://generativelanguage.googleapis.com/v1beta"));
    }

    #[test]
    fn private_hosts_follow_iterm2() {
        for host in [
            "localhost", "LOCALHOST.", "osaurus.local.", "127.0.0.1", "127.9.9.9", "10.1.2.3", "172.16.0.1", "172.31.255.255",
            "192.168.1.1", "169.254.1.1", "::1", "fe80::1", "febf::1", "fc00::1", "fd12:3456::1",
        ] {
            assert!(is_local_or_private(host), "{host} is private");
        }
        for host in ["172.32.0.1", "172.15.0.1", "8.8.8.8", "api.openai.com", "2001:db8::1", "fec0::1", "local", ""] {
            assert!(!is_local_or_private(host), "{host} is public");
        }
    }

    #[test]
    fn key_policy_follows_iterm2() {
        assert_eq!(key_policy("", Api::AppleIntelligence), KeyPolicy::OnDevice);
        assert_eq!(key_policy("http://localhost:11434/api/chat", Api::Llama), KeyPolicy::LocalEndpoint);
        assert_eq!(key_policy("http://192.168.1.5:1234/v1/chat/completions", Api::ChatCompletions), KeyPolicy::LocalEndpoint);
        assert_eq!(key_policy("https://api.openai.com/v1/responses", Api::Responses), KeyPolicy::VendorKey);
        assert_eq!(key_policy("localhost:1234", Api::ChatCompletions), KeyPolicy::VendorKey);
        assert_eq!(SELF_HOSTED_PLACEHOLDER, "Placeholder for self-hosted");
    }

    #[test]
    fn credential_headers_follow_iterm2() {
        let openai = "https://api.openai.com/v1/responses";
        let azure = "https://x.openai.azure.com/openai/deployments/d/chat/completions";
        let anthropic = "https://api.anthropic.com/v1/messages";
        let gemini = "https://generativelanguage.googleapis.com/v1beta";
        let llama = "http://localhost:11434/api/chat";
        assert_eq!(auth_headers(openai, Api::Responses, " k "), h(&[("Authorization", "Bearer k")]));
        assert_eq!(auth_headers(azure, Api::ChatCompletions, "k"), h(&[("api-key", "k")]));
        assert_eq!(auth_headers(anthropic, Api::Anthropic, "k\n"), h(&[("x-api-key", "k"), ("anthropic-version", "2023-06-01")]));
        assert_eq!(auth_headers(gemini, Api::Gemini, "k"), h(&[]));
        assert_eq!(auth_headers(llama, Api::Llama, "k"), h(&[]));
        assert_eq!(auth_headers("", Api::AppleIntelligence, "k"), h(&[]));
        assert_eq!(credential_header_names(openai, Api::Responses), ["Authorization"]);
        assert_eq!(credential_header_names(azure, Api::ChatCompletions), ["api-key"]);
        assert_eq!(credential_header_names(anthropic, Api::Anthropic), ["x-api-key"]);
        assert!(credential_header_names(gemini, Api::Gemini).is_empty());
        assert!(credential_header_names(llama, Api::Llama).is_empty());
        assert!(credential_header_names("", Api::AppleIntelligence).is_empty());
        for api in [Api::DeepSeek, Api::Completions, Api::EarlyO1] {
            assert_eq!(auth_headers(openai, api, "k"), auth_headers(openai, Api::ChatCompletions, "k"));
            assert_eq!(auth_headers(azure, api, "k"), auth_headers(azure, Api::ChatCompletions, "k"));
            assert_eq!(credential_header_names(openai, api), ["Authorization"]);
            assert_eq!(credential_header_names(azure, api), ["api-key"]);
        }
        assert_eq!(COMMON_CREDENTIAL_HEADERS, ["Authorization", "Proxy-Authorization", "X-Api-Key", "Api-Key", "X-Auth-Token", "X-Access-Token"]);
    }

    #[test]
    fn custom_headers_follow_iterm2() {
        assert!(header_name_valid("X-Api_Key.1~"));
        assert!(!header_name_valid(""));
        assert!(!header_name_valid("Bad Name"));
        assert!(!header_name_valid("a:b"));
        assert!(!header_name_valid("caf\u{e9}"));
        assert!(header_value_valid("a b"));
        assert!(!header_value_valid("a\rb"));
        assert!(!header_value_valid("a\nb"));
        assert!(!header_value_valid("a\0b"));
        assert!(header_value_valid(""));
        let base = h(&[("Authorization", "Bearer k"), ("Content-Type", "application/json")]);
        let custom = h(&[("authorization", "Token t"), ("Bad Name", "x"), ("X-V", "a\nb"), ("X-A", "1")]);
        assert_eq!(
            merged_headers(&base, &custom),
            h(&[("Content-Type", "application/json"), ("authorization", "Token t"), ("X-A", "1")])
        );
        assert_eq!(merged_headers(&base, &[]), base);
    }

    #[test]
    fn withheld_key_advice_follows_iterm2() {
        let local = "http://localhost:1234/v1/chat/completions";
        let named = "Tandem did not send an API key with this request. If this endpoint requires authentication, add the header \u{201c}Authorization\u{201d} under \u{201c}Custom headers\u{201d}.";
        let generic = "Tandem did not send an API key with this request. If this endpoint requires authentication, add the header it expects under \u{201c}Custom headers\u{201d}.";
        assert_eq!(withheld_key_hint(local, Api::ChatCompletions, &[]).as_deref(), Some(named));
        assert_eq!(self_hosted_hint("Authorization"), named);
        assert_eq!(SELF_HOSTED_HINT_GENERIC, generic);
        assert_eq!(withheld_key_hint(local, Api::ChatCompletions, &h(&[("x-api-key", "k")])), None);
        assert_eq!(withheld_key_hint(local, Api::ChatCompletions, &h(&[("X-Request-Id", "1")])).as_deref(), Some(named));
        assert_eq!(withheld_key_hint(local, Api::ChatCompletions, &h(&[("Authorization", "a\nb")])).as_deref(), Some(named));
        assert_eq!(withheld_key_hint(local, Api::ChatCompletions, &h(&[("Proxy-Authorization", "")])), None);
        assert_eq!(withheld_key_hint("http://localhost:11434/api/chat", Api::Llama, &[]).as_deref(), Some(generic));
        assert_eq!(
            withheld_key_hint("http://10.0.0.2/v1/messages", Api::Anthropic, &[]).as_deref(),
            Some(self_hosted_hint("x-api-key").as_str())
        );
        assert_eq!(withheld_key_hint("https://api.openai.com/v1/responses", Api::Responses, &[]), None);
        assert_eq!(withheld_key_hint("", Api::AppleIntelligence, &[]), None);

        assert!(looks_like_auth_failure("HTTP status 401"));
        assert!(looks_like_auth_failure("Invalid API Key"));
        assert!(looks_like_auth_failure("FORBIDDEN"));
        assert!(!looks_like_auth_failure("Connection refused"));

        assert_eq!(amended("Unauthorized", "", local, Api::ChatCompletions, &[]), format!("Unauthorized\n\n{named}"));
        assert_eq!(amended("", "status 403", local, Api::ChatCompletions, &[]), named);
        assert_eq!(amended("model not found", "status 404", local, Api::ChatCompletions, &[]), "model not found");
    }
}
