#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: bash scripts/rehearse-package.sh OUTPUT_DIR" >&2
  exit 2
fi

output_dir=$(cd "$1" && pwd -P)
cd "$(git rev-parse --show-toplevel)"
if [ -n "$(git status --porcelain)" ]; then
  echo "Use a clean checkout for the packaging rehearsal." >&2
  exit 1
fi

revision=$(git rev-parse HEAD)
target=$(rustc -vV | sed -n 's/^host: //p')
build_dir=${CARGO_TARGET_DIR:-target}
cargo build --locked -p due-cli --release --features mcp-stdio --target "$target" --target-dir "$build_dir"
binary="$build_dir/$target/release/due"
version=$("$binary" --version)
version=${version#due }

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir "$scratch/package"
cp "$binary" LICENSE-APACHE LICENSE-MIT "$scratch/package"
archive="due-v$version-$target.tar.gz"
tar -czf "$output_dir/$archive" -C "$scratch/package" due LICENSE-APACHE LICENSE-MIT

(cd "$output_dir" && shasum -a 256 "$archive" > "$archive.sha256")
bash scripts/smoke-package.sh "$target" "$revision" "$output_dir/$archive"
