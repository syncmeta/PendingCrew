#!/bin/sh
# 数据目录 EPERM 发作当口的取证：抓一次 fs_usage，并在抓的同时故意触发一次失败的读。
#
# 用法（**必须 sudo**，必须在故障正在发生时跑）：
#     sudo scripts/capture-eperm-fsusage.sh
#
# ## 它能回答什么
#   · 这次 open 的 errno 到底是什么（EPERM 还是别的）
#   · 那一刻还有谁在碰这个文件（有没有别的进程插在中间、独占打开之类）
#   · 失败的那一笔在系统调用序列里长什么样
#
# ## 它**不能**回答什么（写在这儿，免得跑完一趟以为定案了）
#   · **谁拒的。** fs_usage 停在系统调用边界，看不到内核里是哪个授权钩子返的错。
#     形状上最像 Endpoint Security 的 AUTH_OPEN（只否决 open、其余全放行），但本机
#     只有苹果自己的两个 ES 客户端，日志里没有任何一行把它们跟这个目录连起来。
#
# ## 为什么不是抓 TCC 日志
#   2026-09-12 实测：20 分钟 616 行里文件类服务 0 次，TCC 连评估都没评估这件事。
set -eu

ROOT="$HOME/Library/Application Support/PendingCrew"
[ "$(id -u)" = 0 ] || { echo "要 sudo 跑：sudo $0" >&2; exit 2; }

# sudo 下 $HOME 可能变成 /var/root —— 用真实登录用户的家目录。
REAL_USER="${SUDO_USER:-$(stat -f '%Su' /dev/console)}"
ROOT="$(eval echo "~$REAL_USER")/Library/Application Support/PendingCrew"
[ -d "$ROOT" ] || { echo "找不到数据目录：$ROOT" >&2; exit 2; }

OUT="/tmp/eperm-fsusage-$(date +%Y%m%d-%H%M%S).log"
SECS="${1:-8}"

echo "数据目录：$ROOT"
echo "抓 ${SECS}s，原始输出 → $OUT"
echo

# 先起 fs_usage，再触发读 —— 反过来会漏掉那一笔。
/usr/bin/fs_usage -w -f filesys > "$OUT" 2>/dev/null &
FS_PID=$!
sleep 1

echo "触发一次读（预期失败）："
TARGET="$(/usr/bin/find "$ROOT" -maxdepth 2 -name '*.json' -type f 2>/dev/null | head -1)"
if [ -n "$TARGET" ]; then
  su "$REAL_USER" -c "cat '$TARGET' >/dev/null" 2>&1 | sed 's/^/   /' || true
  echo "   目标：$TARGET"
else
  echo "   （这棵树里一个 .json 都没找到，只抓背景流量）"
fi

sleep "$SECS"
kill "$FS_PID" 2>/dev/null || true
wait "$FS_PID" 2>/dev/null || true

echo
echo "=== 跟这棵树有关、且带错误码的行 ==="
grep -i "PendingCrew" "$OUT" | grep -iE "\[[ ]*[0-9]+\]|error" | head -40 || true
echo
echo "=== 这棵树上全部动静（前 60 行，看还有谁在碰）==="
grep -i "PendingCrew" "$OUT" | head -60 || true
echo
echo "原始全量留在：$OUT"
echo "⚠️ 读它的时候记住：fs_usage 只到系统调用边界，**看不到是谁拒的**。"
