//! Process and token facts: elevation, the signed-in user's SID, starting the broker through
//! UAC, checking which program sits at the other end of a pipe, randomness and memory use.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree, WAIT_OBJECT_0};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
use windows::Win32::Security::{
    GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenUser,
};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, GetProcessId, OpenProcess, OpenProcessToken,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{PCWSTR, PWSTR, w};

use crate::winstr::{from_wide, wide};
use crate::{Error, Result};

const ERROR_CANCELLED_HRESULT: u32 = 0x8007_04C7;

/// An owned process handle that closes itself.
pub struct Process {
    handle: HANDLE,
    pub pid: u32,
}

// The handle is only used for waits and exit codes, which Windows allows from any thread.
unsafe impl Send for Process {}
unsafe impl Sync for Process {}

impl Process {
    pub fn open(pid: u32) -> Result<Self> {
        let handle = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            )?
        };
        Ok(Process { handle, pid })
    }

    /// Waits up to `ms` milliseconds; `true` if the process has exited.
    pub fn wait(&self, ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.handle, ms) == WAIT_OBJECT_0 }
    }

    pub fn exit_code(&self) -> Option<u32> {
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(self.handle, &mut code) }.ok()?;
        Some(code)
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut core::ffi::c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        ok.is_ok() && elevation.TokenIsElevated != 0
    }
}

/// String SID (`S-1-5-21-...`) of the user that owns `process`.
fn token_user_sid(process: HANDLE) -> Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token)?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        // u64 storage keeps the TOKEN_USER header aligned.
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(8)];
        let res = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            (buf.len() * 8) as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        res?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s)?;
        let out = s.to_string().map_err(|e| Error::Other(e.to_string()));
        let _ = LocalFree(Some(HLOCAL(s.0 as *mut core::ffi::c_void)));
        out
    }
}

/// SID of the user running this process.
pub fn current_user_sid() -> Result<String> {
    token_user_sid(unsafe { GetCurrentProcess() })
}

/// SID of the user running process `pid`.
pub fn process_user_sid(pid: u32) -> Result<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?;
        let r = token_user_sid(h);
        let _ = CloseHandle(h);
        r
    }
}

/// Full path of the executable running as `pid`.
pub fn process_image(pid: u32) -> Result<PathBuf> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?;
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        let r =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r?;
        Ok(PathBuf::from(from_wide(&buf[..len as usize])))
    }
}

/// True when both paths name the same file, compared after canonicalization.
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
        _ => false,
    }
}

/// Starts `exe` elevated through UAC without a window. `Ok(None)` means the user said no.
pub fn launch_elevated(exe: &Path, params: &str) -> Result<Option<Process>> {
    let file = wide(exe);
    let args = wide(params);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(args.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        if e.code().0 as u32 == ERROR_CANCELLED_HRESULT {
            return Ok(None);
        }
        return Err(e.into());
    }
    if info.hProcess.is_invalid() {
        return Err(Error::Broker(
            "Windows did not return a process handle".into(),
        ));
    }
    let pid = unsafe { GetProcessId(info.hProcess) };
    Ok(Some(Process {
        handle: info.hProcess,
        pid,
    }))
}

/// Waits for another process to exit (used when the parent UI goes away).
pub fn process_alive(pid: u32) -> bool {
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            Ok(h) => {
                let alive = WaitForSingleObject(h, 0) != WAIT_OBJECT_0;
                let _ = CloseHandle(h);
                alive
            }
            Err(_) => false,
        }
    }
}

/// Fills `buf` from the system's cryptographic random number generator.
pub fn random_bytes(buf: &mut [u8]) -> Result<()> {
    unsafe { BCryptGenRandom(None, buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }.ok()?;
    Ok(())
}

/// This process's peak working set and peak private (committed) bytes so far.
pub fn memory_peaks() -> Option<(u64, u64)> {
    let mut c = PROCESS_MEMORY_COUNTERS::default();
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut c, size) }.ok()?;
    Some((c.PeakWorkingSetSize as u64, c.PeakPagefileUsage as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_identity() {
        let sid = current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        assert_eq!(process_user_sid(std::process::id()).unwrap(), sid);
        let me = process_image(std::process::id()).unwrap();
        assert!(same_file(&me, &std::env::current_exe().unwrap()));
        assert!(process_alive(std::process::id()));
    }

    #[test]
    fn randomness_and_memory() {
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        random_bytes(&mut a).unwrap();
        random_bytes(&mut b).unwrap();
        assert_ne!(a, b);
        let (ws, private) = memory_peaks().unwrap();
        assert!(ws > 0 && private > 0);
    }
}
