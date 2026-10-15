//! Paste when the clipboard holds files or an image instead of text (`iTermNonTextPasteHelper.swift`):
//! the question, its buttons for what is on offer, the remembered answer's identifier, and the
//! large base64 confirmation. The Upload buttons are offered only when a session can upload
//! (`nonTextPasteHelperCanUpload`); Termurion's paste has no such session, so they are not here.

/// The buttons of a file paste (`FilePasteAction.displayName`, `:76-87`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileAction {
    PastePath,
    PastePaths,
    Base64,
    Base64Archive,
    AsText,
    Cancel,
}

impl FileAction {
    pub fn label(self) -> &'static str {
        match self {
            FileAction::PastePath => "Paste Path",
            FileAction::PastePaths => "Paste Paths",
            FileAction::Base64 => "Paste Base64-Encoded Contents",
            FileAction::Base64Archive => "Paste Base64-Encoded Archive (tar.gz)",
            FileAction::AsText => "Paste as Text",
            FileAction::Cancel => "Cancel",
        }
    }

    /// The stable name the remembered-answer identifier is built from (`rawValue`).
    fn raw(self) -> &'static str {
        self.label()
    }
}

/// The buttons to offer (`fileActions`, `:152-190`) with no upload.
pub fn file_actions(single: bool, is_dir: bool, can_text: bool) -> Vec<FileAction> {
    let mut v = Vec::new();
    if single {
        v.push(FileAction::PastePath);
        if is_dir {
            v.push(FileAction::Base64Archive);
        } else {
            v.push(FileAction::Base64);
            if can_text {
                v.push(FileAction::AsText);
            }
        }
    } else {
        v.push(FileAction::PastePaths);
    }
    v.push(FileAction::Cancel);
    v
}

/// `warningIdentifier(prefix:names:)` (`:131-139`): the letters and digits of each name, sorted, joined with `_`.
fn warning_identifier(prefix: &str, names: &[&str]) -> String {
    let mut tokens: Vec<String> = names.iter().map(|n| n.chars().filter(|c| c.is_alphanumeric()).collect()).collect();
    tokens.sort();
    std::iter::once(prefix.to_string()).chain(tokens).collect::<Vec<_>>().join("_")
}

/// `fileWarningIdentifier` (`:150`): Cancel and Paste as Text are left out.
pub fn file_identifier(actions: &[FileAction]) -> String {
    let names: Vec<&str> = actions.iter().filter(|a| !matches!(a, FileAction::Cancel | FileAction::AsText)).map(|a| a.raw()).collect();
    warning_identifier("NoSyncPasteNonTextFile", &names)
}

pub const FOLDER_HEADING: &str = "Paste Folder";
pub const FILE_HEADING: &str = "Paste File";
pub const FILES_HEADING: &str = "Paste Files";
pub const IMAGE_HEADING: &str = "Paste Image";

pub fn file_heading(single: bool, is_dir: bool) -> &'static str {
    if is_dir {
        FOLDER_HEADING
    } else if single {
        FILE_HEADING
    } else {
        FILES_HEADING
    }
}

/// `pasteFilesPrompt` (`:262-275`); `names` are the last path components.
pub fn file_prompt(names: &[String], is_dir: bool) -> String {
    match names {
        [one] if is_dir => format!("How would you like to paste the folder \u{201c}{one}\u{201d}?"),
        [one] => format!("How would you like to paste the file \u{201c}{one}\u{201d}?"),
        few if few.len() <= 3 => {
            let joined = few.iter().map(|n| format!("\u{201c}{n}\u{201d}")).collect::<Vec<_>>().join(", ");
            format!("How would you like to paste {joined}?")
        }
        many => format!("How would you like to paste these {} files?", many.len()),
    }
}

/// The errors (`:229-240`, `:299`, `:498`, `:581`, `:601`) and their heading.
pub const FAILED_HEADING: &str = "Paste Failed";
pub const COPIED_FILE_MISSING: &str = "The copied file no longer exists.";

pub fn some_missing(n: usize) -> String {
    format!("{n} dropped files no longer exist and will be skipped.")
}

pub fn could_not_read(name: &str) -> String {
    format!("Could not read file \u{201c}{name}\u{201d}.")
}

pub fn not_utf8(name: &str) -> String {
    format!("File \u{201c}{name}\u{201d} is not valid UTF-8 text.")
}

pub fn archive_failed(name: &str, why: &str) -> String {
    format!("Could not create archive of \u{201c}{name}\u{201d}: {why}")
}

pub const NO_TEMP_DIR: &str = "Could not create temporary directory.";

pub fn image_not_saved(why: &str) -> String {
    format!("Could not save image to temporary file: {why}")
}

/// The sample read to decide whether a file is text (`utf8SampleByteCount`).
pub const UTF8_SAMPLE: usize = 50 * 1024;

/// `prefixIsValidUTF8`: the first 50 KB, a sequence cut by the sample's end forgiven.
pub fn prefix_is_utf8(data: &[u8]) -> bool {
    let cut = data.len() > UTF8_SAMPLE;
    let sample = &data[..data.len().min(UTF8_SAMPLE)];
    match std::str::from_utf8(sample) {
        Ok(_) => true,
        Err(e) => cut && e.error_len().is_none(),
    }
}

/// The buttons of an image paste (`ImagePasteAction`, `:326-388`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageAction {
    SaveTemp,
    Base64,
    Cancel,
}

impl ImageAction {
    pub fn label(self) -> &'static str {
        match self {
            ImageAction::SaveTemp => "Save to Temp File and Paste Path",
            ImageAction::Base64 => "Paste Base64-Encoded Contents",
            ImageAction::Cancel => "Cancel",
        }
    }
}

/// `imageActions` with no upload; the type is always known (the image is made a PNG).
pub fn image_actions() -> Vec<ImageAction> {
    vec![ImageAction::SaveTemp, ImageAction::Base64, ImageAction::Cancel]
}

pub fn image_identifier(actions: &[ImageAction]) -> String {
    let names: Vec<&str> = actions.iter().filter(|a| **a != ImageAction::Cancel).map(|a| a.label()).collect();
    warning_identifier("NoSyncPasteImage", &names)
}

/// `imageDataPrompt` for a PNG (`:437`).
pub fn png_prompt(size: &str) -> String {
    format!("The clipboard contains a PNG image ({size}). How would you like to paste it?")
}

/// `ByteCountFormatter` in file style: decimal units, one decimal place under 10 above KB.
pub fn byte_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if n < 1000 {
        return if n == 1 { "1 byte".to_string() } else { format!("{n} bytes") };
    }
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if v < 10.0 && u > 1 { format!("{v:.1} {}", UNITS[u]) } else { format!("{:.0} {}", v, UNITS[u]) }
}

/// The base64 text over this many characters asks first (`pasteBase64WithConfirmationIfNeeded`, `:623`).
pub const LARGE_PASTE_ABOVE: usize = 10_000;
pub const LARGE_PASTE_ID: &str = "NoSyncPasteLargeBase64";
pub const LARGE_PASTE_HEADING: &str = "Large Paste";

pub fn large_paste_text(n: usize) -> String {
    format!("OK to paste {n} bytes of base64-encoded data?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_buttons_follow_iterm2_without_upload() {
        use FileAction::*;
        assert_eq!(file_actions(true, false, true), [PastePath, Base64, AsText, Cancel]);
        assert_eq!(file_actions(true, false, false), [PastePath, Base64, Cancel]);
        assert_eq!(file_actions(true, true, true), [PastePath, Base64Archive, Cancel]);
        assert_eq!(file_actions(false, false, false), [PastePaths, Cancel]);
        assert_eq!(Base64Archive.label(), "Paste Base64-Encoded Archive (tar.gz)");
    }

    #[test]
    fn identifiers_leave_out_cancel_and_text() {
        use FileAction::*;
        assert_eq!(file_identifier(&[PastePath, Base64, AsText, Cancel]), file_identifier(&[PastePath, Base64, Cancel]));
        assert_eq!(file_identifier(&[PastePath, Base64Archive, Cancel]), "NoSyncPasteNonTextFile_PasteBase64EncodedArchivetargz_PastePath");
        assert_eq!(image_identifier(&image_actions()), "NoSyncPasteImage_PasteBase64EncodedContents_SavetoTempFileandPastePath");
    }

    #[test]
    fn prompts_follow_iterm2() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(file_prompt(&n(&["a.txt"]), false), "How would you like to paste the file \u{201c}a.txt\u{201d}?");
        assert_eq!(file_prompt(&n(&["d"]), true), "How would you like to paste the folder \u{201c}d\u{201d}?");
        assert_eq!(file_prompt(&n(&["a", "b"]), false), "How would you like to paste \u{201c}a\u{201d}, \u{201c}b\u{201d}?");
        assert_eq!(file_prompt(&n(&["a", "b", "c", "d"]), false), "How would you like to paste these 4 files?");
        assert_eq!((file_heading(true, true), file_heading(true, false), file_heading(false, false)), ("Paste Folder", "Paste File", "Paste Files"));
        assert_eq!(png_prompt("1.2 MB"), "The clipboard contains a PNG image (1.2 MB). How would you like to paste it?");
        assert_eq!(large_paste_text(12345), "OK to paste 12345 bytes of base64-encoded data?");
    }

    #[test]
    fn sizes_and_text_sniffing() {
        assert_eq!((byte_size(1).as_str(), byte_size(999).as_str(), byte_size(1500).as_str(), byte_size(1_200_000).as_str(), byte_size(25_000_000).as_str()), ("1 byte", "999 bytes", "2 KB", "1.2 MB", "25 MB"));
        assert!(prefix_is_utf8("h\u{e9}llo".as_bytes()));
        assert!(!prefix_is_utf8(&[0xff, 0xfe, 0x00]));
        let mut cut = "a".repeat(UTF8_SAMPLE - 1).into_bytes();
        cut.extend("\u{e9}".as_bytes());
        cut.push(b'x');
        assert!(prefix_is_utf8(&cut), "a sequence cut by the sample's end is forgiven");
        assert!(!prefix_is_utf8(&{ let mut b = b"ab".to_vec(); b.push(0xc3); b }), "a file that really ends mid-sequence is not text");
    }
}
