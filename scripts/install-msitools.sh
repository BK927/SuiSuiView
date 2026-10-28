#!/usr/bin/env bash
set -euo pipefail
prefix="${1:?installation prefix required}"
build_root="${2:?temporary build directory required}"
mkdir -p "$build_root"
sudo apt-get update -qq
sudo apt-get install -y --no-install-recommends meson ninja-build valac libgsf-1-dev libgcab-dev uuid-dev bison gettext libglib2.0-dev libxml2-dev gobject-introspection libgirepository1.0-dev
curl --fail --location --silent --show-error https://download.gnome.org/sources/msitools/0.106/msitools-0.106.tar.xz -o "$build_root/msitools.tar.xz"
printf '%s  %s\n' '1ed34279cf8080f14f1b8f10e649474125492a089912e7ca70e59dfa2e5a659b' "$build_root/msitools.tar.xz" | sha256sum --check
tar -xf "$build_root/msitools.tar.xz" -C "$build_root"
meson setup "$build_root/build" "$build_root/msitools-0.106" --prefix="$prefix" --libdir=lib
ninja -C "$build_root/build" install
if [[ -n "${GITHUB_PATH:-}" ]]; then
    printf '%s\n' "$prefix/bin" >> "$GITHUB_PATH"
    printf 'LD_LIBRARY_PATH=%s/lib\n' "$prefix" >> "$GITHUB_ENV"
fi
