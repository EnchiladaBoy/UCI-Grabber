# Catalog and recipe contract

UCI Grabber accepts data-only `uci-grabber-recipe/v1` documents. A curated
`uci-grabber-catalog/v1` wraps those recipes and authenticates the exact UTF-8
catalog bytes with a detached 64-byte Ed25519 signature.

The JSON Schemas in `schema/` define the serialized wire shape and support
publisher-side checks. The application's validation is authoritative, adds
cross-field invariants that JSON Schema does not express, and rejects unknown
fields. Recipes cannot run commands, set environment variables, add install
hooks, or write outside an immutable installation generation.

## Trust model

- Curated recipes exist only in the signed catalog embedded in an application
  release or its byte-identical, version-specific catalog asset.
- A local or HTTPS recipe imported by the user is always **Unreviewed**.
- UCI Grabber never grants trust on FishEye's behalf. FishEye fingerprints and
  validates an installed executable independently.

The checked-in `catalog-public-key.pem`, `catalog.pub`, `catalog.json`, and
`catalog.sig` form a signed, empty bootstrap for source builds and v0.1.0
compatibility. A production release first builds and hashes the four Maia
launchers, then generates a fresh one-time Ed25519 key and populated catalog.
Every application build embeds that catalog, signature, and public key; the
private key is discarded before publication.

Remote catalog assets use versioned names such as `catalog-v0.2.0.json` and are
never loaded through a mutable `latest` URL. A source build retains the empty
bootstrap key and cannot authenticate a production release's one-time key, so a
metadata refresh does not populate its catalog.

## Limits

The v1 application enforces a 512 KiB recipe/catalog input limit, 4 KiB
signature limit, 1 GiB runtime artifact limit, 400 MiB model artifact limit,
and 2 GiB limits for both cumulative declared downloads and extracted output.
Each generation is also limited to 40,000 filesystem entries and 1 GiB per
entry. HTTPS redirects and response sizes are bounded.

Extraction rejects traversal, device nodes, absolute paths, Windows-invalid or
aliasing names, duplicate destinations, and case-folding collisions on
case-insensitive destination filesystems. ZIP symbolic links and tar hard links
fail. A contained relative tar symbolic link to a regular file is flattened;
all other symbolic-link targets fail. See the [security model](../docs/SECURITY.md)
for the complete extraction and native-code boundaries.

## Curated Maia3 recipe

Maia3 is the sole curated production recipe. Its 5M, 23M, and 79M variants pin
portable Python, Maia source, runtime dependencies, and checkpoints to reviewed
URLs, sizes, and SHA-256 hashes. Users retrieve those third-party bytes directly
from their publishers; UCI Grabber release assets do not contain them.

The generator includes Maia3 only when `--include-maia3` and the reviewed
metadata digest are supplied. `packaging/maia3/release-review.json` is the
canonical release attestation, `direct-downloads.json` contains the upstream
artifact manifest, and `DIRECT-DOWNLOAD-NOTICES.txt` records the corresponding
license and source notices. See the [release process](../docs/RELEASING.md) for
the publication gates.
