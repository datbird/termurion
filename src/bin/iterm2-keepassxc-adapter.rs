//! The KeePassXC adapter executable (iTerm2's `iterm2-keepassxc-adapter`): argument 1 is the
//! subcommand, stdin the request JSON, stdout the response. All logic is in `tandem::pwm_keepassxc`.

#[cfg(windows)]
fn main() {
    tandem::dll::harden_process();
    let args: Vec<String> = std::env::args().collect();
    let mut runner = tandem::pwm_run::ProcessRunner::new();
    let code = tandem::pwm_adapter::run_main(&args, &mut std::io::stdin().lock(), &mut std::io::stdout().lock(), tandem::pwm_keepassxc::handle, &mut runner);
    std::process::exit(code);
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The KeePassXC adapter runs on Windows only.");
    std::process::exit(1);
}
