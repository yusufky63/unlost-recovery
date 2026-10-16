# Security policy

Unlost reads raw disks through an elevated helper and parses data that may be hostile (a crafted USB stick, a damaged disk image), so security reports get priority.

## Supported versions

Only the latest release receives fixes.

## Reporting a vulnerability

Please do not open a public issue. Use GitHub's private reporting: **Security > Report a vulnerability** on this repository. Include the version, Windows build, steps to reproduce and what an attacker gains. If a disk image is needed to reproduce, make a minimal synthetic one; do not send images with private data.

You can expect a first answer within a week. Fixed issues are credited in the release notes unless you ask otherwise.

## What counts

In scope, for example:

- Any write to a source drive, volume, disk image or shadow copy, or any way to make Unlost create a shadow copy.
- Recovered files, reports or app data reaching the source's physical disk (destination guard bypasses through junctions, mounted folders, `subst`, virtual disks, network paths or anything else).
- A crafted volume, image, `$I` file or partition table that crashes the parser, hangs it, makes it allocate without bound, or runs code in the elevated helper.
- Ways to talk to the helper from another process or another user, to impersonate the UI or the helper on the named pipes, or to make the helper read something other than what the UI asked for.
- Other users' files becoming visible to a standard user.
- Any network traffic.

Recovering files the user deliberately selected, or the honest limits documented in the README (TRIM, encryption, MTP phones), are not vulnerabilities.
