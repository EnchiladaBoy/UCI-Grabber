# Release process

UCI Grabber releases contain the Apache-2.0 application, Rust dependency
notices, and one small UCI Grabber launcher per supported platform. They do not
contain Maia3, CPython, PyTorch, NumPy, python-chess, or Maia checkpoint bytes.
A user-selected install retrieves the reviewed bytes from their upstream
publishers and assembles the portable engine locally.

Stable releases target Windows x86-64, macOS ARM64, Linux x86-64, and Linux
ARM64. Windows includes the no-console `UCI-Grabber.exe` and
`uci-grabber-cli.exe`; macOS and Linux use `uci-grabber`. The packaged
`portable.flag` keeps mutable state and installed engines in
`UCI-Grabber-Data/` beside the extracted application.

Linux artifacts are built natively on Ubuntu 22.04 LTS and require glibc 2.35
or newer on the matching architecture. Each Linux job inspects the executable
copied into the archive: it must be a matching ELF64 binary, require no glibc
symbol newer than 2.35, and have no unresolved dependency on the build runner.

No Authenticode, Apple Developer ID, notarization credential, repository
variable, persistent private release secret, or local GitHub CLI login is
required. Required macOS ad-hoc signatures provide no publisher identity. A
one-time Ed25519 key authenticates only the immutable catalog embedded in one
application release; it is not native-code or publisher signing.

## Review gates

`packaging/maia3/release-review.json` is the fail-closed release attestation.
`review_digest.py verify-release --tag vMAJOR.MINOR.PATCH` verifies the exact
tag, component metadata, direct-download manifest, reviewed distribution
decision, checkpoint-term facts, and digest of every active release input.

`direct-downloads.json` pins portable Python, Maia source, python-chess, and
runtime-only wheels by immutable HTTPS URL, byte count, SHA-256, archive
format, and collision-free destination. Component metadata separately pins the
checkpoint revisions and hashes. The reviewed distribution conclusions live
in `release-review.json`; the corresponding user-facing license and source
statements live in
`DIRECT-DOWNLOAD-NOTICES.txt`. Those files are the canonical records—do not
duplicate their detailed conclusions in release prose.

Any change to a file listed in `RELEASE_INPUTS` in `review_digest.py` makes the
attestation stale, including documentation such as the packaged `README.md`.
Changes to component metadata or `direct-downloads.json` also require their
individual reviewed digests to be updated.

## Prepare a release

1. Update the application version in `Cargo.toml` and `Cargo.lock`. Update the
   release tag in `review_digest.py`, `release-review.json`, and the CI review
   assertion. Search for the previous version and tag to catch documentation
   examples and test fixtures.
2. Review every metadata, download, assembly, catalog, workflow, and launcher
   change. Update the checked component and direct-download digests only after
   that review is complete.
3. Run the complete check set in [DEVELOPMENT.md](DEVELOPMENT.md), plus any
   platform-specific smoke checks required by the release changes.
4. Run `python3 packaging/maia3/review_digest.py inputs`, review the complete
   `RELEASE_INPUTS` diff, and record the resulting digest as
   `release_inputs_sha256` in `release-review.json`.
5. Run `python3 packaging/maia3/review_digest.py verify-release --tag
   vMAJOR.MINOR.PATCH` on the exact commit that will be tagged.

## Publish a tag

1. Push the reviewed commit, then its stable `vMAJOR.MINOR.PATCH` tag, using the
   repository's existing SSH write access.
2. GitHub Actions builds and packages the four UCI Grabber launchers, hashes
   them, and combines those hashes with the reviewed upstream artifacts to
   generate the populated catalog.
3. Actions creates a one-time Ed25519 key, signs the exact catalog bytes, embeds
   that catalog, signature, and public key in every application build, and
   discards the private key. Remote catalog assets are version-specific and
   never use a mutable `releases/latest` URL.
4. The final job creates or reuses the exact draft release, replaces every
   expected asset on a rerun, rejects extra or missing assets, and compares all
   remote SHA-256 digests with the staged files before publication. Only this
   job receives job-scoped `contents: write`.

The workflow's publish job is the authoritative asset whitelist. In summary,
the release includes four application archives, four Maia launcher components,
direct-download notices, versioned catalog assets, the backward-compatible
empty bootstrap catalog, both schemas, and `SHA256SUMS`. It includes no
third-party engine, runtime, dependency, or checkpoint asset.

These gates record the maintainer's release decision; they are not legal
advice.
