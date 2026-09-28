//! The AI permission categories (`RemoteCommand.Content.PermissionCategory`,
//! `AITerm/RemoteCommand.swift:202-236`). Pure. Task 3 adds the permissions, the per-chat
//! overrides and the gate.

/// What a tool needs the person's permission for. The order is iTerm2's `allCases`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    CheckTerminalState,
    RunCommands,
    ViewContents,
    WriteToClipboard,
    ControlTerminal,
    ViewManpages,
    WriteToFilesystem,
    ActInWebBrowser,
}

impl Category {
    pub const ALL: [Category; 8] = [
        Category::CheckTerminalState,
        Category::RunCommands,
        Category::ViewContents,
        Category::WriteToClipboard,
        Category::ControlTerminal,
        Category::ViewManpages,
        Category::WriteToFilesystem,
        Category::ActInWebBrowser,
    ];

    /// The persisted name (`rawValue`), used in `setPermissions` and the per-chat overrides.
    pub fn raw(self) -> &'static str {
        match self {
            Category::CheckTerminalState => "Check Terminal State",
            Category::RunCommands => "Run Commands",
            Category::ViewContents => "View Contents",
            Category::WriteToClipboard => "Write to the Clipboard",
            Category::ControlTerminal => "Control Terminal",
            Category::ViewManpages => "View Manpages",
            Category::WriteToFilesystem => "Write to the File System",
            Category::ActInWebBrowser => "Act in Web Browser",
        }
    }

    /// A persisted name back to its category; the two renamed categories keep their old names
    /// (`RC:217-236`).
    pub fn parse(raw: &str) -> Option<Category> {
        match raw {
            "View History" => Some(Category::ViewContents),
            "Type for You" => Some(Category::ControlTerminal),
            _ => Category::ALL.into_iter().find(|c| c.raw() == raw),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_names_follow_iterm2() {
        let raws: Vec<&str> = Category::ALL.iter().map(|c| c.raw()).collect();
        assert_eq!(raws, ["Check Terminal State", "Run Commands", "View Contents", "Write to the Clipboard", "Control Terminal", "View Manpages", "Write to the File System", "Act in Web Browser"]);
        for c in Category::ALL {
            assert_eq!(Category::parse(c.raw()), Some(c));
        }
        assert_eq!(Category::parse("View History"), Some(Category::ViewContents));
        assert_eq!(Category::parse("Type for You"), Some(Category::ControlTerminal));
        assert_eq!(Category::parse("x"), None);
        assert_eq!(Category::parse("run commands"), None);
        let mut sorted = Category::ALL;
        sorted.sort();
        assert_eq!(sorted, Category::ALL);
    }
}
