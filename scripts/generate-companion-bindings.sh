#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/uniffi-common.sh"
cd "$HERE/.."
mobile_surface_target_dir

cargo build -p bridgething-companion --no-default-features --lib
LIB="$(host_dylib bridgething_companion)"

echo "== kotlin bindings =="
cargo run -q -p bridgething-companion --no-default-features --bin companion-bindgen -- generate \
  --library "$LIB" --language kotlin --out-dir packages/companion/kotlin/core/src/main/kotlin

KT="packages/companion/kotlin/core/src/main/kotlin/uniffi/bridgething_companion/bridgething_companion.kt"
if command -v ktlint >/dev/null 2>&1; then
  ktlint --format --log-level=none "$KT" || true
else
  echo "ktlint not installed, this is going to cause a bad diff" >&2
fi

echo "== swift bindings =="
GEN="$(mktemp -d)"
trap 'rm -rf "$GEN"' EXIT
cargo run -q -p bridgething-companion --no-default-features --bin companion-bindgen -- generate \
  --library "$LIB" --language swift --out-dir "$GEN"
cp "$GEN/bridgething_companion.swift" packages/companion/swift/Sources/BridgethingCompanionCore/bridgething_companion.swift
cp "$GEN/bridgething_companionFFI.h" packages/companion/swift/FFI/bridgething_companionFFI/bridgething_companionFFI.h
printf 'module bridgething_companionFFI {\n    header "bridgething_companionFFI.h"\n    export *\n}\n' \
  > packages/companion/swift/FFI/bridgething_companionFFI/module.modulemap

echo "done: kotlin core bindings + swift core bindings"
