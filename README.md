# Terraforma Memory

**One private memory for Claude Code and Codex.** Your assistants save decisions,
conventions and hard-won fixes, and find them again next session, or from the
other assistant. Everything stays on your machine, in a journal you own.

- **Shared across assistants.** Claude Code and Codex can use the same memory at
  the same time; each call holds the lock only while it runs.
- **Local and private.** A single binary with no runtime dependencies. The MCP
  server talks over stdio and makes no network connections. No account, no telemetry.
- **Tamper-evident history.** Every save is a hash-chained journal entry.
  Updating a memory keeps its earlier versions.
- **Yours to keep.** Export, back up and restore with the same binary. Uninstall
  never deletes your memories.

Built-in assistant memory is usually tied to one tool and one project. Terraforma
Memory is one place both assistants read and write.

## Install (macOS arm64, Linux x86-64)

One command, run inside your project folder:

```sh
curl -fsSL https://raw.githubusercontent.com/janus-ubos-republic/terraforma-memory/main/lander.sh | sh
```

The Lander surveys the machine (system, chip, tools, which assistants are present),
downloads the right archive, checks it against checksums pinned in the script,
installs to `~/.terraforma/terraforma-local-core` and proves the program answers.
It uses no sudo and starts no service. In a terminal it then asks whether to connect
Claude Code and Codex; it changes their configuration only if you say yes, through
their own `mcp add` commands. To look first: download `lander.sh`, read it, and run
`sh lander.sh --dry-run`. Options: `--connect claude,codex|all|none`, `--project NAME`.

### Manual install

Download the archive for your platform from the [releases page](https://github.com/janus-ubos-republic/terraforma-memory/releases), then:

```sh
curl -fsSLO https://github.com/janus-ubos-republic/terraforma-memory/releases/download/v0.3.0/terraforma-local-core-0.3.0-macos-arm64.tar.gz   # or ...-linux-x86-64.tar.gz
tar -xzf terraforma-local-core-0.3.0-*.tar.gz
sh terraforma-local-core-0.3.0-*/install.sh
```

The installer verifies the checksum of every file before installing to
`~/.terraforma/terraforma-local-core`, starts no service and edits no config.
It prints the exact commands to connect your assistants. Downloading with `curl`
(not a browser) avoids the macOS quarantine prompt for this unsigned binary.
Linux needs glibc 2.30 or newer (Ubuntu 20.04+, Debian 11+).

## Connect

Use one memory directory per project. It is created on first use, private to your user.

```sh
# Claude Code (run inside your project)
claude mcp add --scope local terraforma-memory -- \
  ~/.terraforma/terraforma-local-core/bin/terraforma-local-core mcp ~/.terraforma/memory/my-project
```

```toml
# Codex: ~/.codex/config.toml, then restart Codex
[mcp_servers.terraforma-memory]
command = "/Users/you/.terraforma/terraforma-local-core/bin/terraforma-local-core"
args = ["mcp", "/Users/you/.terraforma/memory/my-project"]
```

Point both at the same directory to share one memory.

## What your assistant gets

| Tool | Use |
|---|---|
| `remember` | Save or update a memory (title + text, up to 16 KB). Same title = new version. |
| `recall` | Search memories by words. Put names, paths, error messages and identifiers in the query. |
| `read_memory` | Read one memory in full. |
| `recent` | List the latest memories: a good start for a session. |
| `memory_status` | Count, limits and the journal's integrity digest. |

Optional session briefing (Markdown of the newest memories):
`terraforma-local-core briefing ~/.terraforma/memory/my-project 10`

## Honest limits

- Search matches words, not meaning: a reworded question may miss a memory
  that uses different words. Ask with the specific terms you expect in it.
- Up to 5,000 memories per directory, 16 KB each. Saving one at a time is fast
  (roughly 0.1–0.25 s per call at 5,000); bulk-importing thousands is slow.
- macOS arm64 and Linux x86-64 only. The macOS binary is not yet signed.

## Support the project

Terraforma Memory is free and open source, and every feature works without paying.
If it saves you time, **Founding supporter licenses** open soon: €19 one-time for the
first 100 supporters, then €29. They fund development by an independent builder.

## License

Apache-2.0. See [LICENSE](LICENSE). Build and qualification notes:
[docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).
