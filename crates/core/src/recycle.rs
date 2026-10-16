//! Recycle Bin contents of the signed-in user on every drive, read from `$I`/`$R` pairs.
//!
//! Only the current user's `X:\$Recycle.Bin\<SID>` folder is read, which needs no administrator
//! rights. Other users' bins are private and not listed.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use unlost_formats::recyclebin::{data_name_for, parse_info};

/// `$I` files are a few hundred bytes; anything larger is not one.
const MAX_INFO_BYTES: u64 = 70_000;
/// Files counted when sizing a deleted folder.
const MAX_FOLDER_ENTRIES: usize = 200_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BinItem {
    /// Stable id: the `$I` file path.
    pub id: String,
    /// `C:\`
    pub drive: String,
    pub original_path: String,
    pub name: String,
    pub size: u64,
    /// Unix seconds, 0 if unknown.
    pub deleted: i64,
    pub is_dir: bool,
    /// False when the `$R` data is gone (emptied or restored elsewhere) but `$I` stayed.
    pub present: bool,
    pub data_path: String,
}

pub fn bin_dir(root: &str, sid: &str) -> PathBuf {
    Path::new(root).join("$Recycle.Bin").join(sid)
}

/// Items in one bin folder. Unreadable or malformed entries are skipped.
pub fn list_dir(dir: &Path, drive: &str) -> Vec<BinItem> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(data_name) = data_name_for(&name) else {
            continue;
        };
        let path = e.path();
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_file() || meta.len() > MAX_INFO_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(info) = parse_info(&bytes) else {
            continue;
        };
        let data = dir.join(&data_name);
        let data_meta = std::fs::symlink_metadata(&data).ok();
        let is_dir = data_meta.as_ref().is_some_and(|m| m.is_dir());
        let size = match &data_meta {
            Some(m) if m.is_dir() => folder_size(&data),
            Some(m) => m.len(),
            None => info.size,
        };
        let display = info
            .original_path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(&info.original_path)
            .to_string();
        out.push(BinItem {
            id: path.to_string_lossy().into_owned(),
            drive: drive.to_string(),
            name: display,
            original_path: info.original_path,
            size,
            deleted: info.deleted,
            is_dir,
            present: data_meta.is_some(),
            data_path: data.to_string_lossy().into_owned(),
        });
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.deleted));
    out
}

/// The current user's bin items on each of `roots` (`C:\`, `D:\`, ...).
pub fn list(sid: &str, roots: &[String]) -> Vec<BinItem> {
    let mut all: Vec<BinItem> = roots
        .iter()
        .flat_map(|r| list_dir(&bin_dir(r, sid), r))
        .collect();
    all.sort_by_key(|i| std::cmp::Reverse(i.deleted));
    all
}

fn folder_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    let mut seen = 0usize;
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > MAX_FOLDER_ENTRIES {
                return total;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
            } else if ft.is_file() {
                total += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

/// Validates that `data_path` is a `$R` entry inside the current user's bin on `drive`, so a
/// UI request cannot point the copy at an arbitrary file.
pub fn validate_item(sid: &str, drive: &str, data_path: &Path) -> bool {
    let dir = bin_dir(drive, sid);
    data_path.parent().is_some_and(|p| p == dir)
        && data_path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("$R") && n.len() > 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info_v2(path: &str, size: u64) -> Vec<u8> {
        let units: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut b = Vec::new();
        b.extend_from_slice(&2u64.to_le_bytes());
        b.extend_from_slice(&size.to_le_bytes());
        b.extend_from_slice(&133_485_408_000_000_000u64.to_le_bytes());
        b.extend_from_slice(&(units.len() as u32).to_le_bytes());
        for u in units {
            b.extend_from_slice(&u.to_le_bytes());
        }
        b
    }

    #[test]
    fn lists_files_folders_and_orphans() {
        let tmp = tempfile::tempdir().unwrap();
        let sid = "S-1-5-21-1-2-3-1001";
        let dir = bin_dir(tmp.path().to_str().unwrap(), sid);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("$IAAA111.txt"), info_v2(r"D:\Notlar\Çağrı.txt", 5)).unwrap();
        std::fs::write(dir.join("$RAAA111.txt"), b"hello").unwrap();
        std::fs::write(dir.join("$IBBB222"), info_v2(r"D:\Old folder", 0)).unwrap();
        std::fs::create_dir_all(dir.join("$RBBB222").join("sub")).unwrap();
        std::fs::write(dir.join("$RBBB222").join("sub").join("a.bin"), [1u8; 300]).unwrap();
        std::fs::write(dir.join("$ICCC333.doc"), info_v2(r"D:\gone.doc", 42)).unwrap();
        std::fs::write(dir.join("$IJUNK.txt"), b"not an info file").unwrap();
        std::fs::write(dir.join("desktop.ini"), b"[x]").unwrap();

        let items = list_dir(&dir, "D:\\");
        assert_eq!(items.len(), 3);
        let txt = items.iter().find(|i| i.name == "Çağrı.txt").unwrap();
        assert!(txt.present && !txt.is_dir);
        assert_eq!(txt.size, 5);
        assert_eq!(txt.deleted, 1_704_067_200);
        let folder = items.iter().find(|i| i.name == "Old folder").unwrap();
        assert!(folder.is_dir);
        assert_eq!(folder.size, 300);
        let gone = items.iter().find(|i| i.name == "gone.doc").unwrap();
        assert!(!gone.present);
        assert_eq!(gone.size, 42);

        let root = tmp.path().to_str().unwrap();
        assert!(validate_item(sid, root, Path::new(&txt.data_path)));
        assert!(!validate_item(sid, root, &dir.join("$IAAA111.txt")));
        assert!(!validate_item(sid, root, &tmp.path().join("$RAAA111.txt")));
        assert!(!validate_item(
            "S-1-5-21-other",
            root,
            Path::new(&txt.data_path)
        ));
    }

    #[test]
    fn reads_the_real_bin_without_failing() {
        let sid = crate::system::current_user_sid().unwrap();
        let _ = list(&sid, &["C:\\".to_string()]);
    }
}
