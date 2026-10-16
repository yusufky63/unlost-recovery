//! Request handling shared by the in-process engine and the elevated broker. The same code
//! serves both, so the broker protocol is exercised by ordinary tests without a pipe or UAC.
//!
//! Scan results live in a session database ([`Store`]) in the UI process. A scan run here
//! streams into its database through a writer thread. A scan run by the broker streams its
//! findings over the pipe instead (the broker never writes files); the engine feeds them into a
//! database here and [`Session::adopt`]s it. The broker keeps only the open source and a key
//! per scan: read requests bring the content location back, tagged with that key.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::sync_channel;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{Value, json};
use unlost_formats::findings::{self, FileRow, FolderRow, Message};
use unlost_formats::magic::{self, Category};
use unlost_formats::ntfs::{
    self, DeletedFile, NtfsVolume, PathStatus, ScanOptions, ScanSink, ScanStats, location,
};
use unlost_formats::partition::FsKind;
use unlost_formats::{Assessment, SectorSource, State};

use crate::job::{Job, ProgressSnapshot};
use crate::protocol::{
    EntryView, Hello, PROTOCOL_VERSION, Page, Query, ReadResult, Request, ScanSummary, Sealed, hex,
    unhex,
};
use crate::seal::Sealer;
use crate::sources::{self, SourceSpec};
use crate::store::Store;
use crate::{Error, Result, shadow, system, workspace};

/// Bytes per data event while streaming file content.
const DATA_CHUNK: usize = 1 << 20;
/// Records accepted in one `Entries` request.
const MAX_ENTRIES: usize = 200_000;
/// Scans kept loaded; the oldest is dropped (and its database deleted) beyond this.
const MAX_SCANS: usize = 4;
/// Batches waiting for the database writer before the scan waits.
const WRITE_QUEUE: usize = 4;

/// Where a request's progress and content go.
pub trait Sink: Send + Sync {
    fn progress(&self, id: u64, p: ProgressSnapshot);
    /// Content bytes; an error stops the read (the receiver went away).
    fn data(&self, id: u64, bytes: &[u8]) -> Result<()>;
}

enum Loaded {
    /// Scanned in this process: results in `store`, content read from `src`.
    Local {
        store: Store,
        src: Arc<dyn SectorSource>,
        cluster: u64,
    },
    /// Scanned by the broker: results in `store`, content read through the broker.
    Remote(Store),
    /// The broker's side of a scan: the source, and the key its locations were tagged with.
    Served {
        src: Arc<dyn SectorSource>,
        cluster: u64,
        sealer: Sealer,
    },
}

#[derive(Default)]
pub struct Session {
    /// Folder for session databases when a scan request names none. `None` in the broker:
    /// it never writes files, so its scans stream their findings back instead.
    workspace: Option<PathBuf>,
    scans: Mutex<Vec<(u32, Arc<Loaded>)>>,
    jobs: Mutex<HashMap<u64, Arc<Job>>>,
    next_scan: AtomicU32,
    next_store: AtomicU64,
}

impl Session {
    /// A session in the UI process, keeping scan databases in `workspace` unless a request
    /// names another folder. The caller has checked the folder with [`workspace::approve`].
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Session {
            workspace: Some(workspace.into()),
            ..Self::default()
        }
    }

    /// The broker's session. Scan ids start above `base`, so they never collide with the
    /// UI's own.
    pub fn broker(base: u32) -> Self {
        Session {
            next_scan: AtomicU32::new(base),
            ..Self::default()
        }
    }

    pub fn cancel(&self, id: u64) {
        if let Some(j) = self.jobs.lock().get(&id) {
            j.cancel();
        }
    }

    pub fn handle(&self, id: u64, req: Request, sink: &dyn Sink) -> Result<Value> {
        match req {
            Request::Hello => Ok(serde_json::to_value(Hello {
                version: PROTOCOL_VERSION,
                elevated: system::is_elevated(),
                pid: std::process::id(),
            })?),
            Request::Cancel { id: target } => {
                self.cancel(target);
                Ok(Value::Null)
            }
            Request::Scan { source, workspace } => {
                let job = self.job(id);
                let r = with_ticker(id, &job, sink, || {
                    self.scan(id, source, workspace.map(PathBuf::from), &job, sink)
                });
                self.jobs.lock().remove(&id);
                Ok(serde_json::to_value(r?)?)
            }
            Request::Query { scan, query } => Ok(serde_json::to_value(self.query(scan, &query)?)?),
            Request::Read {
                scan,
                record,
                sealed,
            } => {
                let job = self.job(id);
                let r = self.read(id, scan, record, sealed, &job, sink);
                self.jobs.lock().remove(&id);
                Ok(serde_json::to_value(r?)?)
            }
            Request::Entries { scan, records } => {
                let d = self.get(scan)?;
                let n = records.len().min(MAX_ENTRIES);
                let rows: Vec<EntryView> = store_of(&d)?.entries(&records[..n])?;
                Ok(serde_json::to_value(rows)?)
            }
            Request::Forget { scan } => {
                self.scans.lock().retain(|(s, _)| *s != scan);
                Ok(Value::Null)
            }
            Request::ShadowList => Ok(serde_json::to_value(shadow::list()?)?),
            Request::ShadowDir { device, path } => {
                Ok(serde_json::to_value(shadow::list_dir(&device, &path)?)?)
            }
            Request::ShadowRead { device, path } => {
                let job = self.job(id);
                let r = self.shadow_read(id, &device, &path, &job, sink);
                self.jobs.lock().remove(&id);
                Ok(json!({ "bytes": r? }))
            }
        }
    }

    /// Drops every scan and deletes their databases (the app is closing).
    pub fn forget_all(&self) {
        self.scans.lock().clear();
    }

    /// A new, empty database in `dir` (or the default folder), for a scan's findings.
    pub fn create_store(&self, dir: Option<&Path>) -> Result<Store> {
        let dir = dir
            .or(self.workspace.as_deref())
            .ok_or_else(|| Error::Refused("the administrator helper writes no files".into()))?;
        let n = self.next_store.fetch_add(1, Ordering::Relaxed) + 1;
        Store::create(dir, &workspace::file_name(n))
    }

    /// Takes over a database the engine filled from the broker's scan `scan`.
    pub fn adopt(&self, scan: u32, store: Store) {
        self.insert(scan, Loaded::Remote(store));
    }

    /// What the broker needs to read `record` of its scan `scan`: the stored location with
    /// the broker's own tag, and the scan's grade.
    pub fn sealed(&self, scan: u32, record: u64) -> Result<Sealed> {
        let d = self.get(scan)?;
        let s = store_of(&d)?
            .content(record)?
            .ok_or_else(|| Error::Refused(format!("record {record} is not in scan {scan}")))?;
        refuse_unreadable(&s.assessment)?;
        Ok(Sealed {
            location: hex(&s.location),
            tag: hex(&s.tag),
            assessment: s.assessment,
        })
    }

    fn job(&self, id: u64) -> Arc<Job> {
        let job = Arc::new(Job::new());
        self.jobs.lock().insert(id, job.clone());
        job
    }

    fn insert(&self, scan: u32, loaded: Loaded) {
        let mut scans = self.scans.lock();
        scans.retain(|(s, _)| *s != scan);
        scans.push((scan, Arc::new(loaded)));
        while scans.len() > MAX_SCANS {
            scans.remove(0);
        }
    }

    fn scan(
        &self,
        id: u64,
        source: SourceSpec,
        dir: Option<PathBuf>,
        job: &Job,
        sink: &dyn Sink,
    ) -> Result<ScanSummary> {
        let opened = sources::open(&source)?;
        if opened.fs != FsKind::Ntfs {
            return Err(Error::Refused(format!(
                "the {:?} file system is not supported for deleted-file scans yet",
                opened.fs
            )));
        }
        let vol = NtfsVolume::open(opened.src.clone())?;
        let scan = self.next_scan.fetch_add(1, Ordering::Relaxed) + 1;
        let opts = ScanOptions::default();
        let cluster = vol.cluster_size();
        let (stats, files, counts, loaded) = if self.workspace.is_none() {
            // The broker: findings go back over the pipe, each location tagged so a later
            // read request can prove the broker produced it.
            let sealer = Sealer::new(scan)?;
            let mut send = |b: Vec<u8>| sink.data(id, &b);
            let mut out = BatchSink::new(Some(&sealer), &mut send);
            let stats = run_scan(&vol, &opts, job, &mut out)?;
            let (files, counts) = (out.files, std::mem::take(&mut out.counts));
            drop(out);
            let served = Loaded::Served {
                src: opened.src,
                cluster,
                sealer,
            };
            (stats, files, counts, served)
        } else {
            let store = self.create_store(dir.as_deref())?;
            let stats = scan_into(&vol, &opts, job, &store)?;
            let (files, counts) = store.finish()?;
            let local = Loaded::Local {
                store,
                src: opened.src,
                cluster,
            };
            (stats, files, counts, local)
        };
        let summary = ScanSummary {
            scan,
            source,
            label: vol.label(),
            fs: opened.fs,
            cluster_size: vol.boot.cluster_size,
            volume_bytes: vol.volume_bytes(),
            files,
            stats,
            counts,
        };
        self.insert(scan, loaded);
        Ok(summary)
    }

    fn get(&self, scan: u32) -> Result<Arc<Loaded>> {
        self.scans
            .lock()
            .iter()
            .find(|(s, _)| *s == scan)
            .map(|(_, d)| d.clone())
            .ok_or_else(|| Error::Refused(format!("scan {scan} is not loaded")))
    }

    fn query(&self, scan: u32, q: &Query) -> Result<Page> {
        let d = self.get(scan)?;
        store_of(&d)?.query(q)
    }

    fn read(
        &self,
        id: u64,
        scan: u32,
        record: u64,
        sealed: Option<Sealed>,
        job: &Job,
        sink: &dyn Sink,
    ) -> Result<ReadResult> {
        let d = self.get(scan)?;
        let (src, cluster, location, pre) = match &*d {
            Loaded::Local {
                store,
                src,
                cluster,
            } => {
                let s = store.content(record)?.ok_or_else(|| {
                    Error::Refused(format!("record {record} is not in scan {scan}"))
                })?;
                (src, *cluster, s.location, s.assessment)
            }
            Loaded::Served {
                src,
                cluster,
                sealer,
            } => {
                let s = sealed.ok_or_else(|| {
                    Error::Refused("the read request does not say where the content is".into())
                })?;
                let (Some(location), Some(tag)) = (unhex(&s.location), unhex(&s.tag)) else {
                    return Err(Error::Refused("malformed content location".into()));
                };
                if !sealer.verify(record, &location, &tag) {
                    return Err(Error::Refused(
                        "the content location was not issued by this scan".into(),
                    ));
                }
                (src, *cluster, location, s.assessment)
            }
            Loaded::Remote(_) => {
                return Err(Error::Refused(
                    "this scan is read through the administrator helper".into(),
                ));
            }
        };
        refuse_unreadable(&pre)?;
        let data = location::decode(&location)?;
        let mut pending: Vec<u8> = Vec::with_capacity(DATA_CHUNK);
        let mut out = |b: &[u8]| -> unlost_formats::Result<()> {
            let mut rest = b;
            while !rest.is_empty() {
                let n = (DATA_CHUNK - pending.len()).min(rest.len());
                pending.extend_from_slice(&rest[..n]);
                rest = &rest[n..];
                if pending.len() == DATA_CHUNK {
                    sink.data(id, &pending)
                        .map_err(|_| unlost_formats::Error::Cancelled)?;
                    pending.clear();
                }
            }
            Ok(())
        };
        let stats = ntfs::read_content(src, cluster, &data, &mut out, job)?;
        if !pending.is_empty() {
            sink.data(id, &pending)?;
        }
        Ok(ReadResult {
            assessment: ntfs::after_read(&pre, &stats),
            stats,
        })
    }

    fn shadow_read(
        &self,
        id: u64,
        device: &str,
        path: &str,
        job: &Job,
        sink: &dyn Sink,
    ) -> Result<u64> {
        let full = shadow::full_path(device, path)?;
        let mut f = std::fs::File::open(&full).map_err(|e| Error::io(&full, e))?;
        let mut buf = vec![0u8; DATA_CHUNK];
        let mut total = 0u64;
        loop {
            if job.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let n = f.read(&mut buf).map_err(|e| Error::io(&full, e))?;
            if n == 0 {
                return Ok(total);
            }
            sink.data(id, &buf[..n])?;
            total += n as u64;
        }
    }
}

fn store_of(d: &Loaded) -> Result<&Store> {
    match d {
        Loaded::Local { store, .. } | Loaded::Remote(store) => Ok(store),
        Loaded::Served { .. } => Err(Error::Refused(
            "the administrator helper keeps no scan results".into(),
        )),
    }
}

/// Metadata-only files are refused, not streamed as zeros.
fn refuse_unreadable(a: &Assessment) -> Result<()> {
    if matches!(a.state, State::MetadataOnly | State::NotRecoverableHere) {
        return Err(Error::Refused(a.reason.english()));
    }
    Ok(())
}

/// Runs the scan, preferring the sink's own failure (a full disk, a closed pipe) over the
/// cancellation it caused.
fn run_scan<S: SectorSource>(
    vol: &NtfsVolume<S>,
    opts: &ScanOptions,
    job: &Job,
    out: &mut BatchSink<'_>,
) -> Result<ScanStats> {
    let r = ntfs::scan(vol, opts, job, out);
    if let Some(e) = out.failed.take() {
        return Err(e);
    }
    let stats = r?;
    out.flush()?;
    Ok(stats)
}

/// Scans into `store`, with the database writes on their own thread so they overlap the
/// `$MFT` reads. The queue between them is short: a slow disk slows the scan instead of
/// filling memory.
fn scan_into<S: SectorSource>(
    vol: &NtfsVolume<S>,
    opts: &ScanOptions,
    job: &Job,
    store: &Store,
) -> Result<ScanStats> {
    let (tx, rx) = sync_channel::<Vec<u8>>(WRITE_QUEUE);
    std::thread::scope(|s| {
        let writer = s.spawn(move || -> Result<()> {
            for b in rx {
                store.ingest(&b)?;
            }
            Ok(())
        });
        let scanned = {
            let mut send = move |b: Vec<u8>| {
                tx.send(b)
                    .map_err(|_| Error::Database("the database writer stopped".into()))
            };
            let mut out = BatchSink::new(None, &mut send);
            run_scan(vol, opts, job, &mut out)
        };
        let written = writer
            .join()
            .unwrap_or_else(|_| Err(Error::Database("the database writer panicked".into())));
        // A write failure explains the scan's failure to hand over a batch.
        written?;
        scanned
    })
}

/// Packs findings into [`findings`] batches and hands each full batch to `out`.
struct BatchSink<'a> {
    buf: Vec<u8>,
    out: &'a mut dyn FnMut(Vec<u8>) -> Result<()>,
    sealer: Option<&'a Sealer>,
    files: u64,
    counts: BTreeMap<State, u64>,
    /// The error that made a hand-over fail; the scan itself only sees a cancellation.
    failed: Option<Error>,
}

impl<'a> BatchSink<'a> {
    fn new(sealer: Option<&'a Sealer>, out: &'a mut dyn FnMut(Vec<u8>) -> Result<()>) -> Self {
        BatchSink {
            buf: Vec::with_capacity(findings::BATCH_BYTES + (64 << 10)),
            out,
            sealer,
            files: 0,
            counts: BTreeMap::new(),
            failed: None,
        }
    }

    fn push(&mut self, m: &Message) -> unlost_formats::Result<()> {
        findings::put(&mut self.buf, m);
        if self.buf.len() >= findings::BATCH_BYTES {
            self.flush().map_err(|e| {
                self.failed = Some(e);
                unlost_formats::Error::Cancelled
            })?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let b = std::mem::replace(
            &mut self.buf,
            Vec::with_capacity(findings::BATCH_BYTES + (64 << 10)),
        );
        (self.out)(b)
    }
}

impl ScanSink for BatchSink<'_> {
    fn file(&mut self, f: DeletedFile) -> unlost_formats::Result<()> {
        let mut row = FileRow::new(&f);
        if let Some(s) = self.sealer {
            row.tag = s.tag(row.record, &row.location);
        }
        self.files += 1;
        *self.counts.entry(row.assessment.state).or_default() += 1;
        self.push(&Message::File(row))
    }

    fn folder(&mut self, key: u64, path: &str, status: PathStatus) -> unlost_formats::Result<()> {
        self.push(&Message::Folder(FolderRow {
            key,
            path: path.to_string(),
            status,
        }))
    }

    fn regrade(&mut self, record: u64, a: &Assessment) -> unlost_formats::Result<()> {
        // Only files graded "likely intact, not sampled" are sampled.
        if let Some(n) = self.counts.get_mut(&State::LikelyIntact) {
            *n = n.saturating_sub(1);
        }
        *self.counts.entry(a.state).or_default() += 1;
        self.push(&Message::Regrade {
            record,
            assessment: a.clone(),
        })
    }
}

/// Runs `work` while sending a progress snapshot every 200 ms.
fn with_ticker<T>(id: u64, job: &Job, sink: &dyn Sink, work: impl FnOnce() -> T) -> T {
    let done = AtomicBool::new(false);
    std::thread::scope(|s| {
        // Always at least one snapshot, and a last one after the work ends.
        s.spawn(|| {
            loop {
                sink.progress(id, job.snapshot());
                if done.load(Ordering::Relaxed) {
                    break;
                }
                for _ in 0..4 {
                    if done.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        });
        let r = work();
        done.store(true, Ordering::Relaxed);
        r
    })
}

/// Case folding for search that treats Turkish dotted and dotless i as plain `i`, so
/// "Iğdır", "ığdır" and "IĞDIR" all match "igdir".
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .filter(|&c| c != '\u{0307}')
        .map(|c| if c == 'ı' { 'i' } else { c })
        .collect()
}

pub fn category_of(name: &str) -> Category {
    magic::category(&magic::extension(name))
}

#[cfg(test)]
pub(crate) mod tests_support {
    use unlost_formats::ntfs::build::{Content, FileSpec, ImageBuilder};

    pub fn image(dir: &std::path::Path) -> (std::path::PathBuf, Vec<u8>) {
        let mut b = ImageBuilder::new(2048, 4096, 1024, 64);
        b.dir(30, 1, (5, 5), "Fotoğraflar", true);
        let mut photo: Vec<u8> = (0..3_000_000u32).map(|i| (i % 253) as u8 | 1).collect();
        photo[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
        b.file(FileSpec::new(
            40,
            "IMG_0001.jpg",
            (30, 1),
            Content::Clusters(photo.clone()),
        ));
        b.file(FileSpec::new(
            41,
            "Iğdır notları.txt",
            (30, 1),
            Content::Resident(b"not".to_vec()),
        ));
        b.file(FileSpec::new(
            42,
            "empty.bin",
            (5, 5),
            Content::Runs(vec![], 100),
        ));
        let path = dir.join("test.img");
        std::fs::write(&path, b.build()).unwrap();
        (path, photo)
    }

    /// Records the bytes and progress each request produced.
    pub struct Collect {
        pub data: parking_lot::Mutex<std::collections::HashMap<u64, Vec<u8>>>,
        pub ticks: std::sync::atomic::AtomicU32,
    }

    impl Collect {
        pub fn new() -> Self {
            Collect {
                data: parking_lot::Mutex::new(std::collections::HashMap::new()),
                ticks: std::sync::atomic::AtomicU32::new(0),
            }
        }
        pub fn take(&self, id: u64) -> Vec<u8> {
            self.data.lock().remove(&id).unwrap_or_default()
        }
    }

    impl super::Sink for Collect {
        fn progress(&self, _id: u64, _p: crate::job::ProgressSnapshot) {
            self.ticks
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        fn data(&self, id: u64, bytes: &[u8]) -> crate::Result<()> {
            self.data
                .lock()
                .entry(id)
                .or_default()
                .extend_from_slice(bytes);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::{Collect, image};
    use super::*;
    use crate::protocol::SortKey;

    fn summary(v: Value) -> ScanSummary {
        serde_json::from_value(v).unwrap()
    }

    fn read(record: u64, scan: u32) -> Request {
        Request::Read {
            scan,
            record,
            sealed: None,
        }
    }

    #[test]
    fn scan_query_read_through_the_session() {
        let tmp = tempfile::tempdir().unwrap();
        let (path, photo) = image(tmp.path());
        let ws = tmp.path().join("sessions");
        let s = Session::new(&ws);
        let sink = Collect::new();
        let source = SourceSpec::Image {
            path: path.to_string_lossy().into(),
            partition: None,
        };
        let summary = summary(
            s.handle(
                1,
                Request::Scan {
                    source,
                    workspace: None,
                },
                &sink,
            )
            .unwrap(),
        );
        assert_eq!(summary.files, 3);
        assert_eq!(summary.label, "UNLOSTTEST");
        assert_eq!(summary.counts.get(&State::LikelyIntact), Some(&2));
        assert!(sink.ticks.load(Ordering::Relaxed) >= 1);
        // The findings are in a database in the workspace, not in memory.
        let dbs: Vec<_> = std::fs::read_dir(&ws).unwrap().flatten().collect();
        assert_eq!(dbs.len(), 1);

        let q = Query {
            text: "IĞDIR".into(),
            ..Query::default()
        };
        let page: Page = serde_json::from_value(
            s.handle(
                2,
                Request::Query {
                    scan: summary.scan,
                    query: q,
                },
                &sink,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.rows[0].name, "Iğdır notları.txt");
        assert_eq!(page.rows[0].folder, r"\Fotoğraflar");

        let q = Query {
            states: vec![State::LikelyIntact],
            sort: SortKey::Size,
            desc: true,
            ..Query::default()
        };
        let page: Page = serde_json::from_value(
            s.handle(
                3,
                Request::Query {
                    scan: summary.scan,
                    query: q,
                },
                &sink,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.rows[0].name, "IMG_0001.jpg");
        assert!(matches!(
            page.rows[0].assessment.reason,
            unlost_formats::Reason::ClustersFree { .. }
        ));
        assert_eq!(page.counts.get(&State::MetadataOnly), Some(&1));

        let rows: Vec<EntryView> = serde_json::from_value(
            s.handle(
                4,
                Request::Entries {
                    scan: summary.scan,
                    records: vec![41, 40, 7],
                },
                &sink,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            rows.iter().map(|r| r.record).collect::<Vec<_>>(),
            vec![41, 40]
        );

        let r: ReadResult =
            serde_json::from_value(s.handle(5, read(40, summary.scan), &sink).unwrap()).unwrap();
        assert_eq!(r.assessment.state, State::LikelyIntact);
        assert_eq!(sink.take(5), photo);
        let _ = s.handle(6, read(41, summary.scan), &sink).unwrap();
        assert_eq!(sink.take(6), b"not");

        // Metadata-only files are refused, not streamed as zeros.
        assert!(matches!(
            s.handle(7, read(42, summary.scan), &sink),
            Err(Error::Refused(_))
        ));
        assert!(s.handle(8, read(40, 99), &sink).is_err());
        s.handle(9, Request::Forget { scan: summary.scan }, &sink)
            .unwrap();
        assert!(
            s.handle(
                10,
                Request::Query {
                    scan: summary.scan,
                    query: Query::default()
                },
                &sink
            )
            .is_err()
        );
        // Forgetting deletes the database and its folder.
        assert!(!ws.exists());
    }

    /// The broker's half and the UI's half in one process: findings cross as batches, the UI
    /// keeps them, and reads go back to the broker with the tagged location.
    #[test]
    fn broker_streams_and_reads_only_what_it_issued() {
        let tmp = tempfile::tempdir().unwrap();
        let (path, photo) = image(tmp.path());
        let broker = Session::broker(1_000_000);
        let ui = Session::new(tmp.path().join("sessions"));
        let sink = Collect::new();
        let source = SourceSpec::Image {
            path: path.to_string_lossy().into(),
            partition: None,
        };
        // The broker ignores a workspace: it writes nothing.
        let summary = summary(
            broker
                .handle(
                    1,
                    Request::Scan {
                        source,
                        workspace: Some(tmp.path().join("nope").to_string_lossy().into()),
                    },
                    &sink,
                )
                .unwrap(),
        );
        assert!(!tmp.path().join("nope").exists());
        assert!(summary.scan > 1_000_000);
        assert_eq!(summary.files, 3);
        assert_eq!(summary.counts.get(&State::LikelyIntact), Some(&2));
        let stream = sink.take(1);
        assert!(!stream.is_empty());
        // The broker answers no queries: it has no results to answer from.
        assert!(
            broker
                .handle(
                    2,
                    Request::Query {
                        scan: summary.scan,
                        query: Query::default()
                    },
                    &sink
                )
                .is_err()
        );

        let store = ui.create_store(None).unwrap();
        store.ingest(&stream).unwrap();
        assert_eq!(store.finish().unwrap().0, 3);
        ui.adopt(summary.scan, store);
        let page: Page = serde_json::from_value(
            ui.handle(
                3,
                Request::Query {
                    scan: summary.scan,
                    query: Query::default(),
                },
                &sink,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(page.total, 3);
        assert_eq!(page.rows[0].folder, r"\Fotoğraflar");
        // The UI cannot read a broker scan itself.
        assert!(ui.handle(4, read(40, summary.scan), &sink).is_err());

        let sealed = ui.sealed(summary.scan, 40).unwrap();
        let req = |record, sealed| Request::Read {
            scan: summary.scan,
            record,
            sealed: Some(sealed),
        };
        let r: ReadResult =
            serde_json::from_value(broker.handle(5, req(40, sealed.clone()), &sink).unwrap())
                .unwrap();
        assert_eq!(r.assessment.state, State::LikelyIntact);
        assert_eq!(sink.take(5), photo);

        // Without a location, with another record's tag, or with a forged location: refused.
        assert!(broker.handle(6, read(40, summary.scan), &sink).is_err());
        assert!(broker.handle(7, req(41, sealed.clone()), &sink).is_err());
        let mut forged = sealed.clone();
        let mut loc = unhex(&forged.location).unwrap();
        *loc.last_mut().unwrap() ^= 1;
        forged.location = hex(&loc);
        assert!(matches!(
            broker.handle(8, req(40, forged), &sink),
            Err(Error::Refused(_))
        ));
        assert!(sink.take(6).is_empty() && sink.take(7).is_empty() && sink.take(8).is_empty());
        // Name-only files are refused before anything is sent.
        assert!(ui.sealed(summary.scan, 42).is_err());
    }

    #[test]
    fn folding() {
        assert_eq!(fold("IĞDIR"), fold("ığdır"));
        assert_eq!(fold("İstanbul"), "istanbul");
        assert!(fold("Çağrı Özet").contains(&fold("ÇAĞRI")));
    }
}
