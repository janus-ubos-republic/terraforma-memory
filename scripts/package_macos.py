#!/usr/bin/env python3
"""Build a self-contained, terminal-installable macOS arm64 local-core package."""
from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import shutil
import stat
import subprocess
import tarfile
import tempfile
from pathlib import Path

PACKAGE_SCHEMA = "ubos.terraforma-local-core-package.v1"
PACKAGE_NAME = "terraforma-local-core"
TARGET = "macos-arm64"
PAYLOADS = ("bin/terraforma-local-core", "manifest.json", "uninstall.sh", "update.sh", "rollback.sh")


def cargo_version() -> str:
    """The package version is the crate version, so archives cannot drift from the binary."""
    for line in (Path(__file__).resolve().parent.parent / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        if line.startswith("version = "):
            return line.split('"')[1]
    raise ValueError("Cargo.toml has no package version")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write_executable(path: Path, text: str) -> None:
    path.write_text(text, encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def normalized_tarinfo(info: tarfile.TarInfo) -> tarfile.TarInfo:
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    info.mtime = 0
    return info


def installer_script() -> str:
    return """#!/bin/sh
set -eu
fail() { printf '%s\\n' "terraforma-local-core install: $*" >&2; exit 1; }
[ "$#" -le 1 ] || fail "usage: install.sh [PREFIX/terraforma-local-core]  (default: \\$HOME/.terraforma/terraforma-local-core)"
if [ "$#" -eq 1 ]; then PREFIX=$1; else
  [ -n "${HOME:-}" ] || fail "HOME is not set; pass PREFIX/terraforma-local-core explicitly"
  PREFIX="$HOME/.terraforma/terraforma-local-core"
fi
case "$PREFIX" in ''|/|.|..) fail "unsafe prefix" ;; esac
[ "$(basename "$PREFIX")" = "terraforma-local-core" ] || fail "prefix must end in terraforma-local-core"
[ ! -e "$PREFIX" ] && [ ! -L "$PREFIX" ] || fail "destination already exists"
SOURCE=$(CDPATH= cd "$(dirname "$0")" && pwd)
digest() {
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{print $1}';
  elif command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}';
  else fail "no SHA-256 command is available"; fi
}
verify_payload() {
  while IFS= read -r line; do
    expected=${line%% *}
    relative=${line#*  }
    actual=$(digest "$SOURCE/$relative")
    [ "$actual" = "$expected" ] || fail "payload digest mismatch for $relative"
  done < "$SOURCE/SHA256SUMS"
}
verify_payload
PARENT=$(dirname "$PREFIX")
mkdir -p "$PARENT"
STAGE="$PARENT/.terraforma-local-core.install.$$"
[ ! -e "$STAGE" ] && [ ! -L "$STAGE" ] || fail "temporary destination exists"
mkdir "$STAGE"
mkdir "$STAGE/bin" "$STAGE/share"
cp "$SOURCE/bin/terraforma-local-core" "$STAGE/bin/terraforma-local-core"
chmod 700 "$STAGE/bin/terraforma-local-core"
cp "$SOURCE/manifest.json" "$STAGE/share/manifest.json"
cp "$SOURCE/SHA256SUMS" "$STAGE/share/SHA256SUMS"
cp "$SOURCE/uninstall.sh" "$STAGE/share/uninstall.sh"
cp "$SOURCE/update.sh" "$STAGE/share/update.sh"
cp "$SOURCE/rollback.sh" "$STAGE/share/rollback.sh"
chmod 700 "$STAGE/share/uninstall.sh" "$STAGE/share/update.sh" "$STAGE/share/rollback.sh"
mv "$STAGE" "$PREFIX"
PREFIX=$(CDPATH= cd "$PREFIX" && pwd)
BIN="$PREFIX/bin/terraforma-local-core"
ESTATE="${HOME:-/path/to/home}/.terraforma/memory/my-project"
cat <<EOF
Installed $BIN

Connect Terraforma Memory (one memory directory per project; created on first use,
kept outside this prefix, never removed by uninstall or update):

  Claude Code (run inside your project):
    claude mcp add --scope local terraforma-memory -- "$BIN" mcp "$ESTATE"

  Codex (~/.codex/config.toml, then restart Codex):
    [mcp_servers.terraforma-memory]
    command = "$BIN"
    args = ["mcp", "$ESTATE"]

  Several assistants may share one memory directory.
  Session briefing: "$BIN" briefing "$ESTATE" 10
  Uninstall:        sh "$PREFIX/share/uninstall.sh" "$PREFIX"
EOF
"""


def uninstaller_script() -> str:
    return """#!/bin/sh
set -eu
fail() { printf '%s\\n' "terraforma-local-core uninstall: $*" >&2; exit 1; }
[ "$#" -eq 1 ] || fail "usage: uninstall.sh PREFIX/terraforma-local-core"
PREFIX=$1
case "$PREFIX" in ''|/|.|..) fail "unsafe prefix" ;; esac
[ "$(basename "$PREFIX")" = "terraforma-local-core" ] || fail "prefix must end in terraforma-local-core"
[ -d "$PREFIX" ] || fail "installed prefix is absent"
digest() {
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{print $1}';
  elif command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}';
  else fail "no SHA-256 command is available"; fi
}
for relative in bin bin/terraforma-local-core share share/manifest.json share/SHA256SUMS share/uninstall.sh share/update.sh share/rollback.sh; do
  [ -e "$PREFIX/$relative" ] || fail "required installed path is absent: $relative"
done
COUNT=$(find "$PREFIX" -print | wc -l | tr -d ' ')
[ "$COUNT" = 9 ] || fail "unexpected files are present; refusing removal"
while IFS= read -r line; do
  expected=${line%% *}
  relative=${line#*  }
  case "$relative" in
    bin/*) target="$PREFIX/$relative" ;;
    manifest.json) target="$PREFIX/share/manifest.json" ;;
    uninstall.sh) target="$PREFIX/share/uninstall.sh" ;;
    update.sh) target="$PREFIX/share/update.sh" ;;
    rollback.sh) target="$PREFIX/share/rollback.sh" ;;
    *) fail "unsupported payload entry: $relative" ;;
  esac
  actual=$(digest "$target")
  [ "$actual" = "$expected" ] || fail "installed payload digest mismatch for $relative"
done < "$PREFIX/share/SHA256SUMS"
rm "$PREFIX/bin/terraforma-local-core"
rm "$PREFIX/share/manifest.json"
rm "$PREFIX/share/SHA256SUMS"
rm "$PREFIX/share/uninstall.sh"
rm "$PREFIX/share/update.sh"
rm "$PREFIX/share/rollback.sh"
rmdir "$PREFIX/bin" "$PREFIX/share" "$PREFIX"
printf '%s\\n' "Removed package files only. Explicit estates were not touched."
"""


def package_validation_shell() -> str:
    """Shell helpers shared by the update and rollback scripts.

    Package prefixes are deliberately tiny.  The helpers validate the exact
    stored checksum table before either side of a swap is moved, so a changed
    or user-extended prefix is never treated as a rollback candidate.
    """
    return r'''digest() {
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{print $1}';
  elif command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}';
  else fail "no SHA-256 command is available"; fi
}
verify_source() {
  while IFS= read -r line; do
    expected=${line%% *}; relative=${line#*  }
    case "$relative" in bin/terraforma-local-core|manifest.json|uninstall.sh|update.sh|rollback.sh) ;; *) fail "unsupported package payload: $relative" ;; esac
    [ "$(digest "$SOURCE/$relative")" = "$expected" ] || fail "payload digest mismatch for $relative"
  done < "$SOURCE/SHA256SUMS"
}
verify_prefix() {
  candidate=$1
  [ -d "$candidate" ] && [ ! -L "$candidate" ] || fail "package prefix is absent or linked"
  for relative in bin share; do
    [ -d "$candidate/$relative" ] && [ ! -L "$candidate/$relative" ] || fail "required package directory is absent or linked: $relative"
  done
  for relative in bin/terraforma-local-core share/manifest.json share/SHA256SUMS share/uninstall.sh; do
    [ -f "$candidate/$relative" ] && [ ! -L "$candidate/$relative" ] || fail "required package path is absent or linked: $relative"
  done
  has_update=false; has_rollback=false
  [ ! -e "$candidate/share/update.sh" ] && [ ! -L "$candidate/share/update.sh" ] || has_update=true
  [ ! -e "$candidate/share/rollback.sh" ] && [ ! -L "$candidate/share/rollback.sh" ] || has_rollback=true
  if [ "$has_update" = true ] && [ "$has_rollback" = true ]; then
    [ -f "$candidate/share/update.sh" ] && [ ! -L "$candidate/share/update.sh" ] || fail "required package path is absent or linked: share/update.sh"
    [ -f "$candidate/share/rollback.sh" ] && [ ! -L "$candidate/share/rollback.sh" ] || fail "required package path is absent or linked: share/rollback.sh"
    expected_count=9; expected_lines=5
  elif [ "$has_update" = false ] && [ "$has_rollback" = false ]; then
    expected_count=7; expected_lines=3
  else
    fail "incomplete update lifecycle paths"
  fi
  count=$(find "$candidate" -print | wc -l | tr -d ' ')
  [ "$count" = "$expected_count" ] || fail "unexpected files are present; refusing package move"
  lines=0
  while IFS= read -r line; do
    expected=${line%% *}; relative=${line#*  }; lines=$((lines + 1))
    case "$relative" in
      bin/terraforma-local-core) target="$candidate/$relative" ;;
      manifest.json|uninstall.sh|update.sh|rollback.sh) target="$candidate/share/$relative" ;;
      *) fail "unsupported installed payload: $relative" ;;
    esac
    [ "$(digest "$target")" = "$expected" ] || fail "installed payload digest mismatch for $relative"
  done < "$candidate/share/SHA256SUMS"
  [ "$lines" = "$expected_lines" ] || fail "incomplete installed checksum table"
}
dedicated_path() {
  value=$1; label=$2
  case "$value" in ''|/|.|..) fail "unsafe $label" ;; esac
  [ "$(basename "$value")" = "terraforma-local-core" ] || fail "$label must end in terraforma-local-core"
}
backup_path() {
  value=$1
  case "$value" in ''|/|.|..) fail "unsafe backup prefix" ;; esac
  case "$(basename "$value")" in terraforma-local-core-backup-*) ;; *) fail "backup prefix must begin terraforma-local-core-backup-" ;; esac
}
'''


def update_script() -> str:
    return """#!/bin/sh
set -eu
fail() { printf '%s\\n' "terraforma-local-core update: $*" >&2; exit 1; }
[ "$#" -eq 2 ] || fail "usage: update.sh PREFIX/terraforma-local-core SIBLING-BACKUP/terraforma-local-core-backup-VERSION"
PREFIX=$1
BACKUP=$2
SOURCE=$(CDPATH= cd "$(dirname "$0")" && pwd)
""" + package_validation_shell() + r'''dedicated_path "$PREFIX" "prefix"
backup_path "$BACKUP"
PARENT=$(dirname "$PREFIX")
[ "$(dirname "$BACKUP")" = "$PARENT" ] || fail "backup must be a sibling of prefix"
[ ! -e "$BACKUP" ] && [ ! -L "$BACKUP" ] || fail "backup destination already exists"
verify_source
verify_prefix "$PREFIX"
STAGE="$PARENT/.terraforma-local-core.update.$$"
[ ! -e "$STAGE" ] && [ ! -L "$STAGE" ] || fail "temporary destination exists"
cleanup() {
  status=$?
  if [ "$status" -ne 0 ] && [ -e "$BACKUP" ] && [ ! -e "$PREFIX" ]; then mv "$BACKUP" "$PREFIX" || true; fi
  [ ! -e "$STAGE" ] || rm -rf "$STAGE"
  exit "$status"
}
trap cleanup EXIT HUP INT TERM
mkdir "$STAGE" "$STAGE/bin" "$STAGE/share"
cp "$SOURCE/bin/terraforma-local-core" "$STAGE/bin/terraforma-local-core"
cp "$SOURCE/manifest.json" "$STAGE/share/manifest.json"
cp "$SOURCE/SHA256SUMS" "$STAGE/share/SHA256SUMS"
cp "$SOURCE/uninstall.sh" "$STAGE/share/uninstall.sh"
cp "$SOURCE/update.sh" "$STAGE/share/update.sh"
cp "$SOURCE/rollback.sh" "$STAGE/share/rollback.sh"
chmod 700 "$STAGE/bin/terraforma-local-core" "$STAGE/share/uninstall.sh" "$STAGE/share/update.sh" "$STAGE/share/rollback.sh"
verify_prefix "$STAGE"
mv "$PREFIX" "$BACKUP"
mv "$STAGE" "$PREFIX"
trap - EXIT HUP INT TERM
printf '%s\\n' "Updated $PREFIX; verified prior package retained at $BACKUP. Explicit estates were not touched."
'''


def rollback_script() -> str:
    return """#!/bin/sh
set -eu
fail() { printf '%s\\n' "terraforma-local-core rollback: $*" >&2; exit 1; }
[ "$#" -eq 2 ] || fail "usage: rollback.sh PREFIX/terraforma-local-core SIBLING-BACKUP/terraforma-local-core-backup-VERSION"
PREFIX=$1
BACKUP=$2
SOURCE=$(CDPATH= cd "$(dirname "$0")/.." && pwd)
""" + package_validation_shell() + r'''dedicated_path "$PREFIX" "prefix"
backup_path "$BACKUP"
PARENT=$(dirname "$PREFIX")
[ "$(dirname "$BACKUP")" = "$PARENT" ] || fail "backup must be a sibling of prefix"
verify_prefix "$PREFIX"
verify_prefix "$BACKUP"
STAGE="$PARENT/.terraforma-local-core.rollback.$$"
[ ! -e "$STAGE" ] && [ ! -L "$STAGE" ] || fail "temporary destination exists"
cleanup() {
  status=$?
  if [ "$status" -ne 0 ] && [ -e "$STAGE" ] && [ ! -e "$PREFIX" ]; then mv "$STAGE" "$PREFIX" || true; fi
  exit "$status"
}
trap cleanup EXIT HUP INT TERM
mv "$PREFIX" "$STAGE"
mv "$BACKUP" "$PREFIX"
mv "$STAGE" "$BACKUP"
trap - EXIT HUP INT TERM
printf '%s\\n' "Restored $PREFIX from $BACKUP; replaced package retained as rollback target. Explicit estates were not touched."
'''


def build(binary: Path, output_parent: Path, version: str, *, target: str = TARGET) -> dict:
    binary = binary.resolve()
    output_parent = output_parent.resolve()
    if not binary.is_file():
        raise ValueError("binary must be an existing regular file")
    if not output_parent.is_dir():
        raise ValueError("output parent must already exist")
    if not version or any(char not in "0123456789.abcdefghijklmnopqrstuvwxyz-" for char in version):
        raise ValueError("version must use lowercase letters, digits, dots, or hyphens")
    if not target or any(char not in "0123456789abcdefghijklmnopqrstuvwxyz-" for char in target):
        raise ValueError("target must use lowercase letters, digits, or hyphens")
    stem = f"{PACKAGE_NAME}-{version}-{target}"
    archive = output_parent / f"{stem}.tar.gz"
    if archive.exists():
        raise ValueError("archive destination already exists")
    with tempfile.TemporaryDirectory(prefix=f".{stem}-", dir=output_parent) as temporary:
        root = Path(temporary) / stem
        (root / "bin").mkdir(parents=True)
        shutil.copyfile(binary, root / "bin" / PACKAGE_NAME)
        (root / "bin" / PACKAGE_NAME).chmod(0o700)
        manifest = {
            "schema": PACKAGE_SCHEMA,
            "package": PACKAGE_NAME,
            "version": version,
            "target": target,
            "install": {"prefix_basename": PACKAGE_NAME, "service_installation": False, "default_estate": None},
            "update": {"requires_verified_sibling_backup": True, "rollback_available": True,
                       "preserves_explicit_estates": True},
            "uninstall": {"preserves_explicit_estates": True, "refuses_unexpected_files": True},
            "binary": {"path": "bin/terraforma-local-core", "sha256": sha256(root / "bin" / PACKAGE_NAME), "bytes": (root / "bin" / PACKAGE_NAME).stat().st_size},
        }
        (root / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        write_executable(root / "install.sh", installer_script())
        write_executable(root / "uninstall.sh", uninstaller_script())
        write_executable(root / "update.sh", update_script())
        write_executable(root / "rollback.sh", rollback_script())
        (root / "SHA256SUMS").write_text(
            "".join(f"{sha256(root / relative)}  {relative}\n" for relative in PAYLOADS), encoding="utf-8"
        )
        with archive.open("wb") as raw:
            with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as tar:
                    tar.add(root, arcname=stem, recursive=True, filter=normalized_tarinfo)
    return {
        "schema": PACKAGE_SCHEMA,
        "status": "PACKAGE_BUILT_NOT_RELEASED",
        "archive": str(archive),
        "archive_sha256": sha256(archive),
        "archive_bytes": archive.stat().st_size,
        "binary_sha256": sha256(binary),
        "target": target,
        "version": version,
        "scope": "Local terminal package only; not signed distribution, customer proof, service installation, or release approval.",
    }


def verify_macos_arm64(binary: Path) -> str:
    description = subprocess.run(["file", str(binary)], check=True, text=True, capture_output=True).stdout.strip()
    if "Mach-O" not in description or "arm64" not in description:
        raise ValueError("macOS package requires a Mach-O arm64 binary")
    return description


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output-parent", type=Path, required=True)
    parser.add_argument("--version", default=cargo_version())
    args = parser.parse_args()
    binary_format = verify_macos_arm64(args.binary)
    result = build(args.binary, args.output_parent, args.version)
    result["binary_format"] = binary_format
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
