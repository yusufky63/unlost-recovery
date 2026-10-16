//! Unlost parsers.
//!
//! Everything here reads bytes that may be hostile (a crafted USB stick, a damaged disk) while
//! the caller may be running elevated, so the crate forbids `unsafe`, bounds-checks every offset
//! and caps every allocation. Nothing in this crate can write: the only I/O abstraction,
//! [`SectorSource`], has no write method. Platform code (raw device handles) lives in
//! `unlost-core`; this crate is portable so its parsers can be fuzzed on Linux.

#![forbid(unsafe_code)]

pub mod crc32c;
pub mod error;
pub mod filetime;
pub mod findings;
pub mod fuzzing;
pub mod guid;
pub mod magic;
pub mod ntfs;
pub mod partition;
pub mod progress;
pub mod recyclebin;
pub mod source;
pub mod state;
pub mod vhdx;

pub use error::{Error, Result};
pub use progress::{NoProgress, Progress};
pub use source::{FileImage, MemSource, SectorSource, SliceSource};
pub use state::{Assessment, Reason, State};

/// Little-endian readers that return `None` instead of panicking on short input.
pub(crate) mod le {
    pub fn u16(b: &[u8], off: usize) -> Option<u16> {
        Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?))
    }
    pub fn u32(b: &[u8], off: usize) -> Option<u32> {
        Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
    }
    pub fn u64(b: &[u8], off: usize) -> Option<u64> {
        Some(u64::from_le_bytes(b.get(off..off + 8)?.try_into().ok()?))
    }
    pub fn u8(b: &[u8], off: usize) -> Option<u8> {
        b.get(off).copied()
    }

    /// UTF-16LE text of `chars` code units at `off`, cut at the first NUL.
    pub fn utf16(b: &[u8], off: usize, chars: usize) -> Option<String> {
        let bytes = b.get(off..off.checked_add(chars.checked_mul(2)?)?)?;
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|&u| u != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    }
}
