//! The IME composition drawn at the cursor (filled in by a later task of 13k).

/// What a composition looks like on the grid; empty until the IME task gives it fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawn {}
