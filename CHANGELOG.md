# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- First screen: what not to do, then an outlook for every drive (hard disk, SSD with its TRIM state, USB stick, SD card, Windows drive, BitLocker-locked, ReFS, FAT), Recycle Bin and shadow-copy counts, and the safe order of next steps. Phones connected as media devices are named with an explanation of why they cannot be scanned.
- Recycle Bin recovery on every drive from `$I`/`$R` files, copied to another disk; items whose data is gone are shown as such.
- Previous versions: existing shadow copies listed and browsed read-only, files and folders copied out.
- Deleted files on NTFS volumes and disk images (`.vhdx`, `.img`, `.dd`, `.raw`): `$MFT` scan, rebuilt paths with deleted and lost folders, per-file state and reason from the cluster bitmap and sampled content, search, filters by state and type, sorting and paging.
- Recovery to a new folder on a different physical disk, with SHA-256 hashing while writing and after, `PARTIAL_`, `ZEROED_` and `OVERWRITTEN_` prefixes, and CSV and JSON reports.
- Scan results stream into a SQLite session database in a folder on another physical disk (the app's data folder, or one the user picks when that shares a disk with the source), so scans of busy system drives list every deleted file in bounded memory. The broker keeps nothing per file; read requests bring back the content location it streamed, authenticated with a per-scan key.
- Destination guard by physical disk number, following junctions, mounted folders, `subst` and virtual disks.
- Elevated broker: the same exe through UAC, connected over user-only named pipes, never writing files.
- English and Turkish, light and dark theme, portable mode.
- Tests: synthetic NTFS volumes, mutation tests, Windows-made VHDX scenarios with SHA-256 manifests, elevated tests, cargo-fuzz targets.
