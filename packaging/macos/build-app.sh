#!/usr/bin/env bash
# Build DLightroom.app for macOS without any signing credentials, and zip it:
#
#   $DIST/DLightroom-<version>-macos-<arch>.zip    DLightroom.app (ad-hoc signed, not notarized)
#
# The bundle holds the desktop app (Contents/MacOS/DLightroom) and the headless CLI
# (Contents/MacOS/vectorcraft-cli). It is ad-hoc signed only: Gatekeeper asks before the first
# start on another Mac (see README.md › Install). For a signed, notarized DMG use package.sh.
#
# Usage: packaging/macos/build-app.sh [--arch x86_64|aarch64|universal] [--skip-build]
#   --arch        default: the build machine's (x86_64 on an Intel Mac)
#   --skip-build  reuse target/<triple>/release from an earlier build
#
# Needs Xcode (or its command line tools) for cargo's linker, codesign, plutil and ditto.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/macos"

case "$(uname -m)" in
  arm64) ARCH=aarch64 ;;
  *) ARCH=x86_64 ;;
esac
SKIP_BUILD=0
while [ $# -gt 0 ]; do
  case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    -h | --help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$ARCH" in
  universal) TARGETS=(aarch64-apple-darwin x86_64-apple-darwin) ;;
  aarch64) TARGETS=(aarch64-apple-darwin) ;;
  x86_64) TARGETS=(x86_64-apple-darwin) ;;
  *) echo "unknown --arch $ARCH" >&2; exit 2 ;;
esac

# Keep in sync with LSMinimumSystemVersion in Info.plist.in (macOS 11 Big Sur and later, which
# covers an Intel MacBook Pro 2019 on macOS 15 Sequoia).
export MACOSX_DEPLOYMENT_TARGET=11.0
SHORT_VERSION="${VERSION%%-*}"
WORK="$CARGO_TARGET_DIR/macos-app"
APP="$WORK/DLightroom.app"
ZIP="$DIST/DLightroom-$VERSION-macos-$ARCH.zip"

echo "==> DLightroom $VERSION for macOS ($ARCH), unsigned (ad-hoc)"

if [ "$SKIP_BUILD" = 0 ]; then
  args=()
  for t in "${TARGETS[@]}"; do args+=(--target "$t"); done
  (cd "$ROOT" && cargo build --release --locked -p vectorcraft -p vectorcraft-cli "${args[@]}")
fi

rm -rf "$WORK"
mkdir -p "$WORK/bin"
for bin in vectorcraft vectorcraft-cli; do
  inputs=()
  for t in "${TARGETS[@]}"; do inputs+=("$CARGO_TARGET_DIR/$t/release/$bin"); done
  lipo -create -output "$WORK/bin/$bin" "${inputs[@]}"
  lipo -info "$WORK/bin/$bin"
done

echo "==> assembling $APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
# Executable and icon carry the display name (CFBundleExecutable / CFBundleIconFile).
cp "$WORK/bin/vectorcraft" "$APP/Contents/MacOS/DLightroom"
cp "$WORK/bin/vectorcraft-cli" "$APP/Contents/MacOS/vectorcraft-cli"
cp "$ROOT/assets/app-icon/dlightroom.icns" "$APP/Contents/Resources/DLightroom.icns"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/$SHORT_VERSION/g" \
  -e "s/@BUILD_SHA@/${VECTORCRAFT_BUILD_SHA:-unknown}/g" \
  "$HERE/Info.plist.in" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' >"$APP/Contents/PkgInfo"
# Licences and attributions travel with the app (README, LICENSE-*, NOTICE, ASSETS.md, licenses/).
copy_docs "$APP/Contents/Resources"

# Ad-hoc signatures, inside-out: no identity or certificate needed. Apple silicon refuses to run
# unsigned arm64 code at all; on Intel this keeps the bundle's seal consistent.
codesign --force --sign - --timestamp=none "$APP/Contents/MacOS/vectorcraft-cli"
codesign --force --sign - --timestamp=none "$APP/Contents/MacOS/DLightroom"
codesign --force --sign - --timestamp=none "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"

# Smoke tests that need no display: the bundle's identity, the app prints its usage, the CLI its
# version and renders the examples.
SMOKE="$WORK/smoke"
mkdir -p "$SMOKE"
for key in CFBundleName CFBundleExecutable CFBundleIdentifier CFBundleShortVersionString; do
  echo "$key: $(/usr/libexec/PlistBuddy -c "Print :$key" "$APP/Contents/Info.plist")"
done
VECTORCRAFT_NO_PREFS=1 "$APP/Contents/MacOS/DLightroom" --help >"$SMOKE/help.txt"
grep -q "DLightroom" "$SMOKE/help.txt"
"$APP/Contents/MacOS/vectorcraft-cli" --version
"$APP/Contents/MacOS/vectorcraft-cli" convert "$ROOT/examples/dusk-poster.svg" "$SMOKE/dusk-poster.png" >/dev/null
"$APP/Contents/MacOS/vectorcraft-cli" convert "$ROOT/examples/dusk-poster.vectorcraft" "$SMOKE/dusk-poster.pdf" >/dev/null
test -s "$SMOKE/dusk-poster.png" && test -s "$SMOKE/dusk-poster.pdf"
echo "smoke test: rendered PNG and PDF from the examples"

rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
echo "==> done"
ls -lh "$ZIP"
