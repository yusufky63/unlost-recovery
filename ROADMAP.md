# Roadmap

Phases follow [docs/research.md](docs/research.md#6-phased-plan-with-estimates). Estimates there are unverified.

## Done (first milestone)

- Read-only device layer, destination guard, elevated broker over named pipes.
- First screen with per-drive outlook; Recycle Bin recovery; shadow-copy browsing and copy-out.
- NTFS deleted-file scan with rebuilt paths and evidence-based states; recovery with hashes and reports.
- VHDX scenario fixtures, elevated tests, fuzz targets.

## Next: Phase 1 completion

- Office, Notepad and VS Code autosave finder; cloud client detection with the right recycle-bin link.
- MFT scan of shadow devices to find files deleted before the snapshot, with the snapshot's copy marked intact.
- USN journal evidence for names and deletion times.
- Session store (SQLite in the workspace) so scans of busy system drives are not capped at one million files.
- HTML report.

## Phase 2: cards and sticks

- FAT12/16/32 and exFAT undelete (LFN pairing, CP857 short names, `NoFatChain`).
- Signature carving with validators (JPEG, PNG, HEIC, MP4/MOV, PDF, ZIP/OOXML); only fully decoding files are "Verified".
- Imaging for removable and failing media with a ddrescue-compatible map file and resume.
- Previews (image thumbnails decoded in Rust, text with Windows-1254 support, hex).
- How-it-works and FAQ pages inside the app.

## Phase 3: v1.0

- NTFS LZNT1 and CompactOS (WOF) decompression.
- `$I30` index slack for names of reused records; quick-format reconstruction.
- JPEG bifragment gap carving; more formats.
- Read-only lost-partition scan.
- Signed releases.

## Not planned

- A background "protection" service, secure-delete or wipe features, partition or file-system repair, in-place restore other than through Explorer.
