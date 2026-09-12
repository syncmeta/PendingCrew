#!/bin/sh
# 用法: scripts/release/verify-published-release.sh <版本>
#
# **验「外面下得到的那一份」**，不是「我们打包时手里那一份」。
#
# 这两件事不一样，而区别只在出事时才显出来：`verify-zip-notarized.sh` 验的是
# dist/ 里的产物（发布前把关，对的）；但传上去之后还有几种坏法它一概看不见 ——
# 传了一半、传错文件、cask 的 sha256 没跟着更新、Release 还挂着草稿。
# 2026-09-12 我是手工走了一遍才确认 0.1.35 真的装得上；手工的东西下次就没人做了。
#
# 判据全部落在**真下载回来的字节**上：
#   ① Release 存在且不是草稿（草稿资产外人下不到）
#   ② dmg / zip 两个资产都在
#   ③ 真下 dmg，sha256 与 Homebrew cask 里写的**逐字相同**（brew 就是拿它校验的）
#   ④ 公证票据 staple 得上（决定离线首次打开会不会被拦）
#   ⑤ Gatekeeper 判 accepted
#   ⑥ 包里那个 app 的版本号确实是这一版（防发错包）
#
# **它不验什么**：zip 那一份只验存在与大小 —— 完整验它要解包，而 Sparkle 自更新
# 那条路在 `build-macos-update.sh` 的闸里已经验过一次。这条边界写在这儿，
# 免得跑完一趟以为「自更新也验过了」。
set -eu

version=${1:?usage: verify-published-release.sh <version>}
repo=${PENDING_REPO:-syncmeta/PendingCrew}
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cask="$root/packaging/homebrew/Casks/pendingcrew.rb"

work=$(mktemp -d "/tmp/verify-published.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM

echo "① Release 状态"
draft=$(gh release view "v$version" --repo "$repo" --json isDraft --jq .isDraft 2>/dev/null) || {
  echo "   ❌ 取不到 v$version —— 没发布，或没权限。" >&2; exit 2; }
[ "$draft" = "false" ] || { echo "   ❌ v$version 还是草稿：资产外人下不到，cask 会装失败。" >&2; exit 2; }
echo "   ok（已发布）"

echo "② 资产齐不齐"
names=$(gh release view "v$version" --repo "$repo" --json assets --jq '.assets[].name')
for want in "PendingCrew-$version.dmg" "PendingCrew-$version.zip"; do
  echo "$names" | grep -qx "$want" || { echo "   ❌ 缺 $want" >&2; exit 2; }
done
echo "   ok（dmg + zip 都在）"

echo "③ 真下 dmg，核 sha256 与 cask 是否逐字相同"
url=$(gh release view "v$version" --repo "$repo" --json assets \
      --jq ".assets[]|select(.name==\"PendingCrew-$version.dmg\")|.url")
curl -fsSL -o "$work/app.dmg" "$url" || { echo "   ❌ 下不下来：$url" >&2; exit 2; }
got=$(shasum -a 256 "$work/app.dmg" | awk '{print $1}')
want=$(awk '/sha256 /{gsub(/[",]/,""); print $2; exit}' "$cask")
echo "   cask: $want"
echo "   实际: $got"
[ "$got" = "$want" ] || { echo "   ❌ 对不上 —— brew 会在校验这一步失败。" >&2; exit 2; }
echo "   ok"

echo "④ 公证票据"
xcrun stapler validate "$work/app.dmg" >/dev/null 2>&1 \
  || { echo "   ❌ 没 staple 上：用户离线首次打开会被拦。" >&2; exit 2; }
echo "   ok"

echo "⑤ Gatekeeper"
spctl -a -t open --context context:primary-signature "$work/app.dmg" >/dev/null 2>&1 \
  || { echo "   ❌ 过不了 Gatekeeper。" >&2; exit 2; }
echo "   ok"

echo "⑥ 包里的版本号"
mnt=$(hdiutil attach -nobrowse -readonly "$work/app.dmg" 2>/dev/null | awk -F'\t' '/Volumes/{print $NF}')
[ -n "$mnt" ] || { echo "   ❌ 挂不上这个 dmg。" >&2; exit 2; }
inside=$(defaults read "$mnt/PendingCrew.app/Contents/Info.plist" CFBundleShortVersionString 2>/dev/null || echo "?")
hdiutil detach "$mnt" >/dev/null 2>&1 || true
[ "$inside" = "$version" ] || { echo "   ❌ 包里是 $inside，不是 $version —— 发错包了。" >&2; exit 2; }
echo "   ok（$inside）"

echo
echo "v$version 外面下得到的那一份：装得上、打得开、版本对。"
