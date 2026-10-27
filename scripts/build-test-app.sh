#!/bin/bash
# Builds the dev Mac's test app (`--features bench`, identifier com.kreokit.rec.claude-test)
# into test-build/ and signs it with the local test-only certificate "KreoKit Rec Test
# (local)" from the login keychain. macOS then ties Screen Recording, Microphone and Camera
# to that certificate instead of the binary, so the permissions survive rebuilds.
# Never used for releases. CARGO_TARGET_DIR must point outside the repo (the scratchpad).
set -euo pipefail
: "${CARGO_TARGET_DIR:?set CARGO_TARGET_DIR to a folder outside the repo}"
cd "$(dirname "$0")/.."
identity="KreoKit Rec Test (local)"
pnpm tauri build --debug --bundles app --features bench --config scripts/test-app.json
built="$CARGO_TARGET_DIR/debug/bundle/macos/KreoKit Rec Test.app"
codesign --force --deep --sign "$identity" "$built"
codesign -d -r- "$built" 2>&1 | grep -q 'certificate leaf' || {
    echo "The test app isn't signed with the test certificate; its permissions would reset." >&2
    exit 1
}
mkdir -p test-build
rm -rf "test-build/KreoKit Rec Test.app"
ditto "$built" "test-build/KreoKit Rec Test.app"
echo "test-build/KreoKit Rec Test.app"
