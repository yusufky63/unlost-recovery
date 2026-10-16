//! Read-only block sources.
//!
//! [`SectorSource`] is the only way the engine reads a disk, a volume or an image. It has no
//! write method on purpose: code built on it cannot modify the source, whatever it does.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use crate::{Error, Result};

pub trait SectorSource: Send + Sync {
    /// Size in bytes.
    fn len(&self) -> u64;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Smallest read granularity the device needs. Callers may still read any range;
    /// implementations handle alignment themselves.
    fn sector_size(&self) -> u32 {
        512
    }

    /// Fills `buf` from `offset`. A short read is an error.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()>;
}

impl<T: SectorSource + ?Sized> SectorSource for &T {
    fn len(&self) -> u64 {
        (**self).len()
    }
    fn sector_size(&self) -> u32 {
        (**self).sector_size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        (**self).read_at(offset, buf)
    }
}

impl<T: SectorSource + ?Sized> SectorSource for Arc<T> {
    fn len(&self) -> u64 {
        (**self).len()
    }
    fn sector_size(&self) -> u32 {
        (**self).sector_size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        (**self).read_at(offset, buf)
    }
}

impl<T: SectorSource + ?Sized> SectorSource for Box<T> {
    fn len(&self) -> u64 {
        (**self).len()
    }
    fn sector_size(&self) -> u32 {
        (**self).sector_size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        (**self).read_at(offset, buf)
    }
}

pub(crate) fn out_of_range(offset: u64, len: usize, size: u64) -> Error {
    Error::Read {
        offset,
        source: std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("{len} bytes at {offset} are past the end ({size})"),
        ),
    }
}

/// In-memory source, for tests and small embedded images.
pub struct MemSource {
    data: Vec<u8>,
    sector: u32,
}

impl MemSource {
    pub fn new(data: Vec<u8>) -> Self {
        MemSource { data, sector: 512 }
    }

    pub fn with_sector(data: Vec<u8>, sector: u32) -> Self {
        MemSource { data, sector }
    }
}

impl SectorSource for MemSource {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn sector_size(&self) -> u32 {
        self.sector
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let start =
            usize::try_from(offset).map_err(|_| out_of_range(offset, buf.len(), self.len()))?;
        let src = start
            .checked_add(buf.len())
            .and_then(|end| self.data.get(start..end))
            .ok_or_else(|| out_of_range(offset, buf.len(), self.len()))?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

/// A raw image file (`.img`, `.dd`, the payload of a fixed VHD), opened read-only.
pub struct FileImage {
    file: File,
    len: u64,
}

impl FileImage {
    /// Opens `path` for reading only. Write sharing stays allowed so an image that another
    /// program still has open can be read; the handle itself never has write access.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(|source| Error::Read { offset: 0, source })?;
        let len = file
            .metadata()
            .map_err(|source| Error::Read { offset: 0, source })?
            .len();
        Ok(FileImage { file, len })
    }
}

impl SectorSource for FileImage {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset
            .checked_add(buf.len() as u64)
            .is_none_or(|end| end > self.len)
        {
            return Err(out_of_range(offset, buf.len(), self.len));
        }
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset + done as u64;
            let n = positional_read(&self.file, &mut buf[done..], pos).map_err(|source| {
                Error::Read {
                    offset: pos,
                    source,
                }
            })?;
            if n == 0 {
                return Err(out_of_range(pos, buf.len() - done, self.len));
            }
            done += n;
        }
        Ok(())
    }
}

#[cfg(windows)]
fn positional_read(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}

#[cfg(unix)]
fn positional_read(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

/// A window into another source, for example one partition of a disk image.
pub struct SliceSource<S> {
    inner: S,
    start: u64,
    len: u64,
}

impl<S: SectorSource> SliceSource<S> {
    pub fn new(inner: S, start: u64, len: u64) -> Result<Self> {
        if start.checked_add(len).is_none_or(|end| end > inner.len()) {
            return Err(Error::invalid(
                "partition",
                format!(
                    "range {start}+{len} is outside the source ({})",
                    inner.len()
                ),
            ));
        }
        Ok(SliceSource { inner, start, len })
    }
}

impl<S: SectorSource> SectorSource for SliceSource<S> {
    fn len(&self) -> u64 {
        self.len
    }
    fn sector_size(&self) -> u32 {
        self.inner.sector_size()
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset
            .checked_add(buf.len() as u64)
            .is_none_or(|end| end > self.len)
        {
            return Err(out_of_range(offset, buf.len(), self.len));
        }
        self.inner.read_at(self.start + offset, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_and_slice_bounds() {
        let src = MemSource::new((0..=255u8).collect());
        let mut b = [0u8; 4];
        src.read_at(252, &mut b).unwrap();
        assert_eq!(b, [252, 253, 254, 255]);
        assert!(src.read_at(253, &mut b).is_err());
        assert!(src.read_at(u64::MAX, &mut b).is_err());
        let slice = SliceSource::new(src, 16, 16).unwrap();
        slice.read_at(0, &mut b).unwrap();
        assert_eq!(b, [16, 17, 18, 19]);
        assert!(slice.read_at(13, &mut b).is_err());
        assert!(SliceSource::new(MemSource::new(vec![0; 8]), 4, 8).is_err());
    }

    #[test]
    fn file_image_reads_read_only() {
        let dir = std::env::temp_dir().join(format!("unlost-fi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("img.bin");
        std::fs::write(&path, (0..100u8).collect::<Vec<_>>()).unwrap();
        let img = FileImage::open(&path).unwrap();
        let mut b = [0u8; 3];
        img.read_at(97, &mut b).unwrap();
        assert_eq!(b, [97, 98, 99]);
        assert!(img.read_at(98, &mut b).is_err());
        drop(img);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
