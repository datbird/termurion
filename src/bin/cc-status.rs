//! Claude Code's status hook for Termurion (milestone 12f), the Windows port of iTerm2's
//! `cc-status`: reads one hook event from stdin and reports it to Termurion. A console program.

fn main() {
    #[cfg(windows)]
    termurion::dll::harden_process();
    #[cfg(windows)]
    std::process::exit(termurion::cc_status::real_main());
}
