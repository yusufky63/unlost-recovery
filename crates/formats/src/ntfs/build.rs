//! Synthetic NTFS volumes for tests and fuzz seeds.
//!
//! Builds just enough of a volume for the scanner: a boot sector, an `$MFT` with update
//! sequence fixups, `$Volume`, the root folder, `$Bitmap`, and whatever folders and files a
//! test adds, live or deleted. Real Windows-made images are covered by the VHDX scenario suite.

use crate::ntfs::record::{
    ATTR_ATTRIBUTE_LIST, ATTR_DATA, ATTR_FILE_NAME, ATTR_REPARSE_POINT, ATTR_STANDARD_INFORMATION,
    ATTR_VOLUME_NAME, NAMESPACE_WIN32,
};
use crate::ntfs::runlist;

pub const T2024: u64 = 133_485_408_000_000_000;

pub struct ImageBuilder {
    pub cluster: u32,
    pub record: u32,
    pub clusters: u64,
    mft_lcn: u64,
    bitmap_lcn: u64,
    records: Vec<Option<Vec<u8>>>,
    data: Vec<u8>,
    bitmap: Vec<u8>,
    next_lcn: u64,
}

#[derive(Clone)]
pub enum Content {
    Resident(Vec<u8>),
    /// Data written at freshly allocated clusters.
    Clusters(Vec<u8>),
    /// Explicit runs `(lcn, clusters)` and a size; nothing is written.
    Runs(Vec<(Option<u64>, u64)>, u64),
    /// No `$DATA` attribute at all.
    None,
}

#[derive(Clone)]
pub struct FileSpec {
    pub record: u64,
    pub sequence: u16,
    pub parent: (u64, u16),
    pub name: String,
    pub in_use: bool,
    pub content: Content,
    pub attributes: u32,
    pub data_flags: u16,
    pub reparse: Option<u32>,
    /// Leave the clusters marked allocated even if the file is deleted (simulates reuse).
    pub keep_allocated: bool,
    pub tear: bool,
    pub extra_streams: u32,
}

impl FileSpec {
    pub fn new(record: u64, name: &str, parent: (u64, u16), content: Content) -> Self {
        FileSpec {
            record,
            sequence: 2,
            parent,
            name: name.into(),
            in_use: false,
            content,
            attributes: 0x20,
            data_flags: 0,
            reparse: None,
            keep_allocated: false,
            tear: false,
            extra_streams: 0,
        }
    }
}

pub struct Placed {
    pub lcn: Option<u64>,
    pub clusters: u64,
}

impl ImageBuilder {
    pub fn new(clusters: u64, cluster: u32, record: u32, mft_records: u64) -> Self {
        let mft_bytes = mft_records * u64::from(record);
        let mft_clusters = mft_bytes.div_ceil(u64::from(cluster));
        let bitmap_bytes = clusters.div_ceil(8);
        let bitmap_clusters = bitmap_bytes.div_ceil(u64::from(cluster));
        let mft_lcn = 4;
        let bitmap_lcn = mft_lcn + mft_clusters;
        let mut b = ImageBuilder {
            cluster,
            record,
            clusters,
            mft_lcn,
            bitmap_lcn,
            records: vec![None; mft_records as usize],
            data: vec![0u8; (clusters * u64::from(cluster)) as usize],
            bitmap: vec![0u8; bitmap_bytes as usize],
            next_lcn: bitmap_lcn + bitmap_clusters,
        };
        b.mark(0, mft_lcn, true);
        b.mark(mft_lcn, mft_clusters, true);
        b.mark(bitmap_lcn, bitmap_clusters, true);
        let mft_runs = runlist::encode(&[(Some(mft_lcn), mft_clusters)]);
        b.put_system(
            0,
            "$MFT",
            false,
            vec![nonresident(
                ATTR_DATA,
                "",
                0,
                0,
                mft_clusters - 1,
                &mft_runs,
                mft_bytes,
                mft_bytes,
            )],
        );
        let label: Vec<u8> = "UNLOSTTEST"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        b.put_system(
            3,
            "$Volume",
            false,
            vec![resident(ATTR_VOLUME_NAME, "", 0, &label)],
        );
        b.put_system(5, ".", true, vec![]);
        let bm_runs = runlist::encode(&[(Some(bitmap_lcn), bitmap_clusters)]);
        b.put_system(
            6,
            "$Bitmap",
            false,
            vec![nonresident(
                ATTR_DATA,
                "",
                0,
                0,
                bitmap_clusters - 1,
                &bm_runs,
                bitmap_bytes,
                bitmap_bytes,
            )],
        );
        b
    }

    fn put_system(&mut self, rec: u64, name: &str, dir: bool, mut extra: Vec<Vec<u8>>) {
        let mut attrs = vec![
            resident(ATTR_STANDARD_INFORMATION, "", 0, &std_info(0x06, T2024)),
            resident(
                ATTR_FILE_NAME,
                "",
                0,
                &file_name((5, 5), name, NAMESPACE_WIN32, 0),
            ),
        ];
        attrs.append(&mut extra);
        let flags = if dir { 3 } else { 1 };
        // The root's sequence number is 5 by convention, the other metafiles start at 1.
        let seq = if rec == 5 { 5 } else { 1 };
        self.records[rec as usize] = Some(encode_record(seq, flags, 0, &attrs, self.record));
    }

    pub fn mark(&mut self, lcn: u64, n: u64, used: bool) {
        for c in lcn..lcn + n {
            let (byte, bit) = ((c / 8) as usize, c % 8);
            if used {
                self.bitmap[byte] |= 1 << bit;
            } else {
                self.bitmap[byte] &= !(1 << bit);
            }
        }
    }

    pub fn alloc(&mut self, n: u64) -> u64 {
        let lcn = self.next_lcn;
        self.next_lcn += n;
        assert!(self.next_lcn <= self.clusters, "test volume full");
        lcn
    }

    pub fn write(&mut self, lcn: u64, bytes: &[u8]) {
        let off = (lcn * u64::from(self.cluster)) as usize;
        self.data[off..off + bytes.len()].copy_from_slice(bytes);
    }

    pub fn dir(
        &mut self,
        record: u64,
        sequence: u16,
        parent: (u64, u16),
        name: &str,
        in_use: bool,
    ) {
        let attrs = vec![
            resident(ATTR_STANDARD_INFORMATION, "", 0, &std_info(0x10, T2024)),
            resident(
                ATTR_FILE_NAME,
                "",
                0,
                &file_name(parent, name, NAMESPACE_WIN32, 0),
            ),
        ];
        let flags = if in_use { 3 } else { 2 };
        self.records[record as usize] =
            Some(encode_record(sequence, flags, 0, &attrs, self.record));
    }

    pub fn file(&mut self, spec: FileSpec) -> Placed {
        let cs = u64::from(self.cluster);
        let mut placed = Placed {
            lcn: None,
            clusters: 0,
        };
        let mut attrs = vec![
            resident(
                ATTR_STANDARD_INFORMATION,
                "",
                0,
                &std_info(spec.attributes, T2024),
            ),
            resident(
                ATTR_FILE_NAME,
                "",
                0,
                &file_name(spec.parent, &spec.name, NAMESPACE_WIN32, 0),
            ),
        ];
        match &spec.content {
            Content::Resident(v) => attrs.push(resident(ATTR_DATA, "", spec.data_flags, v)),
            Content::Clusters(v) => {
                let n = (v.len() as u64).div_ceil(cs).max(1);
                let lcn = self.alloc(n);
                self.write(lcn, v);
                self.mark(lcn, n, spec.in_use || spec.keep_allocated);
                let runs = runlist::encode(&[(Some(lcn), n)]);
                attrs.push(nonresident(
                    ATTR_DATA,
                    "",
                    spec.data_flags,
                    0,
                    n - 1,
                    &runs,
                    v.len() as u64,
                    v.len() as u64,
                ));
                placed = Placed {
                    lcn: Some(lcn),
                    clusters: n,
                };
            }
            Content::Runs(runs, size) => {
                let total: u64 = runs.iter().map(|r| r.1).sum();
                let enc = runlist::encode(runs);
                attrs.push(nonresident(
                    ATTR_DATA,
                    "",
                    spec.data_flags,
                    0,
                    total.max(1) - 1,
                    &enc,
                    *size,
                    *size,
                ));
            }
            Content::None => {}
        }
        for i in 0..spec.extra_streams {
            attrs.push(resident(ATTR_DATA, &format!("s{i}"), 0, b"ads"));
        }
        if let Some(tag) = spec.reparse {
            let mut v = tag.to_le_bytes().to_vec();
            v.extend_from_slice(&[0u8; 4]);
            attrs.push(resident(ATTR_REPARSE_POINT, "", 0, &v));
        }
        let flags = u16::from(spec.in_use);
        let mut rec = encode_record(spec.sequence, flags, 0, &attrs, self.record);
        if spec.tear {
            let at = (self.record as usize) - 2;
            rec[at] ^= 0xFF;
        }
        self.records[spec.record as usize] = Some(rec);
        placed
    }

    /// A deleted base record whose `$DATA` lives in a deleted extension record.
    pub fn split_file(
        &mut self,
        base: u64,
        ext: u64,
        parent: (u64, u16),
        name: &str,
        content: &[u8],
    ) {
        let cs = u64::from(self.cluster);
        let n = (content.len() as u64).div_ceil(cs).max(1);
        let lcn = self.alloc(n);
        self.write(lcn, content);
        let mut list = Vec::new();
        for (kind, rec) in [
            (ATTR_STANDARD_INFORMATION, base),
            (ATTR_FILE_NAME, base),
            (ATTR_DATA, ext),
        ] {
            let mut e = vec![0u8; 0x20];
            e[0..4].copy_from_slice(&kind.to_le_bytes());
            e[4..6].copy_from_slice(&0x20u16.to_le_bytes());
            e[7] = 0x1A;
            let r = rec | (2u64 << 48);
            e[0x10..0x18].copy_from_slice(&r.to_le_bytes());
            list.extend_from_slice(&e);
        }
        let attrs = vec![
            resident(ATTR_STANDARD_INFORMATION, "", 0, &std_info(0x20, T2024)),
            resident(ATTR_ATTRIBUTE_LIST, "", 0, &list),
            resident(
                ATTR_FILE_NAME,
                "",
                0,
                &file_name(parent, name, NAMESPACE_WIN32, 0),
            ),
        ];
        // Freed: base sequence 3 (was 2), the extension still points at sequence 2.
        self.records[base as usize] = Some(encode_record(3, 0, 0, &attrs, self.record));
        let runs = runlist::encode(&[(Some(lcn), n)]);
        let ext_attrs = vec![nonresident(
            ATTR_DATA,
            "",
            0,
            0,
            n - 1,
            &runs,
            content.len() as u64,
            content.len() as u64,
        )];
        self.records[ext as usize] = Some(encode_record(
            3,
            0,
            base | (2u64 << 48),
            &ext_attrs,
            self.record,
        ));
    }

    pub fn build(mut self) -> Vec<u8> {
        let cs = self.cluster as usize;
        let mut boot = vec![0u8; 512];
        boot[0..3].copy_from_slice(&[0xEB, 0x52, 0x90]);
        boot[3..11].copy_from_slice(b"NTFS    ");
        boot[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes());
        boot[0x0D] = (cs / 512) as u8;
        boot[0x28..0x30].copy_from_slice(&(self.clusters * (cs as u64 / 512)).to_le_bytes());
        boot[0x30..0x38].copy_from_slice(&self.mft_lcn.to_le_bytes());
        boot[0x38..0x40].copy_from_slice(&(self.mft_lcn + 1).to_le_bytes());
        boot[0x40] = if self.record as usize >= cs {
            (self.record as usize / cs) as u8
        } else {
            (-(self.record.trailing_zeros() as i8)) as u8
        };
        boot[0x44] = 1;
        boot[0x48..0x50].copy_from_slice(&0x1234_5678_9ABC_DEF0u64.to_le_bytes());
        boot[510] = 0x55;
        boot[511] = 0xAA;
        self.data[0..512].copy_from_slice(&boot);
        let mft_off = (self.mft_lcn as usize) * cs;
        let rs = self.record as usize;
        for (i, r) in self.records.iter().enumerate() {
            if let Some(r) = r {
                self.data[mft_off + i * rs..mft_off + (i + 1) * rs].copy_from_slice(r);
            }
        }
        let bm_off = (self.bitmap_lcn as usize) * cs;
        self.data[bm_off..bm_off + self.bitmap.len()].copy_from_slice(&self.bitmap);
        self.data
    }
}

pub fn std_info(attributes: u32, t: u64) -> Vec<u8> {
    let mut v = vec![0u8; 0x48];
    for i in 0..4 {
        v[i * 8..i * 8 + 8].copy_from_slice(&t.to_le_bytes());
    }
    v[0x20..0x24].copy_from_slice(&attributes.to_le_bytes());
    v
}

pub fn file_name(parent: (u64, u16), name: &str, namespace: u8, size: u64) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let mut v = vec![0u8; 0x42 + units.len() * 2];
    let pref = parent.0 | (u64::from(parent.1) << 48);
    v[0..8].copy_from_slice(&pref.to_le_bytes());
    for i in 1..5 {
        v[i * 8..i * 8 + 8].copy_from_slice(&T2024.to_le_bytes());
    }
    v[0x30..0x38].copy_from_slice(&size.to_le_bytes());
    v[0x40] = units.len() as u8;
    v[0x41] = namespace;
    for (i, u) in units.iter().enumerate() {
        v[0x42 + i * 2..0x44 + i * 2].copy_from_slice(&u.to_le_bytes());
    }
    v
}

fn name_bytes(name: &str) -> Vec<u8> {
    name.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

pub fn resident(kind: u32, name: &str, flags: u16, value: &[u8]) -> Vec<u8> {
    let nb = name_bytes(name);
    let voff = (0x18 + nb.len()).next_multiple_of(8);
    let len = (voff + value.len()).next_multiple_of(8);
    let mut a = vec![0u8; len];
    a[0..4].copy_from_slice(&kind.to_le_bytes());
    a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    a[9] = (nb.len() / 2) as u8;
    a[0x0A..0x0C].copy_from_slice(&0x18u16.to_le_bytes());
    a[0x0C..0x0E].copy_from_slice(&flags.to_le_bytes());
    a[0x10..0x14].copy_from_slice(&(value.len() as u32).to_le_bytes());
    a[0x14..0x16].copy_from_slice(&(voff as u16).to_le_bytes());
    a[0x18..0x18 + nb.len()].copy_from_slice(&nb);
    a[voff..voff + value.len()].copy_from_slice(value);
    a
}

#[allow(clippy::too_many_arguments)]
pub fn nonresident(
    kind: u32,
    name: &str,
    flags: u16,
    start_vcn: u64,
    last_vcn: u64,
    runs: &[u8],
    real: u64,
    init: u64,
) -> Vec<u8> {
    let nb = name_bytes(name);
    let roff = (0x40 + nb.len()).next_multiple_of(8);
    let len = (roff + runs.len()).next_multiple_of(8);
    let mut a = vec![0u8; len];
    a[0..4].copy_from_slice(&kind.to_le_bytes());
    a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    a[8] = 1;
    a[9] = (nb.len() / 2) as u8;
    a[0x0A..0x0C].copy_from_slice(&0x40u16.to_le_bytes());
    a[0x0C..0x0E].copy_from_slice(&flags.to_le_bytes());
    a[0x10..0x18].copy_from_slice(&start_vcn.to_le_bytes());
    a[0x18..0x20].copy_from_slice(&last_vcn.to_le_bytes());
    a[0x20..0x22].copy_from_slice(&(roff as u16).to_le_bytes());
    a[0x28..0x30].copy_from_slice(&real.next_multiple_of(4096).to_le_bytes());
    a[0x30..0x38].copy_from_slice(&real.to_le_bytes());
    a[0x38..0x40].copy_from_slice(&init.to_le_bytes());
    a[0x40..0x40 + nb.len()].copy_from_slice(&nb);
    a[roff..roff + runs.len()].copy_from_slice(runs);
    a
}

/// A FILE record with fixups applied the way NTFS stores it on disk.
pub fn encode_record(
    sequence: u16,
    flags: u16,
    base: u64,
    attrs: &[Vec<u8>],
    size: u32,
) -> Vec<u8> {
    let size = size as usize;
    let mut r = vec![0u8; size];
    let usa_count = size / 512 + 1;
    let first = (0x30 + usa_count * 2).next_multiple_of(8);
    r[0..4].copy_from_slice(b"FILE");
    r[4..6].copy_from_slice(&0x30u16.to_le_bytes());
    r[6..8].copy_from_slice(&(usa_count as u16).to_le_bytes());
    r[0x10..0x12].copy_from_slice(&sequence.to_le_bytes());
    r[0x12..0x14].copy_from_slice(&1u16.to_le_bytes());
    r[0x14..0x16].copy_from_slice(&(first as u16).to_le_bytes());
    r[0x16..0x18].copy_from_slice(&flags.to_le_bytes());
    r[0x20..0x28].copy_from_slice(&base.to_le_bytes());
    let mut pos = first;
    for a in attrs {
        assert!(pos + a.len() + 8 <= size, "record overflow");
        r[pos..pos + a.len()].copy_from_slice(a);
        pos += a.len();
    }
    r[pos..pos + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    pos += 8;
    r[0x18..0x1C].copy_from_slice(&(pos as u32).to_le_bytes());
    r[0x1C..0x20].copy_from_slice(&(size as u32).to_le_bytes());
    let usn = [0x07, 0x00];
    r[0x30..0x32].copy_from_slice(&usn);
    for i in 1..usa_count {
        let end = i * 512 - 2;
        let saved = [r[end], r[end + 1]];
        r[0x30 + i * 2..0x32 + i * 2].copy_from_slice(&saved);
        r[end..end + 2].copy_from_slice(&usn);
    }
    r
}
