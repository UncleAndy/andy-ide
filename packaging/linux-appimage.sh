#!/usr/bin/env bash
#
# Packs an already built binary into an AppImage.
#
#   packaging/linux-appimage.sh <binary> <version> [<output dir>]
#
# The AppDir is assembled here and its libraries are collected by `linuxdeploy`; `appimagetool` then
# wraps the directory into one executable. Both tools are AppImages themselves, and a build machine
# usually has no FUSE to run one — so each is unpacked with `--appimage-extract` into a directory of
# its own (the two unpacks would otherwise land on top of each other) and run from there.
set -euo pipefail

binary=${1:?the path to a built andy-ide}
version=${2:?the version, without the leading v}
out=${3:-dist}
root=$(cd "$(dirname "$0")/.." && pwd)
here=$(cd "$(dirname "$0")" && pwd)
arch=${ARCH:-x86_64}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

appdir="$work/andy-ide.AppDir"
install -Dm755 "$binary" "$appdir/usr/bin/andy-ide"
install -Dm644 "$root/assets/andy-ide.png" "$appdir/andy-ide.png"
install -Dm644 "$root/assets/andy-ide.desktop" "$appdir/andy-ide.desktop"
install -Dm755 "$here/AppRun" "$appdir/AppRun"

# Unpacks one AppImage tool and answers the path of the program inside it.
unpack() {
    local name=$1 url=$2
    # The directory is named on a line of its own: `local a=$1 b="$a"` expands `$a` before it is set.
    local into="$work/$name"
    echo "fetching $name" >&2
    mkdir -p "$into"
    curl --fail --silent --show-error --location --retry 3 \
        --output "$into/$name.AppImage" "$url"
    chmod +x "$into/$name.AppImage"
    ( cd "$into" && "./$name.AppImage" --appimage-extract >/dev/null )
    printf '%s\n' "$into/squashfs-root/AppRun"
}

deploy=$(unpack linuxdeploy \
    "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-${arch}.AppImage")
tool=$(unpack appimagetool \
    "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-${arch}.AppImage")

# Deploying: every library the binary links against that a desktop does not already have is copied
# in; the ones that must stay the host's — glibc, libGL, the driver — are left out by the tool's own
# blacklist. `NO_STRIP` is here because stripping a binary someone may want a backtrace from is a
# trade the release should make in the workflow, not silently here.
NO_STRIP=1 "$deploy" \
    --appdir "$appdir" \
    --executable "$appdir/usr/bin/andy-ide" \
    --desktop-file "$appdir/andy-ide.desktop" \
    --icon-file "$appdir/andy-ide.png"

mkdir -p "$out"
name="andy-ide-${version}-${arch}.AppImage"
ARCH="$arch" "$tool" "$appdir" "$out/$name"
echo "built $out/$name"
