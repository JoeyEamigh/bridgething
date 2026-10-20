#!/usr/bin/env bash
set -euo pipefail

OUT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/packages/companion/kotlin/companion/src/androidTest/assets"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$OUT"

scene() {
    local width=$1 height=$2 fps=$3 seconds=$4
    local side=$(( (width / 2 < height ? width / 2 : height) / 3 / 4 * 4 ))
    SCENE_INPUTS=(
        -f lavfi -i "color=c=red:s=${width}x${height}:r=${fps}:d=${seconds}"
        -f lavfi -i "color=c=blue:s=$((width / 2))x${height}:r=${fps}:d=${seconds}"
        -f lavfi -i "color=c=white:s=${side}x${side}:r=${fps}:d=${seconds}"
    )
    SCENE_GRAPH="[0][1]overlay=x=$((width / 2)):y=0[a];[a][2]overlay=x=$((side / 2)):y=$((side / 2))"
}

turn() {
    local source=$1 rotation=$2 out=$3
    ffmpeg -hide_banner -loglevel error -y -display_rotation $(( (360 - rotation) % 360 )) -i "$source" \
        -c copy -movflags +faststart+write_colr "$out"
}

sdr() {
    local width=$1 height=$2 rotation=$3 fps=$4 seconds=$5 drift=${6:-0}
    local name="sdr-${width}x${height}-r${rotation}-${fps}fps-${seconds}s"
    local motion=""
    if [ "$drift" -gt 0 ]; then
        name="${name}-drift${drift}"
        motion=",scroll=horizontal=$(awk "BEGIN { print $drift / $width }")"
    fi
    scene "$width" "$height" "$fps" "$seconds"
    ffmpeg -hide_banner -loglevel error -y "${SCENE_INPUTS[@]}" -f lavfi -i "anullsrc=r=44100:cl=mono" \
        -filter_complex "${SCENE_GRAPH}${motion},format=yuv420p[v]" -map "[v]" -map 3:a \
        -c:v libx264 -crf 18 -g "$fps" -c:a aac -b:a 64k -t "$seconds" "$WORK/upright.mp4"
    turn "$WORK/upright.mp4" "$rotation" "$OUT/$name.mp4"
}

hdr() {
    local codec=$1 transfer=$2 rotation=$3
    local label
    case $transfer in
        arib-std-b67) label=hlg ;;
        smpte2084) label=pq ;;
    esac
    local svt_transfer
    case $transfer in
        arib-std-b67) svt_transfer=18 ;;
        smpte2084) svt_transfer=16 ;;
    esac
    local encoder
    case $codec in
        hevc) encoder=(-c:v libx265 -profile:v main10 -preset slow -crf 4 -g 10 -tag:v hvc1
            -x265-params "log-level=error:colorprim=bt2020:transfer=$transfer:colormatrix=bt2020nc:range=limited") ;;
        vp9) encoder=(-c:v libvpx-vp9 -profile:v 2 -crf 10 -b:v 0 -g 10) ;;
        av1) encoder=(-c:v libsvtav1 -crf 20 -g 10
            -svtav1-params "color-primaries=9:transfer-characteristics=$svt_transfer:matrix-coefficients=9:color-range=0") ;;
    esac
    scene 320 180 10 1
    local tags="setparams=color_primaries=bt2020:color_trc=$transfer:colorspace=bt2020nc:range=tv"
    ffmpeg -hide_banner -loglevel error -y "${SCENE_INPUTS[@]}" \
        -filter_complex "${SCENE_GRAPH},format=rgb24,scale=out_color_matrix=bt2020:out_range=tv,format=yuv420p10le,${tags}[v]" \
        -map "[v]" "${encoder[@]}" -movflags +write_colr "$WORK/upright.mp4" 2>/dev/null
    turn "$WORK/upright.mp4" "$rotation" "$OUT/hdr-${label}-${codec}-${rotation}.mp4"
}

rm -f "$OUT"/*.mp4

for size in 1600x900 720x720; do
    for rotation in 0 90 180 270; do
        sdr "${size%x*}" "${size#*x}" "$rotation" 30 1
    done
done
sdr 1600 900 90 60 2
sdr 1600 900 0 30 8
sdr 960 540 0 30 2 6

for rotation in 0 180; do
    hdr hevc arib-std-b67 "$rotation"
    hdr vp9 arib-std-b67 "$rotation"
    hdr av1 arib-std-b67 "$rotation"
    hdr vp9 smpte2084 "$rotation"
done

ls -l "$OUT"
