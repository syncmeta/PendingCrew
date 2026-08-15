#!/usr/bin/env bash
# 生成 CrewChatOpenCostTests 用的**真实**群聊 fixture（#443）。
#
#   apps/pendingcrew/scripts/make-chat-fixtures.sh [crew-id]
#
# 为什么要现取而不是提交进 git：fixture 是人类真实的群聊内容 + 真实聊天截图
# （约 1MB），不该进版本历史。所以 Fixtures/ 目录进了 .gitignore，谁要跑
# CrewChatOpenCostTests 谁自己跑一遍这个脚本。
#
# 为什么必须是真数据：造出来的数据量不出真问题 —— 本 crew（PendingCrew）338 条、
# 82k 字、也带图，实测比「LED驱动板」那 70 条还快。卡不卡取决于单条有多贵，
# 不取决于条数，所以必须拿那一份特定的真实白板跑。
set -euo pipefail

CREW_ID="${1:?用法: make-chat-fixtures.sh <crew-id>（跑 ls "$HOME/Library/Application Support/PendingCrew/whiteboards" 看本机有哪些）}"
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
cp "$SRC_JSON" "$DEST/whiteboard.json"

rm -f "$DEST"/*.png
if [[ -d "$SRC_ATT" ]]; then
  # 只取图 —— 测量里那一段就是「5 张真图解码」。
  find "$SRC_ATT" -maxdepth 1 -iname '*.png' -exec cp {} "$DEST/" \;
fi

COUNT=$(/usr/bin/python3 -c 'import json,sys;print(len(json.load(open(sys.argv[1]))))' "$DEST/whiteboard.json")
IMGS=$(ls -1 "$DEST"/*.png 2>/dev/null | wc -l | tr -d ' ')

cat > "$DEST/README.txt" <<EOF
本目录由 apps/pendingcrew/scripts/make-chat-fixtures.sh 生成，**不入 git**
（内容是人类真实的群聊 + 真实截图）。

来源 crew: $CREW_ID
条数: $COUNT
图片: $IMGS 张

要重新生成：apps/pendingcrew/scripts/make-chat-fixtures.sh
EOF

echo "✓ fixture 就绪：$DEST"
echo "  whiteboard.json — $COUNT 条"
echo "  图片 — $IMGS 张"
