#!/usr/bin/env bash
# Local parity with CI — run the same gates before pushing (docs/CICD.md).
set -euo pipefail
cd "$(dirname "$0")/.."
echo "==> fmt";        cargo fmt --all -- --check
echo "==> clippy";     cargo clippy --all-targets --all-features -- -D warnings
echo "==> boundaries"; ./scripts/check-crate-boundaries.sh
echo "==> secrets";    ./scripts/scan-secrets.sh
echo "==> tests";      cargo test --workspace --all-features
echo "All local gates passed."
