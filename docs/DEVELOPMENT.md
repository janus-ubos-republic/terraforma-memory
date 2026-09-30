# Terraforma Memory — development notes

Customer documentation is in the top-level README. This file holds build,
qualification and internal-format notes for contributors.

## Terraforma Memory (v0.3.0)

Local, private, tamper-evident memory for AI coding assistants. Claude Code and
Codex connect over MCP (stdio) and get five tools: `remember`, `recall`,
`read_memory`, `recent` and `memory_status`. Every save is a hash-chained
journal entry; updating a memory keeps its earlier versions. Nothing leaves the
machine. Several assistants can share one memory: the estate is locked only for
the duration of a single call, and a busy estate is retried for up to 5 s.

```sh
# Claude Code (the estate directory is created on first use)
claude mcp add --scope local terraforma-memory -- \
  /path/to/terraforma-local-core mcp "$HOME/.terraforma/memory/my-project"

# Codex (~/.codex/config.toml)
[mcp_servers.terraforma-memory]
command = "/path/to/terraforma-local-core"
args = ["mcp", "/Users/me/.terraforma/memory/my-project"]

# Optional session-start briefing (Markdown of the most recent memories)
terraforma-local-core briefing "$HOME/.terraforma/memory/my-project" 10
```

Limits: 5,000 memories, 16 KB each, 50,000 journal events, 32 MB journal. Search
is exact-word and case-insensitive, not semantic. MCP writes record a UTC time;
CLI and browser writes stay undated so v0.2 journals and their digests replay
unchanged.

A small headless Rust library, CLI and embedded local browser workspace.
It reuses three **unchanged** modules from committed Terraforma source, recorded
in `lineage.json`. It runs without Tauri, Node, Python, models, external network access,
daemons, a UBOS checkout or founder configuration. Python is used only by the
developer qualification harness.

This is the selected local packaging boundary, not a second live Republic
daemon. No existing runtime is replaced. The `publish = false` package
is a local qualification candidate; the full installer and public distribution
are not delivered by this crate.

## Reused and added

| Part | Treatment |
|---|---|
| `src/inherited/cell.rs` | Exact upstream DataCell, CellKind and Tooth definitions. |
| `src/inherited/monolith.rs` | Exact upstream genesis, rings, expansion, collapse and coordinate-hash operations. |
| `src/inherited/governor.rs` | Exact upstream admission check. The adapter supplies explicit bounds; the old pressure counter is not advertised as a clock-based rate limiter. |
| `src/lib.rs` | New versioned journal adapter, retained UTF-8 documents, replay, full-state SHA-256, file locking, bounded admission and export/restore. |
| `src/main.rs` | Explicit-path CLI. No default founder directory or automatic service installation. |
| `src/search.rs` | Derived lexical adapter using the existing Desk Drive ranking, exact word/number matching, explicit partial results and duplicate-text grouping. |
| `src/server.rs`, `web/` | Embedded browser interface over the same locked estate; loopback only, bounded typed requests, per-start session token and origin/host checks. |

The old daemon WAL has no expansion/collapse events and silently skips malformed
entries. It is preserved as historical evidence and **not accepted as this v1
journal**. Historical migration requires a separate loss-accounted importer.
There is no implicit use of private live data or old daemon configuration.

## Build and qualify

```sh
cargo build --release --offline --locked -j 1
cargo test --lib --release --offline --locked -j 1
python3 scripts/qualify.py --binary target/release/terraforma-local-core --output-parent /tmp
python3 scripts/qualify_browser.py --binary target/release/terraforma-local-core --output-parent /tmp
```

## Build a local macOS package

On an arm64 Mac, build the verified local binary, then create a self-contained
terminal package in an existing empty delivery directory:

```sh
cargo build --release --offline --locked -j 1
mkdir -p ./deliveries
python3 scripts/package_macos.py \
  --binary target/release/terraforma-local-core \
  --output-parent ./deliveries
```

The command accepts only a Mach-O arm64 binary and takes the version from
`Cargo.toml`. It produces a gzip archive with the binary, a manifest, payload
checksums, and terminal `install.sh`, `uninstall.sh`, `update.sh` and
`rollback.sh` scripts. A Linux x86-64 package is built the same way from a
cross-compiled binary (needs glibc 2.30+, e.g. Ubuntu 20.04, Debian 11):

```sh
cargo zigbuild --release --target x86_64-unknown-linux-gnu
python3 scripts/package_linux.py \
  --binary target/x86_64-unknown-linux-gnu/release/terraforma-local-core \
  --output-parent ./deliveries
```

Extract and install. With no argument the prefix is
`$HOME/.terraforma/terraforma-local-core`; an explicit prefix must be a new
directory whose final component is `terraforma-local-core`:

```sh
tar -xzf ./deliveries/terraforma-local-core-0.3.0-macos-arm64.tar.gz
sh ./terraforma-local-core-0.3.0-macos-arm64/install.sh
```

The installer verifies every packaged payload before creating the destination,
never starts a service and never edits assistant configuration. It prints the
exact `claude mcp add` command and Codex `config.toml` block for the installed
binary. Memory directories live outside the installation directory and are
created on first use (with private, user-only permissions). The supplied
uninstaller checks the original payload hashes and refuses unexpected files; it
only removes the package directory and never removes a memory directory.

This is a local packaging rehearsal, not signed distribution, a customer
installation, release approval, or a claim that the complete Terraforma estate
is restored.

## Build a local Linux package

On the selected x86_64 GNU/Linux host, use the same source package helper with
the qualified ELF candidate:

```sh
mkdir -p ./deliveries
python3 scripts/package_linux.py \
  --binary ./terraforma-local-core \
  --output-parent ./deliveries
tar -xzf ./deliveries/terraforma-local-core-0.2.2-linux-x86-64.tar.gz
sh ./terraforma-local-core-0.2.2-linux-x86-64/install.sh \
  "$HOME/Applications/terraforma-local-core"
```

The Linux builder refuses anything other than an ELF x86-64 binary. Its package
uses `sha256sum` when `shasum` is unavailable and otherwise follows the same
explicit-install and estate-preserving uninstall contract as macOS. This is an
isolated Linux candidate rehearsal, not broad Linux compatibility or a release.

The developer-only `scripts/qualify_ui.cjs` also exercises the real file chooser,
save/search/edit, stale-tab review, download, checkpoint, restart, isolated backup
restore, inert source markup and narrow-screen navigation in Chromium. It takes
explicit Playwright-module, Chromium-executable, binary and output-parent paths;
these are test dependencies and are not needed by the shipped executable.

The harness copies the binary out of the repository, gives it an isolated HOME
and working directory, uses only synthetic files, and retains a qualification
receipt with exact source and executable hashes. It currently tests macOS/Unix
locking and modes. A successful M4 run is not Linux, Windows or customer proof.
Do not run `cargo fmt` over inherited modules: their byte identity is intentional.
Format the adapter with `rustfmt --edition 2021 --config skip_children=true src/lib.rs src/main.rs src/search.rs src/server.rs`.

## CLI contract

Arguments are positional; paths containing spaces must be quoted. Parent
directories must already exist. Init, export and restore refuse existing output
directories, including empty ones.

```text
terraforma-local-core version
terraforma-local-core init ESTATE ESTATE_ID
terraforma-local-core put ESTATE RECORD_ID TITLE UTF8_FILE
terraforma-local-core get ESTATE RECORD_ID
terraforma-local-core query ESTATE "SEARCH WORDS"
terraforma-local-core serve ESTATE [PORT]
terraforma-local-core status ESTATE
terraforma-local-core expand ESTATE
terraforma-local-core collapse ESTATE EARLIER_RING
terraforma-local-core export ESTATE NEW_EXPORT_DIRECTORY
terraforma-local-core restore EXPORT_DIRECTORY NEW_ESTATE
terraforma-local-core backup ESTATE NEW_BACKUP_FILE
terraforma-local-core restore-backup BACKUP_FILE NEW_ESTATE
terraforma-local-core verify-desk-replay SEALED_REPLAY_PACKAGE
```

`verify-desk-replay` reads one caller-supplied, manifest-bound display-only M4 Desk replay package. It verifies every sealed file, the local-core backup, retained status/boundary records and the four non-action stops. It never opens an M4 owner or changes an estate. `get` is exact identity retrieval. `query` derives its results from retained
records using the existing Drive's word/title/phrase ranking. `put` retains the chosen
document's contents and SHA-256; later external edits do not silently change the
record. Identical admission is a no-op. A changed record gets a revision in the
current ring. Collapse drops outer-ring revisions and restores earlier visible
contents. The journal retains the full history, including the collapse itself.
Multiple updates within one ring retain their history in the journal but ring
collapse only selects an earlier ring, not an arbitrary event or wall-clock time.

Limits in the source-bound v1 configuration: 128 currently visible records,
16 KiB UTF-8 content per admission, 256-byte titles, 80-byte restricted IDs,
64 expansion levels, 2,048 journal events and an 8 MiB journal. This is a small
qualification estate, not a scale claim or the final customer corpus capacity.
Document payloads are retained in the versioned journal/record representation;
the historical DataCell offset field is not used as a memory-mapped payload store.

## Persistence and proof boundary

Every mutation validates the complete existing journal, checks the transition,
appends a typed, sequenced, hash-linked event and calls `sync_all` before returning
a committed receipt. A stable lock file excludes cooperating concurrent writers.
An append or sync error invalidates the open handle until it is reopened.
Malformed, missing, reordered, duplicate, unknown-schema or incomplete entries
cause refusal; they never silently become an empty world.

The authoritative **world digest** hashes the deterministic v1 JSON representation
of estate identity, ring membership and retained record versions, including text.
The inherited 64-bit coordinate root remains a separately labelled diagnostic.
The journal-head digest identifies action history; it can change even when a
collapse reproduces an earlier world digest.

Export contains retained documents through the complete journal and a manifest
binding its bytes and reconstructed world. Restore checks both before creating
a new estate. It does not back up unrelated originals elsewhere on disk.
These are integrity checks, not signatures or protection against an owner who
rewrites both data and hashes. A removed complete journal suffix needs an
independently retained head or export manifest to be detected.

Qualified now: synthetic process-separated operation on M4, content-preserving
ring replay/collapse, export/restore, corruption refusal, writer exclusion,
capacity limits and an injected write failure. `sync_all` is implemented, but
power-loss guarantees, hostile filesystem races and crash fault injection have
not been established. Interrupted creation/export may leave an incomplete new
directory; it is not acknowledged as success or silently overwritten on retry.

Remaining product work: complete installer lifecycle, source-folder watching and
richer document parsers, signed distribution,
Linux and native Windows qualification, and an unrelated user's installation.
The broad Recovery road and original runtime lineage remain separately open.

## Try the browser workspace

```sh
./terraforma-local-core init ./my-workspace my-workspace
./terraforma-local-core serve ./my-workspace
```

Open the printed `http://127.0.0.1:PORT` URL. The default selects an available port;
an explicit port is optional. Choose **Try a sample**, review and save it, then
search for **project 7** and open the saved source. Or select a supported UTF-8
text file (at most 16 KiB), review its text and save. Unedited UTF-8 BOM and CRLF
bytes are preserved through the preview. Text edits create the user's revised
copy. PDF, DOCX and OCR are not supported. Files are not watched; replacing saved
text requires an explicit new selection or edit.

Create a checkpoint before changing the budget. Search the new number, then
restore the checkpoint to recover the earlier visible text. **Download backup**
exports all retained journal history; it is not encrypted and does not back up
the source files elsewhere on disk. Recover that download into a new directory:

```sh
./terraforma-local-core restore-backup my-workspace.terraforma-backup.json ./recovered
./terraforma-local-core serve ./recovered
```

Use **Stop workspace** to close the listener and release its estate lock. The
same `serve` command reopens the retained state. This is a foreground process,
with no service installation or autostart. CLI operations against the same
estate are refused while the server holds its lock.

Search responses bind corpus/world, journal head, query, engine version, coverage,
limits and exact source hashes into a reproducible `result_sha256`; elapsed time
is outside that digest. The API and CLI use the same function. Results are
lexical evidence, not generated answers. Repeated identical text is shown once
with all saved source IDs; full coverage ranks before partial coverage. This
adapter is not the entire historical graph Drive, Flywheel or Hologram.

The server accepts only fixed embedded assets and a typed JSON API; it cannot
browse arbitrary filesystem paths. API reads and writes require a per-process
token and the exact loopback origin. Mutations also require the journal head the
user viewed, preventing stale-tab overwrites. Source opening checks the hash
from the search and refuses silently substituted text. Four bounded connections,
request deadlines, header/body limits, strict framing, CSP, no-store and text-only
rendering bound the local interface. These are local browser isolation controls,
not OS-user authentication, encryption, multiuser security or public-hosting
qualification. Do not expose this listener through a tunnel or reverse proxy.
