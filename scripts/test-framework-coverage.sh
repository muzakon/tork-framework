#!/bin/zsh

set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "cargo-llvm-cov is required. Install it with: cargo install cargo-llvm-cov"
  exit 1
fi

mkdir -p target/coverage

cargo llvm-cov \
  --workspace \
  --exclude http-parity \
  --exclude my_api \
  --lcov \
  --output-path target/coverage/framework.lcov

cargo llvm-cov report \
  --package tork \
  --package tork-core \
  --package tork-macros \
  --package tork-openapi \
  --summary-only
