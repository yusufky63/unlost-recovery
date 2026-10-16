# Deleted-file recovery for Windows 10/11: research for a separate ReClaim recovery app

Date: 2026-10-16. Scope: an open-source (MIT) deleted-file recovery program for Windows 10/11, separate from ReClaim Cleaner, built on the same stack (Tauri 2, React, Tailwind, Rust, English and Turkish).

Legend used throughout:

- **(verified)**: confirmed in a primary source (vendor docs, spec, source repo, crates.io/GitHub API) during this research.
- **(secondary)**: from reviews, forums, vendor marketing or course material. Plausible, not confirmed at the source.
- **(unverified)**: my inference or a widely repeated claim I did not confirm. Each needs a test before it drives a decision.

Licensing note: nothing in this report copies code from GPL, LGPL, AGPL, CPL or unlicensed projects. Tools are described in my own words. On-disk structures and file-format magic numbers are public facts, but their implementations in GPL tools are not reusable.

Related prior research in the same scratchpad: `research2-fastscan.md` (MFT fast scan and the elevated-helper named-pipe design for ReClaim Cleaner). This report reuses that helper design rather than repeating it.

---

## 0. Executive summary

1. **The open-source field is no longer empty.** In 2025 and 2026 at least eight new open-source Windows recovery projects appeared (BitBroom Rescue, Rescyo, KVC FileRecoveryTool, unearth, NullVeil, recoverer, Data-Recovery-CLI, winfr-pro). Almost all are under a year old with 0 to 40 GitHub stars (verified, GitHub API). None combines a polished bilingual GUI, Tier-A "easy wins" (Recycle Bin, cloud bins, Previous Versions, autosaves), honest per-file evidence and a tested read-only core. That combination is the realistic way to be "the best open-source option". Raw undelete alone is no longer a differentiator.
2. **Order of techniques matters more than any algorithm.** Intact copies (Recycle Bin, cloud recycle bins, Volume Shadow Copies, File History, app autosaves) beat any forensic method and need no luck. The app should check those first, then do NTFS/FAT metadata undelete, then signature carving.
3. **SSDs change the promise.** On NTFS, Windows sends TRIM for freed clusters unless `DisableDeleteNotify` is set (verified, Microsoft fsutil docs). Most modern SSDs then return zeros for trimmed blocks (secondary, ElcomSoft). On a TRIM-enabled internal SSD, metadata undelete usually finds names but zeroed content. The app must detect this and say so before a long scan.
4. **Read-only by construction is achievable and is the core safety feature.** Open `\\.\X:` or `\\.\PhysicalDriveN` with `GENERIC_READ`, `FILE_SHARE_READ | FILE_SHARE_WRITE` and `OPEN_EXISTING`, never lock or dismount, and expose no write path in the I/O trait. Admin rights are required for raw handles (verified, CreateFile docs).
5. **The app itself must not write to the source disk.** That includes the recovery output, the session database, logs, and the WebView2 user data folder, which defaults to `%LOCALAPPDATA%` on C:. Enforce destination checks by physical disk number (`IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`, verified), not by drive letter. Ship a portable build.
6. **Never create a new shadow copy of the source volume.** VSS snapshots give a consistent view, but creating one writes a diff area to the same volume (unverified detail, standard VSS behaviour). Use existing snapshots only.
7. **Process model.** Keep the WebView2 host non-elevated and do admin work in a separate dedicated process. Microsoft recommends exactly this for WebView2 apps (verified, WebView2 security docs). Reuse the named-pipe helper design from `research2-fastscan.md`.
8. **Rust building blocks exist but are young.** `mft` (MIT/Apache, about 750k downloads) is the mature record parser. `ntfs-reader` 0.6.0 (MIT OR Apache, 2026-10-01) already lists deleted files with path markers. `ntfs` (Colin Finck) is clean but has no compression, encryption or deleted-file focus. Carving validators exist for JPEG, PNG, ZIP, PDF, MP4 and MKV under MIT/Apache licences. Avoid `vshadow` (AGPL), `ms-compress` (LGPL) and all `exhume_*` crates (GPL-3.0) (all verified, crates.io API).
9. **Recommended MVP:** Tier-A recovery, NTFS MFT undelete, FAT32/exFAT undelete for USB sticks and SD cards, a carving-lite engine for photos, videos and documents with validators, a destination guard, honest per-file states and EN/TR UI. That is about 11 to 15 engineer-weeks. A credible v1.0 adds about 8 to 10 more weeks (estimates, unverified).
10. **Separate repository**, with the NTFS reader as a shared crate published on crates.io (or a pinned git dependency). Reasons: a different risk profile (raw disk and admin), release cadence, CI (admin VHDX tests), signing and SmartScreen reputation, and support load.
11. **Naming risk.** "ReclaiMe File Recovery" is an established commercial recovery product (company founded 2009, secondary). "ReClaim Recover" in the same category is confusingly close. Run a trademark search (TÜRKPATENT, EUIPO, USPTO) before choosing any "ReClaim + recovery" name (unverified legal risk).

---

## 1. What "best open-source option" has to mean in 2026

Users who lose files want, in order:

1. To be told immediately whether recovery is realistic, and what not to do.
2. The intact copy if one exists anywhere (bin, cloud, snapshot, autosave).
3. Original names and folders where possible.
4. To see before recovering (preview) and to know which files are good.
5. No paywall after a scan, no adware, no "activate to recover".

The established free options each miss something:

- **PhotoRec:** excellent carving, but names are lost and the UI is text or a basic Qt shell.
- **Windows File Recovery (winfr):** command line only, no preview, no per-file quality.
- **Recuva:** freeware, proprietary, GUI ageing.
- **DMDE Free:** 4,000 files per operation (verified, DMDE EULA), expert UI.

The 2026 open-source newcomers already promise "read-only" and "honest confidence" (BitBroom Rescue, Rescyo), so those words alone will not stand out. Differentiators that remain open:

- **A guided "Easy wins first" flow.** Recycle Bin on every volume, Previous Versions, File History, OneDrive/Google Drive/Dropbox bin guidance, Office/Notepad/VS Code autosaves, all before any raw scan.
- **Evidence-based per-file states**, with the reason shown: "clusters reallocated to X", "reads as zeros (TRIM)", "JPEG decodes fully".
- **A first-class Turkish UI and Turkish-specific handling.** CP857 short names, Windows-1254 text previews, and Turkish document formats (see section 8).
- **Shared UX and trust with ReClaim Cleaner.** One author, one design system, signed releases, docs and FAQ in both languages.
- **Published, reproducible test results.** VHDX scenario suite with hash comparisons, and runs against public corpora (DFRWS 2006 challenge, NIST CFReDS).

---

## 2. Landscape

### 2.1 Established tools

| Tool | License | Techniques | Quality notes | What to borrow (ideas) | What we cannot reuse in MIT |
|---|---|---|---|---|---|
| **TestDisk** (CGSecurity) | GPL-2.0+ (verified, GitHub) | Partition table scan and repair, boot sector rebuild from backup, FAT/NTFS/exFAT/ext undelete listing, copy-out | Gold standard for lost partitions, but its "Write" step changes the disk | Read-only search for partitions by boot-sector signatures and backup boot sectors; listing deleted files per FS | All code. Do not copy its partition-type tables or heuristics verbatim |
| **PhotoRec** (CGSecurity) | GPL-2.0+ (verified) | Signature carving. Finds block size from the first files found (pass 0); later passes recover some fragmented files; free-space-only mode for FAT/NTFS/ext; paranoid validation (default); optional brute force for fragmented JPEG; text detection by index of coincidence (verified, CGSecurity wiki) | Over 480 extensions in about 300 families (secondary, Wikipedia). Very robust, names lost | Block-aligned scanning; validate-then-keep; "keep corrupted files" as an opt-in with a prefix; free-space-only mode; EXIF time for mtime | All `file_*.c` format code and the signature table. Re-derive signatures from format specs |
| **Windows File Recovery** (`winfr`, Microsoft) | Proprietary, free (Microsoft Store) | Regular (healthy NTFS, uses MFT) and Extensive (all FS). Legacy modes: `/ntfs` (MFT), `/segment` (file record segments), `/signature` (headers). Microsoft's table: NTFS recently deleted means Regular; deleted a while ago, formatted or corrupted means Extensive; FAT/exFAT means Extensive (verified, Microsoft support) | Source and destination must differ; no preview; `/u` recovers undeleted files such as Recycle Bin contents | Mode naming users already know ("Quick" vs "Deep"); filters by name or path; recovery folder naming `Recovery_<date>` | Nothing (closed). A GUI wrapper exists (`winfr-pro`, MIT, Tauri) |
| **Recuva** (Piriform / Gen Digital) | Proprietary freeware | NTFS/FAT undelete, deep scan, secure overwrite. Per-file state: Excellent, Poor, Very poor, Unrecoverable, with an overwritten-clusters comment (secondary) | The state column is the best-known UX idea in consumer recovery. Reviewers say it is not always right (secondary) | Per-file state with a reason column; colour coding; "Info" pane showing cluster facts | Names, icons, wording, UI layout |
| **DMDE** | Proprietary; Free edition limited to 4,000 files per operation from the file panel, non-commercial use (verified, EULA) | NTFS/FAT/exFAT/ReFS/ext/HFS/APFS reconstruction, raw search, disk editor, imaging, RAID constructor (secondary) | Expert favourite for reconstruction after corruption | "Virtual reconstruction" of folder trees from scattered metadata; scan-log reuse (Rescyo imports DMDE logs) | Everything |
| **R-Studio** (R-Tools) | Proprietary | "IntelligentScan" (undocumented), known-file-types raw scan with user-added signatures, recovery-chance estimate (secondary) | Professional, many file systems and RAID | User-extensible signature list; recovery chance estimate | Everything |
| **Disk Drill** (CleverFiles) | Proprietary; free tier recovers about 500 MB on Windows (secondary, conflicting figures) | Scan, preview, carving; Recovery Vault on Windows stores deleted-file metadata in the background (no file copies), on FAT32/exFAT/NTFS (secondary, vendor help) | Polished UI | Pre-loss protection idea (but see the caution in section 5.12); preview-first UX | Everything |
| **EaseUS Data Recovery Wizard** | Proprietary; free tier about 2 GB (secondary) | Quick and deep scan, filters, preview | Mass-market UX | Filter chips (type, size, date), "lost files" grouping | Everything |
| **Active@ File Recovery** (LSoft) | Proprietary | QuickScan, SuperScan (signature-based), custom signature templates, disk images (secondary, vendor pages) | Templates editor for signatures | User-defined signature templates (v1+) | Everything |
| **Foremost** | Public domain (US Government work, 17 USC 105). Two files (`api.c`, `ole.h`) are GPL-2+ (verified, Debian copyright) | Header/footer carving with a config file | Classic, simple, unmaintained | Config-driven signatures; per-type max size | Public-domain parts are legally reusable but dated; avoid the GPL files. Prefer re-implementation |
| **Scalpel** | Apache-2.0 since 2013 (verified, README); unmaintained | Foremost-derived carving, two-pass, faster | Unmaintained | Two-pass "index then carve" design | Code is Apache-2.0 (compatible) but old C; ideas only |
| **bulk_extractor** | NPS-era code public domain; post-2015 code GPL-3.0-or-later (verified, LICENSE.md, which supersedes an earlier MIT statement) | Feature extraction (emails, URLs, EXIF), recursive decompression, histogram | Forensic, not undelete | Recursive "scanner" model; parallel page processing | Post-NPS code (GPL-3.0) |
| **The Sleuth Kit** (`tsk_recover`, `icat`, `fls`) | Core code under IBM Public License and Common Public License, with GPL and other parts (verified, TSK licenses README) | FS-aware listing and recovery of unallocated files (`tsk_recover` defaults to unallocated files only, `-e` for all) (verified, man page) | Reference implementation for NTFS/FAT/exFAT forensics | Use as a **test oracle** in CI: compare our listings and extracted bytes with `fls`/`icat` on the same images | Do not copy code (CPL/IPL obligations, copyleft parts) |
| **Autopsy** | Apache-2.0 (verified, README) | GUI over TSK, includes a PhotoRec carver module | Heavy (Java), forensic workflow | Case/session model, timeline, report export ideas | Java code irrelevant; Apache-2.0 would be compatible |
| **ntfsundelete** (ntfs-3g / ntfsprogs) | GPL-2.0+ (verified) | Scans MFT for deleted records; filters by name, size, date and percentage recoverable; never writes the volume; cannot recover compressed or encrypted files (shows 0%); does not join name and data from separate records; picks the largest size when unsure (verified, man page) | Small and honest | "Percentage recoverable" per file; the explicit list of what it refuses to do | All code |
| **RecuperaBit** | GPL-3.0 (verified) | NTFS reconstruction under heavy damage: finds MFT and index records by signature anywhere on disk, then rebuilds the tree bottom-up; estimates partition geometry (verified, MSc thesis abstract) | The best open reference for quick-formatted or damaged NTFS | The bottom-up algorithm (from the thesis, not the code) for Phase 3 | Code |
| **GNU ddrescue** | GPL-3.0+ | Imaging with a mapfile: copy pass (large blocks, skip errors), trim, scrape, optional retry (verified, manual) | De facto standard for failing drives | The pass structure and mapfile format (writing a compatible mapfile gives interop with ddrescueview) | Code |
| **analyzeMFT** | MIT (verified, now `rowingdude/analyzeMFT`) | Python MFT parser: active and deleted entries, CSV/JSON/body-file output, test MFT generator | Good test-data idea | Synthetic MFT generator concept; body-file export for timeline tools | MIT, reusable with notice, but it is Python; ideas only |
| **MFTECmd / RBCmd** (Eric Zimmerman) | MIT (MFTECmd verified via GitHub API) | $MFT, $J, $LogFile and $I30 parsing; RBCmd parses Recycle Bin `$I` files | Industry-standard DFIR parsers (.NET) | Use as cross-check oracles; field coverage checklist | MIT; code reuse possible with notice, but C#; ideas only |
| **KAPE** (Kroll) | Proprietary; Solo edition free for non-commercial, LE and education use; commercial use needs a licence (secondary, Kroll EULA) | Target/module collection of artifacts | DFIR triage, not recovery | Target-list idea for "where autosaves live" (the KapeFiles target definitions are a community repo; check their licence before using data) | Binary and EULA forbid commercial-adjacent use |
| **libyal** (`libfsntfs`, `libvshadow`) | LGPL-3.0 (verified) | NTFS and VSS store parsing | Mature C libraries | Format documentation in the libyal docs repos | Avoid linking (LGPL with Rust static linking is awkward) |
| **NTFSTool** (thewhiteninja) | MIT (verified), 630 stars | MBR/VBR/MFT display, undelete of not-in-use records, USN, $LogFile, BitLocker FVE parsing, EFS key handling, LZNT1 and XPRESS decompression (verified, README) | Mature C++ reference | Check its undelete and compressed-file handling as a reference; MIT allows reuse with notice | Nothing blocked; C++ though |

### 2.2 Rust crates (versions and licences from the crates.io API, 2026-10-16, verified)

| Crate | Version / updated | Licence | What it gives | Fit |
|---|---|---|---|---|
| `ntfs` (Colin Finck) | 0.4.0 / 2023-06 (repo pushed 2026-01) | MIT OR Apache-2.0 | Read-only, `no_std`, no `unsafe`. Resident and non-resident attributes, attribute lists, sparse data, index lookup, Read/Seek on files. **Not supported:** compression, encryption, journaling, reparse points, security descriptors (verified, README) | Clean base for live-file reading. Deleted-record APIs not documented (unverified). Not the best fit for undelete |
| `mft` (omerbenamram) | 0.7.0 / 2025-12 | MIT/Apache-2.0 | Safe Rust MFT record parser, resident stream extraction, JSON/CSV, `mft_dump` (verified) | **Recommended record parser** if we write our own volume layer. Most downloaded (about 750k) |
| `ntfs-reader` (kikijiki) | 0.6.0 / 2026-10-01 | MIT OR Apache-2.0 | Windows volume or image; MFT scan; USN reader; `deleted_files()` with best-effort content; `resolve_deleted_path` with `DeletedPathMarker::{Lost, Deleted, TooLong}`; `MftScan` chunked scanning; compressed and encrypted streams return errors (verified, README and CHANGELOG) | Strong candidate to prototype with. Low adoption (22 stars). `research2-fastscan.md` recommends it only as a test oracle for the cleaner (it lacks allocated size and is memory-heavy) |
| `usn-journal-rs` | 0.5.0 / 2026-10-13 | MIT | USN enumeration, journal iterator, path resolver (Windows only) | For "deleted at" evidence and names |
| `ntfs-core` / `ntfs-forensic` (SecurityRonin) | 0.9.8 / 2026-08 | Apache-2.0 | From-scratch NTFS over any Read+Seek: runlists, attribute lists, LZNT1 (via `lznt1`), MFT entry carving, USN, $LogFile, journal rewind; claims cargo-fuzz and TSK cross-validation (verified, docs.rs) | Feature-rich but created June 2026, about 6k downloads. Read as prior art; audit before depending |
| `sootmark-*` (recyclebin, vss, usn, ntfslog, indx, disk, vhdx, ewf) | 0.1.x / 2026-10 | MIT OR Apache-2.0 | Forensic readers for `$I` files, VSS stores, USN, $LogFile, $I30 indexes, raw disks (verified, crates.io) | Very new (days old, under 300 downloads each). Good reading material; vendor-in only after review |
| `unearth` | 0.4.0 / 2026-07 | MIT | Read-only undelete (FAT/exFAT/NTFS/ext/HFS+), carving of 150+ formats, SHA-256 dedupe, resumable imaging; FAT/exFAT and carving assume contiguous data (verified, README) | Closest MIT prior art in Rust; 0 stars. Read its NTFS/FAT code with care |
| `forsmost` | 0.1.1 / 2026-05 | MIT OR Apache-2.0 | Foremost-style carver | Prior art only |
| `forensic-carve` | 0.1.1 / 2026-08 | Apache-2.0 | Single-pass signature sweep dispatching to per-format carvers | Prior art for engine design |
| `vshadow` (vshadow-rs) | 0.2.0 | **AGPL-3.0-only** | Offline VSS store parser | **Do not use** |
| `vsc-core` | 0.2.2 | Apache-2.0 | VSS store and catalog reader | Possible for offline images (Phase 3); live Windows can use the VSS API |
| `exhume_*` | various | **GPL-3.0-only** | Disk images, partitions, file systems | **Do not use** |
| `ewf` | 0.4.10 | Apache-2.0 | E01 reader | Optional input format (v1+) |
| `lznt1` | 0.1.3 | MIT | LZNT1 decompression | NTFS compressed files |
| `ms-compress` | 0.2.1 | LGPL-2.1 and others | LZNT1/XPRESS/LZX | **Avoid** (LGPL). XPRESS for WOF needs another MIT crate or our own decoder (`xpress-huffman` exists; check its licence) |
| `fatfs` | 0.3.6 | MIT | FAT read/write | No deleted-entry API. Write our own read-only FAT/exFAT directory parser (small) |
| `gpt` / `mbrman` / `bootsector` | 4.1.0 / 0.6.1 / 0.2.0 | MIT / MIT OR Apache / MIT | Partition table parsing | Read-only partition listing |
| `zune-jpeg`, `jpeg-decoder`, `png`, `image` | current | MIT/Apache (zune adds Zlib) | Full decoders, used as validators | JPEG/PNG validation and thumbnails |
| `zip`, `lopdf`, `mp4`, `matroska` | current | MIT or MIT/Apache | Container parsers | ZIP/OOXML, PDF, MP4/MOV, MKV validation |
| `symphonia`, `mp4parse` | current | **MPL-2.0** | Audio decoders, MP4 parser | MPL is file-level copyleft; using unmodified crates in an MIT app is generally fine, but list them in third-party notices (unverified legal reading) |
| `rusqlite` | 0.40.2 | MIT | SQLite (bundled) | Session store and SQLite file validation |
| `aho-corasick`, `memchr` | current | Unlicense OR MIT | Multi-pattern search | Carving header scan |
| `blake3`, `crc32fast` | current | CC0/Apache, MIT/Apache | Hashing | Dedupe and report hashes |
| `libfuzzer-sys`, `arbitrary`, `proptest` | current | MIT/Apache (+NCSA) | Fuzzing and property tests | Parser hardening |

Note: `tauri` 3.0.0-alpha.5 exists (verified, crates.io). Stay on Tauri 2 stable to share the ReClaim Cleaner stack (Cleaner pins `tauri =2.12.4`).

### 2.3 New open-source recovery projects (2025 and 2026)

| Project | Licence | Stack | Notable ideas | Caveats |
|---|---|---|---|---|
| **BitBroom Rescue** | MIT (verified) | C# / WPF, .NET 10 | Same product strategy as ReClaim: a cleaner family plus a "Rescue" app. Read-only `ISectorSource`; destination guard refuses (does not just warn); clone-first imager with bad-sector map; scores High/Good/Fair/Poor with a reason; portable only; `$I/$R`, NTFS, FAT32 (contiguous fallback), exFAT, VSS previous versions, carving with validators, `health` command for SSD/TRIM/SMART (verified, README) | 3 stars, created 2026-07. Its README says TRIMmed blocks are erased "usually within minutes"; treat that as unverified. Read for UX and test ideas; MIT allows reuse with notice |
| **Rescyo** | GPL-3.0-or-later (verified) | Rust, CLI and TUI, GUI planned | Every file reports how it was assembled (FAT chain, mirror FAT, MFT runlist, contiguous guess) and whether its content verifies; bad sectors read once; pauses and recommends imaging when errors pile up; refuses same physical device; SQLite sessions; imports DMDE scan logs; LZNT1 and WOF; fuzzed parsers; ddrescue-compatible map (verified, README) | **GPL: ideas only.** Alpha (v0.3), 0 stars, created 2026-10-11 |
| **KVC FileRecoveryTool** (wesmar) | MIT (verified) | Native C++ Win32 GUI and CLI | Results labelled Full, Partial, MetadataOnly, unavailable; deleted NTFS records with cleared allocation maps reported as metadata only; Hyper-V scenario tests with SHA-256 comparisons and measured fragmentation (verified, README) | Releases are distributed as a **password-protected archive** (password stated in the README). That is a common malware-distribution pattern, so do not run its binaries. Reading MIT source is fine |
| **unearth** | MIT | Rust CLI and MCP server | See 2.2 | 0 stars |
| **NullVeil** | Apache-2.0 (verified) | Rust CLI/TUI | Hand-written FAT/exFAT/NTFS/ext parsers, carving, TUI hex preview; Windows raw access "deferred" (verified, README) | Learning project |
| **recoverer** | README says MIT, but the repo has no licence file (verified) | Rust engine plus WinUI 3 | Confidence by evidence: header only 45%, footer confirmed 65%, MFT-backed 75%; cross-session memory of recovered clusters; span reads in 32 MB chunks (verified, README) | Treat as all-rights-reserved until a LICENSE file exists. Its percentages are arbitrary; ours should be states with reasons |
| **Data-Recovery-CLI** (flerkenstudio) | No licence (verified) | Rust plus Tauri plus React | Same stack as ours; NTFS only | All rights reserved: do not copy |
| **winfr-pro** | MIT (verified) | Tauri GUI over `winfr.exe` | Shows the demand for a GUI over Microsoft's tool | Depends on a closed tool |
| **ntfs-undelete** (NikolaMilosa) | MIT (verified) | Rust | Small MFT undelete | 2023, small |

**Warning about search spam.** Many GitHub repositories named "<Product> Recovery Guide 2026" (Puran, DiskInternals, Glary, MyRecover, Raise) have 68 to 70 stars each and recent pushes (verified, GitHub search). They look like SEO download funnels. The ReClaim docs should tell users to download only from the official release page and to verify SHA-256 sums.

### 2.4 Licence rules for an MIT project

- **Safe to depend on:** MIT, Apache-2.0, BSD, Zlib, Unlicense, CC0. Keep a third-party notices file. MPL-2.0 crates (unmodified) are generally fine with notices (unverified legal reading).
- **Do not copy or link:** GPL/LGPL/AGPL code (TestDisk, PhotoRec, ntfs-3g, ddrescue, RecuperaBit, Rescyo, bulk_extractor post-2015, `exhume_*`, `vshadow`, libyal), and CPL/IPL code (The Sleuth Kit core).
- **Unlicensed repos are all rights reserved:** read for ideas only.
- **Allowed and useful:** running GPL tools (TSK, PhotoRec, ntfs-3g `mkntfs`) **in CI as test oracles or fixture builders**, without distributing them.
- **Do not bundle** GPL binaries such as PhotoRec in the installer. Optional "open in TestDisk if installed" is possible later.
- **Enforce in CI:** `cargo deny` with a licence allow-list. The frontend needs a similar check (`license-checker` or `pnpm licenses list`).
- **Patents:** the sequential-hypothesis-testing fragmentation patent (US8407192, NYU) is listed as "Expired - Fee Related" on Google Patents (verified). No other carving patents were checked (unverified). Do a quick patent search before shipping advanced reassembly.

---

## 3. Techniques, in order of reliability

### 3.1 Tier A: intact copies (check first, works on SSDs too)

**a. Recycle Bin**

- **Location:** `X:\$Recycle.Bin\<SID>\` on every fixed volume. Each deleted item has a `$I<id>.<ext>` metadata file and a `$R<id>.<ext>` data file (or folder) (secondary, consistent across sources).
- **`$I` layout:**
  - Header (8 bytes): version (1 for Vista to 8.1, 2 for Windows 10 and later).
  - 8-byte original size.
  - 8-byte FILETIME deletion time.
  - Path: v1 has a fixed 520-byte UTF-16 field; v2 has a 4-byte character count followed by the UTF-16 path (secondary; RBCmd and community parsers agree; verify with samples).
- **What bypasses the bin:**
  - Removable drives (USB sticks, SD cards), network drives, Shift+Delete, command-line `del`/`Remove-Item`, and files larger than the bin quota all skip it (secondary, Computer Hope).
  - External USB hard disks: unverified.
- **Admin scope:** an admin can read other users' bins. Default to the current user's SID; show others only after an explicit confirmation (privacy).
- **Restore:** "Copy to another drive" is always safe. "Restore to original location" is an in-volume rename, as Explorer does. It allocates no new data clusters but does write metadata. Offer it only when no deep scan is pending on that volume (unverified risk assessment).
- **Crates:** `sootmark-recyclebin` (MIT/Apache, days old), or about 150 lines of our own.

**b. Cloud recycle bins and version history** (guidance and deep links, no API calls in the MVP)

- **OneDrive:**
  - Personal accounts: 30 days. Work or school: 93 days unless the admin changed it.
  - Microsoft 365 subscribers can roll back the whole OneDrive to any point in the last 30 days (verified, Microsoft support).
  - From August 2026, files deleted in the cloud (web or another device) while locally available no longer go to the local Recycle Bin; they are only in the web recycle bin (verified, University of Waterloo IT notice).
- **Google Drive:** trash is deleted forever after 30 days (verified, Google support).
- **Google Photos:** backed-up items stay in the trash for 60 days, others 30 days (verified, Google support). A reported future change unifies this at 30 days (secondary, Android Authority).
- **Dropbox:** 30 days on Basic, Plus and Family; 180 days on Professional and Business (secondary).
- **Detection:**
  - Check whether OneDrive (`HKCU\Software\Microsoft\OneDrive\Accounts`), Google Drive for desktop and Dropbox are installed.
  - Check whether Known Folder Move redirects Desktop, Documents or Pictures into OneDrive (unverified key names; verify).
  - Show the right link and retention text.
- **Cloud-only placeholders** (Files On-Demand) have no local content. Recovery is only possible from the cloud bin (secondary, consistent).

**c. Previous Versions: Volume Shadow Copies, File History, System Restore**

- **Enumerate existing shadow copies:**
  - WMI `Win32_ShadowCopy` (properties include `DeviceObject`, `InstallDate`, `VolumeName`, `ClientAccessible`) (verified, Microsoft WMI docs).
  - Or `IVssBackupComponents::Query`, or `vssadmin list shadows` (needs admin).
- **Read:** use normal read-only file APIs on `\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopyN\<path>` (path format secondary but widely used). Copy out to the destination.
- **Stronger: run the MFT scan on the shadow device itself.** A file that was live, or deleted but intact, when the snapshot was taken can be copied intact. ntfs-reader documents opening shadow device paths (verified, CHANGELOG 0.6.0).
- **Never create a new snapshot on the source volume.** The diff area lives on that volume (unverified detail, standard VSS design).
- **Defaults vary.** System Protection and File History are commonly off on clean Windows 10/11 installs (secondary, conflicting). Detect actual snapshots instead of assuming. Snapshots are purged when shadow storage fills, and feature updates can remove restore points (secondary).
- **File History:**
  - Backs up Libraries, Desktop, Contacts and Favorites hourly by default to an external or network target.
  - Folder layout: `<target>\FileHistory\<user>\<PC>\Data\...`, with timestamped copies; config under `%LOCALAPPDATA%\Microsoft\Windows\FileHistory` (secondary).
  - Detect the configured target and list versions of matching paths. The exact versioned filename pattern is unverified.
- **System Restore** does not restore personal files, but its restore-point snapshots contain the whole C: volume at that time. So VSS browsing is the useful path, not "System Restore".

**d. Application autosaves and temp files**

- **Office:**
  - `%LOCALAPPDATA%\Microsoft\Office\UnsavedFiles` (`.asd`, the "Recover Unsaved Documents" folder).
  - Per-app AutoRecover folders such as `%APPDATA%\Microsoft\Word` (the configured path is in File > Options > Save) (secondary, Microsoft blog).
  - Retention of about 4 days was documented for Office 2010 (secondary; may differ now).
  - Recovered `.asd` files open from inside the Office app.
- **Windows 11 Notepad:** unsaved tab content persists in `%LOCALAPPDATA%\Packages\Microsoft.WindowsNotepad_8wekyb3d8bbwe\LocalState\TabState\{GUID}.bin` (secondary; Velociraptor artifacts and Securelist). The binary format needs a parser (ogmini's Notepad-State-Library research; check its licence).
- **VS Code:** Local History in `%APPDATA%\Code\User\History` (secondary); also `%APPDATA%\Code\Backups` for hot exit (unverified).
- **JetBrains:** Local History in the IDE system directory (secondary).
- **Adobe and other editors:** autosave folders; target lists like KapeFiles can help build the list (check licence).
- **Browsers:** partial downloads (`.crdownload` for Chromium, `.part` for Firefox). Partial only; label them that way (unverified naming details).
- **Explorer thumbnail cache** (`%LOCALAPPDATA%\Microsoft\Windows\Explorer\thumbcache_*.db`) can hold small thumbnails of deleted photos (unverified for current builds). Phase 3, opt-in, clearly labelled "thumbnail only, not the original".

### 3.2 Tier B: file-system metadata undelete

**a. NTFS**

What deletion does:

- **The record is freed, not wiped.** The MFT record's in-use flag (`FILE_RECORD_SEGMENT_IN_USE`, 0x0001) is cleared, and the record's `SequenceNumber` is incremented each time the record is freed (verified, Microsoft FILE_RECORD_SEGMENT_HEADER docs).
- **Clusters are freed.** The file's clusters are marked free in `$Bitmap`, and the entry is removed from the parent's `$I30` index (secondary, consistent across forensic sources).
- **Runs survive.** The attributes, including `$DATA` data runs, normally remain in the freed record until NTFS reuses it. Reuse of low record numbers is commonly observed (unverified).

Scan algorithm:

1. **Open the volume:** `\\.\X:`, a `HarddiskVolumeShadowCopyN` device, or a partition offset inside an image.
2. **Read the boot sector:** bytes per sector, sectors per cluster, `$MFT` LCN, clusters (or bytes) per file record, which is often 1024 B but 4096 B on 4Kn disks (verified, Microsoft NTFS and 4K disks blog). Cross-check with `FSCTL_GET_NTFS_VOLUME_DATA` on live volumes.
3. **Map `$MFT`:** read record 0 to get `$MFT`'s own runlist, since `$MFT` itself can be fragmented. Then stream every record in large sequential reads (1 to 8 MiB).
4. **Validate each record:**
   - Signature `FILE`.
   - Apply the update sequence fixups. A failed fixup means a torn or overwritten record: mark it and keep partial metadata.
   - Keep records where `IN_USE == 0` and a `$FILE_NAME` attribute exists.
   - Extension records (base reference != 0) are joined via `$ATTRIBUTE_LIST` when the base record survives.
5. **Extract:**
   - Names: prefer the Win32 or Win32+DOS namespace over DOS-only; keep all hard-link names.
   - Parent reference (record plus sequence).
   - `$STANDARD_INFORMATION` and `$FILE_NAME` timestamps.
   - Flags: compressed, encrypted, sparse, reparse tag.
   - `$DATA`, unnamed and named (ADS): resident bytes or runlist, real size, allocated size, initialized size.
6. **Reconstruct paths:**
   - Follow parent references.
   - If the parent record is in use with a matching sequence, the path is live.
   - If the parent is deleted, continue through deleted parents.
   - If the parent was reused (sequence mismatch) or is gone, put the file under `Lost folder #<record>` instead of guessing. Same idea as ntfs-reader's `DeletedPathMarker::Lost`.
7. **Assess recoverability per file** (section 4.2):
   - Compare runs against the current cluster bitmap (`FSCTL_GET_VOLUME_BITMAP` on live volumes, or `$Bitmap` from an image).
   - Optionally build a cluster-owner interval map from in-use records to name the file that took them.
   - Sample-read the first, middle and last clusters to detect all-zero (TRIM) content.
8. **Copy out:**
   - Read runs in order. Sparse runs give zeros.
   - Truncate to the real size. Bytes beyond the initialized size are zeros.
   - Hash while writing.

Special cases:

- **Resident files** (small files stored inside the MFT record) survive until the record is reused. They often recover perfectly even on SSDs, because record reuse, not TRIM, destroys them (unverified that TRIM never touches MFT clusters; `$MFT` clusters stay allocated, so they are not trimmed).
- **Compressed NTFS (LZNT1):** decode 16-cluster compression units with the MIT `lznt1` crate. ntfsundelete and older tools refuse these (verified).
- **WOF / CompactOS:** XPRESS-compressed data in a `WofCompressedData` stream behind a reparse point. Phase 3.
- **EFS-encrypted:** content is ciphertext and cannot be decrypted without the user's keys. Mark it as "Encrypted (EFS): not recoverable in usable form". NTFSTool can handle EFS keys given the user's password; out of scope.
- **Cloud placeholders** (cloud-files reparse tags): no local clusters means "Cloud-only". Point to the cloud bin.
- **Hard links:** multiple `$FILE_NAME`s; recover once and list the alternative paths.
- **ADS:** recover the unnamed stream by default. `Zone.Identifier` shows the download origin, which is privacy-sensitive, so show it only in details.
- **Timestamps:** deletion time is not in the record. Use the USN journal (exact) or Recycle Bin `$I`. Otherwise show "deleted: unknown".

Supporting evidence (names and times, not content):

- **USN journal (`$Extend\$UsnJrnl:$J`):**
  - Records with `USN_REASON_FILE_DELETE` give name, parent reference, file reference and time of deletion.
  - Default maximum size on Windows 10 system volumes is about 32 MB (secondary). Measured on the dev machine: 32 MB covering only 0.8 hours of activity (verified, `research2-fastscan.md`).
  - Useful for "what did I just delete and where was it", not for old deletions.
- **`$LogFile`:** redo/undo records can show deallocations, but it is small and cycles fast (secondary). Phase 3 or later; parsers exist (`sootmark-ntfslog`, `ntfs-core`).
- **`$I30` index slack:** deleted entries can survive in the parent folder's index buffer, with names, sizes and times (secondary). Recovers names when the MFT record is reused. Phase 3.

Live volume consistency:

- The volume changes while you scan. Accept the small inconsistency: you are reading free space, and Windows is writing.
- Prefer scanning an **existing** shadow copy or an image when available.
- If the source is C:, say clearly that Windows itself keeps writing (logs, page file, updates, Prefetch) (unverified specifics).

**b. FAT12/16/32**

- **Deletion:** the first byte of the short-name entry becomes 0xE5 and the file's FAT chain is zeroed (marked free).
- **High word cleared:** on FAT32 some Windows versions also clear the high 16 bits of the starting cluster (secondary, forensic course material and Forensic Focus thread). Treat the start cluster as `low16` plus candidate high words (try 0 first, then candidates where the first cluster's content matches the expected type).
- **LFN entries:** long-name entries before the short entry are also marked 0xE5 but usually keep their UTF-16 text. Re-pair them with the short entry using the LFN checksum computed from the 8.3 name (verified concept; the exact deleted-entry behaviour is unverified).
- **Data:** with the chain gone, assume a contiguous allocation of `ceil(size / cluster)` clusters (Carrier's approach, secondary). A second strategy follows only currently free clusters. Validate with the format validator, because fragmentation silently corrupts.
- **Subfolders:** deleted folders keep their directory clusters until reused. Walking them recovers nested names.
- **Turkish 8.3 names:** short names are encoded in the OEM code page, which is CP857 on Turkish Windows (verified, CP857 is MS-DOS Turkish). Decode with the source system's OEM code page when no LFN survives. Make it user-selectable, defaulting to 857 when the UI is Turkish.
- **Card-level cases:** after a camera "format", FAT and the root directory are reset but data clusters are intact. Carving works well there (cameras write mostly contiguous files; unverified as a general rule).

**c. exFAT**

- **Deletion:** the entry type's InUse bit (high bit) is cleared, so 0x85 becomes 0x05 for the File entry, along with the Stream Extension and File Name entries (InUse semantics verified in the spec summary and the Linux `exfat_raw.h`; the exact byte change is inferred).
- **Contiguous files:** the Stream Extension's `NoFatChain` flag says the file was contiguous. Recovery is then exact: first cluster plus length (verified flag meaning).
- **Fragmented files:** the FAT chain is used, and forensic research reports the chain typically survives longer than the metadata (secondary, Vandermeer et al.). Follow it, checking the allocation bitmap.
- **Names:** full UTF-16 names are stored in File Name entries, so no code-page issue.
- **Usage:** SDXC cards (over 32 GB) and many USB sticks ship exFAT.

**d. ReFS** (including Windows 11 Dev Drive volumes)

- **Undocumented and copy-on-write.** Academic work (DFRWS 2020, ReFS 3.4) built a TSK extension and a page carver, and notes that findings go stale across versions (verified, paper abstract).
- **Out of scope for metadata undelete.** Offer carving only and say so.
- **TRIM default:** delete notifications are off by default on ReFS v2 (verified, fsutil docs). That helps carving but does not make metadata recovery feasible.

### 3.3 Tier C: signature carving

Engine design:

- **Input:** a block source (volume, partition, image, or "unallocated only" view).
- **Unallocated-only mode:** carve only clusters marked free in the bitmap or FAT. This is faster, avoids recovering live files and cuts false positives. Offer "whole volume" for corrupted or formatted volumes.
- **Header search:** scan for headers at **cluster or sector boundaries** (files on FAT/NTFS/exFAT start at cluster boundaries). Use an `aho-corasick` automaton over the first bytes of each block. Non-aligned hits (embedded thumbnails, archives inside archives) are found only in a separate "embedded" mode.
- **Size determination,** per format, in order of reliability:
  1. Explicit structure size (box sizes for ISO BMFF, the ZIP end-of-central-directory, SQLite page count times page size, RIFF size, EBML sizes).
  2. Structure walk to an end marker (JPEG EOI after a validated scan, PNG IEND with chunk CRCs, PDF `%%EOF` with xref check).
  3. Footer search with a per-type maximum size.
  4. Heuristic stop (next header found, or entropy or zero change). Mark the result "approximate".
- **Validation:**
  - Every carved candidate gets a validator result: Verified, Truncated, Corrupt or Unknown.
  - Only Verified and Truncated are shown by default; "Show unverified" is a toggle.
  - This mirrors PhotoRec's paranoid default (verified).
- **False-positive control:**
  - Minimum sizes per type.
  - Reject candidates whose header parses but whose first structure fails.
  - Suppress candidates fully inside another verified file (embedded thumbnails) unless embedded mode is on.
  - MP3 frame-sync patterns are notorious for false positives (unverified): require N consecutive valid frames.
- **Dedupe:**
  - Drop carved hits whose start offset equals the first cluster of a metadata-recovered file. Metadata wins, since it has the name.
  - Then dedupe by BLAKE3 content hash.
  - Keep a cluster-claim map so a resumed or later scan does not report the same bytes twice (recoverer's "cross-session memory" idea).
- **Names for carved files:**
  - Use internal metadata: EXIF `DateTimeOriginal` plus camera model for photos, `mvhd` creation time for MP4/MOV, `docProps/core.xml` title and modified date for OOXML, PDF `/Title` and `/CreationDate`.
  - Otherwise `<type>_<offset>.<ext>`.
  - Say clearly that original names and folders are not recoverable by carving (R-Studio's forum says the same, secondary).

Format validators for the MVP and v1:

| Format | Header / structure | Validator | Crate |
|---|---|---|---|
| JPEG | `FF D8 FF`, segments, SOS, entropy data, `FF D9` | Full decode; also check restart markers sequence | `zune-jpeg` or `jpeg-decoder` |
| PNG | 8-byte signature, chunks with CRC, IEND | CRC per chunk plus zlib inflate of IDAT | `png` |
| HEIC/HEIF, AVIF | ISO BMFF `ftyp` brand `heic`/`mif1`/`avif` | Box tree walk, `iloc` extents inside file | Own box walker (small), decode optional |
| MP4/MOV/3GP, CR3 | ISO BMFF boxes (`ftyp`, `moov`, `mdat`) | Box sizes consistent; `moov` present. Interrupted recordings have no `moov`: mark Truncated (repair tools like untrunc are GPL; out of scope) | `mp4` (MIT) or own walker |
| MKV/WebM | EBML header | EBML element sizes, Segment and Cluster structure | `matroska` |
| PDF | `%PDF-` | `startxref` and xref table or stream offsets resolve; handle incremental updates (KVC notes partial last update, verified README) | `lopdf` |
| ZIP, DOCX/XLSX/PPTX, ODF, EPUB, JAR, UDF (Turkish UYAP, see section 8) | `PK\x03\x04`, central directory, EOCD | Central directory consistent, CRC of members; OOXML has `[Content_Types].xml` | `zip` |
| OLE CFB (DOC/XLS/PPT/MSG) | `D0 CF 11 E0 A1 B1 1A E1` | Sector allocation table walk; size from FAT (PhotoRec does this, verified) | own (small) or `cfb` (check licence) |
| GIF, BMP, TIFF, WebP | standard | Structure walk; TIFF-based RAW (CR2, NEF, ARW, DNG) via IFD walk | `image` / own |
| MP3, WAV, FLAC | ID3 or frame sync, RIFF, `fLaC` | N valid frames; RIFF size | own; `symphonia` (MPL) optional |
| SQLite | `SQLite format 3\0` | Page size, page count, b-tree page types | own header check; `rusqlite` to open the copy |
| 7z, RAR | signatures | Header CRCs | own header checks |
| Plain text, CSV, source | no header | Index of coincidence or UTF-8/UTF-16/Windows-1254 validity per block (PhotoRec technique, verified) | Phase 3, off by default |

Fragmentation:

- **Most files are contiguous** (PhotoRec docs, verified). Contiguous carving is the MVP.
- **Bifragment gap carving** (Garfinkel, DFRWS 2007): for a file split in two, try candidate gap sizes and split points between a header fragment and a footer fragment, keeping the combination that validates (verified, paper summary). Practical for JPEG on cards and USB sticks with a fast validator. Phase 3, limited window.
- **Beyond two fragments:**
  - Pal, Sencar and Memon's sequential hypothesis testing (DFRWS 2008 best paper) and later JPEG work (DFRWS 2016, 97% vs 79% on one dataset) exist (verified, abstracts).
  - Too complex for v1. The related US patent is listed as lapsed (verified, Google Patents).
- **Honest output:** a fragmented file carved as contiguous often decodes up to the break and then shows garbage. Show "Partial: decodes to 62%" when the validator can measure it (JPEG MCU rows, MP4 sample table).

### 3.4 Tier D: lost partitions and formatted volumes

- **Read-only partition discovery:**
  - Parse MBR and GPT. GPT has a backup header at the last LBA. MBR has no backup.
  - Scan for volume boot sectors: NTFS (`NTFS    ` OEM ID; backup boot sector at the volume's last sector), FAT32 (backup at sector 6), exFAT (`EXFAT   `; backup boot region at sector 12).
  - Present each candidate as a virtual volume the user can scan. **Never write partition tables or boot sectors.** That is TestDisk's job, and we should point to it with a warning rather than do it.
- **Quick format vs full format:**
  - Since Windows Vista, a **full** format writes zeros to the whole volume (verified, Microsoft KB941961). After a full format, nothing is recoverable. Say so.
  - A **quick** format rewrites file-system metadata only. Data clusters survive until reused.
- **NTFS after a quick format or corruption:**
  - The new `$MFT` overwrites the start of the old one.
  - Old MFT records elsewhere (beyond the new MFT's size, or in old MFT fragments) can still be found by scanning for `FILE` signatures and rebuilt bottom-up by parent references (RecuperaBit's approach, verified thesis abstract). Phase 3.
- **FAT/exFAT after a quick format:**
  - Directory entries of subfolders remain in the data area.
  - Scanning for directory clusters (FAT entries starting with `.` and `..`; exFAT entry sets) recovers subfolder names and files (DMDE and Rescyo do variants of this, secondary). Phase 3.
- **Not supported, with honest copy:** RAID, Storage Spaces internals, dynamic disks (legacy), APFS, ext4 (Windows users rarely need it), BitLocker-locked volumes.

---

## 4. Hard truths for the UI

### 4.1 What the app must say, and when

1. **Stop writing to the disk now.**
   - Every app install, browser cache write, Windows Update, or even this app's own files can overwrite deleted data.
   - Show this on the first screen, before any scan.
   - If the source is the system drive, advise: close apps, do not install anything, and for irreplaceable data shut down and image the disk from another computer (or boot media).
2. **Never recover onto the same disk.** Enforced, not a warning (section 5.4).
3. **SSD and TRIM.**
   - If the device is non-rotational with TRIM enabled, show: "This is an SSD with TRIM on. Deleted files on SSDs are usually erased by the drive within a short time. File names may still appear, but their contents will likely be zeros. Check the Recycle Bin, cloud bins and Previous Versions first."
   - Detect with:
     - `IOCTL_STORAGE_QUERY_PROPERTY` (`StorageDeviceSeekPenaltyProperty`, `StorageDeviceTrimProperty`, `StorageDeviceProperty` for the bus type) (unverified exact behaviour per bus).
     - The NTFS `DisableDeleteNotify` value (0 means TRIM on; verified meaning, Microsoft fsutil docs).
   - TRIM through USB depends on UASP and the bridge chip, so external SSDs vary (secondary, Plugable KB and forums).
   - USB sticks and SD cards generally rely on internal garbage collection, not OS TRIM (secondary). Data there often survives longer, but controller behaviour varies.
   - The speed of erasure after TRIM varies by controller. Most modern SSDs support at least deterministic reads after TRIM (usually zeros) (secondary, ElcomSoft). Do not promise a time window.
4. **BitLocker.**
   - Scan the **unlocked volume** (`\\.\X:`), which returns plaintext (practitioner consensus; verify in tests). Raw physical-disk sectors are ciphertext.
   - If the volume is locked, ask the user to unlock it in Windows first. We do not handle recovery keys.
5. **Encrypted (EFS) and compressed files:**
   - EFS: not usable.
   - Compressed (LZNT1): supported from Phase 3; until then, say "not supported yet".
   - WOF/CompactOS files: same.
6. **Cloud placeholders:** "This file was online-only; its content was never on this disk. Use the OneDrive web recycle bin (30 days personal, 93 days work/school)."
7. **Small files** are often stored inside the MFT record (resident). They come back perfectly or not at all, depending on record reuse.
8. **Overwritten clusters cannot be recovered by any software.** "Free" clusters can also have been used and released since (temp files). That is why validation matters.
9. **Full format, `cipher /w`, secure-erase tools and SSD sanitize** leave nothing to recover. Say so plainly.
10. **Phones.**
    - Internal phone storage connected over USB uses MTP, not a block device, so it cannot be scanned (unverified for every phone; MTP exposes files, not sectors).
    - Modern Android storage is encrypted (file-based encryption).
    - An SD card "adopted" as internal storage is encrypted with a key kept on the phone and cannot be read on a PC (verified, Android source docs).
    - Use the phone's Recently Deleted / Google Photos trash instead.
11. **Physical damage** (clicking, not detected, very slow reads): stop. Repeated scans make it worse. Image once with the imaging tool, or go to a lab.

### 4.2 Per-file recoverability model

States (the label plus the reason are always shown; never "100%"):

| State | Criteria | Default action |
|---|---|---|
| **Intact copy** | From Recycle Bin, VSS, File History or an autosave | Recover |
| **Verified** | Validator decoded the whole file (JPEG, PNG, PDF xref, ZIP CRCs, MP4 boxes) | Recover |
| **Likely intact** | Metadata found; 0% of clusters reallocated; header matches the type; sampled clusters not all zero; no validator for this type | Recover, with "not verified" |
| **Partial** | Some clusters reallocated, or the validator decodes only part (show percentage), or carved and truncated | Recover on request; label the output file `PARTIAL_` |
| **Zeroed** | Clusters free but read as zeros (typical after TRIM) | Hidden by default; explain TRIM |
| **Overwritten** | 95% or more of clusters reallocated (to <file> when known), or header mismatch and validator fails | Hidden by default |
| **Metadata only** | Record has a name but no runs (cleared allocation map, as KVC reports for some cases), or record torn | Show name and dates as evidence only |
| **Not recoverable here** | EFS, cloud-only, unsupported compression, ReFS metadata | Explain why and where to look |

Per-file reason strings:

- "All 214 clusters are free and the JPEG decodes completely."
- "38 of 120 clusters now belong to C:\Windows\Temp\x.tmp."
- "All sampled clusters read as zeros; the SSD probably erased them (TRIM)."
- "Name from the USN journal; content not found."

Overall outlook banner per source:

- Media type (HDD, SSD, USB stick, SD card, image).
- TRIM state.
- Whether it is the system drive.
- Number of shadow copies.
- Recycle Bin item count.
- Free space.
- Time since deletion (user input; optional).

The output is one sentence, for example "Good chances: USB stick, no TRIM, nothing written since."

### 4.3 Suggested UI copy (EN / TR)

- EN: "Stop using this drive now. Anything written to it can overwrite your deleted files."
  TR: "Bu sürücüyü hemen kullanmayı bırakın. Ona yazılan her şey silinen dosyalarınızın üzerine yazılabilir."
- EN: "Recovered files must go to a different drive. This protects the files you are trying to save."
  TR: "Kurtarılan dosyalar başka bir sürücüye kaydedilmelidir. Bu, kurtarmaya çalıştığınız dosyaları korur."
- EN: "This is an SSD with TRIM on. Names may still appear, but contents are usually erased."
  TR: "Bu, TRIM açık bir SSD. Dosya adları görünebilir, ancak içerikler genellikle silinmiştir."
- EN: "Check these first: Recycle Bin, OneDrive bin, Previous Versions, Office autosaves."
  TR: "Önce bunlara bakın: Geri Dönüşüm Kutusu, OneDrive çöp kutusu, Önceki Sürümler, Office otomatik kayıtları."
- EN: "Verified: the file opens completely." TR: "Doğrulandı: dosya eksiksiz açılıyor."
- EN: "Partial: only part of this file could be recovered." TR: "Kısmi: bu dosyanın yalnızca bir kısmı kurtarılabildi."
- EN: "Overwritten: other data now occupies this file's space." TR: "Üzerine yazılmış: bu dosyanın alanında artık başka veriler var."

(Turkish strings are my drafts; have a native speaker review them for tone.)

---

## 5. Recommended architecture

### 5.1 Processes

| Process | Integrity | Responsibilities |
|---|---|---|
| **UI host** (Tauri 2, WebView2, React) | Medium (non-elevated) | Screens, Tier-A non-admin features (current user's Recycle Bin through normal APIs, autosave finders, cloud guidance), result browsing, preview rendering, recovery writes **if** the destination is user-writable (unverified final split; see the next row) |
| **Broker** (`*-broker.exe`, same signed bundle) | High (UAC via `ShellExecuteExW` "runas") | Raw device access, volume and disk enumeration with IOCTLs, VSS enumeration, MFT/FAT scans, carving, imaging, copy-out. Talks to the UI over a named pipe the UI creates, with an explicit DACL, `PIPE_REJECT_REMOTE_CLIENTS`, PID and image checks and SQOS identification (design already in `research2-fastscan.md`, section 7) |

This follows Microsoft's WebView2 guidance: keep the WebView2 host non-elevated and isolate admin work in a dedicated process (verified).

Alternative for a quick prototype only: a `requireAdministrator` manifest via `tauri_build::WindowsAttributes::app_manifest` (API verified in tauri-build docs; the elevation example is secondary). It contradicts that guidance and can break drag and drop from non-elevated Explorer (UIPI, unverified). Do not ship it.

Privacy rule from the fast-scan research:

- If the broker's token user differs from the UI's user (over-the-shoulder elevation), restrict listings to the UI user's profile plus shared folders.
- Require a second confirmation to show other profiles.

### 5.2 Crate layout (new workspace)

```
recover/                      (new repository)
  crates/
    blockio/       Read-only SectorSource trait; Windows volume and disk handles;
                   raw image, split raw, VHD/VHDX (read-only attach or parser), E01 later;
                   aligned reads, read-ahead cache, bad-sector map. No write method exists.
    devinfo/       Disk and volume enumeration, bus type, SSD/TRIM, BitLocker status,
                   physical-disk mapping for the destination guard.
    ntfs/          (shared crate, see section 9) MFT streaming, fixups, attributes,
                   runlists, deleted records, path rebuild, bitmap check, LZNT1.
    fat/           FAT12/16/32 and exFAT read-only parsers with deleted entries.
    carve/         Signature engine, per-format size finders and validators, dedupe.
    artifacts/     Recycle Bin $I, VSS listing, File History, autosave locations,
                   cloud client detection.
    imaging/       ddrescue-style imager, compatible mapfile.
    session/       SQLite session store, fingerprints, resume checkpoints.
    recover/       Destination guard, writer, hashing, reports (CSV, JSON, HTML).
    broker/        Elevated binary: pipe protocol, request validation.
  src-tauri/       UI host (non-elevated).
  src/             React UI (same design system as ReClaim Cleaner).
  fuzz/            cargo-fuzz targets (run on Linux CI).
  tests/           VHDX scenario harness and golden manifests.
```

- All parser crates: `#![forbid(unsafe_code)]`. `unsafe` lives only in `blockio` and `devinfo` (Win32 FFI via the `windows` crate, as in ReClaim).
- Bounded allocations: maximum record size, maximum runs per attribute, maximum path depth, cycle detection on parent chains, maximum carve size per type.

### 5.3 Read-only I/O details

- **`CreateFileW`:**
  - `\\.\X:` or `\\.\PhysicalDriveN` or `\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopyN`.
  - `GENERIC_READ`, `FILE_SHARE_READ | FILE_SHARE_WRITE` (volumes require `FILE_SHARE_WRITE`), `OPEN_EXISTING`, `FILE_FLAG_NO_BUFFERING`.
  - Treat volume handles as non-cached: sector-aligned offsets, lengths and buffers.
  - Use `FSCTL_ALLOW_EXTENDED_DASD_IO` to read the last sectors (backup boot sector).
  - All verified, CreateFile docs.
- **Never call** `FSCTL_LOCK_VOLUME`, `FSCTL_DISMOUNT_VOLUME`, any `IOCTL_DISK_SET_*`, `DeviceIoControl` codes that write, `chkdsk`, or VSS snapshot creation.
- **Disk to volume mapping:** `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` (verified). Layout: `IOCTL_DISK_GET_DRIVE_LAYOUT_EX`. Size: `IOCTL_DISK_GET_LENGTH_INFO`. Sector sizes: `StorageAccessAlignmentProperty`.
- **Throughput:** sequential reads of 1 to 8 MiB with two to four requests in flight (overlapped I/O). Optional low I/O priority so the PC stays usable (unverified benefit).
- **Bad sectors:** on a read error, split the block, read sector by sector once, record bad ranges, fill with a marker pattern, and continue. Never retry endlessly. If error density rises, pause and recommend imaging (Rescyo's behaviour, verified README).

### 5.4 Destination guard

Rules:

- Resolve the destination with `GetFinalPathNameByHandleW` on an opened directory handle, which follows junctions, mount points and `subst`.
- Get the volume with `GetVolumePathNameW` and `GetVolumeNameForVolumeMountPointW`, then the physical disks via `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`.
- Refuse if any physical disk is shared with the source.
- If the source is an image file, refuse destinations on the image's volume.
- If the destination disk cannot be determined (some network or virtual volumes), refuse unless the user explicitly confirms. Network shares are allowed but warn about speed.
- Refuse a VHDX destination that lives on the source disk: resolve the VHDX's backing file location (unverified API path; `GetStorageDependencyInformation` is a candidate).

The app's own writes:

- Session DB, logs, previews cache and the WebView2 user data folder all go to a workspace folder on the destination or on the drive the portable app runs from. Never on the source.
- Tauri 2 lets you set a per-webview data directory (`WebviewWindowBuilder::data_directory`; unverified exact API name in 2.12). Default WebView2 storage is under `%LOCALAPPDATA%` (verified that the app owns the UDF lifecycle, WebView2 docs).
- Also write in-session logs to the destination.

Distribution:

- **Portable zip is the default distribution.** Installers write to C:.
- Document that even launching an exe can create small writes on C: such as Prefetch (unverified).

Writing files:

- Never overwrite an existing destination file; suffix it.
- Recreate folder structure under `Recovered_<timestamp>\<source label>\...`.
- Long paths (`\\?\` prefix), invalid characters, and names that collide case-insensitively are handled.
- Set the recovered file's timestamps to the original modified/created times when known (optional).
- Strip nothing and add no Mark of the Web, but **never execute or auto-open** recovered files. Executables and scripts get a warning badge.

### 5.5 Imaging (clone first)

- Offered prominently for removable media, failing disks and when the source is the system drive (image to another disk, then scan the image).
- Two-pass ddrescue-style algorithm, re-implemented:
  1. Copy pass in large blocks, skipping errors.
  2. Trim from both edges.
  3. Scrape remaining blocks sector by sector.
  4. Optional retries, off by default.
- Mapfile in ddrescue's text format for interop (format verified in the manual; GPL concerns apply to code, not to the file format).
- Output raw `.img` (sparse where the destination FS supports it). Hash the image (SHA-256). Allow resume from the mapfile.
- Progress: bytes read, rescued, bad, current rate, ETA, and a sector map strip (like ddrescueview).

### 5.6 Sessions and resume

- SQLite (via `rusqlite`) in the workspace folder.
- **Source fingerprint:** disk serial, model and size, partition offset and length, volume serial, and a hash of the first 1 MiB plus the boot sector. A mismatch means resume is refused.
- **Tables:** scans (type, parameters, last offset, status), findings (method, names, path, parent chain, times, runs, sizes, flags, state, validator result, preview thumb path), recoveries (finding, destination, SHA-256/BLAKE3, time), bad ranges.
- Deep carving checkpoints every N MiB. MFT scans are fast enough to re-run (seconds to a minute; `$MFT` here is 8.99 GiB on NVMe, `research2-fastscan.md`).
- Stream findings into SQLite instead of holding millions in RAM. Live C: volumes can have 9M+ record slots (verified there).

### 5.7 Preview

- Decode images in Rust and send a bounded-size PNG thumbnail to the UI. The WebView never parses raw recovered bytes, which reduces parser exposure. Do not lean on this; WebView decoders are robust too.
- Text: first 64 KiB, encoding detection (UTF-8, UTF-16 with BOM, Windows-1254 for Turkish legacy text, Windows-1252), with a selector.
- Hex: virtualized view of the first and selected regions, with cluster boundaries and "this cluster is reallocated" shading.
- PDF: first page, either via WebView2's built-in PDF viewer on a `blob:` with scripting off, or via text extraction (`lopdf`). PDFium bindings add a native DLL (check licence). Phase 2 decision.
- Video: show container facts (duration, resolution, codec, `moov` present) and the first decodable frame (Phase 3, optional).
- Office: show `docProps` title and author plus the first paragraph text from `document.xml`.
- Preview reads go through the broker with a size cap. The preview cache lives in the workspace, never on the source.

### 5.8 Results UI and filters

- Views: "Folders" (reconstructed tree, with Lost folders), "Types" (Photos, Videos, Documents, Archives, Audio, Other), and "Sources" (Recycle Bin, Previous Versions, Deleted (file system), Found by content).
- Filters: name/extension (Turkish-safe case folding; ReClaim already avoids locale lowercasing), size range, date range (modified, created, deleted when known), path contains, state, method.
- Default sort: state, then date.
- Selection basket with a running total of size and destination free space.
- Scan progress: phase, bytes processed, rate, ETA, findings by type, bad sectors; pause, resume and cancel. Cancelling keeps the findings.

### 5.9 Reports

- CSV and JSON per recovery run.
- A self-contained HTML summary with source facts (device, TRIM, BitLocker, image hash), counts by state and per-file lines: original path or carved name, size, method, state and reason, hashes, destination path.
- Optional DFXML-style or body-file export for forensic users (Phase 3).

### 5.10 Security and supply chain

- Parsers face hostile input (crafted images, malicious USB sticks) while running elevated. So:
  - No `unsafe`.
  - Bounded loops and allocations.
  - Fuzz every parser (MFT record, attribute list, runlist, FAT and exFAT entries, `$I`, each format validator).
  - The broker validates every request: only device paths it enumerated, only destination paths that passed the guard.
- The pipe protocol is versioned and length-prefixed. The UI treats broker output as untrusted (bounds checks).
- `cargo deny` (licences, advisories, bans), `cargo audit`, exact version pins (`=x.y.z` as in ReClaim, `pnpm add -E`), reproducible release builds, SHA-256 sums, code signing.
- No network access from the broker. The UI makes no network calls during recovery: cloud links open in the browser.
- No telemetry.

### 5.11 i18n

- EN and TR from day one, same i18n setup as ReClaim Cleaner.
- All states, reasons and warnings are translated strings with parameters, not concatenations.
- Use the Turkish-aware CP857 option for FAT short names and Windows-1254 for text previews.

### 5.12 Things not to build

- **No background "protection" service by default.** A Disk Drill-style Recovery Vault service writes metadata continuously. It conflicts with the cleaner's ethos and adds a resident admin component. Better: a one-click "turn on File History / System Protection" guide in docs.
- No secure-delete or wipe features in the recovery app (opposite promise).
- No partition-table or file-system "repair".
- No in-place restore except Recycle Bin restore with the caveat.

---

## 6. Phased plan with estimates

Assumptions:

- One experienced Rust and Tauri developer (the ReClaim author) working full time with AI assistance, reusing the ReClaim design system, i18n and the helper/pipe pattern.
- Calendar time roughly doubles part-time.
- All estimates are unverified.

| Phase | Scope | Estimate | Exit criteria |
|---|---|---|---|
| **0. Spike** | Broker plus pipe from the fast-scan design; raw read throughput on NVMe, SATA, USB stick and SD; decide the NTFS layer (`mft` + own volume layer vs `ntfs-reader`); devinfo probes (bus, TRIM, BitLocker); VHDX harness skeleton | 1 to 2 weeks | Deleted files listed from a test VHDX and a live USB stick; numbers recorded |
| **1. Alpha "Safe basics"** | Device list and outlook banner; Tier A (Recycle Bin all volumes, VSS list/browse/copy, Office/Notepad/VS Code autosave finder, cloud guidance); NTFS MFT undelete on volumes, shadow devices and raw images, with path rebuild, resident data and bitmap-based states; destination guard; copy-out with hashes; CSV/JSON report; EN/TR; portable build with the WebView2 data dir off the source | 5 to 6 weeks | Scenario suite green on NTFS VHDX; no write handle to the source in any test (see 7.5) |
| **2. Public beta = recommended MVP "Cards and sticks"** | FAT12/16/32 and exFAT undelete (LFN pairing, CP857, NoFatChain); carving-lite with validators for JPEG, PNG, HEIC, MP4/MOV, PDF, ZIP/OOXML (and CR2/NEF/ARW if time allows); EXIF/metadata naming; dedupe vs metadata finds; unallocated-only mode; imaging for removable media with mapfile and resume; previews (image, text, hex); fuzzing CI; docs, FAQ and how-it-works pages | 5 to 7 weeks | FAT32/exFAT VHDX suites green; DFRWS 2006 image scored against its published answer key; fuzzers run 1 h per target in CI without crashes |
| **3. v1.0** | Resumable deep-scan sessions; more formats (GIF, BMP, WebP, TIFF/DNG, CR3, MKV/WebM, AVI, MP3, WAV, 7z, RAR, SQLite, OLE CFB, RTF, optional text); JPEG bifragment gap carving; NTFS LZNT1 and WOF; USN and `$I30` evidence for names and deletion times; MFT scan of shadow devices for "deleted before snapshot"; read-only lost-partition scan; quick-format reconstruction (scattered `FILE` records, FAT directory-cluster carving); HTML report; accessibility pass; signed releases | 8 to 10 weeks | NIST CFTT-style assertions (7.4) pass on our corpus; comparison table against TSK and PhotoRec published in docs |
| **Later** | E01 input, VSS store parsing on offline images (Apache `vsc-core`), thumbnail-cache evidence, `$LogFile`, user-defined signatures, MCP or CLI JSON automation | n/a | n/a |

Totals:

- MVP (phases 0 to 2): about **11 to 15 engineer-weeks**.
- v1.0: about **19 to 25 engineer-weeks**.

---

## 7. Test strategy

### 7.1 Layers

1. **Unit tests on synthetic structures** built in memory: MFT records with fixups, attribute lists, runlists (including sparse and negative offsets), FAT chains, exFAT entry sets, `$I` v1 and v2. BitBroom Rescue uses this approach (verified README).
2. **Scenario tests on real Windows-made images** (VHDX via diskpart). This is the source of truth for Windows driver behaviour.
3. **Cross-platform image tests** on Linux CI: `mkntfs` (ntfs-3g), `mkfs.vfat -F 32`, `mkfs.exfat`. ntfs-3g's delete behaviour can differ from Windows, so use these for parser regression and fuzz seeds, not as truth (unverified difference).
4. **Oracles:**
   - TSK `fls`/`icat`/`tsk_recover` and PhotoRec run in CI against the same images (not distributed).
   - Compare listings and extracted bytes.
   - Differences are triaged, not auto-failed.
5. **Public corpora:**
   - DFRWS 2006 carving challenge (raw 50 MB image, answer key and hashes on GitHub; verified).
   - NIST CFReDS images (verified to exist; pick NTFS/FAT sets).
   - Digital Corpora disk images (secondary).
   - Woodblock (PyPI) generates carving test sets in the DFRWS/NIST style (secondary; check licence).
6. **Fuzzing:**
   - `cargo fuzz` targets per parser with `arbitrary`-built structures plus raw byte fuzzing, seeded from real images.
   - Run on Linux CI: cargo-fuzz officially targeted Linux and macOS, and Windows libFuzzer support is experimental (secondary, rust-lang issue #89339 and MSVC docs).
   - Parsers are platform-independent, so this is fine.
   - Add `proptest` round-trips (build a structure, serialize, parse, compare).

### 7.2 VHDX scenario harness (sketch, untested)

```powershell
# Run elevated. Creates a 512 MB NTFS VHDX, fills it, deletes some files, detaches it.
$img  = "$env:TEMP\rr\ntfs-basic.vhdx"
$dp   = "$env:TEMP\rr\mk.txt"
New-Item -ItemType Directory -Force (Split-Path $img) | Out-Null
@"
create vdisk file="$img" maximum=512 type=expandable
select vdisk file="$img"
attach vdisk
convert gpt
create partition primary
format fs=ntfs quick label=RRTEST
assign letter=R
"@ | Set-Content -Encoding ascii $dp
diskpart /s $dp
# populate: deterministic files + real JPEG/PNG/PDF/DOCX fixtures, write manifest of SHA-256
# delete: Remove-Item on a subset (bypasses Recycle Bin), optionally Explorer-style deletes
# fragment: fill with small files, delete every other one, write large files into the gaps
@"
select vdisk file="$img"
detach vdisk
"@ | Set-Content -Encoding ascii $dp
diskpart /s $dp
```

Then:

- Run the recovery CLI against the VHDX file, either parsed directly or attached read-only with `Mount-DiskImage -Access ReadOnly -NoDriveLetter`.
- Recover everything to a temp folder and compare against the manifest: expected states, exact hashes for intact files, and correct names and paths.
- Matrix: NTFS 4K clusters, NTFS 64K clusters, NTFS on a 4Kn-emulated disk (4096-byte records), FAT32, exFAT; resident files; compressed files; ADS; hard links; deep folders; Turkish names (`Şirket Belgeleri\Çağrı Özet.docx`); over 260-character paths; files deleted then partly overwritten (expect Partial); a quick-format scenario.
- diskpart needs admin (verified). Whether GitHub-hosted Windows runners allow VHDX attach is unverified; test early. Fallback: run the Windows scenario suite on a self-hosted runner or locally before releases, and keep Linux-built images in hosted CI.

### 7.3 Real hardware matrix (manual, before each release)

- SATA HDD (best case), SATA SSD with TRIM on, NVMe SSD with TRIM on, USB 3 stick (FAT32 and exFAT), SD and microSD in a USB reader and in a built-in reader, external SSD over UASP, a BitLocker-unlocked volume, a camera-formatted SD card.
- Record what each tool finds (ours, winfr, PhotoRec) for the same deletion script. Publish the table in docs. Honesty is a feature.

### 7.4 Assertion checklist (NIST CFTT style)

NIST published a Forensic File Carving tool specification and test assertions, and a draft Deleted File Recovery specification (verified, nist.gov documents).

Derive assertions, for example:

- Every reported deleted file whose clusters were not reallocated is recovered byte-exact.
- No allocated file is reported as deleted.
- Names and paths are correct when metadata exists.
- Partially overwritten files are flagged.
- No false "Verified" on corrupt data.
- The tool never modifies the source.

### 7.5 Safety tests

- Hash the source image before and after every scenario: it must be identical.
- On live tests, run with a Process Monitor or ETW trace and assert no write-access opens to the source volume or disk (unverified tooling detail).
- In code, a debug assertion in `blockio` that the handle was opened without write access (`GetFileInformationByHandleEx` cannot tell this directly; keep the access mask in the wrapper type).
- Destination-guard tests: same letter, different letter on the same disk, junction into the source, `subst` drive, mounted-folder volume, VHDX on the source disk, network path, removable source with the destination on the same card reader (different cards: allowed).

---

## 8. Turkish market and removable media

- **Typical cases (secondary, Turkish recovery guides; mostly vendor blogs):** phone photos and videos, SD cards from cameras, phones and dashcams, USB sticks (FAT32/exFAT), WhatsApp media, office documents. Most guides push paid tools; trustworthy free software in Turkish is scarce. Turkish "free download" pages for paid tools and cracked builds are common (secondary impression), which makes a signed, open-source, Turkish-language tool a real differentiator.
- **Phones:** be explicit that a PC tool cannot scan phone internal storage (MTP, encryption). Point users to Google Photos trash (60/30 days), Samsung Gallery and iOS Recently Deleted (about 30 days; unverified per vendor), WhatsApp backups (Google Drive or iCloud) and the phone's own bins. Only a **removable** SD card that was not adopted as internal storage can be scanned on a PC.
- **SD cards and USB sticks:**
  - FAT32 (up to 32 GB) and exFAT (SDXC) dominate.
  - The Recycle Bin is not used for removable drives.
  - Camera formats are quick formats, so carving works well.
  - Cards fail physically. Offer "image this card first" as the default for removable media, with a time estimate.
- **Encodings:**
  - FAT 8.3 names in CP857 on Turkish systems (verified code page).
  - Legacy `.txt` files in Windows-1254.
  - Search must use invariant case folding (ReClaim research already notes the dotted/dotless I pitfall).
- **Turkish formats:**
  - **UYAP `.udf`** (the judiciary's document editor format) is reportedly a ZIP container with XML content (unverified; confirm with a sample). If so, add it to the ZIP carver by inner file names.
  - e-Fatura/e-Arşiv invoices are XML (UBL-TR) and PDF; XML carving is low value.
  - Mostly the work is the standard Office, PDF, JPEG and MP4 set.
- **Counterfeit-capacity USB sticks** wrap writes around and destroy earlier data (secondary, existence of tester tools). If the user's stick reports an implausible capacity, explain why recovery can be impossible.
- **KVKK (Turkish data protection law):** the app is local-only with no telemetry, which keeps it out of data-processing questions for the user's own data. Add a short note that recovering data from someone else's device requires their permission (unverified legal framing; not legal advice).
- **Local labs:** for physical damage, say "contact a data recovery lab" without naming companies.

---

## 9. Naming and repository

### 9.1 Name

- **Risk:** "ReclaiMe File Recovery" and "ReclaiMe Pro" are commercial recovery products sold since about 2009-2010 (secondary: Forensic Focus, reviews). "ReClaim Recover" or "ReClaim Recovery" in the same software class is likely to be confused with them. Registration status was not found in this research (unverified). Check TÜRKPATENT, EUIPO, USPTO and WIPO Global Brand Database before committing.
- **Family names** (keep the brand, lower the overlap with "ReclaiMe File Recovery"; still need the trademark check):
  - "ReClaim Rescue" (note BitBroom also uses "Rescue").
  - "ReClaim Undelete".
  - "ReClaim Restore".
- **Independent names**, if the check fails (availability on GitHub, crates.io and domains unverified):
  - "Kurtar" (Turkish for "rescue"; short, distinctive in Turkey, but awkward for English speakers).
  - "Undrop".
  - "Retrace".
  - "Salvo".
- **Recommendation:** decide the name after the trademark search. Prefer a family name only if it is clear of ReclaiMe.

### 9.2 Same repository or separate?

**Recommendation: a separate repository**, plus a small shared crate.

| Factor | Same repo (ReClaim Cleaner) | Separate repo |
|---|---|---|
| Risk profile | Cleaner deletes files; recovery reads raw disks as admin. Mixing them confuses the safety story ("the cleaner that also undeletes") | Clear, auditable promise: this program never writes to the source |
| Release cadence | Recovery needs frequent parser fixes and long pre-release hardware testing; the cleaner would be blocked or dragged along | Independent versions and changelogs |
| Admin and signing | Different elevation surface; one broker for two products complicates review | Own broker, own signing and SmartScreen reputation, own SECURITY.md |
| CI | Admin VHDX tests, fuzzing and corpora downloads would slow cleaner CI | Dedicated CI with heavier jobs |
| Users and support | Issues mix "cleaner removed X" with "cannot recover Y" | Separate trackers and FAQ |
| Licence | Both MIT; no difference | Same |
| Code sharing | Trivial inside one workspace | Needs a shared crate (below) |

Sharing plan:

- **`reclaim-ntfs` crate** (MIT): the volume layer, MFT streaming, record parsing (on `mft` or own), runlists and path rebuild.
  - The cleaner uses in-use records and allocated sizes for its P2 fast scan (ROADMAP: "Fast NTFS scan by reading the MFT"; `research2-fastscan.md` recommends a small in-house reader).
  - The recovery app adds deleted-record and bitmap features behind a cargo feature.
  - Host it in the recovery repo (it is the heavier user) and **publish to crates.io**. The cleaner then pins it with `=x.y.z`, matching its exact-pin policy and keeping `cargo deny` and licence tooling simple.
  - A git dependency pinned to `rev` also works but is less convenient for auditing and for contributors.
- **Helper/broker pipe code:** a second small shared crate (`reclaim-elevate`) once both apps use it. Until then, copy and keep in sync. It is short.
- **UI:** share the design tokens and components by copying the theme files, or later a tiny private package. Do not couple the frontends through a monorepo for this alone.
- **Brand docs:** cross-link the two apps (the cleaner's docs page links "deleted something by mistake?" to the recovery app, and the reverse).

---

## 10. Open questions and unverified items to test first

1. Whether reading `\\.\X:` of an unlocked BitLocker volume always returns plaintext (practitioner consensus).
2. Exact `$I` v2 offsets (community-documented; check with samples from Windows 10 and 11).
3. Whether GitHub-hosted Windows runners can attach VHDX via diskpart.
4. How fast real SSDs in the test matrix return zeros after deletion (measure; do not quote vendor claims).
5. Whether TRIM is sent over the built-in SD reader and common USB readers on Windows 11 (measure with a deletion test).
6. Tauri 2.12 API name for a per-window WebView2 data directory, and whether the WebView2 runtime itself writes elsewhere on C:.
7. Whether VSS `@GMT` paths give non-admin access to Previous Versions locally (would allow Tier A without UAC).
8. UYAP `.udf` container format.
9. FAT32 high-word clearing on current Windows 11 (test by deleting on Windows 11 and reading the entry).
10. Trademark status of "ReclaiMe" and the chosen name.
11. Licences of KapeFiles target data, ogmini's Notepad research code, `cfb` and `xpress-huffman` crates before any reuse.

---

## 11. Sources

All accessed 2026-10-16. Summaries are in my own words.

Microsoft and platform docs

- Windows File Recovery: https://support.microsoft.com/en-us/windows/experience/backup-recovery/windows-file-recovery
- CreateFileW, Physical Disks and Volumes: https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew
- FILE_RECORD_SEGMENT_HEADER: https://learn.microsoft.com/en-us/windows/win32/devnotes/file-record-segment-header
- fsutil behavior (DisableDeleteNotify): https://learn.microsoft.com/windows-server/administration/windows-commands/fsutil-behavior
- Format writes zeros since Vista (KB941961): https://learn.microsoft.com/en-us/troubleshoot/windows-server/backup-and-storage/format-command-not-write-zeros-to-disk
- NTFS and 4K disks (record size): https://learn.microsoft.com/de-de/archive/blogs/ntdebugging/ntfs-and-4k-disks
- WebView2 security (non-elevated host): https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/security
- Win32_ShadowCopy: https://msdn.microsoft.com/en-us/library/aa394428
- diskpart: https://learn.microsoft.com/windows-server/administration/windows-commands/diskpart
- MSVC /fsanitize (libFuzzer experimental): https://learn.microsoft.com/en-us/cpp/build/reference/fsanitize
- Office unsaved files (archived Microsoft blog): https://learn.microsoft.com/en-us/archive/blogs/mssmallbiz/how-to-recover-that-un-saved-microsoft-office-excel-word-or-powerpoint-file-you-closed-before-saving
- OneDrive restore: https://support.microsoft.com/en-us/office/restore-deleted-files-or-folders-in-onedrive-949ada80-0026-4db3-a953-c99083e6a84f
- OneDrive cloud deletions no longer in local bin (University of Waterloo, 2026-08-07): https://uwaterloo.ca/microsoft-365/blog/files-deleted-onedrive-cloud-will-no-longer-appear-local
- Google Drive trash: https://support.google.com/drive/answer/2375102
- Google Photos trash: https://support.google.com/photos/answer/6128858
- Android adoptable storage: https://source.android.com/docs/core/storage/adoptable
- tauri-build (WindowsAttributes::app_manifest): https://docs.rs/tauri-build

Tools and projects

- TestDisk and PhotoRec: https://github.com/cgsecurity/testdisk, https://www.cgsecurity.org/wiki/PhotoRec_Step_By_Step, https://www.cgsecurity.org/wiki/PhotoRec_Data_Carving, https://en.wikipedia.org/wiki/PhotoRec
- The Sleuth Kit: https://github.com/sleuthkit/sleuthkit (licenses/README.md), https://wiki.sleuthkit.org/tsk_recover ; Autopsy: https://github.com/sleuthkit/autopsy
- Scalpel: https://github.com/sleuthkit/scalpel
- Foremost licence (Debian copyright): https://metadata.ftp-master.debian.org/changelogs/main/f/foremost/stable_copyright
- bulk_extractor licence: https://github.com/simsong/bulk_extractor/blob/main/LICENSE.md
- ntfsundelete man page: https://man.archlinux.org/man/ntfsundelete.8.en ; ntfs-3g: https://github.com/tuxera/ntfs-3g
- RecuperaBit: https://github.com/Lazza/RecuperaBit ; thesis: https://unitesi.unive.it/handle/20.500.14247/19884
- GNU ddrescue manual: https://www.gnu.org/software/ddrescue/manual/ddrescue_manual.html
- analyzeMFT: https://github.com/rowingdude/analyzeMFT ; MFTECmd: https://github.com/EricZimmerman/MFTECmd
- NTFSTool: https://github.com/thewhiteninja/ntfstool
- KAPE licence: https://www.kroll.com/en/kape-license-agreement
- DMDE EULA: https://dmde.com/licensenew/eula.txt
- R-Studio: https://www.r-tt.com/RStudio.shtml
- Disk Drill: https://en.wikipedia.org/wiki/Disk_Drill and CleverFiles help pages
- Active@ File Recovery: https://www.file-recovery.com/features.htm
- Recuva states (secondary): https://www.computersecuritystudent.com/FORENSICS/File_Recovery/RECUVA/lesson3/index.html
- ReclaiMe: https://www.forensicfocus.com/news/reclaime-pro-data-recovery-toolkit-for-experts/ , https://www.reclaime.com/library/refs-recovery.aspx

New open-source projects

- BitBroom Rescue: https://github.com/pwnapplehat/BitBroom.Rescue
- Rescyo: https://github.com/akk0sfx/Rescyo
- KVC FileRecoveryTool: https://github.com/wesmar/FileRecoveryTool
- unearth: https://github.com/MarcelRoozekrans/unearth
- NullVeil: https://github.com/bhadri01/NullVeil
- recoverer: https://github.com/ibrahimokdadov/recoverer
- Data-Recovery-CLI: https://github.com/flerkenstudio/Data-Recovery-CLI
- winfr-pro: https://github.com/kil0bit-kb/winfr-pro
- ntfs-undelete: https://github.com/NikolaMilosa/ntfs-undelete
- Sootmark collector and crates: https://github.com/Sootmark/collector

Rust crates

- ntfs: https://github.com/ColinFinck/ntfs ; blog: https://colinfinck.de/posts/an-implementation-of-the-ntfs-filesystem-in-a-rust-crate/
- mft: https://github.com/omerbenamram/mft
- ntfs-reader: https://github.com/kikijiki/ntfs-reader (README, CHANGELOG 0.6.0)
- ntfs-core / ntfs-forensic: https://docs.rs/ntfs-core , https://github.com/SecurityRonin/ntfs-forensic
- Versions, licences and download counts: crates.io API (`/api/v1/crates/<name>`), queried 2026-10-16

Research and forensics

- Garfinkel 2007, carving with object validation: https://dfrws.org/presentation/carving-contiguous-and-fragmented-files-with-object-validation/ ; PDF: https://apps.dtic.mil/sti/pdfs/ADA576165.pdf
- Pal, Sencar, Memon 2008: https://dfrws.org/presentation/detecting-file-fragmentation-point-using-sequential-hypothesis-testing/ ; patent US8407192: https://patents.google.com/patent/US8407192
- Heavily fragmented JPEG recovery (DFRWS 2016): https://dfrws.org/wp-content/uploads/2019/06/2016_USA_paper_recovery_of_heavily_fragmented_jpeg_files.pdf
- File carving overview: https://en.wikipedia.org/wiki/File_carving
- DFRWS 2006 challenge: https://github.com/dfrws/dfrws2006-challenge
- NIST CFTT forensic file carving spec: https://www.nist.gov/document/cftt-ffc-forensic-file-carving-tool-specification-version-10 ; deleted file recovery draft: https://www.nist.gov/document/cftt-dfr-active-file-identification-deleted-file-recovery-tool-specification-draft-1
- ReFS forensic analysis (DFRWS EU 2020): https://dfrws.org/wp-content/uploads/2020/05/Forensic-Analysis-of-the-Resilient-File_2020_Forensic-Science-International-.pdf
- exFAT artefacts (Vandermeer et al.): https://researchrepository.ucd.ie/handle/10197/10148
- FAT32 high word discussion: https://www.forensicfocus.com/forums/general/deleted-fat-files-first-cluster-addr-high-word-gets-cleared/
- TRIM, DRAT, DZAT (ElcomSoft): https://blog.elcomsoft.com/2025/06/what-trim-drat-and-dzat-really-mean-for-ssd-forensics/
- TRIM over USB (Plugable KB): https://kb.plugable.com/data-storage/trim-an-ssd-in-windows-10
- Recycle Bin bypass cases: https://www.computerhope.com/issues/ch001009.htm
- Notepad TabState: https://securelist.com/forensic-artifacts-in-windows-11/117680/ , https://docs.velociraptor.app/exchange/artifacts/pages/tabstate
- USN journal default size (secondary): https://www.voidtools.com/forum/viewtopic.php?p=38813
- CP857: https://en.wikipedia.org/wiki/Code_page_857
- cargo-fuzz on Windows: https://github.com/rust-lang/rust/issues/89339
