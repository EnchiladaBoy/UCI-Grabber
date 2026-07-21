# Release and compliance gates

UCI Grabber application packages contain the Apache-2.0 Rust application and
compatible dependency notices only. The separately distributed Maia3 native runtime contains
Maia3 code offered upstream under AGPL-3.0 and packaged dependencies under their
respective terms. It must be published as separate assets with notices, build
provenance, reviewed source/build materials, a written Corresponding-Source
determination, and checksums.
Checkpoint bytes are fetched directly from their immutable Hugging Face
revisions; they are never bundled in UCI Grabber application or GitHub release
assets.

Every stable tag builds `uci-grabber-windows-x86_64.zip`,
`UCI-Grabber-macos-aarch64.app.zip`, and Linux x86-64/arm64 tarballs. Each
package contains the Apache license, README, and a `cargo-about` notice
inventory generated from the locked dependency graph.

Application and Maia3 runtime archives are portable, checksummed release assets
with no trusted publisher signature. They use no Authenticode or Apple Developer
ID credentials and are not notarized. PyInstaller/macOS may apply required ad-hoc
signatures, which provide no publisher identity or trust. Document the resulting
operating-system warning in release notes. Publisher signing is not a license
requirement and is not part of the portable release contract.

Keep the complete extracted application folder together. Windows packages carry
`UCI-Grabber.exe` for the no-console GUI and `uci-grabber-cli.exe` for terminal
commands; macOS and Linux retain `uci-grabber`. The packaged `portable.flag`
places mutable state and the installed engine library in an adjacent
`UCI-Grabber-Data/` directory rather than a machine-specific application-data
location.

Before a stable catalog release:

1. Confirm the production public key in `catalog.pub`, the PEM file, and the
   signature on the empty bundled catalog match the private key retained only in
   protected signing storage. App-only releases verify and publish those committed
   catalog bytes without handling the private key. A reviewed Maia3 release must
   additionally configure `CATALOG_ED25519_PRIVATE_KEY_BASE64`, because its catalog
   includes hashes of the platform artifacts built in CI. The workflow derives that
   secret's public key and requires it to match `catalog.pub` before signing. The
   application embeds the public key and bundled catalog, so mismatched bytes fail
   its tests and the release gate.
2. Obtain written review for download, use, and redistribution of the exact
   Maia3 5M, 23M, and 79M checkpoint revisions. Set repository variable
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

   The source input set includes the exact release and wheelhouse-review workflow
   files, as well as the Maia3 packaging definitions. Changing any source input
   or wheelhouse digest invalidates this value.
5. Confirm every Maia runtime archive contains `CORRESPONDING-SOURCE.txt`, and
   that the generated recipe Source link resolves to the exact same-tag
   `maia3-corresponding-source.tar.gz` release asset. Confirm release notes state
   that portable application and runtime archives carry no trusted publisher
   signature and may trigger operating-system warnings.
6. Confirm the release contains all four application packages, the signed
   catalog, and checksums. When Maia3 is enabled, also require all four
   model-free runtimes, `maia3-corresponding-source.tar.gz`, and
   `MAIA3-NOTICES.txt`.
7. The workflow creates a draft release so an incomplete upload can never become
   the live catalog endpoint. Inspect every asset and checksum, publish the draft,
   mark it latest, then verify `releases/latest/download/catalog.json` and
   `catalog.sig` before announcing the release.

Changing any model revision, digest, runtime input, or build definition changes
the model, source-input-set, or wheelhouse review digest and requires a new
review/release.
For v0.1.1 and later, an absent or stale model-review variable fails the release;
the workflow must never publish an empty fallback catalog. Missing or stale
source and wheelhouse reviews also fail before installation or publication.
These gates record a review decision; they do not replace legal advice or prove
that a particular source classification is valid.
