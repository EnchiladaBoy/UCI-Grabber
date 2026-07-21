# Custom recipe format

A custom recipe describes complete, directly runnable UCI packages. It cannot
describe arbitrary installation logic. Raw weights without a compatible
zero-argument runtime are invalid.

`catalog/schema/recipe-v1.schema.json` defines the serialized wire shape and is
useful for publisher-side checks. UCI Grabber's own validation is authoritative
and also enforces cross-field rules that JSON Schema does not express. Important
rules are:

- `id` and model IDs are stable lowercase identifiers; `version` is immutable.
- All artifact URLs use HTTPS. Publishers should use stable, versioned URLs;
  the declared byte count and SHA-256 identify the bytes UCI Grabber accepts.
- Every artifact has an exact positive `byte_count` and lowercase SHA-256.
- `destination`, `executable`, and `working_directory` are forward-slash,
  relative paths contained by the installation generation. Every component must
  also be portable to Windows: control characters, `< > : " | ? *`, backslashes,
  dot aliases, trailing dots/spaces, and reserved device names such as `CON`,
  `NUL.txt`, `COM1`, and `LPT1.log` (case-insensitively) are rejected.
- `working_directory` is exactly the executable's parent (`.` for an executable
  at the package root), matching FishEye's path-only handoff contract.
- Each platform occurs at most once per model. It names one runtime artifact,
  at most one model artifact, and the executable produced after extraction.
- One platform package may declare at most 2 GiB across all downloaded artifacts.
  Extraction separately enforces a generation-wide 2 GiB output limit, 40,000
  filesystem-entry limit, and 1 GiB limit for an individual output file.
- ZIP symbolic links, tar hard links, and other special entries are rejected. A
  relative tar symbolic link may be materialized as a regular file only when
  its complete target chain stays inside that archive and ends at a regular
  file. Absolute, escaping, unresolved, cyclic, and directory targets fail.
- The executable takes no command-line arguments and must pass `uci`,
  `isready`, a legal depth-one search from the starting position, and `quit`.

Importing a recipe from disk or HTTPS does not make it curated. UCI Grabber
shows its publisher and license metadata, marks it Unreviewed, asks for explicit
approval before testing it, and does not add it to the signed catalog.

Approval permits UCI Grabber to execute the downloaded native engine for its
UCI check. That process runs with your user account permissions and has no OS
sandbox in v1. The recipe itself is data-only, but the executable can still
access anything your account can access; import only artifacts you trust.

Recipe documents are UTF-8 JSON and must be no larger than 512 KiB. Unknown
fields are errors so a misspelled security-relevant field cannot silently
change meaning.
