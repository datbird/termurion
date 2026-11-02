//! The machine's facts. The processor model is not read yet; the OS is named by `OS_NAME`.

use super::super::sysinfo::Machine;

pub fn machine() -> Machine {
    Machine { cpu: String::new(), product: super::super::OS_NAME.to_string(), build: String::new() }
}
