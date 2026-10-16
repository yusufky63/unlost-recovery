//! Named pipes between the UI (server) and the elevated broker (client).
//!
//! The UI creates both pipes before starting the broker, so nothing can squat the names:
//!
//! - first instance only, one instance, remote clients rejected;
//! - an explicit DACL that admits only the signed-in user's SID, with just the rights the
//!   client needs (never `FILE_CREATE_PIPE_INSTANCE`, which shares its bit with
//!   `FILE_APPEND_DATA` and so with `GENERIC_WRITE`);
//! - the UI checks that the connected client is the process it launched;
//! - the broker connects with identification-level impersonation only and checks that the
//!   server is the UI process it was told about, running this same executable.
//!
//! Two pipes, one per direction, because synchronous handles serialize reads and writes: a
//! cancel request must get through while a large file is streaming the other way.

use std::fs::File;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES, PIPE_ACCESS_INBOUND,
    PIPE_ACCESS_OUTBOUND, SECURITY_IDENTIFICATION,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::core::PCWSTR;

use crate::system::{Process, process_image, same_file};
use crate::winstr::wide;
use crate::{Error, Result};

const BUFFER: u32 = 1 << 20;
/// How long the UI waits for the broker to connect after UAC approval.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE
const CLIENT_READ: u32 = 0x0010_0081;
/// FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE
const CLIENT_WRITE: u32 = 0x0010_0082;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    /// The server writes, the client reads.
    ToClient,
    /// The client writes, the server reads.
    ToServer,
}

/// `\\.\pipe\unlost-<32 hex>-<suffix>`.
pub fn pipe_path(base: &str, dir: Direction) -> String {
    let suffix = match dir {
        Direction::ToClient => "req",
        Direction::ToServer => "evt",
    };
    format!(r"\\.\pipe\{base}-{suffix}")
}

pub fn valid_base(base: &str) -> bool {
    base.strip_prefix("unlost-").is_some_and(|h| {
        h.len() == 32
            && h.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

/// A fresh, unguessable base name. `RandomState` is seeded from the OS random source.
pub fn random_base() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut parts = [0u64; 2];
    for (i, p) in parts.iter_mut().enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i as u64);
        h.write_u128(Instant::now().elapsed().as_nanos());
        *p = h.finish();
    }
    format!("unlost-{:016x}{:016x}", parts[0], parts[1])
}

/// Server end, created by the UI before the broker starts.
pub struct Server {
    handle: OwnedHandle,
    path: String,
}

impl Server {
    pub fn create(base: &str, dir: Direction, user_sid: &str) -> Result<Self> {
        if !valid_base(base)
            || !user_sid.starts_with("S-1-")
            || !user_sid
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'-' || b == b'S')
        {
            return Err(Error::Refused("invalid pipe name or SID".into()));
        }
        let path = pipe_path(base, dir);
        let (open_mode, rights) = match dir {
            Direction::ToClient => (PIPE_ACCESS_OUTBOUND, CLIENT_READ),
            Direction::ToServer => (PIPE_ACCESS_INBOUND, CLIENT_WRITE),
        };
        // Protected DACL: the user, with only the rights the client end needs. No inherited
        // or default entries, so Everyone and anonymous get nothing.
        let sddl = wide(format!(
            "D:P(A;;0x{:08x};;;{user_sid})",
            rights | 0x0002_0000
        ));
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )?;
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        let w = wide(&path);
        let h = unsafe {
            CreateNamedPipeW(
                PCWSTR(w.as_ptr()),
                FILE_FLAGS_AND_ATTRIBUTES(open_mode.0 | FILE_FLAG_FIRST_PIPE_INSTANCE.0),
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                BUFFER,
                BUFFER,
                0,
                Some(&sa),
            )
        };
        unsafe {
            let _ = LocalFree(Some(HLOCAL(sd.0)));
        }
        if h.is_invalid() {
            return Err(Error::Windows(format!(
                "cannot create {path}: {}",
                windows::core::Error::from_thread()
            )));
        }
        Ok(Server {
            handle: unsafe { OwnedHandle::from_raw_handle(h.0) },
            path,
        })
    }

    /// Waits for the broker to connect, then checks it is the process that was launched. A
    /// watchdog unblocks the wait if the broker dies or never connects.
    pub fn accept(self, broker: &Process) -> Result<File> {
        let h = HANDLE(self.handle.as_raw_handle());
        let path = self.path.clone();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (wd_done, pid) = (done.clone(), broker.pid);
        let watchdog = std::thread::spawn(move || {
            let start = Instant::now();
            while !wd_done.load(std::sync::atomic::Ordering::Relaxed) {
                let gone = !crate::system::process_alive(pid);
                if gone || start.elapsed() > CONNECT_TIMEOUT {
                    // Connecting ourselves ends ConnectNamedPipe; the PID check below rejects it.
                    let _ = std::fs::OpenOptions::new()
                        .access_mode(CLIENT_READ | CLIENT_WRITE)
                        .open(&path);
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let r = unsafe { ConnectNamedPipe(h, None) };
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = watchdog.join();
        if let Err(e) = r
            && e.code() != ERROR_PIPE_CONNECTED.to_hresult()
        {
            return Err(e.into());
        }
        let mut client = 0u32;
        unsafe { GetNamedPipeClientProcessId(h, &mut client)? };
        if client != broker.pid {
            return Err(Error::Broker(format!(
                "unexpected client process {client} on {} (expected {})",
                self.path, broker.pid
            )));
        }
        Ok(File::from(self.handle))
    }
}

/// Broker side: opens the UI's pipe and verifies the server process.
pub fn connect(base: &str, dir: Direction, ui_pid: u32) -> Result<File> {
    if !valid_base(base) {
        return Err(Error::Refused("invalid pipe name".into()));
    }
    let path = pipe_path(base, dir);
    let access = match dir {
        Direction::ToClient => CLIENT_READ,
        Direction::ToServer => CLIENT_WRITE,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(access)
        .share_mode(0)
        .security_qos_flags(SECURITY_IDENTIFICATION.0)
        .open(&path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                Error::OtherUser
            } else {
                Error::io(&path, e)
            }
        })?;
    let mut server = 0u32;
    unsafe { GetNamedPipeServerProcessId(HANDLE(file.as_raw_handle()), &mut server)? };
    if server != ui_pid {
        return Err(Error::Broker(format!(
            "pipe server is process {server}, expected {ui_pid}"
        )));
    }
    let image = process_image(server)?;
    let me = std::env::current_exe().map_err(|e| Error::io("current_exe", e))?;
    if !same_file(&image, &me) {
        return Err(Error::Broker(format!(
            "pipe server runs {}, not this program",
            image.display()
        )));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let b = random_base();
        assert!(valid_base(&b), "{b}");
        assert_ne!(b, random_base());
        assert!(!valid_base("unlost-xyz"));
        assert!(!valid_base("other-0123456789abcdef0123456789abcdef"));
        assert!(!valid_base("unlost-0123456789ABCDEF0123456789abcdef"));
        assert_eq!(
            pipe_path("unlost-x", Direction::ToClient),
            r"\\.\pipe\unlost-x-req"
        );
    }

    #[test]
    fn rejects_unexpected_clients_and_squatting() {
        let sid = crate::system::current_user_sid().unwrap();
        let base = random_base();
        let server = Server::create(&base, Direction::ToServer, &sid).unwrap();
        // A second server instance with the same name is refused (first instance only).
        assert!(Server::create(&base, Direction::ToServer, &sid).is_err());
        // The broker side refuses a server that is not the stated UI process.
        let path = pipe_path(&base, Direction::ToServer);
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            std::fs::OpenOptions::new()
                .access_mode(CLIENT_WRITE)
                .open(path)
                .map(|_| ())
        });
        // Accepting with the wrong expected PID fails the client check.
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 4 127.0.0.1 >nul"])
            .spawn()
            .unwrap();
        let other = Process::open(child.id()).unwrap();
        let r = server.accept(&other);
        let _ = child.kill();
        let _ = child.wait();
        assert!(r.is_err(), "a foreign client must be rejected");
        let _ = t.join();
        assert!(Server::create("bad", Direction::ToServer, &sid).is_err());
    }
}
