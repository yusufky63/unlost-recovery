# Testing

## The check matrix

Every change passes these, locally and in CI:

```bash
pnpm typecheck
pnpm lint
pnpm test
pnpm build
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Layers

1. **Synthetic NTFS volumes** (`crates/formats/src/ntfs/build.rs`, `tests.rs`). An image builder writes a boot sector, an `$MFT` with real update-sequence fixups, `$Volume`, the root, `$Bitmap`, and live and deleted folders and files. One fixture covers every state: resident, contiguous and sparse files, a file in a deleted folder, a lost folder, partly and fully reallocated clusters, zeroed clusters (TRIM), a header mismatch, EFS, NTFS compression, CompactOS, a cloud placeholder, a torn record, a record without runs, an attribute list with the data in a deleted extension record, alternate data streams, Turkish names. It runs with 4 KiB clusters, with records larger than clusters (512-byte clusters), with 4 KiB records, and through VHDX and GPT layers. Every recovered byte is compared by SHA-256, and the source is hashed before and after to prove nothing changed.
2. **Mutation tests** (`survives_random_damage`, `crates/formats/src/fuzzing.rs`). Deterministic random damage to records, boot sectors, bitmaps, runlists, `$I` files, VHDX headers and partition tables. They found a real problem during development: a damaged record 0 could claim billions of records and a damaged boot sector a volume larger than the device; both are now bounded.
3. **Fuzzing** (`fuzz/`). cargo-fuzz targets for the volume scan, file records, runlists, `$I` files, VHDX images and partition tables. They call the functions in `unlost_formats::fuzzing`, which the mutation tests also exercise. cargo-fuzz needs a nightly toolchain and runs on Linux (Windows support in libFuzzer is experimental):

   ```bash
   cargo +nightly fuzz run ntfs_volume -- -max_total_time=600
   ```

   CI runs each target for a short time on every push.
4. **Windows-made VHDX scenarios** (`scripts/make-fixtures.ps1`, `crates/core/tests/vhdx_scenarios.rs`). See below.
5. **Elevated tests** (`crates/core/tests/elevated.rs`): raw reads of the system volume (including its last sectors), shadow copy listing, browsing and reading, and a full scan of the newest shadow copy (or of `UNLOST_SCAN_DEVICE`). Ignored by default; run from an elevated prompt:

   ```bash
   cargo test -p unlost-core --test elevated -- --ignored --nocapture
   ```

6. **Windows integration tests without elevation**: drive facts, physical disk numbers, the guard refusing a temp folder on the system disk, pipe security (first-instance only, foreign clients rejected), the broker refusing bad arguments and other users, WMI, the user's real Recycle Bin.
7. **Frontend** (`src/**/*.test.ts`): translations (same keys and variables in English and Turkish, no emoji), the drive outlook rules, state tones and reason parameters, formatting.

## VHDX scenarios

Run once from an elevated PowerShell (diskpart needs administrator rights):

```bash
pnpm fixtures
```

The script creates `fixtures\ntfs-basic.vhdx` and `fixtures\ntfs-overwritten.vhdx` with Windows' own NTFS driver, writes deterministic files (a resident note, a PDF, a 2 MB JPEG, a Turkish-named document, a folder tree that is deleted as a whole, a 10 MB file, a path over 260 characters, a file written into the gaps of deleted files so it is fragmented), deletes them, detaches the disk and writes a manifest with sizes and SHA-256 hashes. The second image deletes a 5 MB file and then fills the volume.

`cargo test -p unlost-core --test vhdx_scenarios` then checks:

- live files are never reported as deleted;
- resident files are "Likely intact" and recover byte for byte;
- **every file graded "Likely intact" recovers byte for byte** (the honesty rule);
- files may instead be "Zeroed" or "Partial" only when Windows was allowed to TRIM inside the virtual disk;
- the reallocated file is "Overwritten" or "Partial", never intact;
- the image file is unchanged afterwards.

Without fixtures the test passes with a note; CI sets `UNLOST_REQUIRE_FIXTURES=1` and builds them first with `-DisableTrimWhileBuilding`, which turns NTFS delete notifications off on the throwaway runner while the images are built so the expectations are exact. Whether GitHub-hosted runners allow VHDX attach was not verified before the first push.

## Manual checks on real hardware

Before a release, record what Unlost finds on: a SATA HDD, an NVMe SSD with TRIM, a USB stick (NTFS and FAT32), an SD card in a USB reader, a BitLocker-unlocked volume, and a phone (expected: shown as a media device, not scannable). The development machine so far: a TRIM SSD system volume (via its shadow copy), a second SSD, and an NTFS USB stick (10,748 deleted files listed, a sample recovered intact through the broker).

## Screenshots

```bash
cargo run -p unlost-core --example demo_image -- target/demo.img
```

Then start the app with a DevTools port and run the capture script (see the header of `scripts/screenshots.mjs`). The demo volume is synthetic (label `UNLOSTTEST`), so screenshots show no real user's files.
