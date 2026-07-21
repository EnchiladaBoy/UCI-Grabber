# Release and compliance gates

UCI Grabber releases contain the Apache-2.0 application, compatible Rust
dependency notices, and one small UCI Grabber launcher per supported platform.
They do not contain Maia3, CPython, PyTorch, NumPy, python-chess, or Maia
checkpoint bytes. A user-selected install retrieves those exact bytes directly
from immutable upstream URLs and assembles the portable engine locally.

Every stable tag builds portable application and launcher archives for Windows
x86-64, macOS ARM64, Linux x86-64, and Linux ARM64. Windows carries the
no-console `UCI-Grabber.exe` plus `uci-grabber-cli.exe`; macOS and Linux carry
`uci-grabber`. The packaged `portable.flag` keeps mutable state and installed
engines in `UCI-Grabber-Data/` beside the extracted application.

Linux x86-64 and ARM64 artifacts are built natively on Ubuntu 22.04 LTS
(Jammy) and require glibc 2.35 or newer on the matching architecture. Both
Linux jobs inspect the exact executable copied into the release archive: it
must be a matching ELF64 binary, may not require a glibc symbol newer than
2.35, and must have no unresolved dependency under `ldd` on the Jammy runner.

No Authenticode, Apple Developer ID, notarization credential, repository
variable, private release secret, or local GitHub CLI login is required.
Required macOS ad-hoc signatures provide no publisher identity. A one-time
Ed25519 key authenticates only the immutable catalog embedded in one app
release; it is not native-code or publisher signing.

## Checked-in review

`packaging/maia3/release-review.json` is the fail-closed release attestation.
`review_digest.py verify-release --tag vMAJOR.MINOR.PATCH` checks the exact tag,
component metadata, direct-download manifest, reviewed distribution decision,
checkpoint-term facts, and digest of every active launcher/catalog/release
input.

`direct-downloads.json` pins portable Python, Maia source, python-chess, and the
runtime-only wheel set by immutable HTTPS URL, byte count, SHA-256, archive
format, and collision-free destination. Offline validation also requires every
wheel to match the appropriate checked platform inventory. The 5M and 23M
model cards apply CC BY 4.0 only to the paper, point to the Maia repository for
code and weights terms, and have no independent weights LICENSE; the 79M card
states AGPLv3. The review consequently approves direct end-user retrieval only.

## Tag publication flow

1. Run all Rust, Python, schema, metadata, catalog, formatting, lint, and
   workflow checks.
2. Confirm `python3 packaging/maia3/review_digest.py verify-release --tag
   vMAJOR.MINOR.PATCH` succeeds on the exact commit to tag.
3. Push that commit and then its stable tag using the repository's existing SSH
   write access. The repository may be public; local `gh` authentication is not
   part of the process.
4. GitHub Actions builds and packages the four UCI Grabber launchers. It then
   hashes them and combines those hashes with the reviewed direct-upstream
   artifacts and checkpoint hashes to generate the populated catalog.
5. Actions generates a one-time Ed25519 key, signs the exact catalog bytes, and
   discards the private key. Every app build embeds that same populated
   catalog, signature, and public key. Remote catalog assets are
   version-specific and never use a mutable `releases/latest` URL.
6. The final job creates or reuses the exact draft release, replaces every
   expected asset on a rerun, rejects any extra/missing asset, and compares
   remote SHA-256 digests with the staged files before publishing and marking it
   latest. Publication alone receives job-scoped `contents: write`.

The release contains four app archives, four UCI Grabber Maia launcher
archives, `MAIA3-DIRECT-DOWNLOAD-NOTICES.txt`, versioned catalog
JSON/signature/public-key assets, the signed empty generic catalog retained for
v0.1.0 compatibility, both schema documents, and `SHA256SUMS`. It contains no
third-party engine/runtime/checkpoint asset.

Changing an upstream URL, byte count, digest, checkpoint revision, launcher,
entry point, extraction rule, review decision, workflow, catalog trust design,
or tag invalidates the checked review. These gates document the maintainer's
release decision; they are not legal advice.
