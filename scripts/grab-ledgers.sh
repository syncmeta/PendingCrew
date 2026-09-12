#!/bin/sh
# 读窗口一开就把账搬出那棵树（2026-09-13 立）。
#
# 为什么要有它：数据根「读不动」那个故障会自己来自己走（实测一次 44 分 19 秒），
# 窗口是随机给的。2026-09-12 第一个窗口里我读了父 crew 的账、**忘了读自己那本**，
# 窗口一关「我还剩几条」又变成答不上来的问题；第二个窗口里先搬账，一整天的
# 「读不出来」当场变成一个能回答的数。**诊断可以等，搬账不能等。**
#
# 它只做一件事：能读就整份复制出来，不能读就如实说读不动 —— 不猜成因
# （那是 scripts/diagnose-data-dir.sh 的活），也不改数据根里的任何东西。
#
#   用法: sh scripts/grab-ledgers.sh <crew-id> [落地目录]
#   默认落地目录: docs/internal/<今天>-ledger-snapshot
#
# 退出码: 0 = 全部搬出；1 = 读不动（一个都没搬出）；2 = 用法/环境不对。
set -u

CREW=${1:-}
[ -n "$CREW" ] || { echo "✋ 用法: sh scripts/grab-ledgers.sh <crew-id> [落地目录]"; exit 2; }
SRC="$HOME/Library/Application Support/PendingCrew/whiteboards"
[ -d "$SRC" ] || { echo "✋ 白板目录不在：$SRC"; exit 2; }
OUT=${2:-"docs/internal/$(date +%Y-%m-%d)-ledger-snapshot"}

# 搬这几本。**名字按仓库里的实际文件名**，别照记忆写：驾驶舱那本叫 .plan.json
# 不是 .plans.json，写错只会静默少搬一本。
SUFFIXES="todos.json human-todos.json plan.json todo-sweep.json approvals.json"

mkdir -p "$OUT" || exit 2
ok=0; fail=0
for suf in $SUFFIXES; do
    src="$SRC/$CREW.$suf"
    [ -e "$src" ] || { echo "—  没有这本：$suf"; continue; }
    # cp 失败的主因就是读不动；不吞它的 stderr，事故当场那句话就是证据。
    if cp "$src" "$OUT/$suf" 2>/dev/null; then
        echo "✅ $suf  ($(wc -c < "$OUT/$suf" | tr -d ' ') 字节)"
        ok=$((ok + 1))
    else
        echo "❌ $suf  读不动"
        fail=$((fail + 1))
    fi
done

echo "—— $(date '+%Y-%m-%d %H:%M:%S')  搬出 $ok 本，读不动 $fail 本 → $OUT"
[ "$ok" -gt 0 ] || { echo "窗口是关着的。别在这儿空转：哨兵盯着，开了再来。"; exit 1; }
exit 0
