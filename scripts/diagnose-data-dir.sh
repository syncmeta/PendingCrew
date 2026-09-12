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
echo "③b 到底是哪个系统调用被拒（三个目录同一秒各建一个文件，逐调用量）"
echo "   ⚠️ **必须直接调那个系统调用。** 用 \`xattr\` 这类命令行量是错的 —— 它是个"
echo "      Python 包装、自己会先 open 文件，于是 open 的 EPERM 被报成「xattr 被拒」。"
echo "      2026-09-12 我就是这么把一整族假说错误排除掉的，详见那份现场档。"
/usr/bin/python3 - "$ROOT" <<'PY' 2>/dev/null || echo "   （python3 不可用，跳过这一节）"
import ctypes, ctypes.util, os, sys
libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)

def getx(path, name):
    buf = ctypes.create_string_buffer(256)
    n = libc.getxattr(path.encode(), name.encode(), buf, 256, ctypes.c_uint32(0), 0)
    return ("FAIL errno=%d" % ctypes.get_errno()) if n < 0 else buf.raw[:n].hex()

def listx(path):
    buf = ctypes.create_string_buffer(1024)
    n = libc.listxattr(path.encode(), buf, 1024, 0)
    return ("FAIL errno=%d" % ctypes.get_errno()) if n < 0 else "ok"

def opn(path):
    try:
        open(path, "rb").read(2); return "ok"
    except OSError as e:
        return "FAIL errno=%d" % e.errno

home = os.path.expanduser("~")
targets = [("本树（疑）", sys.argv[1]),
           ("Codex（对照）", home + "/Library/Application Support/Codex"),
           ("/tmp（对照）", "/tmp")]
print("   %-16s %-14s %-14s %s" % ("目录", "open", "listxattr", "provenance 值"))
for label, d in targets:
    if not os.path.isdir(d):
        print("   %-16s （本机没有）" % label); continue
    f = os.path.join(d, "diag-syscall-%d.txt" % os.getpid())
    try:
        open(f, "w").write("hi")
    except OSError as e:
        print("   %-16s 连建都建不了：errno=%d" % (label, e.errno)); continue
    print("   %-16s %-14s %-14s %s"
          % (label, opn(f), listx(f), getx(f, "com.apple.provenance")))
    os.unlink(f)
print("   读法：只有 open 那一列 FAIL、后两列 ok，且三行 provenance 值相同")
print("   ⇒ 被掐的只有「拿到文件内容」，元数据路径全通 ——「属性被做了手脚」那类假说全出局。")
PY

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

下一步（要人在场，agent 拿不到 sudo）：

  ⚠️ **别再抓 TCC 日志了。** 这里以前写的是
     `sudo log stream --predicate 'subsystem == "com.apple.TCC"'`，
     而 2026-09-12 实测：20 分钟 616 行里**文件类服务 0 次**（全是 AppleEvents）——
     TCC 连评估都没评估这件事。照那条走只会得到一屏无关日志，然后以为「查过了」。

  · 发作当口跑：sudo scripts/capture-eperm-fsusage.sh
    它能告诉你：这次 open 的 errno、是谁在那一刻碰这个文件、有没有别的进程插在中间。
    **它不能告诉你「谁拒的」** —— fs_usage 停在系统调用边界，看不到内核里哪个
    授权钩子返的错。这条边界写在这里，免得跑完一趟以为定案了。
  · 真想知道「谁拒的」，目前没有不要 sudo 且不被 SIP 挡住的现成办法；
    已经排完的层（TCC / 内核沙盒 / 文件提供方 / 磁盘 / 第三方扩展）见现场档，别重跑。

现场与已排除项：docs/internal/2026-09-12-eperm-cause-found.md
TXT
exit 1
