//! Read-only access to raw volumes, physical disks and shadow-copy devices.
//!
//! Handles are opened with `GENERIC_READ` and both share flags, `OPEN_EXISTING`, and
//! `FILE_FLAG_NO_BUFFERING`. Nothing here locks, dismounts or sends a writing IOCTL, and the
//! [`WinDevice`] type exposes reads only (through `SectorSource`). Volume handles are uncached,
//! so every read is widened to sector boundaries through an aligned bounce buffer.

use std::fs::File;
use std::os::windows::fs::{FileExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;

use unlost_formats::SectorSource;
use windows::Win32::Foundation::{GENERIC_READ, HANDLE};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_NO_BUFFERING, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    FSCTL_ALLOW_EXTENDED_DASD_IO, IOCTL_DISK_GET_DRIVE_GEOMETRY, IOCTL_DISK_GET_LENGTH_INFO,
};

use crate::{Error, Result};

/// Largest single device read; bigger requests are split.
const MAX_IO: usize = 8 << 20;

pub struct WinDevice {
    file: File,
    path: String,
    len: u64,
    sector: u32,
    /// The access mask the handle was opened with, kept to assert it never carries write bits.
    access: u32,
}

impl WinDevice {
    /// Opens a device path such as `\\.\D:`, `\\.\PhysicalDrive1` or
    /// `\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy3` for reading.
    pub fn open(path: &str) -> Result<Self> {
        let access = GENERIC_READ.0;
        let file = std::fs::OpenOptions::new()
            .access_mode(access)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
            .custom_flags(FILE_FLAG_NO_BUFFERING.0)
            .open(path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    Error::NeedsAdmin
                } else {
                    Error::io(path, e)
                }
            })?;
        debug_assert_eq!(access & WRITE_BITS, 0, "device handles must be read-only");
        let h = raw(&file);
        // A volume handle refuses reads past the file system's own end (where NTFS keeps its
        // backup boot sector) unless boundary checks are lifted for this handle. This changes
        // nothing on disk and the handle stays read-only; physical disks ignore it.
        let _ = ioctl(h, FSCTL_ALLOW_EXTENDED_DASD_IO, &[], 0);
        let len = ioctl(h, IOCTL_DISK_GET_LENGTH_INFO, &[], 8)
            .ok()
            .and_then(|b| {
                b.get(0..8)
                    .map(|s| i64::from_le_bytes(s.try_into().unwrap_or_default()))
            })
            .filter(|&l| l > 0)
            .map(|l| l as u64)
            .ok_or_else(|| Error::Other(format!("{path}: cannot read the device size")))?;
        // DISK_GEOMETRY.BytesPerSector sits at offset 20.
        let sector = ioctl(h, IOCTL_DISK_GET_DRIVE_GEOMETRY, &[], 24)
            .ok()
            .and_then(|b| {
                b.get(20..24)
                    .map(|s| u32::from_le_bytes(s.try_into().unwrap_or_default()))
            })
            .filter(|s| s.is_power_of_two() && (512..=4096).contains(s))
            .unwrap_or(512);
        Ok(WinDevice {
            file,
            path: path.to_string(),
            len,
            sector,
            access,
        })
    }

    /// `D:\` or `D:` -> `\\.\D:`.
    pub fn open_volume(root: &str) -> Result<Self> {
        let letter = drive_letter(root)
            .ok_or_else(|| Error::Refused(format!("{root} is not a drive letter")))?;
        Self::open(&format!(r"\\.\{letter}:"))
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn access_mask(&self) -> u32 {
        self.access
    }
}

/// `GENERIC_WRITE`, `GENERIC_ALL`, `FILE_WRITE_DATA`, `FILE_APPEND_DATA`, `DELETE`, `WRITE_DAC`,
/// `WRITE_OWNER`, `FILE_WRITE_ATTRIBUTES`, `FILE_WRITE_EA`.
pub const WRITE_BITS: u32 =
    0x4000_0000 | 0x1000_0000 | 0x2 | 0x4 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x100 | 0x10;

pub fn drive_letter(root: &str) -> Option<char> {
    let mut chars = root.chars();
    let c = chars.next()?.to_ascii_uppercase();
    if !c.is_ascii_uppercase() || chars.next() != Some(':') {
        return None;
    }
    let rest: String = chars.collect();
    (rest.is_empty() || rest == "\\").then_some(c)
}

fn raw(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}

/// Sends a read-only query IOCTL. Callers pass only `GET`/`QUERY` codes.
pub(crate) fn ioctl(h: HANDLE, code: u32, input: &[u8], out_len: usize) -> Result<Vec<u8>> {
    let mut out = vec![0u8; out_len];
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            h,
            code,
            (!input.is_empty()).then_some(input.as_ptr() as *const core::ffi::c_void),
            input.len() as u32,
            Some(out.as_mut_ptr() as *mut core::ffi::c_void),
            out.len() as u32,
            Some(&mut returned),
            None,
        )?;
    }
    out.truncate(returned as usize);
    Ok(out)
}

/// Opens a device with no access rights at all: enough for property queries, which work
/// without administrator rights.
pub(crate) fn open_query(path: &str) -> Result<File> {
    std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .open(path)
        .map_err(|e| Error::io(path, e))
}

pub(crate) fn query_ioctl(path: &str, code: u32, input: &[u8], out_len: usize) -> Result<Vec<u8>> {
    let f = open_query(path)?;
    ioctl(raw(&f), code, input, out_len)
}

/// A buffer whose usable slice starts at an `align`-byte boundary, as uncached I/O requires.
struct Aligned {
    buf: Vec<u8>,
    off: usize,
    len: usize,
}

impl Aligned {
    fn new(len: usize, align: usize) -> Self {
        let buf = vec![0u8; len + align];
        let off = buf.as_ptr().align_offset(align).min(align);
        Aligned { buf, off, len }
    }
    fn slice(&mut self) -> &mut [u8] {
        &mut self.buf[self.off..self.off + self.len]
    }
}

impl SectorSource for WinDevice {
    fn len(&self) -> u64 {
        self.len
    }

    fn sector_size(&self) -> u32 {
        self.sector
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> unlost_formats::Result<()> {
        let err =
            |offset: u64, e: std::io::Error| unlost_formats::Error::Read { offset, source: e };
        if offset
            .checked_add(buf.len() as u64)
            .is_none_or(|e| e > self.len)
        {
            return Err(err(
                offset,
                std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "read past the device end",
                ),
            ));
        }
        let s = u64::from(self.sector);
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset + done as u64;
            let start = pos - pos % s;
            let want = (buf.len() - done).min(MAX_IO);
            let end = (pos + want as u64).div_ceil(s) * s;
            let end = end.min(self.len.div_ceil(s) * s);
            let span = (end - start) as usize;
            let mut bounce = Aligned::new(span, self.sector as usize);
            let b = bounce.slice();
            let mut got = 0usize;
            while got < span {
                let n = self
                    .file
                    .seek_read(&mut b[got..], start + got as u64)
                    .map_err(|e| err(start + got as u64, e))?;
                if n == 0 {
                    return Err(err(
                        start + got as u64,
                        std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "short device read"),
                    ));
                }
                got += n;
            }
            let skip = (pos - start) as usize;
            let n = want.min(span - skip);
            buf[done..done + n].copy_from_slice(&b[skip..skip + n]);
            done += n;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters() {
        assert_eq!(drive_letter("d:\\"), Some('D'));
        assert_eq!(drive_letter("C:"), Some('C'));
        assert_eq!(drive_letter("C:\\Windows"), None);
        assert_eq!(drive_letter("\\\\.\\C:"), None);
        assert_eq!(drive_letter(""), None);
    }

    #[test]
    fn write_bits_cover_the_dangerous_rights() {
        assert_ne!(WRITE_BITS & 0x4000_0000, 0, "GENERIC_WRITE");
        assert_eq!(WRITE_BITS & GENERIC_READ.0, 0);
    }

    #[test]
    fn raw_volume_needs_admin_or_reads_read_only() {
        // Without elevation Windows refuses raw volume handles; with it, the boot sector reads.
        match WinDevice::open_volume("C:") {
            Err(Error::NeedsAdmin) => assert!(!crate::system::is_elevated()),
            Err(e) => panic!("unexpected error: {e}"),
            Ok(dev) => {
                assert_eq!(dev.access_mask() & WRITE_BITS, 0);
                let mut b = [0u8; 13];
                dev.read_at(3, &mut b).unwrap();
                assert!(dev.len() > 0);
            }
        }
    }
}
