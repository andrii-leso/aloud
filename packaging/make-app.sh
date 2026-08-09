#!/usr/bin/env bash
# Hand-assembles target/Aloud.app from the release binary and the Swift OCR
# helper, without tauri-cli (see the task-6 report for why: `cargo install
# tauri-cli` on this machine drove free disk to 2.0 GB). A `.app` bundle is
# just a directory with a required layout, so this builds one directly:
#
#   Aloud.app/Contents/
#     Info.plist          <- CFBundle* keys here, plus NSServices merged in
#                             from the crate-root Info.plist (single source
#                             of truth — see the merge step below)
#     MacOS/aloud          <- the release binary
#     MacOS/aloud-ocr       <- the Swift OCR helper (MUST be here, not
#                             Resources/ — VisionOcr resolves it via
#                             current_exe().parent().join("aloud-ocr"), and
#                             the executable lives in Contents/MacOS/ in a
#                             bundle; see src/ocr/macos.rs:23-29)
#     Resources/icon.icns  <- app icon, built from icons/icon.png if the
#                             `iconutil`/`sips` toolchain succeeds
#
# One command produces a runnable app: this script builds the release
# `aloud` binary, runs helpers/macos-ocr/build.sh for the OCR helper, then
# assembles and ad-hoc codesigns the bundle.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP="$ROOT/target/Aloud.app"
CONTENTS="$APP/Contents"
MACOS_DIR="$CONTENTS/MacOS"
RESOURCES_DIR="$CONTENTS/Resources"

echo "==> disk before:"
df -h / | tail -1

echo "==> building release binary (aloud)"
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --bin aloud

echo "==> building the Swift OCR helper"
"$ROOT/helpers/macos-ocr/build.sh"

echo "==> assembling $APP"
rm -rf "$APP"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

install -m 0755 "$ROOT/target/release/aloud" "$MACOS_DIR/aloud"
install -m 0755 "$ROOT/target/aloud-ocr" "$MACOS_DIR/aloud-ocr"

# --- Icon (best-effort; the app is a fully valid bundle without it) -------
ICON_SRC="$ROOT/icons/icon.png"
ICNS_NAME=""
if [ -f "$ICON_SRC" ] && command -v iconutil >/dev/null 2>&1 && command -v sips >/dev/null 2>&1; then
    echo "==> building icon.icns from icons/icon.png"
    ICONSET="$ROOT/target/Aloud.iconset"
    rm -rf "$ICONSET"
    mkdir -p "$ICONSET"
    for size in 16 32 128 256 512; do
        sips -z "$size" "$size" "$ICON_SRC" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
        double=$((size * 2))
        sips -z "$double" "$double" "$ICON_SRC" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
    done
    if iconutil -c icns "$ICONSET" -o "$RESOURCES_DIR/icon.icns"; then
        ICNS_NAME="icon"
    else
        echo "==> iconutil failed; shipping without a Finder icon"
        rm -f "$RESOURCES_DIR/icon.icns"
    fi
    rm -rf "$ICONSET"
else
    echo "==> skipping icon (missing icons/icon.png or iconutil/sips)"
fi

# --- Info.plist -------------------------------------------------------------
# CFBundleIdentifier/Name/Executable/PackageType/version + LSUIElement are
# written directly. NSServices is NOT duplicated here — it is merged in
# from the crate-root Info.plist (the file src/selection/macos.rs's
# `NSMessage`/`readSelection` selector must stay in lockstep with) via
# PlistBuddy's Merge, so there is exactly one place that array is spelled
# out. If the two ever drifted, the Service would silently stop firing.
PLIST="$CONTENTS/Info.plist"
CRATE_PLIST="$ROOT/Info.plist"
plutil -create xml1 "$PLIST"

PB=/usr/libexec/PlistBuddy
"$PB" -c "Add :CFBundleIdentifier string com.andriileso.aloud" "$PLIST"
"$PB" -c "Add :CFBundleName string Aloud" "$PLIST"
"$PB" -c "Add :CFBundleDisplayName string Aloud" "$PLIST"
"$PB" -c "Add :CFBundleExecutable string aloud" "$PLIST"
"$PB" -c "Add :CFBundlePackageType string APPL" "$PLIST"
"$PB" -c "Add :CFBundleShortVersionString string 0.1.0" "$PLIST"
"$PB" -c "Add :CFBundleVersion string 1" "$PLIST"
"$PB" -c "Add :LSUIElement bool true" "$PLIST"
if [ -n "$ICNS_NAME" ]; then
    "$PB" -c "Add :CFBundleIconFile string $ICNS_NAME" "$PLIST"
fi

if [ ! -f "$CRATE_PLIST" ]; then
    echo "ERROR: $CRATE_PLIST (the NSServices source of truth) is missing" >&2
    exit 1
fi
"$PB" -c "Merge \"$CRATE_PLIST\" :" "$PLIST"

if ! /usr/bin/plutil -extract NSServices xml1 -o - "$PLIST" >/dev/null 2>&1; then
    echo "ERROR: NSServices did not make it into $PLIST" >&2
    exit 1
fi

# --- Codesign ----------------------------------------------------------------
#
# Prefer a stable self-signed identity over ad-hoc. This is not about Gatekeeper
# — it is about TCC. macOS binds permission grants (Screen Recording) to the
# app's designated requirement. Ad-hoc signing has no certificate, so the DR
# falls back to the binary's cdhash, and EVERY rebuild produces a new hash and
# silently invalidates the grant while System Settings still shows the app as
# enabled. That cost us most of a debugging session.
#
# With a self-signed cert the DR becomes:
#   identifier "com.andriileso.aloud" and certificate leaf = H"<cert hash>"
# which is stable across rebuilds.
#
# Create the identity once: Keychain Access -> Certificate Assistant ->
# Create a Certificate -> Self Signed Root, Code Signing, named as below.
# It does not need to be trusted; codesign accepts an untrusted self-signed
# cert. If it is absent we fall back to ad-hoc so the build still works.
# Not for Gatekeeper — it gives the app a stable identity so macOS's
# permission grants (Screen Recording) persist across rebuilds instead of
# re-prompting every time the binary's hash changes.
SIGN_IDENTITY="${ALOUD_SIGN_IDENTITY:-Aloud Dev}"
if security find-certificate -c "$SIGN_IDENTITY" >/dev/null 2>&1; then
  echo "==> codesigning $APP with \"$SIGN_IDENTITY\" (stable identity; TCC grants survive rebuilds)"
  codesign --force --deep --sign "$SIGN_IDENTITY" "$APP"
else
  echo "==> WARNING: signing identity \"$SIGN_IDENTITY\" not found — falling back to ad-hoc."
  echo "    Screen Recording permission will need re-granting after every rebuild."
  codesign --force --deep --sign - "$APP"
fi
codesign -dv "$APP"

echo "==> disk after:"
df -h / | tail -1

echo "==> built: $APP"
