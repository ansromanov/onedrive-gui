#!/usr/bin/env bash
# Every check in one go: unit tests, daemon end-to-end, Dolphin overlay. Never touches a real account.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cargo test --locked --manifest-path "$root/daemon/Cargo.toml"
"$root/daemon/tests/smoke.sh"
"$root/dolphin/tests/check.sh"
