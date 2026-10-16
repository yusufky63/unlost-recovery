//! Writing recovered files and the recovery report.
//!
//! Runs in the non-elevated UI process, only after the destination guard approved the folder.
//! Every run gets its own new folder; nothing that exists is ever overwritten (files are
//! created with `create_new` and renamed into place without replacement). Each file is hashed
//! while written and hashed again from disk afterwards. A file whose content is known to be
//! damaged gets a `PARTIAL_`, `ZEROED_` or `OVERWRITTEN_` prefix, so it is never mistaken for a
//! good copy. Recovered files are never opened or executed.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unlost_formats::{Assessment, State};
use windows::Win32::Storage::FileSystem::{MOVE_FILE_FLAGS, MoveFileExW};
use windows::core::PCWSTR;

use crate::winstr::wide;
use crate::{Error, Result};

const REPORT_JSON: &str = "unlost-report.json";
const REPORT_CSV: &str = "unlost-report.csv";
const MAX_SUFFIX: u32 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Method {
    RecycleBin,
    ShadowCopy,
    NtfsRecord,
}

impl Method {
    fn label(self) -> &'static str {
        match self {
            Method::RecycleBin => "Recycle Bin",
            Method::ShadowCopy => "Shadow copy",
            Method::NtfsRecord => "NTFS file record",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportLine {
    pub original: String,
    /// Path relative to the run folder; empty when nothing was written.
    pub recovered: String,
    pub size: u64,
    pub method: Method,
    pub state: State,
    pub reason_code: String,
    pub reason: String,
    pub assessment: Assessment,
    pub sha256: String,
    /// The file was read back from the destination and hashed to the same value.
    pub verified_write: bool,
    pub zero_clusters: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub folder: String,
    pub source: String,
    pub started: String,
    pub finished: String,
    pub written: u64,
    pub failed: u64,
    pub bytes: u64,
    pub lines: Vec<ReportLine>,
    pub app_version: String,
}

pub struct RecoveryRun {
    root: PathBuf,
    source: String,
    started: String,
    lines: Vec<ReportLine>,
    app_version: String,
}

/// What a producer reports after streaming a file.
pub struct Produced {
    pub assessment: Assessment,
    pub zero_clusters: u64,
}

impl RecoveryRun {
    /// Creates `<dest>\Unlost <date time>` (with a suffix if it exists).
    pub fn start(dest: &Path, source: &str, app_version: &str) -> Result<Self> {
        let now = chrono::Local::now();
        let base = format!("Unlost {}", now.format("%Y-%m-%d %H%M%S"));
        let mut n = 1;
        let root = loop {
            let name = if n == 1 {
                base.clone()
            } else {
                format!("{base} ({n})")
            };
            let p = dest.join(&name);
            match std::fs::create_dir(&p) {
                Ok(()) => break p,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && n < 100 => n += 1,
                Err(e) => return Err(Error::io(&p, e)),
            }
        };
        Ok(RecoveryRun {
            root,
            source: source.to_string(),
            started: now.to_rfc3339(),
            lines: Vec::new(),
            app_version: app_version.to_string(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Writes one file at `rel` (a path inside the source, `\`-separated) from `produce`, which
    /// streams the bytes into the sink it is given.
    pub fn write(
        &mut self,
        original: &str,
        rel: &str,
        method: Method,
        pre: &Assessment,
        produce: impl FnOnce(&mut dyn FnMut(&[u8]) -> Result<()>) -> Result<Produced>,
    ) -> &ReportLine {
        let line = match self.write_inner(rel, produce) {
            Ok((path, size, sha, verified, produced)) => ReportLine {
                original: original.to_string(),
                recovered: path,
                size,
                method,
                state: produced.assessment.state,
                reason_code: produced.assessment.reason.code(),
                reason: produced.assessment.reason.english(),
                assessment: produced.assessment,
                sha256: sha,
                verified_write: verified,
                zero_clusters: produced.zero_clusters,
                error: None,
            },
            Err(e) => ReportLine {
                original: original.to_string(),
                recovered: String::new(),
                size: 0,
                method,
                state: pre.state,
                reason_code: pre.reason.code(),
                reason: pre.reason.english(),
                assessment: pre.clone(),
                sha256: String::new(),
                verified_write: false,
                zero_clusters: 0,
                error: Some(match &e {
                    Error::Cancelled => "cancelled".into(),
                    e => e.to_string(),
                }),
            },
        };
        self.lines.push(line);
        self.lines.last().expect("just pushed")
    }

    #[allow(clippy::type_complexity)]
    fn write_inner(
        &mut self,
        rel: &str,
        produce: impl FnOnce(&mut dyn FnMut(&[u8]) -> Result<()>) -> Result<Produced>,
    ) -> Result<(String, u64, String, bool, Produced)> {
        let parts = safe_parts(rel);
        let (name, dirs) = parts
            .split_last()
            .ok_or_else(|| Error::Refused("empty file name".into()))?;
        let mut dir = self.root.clone();
        for d in dirs {
            dir.push(d);
        }
        std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
        let tmp = unique(&dir, &format!("{name}.unlost-partial"))?;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| Error::io(&tmp, e))?;
        let mut out = BufWriter::with_capacity(1 << 20, file);
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        let produced = {
            let mut sink = |b: &[u8]| -> Result<()> {
                out.write_all(b).map_err(|e| Error::io(&tmp, e))?;
                hasher.update(b);
                size += b.len() as u64;
                Ok(())
            };
            produce(&mut sink)
        };
        let finish = out
            .into_inner()
            .map_err(|e| Error::io(&tmp, e.into_error()))
            .and_then(|f| f.sync_all().map_err(|e| Error::io(&tmp, e)));
        let produced = match produced.and_then(|p| finish.map(|_| p)) {
            Ok(p) => p,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        };
        let final_name = match produced.assessment.state {
            State::Partial => format!("PARTIAL_{name}"),
            State::Zeroed => format!("ZEROED_{name}"),
            State::Overwritten => format!("OVERWRITTEN_{name}"),
            _ => name.clone(),
        };
        let dest = rename_unique(&tmp, &dir, &final_name)?;
        let sha = hex(&hasher.finalize());
        let verified = hash_file(&dest).is_ok_and(|h| h == sha);
        let rel_out = dest
            .strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok((rel_out, size, sha, verified, produced))
    }

    /// Copies a whole file from a readable path (Recycle Bin `$R` data, a shadow copy path).
    pub fn copy_file(
        &mut self,
        original: &str,
        rel: &str,
        method: Method,
        pre: &Assessment,
        from: &Path,
    ) -> &ReportLine {
        let from = from.to_path_buf();
        let pre_c = pre.clone();
        self.write(original, rel, method, pre, move |sink| {
            let mut f = File::open(&from).map_err(|e| Error::io(&from, e))?;
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = f.read(&mut buf).map_err(|e| Error::io(&from, e))?;
                if n == 0 {
                    break;
                }
                sink(&buf[..n])?;
            }
            Ok(Produced {
                assessment: pre_c,
                zero_clusters: 0,
            })
        })
    }

    /// Writes the JSON and CSV reports and returns the summary.
    pub fn finish(self) -> Result<RunSummary> {
        let written = self.lines.iter().filter(|l| l.error.is_none()).count() as u64;
        let summary = RunSummary {
            folder: self.root.to_string_lossy().into_owned(),
            source: self.source,
            started: self.started,
            finished: chrono::Local::now().to_rfc3339(),
            written,
            failed: self.lines.len() as u64 - written,
            bytes: self.lines.iter().map(|l| l.size).sum(),
            lines: self.lines,
            app_version: self.app_version,
        };
        let json = self.root.join(REPORT_JSON);
        write_new(&json, &serde_json::to_vec_pretty(&summary)?)?;
        let csv = self.root.join(REPORT_CSV);
        write_new(&csv, csv_report(&summary).as_bytes())?;
        Ok(summary)
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| Error::io(path, e))?;
    f.write_all(bytes).map_err(|e| Error::io(path, e))
}

fn csv_cell(s: &str) -> String {
    // Leading =, +, - or @ would run as a formula in spreadsheet apps.
    let s = if s.starts_with(['=', '+', '-', '@']) {
        format!("'{s}")
    } else {
        s.to_string()
    };
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub fn csv_report(s: &RunSummary) -> String {
    // UTF-8 with BOM so spreadsheet apps read Turkish names correctly.
    let mut out = String::from(
        "\u{feff}original,recovered,size,method,state,reason,sha256,verified_write,zero_clusters,error\r\n",
    );
    for l in &s.lines {
        let row = [
            csv_cell(&l.original),
            csv_cell(&l.recovered),
            l.size.to_string(),
            csv_cell(l.method.label()),
            csv_cell(l.state.label()),
            csv_cell(&l.reason),
            csv_cell(&l.sha256),
            l.verified_write.to_string(),
            l.zero_clusters.to_string(),
            csv_cell(l.error.as_deref().unwrap_or("")),
        ];
        out.push_str(&row.join(","));
        out.push_str("\r\n");
    }
    out
}

pub fn hash_file(path: &Path) -> Result<String> {
    let mut f = File::open(path).map_err(|e| Error::io(path, e))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| Error::io(path, e))?;
        if n == 0 {
            return Ok(hex(&h.finalize()));
        }
        h.update(&buf[..n]);
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// One path component made safe for any Windows folder: invalid characters become `_`,
/// trailing dots and spaces go, reserved device names get a leading `_`.
pub fn safe_name(s: &str) -> String {
    let mut n: String = s
        .chars()
        .map(|c| {
            if c < ' ' || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    while n.ends_with('.') || n.ends_with(' ') {
        n.pop();
    }
    if n.is_empty() || n == "." || n == ".." {
        return "_".into();
    }
    let stem = n.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        n.insert(0, '_');
    }
    if n.chars().count() > 240 {
        n = n.chars().take(240).collect();
    }
    n
}

/// `\a\b\c.txt` -> safe components.
pub fn safe_parts(rel: &str) -> Vec<String> {
    rel.split(['\\', '/'])
        .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        .map(safe_name)
        .collect()
}

fn with_suffix(name: &str, n: u32) -> String {
    if n == 1 {
        return name.to_string();
    }
    match name.rfind('.') {
        Some(i) if i > 0 => format!("{} ({n}){}", &name[..i], &name[i..]),
        _ => format!("{name} ({n})"),
    }
}

fn unique(dir: &Path, name: &str) -> Result<PathBuf> {
    for n in 1..MAX_SUFFIX {
        let p = dir.join(with_suffix(name, n));
        if std::fs::symlink_metadata(&p).is_err() {
            return Ok(p);
        }
    }
    Err(Error::Other(format!("no free name for {name}")))
}

/// Renames without ever replacing an existing file (`MoveFileExW` without
/// `MOVEFILE_REPLACE_EXISTING` fails if the target exists).
fn rename_unique(from: &Path, dir: &Path, name: &str) -> Result<PathBuf> {
    let w_from = wide(from);
    for n in 1..MAX_SUFFIX {
        let to = dir.join(with_suffix(name, n));
        let w_to = wide(&to);
        match unsafe {
            MoveFileExW(
                PCWSTR(w_from.as_ptr()),
                PCWSTR(w_to.as_ptr()),
                MOVE_FILE_FLAGS(0),
            )
        } {
            Ok(()) => return Ok(to),
            Err(e) if e.code().0 as u32 == 0x8007_00B7 || e.code().0 as u32 == 0x8007_0050 => {
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(Error::Other(format!("no free name for {name}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use unlost_formats::Reason;

    fn intact() -> Assessment {
        Assessment::new(State::LikelyIntact, Reason::ResidentIntact)
    }

    #[test]
    fn names_are_safe() {
        assert_eq!(safe_name("a<b>c:d"), "a_b_c_d");
        assert_eq!(safe_name("CON"), "_CON");
        assert_eq!(safe_name("con.txt"), "_con.txt");
        assert_eq!(safe_name("file. . "), "file");
        assert_eq!(safe_name(".."), "_");
        assert_eq!(
            safe_parts(r"\Lost folder #32\..\x\Çağrı.txt"),
            vec!["Lost folder #32", "x", "Çağrı.txt"]
        );
        assert_eq!(with_suffix("a.txt", 2), "a (2).txt");
        assert_eq!(with_suffix(".bashrc", 3), ".bashrc (3)");
        assert_eq!(csv_cell("=cmd|' /C calc'!A0"), "\"'=cmd|' /C calc'!A0\"");
    }

    #[test]
    fn writes_hashes_never_overwrites_and_reports() {
        let tmp = tempfile::tempdir().unwrap();
        let mut run = RecoveryRun::start(tmp.path(), "D:", "0.1.0").unwrap();
        let root = run.root().to_path_buf();
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("Unlost ")
        );
        let second = RecoveryRun::start(tmp.path(), "D:", "0.1.0").unwrap();
        assert_ne!(second.root(), root, "a second run never reuses the folder");

        let body = b"hello world".to_vec();
        let b2 = body.clone();
        let l = run.write(
            r"D:\Docs\a.txt",
            r"\Docs\a.txt",
            Method::NtfsRecord,
            &intact(),
            move |sink| {
                sink(&b2[..5])?;
                sink(&b2[5..])?;
                Ok(Produced {
                    assessment: intact(),
                    zero_clusters: 0,
                })
            },
        );
        assert!(l.error.is_none());
        assert!(l.verified_write);
        assert_eq!(
            l.sha256,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(
            std::fs::read(root.join("Docs").join("a.txt")).unwrap(),
            body
        );

        // Same name again: suffixed, the first file untouched.
        let l = run.write(
            r"D:\Docs\a.txt",
            r"\Docs\a.txt",
            Method::NtfsRecord,
            &intact(),
            |sink| {
                sink(b"other")?;
                Ok(Produced {
                    assessment: Assessment::new(
                        State::Partial,
                        Reason::ClustersReused {
                            reused: 1,
                            total: 2,
                        },
                    ),
                    zero_clusters: 0,
                })
            },
        );
        assert_eq!(l.recovered, r"Docs\PARTIAL_a.txt");
        let l = run.write(
            r"D:\Docs\a.txt",
            r"\Docs\a.txt",
            Method::NtfsRecord,
            &intact(),
            |sink| {
                sink(b"third")?;
                Ok(Produced {
                    assessment: intact(),
                    zero_clusters: 0,
                })
            },
        );
        assert_eq!(l.recovered, r"Docs\a (2).txt");
        assert_eq!(
            std::fs::read(root.join("Docs").join("a.txt")).unwrap(),
            body
        );

        // A failing producer leaves no partial file behind.
        let l = run.write(
            r"D:\b.bin",
            r"\b.bin",
            Method::NtfsRecord,
            &intact(),
            |sink| {
                sink(b"part")?;
                Err(Error::Cancelled)
            },
        );
        assert_eq!(l.error.as_deref(), Some("cancelled"));
        assert!(!root.join("b.bin").exists());
        assert!(
            std::fs::read_dir(&root)
                .unwrap()
                .flatten()
                .all(|e| !e.file_name().to_string_lossy().contains("unlost-partial"))
        );

        let src = tmp.path().join("bin-data");
        std::fs::write(&src, b"from the bin").unwrap();
        let rb = Assessment::new(State::IntactCopy, Reason::RecycleBin);
        let l = run.copy_file(r"C:\x\y.txt", r"\x\y.txt", Method::RecycleBin, &rb, &src);
        assert!(l.verified_write);

        let summary = run.finish().unwrap();
        assert_eq!(summary.written, 4);
        assert_eq!(summary.failed, 1);
        let csv = std::fs::read_to_string(root.join(REPORT_CSV)).unwrap();
        assert!(csv.contains("Recycle Bin"));
        assert!(csv.contains("Intact copy"));
        let json: RunSummary =
            serde_json::from_slice(&std::fs::read(root.join(REPORT_JSON)).unwrap()).unwrap();
        assert_eq!(json.lines.len(), 5);
        // Reports are never overwritten either.
        assert!(write_new(&root.join(REPORT_CSV), b"x").is_err());
    }
}
