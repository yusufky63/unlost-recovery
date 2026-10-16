//! Tests that need administrator rights: raw volume reads, shadow copies, a full NTFS scan of a
//! real volume. They are ignored by default; run them from an elevated prompt:
//! `cargo test -p unlost-core --test elevated -- --ignored --nocapture`
//! They only read. Without elevation they return early with a note.

use std::time::Instant;

use unlost_core::device::{WRITE_BITS, WinDevice};
use unlost_core::job::ProgressSnapshot;
use unlost_core::protocol::{Request, ScanSummary};
use unlost_core::session::{Session, Sink};
use unlost_core::sources::SourceSpec;
use unlost_core::{shadow, system};
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

struct Quiet;
impl Sink for Quiet {
    fn progress(&self, _: u64, _: ProgressSnapshot) {}
    fn data(&self, _: u64, _: &[u8]) -> unlost_core::Result<()> {
        Ok(())
    }
}

/// A full deleted-file scan of the newest shadow copy of a real volume: proves the scanner
/// handles a large `$MFT` in time and memory. Set `UNLOST_SCAN_DEVICE` to scan a specific
/// volume instead (for example `D:\`).
#[test]
#[ignore]
fn full_scan_of_a_real_volume() {
    if !elevated() {
        return;
    }
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
    let session = Session::new();
    let t = Instant::now();
    let summary: ScanSummary = serde_json::from_value(
        session
            .handle(
                1,
                Request::Scan {
                    source: source.clone(),
                },
                &Quiet,
            )
            .unwrap(),
    )
    .unwrap();
    eprintln!(
        "{:?}: {} records, {} deleted files, {:?} in {:.1}s",
        source,
        summary.stats.records,
        summary.files,
        summary.counts,
        t.elapsed().as_secs_f64()
    );
    assert!(summary.stats.records > 0);
    assert!(summary.stats.bitmap_ok);
}
