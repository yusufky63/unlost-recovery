//! Writes a small synthetic NTFS volume image for UI previews and screenshots:
//! `cargo run -p unlost-core --example demo_image -- target/demo.img`
//!
//! It is built by the same test builder the parser tests use, not by Windows, and is labelled
//! `UNLOSTTEST`. Every file state the UI can show appears at least once.

use unlost_formats::ntfs::build::{Content, FileSpec, ImageBuilder};

fn bytes(len: usize, seed: u8, head: &[u8]) -> Vec<u8> {
    let mut v: Vec<u8> = (0..len)
        .map(|i| (((i as u32).wrapping_mul(2_654_435_761) >> 13) as u8 ^ seed) | 1)
        .collect();
    v[..head.len()].copy_from_slice(head);
    v
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "demo.img".into());
    let cs = 4096u32;
    let mut b = ImageBuilder::new(8192, cs, 1024, 96);
    let jpeg = [0xFF, 0xD8, 0xFF, 0xE0];
    let root = (5, 5);
    b.dir(30, 1, root, "Users", true);
    b.dir(31, 1, (30, 1), "Deniz", true);
    b.dir(32, 1, (31, 1), "Pictures", true);
    b.dir(33, 1, (31, 1), "Documents", true);
    b.dir(34, 4, (33, 1), "Tez taslakları", false);
    b.dir(35, 1, (32, 1), "Tatil 2026", true);
    b.dir(36, 9, (31, 1), "Downloads", true);

    let mut rec = 40;
    let mut add = |b: &mut ImageBuilder, name: &str, parent: (u64, u16), content: Content| {
        b.file(FileSpec::new(rec, name, parent, content));
        rec += 1;
        rec - 1
    };
    for i in 1..=6 {
        add(
            &mut b,
            &format!("IMG_20260{i}14_0931.jpg"),
            (35, 1),
            Content::Clusters(bytes(180_000 + i * 9000, i as u8, &jpeg)),
        );
    }
    add(
        &mut b,
        "Çağrı Özet.docx",
        (33, 1),
        Content::Clusters(bytes(48_000, 3, b"PK\x03\x04")),
    );
    add(
        &mut b,
        "Bölüm 3 - Yöntem.docx",
        (34, 4),
        Content::Clusters(bytes(91_000, 4, b"PK\x03\x04")),
    );
    add(
        &mut b,
        "kaynakça.txt",
        (34, 4),
        Content::Resident(b"Kaynaklar: ...".to_vec()),
    );
    add(
        &mut b,
        "budget-2026.xlsx",
        (33, 1),
        Content::Clusters(bytes(26_000, 5, b"PK\x03\x04")),
    );
    let p = add(
        &mut b,
        "family-video.mp4",
        (35, 1),
        Content::Clusters(bytes(600_000, 6, &[0, 0, 0, 0x20, b'f', b't', b'y', b'p'])),
    );
    let _ = p;
    let partial = b.file(FileSpec::new(
        rec,
        "scan-contract.pdf",
        (33, 1),
        Content::Clusters(bytes(64_000, 7, b"%PDF-1.7")),
    ));
    rec += 1;
    b.mark(partial.lcn.unwrap() + 4, 6, true);
    let mut gone = FileSpec::new(
        rec,
        "old-backup.zip",
        (36, 9),
        Content::Clusters(bytes(120_000, 8, b"PK\x03\x04")),
    );
    gone.keep_allocated = true;
    b.file(gone);
    rec += 1;
    let z = b.file(FileSpec::new(
        rec,
        "IMG_0412.jpg",
        (35, 1),
        Content::Clusters(bytes(150_000, 9, &jpeg)),
    ));
    rec += 1;
    b.write(
        z.lcn.unwrap(),
        &vec![0u8; (z.clusters * u64::from(cs)) as usize],
    );
    b.file(FileSpec::new(
        rec,
        "notes-from-meeting.txt",
        (31, 1),
        Content::Runs(vec![], 4000),
    ));
    rec += 1;
    let mut enc = FileSpec::new(
        rec,
        "passwords.kdbx",
        (33, 1),
        Content::Clusters(bytes(9_000, 10, b"\x03\xd9\xa2\x9a")),
    );
    enc.attributes |= 0x4000;
    b.file(enc);
    std::fs::write(&out, b.build()).expect("write image");
    println!("wrote {out}");
}
