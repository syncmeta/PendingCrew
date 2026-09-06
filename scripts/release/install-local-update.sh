#!/bin/sh
# 把一份**已经公证好的** PendingCrew 装到本机 /Applications，替掉正在跑的那一版。
#
#   用法（要脱离 PendingCrew 的进程组跑，见下）：
#     launchctl submit -l pendingcrew-local-install -- \
#       /bin/sh <仓库>/scripts/release/install-local-update.sh <PendingCrew.app 或 .zip>
#
# 为什么要 `launchctl submit` 而不是直接跑：装这一步要等 PendingCrew 退出，而它一退，
# 它底下所有 agent session 跟着结束 —— 包括那个正在替你跑安装脚本的 session。直接跑
# 等于让安装器自己被自己等的那件事杀掉。挂到 launchd 底下它就不在那棵进程树里了。
#
# 顺序（每一步都对应一次真实踩过的坑，别调换）：
#   1. 先验新包：签名 / 公证 / 版本戳 —— **不合格一个文件都不动**
#   2. 等界面进程退出（人按 ⌘Q）。**不替人杀** —— 那些 session 是活的子进程
#   3. SIGTERM 停 daemon。它装了优雅退出：先停光 session 再 exit(0)
#      （没有 `--daemon-stop` 这个命令 —— 2026-09-06 核过，只有 --daemon /
#        --daemon-attach / --daemon-status。停它的正路就是 SIGTERM）
#   4. **冷备份数据目录**（此刻没人在写了才叫冷）
#   5. 旧 app 移到回滚位（不放桌面：~/Desktop 受 TCC 保护，非交互进程读写会被拒）
#   6. 装新的、去隔离属性、再验一次
#   7. 重新打开
#
# 全程写日志到 ~/Library/Logs/PendingCrew/local-install-<时间>.log —— 因为跑到第 2 步
# 之后，就没有任何 agent 活着能把结果告诉你了。**日志是唯一的回执。**
set -eu

src=${1:-}
[ -n "$src" ] || { echo "用法: $0 <PendingCrew.app 或已公证的 .zip>" >&2; exit 2; }

ts=$(date +%Y%m%d-%H%M%S)
logdir="$HOME/Library/Logs/PendingCrew"
mkdir -p "$logdir"
log="$logdir/local-install-$ts.log"
exec >>"$log" 2>&1

say() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*"; }
die() { printf '[%s] ✗ %s\n' "$(date +%H:%M:%S)" "$*"; exit 1; }

say "=== 本机更新开始 ==="
say "源：$src"

# —— 1. 先验新包，验不过一个文件都不动 ——
work=$(mktemp -d /tmp/pendingcrew-local-install.XXXXXX)
case "$src" in
  *.zip)
    say "解压 zip → $work"
    ditto -x -k "$src" "$work" || die "解压失败"
    app="$work/PendingCrew.app"
    ;;
  *.app) app="$src" ;;
  *) die "只认 .app 或 .zip：$src" ;;
esac
[ -d "$app" ] || die "解出来没有 PendingCrew.app：$app"

plist="$app/Contents/Info.plist"
version=$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$plist") || die "读不到版本号"
build=$(/usr/libexec/PlistBuddy -c "Print :CFBundleVersion" "$plist") || die "读不到 build 号"
stamp=$(/usr/libexec/PlistBuddy -c "Print :BuildStampCommit" "$plist" 2>/dev/null || echo "")
say "新包：$version ($build) stamp=${stamp:-无}"

# spctl 判 app 用 -t install（判 dmg 是另一套口径，别混）
spctl -a -t install -vvv "$app" || die "签名/公证校验没过 —— 不装可疑的东西"
xcrun stapler validate "$app" || die "公证票据没 staple 上"
say "签名与公证：通过"

old="/Applications/PendingCrew.app"
if [ -d "$old" ]; then
  oldver=$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$old/Contents/Info.plist" 2>/dev/null || echo "?")
  oldbuild=$(/usr/libexec/PlistBuddy -c "Print :CFBundleVersion" "$old/Contents/Info.plist" 2>/dev/null || echo "?")
  say "在装的旧版：$oldver ($oldbuild)"
fi

# —— 2. 等界面退出。不替人杀 ——
say "等 PendingCrew 界面退出（请按 ⌘Q）…最多等 90 分钟"
# 同一个二进制会以三种身份在跑，名字全是 PendingCrew：
#   · 界面      MacOS/PendingCrew                 ← 只等这个
#   · 后台      MacOS/PendingCrew --daemon        ← 下一步我自己停
#   · MCP 帮手  MacOS/PendingCrew --mcp-serve …   ← 每个 session 一个，界面退了它们还在
# 2026-09-06 自测时第一版判据漏了 --mcp-serve，会把帮手当成「界面还在」，
# 死等 90 分钟然后放弃 —— 失败方向是安全的，但那 90 分钟纯白等。
gui_alive() {
  pgrep -fl "MacOS/PendingCrew" 2>/dev/null \
    | grep -v -- "--daemon" | grep -v -- "--mcp-serve" \
    | grep -q "MacOS/PendingCrew"
}
waited=0
while gui_alive; do
  waited=$((waited + 5))
  [ "$waited" -lt 5400 ] || die "等了 90 分钟界面还在，原样放弃，一个文件都没动"
  sleep 5
done
say "界面已退出（等了 ${waited}s）"

# —— 3. 停 daemon（SIGTERM，它会先停光 session 再 exit 0）——
dpid=$(pgrep -f "MacOS/PendingCrew --daemon" || true)
if [ -n "$dpid" ]; then
  say "停后台进程 pid=$dpid（SIGTERM，优雅退出）"
  kill -TERM $dpid || true
  n=0
  while kill -0 $dpid 2>/dev/null; do
    n=$((n + 1))
    [ "$n" -lt 60 ] || die "后台进程 60 秒没退，停手 —— 不在它还活着的时候换二进制"
    sleep 1
  done
  say "后台进程已退出（等了 ${n}s）"
else
  say "没有在跑的后台进程"
fi

# —— 4. 冷备份数据目录（此刻没人在写了才叫冷）——
data="$HOME/Library/Application Support/PendingCrew"
if [ -d "$data" ]; then
  backup="$HOME/Library/Application Support/PendingCrew-databackup-$ts"
  say "冷备份数据目录 → $backup"
  cp -R "$data" "$backup" || die "备份失败 —— 备份不成，一步都不走"
  say "备份完成：$(du -sh "$backup" | cut -f1)"
else
  say "没有数据目录，跳过备份"
fi

# —— 5. 旧 app 移到回滚位 ——
if [ -d "$old" ]; then
  rollbackdir="$HOME/Library/Application Support/PendingCrew-app-rollback"
  mkdir -p "$rollbackdir"
  rollback="$rollbackdir/PendingCrew-old-$ts.app"
  say "旧 app → $rollback"
  mv "$old" "$rollback" || die "移动旧 app 失败"
fi

# —— 6. 装新的 ——
say "装新的 → $old"
if ! cp -R "$app" "$old"; then
  say "拷贝失败，回滚"
  [ -d "${rollback:-}" ] && mv "$rollback" "$old"
  die "装新版失败，已把旧版放回去"
fi
xattr -dr com.apple.quarantine "$old" 2>/dev/null || true
spctl -a -t install -vvv "$old" || die "装完校验没过 —— 旧版在 ${rollback:-回滚位}，手动放回去"
newstamp=$(/usr/libexec/PlistBuddy -c "Print :BuildStampCommit" "$old/Contents/Info.plist" 2>/dev/null || echo "")
[ "$newstamp" = "$stamp" ] || die "装完的戳跟源对不上（$newstamp vs $stamp）"
say "装完校验：通过，戳对得上 $newstamp"

# —— 7. 重新打开 ——
say "重新打开"
open -a "$old" || say "⚠️ open 失败，手动打开一下"

rm -rf "$work"
say "=== 完成：$version ($build) ==="
say "回滚：rm -rf $old && cp -R ${rollback:-<回滚位>} $old"
