//! Runs the mouse probe under the shipped ConPTY and the Windows copy, and prints
//! a Markdown table. Exits 1 if any scenario fails under the shipped ConPTY.
//! Usage: probe [name filter]

#[cfg(windows)]
fn main() {
    use tandem::conpty::{self, Source, dev_vendor_dir};

    let filter = std::env::args().nth(1).unwrap_or_default();
    let mut shipped_failed = false;
    println!("| Scenario | ConPTY | Result | Detail |");
    println!("|---|---|---|---|");
    for source in [Source::Shipped(dev_vendor_dir()), Source::Inbox] {
        let shipped = matches!(source, Source::Shipped(_));
        let api = match conpty::load(&source) {
            Ok(api) => api,
            Err(e) => {
                println!("| load | {} | FAIL | {} |", source.label(), e.replace('|', "/"));
                shipped_failed |= shipped;
                continue;
            }
        };
        for o in tandem::probe::run_all(&api, &filter) {
            let result = if o.pass { "pass" } else { "FAIL" };
            let detail = o.detail.replace('|', "/").replace('\n', " ");
            println!("| {} | {} | {result} | {detail} |", o.name, o.conpty);
            shipped_failed |= shipped && !o.pass;
        }
    }
    std::process::exit(if shipped_failed { 1 } else { 0 });
}

#[cfg(not(windows))]
fn main() {}
