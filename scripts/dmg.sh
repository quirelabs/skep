#!/usr/bin/env bash
# Wraps Skep.app in a disk image somebody can drag.
#
#   scripts/dmg.sh [path/to/Skep.app] [output.dmg]
#
# Plain on purpose: the app, a link to Applications, and nothing else. A
# background picture and a laid out window need a .DS_Store built by driving
# Finder over AppleScript, which fails differently on a headless runner than
# it does on a desk. That can come later; what this has to do first is be the
# same file every time it is built.
set -euo pipefail

cd "$(dirname "$0")/.."

app="${1:-target/bundle/Skep.app}"
out="${2:-target/bundle/Skep.dmg}"
test -d "$app" || {
	echo "no app at ${app}; run scripts/bundle.sh first" >&2
	exit 1
}

staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT

cp -R "$app" "${staging}/"
# The other half of the gesture. Without it the window is a file you drag
# nowhere in particular.
ln -s /Applications "${staging}/Applications"

rm -f "$out"
hdiutil create \
	-volname "Skep" \
	-srcfolder "$staging" \
	-fs HFS+ \
	-format UDZO \
	-quiet \
	"$out"

# Signed as well as the app inside it. Notarisation takes the image, and an
# unsigned container is refused before anything in it is looked at.
if [ -n "${SKEP_SIGN_IDENTITY:-}" ]; then
	stamp="--timestamp"
	[ "$SKEP_SIGN_IDENTITY" = "-" ] && stamp="--timestamp=none"
	codesign --force $stamp --sign "$SKEP_SIGN_IDENTITY" "$out"
	codesign --verify --verbose=2 "$out"
fi

echo "$out"
