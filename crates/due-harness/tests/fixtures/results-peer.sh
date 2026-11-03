#!/bin/sh
# Keep the executable immutable while tests vary only their private data files.
set -eu
while IFS= read -r line; do :; done
IFS= read -r status < exit-code
exit "$status"
