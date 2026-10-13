//! Every link Termurion shows or opens comes from this one module. The repository is the single
//! base: help pages live in `docs/help/<slug>.md`, downloads are raw files in the repo.

macro_rules! base {
    () => {
        "https://github.com/datbird/termurion"
    };
}

/// The project home.
pub const REPO: &str = base!();
/// Where bugs are filed.
pub const ISSUES: &str = concat!(base!(), "/issues");
/// The raw-file base for the install scripts, utilities and manifests.
pub const RAW_BASE: &str = "https://raw.githubusercontent.com/datbird/termurion/main/";

macro_rules! raw_consts {
    ($($name:ident = $path:literal),* $(,)?) => {
        $(pub const $name: &str = concat!("https://raw.githubusercontent.com/datbird/termurion/main/", $path);)*
    };
}

raw_consts! {
    INSTALL_SHELL_INTEGRATION = "dist/install_shell_integration.sh",
    INSTALL_SHELL_INTEGRATION_AND_UTILITIES = "dist/install_shell_integration_and_utilities.sh",
    DOWNLOAD_SH = "dist/download.sh",
    PYENV_MANIFEST = "dist/downloads/pyenv/manifest.json",
    PYENV_BETA_MANIFEST = "dist/downloads/pyenv/betamanifest.json",
    UV_MANIFEST = "dist/downloads/uv/manifest.json",
    AI_MANIFEST = "dist/downloads/ai/manifest.json",
}

/// The page URL for a help slug, for code that builds one at run time.
pub fn help(slug: &str) -> String {
    format!("{}/blob/main/docs/help/{slug}.md", REPO)
}

/// A help page with an anchor.
pub fn help_at(slug: &str, anchor: &str) -> String {
    format!("{}#{anchor}", help(slug))
}

macro_rules! help_consts {
    ($($name:ident = $slug:literal),* $(,)?) => {
        $(pub const $name: &str = concat!(base!(), "/blob/main/docs/help/", $slug, ".md");)*
        /// Every help slug the code uses. Each needs `docs/help/<slug>.md`.
        pub const SLUGS: &[&str] = &[$($slug),*];
    };
}

help_consts! {
    HELP_SHELL_INTEGRATION = "shell-integration",
    HELP_SMART_SELECTION = "smart-selection",
    HELP_COLOR_PRESETS = "color-presets",
    HELP_PYTHON_API = "python-api",
    HELP_PYTHON_API_SECURITY = "python-api-security",
    HELP_PYTHON_API_AUTH = "python-api-auth",
    HELP_SCRIPTING_FUNDAMENTALS = "scripting-fundamentals",
    HELP_VARIABLES = "variables",
    HELP_COPY_MODE = "copy-mode",
    HELP_TRIGGERS = "triggers",
    HELP_SEARCH_SYNTAX = "search-syntax",
    HELP_PASTE_BRACKETING = "paste-bracketing",
    HELP_INLINE_IMAGES = "inline-images",
    HELP_DYNAMIC_PROFILES = "dynamic-profiles",
    HELP_WEB_BROWSER = "web-browser",
    HELP_COPROCESSES = "coprocesses",
    HELP_CLAUDE_CODE = "claude-code",
    HELP_CAPTURED_OUTPUT = "captured-output",
    HELP_BADGES = "badges",
    HELP_STATUS_BAR = "status-bar",
    HELP_TMUX_INTEGRATION = "tmux-integration",
    HELP_SSH_INTEGRATION = "ssh-integration",
    HELP_PASSWORD_MANAGER = "password-manager",
    HELP_WORKGROUPS = "workgroups",
}

/// Scripting Fundamentals, at the interpolated strings section.
pub const HELP_INTERPOLATED_STRINGS: &str = concat!(base!(), "/blob/main/docs/help/scripting-fundamentals.md#interpolated-strings");
/// The tmux page, at the tmux 2.2 note.
pub const HELP_TMUX_22: &str = concat!(base!(), "/blob/main/docs/help/tmux-integration.md#tmux-22");

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }

    fn files() -> Vec<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut all = Vec::new();
        for d in ["src", "schema", "resources"] {
            walk(&root.join(d), &mut all);
        }
        all.retain(|p| {
            let n = p.file_name().unwrap().to_string_lossy().to_uppercase();
            !(n.starts_with("LICENSE") || n.starts_with("COPYING") || n == "LINKS.RS")
        });
        all
    }

    #[test]
    fn urls_are_built_from_one_base() {
        assert_eq!(REPO, "https://github.com/datbird/termurion");
        assert_eq!(ISSUES, "https://github.com/datbird/termurion/issues");
        assert_eq!(help("copy-mode"), HELP_COPY_MODE);
        assert_eq!(help_at("scripting-fundamentals", "interpolated-strings"), HELP_INTERPOLATED_STRINGS);
        assert_eq!(DOWNLOAD_SH, format!("{RAW_BASE}dist/download.sh"));
        assert!(SLUGS.contains(&"status-bar"));
    }

    /// No link to iTerm2's site or its GitLab remains. The `api.iterm2.com` WebSocket subprotocol
    /// is not a link (iTerm2's Python library sends it), so it is exempt.
    #[test]
    fn no_iterm2_site_links_remain() {
        let mut bad = Vec::new();
        for p in files() {
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            for (i, line) in text.lines().enumerate() {
                let line = line.replace("api.iterm2.com", "");
                if line.contains("iterm2.com") || line.contains("gitlab.com/gnachman") {
                    bad.push(format!("{}:{}", p.display(), i + 1));
                }
            }
        }
        assert!(bad.is_empty(), "old links: {bad:?}");
    }

    #[test]
    fn every_help_slug_has_a_page() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for s in SLUGS {
            assert!(root.join("docs/help").join(format!("{s}.md")).is_file(), "docs/help/{s}.md is missing");
        }
        // Links written as text in resources and schema name slugs too.
        let marker = "/blob/main/docs/help/";
        for p in files() {
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            for part in text.split(marker).skip(1) {
                let slug: String = part.chars().take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-').collect();
                if slug.is_empty() {
                    continue;
                }
                assert!(SLUGS.contains(&slug.as_str()), "{}: slug {slug} is not in links::SLUGS", p.display());
            }
        }
    }
}
