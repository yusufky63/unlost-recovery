//! The session database: one SQLite file per scan, in a folder the guard approved
//! ([`crate::workspace`]), so never on a physical disk the source uses. It lives in the UI
//! process: findings stream in as batches ([`unlost_formats::findings`]), from this process's
//! own scan or from the broker's pipe, and queries, selections and reads come out of it.
//!
//! The file is a cache. It is written with no journal and no syncs, deleted when the scan is
//! forgotten or the app exits, and swept on the next start if a crash left it behind.
//! `temp_store = MEMORY` keeps SQLite from creating temporary files, which would otherwise go
//! to `%TEMP%`, on the system drive.
//!
//! Rows are only appended while a scan runs, so writing keeps up with the `$MFT` reader.
//! There is no index on the files: an index kept up to date doubled the cost of every row,
//! and one built afterwards needs a sort over every row. A page of results is one pass over
//! the table with a sort that keeps only the rows up to that page, about 0.15 s per million
//! files, and memory stays bounded by the page cache.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, ToSql, params};
use unlost_formats::findings::{self, FolderRow, Message};
use unlost_formats::magic::{self, Category};
use unlost_formats::ntfs::{FileRef, PathStatus};
use unlost_formats::{Assessment, Reason, State};

use crate::protocol::{EntryView, Page, Query, SortKey};
use crate::session::fold;
use crate::{Error, Result};

const MAX_PAGE: u32 = 500;
const DEFAULT_PAGE: u32 = 200;
/// Page cache per database, in KiB. A larger cache made neither writing nor querying faster.
const CACHE_KIB: i64 = 8 * 1024;

/// `location` and `tag` come last: a query reading the other columns stops before them.
const SCHEMA: &str = "
PRAGMA page_size = 16384;
CREATE TABLE files (
    record INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    fold TEXT NOT NULL,
    folder INTEGER NOT NULL,
    links INTEGER NOT NULL,
    size INTEGER NOT NULL,
    created INTEGER NOT NULL,
    modified INTEGER NOT NULL,
    streams INTEGER NOT NULL,
    category INTEGER NOT NULL,
    state INTEGER NOT NULL,
    reason INTEGER NOT NULL,
    ra INTEGER NOT NULL,
    rb INTEGER NOT NULL,
    rtext TEXT,
    location BLOB NOT NULL,
    tag BLOB NOT NULL
);
CREATE TABLE folders (
    key INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    fold TEXT NOT NULL,
    status INTEGER NOT NULL
);
-- Each folder's place in alphabetical order, filled by `finish`: sorting files by folder then
-- looks up a small integer per file instead of a path.
CREATE TABLE folder_order (
    key INTEGER PRIMARY KEY,
    ord INTEGER NOT NULL
);
";

/// What a read needs: the scan's grade and where the content is.
pub struct Stored {
    pub assessment: Assessment,
    pub location: Vec<u8>,
    pub tag: Vec<u8>,
}

type Counts = (u64, BTreeMap<State, u64>);

pub struct Store {
    // Declared first so the connection closes before the file is deleted.
    conn: Mutex<Connection>,
    /// Counts of the whole scan, kept by `finish` for queries without a text or type filter.
    totals: Mutex<Option<Counts>>,
    _file: DbFile,
}

/// Deletes the database file, and its folder once empty.
struct DbFile(PathBuf);

impl Drop for DbFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        if let Some(dir) = self.0.parent() {
            // Only succeeds when nothing else is left in the folder.
            let _ = std::fs::remove_dir(dir);
        }
    }
}

impl Store {
    /// Creates `dir\name`. The caller has checked that `dir` is not on the source's disks.
    pub fn create(dir: &Path, name: &str) -> Result<Store> {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        let path = dir.join(name);
        // A file with this name was left by an earlier process with the same id.
        let _ = std::fs::remove_file(&path);
        let file = DbFile(path);
        let conn = Connection::open_with_flags(
            &file.0,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.execute_batch(&format!(
            "PRAGMA journal_mode = OFF;
             PRAGMA synchronous = OFF;
             PRAGMA temp_store = MEMORY;
             PRAGMA locking_mode = EXCLUSIVE;
             PRAGMA cache_size = -{CACHE_KIB};
             {SCHEMA}"
        ))?;
        Ok(Store {
            conn: Mutex::new(conn),
            totals: Mutex::new(None),
            _file: file,
        })
    }

    pub fn path(&self) -> &Path {
        &self._file.0
    }

    /// Writes one batch of findings. The bytes may come from the broker and are not trusted:
    /// the decoder checks them, and a bad batch fails without writing anything.
    pub fn ingest(&self, batch: &[u8]) -> Result<()> {
        let messages = findings::decode(batch)?;
        let mut c = self.conn.lock();
        let tx = c.transaction()?;
        {
            let mut file = tx.prepare_cached(
                "INSERT INTO files VALUES
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            )?;
            let mut folder = tx.prepare_cached("INSERT INTO folders VALUES (?1, ?2, ?3, ?4)")?;
            let mut regrade = tx.prepare_cached(
                "UPDATE files SET state = ?2, reason = ?3, ra = ?4, rb = ?5, rtext = ?6
                 WHERE record = ?1",
            )?;
            for m in &messages {
                match m {
                    Message::File(f) => {
                        let (code, a, b, text) = f.assessment.reason.to_parts();
                        file.execute(params![
                            f.record as i64,
                            f.name,
                            fold(&f.name),
                            f.folder as i64,
                            f.links,
                            clamp(f.size),
                            f.created,
                            f.modified,
                            f.streams,
                            magic::category(&magic::extension(&f.name)).index(),
                            f.assessment.state.rank(),
                            code,
                            a,
                            b,
                            text,
                            f.location,
                            f.tag,
                        ])?;
                    }
                    Message::Folder(FolderRow { key, path, status }) => {
                        folder.execute(params![
                            *key as i64,
                            path,
                            fold(path),
                            findings::status_code(*status)
                        ])?;
                    }
                    Message::Regrade { record, assessment } => {
                        let (code, a, b, text) = assessment.reason.to_parts();
                        regrade.execute(params![
                            *record as i64,
                            assessment.state.rank(),
                            code,
                            a,
                            b,
                            text
                        ])?;
                    }
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Called once every batch is in: orders the folders and counts the files. Returns the
    /// number of files and the per-state counts.
    pub fn finish(&self) -> Result<Counts> {
        let mut c = self.conn.lock();
        // Sorted here rather than by SQLite, whose sorter would hold whole rows in memory.
        let mut folders: Vec<(String, i64)> = c
            .prepare("SELECT fold, key FROM folders")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        folders.sort_unstable();
        let tx = c.transaction()?;
        {
            tx.execute("DELETE FROM folder_order", [])?;
            let mut put = tx.prepare("INSERT INTO folder_order VALUES (?1, ?2)")?;
            for (ord, (_, key)) in folders.iter().enumerate() {
                put.execute(params![key, ord as i64])?;
            }
        }
        tx.commit()?;
        drop(folders);
        let counts = count(&c, "1", &[])?;
        *self.totals.lock() = Some(counts.clone());
        Ok(counts)
    }

    pub fn query(&self, q: &Query) -> Result<Page> {
        let needle = fold(q.text.trim());
        let category = q.category.map(Category::index);
        // Text and type decide the per-state counts; the state filter only picks rows.
        let mut cond: Vec<&str> = Vec::new();
        let mut args: Vec<(&str, &dyn ToSql)> = Vec::new();
        if let Some(cat) = &category {
            cond.push("f.category = :category");
            args.push((":category", cat));
        }
        if !needle.is_empty() {
            cond.push(
                "(instr(f.fold, :needle) > 0
                  OR f.folder IN (SELECT key FROM folders WHERE instr(fold, :needle) > 0))",
            );
            args.push((":needle", &needle));
        }
        let filter = if cond.is_empty() {
            "1".to_string()
        } else {
            cond.join(" AND ")
        };
        let c = self.conn.lock();
        let cached = if cond.is_empty() {
            self.totals.lock().clone()
        } else {
            None
        };
        let (_, counts) = match cached {
            Some(t) => t,
            None => count(&c, &filter, &args)?,
        };
        let total = if q.states.is_empty() {
            counts.values().sum()
        } else {
            q.states.iter().filter_map(|s| counts.get(s)).sum()
        };
        let states = if q.states.is_empty() {
            String::new()
        } else {
            // Ranks are small numbers from our own enum, safe to write into the statement.
            let list: Vec<String> = q.states.iter().map(|s| s.rank().to_string()).collect();
            format!(" AND f.state IN ({})", list.join(","))
        };
        let d = if q.desc { "DESC" } else { "ASC" };
        // The record number breaks ties, so pages never overlap or skip a file.
        let (join, order) = match q.sort {
            SortKey::State if q.desc => ("", "f.state DESC, f.modified ASC, f.record DESC".into()),
            SortKey::State => ("", "f.state ASC, f.modified DESC, f.record ASC".into()),
            SortKey::Name => ("", format!("f.fold {d}, f.record {d}")),
            SortKey::Folder => (
                "LEFT JOIN folder_order o ON o.key = f.folder",
                format!("o.ord {d}, f.fold {d}, f.record {d}"),
            ),
            SortKey::Size => ("", format!("f.size {d}, f.record {d}")),
            SortKey::Modified => ("", format!("f.modified {d}, f.record {d}")),
        };
        let limit = match q.limit {
            0 => DEFAULT_PAGE,
            n => n.min(MAX_PAGE),
        };
        let records: Vec<i64> = {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT f.record FROM files f {join} WHERE {filter}{states}
                 ORDER BY {order} LIMIT :limit OFFSET :offset"
            ))?;
            let mut all = args.clone();
            all.push((":limit", &limit));
            all.push((":offset", &q.offset));
            stmt.query_map(all.as_slice(), |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut rows = Vec::with_capacity(records.len());
        for r in records {
            if let Some(v) = entry(&c, r)? {
                rows.push(v);
            }
        }
        Ok(Page {
            total,
            rows,
            counts,
        })
    }

    /// Rows for specific records, in the order asked; unknown records are skipped.
    pub fn entries(&self, records: &[u64]) -> Result<Vec<EntryView>> {
        let c = self.conn.lock();
        let mut out = Vec::with_capacity(records.len());
        for &r in records {
            if let Some(v) = entry(&c, r as i64)? {
                out.push(v);
            }
        }
        Ok(out)
    }

    pub fn content(&self, record: u64) -> Result<Option<Stored>> {
        let c = self.conn.lock();
        let mut stmt = c.prepare_cached(
            "SELECT state, reason, ra, rb, rtext, location, tag FROM files WHERE record = ?1",
        )?;
        let row = stmt
            .query_row([record as i64], |r| {
                Ok((assessment(r, 0)?, r.get(5)?, r.get(6)?))
            })
            .optional()?;
        Ok(row.map(|(a, location, tag)| Stored {
            assessment: a,
            location,
            tag,
        }))
    }
}

/// Files and per-state counts under `filter`, in one pass: a GROUP BY without an index would
/// sort every row in memory first.
fn count(c: &Connection, filter: &str, args: &[(&str, &dyn ToSql)]) -> Result<Counts> {
    let sums: Vec<String> = State::ALL
        .iter()
        .map(|s| format!("COALESCE(SUM(f.state = {}), 0)", s.rank()))
        .collect();
    let mut stmt = c.prepare_cached(&format!(
        "SELECT {} FROM files f WHERE {filter}",
        sums.join(", ")
    ))?;
    let per: Vec<i64> =
        stmt.query_row(args, |r| (0..State::ALL.len()).map(|i| r.get(i)).collect())?;
    let mut counts = BTreeMap::new();
    for (s, n) in State::ALL.iter().zip(per) {
        if n > 0 {
            counts.insert(*s, n as u64);
        }
    }
    Ok((counts.values().sum(), counts))
}

/// Sizes are u64; SQLite integers are i64. A size beyond i64 is damage anyway.
fn clamp(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn entry(c: &Connection, record: i64) -> Result<Option<EntryView>> {
    let mut stmt = c.prepare_cached(
        "SELECT f.state, f.reason, f.ra, f.rb, f.rtext,
                f.record, f.name, f.folder, f.links, f.size, f.created, f.modified, f.streams,
                d.path, d.status
         FROM files f LEFT JOIN folders d ON d.key = f.folder WHERE f.record = ?1",
    )?;
    let v = stmt
        .query_row([record], |r| {
            let name: String = r.get(6)?;
            let key: i64 = r.get(7)?;
            let path: Option<String> = r.get(13)?;
            let status: Option<u8> = r.get(14)?;
            // Every folder a file references gets a row; a missing one means a cut-off stream.
            let (folder, path_status) = match (path, status.and_then(findings::status_from)) {
                (Some(p), Some(s)) => (p, s),
                _ => (
                    format!("\\Lost folder #{}", FileRef::from_u64(key as u64).record),
                    PathStatus::Lost,
                ),
            };
            let ext = magic::extension(&name);
            Ok(EntryView {
                record: r.get::<_, i64>(5)? as u64,
                category: magic::category(&ext),
                ext,
                name,
                folder,
                path_status,
                size: r.get::<_, i64>(9)? as u64,
                created: r.get(10)?,
                modified: r.get(11)?,
                assessment: assessment(r, 0)?,
                links: r.get(8)?,
                streams: r.get(12)?,
            })
        })
        .optional()?;
    Ok(v)
}

/// The assessment stored in columns `at..at + 5`.
fn assessment(r: &Row<'_>, at: usize) -> rusqlite::Result<Assessment> {
    let rank: u8 = r.get(at)?;
    let code: u8 = r.get(at + 1)?;
    let text: Option<String> = r.get(at + 4)?;
    let bad = || rusqlite::Error::IntegralValueOutOfRange(at, i64::from(rank));
    let state = State::from_rank(rank).ok_or_else(bad)?;
    let reason = Reason::from_parts(code, r.get(at + 2)?, r.get(at + 3)?, text.as_deref())
        .ok_or_else(bad)?;
    Ok(Assessment::new(state, reason))
}

/// Writes rows straight from findings, for tests.
#[cfg(test)]
pub(crate) fn batch(messages: &[Message]) -> Vec<u8> {
    let mut b = Vec::new();
    for m in messages {
        findings::put(&mut b, m);
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use unlost_formats::findings::FileRow;

    fn file(
        record: u64,
        name: &str,
        folder: u64,
        state: State,
        size: u64,
        modified: i64,
    ) -> Message {
        Message::File(FileRow {
            record,
            name: name.into(),
            folder,
            links: 1,
            size,
            created: 0,
            modified,
            streams: 0,
            assessment: Assessment::new(state, Reason::NoRuns),
            location: vec![0],
            tag: Vec::new(),
        })
    }

    fn folder(key: u64, path: &str) -> Message {
        Message::Folder(FolderRow {
            key,
            path: path.into(),
            status: PathStatus::Live,
        })
    }

    fn names(p: &Page) -> Vec<&str> {
        p.rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn queries_sort_filter_count_and_page() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("scan data");
        let store = Store::create(&dir, "scan-test.sqlite").unwrap();
        store
            .ingest(&batch(&[
                file(10, "b.jpg", 1, State::LikelyIntact, 300, 50),
                file(11, "A.txt", 2, State::Partial, 100, 70),
                file(12, "c.jpg", 1, State::LikelyIntact, 200, 60),
                file(13, "Iğdır.docx", 2, State::MetadataOnly, 0, 10),
            ]))
            .unwrap();
        store
            .ingest(&batch(&[
                Message::Regrade {
                    record: 10,
                    assessment: Assessment::new(State::Zeroed, Reason::AllZero { sampled: 3 }),
                },
                folder(1, r"\Photos"),
                folder(2, r"\Belgeler\Şirket"),
            ]))
            .unwrap();
        let (total, counts) = store.finish().unwrap();
        assert_eq!(total, 4);
        assert_eq!(counts.get(&State::Zeroed), Some(&1));
        assert_eq!(counts.get(&State::LikelyIntact), Some(&1));

        // Best first, newest first within a state.
        let all = store.query(&Query::default()).unwrap();
        assert_eq!(names(&all), ["c.jpg", "A.txt", "b.jpg", "Iğdır.docx"]);
        assert_eq!(all.total, 4);
        assert_eq!(
            all.rows[2].assessment.reason,
            Reason::AllZero { sampled: 3 }
        );
        assert_eq!(all.rows[0].folder, r"\Photos");
        assert_eq!(all.rows[0].category, Category::Photo);

        let by = |sort, desc| Query {
            sort,
            desc,
            ..Query::default()
        };
        assert_eq!(
            names(&store.query(&by(SortKey::Name, false)).unwrap()),
            ["A.txt", "b.jpg", "c.jpg", "Iğdır.docx"]
        );
        assert_eq!(
            names(&store.query(&by(SortKey::Size, true)).unwrap()),
            ["b.jpg", "c.jpg", "A.txt", "Iğdır.docx"]
        );
        assert_eq!(
            names(&store.query(&by(SortKey::Folder, false)).unwrap()),
            ["A.txt", "Iğdır.docx", "b.jpg", "c.jpg"]
        );
        assert_eq!(
            names(&store.query(&by(SortKey::Modified, false)).unwrap()),
            ["Iğdır.docx", "b.jpg", "c.jpg", "A.txt"]
        );
        assert_eq!(
            names(&store.query(&by(SortKey::State, true)).unwrap()),
            ["Iğdır.docx", "b.jpg", "A.txt", "c.jpg"]
        );

        // Text matches names and folders, Turkish-insensitive; counts follow text and type.
        let q = Query {
            text: "ŞİRKET".into(),
            states: vec![State::Partial],
            ..Query::default()
        };
        let p = store.query(&q).unwrap();
        assert_eq!(names(&p), ["A.txt"]);
        assert_eq!(p.total, 1);
        assert_eq!(p.counts.get(&State::MetadataOnly), Some(&1));
        let q = Query {
            text: "ığdır".into(),
            ..Query::default()
        };
        assert_eq!(names(&store.query(&q).unwrap()), ["Iğdır.docx"]);
        let q = Query {
            category: Some(Category::Photo),
            ..Query::default()
        };
        let p = store.query(&q).unwrap();
        assert_eq!(p.total, 2);
        assert_eq!(p.counts.get(&State::Partial), None);

        // Paging.
        let q = Query {
            offset: 1,
            limit: 2,
            ..Query::default()
        };
        let p = store.query(&q).unwrap();
        assert_eq!(names(&p), ["A.txt", "b.jpg"]);
        assert_eq!(p.total, 4);

        let rows = store.entries(&[13, 99, 10]).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Iğdır.docx");
        let c = store.content(11).unwrap().unwrap();
        assert_eq!(c.location, vec![0]);
        assert_eq!(c.assessment.state, State::Partial);
        assert!(store.content(99).unwrap().is_none());

        // A damaged batch writes nothing.
        let mut bad = batch(&[file(20, "x", 1, State::Partial, 1, 1)]);
        bad.push(77);
        assert!(store.ingest(&bad).is_err());
        assert!(store.content(20).unwrap().is_none());

        // The file and its folder go away with the store.
        let path = store.path().to_path_buf();
        assert!(path.is_file());
        drop(store);
        assert!(!path.exists());
        assert!(!dir.exists());
    }

    #[test]
    fn keeps_to_one_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::create(tmp.path(), "scan-big.sqlite").unwrap();
        // 2 = MEMORY: sorts and temporary tables never touch %TEMP%.
        let temp_store: i64 = store
            .conn
            .lock()
            .pragma_query_value(None, "temp_store", |r| r.get(0))
            .unwrap();
        assert_eq!(temp_store, 2);
        let mut msgs = Vec::new();
        for i in 0..20_000u64 {
            msgs.push(file(
                i,
                &format!("file {}.bin", 20_000 - i),
                1,
                State::ALL[(i % 8) as usize],
                i * 7 % 1000,
                (i * 31 % 977) as i64,
            ));
        }
        msgs.push(folder(1, r"\x"));
        store.ingest(&batch(&msgs)).unwrap();
        for sort in [SortKey::Name, SortKey::Folder, SortKey::Size] {
            let p = store
                .query(&Query {
                    sort,
                    text: "file".into(),
                    offset: 10_000,
                    ..Query::default()
                })
                .unwrap();
            assert_eq!(p.total, 20_000);
            assert_eq!(p.rows.len(), 200);
        }
        // Only the database itself sits in the folder: no journal, no temporary files.
        let files: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().flatten().collect();
        assert_eq!(files.len(), 1, "{files:?}");
    }
}
