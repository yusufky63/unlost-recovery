//! File-type facts from public format specifications: which extensions belong to which group,
//! and what the first bytes of each type look like. Used to downgrade a deleted file whose
//! first cluster no longer starts like its type. This is a header check, not validation:
//! a matching header never makes a file "Verified".

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    Photo,
    Video,
    Document,
    Archive,
    Audio,
    Other,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::Photo,
        Category::Video,
        Category::Document,
        Category::Archive,
        Category::Audio,
        Category::Other,
    ];

    pub fn index(self) -> u8 {
        self as u8
    }

    pub fn from_index(i: u8) -> Option<Category> {
        Self::ALL.get(usize::from(i)).copied()
    }
}

pub fn extension(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

pub fn category(ext: &str) -> Category {
    match ext {
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "heic" | "heif" | "avif" | "tif"
        | "tiff" | "raw" | "cr2" | "cr3" | "nef" | "arw" | "dng" | "orf" | "rw2" | "psd"
        | "svg" | "ico" => Category::Photo,
        "mp4" | "mov" | "m4v" | "mkv" | "webm" | "avi" | "wmv" | "3gp" | "mts" | "m2ts" | "mpg"
        | "mpeg" | "flv" => Category::Video,
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp"
        | "rtf" | "txt" | "csv" | "md" | "epub" | "udf" | "xml" | "json" | "html" | "htm"
        | "pages" | "key" | "numbers" => Category::Document,
        "zip" | "7z" | "rar" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "cab" | "iso" => {
            Category::Archive
        }
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "opus" | "wma" | "aif" | "aiff"
        | "mid" => Category::Audio,
        _ => Category::Other,
    }
}

/// Human name of the type an extension promises, if we know its header.
pub fn type_name(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "jpg" | "jpeg" => "a JPEG image",
        "png" => "a PNG image",
        "gif" => "a GIF image",
        "bmp" => "a BMP image",
        "webp" => "a WebP image",
        "tif" | "tiff" => "a TIFF image",
        "psd" => "a Photoshop file",
        "pdf" => "a PDF document",
        "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub" | "zip" | "jar" | "apk" => {
            "a ZIP-based file"
        }
        "doc" | "xls" | "ppt" | "msg" => "an Office 97-2003 file",
        "rtf" => "an RTF document",
        "7z" => "a 7-Zip archive",
        "rar" => "a RAR archive",
        "gz" | "tgz" => "a gzip archive",
        "mp4" | "m4v" | "m4a" | "3gp" | "heic" | "heif" | "avif" | "mov" => "an MP4/MOV file",
        "mkv" | "webm" => "a Matroska file",
        "avi" | "wav" => "a RIFF file",
        "mp3" => "an MP3 file",
        "flac" => "a FLAC file",
        "ogg" | "opus" => "an Ogg file",
        "sqlite" | "db" => "an SQLite database",
        "exe" | "dll" => "a Windows program",
        _ => return None,
    })
}

/// `Some(true)` if `head` starts like the type `ext` promises, `Some(false)` if it clearly does
/// not, `None` when we have no rule for this extension or not enough bytes.
pub fn header_matches(ext: &str, head: &[u8]) -> Option<bool> {
    let starts = |sig: &[u8]| head.starts_with(sig);
    let at = |off: usize, sig: &[u8]| head.get(off..off + sig.len()) == Some(sig);
    if head.len() < 16 {
        return None;
    }
    Some(match ext {
        "jpg" | "jpeg" => starts(&[0xFF, 0xD8, 0xFF]),
        "png" => starts(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
        "gif" => starts(b"GIF87a") || starts(b"GIF89a"),
        "bmp" => starts(b"BM"),
        "webp" => starts(b"RIFF") && at(8, b"WEBP"),
        "tif" | "tiff" => starts(b"II*\0") || starts(b"MM\0*"),
        "psd" => starts(b"8BPS"),
        "pdf" => head.windows(5).take(1024).any(|w| w == b"%PDF-"),
        "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub" | "zip" | "jar" | "apk" => {
            starts(b"PK\x03\x04") || starts(b"PK\x05\x06") || starts(b"PK\x07\x08")
        }
        "doc" | "xls" | "ppt" | "msg" => starts(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]),
        "rtf" => starts(b"{\\rtf"),
        "7z" => starts(&[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C]),
        "rar" => starts(b"Rar!\x1A\x07"),
        "gz" | "tgz" => starts(&[0x1F, 0x8B]),
        "mp4" | "m4v" | "m4a" | "3gp" | "heic" | "heif" | "avif" => at(4, b"ftyp"),
        "mov" => [
            b"ftyp", b"moov", b"wide", b"mdat", b"free", b"skip", b"pnot",
        ]
        .iter()
        .any(|s| at(4, *s)),
        "mkv" | "webm" => starts(&[0x1A, 0x45, 0xDF, 0xA3]),
        "avi" => starts(b"RIFF") && at(8, b"AVI "),
        "wav" => starts(b"RIFF") && at(8, b"WAVE"),
        "mp3" => starts(b"ID3") || (head[0] == 0xFF && head[1] & 0xE0 == 0xE0),
        "flac" => starts(b"fLaC"),
        "ogg" | "opus" => starts(b"OggS"),
        "sqlite" => starts(b"SQLite format 3\0"),
        "exe" | "dll" => starts(b"MZ"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_and_categories() {
        assert_eq!(extension("Photo.JPG"), "jpg");
        assert_eq!(extension(".gitignore"), "");
        assert_eq!(extension("noext"), "");
        assert_eq!(extension("trailing."), "");
        assert_eq!(category("jpg"), Category::Photo);
        assert_eq!(category("docx"), Category::Document);
        assert_eq!(category("xyz"), Category::Other);
    }

    #[test]
    fn headers() {
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
        jpeg.resize(32, 0);
        assert_eq!(header_matches("jpg", &jpeg), Some(true));
        assert_eq!(header_matches("png", &jpeg), Some(false));
        assert_eq!(header_matches("xyz", &jpeg), None);
        assert_eq!(header_matches("jpg", &jpeg[..4]), None);
        let mut mp4 = vec![0, 0, 0, 0x20];
        mp4.extend_from_slice(b"ftypisom");
        mp4.resize(32, 0);
        assert_eq!(header_matches("mp4", &mp4), Some(true));
        assert_eq!(header_matches("mov", &mp4), Some(true));
        assert_eq!(header_matches("pdf", &[0u8; 64]), Some(false));
    }
}
