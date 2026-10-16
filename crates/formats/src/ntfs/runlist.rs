//! NTFS data runs ("mapping pairs"): where an attribute's clusters live on the volume.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Runs per attribute piece. Real files rarely exceed a few thousand; the cap keeps a crafted
/// record from allocating without bound.
pub const MAX_RUNS: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// First virtual cluster of the run inside the attribute.
    pub vcn: u64,
    /// First logical cluster on the volume; `None` for a sparse run (reads as zeros).
    pub lcn: Option<u64>,
    /// Length in clusters, never 0.
    pub len: u64,
}

/// Decodes a mapping-pairs array starting at `start_vcn`. Every non-sparse run must lie inside
/// `total_clusters`.
pub fn decode(data: &[u8], start_vcn: u64, total_clusters: u64) -> Result<Vec<Run>> {
    let bad = |d: String| Error::invalid("data runs", d);
    let mut runs = Vec::new();
    let mut i = 0usize;
    let mut vcn = start_vcn;
    let mut lcn: i64 = 0;
    while let Some(&head) = data.get(i) {
        if head == 0 {
            return Ok(runs);
        }
        if runs.len() >= MAX_RUNS {
            return Err(Error::Limit(format!("more than {MAX_RUNS} runs")));
        }
        let len_size = usize::from(head & 0x0F);
        let off_size = usize::from(head >> 4);
        if !(1..=8).contains(&len_size) || off_size > 8 {
            return Err(bad(format!("header byte 0x{head:02X}")));
        }
        let len_bytes = data
            .get(i + 1..i + 1 + len_size)
            .ok_or_else(|| bad("truncated length".into()))?;
        let len = read_unsigned(len_bytes);
        // Sparse runs may exceed the volume (a sparse file can be larger than its disk), but
        // not the 2^48 clusters NTFS can address.
        if len == 0 || len > (1 << 48) {
            return Err(bad(format!("run of {len} clusters")));
        }
        let off_bytes = data
            .get(i + 1 + len_size..i + 1 + len_size + off_size)
            .ok_or_else(|| bad("truncated offset".into()))?;
        let run_lcn = if off_size == 0 {
            None
        } else {
            lcn = lcn
                .checked_add(read_signed(off_bytes))
                .ok_or_else(|| bad("offset overflow".into()))?;
            let start = u64::try_from(lcn).map_err(|_| bad(format!("negative LCN {lcn}")))?;
            if start
                .checked_add(len)
                .is_none_or(|end| end > total_clusters)
            {
                return Err(bad(format!("clusters {start}+{len} outside the volume")));
            }
            Some(start)
        };
        runs.push(Run {
            vcn,
            lcn: run_lcn,
            len,
        });
        vcn = vcn
            .checked_add(len)
            .ok_or_else(|| bad("VCN overflow".into()))?;
        i += 1 + len_size + off_size;
    }
    // A missing terminator is tolerated: the attribute ended.
    Ok(runs)
}

fn read_unsigned(b: &[u8]) -> u64 {
    b.iter()
        .rev()
        .fold(0u64, |acc, &x| (acc << 8) | u64::from(x))
}

fn read_signed(b: &[u8]) -> i64 {
    if b.is_empty() {
        return 0;
    }
    let v = read_unsigned(b);
    let bits = b.len() * 8;
    if bits >= 64 {
        v as i64
    } else if v & (1 << (bits - 1)) != 0 {
        (v | (!0u64 << bits)) as i64
    } else {
        v as i64
    }
}

/// Encodes runs the way NTFS does, for tests and fuzz seeds.
#[cfg(any(test, feature = "test-util"))]
pub fn encode(runs: &[(Option<u64>, u64)]) -> Vec<u8> {
    fn min_unsigned(v: u64) -> Vec<u8> {
        let mut b = v.to_le_bytes().to_vec();
        while b.len() > 1 && b[b.len() - 1] == 0 {
            b.pop();
        }
        b
    }
    fn min_signed(v: i64) -> Vec<u8> {
        let mut b = v.to_le_bytes().to_vec();
        while b.len() > 1 {
            let last = b[b.len() - 1];
            let prev_sign = b[b.len() - 2] & 0x80;
            if (last == 0 && prev_sign == 0) || (last == 0xFF && prev_sign != 0) {
                b.pop();
            } else {
                break;
            }
        }
        b
    }
    let mut out = Vec::new();
    let mut prev: i64 = 0;
    for (lcn, len) in runs {
        let l = min_unsigned(*len);
        let o = match lcn {
            Some(lcn) => {
                let delta = *lcn as i64 - prev;
                prev = *lcn as i64;
                min_signed(delta)
            }
            None => Vec::new(),
        };
        out.push(((o.len() as u8) << 4) | l.len() as u8);
        out.extend_from_slice(&l);
        out.extend_from_slice(&o);
    }
    out.push(0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_sparse_and_backward_runs() {
        let input = [
            (Some(1000), 16),
            (None, 32),
            (Some(200), 1),
            (Some(70_000), 300),
        ];
        let runs = decode(&encode(&input), 0, 1_000_000).unwrap();
        assert_eq!(runs.len(), 4);
        assert_eq!(
            runs[0],
            Run {
                vcn: 0,
                lcn: Some(1000),
                len: 16
            }
        );
        assert_eq!(
            runs[1],
            Run {
                vcn: 16,
                lcn: None,
                len: 32
            }
        );
        assert_eq!(
            runs[2],
            Run {
                vcn: 48,
                lcn: Some(200),
                len: 1
            }
        );
        assert_eq!(
            runs[3],
            Run {
                vcn: 49,
                lcn: Some(70_000),
                len: 300
            }
        );
    }

    #[test]
    fn known_bytes() {
        // 0x21: 1-byte length, 2-byte offset. 0x18 clusters at 0x5634.
        let runs = decode(&[0x21, 0x18, 0x34, 0x56, 0x00], 0, 100_000).unwrap();
        assert_eq!(
            runs,
            vec![Run {
                vcn: 0,
                lcn: Some(0x5634),
                len: 0x18
            }]
        );
    }

    #[test]
    fn rejects_bad_runs() {
        assert!(decode(&[0x21, 0x18, 0x34], 0, 100_000).is_err());
        assert!(decode(&[0x01, 0x00], 0, 100).is_err());
        assert!(decode(&[0x09, 1, 1, 1, 1, 1, 1, 1, 1, 1], 0, 100).is_err());
        // Outside the volume.
        assert!(decode(&encode(&[(Some(90), 20)]), 0, 100).is_err());
        // Negative LCN.
        assert!(decode(&[0x11, 0x01, 0xFF, 0x00], 0, 100).is_err());
        assert!(decode(&[], 0, 100).unwrap().is_empty());
    }
}
