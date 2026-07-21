# Development

UCI Grabber requires Rust 1.92 or newer. The catalog, packaging, and release
checks also require Python 3.12 or newer and the OpenSSL command-line tool.

On Ubuntu or Debian, install the native GUI build dependencies with:

```console
sudo apt-get install build-essential pkg-config \
  libdbus-1-dev libegl1-mesa-dev libgbm-dev libwayland-dev \
  libx11-dev libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libxrandr-dev
```

## Build and run

Run the GUI from the repository root:

```console
cargo run --release
```

The checked-in catalog is an intentionally empty, signed bootstrap. A local
source build can import custom recipes, but only official release builds embed
the curated Maia3 catalog and its release-specific key.

## Checks

Run the local quality checks used by `.github/workflows/ci.yml`:

```console
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked

python3 packaging/maia3/validate_metadata.py
python3 -m unittest discover -s packaging/app/tests -v
python3 -m unittest discover -s packaging/maia3/tests -v
python3 -m unittest discover -s catalog/tests -v
python3 catalog/verify_catalog.py \
  --catalog catalog/catalog.json \
  --signature catalog/catalog.sig \
  --bootstrap
python3 -m compileall -q packaging/app packaging/maia3 catalog
```

Release inputs have an additional tag-bound review gate. After an intentional
review, verify it with the release tag substituted below:

```console
python3 packaging/maia3/review_digest.py verify-release \
  --tag vMAJOR.MINOR.PATCH
```

The authoritative release workflow and review-input list are
`.github/workflows/release.yml` and `RELEASE_INPUTS` in
`packaging/maia3/review_digest.py`. CI additionally verifies that a Windows
checkout preserves every signed and review-gated byte.
