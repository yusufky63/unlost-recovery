//! Messages between the UI and the engine, and their framing on the broker pipe.
//!
//! Frame: `u32 length` (of everything after it), `u8 kind`, `u64 request id`, payload. JSON
//! payloads for requests, progress and results; raw bytes for file content. Every length is
//! checked against [`MAX_FRAME`] before anything is allocated: the UI treats the broker's output
//! as untrusted, and the broker treats the UI's requests the same way.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use serde::{Deserialize, Serialize};
use unlost_formats::magic::Category;
use unlost_formats::ntfs::{PathStatus, ScanStats};
use unlost_formats::partition::FsKind;
use unlost_formats::{Assessment, State};

use crate::sources::SourceSpec;

pub const PROTOCOL_VERSION: u32 = 1;
/// Largest frame accepted in either direction.
pub const MAX_FRAME: usize = 4 << 20;

pub const KIND_REQUEST: u8 = 1;
pub const KIND_PROGRESS: u8 = 2;
pub const KIND_DATA: u8 = 3;
pub const KIND_OK: u8 = 4;
pub const KIND_ERR: u8 = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Request {
    Hello,
    Scan {
        source: SourceSpec,
        /// Folder for the session database, already approved by [`crate::workspace::approve`].
        /// The broker ignores it: it writes nothing and streams its findings instead.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<String>,
    },
    Query {
        scan: u32,
        query: Query,
    },
    Read {
        scan: u32,
        record: u64,
        /// For broker scans: the location the broker streamed for this record, handed back
        /// because the broker keeps nothing per file. Filled in by the engine.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sealed: Option<Sealed>,
    },
    /// Rows for specific records (the selection being recovered).
    Entries {
        scan: u32,
        records: Vec<u64>,
    },
    Forget {
        scan: u32,
    },
    ShadowList,
    ShadowDir {
        device: String,
        path: String,
    },
    ShadowRead {
        device: String,
        path: String,
    },
    Cancel {
        id: u64,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    #[default]
    State,
    Name,
    Folder,
    Size,
    Modified,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Query {
    pub text: String,
    /// Empty means all states.
    pub states: Vec<State>,
    pub category: Option<Category>,
    pub sort: SortKey,
    pub desc: bool,
    pub offset: u32,
    pub limit: u32,
}

/// A content location the broker produced, on its way back to the broker for reading. `tag`
/// authenticates `location` with a key only the broker holds, so a read request can name only
/// clusters the broker itself found for that record. `assessment` is the scan's grade; it
/// steers the refusal and the grade after reading, never what is read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sealed {
    /// Hex of [`unlost_formats::ntfs::location::encode`].
    pub location: String,
    pub tag: String,
    pub assessment: Assessment,
}

pub fn hex(b: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(DIGITS[usize::from(x >> 4)] as char);
        s.push(DIGITS[usize::from(x & 15)] as char);
    }
    s
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let digit = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let (pairs, rest) = s.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|p| Some(digit(p[0])? << 4 | digit(p[1])?))
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub version: u32,
    pub elevated: bool,
    pub pid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSummary {
    pub scan: u32,
    pub source: SourceSpec,
    pub label: String,
    pub fs: FsKind,
    pub cluster_size: u32,
    pub volume_bytes: u64,
    pub files: u64,
    pub stats: ScanStats,
    pub counts: BTreeMap<State, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryView {
    pub record: u64,
    pub name: String,
    pub folder: String,
    pub path_status: PathStatus,
    pub size: u64,
    pub created: i64,
    pub modified: i64,
    pub ext: String,
    pub category: Category,
    pub assessment: Assessment,
    pub links: u16,
    pub streams: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub total: u64,
    pub rows: Vec<EntryView>,
    /// Per-state counts for the text and category filter, ignoring the state filter.
    pub counts: BTreeMap<State, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadResult {
    pub stats: unlost_formats::ntfs::ReadStats,
    pub assessment: Assessment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    pub id: u64,
    pub req: Request,
}

pub fn write_frame(w: &mut dyn Write, kind: u8, id: u64, payload: &[u8]) -> std::io::Result<()> {
    if payload.len() > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "frame too large",
        ));
    }
    let len = (1 + 8 + payload.len()) as u32;
    let mut head = [0u8; 13];
    head[0..4].copy_from_slice(&len.to_le_bytes());
    head[4] = kind;
    head[5..13].copy_from_slice(&id.to_le_bytes());
    w.write_all(&head)?;
    w.write_all(payload)?;
    w.flush()
}

/// Next frame, or `None` on a clean end of stream.
pub fn read_frame(r: &mut dyn Read) -> std::io::Result<Option<(u8, u64, Vec<u8>)>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    if !(9..=MAX_FRAME + 9).contains(&len) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame length {len}"),
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    let kind = body[0];
    if !(KIND_REQUEST..=KIND_ERR).contains(&kind) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame kind {kind}"),
        ));
    }
    let id = u64::from_le_bytes(body[1..9].try_into().unwrap_or_default());
    Ok(Some((kind, id, body.split_off(9))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_reject_garbage() {
        let mut buf = Vec::new();
        write_frame(&mut buf, KIND_DATA, 7, b"abc").unwrap();
        let req = serde_json::to_vec(&Envelope {
            id: 9,
            req: Request::Read {
                scan: 1,
                record: 42,
                sealed: None,
            },
        })
        .unwrap();
        write_frame(&mut buf, KIND_REQUEST, 9, &req).unwrap();
        let mut r = buf.as_slice();
        assert_eq!(
            read_frame(&mut r).unwrap(),
            Some((KIND_DATA, 7, b"abc".to_vec()))
        );
        let (k, id, p) = read_frame(&mut r).unwrap().unwrap();
        assert_eq!((k, id), (KIND_REQUEST, 9));
        let env: Envelope = serde_json::from_slice(&p).unwrap();
        assert_eq!(
            env.req,
            Request::Read {
                scan: 1,
                record: 42,
                sealed: None,
            }
        );
        assert_eq!(read_frame(&mut r).unwrap(), None);

        let huge = [0xFFu8, 0xFF, 0xFF, 0x7F, 1, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(read_frame(&mut huge.as_slice()).is_err());
        let bad_kind = [9u8, 0, 0, 0, 77, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(read_frame(&mut bad_kind.as_slice()).is_err());
        let truncated = [20u8, 0, 0, 0, 1, 0];
        assert!(read_frame(&mut truncated.as_slice()).is_err());
        assert!(write_frame(&mut Vec::new(), KIND_DATA, 1, &vec![0u8; MAX_FRAME + 1]).is_err());
    }

    #[test]
    fn requests_are_tagged_json() {
        let j = serde_json::to_string(&Request::Scan {
            source: SourceSpec::Volume {
                root: "D:\\".into(),
            },
            workspace: None,
        })
        .unwrap();
        assert_eq!(
            j,
            r#"{"op":"scan","source":{"kind":"volume","root":"D:\\"}}"#
        );
        let q: Query = serde_json::from_str(r#"{"text":"x","states":["likelyIntact"]}"#).unwrap();
        assert_eq!(q.states, vec![State::LikelyIntact]);
        assert_eq!(q.limit, 0);
        let r: Request = serde_json::from_str(r#"{"op":"read","scan":3,"record":9}"#).unwrap();
        assert_eq!(
            r,
            Request::Read {
                scan: 3,
                record: 9,
                sealed: None
            }
        );
    }

    #[test]
    fn hex_round_trips() {
        let b: Vec<u8> = (0..=255).collect();
        assert_eq!(unhex(&hex(&b)).unwrap(), b);
        assert_eq!(hex(&[0xAB, 0x01]), "ab01");
        assert!(unhex("abc").is_none());
        assert!(unhex("zz").is_none());
        assert_eq!(unhex("").unwrap(), Vec::<u8>::new());
    }
}
