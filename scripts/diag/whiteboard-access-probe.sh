#!/bin/sh
# 群聊「单向断开」现场探针 —— 诊断用，不改任何产品行为。
#
# ⚠️ 它**不是纯只读**：写路也要量，所以每轮会在 $SUP/.probe/ 下建一个临时文件、
# 读回、删掉。放在自建的 .probe/ 子目录而不是数据根，是为了不给根目录上的目录
# 监听加 tick —— 点名唤醒器每个目录 tick 会全量重解白板（docs/tech-debt.md），
# 探针每 5 秒戳一次真数据根，等于在给正被诊断的系统持续加噪。
#
# 病象：某个 session 突然读不了 ~/Library/Application Support/PendingCrew/ 下的
# 内容（`head -c 1` → Operation not permitted），而 ls / stat 照常，别的 session
# 同一时刻全好，过一会儿自己又好了。事后查了三次都查不出来 —— 所以要在断的当场抓。
#
#   用法:  sh whiteboard-access-probe.sh <tag> [间隔秒数，默认 5]
#   停:    kill $(cat /tmp/pc-access-probe/<tag>.pid)
#
# ── 这一版新增的两条判决性读数（父机长 4-1 提的，成本极低、能一次定层）──────
# 1) **拿原始 errno，不要只拿字符串**。`Operation not permitted` = EPERM(1)，
#    `Permission denied` = EACCES(13)。POSIX 权限/ACL 给的是后者，受害者报的是
#    前者 —— 所以只剩「内核 MAC 层」和「有人在用内核的措辞」两种可能。
# 2) **同一轮里换三种读法**（head / cat / python3 open()）。三者结果不一致，
#    答案就在不一致的地方；三者一致地失败，才轮得到内核。
#    ⚠️ 第四种读法 —— **agent 自己的文件读工具** —— 脚本里跑不了，只有活人/活
#    agent 在断的当场能补。日志里会印一行提示，看到 FAIL 的人请当场补这一刀。

TAG="${1:-anon}"
INTERVAL="${2:-5}"
SUP="$HOME/Library/Application Support/PendingCrew"
OUT="/tmp/pc-access-probe"
mkdir -p "$OUT" 2>/dev/null
LOG="$OUT/$TAG.log"
PY="$OUT/readcheck-$TAG.py"
echo $$ > "$OUT/$TAG.pid"

# 原始 errno 读法。跟 head/cat 走的是同一个 open(2)，但报的是数字而不是本地化字符串。
cat > "$PY" <<'PYEOF'
import sys, os, glob
sup = sys.argv[1]
targets = [("crews", os.path.join(sup, "local-crews.json"))]
b = sorted(glob.glob(os.path.join(sup, "whiteboards", "*.json")))
if b:
    targets.append(("board", b[0]))
targets.append(("ctrl_claude", os.path.expanduser("~/.claude.json")))
for name, p in targets:
    try:
        with open(p, "rb") as f:
            f.read(1)
        print("%s=OK" % name)
    except OSError as e:
        print("%s=errno%d(%s)" % (name, e.errno, e.strerror))
    except Exception as e:
        print("%s=%s(%s)" % (name, type(e).__name__, e))
PYEOF

# 对照组目标：别的 app 的 Application Support 子目录里各取一个**真文件**。
# 用 find -type f，不用 `ls | head -1` —— 后者多半取到子目录。
A_SUP="$HOME/Library/Application Support"
OTHER_A=$(find "$A_SUP/Claude" -maxdepth 2 -type f -size +0 2>/dev/null | head -1)
OTHER_B=$(find "$A_SUP/Code"   -maxdepth 2 -type f -size +0 2>/dev/null | head -1)
[ -z "$OTHER_A" ] && OTHER_A="$A_SUP"
[ -z "$OTHER_B" ] && OTHER_B="$A_SUP"

ts() { date '+%Y-%m-%d %H:%M:%S%z'; }

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

# 按可执行文件路径（comm）数，**不要**按整条命令行 grep —— 那会把探针自己、
# 以及任何命令行里带着这个字符串的包装进程数进去（31-1 在这个坑里栽过一次，
# 同一时刻 `ps | grep -c` 给 2、逐行列出来是 0）。
binary_census() {
  ps -eo comm= 2>/dev/null | grep -c '^/Applications/PendingCrew.app/' | sed 's/^/    Applications: /'
  ps -eo comm= 2>/dev/null | grep -c '^/tmp/pendingcrew-pkg' | sed 's/^/    tmp-pkg:      /'
}

try_read() {
  err=$(head -c 1 "$1" 2>&1 >/dev/null)
  rc=$?
  if [ $rc -eq 0 ] && [ -z "$err" ]; then echo "OK"; else echo "FAIL:${err:-rc=$rc}"; fi
}
try_cat() {
  err=$( { cat "$1" >/dev/null; } 2>&1 )
  rc=$?
  if [ $rc -eq 0 ] && [ -z "$err" ]; then echo "OK"; else echo "FAIL:${err:-rc=$rc}"; fi
}
try_stat() {
  err=$(stat -f '%z' "$1" 2>&1 >/dev/null)
  rc=$?
  if [ $rc -eq 0 ] && [ -z "$err" ]; then echo "OK"; else echo "FAIL:${err:-rc=$rc}"; fi
}
try_write() {
  d="$SUP/.probe"
  mkdir -p "$d" 2>/dev/null
  f="$d/$TAG.tmp"
  err=$( { echo probe > "$f"; } 2>&1 )
  [ -n "$err" ] && { echo "FAIL(create):$err"; return; }
  err=$(head -c 1 "$f" 2>&1 >/dev/null)
  [ -n "$err" ] && { rm -f "$f" 2>/dev/null; echo "FAIL(readback):$err"; return; }
  rm -f "$f" 2>/dev/null
  echo "OK"
}

forensics() {
  echo "  --- 现场 @ $(ts) ---"
  echo "  ⚠️ 看到这段的人：请**当场**用你自己的 agent 文件读工具读一次"
  echo "     $SUP/local-crews.json —— 脚本补不了这一刀，而它是分层的判决证据。"
  echo "  [自己的责任进程链]"
  proc_chain
  echo "  [这一刻两份二进制各有几个进程（按 comm 数，不按命令行 grep）]"
  binary_census
  echo "  [ps 逐行]"
  ps -eo pid,lstart,comm= 2>/dev/null | grep -E 'PendingCrew.app/Contents/MacOS/PendingCrew' | sed 's/^/    /'
  echo "  [目标文件的权限/ACL/xattr/flags]"
  ls -leO@ "$SUP/local-crews.json" 2>&1 | sed 's/^/    /'
  ls -ldeO@ "$SUP" 2>&1 | sed 's/^/    /'
  echo "  [近 10 分钟 tccd 的 code-requirement 失配 —— 这一类拒绝不长 deny 那样]"
  /usr/bin/log show --style syslog --last 10m \
    --predicate 'subsystem == "com.apple.TCC"' 2>/dev/null \
    | grep -i 'Failed to match existing code requirement' | tail -20 | sed 's/^/    /'
  echo "  [近 2 分钟的 Sandbox / TCC 日志]"
  /usr/bin/log show --style syslog --last 2m \
    --predicate 'eventMessage CONTAINS "deny" OR subsystem == "com.apple.TCC"' 2>/dev/null \
    | grep -iE 'pendingcrew|Application Support|claude' | tail -40 | sed 's/^/    /'
  echo "  --- 现场完 ---"
}

echo "=== 探针启动 tag=$TAG 间隔=${INTERVAL}s pid=$$ @ $(ts) ===" >> "$LOG"
echo "    子树: $SUP" >> "$LOG"
proc_chain >> "$LOG"
binary_census >> "$LOG"

LAST=""
while :; do
  WB=$(ls -1 "$SUP/whiteboards"/*.json 2>/dev/null | head -1)
  S=$(try_stat "$SUP/local-crews.json")
  R=$(try_read "$SUP/local-crews.json")
  K=$(try_cat "$SUP/local-crews.json")
  W=$(try_write)
  B=$([ -n "$WB" ] && try_read "$WB" || echo "NA")
  C1=$(try_read "$HOME/.claude.json")
  C2=$(try_read "$HOME/Untitled/Pendingname/PendingCrew/README.md")
  # ⚠️ 对照组**必须**包含别的 app 的 Application Support 子目录。只拿 ~/.claude.json
  # 和仓库文件当对照，会稳定得出「只拦我们这一棵子树」—— 那两个都在 Application
  # Support 之外，范围被划在了刚看过的东西上。这里各取一个**真文件**（不是目录：
  # head 读目录必然失败，跟权限无关，退出码分不出这两件事）。
  C3=$(try_read "$OTHER_A"); C4=$(try_read "$OTHER_B")
  # 红样本：一个**确知不可读**的普通文件。它必须一直是 FAIL —— 它一旦变 OK，
  # 说明这把尺子坏了，不是世界变了。（上一版的洞：唯一会红的那项被 find 跳过，
  # 从未进入样本集，于是「全绿」看起来像「已核」。）
  RED=$(try_read "$HOME/Library/Application Support/com.apple.TCC/TCC.db")
  P=$(python3 "$PY" "$SUP" 2>&1 | tr '\n' ' ')
  case "$RED" in OK) RED="⚠尺子坏了(红样本变绿)";; *) RED="红样本正常";; esac
  STATE="stat=$S head=$R cat=$K write=$W board=$B ctrl_claude=$C1 ctrl_repo=$C2 otherapp1=$C3 otherapp2=$C4 [$RED] | py[ $P]"

  # 判 BAD 时把红样本那一格摘掉 —— 它本来就该是红的。
  JUDGE=$(echo "$STATE" | sed 's/ \[[^]]*\]//')
  case "$JUDGE" in
    *FAIL*|*errno*) BAD=1 ;;
    *)              BAD=0 ;;
  esac

  if [ "$STATE" != "$LAST" ]; then
    echo "$(ts) 翻转 | $STATE" >> "$LOG"
    [ "$BAD" = "1" ] && forensics >> "$LOG" 2>&1
    LAST="$STATE"
  elif [ "$BAD" = "1" ]; then
    echo "$(ts) 持续断 | $STATE" >> "$LOG"
  fi
  sleep "$INTERVAL"
done
