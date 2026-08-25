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
# zip 在 Sparkle 的 feed 目录里（自动更新吃它）；dmg 单独一个目录 —— 它不能待在
# feed 目录，否则 generate_appcast 会因为「同一 bundle version 两个归档」而拒。
zip="$root/dist/updates/pendingcrew/PendingCrew-$version.zip"
dmg="$root/dist/releases/pendingcrew/PendingCrew-$version.dmg"

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

# tag 必须先在远端，而且必须指向**产物真正的来源**。
#
# 不先推的话，`gh release create` 会替你在远端凭空造一个 tag —— 造在
# **origin 默认分支当前的 HEAD** 上。本地 main 领先 origin 时（这台机器上是常态，
# 十几个 session 各自往 main 落东西、谁也没推），那个 tag 就指向一堆根本不在这个
# 包里的代码，而且**一声不吭**。2026-08-25 发 0.1.15 时就这么错过一次：产物构建自
# a0e5d5e，GitHub 上的 v0.1.15 却指着 7ad1669，事后才发现。
build_commit=$(git -C "$root" rev-parse "v$version^{commit}" 2>/dev/null) || {
  echo "本地没有 v$version 这个 tag —— 发版脚本会打它，先把构建跑完。" >&2
  exit 2
}
git -C "$root" merge-base --is-ancestor "$build_commit" origin/main 2>/dev/null || {
  echo "v$version 指向的 $build_commit 还没推到 origin/main。" >&2
  echo "先 git push origin main，再重跑 —— 别发一个源码不在 GitHub 上的 Release。" >&2
  exit 2
}
git -C "$root" push origin "refs/tags/v$version" 2>/dev/null || true
remote_tag=$(git -C "$root" ls-remote --tags origin "v$version" | cut -f1)
[ "$remote_tag" = "$build_commit" ] || {
  echo "远端 tag v$version 指向 $remote_tag，产物却构建自 $build_commit —— 对不上，拒绝发布。" >&2
  exit 2
}

if gh release view "v$version" --repo "$repo" >/dev/null 2>&1; then
  # 正文不动：这一版可能已经被人手工编辑过，补传产物不该顺手覆盖掉它。
  echo "note: v$version 已存在，补传产物（正文保持原样）"
  gh release upload "v$version" "$dmg" "$zip" --repo "$repo" --clobber
else
  # Release 正文取自 CHANGELOG.md，不用 --generate-notes —— 那个会把提交标题
  # 列成一串倒给用户看。取不到就在这儿挂掉，此时还什么都没传上去。
  notes=$(mktemp)
  trap 'rm -f "$notes"' EXIT INT TERM
  "$root/scripts/release/changelog-section.sh" "$version" > "$notes"
  # shellcheck disable=SC2086
  gh release create "v$version" "$dmg" "$zip" --repo "$repo" \
    --title "PendingCrew $version" --notes-file "$notes" $draft
fi

# 发布之后才更新 tap —— cask 的 sha256 取自 Release 上那个资产自己公布的摘要，
# 产物没传上去就没有可信的基准。
#
# 草稿则不更新：草稿资产外人下不到，cask 指过去会把 brew 安装路径给所有人弄坏。
# （update-homebrew-tap.sh 自己也拦这一道，这里先说清楚，免得看着像忘了做。）
if [ "$draft" = "--draft" ]; then
  echo "note: v$version 是草稿，暂不更新 Homebrew tap。"
  echo "      人类点了发布（gh release edit v$version --draft=false）之后，跑："
  echo "        scripts/release/update-homebrew-tap.sh $version $dmg"
else
  "$root/scripts/release/update-homebrew-tap.sh" "$version" "$dmg"
fi
