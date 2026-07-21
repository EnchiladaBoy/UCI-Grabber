# Security model

Catalog authentication occurs before JSON parsing. Stable catalog bytes use
canonical pretty JSON with LF line endings and a detached raw Ed25519
signature. The client keeps a verified catalog for no more than 24 hours and
does not replace a valid cache with a failed download.

The network feed is the immutable asset pair at
`releases/latest/download/catalog.json` and `catalog.sig`. A long-lived signed,
empty catalog is compiled into the application solely as an offline fallback;
the bootstrap policy rejects it if any recipe is added.

Downloads are written to a new staging directory, bounded by each declared size
and a 2 GiB cumulative package declaration, hashed while streaming, and compared
with the recipe before extraction. Extraction applies generation-wide 2 GiB and
40,000-entry budgets and rejects traversal, links, special files, duplicate or
case-folding destinations, Windows-invalid or aliasing names, entry/size bombs,
and any write outside staging. Activation is an atomic rename into an immutable
versioned generation. Cancellation and crashes leave the previous generation
untouched; recoverable staging directories are cleaned on the next startup.

The ready check runs the zero-argument executable with a restricted working
directory and bounded stdin/stdout/stderr, startup, readiness, and search
timeouts. UCI success does not grant FishEye trust. “Use in FishEye” only passes
the path to `fisheye gui --add-external-engine`; FishEye independently inspects,
fingerprints, tests, and asks the user to approve it.

There is no background download, automatic executable replacement, install
hook, shell expansion, telemetry, or direct write to FishEye configuration.

## Native executable boundary

Data-only recipes prevent an author from embedding installer commands, but they
do not make the downloaded engine safe. The final UCI check starts the recipe's
native executable with the current user's account permissions. UCI Grabber
applies protocol and time/output limits, but v1 does not provide an operating
system sandbox, container, privilege drop, or filesystem/network isolation.
Only approve a custom recipe when you trust its publisher and exact artifact
hashes. An Unreviewed label is a warning, not a security boundary.
