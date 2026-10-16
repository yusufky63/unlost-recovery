//! Entry points for the cargo-fuzz targets in `fuzz/`. Each takes arbitrary bytes and must
//! neither panic nor allocate without bound. They live here, compiled and tested on every
//! platform, so the fuzz crate is only one-line wrappers.

use crate::ntfs::{self, NtfsVolume, ScanOptions};
use crate::source::MemSource;
use crate::{NoProgress, partition, recyclebin, vhdx};

/// A whole volume image: boot sector, `$MFT`, bitmap, every deleted file's content.
pub fn ntfs_volume(data: &[u8]) {
    let src = MemSource::new(data.to_vec());
    let Ok(vol) = NtfsVolume::open(&src) else {
        return;
    };
    let opts = ScanOptions {
        max_files: 10_000,
        max_samples: 1_000,
    };
    if let Ok(res) = ntfs::scan(&vol, &opts, &NoProgress) {
        for f in res.files.iter().take(64) {
            let _ = ntfs::read_content(
                &src,
                vol.cluster_size(),
                &f.data,
                &mut |_| Ok(()),
                &NoProgress,
            );
        }
    }
}

/// One file record: fixups and the attribute walk.
pub fn file_record(data: &[u8]) {
    let mut b = data.to_vec();
    let Some(h) = ntfs::record::header(&b) else {
        return;
    };
    ntfs::record::apply_fixups(&mut b);
    for a in ntfs::record::attributes(&b, &h) {
        if let ntfs::record::Body::Resident(v) = a.body {
            let _ = ntfs::record::FileName::parse(v);
            let _ = ntfs::record::StandardInfo::parse(v);
            let _ = ntfs::record::parse_attribute_list(v);
        }
    }
}

pub fn runlist(data: &[u8]) {
    let _ = ntfs::runlist::decode(data, 0, 1 << 32);
}

pub fn boot_sector(data: &[u8]) {
    let _ = ntfs::BootSector::parse(data);
}

pub fn recycle_info(data: &[u8]) {
    let _ = recyclebin::parse_info(data);
}

pub fn vhdx_image(data: &[u8]) {
    let Ok(v) = vhdx::Vhdx::open(MemSource::new(data.to_vec())) else {
        return;
    };
    let mut buf = vec![0u8; 4096];
    let _ = crate::SectorSource::read_at(&v, 0, &mut buf);
}

pub fn partitions(data: &[u8]) {
    let _ = partition::list(&MemSource::new(data.to_vec()));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift, so a failure reproduces.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
        let mut v = seed.to_vec();
        if v.is_empty() {
            return v;
        }
        for _ in 0..1 + rng.next() % 16 {
            let i = (rng.next() as usize) % v.len();
            v[i] = rng.next() as u8;
        }
        if rng.next().is_multiple_of(8) {
            v.truncate((rng.next() as usize) % v.len());
        }
        v
    }

    #[test]
    fn small_parsers_survive_random_input() {
        let mut rng = Rng(0x5EED_1234_ABCD_0001);
        let seeds: Vec<Vec<u8>> = vec![
            crate::ntfs::build::encode_record(
                3,
                0,
                0,
                &[crate::ntfs::build::resident(
                    0x30,
                    "",
                    0,
                    &crate::ntfs::build::file_name((5, 5), "seed.txt", 1, 3),
                )],
                1024,
            ),
            crate::ntfs::runlist::encode(&[(Some(100), 8), (None, 4), (Some(50), 2)]),
            crate::recyclebin::build_v2(10, 133_485_408_000_000_000, r"C:\seed.txt"),
            vec![0u8; 512],
        ];
        for _ in 0..4000 {
            for s in &seeds {
                let m = mutate(s, &mut rng);
                file_record(&m);
                runlist(&m);
                boot_sector(&m);
                recycle_info(&m);
                partitions(&m);
            }
            let len = (rng.next() % 2048) as usize;
            let junk: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            file_record(&junk);
            runlist(&junk);
            recycle_info(&junk);
        }
    }

    #[test]
    fn image_parsers_survive_damage() {
        let mut b = crate::ntfs::build::ImageBuilder::new(256, 4096, 1024, 32);
        b.dir(30, 1, (5, 5), "d", true);
        b.file(crate::ntfs::build::FileSpec::new(
            31,
            "a.bin",
            (30, 1),
            crate::ntfs::build::Content::Clusters(vec![7u8; 9000]),
        ));
        let vol = b.build();
        let image = crate::vhdx::build_test_vhdx(&crate::partition::build_test_gpt(&vol), 512);
        let mut rng = Rng(0xFEED_0000_0000_0042);
        for _ in 0..60 {
            ntfs_volume(&mutate(&vol, &mut rng));
            let mut v = image.clone();
            for _ in 0..8 {
                let i = (rng.next() as usize) % (3 << 20).min(v.len());
                v[i] = rng.next() as u8;
            }
            vhdx_image(&v);
        }
    }
}
