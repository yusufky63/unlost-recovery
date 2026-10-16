# Unlost

Open-source deleted-file recovery for Windows 10 and 11. It reads your drives without ever writing to them, offers the intact copies Windows already keeps before any scan, and tells you for every file what was actually checked. Nothing is ever called "100%".

[Türkçe](README.tr.md) · [How it works](docs/architecture.md) · [FAQ](docs/faq.md) · [Research behind it](docs/research.md)

![First screen in light mode](docs/screenshots/start-light.png)

Unlost is a sibling of [ReClaim Cleaner](https://github.com/yusufky63/reclaim-cleaner). It is a separate app on purpose: the cleaner deletes files as a normal user; Unlost reads raw disks as administrator and must never write to the disk it recovers from.

## What it does

- **Stop writing first.** The first screen says what not to do, then rates each drive: hard disk, SSD (with its TRIM state), USB stick or SD card, Windows drive, BitLocker-locked, ReFS or FAT. The outlook is one sentence with the facts behind it.
- **Intact copies before scans.** In order: your Recycle Bin on every drive (read from the `$I`/`$R` files, copied elsewhere, never restored in place), existing shadow copies ("Previous versions", browsed read-only), and guidance for cloud recycle bins, File History, Office autosaves and phones.
- **Deleted files on NTFS.** Reads the `$MFT` for records Windows marked as deleted, rebuilds folder paths (deleted folders included, broken chains go under "Lost folder #n"), and grades each file against the cluster bitmap and sampled content.
- **Honest states.** Intact copy, Likely intact, Partial, Zeroed (TRIM), Overwritten, Name only, Not recoverable here. Each comes with its reason, for example "38 of 120 clusters now belong to other files." "Verified" is reserved for files a decoder has read end to end; this version has no decoders yet, so it never shows it.
- **Recovery that proves itself.** Files go to a new `Unlost <date>` folder on a different physical disk, are hashed (SHA-256) while written and hashed again from disk, and a CSV and JSON report lists method, state, reason and hash for every file. Damaged files are named `PARTIAL_`, `ZEROED_` or `OVERWRITTEN_`. Nothing existing is overwritten, and recovered files are never opened for you.
- **Disk images too.** Scan a `.vhdx`, `.img`, `.dd` or `.raw` image (whole disk or single volume) without administrator rights.
- English and Turkish, light and dark theme.

| Deleted files | Other places |
|---|---|
| ![Deleted files](docs/screenshots/deleted-dark.png) | ![Other places](docs/screenshots/elsewhere-dark.png) |

## Safety rules

These are enforced in the engine, not only in the UI.

1. **Read-only by construction.** Drives, volumes and shadow copies are opened with `GENERIC_READ` and both share flags. The I/O trait has no write method. Nothing is ever locked, dismounted, repaired or rewritten.
2. **Never the source disk.** Recovered files, reports and the app's own data never go to the physical disk you recover from. Disk numbers are compared, not drive letters, and virtual disks count the disk holding their image file. The same disk is refused, not warned about.
3. **No new shadow copies.** Creating one would write to the source volume. Only existing ones are read.
4. **Intact copies first:** Recycle Bin, shadow copies, cloud bins, autosaves.
5. **Risks before scanning:** SSD with TRIM, Windows drive, BitLocker, cloud-only files, encryption, ReFS and FAT are explained up front.
6. **A state and a reason for every file.** Never "100%"; carved files will never get invented names.
7. **Image first** for removable or failing media; bad sectors are read once on their own, then filled and counted. (Imaging itself is planned; see the roadmap.)
8. **Hardened parsers.** The parser crate forbids `unsafe`, bounds every allocation, and is fuzzed.
9. **No telemetry, no paywall, no network.** Help links open in your browser only when you click them.
10. **A report for every recovery**, with hashes, and existing files are never overwritten.

## How administrator access works

The window you see runs as you, never elevated. When a step needs administrator rights (reading a drive directly, listing shadow copies), Unlost asks Windows once and starts the same program as a separate helper. The helper talks to the window over a named pipe that only your account can open, checks that the window is really Unlost, and sends data back. It never writes files: the window writes them, to the folder the guard approved. If a different administrator account approves the prompt, the helper refuses to serve. Details in [docs/architecture.md](docs/architecture.md#processes).

## What it cannot do (yet)

- FAT, exFAT and ReFS deleted-file scans, signature carving (photos and videos without metadata), disk imaging, previews, NTFS-compressed and CompactOS files. See [ROADMAP.md](ROADMAP.md).
- Phones connected over USB appear as media devices, not drives: Windows exposes their files, not their storage, so no PC tool can scan them. Unlost says so when one is plugged in.
- After TRIM on an SSD, a full format, `cipher /w` or a secure erase, the data is gone. No software can bring it back.

## Download

No release yet. Releases will ship as an NSIS setup (per user), MSI packages (English and Turkish) and a portable zip, with SHA-256 checksums. The portable build is the recommended way to run Unlost: started from a USB stick, its own files stay off the disk you are recovering from.

## Build from source

Requirements: Windows 10 or 11, [Rust](https://rustup.rs) (stable, MSVC), Node.js 22 or newer, pnpm 9, and the Visual Studio C++ build tools.

```bash
pnpm install
pnpm app:dev
```

Installers and the portable zip:

```bash
pnpm app:build
```

```bash
pnpm portable
```

## Tests

```bash
pnpm check
```

```bash
cargo test --workspace
```

The NTFS parser is tested on synthetic volumes built in memory, through VHDX and GPT layers, and on Windows-made VHDX images. `scripts/make-fixtures.ps1` (run elevated) builds those images with Windows' own NTFS, deletes files in them and writes SHA-256 manifests; `crates/core/tests/vhdx_scenarios.rs` checks that every file graded "likely intact" recovers byte for byte. Tests that need administrator rights (raw volumes, shadow copies, a full scan of a real volume) are in `crates/core/tests/elevated.rs`. Parsers have cargo-fuzz targets in `fuzz/`. See [docs/testing.md](docs/testing.md).

## Privacy

Unlost makes no network connections and collects nothing. Only your own Recycle Bin is listed. Recovery reports stay in the recovery folder you chose.

## License

[MIT](LICENSE). No code from GPL, LGPL or AGPL recovery tools is used; see [docs/research.md](docs/research.md#24-licence-rules-for-an-mit-project).
