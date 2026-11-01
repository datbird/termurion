//! No screen reader support on Unix yet.

use super::super::Unsupported;

pub fn available() -> Result<(), Unsupported> {
    super::super::unsupported("Screen reader support")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_a11y_unsupported() {
        assert!(available().unwrap_err().to_string().contains("Screen reader support is not available"));
    }
}
