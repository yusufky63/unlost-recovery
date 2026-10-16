//! Read-only partition discovery for disk images: MBR, GPT, or a bare volume image. Nothing here
//! ever writes a partition table; repairing one is a job for other tools.

use serde::{Deserialize, Serialize};

use crate::guid::Guid;
use crate::source::SectorSource;
use crate::{Error, Result, le};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FsKind {
    Ntfs,
    Fat,
    ExFat,
    Refs,
    BitLocker,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scheme {
    /// The image is a single volume, not a disk.
    Whole,
    Mbr,
    Gpt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Partition {
    /// 1-based position in the table (0 for a whole-volume image).
    pub index: u32,
    pub offset: u64,
    pub length: u64,
    pub scheme: Scheme,
    pub kind: String,
    pub name: String,
    pub fs: FsKind,
}

const MAX_GPT_ENTRIES: u32 = 1024;

const GPT_BASIC_DATA: Guid = Guid::from_fields(
    0xEBD0_A0A2,
    0xB9E5,
    0x4433,
    [0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7],
);
const GPT_MSR: Guid = Guid::from_fields(
    0xE3C9_E316,
    0x0B5C,
    0x4DC5,
    [0xA9, 0xF8, 0x2E, 0x3D, 0x1A, 0x9F, 0x0B, 0x71],
);
const GPT_EFI: Guid = Guid::from_fields(
    0xC12A_7328,
    0xF81F,
    0x11D2,
    [0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B],
);
const GPT_RECOVERY: Guid = Guid::from_fields(
    0xDE94_BBA4,
    0x06D1,
    0x4D40,
    [0xA1, 0x6A, 0xBF, 0xD5, 0x01, 0x79, 0xD6, 0xAC],
);

/// File system of the boot sector at `offset`.
pub fn probe_fs(src: &dyn SectorSource, offset: u64) -> FsKind {
    let mut b = [0u8; 512];
    if src.read_at(offset, &mut b).is_err() {
        return FsKind::Unknown;
    }
    probe_boot(&b)
}

pub fn probe_boot(b: &[u8; 512]) -> FsKind {
    match &b[3..11] {
        b"NTFS    " => return FsKind::Ntfs,
        b"EXFAT   " => return FsKind::ExFat,
        b"-FVE-FS-" => return FsKind::BitLocker,
        _ => {}
    }
    if &b[3..7] == b"ReFS" {
        return FsKind::Refs;
    }
    let signed = b[510] == 0x55 && b[511] == 0xAA;
    if signed && (&b[82..87] == b"FAT32" || &b[54..57] == b"FAT") {
        return FsKind::Fat;
    }
    FsKind::Unknown
}

/// Volumes inside an image. A bare volume image gives one entry with scheme `Whole`.
pub fn list(src: &dyn SectorSource) -> Result<Vec<Partition>> {
    let mut b = [0u8; 512];
    src.read_at(0, &mut b)?;
    let whole = probe_boot(&b);
    if whole != FsKind::Unknown {
        return Ok(vec![Partition {
            index: 0,
            offset: 0,
            length: src.len(),
            scheme: Scheme::Whole,
            kind: "volume".into(),
            name: String::new(),
            fs: whole,
        }]);
    }
    if b[510] != 0x55 || b[511] != 0xAA {
        return Err(Error::invalid(
            "partition table",
            "no MBR signature and no known file system",
        ));
    }
    let entries: Vec<(u8, u32, u32)> = (0..4)
        .map(|i| {
            let e = 446 + i * 16;
            (
                b[e + 4],
                le::u32(&b, e + 8).unwrap_or(0),
                le::u32(&b, e + 12).unwrap_or(0),
            )
        })
        .collect();
    if entries.iter().any(|e| e.0 == 0xEE) {
        for sector in [512u64, 4096] {
            if let Some(parts) = gpt(src, sector)? {
                return Ok(parts);
            }
        }
        return Err(Error::invalid("GPT", "protective MBR without a GPT header"));
    }
    let sector = u64::from(src.sector_size().max(512));
    let mut out = Vec::new();
    for (i, (kind, lba, sectors)) in entries.into_iter().enumerate() {
        if kind == 0 || kind == 0x05 || kind == 0x0F || sectors == 0 {
            continue;
        }
        let offset = u64::from(lba) * sector;
        let length = u64::from(sectors) * sector;
        if offset.checked_add(length).is_none_or(|e| e > src.len()) {
            continue;
        }
        out.push(Partition {
            index: i as u32 + 1,
            offset,
            length,
            scheme: Scheme::Mbr,
            kind: format!("0x{kind:02X}"),
            name: String::new(),
            fs: probe_fs(src, offset),
        });
    }
    Ok(out)
}

fn gpt(src: &dyn SectorSource, sector: u64) -> Result<Option<Vec<Partition>>> {
    let mut h = vec![0u8; sector as usize];
    if src.read_at(sector, &mut h).is_err() || h.get(0..8) != Some(b"EFI PART") {
        return Ok(None);
    }
    let invalid = |d: String| Error::invalid("GPT header", d);
    let lba = le::u64(&h, 72).ok_or_else(|| invalid("short".into()))?;
    let count = le::u32(&h, 80).ok_or_else(|| invalid("short".into()))?;
    let size = le::u32(&h, 84).ok_or_else(|| invalid("short".into()))?;
    if count > MAX_GPT_ENTRIES || !(128..=4096).contains(&size) || size % 8 != 0 {
        return Err(invalid(format!("{count} entries of {size} bytes")));
    }
    let table_off = lba
        .checked_mul(sector)
        .ok_or_else(|| invalid("table offset".into()))?;
    let table_len = u64::from(count) * u64::from(size);
    if table_off
        .checked_add(table_len)
        .is_none_or(|e| e > src.len())
    {
        return Err(invalid("entry table outside the image".into()));
    }
    let mut t = vec![0u8; table_len as usize];
    src.read_at(table_off, &mut t)?;
    let mut out = Vec::new();
    for i in 0..count as usize {
        let e = i * size as usize;
        let Some(kind) = Guid::read(&t, e) else {
            break;
        };
        if kind.is_zero() {
            continue;
        }
        let (Some(first), Some(last)) = (le::u64(&t, e + 32), le::u64(&t, e + 40)) else {
            continue;
        };
        if last < first {
            continue;
        }
        let (Some(offset), Some(length)) = (
            first.checked_mul(sector),
            (last - first + 1).checked_mul(sector),
        ) else {
            continue;
        };
        if offset.checked_add(length).is_none_or(|end| end > src.len()) {
            continue;
        }
        let kind_name = match kind {
            k if k == GPT_BASIC_DATA => "basic data".to_string(),
            k if k == GPT_MSR => "Microsoft reserved".to_string(),
            k if k == GPT_EFI => "EFI system".to_string(),
            k if k == GPT_RECOVERY => "recovery".to_string(),
            k => k.to_string(),
        };
        let fs = if kind == GPT_MSR {
            FsKind::Unknown
        } else {
            probe_fs(src, offset)
        };
        out.push(Partition {
            index: i as u32 + 1,
            offset,
            length,
            scheme: Scheme::Gpt,
            kind: kind_name,
            name: le::utf16(&t, e + 56, 36).unwrap_or_default(),
            fs,
        });
    }
    Ok(Some(out))
}

/// A one-partition GPT disk around `volume`, for tests.
#[cfg(any(test, feature = "test-util"))]
pub fn build_test_gpt(volume: &[u8]) -> Vec<u8> {
    let first_lba = 2048u64;
    let vol_sectors = (volume.len() as u64).div_ceil(512);
    let total = first_lba + vol_sectors + 34;
    let mut d = vec![0u8; (total * 512) as usize];
    // Protective MBR.
    d[446 + 4] = 0xEE;
    d[446 + 8..446 + 12].copy_from_slice(&1u32.to_le_bytes());
    d[446 + 12..446 + 16].copy_from_slice(&((total - 1) as u32).to_le_bytes());
    d[510] = 0x55;
    d[511] = 0xAA;
    let h = 512;
    d[h..h + 8].copy_from_slice(b"EFI PART");
    d[h + 72..h + 80].copy_from_slice(&2u64.to_le_bytes());
    d[h + 80..h + 84].copy_from_slice(&128u32.to_le_bytes());
    d[h + 84..h + 88].copy_from_slice(&128u32.to_le_bytes());
    let e = 1024;
    d[e..e + 16].copy_from_slice(&GPT_BASIC_DATA.0);
    d[e + 16] = 1;
    d[e + 32..e + 40].copy_from_slice(&first_lba.to_le_bytes());
    d[e + 40..e + 48].copy_from_slice(&(first_lba + vol_sectors - 1).to_le_bytes());
    for (i, u) in "Basic data partition".encode_utf16().enumerate() {
        d[e + 56 + i * 2..e + 58 + i * 2].copy_from_slice(&u.to_le_bytes());
    }
    let start = (first_lba * 512) as usize;
    d[start..start + volume.len()].copy_from_slice(volume);
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemSource;

    fn ntfs_boot() -> Vec<u8> {
        let mut v = vec![0u8; 4096];
        v[3..11].copy_from_slice(b"NTFS    ");
        v[510] = 0x55;
        v[511] = 0xAA;
        v
    }

    #[test]
    fn whole_volume_and_gpt() {
        let vol = ntfs_boot();
        let parts = list(&MemSource::new(vol.clone())).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].scheme, Scheme::Whole);
        assert_eq!(parts[0].fs, FsKind::Ntfs);

        let disk = build_test_gpt(&vol);
        let parts = list(&MemSource::new(disk)).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].scheme, Scheme::Gpt);
        assert_eq!(parts[0].offset, 2048 * 512);
        assert_eq!(parts[0].fs, FsKind::Ntfs);
        assert_eq!(parts[0].kind, "basic data");
        assert_eq!(parts[0].name, "Basic data partition");
    }

    #[test]
    fn mbr_and_garbage() {
        let mut d = vec![0u8; 64 * 1024];
        d[446 + 4] = 0x07;
        d[446 + 8..446 + 12].copy_from_slice(&64u32.to_le_bytes());
        d[446 + 12..446 + 16].copy_from_slice(&8u32.to_le_bytes());
        d[510] = 0x55;
        d[511] = 0xAA;
        d[64 * 512 + 3..64 * 512 + 11].copy_from_slice(b"NTFS    ");
        // An entry pointing past the end is skipped.
        d[462 + 4] = 0x07;
        d[462 + 8..462 + 12].copy_from_slice(&u32::MAX.to_le_bytes());
        d[462 + 12..462 + 16].copy_from_slice(&8u32.to_le_bytes());
        let parts = list(&MemSource::new(d)).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].fs, FsKind::Ntfs);
        assert!(list(&MemSource::new(vec![0u8; 1024])).is_err());
    }
}
