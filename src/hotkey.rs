//! Hotkey windows (iTerm2's profile hotkeys): a profile's hotkey settings, the Windows chord
//! for a stored shortcut, siblings (hotkeys that toggle together), what a press does, and the
//! "Hotkey Window" profile that Create a Dedicated Hotkey Window... makes.

use toml::Value;

use crate::keystroke::{COMMAND, CONTROL, OPTION, SHIFT};
use crate::profile::Profile;

/// RegisterHotKey's modifier bits.
pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

/// A stored shortcut, iTerm2's form: the macOS key code, NSEvent modifier flags, and the
/// characters (empty `chars_ignoring` means no shortcut).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub keycode: u16,
    pub flags: u64,
    pub chars: String,
    pub chars_ignoring: String,
}

/// A profile's hotkey window settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub has_hotkey: bool,
    pub primary: Option<Shortcut>,
    /// The double-tap key: 0 Control, 1 Shift, 2 Option (Alt), 3 Command (Win).
    pub double_tap: Option<u8>,
    pub alternates: Vec<Shortcut>,
    pub auto_hides: bool,
    pub reopens: bool,
    pub animates: bool,
    pub floats: bool,
}

/// The key of a Windows chord: a PC scan code (0xE0xx for extended keys), or a character
/// when the key code is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChordKey {
    Scan(u32),
    Char(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: ChordKey,
    pub mods: u32,
}

impl Shortcut {
    /// An entry of `HotKey Alternate Shortcuts` (`keyCode`, `modifiers`, `characters`,
    /// `charactersIgnoringModifiers`).
    pub fn from_value(v: &Value) -> Option<Shortcut> {
        let t = v.as_table()?;
        let s = |k: &str| t.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
        let n = |k: &str| t.get(k).and_then(|x| x.as_integer()).unwrap_or(0);
        let out = Shortcut { keycode: n("keyCode").clamp(0, u16::MAX as i64) as u16, flags: n("modifiers").max(0) as u64, chars: s("characters"), chars_ignoring: s("charactersIgnoringModifiers") };
        (!out.chars_ignoring.is_empty()).then_some(out)
    }

    pub fn to_value(&self) -> Value {
        let mut t = toml::Table::new();
        t.insert("keyCode".into(), Value::Integer(self.keycode as i64));
        t.insert("modifiers".into(), Value::Integer(self.flags as i64));
        t.insert("characters".into(), Value::String(self.chars.clone()));
        t.insert("charactersIgnoringModifiers".into(), Value::String(self.chars_ignoring.clone()));
        Value::Table(t)
    }
}

impl Prefs {
    pub fn from_profile(p: &Profile) -> Prefs {
        let ignoring = p.string("HotKey Characters Ignoring Modifiers");
        let primary = (!ignoring.is_empty()).then(|| Shortcut {
            keycode: p.int("HotKey Key Code").clamp(0, u16::MAX as i64) as u16,
            flags: p.int("HotKey Modifier Flags").max(0) as u64,
            chars: p.string("HotKey Characters"),
            chars_ignoring: ignoring,
        });
        Prefs {
            has_hotkey: p.bool("Has Hotkey"),
            primary,
            double_tap: p.bool("HotKey Activated By Modifier").then(|| p.int("HotKey Modifier Activation").clamp(0, 3) as u8),
            alternates: p.get("HotKey Alternate Shortcuts").and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().filter_map(Shortcut::from_value).collect(),
            auto_hides: p.bool("HotKey Window AutoHides"),
            reopens: p.bool("HotKey Window Reopens On Activation"),
            animates: p.bool("HotKey Window Animates"),
            floats: p.bool("HotKey Window Floats"),
        }
    }

    /// Some hotkey is set: a shortcut, a double tap, or an additional shortcut.
    pub fn assigned(&self) -> bool {
        self.primary.is_some() || self.double_tap.is_some() || !self.alternates.is_empty()
    }

    /// The profile's hotkey is on and set.
    pub fn active(&self) -> bool {
        self.has_hotkey && self.assigned()
    }

    /// The primary shortcut, then the additional ones.
    pub fn shortcuts(&self) -> Vec<&Shortcut> {
        self.primary.iter().chain(self.alternates.iter()).collect()
    }
}

/// The PC scan code of the key at a macOS key code's position (US layout positions).
pub fn scan_code(mac: u16) -> Option<u32> {
    Some(match mac {
        0x00 => 0x1E, 0x01 => 0x1F, 0x02 => 0x20, 0x03 => 0x21, 0x04 => 0x23, 0x05 => 0x22, 0x06 => 0x2C, 0x07 => 0x2D,
        0x08 => 0x2E, 0x09 => 0x2F, 0x0A => 0x56, 0x0B => 0x30, 0x0C => 0x10, 0x0D => 0x11, 0x0E => 0x12, 0x0F => 0x13,
        0x10 => 0x15, 0x11 => 0x14, 0x12 => 0x02, 0x13 => 0x03, 0x14 => 0x04, 0x15 => 0x05, 0x16 => 0x07, 0x17 => 0x06,
        0x18 => 0x0D, 0x19 => 0x0A, 0x1A => 0x08, 0x1B => 0x0C, 0x1C => 0x09, 0x1D => 0x0B, 0x1E => 0x1B, 0x1F => 0x18,
        0x20 => 0x16, 0x21 => 0x1A, 0x22 => 0x17, 0x23 => 0x19, 0x24 => 0x1C, 0x25 => 0x26, 0x26 => 0x24, 0x27 => 0x28,
        0x28 => 0x25, 0x29 => 0x27, 0x2A => 0x2B, 0x2B => 0x33, 0x2C => 0x35, 0x2D => 0x31, 0x2E => 0x32, 0x2F => 0x34,
        0x30 => 0x0F, 0x31 => 0x39, 0x32 => 0x29, 0x33 => 0x0E, 0x35 => 0x01,
        // The keypad.
        0x41 => 0x53, 0x43 => 0x37, 0x45 => 0x4E, 0x47 => 0x45, 0x4B => 0xE035, 0x4C => 0xE01C, 0x4E => 0x4A, 0x51 => 0x59,
        0x52 => 0x52, 0x53 => 0x4F, 0x54 => 0x50, 0x55 => 0x51, 0x56 => 0x4B, 0x57 => 0x4C, 0x58 => 0x4D, 0x59 => 0x47,
        0x5B => 0x48, 0x5C => 0x49,
        // Function and navigation keys.
        0x7A => 0x3B, 0x78 => 0x3C, 0x63 => 0x3D, 0x76 => 0x3E, 0x60 => 0x3F, 0x61 => 0x40, 0x62 => 0x41, 0x64 => 0x42,
        0x65 => 0x43, 0x6D => 0x44, 0x67 => 0x57, 0x6F => 0x58, 0x69 => 0x64, 0x6B => 0x65, 0x71 => 0x66, 0x6A => 0x67,
        0x40 => 0x68, 0x4F => 0x69, 0x50 => 0x6A, 0x5A => 0x6B,
        0x72 => 0xE052, 0x73 => 0xE047, 0x74 => 0xE049, 0x75 => 0xE053, 0x77 => 0xE04F, 0x79 => 0xE051,
        0x7B => 0xE04B, 0x7C => 0xE04D, 0x7D => 0xE050, 0x7E => 0xE048,
        _ => return None,
    })
}

/// The Windows chord for a stored shortcut: Command is Win, Option is Alt.
pub fn chord(s: &Shortcut) -> Option<Chord> {
    let first = s.chars_ignoring.chars().next()?;
    let key = scan_code(s.keycode).map_or(ChordKey::Char(first), ChordKey::Scan);
    let mut mods = 0;
    for (flag, m) in [(COMMAND, MOD_WIN), (OPTION, MOD_ALT), (CONTROL, MOD_CONTROL), (SHIFT, MOD_SHIFT)] {
        if s.flags & flag != 0 {
            mods |= m;
        }
    }
    Some(Chord { key, mods })
}

/// Profiles whose hotkeys toggle together with `guid`'s: the same shortcuts, or the same
/// double-tap key (iTerm2's siblings). `guid` itself is not in the list.
pub fn siblings(guid: &str, all: &[(String, Prefs)]) -> Vec<String> {
    let Some((_, me)) = all.iter().find(|(g, _)| g == guid) else { return Vec::new() };
    let keys = |p: &Prefs| {
        let mut v: Vec<(u16, u64)> = p.shortcuts().iter().map(|s| (s.keycode, s.flags)).collect();
        v.sort();
        v
    };
    let mine = keys(me);
    all.iter()
        .filter(|(g, p)| g != guid && p.active())
        .filter(|(_, p)| (!mine.is_empty() && keys(p) == mine) || (me.double_tap.is_some() && p.double_tap == me.double_tap))
        .map(|(g, _)| g.clone())
        .collect()
}

/// Whether another active hotkey profile uses this one's primary shortcut, and its
/// double-tap key (the sheet's two warnings).
pub fn conflicts(guid: &str, prefs: &Prefs, all: &[(String, Prefs)]) -> (bool, bool) {
    let others = || all.iter().filter(|(g, p)| g != guid && p.active()).map(|(_, p)| p);
    let chord = prefs.primary.as_ref().is_some_and(|me| others().any(|p| p.shortcuts().iter().any(|s| (s.keycode, s.flags) == (me.keycode, me.flags))));
    let tap = prefs.double_tap.is_some() && others().any(|p| p.double_tap == prefs.double_tap);
    (chord, tap)
}

/// A hotkey window's state at a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Win {
    /// Shown (not hidden or never made).
    pub open: bool,
    pub focused: bool,
    /// Pinned: it does not hide when focus leaves.
    pub pinned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Show,
    Hide,
    Focus,
    Nothing,
}

/// What a press does to each window of a sibling group (iTerm2's hotKeyPressedWithSiblings
/// and handleHotkeyPress).
pub fn press(group: &[Win]) -> Vec<Act> {
    let all_open = group.iter().all(|w| w.open);
    let any_focused = group.iter().any(|w| w.focused);
    group
        .iter()
        .map(|w| match (w.open, all_open) {
            (false, _) => Act::Show,
            // Some sibling is hidden: the press shows it and leaves this one.
            (true, false) => Act::Nothing,
            (true, true) if any_focused => Act::Hide,
            // A pinned window open behind another: bring it forward instead.
            (true, true) if w.pinned => Act::Focus,
            (true, true) => Act::Hide,
        })
        .collect()
}

/// "Hotkey Window", then "Hotkey Window (1)", and so on.
pub fn dedicated_name(names: &[String]) -> String {
    let mut name = "Hotkey Window".to_string();
    let mut n = 1;
    while names.contains(&name) {
        name = format!("Hotkey Window ({n})");
        n += 1;
    }
    name
}

/// Create a Dedicated Hotkey Window...: the default profile tuned for a hotkey window, with
/// the sheet's hotkey keys (`hotkey`) and a fresh `guid`.
pub fn dedicated_profile(default: &Profile, names: &[String], hotkey: &toml::Table, guid: &str) -> Profile {
    let mut p = default.clone();
    let set = |p: &mut Profile, k: &str, v: Value| p.set(k, v);
    set(&mut p, "Window Type", Value::Integer(crate::frames::TOP_PERCENT));
    set(&mut p, "Width Percentage", Value::Float(100.0));
    set(&mut p, "Height Percentage", Value::Float(-1.0));
    set(&mut p, "Rows", Value::Integer(25));
    set(&mut p, "Transparency", Value::Float(0.3));
    set(&mut p, "Initial Use Transparency", Value::Boolean(true));
    set(&mut p, "Blend", Value::Float(0.5));
    set(&mut p, "Blur Radius", Value::Float(2.0));
    set(&mut p, "Blur", Value::Boolean(true));
    set(&mut p, "Screen", Value::Integer(-1));
    set(&mut p, "Space", Value::Integer(-1));
    set(&mut p, "Shortcut", Value::String(String::new()));
    set(&mut p, "Name", Value::String(dedicated_name(names)));
    set(&mut p, "Default Bookmark", Value::String("No".into()));
    set(&mut p, "Guid", Value::String(guid.into()));
    p.values.remove("Tags");
    // Win+T opens a new tab of this profile (iTerm2 binds Cmd+T to New Tab with Profile).
    let mut map = p.get("Keyboard Map").and_then(|v| v.as_table().cloned()).unwrap_or_default();
    let t = crate::keystroke::Keystroke::new('t' as u32, COMMAND, Some(0x11), 't' as u32);
    map.insert(t.serialized(), crate::keyaction::Binding::new(27, guid).to_value());
    set(&mut p, "Keyboard Map", Value::Table(map));
    for (k, v) in hotkey {
        p.set(k, v.clone());
    }
    p
}

/// The question before making another hotkey profile: (title, text, buttons).
pub fn create_warning(names: &[String]) -> (String, String, Vec<&'static str>) {
    let quoted: Vec<String> = names.iter().map(|n| format!("\u{201c}{n}\u{201d}")).collect();
    let (text, buttons) = match quoted.as_slice() {
        [one] => (format!("You already have a Profile with a Hotkey Window named {one}."), vec!["OK", "Configure Existing Profile", "Cancel"]),
        [a, b] => (format!("You already have Profiles with Hotkey Windows named {a} and {b}."), vec!["OK", "Cancel"]),
        many => {
            let (last, rest) = many.split_last().expect("at least one name");
            (format!("You already have Profiles with Hotkey Windows named {}, and {last}.", rest.join(", ")), vec!["OK", "Cancel"])
        }
    };
    ("Add Another Hotkey Window Profile?".into(), text, buttons)
}

/// Whether Tandem keeps running with `regular` terminal windows and `hotkey` hotkey windows
/// (shown or hidden): a made hotkey window keeps it alive, as in iTerm2.
pub fn keeps_running(regular: usize, hotkey: usize) -> bool {
    regular + hotkey > 0
}

/// A shortcut that "smells accidental" (iTerm2): no Win (Cmd) and no Control, and a key
/// people type (a letter, digit, punctuation, Space, Delete, Tab, Return, Escape, or a
/// navigation key). Function keys never smell.
pub fn smells_accidental(s: &Shortcut) -> bool {
    if s.flags & (COMMAND | CONTROL) != 0 {
        return false;
    }
    let Some(c) = s.chars_ignoring.chars().next() else { return false };
    let navigation = matches!(c as u32, 0xF700..=0xF703 | 0xF727..=0xF72D);
    let function = (0xF704..=0xF726).contains(&(c as u32));
    !function && (navigation || c.is_alphanumeric() || c.is_ascii_punctuation() || matches!(c, ' ' | '\u{7f}' | '\t' | '\r' | '\u{1b}' | '\u{8}'))
}

/// The question for such a shortcut: (heading, text).
pub fn confirm_text(label: &str) -> (&'static str, String) {
    ("Confirm Shortcut", format!("Are you sure you want to use \u{201c}{label}\u{201d} as a hotkey? This looks like a commonly used keystroke."))
}

/// What the app hotkey (Show/hide all windows) does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppAct {
    HideAll,
    ShowAll,
    OpenWindow,
}

/// iTerm2's app hotkey: Tandem in front hides it; otherwise it comes forward, with a new
/// window when it has none.
pub fn app_press(active: bool, windows: usize) -> AppAct {
    match (active, windows) {
        (true, _) => AppAct::HideAll,
        (false, 0) => AppAct::OpenWindow,
        (false, _) => AppAct::ShowAll,
    }
}

/// The sibling groups a press reaches, each once (iTerm2's `handled` set): `pressed` are
/// the profiles whose hotkey matched.
pub fn groups_for(pressed: &[String], all: &[(String, Prefs)]) -> Vec<Vec<String>> {
    let mut handled: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for g in pressed {
        if handled.contains(g) || !all.iter().any(|(x, p)| x == g && p.active()) {
            continue;
        }
        let mut group = vec![g.clone()];
        group.extend(siblings(g, all));
        handled.extend(group.iter().cloned());
        out.push(group);
    }
    out
}

/// Hotkey windows whose profile no longer has an active hotkey (deleted, or turned off):
/// they must become regular windows, or nothing could show them again.
pub fn stale_windows(windows: &[String], all: &[(String, Prefs)]) -> Vec<String> {
    windows.iter().filter(|g| !all.iter().any(|(x, p)| x == *g && p.active())).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short(keycode: u16, flags: u64, ch: &str) -> Shortcut {
        Shortcut { keycode, flags, chars: ch.into(), chars_ignoring: ch.into() }
    }

    fn prefs() -> Prefs {
        Prefs { has_hotkey: true, primary: None, double_tap: None, alternates: vec![], auto_hides: true, reopens: false, animates: true, floats: true }
    }

    fn profile(pairs: &[(&str, Value)]) -> Profile {
        let mut p = Profile { values: Default::default() };
        for (k, v) in pairs {
            p.values.insert(k.to_string(), v.clone());
        }
        p
    }

    #[test]
    fn has_hotkey_counts_every_key() {
        let p = profile(&[("Has Hotkey", Value::Boolean(true)), ("HotKey Key Code", Value::Integer(0x31)), ("HotKey Modifier Flags", Value::Integer(OPTION as i64)), ("HotKey Characters Ignoring Modifiers", Value::String(" ".into()))]);
        let f = Prefs::from_profile(&p);
        assert_eq!(f.primary, Some(Shortcut { keycode: 0x31, flags: OPTION, chars: String::new(), chars_ignoring: " ".into() }));
        assert!(f.active() && f.auto_hides && f.animates && f.floats && !f.reopens, "iTerm2's defaults");
        let tap = Prefs::from_profile(&profile(&[("Has Hotkey", Value::Boolean(true)), ("HotKey Activated By Modifier", Value::Boolean(true)), ("HotKey Modifier Activation", Value::Integer(3))]));
        assert_eq!((tap.double_tap, tap.active()), (Some(3), true));
        let mut alt = toml::Table::new();
        alt.insert("keyCode".into(), Value::Integer(0x7A));
        alt.insert("modifiers".into(), Value::Integer(0));
        alt.insert("characters".into(), Value::String("\u{f704}".into()));
        alt.insert("charactersIgnoringModifiers".into(), Value::String("\u{f704}".into()));
        let alts = Prefs::from_profile(&profile(&[("Has Hotkey", Value::Boolean(true)), ("HotKey Alternate Shortcuts", Value::Array(vec![Value::Table(alt.clone())]))]));
        assert!(alts.active() && alts.primary.is_none());
        assert_eq!(alts.alternates[0].to_value(), Value::Table(alt));
        assert!(!Prefs::from_profile(&profile(&[("Has Hotkey", Value::Boolean(true))])).active(), "on, but nothing set");
        let off = Prefs::from_profile(&p.clone().tap_set("Has Hotkey", Value::Boolean(false)));
        assert!(off.assigned() && !off.active());
    }

    trait TapSet {
        fn tap_set(self, k: &str, v: Value) -> Self;
    }

    impl TapSet for Profile {
        fn tap_set(mut self, k: &str, v: Value) -> Self {
            self.values.insert(k.into(), v);
            self
        }
    }

    #[test]
    fn chords_map_to_windows_keys() {
        assert_eq!(chord(&short(0x31, OPTION, " ")), Some(Chord { key: ChordKey::Scan(0x39), mods: MOD_ALT }), "Option+Space is Alt+Space");
        assert_eq!(chord(&short(0x00, COMMAND | SHIFT, "a")), Some(Chord { key: ChordKey::Scan(0x1E), mods: MOD_WIN | MOD_SHIFT }), "key code 0 is A");
        assert_eq!(chord(&short(0x32, CONTROL, "`")), Some(Chord { key: ChordKey::Scan(0x29), mods: MOD_CONTROL }));
        assert_eq!(chord(&short(0x7E, CONTROL, "\u{f700}")).map(|c| c.key), Some(ChordKey::Scan(0xE048)), "Up is extended");
        assert_eq!(chord(&short(0x7A, 0, "\u{f704}")).map(|c| c.key), Some(ChordKey::Scan(0x3B)), "F1");
        assert_eq!(chord(&short(0x3F, 0, "x")).map(|c| c.key), Some(ChordKey::Char('x')), "an unknown key code falls to the character");
        assert_eq!(chord(&short(0, 0, "")), None, "nothing set");
        assert_eq!((scan_code(0x24), scan_code(0x4C)), (Some(0x1C), Some(0xE01C)), "Return and keypad Enter");
    }

    #[test]
    fn siblings_share_a_chord() {
        let a = Prefs { primary: Some(short(0x31, OPTION, " ")), ..prefs() };
        let b = a.clone();
        let c = Prefs { primary: Some(short(0x32, CONTROL, "`")), ..prefs() };
        let d = Prefs { double_tap: Some(0), ..prefs() };
        let e = Prefs { double_tap: Some(0), primary: Some(short(1, 0, "s")), ..prefs() };
        let all = vec![("a".to_string(), a.clone()), ("b".into(), b), ("c".into(), c), ("d".into(), d.clone()), ("e".into(), e)];
        assert_eq!(siblings("a", &all), vec!["b".to_string()]);
        assert!(siblings("c", &all).is_empty());
        assert_eq!(siblings("d", &all), vec!["e".to_string()], "the same double-tap key");
        let (chord_taken, tap_taken) = conflicts("a", &a, &all);
        assert!(chord_taken && !tap_taken);
        assert_eq!(conflicts("d", &d, &all), (false, true));
        let off = vec![("x".to_string(), Prefs { has_hotkey: false, ..a.clone() })];
        assert_eq!(conflicts("a", &a, &off), (false, false), "a profile with its hotkey off does not count");
    }

    #[test]
    fn toggle_follows_iterm2() {
        let w = |open, focused, pinned| Win { open, focused, pinned };
        assert_eq!(press(&[w(false, false, false)]), vec![Act::Show]);
        assert_eq!(press(&[w(true, true, false)]), vec![Act::Hide]);
        assert_eq!(press(&[w(true, false, false)]), vec![Act::Hide], "open, not focused, auto-hiding: hide");
        assert_eq!(press(&[w(true, false, true)]), vec![Act::Focus], "pinned and open elsewhere: focus it");
        assert_eq!(press(&[w(true, true, false), w(false, false, false)]), vec![Act::Nothing, Act::Show], "one sibling hidden: show it, leave the open one");
        assert_eq!(press(&[w(true, true, false), w(true, false, true)]), vec![Act::Hide, Act::Hide], "all open and one focused: hide all");
    }

    #[test]
    fn dedicated_profile_clone() {
        let names = vec!["Default".to_string(), "Hotkey Window".into(), "Hotkey Window (1)".into()];
        assert_eq!(dedicated_name(&names[..1]), "Hotkey Window");
        assert_eq!(dedicated_name(&names), "Hotkey Window (2)");
        let default = profile(&[("Name", Value::String("Default".into())), ("Guid", Value::String("D".into())), ("Tags", Value::Array(vec![])), ("Default Bookmark", Value::String("Yes".into())), ("Shortcut", Value::String("A".into()))]);
        let mut hk = toml::Table::new();
        hk.insert("Has Hotkey".into(), Value::Boolean(true));
        hk.insert("HotKey Activated By Modifier".into(), Value::Boolean(true));
        let p = dedicated_profile(&default, &names[..1], &hk, "NEW");
        let get = |k: &str| p.values.get(k).cloned();
        assert_eq!(get("Name"), Some(Value::String("Hotkey Window".into())));
        assert_eq!(get("Guid"), Some(Value::String("NEW".into())));
        assert_eq!(get("Window Type"), Some(Value::Integer(2)));
        assert_eq!(get("Width Percentage"), Some(Value::Float(100.0)));
        assert_eq!(get("Height Percentage"), Some(Value::Float(-1.0)));
        assert_eq!(get("Rows"), Some(Value::Integer(25)));
        assert_eq!(get("Transparency"), Some(Value::Float(0.3)));
        assert_eq!(get("Initial Use Transparency"), Some(Value::Boolean(true)));
        assert_eq!(get("Blend"), Some(Value::Float(0.5)));
        assert_eq!(get("Blur Radius"), Some(Value::Float(2.0)));
        assert_eq!(get("Blur"), Some(Value::Boolean(true)));
        assert_eq!(get("Screen"), Some(Value::Integer(-1)));
        assert_eq!(get("Space"), Some(Value::Integer(-1)));
        assert_eq!(get("Shortcut"), Some(Value::String(String::new())));
        assert_eq!(get("Default Bookmark"), Some(Value::String("No".into())));
        assert_eq!(get("Tags"), None);
        assert_eq!(get("Has Hotkey"), Some(Value::Boolean(true)));
        let map = get("Keyboard Map").and_then(|m| m.as_table().cloned()).unwrap();
        let b = crate::keyaction::Binding::from_value(&map["0x74-0x100000-0x11"]).unwrap();
        assert_eq!((b.action, b.text.as_str()), (27, "NEW"), "Win+T opens a tab of the new profile");
    }

    #[test]
    fn a_hotkey_window_keeps_tandem_running() {
        assert!(keeps_running(0, 1), "only a hidden hotkey window is left");
        assert!(keeps_running(1, 0));
        assert!(!keeps_running(0, 0));
    }

    #[test]
    fn accidental_keystrokes_ask() {
        assert!(smells_accidental(&short(0x31, OPTION, " ")), "Alt+Space");
        assert!(smells_accidental(&short(0x00, SHIFT, "a")), "Shift+A");
        assert!(smells_accidental(&short(0x7E, OPTION, "\u{f700}")), "Alt+Up");
        assert!(smells_accidental(&short(0x35, 0, "\u{1b}")), "Escape");
        assert!(!smells_accidental(&short(0x32, CONTROL, "`")), "Control");
        assert!(!smells_accidental(&short(0x00, COMMAND, "a")), "Win");
        assert!(!smells_accidental(&short(0x6F, 0, "\u{f70f}")), "F12");
        assert!(!smells_accidental(&short(0x6F, OPTION, "\u{f70f}")), "Alt+F12");
        let (heading, text) = confirm_text("Alt+Space");
        assert_eq!(heading, "Confirm Shortcut");
        assert_eq!(text, "Are you sure you want to use \u{201c}Alt+Space\u{201d} as a hotkey? This looks like a commonly used keystroke.");
    }

    #[test]
    fn app_hotkey_follows_iterm2() {
        assert_eq!(app_press(true, 2), AppAct::HideAll);
        assert_eq!(app_press(false, 2), AppAct::ShowAll);
        assert_eq!(app_press(false, 0), AppAct::OpenWindow);
        assert_eq!(app_press(true, 0), AppAct::HideAll, "in front with only a hotkey window up");
    }

    #[test]
    fn one_press_toggles_each_group_once() {
        let a = Prefs { primary: Some(short(0x31, OPTION, " ")), ..prefs() };
        let c = Prefs { primary: Some(short(0x32, CONTROL, "`")), ..prefs() };
        let all = vec![("a".to_string(), a.clone()), ("b".into(), a), ("c".into(), c)];
        assert_eq!(groups_for(&["a".to_string(), "b".into()], &all), vec![vec!["a".to_string(), "b".into()]], "siblings fire together once");
        assert_eq!(groups_for(&["a".to_string(), "c".into()], &all).len(), 2);
        assert!(groups_for(&["gone".to_string()], &all).is_empty());
    }

    #[test]
    fn stale_hotkey_windows() {
        let on = Prefs { primary: Some(short(0x31, OPTION, " ")), ..prefs() };
        let all = vec![("a".to_string(), on.clone()), ("b".into(), Prefs { has_hotkey: false, ..on })];
        assert_eq!(stale_windows(&["a".to_string(), "b".into(), "deleted".into()], &all), vec!["b".to_string(), "deleted".into()]);
    }

    #[test]
    fn create_warning_text() {
        let (title, text, buttons) = create_warning(&["Quake".to_string()]);
        assert_eq!(title, "Add Another Hotkey Window Profile?");
        assert_eq!(text, "You already have a Profile with a Hotkey Window named \u{201c}Quake\u{201d}.");
        assert_eq!(buttons, vec!["OK", "Configure Existing Profile", "Cancel"]);
        let (_, text, buttons) = create_warning(&["A".to_string(), "B".into(), "C".into()]);
        assert_eq!(text, "You already have Profiles with Hotkey Windows named \u{201c}A\u{201d}, \u{201c}B\u{201d}, and \u{201c}C\u{201d}.");
        assert_eq!(buttons, vec!["OK", "Cancel"]);
        let (_, two, _) = create_warning(&["A".to_string(), "B".into()]);
        assert_eq!(two, "You already have Profiles with Hotkey Windows named \u{201c}A\u{201d} and \u{201c}B\u{201d}.");
    }
}
