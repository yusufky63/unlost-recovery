use sha2::{Digest, Sha256};

use crate::ntfs::build::{Content, FileSpec, ImageBuilder};
use crate::ntfs::record::{ATTR_FLAG_COMPRESSION_MASK, FILE_ATTRIBUTE_ENCRYPTED, REPARSE_TAG_WOF};
use crate::ntfs::{
    DeletedFile, NtfsVolume, PathStatus, ScanOptions, after_read, read_content, scan,
};
use crate::partition;
use crate::source::{MemSource, SectorSource, SliceSource};
use crate::state::{Reason, State};
use crate::vhdx::{Vhdx, build_test_vhdx};
use crate::{NoProgress, Progress};

const ROOT: (u64, u16) = (5, 5);

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| ((i as u32).wrapping_mul(2_654_435_761) >> 13) as u8 ^ seed)
        .map(|b| b | 1)
        .collect()
}

fn jpeg(len: usize) -> Vec<u8> {
    let mut v = pattern(len, 9);
    v[..4].copy_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]);
    v
}

struct Fixture {
    image: Vec<u8>,
    expected: Vec<(String, Vec<u8>)>,
}

/// One volume with every case the scanner must grade.
fn fixture(cluster: u32, record: u32) -> Fixture {
    let mut b = ImageBuilder::new(4096, cluster, record, 64);
    let mut expected = Vec::new();
    b.dir(30, 1, ROOT, "Docs", true);
    b.dir(31, 4, ROOT, "Old", false);
    // Record 32 is now a different folder (sequence 9); files that pointed at sequence 3 are lost.
    b.dir(32, 9, ROOT, "Reused", true);
    b.dir(33, 1, (30, 1), "Şirket Belgeleri", true);

    let mut live = FileSpec::new(40, "live.txt", (30, 1), Content::Clusters(pattern(5000, 1)));
    live.in_use = true;
    b.file(live);

    let note = b"Meeting notes: keep this.".to_vec();
    b.file(FileSpec::new(
        41,
        "note.txt",
        (30, 1),
        Content::Resident(note.clone()),
    ));
    expected.push((r"\Docs\note.txt".to_string(), note));

    let photo = jpeg(10_000);
    b.file(FileSpec::new(
        42,
        "photo.jpg",
        (30, 1),
        Content::Clusters(photo.clone()),
    ));
    expected.push((r"\Docs\photo.jpg".to_string(), photo));

    let turkish = pattern(7000, 3);
    b.file(FileSpec::new(
        43,
        "Çağrı Özet.docx",
        (33, 1),
        Content::Clusters({
            let mut v = turkish.clone();
            v[..4].copy_from_slice(b"PK\x03\x04");
            v
        }),
    ));
    let mut t = turkish;
    t[..4].copy_from_slice(b"PK\x03\x04");
    expected.push((r"\Docs\Şirket Belgeleri\Çağrı Özet.docx".to_string(), t));

    // Deleted folder (sequence bumped from 3 to 4 when freed) still links up.
    let old = pattern(3000, 4);
    b.file(FileSpec::new(
        44,
        "old.bin",
        (31, 3),
        Content::Clusters(old.clone()),
    ));
    expected.push((r"\Old\old.bin".to_string(), old));

    b.file(FileSpec::new(
        45,
        "orphan.txt",
        (32, 3),
        Content::Resident(b"orphan".to_vec()),
    ));

    // Partly reused: 2 of 4 clusters allocated again.
    let partial = b.file(FileSpec::new(
        46,
        "partial.bin",
        ROOT,
        Content::Clusters(pattern(4 * cluster as usize, 5)),
    ));
    b.mark(partial.lcn.unwrap(), 2, true);

    let mut gone = FileSpec::new(
        47,
        "gone.bin",
        ROOT,
        Content::Clusters(pattern(3 * cluster as usize, 6)),
    );
    gone.keep_allocated = true;
    b.file(gone);

    // TRIM: clusters free but zero.
    let z = b.file(FileSpec::new(
        48,
        "trimmed.jpg",
        ROOT,
        Content::Clusters(jpeg(9000)),
    ));
    let zeros = vec![0u8; (z.clusters * u64::from(cluster)) as usize];
    b.write(z.lcn.unwrap(), &zeros);

    b.file(FileSpec::new(
        49,
        "notajpeg.jpg",
        ROOT,
        Content::Clusters(pattern(9000, 7)),
    ));

    let mut enc = FileSpec::new(50, "secret.docx", ROOT, Content::Clusters(pattern(5000, 8)));
    enc.attributes |= FILE_ATTRIBUTE_ENCRYPTED;
    b.file(enc);

    let lcn = b.alloc(4);
    let mut comp = FileSpec::new(
        51,
        "packed.log",
        ROOT,
        Content::Runs(vec![(Some(lcn), 2), (None, 2)], 16_000),
    );
    comp.data_flags = ATTR_FLAG_COMPRESSION_MASK & 1;
    b.file(comp);

    b.file(FileSpec::new(
        52,
        "norun.txt",
        ROOT,
        Content::Runs(vec![], 5000),
    ));

    let mut torn = FileSpec::new(53, "torn.txt", ROOT, Content::Resident(b"torn".to_vec()));
    torn.tear = true;
    b.file(torn);

    // Sparse middle: data, hole, data.
    let a = b.alloc(1);
    let c = b.alloc(1);
    let cs = cluster as usize;
    b.write(a, &pattern(cs, 10));
    b.write(c, &pattern(cs, 11));
    b.file(FileSpec::new(
        54,
        "sparse.vhd",
        ROOT,
        Content::Runs(
            vec![(Some(a), 1), (None, 2), (Some(c), 1)],
            4 * cluster as u64,
        ),
    ));
    let mut sparse = pattern(cs, 10);
    sparse.extend(vec![0u8; 2 * cs]);
    sparse.extend(pattern(cs, 11));
    expected.push((r"\sparse.vhd".to_string(), sparse));

    let mut wof = FileSpec::new(
        55,
        "compact.exe",
        ROOT,
        Content::Runs(vec![(None, 4)], 16_000),
    );
    wof.reparse = Some(REPARSE_TAG_WOF);
    b.file(wof);

    let mut cloud = FileSpec::new(
        56,
        "online.pdf",
        ROOT,
        Content::Runs(vec![(None, 4)], 16_000),
    );
    cloud.reparse = Some(0x9000_601A);
    b.file(cloud);

    let split = pattern(6000, 12);
    b.split_file(57, 58, ROOT, "split.dat", &split);
    expected.push((r"\split.dat".to_string(), split));

    let mut ads = FileSpec::new(59, "empty.txt", ROOT, Content::Resident(Vec::new()));
    ads.extra_streams = 1;
    b.file(ads);

    Fixture {
        image: b.build(),
        expected,
    }
}

fn find<'a>(files: &'a [DeletedFile], path: &str) -> &'a DeletedFile {
    files.iter().find(|f| f.path() == path).unwrap_or_else(|| {
        panic!(
            "{path} not found in {:?}",
            files.iter().map(|f| f.path()).collect::<Vec<_>>()
        )
    })
}

fn recover(src: &dyn SectorSource, cluster: u64, f: &DeletedFile) -> Vec<u8> {
    let mut out = Vec::new();
    let stats = read_content(
        src,
        cluster,
        &f.data,
        &mut |b| {
            out.extend_from_slice(b);
            Ok(())
        },
        &NoProgress,
    )
    .unwrap();
    assert_eq!(stats.bytes, out.len() as u64);
    out
}

fn sha(b: &[u8]) -> Vec<u8> {
    Sha256::digest(b).to_vec()
}

fn check_volume(src: &(impl SectorSource + ?Sized), fx: &Fixture) {
    let before = {
        let mut all = vec![0u8; src.len() as usize];
        src.read_at(0, &mut all).unwrap();
        sha(&all)
    };
    let vol = NtfsVolume::open(src).unwrap();
    let res = scan(&vol, &ScanOptions::default(), &NoProgress).unwrap();
    let files = &res.files;
    assert_eq!(res.label, "UNLOSTTEST");
    assert!(res.stats.bitmap_ok);
    assert!(
        files.iter().all(|f| f.name != "live.txt"),
        "a live file was reported deleted"
    );
    let cs = vol.cluster_size();

    for (path, want) in &fx.expected {
        let f = find(files, path);
        assert!(
            matches!(f.assessment.state, State::LikelyIntact),
            "{path}: {:?}",
            f.assessment
        );
        let got = recover(vol.source(), cs, f);
        assert_eq!(sha(&got), sha(want), "{path} content differs");
    }

    let note = find(files, r"\Docs\note.txt");
    assert_eq!(note.assessment.reason, Reason::ResidentIntact);
    assert_eq!(note.path_status, PathStatus::Live);
    assert_eq!(note.modified, 1_704_067_200);
    let photo = find(files, r"\Docs\photo.jpg");
    assert!(
        matches!(photo.assessment.reason, Reason::ClustersFree { sampled, .. } if sampled >= 1)
    );
    assert_eq!(
        find(files, r"\Old\old.bin").path_status,
        PathStatus::DeletedParent
    );

    let orphan = find(files, r"\Lost folder #32\orphan.txt");
    assert_eq!(orphan.path_status, PathStatus::Lost);

    let partial = find(files, r"\partial.bin");
    assert_eq!(partial.assessment.state, State::Partial);
    assert_eq!(
        partial.assessment.reason,
        Reason::ClustersReused {
            reused: 2,
            total: 4
        }
    );
    let gone = find(files, r"\gone.bin");
    assert_eq!(gone.assessment.state, State::Overwritten);

    let trimmed = find(files, r"\trimmed.jpg");
    assert_eq!(trimmed.assessment.state, State::Zeroed);
    let mut stats_out = Vec::new();
    let stats = read_content(
        vol.source(),
        cs,
        &trimmed.data,
        &mut |b| {
            stats_out.extend_from_slice(b);
            Ok(())
        },
        &NoProgress,
    )
    .unwrap();
    assert_eq!(after_read(&trimmed.assessment, &stats).state, State::Zeroed);

    let notajpeg = find(files, r"\notajpeg.jpg");
    assert_eq!(notajpeg.assessment.state, State::Partial);
    assert!(matches!(
        notajpeg.assessment.reason,
        Reason::HeaderMismatch { .. }
    ));

    assert_eq!(
        find(files, r"\secret.docx").assessment.reason,
        Reason::Encrypted
    );
    assert_eq!(
        find(files, r"\packed.log").assessment.reason,
        Reason::Compressed
    );
    assert_eq!(
        find(files, r"\norun.txt").assessment.state,
        State::MetadataOnly
    );
    assert_eq!(
        find(files, r"\torn.txt").assessment.reason,
        Reason::TornRecord
    );
    assert_eq!(
        find(files, r"\compact.exe").assessment.reason,
        Reason::WofCompressed
    );
    assert_eq!(
        find(files, r"\online.pdf").assessment.reason,
        Reason::CloudOnly
    );
    let empty = find(files, r"\empty.txt");
    assert_eq!(empty.assessment.reason, Reason::EmptyFile);
    assert_eq!(empty.streams, 1);

    // Nothing above may have changed the source.
    let after = {
        let mut all = vec![0u8; src.len() as usize];
        src.read_at(0, &mut all).unwrap();
        sha(&all)
    };
    assert_eq!(before, after);
}

#[test]
fn grades_and_recovers_4k_clusters() {
    let fx = fixture(4096, 1024);
    check_volume(&MemSource::new(fx.image.clone()), &fx);
}

#[test]
fn records_larger_than_clusters() {
    // 512-byte clusters with 1 KiB records: every record spans two clusters.
    let fx = fixture(512, 1024);
    check_volume(&MemSource::new(fx.image.clone()), &fx);
}

#[test]
fn four_k_records() {
    let fx = fixture(4096, 4096);
    check_volume(&MemSource::new(fx.image.clone()), &fx);
}

#[test]
fn through_vhdx_and_gpt() {
    let fx = fixture(4096, 1024);
    let disk = partition::build_test_gpt(&fx.image);
    let vhdx = Vhdx::open(MemSource::new(build_test_vhdx(&disk, 512))).unwrap();
    let parts = partition::list(&vhdx).unwrap();
    assert_eq!(parts.len(), 1);
    let p = &parts[0];
    let vol = SliceSource::new(&vhdx as &dyn SectorSource, p.offset, p.length).unwrap();
    check_volume(&vol, &fx);
}

struct CountingCancel(std::sync::atomic::AtomicU64);

impl Progress for CountingCancel {
    fn advance(&self, n: u64) {
        self.0.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
    }
    fn cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed) > 0
    }
}

#[test]
fn cancels_and_truncates() {
    let fx = fixture(4096, 1024);
    let src = MemSource::new(fx.image);
    let vol = NtfsVolume::open(&src as &dyn SectorSource).unwrap();
    let small = ScanOptions {
        max_files: 3,
        max_samples: 0,
    };
    let res = scan(&vol, &small, &NoProgress).unwrap();
    assert_eq!(res.files.len(), 3);
    assert!(res.stats.truncated);
    assert!(
        res.files
            .iter()
            .all(|f| !matches!(f.assessment.reason, Reason::ClustersFree { .. }))
    );
    // The first chunk holds every record, so cancellation shows up at the sampling stage.
    let c = CountingCancel(std::sync::atomic::AtomicU64::new(0));
    assert!(matches!(
        scan(&vol, &ScanOptions::default(), &c),
        Err(crate::Error::Cancelled)
    ));
}

/// Deterministic xorshift for the mutation tests.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn survives_random_damage() {
    let fx = fixture(4096, 1024);
    let mft = 4 * 4096..(4 * 4096 + 64 * 1024);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..300 {
        let mut img = fx.image.clone();
        let flips = 1 + (rng.next() % 64) as usize;
        for _ in 0..flips {
            let at = mft.start + (rng.next() as usize % mft.len());
            img[at] = rng.next() as u8;
        }
        if round % 10 == 0 {
            // Also damage the boot sector and the bitmap.
            let at = (rng.next() % 512) as usize;
            img[at] ^= 0xFF;
        }
        let src = MemSource::new(img);
        if let Ok(vol) = NtfsVolume::open(&src as &dyn SectorSource)
            && let Ok(res) = scan(&vol, &ScanOptions::default(), &NoProgress)
        {
            let readable = res.files.iter().filter(|f| {
                !matches!(
                    f.assessment.state,
                    State::MetadataOnly | State::NotRecoverableHere
                )
            });
            for f in readable.take(8) {
                let _ = read_content(
                    &src,
                    vol.cluster_size(),
                    &f.data,
                    &mut |_| Ok(()),
                    &NoProgress,
                );
            }
        }
    }
}
