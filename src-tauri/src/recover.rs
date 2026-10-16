//! Recovery runs: guard the destination, then stream each selected item into a new run folder
//! through the writer, which hashes, verifies and reports.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use unlost_core::engine::Engine;
use unlost_core::guard;
use unlost_core::job::Job;
use unlost_core::protocol::{EntryView, ReadResult, Request};
use unlost_core::recycle::{self, BinItem};
use unlost_core::shadow::ShadowEntry;
use unlost_core::sources::{self, SourceSpec};
use unlost_core::writer::{Method, Produced, RecoveryRun, RunSummary};
use unlost_core::{Error, Result};
use unlost_formats::{Assessment, Progress, Reason, State};

/// Files collected from one shadow-copy folder selection.
const MAX_SHADOW_FILES: usize = 200_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverRequest {
    pub dest: String,
    #[serde(default)]
    pub confirm_unknown: bool,
    pub items: Items,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowPick {
    pub path: String,
    pub is_dir: bool,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Items {
    Ntfs {
        scan: u32,
        source: SourceSpec,
        records: Vec<u64>,
    },
    RecycleBin {
        ids: Vec<String>,
    },
    Shadow {
        device: String,
        volume: String,
        created: i64,
        picks: Vec<ShadowPick>,
    },
}

pub struct Ctx<'a> {
    pub engine: &'a Engine,
    pub sid: &'a str,
    pub version: &'a str,
    pub job: &'a Job,
}

fn dest_dir(dest: &str) -> Result<PathBuf> {
    let p = PathBuf::from(dest);
    if !p.is_absolute() || !p.is_dir() {
        return Err(Error::Refused(format!("{dest} is not an existing folder")));
    }
    Ok(p)
}

/// Checks every source the request touches against the destination.
pub fn check(req: &RecoverRequest, sid: &str) -> Result<()> {
    let dest = guard::disks_of_path(&dest_dir(&req.dest)?)?;
    for spec in sources_of(&req.items, sid) {
        guard::enforce(&sources::source_disks(&spec)?, &dest, req.confirm_unknown)?;
    }
    Ok(())
}

fn sources_of(items: &Items, sid: &str) -> Vec<SourceSpec> {
    match items {
        Items::Ntfs { source, .. } => vec![source.clone()],
        Items::Shadow { device, volume, .. } => vec![SourceSpec::Shadow {
            device: device.clone(),
            volume: volume.clone(),
        }],
        Items::RecycleBin { ids } => {
            let drives: BTreeSet<String> =
                bin_items(sid, ids).into_iter().map(|i| i.drive).collect();
            drives
                .into_iter()
                .map(|root| SourceSpec::Volume { root })
                .collect()
        }
    }
}

fn bin_items(sid: &str, ids: &[String]) -> Vec<BinItem> {
    let roots: Vec<String> = unlost_core::disks::drives()
        .into_iter()
        .map(|d| d.root)
        .collect();
    recycle::list(sid, &roots)
        .into_iter()
        .filter(|i| ids.contains(&i.id))
        .collect()
}

pub fn run(ctx: &Ctx, req: RecoverRequest) -> Result<RunSummary> {
    check(&req, ctx.sid)?;
    let dest = dest_dir(&req.dest)?;
    match req.items {
        Items::Ntfs {
            scan,
            source,
            records,
        } => ntfs(ctx, &dest, scan, &source, &records),
        Items::RecycleBin { ids } => bin(ctx, &dest, &ids),
        Items::Shadow {
            device,
            volume,
            created,
            picks,
        } => shadow(ctx, &dest, &device, &volume, created, &picks),
    }
}

fn folder_label(spec: &SourceSpec) -> String {
    match spec {
        SourceSpec::Volume { root } => root.trim_end_matches(['\\', ':']).to_string(),
        other => other.label(),
    }
}

fn ntfs(
    ctx: &Ctx,
    dest: &Path,
    scan: u32,
    source: &SourceSpec,
    records: &[u64],
) -> Result<RunSummary> {
    let admin = source.needs_admin();
    let rows: Vec<EntryView> = serde_json::from_value(
        ctx.engine
            .call(
                Request::Entries {
                    scan,
                    records: records.to_vec(),
                },
                admin,
            )?
            .wait(|_| {}, |_| Ok(()))?,
    )?;
    let label = source.label();
    let folder = folder_label(source);
    let mut run = RecoveryRun::start(dest, &label, ctx.version)?;
    ctx.job.stage("recover");
    ctx.job.set_total(rows.len() as u64);
    for row in rows {
        if ctx.job.is_cancelled() {
            break;
        }
        ctx.job.set_current(&row.name);
        let inner = if row.folder == "\\" {
            format!("\\{}", row.name)
        } else {
            format!("{}\\{}", row.folder, row.name)
        };
        let original = format!("{label}{inner}");
        let rel = format!("\\{folder}{inner}");
        let record = row.record;
        run.write(
            &original,
            &rel,
            Method::NtfsRecord,
            &row.assessment,
            |sink| {
                let read = Request::Read {
                    scan,
                    record,
                    sealed: None,
                };
                let call = ctx.engine.call(read, admin)?;
                let v = call.wait(
                    |_| {},
                    |b| {
                        if ctx.job.is_cancelled() {
                            return Err(Error::Cancelled);
                        }
                        ctx.job.add_bytes(b.len() as u64);
                        sink(b)
                    },
                )?;
                let r: ReadResult = serde_json::from_value(v)?;
                Ok(Produced {
                    assessment: r.assessment,
                    zero_clusters: r.stats.zero_clusters,
                })
            },
        );
        ctx.job.advance(1);
    }
    run.finish()
}

fn bin(ctx: &Ctx, dest: &Path, ids: &[String]) -> Result<RunSummary> {
    let items = bin_items(ctx.sid, ids);
    let mut run = RecoveryRun::start(dest, "Recycle Bin", ctx.version)?;
    let intact = Assessment::new(State::IntactCopy, Reason::RecycleBin);
    // Files to copy: (original path, relative output path, data path).
    let mut files: Vec<(String, String, PathBuf)> = Vec::new();
    for item in &items {
        let data = PathBuf::from(&item.data_path);
        if !recycle::validate_item(ctx.sid, &item.drive, &data) || !item.present {
            continue;
        }
        let rel_base = format!("\\{}", item.original_path.replacen(':', "", 1));
        if item.is_dir {
            walk(&data, &mut |p, rel| {
                files.push((
                    format!("{}\\{rel}", item.original_path),
                    format!("{rel_base}\\{rel}"),
                    p.to_path_buf(),
                ));
            });
        } else {
            files.push((item.original_path.clone(), rel_base, data));
        }
    }
    ctx.job.stage("recover");
    ctx.job.set_total(files.len() as u64);
    for (original, rel, from) in files {
        if ctx.job.is_cancelled() {
            break;
        }
        ctx.job.set_current(&original);
        let size = std::fs::metadata(&from).map(|m| m.len()).unwrap_or(0);
        run.copy_file(&original, &rel, Method::RecycleBin, &intact, &from);
        ctx.job.add_bytes(size);
        ctx.job.advance(1);
    }
    run.finish()
}

/// Every regular file under `dir`, with its path relative to `dir`. Links are not followed.
fn walk(dir: &Path, f: &mut dyn FnMut(&Path, &str)) {
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((d, prefix)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}\\{name}")
            };
            if ft.is_dir() {
                stack.push((e.path(), rel));
            } else if ft.is_file() {
                f(&e.path(), &rel);
            }
        }
    }
}

fn shadow(
    ctx: &Ctx,
    dest: &Path,
    device: &str,
    volume: &str,
    created: i64,
    picks: &[ShadowPick],
) -> Result<RunSummary> {
    let roots = unlost_core::disks::volume_roots(volume);
    let letter = roots
        .first()
        .map(|r| r.trim_end_matches(['\\', ':']).to_string())
        .unwrap_or_else(|| "volume".into());
    let stamp = chrono_like(created);
    let label = format!("{letter} shadow copy {stamp}");
    let mut run = RecoveryRun::start(dest, &label, ctx.version)?;
    let intact = Assessment::new(State::IntactCopy, Reason::ShadowCopy { created });
    let mut files: Vec<String> = Vec::new();
    for pick in picks {
        if !pick.is_dir {
            files.push(pick.path.clone());
            continue;
        }
        let mut stack = vec![pick.path.clone()];
        while let Some(dir) = stack.pop() {
            if files.len() >= MAX_SHADOW_FILES || ctx.job.is_cancelled() {
                break;
            }
            let entries: Vec<ShadowEntry> = serde_json::from_value(
                ctx.engine
                    .call(
                        Request::ShadowDir {
                            device: device.into(),
                            path: dir.clone(),
                        },
                        true,
                    )?
                    .wait(|_| {}, |_| Ok(()))?,
            )?;
            for e in entries {
                let p = format!("{}\\{}", dir.trim_end_matches('\\'), e.name);
                if e.is_dir {
                    stack.push(p);
                } else {
                    files.push(p);
                }
            }
        }
    }
    ctx.job.stage("recover");
    ctx.job.set_total(files.len() as u64);
    for path in files {
        if ctx.job.is_cancelled() {
            break;
        }
        ctx.job.set_current(&path);
        let rel_inner = if path.starts_with('\\') {
            path.clone()
        } else {
            format!("\\{path}")
        };
        let original = format!("{letter}:{rel_inner}");
        let rel = format!("\\{label}{rel_inner}");
        let intact_c = intact.clone();
        run.write(&original, &rel, Method::ShadowCopy, &intact, |sink| {
            ctx.engine
                .call(
                    Request::ShadowRead {
                        device: device.into(),
                        path: path.clone(),
                    },
                    true,
                )?
                .wait(
                    |_| {},
                    |b| {
                        if ctx.job.is_cancelled() {
                            return Err(Error::Cancelled);
                        }
                        ctx.job.add_bytes(b.len() as u64);
                        sink(b)
                    },
                )?;
            Ok(Produced {
                assessment: intact_c,
                zero_clusters: 0,
            })
        });
        ctx.job.advance(1);
    }
    run.finish()
}

/// `YYYY-MM-DD HHMM` in UTC for folder names, without a date crate in this crate.
fn chrono_like(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}{:02}",
        secs / 3600,
        (secs % 3600) / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_labels() {
        assert_eq!(chrono_like(1_792_141_200), "2026-10-16 0900");
        assert_eq!(chrono_like(0), "1970-01-01 0000");
        assert_eq!(
            folder_label(&SourceSpec::Volume {
                root: "D:\\".into()
            }),
            "D"
        );
        let walked = {
            let tmp = std::env::temp_dir().join(format!("unlost-walk-{}", std::process::id()));
            std::fs::create_dir_all(tmp.join("a").join("b")).unwrap();
            std::fs::write(tmp.join("a").join("b").join("x.txt"), b"x").unwrap();
            std::fs::write(tmp.join("y.txt"), b"y").unwrap();
            let mut out = Vec::new();
            walk(&tmp, &mut |_, rel| out.push(rel.to_string()));
            std::fs::remove_dir_all(&tmp).unwrap();
            out.sort();
            out
        };
        assert_eq!(walked, vec![r"a\b\x.txt".to_string(), "y.txt".to_string()]);
    }
}
