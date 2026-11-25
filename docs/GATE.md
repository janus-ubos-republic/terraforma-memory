# The Gate (v0)

The Gate lets more than one assistant, each with its own identity, use one
Terraforma Memory over HTTP, and lets you revoke one of them without touching the
others. It is a small front door in the same binary. No new dependencies, no
outbound connections, no account.

## Use

```sh
BIN=~/.terraforma/terraforma-local-core/bin/terraforma-local-core
GATE=~/.terraforma/gate/my-project            # holds principals.json and audit.jsonl (private, 0700)

$BIN principal-add $GATE assistant-a write    # prints a token ONCE; store it in that assistant's config
$BIN principal-add $GATE auditor read         # a read-only principal
$BIN principal-list $GATE
$BIN gate-serve ~/.terraforma/memory/my-project $GATE 8765   # listens on 127.0.0.1:8765/mcp
$BIN principal-revoke $GATE assistant-a       # takes effect on that principal's very next request
```

Point an assistant at `http://127.0.0.1:8765/mcp` with the header
`Authorization: Bearer <token>`. The MCP tools are the same five as the local
server: `remember`, `recall`, `read_memory`, `recent`, `memory_status`.

## Reaching it from another machine (tested with an SSH tunnel)

```sh
# on the server: start the Gate (loopback only)
$BIN gate-serve ~/.terraforma/memory/my-project $GATE 8765
# on your laptop: forward the port, then point the assistant at http://127.0.0.1:8765/mcp
ssh -N -L 8765:127.0.0.1:8765 your-server
```

`python3 scripts/qualify_w1.py <binary> --ssh your-server --remote-bin <path-on-server>`
runs the whole check this way and restores the server's backup on your own machine.

## What it does

- One bearer token per principal. Only the SHA-256 of a token is stored.
- Scopes: `write` (all five tools) or `read` (`remember` is refused).
- Revoke is immediate: the principal file is read on every request, and the Gate keeps no sessions.
- Missing, wrong or revoked token: HTTP 401. A request carrying a browser `Origin` header: 403.
- An audit log (`audit.jsonl`) records time, principal, MCP method, tool and outcome. It never records memory text.

## What it does not do (read this before relying on it)

- **No encryption in transit.** It binds to loopback only. To reach it from another machine, put your own TLS tunnel or reverse proxy in front. That setup is yours and is not covered by the tests below.
- **Revocation stops future requests.** It cannot take back what a principal already read or saved.
- **No per-principal attribution inside the memory itself.** Saved memories do not record which principal wrote them; the audit log does.
- **One estate per Gate.** No per-project permissions inside an estate.
- The full check was run with the Gate on a Linux server reached through an SSH tunnel and the backup restored on a Mac. It has not been run across different hosting providers, behind a TLS reverse proxy, or by anyone other than its author. Synthetic data only.

## Check it yourself

`python3 scripts/qualify_w1.py <path-to-binary> [receipt.json]` runs 21 checks (22 in remote mode):
one assistant saves a decision and another reads the exact text; unauthenticated,
wrong-token, browser-origin and read-only writes are refused; revoking one
assistant refuses it on the next call while the other keeps working; the backup
restores into a clean directory with the same digest, and a tampered backup is
refused; the audit log names each principal and contains no memory text.
