//! Probe helper: prints the first two console mouse events it receives, then exits.

#[cfg(windows)]
fn main() {
    tandem::dll::harden_process();
    use windows_sys::Win32::System::Console::{
        ENABLE_EXTENDED_FLAGS, ENABLE_MOUSE_INPUT, ENABLE_WINDOW_INPUT, GetStdHandle, INPUT_RECORD,
        MOUSE_EVENT, ReadConsoleInputW, STD_INPUT_HANDLE, SetConsoleMode,
    };
    unsafe {
        let input = GetStdHandle(STD_INPUT_HANDLE);
        // ENABLE_EXTENDED_FLAGS without ENABLE_QUICK_EDIT_MODE turns QuickEdit off.
        // QuickEdit would eat the mouse input.
        if SetConsoleMode(input, ENABLE_EXTENDED_FLAGS | ENABLE_MOUSE_INPUT | ENABLE_WINDOW_INPUT) == 0 {
            eprintln!("SetConsoleMode failed: {}", std::io::Error::last_os_error());
            std::process::exit(2);
        }
        println!("READY");
        let mut seen = 0;
        while seen < 2 {
            let mut rec: INPUT_RECORD = std::mem::zeroed();
            let mut n = 0u32;
            if ReadConsoleInputW(input, &mut rec, 1, &mut n) == 0 {
                eprintln!("ReadConsoleInputW failed: {}", std::io::Error::last_os_error());
                std::process::exit(2);
            }
            if n == 1 && rec.EventType as u32 == MOUSE_EVENT {
                let m = rec.Event.MouseEvent;
                println!(
                    "MOUSE x={} y={} buttons={} flags={}",
                    m.dwMousePosition.X, m.dwMousePosition.Y, m.dwButtonState, m.dwEventFlags
                );
                seen += 1;
            }
        }
    }
}

#[cfg(not(windows))]
fn main() {}
