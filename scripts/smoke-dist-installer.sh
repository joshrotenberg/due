#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: bash scripts/smoke-dist-installer.sh ARCHIVE INSTALLER" >&2
  exit 2
fi

archive=$1
installer=$2
sh -n "$installer"
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
system_path=/usr/bin:/bin:/usr/sbin:/sbin
mkdir "$scratch/tmp"

run_installer() {
  local home=$1 archive_dir=$2 url
  mkdir "$home" || return
  url=$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).resolve().as_uri())' "$archive_dir") || return
  env -i HOME="$home" XDG_CONFIG_HOME="$home/.config" TMPDIR="$scratch/tmp" \
    PATH="$system_path" DUE_CLI_DOWNLOAD_URL="$url" sh "$installer"
}

home="$scratch/home"
run_installer "$home" "$(dirname "$archive")"
installed="$home/.local/bin/due"
package_dir=${archive##*/}
tar -xOf "$archive" "${package_dir%.tar.gz}/due" | cmp - "$installed"
# Expand HOME and PATH inside the disposable shell.
# shellcheck disable=SC2016
env -i HOME="$home" XDG_CONFIG_HOME="$home/.config" PATH="$system_path" sh -ec '
  . "$HOME/.profile"
  test "$(command -v due)" = "$HOME/.local/bin/due"
  due --version
'

if PATH="$system_path" command -v sha256sum >/dev/null; then
  mkdir "$scratch/corrupt"
  cp "$archive" "$scratch/corrupt/"
  printf 'tampered' >> "$scratch/corrupt/$package_dir"
  if run_installer "$scratch/rejected" "$scratch/corrupt" > "$scratch/rejected.log" 2>&1; then
    echo "The installer accepted a corrupted archive." >&2
    exit 1
  fi
  if ! grep -q 'checksum mismatch' "$scratch/rejected.log"; then
    cat "$scratch/rejected.log" >&2
    exit 1
  fi
  test ! -e "$scratch/rejected/.local/bin/due"
  echo "Installer rejected a corrupted archive"
else
  echo "Installer checksum rejection skipped because stock PATH has no sha256sum"
fi
