#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: bash scripts/install-rehearsal-dist.sh BIN_DIR" >&2
  exit 2
fi

bin_dir=$1
version=0.34.0
case "$(uname -s):$(uname -m)" in
  Darwin:arm64)
    target=aarch64-apple-darwin
    checksum=d1eec30e63eed23e5a5bd48fe96eae314799811bffca254ec1d2264638ba1890
    ;;
  Linux:x86_64)
    target=x86_64-unknown-linux-gnu
    checksum=7fee44ff3ff71ed0064b32d4ec62fee6b58bd37c5887b12a86aedddc5af0c7b5
    ;;
  *) echo "No dist binary pinned for this host." >&2; exit 1 ;;
esac

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
archive="cargo-dist-$target.tar.xz"
curl --fail --silent --show-error --location --proto '=https' \
  "https://github.com/axodotdev/cargo-dist/releases/download/v$version/$archive" \
  --output "$scratch/$archive"
(cd "$scratch" && printf '%s  %s\n' "$checksum" "$archive" | shasum -a 256 -c -)
tar -xJf "$scratch/$archive" -C "$scratch"
mkdir -p "$bin_dir"
install -m 755 "$scratch/cargo-dist-$target/dist" "$bin_dir/dist"
"$bin_dir/dist" --version
