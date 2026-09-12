#!/bin/sh
# 用法: verify-zip-notarized.sh <PendingCrew-X.Y.Z.zip>
#
# 断言：这个 zip 里的 .app **带着公证票据、且过得了 Gatekeeper**。不满足就退 2。
#
# 为什么需要这道门（2026-09-12）：
#   `build-macos-update.sh` 原来是「先把 zip 写进 Sparkle feed 目录 → 提交公证 →
#   staple → 再写一遍覆盖」。公证一失败，`set -e` 当场退出，**第一份没票据的 zip
#   就永远留在 feed 目录里**。0.1.35 那次就留下了一个（13.7 MB，`spctl` rejected），
#   而当时的记录写的是「停得很干净，没有半成品要收拾」。
#
#   那个目录是三条路的共同入口，每一条都会把它当成正经产物：
#     · `generate_appcast` 扫它 → 可能被签进 feed，推给所有自更新用户
#     · `publish-macos-update-r2.sh` **逐个文件**上传 → 挂到公开 CDN 上
#     · `publish-github-release.sh` 把它当 Release 资产传上去
#   而 feed 目录 **不进 git**，没有任何东西会替我们记得那儿躺了什么。
#
# **zip 不能直接 `stapler validate`**（它认 .app / .dmg / .pkg），所以解到临时目录再验。
# 判据是退出码，不需要读懂输出。
set -eu

zip=${1:?usage: verify-zip-notarized.sh <zip>}
test -f "$zip" || { echo "没有这个文件：$zip" >&2; exit 2; }

work=$(mktemp -d "/tmp/verify-zip-notarized.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM

/usr/bin/ditto -x -k "$zip" "$work" 2>/dev/null \
  || { echo "解不开：$zip" >&2; exit 2; }
app=$(/usr/bin/find "$work" -maxdepth 2 -name '*.app' | head -1)
[ -n "$app" ] || { echo "里面没有 .app：$zip" >&2; exit 2; }

xcrun stapler validate "$app" >/dev/null 2>&1 || {
  echo "✋ **没有公证票据**：$zip" >&2
  echo "   自更新装上去、或用户下载解开，都会被 Gatekeeper 拦。" >&2
  echo "   多半是某次公证失败留下的半成品 —— 先确认它是什么，删掉或补公证后重来。" >&2
  exit 2
}
spctl -a -vv -t exec "$app" >/dev/null 2>&1 || {
  echo "✋ **过不了 Gatekeeper**：$zip" >&2
  exit 2
}
