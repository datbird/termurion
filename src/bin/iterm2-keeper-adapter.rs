//! The Keeper adapter executable (iTerm2's `iterm2-keeper-adapter`): argument 1 is the
//! subcommand, stdin the request JSON, stdout the response. All logic is in `termurion::pwm_keeper`.

#[cfg(windows)]
fn main() {
    termurion::dll::harden_process();
    let args: Vec<String> = std::env::args().collect();
    let mut http = termurion::pwm_keeper::WinHttpBlocking::new();
    let code = termurion::pwm_adapter::run_main_with(&args, &mut std::io::stdin().lock(), &mut std::io::stdout().lock(), |c, i| termurion::pwm_keeper::handle(c, i, &mut http));
    std::process::exit(code);
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The Keeper adapter runs on Windows only.");
    std::process::exit(1);
}
