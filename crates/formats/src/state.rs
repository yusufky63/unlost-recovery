//! Per-file recoverability: a state plus the evidence behind it. There is no percentage and no
//! "100%": every state names what was actually checked.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    /// A complete copy kept by Windows: Recycle Bin, shadow copy, autosave.
    IntactCopy,
    /// A validator decoded the whole file. Only ever set by a format validator.
    Verified,
    /// Metadata found, no cluster reused, sampled content present. Not verified.
    LikelyIntact,
    /// Some clusters reused, header mismatch, or only part could be read.
    Partial,
    /// Clusters free but reading as zeros, typical after TRIM on an SSD.
    Zeroed,
    /// Nearly all clusters now belong to other files.
    Overwritten,
    /// A name and dates, but no usable location for the content.
    MetadataOnly,
    /// Encrypted, cloud-only or a storage format this version cannot read.
    NotRecoverableHere,
}

impl State {
    /// Sort rank: best first.
    pub fn rank(self) -> u8 {
        self as u8
    }

    pub fn label(self) -> &'static str {
        match self {
            State::IntactCopy => "Intact copy",
            State::Verified => "Verified",
            State::LikelyIntact => "Likely intact",
            State::Partial => "Partial",
            State::Zeroed => "Zeroed",
            State::Overwritten => "Overwritten",
            State::MetadataOnly => "Metadata only",
            State::NotRecoverableHere => "Not recoverable here",
        }
    }
}

/// Why a file got its state. Serialized as `{ "code": "...", ...params }` so the UI can render
/// a translated sentence with the numbers filled in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "code",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Reason {
    RecycleBin,
    ShadowCopy {
        created: i64,
    },
    /// Data stored inside the MFT record, which has not been reused.
    ResidentIntact,
    EmptyFile,
    /// All clusters are free; sampled clusters contain data.
    ClustersFree {
        clusters: u64,
        sampled: u32,
    },
    /// All clusters are free; content was not sampled (too many files or not requested).
    ClustersFreeUnsampled {
        clusters: u64,
    },
    ClustersReused {
        reused: u64,
        total: u64,
    },
    AllZero {
        sampled: u32,
    },
    HeaderMismatch {
        expected: String,
    },
    /// The record keeps a name and size but no data runs.
    NoRuns,
    TornRecord,
    RunsOutsideVolume,
    /// The record claims a size far beyond the volume; it is damaged.
    SizeImplausible,
    /// Part of the file's attributes lived in an extension record that is gone.
    ExtensionMissing,
    Encrypted,
    Compressed,
    WofCompressed,
    CloudOnly,
    /// Read result: some bytes could not be read from the device.
    Unreadable {
        bytes: u64,
    },
    /// Read result: this many clusters of the recovered content are all zeros.
    SomeZero {
        zero: u64,
        total: u64,
    },
}

impl Reason {
    /// English sentence for reports. The UI renders its own translated text from the code.
    pub fn english(&self) -> String {
        match self {
            Reason::RecycleBin => "Complete copy from the Recycle Bin.".into(),
            Reason::ShadowCopy { .. } => "Complete copy from a shadow copy (Previous Versions).".into(),
            Reason::ResidentIntact => {
                "Small file stored inside its MFT record; the record has not been reused.".into()
            }
            Reason::EmptyFile => "The file was empty.".into(),
            Reason::ClustersFree { clusters, sampled } => format!(
                "All {clusters} clusters are free and {sampled} sampled clusters contain data. Not verified."
            ),
            Reason::ClustersFreeUnsampled { clusters } => {
                format!("All {clusters} clusters are free. Content not sampled yet.")
            }
            Reason::ClustersReused { reused, total } => {
                format!("{reused} of {total} clusters now belong to other files.")
            }
            Reason::AllZero { sampled } => format!(
                "All {sampled} sampled clusters read as zeros; the SSD probably erased them (TRIM)."
            ),
            Reason::HeaderMismatch { expected } => {
                format!("The start of the file does not look like {expected}; it may be overwritten.")
            }
            Reason::NoRuns => "The record keeps the name and size but not where the data was.".into(),
            Reason::TornRecord => "The file record is damaged.".into(),
            Reason::RunsOutsideVolume => "The record points outside the volume.".into(),
            Reason::SizeImplausible => {
                "The record claims a size far larger than the volume; it is damaged.".into()
            }
            Reason::ExtensionMissing => {
                "Part of the file's record was stored elsewhere and is gone.".into()
            }
            Reason::Encrypted => "Encrypted with EFS; the content cannot be used without the key.".into(),
            Reason::Compressed => "NTFS-compressed; this version cannot decompress it yet.".into(),
            Reason::WofCompressed => {
                "Compressed by Windows (CompactOS); this version cannot read it yet.".into()
            }
            Reason::CloudOnly => {
                "Online-only cloud file; its content was never on this disk. Check the cloud recycle bin."
                    .into()
            }
            Reason::Unreadable { bytes } => {
                format!("{bytes} bytes could not be read from the device and were filled with zeros.")
            }
            Reason::SomeZero { zero, total } => {
                format!("{zero} of {total} clusters read as zeros.")
            }
        }
    }

    pub fn code(&self) -> String {
        serde_json_code(self)
    }
}

/// The serde tag without pulling serde_json into this crate.
fn serde_json_code(r: &Reason) -> String {
    let name = match r {
        Reason::RecycleBin => "recycleBin",
        Reason::ShadowCopy { .. } => "shadowCopy",
        Reason::ResidentIntact => "residentIntact",
        Reason::EmptyFile => "emptyFile",
        Reason::ClustersFree { .. } => "clustersFree",
        Reason::ClustersFreeUnsampled { .. } => "clustersFreeUnsampled",
        Reason::ClustersReused { .. } => "clustersReused",
        Reason::AllZero { .. } => "allZero",
        Reason::HeaderMismatch { .. } => "headerMismatch",
        Reason::NoRuns => "noRuns",
        Reason::TornRecord => "tornRecord",
        Reason::RunsOutsideVolume => "runsOutsideVolume",
        Reason::SizeImplausible => "sizeImplausible",
        Reason::ExtensionMissing => "extensionMissing",
        Reason::Encrypted => "encrypted",
        Reason::Compressed => "compressed",
        Reason::WofCompressed => "wofCompressed",
        Reason::CloudOnly => "cloudOnly",
        Reason::Unreadable { .. } => "unreadable",
        Reason::SomeZero { .. } => "someZero",
    };
    name.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assessment {
    pub state: State,
    pub reason: Reason,
}

impl Assessment {
    pub fn new(state: State, reason: Reason) -> Self {
        Assessment { state, reason }
    }
}
