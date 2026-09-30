#!/usr/bin/env python3
"""Build a self-contained, terminal-installable Linux x86_64 local-core package."""
from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path

from package_macos import build, cargo_version

TARGET = "linux-x86-64"


def verify_linux_x86_64(binary: Path) -> str:
    description = subprocess.run(["file", str(binary)], check=True, text=True, capture_output=True).stdout.strip()
    if "ELF" not in description or "x86-64" not in description:
        raise ValueError("Linux package requires an ELF x86-64 binary")
    return description


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output-parent", type=Path, required=True)
    parser.add_argument("--version", default=cargo_version())
    args = parser.parse_args()
    binary_format = verify_linux_x86_64(args.binary)
    result = build(args.binary, args.output_parent, args.version, target=TARGET)
    result["binary_format"] = binary_format
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
