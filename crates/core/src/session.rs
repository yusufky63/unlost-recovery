//! Request handling shared by the in-process engine and the elevated broker. The same code
//! serves both, so the broker protocol is exercised by ordinary tests without a pipe or UAC.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{Value, json};
use unlost_formats::magic::{self, Category};
use unlost_formats::ntfs::{self, DeletedFile, NtfsVolume, ScanOptions};
use unlost_formats::partition::FsKind;
use unlost_formats::{SectorSource, State};

use crate::job::{Job, ProgressSnapshot};
use crate::protocol::{
    EntryView, Hello, PROTOCOL_VERSION, Page, Query, ReadResult, Request, ScanSummary, SortKey,
};
use crate::sources::{self, SourceSpec};
use crate::{Error, Result, shadow, system};

/// Bytes per data event while streaming file content.
const DATA_CHUNK: usize = 1 << 20;
const MAX_PAGE: u32 = 500;
/// Records accepted in one `Entries` request.
const MAX_ENTRIES: usize = 200_000;
/// Scans kept in memory; the oldest is dropped beyond this.
const MAX_SCANS: usize = 4;

/// Where a request's progress and content go.
pub trait Sink: Send + Sync {
    fn progress(&self, id: u64, p: ProgressSnapshot);
    /// Content bytes; an error stops the read (the receiver went away).
    fn data(&self, id: u64, bytes: &[u8]) -> Result<()>;
}

struct ScanData {
    src: Arc<dyn SectorSource>,
    cluster: u64,
    files: Vec<DeletedFile>,
    by_record: HashMap<u64, usize>,
}

#[derive(Default)]
pub struct Session {
    scans: Mutex<Vec<(u32, Arc<ScanData>)>>,
    jobs: Mutex<HashMap<u64, Arc<Job>>>,
    next_scan: AtomicU32,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// Scan ids start above `base`, so two sessions (UI and broker) never hand out the same id.
    pub fn with_scan_base(base: u32) -> Self {
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
            Request::Scan { source } => {
                let job = self.job(id);
                let r = with_ticker(id, &job, sink, || self.scan(source, &job));
                self.jobs.lock().remove(&id);
                Ok(serde_json::to_value(r?)?)
            }
            Request::Query { scan, query } => Ok(serde_json::to_value(self.query(scan, &query)?)?),
            Request::Read { scan, record } => {
                let job = self.job(id);
                let r = self.read(id, scan, record, &job, sink);
                self.jobs.lock().remove(&id);
                Ok(serde_json::to_value(r?)?)
            }
            Request::Entries { scan, records } => {
                let d = self.get(scan)?;
                let rows: Vec<EntryView> = records
                    .iter()
                    .take(MAX_ENTRIES)
                    .filter_map(|r| d.by_record.get(r).map(|&i| view(&d.files[i])))
                    .collect();
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

    fn job(&self, id: u64) -> Arc<Job> {
        let job = Arc::new(Job::new());
        self.jobs.lock().insert(id, job.clone());
        job
    }

    fn scan(&self, source: SourceSpec, job: &Job) -> Result<ScanSummary> {
        let opened = sources::open(&source)?;
        if opened.fs != FsKind::Ntfs {
            return Err(Error::Refused(format!(
                "the {:?} file system is not supported for deleted-file scans yet",
                opened.fs
            )));
        }
        let vol = NtfsVolume::open(opened.src.clone())?;
        let res = ntfs::scan(&vol, &ScanOptions::default(), job)?;
        let scan = self.next_scan.fetch_add(1, Ordering::Relaxed) + 1;
        let mut counts: BTreeMap<State, u64> = BTreeMap::new();
        for f in &res.files {
            *counts.entry(f.assessment.state).or_default() += 1;
        }
        let summary = ScanSummary {
            scan,
            source,
            label: res.label,
            fs: opened.fs,
            cluster_size: vol.boot.cluster_size,
            volume_bytes: vol.volume_bytes(),
            files: res.files.len() as u64,
            stats: res.stats,
            counts,
        };
        let by_record = res
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.record, i))
            .collect();
        let data = Arc::new(ScanData {
            src: opened.src,
            cluster: vol.cluster_size(),
            files: res.files,
            by_record,
        });
        let mut scans = self.scans.lock();
        scans.push((scan, data));
        while scans.len() > MAX_SCANS {
            scans.remove(0);
        }
        Ok(summary)
    }

    fn get(&self, scan: u32) -> Result<Arc<ScanData>> {
        self.scans
            .lock()
            .iter()
            .find(|(s, _)| *s == scan)
            .map(|(_, d)| d.clone())
            .ok_or_else(|| Error::Refused(format!("scan {scan} is not loaded")))
    }

    fn query(&self, scan: u32, q: &Query) -> Result<Page> {
        let d = self.get(scan)?;
        let needle = fold(q.text.trim());
        let mut counts: BTreeMap<State, u64> = BTreeMap::new();
        let mut hits: Vec<usize> = Vec::new();
        for (i, f) in d.files.iter().enumerate() {
            if let Some(c) = q.category
                && magic::category(&magic::extension(&f.name)) != c
            {
                continue;
            }
            if !needle.is_empty()
                && !fold(&f.name).contains(&needle)
                && !fold(&f.folder).contains(&needle)
            {
                continue;
            }
            *counts.entry(f.assessment.state).or_default() += 1;
            if q.states.is_empty() || q.states.contains(&f.assessment.state) {
                hits.push(i);
            }
        }
        let files = &d.files;
        // Text keys are folded once per hit, not once per comparison.
        match q.sort {
            SortKey::Name => hits.sort_by_cached_key(|&i| fold(&files[i].name)),
            SortKey::Folder => {
                hits.sort_by_cached_key(|&i| (fold(&files[i].folder), fold(&files[i].name)))
            }
            SortKey::State => hits.sort_by_key(|&i| {
                (
                    files[i].assessment.state.rank(),
                    std::cmp::Reverse(files[i].modified),
                )
            }),
            SortKey::Size => hits.sort_by_key(|&i| files[i].size),
            SortKey::Modified => hits.sort_by_key(|&i| files[i].modified),
        }
        if q.desc {
            hits.reverse();
        }
        let limit = if q.limit == 0 {
            200
        } else {
            q.limit.min(MAX_PAGE)
        } as usize;
        let rows = hits
            .iter()
            .skip(q.offset as usize)
            .take(limit)
            .map(|&i| view(&files[i]))
            .collect();
        Ok(Page {
            total: hits.len() as u64,
            rows,
            counts,
        })
    }

    fn read(
        &self,
        id: u64,
        scan: u32,
        record: u64,
        job: &Job,
        sink: &dyn Sink,
    ) -> Result<ReadResult> {
        let d = self.get(scan)?;
        let f = d
            .by_record
            .get(&record)
            .map(|&i| &d.files[i])
            .ok_or_else(|| Error::Refused(format!("record {record} is not in scan {scan}")))?;
        if matches!(
            f.assessment.state,
            State::MetadataOnly | State::NotRecoverableHere
        ) {
            return Err(Error::Refused(f.assessment.reason.english()));
        }
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
        let stats = ntfs::read_content(&d.src, d.cluster, &f.data, &mut out, job)?;
        if !pending.is_empty() {
            sink.data(id, &pending)?;
        }
        Ok(ReadResult {
            assessment: ntfs::after_read(&f.assessment, &stats),
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

fn view(f: &DeletedFile) -> EntryView {
    let ext = magic::extension(&f.name);
    EntryView {
        record: f.record,
        category: magic::category(&ext),
        ext,
        name: f.name.clone(),
        folder: f.folder.to_string(),
        path_status: f.path_status,
        size: f.size,
        created: f.created,
        modified: f.modified,
        assessment: f.assessment.clone(),
        links: f.links,
        streams: f.streams,
    }
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
}

#[cfg(test)]
mod tests {
    use super::tests_support::image;
    use super::*;

    pub struct Collect {
        pub data: Mutex<HashMap<u64, Vec<u8>>>,
        pub ticks: AtomicU32,
    }

    impl Sink for Collect {
        fn progress(&self, _id: u64, _p: ProgressSnapshot) {
            self.ticks.fetch_add(1, Ordering::Relaxed);
        }
        fn data(&self, id: u64, bytes: &[u8]) -> Result<()> {
            self.data
                .lock()
                .entry(id)
                .or_default()
                .extend_from_slice(bytes);
            Ok(())
        }
    }

    #[test]
    fn scan_query_read_through_the_session() {
        let tmp = tempfile::tempdir().unwrap();
        let (path, photo) = image(tmp.path());
        let s = Session::new();
        let sink = Collect {
            data: Mutex::new(HashMap::new()),
            ticks: AtomicU32::new(0),
        };
        let source = SourceSpec::Image {
            path: path.to_string_lossy().into(),
            partition: None,
        };
        let summary: ScanSummary =
            serde_json::from_value(s.handle(1, Request::Scan { source }, &sink).unwrap()).unwrap();
        assert_eq!(summary.files, 3);
        assert_eq!(summary.label, "UNLOSTTEST");
        assert!(sink.ticks.load(Ordering::Relaxed) >= 1);

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
        assert_eq!(page.counts.get(&State::MetadataOnly), Some(&1));

        let r: ReadResult = serde_json::from_value(
            s.handle(
                4,
                Request::Read {
                    scan: summary.scan,
                    record: 40,
                },
                &sink,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(r.assessment.state, State::LikelyIntact);
        assert_eq!(sink.data.lock().get(&4).unwrap(), &photo);

        // Metadata-only files are refused, not streamed as zeros.
        assert!(matches!(
            s.handle(
                5,
                Request::Read {
                    scan: summary.scan,
                    record: 42
                },
                &sink
            ),
            Err(Error::Refused(_))
        ));
        assert!(
            s.handle(
                6,
                Request::Read {
                    scan: 99,
                    record: 40
                },
                &sink
            )
            .is_err()
        );
        s.handle(7, Request::Forget { scan: summary.scan }, &sink)
            .unwrap();
        assert!(
            s.handle(
                8,
                Request::Query {
                    scan: summary.scan,
                    query: Query::default()
                },
                &sink
            )
            .is_err()
        );
    }

    #[test]
    fn folding() {
        assert_eq!(fold("IĞDIR"), fold("ığdır"));
        assert_eq!(fold("İstanbul"), "istanbul");
        assert!(fold("Çağrı Özet").contains(&fold("ÇAĞRI")));
    }
}
