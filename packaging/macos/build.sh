#!/usr/bin/env bash
#
# Wrap a built superbackup in a .app bundle and a .dmg.
#
# It does not build the executable. The release workflow builds each target on
# its own runner and this runs afterwards, so the installer wraps exactly the
# binary that was tested and attested rather than a second one built here.
#
# ## Signing
#
# Signed and notarised when the credentials are in the environment, ad-hoc
# signed when they are not. The difference matters and is stated rather than
# hidden: an unsigned build is refused by Gatekeeper on first open with
# "superbackup is damaged and can't be opened", which is a lie about the file
# and a support case every single time. An ad-hoc signature does not fix that
# — only a Developer ID and notarisation do — but it does keep the bundle from
# being rejected outright by the hardened runtime on the machine that built it.
#
#   MACOS_CERTIFICATE / MACOS_CERTIFICATE_PASSWORD   the Developer ID, base64
#   MACOS_SIGN_IDENTITY                              "Developer ID Application: …"
#   MACOS_NOTARY_PROFILE                             a notarytool keychain profile
#
# Usage: build.sh <path to superbackup> <version> <output dmg>
set -euo pipefail

exe="${1:?the built superbackup executable}"
version="${2:?the version}"
out="${3:?where to write the .dmg}"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT

app="$staging/superbackup.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

sed "s/__VERSION__/${version#v}/g" "$root/packaging/macos/Info.plist" \
    > "$app/Contents/Info.plist"
cp "$exe" "$app/Contents/MacOS/superbackup"
chmod 755 "$app/Contents/MacOS/superbackup"
cp "$root/assets/icons/superbackup.icns" "$app/Contents/Resources/superbackup.icns"
cp "$root/LICENSE" "$app/Contents/Resources/LICENSE"

# --- signing -----------------------------------------------------------------
if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
    echo "signing with $MACOS_SIGN_IDENTITY"
    # `--options runtime` is required for notarisation; `--timestamp` is
    # required for the signature to remain valid after the certificate expires.
    codesign --force --deep --options runtime --timestamp \
        --sign "$MACOS_SIGN_IDENTITY" "$app"
    codesign --verify --strict --verbose=2 "$app"
else
    echo "::warning::no MACOS_SIGN_IDENTITY; signing ad-hoc." >&2
    echo "::warning::Gatekeeper will refuse this build on another Mac until it is signed with a Developer ID and notarised." >&2
    codesign --force --deep --sign - "$app"
fi

# --- the disk image ----------------------------------------------------------
image="$staging/image"
mkdir -p "$image"
cp -R "$app" "$image/"
# The conventional drag-to-install layout: the bundle and a link to where it
# goes. Without the link people copy it to Downloads and run it from there,
# which works until the folder is cleaned out from under a running service.
ln -s /Applications "$image/Applications"

rm -f "$out"
hdiutil create \
    -volname "superbackup ${version#v}" \
    -srcfolder "$image" \
    -ov -format UDZO \
    "$out"

if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
    codesign --force --sign "$MACOS_SIGN_IDENTITY" "$out"
fi

# --- notarisation ------------------------------------------------------------
if [ -n "${MACOS_NOTARY_PROFILE:-}" ]; then
    echo "submitting for notarisation"
    xcrun notarytool submit "$out" --keychain-profile "$MACOS_NOTARY_PROFILE" --wait
    # Stapling puts the ticket in the file, so a Mac with no network can still
    # verify it. Without this, a first open offline fails.
    xcrun stapler staple "$out"
    xcrun stapler validate "$out"
else
    echo "::warning::no MACOS_NOTARY_PROFILE; this disk image is not notarised." >&2
fi

echo "built $out"
