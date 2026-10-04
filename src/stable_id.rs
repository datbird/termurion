//! A session's stable id (iTerm2's `StableSessionID`): "ptys_" plus 12 characters of a
//! confusable-free alphabet and a check character. It is the only way a model names a
//! session, so it is minted from the system random source and never taken on trust:
//! a restored id is adopted only through `adopt`.

pub const PREFIX: &str = "ptys_";
pub const ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const BODY_LEN: usize = 12;
pub const TOKEN_PATTERN: &str = "ptys_[0123456789ABCDEFGHJKMNPQRSTVWXYZ]{13}";

/// The check value of a body of alphabet indices: sum of (position + 1) * value, mod 32.
pub fn checksum(body: &[usize]) -> usize {
    body.iter().enumerate().map(|(i, v)| (i + 1) * v).sum::<usize>() % 32
}

fn letter(index: usize) -> char {
    ALPHABET.as_bytes()[index % 32] as char
}

/// The id with these body indices (each below 32) and its check character.
pub fn from_indices(body: [usize; 12]) -> String {
    let mut s = String::from(PREFIX);
    for v in body {
        s.push(letter(v));
    }
    s.push(letter(checksum(&body)));
    s
}

/// A fresh random id.
pub fn generate() -> String {
    let mut bytes = [0u8; BODY_LEN];
    getrandom::fill(&mut bytes).expect("the system random source failed");
    let mut body = [0usize; 12];
    for (b, v) in body.iter_mut().zip(bytes) {
        *b = (v & 31) as usize;
    }
    from_indices(body)
}

/// The canonical form of a candidate: prefix in any case, I and L read as 1, O as 0, upper
/// case, and the check character verified. None for anything else.
pub fn canonical(candidate: &str) -> Option<String> {
    let head = candidate.get(..PREFIX.len())?;
    if !head.eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    let rest = &candidate[PREFIX.len()..];
    if rest.chars().count() != BODY_LEN + 1 {
        return None;
    }
    let mut values = Vec::with_capacity(BODY_LEN + 1);
    for c in rest.chars() {
        let c = match c.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            other => other,
        };
        values.push(ALPHABET.find(c)?);
    }
    (checksum(&values[..BODY_LEN]) == values[BODY_LEN]).then(|| {
        let mut s = String::from(PREFIX);
        s.extend(values.iter().map(|v| letter(*v)));
        s
    })
}

pub fn is_valid(candidate: &str) -> bool {
    canonical(candidate).is_some()
}

/// The id a restored session takes in place of its fresh one: the saved id in canonical
/// form, only when `launch` (the launch's own restoration of the saved windows; an
/// arrangement, a file or a duplicate never adopts) and no live or buried pane already
/// holds it. None keeps the fresh id.
pub fn adopt(saved: Option<&str>, launch: bool, in_use: &dyn Fn(&str) -> bool) -> Option<String> {
    if !launch {
        return None;
    }
    canonical(saved?).filter(|id| !in_use(id))
}

/// The pane a model's reference names: a stable id (any accepted spelling) of one of `panes`.
/// A decimal pane id or a pane's per-launch session reference names nothing.
pub fn find<'a>(reference: &str, panes: impl IntoIterator<Item = (u64, &'a str)>) -> Option<u64> {
    let want = canonical(reference)?;
    panes.into_iter().find(|(_, id)| *id == want).map(|(p, _)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_ids_follow_iterm2() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..2000 {
            let id = generate();
            assert!(id.starts_with("ptys_") && id.len() == 18, "{id}");
            assert_eq!(canonical(&id).as_deref(), Some(id.as_str()));
            assert!(seen.insert(id));
        }
        assert_eq!(from_indices([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]), "ptys_0123456789ABW");
        assert_eq!(canonical("PTYS_0123456789abw").as_deref(), Some("ptys_0123456789ABW"));
        assert_eq!(canonical("ptys_O123456789ABW").as_deref(), Some("ptys_0123456789ABW"));
        assert_eq!(canonical("ptys_0I23456789ABW").as_deref(), Some("ptys_0123456789ABW"));
        assert_eq!(canonical("ptys_0L23456789ABW").as_deref(), Some("ptys_0123456789ABW"));
        assert_eq!(canonical("ptys_0123456789ABX"), None);
        assert_eq!(canonical("ptys_1123456789ABW"), None);
        assert_eq!(canonical("ptys_0123456789AW"), None);
        assert_eq!(canonical("ptys_0123456789ABWW"), None);
        for bad in ["", "ptys_", "tpys_0123456789ABW", "ptys_U123456789ABW", "ptys_0123456789ABU", "ptys_\u{00e9}123456789ABW"] {
            assert_eq!(canonical(bad), None, "{bad:?}");
        }
        assert_eq!(checksum(&[0; 12]), 0);
        assert!(is_valid("ptys_0123456789ABW") && !is_valid("ptys_bad"));
    }

    #[test]
    fn stable_id_adoption_follows_iterm2() {
        const ID: &str = "ptys_0123456789ABW";
        let free = |_: &str| false;
        assert_eq!(adopt(Some(ID), true, &free).as_deref(), Some(ID));
        assert_eq!(adopt(Some("ptys_0123456789abw"), true, &free).as_deref(), Some(ID));
        // An arrangement, a file or a duplicate (not the launch's restoration) keeps the fresh id.
        assert_eq!(adopt(Some(ID), false, &free), None);
        assert_eq!(adopt(Some("ptys_bad"), true, &free), None);
        assert_eq!(adopt(None, true, &free), None);
        // Held by a live or buried pane: one id never names two panes.
        assert_eq!(adopt(Some(ID), true, &|g| g == ID), None);
    }

    #[test]
    fn references_name_only_stable_ids() {
        let panes = [(1u64, "ptys_0123456789ABW"), (7, "ptys_0000000000000")];
        assert_eq!(find("ptys_0123456789ABW", panes), Some(1));
        assert_eq!(find("PTYS_0123456789abw", panes), Some(1));
        assert_eq!(find("ptys_0000000000000", panes), Some(7));
        // A decimal pane id restarts every launch and names nothing.
        assert_eq!(find("1", panes), None);
        assert_eq!(find("7", panes), None);
        // The per-launch session reference (a UUID) is not a model-visible reference.
        assert_eq!(find("6F1C2A3B-0000-4000-8000-000000000001", panes), None);
        assert_eq!(find("ptys_0123456789ABX", panes), None);
        assert_eq!(find("", panes), None);
    }
}
