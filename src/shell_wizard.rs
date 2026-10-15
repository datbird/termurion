//! The Install Shell Integration installer (iTerm2's `iTermShellIntegrationWindowController` and
//! its page view controllers): a small window that walks through the install in the session it
//! was opened for. Two ways for a Unix shell (bash, zsh, fish, xonsh): "Download and Run Installer"
//! types `curl -L ... | bash`; "Internet-Free Install" types a staged set of commands (discover the
//! shell, write the script, optionally the utilities, update the dotfile), each stage waiting for the
//! terminal to answer before the next. A PowerShell session installs Termurion's own script.
//!
//! This module is the pages and the stages, pure: the commands it types, the text of each state,
//! and the expectations (iTerm2's `iTermExpect`) it feeds with the lines the terminal printed. The
//! app (`panels_app`) types the text and reads the screen.

use regex::Regex;

use crate::panel_ui::{Act, View};

// Texts, from `iTermShellIntegrationWindowController.xib` unless noted. "iTerm2" is Termurion.
pub const HEADER: &str = "Install Shell Integration";
pub const UTILITIES_BOX: &str = "Also install Termurion Utilities";
pub const FIRST_DESCRIPTION: &str = "Shell Integration consists of scripts that help your shell communicate with Termurion so it can track the commands you\u{2019}re running, your current directory, and other state. ";
pub const UTILITIES_DESCRIPTION: &str = "Utilities include scripts to download files, display images inline, and more. ";
pub const LEARN_MORE: &str = "Learn more.";
pub const SECOND_TEXT: &str = "How would you like to install Shell Integration?\n\nIn this process, your shell\u{2019}s startup scripts will be modified. This installer will enter commands in the terminal window for you.";
pub const DOWNLOAD_BUTTON: &str = "Download and Run Installer";
pub const DOWNLOAD_CAPTION: &str = "Downloads the latest version. Runs curl | bash";
pub const OFFLINE_BUTTON: &str = "Internet-Free Install";
pub const OFFLINE_CAPTION: &str = "Guided manual installation. No download required.";
pub const CONFIRM_PREFIX: &str = "This will run the following command:";
pub const FINISHED_TITLE: &str = "Installation complete!";
pub const FINISHED_TEXT: &str = "Create a new tab to try it out.\n\nFor best results, install shell integration on all hosts you ssh in to.";
pub const FINISHED_EMOJI: &str = "\u{1f389}";
pub const CANCEL: &str = "Cancel";
pub const CONTINUE: &str = "Continue";
pub const GO_BACK: &str = "Go Back";
pub const SKIP: &str = "Skip";
pub const OK: &str = "OK";
pub const PREVIEW: &str = "Preview Command";
// Strings the view controllers set (`iTermShellIntegrationDownloadAndRunViewController.m`,
// `iTermShellIntegrationPasteShellCommandsViewController.m`).
pub const WAITING_FOR_COMMAND: &str = "Waiting for this command to finish:";
pub const PRESS_CONTINUE_TO_RUN: &str = "Press \u{201c}Continue\u{201d} to run this command:";
pub const SHELL_UNSUPPORTED: &str = "\u{1f61e} Your shell is not supported, or perhaps your $SHELL environment variable is not set correctly. Press \u{201c}Continue\u{201d} to try again.";
pub const WAITING_TEXT: &str = "\u{23f3} Waiting for command to complete\u{2026}";
pub const SEND_AGAIN: &str = "Send Again";
pub const STEP1_DISCOVER: &str = "1. Discover.";
pub const CONTINUE_TO_DISCOVER: &str = "\u{27a1} Select \u{201c}Continue\u{201d} to discover.";
pub const SHELL_NOT_SUPPORTED: &str = "\u{1f6d1} Your shell is not supported.\n\nOnly bash, fish, xonsh, and zsh work with shell integration";
pub const STEP2_WRITE: &str = "Step 2. Write the shell integration script.";
pub const CONTINUE_TO_WRITE: &str = "\u{27a1} Select \u{201c}Continue\u{201d} to write the shell integration script.";
pub const WROTE: &str = "\u{2705} Wrote the shell integration script.";
pub const STEP3_INSTALL: &str = "Step 3. Install Termurion utility scripts.";
pub const CONTINUE_TO_INSTALL: &str = "\u{27a1} Select \u{201c}Continue\u{201d} to install Termurion utility scripts.";
pub const INSTALLED: &str = "\u{2705} Installed Termurion utility scripts.";
pub const XONSH_DONE: &str = "\u{2705} Xonsh auto-loads scripts from rc.d (no dotfile update needed).";
pub const CONTINUE_TO_UPDATE: &str = "\u{27a1} Select \u{201c}Continue\u{201d} to update your shell\u{2019}s dotfile.";
pub const UPDATED: &str = "\u{2705} Updated your shell\u{2019}s dotfile.";
pub const DONE_PROCEED: &str = "Done! Select \u{201c}Continue\u{201d} to proceed.";

/// "Discovered your shell: you use “%@”." (`ShellIntegrationInstaller.Discovered`)
pub fn discovered(shell: &str) -> String {
    format!("\u{2705} Discovered your shell: you use \u{201c}{shell}\u{201d}.")
}

/// "Step %d. Xonsh auto-loads scripts from rc.d (no dotfile update needed)."
pub fn xonsh_step(n: i32) -> String {
    format!("Step {n}. Xonsh auto-loads scripts from rc.d (no dotfile update needed).")
}

/// "Step %d. Update your shell’s dotfile."
pub fn update_step(n: i32) -> String {
    format!("Step {n}. Update your shell\u{2019}s dotfile.")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Fish,
    Xonsh,
    Zsh,
    Unknown,
}

impl Shell {
    /// `iTermShellIntegrationShellString`. tcsh is not in the list: Termurion ships no tcsh script.
    pub fn name(self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Fish => "fish",
            Shell::Xonsh => "xonsh",
            Shell::Zsh => "zsh",
            Shell::Unknown => "an unsupported shell",
        }
    }

    pub fn from_name(name: &str) -> Shell {
        match name {
            "bash" => Shell::Bash,
            "fish" => Shell::Fish,
            "xonsh" => Shell::Xonsh,
            "zsh" => Shell::Zsh,
            _ => Shell::Unknown,
        }
    }
}

/// How the session's shell is installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Termurion's PowerShell script, loaded from `$PROFILE`.
    Pwsh,
    /// A Unix shell: both ways of the installer.
    Unix,
}

/// The curl command (`DownloadAndRun.command`): a blank line, the command, with Termurion's scripts.
pub fn curl_command(utilities: bool) -> String {
    let url = if utilities { crate::links::INSTALL_SHELL_INTEGRATION_AND_UTILITIES } else { crate::links::INSTALL_SHELL_INTEGRATION };
    format!("\ncurl -L {url} | bash")
}

/// `iTermShellIntegrationDownloadAndRunViewController.textField`: the prefix and the command.
pub fn confirm_text(busy: bool, utilities: bool) -> String {
    let prefix = if busy { WAITING_FOR_COMMAND } else { PRESS_CONTINUE_TO_RUN };
    format!("{prefix}\n{}", curl_command(utilities))
}

/// The second page's xib text on the Download and Run confirmation (`xib:224`).
pub fn confirm_page_text(utilities: bool) -> String {
    format!("{CONFIRM_PREFIX}\n{}", curl_command(utilities))
}

/// One send in a staged install: the text typed, and what the terminal must print after it.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub text: String,
    pub expect: String,
}

fn step(text: impl Into<String>, expect: &str) -> Step {
    Step { text: text.into(), expect: expect.to_string() }
}

// iTerm2's prompts are "^>> " and "^> "; a screen line is read with its trailing blanks trimmed.
const PS1: &str = "^>>( |$)";
const PS2: &str = "^>( |$)";

/// `launchBashString`.
pub fn launch_bash(shell: Shell) -> String {
    if shell == Shell::Xonsh {
        // Xonsh: bash -c sets the prompts, then runs an interactive bash.
        return "bash -c 'INPUTRC=/dev/null PS1=\">> \" PS2=\"> \" bash --noprofile --norc'\n".to_string();
    }
    "bash --noprofile --norc\nINPUTRC='/dev/null' bash --noprofile --norc\n PS1='>> '; PS2='> '\n".to_string()
}

/// `exitBashString`.
pub fn exit_bash(shell: Shell) -> &'static str {
    if shell == Shell::Xonsh { "exit\n" } else { "exit\nexit\n" }
}

/// `shellIntegrationPath`.
pub fn script_path(shell: Shell, dotdir: &str) -> String {
    if shell == Shell::Xonsh {
        return "~/.config/xonsh/rc.d/iterm2.xsh".to_string();
    }
    format!("{dotdir}/.iterm2_shell_integration.{}", shell.name())
}

/// The utility names (`utilities-manifest.txt`).
pub fn utility_names() -> Vec<&'static str> {
    crate::shell_utilities::FILES.iter().map(|(n, _)| *n).collect()
}

/// `scriptForCurrentShell`: the shell's script, and with utilities the aliases (or the PATH line
/// for xonsh) that put them in reach.
pub fn script_text(shell: Shell, dotdir: &str, utilities: bool) -> Option<String> {
    let name = match shell {
        Shell::Bash => "iterm2_shell_integration.bash",
        Shell::Fish => "iterm2_shell_integration.fish",
        Shell::Xonsh => "iterm2_shell_integration.xonsh",
        Shell::Zsh => "iterm2_shell_integration.zsh",
        Shell::Unknown => return None,
    };
    let file = crate::ssh_inject::FILES.iter().find(|f| f.name == name)?;
    let mut text = String::from_utf8_lossy(file.bytes).into_owned();
    if !utilities {
        return Some(text);
    }
    text.push('\n');
    if shell == Shell::Xonsh {
        text.push_str("$PATH.insert(0, $HOME + '/.iterm2')");
        return Some(text);
    }
    let aliases: Vec<String> = utility_names().into_iter().map(|c| format!("alias {c}={dotdir}/.iterm2/{c}")).collect();
    text.push_str(&aliases.join(";"));
    Some(text)
}

/// `dotfileCommandWithBashDotfile:`.
pub fn dotfile_command(shell: Shell, dotdir: &str) -> String {
    let (and, or) = if shell == Shell::Fish { ("; and", "; or") } else { ("&&", "||") };
    let rel = format!("{dotdir}/.iterm2_shell_integration.{}", shell.name());
    format!("test -e {rel} {and} source {rel} {or} true\n")
}

/// `discoverShell:` (the first command; zsh asks a second one for its dotfile folder).
pub fn discover_steps() -> Vec<Step> {
    vec![step("echo My shell is $SHELL\n", "^My shell is (.+)")]
}

pub const ZSH_DOTDIR_COMMAND: &str = "echo My dotfiles go in ${ZDOTDIR:-$HOME}\n";
pub const ZSH_DOTDIR_EXPECT: &str = "^My dotfiles go in (.*)";

fn cat_string(shell: Shell, text: &str, to: &str, steps: &mut Vec<Step>) {
    steps.push(step(launch_bash(shell), "."));
    steps.push(step(format!("cat <<'EOF' > {to}\n"), "."));
    steps.push(step(text, PS2));
    steps.push(step("\nEOF\n", "."));
    steps.push(step(exit_bash(shell), "> EOF"));
}

/// `catToScript:` — write the shell integration script.
pub fn script_steps(shell: Shell, dotdir: &str, utilities: bool) -> Option<Vec<Step>> {
    let text = script_text(shell, dotdir, utilities)?;
    let mut steps = vec![step(launch_bash(shell), ".")];
    if shell == Shell::Xonsh {
        steps.push(step("mkdir -p ~/.config/xonsh/rc.d\n", PS1));
    }
    let path = script_path(shell, dotdir);
    cat_string(shell, &text, &path, &mut steps);
    steps.push(step(format!("chmod +x {path}\n"), PS1));
    steps.push(step(exit_bash(shell), PS1));
    Some(steps)
}

/// `catToUtilities:` — unpack the utilities into `~/.iterm2`.
pub fn utilities_steps(shell: Shell, dotdir: &str) -> Vec<Step> {
    let folder = format!("{dotdir}/.iterm2");
    vec![
        step(launch_bash(shell), "."),
        step(format!("mkdir {folder}; echo ok\n"), "."),
        step(format!("cd {folder}; echo ok\n"), "^ok$"),
        step("base64 -d <<'EOF'| tar xfz -\n", "^ok$"),
        step(crate::shell_utilities::base64_archive(), PS2),
        step("\nEOF\n", "."),
        step(exit_bash(shell), "> EOF"),
    ]
}

/// `amendDotFileWithExpectation:` — add the source line to the shell's startup file. The first
/// text (the bash launch and, per shell, a mkdir or the dotfile choice) is typed at once.
pub fn dotfile_steps(shell: Shell, dotdir: &str) -> Option<Vec<Step>> {
    let mut first: Vec<String> = launch_bash(shell).trim().split('\n').map(|l| format!("{l}\n")).collect();
    let script = match shell {
        Shell::Zsh => format!("{dotdir}/.zshrc"),
        Shell::Bash => {
            first.push("IT2_INSTALLER_DOTFILE=$(test -f ~/.bash_profile && echo -n ~/.bash_profile || echo -n ~/.profile)\n".to_string());
            "\"$IT2_INSTALLER_DOTFILE\"".to_string()
        }
        Shell::Fish => {
            first.push("mkdir -p ~/.config/fish\n".to_string());
            "~/.config/fish/config.fish".to_string()
        }
        // Xonsh loads from rc.d: nothing to amend. Unknown cannot be amended.
        Shell::Xonsh | Shell::Unknown => return None,
    };
    Some(vec![
        step(first.concat(), PS1),
        step(format!("if ! grep iterm2_shell_integration {script}  > /dev/null 2>&1; then\n"), PS1),
        step(format!("    cat <<-EOF >> {script}\n\n"), PS2),
        step(dotfile_command(shell, dotdir), PS2),
        step("EOF\n", PS2),
        step("fi\n", PS1),
        step(exit_bash(shell), PS1),
    ])
}

// Note on `dotfile_steps`: iTerm2 sends the first text with no expectation of its own and the
// "if ! grep" text right after, waiting for ">> " after that one. The first step above carries the
// same wait, one send earlier, so the "if ! grep" line is typed once a prompt has appeared.

/// Which stage a finished run moves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunKind {
    Discover,
    Script,
    Utilities,
    Dotfile { next: i32 },
    Curl,
}

/// A staged send in progress: the steps, which one waits, and what has been captured.
struct Run {
    kind: RunKind,
    steps: Vec<Step>,
    at: usize,
    captures: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    First,
    Second,
    Confirm,
    Paste,
    PwshConfirm,
    Finished,
}

pub struct Wizard {
    pub page: Page,
    plan: Plan,
    pub utilities: bool,
    shell: Shell,
    dotdir: String,
    /// `-1` before the first Continue of the Internet-Free page, then 0 to 4 (or 3).
    pub stage: i32,
    busy: bool,
    run: Option<Run>,
    /// The curl page's error after an unsupported shell.
    unsupported: bool,
    preview: bool,
    pwsh_path: String,
}

impl Wizard {
    pub fn new(plan: Plan, utilities: bool, pwsh_path: &str) -> Wizard {
        Wizard { page: Page::First, plan, utilities, shell: Shell::Unknown, dotdir: "~".into(), stage: -1, busy: false, run: None, unsupported: false, preview: false, pwsh_path: pwsh_path.to_string() }
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    pub fn shell(&self) -> Shell {
        self.shell
    }

    pub fn dotdir(&self) -> &str {
        &self.dotdir
    }

    // ----- pages -----

    /// The first page's Continue.
    pub fn first_continue(&mut self) -> Vec<Act> {
        self.page = if self.plan == Plan::Pwsh { Page::PwshConfirm } else { Page::Second };
        Vec::new()
    }

    pub fn go_back(&mut self) -> Vec<Act> {
        self.cancel_run();
        self.page = match self.page {
            Page::Second | Page::PwshConfirm => Page::First,
            Page::Confirm | Page::Paste => Page::Second,
            other => other,
        };
        self.stage = -1;
        self.shell = Shell::Unknown;
        self.unsupported = false;
        Vec::new()
    }

    pub fn choose_download(&mut self) {
        self.page = Page::Confirm;
        self.unsupported = false;
    }

    pub fn choose_offline(&mut self) {
        self.stage = 0;
        self.page = Page::Paste;
    }

    /// The Download and Run page's Continue: type the command and wait for "Done." or the
    /// unsupported-shell line.
    pub fn download_continue(&mut self) -> Vec<Act> {
        self.unsupported = false;
        self.busy = true;
        let command = format!("{}\n", curl_command(self.utilities));
        self.run = Some(Run { kind: RunKind::Curl, steps: vec![step(command.clone(), r"(^Done\.$)|(^Your shell, .*, is not supported yet)")], at: 0, captures: Vec::new() });
        vec![Act::SendText(command)]
    }

    /// Cancel and the close box: whatever waits is forgotten.
    pub fn cancel_run(&mut self) {
        self.run = None;
        self.busy = false;
    }

    // ----- the staged install -----

    /// The Internet-Free page's Continue at the current stage (`shellIntegrationInstallerSendShellCommands:`).
    pub fn paste_continue(&mut self) -> Vec<Act> {
        if self.busy {
            return Vec::new();
        }
        match self.stage {
            0 => self.start(RunKind::Discover, discover_steps()),
            1 => match script_steps(self.shell, &self.dotdir, self.utilities) {
                Some(steps) => self.start(RunKind::Script, steps),
                None => Vec::new(),
            },
            2 if self.utilities => self.start(RunKind::Utilities, utilities_steps(self.shell, &self.dotdir)),
            2 => self.startup_scripts(3),
            3 if self.utilities => self.startup_scripts(4),
            _ => self.finish(),
        }
    }

    /// `modifyStartupScriptsAndProceedTo:` — xonsh needs none.
    fn startup_scripts(&mut self, next: i32) -> Vec<Act> {
        match dotfile_steps(self.shell, &self.dotdir) {
            Some(steps) => self.start(RunKind::Dotfile { next }, steps),
            None => self.finish(),
        }
    }

    fn finish(&mut self) -> Vec<Act> {
        self.cancel_run();
        self.page = Page::Finished;
        self.stage = 0;
        Vec::new()
    }

    fn start(&mut self, kind: RunKind, steps: Vec<Step>) -> Vec<Act> {
        let first = steps[0].text.clone();
        self.busy = true;
        self.run = Some(Run { kind, steps, at: 0, captures: Vec::new() });
        vec![Act::SendText(first)]
    }

    /// `shellIntegrationInstallerSkipStage`.
    pub fn skip(&mut self) {
        let last = if self.utilities { 4 } else { 3 };
        if self.stage < last {
            self.stage += 1;
        }
    }

    /// The page's Back at a stage (`back:`).
    pub fn paste_back(&mut self) -> Vec<Act> {
        self.cancel_run();
        if self.stage == 0 {
            return self.go_back();
        }
        self.stage -= 1;
        Vec::new()
    }

    /// The command Preview Command shows for the current stage (`NextCommandForSendShellCommands`).
    pub fn next_command(&self) -> Option<String> {
        let join = |steps: Vec<Step>| steps.into_iter().map(|s| s.text).collect::<String>();
        match self.stage {
            0 => Some(join(discover_steps())),
            1 => script_steps(self.shell, &self.dotdir, self.utilities).map(join).or_else(|| Some("# Error: your shell could not be determined or is unsupported.".to_string())),
            2 if self.utilities => Some(join(utilities_steps(self.shell, &self.dotdir))),
            2 => Some(dotfile_steps(self.shell, &self.dotdir).map(join).unwrap_or_default()),
            3 if self.utilities => Some(dotfile_steps(self.shell, &self.dotdir).map(join).unwrap_or_default()),
            _ => None,
        }
    }

    /// Lines the terminal printed since the last send: advances the run when the expectation holds.
    pub fn output(&mut self, lines: &[String]) -> Vec<Act> {
        let Some(run) = self.run.as_mut() else { return Vec::new() };
        let Ok(re) = Regex::new(&run.steps[run.at].expect) else { return Vec::new() };
        let Some(caps) = lines.iter().find_map(|l| re.captures(l)) else { return Vec::new() };
        run.captures = caps.iter().map(|c| c.map_or(String::new(), |m| m.as_str().to_string())).collect();
        // A run whose step matched moves to the next step, or finishes.
        if run.kind == RunKind::Discover && run.at == 0 {
            let shell = run.captures.get(1).map(|c| c.rsplit('/').next().unwrap_or(c).to_string()).unwrap_or_default();
            if shell == "zsh" {
                run.steps.push(step(ZSH_DOTDIR_COMMAND, ZSH_DOTDIR_EXPECT));
            }
            self.shell = Shell::from_name(&shell);
            self.dotdir = match shell.as_str() {
                "fish" => "$HOME".to_string(),
                _ => "~".to_string(),
            };
        }
        if run.kind == RunKind::Discover && run.at == 1 {
            self.dotdir = run.captures.get(1).cloned().unwrap_or_else(|| "~".to_string());
        }
        run.at += 1;
        if run.at < run.steps.len() {
            return vec![Act::SendText(run.steps[run.at].text.clone())];
        }
        let (kind, first) = (run.kind, run.captures.first().cloned().unwrap_or_default());
        self.run = None;
        self.busy = false;
        match kind {
            RunKind::Discover => self.stage = 1,
            RunKind::Script => self.stage = 2,
            RunKind::Utilities => self.stage = 3,
            RunKind::Dotfile { next } => {
                self.stage = next;
                // The last stage of the run just finished: the next Continue proceeds to the end.
            }
            RunKind::Curl => {
                if first.starts_with("Your shell") {
                    self.unsupported = true;
                } else {
                    self.page = Page::Finished;
                }
            }
        }
        Vec::new()
    }

    /// The text of the stages with the bold one (`PasteShellCommandsViewController.update`).
    pub fn stage_lines(&self) -> (Vec<String>, Option<usize>) {
        let (stage, busy, shell, utilities) = (self.stage, self.busy, self.shell, self.utilities);
        let mut lines: Vec<String> = Vec::new();
        let mut bold: Option<usize> = None;
        if stage < 0 {
            lines.push(STEP1_DISCOVER.to_string());
        } else if stage == 0 {
            lines.push(if busy { WAITING_TEXT.to_string() } else { CONTINUE_TO_DISCOVER.to_string() });
            bold = Some(lines.len() - 1);
        } else if shell == Shell::Unknown {
            lines.push(SHELL_NOT_SUPPORTED.to_string());
        } else {
            lines.push(discovered(shell.name()));
        }
        let unavailable = stage == 1 && shell == Shell::Unknown;
        if unavailable {
            return (lines, bold);
        }
        if stage < 1 {
            lines.push(STEP2_WRITE.to_string());
        } else if stage == 1 {
            lines.push(if busy { WAITING_TEXT.to_string() } else { CONTINUE_TO_WRITE.to_string() });
            bold = Some(lines.len() - 1);
        } else {
            lines.push(WROTE.to_string());
        }
        let mut i = 2;
        if utilities {
            i += 1;
            if stage < 2 {
                lines.push(STEP3_INSTALL.to_string());
            } else if stage == 2 && !busy {
                lines.push(CONTINUE_TO_INSTALL.to_string());
                bold = Some(lines.len() - 1);
            } else if stage == 2 && busy {
                lines.push(WAITING_TEXT.to_string());
                bold = Some(lines.len() - 1);
            } else {
                lines.push(INSTALLED.to_string());
            }
        }
        if shell == Shell::Xonsh {
            lines.push(if stage < i { xonsh_step(i + 1) } else { XONSH_DONE.to_string() });
        } else if stage < i {
            lines.push(update_step(i + 1));
        } else if stage == i && !busy {
            lines.push(CONTINUE_TO_UPDATE.to_string());
            bold = Some(lines.len() - 1);
        } else if stage == i && busy {
            lines.push(WAITING_TEXT.to_string());
            bold = Some(lines.len() - 1);
        } else {
            lines.push(UPDATED.to_string());
        }
        let done = stage > i || (stage >= i && shell == Shell::Xonsh);
        if done {
            lines.push(String::new());
            bold = Some(lines.len());
            lines.push(DONE_PROCEED.to_string());
        }
        (lines, bold)
    }

    /// Whether Continue works on the Internet-Free page.
    pub fn continue_enabled(&self) -> bool {
        !((self.stage == 1 && self.shell == Shell::Unknown) || self.busy)
    }

    /// Whether Skip works.
    pub fn skip_enabled(&self) -> bool {
        let unavailable = self.stage == 1 && self.shell == Shell::Unknown;
        let i = if self.utilities { 3 } else { 2 };
        let done = self.stage > i || (self.stage >= i && self.shell == Shell::Xonsh);
        !unavailable && !done && !self.busy
    }
}

/// The window.
pub struct WizardView {
    pub wiz: Wizard,
}

impl WizardView {
    pub fn new(plan: Plan, utilities: bool, pwsh_path: &str) -> WizardView {
        WizardView { wiz: Wizard::new(plan, utilities, pwsh_path) }
    }
}

/// The text of the PowerShell page: where the script goes and what runs.
pub fn pwsh_text(path: &str) -> String {
    format!("Termurion will save its PowerShell script to\n{path}\nand add a line that loads it to your PowerShell profile ($PROFILE).\n\nIf PowerShell's execution policy blocks scripts (Restricted or AllSigned), Termurion also sets it to RemoteSigned for your user only.")
}

impl View for WizardView {
    fn title(&self) -> String {
        HEADER.to_string()
    }

    fn size(&self) -> (f64, f64) {
        (608.0, 330.0)
    }

    fn animating(&self) -> bool {
        self.wiz.busy()
    }

    fn expecting(&self) -> bool {
        self.wiz.run.is_some()
    }

    fn output(&mut self, lines: &[String]) -> Vec<Act> {
        self.wiz.output(lines)
    }

    /// The close box is Cancel.
    fn on_close(&mut self) -> Vec<Act> {
        self.wiz.cancel_run();
        vec![Act::Close]
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> Vec<Act> {
        let mut acts = Vec::new();
        let w = &mut self.wiz;
        match w.page {
            Page::First => {
                ui.vertical_centered(|ui| ui.heading(HEADER));
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(FIRST_DESCRIPTION);
                    crate::panel_ui::link(ui, LEARN_MORE, crate::links::HELP_SHELL_INTEGRATION, &mut acts);
                });
                if w.plan == Plan::Unix {
                    ui.add_space(14.0);
                    if ui.checkbox(&mut w.utilities, UTILITIES_BOX).changed() {
                        acts.push(Act::SetInstallUtilities(w.utilities));
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.add_space(24.0);
                        ui.label(egui::RichText::new(UTILITIES_DESCRIPTION).small());
                        crate::panel_ui::link(ui, LEARN_MORE, crate::links::HELP_SHELL_INTEGRATION, &mut acts);
                    });
                }
                buttons(ui, &mut acts, &[(CANCEL, Side::Left), (CONTINUE, Side::Right)], &mut |name, acts| match name {
                    CANCEL => acts.push(Act::Close),
                    _ => acts.extend(w.first_continue()),
                });
            }
            Page::Second => {
                ui.label(SECOND_TEXT);
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(DOWNLOAD_BUTTON).min_size(egui::vec2(200.0, 26.0))).clicked() {
                        w.choose_download();
                    }
                    ui.label(DOWNLOAD_CAPTION);
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(OFFLINE_BUTTON).min_size(egui::vec2(200.0, 26.0))).clicked() {
                        w.choose_offline();
                    }
                    ui.label(OFFLINE_CAPTION);
                });
                buttons(ui, &mut acts, &[(CANCEL, Side::Left), (GO_BACK, Side::Right)], &mut |name, acts| match name {
                    CANCEL => acts.push(Act::Close),
                    _ => acts.extend(w.go_back()),
                });
            }
            Page::Confirm => {
                ui.label(if w.unsupported { SHELL_UNSUPPORTED.to_string() } else { confirm_text(w.busy, w.utilities) });
                if w.busy {
                    ui.add_space(8.0);
                    ui.add(egui::Spinner::new());
                }
                let enabled = !w.busy;
                buttons_enabled(ui, &mut acts, &[(CANCEL, Side::Left, true), (GO_BACK, Side::Right, true), (CONTINUE, Side::Right, enabled)], &mut |name, acts| match name {
                    CANCEL => acts.push(Act::CancelWork),
                    GO_BACK => acts.extend(w.go_back()),
                    _ => acts.extend(w.download_continue()),
                });
            }
            Page::Paste => {
                let (lines, bold) = w.stage_lines();
                for (i, l) in lines.iter().enumerate() {
                    let t = egui::RichText::new(l);
                    ui.label(if bold == Some(i) { t.strong() } else { t });
                }
                ui.add_space(6.0);
                let unavailable = w.stage == 1 && w.shell == Shell::Unknown;
                let preview = w.next_command();
                if let Some(text) = preview.as_ref().filter(|_| !unavailable && (0..=3).contains(&w.stage)) {
                    let label = if w.busy { SEND_AGAIN } else { PREVIEW };
                    let r = ui.add(egui::Button::new(egui::RichText::new(label).small()));
                    if r.clicked() {
                        if w.busy {
                            w.cancel_run();
                            acts.extend(w.paste_continue());
                        } else {
                            w.preview = !w.preview;
                        }
                    }
                    if w.preview && !w.busy {
                        egui::ScrollArea::vertical().max_height(110.0).show(ui, |ui| {
                            ui.add(egui::Label::new(egui::RichText::new(text).monospace()).selectable(true));
                        });
                    }
                }
                let (can_continue, can_skip) = (w.continue_enabled(), w.skip_enabled());
                buttons_enabled(ui, &mut acts, &[(CANCEL, Side::Left, true), (GO_BACK, Side::Right, true), (SKIP, Side::Right, can_skip), (CONTINUE, Side::Right, can_continue)], &mut |name, acts| match name {
                    CANCEL => acts.push(Act::CancelWork),
                    GO_BACK => acts.extend(w.paste_back()),
                    SKIP => w.skip(),
                    _ => acts.extend(w.paste_continue()),
                });
            }
            Page::PwshConfirm => {
                ui.label(pwsh_text(&w.pwsh_path));
                buttons(ui, &mut acts, &[(CANCEL, Side::Left), (GO_BACK, Side::Right), (CONTINUE, Side::Right)], &mut |name, acts| match name {
                    CANCEL => acts.push(Act::Close),
                    GO_BACK => acts.extend(w.go_back()),
                    _ => {
                        w.page = Page::Finished;
                        acts.push(Act::InstallPwsh);
                    }
                });
            }
            Page::Finished => {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(FINISHED_EMOJI).size(40.0));
                    ui.label(egui::RichText::new(FINISHED_TITLE).size(26.0));
                });
                ui.add_space(10.0);
                ui.label(FINISHED_TEXT);
                buttons(ui, &mut acts, &[(OK, Side::Right)], &mut |_, acts| acts.push(Act::Close));
            }
        }
        acts
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

fn buttons(ui: &mut egui::Ui, acts: &mut Vec<Act>, items: &[(&str, Side)], on: &mut dyn FnMut(&str, &mut Vec<Act>)) {
    let with: Vec<(&str, Side, bool)> = items.iter().map(|(n, s)| (*n, *s, true)).collect();
    buttons_enabled(ui, acts, &with, on);
}

/// A row of buttons at the bottom: the left ones from the left edge, the right ones from the right
/// edge (the last listed is the rightmost).
fn buttons_enabled(ui: &mut egui::Ui, acts: &mut Vec<Act>, items: &[(&str, Side, bool)], on: &mut dyn FnMut(&str, &mut Vec<Act>)) {
    ui.add_space(14.0);
    ui.separator();
    let mut clicked: Option<&str> = None;
    ui.horizontal(|ui| {
        for (name, _, enabled) in items.iter().filter(|t| t.1 == Side::Left) {
            if crate::panel_ui::button_if(ui, *enabled, name).clicked() {
                clicked = Some(*name);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (name, _, enabled) in items.iter().rev().filter(|t| t.1 == Side::Right) {
                if crate::panel_ui::button_if(ui, *enabled, name).clicked() {
                    clicked = Some(*name);
                }
            }
        });
    });
    if let Some(name) = clicked {
        on(name, acts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_texts_are_iterm2s_with_termurion_for_the_app() {
        assert_eq!(HEADER, "Install Shell Integration");
        assert_eq!(DOWNLOAD_CAPTION, "Downloads the latest version. Runs curl | bash");
        assert_eq!(OFFLINE_CAPTION, "Guided manual installation. No download required.");
        assert_eq!(discovered("zsh"), "\u{2705} Discovered your shell: you use \u{201c}zsh\u{201d}.");
        assert_eq!(update_step(4), "Step 4. Update your shell\u{2019}s dotfile.");
        assert!(SHELL_NOT_SUPPORTED.ends_with("Only bash, fish, xonsh, and zsh work with shell integration"));
        for t in [FIRST_DESCRIPTION, SECOND_TEXT, UTILITIES_DESCRIPTION, FINISHED_TEXT] {
            assert!(!t.contains("iTerm2") && !t.contains("\u{2014}"), "{t}");
        }
    }

    #[test]
    fn the_curl_command_is_a_blank_line_then_the_pipe() {
        let c = curl_command(false);
        assert!(c.starts_with("\ncurl -L ") && c.ends_with(" | bash"), "{c}");
        assert_ne!(c, curl_command(true), "the utilities form names another script");
        assert!(confirm_text(true, false).starts_with(WAITING_FOR_COMMAND));
        assert!(confirm_text(false, false).starts_with(PRESS_CONTINUE_TO_RUN));
    }

    #[test]
    fn shells_are_named_by_the_last_word_of_the_path() {
        assert_eq!(Shell::from_name("zsh"), Shell::Zsh);
        assert_eq!(Shell::from_name("tcsh"), Shell::Unknown, "tcsh ships no script");
        assert_eq!(Shell::Fish.name(), "fish");
    }

    #[test]
    fn discovery_finds_the_shell_then_the_zsh_dotfile_folder() {
        let mut w = Wizard::new(Plan::Unix, false, "");
        w.choose_offline();
        w.stage = 0;
        let acts = w.paste_continue();
        assert_eq!(acts, vec![Act::SendText("echo My shell is $SHELL\n".to_string())]);
        assert!(w.busy());
        // A line that is not the answer changes nothing.
        assert!(w.output(&lines(&["echo My shell is $SHELL"])).is_empty());
        assert!(w.busy());
        let next = w.output(&lines(&["My shell is /bin/zsh"]));
        assert_eq!(next, vec![Act::SendText(ZSH_DOTDIR_COMMAND.to_string())], "zsh asks where its dotfiles go");
        assert_eq!(w.shell(), Shell::Zsh);
        assert!(w.output(&lines(&["My dotfiles go in /home/x/.config/zsh"])).is_empty());
        assert!(!w.busy());
        assert_eq!(w.dotdir(), "/home/x/.config/zsh");
        assert_eq!(w.stage, 1);
        let (stage_lines, bold) = w.stage_lines();
        assert!(stage_lines.iter().any(|l| l == &discovered("zsh")), "{stage_lines:?}");
        assert!(bold.is_some());
    }

    #[test]
    fn an_unsupported_shell_stops_the_install() {
        let mut w = Wizard::new(Plan::Unix, false, "");
        w.choose_offline();
        w.stage = 0;
        w.paste_continue();
        w.output(&lines(&["My shell is /bin/tcsh"]));
        assert_eq!(w.shell(), Shell::Unknown);
        assert!(w.paste_continue().is_empty(), "no script for it, so Continue types nothing");
        let (l, _) = w.stage_lines();
        assert!(l.iter().any(|x| x == SHELL_NOT_SUPPORTED));
    }

    #[test]
    fn the_script_is_written_through_a_here_document_of_the_shell() {
        let steps = script_steps(Shell::Bash, "~", false).expect("bash has a script");
        let typed: String = steps.iter().map(|s| s.text.as_str()).collect();
        assert!(typed.contains("cat <<'EOF' > ~/.iterm2_shell_integration.bash\n"), "{typed}");
        assert!(typed.contains("chmod +x ~/.iterm2_shell_integration.bash\n"));
        assert!(script_steps(Shell::Unknown, "~", false).is_none());
        // The terminal's answers are matched as the prompts print them.
        assert_eq!(PS1, "^>>( |$)");
        assert_eq!(PS2, "^>( |$)");
    }

    #[test]
    fn skipping_moves_to_the_next_stage_and_back_returns() {
        let mut w = Wizard::new(Plan::Unix, true, "");
        w.choose_offline();
        w.stage = 0;
        w.skip();
        assert_eq!(w.stage, 1);
        w.paste_back();
        assert_eq!(w.stage, 0);
        let mut plain = Wizard::new(Plan::Unix, false, "");
        plain.stage = 3;
        plain.skip();
        assert_eq!(plain.stage, 3, "without utilities there are four stages");
    }
}
