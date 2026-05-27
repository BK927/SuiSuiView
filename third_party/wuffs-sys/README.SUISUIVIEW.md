# SuiSuiView Wuffs Benchmark Patch

This directory is a bench-only local patch for `wuffs-sys 0.1.0`.

The upstream crate failed to build on the recorded Windows/MSVC environment
because its `bindgen 0.58` dependency generated invalid Rust identifiers for
symbols from the vendored Wuffs C header. This local patch keeps the crate and
vendored Wuffs source otherwise unchanged and updates the build-time bindgen
dependency to `0.72.1`.

This patch is used only by the `bench-native-wuffs` feature. It is not enabled
by the default application build and does not make Wuffs a production decoder
backend.

The feature still requires a native C compiler plus the bindgen/libclang setup
needed by the upstream crate. The patch fixes the generated Rust identifiers on
Windows/MSVC; it does not pre-generate bindings.

License: Apache-2.0, matching upstream `wuffs-sys` and Wuffs generated C
source.
