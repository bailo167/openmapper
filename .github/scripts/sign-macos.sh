#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Code-signs and notarises the staged macOS release (docs/release/signing.md).
# Run by `cargo xtask dist` through OM_SIGN_COMMAND with the staging
# directory as its argument, only when the publisher's secrets exist:
#
#   APPLE_CERTIFICATE_P12        base64 of the Developer ID Application .p12
#   APPLE_CERTIFICATE_PASSWORD   its password
#   APPLE_SIGNING_IDENTITY       e.g. "Developer ID Application: Name (TEAMID)"
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD   notarytool credentials
#
# Untested until the owner has an Apple Developer account; the steps follow
# Apple's documented notarytool workflow for command-line tools.
set -euo pipefail

stage="${1:?staging directory}"
for v in APPLE_CERTIFICATE_P12 APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY APPLE_ID APPLE_TEAM_ID APPLE_APP_PASSWORD; do
    [ -n "${!v:-}" ] || { echo "sign-macos: $v is not set" >&2; exit 1; }
done

work="$(mktemp -d)"
keychain="$work/signing.keychain-db"
keychain_password="$(uuidgen)"
cleanup() {
    security delete-keychain "$keychain" 2>/dev/null || true
    rm -rf "$work"
}
trap cleanup EXIT

echo "$APPLE_CERTIFICATE_P12" | base64 --decode > "$work/cert.p12"
security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 1800 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$work/cert.p12" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" \
    -T /usr/bin/codesign -T /usr/bin/security
security set-key-partition-list -S apple-tool:,apple: -s -k "$keychain_password" "$keychain" > /dev/null
security list-keychains -d user -s "$keychain" $(security list-keychains -d user | tr -d '"')

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
entitlements="$here/entitlements.plist"

# Libraries first, then the executables that load them. Hardened runtime
# with the JIT entitlements the WebAssembly plugin host needs.
find "$stage/lib" -name '*.dylib' -print0 | while IFS= read -r -d '' f; do
    codesign --force --timestamp --options runtime --sign "$APPLE_SIGNING_IDENTITY" "$f"
done
for bin in openmapper openmapper-cli; do
    codesign --force --timestamp --options runtime --entitlements "$entitlements" \
        --sign "$APPLE_SIGNING_IDENTITY" "$stage/$bin"
done
codesign --verify --strict --verbose=2 "$stage/openmapper" "$stage/openmapper-cli"

# Notarise the whole staged folder (command-line tools cannot be stapled;
# Gatekeeper checks the notarisation record online).
ditto -c -k --keepParent "$stage" "$work/stage.zip"
xcrun notarytool submit "$work/stage.zip" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
    --password "$APPLE_APP_PASSWORD" --wait
echo "sign-macos: signed and notarised $(basename "$stage")"
