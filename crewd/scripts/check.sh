#!/bin/sh
# crewd 的唯一检查入口：格式、lint、测试。任何一步失败即非 0 退出。
set -eu
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
