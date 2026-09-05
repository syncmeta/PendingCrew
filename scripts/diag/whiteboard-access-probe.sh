#!/bin/sh
# 群聊「单向断开」现场探针 —— 只读诊断，不改任何产品行为。
#
# 病象：某个 session 突然读不了 ~/Library/Application Support/PendingCrew/ 下的
# 内容（`head -c 1` → Operation not permitted），而 ls / stat 照常，别的 session
# 同一时刻全好，过一会儿自己又好了。事后查了三次都查不出来 —— 所以要在断的当场抓。
#
#   用法:  sh whiteboard-access-probe.sh <tag> [间隔秒数，默认 5]
#   tag 建议写你的 crew 名，日志会落在 /tmp/pc-access-probe/<tag>.log
#   停:    kill $(cat /tmp/pc-access-probe/<tag>.pid)
#
# 每轮对同一批目标做 stat（元数据）+ head -c 1（内容）+ 在目录里建删一个临时文件，
# 并同时量两个**对照目标**（子树之外的 ~/.claude.json 和仓库文件）。只有状态发生
# 翻转、或出现失败时才写整段现场，平时一行心跳，日志不会爆。

TAG="${1:-anon}"
INTERVAL="${2:-5}"
SUP="$HOME/Library/Application Support/PendingCrew"
OUT="/tmp/pc-access-probe"
mkdir -p "$OUT" 2>/dev/null
LOG="$OUT/$TAG.log"
echo $$ > "$OUT/$TAG.pid"

ts() { date '+%Y-%m-%d %H:%M:%S%z'; }

# 逐级上溯自己的责任进程链（断的那一刻它是谁，是本案的核心争点之一）。
proc_chain() {
  p=$$
  i=0
  while [ $i -lt 8 ]; do
    line=$(ps -o pid=,ppid=,comm= -p "$p" 2>/dev/null)
    [ -z "$line" ] && break
    echo "    $line"
    p=$(echo "$line" | awk '{print $2}')
    [ -z "$p" ] || [ "$p" = "1" ] || [ "$p" = "0" ] && break
    i=$((i+1))
  done
}

# 一次读的判定：0=能读到内容，非0=读不到。把 stderr 原样留下（errno 文本是证据）。
try_read() {
  err=$(head -c 1 "$1" 2>&1 >/dev/null)
  rc=$?
  if [ $rc -eq 0 ] && [ -z "$err" ]; then echo "OK"; else echo "FAIL:${err:-rc=$rc}"; fi
}
try_stat() {
  err=$(stat -f '%z' "$1" 2>&1 >/dev/null)
  rc=$?
  if [ $rc -eq 0 ] && [ -z "$err" ]; then echo "OK"; else echo "FAIL:${err:-rc=$rc}"; fi
}
# 在子树里建一个文件、读回、删掉 —— 写路和读路要分开量，别只量读。
try_write() {
  f="$SUP/.probe-$TAG.tmp"
  err=$( { echo probe > "$f"; } 2>&1 )
  [ -n "$err" ] && { echo "FAIL(create):$err"; return; }
  err=$(head -c 1 "$f" 2>&1 >/dev/null)
  [ -n "$err" ] && { rm -f "$f" 2>/dev/null; echo "FAIL(readback):$err"; return; }
  rm -f "$f" 2>/dev/null
  echo "OK"
}

# 断的一刻多抓的现场。事后再跑就没了，所以全在这一函数里一次抓完。
forensics() {
  echo "  --- 现场 @ $(ts) ---"
  echo "  [自己的责任进程链]"
  proc_chain
  echo "  [这一刻机器上有几套 PendingCrew 二进制在跑]"
  ps -eo pid,lstart,args 2>/dev/null | grep -i 'PendingCrew' | grep -v grep | sed 's/^/    /'
  echo "  [目标文件的权限/ACL/xattr/flags]"
  ls -leO@ "$SUP/local-crews.json" 2>&1 | sed 's/^/    /'
  ls -ldeO@ "$SUP" 2>&1 | sed 's/^/    /'
  echo "  [近 2 分钟的 Sandbox / TCC 日志]"
  /usr/bin/log show --style syslog --last 2m \
    --predicate 'eventMessage CONTAINS "deny" OR subsystem == "com.apple.TCC"' 2>/dev/null \
    | grep -iE 'pendingcrew|Application Support|claude' | tail -40 | sed 's/^/    /'
  echo "  [挂载点 / sandbox-exec 痕迹]"
  ps -eo pid,args 2>/dev/null | grep -i 'sandbox-exec' | grep -v grep | sed 's/^/    /'
  echo "  --- 现场完 ---"
}

echo "=== 探针启动 tag=$TAG 间隔=${INTERVAL}s pid=$$ @ $(ts) ===" >> "$LOG"
echo "    子树: $SUP" >> "$LOG"
proc_chain >> "$LOG"

LAST=""
while :; do
  WB=$(ls -1 "$SUP/whiteboards"/*.json 2>/dev/null | head -1)
  S=$(try_stat "$SUP/local-crews.json")
  R=$(try_read "$SUP/local-crews.json")
  W=$(try_write)
  B=$([ -n "$WB" ] && try_read "$WB" || echo "NA")
  C1=$(try_read "$HOME/.claude.json")
  C2=$(try_read "$HOME/Untitled/Pendingname/PendingCrew/README.md")
  STATE="stat=$S read=$R write=$W board=$B ctrl_claude=$C1 ctrl_repo=$C2"

  case "$STATE" in
    *FAIL*) BAD=1 ;;
    *)      BAD=0 ;;
  esac

  if [ "$STATE" != "$LAST" ]; then
    echo "$(ts) 翻转 | $STATE" >> "$LOG"
    [ "$BAD" = "1" ] && forensics >> "$LOG" 2>&1
    # 恢复的那一刻也值得记 —— 「自愈」的宽度是本案唯一还没量到的东西
    LAST="$STATE"
  elif [ "$BAD" = "1" ]; then
    echo "$(ts) 持续断 | $STATE" >> "$LOG"
  fi
  sleep "$INTERVAL"
done
