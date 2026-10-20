FROM debian:bookworm-slim

ARG LLVM_MINGW=20261006
ARG LLVM_MINGW_X86_64=5f9c6ed95b2d4bdb2869a488c5fd5857fbdabcf288a0aa3eb1da43f6a08d8ab4
ARG LLVM_MINGW_AARCH64=e77c4acfa44184f6b0c52b814d724ad63b2ecebb3c3f6f842c0846e717cbd23c
ARG MINGW_W64_COMMIT=a3d93999ef0521681d45e445c39964a7af0f593f
ARG MINGW_W64_SHA256=c08776647d5744df7f6ae2bf131da4c9c0fd9934f7a11ecc85cd5692d4b01b62

RUN set -eu; \
    case "$(dpkg --print-architecture)" in \
      amd64) foreign=aarch64-linux-gnu ;; \
      arm64) foreign=x86-64-linux-gnu ;; \
      *) echo "unsupported build host" >&2; exit 1 ;; \
    esac; \
    apt-get update; \
    apt-get install --assume-yes --no-install-recommends \
      ca-certificates curl xz-utils make nasm meson ninja-build pkgconf python3 \
      gcc g++ "gcc-${foreign}" "g++-${foreign}" binutils-x86-64-linux-gnu binutils-aarch64-linux-gnu; \
    rm -rf /var/lib/apt/lists/*

RUN set -eu; \
    case "$(dpkg --print-architecture)" in \
      amd64) arch=x86_64; sum="${LLVM_MINGW_X86_64}" ;; \
      arm64) arch=aarch64; sum="${LLVM_MINGW_AARCH64}" ;; \
    esac; \
    name="llvm-mingw-${LLVM_MINGW}-ucrt-ubuntu-22.04-${arch}"; \
    curl -fsSL -o /tmp/llvm-mingw.tar.xz "https://github.com/mstorsjo/llvm-mingw/releases/download/${LLVM_MINGW}/${name}.tar.xz"; \
    echo "${sum}  /tmp/llvm-mingw.tar.xz" | sha256sum -c -; \
    tar -xJf /tmp/llvm-mingw.tar.xz -C /opt; \
    mv "/opt/${name}" /opt/llvm-mingw; \
    rm /tmp/llvm-mingw.tar.xz

ENV PATH=/opt/llvm-mingw/bin:$PATH LLVM_MINGW=${LLVM_MINGW} MINGW_W64_COMMIT=${MINGW_W64_COMMIT} MINGW_W64_SHA256=${MINGW_W64_SHA256}

COPY ffmpeg-sidecar.sh /usr/local/bin/ffmpeg-sidecar

ENTRYPOINT ["/usr/local/bin/ffmpeg-sidecar"]
