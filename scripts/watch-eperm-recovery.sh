#!/bin/sh
# 数据目录 EPERM 发作期间的守候：等到「又读得动」的那一刻，当场留现场。
#
# 用法（发作当口跑，不要 sudo）：
#     scripts/watch-eperm-recovery.sh            # 默认最长守 24 小时
#     scripts/watch-eperm-recovery.sh 3600       # 最长守 N 秒
#
# ## 为什么有这个脚本
#   第 5 次发作（2026-09-13 00:24 → 09:28:22）恢复当口的对照没拿到：守候挂在
#   agent 会话里，会话没了守候跟着没了。所以它必须**脱离会话**跑 ——
#   下面用 setsid 另开一个会话、nohup 忽略挂断，父进程退出后由 launchd 收养。
#   ⚠️ 「session 被停时它还活着」是按进程组机制推的，第一次真跑前没验过。
#
# ## 它留什么（全部写在数据根之外：~/Library/Logs/PendingCrew-eperm/）
#   · 起守时：进程表、数据根里读得动/读不动的文件个数
#   · 每 10 秒探一次，状态变了记一行时刻
#   · 恢复那一刻：再抓一次进程表（跟起守时 diff 看「谁变了」）、个数、
#     再用 clonefile 把数据根克隆一份出来（scripts/clone-data-root.sh 同一个办法）
#
# ## 它**不能**回答什么
#   · 谁拒的、为什么恢复。进程表 diff 只是线索，恢复瞬间没变的东西它看不到。
#   · 探的是「从这个进程树里读」。它是谁拉起的就代表谁的视角（agent 拉起 = agent 视角）。
set -eu

MAX_SECS="${1:-86400}"
ROOT="$HOME/Library/Application Support/PendingCrew"
OUTDIR="$HOME/Library/Logs/PendingCrew-eperm"
TS="$(date +%Y%m%d-%H%M%S)"
LOG="$OUTDIR/watch-$TS.log"
mkdir -p "$OUTDIR"

# 第一次调用：把自己脱离当前会话再跑一遍，然后立即返回。
if [ "${EPERM_WATCH_DETACHED:-}" != 1 ]; then
  SELF="$OUTDIR/watch-eperm-recovery-$TS.sh"
  cp "$0" "$SELF"   # 跑副本：仓库里那份以后被改，不会让正在跑的这份读到半行
  EPERM_WATCH_DETACHED=1 nohup /usr/bin/perl -e 'use POSIX qw(setsid); setsid(); exec @ARGV' \
    /bin/sh "$SELF" "$MAX_SECS" > "$LOG" 2>&1 < /dev/null &
  echo "守候已脱离会话起跑，pid $!，日志 → $LOG"
  exit 0
fi

count() {
  ok=0; bad=0
  for f in "$ROOT"/*.json "$ROOT"/whiteboards/*.json; do
    [ -f "$f" ] || continue
    if head -c 1 "$f" >/dev/null 2>&1; then ok=$((ok + 1)); else bad=$((bad + 1)); fi
  done
  echo "$ok $bad"
}

# 探针：顶层这本账是「老文件」，最能代表发作状态（新出生的文件不受影响）。
PROBE="$ROOT/local-crews.json"
readable() { head -c 1 "$PROBE" >/dev/null 2>&1; }

echo "起守 $(date '+%F %T') pid $$ ppid $PPID  最长 ${MAX_SECS}s"
set -- $(count); echo "起守时 读得动 $1 / 读不动 $2"
ps -axo pid=,ppid=,lstart=,command= > "$OUTDIR/ps-start-$TS.txt"

if readable; then
  echo "起守时探针已经读得动 —— 此刻没在发作，守候不必挂。退出。"
  exit 0
fi

state=bad
start=$(date +%s)
while :; do
  now=$(date +%s)
  if [ $((now - start)) -ge "$MAX_SECS" ]; then
    echo "$(date '+%F %T') 守满 ${MAX_SECS}s 仍未恢复，退出。"
    exit 0
  fi
  if readable; then
    echo "$(date '+%F %T') 恢复：探针读得动了"
    ps -axo pid=,ppid=,lstart=,command= > "$OUTDIR/ps-recover-$TS.txt"
    set -- $(count); echo "恢复时 读得动 $1 / 读不动 $2"
    echo "=== 进程表 diff（< 起守时有、恢复时没有；> 恢复时新出现）==="
    diff "$OUTDIR/ps-start-$TS.txt" "$OUTDIR/ps-recover-$TS.txt" | grep -E '^[<>]' | head -80 || true
    CLONE="$OUTDIR/data-root-at-recovery-$TS"
    if /bin/cp -c -R "$ROOT" "$CLONE" 2>/dev/null; then
      echo "数据根已 clonefile 克隆到 $CLONE"
    else
      echo "克隆失败（cp -c 不可用或跨卷），跳过 —— 进程表和时刻已经留下"
    fi
    exit 0
  fi
  sleep 10
done
