#!/usr/bin/env bash
# Assembles Skep.app.
#
# The window hosts the engine, so the command line belongs inside the same
# bundle: one thing to install, and the command and the window can never be
# two different versions of skep. Settings looks for them beside the running
# executable, which is Contents/MacOS here and target/debug in a checkout.
#
#   scripts/bundle.sh [--debug] [--sign] [output directory]
#
# --debug builds the way `cargo run` does, which is what you want while
# working on the app: the same bundle, the same icon, in seconds rather than a
# minute. Releases are built without it.
#
# Signing is off unless asked for, so a checkout builds a runnable app with no
# certificate and no account. Notarisation is a separate step and needs both.
set -euo pipefail

cd "$(dirname "$0")/.."

sign=""
out="target/bundle"
profile="release"
while [ $# -gt 0 ]; do
	case "$1" in
	--sign) sign="${SKEP_SIGN_IDENTITY:--}" ;;
	--debug) profile="debug" ;;
	*) out="$1" ;;
	esac
	shift
done

version="$(cargo metadata --no-deps --format-version 1 |
	python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')"

echo "building skep ${version} (${profile})"
if [ "$profile" = "release" ]; then
	cargo build --release -p skep-app -p skep-cli -p skep-mcp -p skep-helper
else
	cargo build -p skep-app -p skep-cli -p skep-mcp -p skep-helper
fi

app="${out}/Skep.app"
rm -rf "$app"
mkdir -p "${app}/Contents/MacOS" "${app}/Contents/Resources"

# Everything a person installs at once. The helper is here to be found by
# `skep domains install`, which copies it somewhere privileged itself.
for binary in skep-app skep skep-mcp skep-helper; do
	cp "target/${profile}/${binary}" "${app}/Contents/MacOS/"
done
cp app/skep/assets/skep.icns "${app}/Contents/Resources/"
sed "s/__VERSION__/${version}/g" app/skep/Info.plist >"${app}/Contents/Info.plist"
# Four bytes that predate Info.plist and that Finder still reads.
printf 'APPL????' >"${app}/Contents/PkgInfo"

plutil -lint "${app}/Contents/Info.plist" >/dev/null

if [ -n "$sign" ]; then
	echo "signing as ${sign}"
	# Inside out, one binary at a time, rather than --deep. Apple has
	# discouraged --deep for years: it guesses at what is nested and applies
	# the outer entitlements to all of it, and the failures it causes surface
	# at notarisation rather than here. The command line tools inside this
	# bundle are ordinary binaries and have to be signed as such first.
	#
	# A timestamp needs a real identity and the network, so it is asked for
	# only when there is one. Hardened runtime is not optional: notarisation
	# refuses anything without it.
	stamp="--timestamp"
	[ "$sign" = "-" ] && stamp="--timestamp=none"
	for binary in skep skep-mcp skep-helper; do
		codesign --force $stamp --options runtime \
			--sign "$sign" "${app}/Contents/MacOS/${binary}"
	done
	codesign --force $stamp --options runtime --sign "$sign" "$app"
	codesign --verify --strict --verbose=2 "$app"
fi

echo "${app}"
