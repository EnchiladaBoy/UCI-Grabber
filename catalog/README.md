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
half is never tracked and is retained only in protected release-signing storage.
App-only releases verify and publish the already signed empty catalog, so the
private key does not need to leave offline storage. A reviewed Maia3 release
generates an artifact-specific catalog in CI and therefore additionally requires
the `CATALOG_ED25519_PRIVATE_KEY_BASE64` Actions secret; the workflow checks that
key against `catalog.pub` before signing. Rotating the key requires replacing both
public-key files, re-signing the empty bundled catalog, and shipping the new
verification key in an application update.

## Limits

The v1 application enforces a 512 KiB recipe/catalog input limit, 4 KiB
signature limit, 1 GiB runtime artifact limit, 400 MiB model artifact limit,
2 GiB cumulative declared-download and extracted-install limits, a generation-
wide 40,000 filesystem-entry limit, and a 1 GiB per-entry limit. Redirects
remain HTTPS and are bounded. Archive paths, symlinks, device nodes, absolute
paths, Windows drives/alternate streams/invalid characters/reserved device
names, backslashes, dot aliases, trailing dots/spaces, and `..` traversal are
rejected.

## Maia3 production recipe

Maia3 is the sole curated production recipe. It offers the 5M, 23M, and 79M
models as variants of one recipe, with checkpoint downloads pinned directly to
full Hugging Face revisions. The checked-in bootstrap `catalog.json` remains
empty so an application package never promises release assets that do not yet
exist. The release generator includes exactly one Maia3 recipe only when
`--include-maia3` and the exact SHA-256 of the reviewed metadata are supplied.

CI obtains that digest from `MAIA3_MODEL_LICENSE_REVIEW` only after written
review of download, use, and redistribution for the exact checkpoint revisions.
Independent digest gates bind the runtime source/build policy and each
platform's canonical wheelhouse. The recipe's License link points to the exact
same-release `MAIA3-NOTICES.txt`, which carries the Maia3 code license and lists
the pinned model-card/terms pages for every checkpoint without claiming that
one license covers the whole installation. Its Source link points directly to
`maia3-corresponding-source.tar.gz` on that same tagged UCI Grabber release.

Application and runtime archives are portable and carry no Authenticode or Apple
Developer ID publisher signature and are not notarized. The catalog signature
authenticates the catalog and its declared hashes, not native-code publisher
identity, so operating-system warnings may still appear. Required macOS ad-hoc
signatures do not identify or establish trust in a publisher.
