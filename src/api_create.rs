//! The Python API's CreateTab (`H:2465-2588`) and SplitPane (`H:2608-2677`): the profile a
//! request names, its custom properties (`profileByCustomizing:`, `H:2679-2688`), and the
//! statuses iTerm2 answers, so the app only starts the sessions.

use toml::Value;

use crate::api_proto::pb;
use crate::layout::Axis;
use crate::profile::GUID;
use crate::session_profile::ORIGINAL_GUID;

/// The request's custom properties as profile values: each JSON value parsed (iTerm2 allows
/// fragments), None when any fails to parse (`H:2682-2685`); a JSON null gives None, which
/// removes the key. `Guid` and the original-guid key belong to the session copy and are
/// dropped.
pub fn custom_values(props: &[pb::ProfileProperty]) -> Option<Vec<(String, Option<Value>)>> {
    let mut out = Vec::new();
    for p in props {
        let key = p.key.clone().unwrap_or_default();
        let json: serde_json::Value = serde_json::from_str(p.json_value.as_deref().unwrap_or_default()).ok()?;
        if key == GUID || key == ORIGINAL_GUID {
            continue;
        }
        let value = match json {
            serde_json::Value::Null => None,
            v => crate::import::from_json(&v),
        };
        out.push((key, value));
    }
    Some(out)
}

/// The Guid of the profile a request names: the default profile when none is named, else the
/// first saved profile with that name (`bookmarkWithName:`), None when there is none.
pub fn profile_guid(name: Option<&str>, by_name: &dyn Fn(&str) -> Option<String>, default: &str) -> Option<String> {
    match name {
        None => Some(default.to_string()),
        Some(n) => by_name(n),
    }
}

/// CreateTab's status once the tab exists (`H:2570-2588`): a requested index is honored only
/// when it is below the window's tab count and the tab landed there (a move the layout lock
/// refused, or one clamped, is INVALID_TAB_INDEX; the tab is made either way).
pub fn tab_index_status(requested: Option<u32>, count: usize, landed: Option<usize>) -> pb::create_tab_response::Status {
    use pb::create_tab_response::Status;
    match requested {
        None => Status::Ok,
        Some(i) if (i as usize) < count && landed == Some(i as usize) => Status::Ok,
        Some(_) => Status::InvalidTabIndex,
    }
}

/// The split's axis: VERTICAL (0, the proto2 default) is a vertical divider, panes side by
/// side; anything else is HORIZONTAL, as iTerm2 compares only with VERTICAL.
pub fn split_axis(direction: i32) -> Axis {
    if direction == pb::split_pane_request::SplitDirection::Vertical as i32 { Axis::Row } else { Axis::Column }
}

/// What one split of SplitPane did: a new session, a tmux split (tmux makes the pane later),
/// or a refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitResult {
    Made(u64),
    Tmux,
    Refused,
}

/// SplitPane's answer (`H:2650-2676`): OK with the new session ids in order; one refused
/// split outside tmux makes it CANNOT_SPLIT, keeping the ids of the splits that were made;
/// a tmux split adds no id.
pub fn split_response(results: &[SplitResult]) -> pb::SplitPaneResponse {
    use pb::split_pane_response::Status;
    let mut status = Status::Ok;
    let mut session_id = Vec::new();
    for r in results {
        match r {
            SplitResult::Made(id) => session_id.push(id.to_string()),
            SplitResult::Tmux => {}
            SplitResult::Refused => status = Status::CannotSplit,
        }
    }
    pb::SplitPaneResponse { status: Some(status as i32), session_id }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb::create_tab_response::Status as CStatus;
    use pb::split_pane_response::Status as SStatus;

    fn prop(key: &str, json: &str) -> pb::ProfileProperty {
        pb::ProfileProperty { key: Some(key.into()), json_value: Some(json.into()) }
    }

    fn table(pairs: &[(&str, f64)]) -> Value {
        Value::Table(pairs.iter().map(|(k, v)| (k.to_string(), Value::Float(*v))).collect())
    }

    #[test]
    fn custom_values_follow_iterm2() {
        assert_eq!(
            custom_values(&[prop("Columns", "100"), prop("Name", "\"x\"")]),
            Some(vec![("Columns".into(), Some(Value::Integer(100))), ("Name".into(), Some(Value::String("x".into())))])
        );
        assert_eq!(custom_values(&[prop("Columns", "{bad")]), None);
        // A parse failure anywhere is a failure, even after good values.
        assert_eq!(custom_values(&[prop("Columns", "100"), prop("Rows", "")]), None);
        assert_eq!(custom_values(&[prop("Guid", "\"g\""), prop(ORIGINAL_GUID, "\"g\"")]), Some(vec![]));
        assert_eq!(
            custom_values(&[prop("Foreground Color", "{\"Red Component\": 1, \"Green Component\": 0, \"Blue Component\": 0}")]),
            Some(vec![("Foreground Color".into(), Some(table(&[("r", 1.0), ("g", 0.0), ("b", 0.0), ("a", 1.0)])))])
        );
        assert_eq!(custom_values(&[prop("Badge Text", "null")]), Some(vec![("Badge Text".into(), None)]));
        assert_eq!(custom_values(&[]), Some(vec![]));
    }

    #[test]
    fn create_statuses_follow_iterm2() {
        assert_eq!(tab_index_status(None, 3, None), CStatus::Ok);
        assert_eq!(tab_index_status(Some(1), 3, Some(1)), CStatus::Ok);
        assert_eq!(tab_index_status(Some(5), 3, None), CStatus::InvalidTabIndex);
        assert_eq!(tab_index_status(Some(3), 3, Some(3)), CStatus::InvalidTabIndex);
        assert_eq!(tab_index_status(Some(1), 3, Some(2)), CStatus::InvalidTabIndex);
        let finds = |n: &str| (n == "Work").then(|| "W".to_string());
        assert_eq!(profile_guid(None, &finds, "D"), Some("D".into()));
        assert_eq!(profile_guid(Some("Work"), &finds, "D"), Some("W".into()));
        assert_eq!(profile_guid(Some("Nope"), &finds, "D"), None);
        assert_eq!(split_axis(0), Axis::Row);
        assert_eq!(split_axis(1), Axis::Column);
    }

    #[test]
    fn split_results_follow_iterm2() {
        use SplitResult::*;
        let r = split_response(&[Made(4), Made(5)]);
        assert_eq!((r.status, r.session_id), (Some(SStatus::Ok as i32), vec!["4".to_string(), "5".to_string()]));
        let r = split_response(&[Made(4), Refused]);
        assert_eq!((r.status, r.session_id), (Some(SStatus::CannotSplit as i32), vec!["4".to_string()]));
        let r = split_response(&[Tmux]);
        assert_eq!((r.status, r.session_id), (Some(SStatus::Ok as i32), vec![]));
        let r = split_response(&[]);
        assert_eq!((r.status, r.session_id), (Some(SStatus::Ok as i32), vec![]));
    }
}
