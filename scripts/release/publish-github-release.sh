#!/bin/sh
# 用法: scripts/release/publish-github-release.sh <版本> [--draft]
#
# 把 dist/ 里那一版**已经签名+公证+staple 好**的产物挂到 GitHub Release 上，
# 然后顺手把 Homebrew tap 更新到同一版。
#
# 两条纪律写在这里，免得下次又有人把它们分开：
#   ① **不构建任何东西。** 挂 Release 用手边已公证的产物 —— main 上随时可能躺着
#      还没人工验收过的改动，为了挂个包把它们构建成新版本，等于推给自更新用户。
#   ② **tap 更新是这条链路的一部分，不是「记得顺手做一下」。** cask 里的
#      version/sha256 手工维护必然腐烂，腐烂之后用户 brew 装到的是老包，而且
#      没有任何地方会报错。所以放在这儿，跟传产物同生共死。
set -eu

repo=${PENDING_REPO:-syncmeta/PendingCrew}
version=${1:?usage: publish-github-release.sh <version> [--draft]}
draft=${2:-}

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
dir="$root/dist/updates/pendingcrew"
dmg="$dir/PendingCrew-$version.dmg"
zip="$dir/PendingCrew-$version.zip"

for f in "$dmg" "$zip"; do
  test -f "$f" || { echo "缺 $f —— 先跑 build-macos-update.sh 和 make-dmg.sh。" >&2; exit 2; }
done

# 出门前最后一次把关：没公证的包用户双击会被 Gatekeeper 拦成「无法验证开发者」，
# 等于发了个装不上的东西。这一步便宜，别省。
echo "note: 复验 dmg 的签名与公证"
spctl -a -t open --context context:primary-signature "$dmg" >/dev/null 2>&1 \
  || { echo "$dmg 过不了 Gatekeeper（未公证 / 未签名）—— 拒绝发布。" >&2; exit 2; }
xcrun stapler validate "$dmg" >/dev/null 2>&1 \
  || { echo "$dmg 没 staple 上公证票 —— 用户离线首次打开会被拦。拒绝发布。" >&2; exit 2; }

if gh release view "v$version" --repo "$repo" >/dev/null 2>&1; then
  echo "note: v$version 已存在，补传产物"
  gh release upload "v$version" "$dmg" "$zip" --repo "$repo" --clobber
else
  # shellcheck disable=SC2086
  gh release create "v$version" "$dmg" "$zip" --repo "$repo" \
    --title "PendingCrew $version" --generate-notes $draft
fi

# 发布之后才更新 tap —— cask 的 sha256 取自 Release 上那个资产自己公布的摘要，
# 产物没传上去就没有可信的基准。
"$root/scripts/release/update-homebrew-tap.sh" "$version" "$dmg"
