#!/usr/bin/env python3
"""Fetch a pinned app-local PDFium runtime for native-ai development builds."""

from __future__ import annotations

import argparse
import hashlib
import shutil
import sys
import tarfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


PDFIUM_RELEASE_TAG = "chromium/7763"

PACKAGES = {
    "windows-x64": ("pdfium-win-x64.tgz", "pdfium.dll"),
    "windows-x86": ("pdfium-win-x86.tgz", "pdfium.dll"),
    "windows-arm64": ("pdfium-win-arm64.tgz", "pdfium.dll"),
    "linux-x64": ("pdfium-linux-x64.tgz", "libpdfium.so"),
    "macos-x64": ("pdfium-mac-x64.tgz", "libpdfium.dylib"),
    "macos-arm64": ("pdfium-mac-arm64.tgz", "libpdfium.dylib"),
}


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Download a V8/XFA-free PDFium package for native-ai."
    )
    parser.add_argument(
        "--platform",
        choices=sorted(PACKAGES),
        default=default_platform(),
        help="PDFium binary package to fetch.",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=Path("third_party") / "pdfium",
        help="Directory used for downloaded and extracted PDFium files.",
    )
    parser.add_argument(
        "--copy-to",
        type=Path,
        help="Optional executable directory that should receive the PDFium library.",
    )
    parser.add_argument(
        "--sha256",
        help="Expected SHA-256 for the downloaded archive; recommended for release packaging.",
    )
    parser.add_argument(
        "--keep-archive",
        action="store_true",
        help="Keep the downloaded .tgz archive after extraction.",
    )
    args = parser.parse_args()

    package, library_name = PACKAGES[args.platform]
    args.out.mkdir(parents=True, exist_ok=True)
    archive_path = args.out / package
    extract_dir = args.out / args.platform

    download_package(package, archive_path)
    archive_sha256 = sha256_file(archive_path)
    if args.sha256 and archive_sha256.lower() != args.sha256.lower():
        raise SystemExit(
            f"Checksum mismatch for {archive_path}: expected {args.sha256}, got {archive_sha256}"
        )
    if extract_dir.exists():
        shutil.rmtree(extract_dir)
    extract_dir.mkdir(parents=True)
    safe_extract(archive_path, extract_dir)

    library = find_library(extract_dir, library_name)
    if args.copy_to:
        args.copy_to.mkdir(parents=True, exist_ok=True)
        copied = args.copy_to / library_name
        shutil.copy2(library, copied)
        print(f"Copied {library_name} to {copied}")
    else:
        print(f"PDFium library ready at {library}")

    if not args.keep_archive:
        archive_path.unlink(missing_ok=True)

    print(f"PDFium release: {PDFIUM_RELEASE_TAG}")
    print(f"Archive SHA-256: {archive_sha256}")
    print("Record the package URL and checksum in release packaging notes.")
    return 0


def default_platform() -> str:
    if sys.platform == "win32":
        return "windows-x64"
    if sys.platform == "darwin":
        return "macos-arm64" if platform_machine_is_arm() else "macos-x64"
    return "linux-x64"


def platform_machine_is_arm() -> bool:
    import platform

    return platform.machine().lower() in {"arm64", "aarch64"}


def download_package(package: str, archive_path: Path) -> None:
    if archive_path.exists():
        print(f"Using existing archive {archive_path}")
        return

    encoded_tag = urllib.parse.quote(PDFIUM_RELEASE_TAG, safe="")
    urls = [
        f"https://github.com/bblanchon/pdfium-binaries/releases/download/{encoded_tag}/{package}",
        f"https://github.com/bblanchon/pdfium-binaries/releases/download/{PDFIUM_RELEASE_TAG}/{package}",
    ]
    last_error: Exception | None = None
    for url in urls:
        try:
            print(f"Downloading {url}")
            urllib.request.urlretrieve(url, archive_path)
            return
        except urllib.error.URLError as error:
            last_error = error
            archive_path.unlink(missing_ok=True)

    raise SystemExit(f"Failed to download {package}: {last_error}")


def safe_extract(archive_path: Path, extract_dir: Path) -> None:
    root = extract_dir.resolve()
    with tarfile.open(archive_path, "r:gz") as archive:
        for member in archive.getmembers():
            if member.issym() or member.islnk():
                raise SystemExit(f"Refusing to extract link from archive: {member.name}")
            target = (extract_dir / member.name).resolve()
            if root not in [target, *target.parents]:
                raise SystemExit(f"Refusing to extract path outside destination: {member.name}")
        archive.extractall(extract_dir)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def find_library(extract_dir: Path, library_name: str) -> Path:
    matches = sorted(
        extract_dir.rglob(library_name),
        key=lambda path: (len(path.parts), str(path).lower()),
    )
    if not matches:
        raise SystemExit(f"Could not find {library_name} in {extract_dir}")
    return matches[0]


if __name__ == "__main__":
    raise SystemExit(main())
