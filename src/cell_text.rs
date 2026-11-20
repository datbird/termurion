//! The text of one cell. Nearly every cell is one character, so up to 8 bytes live inline and
//! only a long cluster (a ZWJ emoji family) owns a `String`.

use crate::encoding::Form;

const INLINE: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CellText {
    /// The first `len` bytes of `buf` are valid UTF-8.
    Inline { len: u8, buf: [u8; INLINE] },
    Heap(String),
}

impl Default for CellText {
    fn default() -> Self {
        Self::new()
    }
}

impl CellText {
    pub fn new() -> Self {
        CellText::Inline { len: 0, buf: [0; INLINE] }
    }

    pub fn as_str(&self) -> &str {
        match self {
            // SAFETY: only whole chars are ever copied into `buf[..len]` (`push`, `from_str`).
            CellText::Inline { len, buf } => unsafe { std::str::from_utf8_unchecked(&buf[..*len as usize]) },
            CellText::Heap(s) => s,
        }
    }

    pub fn push(&mut self, c: char) {
        let mut tmp = [0u8; 4];
        let bytes = c.encode_utf8(&mut tmp).as_bytes();
        match self {
            CellText::Inline { len, buf } if *len as usize + bytes.len() <= INLINE => {
                buf[*len as usize..*len as usize + bytes.len()].copy_from_slice(bytes);
                *len += bytes.len() as u8;
            }
            CellText::Inline { .. } => {
                let mut s = String::with_capacity(self.as_str().len() + bytes.len());
                s.push_str(self.as_str());
                s.push(c);
                *self = CellText::Heap(s);
            }
            CellText::Heap(s) => s.push(c),
        }
    }

    pub fn extend<I: IntoIterator<Item = char>>(&mut self, it: I) {
        for c in it {
            self.push(c);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.as_str().is_empty()
    }

    pub fn is_ascii(&self) -> bool {
        self.as_str().is_ascii()
    }

    pub fn chars(&self) -> std::str::Chars<'_> {
        self.as_str().chars()
    }

    /// Swaps the first character for `with` (the bidi mirror).
    pub fn replace_first(&mut self, with: char) {
        let Some(first) = self.as_str().chars().next() else { return };
        let mut s = String::with_capacity(self.as_str().len() + 4);
        s.push(with);
        s.push_str(&self.as_str()[first.len_utf8()..]);
        *self = CellText::from(s.as_str());
    }

    /// The text in `form`. ASCII is left alone; anything else goes through a `String`.
    pub fn normalize(&mut self, form: Form) {
        if form == Form::None || self.is_ascii() {
            return;
        }
        let mut s = self.as_str().to_owned();
        crate::encoding::normalize_in_place(&mut s, form);
        *self = CellText::from(s.as_str());
    }
}

impl From<&str> for CellText {
    fn from(s: &str) -> Self {
        if s.len() <= INLINE {
            let mut buf = [0; INLINE];
            buf[..s.len()].copy_from_slice(s.as_bytes());
            CellText::Inline { len: s.len() as u8, buf }
        } else {
            CellText::Heap(s.to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_text_acts_like_a_string() {
        let cases = ["a", "\u{e9}", "e\u{301}", "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}", "\u{2764}\u{fe0f}", "(", ""];
        for text in cases {
            let (mut s, mut t) = (String::new(), CellText::new());
            let mut it = text.chars();
            if let Some(first) = it.next() {
                s.push(first);
                t.push(first);
            }
            s.extend(it.clone());
            t.extend(it);
            assert_eq!(t.as_str(), s, "pushes of {text:?}");
            assert_eq!((t.is_empty(), t.is_ascii()), (s.is_empty(), s.is_ascii()));
            assert_eq!(t.chars().collect::<Vec<_>>(), s.chars().collect::<Vec<_>>());
            assert_eq!(CellText::from(text).as_str(), text);

            if let Some(first) = s.chars().next() {
                let mirrored = crate::bidi::mirror(first);
                let mut s2 = s.clone();
                s2.replace_range(..first.len_utf8(), &mirrored.to_string());
                let mut t2 = t.clone();
                t2.replace_first(mirrored);
                assert_eq!(t2.as_str(), s2, "mirror of {text:?}");
            }
            // A swap to a wider character can cross the inline limit either way.
            let mut t3 = t.clone();
            t3.replace_first('\u{1F468}');
            let mut s3 = s.clone();
            if let Some(f) = s3.chars().next() {
                s3.replace_range(..f.len_utf8(), "\u{1F468}");
                assert_eq!(t3.as_str(), s3);
            }

            for form in [Form::None, Form::Nfc, Form::Nfd] {
                let (mut s4, mut t4) = (s.clone(), t.clone());
                crate::encoding::normalize_in_place(&mut s4, form);
                t4.normalize(form);
                assert_eq!(t4.as_str(), s4, "{form:?} of {text:?}");
            }
        }
    }

    #[test]
    fn eight_bytes_stay_inline_and_nine_move_out() {
        let mut t = CellText::from("abcdefgh");
        assert!(matches!(t, CellText::Inline { .. }));
        t.push('i');
        assert!(matches!(t, CellText::Heap(_)));
        assert_eq!(t.as_str(), "abcdefghi");
    }
}
