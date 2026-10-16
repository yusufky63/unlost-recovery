//! Drives, the physical disks behind them, and the facts that decide how much can be recovered:
//! HDD or SSD, TRIM, removable media, system drive. All of it works without administrator
//! rights: property IOCTLs only need a handle opened with no access.

use std::collections::BTreeSet;
use std::os::windows::io::AsRawHandle;

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE, WIN32_ERROR};
use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
    GetVolumeNameForVolumeMountPointW, GetVolumePathNamesForVolumeNameW,
    IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
};
use windows::Win32::Storage::Vhd::{
    GET_STORAGE_DEPENDENCY_FLAG_HOST_VOLUMES, GetStorageDependencyInformation,
    STORAGE_DEPENDENCY_INFO, STORAGE_DEPENDENCY_INFO_VERSION_2,
};
use windows::Win32::System::Ioctl::IOCTL_STORAGE_QUERY_PROPERTY;
use windows::core::PCWSTR;
use winreg::RegKey;
use winreg::enums::HKEY_LOCAL_MACHINE;

use crate::device::{open_query, query_ioctl};
use crate::winstr::{from_wide, multi, wide};
use crate::{Error, Result};

const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;

const PROP_DEVICE: u32 = 0;
const PROP_SEEK_PENALTY: u32 = 7;
const PROP_TRIM: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Bus {
    Sata,
    Nvme,
    Usb,
    Sd,
    Scsi,
    Raid,
    Virtual,
    Spaces,
    Other,
}

impl Bus {
    fn from_raw(v: u32) -> Self {
        match v {
            3 | 11 => Bus::Sata,
            17 => Bus::Nvme,
            7 => Bus::Usb,
            12 | 13 => Bus::Sd,
            1 | 10 => Bus::Scsi,
            8 => Bus::Raid,
            14 | 15 => Bus::Virtual,
            16 => Bus::Spaces,
            _ => Bus::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Media {
    Hdd,
    Ssd,
    UsbStick,
    SdCard,
    ExternalDisk,
    Virtual,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Trim {
    On,
    Off,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskInfo {
    pub number: u32,
    pub bus: Bus,
    pub removable: bool,
    /// `Some(false)` means solid state.
    pub seek_penalty: Option<bool>,
    pub trim_supported: Option<bool>,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveInfo {
    /// `D:\`
    pub root: String,
    pub label: String,
    pub file_system: String,
    pub total: u64,
    pub free: u64,
    /// `\\?\Volume{...}\`
    pub volume: String,
    pub disks: Vec<u32>,
    pub system: bool,
    pub removable: bool,
    /// False when Windows cannot read the file system (BitLocker locked, damaged, no media).
    pub readable: bool,
    pub media: Media,
    pub trim: Trim,
    pub model: String,
}

pub fn disk_info(number: u32) -> Result<DiskInfo> {
    let path = format!(r"\\.\PhysicalDrive{number}");
    let dev = query_property(&path, PROP_DEVICE, 1024)?;
    let u32_at = |b: &[u8], o: usize| {
        b.get(o..o + 4)
            .map(|s| u32::from_le_bytes(s.try_into().unwrap_or_default()))
    };
    let removable = dev.get(10).is_some_and(|&b| b != 0);
    let bus = Bus::from_raw(u32_at(&dev, 28).unwrap_or(0));
    let text = |off: Option<u32>| -> String {
        let Some(o) = off.filter(|&o| o != 0) else {
            return String::new();
        };
        let s = dev.get(o as usize..).unwrap_or(&[]);
        let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
        String::from_utf8_lossy(&s[..end]).trim().to_string()
    };
    let vendor = text(u32_at(&dev, 12));
    let product = text(u32_at(&dev, 16));
    let model = [vendor, product]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let flag = |id: u32| -> Option<bool> {
        query_property(&path, id, 12)
            .ok()
            .and_then(|b| b.get(8).copied())
            .map(|v| v != 0)
    };
    Ok(DiskInfo {
        number,
        bus,
        removable,
        seek_penalty: flag(PROP_SEEK_PENALTY),
        trim_supported: flag(PROP_TRIM),
        model,
    })
}

fn query_property(path: &str, id: u32, out: usize) -> Result<Vec<u8>> {
    let mut q = [0u8; 12];
    q[0..4].copy_from_slice(&id.to_le_bytes());
    query_ioctl(path, IOCTL_STORAGE_QUERY_PROPERTY, &q, out)
}

/// Physical disk numbers under a volume device (`\\.\D:` or `\\.\Volume{...}`).
pub fn volume_disks(device: &str) -> Result<Vec<u32>> {
    let b = query_ioctl(
        device,
        IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
        &[],
        8 + 24 * 64,
    )?;
    let n = b
        .get(0..4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap_or_default()))
        .unwrap_or(0) as usize;
    let mut out = BTreeSet::new();
    for i in 0..n.min(64) {
        let o = 8 + i * 24;
        if let Some(s) = b.get(o..o + 4) {
            out.insert(u32::from_le_bytes(s.try_into().unwrap_or_default()));
        }
    }
    if out.is_empty() {
        return Err(Error::Other(format!("{device}: no disk extents")));
    }
    Ok(out.into_iter().collect())
}

/// `\\?\Volume{...}\` -> `\\.\Volume{...}` for device IOCTLs.
pub fn volume_device(volume_guid_path: &str) -> String {
    let trimmed = volume_guid_path.trim_end_matches('\\');
    match trimmed.strip_prefix(r"\\?\") {
        Some(rest) => format!(r"\\.\{rest}"),
        None => trimmed.to_string(),
    }
}

pub fn volume_guid(root: &str) -> Option<String> {
    let mut buf = vec![0u16; 64];
    let w_root = wide(root);
    unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(w_root.as_ptr()), &mut buf) }.ok()?;
    Some(from_wide(&buf))
}

/// Drive letters (`D:\`) where a volume is mounted.
pub fn volume_roots(volume_guid_path: &str) -> Vec<String> {
    let w = wide(volume_guid_path);
    let mut buf = vec![0u16; 1024];
    let mut len = 0u32;
    if unsafe { GetVolumePathNamesForVolumeNameW(PCWSTR(w.as_ptr()), Some(&mut buf), &mut len) }
        .is_err()
    {
        return Vec::new();
    }
    multi(&buf)
}

/// Host disks of a virtual disk (VHD/VHDX) volume: the physical disks holding the image file.
/// `None` when Windows cannot say, which the guard treats as "unknown disk".
pub fn virtual_host_disks(volume_dev: &str, depth: u32) -> Option<Vec<u32>> {
    if depth > 4 {
        return None;
    }
    let f = open_query(volume_dev).ok()?;
    let h = HANDLE(f.as_raw_handle());
    let mut size = 4096u32;
    for _ in 0..3 {
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let info = buf.as_mut_ptr() as *mut STORAGE_DEPENDENCY_INFO;
        unsafe { (*info).Version = STORAGE_DEPENDENCY_INFO_VERSION_2 };
        let mut used = 0u32;
        let rc: WIN32_ERROR = unsafe {
            GetStorageDependencyInformation(
                h,
                GET_STORAGE_DEPENDENCY_FLAG_HOST_VOLUMES,
                size,
                info,
                Some(&mut used),
            )
        };
        if rc == ERROR_INSUFFICIENT_BUFFER {
            size = used.max(size * 2);
            continue;
        }
        if rc.0 != 0 {
            return None;
        }
        let n = unsafe { (*info).NumberEntries } as usize;
        let entries = unsafe {
            std::slice::from_raw_parts((*info).Anonymous.Version2Entries.as_ptr(), n.min(16))
        };
        let mut out = BTreeSet::new();
        for e in entries {
            let host = unsafe { e.HostVolumeName.to_string() }.ok()?;
            if host.is_empty() {
                return None;
            }
            let dev = volume_device(&host);
            for d in volume_disks(&dev).ok()? {
                out.insert(d);
                if disk_info(d).is_ok_and(|i| i.bus == Bus::Virtual) {
                    out.extend(virtual_host_disks(&dev, depth + 1)?);
                }
            }
        }
        return (!out.is_empty()).then(|| out.into_iter().collect());
    }
    None
}

/// Phones and cameras connected as media devices (MTP/PTP). They have no drive letter and no
/// sectors to read: Windows only exposes their files, so they cannot be scanned for deleted
/// files. Listed so the app can say that instead of silently not showing them.
pub fn media_devices() -> Vec<String> {
    let Ok(rows) = crate::wmi::query(
        "SELECT Name, DeviceID FROM Win32_PnPEntity WHERE PNPClass = 'WPD'",
        &["Name", "DeviceID"],
    ) else {
        return Vec::new();
    };
    // Storage volumes also appear as portable devices, under the software device bus; phones
    // and cameras sit directly on USB.
    let mut out: Vec<String> = rows
        .into_iter()
        .filter(|r| {
            r.get("DeviceID")
                .is_some_and(|id| id.to_ascii_uppercase().starts_with("USB\\"))
        })
        .filter_map(|r| r.get("Name").cloned())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// NTFS sends TRIM unless `DisableDeleteNotification` is 1.
pub fn ntfs_trim_enabled() -> Option<bool> {
    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SYSTEM\CurrentControlSet\Control\FileSystem")
        .ok()?;
    Some(
        key.get_value::<u32, _>("DisableDeleteNotification")
            .unwrap_or(0)
            == 0,
    )
}

pub fn media_of(disk: &DiskInfo, removable_drive: bool) -> Media {
    match disk.bus {
        Bus::Virtual => Media::Virtual,
        Bus::Sd => Media::SdCard,
        Bus::Usb if disk.removable || removable_drive => Media::UsbStick,
        Bus::Usb => Media::ExternalDisk,
        _ => match disk.seek_penalty {
            Some(true) => Media::Hdd,
            Some(false) => Media::Ssd,
            None => Media::Unknown,
        },
    }
}

pub fn trim_state(media: Media, disk: Option<&DiskInfo>, fs: &str, os_trim: Option<bool>) -> Trim {
    let Some(disk) = disk else {
        return Trim::Unknown;
    };
    if matches!(media, Media::Hdd | Media::UsbStick | Media::SdCard) {
        return Trim::Off;
    }
    let os = if fs.eq_ignore_ascii_case("NTFS") {
        os_trim
    } else {
        None
    };
    match (disk.trim_supported, os) {
        (Some(false), _) | (_, Some(false)) => Trim::Off,
        (Some(true), Some(true)) => Trim::On,
        (Some(true), None) if !fs.eq_ignore_ascii_case("NTFS") => Trim::On,
        _ => Trim::Unknown,
    }
}

pub fn drives() -> Vec<DriveInfo> {
    let mut buf = vec![0u16; 512];
    let len = unsafe { GetLogicalDriveStringsW(Some(&mut buf)) } as usize;
    let sysdrive = std::env::var("SYSTEMDRIVE")
        .unwrap_or_else(|_| "C:".into())
        .to_uppercase();
    let os_trim = ntfs_trim_enabled();
    let mut out = Vec::new();
    for root in buf[..len.min(buf.len())]
        .split(|&c| c == 0)
        .filter(|s| !s.is_empty())
    {
        let root = String::from_utf16_lossy(root);
        let w_root = wide(&root);
        let kind = unsafe { GetDriveTypeW(PCWSTR(w_root.as_ptr())) };
        if kind != DRIVE_FIXED && kind != DRIVE_REMOVABLE {
            continue;
        }
        let mut label = vec![0u16; 261];
        let mut fs = vec![0u16; 64];
        let readable = unsafe {
            GetVolumeInformationW(
                PCWSTR(w_root.as_ptr()),
                Some(&mut label),
                None,
                None,
                None,
                Some(&mut fs),
            )
        }
        .is_ok();
        let (mut free, mut total) = (0u64, 0u64);
        if readable {
            let _ = unsafe {
                GetDiskFreeSpaceExW(
                    PCWSTR(w_root.as_ptr()),
                    Some(&mut free),
                    Some(&mut total),
                    None,
                )
            };
        }
        let letter = root.trim_end_matches('\\');
        let disks = volume_disks(&format!(r"\\.\{letter}")).unwrap_or_default();
        let info = disks.first().and_then(|&d| disk_info(d).ok());
        let removable = kind == DRIVE_REMOVABLE;
        let media = info
            .as_ref()
            .map_or(Media::Unknown, |d| media_of(d, removable));
        let fs = from_wide(&fs);
        out.push(DriveInfo {
            system: root.to_uppercase().starts_with(&sysdrive),
            volume: volume_guid(&root).unwrap_or_default(),
            label: from_wide(&label),
            trim: trim_state(media, info.as_ref(), &fs, os_trim),
            file_system: fs,
            total,
            free,
            disks,
            removable,
            readable,
            media,
            model: info.map(|d| d.model).unwrap_or_default(),
            root,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(bus: Bus, seek: Option<bool>, trim: Option<bool>, removable: bool) -> DiskInfo {
        DiskInfo {
            number: 0,
            bus,
            removable,
            seek_penalty: seek,
            trim_supported: trim,
            model: String::new(),
        }
    }

    #[test]
    fn classifies_media_and_trim() {
        let nvme = disk(Bus::Nvme, Some(false), Some(true), false);
        assert_eq!(media_of(&nvme, false), Media::Ssd);
        assert_eq!(
            trim_state(Media::Ssd, Some(&nvme), "NTFS", Some(true)),
            Trim::On
        );
        assert_eq!(
            trim_state(Media::Ssd, Some(&nvme), "NTFS", Some(false)),
            Trim::Off
        );
        let hdd = disk(Bus::Sata, Some(true), Some(false), false);
        assert_eq!(media_of(&hdd, false), Media::Hdd);
        assert_eq!(
            trim_state(Media::Hdd, Some(&hdd), "NTFS", Some(true)),
            Trim::Off
        );
        let stick = disk(Bus::Usb, None, None, true);
        assert_eq!(media_of(&stick, true), Media::UsbStick);
        assert_eq!(
            media_of(&disk(Bus::Usb, None, None, false), false),
            Media::ExternalDisk
        );
        assert_eq!(
            media_of(&disk(Bus::Sd, None, None, true), true),
            Media::SdCard
        );
        assert_eq!(
            trim_state(Media::Unknown, None, "NTFS", Some(true)),
            Trim::Unknown
        );
    }

    #[test]
    fn device_paths() {
        assert_eq!(volume_device(r"\\?\Volume{abc}\"), r"\\.\Volume{abc}");
        assert_eq!(volume_device(r"\\.\C:"), r"\\.\C:");
    }

    #[test]
    fn this_machine_has_a_system_drive() {
        let all = drives();
        let sys = all.iter().find(|d| d.system).expect("system drive");
        assert!(sys.readable);
        assert!(!sys.disks.is_empty(), "disk extents work without admin");
        assert!(sys.volume.starts_with(r"\\?\Volume{"));
        assert!(
            volume_roots(&sys.volume)
                .iter()
                .any(|r| r.eq_ignore_ascii_case(&sys.root))
        );
        let info = disk_info(sys.disks[0]).unwrap();
        assert_ne!(info.bus, Bus::Other, "{info:?}");
    }
}
