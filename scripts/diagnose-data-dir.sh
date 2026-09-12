#!/bin/sh
# 数据根「读不动」那个故障的定性探针（2026-09-12 立）。
#
# 它**不猜成因**，只把可判别的那几样一次量完：出事时人要的是「这是哪一族」，
# 而不是又一轮从头试。2026-09-12 那次花了一上午才做到下面第 ②③ 步 ——
# 那两步各自只要两分钟，只是没人想到要做。
#
#   用法: sh scripts/diagnose-data-dir.sh [数据根]
#   默认数据根: ~/Library/Application Support/PendingCrew
#
# 退出码: 0 = 读得动；1 = 读不动（那一族）；2 = 用法/环境不对。
# **不需要 sudo，不碰 app 的任何既有文件**（只在目录里建自己的临时文件并删掉）。
set -u
ROOT=${1:-"$HOME/Library/Application Support/PendingCrew"}
[ -d "$ROOT" ] || { echo "✋ 不是目录：$ROOT"; exit 2; }
TAG="datadir-probe-$$"

# 一次完整探针：在 $1 里建、写、再打开读，然后清理。回显 ok / fail。
probe() {
  d=$1; f="$d/$TAG.tmp"
  printf 'probe\n' > "$f" 2>/dev/null || { echo "连新建都不行"; return 1; }
  if cat "$f" >/dev/null 2>&1; then r=ok; else r=fail; fi
  rm -f "$f" 2>/dev/null
  echo "$r"
}

echo "数据根：$ROOT"
echo
echo "① 这棵树本身"
me=$(probe "$ROOT")
case "$me" in
  ok)   echo "   ✅ 新建之后读得回来 —— 不是这一族的故障" ;;
  fail) echo "   ❌ **自己刚建的文件也打不开** —— 就是这一族" ;;
  *)    echo "   ⚠️ $me" ;;
esac

echo
echo "② 是整棵子树，还是只有某些文件？（自己新建一个子目录再试）"
sub="$ROOT/$TAG.dir"
if mkdir -p "$sub" 2>/dev/null; then
  s=$(probe "$sub"); rmdir "$sub" 2>/dev/null
  case "$s" in
    ok)   echo "   子目录里 ✅ —— 那就**不是**整棵子树，去查那些具体文件" ;;
    fail) echo "   子目录里 ❌ —— **整棵子树**，凡是「这些文件被做了什么」的假说全出局" ;;
    *)    echo "   ⚠️ $s" ;;
  esac
else
  echo "   连子目录都建不了 —— 另一种故障，别往下读了"
fi

echo
echo "③ 横向对照：同一条探针，别人的 Application Support"
for d in Codex Claude "Google/Chrome"; do
  p="$HOME/Library/Application Support/$d"
  [ -d "$p" ] || { printf "   %-16s （本机没有）\n" "$d"; continue; }
  printf "   %-16s %s\n" "$d" "$(probe "$p")"
done
echo "   （这些都 ok 而①是 fail ⇒ 拦的是这棵子树，不是整个 Application Support）"

echo
echo "④ 写这一侧还剩什么（解释「为什么文件还在被更新」）"
echo "   整份原子写 = 临时文件 + rename，而 rename / unlink / open(O_CREAT) 都在放行那侧。"
echo "   所以 **mtime 一直在动不代表它读得到东西** —— app 可能在靠启动时的内存快照跑。"

echo
echo "⑤ daemon 日志（不在数据根里，通常读得了）"
L="$HOME/Library/Logs/PendingCrew/daemon.log"
if [ -r "$L" ]; then
  echo "   最后 3 行（日志本来就稀疏，**空白不等于死了**）："
  tail -3 "$L" | sed 's/^/     /'
else
  echo "   读不到 $L"
fi

echo
echo "⑥ 排空不掉的机长命令（只 ls，不读内容；**还在 = 一直没读成**）"
echo "   —— 看的是**躺了多久**，不是有几条：排空要先 open 它，所以一条躺很久"
echo "      说明消费那一侧此刻也 open 不了。⚠️ 但**没有非故障时刻的对照**时，"
echo "      「躺了 N 秒」说不出是症状还是常态（正常要多久，本仓还没量过）。"
n=$(ls "$ROOT"/whiteboards/*.crewcmd.json 2>/dev/null | wc -l | tr -d ' ')
echo "   $n 条"
# 逐条打印「落盘时刻 + 文件名」。**不要拿 awk 切 ls 的列** —— 路径里有空格
# （`Application Support`），切出来的「文件名」会在空格处断掉，看着像另一个文件。
for c in "$ROOT"/whiteboards/*.crewcmd.json; do
  [ -e "$c" ] || break
  age=$(( $(date +%s) - $(stat -f%m "$c") ))
  printf '     %s  躺了 %ss  %s\n' \
    "$(stat -f%Sm -t '%m-%d %H:%M:%S' "$c")" "$age" "$(basename "$c")"
done

echo
[ "$me" = ok ] && { echo "结论：读得动。"; exit 0; }
cat <<'TXT'
结论：**读不动，属于「建得了、看得见、删得掉，只要它已经存在就打不开」那一族。**

下一步（都要人在场，agent 拿不到）：
  · 系统设置 → 隐私与安全性 → App 管理 / 完全磁盘访问，把终端加进去
  · 要钉死成因：sudo log stream --predicate 'subsystem == "com.apple.TCC"'
现场与已排除项：docs/internal/2026-09-12-eperm-cause-found.md
TXT
exit 1
