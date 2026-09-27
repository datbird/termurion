//! The session title from the profile's Title Components, a port of iTerm2's
//! `iTermSessionTitleBuiltInFunction`.


pub const SESSION_NAME: u32 = 1;
pub const JOB: u32 = 1 << 1;
pub const PWD: u32 = 1 << 2;
pub const TTY: u32 = 1 << 3;
/// Custom: the title comes from the profile's Title Function, a script's title provider.
pub const CUSTOM: u32 = 1 << 4;
pub const PROFILE_NAME: u32 = 1 << 5;
pub const PROFILE_AND_SESSION: u32 = 1 << 6;
pub const USER: u32 = 1 << 7;
pub const HOST: u32 = 1 << 8;
pub const COMMAND_LINE: u32 = 1 << 9;
pub const SIZE: u32 = 1 << 10;
pub const TEMPORARY_NAME: u32 = 1 << 11;

/// Separates the title's parts (iTerm2 uses an em dash; Tandem's text uses none).
const SEP: &str = " - ";

/// What a title can show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TitleParts {
    /// A name a program (OSC 0 or 2) or a Set Title trigger gave the session.
    pub explicit: Option<String>,
    pub profile: String,
    /// `processTitle` of the foreground job.
    pub job: String,
    pub cmdline: String,
    pub user: String,
    pub host: String,
    pub path: String,
    pub home: String,
    pub size: (usize, usize),
    /// A tmux pane's `#{pane_title}` (iTerm2's tmuxPaneTitle).
    pub tmux_pane_title: String,
}

/// The path with the home folder shown as `~`.
fn pretty_path(path: &str, home: &str) -> String {
    let h = home.trim_end_matches(['\\', '/']);
    if !h.is_empty() && path.len() >= h.len() && path.is_char_boundary(h.len()) && path[..h.len()].eq_ignore_ascii_case(h) {
        let rest = &path[h.len()..];
        if rest.is_empty() || rest.starts_with(['\\', '/']) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// The profile's Title Function (`PS:22680-22692`, `iTermTuple fromPlistValue:`): the
/// display name and the unique id of its title provider, when the array has both. Tandem
/// writes `[display name, unique id]`; an imported iTerm2 profile has each element as
/// `{value = ...}` (`iTermTuple plistValue`).
pub fn title_function(v: Option<&toml::Value>) -> Option<(String, String)> {
    let a = v?.as_array()?;
    let item = |i: usize| match a.get(i)? {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Table(t) => t.get("value")?.as_str().map(str::to_string),
        _ => None,
    };
    Some((item(0)?, item(1)?))
}

/// The title for `components` (the profile's Title Components bits).
pub fn title(components: u32, p: &TitleParts) -> String {
    let explicit = p.explicit.as_deref().filter(|e| !e.is_empty());
    if components == 0 {
        return explicit.unwrap_or("Shell").to_string();
    }
    // Custom alone has nothing built in to show (`iTermSessionTitleBuiltInFunction.m:274-277`).
    if components == CUSTOM {
        return String::new();
    }
    // A name set by a program or trigger turns on the temporary name, unless Custom is on or
    // a name component already shows it (enableSessionNameTitleComponentIfPossible,
    // `PS:16090-16107`).
    let c = if explicit.is_some() && components & (CUSTOM | SESSION_NAME | PROFILE_AND_SESSION | TEMPORARY_NAME) == 0 { components | TEMPORARY_NAME } else { components };
    // A tmux pane's title is its session name (iTerm2's tmuxPaneTitle).
    let session = if p.tmux_pane_title.is_empty() { explicit.unwrap_or(&p.profile) } else { p.tmux_pane_title.as_str() };
    let mut out = String::new();
    if c & (SESSION_NAME | TEMPORARY_NAME) != 0 {
        out.push_str(session);
    } else if c & PROFILE_NAME != 0 {
        out.push_str(&p.profile);
    } else if c & PROFILE_AND_SESSION != 0 {
        if session == p.profile { out.push_str(session) } else { out.push_str(&format!("{}: {session}", p.profile)) }
    }
    let job = if c & COMMAND_LINE != 0 { p.cmdline.as_str() } else if c & JOB != 0 { p.job.as_str() } else { "" };
    if !job.is_empty() {
        if out.is_empty() { out.push_str(job) } else { out.push_str(&format!(" ({job})")) }
    }
    let user = (c & USER != 0).then_some(p.user.as_str()).filter(|s| !s.is_empty());
    let host = (c & HOST != 0).then_some(p.host.as_str()).filter(|s| !s.is_empty());
    let path = (c & PWD != 0 && !p.path.is_empty()).then(|| pretty_path(&p.path, &p.home));
    let mut uhp = String::new();
    if let Some(u) = user {
        uhp.push_str(u);
    }
    if let Some(h) = host {
        if user.is_some() {
            uhp.push('@');
        }
        uhp.push_str(h);
    }
    if let Some(d) = path {
        if user.is_some() || host.is_some() {
            uhp.push(':');
        }
        uhp.push_str(&d);
    }
    if !uhp.is_empty() {
        if out.is_empty() { out.push_str(&uhp) } else { out.push_str(&format!("{SEP}{uhp}")) }
    }
    if c & SIZE != 0 {
        if !out.ends_with(' ') {
            out.push(' ');
        }
        out.push_str(&format!("{SEP}{}\u{2715}{}", p.size.0, p.size.1));
    }
    if out.is_empty() {
        out.push(' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts() -> TitleParts {
        TitleParts {
            explicit: None,
            profile: "Default".into(),
            job: "vim".into(),
            cmdline: "vim notes.md".into(),
            user: "me".into(),
            host: "box".into(),
            path: r"C:\Users\me\src".into(),
            home: r"C:\Users\me".into(),
            size: (80, 24),
            tmux_pane_title: String::new(),
        }
    }

    #[test]
    fn title_components_follow_iterm2() {
        let p = parts();
        assert_eq!(title(JOB, &p), "vim");
        assert_eq!(title(SESSION_NAME | JOB, &p), "Default (vim)", "the session name defaults to the profile's name");
        assert_eq!(title(JOB, &TitleParts { explicit: Some("notes.md".into()), ..parts() }), "notes.md (vim)", "a program's title comes first");
        assert_eq!(title(PROFILE_NAME, &p), "Default");
        assert_eq!(title(PROFILE_AND_SESSION, &TitleParts { explicit: Some("x".into()), ..parts() }), "Default: x");
        assert_eq!(title(PROFILE_AND_SESSION, &p), "Default", "the same name once");
        assert_eq!(title(USER | HOST | PWD, &p), r"me@box:~\src");
        assert_eq!(title(SESSION_NAME | PWD, &p), r"Default - ~\src");
        assert_eq!(title(SESSION_NAME | SIZE, &p), "Default  - 80\u{2715}24");
        assert_eq!(title(USER, &TitleParts { user: String::new(), ..parts() }), " ", "nothing to show: one space");
        assert_eq!(title(0, &p), "Shell");
        assert_eq!(title(0, &TitleParts { explicit: Some("t".into()), ..parts() }), "t");
    }

    /// `PS:22680-22692` reads the Title Function as a two-element array; `title` gives ""
    /// for Custom alone (`iTermSessionTitleBuiltInFunction.m:274-277`).
    #[test]
    fn title_function_follows_iterm2() {
        let arr = |items: &[&str]| toml::Value::Array(items.iter().map(|s| toml::Value::String(s.to_string())).collect());
        assert_eq!(title_function(Some(&arr(&["Hello", "com.ex.t"]))), Some(("Hello".to_string(), "com.ex.t".to_string())));
        assert_eq!(title_function(Some(&arr(&["Hello"]))), None, "one element");
        assert_eq!(title_function(None), None);
        assert_eq!(title_function(Some(&toml::Value::String("Hello".into()))), None, "not an array");
        // iTerm2's own plist form (`iTermTuple plistValue`), as an imported profile has it.
        let table = |v: &str| toml::Value::Table(toml::Table::from_iter([("value".to_string(), toml::Value::String(v.into()))]));
        assert_eq!(title_function(Some(&toml::Value::Array(vec![table("Hello"), table("com.ex.t")]))), Some(("Hello".to_string(), "com.ex.t".to_string())));
        assert_eq!(title(CUSTOM, &parts()), "");
        assert_eq!(title(CUSTOM, &TitleParts { explicit: Some("t".into()), ..parts() }), "", "a program's title does not turn on the temporary name");
    }

    #[test]
    fn tmux_pane_titles_use_monitors() {
        let p = TitleParts { tmux_pane_title: "build".into(), ..parts() };
        assert_eq!(title(SESSION_NAME | JOB, &p), "build (vim)", "the pane title is the session name");
        assert_eq!(title(SESSION_NAME, &TitleParts { explicit: Some("x".into()), ..p.clone() }), "build");
        assert_eq!(title(SESSION_NAME, &parts()), "Default");
    }

    #[test]
    fn job_alone_or_in_parentheses() {
        let p = parts();
        assert_eq!(title(COMMAND_LINE, &p), "vim notes.md");
        assert_eq!(title(SESSION_NAME | COMMAND_LINE | JOB, &p), "Default (vim notes.md)", "Command Line wins over Job");
        assert_eq!(title(JOB, &TitleParts { job: String::new(), ..parts() }), " ");
    }
}
