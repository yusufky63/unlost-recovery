//! Tests that need administrator rights: raw volume reads, shadow copies, a full NTFS scan of a
//! real volume. They are ignored by default; run them from an elevated prompt:
//! `cargo test -p unlost-core --test elevated -- --ignored --nocapture`
//! They only read the source. The full scan writes its session database under
//! `UNLOST_WORKSPACE` (a folder on another disk) and deletes it afterwards. Without elevation
//! they return early with a note.

use std::time::Instant;

use unlost_core::device::{WRITE_BITS, WinDevice};
use unlost_core::job::ProgressSnapshot;
use unlost_core::protocol::{Page, Query, ReadResult, Request, ScanSummary, SortKey};
use unlost_core::session::{Session, Sink};
use unlost_core::sources::SourceSpec;
use unlost_core::{shadow, system, workspace};
use unlost_formats::SectorSource;

fn elevated() -> bool {
    let e = system::is_elevated();
    if !e {
        eprintln!("not elevated, skipped");
    }
    e
}

#[test]
#[ignore]
fn raw_system_volume_reads_read_only() {
    if !elevated() {
        return;
    }
    let sys = std::env::var("SYSTEMDRIVE").unwrap_or_else(|_| "C:".into());
    let dev = WinDevice::open_volume(&sys).unwrap();
    assert_eq!(dev.access_mask() & WRITE_BITS, 0);
    let mut boot = [0u8; 512];
    dev.read_at(0, &mut boot).unwrap();
    assert_eq!(&boot[3..11], b"NTFS    ");
    // Unaligned reads go through the bounce buffer.
    let mut odd = [0u8; 7];
    dev.read_at(3, &mut odd).unwrap();
    assert_eq!(&odd, b"NTFS   ");
    let mut tail = [0u8; 100];
    dev.read_at(dev.len() - 100, &mut tail).unwrap();
    assert!(dev.read_at(dev.len() - 50, &mut tail).is_err());
    eprintln!("{sys}: {} bytes, sector {}", dev.len(), dev.sector_size());
}

#[test]
#[ignore]
fn shadow_copies_list_browse_and_read() {
    if !elevated() {
        return;
    }
    let copies = shadow::list().unwrap();
    eprintln!("{} shadow copies", copies.len());
    let Some(c) = copies.first() else {
        return;
    };
    let root = shadow::list_dir(&c.device, "").unwrap();
    assert!(root.iter().any(|e| e.is_dir));
    let hosts = shadow::full_path(&c.device, r"Windows\System32\drivers\etc\hosts").unwrap();
    if let Ok(b) = std::fs::read(&hosts) {
        assert!(!b.is_empty());
        eprintln!("read {} bytes of hosts from {}", b.len(), c.device);
    }
    assert!(shadow::full_path(&c.device, r"..\x").is_err());
}

struct Quiet(std::sync::atomic::AtomicU64);
impl Sink for Quiet {
    fn progress(&self, _: u64, _: ProgressSnapshot) {}
    fn data(&self, _: u64, b: &[u8]) -> unlost_core::Result<()> {
        self.0
            .fetch_add(b.len() as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}

fn mib(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

/// A full deleted-file scan of the newest shadow copy of a real volume (or of
/// `UNLOST_SCAN_DEVICE`, for example `D:\`), into a session database under `UNLOST_WORKSPACE`,
/// which must be a folder on another physical disk. Proves the scanner handles a large `$MFT`
/// in bounded memory and that the database answers queries and reads on the full result.
#[test]
#[ignore]
fn full_scan_of_a_real_volume() {
    if !elevated() {
        return;
    }
    let Ok(folder) = std::env::var("UNLOST_WORKSPACE") else {
        eprintln!("set UNLOST_WORKSPACE to a folder on another disk than the source; skipped");
        return;
    };
    let source = match std::env::var("UNLOST_SCAN_DEVICE") {
        Ok(root) => SourceSpec::Volume { root },
        Err(_) => {
            let copies = shadow::list().unwrap();
            let Some(c) = copies.first() else {
                eprintln!("no shadow copy to scan");
                return;
            };
            SourceSpec::Shadow {
                device: c.device.clone(),
                volume: c.volume.clone(),
            }
        }
    };
    // The rule the app applies: the database never goes on a disk the source uses.
    let dir = workspace::approve(
        &source,
        &workspace::chosen_dir(std::path::Path::new(&folder)),
        false,
    )
    .unwrap();
    let session = Session::new(&dir);
    let sink = Quiet(Default::default());
    let t = Instant::now();
    let summary: ScanSummary = serde_json::from_value(
        session
            .handle(
                1,
                Request::Scan {
                    source: source.clone(),
                    workspace: None,
                },
                &sink,
            )
            .unwrap(),
    )
    .unwrap();
    let scanned = t.elapsed().as_secs_f64();
    let (peak_ws, peak_private) = system::memory_peaks().unwrap_or_default();
    let db: u64 = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        // The listing's sizes are stale for a file still open; ask the file itself.
        .filter_map(|e| std::fs::metadata(e.path()).ok())
        .map(|m| m.len())
        .sum();
    eprintln!(
        "{:?}: {} records, {} deleted files, {:?} in {scanned:.1}s; peak working set {:.0} MiB, peak private {:.0} MiB; database {:.0} MiB",
        source,
        summary.stats.records,
        summary.files,
        summary.counts,
        mib(peak_ws),
        mib(peak_private),
        mib(db)
    );
    assert!(summary.stats.records > 0);
    assert!(summary.stats.bitmap_ok);
    assert!(!summary.stats.truncated);

    let query = |id, q: Query| -> (Page, f64) {
        let t = Instant::now();
        let p: Page = serde_json::from_value(
            session
                .handle(
                    id,
                    Request::Query {
                        scan: summary.scan,
                        query: q,
                    },
                    &sink,
                )
                .unwrap(),
        )
        .unwrap();
        (p, t.elapsed().as_secs_f64())
    };
    let (first, t_default) = query(2, Query::default());
    let (_, t_deep) = query(
        3,
        Query {
            offset: 100_000,
            ..Query::default()
        },
    );
    let (found, t_text) = query(
        4,
        Query {
            text: "jpg".into(),
            ..Query::default()
        },
    );
    let (_, t_name) = query(
        5,
        Query {
            sort: SortKey::Name,
            ..Query::default()
        },
    );
    let (_, t_folder) = query(
        6,
        Query {
            sort: SortKey::Folder,
            ..Query::default()
        },
    );
    assert_eq!(
        first.total, summary.files,
        "the default query covers every file"
    );
    let total: u64 = first.counts.values().sum();
    assert_eq!(total, summary.files);
    eprintln!(
        "queries: default {t_default:.2}s, page 500 {t_deep:.2}s, text {t_text:.2}s ({} hits), by name {t_name:.2}s, by folder {t_folder:.2}s",
        found.total
    );
    // One read of the best file found, streamed to a counter, not to disk.
    if let Some(row) = first
        .rows
        .iter()
        .find(|r| r.size > 0 && r.size < (64 << 20))
    {
        let t = Instant::now();
        let r: ReadResult = serde_json::from_value(
            session
                .handle(
                    7,
                    Request::Read {
                        scan: summary.scan,
                        record: row.record,
                        sealed: None,
                    },
                    &sink,
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(r.stats.bytes, row.size);
        eprintln!(
            "read {} ({} bytes) in {:.2}s: {:?}",
            row.name,
            row.size,
            t.elapsed().as_secs_f64(),
            r.assessment.state
        );
    }
    session.forget_all();
    assert!(!dir.exists(), "the database folder is removed afterwards");
}
