#!/usr/bin/env python3
"""Scan dependency manifests for forbidden native codec indicators."""

from __future__ import annotations

import sys
from pathlib import Path


FORBIDDEN = [
    "ffmpeg",
    "x265",
    "libheif",
    "hevc",
    "agpl",
    "gpl-",
    "gpl ",
    "lgpl-static",
]

SCAN_PATHS = [
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
]


def main() -> int:
    failures = []
    for relative in SCAN_PATHS:
        path = Path(relative)
        if not path.exists():
            continue
        text = path.read_text(encoding="utf-8", errors="ignore").lower()
        for token in FORBIDDEN:
            if token in text:
                failures.append(f"{relative}: found forbidden indicator {token!r}")

    if failures:
        print("Native dependency scan failed:")
        for failure in failures:
            print(f"  - {failure}")
        return 1

    print("Native dependency scan passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
