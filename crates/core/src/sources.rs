//! What can be scanned: a mounted volume, a disk image file, or an existing shadow copy.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use unlost_formats::partition::{self, FsKind, Partition};
use unlost_formats::vhdx::{self, Vhdx};
use unlost_formats::{FileImage, SectorSource, SliceSource};

use crate::device::{WinDevice, drive_letter};
use crate::guard::{self, DiskSet};
use crate::{Error, Result, shadow};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SourceSpec {
    /// A mounted volume by drive letter (`D:\`). Needs administrator rights.
    Volume { root: String },
    /// A raw image (`.img`, `.dd`, `.raw`, `.bin`) or a VHDX; `partition` is the index from
    /// [`image_partitions`] (0 for a whole-volume image).
    Image {
        path: String,
        partition: Option<u32>,
    },
    /// An existing shadow copy device of `volume`. Needs administrator rights.
    Shadow { device: String, volume: String },
}

impl SourceSpec {
    pub fn needs_admin(&self) -> bool {
        !matches!(self, SourceSpec::Image { .. })
    }

    pub fn label(&self) -> String {
        match self {
            SourceSpec::Volume { root } => root.trim_end_matches('\\').to_string(),
            SourceSpec::Image { path, partition } => {
                let name = Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                match partition {
                    Some(p) if *p > 0 => format!("{name} (partition {p})"),
                    _ => name,
                }
            }
            SourceSpec::Shadow { device, .. } => {
                device.rsplit('\\').next().unwrap_or(device).to_string()
            }
        }
    }
}

pub struct Opened {
    pub src: Arc<dyn SectorSource>,
    pub fs: FsKind,
}

fn open_image_disk(path: &Path) -> Result<Arc<dyn SectorSource>> {
    let file = FileImage::open(path)?;
    if vhdx::is_vhdx(&file) {
        Ok(Arc::new(Vhdx::open(file)?))
    } else {
        Ok(Arc::new(file))
    }
}

/// Volumes inside an image file.
pub fn image_partitions(path: &Path) -> Result<Vec<Partition>> {
    let disk = open_image_disk(path)?;
    Ok(partition::list(&disk)?)
}

pub fn open(spec: &SourceSpec) -> Result<Opened> {
    match spec {
        SourceSpec::Volume { root } => {
            drive_letter(root).ok_or_else(|| Error::Refused(format!("{root} is not a drive")))?;
            let dev = WinDevice::open_volume(root)?;
            let fs = partition::probe_fs(&dev, 0);
            Ok(Opened {
                src: Arc::new(dev),
                fs,
            })
        }
        SourceSpec::Shadow { device, .. } => {
            if !shadow::valid_device(device) {
                return Err(Error::Refused(format!(
                    "not a shadow copy device: {device}"
                )));
            }
            let dev = WinDevice::open(device)?;
            let fs = partition::probe_fs(&dev, 0);
            Ok(Opened {
                src: Arc::new(dev),
                fs,
            })
        }
        SourceSpec::Image {
            path,
            partition: wanted,
        } => {
            let disk = open_image_disk(Path::new(path))?;
            let parts = partition::list(&disk)?;
            let pick = match wanted {
                Some(i) => parts.iter().find(|p| p.index == *i),
                None => {
                    let ntfs: Vec<&Partition> =
                        parts.iter().filter(|p| p.fs == FsKind::Ntfs).collect();
                    match ntfs.as_slice() {
                        [one] => Some(*one),
                        [] => parts.first(),
                        _ => {
                            return Err(Error::Refused(
                                "the image has several NTFS partitions; choose one".into(),
                            ));
                        }
                    }
                }
            };
            let p = pick.ok_or_else(|| Error::Refused("no such partition in the image".into()))?;
            let fs = p.fs;
            let src: Arc<dyn SectorSource> = if p.offset == 0 && p.length == disk.len() {
                disk
            } else {
                Arc::new(SliceSource::new(disk, p.offset, p.length)?)
            };
            Ok(Opened { src, fs })
        }
    }
}

/// Physical disks the source lives on, for the destination guard.
pub fn source_disks(spec: &SourceSpec) -> Result<DiskSet> {
    match spec {
        SourceSpec::Volume { root } => {
            let letter = drive_letter(root)
                .ok_or_else(|| Error::Refused(format!("{root} is not a drive")))?;
            guard::disks_of_path(Path::new(&format!("{letter}:\\")))
        }
        SourceSpec::Image { path, .. } => guard::disks_of_path(Path::new(path)),
        SourceSpec::Shadow { volume, .. } => {
            if !volume.starts_with(r"\\?\Volume{") {
                return Err(Error::Refused(format!("not a volume path: {volume}")));
            }
            Ok(guard::disks_of_volume(volume))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_image_files_without_admin() {
        let tmp = tempfile::tempdir().unwrap();
        let mut b = unlost_formats::ntfs::build::ImageBuilder::new(512, 4096, 1024, 32);
        b.dir(30, 1, (5, 5), "x", true);
        let vol = b.build();
        let raw = tmp.path().join("vol.img");
        std::fs::write(&raw, &vol).unwrap();
        let disk = tmp.path().join("disk.vhdx");
        let gpt = unlost_formats::partition::build_test_gpt(&vol);
        std::fs::write(&disk, unlost_formats::vhdx::build_test_vhdx(&gpt, 512)).unwrap();

        let o = open(&SourceSpec::Image {
            path: raw.to_string_lossy().into(),
            partition: None,
        })
        .unwrap();
        assert_eq!(o.fs, FsKind::Ntfs);
        assert_eq!(o.src.len(), vol.len() as u64);
        let parts = image_partitions(&disk).unwrap();
        assert_eq!(parts.len(), 1);
        let spec = SourceSpec::Image {
            path: disk.to_string_lossy().into(),
            partition: Some(parts[0].index),
        };
        let o = open(&spec).unwrap();
        assert_eq!(o.fs, FsKind::Ntfs);
        let mut boot = [0u8; 11];
        o.src.read_at(0, &mut boot).unwrap();
        assert_eq!(&boot[3..11], b"NTFS    ");
        assert!(!spec.needs_admin());
        assert_eq!(spec.label(), "disk.vhdx (partition 1)");
        assert!(!source_disks(&spec).unwrap().disks.is_empty());
        assert!(
            open(&SourceSpec::Shadow {
                device: r"C:\Windows".into(),
                volume: String::new()
            })
            .is_err()
        );
        assert!(
            open(&SourceSpec::Volume {
                root: r"C:\Windows".into()
            })
            .is_err()
        );
    }
}
