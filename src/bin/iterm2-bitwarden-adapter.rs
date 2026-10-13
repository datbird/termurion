//! The Bitwarden adapter executable (iTerm2's `iterm2-bitwarden-adapter`): argument 1 is the
//! subcommand, stdin the request JSON, stdout the response. All logic is in `termurion::pwm_bitwarden`.

#[cfg(windows)]
fn main() {
    termurion::dll::harden_process();
    let args: Vec<String> = std::env::args().collect();
    let mut runner = termurion::pwm_run::ProcessRunner::new();
    let code = termurion::pwm_adapter::run_main(&args, &mut std::io::stdin().lock(), &mut std::io::stdout().lock(), termurion::pwm_bitwarden::handle, &mut runner);
    std::process::exit(code);
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The Bitwarden adapter runs on Windows only.");
    std::process::exit(1);
}
