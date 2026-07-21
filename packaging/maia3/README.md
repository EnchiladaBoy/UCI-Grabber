# Directly retrieved Maia3 package

UCI Grabber does not bundle or republish Maia3, CPython, PyTorch, NumPy,
python-chess, or checkpoint bytes. The release contains one small Apache-2.0
launcher per platform. After the user chooses a model, the signed catalog makes
UCI Grabber retrieve every other artifact directly from its upstream publisher
and assemble a private portable package on the user's machine.

The catalog pins Maia3 commit
`1e13597c42d4858b7cfd7cfdae01e297263364b2`, chess 1.11.2, CPython 3.12.13 from
an immutable python-build-standalone release, PyTorch 2.11.0 CPU, NumPy 2.2.6,
and the small runtime dependency set by URL, byte count, and SHA-256. The three
checkpoints are likewise fetched directly from immutable Hugging Face
revisions. GitHub release assets contain none of those upstream bytes.

The launcher finds the one selected checkpoint, starts the local portable
Python with isolated user-site settings, forces CPU/offline behavior, and runs
the source entry point with inherited UCI standard input/output. The entry point
verifies the checkpoint again before importing Maia3. The installed executable
therefore works in FishEye or any other GUI that accepts a zero-argument UCI
engine path.

After assembly, UCI Grabber binds the launcher to the path and contents of every
other regular file in that local package. Each launch verifies that snapshot
using read-only access before Python starts. FishEye separately fingerprints the
personalized launcher; other GUIs can use the same executable, but should rely
on the published UCI Grabber checksum/origin for authenticity rather than
treating the embedded, unkeyed snapshot as a publisher signature.

The Linux x86-64 and ARM64 launchers require glibc 2.35 or newer (Ubuntu 22.04
LTS or an equivalent distribution) on the matching CPU architecture.

Portable CPython archives contain relative links. Extraction resolves only
in-archive links to regular files and materializes them as regular files;
absolute, escaping, cyclic, unresolved, directory, and special-file links fail
closed. The existing path, collision, entry-count, and byte budgets still apply.

## Fail-closed publication

The source, Python, dependency, and checkpoint revisions and hashes are accepted
only through strict checked metadata and the tag-bound `release-review.json`.
That review is limited to direct end-user retrieval. It does not claim that UCI
Grabber relicenses or redistributes upstream bytes. Any absent, malformed, or
stale field fails before the release builds its signed catalog.

The released application and launcher archives are portable and carry no
trusted publisher signature. Publisher signing is not treated as a license
condition. Release notes state that Windows or macOS may display an
unidentified-developer or similar warning and direct users to verify the
release origin and published checksums.

## Local checks

```console
python3 packaging/maia3/validate_metadata.py
python3 packaging/maia3/review_digest.py verify-release --tag v0.2.0
python3 -m unittest discover -s packaging/maia3/tests -v
python3 -m compileall -q packaging/maia3 catalog
python3 catalog/verify_catalog.py --catalog catalog/catalog.json \
  --signature catalog/catalog.sig --bootstrap
```
