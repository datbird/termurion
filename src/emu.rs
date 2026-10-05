//! Terminal emulation switches (milestone 13o): the Advanced knobs a program's sequences meet,
//! read once from the globals into one Copy value. Pure.

/// The Advanced emulation knobs, iTerm2's defaults in `Default`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    pub focus_reporting: bool,
    pub accept_osc7: bool,
    pub tab_width: usize,
    pub xterm_version: u32,
    pub color_16: bool,
    pub buffer_depth: i64,
    pub clear_history: Option<bool>,
    pub save_on_clear: bool,
    pub save_above_prompt: bool,
    pub max_url: usize,
    pub rep: bool,
    pub reset_sgr_on_prompt: bool,
    pub decrqcra_off: Option<bool>,
    pub tabs_wrap: bool,
    pub zwsp_advances: bool,
    pub vs16_all: bool,
    pub vs16_primary: bool,
    pub full_width_flags: bool,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules {
            focus_reporting: true,
            accept_osc7: true,
            tab_width: 8,
            xterm_version: 2500,
            color_16: true,
            buffer_depth: 40,
            clear_history: None,
            save_on_clear: true,
            save_above_prompt: true,
            max_url: 2_097_152,
            rep: true,
            reset_sgr_on_prompt: true,
            decrqcra_off: None,
            tabs_wrap: false,
            zwsp_advances: true,
            vs16_all: false,
            vs16_primary: true,
            full_width_flags: true,
        }
    }
}

impl Rules {
    /// The rules as the globals stand. A key that has no row yet reads iTerm2's default, so a
    /// row added later changes nothing until it is edited.
    pub fn from_globals(g: &crate::globals::Globals) -> Rules {
        let d = Rules::default();
        let flag = |key: &str, default: bool| g.get(key).and_then(|v| v.as_bool()).unwrap_or(default);
        let int = |key: &str, default: i64| g.get(key).and_then(|v| v.as_integer()).unwrap_or(default);
        Rules {
            focus_reporting: flag("FocusReportingEnabled", d.focus_reporting),
            accept_osc7: flag("AcceptOSC7", d.accept_osc7),
            // iTerm2's `MAX(1, width)`.
            tab_width: int("DefaultTabStopWidth", d.tab_width as i64).clamp(1, 4096) as usize,
            xterm_version: int("XtermVersion", d.xterm_version as i64).clamp(0, u32::MAX as i64) as u32,
            color_16: flag("OscColorReport16Bits", d.color_16),
            buffer_depth: int("BufferDepth", d.buffer_depth),
            clear_history: g.optional_bool("PreventEscapeSequenceFromClearingHistory"),
            save_on_clear: flag("SaveScrollBufferWhenClearing", d.save_on_clear),
            save_above_prompt: flag("SaveScrollbackWhenCursorMovesAbovePrompt", d.save_above_prompt),
            max_url: int("MaxURLLength", d.max_url as i64).max(0) as usize,
            rep: flag("SupportREPCode", d.rep),
            reset_sgr_on_prompt: flag("ResetSGROnPrompt", d.reset_sgr_on_prompt),
            decrqcra_off: g.optional_bool("NoSyncDisableDECRQCRA"),
            tabs_wrap: flag("TabsWrapAround", d.tabs_wrap),
            zwsp_advances: flag("ZeroWidthSpaceAdvancesCursor", d.zwsp_advances),
            vs16_all: flag("Vs16Supported", d.vs16_all),
            vs16_primary: flag("Vs16SupportedInPrimaryScreen", d.vs16_primary),
            full_width_flags: flag("FullWidthFlags", d.full_width_flags),
        }
    }
}

pub const OSC7_WARNING_ID: &str = "NoSyncOSC7DisabledBreaksShellIntegration";
pub const OSC7_HEADING: &str = "Shell Integration Setting Conflict";
pub const OSC7_TEXT: &str = "Tandem\u{2019}s shell integration now reports your username, hostname, and current directory using OSC 7, but the \u{201c}Accept OSC 7\u{201d} advanced setting is turned off. Shell integration will not work correctly until it is turned back on.";
pub const OSC7_TURN_ON: &str = "Turn On OSC 7";

/// iTerm2's once-per-process OSC 7 warning flag, held on the App.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Osc7Latch {
    warned: bool,
}

impl Osc7Latch {
    /// True the first time, false after.
    pub fn first(&mut self) -> bool {
        !std::mem::replace(&mut self.warned, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::globals::Globals;
    use toml::Value;

    fn empty() -> Globals {
        Globals::empty(std::path::Path::new("emu-test.toml"))
    }

    #[test]
    fn plumb_rules_follow_iterm2() {
        let d = Rules::default();
        assert!(d.focus_reporting && d.accept_osc7 && d.color_16);
        assert_eq!((d.tab_width, d.xterm_version, d.buffer_depth), (8, 2500, 40));
        assert_eq!((d.clear_history, d.decrqcra_off), (None, None));
        assert!(d.save_on_clear && d.save_above_prompt && d.rep && d.reset_sgr_on_prompt);
        assert_eq!(d.max_url, 2_097_152);
        assert!(!d.tabs_wrap && !d.vs16_all);
        assert!(d.zwsp_advances && d.vs16_primary && d.full_width_flags);
        assert_eq!(Rules::from_globals(&empty()), d);
        let mut g = empty();
        g.set("DefaultTabStopWidth", Value::Integer(0));
        assert_eq!(Rules::from_globals(&g).tab_width, 1, "MAX(1, width)");
        g.set("DefaultTabStopWidth", Value::Integer(-3));
        assert_eq!(Rules::from_globals(&g).tab_width, 1);
        g.set("DefaultTabStopWidth", Value::Integer(4));
        g.set("XtermVersion", Value::Integer(95));
        g.set("OscColorReport16Bits", Value::Boolean(false));
        g.set("FocusReportingEnabled", Value::Boolean(false));
        g.set("AcceptOSC7", Value::Boolean(false));
        g.set("BufferDepth", Value::Integer(7));
        let r = Rules::from_globals(&g);
        assert_eq!((r.tab_width, r.xterm_version, r.color_16, r.focus_reporting, r.accept_osc7, r.buffer_depth), (4, 95, false, false, false, 7));
        let mut g = empty();
        g.set("PreventEscapeSequenceFromClearingHistory", Value::Boolean(false));
        g.set("NoSyncDisableDECRQCRA", Value::Boolean(true));
        let r = Rules::from_globals(&g);
        assert_eq!((r.clear_history, r.decrqcra_off), (Some(false), Some(true)));
    }

    #[test]
    fn osc7_latch_is_once() {
        let mut a = Osc7Latch::default();
        assert!(a.first());
        assert!(!a.first());
        assert!(Osc7Latch::default().first());
    }
}
