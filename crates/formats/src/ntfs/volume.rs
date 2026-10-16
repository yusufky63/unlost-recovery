use crate::ntfs::boot::BootSector;
use crate::ntfs::record::{
    ATTR_ATTRIBUTE_LIST, ATTR_DATA, ATTR_VOLUME_NAME, Body, apply_fixups, attributes, header,
    parse_attribute_list,
};
use crate::ntfs::runlist::{self, Run};
use crate::source::SectorSource;
use crate::{Error, Result};

/// Record numbers of the metafiles the scanner reads.
const MFT_RECORD: u64 = 0;
const VOLUME_RECORD: u64 = 3;
const BITMAP_RECORD: u64 = 6;

/// No volume needs more than 2^32 file records; a larger `$MFT` size is corruption.
const MAX_RECORDS: u64 = 1 << 32;
/// A `$Bitmap` for the largest NTFS volume Windows formats (256 TiB, 64 KiB clusters) is 512 MiB.
const MAX_BITMAP_BYTES: u64 = 1 << 30;

pub struct NtfsVolume<S> {
    src: S,
    pub boot: BootSector,
    mft_runs: Vec<Run>,
    pub record_count: u64,
}

impl<S: SectorSource> NtfsVolume<S> {
    pub fn open(src: S) -> Result<Self> {
        let mut b = [0u8; 512];
        src.read_at(0, &mut b)?;
        let boot = BootSector::parse(&b)?;
        let cluster = u64::from(boot.cluster_size);
        // Every cluster count below is bounded by this check, so a crafted boot sector cannot
        // make the scanner walk or allocate beyond the real device.
        if boot.total_clusters.saturating_mul(cluster) > src.len() {
            return Err(Error::invalid(
                "NTFS boot sector",
                format!(
                    "{} clusters of {cluster} bytes do not fit in {} bytes",
                    boot.total_clusters,
                    src.len()
                ),
            ));
        }
        let rs = boot.record_size as usize;

        let mut rec0 = vec![0u8; rs];
        src.read_at(boot.mft_lcn * cluster, &mut rec0)?;
        if !apply_fixups(&mut rec0) {
            return Err(Error::invalid("$MFT", "record 0 is damaged"));
        }
        let h = header(&rec0)
            .ok_or_else(|| Error::invalid("$MFT", "record 0 has no FILE signature"))?;
        let mut pieces = unnamed_data_pieces(&rec0, &h, boot.total_clusters);
        let first = pieces
            .iter()
            .find(|p| p.start_vcn == 0)
            .cloned()
            .ok_or_else(|| Error::invalid("$MFT", "record 0 has no $DATA"))?;
        let real = first.real;

        // A very fragmented $MFT keeps the rest of its runlist in extension records listed in
        // its $ATTRIBUTE_LIST. Those records sit in the part already mapped.
        let list = attributes(&rec0, &h).find(|a| a.kind == ATTR_ATTRIBUTE_LIST);
        if let Some(list) = list {
            let bytes = match list.body {
                Body::Resident(v) => v.to_vec(),
                Body::NonResident(nr) => {
                    let runs = runlist::decode(nr.runlist, nr.start_vcn, boot.total_clusters)?;
                    let len = nr.real.min(1 << 20) as usize;
                    let mut v = vec![0u8; len];
                    read_runs(&src, &runs, cluster, 0, &mut v)?;
                    v
                }
            };
            let mut runs: Vec<Run> = pieces.iter().flat_map(|p| p.runs.clone()).collect();
            for e in parse_attribute_list(&bytes) {
                if e.kind != ATTR_DATA || !e.name.is_empty() || e.record.record == MFT_RECORD {
                    continue;
                }
                let mut ext = vec![0u8; rs];
                runs.sort_by_key(|r| r.vcn);
                let pos = e.record.record.saturating_mul(rs as u64);
                if read_runs(&src, &runs, cluster, pos, &mut ext).is_err()
                    || !apply_fixups(&mut ext)
                {
                    continue;
                }
                let Some(eh) = header(&ext) else { continue };
                for p in unnamed_data_pieces(&ext, &eh, boot.total_clusters) {
                    runs.extend(p.runs.iter().copied());
                    pieces.push(p);
                }
            }
        }
        pieces.sort_by_key(|p| p.start_vcn);
        let mut mft_runs: Vec<Run> = pieces.into_iter().flat_map(|p| p.runs).collect();
        mft_runs.sort_by_key(|r| r.vcn);
        mft_runs.dedup_by_key(|r| r.vcn);

        // A damaged record 0 can claim any size; only clusters the runs actually map count.
        let mapped = mft_runs
            .iter()
            .filter(|r| r.lcn.is_some())
            .fold(0u64, |acc, r| acc.saturating_add(r.len))
            .saturating_mul(cluster);
        let record_count = (real.min(mapped) / rs as u64).min(MAX_RECORDS);
        Ok(NtfsVolume {
            src,
            boot,
            mft_runs,
            record_count,
        })
    }

    pub fn source(&self) -> &S {
        &self.src
    }

    pub fn cluster_size(&self) -> u64 {
        u64::from(self.boot.cluster_size)
    }

    pub fn volume_bytes(&self) -> u64 {
        self.boot.total_clusters * self.cluster_size()
    }

    /// Reads `count` consecutive records starting at `first` into `buf` (no fixups applied).
    pub fn read_records(&self, first: u64, buf: &mut [u8]) -> Result<()> {
        let pos = first
            .checked_mul(u64::from(self.boot.record_size))
            .ok_or_else(|| Error::invalid("$MFT", "record offset overflow"))?;
        read_runs(&self.src, &self.mft_runs, self.cluster_size(), pos, buf)
    }

    /// One record with fixups applied; `None` if it is not a valid FILE record.
    pub fn record(&self, n: u64) -> Result<Option<(Vec<u8>, bool)>> {
        let mut b = vec![0u8; self.boot.record_size as usize];
        self.read_records(n, &mut b)?;
        if header(&b).is_none() {
            return Ok(None);
        }
        let clean = apply_fixups(&mut b);
        Ok(Some((b, clean)))
    }

    pub fn label(&self) -> String {
        let Ok(Some((b, _))) = self.record(VOLUME_RECORD) else {
            return String::new();
        };
        let Some(h) = header(&b) else {
            return String::new();
        };
        attributes(&b, &h)
            .find(|a| a.kind == ATTR_VOLUME_NAME)
            .and_then(|a| match a.body {
                Body::Resident(v) => {
                    let units: Vec<u16> = v
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_le_bytes(*c))
                        .collect();
                    Some(String::from_utf16_lossy(&units))
                }
                Body::NonResident(_) => None,
            })
            .unwrap_or_default()
    }

    /// The volume's cluster allocation bitmap as of now.
    pub fn bitmap(&self) -> Result<Bitmap> {
        let (b, _) = self
            .record(BITMAP_RECORD)?
            .ok_or_else(|| Error::invalid("$Bitmap", "record 6 is not a file record"))?;
        let h = header(&b).ok_or_else(|| Error::invalid("$Bitmap", "no header"))?;
        let piece = unnamed_data_pieces(&b, &h, self.boot.total_clusters)
            .into_iter()
            .find(|p| p.start_vcn == 0)
            .ok_or_else(|| Error::invalid("$Bitmap", "no $DATA"))?;
        let want = self.boot.total_clusters.div_ceil(8);
        if piece.real < want || piece.real > MAX_BITMAP_BYTES {
            return Err(Error::invalid(
                "$Bitmap",
                format!(
                    "{} bytes for {} clusters",
                    piece.real, self.boot.total_clusters
                ),
            ));
        }
        let mut bits = vec![0u8; want as usize];
        match piece.resident {
            Some(v) => {
                let n = v.len().min(bits.len());
                bits[..n].copy_from_slice(&v[..n]);
            }
            None => read_runs(&self.src, &piece.runs, self.cluster_size(), 0, &mut bits)?,
        }
        Ok(Bitmap {
            bits,
            clusters: self.boot.total_clusters,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Piece {
    pub start_vcn: u64,
    pub runs: Vec<Run>,
    pub runs_ok: bool,
    pub real: u64,
    pub initialized: u64,
    pub flags: u16,
    pub resident: Option<Vec<u8>>,
}

/// Unnamed `$DATA` pieces of one record.
pub(crate) fn unnamed_data_pieces(
    b: &[u8],
    h: &crate::ntfs::record::Header,
    total_clusters: u64,
) -> Vec<Piece> {
    let mut out = Vec::new();
    for a in attributes(b, h) {
        if a.kind != ATTR_DATA || !a.name.is_empty() {
            continue;
        }
        out.push(piece_of(&a, total_clusters));
    }
    out
}

pub(crate) fn piece_of(a: &crate::ntfs::record::Attr<'_>, total_clusters: u64) -> Piece {
    match &a.body {
        Body::Resident(v) => Piece {
            real: v.len() as u64,
            initialized: v.len() as u64,
            flags: a.flags,
            resident: Some(v.to_vec()),
            runs_ok: true,
            ..Piece::default()
        },
        Body::NonResident(nr) => {
            let decoded = runlist::decode(nr.runlist, nr.start_vcn, total_clusters);
            Piece {
                start_vcn: nr.start_vcn,
                runs_ok: decoded.is_ok(),
                runs: decoded.unwrap_or_default(),
                real: nr.real,
                initialized: nr.initialized.min(nr.real),
                flags: a.flags,
                resident: None,
            }
        }
    }
}

/// Reads attribute bytes `[pos, pos + buf.len())` through `runs` (sorted by VCN). Sparse runs
/// read as zeros; a hole in the runlist is an error.
pub(crate) fn read_runs(
    src: &dyn SectorSource,
    runs: &[Run],
    cluster: u64,
    pos: u64,
    buf: &mut [u8],
) -> Result<()> {
    let mut done = 0usize;
    while done < buf.len() {
        let at = pos + done as u64;
        let vcn = at / cluster;
        let within = at % cluster;
        let i = runs.partition_point(|r| r.vcn + r.len <= vcn);
        let run = runs
            .get(i)
            .filter(|r| r.vcn <= vcn)
            .ok_or_else(|| Error::invalid("runlist", format!("no run maps cluster {vcn}")))?;
        let avail = (run.vcn + run.len - vcn) * cluster - within;
        let n = (avail.min((buf.len() - done) as u64)) as usize;
        let part = &mut buf[done..done + n];
        match run.lcn {
            Some(lcn) => {
                let off = (lcn + (vcn - run.vcn))
                    .checked_mul(cluster)
                    .and_then(|o| o.checked_add(within))
                    .ok_or_else(|| Error::invalid("runlist", "offset overflow"))?;
                src.read_at(off, part)?;
            }
            None => part.fill(0),
        }
        done += n;
    }
    Ok(())
}

/// Cluster allocation bitmap. Clusters outside the bitmap count as allocated, which errs on
/// the side of "not recoverable".
pub struct Bitmap {
    bits: Vec<u8>,
    clusters: u64,
}

impl Bitmap {
    pub fn from_bytes(bits: Vec<u8>, clusters: u64) -> Self {
        Bitmap { bits, clusters }
    }

    pub fn is_allocated(&self, lcn: u64) -> bool {
        if lcn >= self.clusters {
            return true;
        }
        self.bits
            .get((lcn / 8) as usize)
            .is_none_or(|b| b & (1 << (lcn % 8)) != 0)
    }

    pub fn count_allocated(&self, start: u64, len: u64) -> u64 {
        let end = start.saturating_add(len);
        let mut n = 0u64;
        let mut i = start;
        while i < end {
            if i.is_multiple_of(8) && i + 8 <= end && i + 8 <= self.clusters {
                n += u64::from(
                    self.bits
                        .get((i / 8) as usize)
                        .map_or(8, |b| b.count_ones()),
                );
                i += 8;
            } else {
                n += u64::from(self.is_allocated(i));
                i += 1;
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_counts() {
        let bm = Bitmap::from_bytes(vec![0b0000_0101, 0xFF, 0x00], 20);
        assert!(bm.is_allocated(0));
        assert!(!bm.is_allocated(1));
        assert!(bm.is_allocated(2));
        assert_eq!(bm.count_allocated(0, 8), 2);
        assert_eq!(bm.count_allocated(0, 16), 10);
        assert_eq!(bm.count_allocated(16, 4), 0);
        // Past the end counts as allocated.
        assert_eq!(bm.count_allocated(18, 4), 2);
        assert!(bm.is_allocated(1_000));
    }
}
