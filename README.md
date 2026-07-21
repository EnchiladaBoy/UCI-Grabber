# UCI Grabber

UCI Grabber installs complete, zero-argument UCI chess engine packages for use
with [FishEye](https://github.com/EnchiladaBoy/FishEye). It is a separate
Apache-2.0 application and never writes FishEye settings or grants an engine
trust on FishEye's behalf.

The signed curated catalog is empty until each engine's runtime, model, and
redistribution terms pass the release gates. You can already import strict,
data-only recipes for other engines. Custom recipes cannot contain commands,
hooks, environment variables, or absolute paths. Their downloaded executable is
still arbitrary native code: UCI testing runs it with your account permissions
and no OS sandbox, so explicit approval is required first.

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

`open-in-fisheye` launches only:

```text
fisheye gui --add-external-engine PATH
```

FishEye still fingerprints, tests, and asks the user to approve that path.
The GUI always provides **Copy path** and **Reveal** fallbacks.

See [the recipe format](docs/RECIPE_FORMAT.md), [security model](docs/SECURITY.md),
and [release process](docs/RELEASING.md) for the exact contracts and limits.

## License

UCI Grabber itself is licensed under Apache-2.0. Downloaded engines, runtimes,
and models retain their own licenses; review the metadata shown before install.
