//! NTFS deleted-file discovery and copy-out, read-only.
//!
//! The scanner reads the whole `$MFT` and hands each deleted file record (in-use flag cleared,
//! name still present) to a sink as soon as it is graded against `$Bitmap`, keeping only a
//! compact folder map for path rebuilding. A few sampled clusters refine the grades afterwards.

pub mod boot;
#[cfg(any(test, feature = "test-util"))]
pub mod build;
pub mod location;
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
pub use scan::{
    DataLocation, DeletedFile, Folder, NonResidentData, PathStatus, ScanOptions, ScanResult,
    ScanSink, ScanStats, scan, scan_all,
};
pub use volume::{Bitmap, NtfsVolume};

/// Root folder record number.
pub const ROOT_RECORD: u64 = 5;
