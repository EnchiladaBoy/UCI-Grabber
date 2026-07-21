# Directly retrieved Maia3 package

UCI Grabber releases do not bundle or republish Maia3, CPython, PyTorch, NumPy,
python-chess, or checkpoint bytes. They contain one small Apache-2.0 launcher
per platform. After the user chooses a model, the signed catalog retrieves the
reviewed upstream artifacts and assembles a private portable package locally.

`direct-downloads.json` is the canonical URL, byte-count, and SHA-256 manifest
for Python, Maia source, and runtime dependencies. `component-metadata.json`
pins each checkpoint repository, revision, filename, byte count, and digest.
GitHub release assets contain none of those upstream bytes.

## Launcher and package integrity

The launcher finds the selected checkpoint, starts portable Python with
isolated user-site settings, forces CPU/offline behavior, and runs the Maia3
entry point with inherited UCI input and output. The entry point verifies the
checkpoint before importing Maia3. The resulting executable works in FishEye
or any chess GUI that accepts a zero-argument UCI engine path.

After assembly, UCI Grabber personalizes the launcher with a digest, regular
file count, and byte count for every other package file. Each launch checks that
snapshot before Python starts. FishEye separately fingerprints the personalized
launcher. Other GUIs should use the published UCI Grabber checksum and origin
as their authenticity reference; the embedded unkeyed snapshot is change
detection, not a publisher signature.

Linux x86-64 and ARM64 launchers require glibc 2.35 or newer. Portable CPython
tar archives contain relative symbolic links; extraction materializes one only
when its complete in-archive target chain ends at a regular file. ZIP symbolic
links, tar hard links, escaping or unresolved targets, and other special entries
are rejected. The normal path, collision, entry-count, and byte budgets still
apply.

## Review gate

`release-review.json` binds the exact release tag, component metadata,
direct-download manifest, distribution decision, and every active
launcher/catalog/release input. An absent, malformed, changed, or stale field
fails before release CI builds the signed catalog. The review covers direct
end-user retrieval only and does not claim that UCI Grabber relicenses or
redistributes upstream bytes. `DIRECT-DOWNLOAD-NOTICES.txt` is the canonical
user-facing license and source record.

## Component checks

Run these from the repository root, substituting the release's exact tag. See
the [development guide](../../docs/DEVELOPMENT.md) for the complete project
check set.

```console
python3 packaging/maia3/validate_metadata.py
python3 packaging/maia3/review_digest.py verify-release \
  --tag vMAJOR.MINOR.PATCH
python3 -m unittest discover -s packaging/maia3/tests -v
python3 catalog/verify_catalog.py --catalog catalog/catalog.json \
  --signature catalog/catalog.sig --bootstrap
```
