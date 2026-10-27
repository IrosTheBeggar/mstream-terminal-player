#!/usr/bin/env bash
# Assemble "mStream Player.app" around a desktop-flavour binary.
#
#   scripts/package-macos.sh <binary> <icns> <version> <out dir>
#
# Writes <out dir>/mStream Player.app and nothing else: no signing, no zip.
# release.yml's package-desktop calls it, then signs, notarizes, staples
# and zips the bundle when the signing secrets exist; the same script runs
# on a dev Mac for the local check (docs/window-spike/README.md).
#
# <version> is the release's full version without the v ("0.10.0" or
# "0.10.0-rc.1"). Needs macOS for `otool` and `plutil`.
set -eo pipefail

if [ "$#" -ne 4 ]; then
  echo "usage: $0 <binary> <icns> <version> <out dir>" >&2
  exit 2
fi
binary="$1"; icns="$2"; version="$3"; out="$4"

# The app's identity, from src/identity.rs: the bundle id is the codesign
# identifier the bare binaries already carry, and the executable and icon
# names are the ones a desktop entry and the hicolor theme use on Linux.
app_id="io.mstream.player"
app_name="mStream Player"
exe_name="mstream-player"
# The mDNS service discovery browses (src/discovery.rs SERVICE_TYPE, less
# its ".local." domain): macOS 11+ asks before an app touches the local
# network, and lists only the services named here.
bonjour_service="_mstream._tcp"

# CFBundleShortVersionString and CFBundleVersion must be one to three
# period-separated integers; a pre-release ("-rc.1") or build ("+...")
# suffix is not, so those keys carry the numeric core and the full string
# goes in MStreamPlayerVersion (a key of our own, read by nothing but a
# person running `defaults read`/`plutil -p` on the bundle).
short_version="${version%%[-+]*}"
if ! [[ "$short_version" =~ ^[0-9]+(\.[0-9]+){0,2}$ ]]; then
  echo "error: version '$version' has no X.Y.Z core for CFBundleShortVersionString" >&2
  exit 1
fi

# LSMinimumSystemVersion is what the binary itself was linked for (its
# LC_BUILD_VERSION minos, which rustc sets from the target's default
# deployment target: 11.0 for aarch64-apple-darwin, 10.12 for x86_64 on
# rustc 1.98), so the bundle cannot promise an older macOS than the code
# inside it accepts, and no table here can drift from the toolchain.
min_os=$(otool -l "$binary" | awk '/LC_BUILD_VERSION/ {b=1} b && $1=="minos" {print $2; exit}')
if [ -z "$min_os" ]; then
  # Older linkers wrote LC_VERSION_MIN_MACOSX instead.
  min_os=$(otool -l "$binary" | awk '/LC_VERSION_MIN_MACOSX/ {b=1} b && $1=="version" {print $2; exit}')
fi
if [ -z "$min_os" ]; then
  echo "error: no minimum macOS version in $binary — is it a Mach-O?" >&2
  exit 1
fi

# CFBundleIconFile names the icns WITH its extension: macOS appends
# ".icns" only to a name that has no extension, and "io.mstream.player"
# reads as one ending in ".player", so the bare id leaves the bundle with
# the generic app icon (seen on macOS 26 while the running app's Dock tile,
# which src/gui/window/icon.rs sets at run time, still showed the logo).

app="$out/$app_name.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

install -m 755 "$binary" "$app/Contents/MacOS/$exe_name"
install -m 644 "$icns" "$app/Contents/Resources/$app_id.icns"
printf 'APPL????' > "$app/Contents/PkgInfo"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>en</string>
	<key>CFBundleDisplayName</key>
	<string>$app_name</string>
	<key>CFBundleExecutable</key>
	<string>$exe_name</string>
	<key>CFBundleIconFile</key>
	<string>$app_id.icns</string>
	<key>CFBundleIdentifier</key>
	<string>$app_id</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleName</key>
	<string>$app_name</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>$short_version</string>
	<key>CFBundleVersion</key>
	<string>$short_version</string>
	<key>MStreamPlayerVersion</key>
	<string>$version</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.music</string>
	<key>LSMinimumSystemVersion</key>
	<string>$min_os</string>
	<key>NSBonjourServices</key>
	<array>
		<string>$bonjour_service</string>
	</array>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSLocalNetworkUsageDescription</key>
	<string>mStream Player looks for mStream servers on your network.</string>
	<key>NSSupportsAutomaticGraphicsSwitching</key>
	<true/>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null

# No extended attributes into the signature or the zip: codesign refuses
# Finder info and resource forks ("detritus not allowed"), and ditto would
# carry any others as ._ files. Best effort: a sandboxed shell on a dev Mac
# stamps com.apple.provenance, which cannot be removed and is harmless
# (its ._ entries then show in a local zip; a CI runner has none).
xattr -cr "$app" 2>/dev/null || true

echo "$app (version $version, CFBundleShortVersionString $short_version, LSMinimumSystemVersion $min_os)"
