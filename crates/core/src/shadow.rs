//! Existing Volume Shadow Copies ("Previous Versions"), read-only.
//!
//! Unlost never creates a shadow copy: on the source volume that would write a diff area over
//! the very space deleted files occupy. It lists the ones Windows already has (WMI
//! `Win32_ShadowCopy`) and reads files from their devices with normal read-only file APIs.

use std::path::{Component, Path, PathBuf};

use crate::disks::volume_roots;
use crate::{Error, Result, wmi};
use serde::{Deserialize, Serialize};

const DEVICE_PREFIX: &str = r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy";
/// Entries returned for one folder listing.
const MAX_DIR_ENTRIES: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShadowCopy {
    pub id: String,
    /// `\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopyN`
    pub device: String,
    /// Original volume, `\\?\Volume{...}\`.
    pub volume: String,
    /// Drive letters of the original volume.
    pub roots: Vec<String>,
    /// Unix seconds.
    pub created: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShadowEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: i64,
}

/// `\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy<digits>` and nothing else.
pub fn valid_device(device: &str) -> bool {
    device
        .strip_prefix(DEVICE_PREFIX)
        .is_some_and(|n| !n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// A relative path inside a snapshot: plain names only, no `..`, roots or drive prefixes.
pub fn safe_relative(path: &str) -> Option<PathBuf> {
    let p = Path::new(path.trim_start_matches(['\\', '/']));
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(n) => {
                let s = n.to_str()?;
                if s.contains(':') {
                    return None;
                }
                out.push(n);
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(out)
}

pub fn full_path(device: &str, rel: &str) -> Result<PathBuf> {
    if !valid_device(device) {
        return Err(Error::Refused(format!(
            "not a shadow copy device: {device}"
        )));
    }
    let rel = safe_relative(rel).ok_or_else(|| Error::Refused(format!("bad path: {rel}")))?;
    let mut p = PathBuf::from(format!("{device}\\"));
    p.push(rel);
    Ok(p)
}

pub fn list_dir(device: &str, rel: &str) -> Result<Vec<ShadowEntry>> {
    let dir = full_path(device, rel)?;
    let rd = std::fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))?;
    let mut out = Vec::new();
    for e in rd.flatten().take(MAX_DIR_ENTRIES) {
        let Ok(m) = e.metadata() else { continue };
        let modified = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        out.push(ShadowEntry {
            name: e.file_name().to_string_lossy().into_owned(),
            is_dir: m.is_dir(),
            size: if m.is_dir() { 0 } else { m.len() },
            modified,
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

/// Lists existing shadow copies through WMI (`Win32_ShadowCopy`, administrators only).
pub fn list() -> Result<Vec<ShadowCopy>> {
    let rows = wmi::query(
        "SELECT ID, DeviceObject, VolumeName, InstallDate FROM Win32_ShadowCopy",
        &["ID", "DeviceObject", "VolumeName", "InstallDate"],
    )?;
    let mut out: Vec<ShadowCopy> = rows
        .into_iter()
        .filter_map(|r| {
            let device = r.get("DeviceObject")?.clone();
            if !valid_device(&device) {
                return None;
            }
            let volume = r.get("VolumeName").cloned().unwrap_or_default();
            Some(ShadowCopy {
                id: r.get("ID").cloned().unwrap_or_default(),
                roots: volume_roots(&volume),
                created: r
                    .get("InstallDate")
                    .and_then(|s| cim_datetime(s))
                    .unwrap_or(0),
                device,
                volume,
            })
        })
        .collect();
    out.sort_by_key(|c| std::cmp::Reverse(c.created));
    Ok(out)
}

/// CIM datetime `yyyymmddHHMMSS.mmmmmmsUUU` (UUU: minutes east of UTC) to Unix seconds.
pub fn cim_datetime(s: &str) -> Option<i64> {
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<u32>().ok();
    let date = chrono::NaiveDate::from_ymd_opt(num(0..4)? as i32, num(4..6)?, num(6..8)?)?;
    let local = date.and_hms_opt(num(8..10)?, num(10..12)?, num(12..14)?)?;
    let sign = match s.get(21..22)? {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    let offset = i64::from(num(22..25)?) * sign;
    Some(local.and_utc().timestamp() - offset * 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_devices_and_paths() {
        assert!(valid_device(
            r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy12"
        ));
        assert!(!valid_device(
            r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy"
        ));
        assert!(!valid_device(r"\\?\GLOBALROOT\Device\HarddiskVolume3"));
        assert!(!valid_device(
            r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy1\..\x"
        ));
        assert_eq!(
            safe_relative(r"\Users\me\a.txt"),
            Some(PathBuf::from(r"Users\me\a.txt"))
        );
        assert_eq!(safe_relative(""), Some(PathBuf::new()));
        assert_eq!(safe_relative(r"Users\..\..\Windows"), None);
        assert_eq!(safe_relative(r"C:\Windows"), None);
        assert_eq!(safe_relative(r"a\b:stream"), None);
        let p = full_path(
            r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy3",
            r"\Users\me",
        )
        .unwrap();
        assert_eq!(
            p,
            PathBuf::from(r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy3\Users\me")
        );
        assert!(full_path(r"C:\", "x").is_err());
    }

    #[test]
    fn parses_cim_dates() {
        // 2026-10-16 12:00:00 at UTC+3 is 09:00:00 UTC.
        assert_eq!(
            cim_datetime("20261016120000.000000+180"),
            Some(1_792_141_200)
        );
        assert_eq!(cim_datetime("garbage"), None);
    }

    #[test]
    fn wmi_listing_returns_or_fails_cleanly() {
        // Non-elevated processes may be refused; either outcome must not panic.
        match list() {
            Ok(copies) => assert!(copies.iter().all(|c| valid_device(&c.device))),
            Err(e) => assert!(!e.to_string().is_empty()),
        }
    }
}
