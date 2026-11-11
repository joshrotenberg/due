#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: bash scripts/smoke-package.sh TARGET REVISION ARCHIVE" >&2
  exit 2
fi

target=$1
revision=$2
archive=${3##*/}
cd "$(dirname "$3")"

host="$(uname -s):$(uname -m)"
case "$target:$host" in
  aarch64-apple-darwin:Darwin:arm64|x86_64-unknown-linux-gnu:Linux:x86_64) ;;
  *) echo "Cannot smoke-test $target on $host." >&2; exit 1 ;;
esac

actual_checksum=$(shasum -a 256 "$archive")
expected_checksum=$(cat "$archive.sha256")
if [ "$actual_checksum" != "$expected_checksum" ]; then
  echo "SHA-256 check failed for $archive." >&2
  exit 1
fi
echo "Verified SHA-256 for $archive"

members=$(tar -tzf "$archive" | LC_ALL=C sort)
if [ "$members" != $'LICENSE-APACHE\nLICENSE-MIT\ndue' ]; then
  echo "Unexpected archive contents for $archive: $members" >&2
  exit 1
fi

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
tar -xzf "$archive" -C "$scratch"
cd "$scratch"
test -s LICENSE-APACHE
test -s LICENSE-MIT
PATH='' ./due build-info --output json > build-info.json
python3 - "$revision" "$target" "$archive" <<'PY'
import json
import sys
from pathlib import Path

revision, target, archive = sys.argv[1:]
info = json.loads(Path("build-info.json").read_text())
assert info["package"]["name"] == "due", info
version = info["package"]["version"]
assert archive == f"due-v{version}-{target}.tar.gz", (archive, info)
assert info["source"] == {"status": "recorded", "revision": revision, "tree": "clean"}, info
assert {"harness", "rpc", "mcp-stdio"} <= set(info["features"]), info
print(json.dumps(info))
PY
for command in run init rpc script mcp; do
  PATH='' ./due "$command" --help > /dev/null
done
PATH='' ./due skills show mcp > /dev/null
