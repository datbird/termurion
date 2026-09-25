//! tmux's prefix-table key bindings as Leader bindings (iTerm2's TmuxController
//! handleListKeys and iTermKeystroke withTmuxKey): `prefix c` becomes Leader then c, which
//! sends `new-window` as a tmux command (key action 65, "Send tmux Command").

use crate::keystroke::{self, Keystroke};

/// Commands that make no sense from a key in control mode (iTerm2's list).
const FORBIDDEN: [&str; 25] = [
    "bind-key",
    "choose-buffer",
    "choose-client",
    "choose-tree",
    "clear-history",
    "clock-mode",
    "command-prompt",
    "confirm-before",
    "copy-mode",
    "customize-mode",
    "display-menu",
    "display-message",
    "display-panes",
    "display-popup",
    "find-window",
    "list-buffers",
    "list-clients",
    "list-commands",
    "list-keys",
    "list-panes",
    "list-sessions",
    "list-windows",
    "show-buffer",
    "show-messages",
    "unbind-key",
];

/// The prefix-table bindings of a `list-keys` reply: (key, command).
pub fn prefix_bindings(reply: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in reply.split('\n') {
        // iTerm2 splits on runs of spaces.
        let args: Vec<&str> = line.split(' ').filter(|a| !a.is_empty()).collect();
        let (mut key, mut command, mut table) = (None, None, None::<String>);
        let mut skip = 0;
        let mut flag: Option<&str> = None;
        let mut i = 1;
        while i < args.len() {
            if skip > 0 {
                if flag == Some("T") {
                    table = Some(args[i].to_string());
                }
                skip -= 1;
                i += 1;
                continue;
            }
            flag = None;
            if args[i].starts_with('-') && args[i].len() > 1 {
                let f = &args[i][1..];
                flag = Some(f);
                skip = usize::from(f == "N" || f == "T");
                i += 1;
                continue;
            }
            if key.is_none() {
                key = Some(args[i].to_string());
                i += 1;
                continue;
            }
            command = Some(args[i..].join(" "));
            break;
        }
        let (Some(key), Some(command)) = (key, command) else { continue };
        let first = command.split(' ').next().unwrap_or("");
        if table.as_deref() == Some("prefix") && !FORBIDDEN.contains(&first) {
            out.push((key, command));
        }
    }
    out
}

/// A tmux key name as a Leader keystroke, or None for one iTerm2 does not know.
pub fn keystroke(escaped: &str) -> Option<Keystroke> {
    // iTerm2 drops each backslash and keeps the character after it.
    let mut key = String::new();
    let mut chars = escaped.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                key.push(n);
            }
        } else {
            key.push(c);
        }
    }
    let k: Vec<char> = key.chars().collect();
    let mut flags = keystroke::LEADER;
    let mut i = 0;
    while k.len() > i + 1 && k[i + 1] == '-' {
        flags |= match k[i] {
            'C' | 'c' => keystroke::CONTROL,
            'S' | 's' => keystroke::SHIFT,
            'M' | 'm' => keystroke::OPTION,
            _ => return None,
        };
        i += 2;
    }
    let name: String = k[i..].iter().collect();
    if k.len() == i + 1 {
        // One character: only the typed character is known.
        let c = k[i] as u32;
        return Some(Keystroke { ch: c, flags, keycode: None, modified: c, char_is_modified: true });
    }
    let f = |n: u32| keystroke::F1 + n - 1;
    let c = match name.as_str() {
        n if n.starts_with('F') && n[1..].parse::<u32>().is_ok_and(|x| (1..=20).contains(&x)) => f(n[1..].parse().ok()?),
        "IC" => keystroke::INSERT,
        "DC" => keystroke::DELETE,
        "Home" => keystroke::HOME,
        "End" => keystroke::END,
        "NPage" | "PageDown" | "PgDn" => keystroke::PAGE_DOWN,
        "PPage" | "PageUp" | "PgUp" => keystroke::PAGE_UP,
        "Tab" => '\t' as u32,
        "Space" => ' ' as u32,
        "BSpace" => 8,
        "Enter" => '\n' as u32,
        "Escape" => 27,
        "Up" => keystroke::UP,
        "Down" => keystroke::DOWN,
        "Left" => keystroke::LEFT,
        "Right" => keystroke::RIGHT,
        "KP/" => '/' as u32,
        "KP*" => '*' as u32,
        "KP-" => '-' as u32,
        "KP+" => '+' as u32,
        "KP." => '.' as u32,
        "KPEnter" => '\n' as u32,
        n if n.len() == 3 && n.starts_with("KP") && n.as_bytes()[2].is_ascii_digit() => n.as_bytes()[2] as u32,
        _ => return None,
    };
    Some(Keystroke::new(c, flags, None, c))
}

/// The bindings as a key map layered over the profile's (iTerm2's shared key-mapping
/// overrides): each keystroke runs "Send tmux Command" with the tmux command.
pub fn key_map(reply: &str) -> toml::Table {
    let mut map = toml::Table::new();
    for (key, command) in prefix_bindings(reply) {
        if let Some(k) = keystroke(&key) {
            // iTermSendTextEscapingNone: the command goes as written.
            let b = crate::keyaction::Binding { escaping: Some(0), ..crate::keyaction::Binding::new(65, &command) };
            map.insert(k.serialized(), b.to_value());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_bindings_follow_iterm2() {
        let reply = "bind-key    -T prefix       C-c              new-window\nbind-key    -T prefix       :                command-prompt\nbind-key    -T root         C-b              send-prefix\nbind-key -N \"Split\" -T prefix % split-window -h\nbind-key    -T prefix       \\;               last-pane";
        let b = prefix_bindings(reply);
        assert_eq!(b, vec![("C-c".to_string(), "new-window".to_string()), ("%".into(), "split-window -h".into()), ("\\;".into(), "last-pane".into())], "prefix table only, no command prompt");
        let k = keystroke("C-c").unwrap();
        assert_eq!(k.flags, keystroke::LEADER | keystroke::CONTROL);
        assert_eq!((k.modified, k.char_is_modified), ('c' as u32, true));
        assert_eq!(keystroke("\\;").unwrap().modified, ';' as u32, "backslashes escape");
        assert_eq!(keystroke("M-Up").unwrap().ch, keystroke::UP);
        assert_eq!(keystroke("F12").unwrap().ch, keystroke::F1 + 11);
        assert_eq!(keystroke("KP5").unwrap().ch, '5' as u32);
        assert!(keystroke("X-a").is_none() && keystroke("Nope").is_none());
        let map = key_map(reply);
        assert_eq!(map.len(), 3);
        let b = crate::keyaction::Binding::from_value(&map[&keystroke("C-c").unwrap().serialized()]).unwrap();
        assert_eq!((b.action, b.text.as_str()), (65, "new-window"));
    }
}
