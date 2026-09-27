//! The Python runtime for API scripts (iTerm2's `iTermScriptRuntime`, `iTermUvProvisioner`,
//! `iTermUvCommand`, `iTermUvPythonVersion` and `iTermSetupCfgParser`): where uv, its Pythons
//! and the shared environments live, which Python a script gets, the environment marker, the
//! `setup.cfg` text, the uv and pip command lines, the consent and error texts, and the pinned
//! uv release.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

/// The Python a new or exported script gets when nothing names one (`SR:88`).
pub const DEFAULT_PYTHON: &str = "3.12";
/// Where a version python-build-standalone cannot provide lands (`UP:22`).
pub const SAFE_FALLBACK: &str = "3.9";
/// What every environment gets besides a script's own dependencies (`UV:341`, without
/// `pyobjc`, which has no Windows build).
pub const PACKAGES: [&str; 2] = ["iterm2", "certifi"];
/// A full environment's virtual environment folder (`SR:81`).
pub const VENV_DIR: &str = ".venv";
/// The marker beside `.venv` that says how it was built (`SR:82`).
pub const MARKER: &str = "python-runtime.json";
/// A full environment's metadata file (`CFG:17-53`).
pub const SETUP_CFG: &str = "setup.cfg";

/// Tandem's uv folder: `%APPDATA%\Tandem\uv` (`UV:132-141`).
pub fn uv_dir(appdata: &Path) -> PathBuf {
    appdata.join("Tandem").join("uv")
}

/// Tandem's own uv (`UV:143-146`).
pub fn uv_exe(uv_dir: &Path) -> PathBuf {
    uv_dir.join("bin").join("uv.exe")
}

/// Where uv keeps the Pythons it downloads (`UV:152-154`).
pub fn python_install_dir(uv_dir: &Path) -> PathBuf {
    uv_dir.join("python")
}

/// uv's package cache (`UV:156-158`).
pub fn cache_dir(uv_dir: &Path) -> PathBuf {
    uv_dir.join("cache")
}

/// The shared environments, one per Python minor (`UV:812-905`).
pub fn venvs_dir(uv_dir: &Path) -> PathBuf {
    uv_dir.join("venvs")
}

/// The shared environment for one Python minor, used by basic scripts and the REPL.
pub fn shared_venv(uv_dir: &Path, minor: &str) -> PathBuf {
    venvs_dir(uv_dir).join(minor)
}

/// A Windows virtual environment's interpreter.
pub fn venv_python(venv: &Path) -> PathBuf {
    venv.join("Scripts").join("python.exe")
}

/// The certifi roots inside a Windows virtual environment (`SSL_CERT_FILE`, `L:855-920`).
pub fn certifi_path(venv: &Path) -> PathBuf {
    venv.join("Lib").join("site-packages").join("certifi").join("cacert.pem")
}

/// The major and minor parts of a version: "3.12.4" is "3.12" (`UP:55-61`).
pub fn two_part(v: &str) -> String {
    let parts: Vec<&str> = v.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 2 { format!("{}.{}", parts[0], parts[1]) } else { v.to_string() }
}

/// Whether a Python version is plain digits and dots ("3", "3.12", "3.10.2"): the only form
/// Tandem passes to uv or resolves, so a request such as "pypy" or
/// "cpython@3.12+freethreaded" never reaches uv's `--python`.
pub fn numeric_version(v: &str) -> bool {
    v.split('.').all(numeric)
}

/// Compares dotted versions part by part as numbers, a missing part reading as 0.
pub fn dotted(a: &str, b: &str) -> Ordering {
    let nums = |v: &str| v.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (a, b) = (nums(a), nums(b));
    for i in 0..a.len().max(b.len()) {
        match a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0)) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    Ordering::Equal
}

/// The Python minor a script runs under and, when it is not the one asked for, the version
/// asked for (`UP:63-89`): the pin when available; else 3.9 when that is not older than the
/// pin; else the nearest minor above the pin; else the newest. With nothing known available
/// the pin is kept. None when the request is not a `numeric_version`.
pub fn resolve(requested: &str, available: &[String]) -> Option<(String, Option<String>)> {
    if !numeric_version(requested) {
        return None;
    }
    let minor = two_part(requested);
    if available.is_empty() || available.contains(&minor) {
        return Some((minor, None));
    }
    let remapped = Some(requested.to_string());
    if available.iter().any(|a| a == SAFE_FALLBACK) && dotted(SAFE_FALLBACK, &minor) != Ordering::Less {
        return Some((SAFE_FALLBACK.to_string(), remapped));
    }
    let mut forward: Vec<&String> = available.iter().filter(|a| dotted(a, &minor) != Ordering::Less).collect();
    forward.sort_by(|a, b| dotted(a, b));
    if let Some(nearest) = forward.first() {
        return Some((nearest.to_string(), remapped));
    }
    let newest = available.iter().max_by(|a, b| dotted(a, b)).cloned().unwrap_or(minor);
    Some((newest, remapped))
}

fn numeric(part: &str) -> bool {
    !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())
}

/// The version a script's first line names: `#!/usr/bin/env python` followed by an all-numeric
/// version of at least two parts (`L:677-723`).
pub fn shebang_version(first_line: &str) -> Option<String> {
    let line = first_line.split('\n').next().unwrap_or("");
    let candidate = line.strip_prefix("#!/usr/bin/env python")?.trim();
    let parts: Vec<&str> = candidate.split('.').collect();
    (parts.len() >= 2 && parts.iter().all(|p| numeric(p))).then(|| candidate.to_string())
}

/// The stable CPython default-variant minors in `uv python list --output-format json`,
/// sorted and unique; anything unreadable is none (`UP:38-52`).
pub fn uv_minors(json: &str) -> Vec<String> {
    let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let mut minors: Vec<String> = Vec::new();
    for entry in &entries {
        let Some(version) = entry.get("version").and_then(Value::as_str) else {
            return Vec::new();
        };
        let field = |key: &str, default: &'static str| match entry.get(key) {
            None | Some(Value::Null) => Some(default.to_string()),
            Some(Value::String(v)) => Some(v.clone()),
            Some(_) => None,
        };
        let (Some(implementation), Some(variant)) = (field("implementation", "cpython"), field("variant", "default")) else {
            return Vec::new();
        };
        let stable = !version.is_empty() && version.chars().all(|c| c.is_ascii_digit() || c == '.');
        if implementation == "cpython" && variant == "default" && stable {
            let minor = two_part(version);
            if !minors.contains(&minor) {
                minors.push(minor);
            }
        }
    }
    minors.sort_by(|a, b| dotted(a, b));
    minors
}

/// The installed Pythons in `py -0p` output, in the launcher's order: only `-V:<major>.<minor>`
/// and `-<major>.<minor>[-32|-64]` tags, each with its interpreter path.
pub fn py_list(output: &str) -> Vec<(String, PathBuf)> {
    let two_numbers = |v: &str| {
        let parts: Vec<&str> = v.split('.').collect();
        (parts.len() == 2 && parts.iter().all(|p| numeric(p))).then(|| v.to_string())
    };
    let mut out = Vec::new();
    for line in output.lines() {
        let Some((tag, rest)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        let version = match tag.strip_prefix("-V:") {
            Some(v) => two_numbers(v),
            None => tag.strip_prefix('-').and_then(|t| {
                let t = t.strip_suffix("-64").or_else(|| t.strip_suffix("-32")).unwrap_or(t);
                two_numbers(t)
            }),
        };
        let rest = rest.trim();
        let path = rest.strip_prefix('*').map(str::trim).unwrap_or(rest);
        if let (Some(v), false) = (version, path.is_empty()) {
            out.push((v, PathBuf::from(path)));
        }
    }
    out
}

/// The version in `uv --version` output: its second word (`UV:316-319`).
pub fn uv_version(output: &str) -> Option<String> {
    output.split_whitespace().nth(1).map(str::to_string)
}

/// The `python-runtime.json` marker beside a full environment's `.venv` (`SR:23-46`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marker {
    pub schema: u32,
    pub backend: String,
    pub uv_version: String,
    pub python: String,
    pub remapped_from: Option<String>,
}

impl Marker {
    /// Swift's sorted, pretty JSON: keys in order, `"key" : value`, two-space indent, and
    /// `remapped_from` absent when unset.
    pub fn json(&self) -> String {
        let q = |v: &str| Value::String(v.to_string()).to_string();
        let mut lines = vec![format!("  \"backend\" : {}", q(&self.backend)), format!("  \"python\" : {}", q(&self.python))];
        if let Some(from) = &self.remapped_from {
            lines.push(format!("  \"remapped_from\" : {}", q(from)));
        }
        lines.push(format!("  \"schema\" : {}", self.schema));
        lines.push(format!("  \"uv_version\" : {}", q(&self.uv_version)));
        format!("{{\n{}\n}}", lines.join(",\n"))
    }

    /// Reads a marker; every key but `remapped_from` is required (`SR:43-45`).
    pub fn parse(text: &str) -> Option<Marker> {
        let v: Value = serde_json::from_str(text).ok()?;
        let text_of = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
        let remapped_from = match v.get("remapped_from") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return None,
        };
        Some(Marker {
            schema: u32::try_from(v.get("schema")?.as_u64()?).ok()?,
            backend: text_of("backend")?,
            uv_version: text_of("uv_version")?,
            python: text_of("python")?,
            remapped_from,
        })
    }
}

/// What Tandem reads from a full environment's `setup.cfg` (`CFG:55-130`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetupCfg {
    pub name: String,
    pub dependencies: Vec<String>,
    pub python_version: Option<String>,
    pub environment_version: i64,
}

/// Where a value ends in inih: at one of `chars`, or at a `;` that follows whitespace.
fn ini_end(s: &str, chars: &str) -> usize {
    let mut was_space = false;
    for (i, c) in s.char_indices() {
        if chars.contains(c) || (was_space && c == ';') {
            return i;
        }
        was_space = c.is_whitespace();
    }
    s.len()
}

/// Reads an INI text as iTerm2's inih does (`ThirdParty/inih/ini.c`): `;` and `#` start
/// comments, `;` after whitespace ends a value, an indented line continues the previous
/// name, a bad line fails the whole parse. Calls `handle(section, name, value)`.
fn ini_parse(text: &str, mut handle: impl FnMut(&str, &str, &str)) -> bool {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut section = String::new();
    let mut prev_name = String::new();
    let mut ok = true;
    for raw in text.split('\n') {
        let start = raw.trim();
        if start.is_empty() || start.starts_with(';') || start.starts_with('#') {
            continue;
        }
        if !prev_name.is_empty() && raw.starts_with(char::is_whitespace) {
            handle(&section, &prev_name, start);
        } else if let Some(rest) = start.strip_prefix('[') {
            let end = ini_end(rest, "]");
            if rest[end..].starts_with(']') {
                section = rest[..end].to_string();
                prev_name.clear();
            } else {
                ok = false;
            }
        } else {
            let end = ini_end(start, "=:");
            if end < start.len() && (start[end..].starts_with('=') || start[end..].starts_with(':')) {
                let name = start[..end].trim_end();
                let value = &start[end + 1..];
                let value = value[..ini_end(value, "")].trim();
                prev_name = name.to_string();
                handle(&section, name, value);
            } else {
                ok = false;
            }
        }
    }
    ok
}

impl SetupCfg {
    /// `[metadata] name`, `[options] install_requires` (every line, joined with ";"),
    /// `[options] python_requires` (only "=<numeric version>"), `[iterm2] environment`
    /// (">=<n>"). Dependencies keep their order and drop repeats.
    pub fn parse(text: &str) -> Option<SetupCfg> {
        let mut cfg = SetupCfg::default();
        let mut requires: Vec<String> = Vec::new();
        let ok = ini_parse(text, |section, name, value| match (section, name) {
            ("metadata", "name") => cfg.name = value.to_string(),
            ("options", "python_requires") => {
                if let Some(v) = value.strip_prefix('=') {
                    if v.split('.').all(numeric) {
                        cfg.python_version = Some(v.to_string());
                    }
                }
            }
            ("options", "install_requires") => {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    requires.push(trimmed.to_string());
                }
            }
            ("iterm2", "environment") => {
                if let Some(v) = value.strip_prefix(">=") {
                    cfg.environment_version = leading_int(v);
                }
            }
            _ => {}
        });
        if !ok {
            return None;
        }
        for dep in requires.join(";").split(';').map(str::trim).filter(|d| !d.is_empty()) {
            if !cfg.dependencies.iter().any(|d| d == dep) {
                cfg.dependencies.push(dep.to_string());
            }
        }
        Some(cfg)
    }
}

/// `NSString integerValue`: optional spaces and sign, then the leading digits; else 0.
fn leading_int(v: &str) -> i64 {
    let v = v.trim_start();
    let (sign, digits) = match v.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, v.strip_prefix('+').unwrap_or(v)),
    };
    let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
    digits[..end].parse::<i64>().map(|n| sign * n).unwrap_or(0)
}

/// The `setup.cfg` Tandem writes for a full environment (`CFG:17-53`): the version cut to two
/// parts, `iterm2` put first when asked and missing, dependencies in order without repeats.
pub fn setup_cfg_text(name: &str, deps: &[String], ensure_iterm2: bool, python_version: &str, environment_version: i64) -> String {
    let parts: Vec<&str> = python_version.split('.').collect();
    let version = if parts.len() > 2 { parts[..2].join(".") } else { python_version.to_string() };
    let mut all: Vec<&str> = Vec::new();
    if ensure_iterm2 && !deps.iter().any(|d| python_package(d).as_deref() == Some("iterm2")) {
        all.push("iterm2");
    }
    for d in deps {
        if !all.contains(&d.as_str()) {
            all.push(d);
        }
    }
    format!(
        "# WARNING: This file was generated automatically by Tandem. Do not edit it manually.\n[metadata]\nname={name}\nversion=1.0\n\n[options]\nscripts={name}/{name}.py\ninstall_requires={}\npython_requires = ={version}\n\n[iterm2]\nenvironment = >={environment_version}\n",
        all.join("; ")
    )
}

/// The package name of a dependency such as "aiohttp>=3.1": a PEP 508 name, optionally
/// followed by one comparison and a version (`CFG:143-185`).
pub fn python_package(dep: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^((?:[A-Za-z0-9][A-Za-z0-9._-]*[A-Za-z0-9])|[A-Za-z0-9])(?: *(?:<|>|==|<=|>=|!=) *(?:[0-9a-z.]+))? *$").unwrap()
    });
    re.captures(dep).map(|c| c[1].to_string())
}

/// The dependencies with `dep` replacing the one for the same package, or appended
/// (`CFG:189-205`).
pub fn set_dependency(deps: &[String], dep: &str) -> Vec<String> {
    let package = python_package(dep);
    let mut out = deps.to_vec();
    match deps.iter().position(|d| python_package(d) == package) {
        Some(i) => out[i] = dep.to_string(),
        None => out.push(dep.to_string()),
    }
    out
}

/// Whether a dependency is a plain PyPI requirement Tandem passes to uv or pip: a package
/// name with at most one version comparison (`python_package`), with no run of spaces.
/// Anything else (an option such as "--index-url=...", a direct reference such as
/// "pkg @ https://...") could download from a host other than pypi.org, which the consent
/// names, so it is refused.
pub fn plain_dependency(dep: &str) -> bool {
    python_package(dep).is_some() && !dep.contains("  ")
}

/// The dependencies `plain_dependency` refuses, in order, for the log.
pub fn refused_dependencies(deps: &[String]) -> Vec<String> {
    deps.iter().filter(|d| !plain_dependency(d)).cloned().collect()
}

/// The log line for a refused dependency.
pub fn refused_dependency_line(dep: &str) -> String {
    format!("Skipped the dependency \u{201c}{dep}\u{201d}: it is not a plain PyPI package name.\n")
}

/// What to install for a script: its plain dependencies (`plain_dependency`), then each of
/// `PACKAGES` it does not name.
pub fn packages_for(deps: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in deps.iter().filter(|d| plain_dependency(d)) {
        if !out.contains(d) {
            out.push(d.clone());
        }
    }
    for p in PACKAGES {
        if !deps.iter().any(|d| python_package(d).as_deref() == Some(p)) {
            out.push(p.to_string());
        }
    }
    out
}

/// What builds environments: Tandem's own uv, or an installed Python (its command line, such
/// as `py -3`, and its minor).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    Uv(PathBuf),
    Python { argv: Vec<String>, minor: String },
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The variables every uv run gets (`UC:30-41`, without `UV_LINK_MODE`); `downloads` says
/// whether uv may download a Python ("automatic") or not ("never").
pub fn uv_env(uv_dir: &Path, downloads: bool) -> Vec<(String, String)> {
    [
        ("UV_PYTHON_INSTALL_DIR", text(&python_install_dir(uv_dir))),
        ("UV_CACHE_DIR", text(&cache_dir(uv_dir))),
        ("UV_PYTHON_PREFERENCE", "only-managed".to_string()),
        ("UV_NO_CONFIG", "1".to_string()),
        ("UV_PYTHON_DOWNLOADS", if downloads { "automatic" } else { "never" }.to_string()),
        ("UV_SYSTEM_CERTS", "1".to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// The environment a uv, pip or venv run gets: Tandem's own with every inherited `UV_*` and
/// `PIP_*` variable and `PYTHONPATH` removed (names compared ignoring case, as Windows does),
/// then `extra` added. iTerm2 replaces the whole environment (`iTermCommandRunner.m:211-212`);
/// Windows programs need `SystemRoot`, `TEMP` and the like, so only the variables that could
/// point uv or pip at another index or Python are dropped.
pub fn child_env(inherited: impl IntoIterator<Item = (String, String)>, extra: &[(String, String)]) -> Vec<(String, String)> {
    let dropped = |k: &str| {
        let k = k.to_ascii_uppercase();
        k.starts_with("UV_") || k.starts_with("PIP_") || k == "PYTHONPATH"
    };
    let mut out: Vec<(String, String)> = inherited.into_iter().filter(|(k, _)| !dropped(k)).collect();
    for (k, v) in extra {
        out.retain(|(o, _)| !o.eq_ignore_ascii_case(k));
        out.push((k.clone(), v.clone()));
    }
    out
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// `uv venv --relocatable --python <minor> <venv>` (`UC:48-50`); None when the minor is not
/// a `numeric_version`.
pub fn uv_venv_args(minor: &str, venv: &Path) -> Option<Vec<String>> {
    if !numeric_version(minor) {
        return None;
    }
    let mut args = strings(&["venv", "--relocatable", "--python", minor]);
    args.push(text(venv));
    Some(args)
}

/// `uv pip install --python <python> --only-binary :all: [--upgrade] <packages>` (`UC:55-61`).
pub fn uv_pip_install_args(venv_python: &Path, packages: &[String], upgrade: bool) -> Vec<String> {
    let mut args = strings(&["pip", "install", "--python"]);
    args.push(text(venv_python));
    args.extend(strings(&["--only-binary", ":all:"]));
    if upgrade {
        args.push("--upgrade".to_string());
    }
    args.extend(packages.iter().filter(|p| plain_dependency(p)).cloned());
    args
}

/// `uv python list --all-versions --only-downloads --output-format json` (`UC:73-75`).
pub fn uv_python_list_args() -> Vec<String> {
    strings(&["python", "list", "--all-versions", "--only-downloads", "--output-format", "json"])
}

/// A pip command run through uv against one environment, installs wheels only (`UC:81-88`).
pub fn uv_pip_passthrough(pip_args: &[String], venv_python: &Path) -> Vec<String> {
    let mut args = vec!["pip".to_string()];
    args.extend(pip_args.iter().cloned());
    if pip_args.first().map(String::as_str) == Some("install") {
        args.extend(strings(&["--only-binary", ":all:"]));
    }
    args.push("--python".to_string());
    args.push(text(venv_python));
    args
}

/// An installed Python's arguments to make an environment: `-m venv <venv>`.
pub fn venv_args(venv: &Path) -> Vec<String> {
    let mut args = strings(&["-m", "venv"]);
    args.push(text(venv));
    args
}

/// An environment's Python's arguments to install packages, wheels only (as uv's).
pub fn pip_install_args(packages: &[String], upgrade: bool) -> Vec<String> {
    let mut args = strings(&["-m", "pip", "install", "--only-binary", ":all:"]);
    if upgrade {
        args.push("--upgrade".to_string());
    }
    args.extend(packages.iter().filter(|p| plain_dependency(p)).cloned());
    args
}

/// A pip command run by an environment's Python, installs wheels only (as uv's).
pub fn pip_passthrough(pip_args: &[String]) -> Vec<String> {
    let mut args = strings(&["-m", "pip"]);
    args.extend(pip_args.iter().cloned());
    if pip_args.first().map(String::as_str) == Some("install") {
        args.extend(strings(&["--only-binary", ":all:"]));
    }
    args
}

/// The processor uv is downloaded for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X64,
    Arm64,
}

/// The uv release Tandem downloads (github.com/astral-sh/uv, published 2026-09-25).
pub const UV_VERSION: &str = "0.12.19";
/// SHA-256 of `uv-x86_64-pc-windows-msvc.zip` in that release.
pub const UV_SHA256_X64: &str = "6dbb02d79e419522f1c500f0adb1cddcff0cda7d59b0d66ea7f5e3b4a1b2f5f0";
/// SHA-256 of `uv-aarch64-pc-windows-msvc.zip` in that release.
pub const UV_SHA256_ARM64: &str = "115b54cb823bc48260670f5782001add6067ac8d98d18c8263a833704e287de9";
/// The x64 zip's size (17,955,780 bytes) in whole megabytes as iTerm2 rounds it (`UV:1434`).
pub const UV_MEGABYTES: u32 = 17;

/// The pinned uv zip for a processor.
pub fn uv_zip_url(arch: Arch) -> String {
    let target = match arch {
        Arch::X64 => "x86_64",
        Arch::Arm64 => "aarch64",
    };
    format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/uv-{target}-pc-windows-msvc.zip")
}

/// The pinned zip's SHA-256, checked before it is unpacked.
pub fn uv_sha256(arch: Arch) -> &'static str {
    match arch {
        Arch::X64 => UV_SHA256_X64,
        Arch::Arm64 => UV_SHA256_ARM64,
    }
}

/// The Scripts menu's runtime item (`SR:62-79`, uv on).
pub fn menu_title(uv_installed: bool) -> &'static str {
    if uv_installed { "Check for Updated Runtime" } else { "Install Python Runtime" }
}

/// Asks before uv, the first Python and the packages of the environments it builds are
/// downloaded (`UV:1436-1438`), naming every source: the pinned uv release on github.com, Python
/// from python-build-standalone and each package from pypi.org (`packages_for`'s list).
pub fn uv_consent(megabytes: u32, packages: &[String]) -> (&'static str, String) {
    (
        "Download Python Support?",
        format!(
            "To run Python scripts, Tandem needs to download uv {UV_VERSION} (about {megabytes} MB) from github.com/astral-sh/uv, a Python interpreter from github.com/astral-sh/python-build-standalone, and these Python packages from pypi.org: {}. OK to download them now?",
            packages.join(", ")
        ),
    )
}

/// Asks before a script's packages (and perhaps its Python) are downloaded.
pub fn packages_consent(packages: &[String], interpreter: Option<&str>) -> (&'static str, String) {
    let python = interpreter
        .map(|v| format!(" Python {v} may also be downloaded from github.com/astral-sh/python-build-standalone."))
        .unwrap_or_default();
    (
        "Download Python Packages?",
        format!("To run this script, Tandem needs to download these Python packages from pypi.org: {}.{python} OK to download them now?", packages.join(", ")),
    )
}

/// Asks before Check for Updated Runtime downloads anything.
pub fn update_consent() -> (&'static str, String) {
    (
        "Check for Updated Runtime?",
        "Tandem will update the iterm2 and certifi packages in its shared Python environments from pypi.org, and uv from github.com/astral-sh/uv when this Tandem ships a newer uv. OK to continue?".to_string(),
    )
}

pub const ENV_UNAVAILABLE_HEADING: &str = "Python Environment Unavailable";
pub fn env_unavailable(e: &str) -> String {
    format!("Could not prepare the Python environment for this script: {e}")
}
pub fn repl_prepare_failed(e: &str) -> String {
    format!("Could not prepare the Python environment for the REPL: {e}")
}
pub const INSTALL_FAILED_HEADING: &str = "Installation Failed";
pub fn install_failed(e: &str) -> String {
    format!("Could not install the Python runtime: {e}")
}
pub const RUNTIME_HEADING: &str = "Python Runtime";
pub const UPDATE_FAILED_HEADING: &str = "Update Failed";
pub const NOT_INSTALLED: &str = "uv is not installed.";
pub fn up_to_date(v: &str) -> String {
    format!("uv {v} is up to date.")
}
pub fn upgraded(from: &str, to: &str) -> String {
    format!("Upgraded uv {from} to {to}. Updated the Python modules in shared environments.")
}
pub fn update_check_failed(reason: &str) -> String {
    format!("Could not check for a uv update: {reason}")
}
pub const UPDATE_DOWNLOAD_FAILED: &str = "Could not download the uv update.";
pub const SETTING_UP: &str = "Setting up the Python environment\u{2026}";
/// The Script Console line when a script's environment is missing and nothing may download
/// it (AutoLaunch and other paths no one chose, `M:916`).
pub fn env_missing_line(name: &str) -> String {
    format!("The Python environment for \u{201c}{name}\u{201d} is missing (the shared runtime may have been deleted). Launch it from the Scripts menu to rebuild it.\n")
}
pub const CHECKSUM_MISMATCH: &str = "The downloaded uv did not match its checksum.";


#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn versions_follow_iterm2() {
        assert_eq!(two_part("3.12.4"), "3.12");
        assert_eq!(resolve("3.12", &s(&["3.10", "3.12"])), Some(("3.12".to_string(), None)));
        assert_eq!(resolve("3.7", &s(&["3.9", "3.12"])), Some(("3.9".to_string(), Some("3.7".to_string()))));
        assert_eq!(resolve("3.8", &s(&["3.10", "3.12"])), Some(("3.10".to_string(), Some("3.8".to_string()))));
        assert_eq!(resolve("3.14", &s(&["3.10", "3.12"])), Some(("3.12".to_string(), Some("3.14".to_string()))));
        assert_eq!(resolve("3.11", &[]), Some(("3.11".to_string(), None)));
        assert_eq!(resolve("3", &[]), Some(("3".to_string(), None)));
        // Only digits and dots reach uv.
        for bad in ["pypy", "cpython@3.12+freethreaded", "3.12t", "", "3..1", "--python"] {
            assert_eq!(resolve(bad, &s(&["3.12"])), None, "{bad}");
            assert!(!numeric_version(bad), "{bad}");
        }
        assert_eq!(shebang_version("#!/usr/bin/env python3.10\n"), Some("3.10".to_string()));
        assert_eq!(shebang_version("#!/usr/bin/env python3"), None);
        assert_eq!(shebang_version("#!/usr/bin/python3.10"), None);
        assert_eq!(shebang_version("#!/usr/bin/env python3.x"), None);
        let json = r#"[{"version":"3.12.5","implementation":"cpython","variant":"default"},{"version":"3.12.4"},{"version":"3.13.0a2","implementation":"cpython"},{"version":"3.10.14","implementation":"pypy"},{"version":"3.11.9","variant":"freethreaded"},{"version":"3.9.20"}]"#;
        assert_eq!(uv_minors(json), s(&["3.9", "3.12"]));
        assert!(uv_minors("not json").is_empty());
        let py = " -V:3.12 *        C:\\Py312\\python.exe\n -V:3.10          C:\\Py310\\python.exe\n -V:Astral/CPython3.12 C:\\a\\python.exe\n -3.11-64 *   C:\\Py311\\python.exe\n";
        assert_eq!(
            py_list(py),
            vec![
                ("3.12".to_string(), PathBuf::from(r"C:\Py312\python.exe")),
                ("3.10".to_string(), PathBuf::from(r"C:\Py310\python.exe")),
                ("3.11".to_string(), PathBuf::from(r"C:\Py311\python.exe")),
            ]
        );
        assert_eq!(uv_version("uv 0.9.4 (abc123 2026-09-01)"), Some("0.9.4".to_string()));
    }

    #[test]
    fn layout_follows_windows() {
        let uv = uv_dir(Path::new(r"C:\A"));
        assert_eq!(uv, PathBuf::from(r"C:\A\Tandem\uv"));
        assert_eq!(uv_exe(&uv), PathBuf::from(r"C:\A\Tandem\uv\bin\uv.exe"));
        assert_eq!(python_install_dir(&uv), PathBuf::from(r"C:\A\Tandem\uv\python"));
        assert_eq!(cache_dir(&uv), PathBuf::from(r"C:\A\Tandem\uv\cache"));
        assert_eq!(venvs_dir(&uv), PathBuf::from(r"C:\A\Tandem\uv\venvs"));
        assert_eq!(shared_venv(&uv, "3.12"), PathBuf::from(r"C:\A\Tandem\uv\venvs\3.12"));
        assert_eq!(venv_python(Path::new(r"C:\v")), PathBuf::from(r"C:\v\Scripts\python.exe"));
        assert_eq!(certifi_path(Path::new(r"C:\v")), PathBuf::from(r"C:\v\Lib\site-packages\certifi\cacert.pem"));
        assert_eq!((DEFAULT_PYTHON, SAFE_FALLBACK, VENV_DIR, MARKER, SETUP_CFG), ("3.12", "3.9", ".venv", "python-runtime.json", "setup.cfg"));
        assert_eq!(PACKAGES, ["iterm2", "certifi"]);
        let m = Marker { schema: 1, backend: "uv".into(), uv_version: "0.9.4".into(), python: "3.12".into(), remapped_from: None };
        assert_eq!(m.json(), "{\n  \"backend\" : \"uv\",\n  \"python\" : \"3.12\",\n  \"schema\" : 1,\n  \"uv_version\" : \"0.9.4\"\n}");
        let r = Marker { remapped_from: Some("3.7".into()), ..m.clone() };
        assert_eq!(
            r.json(),
            "{\n  \"backend\" : \"uv\",\n  \"python\" : \"3.12\",\n  \"remapped_from\" : \"3.7\",\n  \"schema\" : 1,\n  \"uv_version\" : \"0.9.4\"\n}"
        );
        assert_eq!(Marker::parse(&m.json()), Some(m));
        assert_eq!(Marker::parse(&r.json()), Some(r));
        assert_eq!(Marker::parse("{}"), None);
        assert_eq!(Marker::parse("not json"), None);
    }

    #[test]
    fn setup_cfg_follows_iterm2() {
        let text = setup_cfg_text("tool", &s(&["requests", "iterm2"]), true, "3.12.1", 0);
        assert_eq!(
            text,
            "# WARNING: This file was generated automatically by Tandem. Do not edit it manually.\n[metadata]\nname=tool\nversion=1.0\n\n[options]\nscripts=tool/tool.py\ninstall_requires=requests; iterm2\npython_requires = =3.12\n\n[iterm2]\nenvironment = >=0\n"
        );
        let twice = setup_cfg_text("tool", &s(&["requests", "requests"]), true, "3.12", 0);
        assert!(twice.contains("\ninstall_requires=iterm2; requests\n"), "{twice}");
        let cfg = SetupCfg::parse(&text).unwrap();
        assert_eq!(
            cfg,
            SetupCfg { name: "tool".into(), dependencies: s(&["requests", "iterm2"]), python_version: Some("3.12".into()), environment_version: 0 }
        );
        let lax = SetupCfg::parse("[options]\npython_requires = >=3.8\n").unwrap();
        assert_eq!(lax.python_version, None);
        let two = SetupCfg::parse("[options]\ninstall_requires=a; b\ninstall_requires=c\n").unwrap();
        assert_eq!(two.dependencies, s(&["a", "b", "c"]));
        let env = SetupCfg::parse("[iterm2]\nenvironment = >=3\n").unwrap();
        assert_eq!(env.environment_version, 3);
        assert_eq!(SetupCfg::parse("[metadata\nname=x\n"), None);
        assert_eq!(SetupCfg::parse("just words\n"), None);
        assert_eq!(python_package("aiohttp>=3.14.3"), Some("aiohttp".to_string()));
        assert_eq!(python_package("a b"), None);
        assert_eq!(python_package("x == 1.0"), Some("x".to_string()));
        assert_eq!(set_dependency(&s(&["aiohttp", "x"]), "aiohttp>=3.1"), s(&["aiohttp>=3.1", "x"]));
        assert_eq!(set_dependency(&s(&["x"]), "aiohttp>=3.1"), s(&["x", "aiohttp>=3.1"]));
        assert_eq!(packages_for(&s(&["requests", "certifi"])), s(&["requests", "certifi", "iterm2"]));
        // A dependency that could download from another host is refused everywhere.
        let hostile = s(&["--index-url=https://h/simple", "a @ https://h/a.whl", "requests", "b   >= 1", "-e", "c://d"]);
        assert_eq!(refused_dependencies(&hostile), s(&["--index-url=https://h/simple", "a @ https://h/a.whl", "b   >= 1", "-e", "c://d"]));
        assert_eq!(packages_for(&hostile), s(&["requests", "iterm2", "certifi"]));
        assert!(plain_dependency("requests >= 2.0") && plain_dependency("x==1"));
        let py = Path::new(r"C:\v\Scripts\python.exe");
        let pys = py.to_string_lossy().to_string();
        assert_eq!(uv_pip_install_args(py, &hostile, false), s(&["pip", "install", "--python", &pys, "--only-binary", ":all:", "requests"]));
        assert_eq!(pip_install_args(&hostile, false), s(&["-m", "pip", "install", "--only-binary", ":all:", "requests"]));
        assert_eq!(
            refused_dependency_line("a @ https://h/a.whl"),
            "Skipped the dependency \u{201c}a @ https://h/a.whl\u{201d}: it is not a plain PyPI package name.\n"
        );
    }

    #[test]
    fn commands_follow_iterm2() {
        let expect = |downloads: &str| {
            vec![
                ("UV_PYTHON_INSTALL_DIR".to_string(), r"C:\u\python".to_string()),
                ("UV_CACHE_DIR".to_string(), r"C:\u\cache".to_string()),
                ("UV_PYTHON_PREFERENCE".to_string(), "only-managed".to_string()),
                ("UV_NO_CONFIG".to_string(), "1".to_string()),
                ("UV_PYTHON_DOWNLOADS".to_string(), downloads.to_string()),
                ("UV_SYSTEM_CERTS".to_string(), "1".to_string()),
            ]
        };
        assert_eq!(uv_env(Path::new(r"C:\u"), true), expect("automatic"));
        assert_eq!(uv_env(Path::new(r"C:\u"), false), expect("never"));
        let v = Path::new(r"C:\v");
        let p = Path::new(r"C:\v\Scripts\python.exe");
        let vs = v.to_string_lossy().to_string();
        let ps = p.to_string_lossy().to_string();
        assert_eq!(uv_venv_args("3.12", v), Some(s(&["venv", "--relocatable", "--python", "3.12", &vs])));
        assert_eq!(uv_venv_args("pypy", v), None);
        let packages: Vec<String> = vec!["iterm2".into(), "certifi".into()];
        assert_eq!(uv_pip_install_args(p, &packages, false), s(&["pip", "install", "--python", &ps, "--only-binary", ":all:", "iterm2", "certifi"]));
        assert_eq!(
            uv_pip_install_args(p, &packages, true),
            s(&["pip", "install", "--python", &ps, "--only-binary", ":all:", "--upgrade", "iterm2", "certifi"])
        );
        assert_eq!(uv_python_list_args(), s(&["python", "list", "--all-versions", "--only-downloads", "--output-format", "json"]));
        assert_eq!(uv_pip_passthrough(&s(&["install", "x"]), p), s(&["pip", "install", "x", "--only-binary", ":all:", "--python", &ps]));
        assert_eq!(uv_pip_passthrough(&s(&["show", "x"]), p), s(&["pip", "show", "x", "--python", &ps]));
        assert_eq!(venv_args(v), s(&["-m", "venv", &vs]));
        assert_eq!(pip_install_args(&s(&["iterm2"]), false), s(&["-m", "pip", "install", "--only-binary", ":all:", "iterm2"]));
        assert_eq!(pip_install_args(&s(&["iterm2"]), true), s(&["-m", "pip", "install", "--only-binary", ":all:", "--upgrade", "iterm2"]));
        assert_eq!(pip_passthrough(&s(&["install", "x"])), s(&["-m", "pip", "install", "x", "--only-binary", ":all:"]));
        assert_eq!(pip_passthrough(&s(&["show", "x"])), s(&["-m", "pip", "show", "x"]));
        // The child environment drops what could point uv or pip elsewhere, then adds uv's.
        let inherited = vec![
            ("SystemRoot".to_string(), r"C:\Windows".to_string()),
            ("uv_index_url".to_string(), "https://h".to_string()),
            ("PIP_INDEX_URL".to_string(), "https://h".to_string()),
            ("PythonPath".to_string(), r"C:\x".to_string()),
            ("UV_PYTHON_DOWNLOADS".to_string(), "automatic".to_string()),
            ("PATH".to_string(), r"C:\bin".to_string()),
        ];
        assert_eq!(
            child_env(inherited, &[("UV_PYTHON_DOWNLOADS".to_string(), "never".to_string())]),
            vec![
                ("SystemRoot".to_string(), r"C:\Windows".to_string()),
                ("PATH".to_string(), r"C:\bin".to_string()),
                ("UV_PYTHON_DOWNLOADS".to_string(), "never".to_string()),
            ]
        );
        let b = Backend::Python { argv: s(&[r"C:\Windows\py.exe", "-3"]), minor: "3.12".into() };
        assert_ne!(b, Backend::Uv(PathBuf::from(r"C:\u\bin\uv.exe")));
    }

    #[test]
    fn consent_names_the_source() {
        let packages = s(&["iterm2", "certifi", "requests"]);
        assert_eq!(
            uv_consent(12, &packages),
            (
                "Download Python Support?",
                format!("To run Python scripts, Tandem needs to download uv {UV_VERSION} (about 12 MB) from github.com/astral-sh/uv, a Python interpreter from github.com/astral-sh/python-build-standalone, and these Python packages from pypi.org: iterm2, certifi, requests. OK to download them now?")
            )
        );
        // Install Python Runtime's list: the packages every environment gets.
        let text = uv_consent(12, &packages_for(&[])).1;
        assert!(text.contains("pypi.org: iterm2, certifi."), "{text}");
        assert_eq!(
            packages_consent(&packages, Some("3.12")),
            (
                "Download Python Packages?",
                "To run this script, Tandem needs to download these Python packages from pypi.org: iterm2, certifi, requests. Python 3.12 may also be downloaded from github.com/astral-sh/python-build-standalone. OK to download them now?".to_string()
            )
        );
        assert_eq!(
            packages_consent(&packages, None).1,
            "To run this script, Tandem needs to download these Python packages from pypi.org: iterm2, certifi, requests. OK to download them now?"
        );
        let (heading, text) = update_consent();
        assert_eq!(heading, "Check for Updated Runtime?");
        assert!(text.contains("pypi.org") && text.contains("github.com/astral-sh/uv"), "{text}");
        assert_eq!(
            env_missing_line("Full"),
            "The Python environment for \u{201c}Full\u{201d} is missing (the shared runtime may have been deleted). Launch it from the Scripts menu to rebuild it.\n"
        );
        for t in [uv_consent(12, &packages).1, packages_consent(&packages, Some("3.12")).1, text] {
            assert!(!t.contains("automatically"), "{t}");
        }
    }

    #[test]
    fn uv_source_is_pinned() {
        assert_eq!(uv_zip_url(Arch::X64), format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/uv-x86_64-pc-windows-msvc.zip"));
        assert_eq!(uv_zip_url(Arch::Arm64), format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/uv-aarch64-pc-windows-msvc.zip"));
        for sum in [UV_SHA256_X64, UV_SHA256_ARM64, uv_sha256(Arch::X64), uv_sha256(Arch::Arm64)] {
            assert_eq!(sum.len(), 64);
            assert!(sum.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)), "{sum}");
        }
        assert_eq!((uv_sha256(Arch::X64), uv_sha256(Arch::Arm64)), (UV_SHA256_X64, UV_SHA256_ARM64));
        // A digest pasted into both constants would pass the checks above.
        assert_ne!(UV_SHA256_X64, UV_SHA256_ARM64);
        let parts: Vec<&str> = UV_VERSION.split('.').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())));
        assert!(UV_MEGABYTES > 0);
        assert_eq!(menu_title(false), "Install Python Runtime");
        assert_eq!(menu_title(true), "Check for Updated Runtime");
    }
}
