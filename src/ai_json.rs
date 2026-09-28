//! JSON the way iTerm2's request builders write it: the sorted-key compact encoder (Swift's
//! `JSONEncoder` with `.sortedKeys`) and the byte-level splice that drops a chat's frozen history
//! into a request's array (`AITerm/JSONArraySplice.swift`). Pure.

use serde_json::Value;

/// Compact JSON with every object's keys sorted by their bytes, at every level. Unlike Swift's
/// encoder it does not escape "/" (Decision 2).
pub fn sorted(v: &Value) -> String {
    let mut out = String::new();
    write_sorted(v, &mut out);
    out
}

fn write_sorted(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write_sorted(&map[k], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_sorted(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// `JSONArraySplice` on a whole array: `elements` (array elements joined by commas, without the
/// brackets) inserted after the first `after` elements of `array_json`, or at its end when it
/// has fewer. Every other byte is kept. Empty `elements`, or input that is not an array, comes
/// back unchanged.
pub fn splice(array_json: &[u8], after: usize, elements: &[u8]) -> Vec<u8> {
    let open = array_json.iter().position(|b| !b.is_ascii_whitespace());
    match open.filter(|&i| array_json[i] == b'[') {
        Some(open) => insert_at(array_json, open, after, elements).unwrap_or_else(|| array_json.to_vec()),
        None => array_json.to_vec(),
    }
}

/// `ChatBlobAssembler.spliceFrozenHistory` (`ChatBlobAssembler.swift:221-231`): `elements`
/// inserted after the first `after` elements of the array under `key` in `body`, found as
/// iTerm2 finds it, at the first `"key":` in the bytes (the builders write the key before
/// anything that could hold it). None or empty `elements` returns the body unchanged.
pub fn splice_at_key(body: &[u8], key: &str, after: usize, elements: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let Some(elements) = elements.filter(|e| !e.is_empty()) else {
        return Ok(body.to_vec());
    };
    let failed = || format!("Failed to splice frozen chat history into the {key} request body");
    let pattern = format!("\"{key}\":");
    let start = body.windows(pattern.len()).position(|w| w == pattern.as_bytes()).ok_or_else(failed)?;
    let open = start + pattern.len();
    if body.get(open) != Some(&b'[') {
        return Err(failed());
    }
    insert_at(body, open, after, elements).ok_or_else(failed)
}

/// Inserts `elements` into the array whose "[" is at `open` (`JSONArraySplice.insert`); None
/// when the array does not close.
pub(crate) fn insert_at(bytes: &[u8], open: usize, after: usize, elements: &[u8]) -> Option<Vec<u8>> {
    if elements.is_empty() {
        return Some(bytes.to_vec());
    }
    let (at, kind) = insertion_point(bytes, open, after)?;
    let mut out = Vec::with_capacity(bytes.len() + elements.len() + 1);
    out.extend_from_slice(&bytes[..at]);
    match kind {
        Insert::Empty => out.extend_from_slice(elements),
        Insert::Before => {
            out.extend_from_slice(elements);
            out.push(b',');
        }
        Insert::AtEnd => {
            out.push(b',');
            out.extend_from_slice(elements);
        }
    }
    out.extend_from_slice(&bytes[at..]);
    Some(out)
}

enum Insert {
    /// "[]" becomes "[elements]".
    Empty,
    /// Before an element: "elements,element".
    Before,
    /// After the last element, before "]".
    AtEnd,
}

/// Where the elements go: walks the array counting top-level commas, aware of strings and
/// nesting (`JSONArraySplice.swift:61-107`).
fn insertion_point(bytes: &[u8], open: usize, after: usize) -> Option<(usize, Insert)> {
    let mut i = open + 1;
    if bytes.get(i) == Some(&b']') {
        return Some((i, Insert::Empty));
    }
    if after == 0 {
        return Some((i, Insert::Before));
    }
    let (mut passed, mut depth, mut in_string, mut escaped) = (0, 0i64, false, false);
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' => depth -= 1,
            b']' if depth == 0 => return Some((i, Insert::AtEnd)),
            b']' => depth -= 1,
            b',' if depth == 0 => {
                passed += 1;
                if passed == after {
                    return Some((i + 1, Insert::Before));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorted_json_follows_iterm2() {
        assert_eq!(sorted(&json!({"b":1,"a":{"d":[{"z":1,"y":2}],"c":"x/y"}})), r#"{"a":{"c":"x/y","d":[{"y":2,"z":1}]},"b":1}"#);
        assert_eq!(sorted(&json!({"B":1,"a":2,"_":3})), r#"{"B":1,"_":3,"a":2}"#, "byte order");
        assert_eq!(sorted(&json!({"s":"q\"\n\u{e9}","n":null,"t":true,"f":1.5,"e":[]})), "{\"e\":[],\"f\":1.5,\"n\":null,\"s\":\"q\\\"\\n\u{e9}\",\"t\":true}");

        assert_eq!(splice(b"[1,2]", 1, b"9,8"), b"[1,9,8,2]");
        assert_eq!(splice(b"[]", 0, b"7"), b"[7]");
        assert_eq!(splice(b"[1]", 1, b""), b"[1]");
        assert_eq!(splice(b"[1,2]", 0, b"0"), b"[0,1,2]");
        assert_eq!(splice(b"[1,2]", 5, b"3"), b"[1,2,3]", "fewer elements: at the end");
        assert_eq!(splice(br#"[{"a":[1,2]},"x,]",3]"#, 2, b"9"), br#"[{"a":[1,2]},"x,]",9,3]"#, "nesting and strings");
        assert_eq!(splice(b"[1,2", 5, b"3"), b"[1,2", "no closing bracket: unchanged");
        assert_eq!(splice(b"{}", 0, b"3"), b"{}", "not an array: unchanged");
    }

    #[test]
    fn splice_at_key_follows_iterm2() {
        let body = br#"{"input":[{"a":1},{"b":2}],"model":"m"}"#;
        assert_eq!(splice_at_key(body, "input", 1, Some(b"9")).unwrap(), br#"{"input":[{"a":1},9,{"b":2}],"model":"m"}"#);
        assert_eq!(splice_at_key(body, "input", 0, Some(b"9")).unwrap(), br#"{"input":[9,{"a":1},{"b":2}],"model":"m"}"#);
        assert_eq!(splice_at_key(br#"{"input":[]}"#, "input", 3, Some(b"9")).unwrap(), br#"{"input":[9]}"#);
        assert_eq!(splice_at_key(body, "input", 1, None).unwrap(), body, "no history: unchanged");
        assert_eq!(splice_at_key(body, "missing", 1, None).unwrap(), body);
        assert_eq!(splice_at_key(body, "input", 1, Some(b"")).unwrap(), body);
        assert_eq!(splice_at_key(body, "messages", 0, Some(b"9")), Err("Failed to splice frozen chat history into the messages request body".to_string()));
        assert!(splice_at_key(body, "model", 0, Some(b"9")).is_err(), "not an array");
        assert!(splice_at_key(br#"{"input":[1"#, "input", 5, Some(b"9")).is_err(), "no closing bracket");
    }
}
