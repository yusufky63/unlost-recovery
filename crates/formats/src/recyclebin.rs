//! Recycle Bin `$I` metadata files.
//!
//! Each deleted item in `X:\$Recycle.Bin\<SID>\` has a `$I<id>.<ext>` file describing it and a
//! `$R<id>.<ext>` file or folder holding the data. Layout (community-documented, matching
//! Eric Zimmerman's RBCmd):
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 8 | version: 1 (Vista to 8.1) or 2 (Windows 10 and later) |
//! | 8 | 8 | original size in bytes |
//! | 16 | 8 | deletion time, FILETIME |
//! | v1: 24 | 520 | original path, UTF-16, NUL-padded |
//! | v2: 24 | 4 | path length in UTF-16 units, including the NUL |
//! | v2: 28 | 2 x n | original path, UTF-16 |

use serde::{Deserialize, Serialize};

use crate::{Error, Result, filetime, le};

/// NTFS paths are at most 32,767 UTF-16 units.
const MAX_PATH_UNITS: u32 = 32_768;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecycleInfo {
    pub version: u64,
    pub size: u64,
    /// Unix seconds, 0 if unknown.
    pub deleted: i64,
    pub original_path: String,
}

pub fn parse_info(b: &[u8]) -> Result<RecycleInfo> {
    let short = || Error::invalid("$I file", format!("only {} bytes", b.len()));
    let version = le::u64(b, 0).ok_or_else(short)?;
    let size = le::u64(b, 8).ok_or_else(short)?;
    let deleted = filetime::to_unix(le::u64(b, 16).ok_or_else(short)?);
    let original_path = match version {
        1 => le::utf16(b, 24, 260).ok_or_else(short)?,
        2 => {
            let units = le::u32(b, 24).ok_or_else(short)?;
            if units == 0 || units > MAX_PATH_UNITS {
                return Err(Error::invalid("$I file", format!("path length {units}")));
            }
            le::utf16(b, 28, units as usize).ok_or_else(short)?
        }
        v => return Err(Error::Unsupported(format!("$I version {v}"))),
    };
    if original_path.is_empty() {
        return Err(Error::invalid("$I file", "empty path"));
    }
    Ok(RecycleInfo {
        version,
        size,
        deleted,
        original_path,
    })
}

/// `$IAB12CD.docx` -> `$RAB12CD.docx`.
pub fn data_name_for(info_name: &str) -> Option<String> {
    let rest = info_name.strip_prefix("$I")?;
    if rest.is_empty() {
        return None;
    }
    Some(format!("$R{rest}"))
}

#[cfg(test)]
pub(crate) fn build_v2(size: u64, ft: u64, path: &str) -> Vec<u8> {
    let units: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut b = Vec::new();
    b.extend_from_slice(&2u64.to_le_bytes());
    b.extend_from_slice(&size.to_le_bytes());
    b.extend_from_slice(&ft.to_le_bytes());
    b.extend_from_slice(&(units.len() as u32).to_le_bytes());
    for u in units {
        b.extend_from_slice(&u.to_le_bytes());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v2() {
        let b = build_v2(
            1234,
            133_485_408_000_000_000,
            r"D:\Belgeler\Çağrı Özet.docx",
        );
        let info = parse_info(&b).unwrap();
        assert_eq!(info.version, 2);
        assert_eq!(info.size, 1234);
        assert_eq!(info.deleted, 1_704_067_200);
        assert_eq!(info.original_path, r"D:\Belgeler\Çağrı Özet.docx");
    }

    #[test]
    fn parses_v1() {
        let mut b = vec![0u8; 544];
        b[0] = 1;
        b[8..16].copy_from_slice(&10u64.to_le_bytes());
        for (i, u) in r"C:\a.txt".encode_utf16().enumerate() {
            b[24 + i * 2..26 + i * 2].copy_from_slice(&u.to_le_bytes());
        }
        let info = parse_info(&b).unwrap();
        assert_eq!(info.original_path, r"C:\a.txt");
        assert_eq!(info.deleted, 0);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse_info(&[]).is_err());
        assert!(parse_info(&[2, 0, 0, 0, 0, 0, 0, 0]).is_err());
        let mut b = build_v2(1, 1, "x");
        b[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_info(&b).is_err());
        let mut b = build_v2(1, 1, "x");
        b[0] = 9;
        assert!(parse_info(&b).is_err());
        assert_eq!(data_name_for("$IAB12.txt").as_deref(), Some("$RAB12.txt"));
        assert_eq!(data_name_for("$I"), None);
        assert_eq!(data_name_for("desktop.ini"), None);
    }
}
