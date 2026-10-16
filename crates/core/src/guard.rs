//! Destination guard: recovered files never go to a physical disk the source lives on.
//!
//! Drive letters are not trusted. The destination folder is opened and resolved with
//! `GetFinalPathNameByHandleW`, which follows junctions, mounted folders and `subst`, to its
//! volume; the volume is mapped to physical disks with `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`;
//! a virtual disk (VHD/VHDX) also counts the disks holding its image file. A shared disk is
//! refused, not warned about. When the disk cannot be determined (network share, unknown
//! virtual disk) the copy is refused unless the user explicitly confirms.

use std::collections::BTreeSet;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, GetFinalPathNameByHandleW, VOLUME_NAME_DOS, VOLUME_NAME_GUID,
};

use crate::disks::{Bus, disk_info, virtual_host_disks, volume_device, volume_disks};
use crate::winstr::from_wide;
use crate::{Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskSet {
    pub disks: Vec<u32>,
    /// Why the disks could not be fully determined, if they could not.
    pub unknown: Option<String>,
    pub network: bool,
    /// `\\?\Volume{...}\` of the resolved volume, when known.
    pub volume: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "verdict",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Verdict {
    Ok { source: Vec<u32>, dest: Vec<u32> },
    SameDisk { disks: Vec<u32> },
    Unknown { reason: String, network: bool },
}

/// Pure decision, given both disk sets.
pub fn verdict(source: &DiskSet, dest: &DiskSet) -> Verdict {
    let shared: Vec<u32> = source
        .disks
        .iter()
        .filter(|d| dest.disks.contains(d))
        .copied()
        .collect();
    if !shared.is_empty() {
        return Verdict::SameDisk { disks: shared };
    }
    if let Some(r) = dest.unknown.as_ref().or(source.unknown.as_ref()) {
        return Verdict::Unknown {
            reason: r.clone(),
            network: dest.network,
        };
    }
    if source.disks.is_empty() || dest.disks.is_empty() {
        return Verdict::Unknown {
            reason: "no disk found".into(),
            network: dest.network,
        };
    }
    Verdict::Ok {
        source: source.disks.clone(),
        dest: dest.disks.clone(),
    }
}

/// Refuses unless the verdict is `Ok`, or `Unknown` and the user confirmed.
pub fn enforce(source: &DiskSet, dest: &DiskSet, confirmed_unknown: bool) -> Result<()> {
    match verdict(source, dest) {
        Verdict::Ok { .. } => Ok(()),
        Verdict::SameDisk { disks } => Err(Error::SameDisk(format!("disk {disks:?}"))),
        Verdict::Unknown { .. } if confirmed_unknown => Ok(()),
        Verdict::Unknown { reason, .. } => Err(Error::UnknownDisk(reason)),
    }
}

/// Physical disks holding `path` (a folder or a file), following links and virtual disks.
pub fn disks_of_path(path: &Path) -> Result<DiskSet> {
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
        .open(path)
        .map_err(|e| Error::io(path, e))?;
    let h = HANDLE(file.as_raw_handle());
    let mut buf = vec![0u16; 32_768];
    let n = unsafe { GetFinalPathNameByHandleW(h, &mut buf, VOLUME_NAME_GUID) } as usize;
    if n == 0 || n >= buf.len() {
        let n = unsafe { GetFinalPathNameByHandleW(h, &mut buf, VOLUME_NAME_DOS) } as usize;
        let dos = from_wide(&buf[..n.min(buf.len())]);
        let network = dos.starts_with(r"\\?\UNC\") || dos.starts_with(r"\\");
        return Ok(DiskSet {
            unknown: Some(if network {
                "network location".into()
            } else {
                "the volume has no GUID path".into()
            }),
            network,
            ..DiskSet::default()
        });
    }
    let final_path = from_wide(&buf[..n]);
    let Some(end) = final_path.find("}\\") else {
        return Ok(DiskSet {
            unknown: Some(format!("unexpected path {final_path}")),
            ..DiskSet::default()
        });
    };
    let volume = final_path[..end + 2].to_string();
    Ok(disks_of_volume(&volume))
}

/// Physical disks of a volume given as `\\?\Volume{...}\`.
pub fn disks_of_volume(volume: &str) -> DiskSet {
    let dev = volume_device(volume);
    let mut set = DiskSet {
        volume: volume.to_string(),
        ..DiskSet::default()
    };
    let disks = match volume_disks(&dev) {
        Ok(d) => d,
        Err(e) => {
            set.unknown = Some(e.to_string());
            return set;
        }
    };
    let mut all: BTreeSet<u32> = disks.iter().copied().collect();
    for d in disks {
        if disk_info(d).is_ok_and(|i| i.bus == Bus::Virtual) {
            match virtual_host_disks(&dev, 0) {
                Some(hosts) => all.extend(hosts),
                None => set.unknown = Some("virtual disk whose image location is unknown".into()),
            }
        }
    }
    set.disks = all.into_iter().collect();
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(disks: &[u32]) -> DiskSet {
        DiskSet {
            disks: disks.to_vec(),
            ..DiskSet::default()
        }
    }

    #[test]
    fn decisions() {
        assert_eq!(
            verdict(&set(&[0]), &set(&[1])),
            Verdict::Ok {
                source: vec![0],
                dest: vec![1]
            }
        );
        assert_eq!(
            verdict(&set(&[0, 2]), &set(&[2, 3])),
            Verdict::SameDisk { disks: vec![2] }
        );
        let net = DiskSet {
            unknown: Some("network location".into()),
            network: true,
            ..DiskSet::default()
        };
        assert!(matches!(
            verdict(&set(&[0]), &net),
            Verdict::Unknown { network: true, .. }
        ));
        // A known shared disk wins over "unknown" for the rest.
        let mut partly = set(&[0]);
        partly.unknown = Some("virtual".into());
        assert!(matches!(
            verdict(&set(&[0]), &partly),
            Verdict::SameDisk { .. }
        ));
        assert!(enforce(&set(&[0]), &net, false).is_err());
        assert!(enforce(&set(&[0]), &net, true).is_ok());
        assert!(matches!(
            enforce(&set(&[1]), &set(&[1]), true),
            Err(Error::SameDisk(_))
        ));
        assert!(matches!(
            verdict(&set(&[]), &set(&[1])),
            Verdict::Unknown { .. }
        ));
    }

    #[test]
    fn same_disk_on_this_machine_is_refused() {
        let sys = std::env::var("SYSTEMDRIVE").unwrap_or_else(|_| "C:".into()) + "\\";
        let source = disks_of_path(Path::new(&sys)).unwrap();
        assert!(!source.disks.is_empty(), "{source:?}");
        let tmp = tempfile::tempdir_in(&sys).unwrap();
        let dest = disks_of_path(tmp.path()).unwrap();
        assert!(matches!(verdict(&source, &dest), Verdict::SameDisk { .. }));
        // An image file on the system drive counts as that drive's disk.
        let img = tmp.path().join("image.vhdx");
        std::fs::write(&img, b"x").unwrap();
        assert_eq!(disks_of_path(&img).unwrap().disks, source.disks);
        // A junction on the system drive pointing elsewhere on it still resolves to it.
        assert!(disks_of_path(Path::new("Z:\\no\\such\\folder")).is_err());
    }
}
