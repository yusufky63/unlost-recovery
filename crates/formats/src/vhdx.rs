//! Read-only VHDX reader (Microsoft's open MS-VHDX specification), so scenario images and user
//! images can be scanned without attaching them to Windows.
//!
//! Supported: fixed and dynamic VHDX. Refused: differencing disks (they need a parent chain)
//! and images with an unapplied log, which only happens when the file was not closed cleanly;
//! replaying the log would mean writing it.

use crate::guid::Guid;
use crate::source::{SectorSource, out_of_range};
use crate::{Error, Result, crc32c, le};

const MIB: u64 = 1024 * 1024;
const HEADER_OFFSETS: [u64; 2] = [64 * 1024, 128 * 1024];
const REGION_OFFSETS: [u64; 2] = [192 * 1024, 256 * 1024];
const HEADER_SIZE: usize = 4096;
const REGION_SIZE: usize = 64 * 1024;
/// A BAT larger than this would describe a disk far beyond the 64 TiB VHDX limit.
const MAX_BAT_BYTES: u64 = 512 * MIB;
const MAX_METADATA_BYTES: u64 = 16 * MIB;

pub const BAT_REGION: Guid = Guid::from_fields(
    0x2DC2_7766,
    0xF623,
    0x4200,
    [0x9D, 0x64, 0x11, 0x5E, 0x9B, 0xFD, 0x4A, 0x08],
);
pub const METADATA_REGION: Guid = Guid::from_fields(
    0x8B7C_A206,
    0x4790,
    0x4B9A,
    [0xB8, 0xFE, 0x57, 0x5F, 0x05, 0x0F, 0x88, 0x6E],
);
pub const FILE_PARAMETERS: Guid = Guid::from_fields(
    0xCAA1_6737,
    0xFA36,
    0x4D43,
    [0xB3, 0xB6, 0x33, 0xF0, 0xAA, 0x44, 0xE7, 0x6B],
);
pub const VIRTUAL_DISK_SIZE: Guid = Guid::from_fields(
    0x2FA5_4224,
    0xCD1B,
    0x4876,
    [0xB2, 0x11, 0x5D, 0xBE, 0xD8, 0x3B, 0xF4, 0xB8],
);
pub const LOGICAL_SECTOR_SIZE: Guid = Guid::from_fields(
    0x8141_BF1D,
    0xA96F,
    0x4709,
    [0xBA, 0x47, 0xF2, 0x33, 0xA8, 0xFA, 0xAB, 0x5F],
);

const PAYLOAD_NOT_PRESENT: u64 = 0;
const PAYLOAD_UNDEFINED: u64 = 1;
const PAYLOAD_ZERO: u64 = 2;
const PAYLOAD_UNMAPPED: u64 = 3;
const PAYLOAD_FULLY_PRESENT: u64 = 6;

pub fn is_vhdx(src: &dyn SectorSource) -> bool {
    let mut sig = [0u8; 8];
    src.len() >= 8 && src.read_at(0, &mut sig).is_ok() && &sig == b"vhdxfile"
}

pub struct Vhdx<S> {
    file: S,
    block_size: u64,
    disk_size: u64,
    logical_sector: u32,
    chunk_ratio: u64,
    bat: Vec<u64>,
}

impl<S: SectorSource> Vhdx<S> {
    pub fn open(file: S) -> Result<Self> {
        if !is_vhdx(&file) {
            return Err(Error::invalid("VHDX", "missing the vhdxfile signature"));
        }
        let header = best_header(&file)?;
        if !header.log_guid.is_zero() {
            return Err(Error::Unsupported(
                "this VHDX was not closed cleanly and has an unapplied log; attach and detach it in Windows once, then try again".into(),
            ));
        }
        let regions = region_table(&file)?;
        let bat_region = regions
            .iter()
            .find(|r| r.guid == BAT_REGION)
            .ok_or_else(|| Error::invalid("VHDX", "no BAT region"))?;
        let meta_region = regions
            .iter()
            .find(|r| r.guid == METADATA_REGION)
            .ok_or_else(|| Error::invalid("VHDX", "no metadata region"))?;
        if let Some(r) = regions
            .iter()
            .find(|r| r.required && r.guid != BAT_REGION && r.guid != METADATA_REGION)
        {
            return Err(Error::Unsupported(format!(
                "required VHDX region {}",
                r.guid
            )));
        }

        let meta = metadata(&file, meta_region)?;
        let block_size = u64::from(meta.block_size);
        if !(MIB..=256 * MIB).contains(&block_size) || !block_size.is_power_of_two() {
            return Err(Error::invalid("VHDX", format!("block size {block_size}")));
        }
        if meta.has_parent {
            return Err(Error::Unsupported("differencing VHDX images".into()));
        }
        if meta.logical_sector != 512 && meta.logical_sector != 4096 {
            return Err(Error::invalid(
                "VHDX",
                format!("logical sector size {}", meta.logical_sector),
            ));
        }
        if meta.disk_size == 0
            || meta.disk_size > 64 * 1024 * 1024 * MIB
            || meta.disk_size % u64::from(meta.logical_sector) != 0
        {
            return Err(Error::invalid(
                "VHDX",
                format!("disk size {}", meta.disk_size),
            ));
        }
        let chunk_ratio = ((1u64 << 23) * u64::from(meta.logical_sector)) / block_size;
        let data_blocks = meta.disk_size.div_ceil(block_size);
        let needed = data_blocks + (data_blocks - 1) / chunk_ratio;
        let bat_len = u64::from(bat_region.length);
        if bat_len > MAX_BAT_BYTES || bat_len / 8 < needed {
            return Err(Error::invalid(
                "VHDX",
                format!("BAT of {bat_len} bytes for {needed} entries"),
            ));
        }
        let mut raw = vec![0u8; (needed * 8) as usize];
        file.read_at(bat_region.offset, &mut raw)?;
        let bat = raw
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_le_bytes(*c))
            .collect();
        Ok(Vhdx {
            file,
            block_size,
            disk_size: meta.disk_size,
            logical_sector: meta.logical_sector,
            chunk_ratio,
            bat,
        })
    }

    pub fn logical_sector(&self) -> u32 {
        self.logical_sector
    }
}

impl<S: SectorSource> SectorSource for Vhdx<S> {
    fn len(&self) -> u64 {
        self.disk_size
    }

    fn sector_size(&self) -> u32 {
        self.logical_sector
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset
            .checked_add(buf.len() as u64)
            .is_none_or(|e| e > self.disk_size)
        {
            return Err(out_of_range(offset, buf.len(), self.disk_size));
        }
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset + done as u64;
            let block = pos / self.block_size;
            let within = pos % self.block_size;
            let n = ((self.block_size - within) as usize).min(buf.len() - done);
            let idx = block + block / self.chunk_ratio;
            let entry = *self
                .bat
                .get(idx as usize)
                .ok_or_else(|| Error::invalid("VHDX", format!("BAT index {idx}")))?;
            let part = &mut buf[done..done + n];
            match entry & 7 {
                PAYLOAD_FULLY_PRESENT => {
                    let file_off = (entry >> 20)
                        .checked_mul(MIB)
                        .and_then(|o| o.checked_add(within))
                        .ok_or_else(|| Error::invalid("VHDX", "block offset overflow"))?;
                    self.file.read_at(file_off, part)?;
                }
                PAYLOAD_NOT_PRESENT | PAYLOAD_UNDEFINED | PAYLOAD_ZERO | PAYLOAD_UNMAPPED => {
                    part.fill(0)
                }
                s => {
                    return Err(Error::Unsupported(format!(
                        "VHDX block state {s} (differencing data)"
                    )));
                }
            }
            done += n;
        }
        Ok(())
    }
}

struct Header {
    sequence: u64,
    log_guid: Guid,
}

fn best_header(file: &dyn SectorSource) -> Result<Header> {
    let mut best: Option<Header> = None;
    for off in HEADER_OFFSETS {
        let mut b = vec![0u8; HEADER_SIZE];
        if file.read_at(off, &mut b).is_err() || &b[0..4] != b"head" {
            continue;
        }
        let stored = le::u32(&b, 4).unwrap_or(0);
        b[4..8].fill(0);
        if crc32c::checksum(&b) != stored || le::u16(&b, 66) != Some(1) {
            continue;
        }
        let h = Header {
            sequence: le::u64(&b, 8).unwrap_or(0),
            log_guid: Guid::read(&b, 48).unwrap_or_default(),
        };
        if best.as_ref().is_none_or(|x| h.sequence > x.sequence) {
            best = Some(h);
        }
    }
    best.ok_or_else(|| Error::invalid("VHDX", "no valid header"))
}

struct Region {
    guid: Guid,
    offset: u64,
    length: u32,
    required: bool,
}

fn region_table(file: &dyn SectorSource) -> Result<Vec<Region>> {
    for off in REGION_OFFSETS {
        let mut b = vec![0u8; REGION_SIZE];
        if file.read_at(off, &mut b).is_err() || &b[0..4] != b"regi" {
            continue;
        }
        let stored = le::u32(&b, 4).unwrap_or(0);
        b[4..8].fill(0);
        if crc32c::checksum(&b) != stored {
            continue;
        }
        let count = le::u32(&b, 8).unwrap_or(0);
        if count > 2047 {
            continue;
        }
        let mut out = Vec::new();
        for i in 0..count as usize {
            let e = 16 + i * 32;
            let (Some(guid), Some(offset), Some(length), Some(flags)) = (
                Guid::read(&b, e),
                le::u64(&b, e + 16),
                le::u32(&b, e + 24),
                le::u32(&b, e + 28),
            ) else {
                break;
            };
            if offset < MIB || offset % MIB != 0 || u64::from(length) % MIB != 0 {
                return Err(Error::invalid(
                    "VHDX",
                    format!("region {guid} at {offset} ({length} bytes)"),
                ));
            }
            out.push(Region {
                guid,
                offset,
                length,
                required: flags & 1 != 0,
            });
        }
        return Ok(out);
    }
    Err(Error::invalid("VHDX", "no valid region table"))
}

struct Meta {
    block_size: u32,
    has_parent: bool,
    disk_size: u64,
    logical_sector: u32,
}

fn metadata(file: &dyn SectorSource, region: &Region) -> Result<Meta> {
    let len = u64::from(region.length).min(MAX_METADATA_BYTES);
    let mut b = vec![0u8; len as usize];
    file.read_at(region.offset, &mut b)?;
    if b.get(0..8) != Some(b"metadata") {
        return Err(Error::invalid("VHDX", "metadata table signature"));
    }
    let count = le::u16(&b, 10).unwrap_or(0).min(2047) as usize;
    let item = |id: Guid, size: usize| -> Option<&[u8]> {
        (0..count).find_map(|i| {
            let e = 32 + i * 32;
            if Guid::read(&b, e)? != id {
                return None;
            }
            let off = le::u32(&b, e + 16)? as usize;
            let l = le::u32(&b, e + 20)? as usize;
            if l < size {
                return None;
            }
            b.get(off..off.checked_add(size)?)
        })
    };
    let params =
        item(FILE_PARAMETERS, 8).ok_or_else(|| Error::invalid("VHDX", "no file parameters"))?;
    let size = item(VIRTUAL_DISK_SIZE, 8).ok_or_else(|| Error::invalid("VHDX", "no disk size"))?;
    let sector = item(LOGICAL_SECTOR_SIZE, 4)
        .ok_or_else(|| Error::invalid("VHDX", "no logical sector size"))?;
    Ok(Meta {
        block_size: le::u32(params, 0).unwrap_or(0),
        has_parent: le::u32(params, 4).unwrap_or(0) & 2 != 0,
        disk_size: le::u64(size, 0).unwrap_or(0),
        logical_sector: le::u32(sector, 0).unwrap_or(0),
    })
}

/// Builds a dynamic VHDX around `disk`, block size 1 MiB, for tests. All-zero blocks are left
/// unallocated so the reader's sparse path is exercised.
#[cfg(any(test, feature = "test-util"))]
pub fn build_test_vhdx(disk: &[u8], logical_sector: u32) -> Vec<u8> {
    let block = MIB as usize;
    let disk_size = disk.len().div_ceil(logical_sector as usize) * logical_sector as usize;
    let blocks = disk_size.div_ceil(block);
    let chunk_ratio = ((1usize << 23) * logical_sector as usize) / block;
    let bat_entries = blocks + (blocks - 1) / chunk_ratio;
    let meta_off = MIB as usize;
    let bat_off = 2 * MIB as usize;
    let bat_len = (bat_entries * 8).div_ceil(block) * block;
    let mut payload_at = bat_off + bat_len;
    let mut out = vec![0u8; payload_at];
    out[0..8].copy_from_slice(b"vhdxfile");

    for (i, off) in HEADER_OFFSETS.iter().enumerate() {
        let mut h = vec![0u8; HEADER_SIZE];
        h[0..4].copy_from_slice(b"head");
        h[8..16].copy_from_slice(&(i as u64 + 1).to_le_bytes());
        h[66..68].copy_from_slice(&1u16.to_le_bytes());
        let c = crc32c::checksum(&h);
        h[4..8].copy_from_slice(&c.to_le_bytes());
        out[*off as usize..*off as usize + HEADER_SIZE].copy_from_slice(&h);
    }
    let mut r = vec![0u8; REGION_SIZE];
    r[0..4].copy_from_slice(b"regi");
    r[8..12].copy_from_slice(&2u32.to_le_bytes());
    for (i, (g, off, len)) in [
        (BAT_REGION, bat_off, bat_len),
        (METADATA_REGION, meta_off, block),
    ]
    .iter()
    .enumerate()
    {
        let e = 16 + i * 32;
        r[e..e + 16].copy_from_slice(&g.0);
        r[e + 16..e + 24].copy_from_slice(&(*off as u64).to_le_bytes());
        r[e + 24..e + 28].copy_from_slice(&(*len as u32).to_le_bytes());
        r[e + 28..e + 32].copy_from_slice(&1u32.to_le_bytes());
    }
    let c = crc32c::checksum(&r);
    r[4..8].copy_from_slice(&c.to_le_bytes());
    for off in REGION_OFFSETS {
        out[off as usize..off as usize + REGION_SIZE].copy_from_slice(&r);
    }
    let m = &mut out[meta_off..meta_off + block];
    m[0..8].copy_from_slice(b"metadata");
    m[10..12].copy_from_slice(&3u16.to_le_bytes());
    let items: [(Guid, Vec<u8>); 3] = [
        (FILE_PARAMETERS, {
            let mut v = (block as u32).to_le_bytes().to_vec();
            v.extend_from_slice(&0u32.to_le_bytes());
            v
        }),
        (VIRTUAL_DISK_SIZE, (disk_size as u64).to_le_bytes().to_vec()),
        (LOGICAL_SECTOR_SIZE, logical_sector.to_le_bytes().to_vec()),
    ];
    for (i, (g, v)) in items.iter().enumerate() {
        let e = 32 + i * 32;
        let data_off = 64 * 1024 + i * 64;
        m[e..e + 16].copy_from_slice(&g.0);
        m[e + 16..e + 20].copy_from_slice(&(data_off as u32).to_le_bytes());
        m[e + 20..e + 24].copy_from_slice(&(v.len() as u32).to_le_bytes());
        m[data_off..data_off + v.len()].copy_from_slice(v);
    }
    for b in 0..blocks {
        let start = b * block;
        let end = (start + block).min(disk.len());
        let data = disk.get(start..end).unwrap_or(&[]);
        let idx = b + b / chunk_ratio;
        let entry = if data.iter().all(|&x| x == 0) {
            PAYLOAD_NOT_PRESENT
        } else {
            let mut blk = data.to_vec();
            blk.resize(block, 0);
            out.extend_from_slice(&blk);
            let e = ((payload_at as u64 / MIB) << 20) | PAYLOAD_FULLY_PRESENT;
            payload_at += block;
            e
        };
        out[bat_off + idx * 8..bat_off + idx * 8 + 8].copy_from_slice(&entry.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemSource;

    #[test]
    fn reads_back_a_synthetic_image() {
        let mut disk = vec![0u8; 3 * MIB as usize + 512];
        for (i, b) in disk.iter_mut().enumerate().take(MIB as usize + 100) {
            *b = (i % 251) as u8;
        }
        let len = disk.len();
        disk[len - 1] = 7;
        let image = build_test_vhdx(&disk, 512);
        let src = MemSource::new(image);
        assert!(is_vhdx(&src));
        let v = Vhdx::open(src).unwrap();
        assert_eq!(v.len(), disk.len() as u64);
        let mut out = vec![0u8; disk.len()];
        v.read_at(0, &mut out).unwrap();
        assert_eq!(out, disk);
        let mut tail = [0u8; 2];
        v.read_at(disk.len() as u64 - 2, &mut tail).unwrap();
        assert_eq!(tail, [0, 7]);
        assert!(v.read_at(disk.len() as u64 - 1, &mut tail).is_err());
    }

    #[test]
    fn rejects_damage() {
        let image = build_test_vhdx(&[1u8; 4096], 512);
        // Both headers broken.
        let mut bad = image.clone();
        bad[64 * 1024 + 20] ^= 1;
        bad[128 * 1024 + 20] ^= 1;
        assert!(Vhdx::open(MemSource::new(bad)).is_err());
        // One header broken still opens.
        let mut one = image.clone();
        one[128 * 1024 + 20] ^= 1;
        assert!(Vhdx::open(MemSource::new(one)).is_ok());
        assert!(Vhdx::open(MemSource::new(vec![0u8; 4096])).is_err());
        assert!(Vhdx::open(MemSource::new(image[..300 * 1024].to_vec())).is_err());
    }
}
