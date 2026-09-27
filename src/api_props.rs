//! The Python API's GetProperty (`H:2852-2984`) and SetProperty (`H:2690-2850`): the names
//! a window and a session answer to, the JSON each value is read from and written as, and
//! the statuses iTerm2 answers, so the app only reads and changes its windows.

use serde_json::{json, Value};

use crate::api_proto::pb;

/// A property a window or a session answers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prop {
    Frame,
    Fullscreen,
    GridSize,
    Buried,
    NumberOfLines,
}

/// A window's properties, got and set alike (`H:2789-2791`, `H:2896-2898`).
pub fn window_prop(name: &str) -> Option<Prop> {
    match name {
        "frame" => Some(Prop::Frame),
        "fullscreen" => Some(Prop::Fullscreen),
        _ => None,
    }
}

/// A session's properties: `number_of_lines` is only got (`H:2838-2841`, `H:2972-2976`).
pub fn session_prop(name: &str, setting: bool) -> Option<Prop> {
    match name {
        "grid_size" => Some(Prop::GridSize),
        "buried" => Some(Prop::Buried),
        "number_of_lines" if !setting => Some(Prop::NumberOfLines),
        _ => None,
    }
}

/// A SetProperty value: JSON with fragments allowed (`NSJSONReadingAllowFragments`), so a
/// bare number, boolean, string or null parses too; None when it does not parse.
pub fn parse_value(json: &str) -> Option<Value> {
    serde_json::from_str(json).ok()
}

/// A number as NSJSONSerialization writes it: whole values without a fraction.
fn number(v: f64) -> Value {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9.0e15 {
        json!(v as i64)
    } else {
        json!(v)
    }
}

/// A window frame as GetProperty answers it (`H:2881-2890`).
pub fn frame_json(x: f64, y: f64, w: f64, h: f64) -> String {
    json!({"origin": {"x": number(x), "y": number(y)}, "size": {"width": number(w), "height": number(h)}}).to_string()
}

/// An NSNumber's value: a JSON number, or a boolean (1 or 0); anything else is not a number.
fn ns_number(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// A frame to set (`H:2750-2762`): origin x and y and size width and height, each a number;
/// None when any is missing.
pub fn parse_frame(v: &Value) -> Option<(f64, f64, f64, f64)> {
    let (origin, size) = (v.get("origin")?, v.get("size")?);
    Some((ns_number(origin.get("x"))?, ns_number(origin.get("y"))?, ns_number(size.get("width"))?, ns_number(size.get("height"))?))
}

/// A session's grid size as GetProperty answers it: width and height each one less than the
/// grid's (`H:2953-2956`, kept).
pub fn grid_json(cols: usize, rows: usize) -> String {
    json!({"width": cols as i64 - 1, "height": rows as i64 - 1}).to_string()
}

/// A grid size to set (`H:2804-2810`): width and height, each a number taken as an integer
/// (`integerValue` truncates); None when either is missing.
pub fn parse_size(v: &Value) -> Option<(i64, i64)> {
    let (w, h) = (ns_number(v.get("width"))?, ns_number(v.get("height"))?);
    Some((w.trunc() as i64, h.trunc() as i64))
}

/// A session's line counts (`H:2967-2970`): lines dropped from history, the grid's rows, the
/// history kept, and the absolute number of the first visible line.
pub fn lines_json(overflow: i64, grid: i64, history: i64, first_visible: i64) -> String {
    json!({"overflow": overflow, "grid": grid, "history": history, "first_visible": first_visible}).to_string()
}

/// A boolean to set (`NSNumber castFrom:` then `boolValue`): a boolean, or a number that is
/// true when not zero; None for anything else.
pub fn as_bool(v: &Value) -> Option<bool> {
    ns_number(Some(v)).map(|n| n != 0.0)
}

/// Whether a session's grid can be set (`sessionInitiatedResize:`, `PT:7126-7160`): never in
/// full screen, with Lock Size, for a tmux pane (tmux owns its size) or for a session in no
/// window (a buried one).
pub fn grid_status(fullscreen: bool, locked: bool, tmux: bool, in_window: bool) -> pb::set_property_response::Status {
    use pb::set_property_response::Status;
    if fullscreen || locked || tmux || !in_window { Status::Impossible } else { Status::Ok }
}

/// The grid a set asks for (`safelySetSessionSize:`, `PT`): at least 20 columns and 2 rows,
/// and no more rows than the window can grow by (`max_new_rows` beyond the session's `rows`).
pub fn wanted_grid(cols: i64, rows: i64, current_rows: i64, max_new_rows: i64) -> (i64, i64) {
    let width = cols.max(20);
    let mut height = rows.max(2);
    let grow = rows - current_rows;
    if grow > max_new_rows {
        height -= grow - max_new_rows;
    }
    (width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb::set_property_response::Status;

    fn value(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn get_property_follows_iterm2() {
        assert_eq!(value(&frame_json(10.0, 20.0, 800.0, 600.0)), json!({"origin": {"x": 10, "y": 20}, "size": {"width": 800, "height": 600}}));
        assert_eq!(value(&frame_json(-5.0, 0.0, 1.5, 2.0)), json!({"origin": {"x": -5, "y": 0}, "size": {"width": 1.5, "height": 2}}));
        // Kept: one less than the grid (`H:2953-2956`).
        assert_eq!(value(&grid_json(80, 24)), json!({"width": 79, "height": 23}));
        assert_eq!(value(&lines_json(3, 24, 100, 90)), json!({"overflow": 3, "grid": 24, "history": 100, "first_visible": 90}));
        assert_eq!(window_prop("frame"), Some(Prop::Frame));
        assert_eq!(window_prop("fullscreen"), Some(Prop::Fullscreen));
        assert_eq!(window_prop("grid_size"), None);
        assert_eq!(session_prop("grid_size", false), Some(Prop::GridSize));
        assert_eq!(session_prop("buried", true), Some(Prop::Buried));
        assert_eq!(session_prop("number_of_lines", false), Some(Prop::NumberOfLines));
        assert_eq!(session_prop("number_of_lines", true), None);
        assert_eq!(session_prop("frame", false), None);
        assert_eq!(session_prop("bogus", false), None);
    }

    #[test]
    fn set_property_follows_iterm2() {
        assert_eq!(parse_value("{bad"), None);
        assert_eq!(parse_value(""), None);
        assert_eq!(parse_value("5"), Some(json!(5)));
        assert_eq!(parse_value("true"), Some(json!(true)));
        assert_eq!(parse_value("null"), Some(Value::Null));
        let frame = json!({"origin": {"x": 1, "y": 2}, "size": {"width": 300, "height": 200.5}});
        assert_eq!(parse_frame(&frame), Some((1.0, 2.0, 300.0, 200.5)));
        assert_eq!(parse_frame(&json!({"origin": {"x": 1, "y": 2}, "size": {"width": 300}})), None);
        assert_eq!(parse_frame(&json!({"origin": {"x": "1", "y": 2}, "size": {"width": 300, "height": 200}})), None);
        assert_eq!(parse_frame(&json!(5)), None);
        assert_eq!(as_bool(&json!(1)), Some(true));
        assert_eq!(as_bool(&json!(0)), Some(false));
        assert_eq!(as_bool(&json!(false)), Some(false));
        assert_eq!(as_bool(&json!("yes")), None);
        assert_eq!(as_bool(&Value::Null), None);
        assert_eq!(parse_size(&json!({"width": 80})), None);
        assert_eq!(parse_size(&json!({"width": 80, "height": 25})), Some((80, 25)));
        assert_eq!(parse_size(&json!({"width": 80.9, "height": 25})), Some((80, 25)));
        assert_eq!(parse_size(&json!([80, 25])), None);
        assert_eq!(grid_status(true, false, false, true), Status::Impossible);
        assert_eq!(grid_status(false, true, false, true), Status::Impossible);
        assert_eq!(grid_status(false, false, true, true), Status::Impossible);
        assert_eq!(grid_status(false, false, false, false), Status::Impossible);
        assert_eq!(grid_status(false, false, false, true), Status::Ok);
        // `safelySetSessionSize:`: at least 20 by 2, rows capped by the room to grow.
        assert_eq!(wanted_grid(10, 1, 24, 100), (20, 2));
        assert_eq!(wanted_grid(100, 60, 24, 10), (100, 34));
        assert_eq!(wanted_grid(100, 30, 24, 10), (100, 30));
    }
}
