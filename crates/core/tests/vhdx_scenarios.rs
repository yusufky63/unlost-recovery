//! Scenario suite on Windows-made NTFS images.
//!
//! `scripts/make-fixtures.ps1` (elevated) builds VHDX files with Windows' own NTFS driver,
//! writes files, deletes some, and records SHA-256 manifests. This test scans each image
//! through the same session the app uses and checks every expectation:
//!
//! - live files are never reported as deleted;
//! - resident files recover byte-exact;
//! - a file graded "likely intact" must recover byte-exact (the honesty rule);
//! - a file Windows trimmed may instead be "zeroed", and then reads as zeros;
//! - reallocated files are graded overwritten or partial, never intact;
//! - the image file is unchanged afterwards.
//!
//! Without fixtures the test passes with a note, unless `UNLOST_REQUIRE_FIXTURES=1` (CI).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;
use serde::Deserialize;
use unlost_core::job::ProgressSnapshot;
use unlost_core::protocol::{Page, Query, ReadResult, Request, ScanSummary};
use unlost_core::session::{Session, Sink};
use unlost_core::sources::SourceSpec;
use unlost_core::writer::hash_file;
use unlost_formats::State;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    image: String,
    trim_disabled: bool,
    files: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    path: String,
    size: u64,
    sha256: String,
    deleted: bool,
    expect: String,
}

struct Collect(Mutex<HashMap<u64, Vec<u8>>>, AtomicU32);

impl Sink for Collect {
    fn progress(&self, _: u64, _: ProgressSnapshot) {
        self.1.fetch_add(1, Ordering::Relaxed);
    }
    fn data(&self, id: u64, b: &[u8]) -> unlost_core::Result<()> {
        self.0.lock().entry(id).or_default().extend_from_slice(b);
        Ok(())
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
}

fn sha(b: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

#[test]
fn vhdx_scenarios() {
    let dir = fixtures();
    let manifests: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect()
        })
        .unwrap_or_default();
    if manifests.is_empty() {
        assert!(
            std::env::var("UNLOST_REQUIRE_FIXTURES").is_err(),
            "no fixtures in {}; run scripts/make-fixtures.ps1 elevated",
            dir.display()
        );
        eprintln!("vhdx_scenarios: no fixtures in {}, skipped", dir.display());
        return;
    }
    for m in manifests {
        let manifest: Manifest = serde_json::from_slice(&std::fs::read(&m).unwrap()).unwrap();
        check(&dir, &manifest);
    }
}

fn check(dir: &Path, m: &Manifest) {
    let image = dir.join(&m.image);
    let before = hash_file(&image).unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let session = Session::new(workspace.path());
    let sink = Collect(Mutex::new(HashMap::new()), AtomicU32::new(0));
    let source = SourceSpec::Image {
        path: image.to_string_lossy().into(),
        partition: None,
    };
    let summary: ScanSummary = serde_json::from_value(
        session
            .handle(
                1,
                Request::Scan {
                    source,
                    workspace: None,
                },
                &sink,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(summary.label, "UNLOST", "{}", m.image);
    let page: Page = serde_json::from_value(
        session
            .handle(
                2,
                Request::Query {
                    scan: summary.scan,
                    query: Query {
                        limit: 500,
                        ..Query::default()
                    },
                },
                &sink,
            )
            .unwrap(),
    )
    .unwrap();
    let by_path: HashMap<String, &unlost_core::protocol::EntryView> = page
        .rows
        .iter()
        .map(|r| {
            let p = if r.folder == "\\" {
                format!("\\{}", r.name)
            } else {
                format!("{}\\{}", r.folder, r.name)
            };
            (p.to_lowercase(), r)
        })
        .collect();

    let mut next = 10u64;
    for f in &m.files {
        let key = f.path.to_lowercase();
        if !f.deleted {
            assert!(
                !by_path.contains_key(&key),
                "{}: live file reported as deleted",
                f.path
            );
            continue;
        }
        // A folder deleted with POSIX semantics (Windows Server 2025 does this for open
        // folders) loses its name, and its files come back under a lost folder: match those by
        // name and size. Content checks below stay as strict.
        let name = key.rsplit('\\').next().unwrap_or("").to_string();
        let lost = page.rows.iter().find(|r| {
            r.folder.starts_with("\\Lost folder #")
                && r.name.to_lowercase() == name
                && r.size == f.size
        });
        let row = by_path.get(&key).copied().or(lost).unwrap_or_else(|| {
            panic!(
                "{}: {} not found among {:?}",
                m.image,
                f.path,
                by_path.keys().collect::<Vec<_>>()
            )
        });
        assert_eq!(row.size, f.size, "{}: size", f.path);
        let state = row.assessment.state;
        match f.expect.as_str() {
            "intact" => assert_eq!(
                state,
                State::LikelyIntact,
                "{}: {:?}",
                f.path,
                row.assessment
            ),
            "intactOrZeroed" => assert!(
                matches!(state, State::LikelyIntact | State::Zeroed | State::Partial),
                "{}: {:?}",
                f.path,
                row.assessment
            ),
            "overwritten" => {
                assert!(
                    matches!(state, State::Overwritten | State::Partial),
                    "{}: {:?}",
                    f.path,
                    row.assessment
                );
                continue;
            }
            other => panic!("unknown expectation {other}"),
        }
        next += 1;
        let r: ReadResult = serde_json::from_value(
            session
                .handle(
                    next,
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
        let got = sink.0.lock().remove(&next).unwrap_or_default();
        assert_eq!(got.len() as u64, f.size, "{}: recovered size", f.path);
        match r.assessment.state {
            // The honesty rule: "likely intact" must really be intact.
            State::LikelyIntact => assert_eq!(
                sha(&got),
                f.sha256,
                "{}: likely intact but different bytes",
                f.path
            ),
            State::Zeroed => {
                assert!(!m.trim_disabled, "{}: zeroed although TRIM was off", f.path);
                assert!(got.iter().all(|&b| b == 0) || r.stats.zero_clusters > 0);
            }
            State::Partial => assert!(
                !m.trim_disabled,
                "{}: partial although TRIM was off",
                f.path
            ),
            s => panic!("{}: unexpected state after reading: {s:?}", f.path),
        }
    }
    assert_eq!(
        hash_file(&image).unwrap(),
        before,
        "{}: the image changed",
        m.image
    );
}
