//! iTerm2's key binding actions (the KEY_ACTION codes), their stored form, the text
//! escaping modes of the Send actions, and which milestone makes each one work.

use std::sync::OnceLock;

use toml::Value;

use crate::commands::Status;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub action: i64,
    /// iTerm2's "Text": the action's parameter.
    pub text: String,
    pub label: String,
    pub version: i64,
    pub escaping: Option<i64>,
    pub apply_mode: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Escaping {
    None,
    Compatibility,
    Common,
    Vim,
    VimAndCompatibility,
}

impl Escaping {
    pub fn code(self) -> i64 {
        match self {
            Escaping::None => 0,
            Escaping::Compatibility => 1,
            Escaping::Common => 2,
            Escaping::Vim => 3,
            Escaping::VimAndCompatibility => 4,
        }
    }

    pub fn from_code(code: i64) -> Escaping {
        match code {
            0 => Escaping::None,
            1 => Escaping::Compatibility,
            3 => Escaping::Vim,
            4 => Escaping::VimAndCompatibility,
            _ => Escaping::Common,
        }
    }
}

fn int(t: &toml::Table, key: &str) -> Option<i64> {
    t.get(key).and_then(|v| v.as_integer().or_else(|| v.as_float().map(|f| f as i64)).or_else(|| v.as_str()?.parse().ok()))
}

fn string(t: &toml::Table, key: &str) -> String {
    t.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

impl Binding {
    pub fn new(action: i64, text: &str) -> Binding {
        // New bindings from the editor use explicit Common escaping, as iTerm2's editor does.
        Binding { action, text: text.into(), label: String::new(), version: 2, escaping: Some(2), apply_mode: 0 }
    }

    pub fn from_value(v: &Value) -> Option<Binding> {
        let t = v.as_table()?;
        Some(Binding {
            action: int(t, "Action")?,
            text: string(t, "Text"),
            label: string(t, "Label"),
            version: int(t, "Version").unwrap_or(0),
            escaping: int(t, "Escaping"),
            apply_mode: int(t, "Apply Mode").unwrap_or(0),
        })
    }

    pub fn to_value(&self) -> Value {
        let mut t = toml::Table::new();
        t.insert("Action".into(), Value::Integer(self.action));
        t.insert("Text".into(), Value::String(self.text.clone()));
        if !self.label.is_empty() {
            t.insert("Label".into(), Value::String(self.label.clone()));
        }
        if self.version != 0 {
            t.insert("Version".into(), Value::Integer(self.version));
        }
        if let Some(e) = self.escaping {
            t.insert("Escaping".into(), Value::Integer(e));
        }
        if self.apply_mode != 0 {
            t.insert("Apply Mode".into(), Value::Integer(self.apply_mode));
        }
        Value::Table(t)
    }

    /// Version 0 (or none) means Compatibility, 1 Common, and 2 the stored Escaping.
    pub fn escaping(&self) -> Escaping {
        match self.version {
            0 => Escaping::Compatibility,
            1 => Escaping::Common,
            _ => Escaping::from_code(self.escaping.unwrap_or(2)),
        }
    }

    /// The escaping for the vim Send Text actions (iTerm2 vimEscaping).
    pub fn vim_escaping(&self) -> Escaping {
        match self.escaping() {
            Escaping::Compatibility | Escaping::VimAndCompatibility => Escaping::VimAndCompatibility,
            _ => Escaping::Vim,
        }
    }
}

/// The bindings of a Run Sequence action (a JSON array of binding dictionaries).
pub fn sequence(text: &str) -> Vec<Binding> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| {
            let v: Value = toml::Value::try_from(item).ok()?;
            Binding::from_value(&v)
        })
        .collect()
}

pub fn sequence_text(items: &[Binding]) -> String {
    let list: Vec<serde_json::Value> = items.iter().filter_map(|b| serde_json::to_value(b.to_value()).ok()).collect();
    serde_json::Value::Array(list).to_string()
}

/// Replaces `\c` with `with` wherever the backslash is not itself escaped (iTerm2
/// stringByReplacingEscapedChar:withString:).
fn replace_escaped(s: &str, c: char, with: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && chars.get(i + 1) == Some(&c) {
            let slashes = chars[..i].iter().rev().take_while(|&&x| x == '\\').count();
            if slashes % 2 == 0 {
                out.push_str(with);
                i += 2;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `\xHH` with an even run of backslashes before it becomes that character.
fn replace_hex(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && chars.get(i + 1) == Some(&'x') && i + 3 < chars.len() {
            let slashes = chars[..i].iter().rev().take_while(|&&x| x == '\\').count();
            if let (0, Some(h), Some(l)) = (slashes % 2, chars[i + 2].to_digit(16), chars[i + 3].to_digit(16)) {
                out.push(char::from_u32(h * 16 + l).unwrap_or('\u{fffd}'));
                i += 4;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn compatibility(s: &str) -> String {
    let s = replace_escaped(s, 'n', "\n");
    let s = replace_escaped(&s, 'e', "\x1b");
    let s = replace_escaped(&s, 'a', "\x07");
    replace_escaped(&s, 't', "\t")
}

fn common(s: &str) -> String {
    let mut s = s.to_string();
    for (c, with) in [('a', "\x07"), ('b', "\x08"), ('e', "\x1b"), ('n', "\n"), ('r', "\r"), ('t', "\t"), ('\\', "\\")] {
        s = replace_escaped(&s, c, with);
    }
    replace_hex(&s)
}

/// vim's special characters (iTerm2 stringByExpandingVimSpecialCharacters).
fn vim(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let digits = |from: usize, max: usize, ok: &dyn Fn(char) -> bool| chars[from..].iter().take(max).take_while(|c| ok(**c)).count();
    let octal = |c: char| ('0'..='7').contains(&c);
    while i < chars.len() {
        if chars[i] != '\\' || i + 1 >= chars.len() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let j = i + 1;
        let next = chars[j];
        let rest: String = chars[j..].iter().collect();
        let push = |out: &mut String, v: u32| out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
        // Octal: three digits, or one or two not followed by 0-8.
        let n = digits(j, 3, &octal);
        let after = chars.get(j + n).copied();
        if n == 3 || (n > 0 && !after.is_some_and(|c| ('0'..='8').contains(&c))) {
            let v = chars[j..j + n].iter().fold(0, |a, c| a * 8 + c.to_digit(8).unwrap());
            push(&mut out, v);
            i = j + n;
            continue;
        }
        if next == 'x' {
            let n = digits(j + 1, 2, &|c: char| c.is_ascii_hexdigit());
            let after = chars.get(j + 1 + n).copied();
            if n == 2 || (n == 1 && !after.is_some_and(|c| c.is_ascii_hexdigit())) {
                let v = u32::from_str_radix(&chars[j + 1..j + 1 + n].iter().collect::<String>(), 16).unwrap();
                push(&mut out, v);
                i = j + 1 + n;
                continue;
            }
        }
        if next == 'u' && digits(j + 1, 4, &|c: char| c.is_ascii_hexdigit()) == 4 {
            let v = u32::from_str_radix(&chars[j + 1..j + 5].iter().collect::<String>(), 16).unwrap();
            push(&mut out, v);
            i = j + 5;
            continue;
        }
        let simple = match next {
            'b' => Some("\x7f"),
            'e' => Some("\x1b"),
            'f' => Some("\x0c"),
            'n' => Some("\n"),
            'r' => Some("\r"),
            't' => Some("\t"),
            '\\' => Some("\\"),
            '"' => Some("\""),
            _ => None,
        };
        if let Some(t) = simple {
            out.push_str(t);
            i = j + 1;
            continue;
        }
        // \<C-x> and \<M-x>
        let b = rest.as_bytes();
        if b.len() >= 5 && b[0] == b'<' && (b[1] == b'C' || b[1] == b'M') && b[2] == b'-' && b[3].is_ascii_alphabetic() && b[4] == b'>' {
            let letter = b[3] as char;
            if b[1] == b'C' {
                out.push((letter.to_ascii_lowercase() as u8 - b'a' + 1) as char);
            } else {
                out.push('\x1b');
                out.push(letter);
            }
            i = j + 5;
            continue;
        }
        out.push('\\');
        i = j;
    }
    out
}

/// Applies a Send Text escaping mode (iTerm2 escapedText:mode:).
pub fn expand(text: &str, e: Escaping) -> String {
    match e {
        Escaping::None => text.to_string(),
        Escaping::Compatibility => compatibility(text),
        Escaping::Common => common(text),
        Escaping::Vim => vim(text),
        Escaping::VimAndCompatibility => compatibility(&vim(text)),
    }
}

/// Send Hex Code's parameter: space-separated hex numbers, one byte each. A part with
/// no hex digits is skipped, and a number reads up to its first non-hex character (strtol).
pub fn hex_bytes(text: &str) -> Vec<u8> {
    text.split(' ')
        .filter_map(|part| {
            let p = part.trim_start();
            let (neg, p) = p.strip_prefix('-').map_or((false, p), |r| (true, r));
            let p = p.strip_prefix("0x").or_else(|| p.strip_prefix("0X")).filter(|r| r.starts_with(|c: char| c.is_ascii_hexdigit())).unwrap_or(p);
            let digits: String = p.chars().take_while(char::is_ascii_hexdigit).collect();
            if digits.is_empty() {
                return None;
            }
            let v = i64::from_str_radix(&digits, 16).unwrap_or(i64::MAX);
            Some((if neg { -v } else { v }) as u8)
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arg {
    None,
    Text,
    VimText,
    Hex,
    Escape,
    Profile,
    MenuItem,
    Sequence,
    Setting,
    Other,
}

pub struct ActionInfo {
    pub code: i64,
    pub name: String,
    pub arg: Arg,
}

fn arg_of(code: i64) -> Arg {
    match code {
        10 => Arg::Escape,
        11 => Arg::Hex,
        12 => Arg::Text,
        38 | 76 => Arg::VimText,
        25 => Arg::MenuItem,
        26..=29 | 37 => Arg::Profile,
        66 => Arg::Sequence,
        75 => Arg::Setting,
        35 | 36 | 40 | 41 | 42 | 45..=48 | 60 | 63 | 64 | 65 | 72 | 73 => Arg::Other,
        _ => Arg::None,
    }
}

fn keymaps_groups() -> &'static Vec<(String, Vec<(i64, String)>)> {
    static GROUPS: OnceLock<Vec<(String, Vec<(i64, String)>)>> = OnceLock::new();
    GROUPS.get_or_init(|| {
        let file: toml::Table = include_str!("../schema/keymaps.toml").parse().expect("schema/keymaps.toml must parse");
        file["action_group"]
            .as_array()
            .expect("action_group is a list")
            .iter()
            .map(|g| {
                let items = g["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| (i[0].as_integer().unwrap(), i[1].as_str().unwrap().to_string()))
                    .collect();
                (g["name"].as_str().unwrap().to_string(), items)
            })
            .collect()
    })
}

/// iTerm2's action menu: group names and the codes in each, in its order.
pub fn groups() -> Vec<(&'static str, Vec<i64>)> {
    keymaps_groups().iter().map(|(g, items)| (g.as_str(), items.iter().map(|(c, _)| *c).collect())).collect()
}

/// Every action code, -1 to 76, with iTerm2's menu name.
pub fn actions() -> &'static [ActionInfo] {
    static LIST: OnceLock<Vec<ActionInfo>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut list: Vec<ActionInfo> = keymaps_groups()
            .iter()
            .flat_map(|(_, items)| items.iter())
            .map(|(code, name)| ActionInfo { code: *code, name: name.clone(), arg: arg_of(*code) })
            .collect();
        // Not in iTerm2's menu: no action, and the retired Instant Replay step.
        list.push(ActionInfo { code: -1, name: "No Action".into(), arg: Arg::None });
        list.push(ActionInfo { code: 14, name: "Unsupported Command".into(), arg: Arg::None });
        list.sort_by_key(|a| a.code);
        list
    })
}

pub fn info(code: i64) -> Option<&'static ActionInfo> {
    actions().iter().find(|a| a.code == code)
}

/// Whether an action works now, or the milestone that brings it.
pub fn status(code: i64) -> Status {
    match code {
        1 | 3 | 26 | 43 | 44 | 62 => Status::Later(7),
        15 => Status::Later(10),
        35 | 72 => Status::Later(6),
        45..=48 | 73 => Status::Later(4),
        40 => Status::Later(13),
        41 | 42 | 63 | 64 => Status::Later(9),
        60 => Status::Later(11),
        65 => Status::Later(8),
        _ => Status::Done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Status;

    fn b(version: i64, escaping: Option<i64>) -> Binding {
        Binding { action: 12, text: String::new(), label: String::new(), version, escaping, apply_mode: 0 }
    }

    #[test]
    fn every_action_code_has_a_name() {
        assert_eq!(actions().len(), 78); // -1 through 76
        assert_eq!(info(32).unwrap().name, "Cycle Tabs Forward");
        assert_eq!(info(14).unwrap().name, "Unsupported Command");
        assert!(groups().iter().any(|(g, items)| *g == "Send Keystrokes" && items.contains(&12)));
    }

    #[test]
    fn escaping_follows_the_version() {
        assert_eq!(b(0, None).escaping(), Escaping::Compatibility);
        assert_eq!(b(1, None).escaping(), Escaping::Common);
        assert_eq!(b(2, Some(3)).escaping(), Escaping::Vim);
        assert_eq!(b(2, Some(0)).vim_escaping(), Escaping::Vim);
        assert_eq!(b(0, None).vim_escaping(), Escaping::VimAndCompatibility);
    }

    #[test]
    fn expands_each_escaping_mode() {
        assert_eq!(expand(r"a\nb\x41", Escaping::Common), "a\nbA");
        assert_eq!(expand(r"a\\n", Escaping::Common), r"a\n");
        assert_eq!(expand(r"a\x41", Escaping::Compatibility), r"a\x41");
        assert_eq!(expand(r"\e\t", Escaping::Compatibility), "\x1b\t");
        assert_eq!(expand(r"\<C-a>\101\b", Escaping::Vim), "\x01A\x7f");
        assert_eq!(expand(r"\<M-x>é", Escaping::Vim), "\x1bx\u{e9}");
        assert_eq!(expand(r"\x4g", Escaping::Vim), "\x04g");
        assert_eq!(expand(r"\q", Escaping::Vim), r"\q");
        assert_eq!(expand(r"\n", Escaping::None), r"\n");
    }

    #[test]
    fn hex_codes_become_bytes() {
        assert_eq!(hex_bytes("0x1b 0x7f"), [0x1b, 0x7f]);
        assert_eq!(hex_bytes("15"), [0x15]);
        assert_eq!(hex_bytes(" 0x1  zz 0x2 1g"), [0x01, 0x02, 0x01]);
    }

    #[test]
    fn statuses_split_now_from_later() {
        for code in [0, 2, 4, 9, 10, 11, 12, 13, 16, 17, 18, 21, 23, 25, 27, 28, 29, 30, 31, 32, 33, 34, 37, 38, 39, 49, 52, 53, 56, 59, 61, 66, 67, 68, 69, 70, 74, 75, 76] {
            assert_eq!(status(code), Status::Done, "{code}");
        }
        assert_eq!(status(15), Status::Later(10));
        assert_eq!(status(65), Status::Later(8));
        assert_eq!(status(1), Status::Later(7));
        assert_eq!(status(45), Status::Later(4));
        assert_eq!(status(57), Status::Done);
    }

    #[test]
    fn reads_and_writes_a_stored_binding() {
        let v: toml::Value = toml::from_str("Action = 11\nText = \"0x15\"").unwrap();
        let b = Binding::from_value(&v).unwrap();
        assert_eq!((b.action, b.text.as_str(), b.version), (11, "0x15", 0));
        assert_eq!(Binding::from_value(&b.to_value()).unwrap(), b);
        let mut vim = b.clone();
        vim.version = 2;
        vim.escaping = Some(3);
        assert_eq!(vim.to_value()["Escaping"].as_integer(), Some(3));
    }

    #[test]
    fn a_sequence_holds_bindings() {
        let seq = sequence(r#"[{"Action":12,"Text":"ls\\n","Version":1},{"Action":13}]"#);
        assert_eq!(seq.len(), 2);
        assert_eq!((seq[0].action, seq[0].text.as_str(), seq[0].escaping()), (12, "ls\\n", Escaping::Common));
        assert!(sequence("not json").is_empty());
        assert_eq!(sequence(&sequence_text(&seq)), seq);
    }
}
