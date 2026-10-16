//! MFT file records and their attributes.
//!
//! Layout from Microsoft's FILE_RECORD_SEGMENT_HEADER and ATTRIBUTE_RECORD_HEADER docs plus the
//! long-standing public NTFS documentation. Every read is bounds-checked; a malformed attribute
//! ends the walk instead of panicking.

use serde::{Deserialize, Serialize};

use crate::le;

pub const ATTR_STANDARD_INFORMATION: u32 = 0x10;
pub const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
pub const ATTR_FILE_NAME: u32 = 0x30;
pub const ATTR_VOLUME_NAME: u32 = 0x60;
pub const ATTR_DATA: u32 = 0x80;
pub const ATTR_REPARSE_POINT: u32 = 0xC0;
const ATTR_END: u32 = 0xFFFF_FFFF;

pub const RECORD_IN_USE: u16 = 0x0001;
pub const RECORD_DIRECTORY: u16 = 0x0002;

pub const ATTR_FLAG_COMPRESSION_MASK: u16 = 0x00FF;
pub const ATTR_FLAG_ENCRYPTED: u16 = 0x4000;
pub const ATTR_FLAG_SPARSE: u16 = 0x8000;

pub const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
pub const FILE_ATTRIBUTE_ENCRYPTED: u32 = 0x4000;
pub const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
pub const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

pub const REPARSE_TAG_WOF: u32 = 0x8000_0017;

/// Cloud Files placeholders use `IO_REPARSE_TAG_CLOUD` and its 16 numbered variants.
pub fn is_cloud_tag(tag: u32) -> bool {
    tag & 0xFFFF_0FFF == 0x9000_001A
}

/// Update sequence arrays protect every 512 bytes of a record, whatever the sector size.
const FIXUP_STRIDE: usize = 512;

/// Record number (48 bits) plus sequence number (16 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct FileRef {
    pub record: u64,
    pub sequence: u16,
}

impl FileRef {
    pub fn from_u64(v: u64) -> Self {
        FileRef {
            record: v & 0x0000_FFFF_FFFF_FFFF,
            sequence: (v >> 48) as u16,
        }
    }

    /// The on-disk form: record number in the low 48 bits, sequence above.
    pub fn to_u64(self) -> u64 {
        (self.record & 0x0000_FFFF_FFFF_FFFF) | (u64::from(self.sequence) << 48)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub sequence: u16,
    pub flags: u16,
    pub base: FileRef,
    pub first_attr: usize,
    pub used: usize,
}

impl Header {
    pub fn in_use(&self) -> bool {
        self.flags & RECORD_IN_USE != 0
    }
    pub fn is_dir(&self) -> bool {
        self.flags & RECORD_DIRECTORY != 0
    }
}

/// Header of a record that carries the `FILE` signature, or `None` (free slot, `BAAD`, junk).
pub fn header(b: &[u8]) -> Option<Header> {
    if b.get(0..4)? != b"FILE" {
        return None;
    }
    let first_attr = usize::from(le::u16(b, 0x14)?);
    let used = (le::u32(b, 0x18)? as usize).min(b.len());
    if first_attr < 0x18 || first_attr >= used {
        return None;
    }
    Some(Header {
        sequence: le::u16(b, 0x10)?,
        flags: le::u16(b, 0x16)?,
        base: FileRef::from_u64(le::u64(b, 0x20)?),
        first_attr,
        used,
    })
}

/// Applies the update sequence array. Returns `false` when a sector's check value does not
/// match: the record was torn by an interrupted write or partly overwritten.
pub fn apply_fixups(b: &mut [u8]) -> bool {
    let (Some(off), Some(count)) = (le::u16(b, 4), le::u16(b, 6)) else {
        return false;
    };
    let (off, count) = (usize::from(off), usize::from(count));
    if count < 2 || off.checked_add(count * 2).is_none_or(|e| e > b.len()) {
        return false;
    }
    let strides = (count - 1).min(b.len() / FIXUP_STRIDE);
    let check = [b[off], b[off + 1]];
    let mut clean = strides == count - 1;
    for i in 1..=strides {
        let pos = i * FIXUP_STRIDE - 2;
        if b[pos..pos + 2] != check {
            clean = false;
        }
        let saved = [b[off + i * 2], b[off + i * 2 + 1]];
        b[pos..pos + 2].copy_from_slice(&saved);
    }
    clean
}

pub struct Attr<'a> {
    pub kind: u32,
    pub flags: u16,
    pub name: String,
    pub body: Body<'a>,
}

pub enum Body<'a> {
    Resident(&'a [u8]),
    NonResident(NonResident<'a>),
}

pub struct NonResident<'a> {
    pub start_vcn: u64,
    pub last_vcn: u64,
    pub runlist: &'a [u8],
    pub allocated: u64,
    pub real: u64,
    pub initialized: u64,
}

/// Iterates the attributes of a record whose fixups were applied.
pub fn attributes<'a>(b: &'a [u8], h: &Header) -> Attributes<'a> {
    Attributes {
        b: &b[..h.used.min(b.len())],
        pos: h.first_attr,
        count: 0,
    }
}

pub struct Attributes<'a> {
    b: &'a [u8],
    pos: usize,
    count: usize,
}

impl<'a> Iterator for Attributes<'a> {
    type Item = Attr<'a>;

    fn next(&mut self) -> Option<Attr<'a>> {
        // 16 bytes is the smallest legal attribute; a record cannot hold more than this many.
        if self.count > self.b.len() / 16 {
            return None;
        }
        self.count += 1;
        let at = self.pos;
        let kind = le::u32(self.b, at)?;
        if kind == ATTR_END {
            return None;
        }
        let len = le::u32(self.b, at + 4)? as usize;
        if len < 0x18 || at.checked_add(len)? > self.b.len() {
            return None;
        }
        let a = &self.b[at..at + len];
        self.pos = at + len;
        let non_resident = le::u8(a, 8)? != 0;
        let name_len = usize::from(le::u8(a, 9)?);
        let name_off = usize::from(le::u16(a, 0x0A)?);
        let flags = le::u16(a, 0x0C)?;
        let name = if name_len == 0 {
            String::new()
        } else {
            le::utf16(a, name_off, name_len)?
        };
        let body = if non_resident {
            if len < 0x40 {
                return None;
            }
            let runs_off = usize::from(le::u16(a, 0x20)?);
            Body::NonResident(NonResident {
                start_vcn: le::u64(a, 0x10)?,
                last_vcn: le::u64(a, 0x18)?,
                runlist: a.get(runs_off..)?,
                allocated: le::u64(a, 0x28)?,
                real: le::u64(a, 0x30)?,
                initialized: le::u64(a, 0x38)?,
            })
        } else {
            let vlen = le::u32(a, 0x10)? as usize;
            let voff = usize::from(le::u16(a, 0x14)?);
            Body::Resident(a.get(voff..voff.checked_add(vlen)?)?)
        };
        Some(Attr {
            kind,
            flags,
            name,
            body,
        })
    }
}

#[derive(Debug, Clone)]
pub struct FileName {
    pub parent: FileRef,
    pub created: u64,
    pub modified: u64,
    pub mft_modified: u64,
    pub accessed: u64,
    pub real_size: u64,
    pub namespace: u8,
    pub name: String,
}

pub const NAMESPACE_POSIX: u8 = 0;
pub const NAMESPACE_WIN32: u8 = 1;
pub const NAMESPACE_DOS: u8 = 2;
pub const NAMESPACE_WIN32_DOS: u8 = 3;

impl FileName {
    pub fn parse(v: &[u8]) -> Option<Self> {
        let chars = usize::from(le::u8(v, 0x40)?);
        Some(FileName {
            parent: FileRef::from_u64(le::u64(v, 0)?),
            created: le::u64(v, 0x08)?,
            modified: le::u64(v, 0x10)?,
            mft_modified: le::u64(v, 0x18)?,
            accessed: le::u64(v, 0x20)?,
            real_size: le::u64(v, 0x30)?,
            namespace: le::u8(v, 0x41)?,
            name: le::utf16(v, 0x42, chars)?,
        })
    }

    /// Lower is better: long names first, 8.3 short names last.
    pub fn preference(&self) -> u8 {
        match self.namespace {
            NAMESPACE_WIN32 | NAMESPACE_WIN32_DOS => 0,
            NAMESPACE_POSIX => 1,
            _ => 2,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct StandardInfo {
    pub created: u64,
    pub modified: u64,
    pub mft_modified: u64,
    pub accessed: u64,
    pub attributes: u32,
}

impl StandardInfo {
    pub fn parse(v: &[u8]) -> Option<Self> {
        Some(StandardInfo {
            created: le::u64(v, 0)?,
            modified: le::u64(v, 0x08)?,
            mft_modified: le::u64(v, 0x10)?,
            accessed: le::u64(v, 0x18)?,
            attributes: le::u32(v, 0x20)?,
        })
    }
}

/// One `$ATTRIBUTE_LIST` entry: which record holds a piece of an attribute.
#[derive(Debug, Clone)]
pub struct ListEntry {
    pub kind: u32,
    pub start_vcn: u64,
    pub record: FileRef,
    pub name: String,
}

pub fn parse_attribute_list(v: &[u8]) -> Vec<ListEntry> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 0x1A <= v.len() && out.len() < 4096 {
        let (Some(kind), Some(len)) = (le::u32(v, pos), le::u16(v, pos + 4)) else {
            break;
        };
        let len = usize::from(len);
        if len < 0x1A || pos + len > v.len() {
            break;
        }
        let name_len = usize::from(le::u8(v, pos + 6).unwrap_or(0));
        let name_off = usize::from(le::u8(v, pos + 7).unwrap_or(0));
        let name = if name_len == 0 {
            String::new()
        } else {
            le::utf16(&v[pos..pos + len], name_off, name_len).unwrap_or_default()
        };
        out.push(ListEntry {
            kind,
            start_vcn: le::u64(v, pos + 8).unwrap_or(0),
            record: FileRef::from_u64(le::u64(v, pos + 0x10).unwrap_or(0)),
            name,
        });
        pos += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixups_detect_torn_sectors() {
        let mut b = vec![0u8; 1024];
        b[0..4].copy_from_slice(b"FILE");
        b[4..6].copy_from_slice(&0x30u16.to_le_bytes());
        b[6..8].copy_from_slice(&3u16.to_le_bytes());
        b[0x30..0x32].copy_from_slice(&[0xAB, 0xCD]);
        b[0x32..0x34].copy_from_slice(&[1, 2]);
        b[0x34..0x36].copy_from_slice(&[3, 4]);
        b[510..512].copy_from_slice(&[0xAB, 0xCD]);
        b[1022..1024].copy_from_slice(&[0xAB, 0xCD]);
        let mut clean = b.clone();
        assert!(apply_fixups(&mut clean));
        assert_eq!(&clean[510..512], &[1, 2]);
        assert_eq!(&clean[1022..1024], &[3, 4]);
        let mut torn = b.clone();
        torn[1022] = 0;
        assert!(!apply_fixups(&mut torn));
        let mut silly = b;
        silly[6..8].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(!apply_fixups(&mut silly));
    }

    #[test]
    fn cloud_tags() {
        assert!(is_cloud_tag(0x9000_001A));
        assert!(is_cloud_tag(0x9000_F01A));
        assert!(!is_cloud_tag(REPARSE_TAG_WOF));
    }
}
