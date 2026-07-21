# Security model

Catalog authentication occurs before JSON parsing. Release catalog bytes use
canonical pretty JSON with LF line endings and a detached raw Ed25519
signature. The client keeps a verified catalog for no more than 24 hours and
does not replace a valid cache with a failed download.

Each production app embeds its populated catalog, detached signature, and a
one-time public key generated only for that release. An optional explicit
verification downloads the byte-identical, version-specific asset pair from
`releases/download/vVERSION/catalog-vVERSION.json` and `.sig`; it never follows
a mutable `latest` feed. The private catalog key is discarded before release,
so catalog changes require a new application release. The checked-in source
bootstrap remains signed and empty because launcher hashes do not exist until
release CI has built them. UCI Grabber never downloads portable Python, engine
source, dependencies, or a model until the user selects a model and presses
Install.

Downloads are written to a new staging directory, bounded by each declared size
and a 2 GiB cumulative package declaration, hashed while streaming, and compared
with the recipe before extraction. Extraction applies generation-wide 2 GiB and
40,000-entry budgets and rejects traversal, special files, exact duplicate
destinations, Windows-invalid or aliasing names, entry/size bombs, and any write
outside staging. Case-folding collisions are additionally rejected whenever the
destination filesystem is case-insensitive; case-distinct paths remain distinct
on a case-sensitive filesystem. Relative archive links are accepted only when
they resolve inside the same archive to a regular file and are flattened to
regular files; absolute, escaping, cyclic, unresolved, and directory links are
rejected. A bounded, well-formed global PAX `comment` record is ignored;
behavior-changing global PAX keys and every other special archive member are
rejected. Activation is an atomic rename into an immutable versioned generation.
Cancellation and crashes leave the previous generation untouched; recoverable
staging directories are cleaned on the next startup.

For the curated Maia package, UCI Grabber personalizes the reviewed launcher
with a digest, regular-file count, and byte count for every other package file.
The launcher recomputes that content snapshot with read-only access before every
start and refuses to run after a file is added, removed, renamed, or changed.
FishEye's independent fingerprint of the personalized launcher anchors the
embedded expected digest. The snapshot intentionally covers file paths and
contents, not empty directories or portable permission metadata. Outside a GUI
that fingerprints the launcher, this is change detection rather than standalone
authenticity: coordinated replacement of both launcher and payload requires an
independent checksum or trusted origin check.

The ready check runs the zero-argument executable with a restricted working
directory and bounded stdin/stdout/stderr, startup, readiness, and search
timeouts. The curated Maia launcher replaces itself with Python on Unix. On
Windows it joins a kill-on-close job before Python starts, so cancelling the
launcher also terminates Python and its descendants. UCI success does not grant
FishEye trust. “Use in FishEye” only passes the path to `fisheye gui
--add-external-engine`; FishEye independently inspects, fingerprints, tests, and
asks the user to approve it.

There is no background engine/artifact download, automatic executable
replacement, install hook, shell expansion, telemetry, or direct write to
FishEye configuration.

## Native executable boundary

Data-only recipes prevent an author from embedding installer commands, but they
do not make the downloaded engine safe. The final UCI check starts the recipe's
native executable with the current user's account permissions. UCI Grabber
applies protocol and time/output limits, but v1 does not provide an operating
system sandbox, container, privilege drop, or filesystem/network isolation.
Only approve a custom recipe when you trust its publisher and exact artifact
hashes. An Unreviewed label is a warning, not a security boundary.
