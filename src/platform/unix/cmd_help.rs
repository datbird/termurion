//! Get-Help is PowerShell's; the answer says so where there is none.

pub fn get_help(_args: &[String]) -> String {
    format!("Error running Get-Help: PowerShell is not available on {} yet.", crate::platform::OS_NAME)
}
