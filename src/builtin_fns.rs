//! iTerm2's built-in functions and methods (`BIF`, `BF/`, `PS:23440-23578`,
//! `PTYTab.m:7893-7953`, `PseudoTerminal.m:15993-16225`): the catalog of names, argument
//! types, optional arguments, defaults from the scope and side-effect placeholders, the
//! functions that need no app, and the texts. The app runs the rest (`app::builtin_ui`).

use std::collections::BTreeMap;

use crate::expr::{self, Value};

/// The built-ins' namespace.
pub const NAMESPACE: &str = "iterm2";

/// An argument's type: iTerm2's `NSString`, `NSNumber` (booleans too), `NSArray`, or
/// `NSObject` (anything).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    Str,
    Num,
    Array,
    Any,
}

/// What a built-in runs on: a plain function, or a method of a session, tab or window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum On {
    Function,
    Session,
    Tab,
    Window,
}

/// A built-in: its name in the namespace, what it runs on, its arguments, those that may be
/// left out, the arguments filled from a scope path when not given, and the answer when side
/// effects are off.
#[derive(Debug)]
pub struct Builtin {
    pub name: &'static str,
    pub on: On,
    pub args: &'static [(&'static str, Ty)],
    pub optional: &'static [&'static str],
    pub defaults: &'static [(&'static str, &'static str)],
    pub placeholder: Option<&'static str>,
}

const fn f(name: &'static str, args: &'static [(&'static str, Ty)], optional: &'static [&'static str], defaults: &'static [(&'static str, &'static str)], placeholder: Option<&'static str>) -> Builtin {
    Builtin { name, on: On::Function, args, optional, defaults, placeholder }
}

const fn m(on: On, name: &'static str, args: &'static [(&'static str, Ty)], optional: &'static [&'static str], placeholder: Option<&'static str>) -> Builtin {
    Builtin { name, on, args, optional, defaults: &[], placeholder }
}

/// The ported built-ins (`BIF:164-183`, `PS:815`, `PTYTab.m:335`, and the objects'
/// `objectMethodRegistry`). The session id default is `iTermVariableKeySessionID` ("id"),
/// the tab id's `iTermVariableKeyTabID` ("id").
static CATALOG: [Builtin; 22] = [
    f("add_clipping", &[("type", Ty::Str), ("title", Ty::Str), ("detail", Ty::Str)], &[], &[("session", "id")], Some("[add_clipping]")),
    f("archive_clippings", &[], &[], &[("session", "id")], Some("[archive_clippings]")),
    f("count", &[("array", Ty::Array)], &[], &[], None),
    f("alert", &[("title", Ty::Str), ("subtitle", Ty::Str), ("buttons", Ty::Array), ("window_id", Ty::Any)], &["window_id"], &[("window_id", "")], Some("[alert]")),
    f("get_string", &[("title", Ty::Str), ("subtitle", Ty::Str), ("placeholder", Ty::Str), ("defaultValue", Ty::Str), ("window_id", Ty::Any)], &["window_id"], &[], Some("[get_string]")),
    f("focus", &[], &[], &[("session_id", "id")], Some("[focus]")),
    f("set_status_bar_component_unread_count", &[("identifier", Ty::Str), ("count", Ty::Num)], &[], &[], Some("[set_status_bar_component_unread_count]")),
    f("paste", &[], &[], &[("session_id", "id")], Some("[paste]")),
    f("get_profile_property", &[("key", Ty::Str)], &[], &[("session_id", "id")], None),
    f("with_alpha", &[("color", Ty::Str), ("alpha", Ty::Num)], &[], &[], None),
    f("urlEncode", &[("string", Ty::Str)], &[], &[], None),
    f(
        "set_status",
        &[("status", Ty::Str), ("text_color", Ty::Str), ("dot_color", Ty::Str), ("detail", Ty::Str), ("background_tasks", Ty::Num)],
        &["status", "text_color", "dot_color", "detail", "background_tasks"],
        &[("session_id", "id")],
        Some("[set_status]"),
    ),
    f("move_tab_to_window", &[], &[], &[("tab_id", "id")], Some("[move_tab_to_window]")),
    m(On::Session, "set_name", &[("name", Ty::Str)], &[], Some("[set_name]")),
    m(On::Session, "run_tmux_command", &[("command", Ty::Str)], &[], Some("[run_tmux_command]")),
    m(On::Session, "set_status_bar_component_unread_count", &[("identifier", Ty::Str), ("count", Ty::Num)], &[], Some("[set_status_bar_component_unread_count]")),
    m(On::Session, "stop_coprocess", &[], &[], Some("[stop_coprocess]")),
    m(On::Session, "get_coprocess", &[], &[], None),
    m(On::Session, "run_coprocess", &[("commandLine", Ty::Str), ("mute", Ty::Num)], &["mute"], Some("[run_coprocess]")),
    m(On::Tab, "set_title", &[("title", Ty::Str)], &[], Some("[set_title]")),
    m(On::Tab, "select_pane_in_direction", &[("direction", Ty::Str)], &[], Some("[select_pane_in_direction]")),
    m(On::Window, "set_title", &[("title", Ty::Str)], &[], Some("[set_title]")),
];

pub fn catalog() -> &'static [Builtin] {
    &CATALOG
}

impl Builtin {
    /// `iterm2.<name>`.
    pub fn fq_name(&self) -> String {
        format!("{NAMESPACE}.{}", self.name)
    }

    fn ty(&self, arg: &str) -> Option<Ty> {
        self.args.iter().find(|(n, _)| *n == arg).map(|(_, t)| *t)
    }
}

/// The built-in on `on` a call's signature names: its fully qualified name, and argument
/// names holding every required argument and only known ones (iTerm2 registers every
/// combination of the optional arguments, `BIF:20-33`, and matches methods the same way,
/// `BIF:494-520`).
pub fn find(on: On, signature: &str) -> Option<&'static Builtin> {
    let (fq, rest) = signature.split_once('(')?;
    let list = rest.strip_suffix(')')?;
    let names: Vec<&str> = if list.is_empty() { Vec::new() } else { list.split(',').collect() };
    CATALOG.iter().find(|b| {
        b.on == on
            && b.fq_name() == fq
            && names.iter().all(|n| b.ty(n).is_some())
            && b.args.iter().all(|(n, _)| b.optional.contains(n) || names.contains(n))
    })
}

/// A signature of a built-in on `on` with fully qualified name `fq`, for the unregistered
/// text's "similarly named function" (`signatureOfAnyRegisteredFunctionWithName:`).
pub fn similar(on: On, fq: &str) -> Option<String> {
    let b = CATALOG.iter().find(|b| b.on == on && b.fq_name() == fq)?;
    let names: Vec<&str> = b.args.iter().map(|(n, _)| *n).collect();
    Some(expr::signature(Some(NAMESPACE), b.name, &names))
}

/// The explicit arguments plus each default not given whose path has a value in the scope
/// (`BIF:251-263`).
pub fn amend(b: &Builtin, explicit: &BTreeMap<String, Value>, scope: &dyn expr::Scope) -> BTreeMap<String, Value> {
    let mut out = explicit.clone();
    for (arg, path) in b.defaults {
        if explicit.contains_key(*arg) {
            continue;
        }
        if let Some(v) = scope.value(path) {
            out.insert(arg.to_string(), v);
        }
    }
    out
}

/// iTerm2's `typeCheckParameters:` (`BIF:106-127`) over the explicit arguments: a null for an
/// optional argument passes, an unknown name is an invalid argument, a value of another type a
/// mismatch.
pub fn type_check(b: &Builtin, explicit: &BTreeMap<String, Value>) -> Result<(), String> {
    for (name, v) in explicit {
        if v.is_null() && b.optional.contains(&name.as_str()) {
            continue;
        }
        let Some(want) = b.ty(name) else { return Err(invalid_argument(name, b.name)) };
        let fits = match want {
            Ty::Str => v.is_string(),
            Ty::Num => v.is_number() || v.is_boolean(),
            Ty::Array => v.is_array(),
            Ty::Any => true,
        };
        if !fits {
            return Err(type_mismatch(name, want, Some(v)));
        }
    }
    Ok(())
}

/// `iterm2.count(array:)` (`BIF:318-338`).
pub fn count(v: Option<&Value>) -> Result<Value, String> {
    match v {
        None | Some(Value::Null) => Err("Array argument must be non-null".into()),
        Some(Value::Array(a)) => Ok(Value::from(a.len())),
        Some(other) => Err(format!("Argument must be an array (was {})", expr::type_name(other))),
    }
}

/// `iterm2.urlEncode(string:)` (`BIF:365-389`): percent-encoded outside
/// `URLQueryAllowedCharacterSet`.
pub fn url_encode(v: Option<&Value>) -> Result<Value, String> {
    let s = match v {
        None | Some(Value::Null) => return Err("string argument must be non-null".into()),
        Some(Value::String(s)) => s,
        Some(other) => return Err(format!("Argument must be a string (was {})", expr::type_name(other))),
    };
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"!$&'()*+,-./:;=?@_~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    Ok(Value::String(out))
}

/// `getHashColorRed:green:blue:` (`NSStringITerm.m:2620-2662`): `#rgb`, `#rrggbb` or
/// `#rrrrggggbbbb`, as 16-bit components. Every digit must be hex.
fn hash_color(s: &str) -> Option<[u16; 3]> {
    let digits = s.strip_prefix('#').filter(|d| d.bytes().all(|b| b.is_ascii_hexdigit()))?;
    let n = |t: &str| u16::from_str_radix(t, 16).ok();
    match digits.len() {
        3 => {
            let d: Vec<u16> = digits.chars().map(|c| n(&c.to_string()).map(|v| v * 0x1111)).collect::<Option<_>>()?;
            Some([d[0], d[1], d[2]])
        }
        6 => Some([n(&digits[0..2])? * 257, n(&digits[2..4])? * 257, n(&digits[4..6])? * 257]),
        12 => Some([n(&digits[0..4])?, n(&digits[4..8])?, n(&digits[8..12])?]),
        _ => None,
    }
}

/// A color `set_status` accepts (`iTermSRGBColorFromHexString`).
pub fn is_hash_color(s: &str) -> bool {
    hash_color(s).is_some()
}

/// `iterm2.with_alpha(color:, alpha:)` (`BF/ColorAlphaBuiltInFunction.swift:20-54`).
pub fn with_alpha(color: &str, alpha: f64) -> Result<Value, String> {
    // `colorFromHexString:allowingAlpha:YES` (`NSColor+iTerm.m:963-1014`): eight digits carry
    // an alpha, which the new one replaces.
    let rgb = match color.strip_prefix('#') {
        Some(d) if d.len() == 8 => {
            if !d[6..].bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(cannot_parse_color(color));
            }
            format!("#{}", &d[..6])
        }
        _ => color.to_string(),
    };
    let c = hash_color(&rgb).ok_or_else(|| cannot_parse_color(color))?;
    // `hexStringForDictionary:` truncates each component to 8 bits.
    let byte = |v: u16| (f64::from(v) / 65535.0 * 255.0 + 1e-9) as u8;
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    Ok(Value::String(format!("#{:02x}{:02x}{:02x}{a:02x}", byte(c[0]), byte(c[1]), byte(c[2]))))
}

/// `select_pane_in_direction`'s direction (`PTYTab.m:7927-7942`).
pub fn direction(s: &str) -> Option<crate::layout::Dir> {
    use crate::layout::Dir;
    match s {
        "left" => Some(Dir::Left),
        "right" => Some(Dir::Right),
        "above" => Some(Dir::Up),
        "below" => Some(Dir::Down),
        _ => None,
    }
}

/// What a method's receiver id names: a window when it starts `pty-`, else a session or a tab,
/// the one whose catalog has the signature (a session when both or neither do).
pub fn receiver_kind(id: &str, pane: bool, tab: bool, window: bool, sig: &str) -> Option<On> {
    if id.starts_with("pty-") {
        return window.then_some(On::Window);
    }
    match (pane, tab) {
        (true, true) if find(On::Session, sig).is_none() && find(On::Tab, sig).is_some() => Some(On::Tab),
        (true, _) => Some(On::Session),
        (false, true) => Some(On::Tab),
        (false, false) => None,
    }
}

/// `NSAlertFirstButtonReturn`: `alert` answers it plus the button's index.
pub const ALERT_FIRST_BUTTON: i64 = 1000;

pub const NO_SUCH_SESSION: &str = "No such session";
pub const MISSING_SESSION_ID: &str = "Missing session_id. This shouldn\u{2019}t happen so please report a bug.";
pub const NO_SUCH_TAB: &str = "No such tab";
pub const TAB_HAS_NO_WINDOW: &str = "Tab has no window";
pub const ONLY_ONE_TAB: &str = "Window has only one tab";
pub const WINDOW_FAILED: &str = "Failed to create new window";
pub const MISSING_ARGUMENT: &str = "Missing required argument";
pub const INVALID_DIRECTION: &str = "Invalid direction. Should be left, right, above or below.";
pub const INVALID_TEXT_COLOR: &str = "Invalid text_color (expected #rrggbb)";
pub const INVALID_DOT_COLOR: &str = "Invalid dot_color (expected #rrggbb)";
pub const NOT_TMUX: &str = "Not a tmux integration session";
/// tmux answered `run_tmux_command` with an error (`PS:23737-23741`).
pub const TMUX_ERROR: &str = "tmux error";

/// "Undeclared identifier ..." (`BIF:270-276`).
pub fn undeclared(sig: &str) -> String {
    format!("Undeclared identifier {sig}")
}

/// "Type mismatch for argument ..." (`BIF:131-139`); `got` None is a missing value.
pub fn type_mismatch(arg: &str, want: Ty, got: Option<&Value>) -> String {
    let want = match want {
        Ty::Str => "string",
        Ty::Num => "number",
        Ty::Array => "array",
        Ty::Any => "value",
    };
    format!("Type mismatch for argument {arg}. Expected {want} but got {}.", got.map_or("(null)", expr::type_name))
}

/// "Invalid argument ... to ..." (`BIF:141-147`).
pub fn invalid_argument(arg: &str, name: &str) -> String {
    format!("Invalid argument {arg} to {name}")
}

/// A method call whose receiver has no method with the signature (`iTermObject.m:79-88`).
pub fn no_method(on: On, id: &str, sig: &str) -> String {
    let object = match on {
        On::Session | On::Function => "Session",
        On::Tab => "Tab",
        On::Window => "Window",
    };
    format!("No method found on <{object} id={id}> with signature {sig}")
}

/// `with_alpha`'s parse failure.
pub fn cannot_parse_color(c: &str) -> String {
    format!("with_alpha could not parse color \u{201c}{c}\u{201d}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn catalog_follows_iterm2() {
        let alert = find(On::Function, "iterm2.alert(buttons,subtitle,title)").expect("alert");
        assert_eq!(alert.name, "alert");
        assert_eq!(find(On::Function, "iterm2.alert(buttons,subtitle,title,window_id)").map(|b| b.name), Some("alert"));
        assert!(find(On::Function, "iterm2.alert(title)").is_none());
        assert!(find(On::Function, "iterm2.alert(bogus,buttons,subtitle,title)").is_none());
        assert!(find(On::Function, "alert(buttons,subtitle,title)").is_none(), "the namespace is part of the name");
        assert_eq!(find(On::Function, "iterm2.set_status()").map(|b| b.name), Some("set_status"));
        assert_eq!(find(On::Function, "iterm2.set_status(detail,status)").map(|b| b.name), Some("set_status"));
        assert_eq!(find(On::Session, "iterm2.set_name(name)").map(|b| (b.name, b.on)), Some(("set_name", On::Session)));
        assert!(find(On::Tab, "iterm2.set_name(name)").is_none());
        assert!(find(On::Function, "iterm2.set_name(name)").is_none(), "methods are not functions");
        assert_eq!(find(On::Tab, "iterm2.set_title(title)").map(|b| b.on), Some(On::Tab));
        assert_eq!(find(On::Window, "iterm2.set_title(title)").map(|b| b.on), Some(On::Window));
        assert_eq!(find(On::Session, "iterm2.run_coprocess(commandLine)").map(|b| b.name), Some("run_coprocess"));
        assert_eq!(find(On::Session, "iterm2.run_coprocess(commandLine,mute)").map(|b| b.name), Some("run_coprocess"));

        // Defaults from the scope; an explicit value wins; a missing path is left out.
        let focus = find(On::Function, "iterm2.focus()").unwrap();
        let scope = args(&[("id", json!("7"))]);
        assert_eq!(amend(focus, &BTreeMap::new(), &scope), args(&[("session_id", json!("7"))]));
        assert_eq!(amend(focus, &args(&[("session_id", json!("9"))]), &scope), args(&[("session_id", json!("9"))]));
        assert_eq!(amend(focus, &BTreeMap::new(), &BTreeMap::new()), BTreeMap::new());
        assert_eq!(amend(alert, &args(&[("title", json!("t"))]), &scope), args(&[("title", json!("t"))]), "window_id's empty path has no value");

        // Type checks over the explicit arguments.
        let base = |title: Value| args(&[("title", title), ("subtitle", json!("s")), ("buttons", json!(["OK"]))]);
        assert_eq!(type_check(alert, &base(json!(1))), Err("Type mismatch for argument title. Expected string but got number.".to_string()));
        assert_eq!(type_check(alert, &base(json!("t"))), Ok(()));
        let mut bogus = base(json!("t"));
        bogus.insert("bogus".into(), json!(1));
        assert_eq!(type_check(alert, &bogus), Err("Invalid argument bogus to alert".to_string()));
        let mut null_window = base(json!("t"));
        null_window.insert("window_id".into(), Value::Null);
        assert_eq!(type_check(alert, &null_window), Ok(()), "a null for an optional argument passes");
        assert_eq!(type_check(alert, &base(Value::Null)), Err("Type mismatch for argument title. Expected string but got null.".to_string()));
        let set_status = find(On::Function, "iterm2.set_status()").unwrap();
        assert_eq!(type_check(set_status, &args(&[("background_tasks", json!(true))])), Ok(()), "a boolean is a number");
        assert_eq!(type_check(set_status, &args(&[("background_tasks", json!("2"))])), Err("Type mismatch for argument background_tasks. Expected number but got string.".to_string()));
        assert_eq!(type_mismatch("x", Ty::Array, None), "Type mismatch for argument x. Expected array but got (null).");

        assert_eq!(alert.placeholder, Some("[alert]"));
        assert_eq!(find(On::Function, "iterm2.get_profile_property(key)").unwrap().placeholder, None);
        assert_eq!(undeclared("iterm2.x()"), "Undeclared identifier iterm2.x()");
        assert_eq!(similar(On::Function, "iterm2.alert"), Some("iterm2.alert(buttons,subtitle,title,window_id)".to_string()));
        assert_eq!(similar(On::Function, "iterm2.set_name"), None);
        assert_eq!(similar(On::Function, "iterm2.nothing"), None);

        // Exactly the ported names (the Resume notes' "Built-ins").
        let names: Vec<(On, &str)> = catalog().iter().map(|b| (b.on, b.name)).collect();
        let want: Vec<(On, &str)> = [
            "add_clipping",
            "archive_clippings",
            "count",
            "alert",
            "get_string",
            "focus",
            "set_status_bar_component_unread_count",
            "paste",
            "get_profile_property",
            "with_alpha",
            "urlEncode",
            "set_status",
            "move_tab_to_window",
        ]
        .into_iter()
        .map(|n| (On::Function, n))
        .chain(["set_name", "run_tmux_command", "set_status_bar_component_unread_count", "stop_coprocess", "get_coprocess", "run_coprocess"].into_iter().map(|n| (On::Session, n)))
        .chain([(On::Tab, "set_title"), (On::Tab, "select_pane_in_direction"), (On::Window, "set_title")])
        .collect();
        assert_eq!(names, want);
    }

    #[test]
    fn pure_builtins_follow_iterm2() {
        assert_eq!(count(Some(&json!([1, 2]))), Ok(json!(2)));
        assert_eq!(count(None), Err("Array argument must be non-null".to_string()));
        assert_eq!(count(Some(&json!("x"))), Err("Argument must be an array (was string)".to_string()));
        assert_eq!(url_encode(Some(&json!("a b&c"))), Ok(json!("a%20b&c")));
        assert_eq!(url_encode(Some(&json!("x=1?/:@!$'()*+,;~-._"))), Ok(json!("x=1?/:@!$'()*+,;~-._")), "the query-allowed set");
        assert_eq!(url_encode(Some(&json!("%#[]\"<>\u{e9}"))), Ok(json!("%25%23%5B%5D%22%3C%3E%C3%A9")));
        assert_eq!(url_encode(None), Err("string argument must be non-null".to_string()));
        assert_eq!(url_encode(Some(&json!(3))), Err("Argument must be a string (was number)".to_string()));
        assert_eq!(with_alpha("#ff0000", 0.5), Ok(json!("#ff000080")));
        assert_eq!(with_alpha("#ff0000", 2.0), Ok(json!("#ff0000ff")));
        assert_eq!(with_alpha("#ff0000", -1.0), Ok(json!("#ff000000")));
        assert_eq!(with_alpha("#f00", 1.0), Ok(json!("#ff0000ff")), "the short form");
        assert_eq!(with_alpha("#11223344", 1.0), Ok(json!("#112233ff")), "an alpha given is replaced");
        assert_eq!(with_alpha("#ffff80800000", 1.0), Ok(json!("#ff8000ff")), "the 16-bit form");
        assert_eq!(with_alpha("bogus", 1.0), Err("with_alpha could not parse color \u{201c}bogus\u{201d}".to_string()));
        assert_eq!(with_alpha("#ff00zz", 1.0), Err(cannot_parse_color("#ff00zz")));
        assert_eq!(direction("left"), Some(crate::layout::Dir::Left));
        assert_eq!(direction("right"), Some(crate::layout::Dir::Right));
        assert_eq!(direction("above"), Some(crate::layout::Dir::Up));
        assert_eq!(direction("below"), Some(crate::layout::Dir::Down));
        assert_eq!(direction("up"), None);
        assert_eq!(invalid_argument("a", "b"), "Invalid argument a to b");
    }

    #[test]
    fn receivers_follow_iterm2() {
        assert_eq!(receiver_kind("pty-A", false, false, true, "iterm2.set_title(title)"), Some(On::Window));
        assert_eq!(receiver_kind("pty-A", false, false, false, "iterm2.set_title(title)"), None);
        assert_eq!(receiver_kind("7", true, true, false, "iterm2.set_name(name)"), Some(On::Session));
        assert_eq!(receiver_kind("7", true, true, false, "iterm2.set_title(title)"), Some(On::Tab));
        assert_eq!(receiver_kind("7", true, true, false, "iterm2.select_pane_in_direction(direction)"), Some(On::Tab));
        assert_eq!(receiver_kind("7", true, true, false, "iterm2.bogus()"), Some(On::Session), "neither has it: the session");
        assert_eq!(receiver_kind("7", false, true, false, "iterm2.set_name(name)"), Some(On::Tab));
        assert_eq!(receiver_kind("7", true, false, false, "iterm2.set_title(title)"), Some(On::Session));
        assert_eq!(receiver_kind("7", false, false, false, "iterm2.set_name(name)"), None);
        assert_eq!(crate::script_eval::object_not_found("7"), "Object with identifier \u{201c}7\u{201d} not found");
        assert_eq!(no_method(On::Session, "7", "iterm2.x()"), "No method found on <Session id=7> with signature iterm2.x()");
    }
}
