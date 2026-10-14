#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
# Use the caller-selected Rust toolchain; never depend on one developer's home.
if [ -n "${FINOCCHIO_TOOLCHAIN_BIN:-}" ]; then
    export PATH="$FINOCCHIO_TOOLCHAIN_BIN:$PATH"
fi
export TMPDIR="$root/target/finocchio-tmp"
mkdir -p "$TMPDIR"
if command -v shasum >/dev/null 2>&1; then
    compiler_hash=$(shasum -a 256 "$root/assets/vendor/fennel/fennel-1.6.1.lua")
elif command -v sha256sum >/dev/null 2>&1; then
    compiler_hash=$(sha256sum "$root/assets/vendor/fennel/fennel-1.6.1.lua")
else
    echo "Finocchio acceptance requires shasum or sha256sum" >&2
    exit 1
fi
if [ "${compiler_hash%% *}" != c3d45602041e7d8ef8a212563573df040c48a85c648a29fb4597ebed4bc38ec2 ]; then
    echo "Finocchio vendor SHA256 mismatch" >&2
    exit 1
fi
if ! rustc --print cfg | grep -qx unix; then
    echo "Finocchio ownership checks require a working Unix Rust toolchain (1.90+); cannot skip them" >&2
    exit 1
fi
checked_tests() {
    expected=$1
    log=$2
    shift 2
    if cargo test --offline --locked "$@" >"$log" 2>&1; then
        cat "$log"
    else
        cat "$log" >&2
        echo "Finocchio required checks failed; verify Rust 1.90+, Unix support and cached locked crates" >&2
        exit 1
    fi
    if ! grep -q "^test result: ok\. $expected passed; 0 failed; 0 ignored;" "$log"; then
        echo "Finocchio expected $expected passing tests: filter/cfg/toolchain mismatch; zero or skipped tests are not acceptance" >&2
        exit 1
    fi
}
export CARGO_TARGET_DIR="$root/target/finocchio"
# 32 original language tests + 29 integrated tests. One invocation runs them all;
# a missing test binary/cfg must not silently shrink the acceptance surface.
checked_tests 29 "$TMPDIR/spike-tests.log" --manifest-path "$root/spikes/finocchio/Cargo.toml"
spike_total=$(awk '/^test result: ok\./ { sum += $4 } END { print sum+0 }' "$TMPDIR/spike-tests.log")
if [ "$spike_total" != 61 ]; then
    echo "Finocchio expected 61 spike tests (32 prior + 29 integrated), got $spike_total" >&2
    exit 1
fi
export CARGO_TARGET_DIR="$root/target/finocchio-due"
checked_tests 3 "$TMPDIR/ownership-tests.log" --manifest-path "$root/Cargo.toml" \
    --features workflow-fennel-spike --test workflow_composition finocchio::
# Ensure the production-adjacent compiler's no-user-macros boundary stays intact.
checked_tests 1 "$TMPDIR/compiler-regression.log" --manifest-path "$root/Cargo.toml" \
    --features workflow-fennel-spike --test workflow_fennel \
    compile_time_surfaces_are_rejected_and_ambient_paths_are_ignored -- --exact
