//! NTFS deleted-file discovery and copy-out, read-only.
//!
//! The scanner streams the whole `$MFT`, keeps deleted file records (in-use flag cleared, name
//! still present) plus a compact folder map for path rebuilding, then grades each file against
//! the current `$Bitmap` and a few sampled clusters.

pub mod boot;
#[cfg(any(test, feature = "test-util"))]
pub mod build;
pub mod read;
pub mod record;
pub mod runlist;
pub mod scan;
#[cfg(test)]
mod tests;
pub mod volume;

pub use boot::BootSector;
pub use read::{ReadStats, after_read, read_content};
pub use record::FileRef;
pub use runlist::Run;
pub use scan::{DataLocation, DeletedFile, PathStatus, ScanOptions, ScanResult, ScanStats, scan};
pub use volume::{Bitmap, NtfsVolume};

/// Root folder record number.
pub const ROOT_RECORD: u64 = 5;
