//! iTerm2's Smart Selection: regular-expression rules that decide what a click selects,
//! each with a precision and optional actions.

/// One action on a rule, as iTerm2 stores it: `action` is its selector ("openFile:",
/// "openUrl:", "runCommand:", "runCoprocess:", "sendText:", "runCommandInWindow:",
/// "copy:").
#[derive(Clone, Debug, PartialEq)]
pub struct RuleAction {
    pub title: String,
    pub action: String,
    pub parameter: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub regex: String,
    pub precision: f64,
    pub notes: String,
    pub actions: Vec<RuleAction>,
}

/// iTerm2's precision names and their score multipliers.
pub const PRECISIONS: [(&str, f64); 5] =
    [("very_low", 0.00001), ("low", 0.001), ("normal", 1.0), ("high", 1000.0), ("very_high", 1_000_000.0)];

pub fn precision_of(name: &str) -> f64 {
    PRECISIONS.iter().find(|(n, _)| *n == name).map_or(1.0, |(_, v)| *v)
}

pub fn precision_name(v: f64) -> &'static str {
    PRECISIONS.iter().min_by(|a, b| (a.1 - v).abs().total_cmp(&(b.1 - v).abs())).map_or("normal", |(n, _)| n)
}

/// Rules from a profile's "Smart Selection Rules" list.
pub fn rules_from(value: &toml::Value) -> Vec<Rule> {
    let s = |t: &toml::Table, k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_table())
        .map(|t| Rule {
            regex: s(t, "regex"),
            precision: precision_of(&s(t, "precision")),
            notes: s(t, "notes"),
            actions: t
                .get("actions")
                .and_then(|a| a.as_array())
                .into_iter()
                .flatten()
                .filter_map(|a| a.as_table())
                .map(|a| RuleAction { title: s(a, "title"), action: s(a, "action"), parameter: s(a, "parameter") })
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_parse_with_their_actions() {
        let v: toml::Value = toml::from_str::<toml::Table>(
            r#"r = [ { regex = "\\S+", precision = "very_high", notes = "n", actions = [ { title = "Open", action = "openUrl:", parameter = "https://x/\\0" } ] } ]"#,
        )
        .unwrap()["r"]
            .clone();
        let r = rules_from(&v);
        assert_eq!(r[0].precision, 1_000_000.0);
        assert_eq!(r[0].actions[0], RuleAction { title: "Open".into(), action: "openUrl:".into(), parameter: "https://x/\\0".into() });
        assert_eq!(precision_name(1000.0), "high");
    }
}
