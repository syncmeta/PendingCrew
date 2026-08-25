#!/bin/sh
# 用法: scripts/release/update-homebrew-tap.sh <版本> [dmg 路径]
#
# 把 Homebrew cask 里的 version / sha256 更新到这一版，提交并推到 tap 仓库。
#
# **为什么必须是脚本而不是「记得手工改」**：cask 里那两个字段每发一版都要动，
# 手工维护必然腐烂 —— 发个一两次之后 tap 就停在旧版本，而用户
# `brew install` 装到的是老包，且没有任何地方会报错。所以这一步接进发版链路，
# 由 scripts/release/publish-github-release.sh 在传完产物之后调用。
#
# 真值来源：**GitHub Release 上那个资产自己公布的 sha256 摘要**，不是本地文件
# 算出来的 —— 用户 brew 下的是 Release 上那一份，就该拿那一份的摘要当基准。
# 给了本地 dmg 路径的话会额外比一次，两边不一致直接中止（说明传上去的和手边
# 这份不是同一个文件）。
set -eu

repo=${PENDING_REPO:-syncmeta/PendingCrew}
tap_repo=${PENDING_TAP_REPO:-syncmeta/homebrew-tap}
version=${1:?usage: update-homebrew-tap.sh <version> [dmg]}
local_dmg=${2:-}

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
template="$root/packaging/homebrew/Casks/pendingcrew.rb"
test -f "$template" || { echo "找不到 $template" >&2; exit 2; }

asset="PendingCrew-$version.dmg"

# 草稿 Release 上的资产**外人下不到**（URL 要登录，公开访问 404）。
# 这里如果照常更新 cask，`brew install --cask syncmeta/tap/pendingcrew`
# 会对**所有用户**立刻坏掉，而且没有任何地方会报错 —— 我们自己用 gh
# （带 token）去看那个 draft 是好的，看不出问题。所以在这儿拦死。
# 发布之后（`gh release edit v<版本> --draft=false`）再跑一次本脚本即可。
if [ "$(gh release view "v$version" --repo "$repo" --json isDraft --jq .isDraft 2>/dev/null)" = "true" ]; then
  echo "v$version 还是**草稿**，不更新 tap —— 草稿资产外人下不到，cask 指过去等于把" >&2
  echo "brew 安装路径给所有人弄坏。发布之后再跑：" >&2
  echo "  scripts/release/update-homebrew-tap.sh $version" >&2
  exit 3
fi

echo "note: 问 GitHub 要 $repo v$version 上 $asset 的摘要"
digest=$(gh release view "v$version" --repo "$repo" \
  --json assets --jq ".assets[] | select(.name==\"$asset\") | .digest" 2>/dev/null || true)
digest=${digest#sha256:}
[ -n "$digest" ] || {
  echo "v$version 的 Release 上没有 $asset（或者还没发布）—— 先把产物传上去再更新 tap。" >&2
  exit 2
}

if [ -n "$local_dmg" ]; then
  got=$(/usr/bin/shasum -a 256 "$local_dmg" | /usr/bin/awk '{print $1}')
  [ "$got" = "$digest" ] || {
    echo "本地 dmg 与 Release 上那份不是同一个文件 —— 中止，别让 cask 指向一个你没验过的包。" >&2
    echo "  Release $digest" >&2
    echo "  本地    $got" >&2
    exit 2
  }
fi

# tap 的工作副本：优先用本机已 tap 的那份，否则临时 clone。
tapdir=$(brew --repository 2>/dev/null)/Library/Taps/${tap_repo%%/*}/homebrew-${tap_repo##*/}
if [ ! -d "$tapdir/.git" ]; then
  tmp=$(mktemp -d /tmp/pendingcrew-tap.XXXXXX)
  trap 'rm -rf "$tmp"' EXIT HUP INT TERM
  git clone --quiet "https://github.com/$tap_repo.git" "$tmp/tap" || {
    echo "clone 不到 tap 仓库 https://github.com/$tap_repo —— 它还没建。" >&2
    echo "建仓库这一下要人点头，不代劳。建好之后重跑这条命令即可。" >&2
    exit 3
  }
  tapdir="$tmp/tap"
fi

mkdir -p "$tapdir/Casks"
sed -e "s/^  version \".*\"$/  version \"$version\"/" \
    -e "s/^  sha256 \".*\"$/  sha256 \"$digest\"/" \
    "$template" > "$tapdir/Casks/pendingcrew.rb"

# 回写模板，让仓库里那份和 tap 上那份始终一致（否则下次发版的基线就是旧的）。
cp "$tapdir/Casks/pendingcrew.rb" "$template"

# 落地自查：改完的文件里必须真的是这一版的号和摘要。
grep -q "version \"$version\"" "$tapdir/Casks/pendingcrew.rb" || { echo "版本号没写进去" >&2; exit 2; }
grep -q "sha256 \"$digest\"" "$tapdir/Casks/pendingcrew.rb" || { echo "摘要没写进去" >&2; exit 2; }

if command -v brew >/dev/null 2>&1; then
  brew style --cask "$tapdir/Casks/pendingcrew.rb" >/dev/null \
    || { echo "brew style 不过 —— 别推一个格式坏的 cask。" >&2; exit 2; }
fi

if git -C "$tapdir" diff --quiet -- Casks/pendingcrew.rb; then
  echo "note: tap 上已经是 $version，无需改动"
  exit 0
fi

git -C "$tapdir" add Casks/pendingcrew.rb
git -C "$tapdir" commit -q -m "pendingcrew $version"
git -C "$tapdir" push -q origin HEAD || {
  echo "推 tap 失败 —— cask 已在本地改好（$tapdir/Casks/pendingcrew.rb），手工推一下。" >&2
  exit 2
}
echo "✓ tap 已更新到 $version（$tap_repo）"
