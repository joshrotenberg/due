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
cargo build --locked --release --features mcp-stdio --target "$target" --target-dir "$build_dir"
binary="$build_dir/$target/release/due"
version=$("$binary" --version)
version=${version#due }

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir "$scratch/package" "$scratch/unpacked"
cp "$binary" LICENSE-APACHE LICENSE-MIT "$scratch/package"
archive="due-v$version-$target.tar.gz"
tar -czf "$output_dir/$archive" -C "$scratch/package" due LICENSE-APACHE LICENSE-MIT

cd "$output_dir"
shasum -a 256 "$archive" > "$archive.sha256"
shasum -a 256 -c "$archive.sha256"
tar -xzf "$archive" -C "$scratch/unpacked"

cd "$scratch/unpacked"
PATH='' ./due build-info --output json > build-info.json
python3 - "$revision" "$version" <<'PY'
import json
import sys
from pathlib import Path

info = json.loads(Path("build-info.json").read_text())
assert info["package"]["version"] == sys.argv[2], info
assert info["source"] == {"status": "recorded", "revision": sys.argv[1], "tree": "clean"}, info
assert {"harness", "rpc", "mcp-stdio"} <= set(info["features"]), info
print(json.dumps(info))
PY
for command in run init rpc script mcp; do
  PATH='' ./due "$command" --help > /dev/null
done
PATH='' ./due skills show mcp > /dev/null
