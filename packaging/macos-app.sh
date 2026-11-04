#!/usr/bin/env bash
#
# Packs a built binary into an `andy-ide.app` bundle and zips it.
#
#   packaging/macos-app.sh <binary> <version> [<output dir>]
#
# The binary may be a universal one (`lipo` of both architectures): the bundle does not care.
#
# Signing: with `MACOS_SIGN_IDENTITY` set the bundle is signed with that Developer ID and the zip is
# ready for notarisation; without it the signature is ad-hoc (`-`), which is what a build from a
# machine without a certificate can honestly do. An ad-hoc signature runs on the machine that built it
# and anywhere after the quarantine flag is taken off — the README says how, and says why.
set -euo pipefail

binary=${1:?the path to a built andy-ide}
version=${2:?the version, without the leading v}
out=${3:-dist}
root=$(cd "$(dirname "$0")/.." && pwd)
here=$(cd "$(dirname "$0")" && pwd)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

app="$work/Andy IDE.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
install -m755 "$binary" "$app/Contents/MacOS/andy-ide"
install -m644 "$root/assets/andy-ide.icns" "$app/Contents/Resources/andy-ide.icns"
sed -e "s/@VERSION@/${version}/g" -e "s/@YEAR@/$(date +%Y)/g" \
    "$here/Info.plist" > "$app/Contents/Info.plist"

identity=${MACOS_SIGN_IDENTITY:--}
echo "signing with: $identity"
codesign --force --deep --timestamp --options runtime --sign "$identity" "$app" 2>/dev/null ||
    codesign --force --deep --sign "$identity" "$app"

codesign --verify --deep --strict "$app" && echo "signature verified"

mkdir -p "$out"
name="andy-ide-${version}-universal.app"
rm -rf "${out:?}/$name"
cp -R "$app" "$out/$name"
# `ditto` keeps the extended attributes and the signature intact, which a plain `zip` does not: an
# archive made with `zip` unpacks into a bundle the system refuses to start.
ditto -c -k --keepParent --sequesterRsrc "$app" "$out/$name.zip"
echo "built $out/$name.zip"
