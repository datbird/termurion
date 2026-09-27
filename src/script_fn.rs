//! Functions scripts register and iTerm2's calls into them (`H:81-257`, `H:1139-1436`,
//! `H:1702-1716`, `H:1898-1960`, `H:2154-2214`, `H:3428-3495`): validation, namespaces and
//! signatures of RPC registrations, subscribing and unsubscribing them, resolving a call's
//! defaults from a scope, the server-originated RPC notification with its request id and
//! deadline, decoding a script's result, and cleanup when the script's connection closes.
//! Title providers, status bar components and context menu items are registrations with a
//! role.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::api_proto::pb;
use crate::expr::{self, FailKind, Failure, Value};
use pb::rpc_registration_request::{Role, RoleSpecificAttributes};

/// How long a call waits for its script when the registration names no timeout (`H:1323`).
pub const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(5);
/// Names never sent to scripts (`H:1144`).
pub const PRIVATE_PREFIX: &str = "iterm2.";

pub const TERMINATED: &str = "Script terminated during function call";
pub const TIMEOUT: &str = "Timeout";
pub const UNKNOWN_REASON: &str = "Unknown reason";
pub const MALFORMED_RESULT: &str = "Malformed result.";
pub const MALFORMED_EXCEPTION: &str = "Malformed exception";

/// No registration has the invocation's signature (`H:1262-1283`).
pub fn unregistered_text(sig: &str, similar: Option<&str>) -> String {
    let mut s = format!("No function registered for invocation \u{201c}{sig}\u{201d}. Ensure the script is running and the function name and argument names are correct.");
    if let Some(other) = similar {
        s.push_str(&format!(" There is a similarly named function available with a different signature: {other}"));
    }
    s
}

/// The Python API is off (`SFC:536-545`).
pub fn disabled_text(sig: &str) -> String {
    format!("Undefined function with signature \u{201c}{sig}\u{201d}")
}

/// An argument that has no JSON form (`H:1193`).
pub fn not_encodable(value: &str) -> String {
    format!("Could not JSON encode value \u{201c}{value}\u{201d}")
}

pub fn undecodable_value(e: &str) -> String {
    format!("Undecodable value: {e}")
}

pub fn undecodable_exception(e: &str) -> String {
    format!("Undecodable exception: {e}")
}

/// Logged for a registration that fails `validate` (`H:1710`).
pub fn malformed_signature(e: &str) -> String {
    format!("Malformed RPC function signature: {e}")
}

/// Logged for an RPC NotificationRequest without a registration (`H:1704`).
pub fn expected_registration(req: &str) -> String {
    format!("Expected an RPC registration request, but got:\n{req}")
}

/// An identifier character (`H:114-120`): an ASCII letter, digit or `_`.
fn identifier_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// iTerm2's registration check (`H:114-163`), in its order.
pub fn validate(r: &pb::RpcRegistrationRequest) -> Result<(), String> {
    let name = r.name.as_deref().unwrap_or_default();
    if name.is_empty() {
        return Err("Name has length 0".into());
    }
    if !name.chars().all(identifier_char) {
        return Err(format!("Function name '{name}' contains an invalid character. Must match /[A-Za-z0-9_]/"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for arg in &r.arguments {
        let arg = arg.name.as_deref().unwrap_or_default();
        if arg.is_empty() {
            return Err("Argument has 0-length name".into());
        }
        if !arg.chars().all(identifier_char) {
            return Err(format!("Argument name '{arg}' contains an invalid character. Must match /[A-Za-z0-9_]/"));
        }
        if !seen.insert(arg) {
            return Err(format!("Two arguments share the name '{arg}'. Argument names must be unique."));
        }
    }
    Ok(())
}

/// The namespace a registration's attributes give it (`H:165-178`): the role's prefix and
/// the unique id with `-` as `_` (dots stay); none without attributes.
pub fn namespace(r: &pb::RpcRegistrationRequest) -> Option<String> {
    let (prefix, id) = match r.role_specific_attributes.as_ref()? {
        RoleSpecificAttributes::SessionTitleAttributes(a) => ("title", &a.unique_identifier),
        RoleSpecificAttributes::StatusBarComponentAttributes(a) => ("statusbar", &a.unique_identifier),
        RoleSpecificAttributes::ContextMenuAttributes(a) => ("contextMenu", &a.unique_identifier),
    };
    Some(format!("{prefix}.{}", id.as_deref().unwrap_or_default().replace('-', "_")))
}

/// `namespace.name`, or the name alone (`H:188-194`).
pub fn fq_name(r: &pb::RpcRegistrationRequest) -> String {
    let name = r.name.as_deref().unwrap_or_default();
    match namespace(r) {
        Some(ns) => format!("{ns}.{name}"),
        None => name.to_string(),
    }
}

/// The signature a registration is kept under (`H:180-186`).
pub fn reg_signature(r: &pb::RpcRegistrationRequest) -> String {
    let args: Vec<&str> = r.arguments.iter().map(|a| a.name.as_deref().unwrap_or_default()).collect();
    expr::signature(namespace(r).as_deref(), r.name.as_deref().unwrap_or_default(), &args)
}

/// A provider's invocation from its defaults (`H:1371-1383`): `fq(name:path,...)`, sorted.
pub fn invocation(fq: &str, defaults: &[(String, String)]) -> String {
    let mut parts: Vec<String> = defaults.iter().map(|(n, p)| format!("{n}:{p}")).collect();
    parts.sort();
    format!("{fq}({})", parts.join(","))
}

/// The signature of a call sent to a script (`H:1245`): its namespace, relative name and
/// argument names.
pub fn rpc_signature(namespace: Option<&str>, rpc: &pb::ServerOriginatedRpc) -> String {
    let args: Vec<&str> = rpc.arguments.iter().map(|a| a.name.as_deref().unwrap_or_default()).collect();
    expr::signature(namespace, rpc.name.as_deref().unwrap_or_default(), &args)
}

/// A registered session title provider (`H:81-100`).
#[derive(Clone, Debug, PartialEq)]
pub struct TitleProvider {
    pub display_name: String,
    pub unique_id: String,
    pub invocation: String,
}

/// A call waiting for its script: the connection it went to, the caller's token and when it
/// stops waiting.
#[derive(Clone, Debug)]
pub struct Pending {
    pub conn: u64,
    pub token: u64,
    pub deadline: Instant,
}

/// The registrations by signature with their connections, the calls waiting by request id,
/// the next request id, and a count bumped whenever registrations are added or a closed
/// connection took some away (iTerm2's registered-functions-changed notification).
#[derive(Clone, Debug, Default)]
pub struct Registry {
    regs: BTreeMap<String, (u64, pb::RpcRegistrationRequest)>,
    pending: BTreeMap<String, Pending>,
    next_id: u64,
    generation: u64,
}

/// A registration's role; iTerm2's default, Generic, when unset or unknown.
fn role(r: &pb::RpcRegistrationRequest) -> Role {
    r.role.and_then(|v| Role::try_from(v).ok()).unwrap_or(Role::Generic)
}

/// A registration's defaults as (name, path).
fn defaults(r: &pb::RpcRegistrationRequest) -> Vec<(String, String)> {
    r.defaults.iter().map(|d| (d.name.clone().unwrap_or_default(), d.path.clone().unwrap_or_default())).collect()
}

/// A script's failure answering a call (`H:1285-1304`): never unregistered.
fn failed(reason: String, detail: Option<String>, conn: u64) -> Failure {
    Failure { reason, detail, kind: FailKind::Plain, conn: Some(conn) }
}

/// A field of a script's exception as text: a string itself, another value its text, null
/// or missing nothing.
fn exception_field(o: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    match o.get(key) {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(v) => Some(expr::to_text(v)),
    }
}

impl Registry {
    /// A NOTIFY_ON_SERVER_ORIGINATED_RPC subscribe or unsubscribe (`H:1898-1960`), with a
    /// line for the connection's log when it is refused as malformed: no `subscribe` field
    /// REQUEST_MALFORMED, then the registration's check; a signature anyone registered
    /// DUPLICATE_SERVER_ORIGINATED_RPC; an unsubscribe by a connection that does not own the
    /// signature NOT_SUBSCRIBED; else OK.
    pub fn request(&mut self, conn: u64, r: &pb::NotificationRequest) -> (pb::notification_response::Status, Option<String>) {
        use pb::notification_response::Status;
        let Some(subscribe) = r.subscribe else { return (Status::RequestMalformed, None) };
        let Some(pb::notification_request::Arguments::RpcRegistrationRequest(reg)) = &r.arguments else {
            return (Status::RequestMalformed, Some(expected_registration(&format!("{r:?}"))));
        };
        if let Err(e) = validate(reg) {
            return (Status::RequestMalformed, Some(malformed_signature(&e)));
        }
        let sig = reg_signature(reg);
        if subscribe {
            if self.regs.contains_key(&sig) {
                return (Status::DuplicateServerOriginatedRpc, None);
            }
            self.regs.insert(sig, (conn, reg.clone()));
            self.generation += 1;
            return (Status::Ok, None);
        }
        match self.regs.get(&sig) {
            Some((owner, _)) if *owner == conn => {
                self.regs.remove(&sig);
                (Status::Ok, None)
            }
            _ => (Status::NotSubscribed, None),
        }
    }

    pub fn has_signature(&self, sig: &str) -> bool {
        self.regs.contains_key(sig)
    }

    /// The connection a call goes to and its full arguments (`H:1139-1168`, `H:214-257`): a
    /// registration with the fully qualified name whose required arguments (those without
    /// defaults) are all given, each missing default filled from `scope` (a path ending `?`
    /// may be undefined, as null; any other undefined path refuses). Never for a name in the
    /// private namespace.
    pub fn resolve(&self, fq: &str, explicit: &BTreeMap<String, Value>, scope: &dyn expr::Scope) -> Option<(u64, BTreeMap<String, Value>)> {
        if fq.starts_with(PRIVATE_PREFIX) {
            return None;
        }
        'regs: for (conn, reg) in self.regs.values() {
            if fq_name(reg) != fq {
                continue;
            }
            let defaulted: Vec<(String, String)> = defaults(reg);
            let required = reg.arguments.iter().map(|a| a.name.as_deref().unwrap_or_default()).filter(|n| !defaulted.iter().any(|(d, _)| d == n));
            for name in required {
                if !explicit.contains_key(name) {
                    continue 'regs;
                }
            }
            let mut params = explicit.clone();
            for (name, path) in defaulted {
                if params.contains_key(&name) {
                    continue;
                }
                let (path, optional) = match path.strip_suffix('?') {
                    Some(p) => (p, true),
                    None => (path.as_str(), false),
                };
                match scope.value(path) {
                    Some(v) => {
                        params.insert(name, v);
                    }
                    None if optional => {
                        params.insert(name, Value::Null);
                    }
                    None => continue 'regs,
                }
            }
            return Some((*conn, params));
        }
        None
    }

    /// The signature of a registration with the fully qualified name, for the unregistered
    /// text (`H:1250-1260`).
    pub fn similar(&self, fq: &str) -> Option<String> {
        self.regs.values().find(|(_, r)| fq_name(r) == fq).map(|(_, r)| reg_signature(r))
    }

    /// Sends a call (`H:1176-1237`, `H:1306-1352`): the relative name, the arguments sorted by
    /// name with null as no `json_value`, the next request id, and the deadline from the
    /// registration's timeout or 5 s. Fails as unregistered when `conn` owns no registration
    /// with the call's signature. Returns the request id and the notification for `conn`.
    pub fn start(&mut self, conn: u64, fq: &str, args: &BTreeMap<String, Value>, token: u64, now: Instant) -> Result<(String, pb::Notification), Failure> {
        let (ns, name) = expr::split_name(fq);
        let rpc = pb::ServerOriginatedRpc {
            name: Some(name.to_string()),
            arguments: args
                .iter()
                .map(|(n, v)| pb::server_originated_rpc::RpcArgument { name: Some(n.clone()), json_value: (!v.is_null()).then(|| expr::json_text(v)) })
                .collect(),
        };
        let sig = rpc_signature(ns, &rpc);
        let timeout = match self.regs.get(&sig) {
            Some((owner, reg)) if *owner == conn && !fq.starts_with(PRIVATE_PREFIX) => reg.timeout,
            _ => {
                let reason = unregistered_text(&sig, self.similar(fq).as_deref());
                return Err(Failure { reason, detail: None, kind: FailKind::Unregistered, conn: None });
            }
        };
        let wait = match timeout {
            None => DEFAULT_RPC_TIMEOUT,
            // dispatch_after runs a past time at once; a timeout too long to represent waits a
            // year.
            Some(t) if t.is_nan() || t <= 0.0 => Duration::ZERO,
            Some(t) => Duration::try_from_secs_f64(t as f64).unwrap_or(MAX_WAIT).min(MAX_WAIT),
        };
        let id = format!("rpc-{}", self.next_id);
        self.next_id += 1;
        self.pending.insert(id.clone(), Pending { conn, token, deadline: now + wait });
        let n = pb::Notification {
            server_originated_rpc_notification: Some(pb::ServerOriginatedRpcNotification { request_id: Some(id.clone()), rpc: Some(rpc) }),
            ..Default::default()
        };
        Ok((id, n))
    }

    /// A script's answer (`H:3428-3495`): the waiting call's token and its value or failure;
    /// None for an unknown or finished request id.
    pub fn finish(&mut self, r: &pb::ServerOriginatedRpcResultRequest) -> Option<(u64, Result<Value, Failure>)> {
        use pb::server_originated_rpc_result_request::Result as R;
        let p = self.pending.remove(r.request_id.as_deref()?)?;
        let result = match &r.result {
            Some(R::JsonValue(s)) => serde_json::from_str::<Value>(s).map_err(|e| failed(undecodable_value(&e.to_string()), None, p.conn)),
            Some(R::JsonException(s)) => Err(match serde_json::from_str::<Value>(s) {
                Err(e) => failed(undecodable_exception(&e.to_string()), None, p.conn),
                Ok(Value::Object(o)) => failed(exception_field(&o, "reason").unwrap_or_else(|| UNKNOWN_REASON.into()), exception_field(&o, "traceback"), p.conn),
                Ok(_) => failed(MALFORMED_EXCEPTION.into(), None, p.conn),
            }),
            None => Err(failed(MALFORMED_RESULT.into(), None, p.conn)),
        };
        Some((p.token, result))
    }

    /// Calls whose deadline passed, failed "Timeout" (`H:1344-1356`), earliest first.
    pub fn expire(&mut self, now: Instant) -> Vec<(u64, Failure)> {
        let mut due: Vec<(String, Pending)> = Vec::new();
        self.pending.retain(|id, p| {
            if p.deadline <= now {
                due.push((id.clone(), p.clone()));
                false
            } else {
                true
            }
        });
        due.sort_by_key(|(_, p)| p.deadline);
        due.into_iter().map(|(_, p)| (p.token, failed(TIMEOUT.into(), None, p.conn))).collect()
    }

    /// The connection a waiting call went to, by the caller's token.
    pub fn conn_of(&self, token: u64) -> Option<u64> {
        self.pending.values().find(|p| p.token == token).map(|p| p.conn)
    }

    /// When the next waiting call times out.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().map(|p| p.deadline).min()
    }

    /// A closed connection (`H:2154-2214`): its registrations go and its waiting calls fail
    /// "Script terminated during function call", as unregistered.
    pub fn drop_conn(&mut self, conn: u64) -> Vec<(u64, Failure)> {
        let before = self.regs.len();
        self.regs.retain(|_, (c, _)| *c != conn);
        if self.regs.len() != before {
            self.generation += 1;
        }
        let mut failed = Vec::new();
        self.pending.retain(|_, p| {
            if p.conn == conn {
                failed.push((p.token, Failure { reason: TERMINATED.into(), detail: None, kind: FailKind::Unregistered, conn: Some(conn) }));
                false
            } else {
                true
            }
        });
        failed
    }

    /// Every SESSION_TITLE registration as a provider (`H:81-100`, `H:1393-1402`).
    pub fn title_providers(&self) -> Vec<TitleProvider> {
        self.regs
            .values()
            .filter(|(_, r)| role(r) == Role::SessionTitle)
            .map(|(_, r)| {
                let (display_name, unique_id) = match &r.role_specific_attributes {
                    Some(RoleSpecificAttributes::SessionTitleAttributes(a)) => (a.display_name.clone().unwrap_or_default(), a.unique_identifier.clone().unwrap_or_default()),
                    _ => Default::default(),
                };
                TitleProvider { display_name, unique_id, invocation: invocation(&fq_name(r), &defaults(r)) }
            })
            .collect()
    }

    /// The STATUS_BAR_COMPONENT registrations (`H:1404-1415`).
    pub fn status_bar_components(&self) -> Vec<pb::RpcRegistrationRequest> {
        self.regs.values().filter(|(_, r)| role(r) == Role::StatusBarComponent).map(|(_, r)| r.clone()).collect()
    }

    /// The status bar component registration with a unique id (`H:1438-1442`).
    pub fn status_bar_component(&self, unique_id: &str) -> Option<&pb::RpcRegistrationRequest> {
        self.regs.values().map(|(_, r)| r).find(|r| {
            role(r) == Role::StatusBarComponent
                && matches!(&r.role_specific_attributes, Some(RoleSpecificAttributes::StatusBarComponentAttributes(a)) if a.unique_identifier.as_deref().unwrap_or_default() == unique_id)
        })
    }

    /// The CONTEXT_MENU registrations sorted by display name ignoring case (`H:1417-1436`).
    pub fn context_menu_items(&self) -> Vec<pb::RpcRegistrationRequest> {
        let mut items: Vec<pb::RpcRegistrationRequest> = self.regs.values().filter(|(_, r)| role(r) == Role::ContextMenu).map(|(_, r)| r.clone()).collect();
        let key = |r: &pb::RpcRegistrationRequest| match &r.role_specific_attributes {
            Some(RoleSpecificAttributes::ContextMenuAttributes(a)) => a.display_name.clone().unwrap_or_default().to_lowercase(),
            _ => String::new(),
        };
        items.sort_by_cached_key(key);
        items
    }

    /// Bumped whenever registrations were added, or a closed connection took some away.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// The longest a call waits (a registration's timeout beyond it is clamped).
const MAX_WAIT: Duration = Duration::from_secs(365 * 24 * 3600);

#[cfg(test)]
mod tests {
    use super::*;
    use pb::notification_response::Status;
    use pb::rpc_registration_request::{ContextMenuAttributes, RpcArgument, RpcArgumentSignature, SessionTitleAttributes, StatusBarComponentAttributes};
    use serde_json::json;

    fn reg(name: &str, args: &[&str]) -> pb::RpcRegistrationRequest {
        pb::RpcRegistrationRequest {
            name: Some(name.into()),
            arguments: args.iter().map(|a| RpcArgumentSignature { name: Some(a.to_string()) }).collect(),
            ..Default::default()
        }
    }

    fn with_defaults(mut r: pb::RpcRegistrationRequest, defaults: &[(&str, &str)]) -> pb::RpcRegistrationRequest {
        r.defaults = defaults.iter().map(|(n, p)| RpcArgument { name: Some(n.to_string()), path: Some(p.to_string()) }).collect();
        r
    }

    fn title(mut r: pb::RpcRegistrationRequest, display: &str, id: &str) -> pb::RpcRegistrationRequest {
        r.role = Some(Role::SessionTitle as i32);
        r.role_specific_attributes = Some(RoleSpecificAttributes::SessionTitleAttributes(SessionTitleAttributes { display_name: Some(display.into()), unique_identifier: Some(id.into()) }));
        r
    }

    fn statusbar(mut r: pb::RpcRegistrationRequest, id: &str) -> pb::RpcRegistrationRequest {
        r.role = Some(Role::StatusBarComponent as i32);
        r.role_specific_attributes = Some(RoleSpecificAttributes::StatusBarComponentAttributes(StatusBarComponentAttributes { unique_identifier: Some(id.into()), ..Default::default() }));
        r
    }

    fn menu(mut r: pb::RpcRegistrationRequest, display: &str, id: &str) -> pb::RpcRegistrationRequest {
        r.role = Some(Role::ContextMenu as i32);
        r.role_specific_attributes = Some(RoleSpecificAttributes::ContextMenuAttributes(ContextMenuAttributes { display_name: Some(display.into()), unique_identifier: Some(id.into()) }));
        r
    }

    fn notif(subscribe: Option<bool>, r: Option<pb::RpcRegistrationRequest>) -> pb::NotificationRequest {
        pb::NotificationRequest {
            session: None,
            subscribe,
            notification_type: Some(pb::NotificationType::NotifyOnServerOriginatedRpc as i32),
            arguments: r.map(pb::notification_request::Arguments::RpcRegistrationRequest),
        }
    }

    fn subscribe(fns: &mut Registry, conn: u64, r: pb::RpcRegistrationRequest) {
        assert_eq!(fns.request(conn, &notif(Some(true), Some(r))), (Status::Ok, None));
    }

    struct Vars(BTreeMap<String, Value>);

    impl expr::Scope for Vars {
        fn value(&self, path: &str) -> Option<Value> {
            self.0.get(path).cloned()
        }
    }

    fn args(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn result(id: &str, r: Option<pb::server_originated_rpc_result_request::Result>) -> pb::ServerOriginatedRpcResultRequest {
        pb::ServerOriginatedRpcResultRequest { request_id: Some(id.into()), result: r }
    }

    fn value(s: &str) -> Option<pb::server_originated_rpc_result_request::Result> {
        Some(pb::server_originated_rpc_result_request::Result::JsonValue(s.into()))
    }

    fn exception(s: &str) -> Option<pb::server_originated_rpc_result_request::Result> {
        Some(pb::server_originated_rpc_result_request::Result::JsonException(s.into()))
    }

    #[test]
    fn registration_follows_iterm2() {
        assert_eq!(validate(&reg("", &[])), Err("Name has length 0".into()));
        assert_eq!(validate(&pb::RpcRegistrationRequest::default()), Err("Name has length 0".into()));
        assert_eq!(validate(&reg("a-b", &[])), Err("Function name 'a-b' contains an invalid character. Must match /[A-Za-z0-9_]/".into()));
        assert_eq!(validate(&reg("\u{e9}", &[])), Err("Function name '\u{e9}' contains an invalid character. Must match /[A-Za-z0-9_]/".into()));
        assert_eq!(validate(&reg("f_1", &["x_2"])), Ok(()));
        // The name is checked before any argument.
        assert_eq!(validate(&reg("", &[""])), Err("Name has length 0".into()));
        assert_eq!(validate(&reg("f", &[""])), Err("Argument has 0-length name".into()));
        assert_eq!(validate(&reg("f", &["x y"])), Err("Argument name 'x y' contains an invalid character. Must match /[A-Za-z0-9_]/".into()));
        assert_eq!(validate(&reg("f", &["a", "a"])), Err("Two arguments share the name 'a'. Argument names must be unique.".into()));
        // Namespaces: only `-` becomes `_`.
        assert_eq!(namespace(&title(reg("f", &[]), "T", "com.x-y")).as_deref(), Some("title.com.x_y"));
        assert_eq!(namespace(&statusbar(reg("f", &[]), "com.x-y")).as_deref(), Some("statusbar.com.x_y"));
        assert_eq!(namespace(&menu(reg("f", &[]), "M", "com.x-y")).as_deref(), Some("contextMenu.com.x_y"));
        assert_eq!(namespace(&reg("f", &[])), None);
        // Signatures sort the argument names.
        assert_eq!(reg_signature(&reg("f", &["b", "a"])), "f(a,b)");
        assert_eq!(reg_signature(&title(reg("f", &["b", "a"]), "T", "com.x-y")), "title.com.x_y.f(a,b)");
        assert_eq!(fq_name(&title(reg("f", &[]), "T", "com.x-y")), "title.com.x_y.f");
        assert_eq!(fq_name(&reg("f", &[])), "f");
        assert_eq!(invocation("title.x.f", &[("session_id".into(), "id".into()), ("a".into(), "path?".into())]), "title.x.f(a:path?,session_id:id)");
        assert_eq!(invocation("f", &[]), "f()");
        let rpc = pb::ServerOriginatedRpc {
            name: Some("f".into()),
            arguments: ["b", "a"].iter().map(|n| pb::server_originated_rpc::RpcArgument { name: Some(n.to_string()), json_value: None }).collect(),
        };
        assert_eq!(rpc_signature(Some("title.x"), &rpc), "title.x.f(a,b)");
        assert_eq!(rpc_signature(None, &rpc), "f(a,b)");
    }

    #[test]
    fn subscriptions_follow_iterm2() {
        let mut fns = Registry::default();
        assert_eq!(fns.generation(), 0);
        let f = reg("f", &["a"]);
        subscribe(&mut fns, 1, f.clone());
        assert_eq!(fns.generation(), 1);
        assert!(fns.has_signature("f(a)"));
        // Anyone's signature is a duplicate, the owner's too.
        assert_eq!(fns.request(2, &notif(Some(true), Some(f.clone()))), (Status::DuplicateServerOriginatedRpc, None));
        assert_eq!(fns.request(1, &notif(Some(true), Some(f.clone()))), (Status::DuplicateServerOriginatedRpc, None));
        assert_eq!(fns.generation(), 1);
        // Only the owner unsubscribes; unsubscribing changes no generation.
        assert_eq!(fns.request(2, &notif(Some(false), Some(f.clone()))), (Status::NotSubscribed, None));
        assert_eq!(fns.request(1, &notif(Some(false), Some(f.clone()))), (Status::Ok, None));
        assert!(!fns.has_signature("f(a)"));
        assert_eq!(fns.request(1, &notif(Some(false), Some(f.clone()))), (Status::NotSubscribed, None));
        assert_eq!(fns.generation(), 1);
        // No `subscribe`: malformed, nothing logged, before the registration is looked at.
        assert_eq!(fns.request(1, &notif(None, Some(f.clone()))), (Status::RequestMalformed, None));
        assert_eq!(fns.request(1, &notif(None, None)), (Status::RequestMalformed, None));
        // Arguments that are not a registration.
        let other = pb::NotificationRequest { arguments: Some(pb::notification_request::Arguments::KeystrokeMonitorRequest(Default::default())), ..notif(Some(true), None) };
        for r in [notif(Some(true), None), other] {
            let (status, line) = fns.request(1, &r);
            assert_eq!(status, Status::RequestMalformed);
            assert!(line.unwrap().starts_with("Expected an RPC registration request, but got:\n"));
        }
        // An invalid registration, subscribing or not.
        for sub in [true, false] {
            assert_eq!(fns.request(1, &notif(Some(sub), Some(reg("", &[])))), (Status::RequestMalformed, Some("Malformed RPC function signature: Name has length 0".into())));
        }
        assert_eq!(fns.generation(), 1);
        // A closed connection's registrations go; generation moves only when one went.
        subscribe(&mut fns, 1, f.clone());
        subscribe(&mut fns, 1, reg("g", &[]));
        subscribe(&mut fns, 2, reg("h", &[]));
        assert_eq!(fns.generation(), 4);
        assert!(fns.drop_conn(1).is_empty());
        assert_eq!(fns.generation(), 5);
        assert!(!fns.has_signature("f(a)") && !fns.has_signature("g()"));
        assert!(fns.has_signature("h()"));
        fns.drop_conn(1);
        fns.drop_conn(3);
        assert_eq!(fns.generation(), 5);
    }

    #[test]
    fn resolution_follows_iterm2() {
        let mut fns = Registry::default();
        subscribe(&mut fns, 4, with_defaults(reg("f", &["a", "session_id"]), &[("session_id", "id")]));
        let scope = Vars([("id".to_string(), json!("7"))].into());
        assert_eq!(fns.resolve("f", &args(&[("a", json!(1))]), &scope), Some((4, args(&[("a", json!(1)), ("session_id", json!("7"))]))));
        // An explicit value wins over the default.
        assert_eq!(fns.resolve("f", &args(&[("a", json!(1)), ("session_id", json!("x"))]), &scope), Some((4, args(&[("a", json!(1)), ("session_id", json!("x"))]))));
        // A required argument missing.
        assert_eq!(fns.resolve("f", &args(&[]), &scope), None);
        assert_eq!(fns.resolve("f", &args(&[("session_id", json!("x"))]), &scope), None);
        // A required default the scope lacks.
        let empty = Vars(BTreeMap::new());
        assert_eq!(fns.resolve("f", &args(&[("a", json!(1))]), &empty), None);
        // An optional default path the scope lacks is null.
        subscribe(&mut fns, 5, with_defaults(reg("g", &["p"]), &[("p", "missing?")]));
        assert_eq!(fns.resolve("g", &args(&[]), &empty), Some((5, args(&[("p", Value::Null)]))));
        // Another name, and the private namespace.
        assert_eq!(fns.resolve("h", &args(&[("a", json!(1))]), &scope), None);
        subscribe(&mut fns, 6, title(reg("t", &[]), "T", "x"));
        assert_eq!(fns.resolve("title.x.t", &args(&[]), &scope).map(|r| r.0), Some(6));
        assert_eq!(fns.resolve("iterm2.f", &args(&[("a", json!(1))]), &scope), None);
        assert_eq!(fns.resolve("iterm2.private.session_title", &args(&[]), &scope), None);
        assert_eq!(fns.similar("f").as_deref(), Some("f(a,session_id)"));
        assert_eq!(fns.similar("nope"), None);
    }

    #[test]
    fn calls_follow_iterm2() {
        let t0 = Instant::now();
        let mut fns = Registry::default();
        subscribe(&mut fns, 1, reg("f", &["a", "b", "c"]));
        let call = args(&[("c", Value::Null), ("b", json!("s")), ("a", json!(1.0))]);
        let (id, n) = fns.start(1, "f", &call, 10, t0).unwrap();
        assert_eq!(id, "rpc-0");
        let rpc_n = n.server_originated_rpc_notification.clone().unwrap();
        assert_eq!(rpc_n.request_id.as_deref(), Some("rpc-0"));
        let rpc = rpc_n.rpc.unwrap();
        assert_eq!(rpc.name.as_deref(), Some("f"));
        let sent: Vec<(Option<String>, Option<String>)> = rpc.arguments.into_iter().map(|a| (a.name, a.json_value)).collect();
        assert_eq!(sent, [(Some("a".into()), Some("1".into())), (Some("b".into()), Some("\"s\"".into())), (Some("c".into()), None)]);
        assert_eq!(n, pb::Notification { server_originated_rpc_notification: n.server_originated_rpc_notification.clone(), ..Default::default() });
        assert_eq!(fns.next_deadline(), Some(t0 + DEFAULT_RPC_TIMEOUT));
        // Results.
        assert_eq!(fns.finish(&result("rpc-0", value("3"))), Some((10, Ok(json!(3)))));
        assert_eq!(fns.finish(&result("rpc-0", value("3"))), None, "already finished");
        assert_eq!(fns.finish(&result("rpc-99", value("3"))), None, "unknown");
        assert_eq!(fns.finish(&pb::ServerOriginatedRpcResultRequest { request_id: None, result: value("3") }), None);
        assert_eq!(fns.next_deadline(), None);
        let next = |fns: &mut Registry, token| fns.start(1, "f", &call, token, t0).unwrap().0;
        let id = next(&mut fns, 11);
        assert_eq!(id, "rpc-1");
        assert_eq!(fns.finish(&result(&id, value("null"))), Some((11, Ok(Value::Null))));
        let id = next(&mut fns, 12);
        let (token, r) = fns.finish(&result(&id, value("{bad"))).unwrap();
        let f = r.unwrap_err();
        assert_eq!((token, f.kind, f.conn), (12, FailKind::Plain, Some(1)));
        assert!(f.reason.starts_with("Undecodable value: "), "{}", f.reason);
        let id = next(&mut fns, 13);
        assert_eq!(fns.finish(&result(&id, exception(r#"{"reason": "boom", "traceback": "tb"}"#))), Some((13, Err(Failure { reason: "boom".into(), detail: Some("tb".into()), kind: FailKind::Plain, conn: Some(1) }))));
        let id = next(&mut fns, 14);
        assert_eq!(fns.finish(&result(&id, exception("{}"))).unwrap().1.unwrap_err().reason, UNKNOWN_REASON);
        let id = next(&mut fns, 15);
        assert_eq!(fns.finish(&result(&id, exception("[1]"))).unwrap().1.unwrap_err().reason, "Malformed exception");
        let id = next(&mut fns, 16);
        assert!(fns.finish(&result(&id, exception("{bad"))).unwrap().1.unwrap_err().reason.starts_with("Undecodable exception: "));
        let id = next(&mut fns, 17);
        assert_eq!(fns.finish(&result(&id, None)).unwrap().1.unwrap_err().reason, "Malformed result.");
        // No registration with the signature, or another connection's: unregistered.
        let f = fns.start(1, "f", &args(&[("a", json!(1))]), 18, t0).unwrap_err();
        assert_eq!(f.kind, FailKind::Unregistered);
        assert_eq!(f.reason, unregistered_text("f(a)", Some("f(a,b,c)")));
        assert_eq!(fns.start(2, "f", &call, 18, t0).unwrap_err().kind, FailKind::Unregistered);
        assert_eq!(fns.start(1, "g", &args(&[]), 18, t0).unwrap_err().reason, unregistered_text("g()", None));
        // A failed start takes no request id.
        assert_eq!(next(&mut fns, 19), "rpc-8");
        // The deadline: "Timeout" at it, not before.
        assert!(fns.expire(t0 + DEFAULT_RPC_TIMEOUT - Duration::from_millis(1)).is_empty());
        let out = fns.expire(t0 + DEFAULT_RPC_TIMEOUT);
        assert_eq!(out, [(19, Failure { reason: "Timeout".into(), detail: None, kind: FailKind::Plain, conn: Some(1) })]);
        assert!(fns.finish(&result("rpc-8", value("1"))).is_none(), "a timed-out call ignores its result");
        // A registration's timeout.
        let mut timed = reg("slow", &[]);
        timed.timeout = Some(2.5);
        subscribe(&mut fns, 1, timed);
        fns.start(1, "slow", &args(&[]), 20, t0).unwrap();
        assert_eq!(fns.next_deadline(), Some(t0 + Duration::from_millis(2500)));
        // A closed connection fails its waiting calls as unregistered; others wait on.
        subscribe(&mut fns, 2, reg("other", &[]));
        fns.start(2, "other", &args(&[]), 21, t0).unwrap();
        assert_eq!((fns.conn_of(20), fns.conn_of(21), fns.conn_of(99)), (Some(1), Some(2), None));
        assert_eq!(fns.drop_conn(1), [(20, Failure { reason: "Script terminated during function call".into(), detail: None, kind: FailKind::Unregistered, conn: Some(1) })]);
        assert_eq!(fns.next_deadline(), Some(t0 + DEFAULT_RPC_TIMEOUT));
        assert!(fns.drop_conn(1).is_empty());
    }

    #[test]
    fn providers_follow_iterm2() {
        let mut fns = Registry::default();
        subscribe(&mut fns, 1, title(with_defaults(reg("f", &["session_id", "a"]), &[("session_id", "id"), ("a", "path?")]), "My Title", "com.x-y"));
        subscribe(&mut fns, 1, reg("generic", &[]));
        subscribe(&mut fns, 1, statusbar(reg("s", &[]), "com.x"));
        subscribe(&mut fns, 2, statusbar(reg("s", &[]), "com.y"));
        assert_eq!(fns.title_providers(), [TitleProvider { display_name: "My Title".into(), unique_id: "com.x-y".into(), invocation: "title.com.x_y.f(a:path?,session_id:id)".into() }]);
        // iTerm2 reads the attributes through protobuf accessors that never give nil: a title
        // registration without them is still a provider, with empty names.
        let mut bare = reg("bare", &[]);
        bare.role = Some(Role::SessionTitle as i32);
        subscribe(&mut fns, 1, bare);
        assert_eq!(fns.title_providers().len(), 2);
        assert!(fns.title_providers().contains(&TitleProvider { display_name: String::new(), unique_id: String::new(), invocation: "bare()".into() }));
        // Status bar components.
        assert_eq!(fns.status_bar_components().len(), 2);
        assert_eq!(fns.status_bar_component("com.x").map(fq_name).as_deref(), Some("statusbar.com.x.s"));
        assert_eq!(fns.status_bar_component("com.y").map(fq_name).as_deref(), Some("statusbar.com.y.s"));
        assert!(fns.status_bar_component("com.z").is_none());
        // Context menu items by display name, ignoring case.
        for (name, display) in [("f1", "b"), ("f2", "A"), ("f3", "c")] {
            subscribe(&mut fns, 1, menu(reg(name, &[]), display, name));
        }
        let names: Vec<String> = fns.context_menu_items().iter().map(|r| r.name.clone().unwrap()).collect();
        assert_eq!(names, ["f2", "f1", "f3"]);
        assert_eq!(
            unregistered_text("f(a)", Some("f(b)")),
            "No function registered for invocation \u{201c}f(a)\u{201d}. Ensure the script is running and the function name and argument names are correct. There is a similarly named function available with a different signature: f(b)"
        );
        assert_eq!(
            unregistered_text("f(a)", None),
            "No function registered for invocation \u{201c}f(a)\u{201d}. Ensure the script is running and the function name and argument names are correct."
        );
        assert_eq!(disabled_text("f(a)"), "Undefined function with signature \u{201c}f(a)\u{201d}");
        assert_eq!(not_encodable("x"), "Could not JSON encode value \u{201c}x\u{201d}");
        assert_eq!(undecodable_value("e"), "Undecodable value: e");
        assert_eq!(undecodable_exception("e"), "Undecodable exception: e");
    }
}
