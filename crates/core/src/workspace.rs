//! Where a scan keeps its session database.
//!
//! The database is written by the UI process (the broker never writes files) and follows the
//! destination guard's rule: never on a physical disk the source uses. By default it goes to
//! `sessions` in the app's data folder (`%LOCALAPPDATA%\Unlost`, or `data` next to a portable
//! exe). When that folder shares a disk with the source, which is the case for a scan of the
//! Windows drive with an installed app, the user picks a folder on another disk, such as the
//! one they will recover to, and the database goes to `Unlost scan data` inside it.

use std::path::{Path, PathBuf};

use crate::guard::{self, Verdict};
use crate::sources::{self, SourceSpec};
use crate::{Error, Result};

/// Subfolder created in a folder the user picked; removed again when it is empty.
pub const FOLDER_NAME: &str = "Unlost scan data";
const PREFIX: &str = "scan-";
const SUFFIX: &str = ".sqlite";

pub fn default_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sessions")
}

pub fn chosen_dir(folder: &Path) -> PathBuf {
    folder.join(FOLDER_NAME)
}

/// File name for a new database: the process id keeps a running app's files apart from those
/// a crashed run left behind.
pub fn file_name(n: u64) -> String {
    format!("{PREFIX}{}-{n}{SUFFIX}", std::process::id())
}

/// Checks that `dir` is on none of the source's physical disks, then creates it. Nothing is
/// created before the verdict: the disks are read from `dir`'s nearest existing parent.
pub fn approve(source: &SourceSpec, dir: &Path, confirm_unknown: bool) -> Result<PathBuf> {
    if !dir.is_absolute() {
        return Err(Error::Refused(format!(
            "{} is not an absolute path",
            dir.display()
        )));
    }
    let existing = dir
        .ancestors()
        .find(|p| p.is_dir())
        .ok_or_else(|| Error::Refused(format!("{} does not exist", dir.display())))?;
    let src = sources::source_disks(source)?;
    let ws = guard::disks_of_path(existing)?;
    match guard::verdict(&src, &ws) {
        Verdict::Ok { .. } => {}
        Verdict::SameDisk { disks } => {
            return Err(Error::WorkspaceSameDisk(format!("disk {disks:?}")));
        }
        Verdict::Unknown { .. } if confirm_unknown => {}
        Verdict::Unknown { reason, .. } => return Err(Error::WorkspaceUnknown(reason)),
    }
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    Ok(dir.to_path_buf())
}

/// Deletes databases that an earlier run of the app left in `dir` (a crash, a forced exit).
/// The app runs one instance at a time, so only this process's own files are live.
pub fn sweep(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mine = format!("{PREFIX}{}-", std::process::id());
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let ours = name.starts_with(PREFIX) && name.contains(SUFFIX);
        if ours && !name.starts_with(&mine) && e.file_type().is_ok_and(|t| t.is_file()) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_the_source_disk_and_creates_nothing() {
        let sys = std::env::var("SYSTEMDRIVE").unwrap_or_else(|_| "C:".into()) + "\\";
        let tmp = tempfile::tempdir_in(&sys).unwrap();
        let dir = tmp.path().join("a").join("b");
        let source = SourceSpec::Volume { root: sys.clone() };
        assert!(matches!(
            approve(&source, &dir, false),
            Err(Error::WorkspaceSameDisk(_))
        ));
        // Even a confirmation does not override a known shared disk.
        assert!(approve(&source, &dir, true).is_err());
        assert!(!tmp.path().join("a").exists());
        assert!(approve(&source, Path::new("relative"), false).is_err());
    }

    #[test]
    fn sweeps_only_other_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let mine = tmp.path().join(file_name(1));
        let old = tmp.path().join("scan-1-1.sqlite");
        let other = tmp.path().join("notes.txt");
        for p in [&mine, &old, &other] {
            std::fs::write(p, b"x").unwrap();
        }
        sweep(tmp.path());
        assert!(mine.exists() && other.exists());
        assert!(!old.exists());
        sweep(&tmp.path().join("missing"));
    }
}
