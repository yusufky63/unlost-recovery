use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::magic;
use crate::ntfs::location;
use crate::ntfs::record::{
    ATTR_ATTRIBUTE_LIST, ATTR_DATA, ATTR_FILE_NAME, ATTR_FLAG_COMPRESSION_MASK,
    ATTR_FLAG_ENCRYPTED, ATTR_REPARSE_POINT, ATTR_STANDARD_INFORMATION, Body,
    FILE_ATTRIBUTE_ENCRYPTED, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
    FILE_ATTRIBUTE_RECALL_ON_OPEN, FileName, FileRef, NAMESPACE_DOS, REPARSE_TAG_WOF, StandardInfo,
    apply_fixups, attributes, header, is_cloud_tag,
};
use crate::ntfs::runlist::Run;
use crate::ntfs::volume::{Bitmap, NtfsVolume, Piece, piece_of};
use crate::ntfs::{EXTEND_RECORD, ROOT_RECORD};
use crate::source::SectorSource;
use crate::state::{Assessment, Reason, State};
use crate::{Error, Progress, Result, filetime};

/// Bytes of `$MFT` read per I/O.
const CHUNK: usize = 4 << 20;
/// Deepest folder chain followed when rebuilding a path.
const MAX_DEPTH: usize = 1024;
/// A sparse file may be larger than its volume, but not by this much.
pub const MAX_SIZE_FACTOR: u64 = 16;
/// Bytes sampled from each sampled cluster.
const SAMPLE_BYTES: usize = 4096;
/// Deleted files with an attribute list wait for their extension records until the `$MFT`
/// pass ends; these bound what they and the extension records may hold meanwhile. Beyond
/// them a file is graded with what its own record has.
const MAX_HELD: usize = 65_536;
const MAX_HELD_BYTES: usize = 64 << 20;

#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Deleted files reported; the scan sets `truncated` beyond this. Files stream out as
    /// they are found, so the default is no limit.
    pub max_files: usize,
    /// Cluster reads spent on content sampling.
    pub max_samples: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            max_files: usize::MAX,
            max_samples: 300_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PathStatus {
    /// Every folder on the path still exists.
    Live,
    /// Some folder on the path was deleted too, but the chain is intact.
    DeletedParent,
    /// The chain is broken (folder record reused or gone); the file sits under "Lost folder".
    Lost,
}

#[derive(Debug, Clone, Default)]
pub enum DataLocation {
    #[default]
    Missing,
    Resident(Vec<u8>),
    NonResident(NonResidentData),
}

#[derive(Debug, Clone)]
pub struct NonResidentData {
    pub runs: Vec<Run>,
    pub real: u64,
    pub initialized: u64,
    pub compressed: bool,
}

#[derive(Debug, Clone)]
pub struct DeletedFile {
    pub record: u64,
    pub sequence: u16,
    pub name: String,
    /// The parent folder's NTFS reference ([`FileRef::to_u64`]). Its path arrives separately,
    /// through [`ScanSink::folder`] with this key, once the whole `$MFT` has been read.
    pub folder: u64,
    /// Number of names (hard links) the record had.
    pub links: u16,
    pub size: u64,
    pub created: i64,
    pub modified: i64,
    pub accessed: i64,
    pub changed: i64,
    pub attributes: u32,
    /// Named data streams (alternate data streams) left in the record.
    pub streams: u32,
    pub assessment: Assessment,
    pub data: DataLocation,
}

impl DeletedFile {
    /// First cluster of the content, for sorting reads.
    pub fn first_lcn(&self) -> Option<u64> {
        match &self.data {
            DataLocation::NonResident(nr) => nr.runs.iter().find_map(|r| r.lcn),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStats {
    pub records: u64,
    pub in_use: u64,
    pub deleted_records: u64,
    pub deleted_folders: u64,
    pub torn: u64,
    pub sampled: u64,
    pub truncated: bool,
    pub bitmap_ok: bool,
}

/// Where the scan's findings go, as they are found. Nothing about a file is kept once it has
/// been handed over, so a scan's memory does not grow with the number of deleted files.
pub trait ScanSink {
    /// A deleted file, graded against `$Bitmap`.
    fn file(&mut self, f: DeletedFile) -> Result<()>;
    /// The path of a folder that files were reported in, keyed by [`DeletedFile::folder`].
    /// Sent once per key, after all files.
    fn folder(&mut self, key: u64, path: &str, status: PathStatus) -> Result<()>;
    /// A new grade for a file already reported, after its content was sampled.
    fn regrade(&mut self, record: u64, assessment: &Assessment) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct Folder {
    pub path: String,
    pub status: PathStatus,
}

/// Everything a scan found, in memory: for tests, fuzzing and tools. The app streams instead.
#[derive(Default)]
pub struct ScanResult {
    pub label: String,
    pub files: Vec<DeletedFile>,
    pub folders: HashMap<u64, Folder>,
    pub stats: ScanStats,
    index: HashMap<u64, usize>,
}

impl ScanResult {
    /// Full path inside the volume, starting with `\`.
    pub fn path(&self, f: &DeletedFile) -> String {
        match self.folders.get(&f.folder) {
            Some(d) if d.path == "\\" => format!("\\{}", f.name),
            Some(d) => format!("{}\\{}", d.path, f.name),
            None => format!("\\{}", f.name),
        }
    }

    pub fn status(&self, f: &DeletedFile) -> Option<PathStatus> {
        self.folders.get(&f.folder).map(|d| d.status)
    }
}

impl ScanSink for ScanResult {
    fn file(&mut self, f: DeletedFile) -> Result<()> {
        self.index.insert(f.record, self.files.len());
        self.files.push(f);
        Ok(())
    }
    fn folder(&mut self, key: u64, path: &str, status: PathStatus) -> Result<()> {
        self.folders.insert(
            key,
            Folder {
                path: path.to_string(),
                status,
            },
        );
        Ok(())
    }
    fn regrade(&mut self, record: u64, assessment: &Assessment) -> Result<()> {
        if let Some(&i) = self.index.get(&record) {
            self.files[i].assessment = assessment.clone();
        }
        Ok(())
    }
}

/// [`scan`] into memory.
pub fn scan_all<S: SectorSource>(
    vol: &NtfsVolume<S>,
    opts: &ScanOptions,
    progress: &dyn Progress,
) -> Result<ScanResult> {
    let mut res = ScanResult {
        label: vol.label(),
        ..ScanResult::default()
    };
    res.stats = scan(vol, opts, progress, &mut res)?;
    Ok(res)
}

struct Dir {
    sequence: u16,
    in_use: bool,
    parent: FileRef,
    name: String,
}

#[derive(Default)]
struct Candidate {
    record: u64,
    sequence: u16,
    names: Vec<FileName>,
    info: Option<StandardInfo>,
    pieces: Vec<Piece>,
    streams: u32,
    reparse: Option<u32>,
    has_list: bool,
    torn: bool,
}

struct Extension {
    base: FileRef,
    names: Vec<FileName>,
    pieces: Vec<Piece>,
}

/// Rough heap bytes of names and pieces, for the [`MAX_HELD_BYTES`] budget.
fn held_cost(names: &[FileName], pieces: &[Piece]) -> usize {
    let n: usize = names.iter().map(|n| n.name.len() + 64).sum();
    let p: usize = pieces
        .iter()
        .map(|p| p.runs.len() * size_of::<Run>() + p.resident.as_ref().map_or(0, Vec::len) + 64)
        .sum();
    n + p
}

/// Lists deleted files on the volume, grades each one, and streams them to `sink`.
///
/// Order: files as the `$MFT` is read (those with an attribute list at the end, once their
/// extension records are known), then regrades from content sampling, then folder paths.
pub fn scan<S: SectorSource>(
    vol: &NtfsVolume<S>,
    opts: &ScanOptions,
    progress: &dyn Progress,
    sink: &mut dyn ScanSink,
) -> Result<ScanStats> {
    let rs = vol.boot.record_size as usize;
    let total_clusters = vol.boot.total_clusters;
    let mut stats = ScanStats {
        records: vol.record_count,
        ..ScanStats::default()
    };
    // Files are graded as they stream out, so `$Bitmap` is read first.
    let bitmap = vol.bitmap();
    stats.bitmap_ok = bitmap.is_ok();
    let mut out = Emitter {
        sink,
        bitmap: bitmap.as_ref().ok(),
        cluster: vol.cluster_size(),
        volume_bytes: vol.volume_bytes(),
        parents: HashSet::new(),
        sampler: Sampler::new(opts.max_samples),
        emitted: 0,
    };
    let mut dirs: HashMap<u64, Dir> = HashMap::new();
    let mut held: Vec<Candidate> = Vec::new();
    let mut exts: Vec<Extension> = Vec::new();
    let mut held_bytes = 0usize;

    progress.stage("mft");
    progress.set_total(vol.record_count);
    let per_chunk = (CHUNK / rs).max(1) as u64;
    let mut buf = vec![0u8; per_chunk as usize * rs];
    let mut first = 0u64;
    while first < vol.record_count {
        if progress.cancelled() {
            return Err(Error::Cancelled);
        }
        let n = per_chunk.min(vol.record_count - first);
        let chunk = &mut buf[..n as usize * rs];
        if let Err(e) = vol.read_records(first, chunk) {
            // One unreadable stretch of the MFT should not end the scan: skip it, count it.
            if first == 0 || !matches!(e, Error::Read { .. }) {
                return Err(e);
            }
            stats.torn += n;
            first += n;
            progress.advance(n);
            continue;
        }
        for (i, rec) in chunk.chunks_exact_mut(rs).enumerate() {
            let number = first + i as u64;
            let Some(h) = header(rec) else { continue };
            let clean = apply_fixups(rec);
            if h.in_use() {
                stats.in_use += 1;
                if h.is_dir()
                    && h.base.record == 0
                    && let Some(name) = best_name(rec, &h)
                {
                    dirs.insert(
                        number,
                        Dir {
                            sequence: h.sequence,
                            in_use: true,
                            parent: name.parent,
                            name: name.name,
                        },
                    );
                }
                continue;
            }
            if h.base.record != 0 {
                if clean {
                    let (names, pieces) = names_and_data(rec, &h, total_clusters);
                    let cost = held_cost(&names, &pieces);
                    let room =
                        held.len() + exts.len() < MAX_HELD && held_bytes + cost <= MAX_HELD_BYTES;
                    if room && (!names.is_empty() || !pieces.is_empty()) {
                        held_bytes += cost;
                        exts.push(Extension {
                            base: h.base,
                            names,
                            pieces,
                        });
                    }
                }
                continue;
            }
            let mut c = Candidate {
                record: number,
                sequence: h.sequence,
                torn: !clean,
                ..Candidate::default()
            };
            for a in attributes(rec, &h) {
                match (a.kind, &a.body) {
                    (ATTR_FILE_NAME, Body::Resident(v)) => {
                        if let Some(f) = FileName::parse(v) {
                            c.names.push(f);
                        }
                    }
                    (ATTR_STANDARD_INFORMATION, Body::Resident(v)) => {
                        c.info = StandardInfo::parse(v);
                    }
                    (ATTR_DATA, _) if a.name.is_empty() => {
                        c.pieces.push(piece_of(&a, total_clusters))
                    }
                    (ATTR_DATA, _) => c.streams += 1,
                    (ATTR_REPARSE_POINT, Body::Resident(v)) => {
                        c.reparse = crate::le::u32(v, 0);
                    }
                    (ATTR_ATTRIBUTE_LIST, _) => c.has_list = true,
                    _ => {}
                }
            }
            if c.names.is_empty() {
                continue;
            }
            // 8.3 short names only matter when no long name survived.
            if c.names.iter().any(|n| n.namespace != NAMESPACE_DOS) {
                c.names.retain(|n| n.namespace != NAMESPACE_DOS);
            }
            c.names.truncate(8);
            stats.deleted_records += 1;
            if h.is_dir() {
                stats.deleted_folders += 1;
                if let Some(best) = pick_name(&c.names) {
                    dirs.insert(
                        number,
                        Dir {
                            sequence: h.sequence,
                            in_use: false,
                            parent: best.parent,
                            name: best.name.clone(),
                        },
                    );
                }
                continue;
            }
            if c.torn {
                stats.torn += 1;
            }
            if out.emitted + held.len() >= opts.max_files {
                stats.truncated = true;
                continue;
            }
            progress.found(1);
            if c.has_list {
                let cost = held_cost(&c.names, &c.pieces);
                if held.len() + exts.len() < MAX_HELD && held_bytes + cost <= MAX_HELD_BYTES {
                    held_bytes += cost;
                    held.push(c);
                    continue;
                }
            }
            out.file(c)?;
        }
        first += n;
        progress.advance(n);
    }

    merge_extensions(&mut held, exts);
    for c in held {
        out.file(c)?;
    }

    progress.stage("sample");
    stats.sampled = out.sampler.run(vol, progress, out.sink)?;

    progress.stage("paths");
    progress.set_total(out.parents.len() as u64);
    for (i, key) in out.parents.iter().enumerate() {
        if i % 4096 == 0 && progress.cancelled() {
            return Err(Error::Cancelled);
        }
        let (path, status) = folder_path(FileRef::from_u64(*key), &dirs);
        out.sink.folder(*key, &path, status)?;
        progress.advance(1);
    }
    Ok(stats)
}

/// Grades candidates and hands them to the sink, remembering only which folders they
/// reference and which clusters to sample.
struct Emitter<'a> {
    sink: &'a mut dyn ScanSink,
    bitmap: Option<&'a Bitmap>,
    cluster: u64,
    volume_bytes: u64,
    parents: HashSet<u64>,
    sampler: Sampler,
    emitted: usize,
}

impl Emitter<'_> {
    fn file(&mut self, c: Candidate) -> Result<()> {
        let Some(best) = pick_name(&c.names).cloned() else {
            return Ok(());
        };
        let links = c
            .names
            .iter()
            .filter(|n| n.namespace != NAMESPACE_DOS)
            .map(|n| (n.parent, n.name.as_str()))
            .collect::<HashSet<_>>()
            .len()
            .max(1) as u16;
        let (data, has_unnamed, cut) = data_location(&c.pieces);
        let size = match &data {
            DataLocation::Resident(v) => v.len() as u64,
            DataLocation::NonResident(nr) => nr.real,
            DataLocation::Missing => best.real_size,
        };
        let times = c.info.as_ref().map_or(
            (
                best.created,
                best.modified,
                best.accessed,
                best.mft_modified,
            ),
            |i| (i.created, i.modified, i.accessed, i.mft_modified),
        );
        let attributes = c.info.as_ref().map_or(0, |i| i.attributes);
        let assessment = assess(
            &c,
            &data,
            has_unnamed,
            cut,
            attributes,
            self.bitmap,
            self.cluster,
            self.volume_bytes,
        );
        let folder = best.parent.to_u64();
        self.parents.insert(folder);
        self.sampler
            .offer(c.record, &best.name, &assessment, &data, self.cluster);
        self.emitted += 1;
        self.sink.file(DeletedFile {
            record: c.record,
            sequence: c.sequence,
            name: best.name,
            folder,
            links,
            size,
            created: filetime::to_unix(times.0),
            modified: filetime::to_unix(times.1),
            accessed: filetime::to_unix(times.2),
            changed: filetime::to_unix(times.3),
            attributes,
            streams: c.streams,
            assessment,
            data,
        })
    }
}

fn best_name(rec: &[u8], h: &crate::ntfs::record::Header) -> Option<FileName> {
    let names: Vec<FileName> = attributes(rec, h)
        .filter(|a| a.kind == ATTR_FILE_NAME)
        .filter_map(|a| match a.body {
            Body::Resident(v) => FileName::parse(v),
            Body::NonResident(_) => None,
        })
        .collect();
    pick_name(&names).cloned()
}

fn pick_name(names: &[FileName]) -> Option<&FileName> {
    names.iter().min_by_key(|n| n.preference())
}

fn names_and_data(
    rec: &[u8],
    h: &crate::ntfs::record::Header,
    total_clusters: u64,
) -> (Vec<FileName>, Vec<Piece>) {
    let mut names = Vec::new();
    let mut pieces = Vec::new();
    for a in attributes(rec, h) {
        match (a.kind, &a.body) {
            (ATTR_FILE_NAME, Body::Resident(v)) => names.extend(FileName::parse(v)),
            (ATTR_DATA, _) if a.name.is_empty() => pieces.push(piece_of(&a, total_clusters)),
            _ => {}
        }
    }
    (names, pieces)
}

/// Joins pieces kept in deleted extension records to their deleted base records. The base
/// record's sequence number was bumped when it was freed, so both values match.
fn merge_extensions(cands: &mut [Candidate], exts: Vec<Extension>) {
    if exts.is_empty() {
        return;
    }
    let index: HashMap<u64, usize> = cands
        .iter()
        .enumerate()
        .map(|(i, c)| (c.record, i))
        .collect();
    for e in exts {
        let Some(&i) = index.get(&e.base.record) else {
            continue;
        };
        let c = &mut cands[i];
        if e.base.sequence == c.sequence || e.base.sequence.wrapping_add(1) == c.sequence {
            c.names.extend(e.names);
            c.pieces.extend(e.pieces);
        }
    }
}

/// (location, has unnamed data, original run count when the map had to be cut).
fn data_location(pieces: &[Piece]) -> (DataLocation, bool, Option<u64>) {
    if pieces.is_empty() {
        return (DataLocation::Missing, false, None);
    }
    if let Some(p) = pieces.iter().find(|p| p.resident.is_some()) {
        return (
            DataLocation::Resident(p.resident.clone().unwrap_or_default()),
            true,
            None,
        );
    }
    let Some(first) = pieces.iter().find(|p| p.start_vcn == 0) else {
        return (DataLocation::Missing, true, None);
    };
    let mut sorted: Vec<&Piece> = pieces.iter().filter(|p| p.runs_ok).collect();
    sorted.sort_by_key(|p| p.start_vcn);
    let mut runs: Vec<Run> = sorted.iter().flat_map(|p| p.runs.iter().copied()).collect();
    runs.sort_by_key(|r| r.vcn);
    runs.dedup_by_key(|r| r.vcn);
    let cut = location::fit(&mut runs);
    (
        DataLocation::NonResident(NonResidentData {
            runs,
            real: first.real,
            initialized: first.initialized,
            compressed: pieces
                .iter()
                .any(|p| p.flags & ATTR_FLAG_COMPRESSION_MASK != 0),
        }),
        true,
        cut,
    )
}

/// Folder path of `parent`, following references up to the root.
fn folder_path(parent: FileRef, dirs: &HashMap<u64, Dir>) -> (String, PathStatus) {
    let mut names: Vec<&str> = Vec::new();
    let mut seen: HashSet<u64> = HashSet::new();
    let mut cur = parent;
    let mut below: Option<u64> = None;
    let mut status = PathStatus::Live;
    let lost = |names: &[&str], record: u64| {
        let mut p = format!("\\Lost folder #{record}");
        for n in names.iter().rev() {
            p.push('\\');
            p.push_str(n);
        }
        (p, PathStatus::Lost)
    };
    loop {
        if cur.record == ROOT_RECORD {
            break;
        }
        if names.len() >= MAX_DEPTH || !seen.insert(cur.record) {
            return lost(&names, cur.record);
        }
        let Some(d) = dirs.get(&cur.record) else {
            return lost(&names, cur.record);
        };
        // A freed record has its sequence number bumped, so a deleted parent matches either.
        let same = if d.in_use {
            d.sequence == cur.sequence
        } else {
            d.sequence == cur.sequence || d.sequence == cur.sequence.wrapping_add(1)
        };
        if !same {
            return lost(&names, cur.record);
        }
        // Windows deletes open folders with POSIX semantics: the folder is renamed into
        // `\$Extend\$Deleted` under its file id, so its own name and place are gone. Report it
        // as lost instead of showing that internal path as if it were real.
        if d.parent.record == EXTEND_RECORD && d.name.eq_ignore_ascii_case("$Deleted") {
            return match below {
                Some(renamed) => {
                    names.pop();
                    lost(&names, renamed)
                }
                None => lost(&names, cur.record),
            };
        }
        if !d.in_use {
            status = PathStatus::DeletedParent;
        }
        names.push(&d.name);
        below = Some(cur.record);
        cur = d.parent;
    }
    if names.is_empty() {
        return ("\\".into(), status);
    }
    let mut p = String::new();
    for n in names.iter().rev() {
        p.push('\\');
        p.push_str(n);
    }
    (p, status)
}

#[allow(clippy::too_many_arguments)]
fn assess(
    c: &Candidate,
    data: &DataLocation,
    has_unnamed: bool,
    cut: Option<u64>,
    attributes: u32,
    bitmap: Option<&Bitmap>,
    cluster: u64,
    volume_bytes: u64,
) -> Assessment {
    use Reason as R;
    use State as S;
    if c.torn {
        return Assessment::new(S::MetadataOnly, R::TornRecord);
    }
    let encrypted = attributes & FILE_ATTRIBUTE_ENCRYPTED != 0
        || c.pieces.iter().any(|p| p.flags & ATTR_FLAG_ENCRYPTED != 0);
    if encrypted {
        return Assessment::new(S::NotRecoverableHere, R::Encrypted);
    }
    if c.reparse == Some(REPARSE_TAG_WOF) {
        return Assessment::new(S::NotRecoverableHere, R::WofCompressed);
    }
    if c.pieces.iter().any(|p| !p.runs_ok) {
        return Assessment::new(S::MetadataOnly, R::RunsOutsideVolume);
    }
    let cloud = c.reparse.is_some_and(is_cloud_tag)
        || attributes
            & (FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
                | FILE_ATTRIBUTE_RECALL_ON_OPEN
                | FILE_ATTRIBUTE_OFFLINE)
            != 0;
    match data {
        DataLocation::Missing => {
            let reason = if c.has_list || has_unnamed {
                R::ExtensionMissing
            } else {
                R::NoRuns
            };
            Assessment::new(S::MetadataOnly, reason)
        }
        DataLocation::Resident(v) if v.is_empty() => Assessment::new(S::LikelyIntact, R::EmptyFile),
        DataLocation::Resident(_) => Assessment::new(S::LikelyIntact, R::ResidentIntact),
        DataLocation::NonResident(nr) => {
            if nr.compressed {
                return Assessment::new(S::NotRecoverableHere, R::Compressed);
            }
            if nr.real == 0 {
                return Assessment::new(S::LikelyIntact, R::EmptyFile);
            }
            if nr.real > volume_bytes.saturating_mul(MAX_SIZE_FACTOR) {
                return Assessment::new(S::MetadataOnly, R::SizeImplausible);
            }
            if let Some(runs) = cut {
                return Assessment::new(S::Partial, R::TooFragmented { runs });
            }
            let needed = nr.real.div_ceil(cluster.max(1));
            let (mapped, covered) = mapped_clusters(&nr.runs, needed);
            if mapped == 0 {
                if cloud {
                    return Assessment::new(S::NotRecoverableHere, R::CloudOnly);
                }
                return Assessment::new(S::MetadataOnly, R::NoRuns);
            }
            if covered < needed {
                let reason = if c.has_list {
                    R::ExtensionMissing
                } else {
                    R::NoRuns
                };
                return Assessment::new(S::Partial, reason);
            }
            let Some(bm) = bitmap else {
                return Assessment::new(
                    S::LikelyIntact,
                    R::ClustersFreeUnsampled { clusters: mapped },
                );
            };
            let reused = reused_clusters(bm, &nr.runs, needed);
            if reused == 0 {
                Assessment::new(
                    S::LikelyIntact,
                    R::ClustersFreeUnsampled { clusters: mapped },
                )
            } else if reused * 100 >= mapped * 95 {
                Assessment::new(
                    S::Overwritten,
                    R::ClustersReused {
                        reused,
                        total: mapped,
                    },
                )
            } else {
                Assessment::new(
                    S::Partial,
                    R::ClustersReused {
                        reused,
                        total: mapped,
                    },
                )
            }
        }
    }
}

/// (non-sparse clusters within the first `needed`, clusters covered by any run).
fn mapped_clusters(runs: &[Run], needed: u64) -> (u64, u64) {
    let mut mapped = 0;
    let mut covered = 0;
    for r in runs {
        if r.vcn >= needed {
            break;
        }
        let n = r.len.min(needed - r.vcn);
        covered += n;
        if r.lcn.is_some() {
            mapped += n;
        }
    }
    (mapped, covered)
}

fn reused_clusters(bm: &Bitmap, runs: &[Run], needed: u64) -> u64 {
    runs.iter()
        .take_while(|r| r.vcn < needed)
        .filter_map(|r| {
            r.lcn
                .map(|l| bm.count_allocated(l, r.len.min(needed - r.vcn)))
        })
        .sum()
}

/// Content sampling: the first, middle and last cluster of every file whose clusters are all
/// free, read in disk order after the `$MFT` pass, to catch SSD-erased (all-zero) content and
/// overwritten headers. Files are taken in the order they stream out until the budget is
/// spent; only their picks and a few counters are kept.
struct Sampler {
    budget: usize,
    /// (cluster, index into `files`, is the file's first cluster).
    plan: Vec<(u64, u32, bool)>,
    files: Vec<Sampled>,
}

struct Sampled {
    record: u64,
    clusters: u64,
    ext: String,
    read: u32,
    zero: u32,
    /// Whether the first cluster starts like the extension promises, when known.
    header: Option<bool>,
}

impl Sampler {
    fn new(budget: usize) -> Self {
        Sampler {
            budget,
            plan: Vec::new(),
            files: Vec::new(),
        }
    }

    fn offer(&mut self, record: u64, name: &str, a: &Assessment, data: &DataLocation, cs: u64) {
        let Reason::ClustersFreeUnsampled { clusters } = a.reason else {
            return;
        };
        let DataLocation::NonResident(nr) = data else {
            return;
        };
        let needed = nr.real.div_ceil(cs.max(1));
        let mapped: Vec<(u64, u64)> = nr
            .runs
            .iter()
            .take_while(|r| r.vcn < needed)
            .filter_map(|r| r.lcn.map(|l| (l, r.len.min(needed - r.vcn))))
            .collect();
        let total: u64 = mapped.iter().map(|m| m.1).sum();
        if total == 0 || self.plan.len() + 3 > self.budget {
            return;
        }
        let Ok(idx) = u32::try_from(self.files.len()) else {
            return;
        };
        let first_is_vcn0 = nr
            .runs
            .first()
            .is_some_and(|r| r.vcn == 0 && r.lcn.is_some());
        let mut picks = vec![0, total / 2, total - 1];
        picks.dedup();
        for (k, pick) in picks.into_iter().enumerate() {
            let mut left = pick;
            for (lcn, len) in &mapped {
                if left < *len {
                    self.plan.push((lcn + left, idx, k == 0 && first_is_vcn0));
                    break;
                }
                left -= len;
            }
        }
        self.files.push(Sampled {
            record,
            clusters,
            ext: magic::extension(name),
            read: 0,
            zero: 0,
            header: None,
        });
    }

    /// Reads the planned clusters and sends the new grades. Returns the number of reads.
    fn run<S: SectorSource>(
        &mut self,
        vol: &NtfsVolume<S>,
        progress: &dyn Progress,
        sink: &mut dyn ScanSink,
    ) -> Result<u64> {
        let cs = vol.cluster_size();
        self.plan.sort_unstable_by_key(|p| p.0);
        progress.set_total(self.plan.len() as u64);
        let mut buf = vec![0u8; SAMPLE_BYTES.min(cs as usize)];
        for &(lcn, i, is_head) in &self.plan {
            if progress.cancelled() {
                return Err(Error::Cancelled);
            }
            progress.advance(1);
            if vol.source().read_at(lcn * cs, &mut buf).is_err() {
                continue;
            }
            let f = &mut self.files[i as usize];
            f.read += 1;
            if buf.iter().all(|&b| b == 0) {
                f.zero += 1;
            }
            if is_head {
                f.header = magic::header_matches(&f.ext, &buf[..buf.len().min(1024)]);
            }
        }
        for f in &self.files {
            if f.read == 0 {
                continue;
            }
            let a = if f.zero == f.read {
                Assessment::new(State::Zeroed, Reason::AllZero { sampled: f.read })
            } else if f.header == Some(false) {
                Assessment::new(
                    State::Partial,
                    Reason::HeaderMismatch {
                        expected: magic::type_name(&f.ext).unwrap_or("its type").into(),
                    },
                )
            } else {
                Assessment::new(
                    State::LikelyIntact,
                    Reason::ClustersFree {
                        clusters: f.clusters,
                        sampled: f.read,
                    },
                )
            };
            sink.regrade(f.record, &a)?;
        }
        Ok(self.plan.len() as u64)
    }
}
