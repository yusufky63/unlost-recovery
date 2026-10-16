# Unlost: project brief

Paste this into a new Claude Code session opened in `C:\Projeler\unlost` (or let the session that was started for it read this file).

---

Build **Unlost**, an open-source (MIT) deleted-file recovery app for Windows 10 and 11, as a sibling of ReClaim Cleaner (`C:\Projeler\pc-cleaner`, github.com/yusufky63/reclaim-cleaner). The research behind it is in `docs/research.md`: landscape, techniques, honest limits, architecture, phased plan and test strategy. Read it first.

## Stack and conventions (copy from ReClaim)

- Tauri 2 + Rust engine crate (`crates/core`, no Tauri dependency, unit tested) + React 19, TypeScript, Tailwind CSS 4, i18next (English and Turkish, Turkish typed against English), zustand, lucide-react.
- Reuse ReClaim's design tokens and primitives (`src/styles/globals.css`, `src/components/ui.tsx`, the themed `Select`, house style: DM Sans / Space Grotesk / JetBrains Mono, one brand colour, no gradients, no emoji). Pick a distinct brand colour so the two apps are told apart.
- Same packaging as ReClaim: NSIS setup (per user), MSI (en-US, tr-TR), portable zip with a `portable` marker, GitHub Actions CI (typecheck, lint `--max-warnings=0`, Vitest, build, `cargo fmt --check`, `clippy -D warnings`, `cargo test`) and a tag-triggered release workflow, MIT LICENSE, README (EN + TR), CONTRIBUTING, SECURITY, issue forms, dependabot.
- Elevation like ReClaim: the WebView process stays a normal user; raw-disk work runs in an elevated broker (the same exe in helper mode through UAC) that streams results to the UI over a named pipe locked to the user (see docs/research.md, section 5).

## Non-negotiable safety rules

1. Open the source read-only by construction: `GENERIC_READ` with both share flags; the I/O layer has no write method. Never lock, dismount, repair or rewrite anything.
2. Never write to the source physical disk (output, logs, session files, WebView2 data). Compare physical disk numbers, not drive letters, and refuse instead of warning.
3. Never create a new shadow copy of the source volume; use existing ones only.
4. Offer the intact sources first: Recycle Bin, existing shadow copies (Previous Versions), File History, app autosaves, cloud recycle bins.
5. Explain the risks before scanning: SSD with TRIM, system drive, BitLocker locked, cloud placeholders, EFS, full format, ReFS.
6. Every file gets a recoverability state with an evidence-based reason. Never "100%". "Verified" only when a validator decodes the whole file. Carved files never get invented names.
7. Image first for removable or failing media; read bad sectors once.
8. Parsers use no `unsafe`, have bounded allocations and are fuzzed.
9. No telemetry, no paywall, no network calls during recovery; never auto-open recovered files.
10. Every recovery writes a report with method, state, reason and hashes; never overwrite existing files at the destination.

Do not copy code from GPL/AGPL/LGPL projects (TestDisk/PhotoRec, vshadow, exhume_*, ms-compress, post-2015 bulk_extractor, The Sleuth Kit core). Permissive crates listed in docs/research.md (`mft`, `ntfs-reader`, `zune-jpeg`, `png`, `zip`, `lopdf`, `mp4`, `matroska`, `lznt1`) are fine.

## First milestone (Phase 0 + start of Phase 1)

1. Scaffold the repo (workspace, Tauri app, engine crate, CI, docs, LICENSE) and `docs/architecture.md`.
2. Read-only volume and physical-disk access layer, destination guard (refuses the same physical disk), and tests.
3. "Stop writing to this drive" first screen and a per-drive outlook: HDD/SSD/removable, TRIM state, system drive, shadow copy count.
4. Easy wins: Recycle Bin contents on all volumes (from `$I`/`$R` files) with restore to another location, and browsing existing shadow copies read-only.
5. NTFS: list deleted MFT records with rebuilt paths, size, timestamps and a first recoverability state (clusters free or reused according to `$Bitmap`); recover resident and non-resident files to another volume with hashes.
6. VHDX-based test fixtures created by a script (create files, delete them, recover, compare hashes).

Done means: the app builds and runs, every check passes, screenshots were checked in light and dark theme, everything is committed locally. Ask before creating the GitHub repository or pushing.
