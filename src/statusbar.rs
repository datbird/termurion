//! iTerm2's status bar framework: the profile's `Status Bar Layout` (components with
//! their knobs, and the advanced configuration), the base knobs every component shares,
//! knob descriptors by type, the registry of the framework's own components (the Spring
//! and the Fixed-size Spacer; the placeholder shown when a layout is empty) plus 9h's and
//! 9i's content components (`statusbar_components`, folded into this module's own
//! `registry`, `info`, `knobs`, `default_knobs` and `exemplar`), Hide, and auto-rainbow
//! colors. 9i's five content classes (Battery, CPU, Memory, Network, git) are registered
//! here (Task 1) but stay off screen (`src/app/statusbar_ui.rs`'s `Piece::of`) until
//! Tasks 3 and 5.
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

/// `iTermStatusBarPriorityKey`. `pub(crate)` so `statusbar_components` can build the same
/// shared knob descriptors these consts describe (`BASE:23-27`), rather than duplicating
/// the strings.
pub(crate) const BASE_PRIORITY_KEY: &str = "base: priority";
pub(crate) const BASE_COMPRESSION_RESISTANCE_KEY: &str = "base: compression resistance";
pub(crate) const MIN_WIDTH_KEY: &str = "minwidth";
pub(crate) const MAX_WIDTH_KEY: &str = "maxwidth";
pub(crate) const SHARED_TEXT_COLOR_KEY: &str = "shared text color";
pub(crate) const SHARED_BACKGROUND_COLOR_KEY: &str = "shared background color";
pub(crate) const SHARED_FONT_KEY: &str = "shared font";
pub(crate) const SPRING_CONSTANT_KEY: &str = "iTermStatusBarSpringComponentSpringConstantKey";
const SIZE_MULTIPLE_KEY: &str = "iTermStatusBarSpringComponentSizeMultipleKey";
/// `iTermStatusBarFixedSpacerComponentWidthKnob`. `pub(crate)` so the app side's own tests
/// can build a spacer `Component` with it, rather than duplicating the string.
pub(crate) const SPACER_WIDTH_KEY: &str = "iTermStatusBarFixedSpacerComponentWidthKnob";
/// A spring constant's upper clamp (review fix): large enough that a user-typed knob is
/// still effectively "take everything", but finite, so two such items dividing infinity by
/// infinity for a growth weight never produces NaN placements.
const SPRING_CONSTANT_MAX: f64 = 1_000_000.0;

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

    /// `statusBarComponentSpringConstant`, clamped to between 0.01 and a large finite
    /// value (review fix: an infinite Size Multiple or Compression Resistance knob, which
    /// the numeric knob's text field accepts as "\u{221e}", used to reach the layout
    /// algorithms unclamped above and give NaN placements when two such items divided
    /// infinity by infinity for a growth weight; a NaN knob, which should not occur but
    /// would poison every comparison below, falls back to the same default as an absent
    /// one). A Spring reads its own key for the active algorithm; every other component
    /// reads `"base: compression resistance"`. Either way the fallback when the key is
    /// absent is 1, even for the Spring's Tightly Packed key (whose knob *descriptor*
    /// default is 0.01; iTerm2's own surprise, see `iTermStatusBarSpringComponent.m`).
    pub fn spring_constant(&self, algorithm: Algorithm) -> f64 {
        let key = if self.class == SPRING_CLASS {
            match algorithm {
                Algorithm::TightlyPacked => SPRING_CONSTANT_KEY,
                Algorithm::Stable => SIZE_MULTIPLE_KEY,
            }
        } else {
            BASE_COMPRESSION_RESISTANCE_KEY
        };
        let v = knob_f64(&self.knobs, key).unwrap_or(1.0);
        if v.is_nan() { 1.0 } else { v.clamp(0.01, SPRING_CONSTANT_MAX) }
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

    /// `iTermStatusBarFixedSpacerComponent`'s width: `knob ?: 5`, so 0 also means 5.
    pub fn spacer_width(&self) -> f64 {
        match knob_f64(&self.knobs, SPACER_WIDTH_KEY) {
            Some(w) if w != 0.0 => w,
            _ => 5.0,
        }
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
    pub fn to_table(&self) -> toml::Table {
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

pub(crate) fn priority_knob() -> Knob {
    Knob { label: "Priority:".into(), kind: KnobKind::Double, key: BASE_PRIORITY_KEY.into(), placeholder: Some(String::new()), default: Some(Value::Float(5.0)) }
}

/// A Color knob, `"Color"` (the Spring, the Fixed-size Spacer, Search and Filter) or
/// labeled (`"Text Color:"`, `"Icon Color:"`, ...): always no placeholder or default (a
/// color knob's absence just means "use the layout's own").
pub(crate) fn color_knob(label: &'static str, key: &'static str) -> Knob {
    Knob { label: label.into(), kind: KnobKind::Color, key: key.into(), placeholder: None, default: None }
}

/// `"Minimum Width:"` and `"Maximum Width:"` (`BASE:189-203`), shown by every Text
/// component. The minimum's descriptor default is oddly the STRING `"0"`
/// (`[@(defaultMinimumWidth) stringValue]`); the maximum's is the float INFINITY.
pub(crate) fn min_max_width_knobs() -> [Knob; 2] {
    [
        Knob { label: "Minimum Width:".into(), kind: KnobKind::Double, key: MIN_WIDTH_KEY.into(), placeholder: Some(String::new()), default: Some(Value::String("0".into())) },
        Knob { label: "Maximum Width:".into(), kind: KnobKind::Double, key: MAX_WIDTH_KEY.into(), placeholder: Some(String::new()), default: Some(Value::Float(f64::INFINITY)) },
    ]
}

/// The Compression Resistance / Size Multiple knob (by `algorithm`) a generic stretchable
/// component shows: descriptor default 1 under either algorithm (`BASE:231-237`), unlike
/// the Spring's own knob above. Shared by `text_component_knobs` and by git's own knob
/// suite (`statusbar_components`), which is not built on Text (no Custom Font knob) but
/// still stretches.
pub(crate) fn stretch_knob(algorithm: Algorithm) -> Knob {
    let label = match algorithm {
        Algorithm::TightlyPacked => "Compression Resistance:",
        Algorithm::Stable => "Size Multiple:",
    };
    Knob { label: label.into(), kind: KnobKind::Double, key: BASE_COMPRESSION_RESISTANCE_KEY.into(), placeholder: Some(String::new()), default: Some(Value::Float(1.0)) }
}

/// The knob suite every stretchable `iTermStatusBarTextComponent` shows after its own
/// knobs (`TXT:27-48` plus the Base knobs): Text Color, Background Color, Custom Font,
/// Compression Resistance or Size Multiple (by `algorithm`), Priority, Minimum Width,
/// Maximum Width. Shared by every 9h content class built on Text (`statusbar_components`).
pub(crate) fn text_component_knobs(algorithm: Algorithm) -> Vec<Knob> {
    let mut v = vec![
        color_knob("Text Color:", SHARED_TEXT_COLOR_KEY),
        color_knob("Background Color:", SHARED_BACKGROUND_COLOR_KEY),
        Knob { label: "Custom Font".into(), kind: KnobKind::Font, key: SHARED_FONT_KEY.into(), placeholder: None, default: None },
        stretch_knob(algorithm),
        priority_knob(),
    ];
    v.extend(min_max_width_knobs());
    v
}

/// This milestone's framework registry, in iTerm2's palette order (`SETUP:107-134`): the
/// content classes (`statusbar_components::infos`) up through Composer, the Fixed-size
/// Spacer and the Spring (in that palette position), the rest of the content classes, then
/// the internal Placeholder shown when a layout has no components. 9i's five content
/// classes have an `Info`, knobs, and an exemplar here now (Task 1); they stay off screen
/// only because `Piece::of` (`src/app/statusbar_ui.rs`) does not map their `Kind` yet.
pub fn registry() -> Vec<Info> {
    let content = crate::statusbar_components::infos();
    // `infos` gives every content kind in one run, Composer immediately followed by
    // Interpolated String; the palette threads the two framework classes in between.
    let split = content.iter().position(|i| i.class == crate::statusbar_components::COMPOSER_CLASS).map_or(content.len(), |i| i + 1);
    let mut v = Vec::with_capacity(content.len() + 3);
    v.extend_from_slice(&content[..split]);
    v.push(Info {
        class: SPACER_CLASS,
        short: "Fixed-size Spacer",
        detail: "Adds a fixed amount of space",
        exemplar: "╠══╣",
        stretch: false,
        margins: true,
        internal: false,
    });
    v.push(Info {
        class: SPRING_CLASS,
        // Tightly Packed's naming ("Spring"); under Stable this component displays
        // as "Empty Space" with a matching exemplar and detail text: `info_for`.
        short: "Spring",
        detail: "Pushes items apart. Use one spring to right-align status bar elements that follow it. Use two to center those inbetween.",
        exemplar: "║〜〜║",
        stretch: true,
        margins: false,
        internal: false,
    });
    v.extend_from_slice(&content[split..]);
    v.push(Info {
        class: PLACEHOLDER_CLASS,
        short: "Placeholder",
        detail: "Placeholder",
        // iTerm2's own exemplar method asserts false: the placeholder never appears
        // in the setup picker (it is internal).
        exemplar: "",
        stretch: true,
        margins: true,
        internal: true,
    });
    v
}

pub fn info(class: &str) -> Option<Info> {
    registry().into_iter().find(|i| i.class == class)
}

/// The knob descriptors `statusBarComponentKnobs` returns, in order, for a class this
/// milestone knows. A class not here yet (the 9i content components, or one Tandem truly
/// does not recognize) returns no knobs: it has no Configure sheet, only Hide.
pub fn knobs(class: &str, algorithm: Algorithm) -> Vec<Knob> {
    match class {
        SPRING_CLASS => {
            let (label, key, default) = match algorithm {
                Algorithm::TightlyPacked => ("Compression Resistance:", SPRING_CONSTANT_KEY, 0.01),
                Algorithm::Stable => ("Size Multiple:", SIZE_MULTIPLE_KEY, 1.0),
            };
            vec![
                Knob { label: label.into(), kind: KnobKind::Double, key: key.into(), placeholder: Some(String::new()), default: Some(Value::Float(default)) },
                color_knob("Color", SHARED_BACKGROUND_COLOR_KEY),
            ]
        }
        SPACER_CLASS => vec![
            color_knob("Color", SHARED_BACKGROUND_COLOR_KEY),
            Knob { label: "Width".into(), kind: KnobKind::Double, key: SPACER_WIDTH_KEY.into(), placeholder: None, default: Some(Value::Float(5.0)) },
            priority_knob(),
        ],
        _ => crate::statusbar_components::Kind::of(class).map_or_else(Vec::new, |kind| crate::statusbar_components::knobs(kind, algorithm)),
    }
}

/// `info` as the setup window shows it under `algorithm`: the Spring is "Spring" with the
/// `║〜〜║` exemplar under Tightly Packed, and "Empty Space" with seven spaces under Stable
/// (`SPR:32-58`). Every other class is the same under either.
pub fn info_for(class: &str, algorithm: Algorithm) -> Option<Info> {
    let i = info(class)?;
    Some(if class == SPRING_CLASS && algorithm == Algorithm::Stable { Info { short: "Empty Space", detail: "Empty space that draws only a background color.", exemplar: "       ", ..i } } else { i })
}

/// The setup window's component menu, in iTerm2's order (`SETUP:107-134`): the four graph
/// classes, the rest of the content classes up through Composer (git state threaded in
/// after Triggers Menu), the Fixed-size Spacer and Spring at their iTerm2 position, then
/// Interpolated String and Call Script Function.
pub const PALETTE: [&str; 21] = [
    crate::statusbar_components::BATTERY_CLASS,
    crate::statusbar_components::CPU_CLASS,
    crate::statusbar_components::MEMORY_CLASS,
    crate::statusbar_components::NETWORK_CLASS,
    crate::statusbar_components::CLOCK_CLASS,
    crate::statusbar_components::CUSTOM_ACTION_CLASS,
    crate::statusbar_components::ACTIONS_MENU_CLASS,
    crate::statusbar_components::SNIPPETS_MENU_CLASS,
    crate::statusbar_components::TRIGGERS_MENU_CLASS,
    crate::statusbar_components::GIT_CLASS,
    crate::statusbar_components::HOST_NAME_CLASS,
    crate::statusbar_components::USER_NAME_CLASS,
    crate::statusbar_components::JOB_NAME_CLASS,
    crate::statusbar_components::CURRENT_DIRECTORY_CLASS,
    crate::statusbar_components::SEARCH_TOOL_CLASS,
    crate::statusbar_components::FILTER_TOOL_CLASS,
    crate::statusbar_components::COMPOSER_CLASS,
    SPACER_CLASS,
    SPRING_CLASS,
    crate::statusbar_components::INTERPOLATED_STRING_CLASS,
    crate::statusbar_components::CALL_SCRIPT_FUNCTION_CLASS,
];

/// `"layout advanced configuration dictionary value"`: the copy of the layout's advanced
/// configuration iTerm2 keeps in each component's configuration.
pub const ADVANCED_COPY_KEY: &str = "layout advanced configuration dictionary value";

/// `statusBarComponentDefaultKnobs`: what a component dropped from the palette starts with.
pub fn default_knobs(class: &str) -> toml::Table {
    let mut t = toml::Table::new();
    t.insert(BASE_COMPRESSION_RESISTANCE_KEY.into(), Value::Float(1.0));
    t.insert(BASE_PRIORITY_KEY.into(), Value::Float(5.0));
    match class {
        SPRING_CLASS => {
            t.insert(SPRING_CONSTANT_KEY.into(), Value::Float(0.01));
            t.insert(SIZE_MULTIPLE_KEY.into(), Value::Float(1.0));
        }
        SPACER_CLASS => {
            t.insert(SPACER_WIDTH_KEY.into(), Value::Float(5.0));
        }
        _ => {
            if let Some(kind) = crate::statusbar_components::Kind::of(class) {
                t.extend(crate::statusbar_components::default_knobs(kind));
            }
        }
    }
    t
}

/// `statusBarComponentExemplarWithBackgroundColor:textColor:` for the setup window's
/// palette and Active Components: the content module's dynamic exemplar (Task 2's text
/// formatter supplies `clock_text`; `user` and `home` are the caller's own OS lookups) when
/// it knows `class`, else this module's own static one under `algorithm` (the Spring,
/// whose exemplar and name both switch with it, `info_for`; the Fixed-size Spacer, the
/// Placeholder, or a class from a later milestone, none of which need live data).
pub fn exemplar(class: &str, knobs: &toml::Table, clock_text: &str, user: &str, home: &str, algorithm: Algorithm) -> String {
    crate::statusbar_components::exemplar(class, knobs, clock_text, user, home).unwrap_or_else(|| info_for(class, algorithm).map_or_else(String::new, |i| i.exemplar.to_string()))
}

/// A drop from the palette (`DCV:320-360`): a new component of `class` with its default
/// knobs goes before `index` (clamped to the end). Returns where it went.
pub fn insert_component(layout: &mut Layout, index: usize, class: &str) -> usize {
    let mut config = toml::Table::new();
    config.insert(ADVANCED_COPY_KEY.into(), Value::Table(layout.advanced.to_table()));
    let at = index.min(layout.components.len());
    layout.components.insert(at, Component { class: class.to_string(), knobs: default_knobs(class), config });
    at
}

/// A drag within Active Components (`DCV:340-356`): the component at `from` goes before
/// the one that was at `to` (`to` may be the length: last). Returns its new index; a bad
/// `from` changes nothing and returns it.
pub fn move_component(layout: &mut Layout, from: usize, to: usize) -> usize {
    let n = layout.components.len();
    if from >= n {
        return from;
    }
    let to = to.min(n);
    let c = layout.components.remove(from);
    let at = if from < to { to - 1 } else { to };
    layout.components.insert(at, c);
    at
}

/// The Configure Component sheet's values (`KVC:60-70`, `KVC:125-141`): each of the
/// class's knobs under `algorithm`, at its saved value or else its default; a knob with
/// neither is left out, and so is any key no knob shows.
pub fn knob_values(c: &Component, algorithm: Algorithm) -> toml::Table {
    knobs(&c.class, algorithm).into_iter().filter_map(|k| c.knobs.get(&k.key).cloned().or(k.default).map(|v| (k.key, v))).collect()
}

/// Two knob values equal within iTerm2's 0.0001 (`isApproximatelyEqualToObject:epsilon:`,
/// for the color picker's perturbations).
fn approx_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(Value::Table(x)), Some(Value::Table(y))) => x.len() == y.len() && x.iter().all(|(k, v)| approx_eq(Some(v), y.get(k))),
        (Some(x), Some(y)) => match (x.as_float().or_else(|| x.as_integer().map(|i| i as f64)), y.as_float().or_else(|| y.as_integer().map(|i| i as f64))) {
            (Some(p), Some(q)) => p == q || (p - q).abs() < 0.0001,
            _ => x == y,
        },
        (None, None) => true,
        _ => false,
    }
}

/// OK in the Configure Component sheet (`statusBarComponentSetKnobValues:`, `BC:276-302`):
/// the component's knobs become `values`, and a changed text color turns auto-rainbow off
/// (`DCV:421-452`). A bad index changes nothing.
pub fn set_knob_values(layout: &mut Layout, index: usize, values: toml::Table) {
    let Some(c) = layout.components.get_mut(index) else { return };
    let text_changed = values.contains_key(SHARED_TEXT_COLOR_KEY) && !approx_eq(c.knobs.get(SHARED_TEXT_COLOR_KEY), values.get(SHARED_TEXT_COLOR_KEY));
    c.knobs = values;
    if text_changed {
        layout.advanced.rainbow = Rainbow::Disabled;
    }
}

/// Each component's copy of the advanced configuration becomes the layout's, as iTerm2's
/// layout has it after building itself again from its dictionary (`L:122-123`).
pub fn sync_advanced_copies(layout: &mut Layout) {
    let copy = Value::Table(layout.advanced.to_table());
    for c in layout.components.iter_mut() {
        c.config.insert(ADVANCED_COPY_KEY.into(), copy.clone());
    }
}

/// `NSColor colorWithHue:saturation:brightness:alpha:`'s RGB, each 0 to 1. Shared with the
/// app side (`statusbar_ui::hsb`), which converts this to its own `Rgb` byte type; this
/// module only ever needs the `{r, g, b, a}` table `hsb_value` builds from it.
pub fn hsb_to_rgb(h: f32, s: f32, b: f32) -> (f32, f32, f32) {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let f = h6 - h6.floor();
    let (p, q, t) = (b * (1.0 - s), b * (1.0 - s * f), b * (1.0 - s * (1.0 - f)));
    match h6.floor() as i32 {
        0 => (b, t, p),
        1 => (q, b, p),
        2 => (p, b, t),
        3 => (p, q, b),
        4 => (t, p, b),
        _ => (b, p, q),
    }
}

/// `NSColor colorWithHue:saturation:brightness:alpha:1` as a `{r, g, b, a}` table.
fn hsb_value(h: f32, s: f32, b: f32) -> Value {
    let (r, g, bl) = hsb_to_rgb(h, s, b);
    let mut out = toml::Table::new();
    for (k, v) in [("r", r), ("g", g), ("b", bl), ("a", 1.0)] {
        out.insert(k.into(), Value::Float(v as f64));
    }
    Value::Table(out)
}

/// The setup window's auto-rainbow (`DCV:128-142`): every component in the layout gets
/// its rainbow color in its `"shared text color"` knob. Disabled leaves the knobs alone
/// (colors an earlier style wrote stay, as in iTerm2).
pub fn apply_rainbow(layout: &mut Layout, dark_background: bool) {
    let colors = rainbow_colors(layout.components.len(), layout.advanced.rainbow, dark_background);
    for (c, (h, s, b)) in layout.components.iter_mut().zip(colors) {
        c.knobs.insert(SHARED_TEXT_COLOR_KEY.into(), hsb_value(h, s, b));
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

        // Review fix: an infinite knob (typed as the numeric knob's "\u{221e}") no longer
        // reaches the layout algorithms as infinity, which divided infinity by infinity
        // into NaN placements there; it is clamped to a large finite value instead. A knob
        // that is somehow NaN falls back to the same default as an absent one (1).
        spring.knobs.insert(SPRING_CONSTANT_KEY.into(), Value::Float(f64::INFINITY));
        assert!(spring.spring_constant(Algorithm::TightlyPacked).is_finite());
        spring.knobs.insert(SPRING_CONSTANT_KEY.into(), Value::Float(f64::NAN));
        assert_eq!(spring.spring_constant(Algorithm::TightlyPacked), 1.0);
    }

    #[test]
    fn framework_components_follow_iterm2() {
        let reg = registry();
        // The framework's own 3 (Spring, Fixed-size Spacer, Placeholder) plus the content
        // registry (19 palette classes, 3 internal): `statusbar_components::infos().len()`.
        assert_eq!(reg.len(), 25);

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

    #[test]
    fn info_for_follows_the_algorithm() {
        let tight = info_for(SPRING_CLASS, Algorithm::TightlyPacked).unwrap();
        assert_eq!((tight.short, tight.exemplar), ("Spring", "║〜〜║"));
        assert!(tight.detail.starts_with("Pushes items apart."));
        let stable = info_for(SPRING_CLASS, Algorithm::Stable).unwrap();
        assert_eq!((stable.short, stable.exemplar, stable.detail), ("Empty Space", "       ", "Empty space that draws only a background color."));
        assert_eq!(stable.exemplar.chars().count(), 7);
        // Everything else is the same under either algorithm.
        assert_eq!(info_for(SPACER_CLASS, Algorithm::Stable), info(SPACER_CLASS));
        assert_eq!(info_for(SPACER_CLASS, Algorithm::TightlyPacked), info(SPACER_CLASS));
        assert_eq!(info_for("iTermStatusBarNoSuchComponent", Algorithm::Stable), None);
        // The palette: iTerm2's order, no internal class; the Fixed-size Spacer and Spring
        // (numbers 18 and 19 in iTerm2's own list) sit between Composer and Interpolated
        // String, where the content classes leave a gap for them (`content_registry_
        // follows_iterm2` in `statusbar_components.rs` checks the content classes' own
        // positions in full).
        assert_eq!(PALETTE[17], SPACER_CLASS);
        assert_eq!(PALETTE[18], SPRING_CLASS);
        assert!(PALETTE.iter().all(|&c| info(c).is_some_and(|i| !i.internal)));
        // PALETTE is exactly registry()'s own non-internal classes, in order: the two
        // never drift apart.
        assert_eq!(PALETTE.to_vec(), registry().into_iter().filter(|i| !i.internal).map(|i| i.class).collect::<Vec<_>>());
    }

    #[test]
    fn exemplar_follows_the_algorithm() {
        let none = toml::Table::new();
        // The Spring's own exemplar switches with the algorithm (`info_for`), unlike a
        // content class's, which `exemplar` resolves before ever falling back to it.
        assert_eq!(exemplar(SPRING_CLASS, &none, "", "", "", Algorithm::TightlyPacked), "║〜〜║");
        assert_eq!(exemplar(SPRING_CLASS, &none, "", "", "", Algorithm::Stable), "       ");
        // A content class: the fallback is never reached, so the algorithm makes no
        // difference to its own exemplar.
        let user_name = crate::statusbar_components::USER_NAME_CLASS;
        assert_eq!(exemplar(user_name, &none, "", "bob", "/home/bob", Algorithm::TightlyPacked), "bob");
        assert_eq!(exemplar(user_name, &none, "", "bob", "/home/bob", Algorithm::Stable), "bob");
    }

    #[test]
    fn setup_edits_follow_iterm2() {
        // A drop from the palette inserts a new component with the class's default knobs
        // (`statusBarComponentDefaultKnobs`) and an advanced-configuration copy.
        let mut l = Layout::default();
        l.advanced.algorithm = Algorithm::TightlyPacked;
        assert_eq!(insert_component(&mut l, 0, SPACER_CLASS), 0);
        let spacer = &l.components[0];
        assert_eq!(spacer.class, SPACER_CLASS);
        assert_eq!(spacer.knobs, table(&[(BASE_COMPRESSION_RESISTANCE_KEY, Value::Float(1.0)), (BASE_PRIORITY_KEY, Value::Float(5.0)), (SPACER_WIDTH_KEY, Value::Float(5.0))]));
        let injected = spacer.config.get(ADVANCED_COPY_KEY).and_then(Value::as_table).unwrap();
        assert_eq!(injected.get("algorithm"), Some(&Value::Integer(1)));
        // Drop before the spacer.
        assert_eq!(insert_component(&mut l, 0, SPRING_CLASS), 0);
        assert_eq!(
            l.components[0].knobs,
            table(&[(BASE_COMPRESSION_RESISTANCE_KEY, Value::Float(1.0)), (BASE_PRIORITY_KEY, Value::Float(5.0)), (SPRING_CONSTANT_KEY, Value::Float(0.01)), (SIZE_MULTIPLE_KEY, Value::Float(1.0))])
        );
        // Past the end goes last.
        assert_eq!(insert_component(&mut l, 99, SPACER_CLASS), 2);
        let classes = |l: &Layout| l.components.iter().map(|c| c.class.clone()).collect::<Vec<String>>();
        assert_eq!(classes(&l), vec![SPRING_CLASS, SPACER_CLASS, SPACER_CLASS]);

        // Reorder: `to` is the drop-before index in the list as it was (DCV acceptDrop).
        let named = |names: &[&str]| Layout { components: names.iter().map(|n| Component { class: n.to_string(), knobs: toml::Table::new(), config: toml::Table::new() }).collect(), ..Layout::default() };
        let mut m = named(&["a", "b", "c"]);
        assert_eq!(move_component(&mut m, 0, 2), 1);
        assert_eq!(classes(&m), vec!["b", "a", "c"]);
        let mut m = named(&["a", "b", "c"]);
        assert_eq!(move_component(&mut m, 2, 0), 0);
        assert_eq!(classes(&m), vec!["c", "a", "b"]);
        let mut m = named(&["a", "b", "c"]);
        assert_eq!(move_component(&mut m, 0, 3), 2);
        assert_eq!(classes(&m), vec!["b", "c", "a"]);
        // Dropped on itself, or just after itself: nothing moves.
        for to in [1, 2] {
            let mut m = named(&["a", "b", "c"]);
            assert_eq!(move_component(&mut m, 1, to), 1);
            assert_eq!(classes(&m), vec!["a", "b", "c"]);
        }
        let mut m = named(&["a", "b"]);
        assert_eq!(move_component(&mut m, 5, 0), 5, "a bad index leaves the layout alone");
        assert_eq!(classes(&m), vec!["a", "b"]);

        // The Configure sheet's values: each knob's saved value, else its default; a knob
        // with neither (Color) is left out, and keys no knob shows are dropped (KVC:125-141).
        let red = Value::Table(table(&[("r", Value::Float(1.0)), ("g", Value::Float(0.0)), ("b", Value::Float(0.0)), ("a", Value::Float(1.0))]));
        let spring = Component { class: SPRING_CLASS.into(), knobs: toml::Table::new(), config: toml::Table::new() };
        assert_eq!(knob_values(&spring, Algorithm::Stable), table(&[(SIZE_MULTIPLE_KEY, Value::Float(1.0))]));
        assert_eq!(knob_values(&spring, Algorithm::TightlyPacked), table(&[(SPRING_CONSTANT_KEY, Value::Float(0.01))]));
        let spacer = Component { class: SPACER_CLASS.into(), knobs: table(&[(SPACER_WIDTH_KEY, Value::Integer(30)), (SHARED_BACKGROUND_COLOR_KEY, red.clone()), ("stray", Value::Boolean(true))]), config: toml::Table::new() };
        assert_eq!(knob_values(&spacer, Algorithm::Stable), table(&[(SHARED_BACKGROUND_COLOR_KEY, red.clone()), (SPACER_WIDTH_KEY, Value::Integer(30)), (BASE_PRIORITY_KEY, Value::Float(5.0))]));
        assert!(knob_values(&Component { class: "iTermStatusBarNoSuchComponent".into(), knobs: table(&[("x", Value::Float(1.0))]), config: toml::Table::new() }, Algorithm::Stable).is_empty());

        // OK in the Configure sheet replaces the knobs wholesale (`statusBarComponentSetKnobValues:`).
        let mut l = Layout { components: vec![spacer.clone()], ..Layout::default() };
        l.advanced.rainbow = Rainbow::Light;
        set_knob_values(&mut l, 0, table(&[(SPACER_WIDTH_KEY, Value::Float(12.0))]));
        assert_eq!(l.components[0].knobs, table(&[(SPACER_WIDTH_KEY, Value::Float(12.0))]));
        assert_eq!(l.advanced.rainbow, Rainbow::Light, "no text color changed");
        // A changed text color turns auto-rainbow off (DCV:421-452); within 0.0001 is unchanged.
        l.components[0].knobs.insert(SHARED_TEXT_COLOR_KEY.into(), red.clone());
        let nearly_red = Value::Table(table(&[("r", Value::Float(0.99995)), ("g", Value::Float(0.0)), ("b", Value::Float(0.0)), ("a", Value::Float(1.0))]));
        set_knob_values(&mut l, 0, table(&[(SHARED_TEXT_COLOR_KEY, nearly_red)]));
        assert_eq!(l.advanced.rainbow, Rainbow::Light);
        let green = Value::Table(table(&[("r", Value::Float(0.0)), ("g", Value::Float(1.0)), ("b", Value::Float(0.0)), ("a", Value::Float(1.0))]));
        set_knob_values(&mut l, 0, table(&[(SHARED_TEXT_COLOR_KEY, green)]));
        assert_eq!(l.advanced.rainbow, Rainbow::Disabled);
        set_knob_values(&mut l, 7, toml::Table::new());
        assert_eq!(l.components.len(), 1, "a bad index changes nothing");

        // Auto-rainbow in the setup window writes each component's text color knob, every
        // component in the layout (DCV:128-142); Disabled writes nothing.
        let mut l = named(&["a", "b", "c"]);
        l.advanced.rainbow = Rainbow::Light;
        apply_rainbow(&mut l, false);
        let rgb = |c: &Component| {
            let t = c.knobs.get(SHARED_TEXT_COLOR_KEY).and_then(Value::as_table).unwrap().clone();
            ["r", "g", "b", "a"].map(|k| t.get(k).and_then(Value::as_float).unwrap())
        };
        let first = rgb(&l.components[0]);
        for (got, want) in first.iter().zip([0.9, 0.63, 0.63, 1.0]) {
            assert!((got - want).abs() < 1e-4, "{first:?}");
        }
        assert_ne!(rgb(&l.components[1]), first);
        let mut off = named(&["a"]);
        apply_rainbow(&mut off, true);
        assert!(off.components[0].knobs.is_empty());

        // After an Advanced change every component's copy follows the layout's.
        l.advanced.algorithm = Algorithm::TightlyPacked;
        sync_advanced_copies(&mut l);
        assert!(l.components.iter().all(|c| c.config.get(ADVANCED_COPY_KEY) == Some(&Value::Table(l.advanced.to_table()))));
    }
}
