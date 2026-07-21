# Catalog and recipe contract

UCI Grabber accepts strict, data-only `uci-grabber-recipe/v1` documents. The
curated feed wraps those recipes in `uci-grabber-catalog/v1` and authenticates
the exact UTF-8 catalog bytes with a detached 64-byte Ed25519 signature.

The JSON Schemas in `schema/` are documentation and publisher-side validation.
The application also validates every field itself and rejects unknown fields.
A recipe cannot run shell commands, add environment variables, use install
hooks, or write outside its immutable installation generation.

## Trust classes

- Curated recipes are present only in the signed catalog embedded in that app
  release (or the byte-identical, version-specific release asset).
- A local or HTTPS recipe imported by the user is always labelled Unreviewed.
- An installed executable is never trusted on behalf of FishEye. FishEye
  fingerprints and validates the executable independently.

The checked-in `catalog-public-key.pem`, `catalog.pub`, `catalog.json`, and
`catalog.sig` form a signed empty bootstrap for source builds and v0.1.0
compatibility. A production release first builds and hashes UCI Grabber's four
small Maia launchers, combines those hashes with reviewed immutable upstream
artifacts, then generates a fresh one-time Ed25519 key and a populated catalog.
The release app is compiled with that catalog, signature, and public key; the
private key is discarded before publication. The matching remote assets use versioned names
such as `catalog-v0.2.0.json` and are never obtained through a mutable `latest`
URL. Consequently, changing the curated catalog requires a new app release and
no persistent signing secret or GitHub repository setting is required.

A plain build from the tagged source keeps the bootstrap key. It cannot verify
the different one-time key created later by release CI, so refreshing metadata
does not turn a source build into the populated production package.

## Limits

The v1 application enforces a 512 KiB recipe/catalog input limit, 4 KiB
signature limit, 1 GiB runtime artifact limit, 400 MiB model artifact limit,
2 GiB cumulative declared-download and extracted-install limits, a generation-
wide 40,000 filesystem-entry limit, and a 1 GiB per-entry limit. Redirects
remain HTTPS and are bounded. Archive paths, device nodes, absolute paths,
Windows drives/alternate streams/invalid characters/reserved device names,
backslashes, dot aliases, trailing dots/spaces, and `..` traversal are rejected.
Exact duplicate output paths always fail; case-folding collisions also fail on
case-insensitive destination filesystems. Contained relative links to regular
files are flattened; all other links fail.

## Maia3 production recipe

Maia3 is the sole curated production recipe. It offers the 5M, 23M, and 79M
models as variants of one recipe, with checkpoint downloads pinned directly to
full Hugging Face revisions. The checked-in bootstrap `catalog.json` remains
empty so source builds never promise release assets that do not yet exist.
Release CI overlays the generated populated catalog before compiling each
portable application package. The generator includes exactly one Maia3 recipe
only when `--include-maia3` and the exact SHA-256 of the reviewed metadata are
supplied. The recipe combines a same-release UCI Grabber launcher with portable
Python, Maia source, runtime-only dependency wheels, and a checkpoint retrieved
directly from immutable upstream publisher URLs.

CI obtains that digest from the strict, checked-in `release-review.json`, which
records the exact direct-retrieval scope and binds the component metadata,
artifact manifest, and active assembly inputs. The recipe's License link points
to the exact same-release `MAIA3-DIRECT-DOWNLOAD-NOTICES.txt`; its Source link
points to the exact immutable Maia upstream commit. UCI Grabber release assets
contain none of those third-party bytes.

Application and launcher archives are portable and carry no Authenticode or Apple
Developer ID publisher signature and are not notarized. The catalog signature
authenticates the catalog and its declared hashes, not native-code publisher
identity, so operating-system warnings may still appear. Required macOS ad-hoc
signatures do not identify or establish trust in a publisher. The one-time
catalog key likewise provides no publisher identity.
