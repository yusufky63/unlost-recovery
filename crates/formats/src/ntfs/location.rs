//! Compact encoding of where a deleted file's content lies. The session database stores it, and
//! a read request hands it back to the broker, so the broker keeps nothing per file. Runs are
//! delta-coded like NTFS mapping pairs, with LEB128 numbers: a typical run takes 4 to 6 bytes.
//!
//! Decoding is bounded by the input: every run takes at least three bytes, so a short buffer
//! cannot claim millions of runs.

use crate::ntfs::runlist::Run;
use crate::ntfs::scan::{DataLocation, NonResidentData};
use crate::{Error, Result};

/// Largest encoding. A file in so many pieces that its map would not fit keeps the runs that do
/// (see [`fit`]) and is graded partial.
pub const MAX_BYTES: usize = 1 << 20;
/// Tag byte plus three LEB128 numbers of at most 10 bytes.
const HEADER_MAX: usize = 1 + 3 * 10;
/// Same bound as the mapping-pairs decoder: NTFS addresses at most 2^48 clusters.
const MAX_RUN_LEN: u64 = 1 << 48;

const MISSING: u8 = 0;
const RESIDENT: u8 = 1;
const RUNS: u8 = 2;
const RUNS_COMPRESSED: u8 = 3;

pub fn encode(d: &DataLocation) -> Vec<u8> {
    match d {
        DataLocation::Missing => vec![MISSING],
        DataLocation::Resident(v) => {
            let mut b = Vec::with_capacity(1 + v.len());
            b.push(RESIDENT);
            b.extend_from_slice(v);
            b
        }
        DataLocation::NonResident(nr) => {
            let mut body = Vec::with_capacity(nr.runs.len() * 6);
            let (n, _) = put_runs(&nr.runs, Some(&mut body), usize::MAX);
            let mut b = Vec::with_capacity(HEADER_MAX + body.len());
            b.push(if nr.compressed { RUNS_COMPRESSED } else { RUNS });
            put(&mut b, nr.real);
            put(&mut b, nr.initialized);
            put(&mut b, n as u64);
            b.extend_from_slice(&body);
            b
        }
    }
}

/// Cuts `runs` so their encoding fits [`MAX_BYTES`]. Returns the original number of runs when
/// it had to cut.
pub fn fit(runs: &mut Vec<Run>) -> Option<u64> {
    let (n, _) = put_runs(runs, None, MAX_BYTES - HEADER_MAX);
    if n == runs.len() {
        return None;
    }
    let total = runs.len() as u64;
    runs.truncate(n);
    Some(total)
}

pub fn decode(b: &[u8]) -> Result<DataLocation> {
    let bad = |d: &str| Error::invalid("content location", d.to_string());
    let (&tag, rest) = b.split_first().ok_or_else(|| bad("empty"))?;
    match tag {
        MISSING if rest.is_empty() => Ok(DataLocation::Missing),
        RESIDENT => Ok(DataLocation::Resident(rest.to_vec())),
        RUNS | RUNS_COMPRESSED => {
            let mut c = Cursor { b: rest, i: 0 };
            let real = c.num()?;
            let initialized = c.num()?;
            let n = c.num()?;
            if n > (rest.len() / 3) as u64 {
                return Err(bad("more runs than bytes"));
            }
            let mut runs = Vec::with_capacity(n as usize);
            let (mut end, mut lcn) = (0u64, 0u64);
            for _ in 0..n {
                let vcn = end.wrapping_add(unzigzag(c.num()?) as u64);
                let len = c.num()?;
                if len == 0 || len > MAX_RUN_LEN {
                    return Err(bad("run length"));
                }
                let code = c.num()?;
                let run_lcn = if code == 0 {
                    None
                } else {
                    lcn = lcn.wrapping_add(unzigzag(code - 1) as u64);
                    lcn.checked_add(len)
                        .ok_or_else(|| bad("cluster overflow"))?;
                    Some(lcn)
                };
                end = vcn.checked_add(len).ok_or_else(|| bad("VCN overflow"))?;
                runs.push(Run {
                    vcn,
                    lcn: run_lcn,
                    len,
                });
            }
            if c.i != rest.len() {
                return Err(bad("trailing bytes"));
            }
            Ok(DataLocation::NonResident(NonResidentData {
                runs,
                real,
                initialized,
                compressed: tag == RUNS_COMPRESSED,
            }))
        }
        _ => Err(bad("unknown kind")),
    }
}

/// Writes runs (into `out` when given) until `limit` bytes; returns (runs written, bytes).
fn put_runs(runs: &[Run], mut out: Option<&mut Vec<u8>>, limit: usize) -> (usize, usize) {
    let mut scratch = Vec::with_capacity(32);
    let (mut end, mut lcn, mut bytes) = (0u64, 0u64, 0usize);
    for (i, r) in runs.iter().enumerate() {
        scratch.clear();
        // Wrapping differences keep the encoding exact for any order the runs come in.
        put(&mut scratch, zigzag(r.vcn.wrapping_sub(end) as i64));
        put(&mut scratch, r.len);
        match r.lcn {
            None => put(&mut scratch, 0),
            Some(l) => {
                put(&mut scratch, zigzag(l.wrapping_sub(lcn) as i64) + 1);
                lcn = l;
            }
        }
        if bytes + scratch.len() > limit {
            return (i, bytes);
        }
        bytes += scratch.len();
        if let Some(o) = out.as_deref_mut() {
            o.extend_from_slice(&scratch);
        }
        end = r.vcn.wrapping_add(r.len);
    }
    (runs.len(), bytes)
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

pub(crate) fn put(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn num(&mut self) -> Result<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self
                .b
                .get(self.i)
                .ok_or_else(|| Error::invalid("content location", "truncated number"))?;
            self.i += 1;
            v |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(Error::invalid("content location", "number too long"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nr(runs: Vec<Run>) -> DataLocation {
        DataLocation::NonResident(NonResidentData {
            runs,
            real: 123_456,
            initialized: 100_000,
            compressed: false,
        })
    }

    fn same(a: &DataLocation, b: &DataLocation) -> bool {
        match (a, b) {
            (DataLocation::Missing, DataLocation::Missing) => true,
            (DataLocation::Resident(x), DataLocation::Resident(y)) => x == y,
            (DataLocation::NonResident(x), DataLocation::NonResident(y)) => {
                x.runs == y.runs
                    && x.real == y.real
                    && x.initialized == y.initialized
                    && x.compressed == y.compressed
            }
            _ => false,
        }
    }

    #[test]
    fn round_trips() {
        let run = |vcn, lcn, len| Run { vcn, lcn, len };
        let cases = [
            DataLocation::Missing,
            DataLocation::Resident(Vec::new()),
            DataLocation::Resident(b"note".to_vec()),
            nr(vec![]),
            nr(vec![run(0, Some(1_000_000), 8)]),
            // Sparse middle, a jump backwards on disk, a gap in the VCNs, an overlap.
            nr(vec![
                run(0, Some(500), 1),
                run(1, None, 2),
                run(3, Some(20), 1),
                run(10, Some((1 << 48) - 5), 4),
                run(12, Some(7), 1),
            ]),
            DataLocation::NonResident(NonResidentData {
                runs: vec![run(0, Some(4), 2), run(2, None, 2)],
                real: 16_000,
                initialized: 16_000,
                compressed: true,
            }),
        ];
        for c in &cases {
            let enc = encode(c);
            let back = decode(&enc).unwrap();
            assert!(same(c, &back), "{c:?} -> {back:?}");
        }
        // A typical contiguous run stays small.
        assert!(encode(&nr(vec![run(0, Some(123_456_789), 300)])).len() <= 16);
    }

    #[test]
    fn rejects_garbage_without_panicking() {
        assert!(decode(&[]).is_err());
        assert!(decode(&[MISSING, 1]).is_err());
        assert!(decode(&[9]).is_err());
        // Claims a billion runs in a few bytes.
        let mut b = vec![RUNS, 0, 0];
        put(&mut b, 1 << 30);
        assert!(decode(&b).is_err());
        // Zero-length run.
        assert!(decode(&[RUNS, 0, 0, 1, 0, 0, 0]).is_err());
        // Trailing bytes.
        let mut ok = encode(&nr(vec![Run {
            vcn: 0,
            lcn: Some(3),
            len: 1,
        }]));
        ok.push(0);
        assert!(decode(&ok).is_err());
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let len = (x % 40) as usize;
            let bytes: Vec<u8> = (0..len)
                .map(|i| (x.rotate_left(i as u32 * 7) & 0xFF) as u8)
                .collect();
            if let Ok(d) = decode(&bytes) {
                assert!(same(&d, &decode(&encode(&d)).unwrap()));
            }
        }
    }

    #[test]
    fn fit_cuts_huge_maps() {
        let mut runs: Vec<Run> = (0..400_000u64)
            .map(|i| Run {
                vcn: i,
                lcn: Some(((i * 7_919) % 1_000_000) * 1_000),
                len: 1,
            })
            .collect();
        let total = fit(&mut runs);
        assert_eq!(total, Some(400_000));
        assert!(runs.len() < 400_000 && runs.len() > 100_000);
        let enc = encode(&nr(runs.clone()));
        assert!(enc.len() <= MAX_BYTES);
        assert_eq!(fit(&mut runs), None);
    }
}
