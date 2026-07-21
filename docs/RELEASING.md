# Release and compliance gates

UCI Grabber application packages contain the Apache-2.0 Rust application and
compatible dependency notices only. The optional Maia3 native runtime is a
separate AGPL component and must be published as separate assets with notices,
build provenance, reviewed source/build materials, a written
Corresponding-Source determination, and platform signatures.
Checkpoint bytes are fetched directly from their immutable Hugging Face
revisions; they are never bundled in UCI Grabber application or GitHub release
assets.

Every stable tag builds `uci-grabber-windows-x86_64.zip`,
`UCI-Grabber-macos-aarch64.app.zip`, and Linux x86-64/arm64 tarballs. Each
package contains the Apache license, README, and a `cargo-about` notice
inventory generated from the locked dependency graph.

The v1 application archives are checksummed release assets but are not yet
Authenticode-signed or Apple-signed/notarized. Document the resulting operating
system warning in release notes; do not describe those archives as signed. This
is separate from curated Maia3: its Windows and macOS runtime publication fails
closed unless Authenticode and Developer ID/notarization credentials are
present. Adding application signing is a future release-hardening task.

Before a stable catalog release:

1. Confirm the production public key in `catalog.pub` and the PEM file matches
   the private key stored only in protected signing storage and
   `CATALOG_ED25519_PRIVATE_KEY_BASE64`. The application embeds the public key
   and bundled catalog, so mismatched bytes fail its tests and the release gate.
2. Obtain written review for commercial download/use of the exact Maia3 5M,
   23M, and 79M revisions. Set repository variable
   `MAIA3_MODEL_LICENSE_REVIEW` to the SHA-256 of
   `packaging/maia3/component-metadata.json` from the tagged commit.
3. Run the manual **Prepare Maia3 wheelhouse review** workflow. It downloads but
   never installs candidate wheels, then uploads a canonical
   `WHEELHOUSE.lock.json` for every platform. Review every filename,
   compatibility tag, size, and digest, then set these variables to the SHA-256
   of the corresponding inventory file:

   - `MAIA3_WHEELHOUSE_REVIEW_WINDOWS_X86_64`
   - `MAIA3_WHEELHOUSE_REVIEW_MACOS_AARCH64`
   - `MAIA3_WHEELHOUSE_REVIEW_LINUX_X86_64`
   - `MAIA3_WHEELHOUSE_REVIEW_LINUX_AARCH64`

   The release workflow resolves the inventories again, uploads them for audit,
   and requires those exact variable values before pip installs anything.
4. Review `packaging/maia3/corresponding-source-policy.json` against the exact
   frozen runtime and all four reviewed wheel inventories. Determine whether
   CPython, PyTorch, NumPy, PyInstaller, or transitive dependency sources/offers
   must be added. After satisfying that determination, compute the combined
   source/build-and-wheel review digest and set it as
   `MAIA3_CORRESPONDING_SOURCE_REVIEW`:

   ```console
   python3 packaging/maia3/review_digest.py source-release \
     --wheelhouse "windows-x86_64=$WINDOWS_WHEELHOUSE_SHA256" \
     --wheelhouse "macos-aarch64=$MACOS_WHEELHOUSE_SHA256" \
     --wheelhouse "linux-x86_64=$LINUX_X86_WHEELHOUSE_SHA256" \
     --wheelhouse "linux-aarch64=$LINUX_ARM_WHEELHOUSE_SHA256"
   ```

   Changing any source input or wheelhouse digest invalidates this value.
5. Configure `MAIA_WINDOWS_SIGNING_PFX_BASE64` and
   `MAIA_WINDOWS_SIGNING_PASSWORD`; configure the Apple certificate,
   identity, notarization, and team secrets documented in the workflow.
6. Confirm the release contains all four application packages, the signed
   catalog, and checksums. When Maia3 is enabled, also require all four
   model-free runtimes, `maia3-corresponding-source.tar.gz`, and
   `MAIA3-NOTICES.txt`.

Changing any model revision, digest, runtime input, or build definition changes
the model, source-input-set, or wheelhouse review digest and requires a new
review/release.
Absence or mismatch of the model-review variable omits Maia3 from the catalog;
it must never be interpreted as approval. Once that model gate is enabled,
missing/stale source or wheelhouse reviews fail the Maia release before
installation/publication. These gates record a review decision; they do not
replace legal advice or prove that a particular source classification is valid.
