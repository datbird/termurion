//! iTerm2's utilities for PowerShell and cmd (milestone 11e): one console program that is
//! imgcat, imgls, it2api, it2attention, it2cat, it2check, it2copy, it2dl, it2getvar, it2git,
//! it2profile, it2setcolor, it2setkeylabel, it2tip, it2ul and it2universion, chosen by its
//! own file name (the `utilities` folder's links) or its first argument.

fn main() {
    #[cfg(windows)]
    tandem::dll::harden_process();
    #[cfg(windows)]
    std::process::exit(tandem::util_console::real_main());
}
