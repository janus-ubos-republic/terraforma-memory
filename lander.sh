#!/bin/sh
# Terraforma Lander: survey this machine, install Terraforma Memory, connect your assistants.
# Usage: sh lander.sh [--dry-run] [--connect claude,codex|all|none|ask] [--project NAME] [--from DIR]
# No sudo, no service, no network access except downloading the release from GitHub.
set -eu

VERSION=0.3.0
RELEASE_URL="https://github.com/janus-ubos-republic/terraforma-memory/releases/download/v$VERSION"
SHA_MACOS_ARM64=fe0347e5e4ddfc9d1c60557dc67289416a8625ae317c0331686a67e645355472
SHA_LINUX_X86_64=cfd7e9d33ab94c5a29c7ac7af2658ece94ef78be780c71907de4dfe478b95a42
MIN_GLIBC_MINOR=30

say() { printf '%s\n' "$*"; }
fail() { printf '%s\n' "terraforma lander: $*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

usage() {
  cat <<EOF
Terraforma Lander $VERSION

  sh lander.sh [options]

  --dry-run          survey the machine and print the plan; change nothing
  --connect LIST     claude, codex, all, none or ask (default: ask in a terminal, else none)
  --project NAME     memory folder name (default: the current folder's name)
  --from DIR         install from release archives already in DIR instead of downloading
  -h, --help         show this help
EOF
}

digest() {
  if have shasum; then shasum -a 256 "$1" | awk '{print $1}'
  elif have sha256sum; then sha256sum "$1" | awk '{print $1}'
  else fail "no SHA-256 command (shasum or sha256sum) is available"; fi
}

fetch() {
  if have curl; then curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"
  elif have wget; then wget -q -O "$2" "$1"
  else fail "neither curl nor wget is available"; fi
}

glibc_version() {
  if have getconf && getconf GNU_LIBC_VERSION >/dev/null 2>&1; then
    getconf GNU_LIBC_VERSION | awk '{print $2}'
  elif have ldd; then
    ldd --version 2>&1 | awk 'NR==1 && /GLIBC|GNU libc/ {print $NF}'
  fi
}

can_ask() { ( : </dev/tty ) 2>/dev/null; }

ask() {
  printf '%s [y/N] ' "$1" >/dev/tty
  IFS= read -r answer </dev/tty || answer=
  case "$answer" in y|Y|yes|YES) return 0 ;; *) return 1 ;; esac
}

wants() {
  case ",$CONNECT," in *",all,"*|*",$1,"*) return 0 ;; esac
  return 1
}

main() {
  DRY_RUN=0 CONNECT= PROJECT= FROM=
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --dry-run) DRY_RUN=1 ;;
      --connect) [ "$#" -ge 2 ] || fail "--connect needs a value"; CONNECT=$2; shift ;;
      --project) [ "$#" -ge 2 ] || fail "--project needs a value"; PROJECT=$2; shift ;;
      --from) [ "$#" -ge 2 ] || fail "--from needs a value"; FROM=$2; shift ;;
      -h|--help) usage; exit 0 ;;
      *) usage >&2; fail "unknown option: $1" ;;
    esac
    shift
  done
  [ -n "${HOME:-}" ] || fail "HOME is not set"
  case "$CONNECT" in
    ''|ask|none|all) ;;
    *) for item in $(printf '%s' "$CONNECT" | tr ',' ' '); do
         case "$item" in claude|codex) ;; *) fail "--connect accepts claude, codex, all, none or ask" ;; esac
       done ;;
  esac
  if [ -z "$PROJECT" ]; then
    PROJECT=$(basename "$PWD")
    [ "$PWD" != "$HOME" ] || PROJECT=my-project
  fi
  case "$PROJECT" in
    ''|.|..|*[!A-Za-z0-9._-]*) fail "project name may only use letters, digits, dot, dash and underscore: $PROJECT" ;;
  esac

  # 1. Survey
  OS=$(uname -s) ARCH=$(uname -m)
  GLIBC= ASSET= EXPECTED= BLOCKER=
  case "$OS/$ARCH" in
    Darwin/arm64) ASSET=macos-arm64 EXPECTED=$SHA_MACOS_ARM64 ;;
    Linux/x86_64|Linux/amd64)
      ASSET=linux-x86-64 EXPECTED=$SHA_LINUX_X86_64
      GLIBC=$(glibc_version || true)
      if [ -z "$GLIBC" ]; then
        BLOCKER="this Linux does not report glibc (musl systems such as Alpine are not supported)"
      else
        major=${GLIBC%%.*} minor=${GLIBC#*.}; minor=${minor%%.*}
        if [ "$major" -lt 2 ] || { [ "$major" -eq 2 ] && [ "$minor" -lt "$MIN_GLIBC_MINOR" ]; }; then
          BLOCKER="glibc $GLIBC is older than 2.$MIN_GLIBC_MINOR (Ubuntu 20.04+ or Debian 11+ is needed)"
        fi
      fi ;;
    *) BLOCKER="no build for $OS $ARCH (available: macOS arm64, Linux x86-64)" ;;
  esac
  if have curl; then DOWNLOADER=curl; elif have wget; then DOWNLOADER=wget; else DOWNLOADER=; fi
  if have shasum; then HASHER=shasum; elif have sha256sum; then HASHER=sha256sum; else HASHER=; fi
  if have claude; then HAS_CLAUDE=yes; else HAS_CLAUDE=no; fi
  if have codex; then HAS_CODEX=yes; else HAS_CODEX=no; fi
  PREFIX="$HOME/.terraforma/terraforma-local-core"
  BIN="$PREFIX/bin/terraforma-local-core"
  ESTATE="$HOME/.terraforma/memory/$PROJECT"
  if [ -e "$PREFIX" ] || [ -L "$PREFIX" ]; then INSTALLED=yes; else INSTALLED=no; fi
  if [ -z "$CONNECT" ]; then if can_ask; then CONNECT=ask; else CONNECT=none; fi; fi
  [ -n "$FROM" ] || [ -n "$DOWNLOADER" ] || BLOCKER=${BLOCKER:-"neither curl nor wget is available"}
  [ -n "$HASHER" ] || BLOCKER=${BLOCKER:-"no SHA-256 command (shasum or sha256sum) is available"}
  have tar || BLOCKER=${BLOCKER:-"tar is not available"}

  say "Terraforma Lander $VERSION"
  say ""
  say "Survey"
  say "  system        $OS $ARCH${GLIBC:+ (glibc $GLIBC)}"
  say "  tools         download: ${DOWNLOADER:-none}, checksum: ${HASHER:-none}"
  say "  Claude Code   $HAS_CLAUDE"
  say "  Codex         $HAS_CODEX"
  say "  installed     $INSTALLED ($PREFIX)"
  say "  memory        $ESTATE"
  say ""
  [ -z "$BLOCKER" ] || fail "cannot install here: $BLOCKER. Nothing was changed."

  ARCHIVE="terraforma-local-core-$VERSION-$ASSET.tar.gz"
  say "Plan"
  if [ "$INSTALLED" = yes ]; then say "  keep the existing install; do not overwrite it"
  elif [ -n "$FROM" ]; then say "  install $ARCHIVE from $FROM"
  else say "  download $ARCHIVE from the v$VERSION GitHub release"; fi
  say "  connect assistants: $CONNECT"
  say ""
  if [ "$DRY_RUN" -eq 1 ]; then say "Dry run: nothing was changed."; exit 0; fi

  # 2. Fetch and install
  if [ "$INSTALLED" = no ]; then
    WORK=$(mktemp -d "${TMPDIR:-/tmp}/terraforma-lander.XXXXXX")
    trap 'rm -rf "$WORK"' EXIT
    trap 'exit 1' HUP INT TERM
    if [ -n "$FROM" ]; then
      [ -f "$FROM/$ARCHIVE" ] || fail "$FROM/$ARCHIVE does not exist"
      cp "$FROM/$ARCHIVE" "$WORK/$ARCHIVE"
    else
      say "Downloading $ARCHIVE"
      fetch "$RELEASE_URL/$ARCHIVE" "$WORK/$ARCHIVE" || fail "download failed. Nothing was changed."
    fi
    ACTUAL=$(digest "$WORK/$ARCHIVE")
    [ "$ACTUAL" = "$EXPECTED" ] || fail "checksum mismatch for $ARCHIVE (expected $EXPECTED, got $ACTUAL). Nothing was changed."
    say "Checksum verified"
    tar -xzf "$WORK/$ARCHIVE" -C "$WORK"
    sh "$WORK/terraforma-local-core-$VERSION-$ASSET/install.sh" "$PREFIX" >/dev/null
    say "Installed $BIN"
  fi
  [ -x "$BIN" ] || fail "$BIN is missing or not executable; the existing install looks damaged"

  # 3. Prove the program runs on this machine, in a throwaway folder
  PROBE=$(mktemp -d "${TMPDIR:-/tmp}/terraforma-lander-probe.XXXXXX")
  REPLY=$(printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"terraforma-lander","version":"'"$VERSION"'"}}}' \
    | "$BIN" mcp "$PROBE/estate" 2>&1 || true)
  rm -rf "$PROBE"
  case "$REPLY" in
    *'"result"'*) say "Self-test passed: the memory server answers on this machine" ;;
    *) fail "the installed program did not answer a test request: $REPLY" ;;
  esac

  # 4. Connect assistants, only when asked
  CONNECTED=
  if [ "$CONNECT" = ask ]; then
    CONNECT=
    if [ "$HAS_CLAUDE" = yes ] && ask "Connect Claude Code for this folder ($PWD)?"; then CONNECT=claude; fi
    if [ "$HAS_CODEX" = yes ] && ask "Connect Codex (applies to all your Codex projects)?"; then CONNECT="${CONNECT:+$CONNECT,}codex"; fi
    [ -n "$CONNECT" ] || CONNECT=none
  fi
  if wants claude; then
    if [ "$HAS_CLAUDE" = no ]; then say "Claude Code: not found on this machine, skipped"
    elif claude mcp add --scope local terraforma-memory -- "$BIN" mcp "$ESTATE" >/dev/null 2>&1; then
      say "Claude Code: connected for $PWD"; CONNECTED="${CONNECTED:+$CONNECTED, }Claude Code"
    else say "Claude Code: could not connect (it may already be connected here). Command to run yourself:"
      say "  claude mcp add --scope local terraforma-memory -- \"$BIN\" mcp \"$ESTATE\""; fi
  fi
  if wants codex; then
    if [ "$HAS_CODEX" = no ]; then say "Codex: not found on this machine, skipped"
    elif codex mcp add terraforma-memory -- "$BIN" mcp "$ESTATE" >/dev/null 2>&1; then
      say "Codex: connected; restart Codex to load it"; CONNECTED="${CONNECTED:+$CONNECTED, }Codex"
    else say "Codex: could not connect (it may already be connected). Command to run yourself:"
      say "  codex mcp add terraforma-memory -- \"$BIN\" mcp \"$ESTATE\""; fi
  fi

  # 5. Report
  say ""
  say "Done"
  say "  program     $BIN"
  say "  memory      $ESTATE (created on first use; uninstall never deletes it)"
  say "  connected   ${CONNECTED:-nothing}"
  if [ -z "$CONNECTED" ]; then
    say ""
    say "To connect an assistant, run inside your project:"
    say "  claude mcp add --scope local terraforma-memory -- \"$BIN\" mcp \"$ESTATE\""
    say "  codex mcp add terraforma-memory -- \"$BIN\" mcp \"$ESTATE\""
  fi
  say ""
  say "Uninstall: sh \"$PREFIX/share/uninstall.sh\" \"$PREFIX\""
}

main "$@"
