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

# **zip 也要验，而且理由比 dmg 更硬**（2026-09-12 补）：dmg 是人手动下载的，
# zip 是 **Sparkle 自更新吃的那一份**。原来这里只验 dmg —— 于是一个没公证的 zip
# 可以一路挂上 Release，而 dmg 全绿、脚本一声不吭。
#
# 这不是假想：0.1.35 公证失败那次，`dist/updates/pendingcrew/PendingCrew-0.1.35.zip`
# 就是个没票据的包（`spctl` rejected），在 feed 目录里躺了一上午。
#
# zip 不能直接 `stapler validate`（它认 .app/.dmg/.pkg），所以解到临时目录再验。
echo "note: 复验 zip 的签名与公证（Sparkle 自更新吃的就是它）"
zt=$(mktemp -d "/tmp/pendingcrew-zipcheck.XXXXXX")
trap 'rm -rf "$zt"' EXIT INT TERM
/usr/bin/ditto -x -k "$zip" "$zt" 2>/dev/null \
  || { echo "$zip 解不开 —— 拒绝发布。" >&2; exit 2; }
zip_app=$(/usr/bin/find "$zt" -maxdepth 2 -name '*.app' | head -1)
[ -n "$zip_app" ] || { echo "$zip 里没有 .app —— 拒绝发布。" >&2; exit 2; }
xcrun stapler validate "$zip_app" >/dev/null 2>&1 \
  || { echo "$zip 里的 app 没 staple 上公证票 —— 自更新装上去会被 Gatekeeper 拦。拒绝发布。" >&2; exit 2; }
spctl -a -vv -t exec "$zip_app" >/dev/null 2>&1 \
  || { echo "$zip 里的 app 过不了 Gatekeeper —— 拒绝发布。" >&2; exit 2; }

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
# ⚠️ annotated tag 有两层：`refs/tags/X` 是 **tag 对象**自己的 sha，
# `refs/tags/X^{}` 才是它指向的 commit。以前这里只取前者 —— 对
# **轻量 tag**（发版脚本自己 `git tag X <commit>` 打的那种）恰好相等，所以一直没露馅；
# 有人改用 `git tag -a` 之后，这条判据会拿 tag 对象的 sha 去跟 commit 比，
# **稳定误报「对不上，拒绝发布」**，而产物其实是对的。2026-09-09 发 0.1.31 时撞上。
# 取 `^{}` 那一行；它对两种 tag 都成立（轻量 tag 没有 `^{}` 行时回退到普通那行）。
remote_tag=$(git -C "$root" ls-remote --tags origin "v$version^{}" | cut -f1)
[ -n "$remote_tag" ] || remote_tag=$(git -C "$root" ls-remote --tags origin "v$version" | cut -f1)
[ "$remote_tag" = "$build_commit" ] || {
  echo "远端 tag v$version 指向 ${remote_tag}，产物却构建自 $build_commit —— 对不上，拒绝发布。" >&2
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
  # ⚠️ **两个 trap 只有最后一个算数** —— 这里必须把上面那个解压临时目录一起带上，
  # 否则每发一版漏一个几十 MB 的 /tmp 目录，而且一声不吭（`trap` 是覆盖不是叠加）。
  trap 'rm -f "$notes"; rm -rf "$zt"' EXIT INT TERM
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
