//! Copying a deleted file's content out of the volume.

use serde::{Deserialize, Serialize};

use crate::ntfs::scan::DataLocation;
use crate::source::SectorSource;
use crate::state::{Assessment, Reason, State};
use crate::{Error, Progress, Result};

/// Bytes read per I/O while copying.
const CHUNK: u64 = 1 << 20;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadStats {
    pub bytes: u64,
    /// Clusters actually read from the device (sparse and uninitialized ones are not read).
    pub clusters_read: u64,
    /// Read clusters whose bytes were all zero.
    pub zero_clusters: u64,
    /// Bytes the device refused to return, filled with zeros.
    pub unreadable_bytes: u64,
    /// Bytes no run described, filled with zeros.
    pub missing_bytes: u64,
}

/// Streams the file's content (exactly its size) to `out`. Sparse runs, bytes past the
/// initialized size and gaps become zeros. Unreadable sectors are read once on their own after
/// a failed block read, then filled with zeros and counted.
pub fn read_content(
    src: &dyn SectorSource,
    cluster: u64,
    data: &DataLocation,
    out: &mut dyn FnMut(&[u8]) -> Result<()>,
    progress: &dyn Progress,
) -> Result<ReadStats> {
    let mut stats = ReadStats::default();
    let nr = match data {
        DataLocation::Missing => return Ok(stats),
        DataLocation::Resident(v) => {
            out(v)?;
            stats.bytes = v.len() as u64;
            return Ok(stats);
        }
        DataLocation::NonResident(nr) => nr,
    };
    let real = nr.real;
    let limit = src.len().saturating_mul(crate::ntfs::scan::MAX_SIZE_FACTOR);
    if real > limit {
        return Err(Error::Limit(format!(
            "file size {real} is implausible for a {}-byte source",
            src.len()
        )));
    }
    let mut pos = 0u64;
    let zeros = vec![0u8; CHUNK as usize];
    let mut buf = vec![0u8; CHUNK as usize];
    let emit_zeros = |len: u64, out: &mut dyn FnMut(&[u8]) -> Result<()>| -> Result<()> {
        let mut left = len;
        while left > 0 {
            let n = left.min(CHUNK);
            out(&zeros[..n as usize])?;
            left -= n;
        }
        Ok(())
    };
    for run in &nr.runs {
        if pos >= real {
            break;
        }
        let run_start = run.vcn.saturating_mul(cluster);
        if run_start > pos {
            let gap = run_start.min(real) - pos;
            emit_zeros(gap, out)?;
            stats.missing_bytes += gap;
            pos += gap;
        }
        if run_start < pos && run_start + run.len * cluster <= pos {
            continue;
        }
        let run_end = run_start
            .saturating_add(run.len.saturating_mul(cluster))
            .min(real);
        while pos < run_end {
            if progress.cancelled() {
                return Err(Error::Cancelled);
            }
            let n = (run_end - pos).min(CHUNK);
            match run.lcn {
                Some(lcn) if pos < nr.initialized => {
                    let rel = pos - run_start;
                    let dev_off = lcn
                        .checked_mul(cluster)
                        .and_then(|o| o.checked_add(rel))
                        .ok_or_else(|| Error::invalid("runlist", "offset overflow"))?;
                    let aligned = n.div_ceil(cluster) * cluster;
                    let part = &mut buf[..aligned as usize];
                    stats.unreadable_bytes += read_scraping(src, dev_off, part);
                    for c in part.chunks(cluster as usize) {
                        stats.clusters_read += 1;
                        if c.iter().all(|&b| b == 0) {
                            stats.zero_clusters += 1;
                        }
                    }
                    let init_end = nr.initialized.saturating_sub(pos).min(n) as usize;
                    part[init_end..n as usize].fill(0);
                    out(&part[..n as usize])?;
                }
                _ => emit_zeros(n, out)?,
            }
            pos += n;
            progress.advance(n);
        }
    }
    if pos < real {
        emit_zeros(real - pos, out)?;
        stats.missing_bytes += real - pos;
    }
    stats.bytes = real;
    Ok(stats)
}

/// Reads a block; on failure reads each sector once and zero-fills the ones that fail.
/// Returns the number of unreadable bytes.
fn read_scraping(src: &dyn SectorSource, offset: u64, buf: &mut [u8]) -> u64 {
    if src.read_at(offset, buf).is_ok() {
        return 0;
    }
    let sector = src.sector_size().max(512) as usize;
    let mut bad = 0u64;
    for (i, s) in buf.chunks_mut(sector).enumerate() {
        if src.read_at(offset + (i * sector) as u64, s).is_err() {
            s.fill(0);
            bad += s.len() as u64;
        }
    }
    bad
}

/// The state after reading: all-zero content means the SSD erased it; unreadable bytes make it
/// partial. Otherwise the scan's assessment stands.
pub fn after_read(pre: &Assessment, stats: &ReadStats) -> Assessment {
    if stats.unreadable_bytes > 0 {
        return Assessment::new(
            State::Partial,
            Reason::Unreadable {
                bytes: stats.unreadable_bytes,
            },
        );
    }
    let gradable = matches!(pre.state, State::LikelyIntact | State::Partial);
    if gradable && stats.clusters_read > 0 && stats.zero_clusters == stats.clusters_read {
        return Assessment::new(
            State::Zeroed,
            Reason::AllZero {
                sampled: u32::try_from(stats.clusters_read).unwrap_or(u32::MAX),
            },
        );
    }
    pre.clone()
}
