#!/usr/bin/env bash
# Pins one service release for the catalog: downloads it, records the hash we
# will verify against from then on, and prints the entry to paste in.
#
#   scripts/pin-release.sh mailpit 1.31.0
#   scripts/pin-release.sh postgres 17.6.0 macos-arm64
#
# The hash this prints is the trust root. It is computed here, once, and every
# later download is checked against it, so a changed upstream asset fails loudly
# instead of being accepted. That matters most for postgres, which comes from a
# third-party redistribution rather than postgresql.org.

set -euo pipefail

service=${1:-}
version=${2:-}
platform=${3:-macos-arm64}

if [[ -z $service || -z $version ]]; then
    echo "usage: $0 <service> <version> [platform]" >&2
    echo "services:  mailpit postgres mysql mongodb valkey" >&2
    echo "platforms: macos-arm64 (linux-arm64 and linux-x86_64 to come)" >&2
    exit 2
fi

# Keyed on service and platform together. No template hardcodes an
# architecture, so adding Linux is adding rows here.
case "$service:$platform" in
mailpit:macos-arm64)
    url="https://github.com/axllent/mailpit/releases/download/v${version}/mailpit-darwin-arm64.tar.gz"
    ;;
mailpit:linux-arm64)
    url="https://github.com/axllent/mailpit/releases/download/v${version}/mailpit-linux-arm64.tar.gz"
    ;;
mailpit:linux-x86_64)
    url="https://github.com/axllent/mailpit/releases/download/v${version}/mailpit-linux-amd64.tar.gz"
    ;;
postgres:macos-arm64)
    url="https://github.com/theseus-rs/postgresql-binaries/releases/download/${version}/postgresql-${version}-aarch64-apple-darwin.tar.gz"
    ;;
postgres:linux-arm64)
    url="https://github.com/theseus-rs/postgresql-binaries/releases/download/${version}/postgresql-${version}-aarch64-unknown-linux-gnu.tar.gz"
    ;;
postgres:linux-x86_64)
    url="https://github.com/theseus-rs/postgresql-binaries/releases/download/${version}/postgresql-${version}-x86_64-unknown-linux-gnu.tar.gz"
    ;;
mysql:macos-arm64)
    url="https://dev.mysql.com/get/Downloads/MySQL-${version%.*}/mysql-${version}-macos15-arm64.tar.gz"
    ;;
mongodb:macos-arm64)
    url="https://fastdl.mongodb.org/osx/mongodb-macos-arm64-${version}.tgz"
    ;;
cloudflared:macos-arm64)
    url="https://github.com/cloudflare/cloudflared/releases/download/${version}/cloudflared-darwin-arm64.tgz"
    ;;
valkey:*)
    # Source only: upstream ships no prebuilt binaries for any platform.
    url="https://github.com/valkey-io/valkey/archive/refs/tags/${version}.tar.gz"
    ;;
*)
    echo "no source known for $service on $platform" >&2
    exit 2
    ;;
esac

archive=$(mktemp -t skep-pin)
trap 'rm -f "$archive"' EXIT

echo "fetching $url" >&2
curl --fail --location --silent --show-error --proto '=https' --tlsv1.2 --output "$archive" "$url"

sha256=$(shasum -a 256 "$archive" | cut -d' ' -f1)
size=$(wc -c <"$archive" | tr -d ' ')

echo >&2
echo "archive contents:" >&2
tar tzf "$archive" | head -12 | sed 's/^/  /' >&2
echo >&2

# One shared top-level directory means the payload is wrapped and must be
# stripped. Inferring it beats remembering it per service.
roots=$(tar tzf "$archive" | cut -d/ -f1 | sort -u | wc -l | tr -d ' ')
if [[ $roots == 1 ]]; then strip=1; else strip=0; fi

cat <<ENTRY
Release {
    version: Version::new("$version")?,
    platform: Platform::$(python3 -c "
import sys
print({'macos-arm64':'MacosArm64','macos-x86_64':'MacosX8664','linux-arm64':'LinuxArm64','linux-x86_64':'LinuxX8664'}['$platform'])"),
    url: "$url".to_string(),
    sha256: "$sha256".to_string(),
    size: $size,
    strip_components: $strip,
}
ENTRY
