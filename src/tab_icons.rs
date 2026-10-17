//! Tab icons (milestone 13d): the picture a tab shows for its session, as iTerm2's
//! `iTermGraphicSource` (`GS`) picks it. A profile's Icon is None, Automatic (the running
//! command's built-in icon from iTerm2's MIT icon set, tinted, else the command's first letter
//! in a colored circle) or Custom (an image file). The icon maps are iTerm2's bundled
//! `graphic_icons.json` and `graphic_colors.json` with the user's copies in `%APPDATA%\Termurion`
//! merged over them (`GS:83-200`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::palette::{Rgb, rgb};

/// Profile Icon values (`iTermProfileIcon`, `PS:6659-6684`).
pub const NONE: i64 = 0;
pub const AUTOMATIC: i64 = 1;
pub const CUSTOM: i64 = 2;

/// The user's icon map and color map in `%APPDATA%\Termurion` (`iTermRemoteDataFileSync`
/// `graphicIconsName`, `graphicColorsName`; 13y2 syncs them).
pub const ICONS_FILE: &str = "graphic_icons.json";
pub const COLORS_FILE: &str = "graphic_colors.json";

/// How many parents of the job the walk looks at (`GS imageForProcessID:`).
const MAX_ANCESTORS: usize = 50;
/// The tint when neither the command nor the logical name has a color ("#888").
const DEFAULT_TINT: Rgb = rgb(0x88, 0x88, 0x88);
/// The letter icon's color is rehashed until its perceived brightness is in this range
/// (`GS randomTintColorForString:`).
const LETTER_BRIGHTNESS: (f64, f64) = (0.30, 0.50);

const BUNDLED_ICONS: &str = include_str!("../resources/tab_icons/graphic_icons.json");
const BUNDLED_COLORS: &str = include_str!("../resources/tab_icons/graphic_colors.json");

/// The icon maps: `icons` is command to logical names (iTerm2's `graphic_icons.json`
/// inverted), `colors` command or logical name to a hex color. A color value that was not a
/// string in its file is kept as "" so it still shadows the bundled value and reads as absent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Maps {
    pub icons: HashMap<String, Vec<String>>,
    pub colors: HashMap<String, String>,
}

/// Drops a comma that only whitespace separates from a closing `]` or `}`, outside strings.
/// iTerm2's bundled `graphic_icons.json` has one (`"pico",`), which strict JSON rejects.
fn lenient(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let (mut in_string, mut escaped) = (false, false);
    for (i, &c) in chars.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if c == ',' && matches!(chars[i + 1..].iter().find(|c| !c.is_whitespace()), Some(']' | '}')) {
            continue;
        }
        out.push(c);
    }
    out
}

/// A JSON object, leniently parsed; None when the bytes are not one.
fn parse(bytes: &[u8]) -> Option<serde_json::Map<String, serde_json::Value>> {
    let text = std::str::from_utf8(bytes).ok()?;
    match serde_json::from_str(&lenient(text)).ok()? {
        serde_json::Value::Object(map) => Some(map),
        _ => None,
    }
}

/// `it_invertedGraphicDictionary`: logical name to commands becomes command to logical names,
/// in file order; a value that is not an array, and an element that is not a string, is
/// skipped.
fn invert(map: &serde_json::Map<String, serde_json::Value>) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for (logical, commands) in map {
        let Some(commands) = commands.as_array() else { continue };
        for command in commands.iter().filter_map(|c| c.as_str()) {
            out.entry(command.to_string()).or_default().push(logical.clone());
        }
    }
    out
}

/// The color map; a value that is not a string is kept as "" (`GS:393-399` casts it away and
/// falls through to the next lookup).
fn colors(map: &serde_json::Map<String, serde_json::Value>) -> HashMap<String, String> {
    map.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect()
}

fn bundled() -> &'static Maps {
    static MAPS: OnceLock<Maps> = OnceLock::new();
    MAPS.get_or_init(|| Maps {
        icons: parse(BUNDLED_ICONS.as_bytes()).map(|m| invert(&m)).unwrap_or_default(),
        colors: parse(BUNDLED_COLORS.as_bytes()).map(|m| colors(&m)).unwrap_or_default(),
    })
}

/// One table (`GS loadGraphicMapFromBundleResource:appSupportName:previousMap:transform:`): the
/// bundled table, a present and parseable user file merged over it key by key, a present but
/// unparseable one keeping `previous`.
fn load_table<T: Clone>(bundled: &HashMap<String, T>, user: Option<&[u8]>, previous: Option<&HashMap<String, T>>, read: impl Fn(&serde_json::Map<String, serde_json::Value>) -> HashMap<String, T>) -> HashMap<String, T> {
    let Some(bytes) = user else { return bundled.clone() };
    match parse(bytes) {
        Some(map) => {
            let mut out = bundled.clone();
            out.extend(read(&map));
            out
        }
        None => previous.cloned().unwrap_or_else(|| bundled.clone()),
    }
}

/// The icon maps (`GS reloadGraphicMaps`) from the user's two files' bytes (None when absent)
/// and the maps loaded before (kept for a file that no longer parses).
pub fn load_maps(user_icons: Option<&[u8]>, user_colors: Option<&[u8]>, previous: Option<&Maps>) -> Maps {
    let b = bundled();
    Maps {
        icons: load_table(&b.icons, user_icons, previous.map(|p| &p.icons), invert),
        colors: load_table(&b.colors, user_colors, previous.map(|p| &p.colors), colors),
    }
}

/// `GS normalizedCommand:`: "Emacs-"/"emacs-" anything is "emacs"; "Python"/"python" followed
/// by no letter (python3.12) is "python".
pub fn normalized(command: &str) -> String {
    if command.starts_with("Emacs-") || command.starts_with("emacs-") {
        return "emacs".into();
    }
    if let Some(suffix) = command.strip_prefix("Python").or_else(|| command.strip_prefix("python"))
        && !suffix.chars().any(char::is_alphabetic) {
            return "python".into();
        }
    command.to_string()
}

/// The command's logical icon name, the first the map lists for it (`GS:370`).
pub fn logical_name(maps: &Maps, command: &str) -> Option<String> {
    maps.icons.get(&normalized(command)).and_then(|l| l.first()).cloned()
}

/// The picture's tint (`GS:393-402`): the command's color, else the logical name's, else
/// #888888; a value that is not a hex color is absent.
pub fn tint(maps: &Maps, command: &str, logical: &str) -> Rgb {
    let of = |k: &str| maps.colors.get(k).and_then(|c| crate::hexcolor::parse(c));
    of(&normalized(command)).or_else(|| of(logical)).unwrap_or(DEFAULT_TINT)
}

/// A bundled PNG by file name.
fn embedded(file: &str) -> Option<&'static [u8]> {
    Some(match file {
        "graphic_atom.png" => include_bytes!("../resources/tab_icons/png/graphic_atom.png"),
        "graphic_atom@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_atom@2x.png"),
        "graphic_bower.png" => include_bytes!("../resources/tab_icons/png/graphic_bower.png"),
        "graphic_bower@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_bower@2x.png"),
        "graphic_bq.png" => include_bytes!("../resources/tab_icons/png/graphic_bq.png"),
        "graphic_bq@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_bq@2x.png"),
        "graphic_bullhorn.png" => include_bytes!("../resources/tab_icons/png/graphic_bullhorn.png"),
        "graphic_bullhorn@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_bullhorn@2x.png"),
        "graphic_claude_code.png" => include_bytes!("../resources/tab_icons/png/graphic_claude_code.png"),
        "graphic_claude_code@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_claude_code@2x.png"),
        "graphic_clojure.png" => include_bytes!("../resources/tab_icons/png/graphic_clojure.png"),
        "graphic_clojure@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_clojure@2x.png"),
        "graphic_code.png" => include_bytes!("../resources/tab_icons/png/graphic_code.png"),
        "graphic_code@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_code@2x.png"),
        "graphic_coffee.png" => include_bytes!("../resources/tab_icons/png/graphic_coffee.png"),
        "graphic_coffee@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_coffee@2x.png"),
        "graphic_compile.png" => include_bytes!("../resources/tab_icons/png/graphic_compile.png"),
        "graphic_compile@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_compile@2x.png"),
        "graphic_composer.png" => include_bytes!("../resources/tab_icons/png/graphic_composer.png"),
        "graphic_composer@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_composer@2x.png"),
        "graphic_curl.png" => include_bytes!("../resources/tab_icons/png/graphic_curl.png"),
        "graphic_curl@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_curl@2x.png"),
        "graphic_database.png" => include_bytes!("../resources/tab_icons/png/graphic_database.png"),
        "graphic_database@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_database@2x.png"),
        "graphic_docker.png" => include_bytes!("../resources/tab_icons/png/graphic_docker.png"),
        "graphic_docker@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_docker@2x.png"),
        "graphic_elixir.png" => include_bytes!("../resources/tab_icons/png/graphic_elixir.png"),
        "graphic_elixir@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_elixir@2x.png"),
        "graphic_emacs.png" => include_bytes!("../resources/tab_icons/png/graphic_emacs.png"),
        "graphic_emacs@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_emacs@2x.png"),
        "graphic_erlang.png" => include_bytes!("../resources/tab_icons/png/graphic_erlang.png"),
        "graphic_erlang@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_erlang@2x.png"),
        "graphic_ethereum.png" => include_bytes!("../resources/tab_icons/png/graphic_ethereum.png"),
        "graphic_ethereum@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_ethereum@2x.png"),
        "graphic_git.png" => include_bytes!("../resources/tab_icons/png/graphic_git.png"),
        "graphic_git@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_git@2x.png"),
        "graphic_go.png" => include_bytes!("../resources/tab_icons/png/graphic_go.png"),
        "graphic_go@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_go@2x.png"),
        "graphic_grunt.png" => include_bytes!("../resources/tab_icons/png/graphic_grunt.png"),
        "graphic_grunt@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_grunt@2x.png"),
        "graphic_gulp.png" => include_bytes!("../resources/tab_icons/png/graphic_gulp.png"),
        "graphic_gulp@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_gulp@2x.png"),
        "graphic_heroku.png" => include_bytes!("../resources/tab_icons/png/graphic_heroku.png"),
        "graphic_heroku@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_heroku@2x.png"),
        "graphic_http.png" => include_bytes!("../resources/tab_icons/png/graphic_http.png"),
        "graphic_http@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_http@2x.png"),
        "graphic_java.png" => include_bytes!("../resources/tab_icons/png/graphic_java.png"),
        "graphic_java@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_java@2x.png"),
        "graphic_less.png" => include_bytes!("../resources/tab_icons/png/graphic_less.png"),
        "graphic_less@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_less@2x.png"),
        "graphic_micro.png" => include_bytes!("../resources/tab_icons/png/graphic_micro.png"),
        "graphic_micro@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_micro@2x.png"),
        "graphic_monitor.png" => include_bytes!("../resources/tab_icons/png/graphic_monitor.png"),
        "graphic_monitor@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_monitor@2x.png"),
        "graphic_mysql.png" => include_bytes!("../resources/tab_icons/png/graphic_mysql.png"),
        "graphic_mysql@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_mysql@2x.png"),
        "graphic_nano.png" => include_bytes!("../resources/tab_icons/png/graphic_nano.png"),
        "graphic_nano@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_nano@2x.png"),
        "graphic_neovim.png" => include_bytes!("../resources/tab_icons/png/graphic_neovim.png"),
        "graphic_neovim@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_neovim@2x.png"),
        "graphic_nginx.png" => include_bytes!("../resources/tab_icons/png/graphic_nginx.png"),
        "graphic_nginx@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_nginx@2x.png"),
        "graphic_nodejs.png" => include_bytes!("../resources/tab_icons/png/graphic_nodejs.png"),
        "graphic_nodejs@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_nodejs@2x.png"),
        "graphic_npm.png" => include_bytes!("../resources/tab_icons/png/graphic_npm.png"),
        "graphic_npm@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_npm@2x.png"),
        "graphic_perl.png" => include_bytes!("../resources/tab_icons/png/graphic_perl.png"),
        "graphic_perl@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_perl@2x.png"),
        "graphic_php.png" => include_bytes!("../resources/tab_icons/png/graphic_php.png"),
        "graphic_php@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_php@2x.png"),
        "graphic_postgres.png" => include_bytes!("../resources/tab_icons/png/graphic_postgres.png"),
        "graphic_postgres@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_postgres@2x.png"),
        "graphic_psql.png" => include_bytes!("../resources/tab_icons/png/graphic_psql.png"),
        "graphic_psql@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_psql@2x.png"),
        "graphic_python.png" => include_bytes!("../resources/tab_icons/png/graphic_python.png"),
        "graphic_python@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_python@2x.png"),
        "graphic_rails.png" => include_bytes!("../resources/tab_icons/png/graphic_rails.png"),
        "graphic_rails@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_rails@2x.png"),
        "graphic_read.png" => include_bytes!("../resources/tab_icons/png/graphic_read.png"),
        "graphic_read@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_read@2x.png"),
        "graphic_ruby.png" => include_bytes!("../resources/tab_icons/png/graphic_ruby.png"),
        "graphic_ruby@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_ruby@2x.png"),
        "graphic_search.png" => include_bytes!("../resources/tab_icons/png/graphic_search.png"),
        "graphic_search@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_search@2x.png"),
        "graphic_shell.png" => include_bytes!("../resources/tab_icons/png/graphic_shell.png"),
        "graphic_shell@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_shell@2x.png"),
        "graphic_vim.png" => include_bytes!("../resources/tab_icons/png/graphic_vim.png"),
        "graphic_vim@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_vim@2x.png"),
        "graphic_yarn.png" => include_bytes!("../resources/tab_icons/png/graphic_yarn.png"),
        "graphic_yarn@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_yarn@2x.png"),
        "graphic_yo.png" => include_bytes!("../resources/tab_icons/png/graphic_yo.png"),
        "graphic_yo@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_yo@2x.png"),
        "graphic_zip.png" => include_bytes!("../resources/tab_icons/png/graphic_zip.png"),
        "graphic_zip@2x.png" => include_bytes!("../resources/tab_icons/png/graphic_zip@2x.png"),
        _ => return None,
    })
}

/// The bundled picture for a logical name (`NSImage it_imageNamed:`): the double-size PNG,
/// else the single one.
pub fn png(logical: &str) -> Option<&'static [u8]> {
    embedded(&format!("graphic_{logical}@2x.png")).or_else(|| embedded(&format!("graphic_{logical}.png")))
}

/// The user's picture for a logical name with no bundled one (`GS:383-388`), read at 16 by 16
/// points.
pub fn user_png_path(dir: &Path, logical: &str) -> PathBuf {
    dir.join(format!("graphic_{logical}.png"))
}

/// `it_imageWithTintColor:`: every pixel painted the tint, its alpha kept.
pub fn tinted(rgba: &[u8], tint: Rgb) -> Vec<u8> {
    rgba.chunks_exact(4).flat_map(|p| [tint.r, tint.g, tint.b, p[3]]).collect()
}

/// `hashWithDJB2` over the string's UTF-16 units, 64-bit wrapping.
pub fn djb2(s: &str) -> u64 {
    s.encode_utf16().fold(5381u64, |h, u| h.wrapping_mul(33) ^ u64::from(u))
}

/// `iTermPerceptualBrightnessSRGB`: linearized sRGB weighted 0.2126, 0.7152, 0.0722.
fn brightness(r: f64, g: f64, b: f64) -> f64 {
    let lin = |n: f64| if n > 0.04045 { ((n + 0.055) / 1.055).powf(2.4) } else { n / 12.92 };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// The letter icon's color (`GS randomTintColorForString:`): the low three bytes of the
/// string's DJB2 hash; while its brightness is outside 0.30 to 0.50 the components are
/// appended as "%0.2f%0.2f%0.2f" and the string rehashed.
pub fn letter_color(job: &str) -> Rgb {
    let mut acc = job.to_string();
    // iTerm2 loops unbounded; the cap only guards the UI thread.
    for _ in 0..10_000 {
        let h = djb2(&acc);
        let (r, g, b) = ((h & 255) as u8, ((h >> 8) & 255) as u8, ((h >> 16) & 255) as u8);
        let (fr, fg, fb) = (f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(b) / 255.0);
        let lum = brightness(fr, fg, fb);
        if (LETTER_BRIGHTNESS.0..=LETTER_BRIGHTNESS.1).contains(&lum) {
            return rgb(r, g, b);
        }
        acc.push_str(&format!("{fr:.2}{fg:.2}{fb:.2}"));
    }
    DEFAULT_TINT
}

/// The job's first composed character, upper-cased (`firstComposedCharacter:`,
/// `uppercaseString`): the first character with the zero-width ones that follow it (combining
/// marks, variation selectors) and whatever a zero-width joiner joins.
pub fn first_letter(job: &str) -> Option<String> {
    use unicode_width::UnicodeWidthChar;
    let mut chars = job.chars().peekable();
    let mut out = String::from(chars.next()?);
    let mut joined = false;
    while let Some(&c) = chars.peek() {
        if !(joined || c.width() == Some(0)) {
            break;
        }
        joined = c == '\u{200d}';
        out.push(c);
        chars.next();
    }
    Some(out.to_uppercase())
}

/// A command's first shell word's last path component without ".exe"
/// (`componentsInShellCommand`, `lastPathComponent`).
fn first_word(raw: &str) -> String {
    let s = raw.trim_start();
    let word = match s.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => s.split(char::is_whitespace).next().unwrap_or(s),
    };
    crate::proctree::strip_exe(word.rsplit(['\\', '/']).next().unwrap_or(word)).to_string()
}

/// The names a process's icon is looked up by (`GS iconCandidateNamesForProcessInfo:`): its
/// name, argv0 and command line, each's first word, deduplicated, empty ones dropped.
pub fn candidates(name: &str, argv0: Option<&str>, command_line: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in [Some(name), argv0, command_line].into_iter().flatten() {
        let first = first_word(raw);
        if !first.is_empty() && !out.contains(&first) {
            out.push(first);
        }
    }
    out
}

/// The fallback name for the letter icon (`GS preferredFallbackNameForProcessInfo:`): argv0,
/// else the command line, else the name, as its first word.
pub fn fallback_name(name: &str, argv0: Option<&str>, command_line: Option<&str>) -> String {
    first_word(argv0.or(command_line).unwrap_or(name))
}

/// The ancestor walk ends at a login shell ("-" first) or at the host that owns the shells
/// ("termurion-host", iTerm2's "iTermServer").
pub fn stop_at(title: &str) -> bool {
    title.starts_with('-') || title.get(.."termurion-host".len()).is_some_and(|p| p.eq_ignore_ascii_case("termurion-host"))
}

/// The lookup chain's tail (`GS:222-236`): each ancestor of the job (nearest first) up to 50,
/// ending before the first `stop_at`, as its candidate names.
pub fn ancestor_chain(ancestors: &[String]) -> Vec<Vec<String>> {
    ancestors.iter().take(MAX_ANCESTORS).take_while(|a| !stop_at(a)).map(|a| candidates(a, None, None)).collect()
}

/// What a tab shows for its session.
#[derive(Clone, Debug, PartialEq)]
pub enum Icon {
    None,
    /// A bundled or user picture by logical name, and its tint.
    Picture(String, Rgb),
    /// The command's first letter in a circle of the color.
    Letter(String, Rgb),
    /// The profile's Custom Icon Path.
    Custom(PathBuf),
}

/// The icon for a session (`PS tabGraphicForProfile:`, `GS imageForProcessID:`): None shows
/// nothing; Custom its path; Automatic the first command in `chain` (the job's candidates, then
/// each ancestor's) whose logical name has a picture (`has_picture`: bundled, or the user's
/// PNG), else the fallback name's picture, else with `letters` (`DefaultIconsUsingLetters`) the
/// fallback's first letter. With no job (`fallback` None) nothing shows.
pub fn resolve(maps: &Maps, setting: i64, custom_path: &str, chain: &[Vec<String>], fallback: Option<&str>, letters: bool, has_picture: &dyn Fn(&str) -> bool) -> Icon {
    match setting {
        CUSTOM if !custom_path.is_empty() => Icon::Custom(PathBuf::from(custom_path)),
        AUTOMATIC => {
            let Some(fallback) = fallback else { return Icon::None };
            let picture = |command: &str| logical_name(maps, command).filter(|l| has_picture(l)).map(|l| Icon::Picture(l.clone(), tint(maps, command, &l)));
            if let Some(icon) = chain.iter().flatten().map(String::as_str).chain([fallback]).find_map(picture) {
                return icon;
            }
            // `GS defaultImageForCommand:`.
            match first_letter(fallback) {
                Some(letter) if letters && !normalized(fallback).is_empty() => Icon::Letter(letter, letter_color(fallback)),
                _ => Icon::None,
            }
        }
        _ => Icon::None,
    }
}

/// Whether a path is read for an icon or a map: never one naming a Windows device (`COM1`,
/// `NUL`, `CON.png`, `C:\x\LPT1.txt`, `COM1:`; `device_path`), which opening can block on;
/// never a network or device-namespace path, however spelled (`\\server`, `\\.\COM1`,
/// `scripts::is_network_path`); and never one reached through a symbolic link or junction to
/// one, at the file or any folder above it (`scripts::ancestors_stay_local`, reading each link
/// with `read_link`). The device check comes first, so no link is read on a device.
pub fn readable_path(path: &Path, read_link: &dyn Fn(&Path) -> Option<PathBuf>) -> bool {
    !device_path(path) && crate::scripts::ancestors_stay_local(path, read_link)
}

/// Whether any part of a path, between separators or a drive's or device's colon, is a
/// Windows device name with or without an extension (`zip_check::device_name`): Windows opens
/// the device for such a path, in any folder.
pub fn device_path(path: &Path) -> bool {
    path.to_string_lossy().split(['\\', '/', ':']).any(crate::zip_check::device_name)
}

/// The largest user icon or map file read, in bytes; a larger one is not read at all.
pub const MAX_FILE_BYTES: u64 = 8 << 20;

/// The largest picture decoded for an icon, per side in pixels.
pub const MAX_DECODE_SIDE: u32 = 4096;

/// What decoding one icon picture may allocate, in bytes (a `MAX_DECODE_SIDE` square of RGBA).
const MAX_DECODE_ALLOC: u64 = 80 << 20;

/// An icon file's bytes decoded to RGBA, within `MAX_DECODE_SIDE` a side and
/// `MAX_DECODE_ALLOC`: a picture whose header claims more fails at the header, before any
/// pixel buffer is made. None when it is not a picture or goes past the limits.
pub fn decode_limited(bytes: &[u8]) -> Option<image::RgbaImage> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    reader.limits(limits);
    Some(reader.decode().ok()?.into_rgba8())
}

/// Whether a logical name may name a user PNG: a plain file name part, no separators.
pub fn plain_logical(logical: &str) -> bool {
    !logical.is_empty() && !logical.contains(['\\', '/', ':']) && !logical.contains("..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(_: &str) -> bool {
        true
    }

    #[test]
    fn icon_lookup_follows_iterm2() {
        assert_eq!(normalized("Emacs-29"), "emacs");
        assert_eq!(normalized("python3.12"), "python");
        assert_eq!(normalized("pythonista"), "pythonista");
        let m = load_maps(None, None, None);
        assert_eq!(logical_name(&m, "gcc").as_deref(), Some("compile"));
        assert_eq!(logical_name(&m, "zsh").as_deref(), Some("shell"));
        assert_eq!(logical_name(&m, "nosuch"), None);
        assert_eq!(logical_name(&m, "pico").as_deref(), Some("code"), "the entry before the trailing comma");
        assert_eq!(tint(&m, "docker", "docker"), rgb(0x0e, 0xb7, 0xed));
        assert_eq!(tint(&m, "nosuch-cmd", "compile"), rgb(0x88, 0x88, 0x88), "compile has no color");
        assert_eq!(tint(&m, "nosuch-cmd", "git"), rgb(0xfc, 0x6d, 0x26), "the logical name's color");
        assert_eq!(tint(&m, "bash", "shell"), rgb(0xff, 0xff, 0xff), "#FFF");
        assert!(png("compile").is_some());
        assert!(png("emacs").is_some(), "iTerm2's first-party icon");
        assert!(png("nosuch").is_none());
        assert_eq!(tinted(&[10, 20, 30, 128], rgb(1, 2, 3)), vec![1, 2, 3, 128]);
        assert_eq!(candidates("vim.exe", Some("C:\\bin\\vim.exe"), Some("vim.exe notes.md")), vec!["vim".to_string()]);
        assert_eq!(candidates("python.exe", None, Some("\"C:\\Program Files\\Py\\python.exe\" x.py")), vec!["python".to_string()]);
        assert_eq!(fallback_name("vim.exe", Some("C:\\bin\\vi.exe"), None), "vi");
        let chain = [vec!["nosuch".to_string()], vec!["git".to_string()]];
        assert_eq!(resolve(&m, NONE, "C:\\i.png", &chain, Some("nosuch"), true, &all), Icon::None);
        assert_eq!(resolve(&m, CUSTOM, "C:\\i.png", &chain, Some("nosuch"), true, &all), Icon::Custom(PathBuf::from("C:\\i.png")));
        assert_eq!(resolve(&m, AUTOMATIC, "", &chain, Some("nosuch"), true, &all), Icon::Picture("git".into(), rgb(0xfc, 0x6d, 0x26)), "an ancestor's icon");
        let none = [vec!["nosuch".to_string()]];
        assert_eq!(resolve(&m, AUTOMATIC, "", &none, Some("nosuch"), true, &all), Icon::Letter("N".into(), letter_color("nosuch")));
        assert_eq!(resolve(&m, AUTOMATIC, "", &none, Some("nosuch"), false, &all), Icon::None);
        assert_eq!(resolve(&m, AUTOMATIC, "", &[], None, true, &all), Icon::None, "no job");
        // `GS:380-400`: a logical name with no picture anywhere gives no image, so the walk
        // goes on to the next command.
        assert_eq!(resolve(&m, AUTOMATIC, "", &chain, Some("nosuch"), true, &|l| l != "git"), Icon::Letter("N".into(), letter_color("nosuch")));
        assert!(stop_at("-bash"));
        assert!(stop_at("termurion-host"));
        assert!(!stop_at("bash"));
        let ancestors: Vec<String> = ["awk", "claude", "-bash", "git"].iter().map(|s| s.to_string()).collect();
        assert_eq!(ancestor_chain(&ancestors), vec![vec!["awk".to_string()], vec!["claude".to_string()]]);
        let deep: Vec<String> = (0..60).map(|i| format!("p{i}")).collect();
        assert_eq!(ancestor_chain(&deep).len(), 50);
        // Never a network path however spelled, nor a link or junction to one (`C:\x` to a
        // share, at the file or a folder above it); a link to a local folder is read.
        let links = |p: &Path| match p.to_str() {
            Some(r"C:\x") => Some(PathBuf::from(r"\\server\share")),
            Some(r"C:\ok") => Some(PathBuf::from(r"D:\icons")),
            _ => None,
        };
        for p in [r"\\server\share\i.png", r"\\?\UNC\server\share\i.png", r"\/server/share/i.png", r"/\server\share\i.png", r"\??\UNC\server\share\i.png", r"C:\x\i.png", r"C:\x"] {
            assert!(!readable_path(Path::new(p), &links), "{p}");
        }
        assert!(readable_path(Path::new(r"C:\i.png"), &links));
        assert!(readable_path(Path::new(r"C:\ok\i.png"), &links));
        // Never a device, with or without an extension, in any folder or namespace: opening
        // one can block (m13d ruling).
        for p in ["COM1", "con", "NUL", "aux.png", "PRN.json", "LPT1", "lpt9.txt", "COM1:", r"C:COM1", r"C:\x\NUL.png", r"C:\icons\CON\i.png", "CONIN$", r"\\.\COM1", r"\\.\PhysicalDrive0", r"\\?\C:\COM1.png", "C:/Users/me/AppData/Roaming/Termurion/com3.png", "nul .png"] {
            assert!(!readable_path(Path::new(p), &links), "{p}");
        }
        for p in [r"C:\icons\console.png", r"C:\icons\COM10.png", r"C:\icons\nully.png", r"C:\icons\my.con.png"] {
            assert!(readable_path(Path::new(p), &links), "{p}");
        }
        assert!(!plain_logical(r"..\x") && !plain_logical("a/b") && plain_logical("mytool"));
    }

    #[test]
    fn user_maps_follow_iterm2() {
        let m = load_maps(Some(br#"{"compile": ["mytool"]}"#), Some(br##"{"mytool": "#ff0000"}"##), None);
        assert_eq!(logical_name(&m, "mytool").as_deref(), Some("compile"));
        assert_eq!(logical_name(&m, "gcc").as_deref(), Some("compile"), "merged over the bundled map");
        assert_eq!(tint(&m, "mytool", "compile"), rgb(255, 0, 0));
        // A present but unparseable file keeps what was loaded before, else the bundled map.
        let kept = load_maps(Some(b"{not json"), None, Some(&m));
        assert_eq!(logical_name(&kept, "mytool").as_deref(), Some("compile"));
        let fresh = load_maps(Some(b"{not json"), None, None);
        assert_eq!(logical_name(&fresh, "mytool"), None);
        assert_eq!(fresh, load_maps(None, None, None));
        // Entries of the wrong shape are skipped.
        let bad = load_maps(Some(br#"{"compile": "gcc", "x": [1], "y": ["ok", 2]}"#), None, None);
        assert_eq!(bad.icons, load_maps(Some(br#"{"y": ["ok"]}"#), None, None).icons);
        // A color that is not a string reads as absent, and still shadows the bundled one.
        let odd = load_maps(None, Some(br#"{"mytool": 5, "docker": 5}"#), None);
        assert_eq!(tint(&odd, "mytool", "compile"), rgb(0x88, 0x88, 0x88));
        assert_eq!(tint(&odd, "docker", "nosuch"), rgb(0x88, 0x88, 0x88));
        assert_eq!(tint(&odd, "nosuch", "docker"), rgb(0x88, 0x88, 0x88));
        assert_eq!(tint(&load_maps(None, Some(br#"{"mytool": "red"}"#), None), "mytool", "x"), rgb(0x88, 0x88, 0x88), "not a hex color");
        assert_eq!(user_png_path(Path::new("C:\\T"), "foo"), Path::new("C:\\T").join("graphic_foo.png"));
        // The lenient parse only drops a comma before a closing bracket, outside strings.
        assert_eq!(lenient(r#"{"a": ["b", ], "c,]": 1,}"#), r#"{"a": ["b" ], "c,]": 1}"#);
    }

    #[test]
    fn icon_decode_is_bounded() {
        let png = |w: u32, h: u32| {
            let mut bytes = Vec::new();
            image::RgbaImage::new(w, h).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
            bytes
        };
        assert_eq!(decode_limited(&png(MAX_DECODE_SIDE, 1)).map(|i| i.dimensions()), Some((MAX_DECODE_SIDE, 1)));
        // A header past the limit fails before any pixels are made (a PNG bomb draws nothing).
        assert_eq!(decode_limited(&png(MAX_DECODE_SIDE + 1, 1)).map(|i| i.dimensions()), None);
        assert_eq!(decode_limited(&png(1, MAX_DECODE_SIDE + 1)).map(|i| i.dimensions()), None);
        let mut huge = png(1, 1);
        // IHDR's width and height (bytes 16..24) claiming 100000 by 100000.
        huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        assert!(decode_limited(&huge).is_none());
        assert!(decode_limited(b"not a picture").is_none());
    }

    #[test]
    fn letter_color_follows_iterm2() {
        assert_eq!(djb2(""), 5381);
        assert_eq!(djb2("a"), (5381 * 33) ^ 97);
        assert_eq!(djb2("a"), 177604);
        for s in ["", "vim", "nosuch", "bash", "\u{e9}cho", "python3.11", "a very long command name"] {
            let c = letter_color(s);
            let b = brightness(f64::from(c.r) / 255.0, f64::from(c.g) / 255.0, f64::from(c.b) / 255.0);
            assert!((0.30..=0.50).contains(&b), "{s}: {b}");
        }
        // By the rule: "vim"'s hash is in range at once; "nosuch" takes 7 rehashes.
        assert_eq!(letter_color("vim"), rgb(0xf7, 0x7b, 0x87));
        assert_eq!(letter_color("nosuch"), rgb(0x97, 0x9b, 0x00));
        assert_eq!(first_letter("\u{e9}cho").as_deref(), Some("\u{c9}"));
        assert_eq!(first_letter("e\u{301}cho").as_deref(), Some("E\u{301}"));
        assert_eq!(first_letter("vim").as_deref(), Some("V"));
        assert_eq!(first_letter(""), None);
    }
}

/// Whether bytes are a JSON object, as the user's icon and color maps must be (`parse`); the
/// check a map pulled from the settings folder passes before it is written.
pub fn is_map_file(bytes: &[u8]) -> bool {
    parse(bytes).is_some()
}
