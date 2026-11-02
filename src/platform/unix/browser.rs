//! No embedded browser on Unix yet.

use super::super::Unsupported;

pub fn available() -> Result<(), Unsupported> {
    super::super::unsupported("The embedded browser")
}

/// The registered handler of http is not looked up on Unix yet; the button says "Default Browser".
pub fn default_browser_name() -> Option<String> {
    None
}

/// The handler of https is not looked up on Unix yet.
pub fn https_handler_name() -> Option<String> {
    None
}

/// There is no view setup here, so nothing starts InPrivate.
pub fn default_view_in_private() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_browser_unsupported() {
        assert!(available().unwrap_err().to_string().contains("The embedded browser is not available"));
        assert_eq!(default_browser_name(), None);
    }
}
