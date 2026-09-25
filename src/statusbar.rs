//! iTerm2's status bar framework: the profile's `Status Bar Layout` (components with
//! their knobs, and the advanced configuration), the base knobs every component shares,
//! knob descriptors by type, the registry of the framework's own components (the Spring
//! and the Fixed-size Spacer; the placeholder shown when a layout is empty), Hide, and
//! auto-rainbow colors. The 19 content components arrive in milestone 9h.
//!
//! A layout dictionary looks like `{"components": [{"class": <ObjC name>, "configuration":
//! {"knobs": {...}, "layout advanced configuration dictionary value": {...}}}, ...],
//! "advanced configuration": {...}}`. iTerm2 drops a component with no `configuration`; an
//! unknown class is kept in the file (there so a round trip through Tandem does not lose
//! it) and skipped on screen by Task 3.

use toml::Value;

/// `iTermStatusBarSpringComponent`.
pub const SPRING_CLASS: &str = "iTermStatusBarSpringComponent";
/// `iTermStatusBarFixedSpacerComponent`.
pub const SPACER_CLASS: &str = "iTermStatusBarFixedSpacerComponent";
/// `iTermStatusBarPlaceholderComponent`.
pub const PLACEHOLDER_CLASS: &str = "iTermStatusBarPlaceholderComponent";

const BASE_PRIORITY_KEY: &str = "base: priority";
const BASE_COMPRESSION_RESISTANCE_KEY: &str = "base: compression resistance";
const MIN_WIDTH_KEY: &str = "minwidth";
const MAX_WIDTH_KEY: &str = "maxwidth";
const SHARED_TEXT_COLOR_KEY: &str = "shared text color";
const SHARED_BACKGROUND_COLOR_KEY: &str = "shared background color";
const SHARED_FONT_KEY: &str = "shared font";
const SPRING_CONSTANT_KEY: &str = "iTermStatusBarSpringComponentSpringConstantKey";
const SIZE_MULTIPLE_KEY: &str = "iTermStatusBarSpringComponentSizeMultipleKey";
const SPACER_WIDTH_KEY: &str = "iTermStatusBarFixedSpacerComponentWidthKnob";

/// `iTermStatusBarLayoutAlgorithmSetting`: which layout algorithm lays out the bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Algorithm {
    #[default]
    Stable,
    TightlyPacked,
}

impl Algorithm {
    pub fn code(self) -> i64 {
        match self {
            Algorithm::Stable => 0,
            Algorithm::TightlyPacked => 1,
        }
    }

    pub fn from_code(code: i64) -> Algorithm {
        if code == 1 { Algorithm::TightlyPacked } else { Algorithm::Stable }
    }
}

/// `iTermStatusBarAutoRainbowStyle`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rainbow {
    #[default]
    Disabled,
    Light,
    Dark,
    Automatic,
}

impl Rainbow {
    pub fn code(self) -> i64 {
        match self {
            Rainbow::Disabled => 0,
            Rainbow::Light => 1,
            Rainbow::Dark => 2,
            Rainbow::Automatic => 3,
        }
    }

    pub fn from_code(code: i64) -> Rainbow {
        match code {
            1 => Rainbow::Light,
            2 => Rainbow::Dark,
            3 => Rainbow::Automatic,
            _ => Rainbow::Disabled,
        }
    }
}

/// `iTermStatusBarComponentKnobType`, in iTerm2's own declared order. Nothing here
/// serializes this enum's discriminant (a knob's identity is its string key), so the
/// order is only for easy comparison against the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnobKind {
    Checkbox,
    Text,
    Double,
    Color,
    Action,
    Invocation,
    Font,
}

/// `iTermStatusBarComponentKnob`: one configurable property of a component, as shown in
/// the setup UI's Configure Component sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct Knob {
    pub label: String,
    pub kind: KnobKind,
    pub key: String,
    pub placeholder: Option<String>,
    pub default: Option<Value>,
}

/// A component's description in the setup picker: `iTermStatusBarComponent`'s
/// `statusBarComponentShortDescription`, `statusBarComponentDetailedDescription`,
/// `statusBarComponentExemplarWithBackgroundColor:textColor:`, `statusBarComponentCanStretch`,
/// `statusBarComponentHasMargins`, `statusBarComponentIsInternal`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub class: &'static str,
    pub short: &'static str,
    pub detail: &'static str,
    pub exemplar: &'static str,
    pub stretch: bool,
    pub margins: bool,
    pub internal: bool,
}

/// One component of a layout: its class and configuration. `knobs` is the component's
/// current knob values; `config` is the whole `"configuration"` dictionary as loaded
/// (knobs plus whatever else it carries, such as the injected advanced-configuration
/// copy, or a key from a future Tandem or iTerm2 version), kept so a component of a class
/// Tandem does not know round-trips through the profile untouched.
#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub class: String,
    pub knobs: toml::Table,
    pub config: toml::Table,
}

fn knob_f64(knobs: &toml::Table, key: &str) -> Option<f64> {
    knobs.get(key).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
}

impl Component {
    fn from_value(v: &Value) -> Option<Component> {
        let t = v.as_table()?;
        let class = t.get("class")?.as_str()?.to_string();
        // iTerm2 drops a component with no configuration ("Missing configuration for %@").
        let config = t.get("configuration")?.as_table()?.clone();
        let knobs = config.get("knobs").and_then(Value::as_table).cloned().unwrap_or_default();
        Some(Component { class, knobs, config })
    }

    fn to_value(&self) -> Value {
        let mut config = self.config.clone();
        config.insert("knobs".into(), Value::Table(self.knobs.clone()));
        let mut t = toml::Table::new();
        t.insert("class".into(), Value::String(self.class.clone()));
        t.insert("configuration".into(), Value::Table(config));
        Value::Table(t)
    }

    /// `iTermStatusBarPriorityKey`, `"base: priority"`, default 5.
    pub fn priority(&self) -> f64 {
        knob_f64(&self.knobs, BASE_PRIORITY_KEY).unwrap_or(5.0)
    }

    /// `statusBarComponentSpringConstant`, clamped to a minimum of 0.01. A Spring reads
    /// its own key for the active algorithm; every other component reads
    /// `"base: compression resistance"`. Either way the fallback when the key is absent
    /// is 1, even for the Spring's Tightly Packed key (whose knob *descriptor* default is
    /// 0.01; iTerm2's own surprise, see `iTermStatusBarSpringComponent.m`).
    pub fn spring_constant(&self, algorithm: Algorithm) -> f64 {
        let key = if self.class == SPRING_CLASS {
            match algorithm {
                Algorithm::TightlyPacked => SPRING_CONSTANT_KEY,
                Algorithm::Stable => SIZE_MULTIPLE_KEY,
            }
        } else {
            BASE_COMPRESSION_RESISTANCE_KEY
        };
        knob_f64(&self.knobs, key).unwrap_or(1.0).max(0.01)
    }

    /// `"minwidth"`, default 0.
    pub fn min_width_knob(&self) -> f64 {
        knob_f64(&self.knobs, MIN_WIDTH_KEY).unwrap_or(0.0)
    }

    /// `statusBarComponentMaximumWidth`: `MAX(24, knob "maxwidth" ?: INFINITY)`.
    pub fn max_width_knob(&self) -> f64 {
        knob_f64(&self.knobs, MAX_WIDTH_KEY).unwrap_or(f64::INFINITY).max(24.0)
    }

    /// The `"shared text color"` knob, a `{r, g, b, a}` table, if this component's own
    /// color overrides the layout's.
    pub fn text_color(&self) -> Option<Value> {
        self.knobs.get(SHARED_TEXT_COLOR_KEY).cloned()
    }

    /// The `"shared background color"` knob.
    pub fn background_color(&self) -> Option<Value> {
        self.knobs.get(SHARED_BACKGROUND_COLOR_KEY).cloned()
    }

    /// The `"shared font"` knob ("Custom Font"): a font string, family and size, like a
    /// profile's own font keys.
    pub fn font(&self) -> Option<String> {
        self.knobs.get(SHARED_FONT_KEY).and_then(Value::as_str).map(str::to_string)
    }
}

/// The layout's advanced configuration: `iTermStatusBarAdvancedConfiguration`'s
/// dictionary keys.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Advanced {
    /// `"separator color 2"` (`"separator color"` is deprecated with no migration).
    pub separator_color: Option<Value>,
    /// `"background color"`.
    pub background_color: Option<Value>,
    /// `"default text color"`.
    pub default_text_color: Option<Value>,
    /// `"algorithm"`, default Stable.
    pub algorithm: Algorithm,
    /// `"remove empty components"`, default false.
    pub remove_empty: bool,
    /// `"draw separator between status bar and terminal"`, default false.
    pub edge_separator: bool,
    /// `"font"`: the layout's own font string; nil means the prose font at 12 pt.
    pub font: Option<String>,
    /// `"auto-rainbow style"`, default Disabled.
    pub rainbow: Rainbow,
}

fn int_key(t: &toml::Table, key: &str) -> Option<i64> {
    t.get(key).and_then(|v| v.as_integer().or_else(|| v.as_float().map(|f| f as i64)))
}

fn bool_key(t: &toml::Table, key: &str) -> bool {
    t.get(key).and_then(Value::as_bool).unwrap_or(false)
}

impl Advanced {
    fn from_table(t: Option<&toml::Table>) -> Advanced {
        let empty = toml::Table::new();
        let t = t.unwrap_or(&empty);
        Advanced {
            separator_color: t.get("separator color 2").cloned(),
            background_color: t.get("background color").cloned(),
            default_text_color: t.get("default text color").cloned(),
            algorithm: Algorithm::from_code(int_key(t, "algorithm").unwrap_or(0)),
            remove_empty: bool_key(t, "remove empty components"),
            edge_separator: bool_key(t, "draw separator between status bar and terminal"),
            font: t.get("font").and_then(Value::as_str).map(str::to_string),
            rainbow: Rainbow::from_code(int_key(t, "auto-rainbow style").unwrap_or(0)),
        }
    }

    /// Matches iTerm2's computed default exactly: the four non-color keys are always
    /// written; a nil color or font is left out (`dictionaryByRemovingNullValues`).
    fn to_table(&self) -> toml::Table {
        let mut t = toml::Table::new();
        if let Some(c) = &self.separator_color {
            t.insert("separator color 2".into(), c.clone());
        }
        if let Some(c) = &self.background_color {
            t.insert("background color".into(), c.clone());
        }
        if let Some(c) = &self.default_text_color {
            t.insert("default text color".into(), c.clone());
        }
        t.insert("algorithm".into(), Value::Integer(self.algorithm.code()));
        t.insert("remove empty components".into(), Value::Boolean(self.remove_empty));
        t.insert("draw separator between status bar and terminal".into(), Value::Boolean(self.edge_separator));
        if let Some(f) = &self.font {
            t.insert("font".into(), Value::String(f.clone()));
        }
        t.insert("auto-rainbow style".into(), Value::Integer(self.rainbow.code()));
        t
    }
}

/// The profile's `Status Bar Layout`: its components, in order, and the advanced
/// configuration. `extra` keeps any top-level key besides `"components"` and
/// `"advanced configuration"` (there are none in current iTerm2, but a value from an
/// unreleased or future one round-trips rather than vanishing).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub components: Vec<Component>,
    pub advanced: Advanced,
    pub extra: toml::Table,
}

impl Layout {
    /// None, or a value that is not a table, gives iTerm2's computed default layout:
    /// `initWithDictionary:@{}`, i.e. no components and the advanced configuration's
    /// own defaults. A table with per-key trouble degrades key by key, the way iTerm2's
    /// own dictionary lookups (each with a fallback) do.
    pub fn from_value(v: Option<&Value>) -> Layout {
        let Some(Value::Table(t)) = v else { return Layout::default() };
        let components = t
            .get("components")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Component::from_value).collect())
            .unwrap_or_default();
        let advanced = Advanced::from_table(t.get("advanced configuration").and_then(Value::as_table));
        let mut extra = t.clone();
        extra.remove("components");
        extra.remove("advanced configuration");
        Layout { components, advanced, extra }
    }

    pub fn to_value(&self) -> Value {
        let mut t = self.extra.clone();
        t.insert("components".into(), Value::Array(self.components.iter().map(Component::to_value).collect()));
        t.insert("advanced configuration".into(), Value::Table(self.advanced.to_table()));
        Value::Table(t)
    }
}

/// Hide: removes every component of `class`, not just one instance
/// (`isEqualToComponentIgnoringConfiguration` compares class only).
pub fn hide_class(layout: &mut Layout, class: &str) {
    layout.components.retain(|c| c.class != class);
}

fn priority_knob() -> Knob {
    Knob { label: "Priority:".into(), kind: KnobKind::Double, key: BASE_PRIORITY_KEY.into(), placeholder: Some(String::new()), default: Some(Value::Float(5.0)) }
}

/// This milestone's framework registry: the Spring, the Fixed-size Spacer, and the
/// internal Placeholder shown when a layout has no components. The other 19 classes
/// arrive in milestone 9h; `info` and `knobs` return nothing for a class not here yet,
/// which is exactly the "kept in the file, skipped on screen" behavior for them.
pub fn registry() -> Vec<Info> {
    vec![
        Info {
            class: SPRING_CLASS,
            // Tightly Packed's naming ("Spring"); under Stable this component displays
            // as "Empty Space" with a matching exemplar and detail text (see `knobs`'s
            // own algorithm-driven label) — the setup UI picks that variant itself.
            short: "Spring",
            detail: "Pushes items apart. Use one spring to right-align status bar elements that follow it. Use two to center those inbetween.",
            exemplar: "║〜〜║",
            stretch: true,
            margins: false,
            internal: false,
        },
        Info {
            class: SPACER_CLASS,
            short: "Fixed-size Spacer",
            detail: "Adds a fixed amount of space",
            exemplar: "╠══╣",
            stretch: false,
            margins: true,
            internal: false,
        },
        Info {
            class: PLACEHOLDER_CLASS,
            short: "Placeholder",
            detail: "Placeholder",
            // iTerm2's own exemplar method asserts false: the placeholder never appears
            // in the setup picker (it is internal).
            exemplar: "",
            stretch: true,
            margins: true,
            internal: true,
        },
    ]
}

pub fn info(class: &str) -> Option<Info> {
    registry().into_iter().find(|i| i.class == class)
}

/// The knob descriptors `statusBarComponentKnobs` returns, in order, for a class this
/// milestone knows. A class not here yet (the 9h content components, or one Tandem
/// truly does not recognize) returns no knobs: it has no Configure sheet, only Hide.
pub fn knobs(class: &str, algorithm: Algorithm) -> Vec<Knob> {
    match class {
        SPRING_CLASS => {
            let (label, key, default) = match algorithm {
                Algorithm::TightlyPacked => ("Compression Resistance:", SPRING_CONSTANT_KEY, 0.01),
                Algorithm::Stable => ("Size Multiple:", SIZE_MULTIPLE_KEY, 1.0),
            };
            vec![
                Knob { label: label.into(), kind: KnobKind::Double, key: key.into(), placeholder: Some(String::new()), default: Some(Value::Float(default)) },
                Knob { label: "Color".into(), kind: KnobKind::Color, key: SHARED_BACKGROUND_COLOR_KEY.into(), placeholder: None, default: None },
            ]
        }
        SPACER_CLASS => vec![
            Knob { label: "Color".into(), kind: KnobKind::Color, key: SHARED_BACKGROUND_COLOR_KEY.into(), placeholder: None, default: None },
            Knob { label: "Width".into(), kind: KnobKind::Double, key: SPACER_WIDTH_KEY.into(), placeholder: None, default: Some(Value::Float(5.0)) },
            priority_knob(),
        ],
        _ => Vec::new(),
    }
}

/// `iTermStatusBarAutoRainbowController.enumerateColorsWithCount:block:`: `count` colors
/// as `(hue, saturation, brightness)`, each in 0 to 1. Disabled gives none. Automatic
/// picks the Light parameters when the bar's background is dark, else the Dark ones
/// (iTerm2's own choice: a dark bar gets the brighter set).
pub fn rainbow_colors(count: usize, style: Rainbow, dark_background: bool) -> Vec<(f32, f32, f32)> {
    let (saturation, brightness) = match style {
        Rainbow::Disabled => return Vec::new(),
        Rainbow::Light => (0.3, 0.9),
        Rainbow::Dark => (0.5, 0.5),
        Rainbow::Automatic => if dark_background { (0.3, 0.9) } else { (0.5, 0.5) },
    };
    let stride = 0.91 / (count as f32 - 1.0);
    let mut hue = 0.0f32;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push((hue, saturation, brightness));
        hue += stride;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(pairs: &[(&str, Value)]) -> toml::Table {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn layouts_round_trip_like_iterm2() {
        let default = Layout::from_value(None);
        assert_eq!(default, Layout::default());
        assert_eq!(default.advanced.algorithm, Algorithm::Stable);
        assert!(!default.advanced.remove_empty && !default.advanced.edge_separator);
        assert_eq!(default.advanced.rainbow, Rainbow::Disabled);
        assert_eq!(default.advanced.font, None);
        assert_eq!(
            default.advanced.to_table(),
            table(&[
                ("algorithm", Value::Integer(0)),
                ("remove empty components", Value::Boolean(false)),
                ("draw separator between status bar and terminal", Value::Boolean(false)),
                ("auto-rainbow style", Value::Integer(0)),
            ])
        );
        // A value that is not a table is malformed too; it gives the same default.
        assert_eq!(Layout::from_value(Some(&Value::String("nope".into()))), default);

        // A component missing "configuration" is dropped ("Missing configuration for %@").
        let no_config = table(&[("class", Value::String("iTermStatusBarHostnameComponent".into()))]);

        // A class Tandem does not know is kept, with its knobs and an unrecognized key
        // inside "configuration" (e.g. a future component or Tandem release's own key).
        let unknown_config = table(&[
            ("knobs", Value::Table(table(&[(BASE_PRIORITY_KEY, Value::Float(3.0))]))),
            ("future component key", Value::String("kept".into())),
        ]);
        let unknown = table(&[("class", Value::String("iTermStatusBarFutureComponent".into())), ("configuration", Value::Table(unknown_config))]);

        let spring_config = table(&[("knobs", Value::Table(table(&[(SPRING_CONSTANT_KEY, Value::Float(2.0))])))]);
        let spring = table(&[("class", Value::String(SPRING_CLASS.into())), ("configuration", Value::Table(spring_config))]);

        let color = table(&[("r", Value::Float(0.1)), ("g", Value::Float(0.2)), ("b", Value::Float(0.3)), ("a", Value::Float(1.0))]);
        let layout_table = table(&[
            ("components", Value::Array(vec![Value::Table(no_config), Value::Table(unknown), Value::Table(spring)])),
            ("advanced configuration", Value::Table(table(&[("background color", Value::Table(color)), ("algorithm", Value::Integer(1))]))),
            ("future top level key", Value::String("kept too".into())),
        ]);

        let layout = Layout::from_value(Some(&Value::Table(layout_table)));
        // The bogus component is gone; the unknown class and the spring survive, in order.
        assert_eq!(layout.components.len(), 2);
        assert_eq!(layout.components[0].class, "iTermStatusBarFutureComponent");
        assert_eq!(layout.components[0].knobs.get(BASE_PRIORITY_KEY), Some(&Value::Float(3.0)));
        assert_eq!(layout.components[1].class, SPRING_CLASS);
        assert_eq!(layout.advanced.algorithm, Algorithm::TightlyPacked);
        assert!(layout.advanced.background_color.is_some());
        assert_eq!(layout.extra.get("future top level key").and_then(Value::as_str), Some("kept too"));

        // Round trip: the value serialized back out reparses to the same layout.
        let value = layout.to_value();
        assert_eq!(Layout::from_value(Some(&value)), layout);
        // The unknown component's own unrecognized key survived inside it.
        let components = value.as_table().unwrap().get("components").unwrap().as_array().unwrap();
        let reread_config = components[0].as_table().unwrap().get("configuration").unwrap().as_table().unwrap();
        assert_eq!(reread_config.get("future component key").and_then(Value::as_str), Some("kept"));

        // Hide on a spring removes every spring, regardless of its own knobs, but leaves
        // the unrelated unknown-class component alone.
        let mut two_springs = layout.clone();
        two_springs.components.push(Component { class: SPRING_CLASS.into(), knobs: toml::Table::new(), config: toml::Table::new() });
        assert_eq!(two_springs.components.iter().filter(|c| c.class == SPRING_CLASS).count(), 2);
        hide_class(&mut two_springs, SPRING_CLASS);
        assert_eq!(two_springs.components.len(), 1);
        assert!(two_springs.components.iter().all(|c| c.class != SPRING_CLASS));
    }

    #[test]
    fn base_knobs_follow_iterm2() {
        let mut c = Component { class: "iTermStatusBarClockComponent".into(), knobs: toml::Table::new(), config: toml::Table::new() };
        // Defaults when every knob is absent.
        assert_eq!(c.priority(), 5.0);
        assert_eq!(c.spring_constant(Algorithm::TightlyPacked), 1.0);
        assert_eq!(c.spring_constant(Algorithm::Stable), 1.0);
        assert_eq!(c.min_width_knob(), 0.0);
        assert_eq!(c.max_width_knob(), f64::INFINITY);
        assert_eq!(c.text_color(), None);
        assert_eq!(c.background_color(), None);
        assert_eq!(c.font(), None);

        c.knobs.insert(BASE_PRIORITY_KEY.into(), Value::Float(2.5));
        // Clamped up from 0 to the 0.01 floor.
        c.knobs.insert(BASE_COMPRESSION_RESISTANCE_KEY.into(), Value::Float(0.0));
        c.knobs.insert(MIN_WIDTH_KEY.into(), Value::Float(10.0));
        // Floored to 24 (MAX(24, knob)).
        c.knobs.insert(MAX_WIDTH_KEY.into(), Value::Float(1.0));
        let color = table(&[("r", Value::Float(1.0)), ("g", Value::Float(0.0)), ("b", Value::Float(0.0)), ("a", Value::Float(1.0))]);
        c.knobs.insert(SHARED_TEXT_COLOR_KEY.into(), Value::Table(color.clone()));
        c.knobs.insert(SHARED_BACKGROUND_COLOR_KEY.into(), Value::Table(color.clone()));
        c.knobs.insert(SHARED_FONT_KEY.into(), Value::String("Menlo 14".into()));

        assert_eq!(c.priority(), 2.5);
        // Not a spring: reads "base: compression resistance" regardless of algorithm.
        assert_eq!(c.spring_constant(Algorithm::TightlyPacked), 0.01);
        assert_eq!(c.spring_constant(Algorithm::Stable), 0.01);
        assert_eq!(c.min_width_knob(), 10.0);
        assert_eq!(c.max_width_knob(), 24.0);
        assert_eq!(c.text_color(), Some(Value::Table(color.clone())));
        assert_eq!(c.background_color(), Some(Value::Table(color)));
        assert_eq!(c.font(), Some("Menlo 14".to_string()));

        // A Spring reads its own key for the active algorithm; either way the runtime
        // fallback when the key is absent is 1 (iTerm2's surprise: the Tightly Packed
        // knob's UI default is 0.01, but the code falls back to 1 when it is unset).
        let mut spring = Component { class: SPRING_CLASS.into(), knobs: toml::Table::new(), config: toml::Table::new() };
        assert_eq!(spring.spring_constant(Algorithm::TightlyPacked), 1.0);
        assert_eq!(spring.spring_constant(Algorithm::Stable), 1.0);
        spring.knobs.insert(SPRING_CONSTANT_KEY.into(), Value::Float(0.02));
        spring.knobs.insert(SIZE_MULTIPLE_KEY.into(), Value::Float(3.0));
        assert_eq!(spring.spring_constant(Algorithm::TightlyPacked), 0.02);
        assert_eq!(spring.spring_constant(Algorithm::Stable), 3.0);
        spring.knobs.insert(SPRING_CONSTANT_KEY.into(), Value::Float(-1.0));
        assert_eq!(spring.spring_constant(Algorithm::TightlyPacked), 0.01);
    }

    #[test]
    fn framework_components_follow_iterm2() {
        let reg = registry();
        assert_eq!(reg.len(), 3);

        let spring = info(SPRING_CLASS).unwrap();
        assert_eq!((spring.short, spring.exemplar, spring.stretch, spring.margins, spring.internal), ("Spring", "║〜〜║", true, false, false));

        let spacer = info(SPACER_CLASS).unwrap();
        assert_eq!(
            (spacer.short, spacer.detail, spacer.exemplar, spacer.stretch, spacer.margins, spacer.internal),
            ("Fixed-size Spacer", "Adds a fixed amount of space", "╠══╣", false, true, false)
        );

        let placeholder = info(PLACEHOLDER_CLASS).unwrap();
        assert!(placeholder.internal && placeholder.stretch && placeholder.margins);

        assert_eq!(info("iTermStatusBarNoSuchComponent"), None);

        // Spring's own knob switches label, key and descriptor default with the
        // algorithm; no Priority knob (unlike every other component).
        let tight = knobs(SPRING_CLASS, Algorithm::TightlyPacked);
        assert_eq!(tight.len(), 2);
        assert_eq!((tight[0].label.as_str(), tight[0].key.as_str(), tight[0].default.as_ref().and_then(Value::as_float)), ("Compression Resistance:", SPRING_CONSTANT_KEY, Some(0.01)));
        assert_eq!((tight[1].label.as_str(), tight[1].kind, tight[1].key.as_str()), ("Color", KnobKind::Color, SHARED_BACKGROUND_COLOR_KEY));
        assert_eq!(tight[1].placeholder, None);
        let stable = knobs(SPRING_CLASS, Algorithm::Stable);
        assert_eq!((stable[0].label.as_str(), stable[0].key.as_str(), stable[0].default.as_ref().and_then(Value::as_float)), ("Size Multiple:", SIZE_MULTIPLE_KEY, Some(1.0)));

        // Fixed spacer: Color, Width, then the inherited Priority (no compression knob:
        // it cannot stretch).
        let spacer_knobs = knobs(SPACER_CLASS, Algorithm::Stable);
        assert_eq!(spacer_knobs.iter().map(|k| k.label.as_str()).collect::<Vec<_>>(), vec!["Color", "Width", "Priority:"]);
        assert_eq!(spacer_knobs[1].key, SPACER_WIDTH_KEY);
        assert_eq!(spacer_knobs[1].default.as_ref().and_then(Value::as_float), Some(5.0));
        assert_eq!(spacer_knobs[2].default.as_ref().and_then(Value::as_float), Some(5.0));

        // Internal (Placeholder) and unknown classes have no Configure sheet.
        assert!(knobs(PLACEHOLDER_CLASS, Algorithm::Stable).is_empty());
        assert!(knobs("iTermStatusBarNoSuchComponent", Algorithm::Stable).is_empty());
    }

    #[test]
    fn rainbow_follows_iterm2() {
        assert_eq!(rainbow_colors(0, Rainbow::Light, false), Vec::new());
        assert_eq!(rainbow_colors(3, Rainbow::Disabled, false), Vec::new());
        assert_eq!(rainbow_colors(1, Rainbow::Light, false), vec![(0.0, 0.3, 0.9)]);
        assert_eq!(rainbow_colors(1, Rainbow::Dark, false), vec![(0.0, 0.5, 0.5)]);

        let five = rainbow_colors(5, Rainbow::Light, false);
        assert_eq!(five.len(), 5);
        let stride = 0.91 / 4.0;
        for (i, &(h, s, b)) in five.iter().enumerate() {
            assert!((h - stride * i as f32).abs() < 1e-5, "{i}: {h}");
            assert_eq!((s, b), (0.3, 0.9));
        }
        assert!((five[4].0 - 0.91).abs() < 1e-5);

        // Automatic: a dark bar gets the Light parameters; a light bar gets the Dark ones.
        assert_eq!(rainbow_colors(1, Rainbow::Automatic, true), vec![(0.0, 0.3, 0.9)]);
        assert_eq!(rainbow_colors(1, Rainbow::Automatic, false), vec![(0.0, 0.5, 0.5)]);
    }
}
