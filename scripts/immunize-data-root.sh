#!/bin/sh
# 用法: sh scripts/immunize-data-root.sh [--apply] [数据根]
#
# 把数据根里**已经存在的**文件逐个「重新出生」一次：读出字节 → 在数据根之外
# 写成新文件 → rename 回原位 → 回读比对。内容一个字节不变。
#
# ## 为什么要做这件事
#
# 那个周期性 EPERM 故障拦的是「在 Application Support 底下出生的文件」——
# 标记在创建时打上、之后跟着文件走（现场：docs/internal/2026-09-12-eperm-marker-travels.md）。
# 0.1.36 起所有新写的文件都改在数据根之外出生再挪进来，所以**以后写的**都免疫；
# 但**已经存在的那些仍然带着标记**。常写的账（白板 / Todo / 驾驶舱）一次正常写入
# 就换新了，而 `local-crews.json`、机长模板、后端表这种几天才写一次的，
# 装了新版之后**下一窗照样读不出来**——而人会以为已经修好了。
#
# 这个脚本把那一步一次做完。
#
# ## 为什么要人跑，而且要 app 退出
#
# 它是对**全部账本**的批量重写。两条理由让它不该在后台自动跑：
#   ① `.lock` 是 flock 的 sidecar。重写它 = 换一个新 inode，而正持有锁的进程
#      锁的还是旧 inode —— 两个进程会同时以为自己拿到了锁。所以必须 app 不在跑。
#   ② 批量重写第一次真跑不该发生在没人看着的时候。
#
# 默认**只看不动**（dry run）。真要改传 `--apply`。
set -eu

apply=0
root=""
for a in "$@"; do
  case "$a" in
    --apply) apply=1 ;;
    -*) echo "未知参数 $a" >&2; exit 2 ;;
    *) root=$a ;;
  esac
done
[ -n "$root" ] || root="$HOME/Library/Application Support/PendingCrew"
[ -d "$root" ] || { echo "数据根不存在：$root" >&2; exit 2; }

# ① app 不许在跑（理由见上面 ①）。认的是界面/daemon 那个进程，不是 MCP helper。
#
# **只对真数据根生效**：显式传别的路径 = 一份副本 / 另一台机器的目录 / 测试用的树，
# 那上面没有进程持着锁，拦它没有意义、还会让这个脚本自己没法被验证
# （拦住之后跑出来的「内容一字未变」是一次空操作的假绿——我第一版就这么量过一次）。
# 真数据根即使显式传进来也照拦：下面比的是解析后的路径。
default_root="$HOME/Library/Application Support/PendingCrew"
if [ "$(cd "$root" && pwd)" = "$(cd "$default_root" 2>/dev/null && pwd || echo /nonexistent)" ] \
   && pgrep -f '^/Applications/PendingCrew\.app/Contents/MacOS/PendingCrew' >/dev/null 2>&1; then
  echo "PendingCrew 还在跑 —— 先 ⌘Q 退出（daemon 也会跟着停）再来。" >&2
  echo "理由：重写 .lock 会换掉 inode，正持锁的进程锁的还是旧的，两边会同时以为自己拿到锁。" >&2
  exit 2
fi

# ② 故障发作时做不了：读都读不出来，谈不上重写。
probe=$(find "$root" -maxdepth 2 -type f -name '*.json' 2>/dev/null | head -1 || true)
if [ -n "$probe" ] && ! head -c 1 "$probe" >/dev/null 2>&1; then
  echo "数据根此刻读不出来（故障正在发作）—— 等它恢复再跑。" >&2
  echo "  探针：$probe" >&2
  exit 2
fi

stage="$HOME/Library/Caches/PendingCrew/immunize.$$"
mkdir -p "$stage"

# ③ 落脚点必须与数据根**同卷**。
#
# 不同卷时 `mv` 退化成「复制 + 删原件」，而复制出来的那个文件是**在目标目录里
# 出生的** —— 于是它照样带上标记，这个脚本等于一件事没做，却一条 ⚠️ 都不会打。
# 这是本脚本唯一一种会静默失效的方式，所以当场判掉，不留给以后去猜。
#
# ⚠️ **想验这道闸会不会响，别拿 `/System/...` 试** —— APFS 的 firmlink 让系统卷
# 和数据卷在 `stat -f %d` 下是**同一个 dev**（本机实测 `/`、`/System/Library`、
# `/tmp`、`~/Library` 全是 16777230），试出来会以为这道闸是摆设。
# 拿一个真正独立的卷试：`/System/Volumes/Preboot`（16777229）当场就红了。
dev_stage=$(stat -f %d "$stage")
dev_root=$(stat -f %d "$root")
if [ "$dev_stage" != "$dev_root" ]; then
  echo "落脚点与数据根不在同一个卷（$dev_stage vs $dev_root）—— 拒绝执行。" >&2
  echo "跨卷时 mv 会变成复制，新文件仍然在目标目录里出生，做了等于没做而且不会报错。" >&2
  echo "把落脚点换到跟数据根同卷的位置（改本脚本里的 stage）再来。" >&2
  exit 2
fi
trap 'rm -rf "$stage"' EXIT INT TERM

# 用 find -print0 + read 逐个走，文件名里有空格也不会散架。
# **不在这儿计数**：这个 while 跑在管道的子 shell 里，加出来的数传不回来 ——
# 与其留一组看着像在数、其实恒为 0 的变量，不如逐条打印，让眼睛去数。
find "$root" -type f -print0 | while IFS= read -r -d '' f; do
  if [ $apply -eq 0 ]; then
    echo "会重写: ${f#$root/}"
    continue
  fi
  tmp="$stage/$(/usr/bin/uuidgen)"
  if ! cat "$f" > "$tmp" 2>/dev/null; then
    echo "  ⚠️ 读不出来，原样留着: ${f#$root/}" >&2
    rm -f "$tmp"; continue
  fi
  # 回读比对**在挪之前做**：比不上就根本不动原件。
  if ! cmp -s "$f" "$tmp"; then
    echo "  ⚠️ 副本与原件不一致，原样留着: ${f#$root/}" >&2
    rm -f "$tmp"; continue
  fi
  # 权限位跟着原件走（rename 之后拿的是新文件的，得先设好）。
  # macOS 的 chmod 没有 --reference，用 stat 取八进制权限位。
  mode=$(stat -f %Lp "$f" 2>/dev/null || echo 644)
  chmod "$mode" "$tmp"
  if mv -f "$tmp" "$f" 2>/dev/null; then
    :
  else
    echo "  ⚠️ 挪回去失败（多半是跨卷），原件没动: ${f#$root/}" >&2
    rm -f "$tmp"
  fi
done

if [ $apply -eq 0 ]; then
  echo
  echo "以上是**只看不动**的结果。真要做，重跑一次并加 --apply："
  echo "  sh scripts/immunize-data-root.sh --apply"
  echo
  echo "做之前建议先冷备份一份数据目录（这台机器上有过白板被清光的教训）："
  echo "  cp -a \"$root\" \"$root-backup-\$(date +%Y%m%d-%H%M%S)\""
else
  echo
  echo "做完了。逐条结果见上面；没有输出 ⚠️ 就是全部重新出生成功。"
  echo "**验一眼**（下次故障发作时才看得出来）：那时再跑一次"
  echo "  sh scripts/diagnose-data-dir.sh"
  echo "它仍然会说「读不动」——因为探针文件是它自己**当场新建**的，那种照旧被拦；"
  echo "但 app 里的 Todo / 群聊应该照常刷得出来。这两件事不矛盾，别看到前一句就以为没用。"
fi
