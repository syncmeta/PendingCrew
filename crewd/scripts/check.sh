#!/bin/sh
# crewd 的唯一检查入口：格式、lint、测试。任何一步失败即非 0 退出。
# 本机和 CI（.github/workflows/crewd.yml）都调这一个脚本，所以两边不会漂移。
#
# CREWD_TEST_RUNNER=nextest   用 cargo-nextest（`ci` 档：失败重试 2 次、抖动即红、出 JUnit）
#                             + 单独跑 doctest（nextest 不跑 doctest）。
#                             缺省用 `cargo test --workspace`，本机日常不需要装 nextest。
# 永远不跑 `--ignored`：那些是要真 claude/codex/付费 API/钥匙串的活测试。
set -eu
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
case "${CREWD_TEST_RUNNER:-cargo}" in
  nextest)
    cargo nextest run --workspace --profile ci
    cargo test --workspace --doc
    ;;
  cargo)
    cargo test --workspace
    ;;
  *)
    echo "scripts/check.sh: CREWD_TEST_RUNNER 只认 cargo | nextest，收到 '${CREWD_TEST_RUNNER}'" >&2
    exit 2
    ;;
esac
