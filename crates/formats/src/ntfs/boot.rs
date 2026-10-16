use serde::{Deserialize, Serialize};

use crate::{Error, Result, le};

/// The NTFS boot sector fields the scanner needs, validated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootSector {
    pub bytes_per_sector: u32,
    pub cluster_size: u32,
    pub total_clusters: u64,
    pub mft_lcn: u64,
    pub record_size: u32,
    pub serial: u64,
}

/// Largest NTFS cluster Windows supports (2 MiB since Windows 10 1709).
const MAX_CLUSTER: u32 = 2 * 1024 * 1024;

impl BootSector {
    pub fn parse(b: &[u8]) -> Result<Self> {
        let bad = |d: String| Error::invalid("NTFS boot sector", d);
        if b.get(3..11) != Some(b"NTFS    ") {
            return Err(bad("no NTFS signature".into()));
        }
        let bps = u32::from(le::u16(b, 0x0B).unwrap_or(0));
        if !matches!(bps, 256 | 512 | 1024 | 2048 | 4096) {
            return Err(bad(format!("{bps} bytes per sector")));
        }
        let raw_spc = le::u8(b, 0x0D).unwrap_or(0);
        let spc: u32 = match raw_spc {
            0 => return Err(bad("0 sectors per cluster".into())),
            1..=0x80 => u32::from(raw_spc),
            // Large clusters store a negative power of two.
            _ => {
                let shift = 256 - u32::from(raw_spc);
                if shift > 20 {
                    return Err(bad(format!("sectors per cluster 2^{shift}")));
                }
                1 << shift
            }
        };
        if !spc.is_power_of_two() {
            return Err(bad(format!("{spc} sectors per cluster")));
        }
        let cluster_size = bps * spc;
        if cluster_size > MAX_CLUSTER {
            return Err(bad(format!("{cluster_size}-byte clusters")));
        }
        let total_sectors = le::u64(b, 0x28).unwrap_or(0);
        let total_clusters = total_sectors / u64::from(spc);
        if total_clusters == 0 {
            return Err(bad("empty volume".into()));
        }
        let mft_lcn = le::u64(b, 0x30).unwrap_or(u64::MAX);
        if mft_lcn >= total_clusters {
            return Err(bad(format!("$MFT at cluster {mft_lcn}")));
        }
        let raw_rec = le::u8(b, 0x40).unwrap_or(0) as i8;
        let record_size: u32 = if raw_rec > 0 {
            u32::from(raw_rec as u8).saturating_mul(cluster_size)
        } else {
            let shift = u32::from(raw_rec.unsigned_abs());
            if shift > 16 {
                return Err(bad(format!("record size 2^{shift}")));
            }
            1 << shift
        };
        if !(256..=65536).contains(&record_size) || !record_size.is_power_of_two() {
            return Err(bad(format!("{record_size}-byte file records")));
        }
        Ok(BootSector {
            bytes_per_sector: bps,
            cluster_size,
            total_clusters,
            mft_lcn,
            record_size,
            serial: le::u64(b, 0x48).unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boot(spc: u8, rec: u8) -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[3..11].copy_from_slice(b"NTFS    ");
        b[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes());
        b[0x0D] = spc;
        b[0x28..0x30].copy_from_slice(&1_000_000u64.to_le_bytes());
        b[0x30..0x38].copy_from_slice(&4u64.to_le_bytes());
        b[0x40] = rec;
        b
    }

    #[test]
    fn parses_common_layouts() {
        let b = BootSector::parse(&boot(8, 0xF6)).unwrap();
        assert_eq!(b.cluster_size, 4096);
        assert_eq!(b.record_size, 1024);
        assert_eq!(b.total_clusters, 125_000);
        // 4Kn-style record size given in clusters.
        let b = BootSector::parse(&boot(8, 1)).unwrap();
        assert_eq!(b.record_size, 4096);
        // 64 KiB clusters as 2^7 sectors written as 0x80, and 2 MiB as a negative exponent.
        assert_eq!(
            BootSector::parse(&boot(0x80, 0xF6)).unwrap().cluster_size,
            65536
        );
        assert_eq!(
            BootSector::parse(&boot(0xF4, 0xF6)).unwrap().cluster_size,
            2 * 1024 * 1024
        );
    }

    #[test]
    fn rejects_nonsense() {
        assert!(BootSector::parse(&[]).is_err());
        assert!(BootSector::parse(&boot(0, 0xF6)).is_err());
        assert!(BootSector::parse(&boot(3, 0xF6)).is_err());
        assert!(BootSector::parse(&boot(0xE0, 0xF6)).is_err());
        assert!(BootSector::parse(&boot(8, 0xE0)).is_err());
        let mut b = boot(8, 0xF6);
        b[0x30..0x38].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(BootSector::parse(&b).is_err());
    }
}
