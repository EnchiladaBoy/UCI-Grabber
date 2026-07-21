# Catalog and recipe contract

UCI Grabber accepts strict, data-only `uci-grabber-recipe/v1` documents. The
curated feed wraps those recipes in `uci-grabber-catalog/v1` and authenticates
the exact UTF-8 catalog bytes with a detached 64-byte Ed25519 signature.

The JSON Schemas in `schema/` are documentation and publisher-side validation.
The application also validates every field itself and rejects unknown fields.
A recipe cannot run shell commands, add environment variables, use install
hooks, or write outside its immutable installation generation.

## Trust classes

- Curated recipes are present only in a currently valid, signed catalog.
- A local or HTTPS recipe imported by the user is always labelled Unreviewed.
- An installed executable is never trusted on behalf of FishEye. FishEye
  fingerprints and validates the executable independently.

`catalog-public-key.pem` is the production catalog verification key. Its private
half is never tracked and is retained only in protected release-signing storage
and the `CATALOG_ED25519_PRIVATE_KEY_BASE64` Actions secret. The release workflow
checks that the secret key matches `catalog.pub` before it can publish anything.
Rotating the key requires replacing both public-key files, re-signing the empty
bundled catalog, and shipping the new verification key in an application update.

## Limits

The v1 application enforces a 512 KiB recipe/catalog input limit, 4 KiB
signature limit, 1 GiB runtime artifact limit, 400 MiB model artifact limit,
2 GiB cumulative declared-download and extracted-install limits, a generation-
wide 40,000 filesystem-entry limit, and a 1 GiB per-entry limit. Redirects
remain HTTPS and are bounded. Archive paths, symlinks, device nodes, absolute
paths, Windows drives/alternate streams/invalid characters/reserved device
names, backslashes, dot aliases, trailing dots/spaces, and `..` traversal are
rejected.

## Maia3 is not yet published

`packaging/maia3/component-metadata.json` records the proposed 5M, 23M, and 79M
inputs, but the default generated catalog is empty. The generator includes the
Maia3 recipe only when `--include-maia3` and the exact SHA-256 of that metadata
are both supplied. CI obtains the digest from `MAIA3_MODEL_LICENSE_REVIEW` only
after written review of the exact checkpoint revisions and terms. Runtime jobs
add independent digest gates for the corresponding-source policy/build inputs
and each platform's canonical wheelhouse before a Maia recipe can reach the
published catalog.
