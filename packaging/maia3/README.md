# Separately distributed Maia3 component

UCI Grabber does not bundle Maia3 or checkpoint bytes in its Apache-2.0
application packages. Release CI may build four model-free native CPU runtimes
from immutable upstream commit
`1e13597c42d4858b7cfd7cfdae01e297263364b2`. Those runtime assets are an AGPL
component and travel with reviewed source/build materials, notices, dependency
inventory, provenance, platform signatures, and checksums.

Each archive has one `maia3-runtime/` root and three zero-argument launchers:
`maia3-5m`, `maia3-23m`, and `maia3-79m` (with `.exe` on Windows). A launcher
derives its checkpoint from `maia3-runtime/models/`, verifies the exact size and
SHA-256 before importing Maia3, forces CPU/offline mode, and then speaks UCI.
Models are deliberately absent from the runtime and corresponding-source
archives.

Archive creation rejects escaping, broken, and directory symlinks. A safe
in-root file symlink is flattened to a regular archive member because UCI
Grabber deliberately rejects all links during extraction. Release CI extracts
the finished macOS archive and revalidates its code signature and stapled
notarization ticket before publication.

Builds use CPython 3.12.10, PyTorch 2.11.0 CPU, and PyInstaller 6.21.0. Release
jobs download a candidate native wheelhouse and produce a canonical
platform-labelled `WHEELHOUSE.lock.json`. That inventory is uploaded for review
before installation. Its exact file SHA-256 must match the platform repository
variable; only then does pip install the local wheelhouse with `--require-hashes`.
The exact dependency and source inputs are recorded in each runtime's
`BUILDINFO.json`.

The source/build archive includes the exact Maia3 checkout, chess 1.11.2 source
distribution, and these build definitions. It does not claim that CPython,
PyTorch, NumPy, PyInstaller, and every transitive wheel source are automatically
included or excluded from Corresponding Source. `corresponding-source-policy.json`
records that open classification. `review_digest.py source` identifies the
precise base build inputs; the publication value produced by `source-release`
also binds all four reviewed wheelhouse digests. Written review must determine
whether additional source archives or durable offers are required before
runtime publication.

## Fail-closed publication

The three checkpoint revisions and hashes in `component-metadata.json` are
proposed inputs, not a statement that their commercial/download terms have
been approved. The ordinary generated catalog contains no Maia3 recipe. Only a
release job with a matching `MAIA3_MODEL_LICENSE_REVIEW` digest may request
`--include-maia3`. Once that checkpoint gate is enabled, a matching
`MAIA3_CORRESPONDING_SOURCE_REVIEW` and all four reviewed wheelhouse digests are
also mandatory; missing or stale values fail before any candidate wheel is
installed.

The Apple and Windows release jobs also fail without code-signing/notarization
credentials. Do not weaken these gates or publish unsigned stable runtimes.

## Local checks

```console
python3 packaging/maia3/validate_metadata.py
python3 packaging/maia3/review_digest.py source
python3 -m unittest discover -s packaging/maia3/tests -v
python3 -m compileall -q packaging/maia3 catalog
python3 catalog/verify_catalog.py --catalog catalog/catalog.json \
  --signature catalog/catalog.sig --bootstrap
```
