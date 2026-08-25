#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/uniffi-common.sh"

[ "$(uname -s)" = Darwin ] || { echo "ios xcframework build requires macos" >&2; exit 1; }
[ $# -eq 2 ] || { echo "usage: $0 <crate> <SwiftModule>" >&2; exit 1; }

NAME="$1"
MODULE="$2"

cd "$HERE/.."
mobile_surface_target_dir
CRATE="crates/$NAME"
PKG="packages/$NAME/swift"
CARGO_PKG="$(sed -n 's/^name = "\(.*\)"$/\1/p' "$CRATE/Cargo.toml" | head -1)"
LIB_BASE="${CARGO_PKG//-/_}"
LIB="lib${LIB_BASE}.a"
PROFILE=release

DEVICE=aarch64-apple-ios
SIM_ARM=aarch64-apple-ios-sim
SIM_X86=x86_64-apple-ios
MAC_ARM=aarch64-apple-darwin
MAC_X86=x86_64-apple-darwin

TARGET_DIR="${CARGO_TARGET_DIR:-target}"

FEATURES=(--no-default-features)

if [ "${XCFRAMEWORK_DEVICE_ONLY:-0}" = "1" ]; then
  TARGETS=("$DEVICE")
elif [ "${XCFRAMEWORK_SIM_ONLY:-0}" = "1" ]; then
  TARGETS=("$SIM_ARM")
else
  TARGETS=("$DEVICE" "$SIM_ARM" "$SIM_X86" "$MAC_ARM" "$MAC_X86")
fi

XCF="$PKG/Frameworks/${MODULE}FFI.xcframework"
SWIFT_OUT="$PKG/Sources/$MODULE"
WORK="$CRATE/build"
HDRS="$WORK/headers"

echo "== rustup targets =="
rustup target add "${TARGETS[@]}" >/dev/null
rustup component add llvm-tools >/dev/null 2>&1 || true
OBJCOPY="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/host: //p')/bin/rust-objcopy"

echo "== generate swift bindings =="
cargo build -q -p "$CARGO_PKG" "${FEATURES[@]}" --lib
rm -rf "$WORK"; mkdir -p "$WORK/gen" "$HDRS"
cargo run -q -p "$CARGO_PKG" "${FEATURES[@]}" --bin "$NAME-bindgen" -- generate --library "$(host_dylib "$LIB_BASE")" --language swift --out-dir "$WORK/gen"
mkdir -p "$SWIFT_OUT"
cp "$WORK/gen/${LIB_BASE}.swift" "$SWIFT_OUT/${LIB_BASE}.swift"
mkdir -p "$HDRS/${LIB_BASE}FFI"
cp "$WORK/gen/${LIB_BASE}FFI.h" "$HDRS/${LIB_BASE}FFI/${LIB_BASE}FFI.h"
cp "$WORK/gen/${LIB_BASE}FFI.modulemap" "$HDRS/${LIB_BASE}FFI/module.modulemap"

echo "== build staticlibs (release) =="
export IPHONEOS_DEPLOYMENT_TARGET=18.0
export MACOSX_DEPLOYMENT_TARGET=15.0
for t in "${TARGETS[@]}"; do
  cargo rustc -q -p "$CARGO_PKG" "${FEATURES[@]}" --lib --crate-type staticlib --"$PROFILE" --target "$t"
  "$OBJCOPY" --remove-section=__TEXT,__eh_frame --remove-section=__LD,__compact_unwind "$TARGET_DIR/$t/$PROFILE/$LIB"
done

xcf_args=(-library "$TARGET_DIR/${TARGETS[0]}/$PROFILE/$LIB" -headers "$HDRS")

if [ "${#TARGETS[@]}" -gt 1 ]; then
  echo "== lipo simulator + macos arches =="
  SIM_FAT="$WORK/sim/$LIB"
  MAC_FAT="$WORK/mac/$LIB"
  mkdir -p "$WORK/sim" "$WORK/mac"
  lipo -create "$TARGET_DIR/$SIM_ARM/$PROFILE/$LIB" "$TARGET_DIR/$SIM_X86/$PROFILE/$LIB" -output "$SIM_FAT"
  lipo -create "$TARGET_DIR/$MAC_ARM/$PROFILE/$LIB" "$TARGET_DIR/$MAC_X86/$PROFILE/$LIB" -output "$MAC_FAT"
  xcf_args+=(-library "$SIM_FAT" -headers "$HDRS" -library "$MAC_FAT" -headers "$HDRS")
fi

echo "== assemble xcframework =="
rm -rf "$XCF"; mkdir -p "$PKG/Frameworks"
xcodebuild -create-xcframework "${xcf_args[@]}" -output "$XCF"

echo "done: $XCF + $SWIFT_OUT/${LIB_BASE}.swift"
