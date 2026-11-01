//! No printing on Unix yet. Print reports Unsupported; Page Setup has no dialog to open.

use super::super::Unsupported;
use crate::styled_copy::Row;

/// The page setup a print would use. Nothing is kept on Unix.
#[derive(Clone, Debug, Default)]
pub struct Setup;

/// Page Setup has no dialog here, so nothing changes.
pub fn page_setup(_owner: isize, _setup: &mut Setup) {}

pub fn print(_owner: isize, _setup: &mut Setup, _title: &str, _rows: &mut [Row], _black_and_white: &mut bool, _cols: usize, _family: &str, _size_pt: f32) -> Result<bool, String> {
    Err(Unsupported { feature: "Printing" }.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_print_unsupported() {
        let e = print(0, &mut Setup, "t", &mut [], &mut true, 80, "f", 10.0).unwrap_err();
        assert!(e.contains("Printing is not available"), "{e}");
    }
}
