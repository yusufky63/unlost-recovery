# Architecture

How Unlost is built and why. The research behind these choices (landscape, techniques, limits, licences) is in [research.md](research.md).

## Overview

```
 UI process (normal user)                          Broker (same exe, elevated through UAC)
 +-------------------------------------------+     +--------------------------------------+
 | WebView2 + React (src/)                   |     | unlost-core::broker                  |
 |   |  Tauri commands (src-tauri/)          |     |   Session (scans, reads, shadows)    |
 |   v                                       |     |   WinDevice: \\.\X:, shadow devices  |
 | unlost-core::engine ---- local calls ---> |     |   (GENERIC_READ only)                |
 |   Session (image files)                   |     +------------------^-------------------+
 |   writer: recovery folder, hashes, report |                        |
 |   guard: physical disk check              |  named pipes (user-only DACL, PID and
 |   recycle bin, drive facts                | <----- image checks, length-prefixed frames)
 +-------------------------------------------+
```

Three crates:

| Crate | Path | Role | `unsafe` |
|---|---|---|---|
| `unlost-formats` | `crates/formats` | Parsers: NTFS (boot sector, file records, runlists, `$Bitmap`, scan, copy-out), Recycle Bin `$I`, VHDX, MBR/GPT, file-type headers. The only I/O abstraction is the read-only `SectorSource` trait. Portable, so it fuzzes on Linux. | Forbidden (`#![forbid(unsafe_code)]`) |
| `unlost-core` | `crates/core` | Windows layer and workflow: raw device handles, drive facts, destination guard, Recycle Bin, shadow copies (WMI), the request `Session`, broker and pipes, recovery writer and reports. | Win32 FFI only |
| `unlost` | `src-tauri` | Tauri shell: commands, progress events, window and WebView2 profile, broker entry point (`--broker`). | None of its own |

## Processes

The WebView2 host never runs elevated, following Microsoft's WebView2 guidance. Administrator work runs in a broker:

1. The UI creates two named pipes (`\\.\pipe\unlost-<128-bit random>-req` and `-evt`) **before** starting the broker, with `FILE_FLAG_FIRST_PIPE_INSTANCE`, one instance, `PIPE_REJECT_REMOTE_CLIENTS`, and a protected DACL that grants only the signed-in user's SID the exact rights the client end needs. `FILE_CREATE_PIPE_INSTANCE` (the same bit as `FILE_APPEND_DATA`, part of `GENERIC_WRITE`) is never granted.
2. It starts its own exe with `--broker --pipe <name> --ui-pid <pid>` through `ShellExecuteExW` "runas". The command line carries no paths.
3. The broker refuses to serve if the UI process belongs to another user (over-the-shoulder elevation would otherwise let a standard user read raw disks). It connects with identification-level impersonation only (`SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`), checks that the pipe server is the stated PID and that it runs the same executable.
4. The UI checks that the connected client is the process it launched. A watchdog unblocks the wait if the broker dies or never connects.
5. Requests go UI to broker on one pipe, events broker to UI on the other. Two pipes because synchronous handles serialize reads and writes: a cancel must get through while a file streams back.
6. The broker exits when the pipe closes or the UI process ends.

**The broker never writes files.** File content streams back over the pipe and the non-elevated UI process writes it, after the destination guard approved the folder. An elevated process therefore cannot be tricked into writing somewhere the user could not.

### Protocol

Frames: `u32 length`, `u8 kind`, `u64 request id`, payload. Kinds: request (JSON), progress (JSON), data (raw bytes), ok (JSON), error (JSON `{code, message}`). Every length is checked against a 4 MiB maximum before allocating, in both directions. Requests (`crates/core/src/protocol.rs`):

| Request | Answer |
|---|---|
| `scan { source }` | progress events, then a scan summary |
| `query { scan, query }` | one page of results with per-state counts |
| `entries { scan, records }` | rows for a selection |
| `read { scan, record }` | data events (the file's bytes), then read statistics and the final state |
| `shadowList`, `shadowDir`, `shadowRead` | shadow copies, a folder listing, a file's bytes |
| `cancel { id }`, `forget { scan }` | |

The same `Session` handles requests in-process (image files, or when the UI already runs elevated) and in the broker, so the protocol is covered by ordinary tests. Results flow through bounded channels: a slow destination slows the reader instead of filling memory.

## Read-only I/O

`WinDevice` (`crates/core/src/device.rs`) opens `\\.\X:`, `\\.\PhysicalDriveN` or `\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopyN` with `GENERIC_READ`, `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`, `OPEN_EXISTING` and `FILE_FLAG_NO_BUFFERING`. Reads are widened to sector boundaries through an aligned bounce buffer. The access mask is kept in the type and asserted to carry no write bit. Nothing locks, dismounts or sends a writing IOCTL. `FSCTL_ALLOW_EXTENDED_DASD_IO` is sent on volume handles so the last sectors (where NTFS keeps its backup boot sector) can be read; it lifts a boundary check for that handle and changes nothing on disk.

Image files are read through `FileImage` (opened read-only) and, for `.vhdx`, a VHDX reader that follows the block allocation table. Partitions come from MBR or GPT; a bare volume image is used as is.

## Destination guard

`crates/core/src/guard.rs`. Before any recovery, for every source the request touches:

1. The destination folder is opened and resolved with `GetFinalPathNameByHandleW(VOLUME_NAME_GUID)`, which follows junctions, mounted folders and `subst`, to `\\?\Volume{...}\`.
2. The volume's physical disks come from `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` (works without administrator rights).
3. A virtual disk (VHD/VHDX) adds the disks holding its image file, from `GetStorageDependencyInformation`.
4. The source's disks are found the same way: a drive letter, the volume holding an image file, or the original volume of a shadow copy.
5. A shared disk is refused (`same_disk`), even if the user ticks the "unknown disk" confirmation. A destination whose disk cannot be determined (network share, unknown virtual disk) is refused unless the user confirms.

Tested on real hardware: two partitions of one USB stick (`E:` and `F:`) are refused as the same disk (disk 2); `C:` to `E:` and `E:` to `D:` are allowed.

## NTFS scan

`crates/formats/src/ntfs/`.

1. Boot sector: sector and cluster size (including the negative-exponent encodings for large clusters and small records), `$MFT` location, record size. The claimed volume must fit in the device.
2. Record 0 gives `$MFT`'s own runlist, including pieces in extension records listed in its `$ATTRIBUTE_LIST`. The record count is capped by the clusters the runs actually map, so a damaged record 0 cannot claim billions of records.
3. The whole `$MFT` is read in 4 MiB chunks. Each record: `FILE` signature, update-sequence fixups (always a 512-byte stride; a mismatch marks the record torn), header flags.
   - In-use folders go into a compact folder map (sequence, parent, best name).
   - Not-in-use records with a `$FILE_NAME` become candidates: names (8.3 short names dropped when a long one exists), `$STANDARD_INFORMATION` times and attributes, unnamed `$DATA` (resident bytes or runlist), named streams counted, reparse tag.
   - Not-in-use extension records are kept and joined to their base record afterwards.
4. Paths are rebuilt through parent references with sequence checks. A freed record has its sequence number bumped, so a deleted parent matches either the stored sequence or the next one; an in-use parent must match exactly. A broken chain puts the file under `\Lost folder #<record>`, keeping the folder names below the break. Folder paths are shared between files.
5. Grading (next section), then content sampling.

Limits: 1,000,000 deleted files per scan (about 500 bytes each in memory), 65,536 runs per attribute, folder depth 1,024 with cycle detection. Measured on this project's development machine: a shadow copy of a 1 TB NVMe system volume, 9.4 million records, scanned in 23 s with a peak of 1.2 GB (the 1 million cap was reached); a 64 GB USB stick with 50,000 records in 11 s through the broker.

## Recoverability states

| State | When | Default view |
|---|---|---|
| Intact copy | Recycle Bin or shadow copy | Shown |
| Verified | A format validator decoded the whole file. No validators ship yet, so this state does not appear | Shown |
| Likely intact | Resident data in an unreused record; or all clusters free in `$Bitmap` and sampled clusters not all zero, header consistent with the extension | Shown |
| Partial | Some clusters reallocated, header does not match the extension, runs missing, or unreadable sectors while copying | Shown |
| Zeroed | Clusters free but every sampled (or copied) cluster reads as zeros: TRIM | Hidden |
| Overwritten | 95% or more of the clusters reallocated | Hidden |
| Name only | Name and dates but no runs, a torn record, runs outside the volume, an implausible size | Hidden |
| Not recoverable here | EFS-encrypted, cloud-only placeholder, NTFS-compressed, CompactOS (WOF) | Hidden |

Every state carries a reason code with numbers (`clustersReused {reused, total}`, `allZero {sampled}`, ...). The UI translates it; reports carry the code and an English sentence.

Sampling reads the first, middle and last mapped cluster of each candidate whose clusters are all free, sorted by disk position, within a budget of 300,000 reads. Candidates beyond the budget stay "Likely intact" with the reason "content not sampled". During copy-out every cluster read is checked again: an all-zero result turns the file "Zeroed"; unreadable sectors (read once in a block, once alone, then zero-filled) turn it "Partial".

## Recovery output

`crates/core/src/writer.rs`, in the UI process:

- A new folder `Unlost <date time>` under the destination; never an existing one.
- Inside, the source's folder structure. Names are made safe for Windows (invalid characters, reserved names, trailing dots).
- Each file is written to a temporary name with `create_new`, hashed while written, flushed, renamed into place with `MoveFileExW` without replacement (suffix ` (2)` on collision), then read back and hashed again (`verifiedWrite`).
- Damaged content gets a `PARTIAL_`, `ZEROED_` or `OVERWRITTEN_` prefix.
- `unlost-report.json` and `unlost-report.csv` (UTF-8 with BOM, formula-safe cells) list original path, recovered path, size, method, state, reason, SHA-256, write check and errors.
- Recovered files are never opened. Only the folder can be shown in Explorer.

## The app's own writes

- Settings (`settings.json`, only when changed) and the WebView2 profile go to `%LOCALAPPDATA%\Unlost`, or to `data\` next to the exe in portable mode. The WebView2 window runs InPrivate.
- No logs, sessions or caches are written in this version.
- The first screen warns when the app's own disk is the disk being recovered from.

## Recycle Bin

`X:\$Recycle.Bin\<user SID>\` on every fixed and removable drive. `$I` files (version 1 and 2) give the original path, size and deletion time; `$R` is the data (a file or a folder). Items whose `$R` is gone are shown as "Data gone" instead of being hidden. Items are copied to another disk; the Recycle Bin is never changed. Other users' bins are not listed.

## Shadow copies

Listed through WMI `Win32_ShadowCopy` (administrators only), browsed with normal read-only file APIs on the shadow device. Unlost never creates one: on the source volume that would write a diff area over the space deleted files occupy.

## Phones

Phones and cameras connected over USB appear as portable (MTP/PTP) devices without a drive letter; Windows exposes their files, not their storage. Unlost lists them through WMI (`Win32_PnPEntity`, class `WPD`, on the USB bus) only to say they cannot be scanned and where to look instead.
