//! A script's status bar component (`iTermStatusBarRPCProvidedTextComponent.m`, `RPC`): its
//! saved configuration (the registration request in base64 plus the knob values), its
//! knobs, the invocation that computes its text (`knobs:__knobs` added to the registration's
//! defaults), the variants a result gives, iTerm2's two error texts, the click invocation,
//! the update cadence, unread counts and StatusBarComponentRequest's statuses (`H:4101-4140`).
//! The live registrations (`latestStatusBarRequest`, `RPC:54-57`) are kept here by the app,
//! so the status bar's class functions (`statusbar::knobs`, `info_for`, ...) find a script's
//! newest registration of a unique id.

use std::collections::BTreeSet;
use std::sync::RwLock;
use std::time::Duration;

use base64::Engine;
use prost::Message;
use serde_json::Value;

use crate::api_proto::pb;
use crate::statusbar::{self, Knob, KnobKind};
use pb::rpc_registration_request::status_bar_component_attributes::{knob::Type as KnobType, Format};
use pb::rpc_registration_request::{RoleSpecificAttributes, RpcArgument, StatusBarComponentAttributes};

/// The plain text script component's class (`RPC:613`).
pub const TEXT_CLASS: &str = "iTermStatusBarRPCProvidedTextComponent";
/// The HTML script component's class (`RPC:737`).
pub const HTML_CLASS: &str = "iTermStatusBarRPCProvidedAttributedTextComponent";
/// The configuration key of the registration's bytes in base64 (`RPC:35`).
pub const REQUEST_KEY: &str = "registration request v2";
/// The frame variable holding the knob values' JSON (`RPC:236`, `RPC:370`).
pub const KNOBS_PATH: &str = "__knobs";
/// The heading of a failed component's alert (`RPC:552`; assigned last of the two).
pub const ERROR_HEADING: &str = "Status Bar Script Error";

/// Appended to a failed component's text when its script could fix it by launching
/// (`RPC:548`).
pub const NOT_RUNNING_SUFFIX: &str = "It looks like the script is not running. Launching it might fix the problem.";
/// The failed component alert's recovery buttons (`RPC:540-545`; "Reveal in Finder").
pub const LAUNCH_SCRIPT: &str = "Launch Script";
pub const REVEAL_IN_EXPLORER: &str = "Reveal in Explorer";

/// The offer after Launch Script fixed a component (`RPC:404-409`).
pub const AUTOLAUNCH_HEADING: &str = "Always launch this script when Termurion starts?";
pub const AUTOLAUNCH_TEXT: &str = "This will move the script into the AutoLaunch folder.";

/// The silence key of that offer, one per script.
pub fn autolaunch_id(full_path: &str) -> String {
    format!("NoSyncAutoLaunchScript_{full_path}")
}

/// The setting that remembers which script registered a component (its full path), written
/// at registration (`H:1880-1896`, `H:1385-1387`).
pub fn script_path_key(unique_id: &str) -> String {
    format!("NoSyncScriptNameForStatusBarComponent_{unique_id}")
}

/// The path to store under `script_path_key` when a component registers (`H:1880-1896`):
/// the registering script's full path, when there is one and it differs from what is
/// stored (the settings file is written only on a change).
pub fn path_to_store(full: Option<&std::path::Path>, stored: Option<&str>) -> Option<String> {
    let text = full?.to_string_lossy().into_owned();
    (stored != Some(text.as_str())).then_some(text)
}

/// A failed component's click (`RPC:533-555`): when its script is known, not running and
/// could launch, the text gains `NOT_RUNNING_SUFFIX` and the alert Launch Script and Reveal in
/// Explorer; else the text alone with OK.
pub fn recovery(error: &str, path_known: bool, running: bool, could_launch: bool) -> (String, Vec<&'static str>) {
    if path_known && !running && could_launch {
        (format!("{error}{NOT_RUNNING_SUFFIX}"), vec![crate::api_auth::OK, LAUNCH_SCRIPT, REVEAL_IN_EXPLORER])
    } else {
        (error.to_string(), vec![crate::api_auth::OK])
    }
}

/// A registration's status bar attributes (none for another role).
fn attributes(r: &pb::RpcRegistrationRequest) -> Option<&StatusBarComponentAttributes> {
    match r.role_specific_attributes.as_ref()? {
        RoleSpecificAttributes::StatusBarComponentAttributes(a) => Some(a),
        _ => None,
    }
}

/// The class a registration's components have: by its format (`RPC:118-129`).
pub fn class_of(r: &pb::RpcRegistrationRequest) -> &'static str {
    let html = attributes(r).and_then(|a| a.format).and_then(|f| Format::try_from(f).ok()) == Some(Format::Html);
    if html { HTML_CLASS } else { TEXT_CLASS }
}

/// A new component's configuration (`statusBarConfiguration`, `RPC:48-51`, with the
/// factory's default knobs, `RPC:104-113`): the registration in base64 and its knobs.
pub fn config_for(r: &pb::RpcRegistrationRequest) -> toml::Table {
    let mut t = toml::Table::new();
    t.insert(REQUEST_KEY.into(), toml::Value::String(base64::engine::general_purpose::STANDARD.encode(r.encode_to_vec())));
    t.insert("knobs".into(), toml::Value::Table(default_knobs(r)));
    t
}

/// The registration a configuration saved (`RPC:165-190`); None without one, or when it is
/// not base64 of a registration.
pub fn request_of(config: &toml::Table) -> Option<pb::RpcRegistrationRequest> {
    let b64 = config.get(REQUEST_KEY)?.as_str()?;
    let data = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    pb::RpcRegistrationRequest::decode(data.as_slice()).ok()
}

/// Whether a registration is a status bar component's (role STATUS_BAR_COMPONENT, `H:1944`).
pub fn registration_is_component(r: &pb::RpcRegistrationRequest) -> bool {
    r.role.and_then(|v| pb::rpc_registration_request::Role::try_from(v).ok()) == Some(pb::rpc_registration_request::Role::StatusBarComponent)
}

/// The registration's unique identifier.
pub fn unique_id(r: &pb::RpcRegistrationRequest) -> Option<&str> {
    attributes(r)?.unique_identifier.as_deref()
}

/// A knob descriptor's type as a knob kind (`RPC:306-318`): an unknown one is text.
fn knob_kind(t: Option<i32>) -> KnobKind {
    match t.and_then(|t| KnobType::try_from(t).ok()) {
        Some(KnobType::Checkbox) => KnobKind::Checkbox,
        Some(KnobType::PositiveFloatingPoint) => KnobKind::Double,
        Some(KnobType::Color) => KnobKind::Color,
        Some(KnobType::String) | None => KnobKind::Text,
    }
}

/// A JSON value as a knob value; None for null (a TOML table holds no null).
fn toml_of(v: &Value) -> Option<toml::Value> {
    Some(match v {
        Value::Null => return None,
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => toml::Value::Integer(i),
            None => toml::Value::Float(n.as_f64()?),
        },
        Value::String(s) => toml::Value::String(s.clone()),
        Value::Array(a) => toml::Value::Array(a.iter().filter_map(toml_of).collect()),
        Value::Object(o) => toml::Value::Table(o.iter().filter_map(|(k, v)| toml_of(v).map(|v| (k.clone(), v))).collect()),
    })
}

/// A knob value as JSON.
fn json_of(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::from(*i),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(a) => Value::Array(a.iter().map(json_of).collect()),
        toml::Value::Table(t) => Value::Object(t.iter().map(|(k, v)| (k.clone(), json_of(v))).collect()),
    }
}

/// A descriptor's default value (`it_objectForJsonString:`): None without one or when it
/// is not JSON.
fn default_of(json: Option<&str>) -> Option<toml::Value> {
    toml_of(&serde_json::from_str::<Value>(json?).ok()?)
}

/// The component's knobs (`RPC:320-330`): the registration's, each by its type with its
/// name, placeholder, JSON default and key, then the class's own knobs (`tail`).
pub fn knobs(r: &pb::RpcRegistrationRequest, tail: Vec<Knob>) -> Vec<Knob> {
    let mut v: Vec<Knob> = attributes(r)
        .map(|a| a.knobs.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|k| Knob {
            label: k.name.clone().unwrap_or_default(),
            kind: knob_kind(k.r#type),
            key: k.key.clone().unwrap_or_default(),
            placeholder: k.placeholder.clone(),
            default: default_of(k.json_default_value.as_deref()),
        })
        .collect();
    v.extend(tail);
    v
}

/// The factory's default knobs (`RPC:104-113`): each knob's JSON default by its key.
pub fn default_knobs(r: &pb::RpcRegistrationRequest) -> toml::Table {
    attributes(r)
        .map(|a| a.knobs.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|k| Some((k.key.clone().unwrap_or_default(), default_of(k.json_default_value.as_deref())?)))
        .collect()
}

/// The invocation that computes the text (`RPC:232-239`): the registration's defaults plus
/// `knobs:__knobs`, as a provider's invocation. None when the registration's unique id, name
/// or defaults are anything but what they claim (`script_fn::invocation_parts_ok`): a saved
/// registration comes from profile data, which must never be able to name a call.
pub fn invocation(r: &pb::RpcRegistrationRequest) -> Option<String> {
    if !crate::script_fn::invocation_parts_ok(r) {
        return None;
    }
    let mut defaults: Vec<(String, String)> = r.defaults.iter().map(|d: &RpcArgument| (d.name.clone().unwrap_or_default(), d.path.clone().unwrap_or_default())).collect();
    defaults.push(("knobs".into(), KNOBS_PATH.into()));
    Some(crate::script_fn::invocation(&crate::script_fn::fq_name(r), &defaults))
}

/// `__knobs`: the knob values' JSON (`RPC:368-370`).
pub fn knobs_json(knobs: &toml::Table) -> String {
    crate::expr::json_text(&json_of(&toml::Value::Table(knobs.clone())))
}

/// The variants a result gives (`RPC:413-434`): a string, or a list of strings; anything
/// else is iTerm2's invalid return value text.
pub fn variants(invocation: &str, v: &Value) -> Result<Vec<String>, String> {
    match v {
        Value::String(s) => Ok(vec![s.clone()]),
        Value::Array(a) if a.iter().all(Value::is_string) => Ok(a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
        other => Err(format!(
            "Return value from {invocation} invalid.\n\nIt should have returned a string or a list of strings.\n\nInstead, it returned:\n\n{}",
            crate::script_eval::description_text(other)
        )),
    }
}

/// A failed evaluation's text (`RPC:436-449`): the short description, the unique id, the
/// invocation, the reason, and the traceback after two newlines when there is one.
pub fn failed_text(r: &pb::RpcRegistrationRequest, invocation: &str, reason: &str, detail: Option<&str>) -> String {
    let a = attributes(r);
    let short = a.and_then(|a| a.short_description.as_deref()).unwrap_or_default();
    let id = a.and_then(|a| a.unique_identifier.as_deref()).unwrap_or_default();
    let tail = detail.map(|d| format!("\n\n{d}")).unwrap_or_default();
    format!("Status bar component \u{201c}{short}\u{201d} ({id}) failed.\n\nThis function call had an error:\n\n{invocation}\n\nThe error was:\n\n{reason}\n\n{tail}")
}

/// A click's invocation (`RPC:557-559`): the unique id with `.` and `-` as `_`; none when
/// the id is not one (`script_fn::unique_id_ok`).
pub fn click_invocation(unique_id: &str, session_id: &str) -> Option<String> {
    if !crate::script_fn::unique_id_ok(unique_id) {
        return None;
    }
    let id = unique_id.replace(['.', '-'], "_");
    Some(format!("__{id}__on_click(session_id: \"{session_id}\")"))
}

/// A failed click, for the console (`RPC:567`).
pub fn onclick_error(reason: &str, detail: &str) -> String {
    format!("Error in onclick handler: {reason}\n{detail}")
}

/// How often the component evaluates again (`RPC:475-481`): its update cadence, else never.
pub fn cadence(r: &pb::RpcRegistrationRequest) -> Option<Duration> {
    let c = attributes(r)?.update_cadence?;
    (c.is_finite() && c > 0.0).then(|| Duration::from_secs_f64(f64::from(c)))
}

/// An unread count (`iTermStatusBarUnreadCountController.m:51-60`): the session's when set,
/// else the component's global one, else 0.
pub fn unread(global: Option<i64>, session: Option<i64>) -> i64 {
    session.or(global).unwrap_or(0)
}

/// A component's identifier (`statusBarComponentIdentifier`): a script component's unique id
/// (the saved registration's, `RPC:241-244`), a built-in's `com.iterm2.<class>`
/// (`BASE:133-139`). Unread counts and StatusBarComponentRequest find components by it.
pub fn identifier(class: &str, config: &toml::Table) -> String {
    if class == TEXT_CLASS || class == HTML_CLASS {
        request_of(config).and_then(|r| unique_id(&r).map(str::to_string)).unwrap_or_default()
    } else {
        format!("com.iterm2.{class}")
    }
}

/// Whether a component answers to an identifier (`componentWithIdentifier:`, `VC:310-314`):
/// no identifier matches nothing (iTerm2 compares with nil), so a script component whose
/// configuration holds no registration (identifier "", never made, `RPC:172-183`) matches
/// no request.
pub fn has_identifier(class: &str, config: &toml::Table, id: &str) -> bool {
    !id.is_empty() && identifier(class, config) == id
}

/// An unread count as the badge writes it (`NSNumberFormatterDecimalStyle`): thousands
/// grouped with commas.
pub fn count_text(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// StatusBarComponentRequest's status (`H:4101-4140`): no request REQUEST_MALFORMED; no such
/// session SESSION_NOT_FOUND; no component with the identifier in that session's status bar
/// INVALID_IDENTIFIER; else OK.
pub fn popover_status(r: &pb::StatusBarComponentRequest, session: bool, component: bool) -> pb::status_bar_component_response::Status {
    use pb::status_bar_component_response::Status;
    if r.request.is_none() {
        Status::RequestMalformed
    } else if !session {
        Status::SessionNotFound
    } else if !component {
        Status::InvalidIdentifier
    } else {
        Status::Ok
    }
}

/// An element whose text is not shown.
const HIDDEN: [&str; 4] = ["head", "script", "style", "title"];
/// Elements that end a line.
const BREAKS: [&str; 9] = ["br", "p", "div", "li", "tr", "h1", "h2", "h3", "h4"];

/// An entity's character: the named ones HTML text commonly uses and numeric ones.
fn entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        _ => {
            let n = name.strip_prefix('#')?;
            let code = match n.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => n.parse().ok()?,
            };
            char::from_u32(code)?
        }
    })
}

/// The text of HTML (Decision: text, not a web view): tags dropped (a line break for `br`
/// and the end of a block), the head, scripts and styles left out, runs of white space one
/// space as HTML lays them out, entities decoded.
pub fn html_text(html: &str) -> String {
    let mut out = String::new();
    let mut hidden: Option<String> = None;
    let mut space = false;
    let mut rest = html;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('<') {
            let end = after.find('>').map_or(after.len(), |i| i + 1);
            let tag = &after[..end.saturating_sub(1).min(after.len())];
            rest = &after[end.min(after.len())..];
            let closing = tag.starts_with('/');
            let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
            if let Some(h) = &hidden {
                if closing && *h == name {
                    hidden = None;
                }
                continue;
            }
            if !closing && !tag.ends_with('/') && HIDDEN.contains(&name.as_str()) {
                hidden = Some(name);
                continue;
            }
            if BREAKS.contains(&name.as_str()) && (name == "br" || closing) {
                out.push('\n');
                space = false;
            }
            continue;
        }
        let c = rest.chars().next().unwrap_or_default();
        rest = &rest[c.len_utf8()..];
        if hidden.is_some() {
            continue;
        }
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() && !out.ends_with('\n') {
            out.push(' ');
        }
        space = false;
        if c == '&' {
            // An entity's name is short: only its first characters are searched, so a long
            // run of `&` stays linear.
            if let Some(semi) = rest.char_indices().take(11).find(|&(_, ch)| ch == ';').map(|(i, _)| i).filter(|&i| i <= 10) {
                if let Some(ch) = entity(&rest[..semi]) {
                    out.push(ch);
                    rest = &rest[semi + 1..];
                    continue;
                }
            }
        }
        out.push(c);
    }
    out.trim_end_matches('\n').to_string()
}

/// A copy of the app registry's live STATUS_BAR_COMPONENT registrations, each with its class
/// and a new component's configuration (the setup window's palette entry): the pure
/// `statusbar` functions and the setup window cannot reach the App. Its only writer is
/// `App::script_registry_changed`, which every registry change calls.
static LIVE: RwLock<Vec<(pb::RpcRegistrationRequest, &'static str, toml::Table)>> = RwLock::new(Vec::new());

/// The app's registrations changed: the live status bar components now.
pub fn set_live(regs: Vec<pb::RpcRegistrationRequest>) {
    let entries = regs.into_iter().map(|r| (class_of(&r), config_for(&r), r)).map(|(class, config, r)| (r, class, config)).collect();
    if let Ok(mut live) = LIVE.write() {
        *live = entries;
    }
}

/// The live status bar component registrations (the tests read them back).
#[cfg(test)]
pub fn live() -> Vec<pb::RpcRegistrationRequest> {
    LIVE.read().map(|l| l.iter().map(|(r, _, _)| r.clone()).collect()).unwrap_or_default()
}

/// The setup window's palette entries for the live registrations: class and configuration.
pub fn live_palette() -> Vec<(&'static str, toml::Table)> {
    LIVE.read().map(|l| l.iter().map(|(_, class, config)| (*class, config.clone())).collect()).unwrap_or_default()
}

/// `latestStatusBarRequest` (`RPC:54-57`): the live registration with the same unique id,
/// else the saved one.
pub fn latest(r: &pb::RpcRegistrationRequest) -> pb::RpcRegistrationRequest {
    let Some(id) = unique_id(r) else { return r.clone() };
    LIVE.read().ok().and_then(|l| l.iter().map(|(x, _, _)| x).find(|x| unique_id(x) == Some(id)).cloned()).unwrap_or_else(|| r.clone())
}

/// The registration a component of a script class uses: the saved one's latest; None for
/// another class or a configuration without one.
pub fn registration(class: &str, config: &toml::Table) -> Option<pb::RpcRegistrationRequest> {
    (class == TEXT_CLASS || class == HTML_CLASS).then(|| request_of(config)).flatten().map(|r| latest(&r))
}

/// The short description, detailed description and exemplar the setup window shows
/// (`RPC:259-265`, `RPC:332-335`; no exemplar is "").
pub fn descriptions(r: &pb::RpcRegistrationRequest) -> (String, String, String) {
    let a = attributes(r);
    let text = |f: fn(&StatusBarComponentAttributes) -> &Option<String>| a.and_then(|a| f(a).clone()).unwrap_or_default();
    (text(|a| &a.short_description), text(|a| &a.detailed_description), text(|a| &a.exemplar))
}

/// The knobs a script class's own component kind adds after the registration's
/// (`RPC:674-676`, `RPC:798-800`): a text component's suite, or an attributed text
/// component's (Background Color and Text Color, then the base knobs, `ATXT:148-163`).
pub fn class_knobs(class: &str, algorithm: statusbar::Algorithm) -> Vec<Knob> {
    if class == HTML_CLASS {
        let mut v = vec![statusbar::color_knob("Background Color:", statusbar::SHARED_BACKGROUND_COLOR_KEY), statusbar::color_knob("Text Color:", statusbar::SHARED_TEXT_COLOR_KEY), statusbar::stretch_knob(algorithm), statusbar::priority_knob()];
        v.extend(statusbar::min_max_width_knobs());
        v
    } else {
        statusbar::text_component_knobs(algorithm)
    }
}

/// The paths an invocation's calls name (its dependencies, `RPC:201-216`).
pub fn invocation_paths(invocation: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for c in crate::expr::parse_calls(invocation).unwrap_or_default() {
        crate::script_eval::call_paths(&c, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb::rpc_registration_request::status_bar_component_attributes::Knob as PbKnob;
    use pb::rpc_registration_request::{Role, RpcArgumentSignature};
    use serde_json::json;

    fn knob(name: &str, t: KnobType, key: &str, placeholder: Option<&str>, default: Option<&str>) -> PbKnob {
        PbKnob { name: Some(name.into()), r#type: Some(t as i32), placeholder: placeholder.map(str::to_string), json_default_value: default.map(str::to_string), key: Some(key.into()) }
    }

    /// The fixture: "f(knobs, sid)", `sid` defaulting to `id`, short "Clock", detailed
    /// "Shows it", unique id "com.ex.clock-1", four knobs, exemplar "12:00", cadence 2.
    fn fixture() -> pb::RpcRegistrationRequest {
        pb::RpcRegistrationRequest {
            name: Some("f".into()),
            arguments: vec![RpcArgumentSignature { name: Some("knobs".into()) }, RpcArgumentSignature { name: Some("sid".into()) }],
            defaults: vec![RpcArgument { name: Some("sid".into()), path: Some("id".into()) }],
            role: Some(Role::StatusBarComponent as i32),
            role_specific_attributes: Some(RoleSpecificAttributes::StatusBarComponentAttributes(StatusBarComponentAttributes {
                short_description: Some("Clock".into()),
                detailed_description: Some("Shows it".into()),
                knobs: vec![
                    knob("Seconds", KnobType::Checkbox, "sec", None, Some("true")),
                    knob("Label", KnobType::String, "lbl", Some("x"), None),
                    knob("Rate", KnobType::PositiveFloatingPoint, "rate", None, None),
                    knob("Tint", KnobType::Color, "tint", None, None),
                ],
                exemplar: Some("12:00".into()),
                update_cadence: Some(2.0),
                unique_identifier: Some("com.ex.clock-1".into()),
                format: Some(Format::PlainText as i32),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    fn with_attributes(mut r: pb::RpcRegistrationRequest, f: impl FnOnce(&mut StatusBarComponentAttributes)) -> pb::RpcRegistrationRequest {
        if let Some(RoleSpecificAttributes::StatusBarComponentAttributes(a)) = r.role_specific_attributes.as_mut() {
            f(a);
        }
        r
    }

    #[test]
    fn script_components_follow_iterm2() {
        let r = fixture();
        assert_eq!(class_of(&r), TEXT_CLASS);
        assert_eq!(class_of(&with_attributes(fixture(), |a| a.format = Some(Format::Html as i32))), HTML_CLASS);

        // The saved configuration round-trips the registration; its knobs are the defaults.
        let config = config_for(&r);
        assert_eq!(request_of(&config), Some(r.clone()));
        let mut sec = toml::Table::new();
        sec.insert("sec".into(), toml::Value::Boolean(true));
        assert_eq!(config.get("knobs"), Some(&toml::Value::Table(sec.clone())));
        assert_eq!(default_knobs(&r), sec);
        assert_eq!(request_of(&toml::Table::new()), None, "no registration");
        let mut bad = toml::Table::new();
        bad.insert(REQUEST_KEY.into(), toml::Value::String("%%% not base64".into()));
        assert_eq!(request_of(&bad), None, "bad base64");
        assert_eq!(unique_id(&r), Some("com.ex.clock-1"));

        // The script's knobs by type, then the class's.
        let tail = vec![statusbar::priority_knob()];
        let k = knobs(&r, tail.clone());
        let want = vec![
            Knob { label: "Seconds".into(), kind: KnobKind::Checkbox, key: "sec".into(), placeholder: None, default: Some(toml::Value::Boolean(true)) },
            Knob { label: "Label".into(), kind: KnobKind::Text, key: "lbl".into(), placeholder: Some("x".into()), default: None },
            Knob { label: "Rate".into(), kind: KnobKind::Double, key: "rate".into(), placeholder: None, default: None },
            Knob { label: "Tint".into(), kind: KnobKind::Color, key: "tint".into(), placeholder: None, default: None },
        ];
        assert_eq!(k[..4], want[..]);
        assert_eq!(k[4..], tail[..]);

        assert_eq!(invocation(&r).as_deref(), Some("statusbar.com.ex.clock_1.f(knobs:__knobs,sid:id)"));
        assert_eq!(knobs_json(&sec), "{\"sec\":true}");
        assert_eq!(cadence(&r), Some(Duration::from_secs(2)));
        assert_eq!(cadence(&with_attributes(fixture(), |a| a.update_cadence = None)), None);
        assert_eq!(click_invocation("com.ex.clock-1", "7").as_deref(), Some("__com_ex_clock_1__on_click(session_id: \"7\")"));
        assert_eq!(unread(Some(3), None), 3);
        assert_eq!(unread(Some(3), Some(1)), 1);
        assert_eq!(unread(None, None), 0);
        assert_eq!(invocation_paths(&invocation(&r).unwrap()), BTreeSet::from(["__knobs".to_string(), "id".to_string()]));
        assert_eq!(identifier(TEXT_CLASS, &config), "com.ex.clock-1");
        assert_eq!(identifier("iTermStatusBarClockComponent", &toml::Table::new()), "com.iterm2.iTermStatusBarClockComponent");
        assert_eq!((count_text(7), count_text(1234), count_text(1234567)), ("7".to_string(), "1,234".to_string(), "1,234,567".to_string()));
        assert_eq!(descriptions(&r), ("Clock".to_string(), "Shows it".to_string(), "12:00".to_string()));

        // The status bar's class functions read a component's registration from its
        // configuration, preferring the live registration of the same unique id.
        let algorithm = statusbar::Algorithm::Stable;
        let info = statusbar::info_for(TEXT_CLASS, &config, algorithm).unwrap();
        assert_eq!((info.class, &*info.short, &*info.detail, &*info.exemplar, info.stretch), (TEXT_CLASS, "Clock", "Shows it", "12:00", true));
        assert_eq!(statusbar::info_for(TEXT_CLASS, &toml::Table::new(), algorithm), None);
        assert_eq!(statusbar::knobs(TEXT_CLASS, &config, algorithm), knobs(&r, statusbar::text_component_knobs(algorithm)));
        assert_eq!(statusbar::default_knobs(TEXT_CLASS, &config), sec);
        assert_eq!(statusbar::exemplar(TEXT_CLASS, &config, &sec, "", "", "", algorithm), "12:00");
        let html = config_for(&with_attributes(fixture(), |a| a.format = Some(Format::Html as i32)));
        let tail: Vec<String> = statusbar::knobs(HTML_CLASS, &html, algorithm)[4..].iter().map(|k| k.label.clone()).collect();
        assert_eq!(tail, ["Background Color:", "Text Color:", "Size Multiple:", "Priority:", "Minimum Width:", "Maximum Width:"]);
        let mut layout = statusbar::Layout::default();
        statusbar::insert_component(&mut layout, 0, TEXT_CLASS, config.clone());
        assert_eq!((layout.components[0].knobs.clone(), request_of(&layout.components[0].config)), (sec.clone(), Some(r.clone())));
        // A live registration of the same unique id wins (another test's ids never match).
        let newer = with_attributes(fixture(), |a| {
            a.short_description = Some("Clock 2".into());
            a.unique_identifier = Some("com.ex.live-test".into());
        });
        let older = with_attributes(fixture(), |a| a.unique_identifier = Some("com.ex.live-test".into()));
        assert_eq!(latest(&older), older);
        let mut others = live();
        others.retain(|x| unique_id(x) != Some("com.ex.live-test"));
        let before = others.clone();
        others.push(newer.clone());
        set_live(others);
        assert_eq!(latest(&older), newer);
        assert_eq!(&*statusbar::info_for(TEXT_CLASS, &config_for(&older), algorithm).unwrap().short, "Clock 2");
        assert!(live_palette().contains(&(TEXT_CLASS, config_for(&newer))), "the palette entry");
        // The test leaves the process-wide list as it found it.
        set_live(before);
        assert_eq!(latest(&older), older);
    }

    #[test]
    fn saved_components_never_name_a_call() {
        // Profile data (a Dynamic Profiles import) decides a saved registration's unique id and
        // default paths: none of these may reach an invocation, so nothing runs.
        let paste = "iterm2.paste(session_id: id, text: \"x\")";
        let by_id = with_attributes(fixture(), |a| a.unique_identifier = Some(format!("x.f(a:{paste});y")));
        assert_eq!(invocation(&by_id), None);
        let mut by_path = fixture();
        by_path.defaults[0].path = Some(format!("id, x: {paste}"));
        assert_eq!(invocation(&by_path), None);
        let mut by_name = fixture();
        by_name.defaults[0].name = Some(format!("x: {paste}, sid"));
        assert_eq!(invocation(&by_name), None);
        assert_eq!(click_invocation(paste, "7"), None);
        assert_eq!(click_invocation("a b", "7"), None);
        assert_eq!(click_invocation("", "7"), None);
        // What a real registration carries still builds.
        let mut optional = fixture();
        optional.defaults[0].path = Some("session.user_x?".into());
        assert_eq!(invocation(&optional).as_deref(), Some("statusbar.com.ex.clock_1.f(knobs:__knobs,sid:session.user_x?)"));
        for bad in ["", "?", "1a", "a..b", "a.", ".a", "a b", "a(b)", "a?b"] {
            assert!(!crate::script_fn::path_ok(bad), "{bad}");
        }
        for good in ["id", "session.name", "user.x?", "_a.b_2"] {
            assert!(crate::script_fn::path_ok(good), "{good}");
        }
    }

    #[test]
    fn script_component_texts_follow_iterm2() {
        let r = fixture();
        let inv = invocation(&r).unwrap();
        assert_eq!(variants(&inv, &json!("a")), Ok(vec!["a".to_string()]));
        assert_eq!(variants(&inv, &json!(["a", "b"])), Ok(vec!["a".to_string(), "b".to_string()]));
        assert_eq!(variants(&inv, &json!(3)), Err(format!("Return value from {inv} invalid.\n\nIt should have returned a string or a list of strings.\n\nInstead, it returned:\n\n3")));
        assert!(variants(&inv, &json!(["a", 1])).is_err());
        assert_eq!(
            failed_text(&r, &inv, "boom", Some("tb")),
            format!("Status bar component \u{201c}Clock\u{201d} (com.ex.clock-1) failed.\n\nThis function call had an error:\n\n{inv}\n\nThe error was:\n\nboom\n\n\n\ntb")
        );
        assert!(failed_text(&r, &inv, "boom", None).ends_with("boom\n\n"));
        assert_eq!(onclick_error("boom", "tb"), "Error in onclick handler: boom\ntb");
        assert_eq!(html_text("<b>a</b> &amp; b<br>c"), "a & b\nc");
        assert_eq!(html_text("<html><head><style>p{}</style></head><body><p>one</p>\n  <p>two&#33;</p></body></html>"), "one\ntwo!");
        // Review finding 5: a 1 MB run of `&` (a `;` only at its end) or of `<` stays linear.
        let amps = format!("{};", "&".repeat(1 << 20));
        let t = std::time::Instant::now();
        assert_eq!(html_text(&amps), amps);
        assert_eq!(html_text(&"<".repeat(1 << 20)), "");
        assert_eq!(html_text(&"<a".repeat(1 << 19)), "");
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
        assert_eq!(html_text("&#x41;&amp;&unknown;&"), "A&&unknown;&");
        assert_eq!(ERROR_HEADING, "Status Bar Script Error");
    }

    #[test]
    fn status_bar_recovery_follows_iterm2() {
        assert_eq!(
            recovery("boom\n\n", true, false, true),
            ("boom\n\nIt looks like the script is not running. Launching it might fix the problem.".to_string(), vec!["OK", "Launch Script", "Reveal in Explorer"])
        );
        assert_eq!(recovery("boom\n\n", true, true, true), ("boom\n\n".to_string(), vec!["OK"]));
        assert_eq!(recovery("boom\n\n", false, false, true), ("boom\n\n".to_string(), vec!["OK"]));
        assert_eq!(recovery("boom\n\n", true, false, false), ("boom\n\n".to_string(), vec!["OK"]));
        assert_eq!(script_path_key("com.x"), "NoSyncScriptNameForStatusBarComponent_com.x");
        let p = std::path::Path::new(r"C:\S\tool.py");
        assert_eq!(path_to_store(Some(p), None), Some(r"C:\S\tool.py".to_string()));
        assert_eq!(path_to_store(Some(p), Some(r"C:\S\old.py")), Some(r"C:\S\tool.py".to_string()));
        assert_eq!(path_to_store(Some(p), Some(r"C:\S\tool.py")), None, "unchanged: not written again");
        assert_eq!(path_to_store(None, Some("x")), None, "a connection with no script stores nothing");
        assert_eq!((LAUNCH_SCRIPT, REVEAL_IN_EXPLORER), ("Launch Script", "Reveal in Explorer"));
    }

    #[test]
    fn popover_requests_follow_iterm2() {
        use pb::status_bar_component_request::{OpenPopover, Request};
        use pb::status_bar_component_response::Status;
        let none = pb::StatusBarComponentRequest { request: None, identifier: Some("com.ex.clock-1".into()) };
        assert_eq!(popover_status(&none, true, true), Status::RequestMalformed);
        let open = pb::StatusBarComponentRequest { request: Some(Request::OpenPopover(OpenPopover { session_id: Some("7".into()), html: Some("<b>x</b>".into()), size: None })), identifier: Some("com.ex.clock-1".into()) };
        assert_eq!(popover_status(&open, false, false), Status::SessionNotFound);
        assert_eq!(popover_status(&open, true, false), Status::InvalidIdentifier);
        assert_eq!(popover_status(&open, true, true), Status::Ok);
        // Review finding 6: no identifier matches no component, not even one whose
        // configuration holds no registration.
        let broken = toml::Table::new();
        assert_eq!(identifier(TEXT_CLASS, &broken), "");
        assert!(!has_identifier(TEXT_CLASS, &broken, ""));
        assert!(has_identifier(TEXT_CLASS, &config_for(&fixture()), "com.ex.clock-1"));
        assert!(has_identifier("iTermStatusBarClockComponent", &broken, "com.iterm2.iTermStatusBarClockComponent"));
    }
}
