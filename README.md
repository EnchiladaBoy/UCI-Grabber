# UCI Grabber

UCI Grabber installs complete, zero-argument UCI chess engine packages for use
with [FishEye](https://github.com/EnchiladaBoy/FishEye). It is a separate
Apache-2.0 application and never writes FishEye settings or grants an engine
trust on FishEye's behalf. Every installed package is also a standard portable
UCI engine: users of another compatible chess GUI can copy or reveal the engine
path and add it there without using the FishEye handoff.

The checked-in bootstrap catalog is deliberately empty. A reviewed production
release can publish one curated Maia3 recipe covering the 5M, 23M, and 79M
checkpoints; each checkpoint URL is pinned to an immutable Hugging Face revision
and each platform runtime is pinned to the same UCI Grabber release. Exact
checkpoint download, use, and redistribution terms remain a fail-closed release
review. You can also import strict, data-only recipes for other engines. Custom
recipes cannot contain commands, hooks, environment variables, or absolute
paths. Their downloaded executable is still arbitrary native code: UCI testing
runs it with your account permissions and no OS sandbox, so explicit approval is
required first.

Release archives are portable and carry no trusted publisher signature. They
use no Authenticode or Apple Developer ID credentials and are not notarized. The
signed catalog authenticates curated artifact hashes, but it does not make an
executable trusted by the operating system or by FishEye. Windows or macOS may
therefore warn before first launch; verify the published checksums and release
origin before proceeding. macOS build tools may apply required ad-hoc signatures,
which provide no publisher identity or trust.

Keep the complete extracted application folder together. On Windows,
`UCI-Grabber.exe` launches the no-console GUI and `uci-grabber-cli.exe` provides
terminal commands; macOS and Linux use `uci-grabber`. Packaged builds keep their
mutable state and portable engine library in `UCI-Grabber-Data/` beside the
application instead of a machine-specific application-data location.
Version 0.1.0 used the operating system's application-data directory; upgrading
does not delete that legacy state. Advanced users can still inspect it with the
CLI's explicit `--data-dir` option and re-import any custom recipes they need.

## Quick start

1. Extract the whole release folder to a writable location and open UCI Grabber.
   There is no system installer and no administrator setup.
2. The app refreshes the signed catalog automatically. Choose Maia3 5M, 23M,
   or 79M, then select **Install** or **Install & open in FishEye**.
3. UCI Grabber downloads the model and runtime, verifies both, assembles the
   portable package, and tests the resulting engine. The ready screen can copy
   the engine executable path or open its package folder for any UCI-compatible
   chess GUI. The FishEye option opens FishEye 1.8.0 or newer at its own review
   screen; FishEye still asks before saving the engine.

## Build and run

Rust 1.92 or newer is required.

```console
cargo run --release
```

The GUI has **Catalog**, **Installed**, and **Custom Recipes** views. An install
is downloaded into staging, checked against its declared byte count and SHA-256,
safely extracted, tested with `uci`, `isready`, and a legal depth-one move from
the starting position, then atomically activated as an immutable generation.

Useful CLI commands:

```console
uci-grabber list --refresh
uci-grabber import ./engine-recipe.json
uci-grabber install recipe-id --model model-id --approve-unreviewed
uci-grabber status --repair
uci-grabber open-in-fisheye 'recipe-id:model-id:1.0.0:linux-x86_64'
```

Use `uci-grabber-cli.exe` in place of `uci-grabber` for these commands on
Windows.

`open-in-fisheye` launches only:

```text
fisheye gui --add-external-engine PATH
```

FishEye still fingerprints, tests, and asks the user to approve that path.
The GUI always provides **Copy engine path** and **Open package folder**
fallbacks.

See [the recipe format](docs/RECIPE_FORMAT.md), [security model](docs/SECURITY.md),
and [release process](docs/RELEASING.md) for the exact contracts and limits.

## License

UCI Grabber itself is licensed under Apache-2.0. Downloaded engines, runtimes,
and models retain their own licenses; review the metadata shown before install.
