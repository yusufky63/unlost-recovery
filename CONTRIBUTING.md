# Contributing

Thanks for helping. Unlost reads raw disks as administrator on computers where someone has just lost something, so the bar for changes to the engine is high, and the bar for everything else is friendly.

## Ground rules

- **Never write to the source.** No change may open a source with write access, add a write method to `SectorSource`, lock or dismount a volume, create a shadow copy, or write anything (output, logs, caches) to the disk being recovered. The ten safety rules in the [README](README.md#safety-rules) are not negotiable.
- **Honest states.** A file is only "Likely intact" with evidence, and only a format validator that decodes the whole file may mark it "Verified". Never show percentages of confidence.
- **Parsers stay safe.** `crates/formats` forbids `unsafe`. Every offset is bounds-checked and every allocation capped. New parsers come with a synthetic-input test, a mutation test and a fuzz entry point in `crates/formats/src/fuzzing.rs`.
- **No code from GPL, LGPL or AGPL projects** (TestDisk, PhotoRec, ntfs-3g, ddrescue, RecuperaBit, Rescyo, The Sleuth Kit core, libyal, `vshadow`, `exhume_*`). Reading their documentation and papers for ideas is fine; write your own code. See [docs/research.md](docs/research.md#24-licence-rules-for-an-mit-project).
- **No telemetry, network calls, paywalls or bundled offers.**
- **English for code, comments, UI strings and commits.** Every UI string also goes into `src/i18n/tr.ts`; the typecheck fails if a key is missing.

## Setup

Windows 10 or 11 with Rust (stable, MSVC), Node.js 22+, pnpm 9 and the Visual Studio C++ build tools.

```bash
pnpm install
pnpm app:dev
```

## Before you open a pull request

All of these must pass; CI runs the same set.

```bash
pnpm typecheck
pnpm lint
pnpm test
pnpm build
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- Parser or recovery changes: add a case to the synthetic NTFS fixture (`crates/formats/src/ntfs/tests.rs`) and, if Windows behaviour matters, to `scripts/make-fixtures.ps1`. See [docs/testing.md](docs/testing.md).
- Code that needs administrator rights: add a test to `crates/core/tests/elevated.rs` and run it from an elevated prompt.
- UI changes: check light and dark theme and the minimum window width (960 px). Attach screenshots made from the synthetic demo volume, never from a real user's files.
- User-visible changes: add a line to `CHANGELOG.md` under "Unreleased" and update the FAQ when behaviour changes.

## Reporting bugs

Use the issue form. Never attach recovered files or disk images that hold private data; the state, reason and report line usually say enough.
