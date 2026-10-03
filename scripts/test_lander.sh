#!/bin/sh
# Tests for lander.sh. Runs every case in a throwaway HOME with fake assistant commands.
# Usage: sh scripts/test_lander.sh DIST_DIR [SHELL]   (DIST_DIR holds the release archives)
set -eu

[ "$#" -ge 1 ] || { echo "usage: test_lander.sh DIST_DIR [SHELL]" >&2; exit 2; }
DIST=$(CDPATH= cd "$1" && pwd)
RUN=${2:-sh}
ROOT=$(CDPATH= cd "$(dirname "$0")/.." && pwd)
LANDER="$ROOT/lander.sh"
BASE=$(mktemp -d "${TMPDIR:-/tmp}/lander-test.XXXXXX")
trap 'rm -rf "$BASE"' EXIT
PASSED=0 FAILED=0

ok() { PASSED=$((PASSED + 1)); printf 'ok    %s\n' "$1"; }
bad() { FAILED=$((FAILED + 1)); printf 'FAIL  %s\n' "$1"; sed 's/^/      /' "$BASE/out" 2>/dev/null || true; }
check() { if "$@"; then return 0; else return 1; fi; }

# Fresh HOME, project folder and fake commands for one case.
setup() {
  CASE="$BASE/$1"; mkdir -p "$CASE/home" "$CASE/proj/alpha" "$CASE/bin"
  for name in claude codex; do
    printf '#!/bin/sh\nprintf "%%s\\n" "$*" >> "%s/%s.calls"\n' "$CASE" "$name" > "$CASE/bin/$name"
    chmod +x "$CASE/bin/$name"
  done
}
# run CASE_NAME [lander args...]: output in $BASE/out, exit code in $RC
run() {
  set +e
  ( cd "$CASE/proj/alpha" && HOME="$CASE/home" PATH="$CASE/bin:$PATH" "$RUN" "$LANDER" "$@" ) >"$BASE/out" 2>&1 </dev/null
  RC=$?
  set -e
}
PREFIX_OF() { printf '%s' "$CASE/home/.terraforma/terraforma-local-core"; }

setup dry
run --dry-run --from "$DIST"
if [ "$RC" -eq 0 ] && grep -q 'Dry run: nothing was changed' "$BASE/out" && [ ! -e "$CASE/home/.terraforma" ]; then
  ok "dry run surveys and changes nothing"; else bad "dry run surveys and changes nothing"; fi

setup install
run --connect none --from "$DIST"
if [ "$RC" -eq 0 ] && [ -x "$(PREFIX_OF)/bin/terraforma-local-core" ] && grep -q 'Self-test passed' "$BASE/out" \
   && [ ! -e "$CASE/claude.calls" ] && [ ! -e "$CASE/codex.calls" ] && [ ! -e "$CASE/home/.terraforma/memory" ]; then
  ok "install verifies, self-tests, connects nothing, creates no memory"; else bad "install verifies, self-tests, connects nothing, creates no memory"; fi

run --connect none --from "$DIST"
if [ "$RC" -eq 0 ] && grep -q 'keep the existing install' "$BASE/out" && grep -q 'Self-test passed' "$BASE/out"; then
  ok "second run keeps the existing install"; else bad "second run keeps the existing install"; fi

setup default
run --from "$DIST"
if [ "$RC" -eq 0 ] && [ ! -e "$CASE/claude.calls" ] && [ ! -e "$CASE/codex.calls" ] && grep -q 'connected   nothing' "$BASE/out"; then
  ok "without a terminal the default connects nothing"; else bad "without a terminal the default connects nothing"; fi

setup connect
run --connect all --from "$DIST"
BIN="$(PREFIX_OF)/bin/terraforma-local-core"; ESTATE="$CASE/home/.terraforma/memory/alpha"
if [ "$RC" -eq 0 ] \
   && [ "$(cat "$CASE/claude.calls")" = "mcp add --scope local terraforma-memory -- $BIN mcp $ESTATE" ] \
   && [ "$(cat "$CASE/codex.calls")" = "mcp add terraforma-memory -- $BIN mcp $ESTATE" ] \
   && grep -q 'connected   Claude Code, Codex' "$BASE/out"; then
  ok "connect all calls both assistants with the project memory folder"; else bad "connect all calls both assistants with the project memory folder"; fi

setup project
run --connect claude --project "shop-1" --from "$DIST"
if [ "$RC" -eq 0 ] && grep -q "memory/shop-1" "$CASE/claude.calls" && [ ! -e "$CASE/codex.calls" ]; then
  ok "--project names the memory folder; only the chosen assistant is connected"; else bad "--project names the memory folder; only the chosen assistant is connected"; fi

setup badname
run --connect none --project "../etc" --from "$DIST"
if [ "$RC" -ne 0 ] && [ ! -e "$CASE/home/.terraforma" ]; then
  ok "unsafe project name is refused"; else bad "unsafe project name is refused"; fi

setup missing
rm "$CASE/bin/claude" "$CASE/bin/codex"
( cd "$CASE/proj/alpha" && HOME="$CASE/home" PATH="$CASE/bin:/usr/bin:/bin" "$RUN" "$LANDER" --connect all --from "$DIST" ) >"$BASE/out" 2>&1 </dev/null && RC=0 || RC=$?
if [ "$RC" -eq 0 ] && grep -q 'Claude Code: not found' "$BASE/out" && grep -q 'Codex: not found' "$BASE/out"; then
  ok "missing assistants are reported, install still succeeds"; else bad "missing assistants are reported, install still succeeds"; fi

setup tamper
mkdir "$CASE/dist"
for f in "$DIST"/terraforma-local-core-*.tar.gz; do cp "$f" "$CASE/dist/"; printf 'x' >> "$CASE/dist/$(basename "$f")"; done
run --connect none --from "$CASE/dist"
if [ "$RC" -ne 0 ] && grep -q 'checksum mismatch' "$BASE/out" && [ ! -e "$(PREFIX_OF)" ]; then
  ok "altered archive is refused and nothing is installed"; else bad "altered archive is refused and nothing is installed"; fi

setup platform
printf '#!/bin/sh\ncase "$1" in -s) echo Linux ;; -m) echo riscv64 ;; esac\n' > "$CASE/bin/uname"; chmod +x "$CASE/bin/uname"
run --connect none --from "$DIST"
if [ "$RC" -ne 0 ] && grep -q 'no build for Linux riscv64' "$BASE/out" && [ ! -e "$CASE/home/.terraforma" ]; then
  ok "unsupported machine is refused and nothing is changed"; else bad "unsupported machine is refused and nothing is changed"; fi

setup badflag
run --connect everyone --from "$DIST"
if [ "$RC" -ne 0 ] && [ ! -e "$CASE/home/.terraforma" ]; then
  ok "unknown --connect value is refused"; else bad "unknown --connect value is refused"; fi

printf '\n%s passed, %s failed (shell: %s)\n' "$PASSED" "$FAILED" "$RUN"
[ "$FAILED" -eq 0 ]
