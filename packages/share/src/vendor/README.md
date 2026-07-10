# Vendored JavaScript — provenance & integrity

This directory holds third-party JavaScript that is `include_str!`'d into every
generated share viewer at compile time (so the viewer is self-contained and the
build needs no `node_modules`). Because these bytes ship inside every viewer we
produce, each file is pinned by SHA-256 and enforced by a Rust test.

## dicomParser.min.js

- **Library:** dicom-parser (browser UMD build, exposes global `dicomParser`)
- **Version:** 1.8.12 — **frozen**
- **Upstream:** https://github.com/cornerstonejs/dicomParser
- **File:** `dist/dicomParser.min.js` from the above release
- **SHA-256:** `2b990e92de021a9c0d58f7dca693c95fa76be6398648b68441df9423de284a2b`

### Integrity enforcement

`../share.rs` inlines this file via `include_str!("vendor/dicomParser.min.js")`.
The test `vendored_dicom_parser_matches_known_good_sha256` (same file,
`#[cfg(test)]`) `include_bytes!`'s it, computes the SHA-256, and asserts it
equals the constant above. Any tamper or accidental drift — even one byte —
fails CI.

### Updating

This file is deliberately frozen at 1.8.12. To change it:

1. Replace the file with the new upstream build.
2. Recompute the SHA-256 (`shasum -a 256 dicomParser.min.js`).
3. Update the constant in `../share.rs` **and** the SHA-256 above.
4. Bump the version noted here.

Track dicom-parser for CVEs; a security advisory is the main reason to unfreeze.
