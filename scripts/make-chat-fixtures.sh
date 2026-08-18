#!/usr/bin/env bash
# 生成 CrewChatOpenCostTests 用的**真实**群聊 fixture（#443）。
#
#   scripts/make-chat-fixtures.sh <crew-id>              # 在仓库根目录跑
#   scripts/make-chat-fixtures.sh <crew-id> --limit 70   # 显式指定取前多少条
#   scripts/make-chat-fixtures.sh <crew-id> --full       # 取今天的全量（预算会失真，见下）
#
# 为什么要现取而不是提交进 git：fixture 是人类真实的群聊内容 + 真实聊天截图
# （约 1MB），不该进版本历史。所以 Fixtures/ 目录进了 .gitignore，谁要跑
# CrewChatOpenCostTests 谁自己跑一遍这个脚本。
#
# 为什么必须是真数据：造出来的数据量不出真问题 —— 本 crew（PendingCrew）338 条、
# 82k 字、也带图，实测比「LED驱动板」那 70 条还快。卡不卡取决于单条有多贵，
# 不取决于条数，所以必须拿那一份特定的真实白板跑。
#
# 为什么默认**只取前 70 条**（2026-08-18 加的闸）：白板只增不删，而
# CrewChatOpenCostTests 那一整套预算（一次重排 100ms、「重复 5 倍 ≈ 360 条」等）
# 是在这个 crew 还是 70 条 / 58,945 字节那天定标的。直接取今天的全量（342 条）会
# 出两种假红：
#   ① 前置用例的负载跟着涨 5 倍，同一个进程里做完大表布局后，**后面每一次布局都要
#      贵将近一倍** —— 实测同一个用例单独跑 73ms、跟在 1700 行那个用例后面跑 139ms，
#      预算 100ms 于是变成看运气；
#   ② 「重复 5 倍 ≈ 360 条」的注释也不再成立（变成 1700 条）。
# 白板是 append-only，所以「前 70 条」就是当初那份快照本身 —— 既是真数据，又让预算
# 恢复可比、跨机器跨时间可复现。要拿今天的全量看现状就加 --full，但别拿它的绝对
# 毫秒去对既有预算。
set -euo pipefail

CREW_ID="${1:?用法: make-chat-fixtures.sh <crew-id> [--limit N | --full]（跑 ls "$HOME/Library/Application Support/PendingCrew/whiteboards" 看本机有哪些）}"
shift

# 基线口径：定标那天的条数。改它 = 改整套预算的参照系，别顺手改。
LIMIT=70
while [[ $# -gt 0 ]]; do
  case "$1" in
    --full) LIMIT=0; shift ;;
    --limit) LIMIT="${2:?--limit 后面要跟条数}"; shift 2 ;;
    *) echo "✗ 不认识的参数：$1" >&2; exit 2 ;;
  esac
done

SUPPORT="$HOME/Library/Application Support/PendingCrew"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEST="$HERE/../Tests/PendingCrewTests/Fixtures/LEDDriverCrew"

SRC_JSON="$SUPPORT/whiteboards/$CREW_ID.json"
SRC_ATT="$SUPPORT/attachments/$CREW_ID"

if [[ ! -f "$SRC_JSON" ]]; then
  echo "✗ 找不到白板：$SRC_JSON" >&2
  echo "  这台机器上没有这个 crew。换一个 crew id 重跑：" >&2
  echo "    ls \"$SUPPORT/whiteboards\"" >&2
  exit 1
fi

mkdir -p "$DEST"
# 取前 LIMIT 条（0 = 全量）。白板 append-only，所以前 N 条 = 当初那份快照。
/usr/bin/python3 - "$SRC_JSON" "$DEST/whiteboard.json" "$LIMIT" <<'PY'
import json, sys
src, dst, limit = sys.argv[1], sys.argv[2], int(sys.argv[3])
rows = json.load(open(src))
if limit > 0:
    rows = rows[:limit]
# 紧凑分隔符 —— 与 app 侧 JSONEncoder 的落盘形态一致，字节数才和线上可比。
json.dump(rows, open(dst, "w"), ensure_ascii=False, separators=(",", ":"))
PY

rm -f "$DEST"/*.png
if [[ -d "$SRC_ATT" ]]; then
  # 只取图 —— 测量里那一段就是「5 张真图解码」。
  find "$SRC_ATT" -maxdepth 1 -iname '*.png' -exec cp {} "$DEST/" \;
fi

COUNT=$(/usr/bin/python3 -c 'import json,sys;print(len(json.load(open(sys.argv[1]))))' "$DEST/whiteboard.json")
BYTES=$(/usr/bin/stat -f%z "$DEST/whiteboard.json")
IMGS=$(ls -1 "$DEST"/*.png 2>/dev/null | wc -l | tr -d ' ')
SLICE=$([[ "$LIMIT" -gt 0 ]] && echo "前 $LIMIT 条（基线口径）" || echo "全量（--full）")

cat > "$DEST/README.txt" <<EOF
本目录由 scripts/make-chat-fixtures.sh 生成，**不入 git**
（内容是人类真实的群聊 + 真实截图）。

来源 crew: $CREW_ID
口径: $SLICE
条数: $COUNT（$BYTES 字节）
图片: $IMGS 张

要重新生成（仓库根目录）：scripts/make-chat-fixtures.sh $CREW_ID
EOF

echo "✓ fixture 就绪：$DEST"
echo "  whiteboard.json — $COUNT 条 / $BYTES 字节（$SLICE）"
echo "  图片 — $IMGS 张"
