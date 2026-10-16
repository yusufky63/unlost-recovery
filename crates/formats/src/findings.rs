//! The scan's output stream: deleted files, the folders they sit in, and new grades after
//! content sampling, packed into batches of bytes. The elevated broker sends batches over its
//! pipe and the UI process writes them into its session database; an in-process scan uses the
//! same batches. The UI does not trust the broker's bytes, so decoding checks every length and
//! caps every allocation.

use crate::ntfs::location;
use crate::ntfs::scan::{DeletedFile, PathStatus};
use crate::state::{Assessment, Reason, State};
use crate::{Error, Result};

/// A batch is handed on once it holds this many bytes. One message is at most a little over
/// [`location::MAX_BYTES`] or [`MAX_PATH`], so a batch stays well under the pipe's 4 MiB frames.
pub const BATCH_BYTES: usize = 1 << 20;
/// NTFS names are at most 255 UTF-16 units: 765 bytes of UTF-8.
const MAX_NAME: usize = 4096;
/// 1,024 folder levels of 255 characters.
pub const MAX_PATH: usize = 1 << 20;
const MAX_TEXT: usize = 1024;
const MAX_TAG: usize = 64;

const FILE: u8 = 1;
const FOLDER: u8 = 2;
const REGRADE: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub record: u64,
    pub name: String,
    /// Key of the parent folder's [`FolderRow`].
    pub folder: u64,
    pub links: u16,
    pub size: u64,
    pub created: i64,
    pub modified: i64,
    pub streams: u32,
    pub assessment: Assessment,
    /// [`location::encode`]d content location.
    pub location: Vec<u8>,
    /// Authenticates `location` when the broker produced it; empty otherwise.
    pub tag: Vec<u8>,
}

impl FileRow {
    pub fn new(f: &DeletedFile) -> FileRow {
        FileRow {
            record: f.record,
            name: f.name.clone(),
            folder: f.folder,
            links: f.links,
            size: f.size,
            created: f.created,
            modified: f.modified,
            streams: f.streams,
            assessment: f.assessment.clone(),
            location: location::encode(&f.data),
            tag: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderRow {
    pub key: u64,
    pub path: String,
    pub status: PathStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    File(FileRow),
    Folder(FolderRow),
    /// A file already sent, graded again after its content was sampled.
    Regrade {
        record: u64,
        assessment: Assessment,
    },
}

pub fn put(out: &mut Vec<u8>, m: &Message) {
    match m {
        Message::File(f) => {
            out.push(FILE);
            out.extend_from_slice(&f.record.to_le_bytes());
            bytes(out, f.name.as_bytes());
            out.extend_from_slice(&f.folder.to_le_bytes());
            out.extend_from_slice(&f.links.to_le_bytes());
            out.extend_from_slice(&f.size.to_le_bytes());
            out.extend_from_slice(&f.created.to_le_bytes());
            out.extend_from_slice(&f.modified.to_le_bytes());
            out.extend_from_slice(&f.streams.to_le_bytes());
            assessment(out, &f.assessment);
            bytes(out, &f.location);
            bytes(out, &f.tag);
        }
        Message::Folder(d) => {
            out.push(FOLDER);
            out.extend_from_slice(&d.key.to_le_bytes());
            bytes(out, d.path.as_bytes());
            out.push(status_code(d.status));
        }
        Message::Regrade {
            record,
            assessment: a,
        } => {
            out.push(REGRADE);
            out.extend_from_slice(&record.to_le_bytes());
            assessment(out, a);
        }
    }
}

pub fn decode(batch: &[u8]) -> Result<Vec<Message>> {
    let mut c = Cursor { b: batch, i: 0 };
    let mut out = Vec::new();
    while c.i < batch.len() {
        let m = match c.u8()? {
            FILE => Message::File(FileRow {
                record: c.u64()?,
                name: c.text(MAX_NAME)?,
                folder: c.u64()?,
                links: u16::from_le_bytes(c.array()?),
                size: c.u64()?,
                created: c.u64()? as i64,
                modified: c.u64()? as i64,
                streams: u32::from_le_bytes(c.array()?),
                assessment: c.assessment()?,
                location: c.bytes(location::MAX_BYTES + 64)?.to_vec(),
                tag: c.bytes(MAX_TAG)?.to_vec(),
            }),
            FOLDER => Message::Folder(FolderRow {
                key: c.u64()?,
                path: c.text(MAX_PATH)?,
                status: status_from(c.u8()?).ok_or_else(|| bad("path status"))?,
            }),
            REGRADE => Message::Regrade {
                record: c.u64()?,
                assessment: c.assessment()?,
            },
            _ => return Err(bad("message kind")),
        };
        out.push(m);
    }
    Ok(out)
}

pub fn status_code(s: PathStatus) -> u8 {
    match s {
        PathStatus::Live => 0,
        PathStatus::DeletedParent => 1,
        PathStatus::Lost => 2,
    }
}

pub fn status_from(c: u8) -> Option<PathStatus> {
    Some(match c {
        0 => PathStatus::Live,
        1 => PathStatus::DeletedParent,
        2 => PathStatus::Lost,
        _ => return None,
    })
}

fn bad(detail: &str) -> Error {
    Error::invalid("scan batch", detail.to_string())
}

fn bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u32).to_le_bytes());
    out.extend_from_slice(b);
}

fn assessment(out: &mut Vec<u8>, a: &Assessment) {
    let (code, x, y, text) = a.reason.to_parts();
    out.push(a.state.rank());
    out.push(code);
    out.extend_from_slice(&x.to_le_bytes());
    out.extend_from_slice(&y.to_le_bytes());
    match text {
        Some(t) => {
            out.push(1);
            bytes(out, t.as_bytes());
        }
        None => out.push(0),
    }
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .i
            .checked_add(n)
            .filter(|&e| e <= self.b.len())
            .ok_or_else(|| bad("truncated"))?;
        let s = &self.b[self.i..end];
        self.i = end;
        Ok(s)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        self.take(N)?.try_into().map_err(|_| bad("truncated"))
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    fn bytes(&mut self, max: usize) -> Result<&'a [u8]> {
        let n = u32::from_le_bytes(self.array()?) as usize;
        if n > max {
            return Err(bad("field too long"));
        }
        self.take(n)
    }
    fn text(&mut self, max: usize) -> Result<String> {
        let b = self.bytes(max)?;
        std::str::from_utf8(b)
            .map(str::to_string)
            .map_err(|_| bad("text is not UTF-8"))
    }
    fn assessment(&mut self) -> Result<Assessment> {
        let state = State::from_rank(self.u8()?).ok_or_else(|| bad("state"))?;
        let code = self.u8()?;
        let x = self.u64()? as i64;
        let y = self.u64()? as i64;
        let text = match self.u8()? {
            0 => None,
            1 => Some(self.text(MAX_TEXT)?),
            _ => return Err(bad("reason text flag")),
        };
        let reason =
            Reason::from_parts(code, x, y, text.as_deref()).ok_or_else(|| bad("reason"))?;
        Ok(Assessment::new(state, reason))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Message> {
        vec![
            Message::File(FileRow {
                record: 40,
                name: "Iğdır notları.txt".into(),
                folder: 30 | (1 << 48),
                links: 2,
                size: u64::MAX,
                created: -1,
                modified: 1_704_067_200,
                streams: 1,
                assessment: Assessment::new(
                    State::Partial,
                    Reason::HeaderMismatch {
                        expected: "a JPEG image".into(),
                    },
                ),
                location: vec![1, b'h', b'i'],
                tag: vec![7; 16],
            }),
            Message::Folder(FolderRow {
                key: 30 | (1 << 48),
                path: r"\Fotoğraflar".into(),
                status: PathStatus::DeletedParent,
            }),
            Message::Regrade {
                record: 40,
                assessment: Assessment::new(State::Zeroed, Reason::AllZero { sampled: 3 }),
            },
        ]
    }

    #[test]
    fn round_trips() {
        let msgs = sample();
        let mut b = Vec::new();
        for m in &msgs {
            put(&mut b, m);
        }
        assert_eq!(decode(&b).unwrap(), msgs);
        assert!(decode(&[]).unwrap().is_empty());
    }

    #[test]
    fn rejects_damage_without_panicking() {
        let mut b = Vec::new();
        for m in &sample() {
            put(&mut b, m);
        }
        for cut in 0..b.len() {
            let _ = decode(&b[..cut]);
        }
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..5_000 {
            let mut m = b.clone();
            for _ in 0..4 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let at = (x % m.len() as u64) as usize;
                m[at] = (x >> 32) as u8;
            }
            let _ = decode(&m);
        }
        // A length field claiming 4 GiB is refused before anything is allocated.
        let mut huge = vec![FOLDER];
        huge.extend_from_slice(&0u64.to_le_bytes());
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&huge).is_err());
    }
}
