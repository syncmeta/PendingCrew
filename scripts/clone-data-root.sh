#!/bin/sh
# 用法: sh scripts/clone-data-root.sh [目标路径]
#
# 把整个 PendingCrew 数据根**克隆**一份出来（APFS clonefile，不是复制）。
#
# ## 为什么要有这个：故障期间 `cp` 一个字节都拿不到
#
# 仓库里有条硬规矩：**换掉正在跑的 app 之前先冷备份数据目录**（2026-08-11 白板
# 被清光那次立的）。而那个周期性 EPERM 一发作，这条规矩就**执行不了**：
#
#     $ cp <数据根里任何一个已存在的文件> /tmp/
#     cp: ... Operation not permitted
#
# `cp` / `ditto` / `rsync` / `tar` 全都要 `open()` 源文件，而 open 正是被拒的那个。
# `cp -c` 也不行 —— 它在决定用不用克隆之前就先 open 了。
#
# ## clonefile(2) 能过，因为它按**路径**克隆，从不打开源
#
# 2026-09-13 07:09 在第 5 次发作期间实测：整棵数据根一次调用克隆成功，
# 两边 `find -type f` 都是 4602 个。
#
# ⚠️ **克隆出来的文件此刻同样读不出来**（标记跟着克隆走，见
# `docs/internal/2026-09-12-eperm-marker-travels.md` 第 1 条）。这是**预期**，
# 不是备份失败：字节在里面，等这一窗过去就读得动了。
# 备份的意义是「换 app 之前先把现状钉住」，不是「现在就能翻」。
#
# ## 空间
#
# 克隆是 COW：两边共享块，直到某一边被改。`du` 不知道这回事，会把整棵树按
# **表观大小**报一遍（实测报 394M）—— 那个数不是新占的磁盘。真正的增量我没量。
set -eu

src="$HOME/Library/Application Support/PendingCrew"
dest=${1:-"$HOME/PendingCrew-backup-$(date +%Y%m%d-%H%M%S)"}

[ -d "$src" ] || { echo "找不到数据根：${src}" >&2; exit 2; }

# **目标必须不存在**：clonefile 要求目标是新的，而且「往一个已有备份里再塞一份」
# 本身就是个会让人搞混哪份是哪份的动作。
[ ! -e "$dest" ] || { echo "目标已存在，拒绝覆盖：${dest}" >&2; exit 2; }

/usr/bin/python3 - "$src" "$dest" <<'PY'
import ctypes, os, sys

libc = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
libc.clonefile.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint32]
libc.clonefile.restype = ctypes.c_int

src, dest = sys.argv[1], sys.argv[2]
if libc.clonefile(src.encode(), dest.encode(), 0) != 0:
    e = ctypes.get_errno()
    print(f"clonefile 失败：errno={e}（{os.strerror(e)}）", file=sys.stderr)
    if e == 18:  # EXDEV
        print("目标跟数据根不在同一个卷上 —— 克隆只能在卷内做。换个同卷的目标。",
              file=sys.stderr)
    sys.exit(1)
PY

# 核一下：`readdir` / `stat` 在故障期间是通的，所以**数数是能做的**
# —— 逐字节比对不能做（那要 open）。这是这一步能给出的最强判据，把它的边界说清楚。
n_src=$(find "$src" -type f 2>/dev/null | wc -l | tr -d ' ')
n_dest=$(find "$dest" -type f 2>/dev/null | wc -l | tr -d ' ')
echo "已克隆：${dest}"
echo "文件数 源 ${n_src} / 备份 ${n_dest}"
[ "$n_src" = "$n_dest" ] || {
  echo "两边文件数对不上 —— 这份备份不可信，别拿它当依据。" >&2
  exit 2
}
echo "note: 只核了文件数。逐字节比对要 open()，故障期间做不了。"
