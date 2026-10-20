#!/usr/bin/env bash
set -euo pipefail

FFMPEG_VERSION=9.0.2
FFMPEG_SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
OPENH264_VERSION=2.6.0
OPENH264_SHA256=558544ad358283a7ab2930d69a9ceddf913f4a51ee9bf1bfb9e377322af81a69
DAV1D_VERSION=1.5.4
DAV1D_SHA256=686616b7c69eb88d44459391ab25cac13b6647a3b288835c5784e71c1514a5c5
FFMPEG_URL="https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz"
OPENH264_URL="https://github.com/cisco/openh264/archive/refs/tags/v${OPENH264_VERSION}.tar.gz"
DAV1D_URL="https://downloads.videolan.org/pub/videolan/dav1d/${DAV1D_VERSION}/dav1d-${DAV1D_VERSION}.tar.xz"
FFMPEG_TARBALL="ffmpeg-${FFMPEG_VERSION}.tar.xz"
OPENH264_TARBALL="openh264-${OPENH264_VERSION}.tar.gz"
DAV1D_TARBALL="dav1d-${DAV1D_VERSION}.tar.xz"
MINGW_W64_COMMIT="${MINGW_W64_COMMIT:?the image pins the mingw-w64 commit its llvm-mingw toolchain was built from}"
MINGW_W64_SHA256="${MINGW_W64_SHA256:?the image pins the mingw-w64 source checksum}"
MINGW_W64_URL="https://github.com/mingw-w64/mingw-w64/archive/${MINGW_W64_COMMIT}.tar.gz"
MINGW_W64_TARBALL="mingw-w64-${MINGW_W64_COMMIT}.tar.gz"
LLVM_MINGW_LICENSE=/opt/llvm-mingw/LICENSE.TXT
SOURCES=(
  "${FFMPEG_TARBALL} ${FFMPEG_URL} ${FFMPEG_SHA256}"
  "${OPENH264_TARBALL} ${OPENH264_URL} ${OPENH264_SHA256}"
  "${DAV1D_TARBALL} ${DAV1D_URL} ${DAV1D_SHA256}"
  "${MINGW_W64_TARBALL} ${MINGW_W64_URL} ${MINGW_W64_SHA256}"
)
MIRROR=https://ota.bridgething.com/ffmpeg-sidecar
COMPONENTS=(
  --disable-everything --disable-autodetect --disable-network --disable-doc --disable-debug
  --disable-ffplay --disable-ffprobe --disable-avdevice --disable-swresample
  --enable-libopenh264 --enable-libdav1d
  --enable-demuxer=mov,matroska,avi
  --enable-decoder=h264,hevc,vp8,vp9,mpeg4,prores,mjpeg,libdav1d
  --enable-encoder=libopenh264,pam
  --enable-muxer=mp4,image2pipe
  --enable-parser=h264,hevc,vp8,vp9,mpeg4video,mjpeg,av1
  --enable-filter=buffer,buffersink,null,format,scale,crop,transpose,hflip,vflip,xstack
  --enable-protocol=file,pipe
)

target="${1:?usage: ffmpeg-sidecar <rust target triple>|sources}"
cache="${SIDECAR_CACHE:-/cache}"
out="${SIDECAR_OUT:-/out}"
mkdir -p "$cache/src" "$out"

download() {
  local name="$1" url="$2" sum="$3" file="$cache/src/$1"
  if ! echo "${sum}  ${file}" | sha256sum -c --status - 2>/dev/null; then
    curl -fsSL -o "${file}.part" "$url"
    echo "${sum}  ${file}.part" | sha256sum -c -
    mv "${file}.part" "$file"
  fi
}

for source in "${SOURCES[@]}"; do
  read -r name url sum <<<"$source"
  download "$name" "$url" "$sum"
done

if [ "$target" = sources ]; then
  mkdir -p "$out/source"
  for source in "${SOURCES[@]}"; do
    read -r name _ <<<"$source"
    cp "$cache/src/$name" "$out/source/$name"
  done
  exit 0
fi

recipe="${SIDECAR_RECIPE:?SIDECAR_RECIPE must name the recipe hash this build publishes under}"
jobs="$(nproc)"

case "$target" in
  x86_64-unknown-linux-gnu) host=x86_64-linux-gnu os=linux cpu=x86_64 ;;
  aarch64-unknown-linux-gnu) host=aarch64-linux-gnu os=linux cpu=aarch64 ;;
  x86_64-pc-windows-msvc) host=x86_64-w64-mingw32 os=windows cpu=x86_64 ;;
  aarch64-pc-windows-msvc) host=aarch64-w64-mingw32 os=windows cpu=aarch64 ;;
  *) echo "no sidecar recipe for ${target}" >&2; exit 2 ;;
esac

if [ "$os" = linux ]; then
  cc="${host}-gcc" cxx="${host}-g++" exe='' ffmpeg_os=linux openh264_os=linux
  link_static='-static-libgcc -static-libstdc++' openh264_libs='-lpthread -lm'
else
  cc="${host}-clang" cxx="${host}-clang++" exe=.exe ffmpeg_os=mingw32 openh264_os=mingw_nt
  link_static='-static' openh264_libs=''
fi
ar="${host}-ar" nm="${host}-nm" ranlib="${host}-ranlib" strip="${host}-strip"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
prefix="$work/prefix"
mkdir -p "$prefix"

for source in "${SOURCES[@]}"; do
  read -r name _ <<<"$source"
  mkdir -p "$work/src/${name%%-*}"
  tar -xf "$cache/src/$name" -C "$work/src/${name%%-*}" --strip-components=1
done

source_of() {
  printf '%s %s\nsource: %s/source/%s\nupstream: %s\nsha256: %s\n' "$1" "$2" "$MIRROR" "$3" "$4" "$5"
}

{
  echo "bridgething-ffmpeg is FFmpeg built from unmodified upstream sources, statically linked with openh264 and dav1d."
  echo "The complete corresponding source and the scripts that build it are served from the same host as this binary."
  echo
  echo "build recipe ${recipe}"
  echo "script: ${MIRROR}/${recipe}/ffmpeg-sidecar.sh"
  echo "image: ${MIRROR}/${recipe}/ffmpeg-sidecar.Dockerfile"
  echo
  source_of FFmpeg "$FFMPEG_VERSION" "$FFMPEG_TARBALL" "$FFMPEG_URL" "$FFMPEG_SHA256"
  echo "license: GNU Lesser General Public License version 2.1 or later"
  echo "configured with: ${COMPONENTS[*]}"
  echo
  source_of openh264 "$OPENH264_VERSION" "$OPENH264_TARBALL" "$OPENH264_URL" "$OPENH264_SHA256"
  echo "license: BSD 2-Clause"
  echo
  source_of dav1d "$DAV1D_VERSION" "$DAV1D_TARBALL" "$DAV1D_URL" "$DAV1D_SHA256"
  echo "license: BSD 2-Clause"
  echo
  echo "The windows builds also statically link the mingw-w64 runtime, winpthreads and the LLVM C++ runtime"
  echo "(libc++, libc++abi, libunwind, compiler-rt) from llvm-mingw ${LLVM_MINGW}."
  echo
  source_of mingw-w64 "$MINGW_W64_COMMIT" "$MINGW_W64_TARBALL" "$MINGW_W64_URL" "$MINGW_W64_SHA256"
  echo "license: Zope Public License 2.1, with public domain, BSD and MIT parts; winpthreads is MIT"
  echo
  echo "LLVM runtime libraries from llvm-mingw ${LLVM_MINGW}"
  echo "upstream: https://github.com/mstorsjo/llvm-mingw/releases/tag/${LLVM_MINGW}"
  echo "license: Apache License 2.0 with LLVM Exceptions"
  echo
  echo "======== FFmpeg: GNU Lesser General Public License version 2.1 ========"
  echo
  cat "$work/src/ffmpeg/COPYING.LGPLv2.1"
  echo
  echo "======== openh264 ========"
  echo
  cat "$work/src/openh264/LICENSE"
  echo
  echo "======== dav1d ========"
  echo
  cat "$work/src/dav1d/COPYING"
  echo
  echo "======== mingw-w64 runtime ========"
  echo
  cat "$work/src/mingw/COPYING.MinGW-w64-runtime/COPYING.MinGW-w64-runtime.txt"
  echo
  echo "======== winpthreads ========"
  echo
  cat "$work/src/mingw/mingw-w64-libraries/winpthreads/COPYING"
  echo
  echo "======== LLVM runtime libraries ========"
  echo
  cat "$LLVM_MINGW_LICENSE"
} >"$work/notice.txt"

make -C "$work/src/openh264" -j"$jobs" \
  OS="$openh264_os" ARCH="$cpu" CC="$cc" CXX="$cxx" AR="$ar" \
  PREFIX="$prefix" STATIC_LDFLAGS="$openh264_libs" install-static

cat >"$work/cross.ini" <<EOF
[binaries]
c = '${cc}'
cpp = '${cxx}'
ar = '${ar}'
strip = '${strip}'
nasm = 'nasm'

[host_machine]
system = '${os}'
cpu_family = '${cpu}'
cpu = '${cpu}'
endian = 'little'
EOF

meson setup "$work/build/dav1d" "$work/src/dav1d" \
  --cross-file "$work/cross.ini" --prefix "$prefix" --libdir lib \
  --buildtype release --default-library static \
  -Denable_tools=false -Denable_tests=false -Denable_examples=false
ninja -C "$work/build/dav1d" install

cd "$work/src/ffmpeg"
PKG_CONFIG_LIBDIR="$prefix/lib/pkgconfig" ./configure \
  --enable-cross-compile --target-os="$ffmpeg_os" --arch="$cpu" \
  --cc="$cc" --cxx="$cxx" --ld="$cxx" --ar="$ar" --nm="$nm" --ranlib="$ranlib" --strip="$strip" \
  --pkg-config=pkg-config --pkg-config-flags=--static \
  --extra-cflags="-I$prefix/include" --extra-ldflags="-L$prefix/lib $link_static" \
  "${COMPONENTS[@]}"
make -j"$jobs" "ffmpeg${exe}"

name="bridgething-ffmpeg-${target}${exe}"
binary="$work/$name"
"$strip" --strip-all -o "$binary" "ffmpeg${exe}"

if [ "$os" = linux ]; then
  needed="$("${host}-readelf" -d "$binary" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')"
  stray="$(grep -vE '^(libc|libm|ld-linux-[a-z0-9_-]+)\.so\.[0-9]+$' <<<"$needed" || true)"
  glibc="$("${host}-objdump" -T "$binary" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)"
  echo "links: $(tr '\n' ' ' <<<"$needed")newest symbol ${glibc}"
else
  needed="$(llvm-readobj --coff-imports "$binary" | sed -n 's/^ *Name: \(.*\.dll\)$/\1/Ip')"
  stray="$(grep -viE '^(kernel32|bcrypt|advapi32|shell32|user32|ole32|api-ms-win-[a-z0-9-]+)\.dll$' <<<"$needed" || true)"
  echo "imports: $(tr '\n' ' ' <<<"$needed")"
fi
if [ -n "$stray" ]; then
  echo "${name} depends on libraries the host cannot be assumed to have: ${stray}" >&2
  exit 1
fi

cp "$binary" "$out/$name"
cp "$work/notice.txt" "$out/bridgething-ffmpeg-NOTICE.txt"
(cd "$work" && sha256sum "$name") >"$out/$name.sha256"
echo "$recipe" >"$out/$name.recipe"
echo "${name} $(stat -c %s "$binary") bytes"
