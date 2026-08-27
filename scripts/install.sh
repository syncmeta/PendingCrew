#!/bin/sh
# PendingCrew 一键安装（没有 Homebrew 时用这个；有 Homebrew 请优先用 brew，见 README）
#
#   先看一眼再执行 —— 这段脚本要在你的机器上放一个 app，值得你花十秒读完：
#     curl -fsSLO https://raw.githubusercontent.com/syncmeta/PendingCrew/main/scripts/install.sh
#     less install.sh && sh install.sh
#
# 它做的事，就这些：
#   1. 查 macOS 版本（要 14+）和芯片
#   2. 从 GitHub Release 取最新一版的 .dmg
#   3. **按 Release 上公布的 sha256 校验**，对不上立刻中止（不装可疑的东西）
#   4. 挂载 → 拷进 /Applications → 卸载镜像 → 去掉隔离属性
#
# 它**不做**的事：不装任何后台服务、不改你的 shell 配置、不碰
# ~/Library 里的任何数据、**不会替你杀掉正在运行的 PendingCrew**（见下）。
set -eu

repo=syncmeta/PendingCrew
app_name=PendingCrew
dest=/Applications

say() { printf '%s\n' "$*"; }
die() { printf '✗ %s\n' "$*" >&2; exit 1; }

# —— 1. 环境 ——
[ "$(uname -s)" = "Darwin" ] || die "这是 macOS 专用的 app。"
os=$(sw_vers -productVersion)
major=${os%%.*}
[ "$major" -ge 14 ] 2>/dev/null || die "需要 macOS 14 (Sonoma) 或更新，你这台是 ${os}。"

# 正在跑的话就停手让人自己退 —— **不替用户杀进程**：那些 session 是活的子进程，
# 替他 kill 等于替他中断正在干的活。
if pgrep -x "$app_name" >/dev/null 2>&1; then
  die "PendingCrew 正在运行。请先退出它（⌘Q）再跑这个脚本 —— 我不会替你结束它，
  那会中断正在跑的 session。"
fi

command -v curl >/dev/null || die "找不到 curl。"

# —— 2. 问 GitHub 要最新一版 ——
say "→ 查最新版本…"
api="https://api.github.com/repos/$repo/releases/latest"
meta=$(curl -fsSL -H 'Accept: application/vnd.github+json' "$api") \
  || die "拉不到 Release 信息（${api}）。网络不通，或者仓库还没有已发布的版本。"

# 只认 .dmg 那一个资产。Release 上还有一个给 Sparkle 自更新用的 .zip，不是给这儿的。
url=$(printf '%s' "$meta" | /usr/bin/awk -F'"' '/"browser_download_url"/ && /\.dmg"/ {print $4; exit}')
tag=$(printf '%s' "$meta" | /usr/bin/awk -F'"' '/"tag_name"/ {print $4; exit}')
[ -n "$url" ] || die "最新的 Release（${tag:-?}）里没有 .dmg 资产。"

# GitHub 在资产上公布 sha256 摘要，形如 "digest": "sha256:abcd…"。
# 拿它当校验基准 —— 它和文件来自同一个 API 响应，能挡住传输损坏和被掉包的镜像。
want=$(printf '%s' "$meta" \
  | /usr/bin/tr ',' '\n' \
  | /usr/bin/awk -F'"' '/"digest"/ && /sha256:/ {sub(/^sha256:/, "", $4); print $4; exit}')

say "→ 最新版本：$tag"

tmp=$(mktemp -d "/tmp/$app_name-install.XXXXXX")
cleanup() {
  [ -n "${mnt:-}" ] && /usr/bin/hdiutil detach "$mnt" -quiet 2>/dev/null || true
  rm -rf "$tmp"
}
trap cleanup EXIT HUP INT TERM

dmg="$tmp/$app_name.dmg"
say "→ 下载…"
curl -fL --progress-bar -o "$dmg" "$url" || die "下载失败。"

# —— 3. 校验。对不上就停，绝不「先装上再说」——
if [ -n "$want" ]; then
  got=$(/usr/bin/shasum -a 256 "$dmg" | /usr/bin/awk '{print $1}')
  [ "$got" = "$want" ] || die "下载到的文件校验和对不上，已中止，什么都没装。
  期望 $want
  实际 $got"
  say "→ sha256 校验通过"
else
  die "这个 Release 没有公布 sha256 摘要，无法校验下载内容 —— 中止。
  你可以自己去 https://github.com/$repo/releases 手动下载 .dmg。"
fi

# —— 4. 装 ——
say "→ 挂载并安装到 $dest…"
mnt=$(/usr/bin/hdiutil attach "$dmg" -nobrowse -readonly | /usr/bin/awk -F'\t' '/\/Volumes\// {print $NF}' | tail -1)
[ -d "${mnt:-}" ] || die "挂载失败。"
[ -d "$mnt/$app_name.app" ] || die "镜像里没有 $app_name.app。"

# 覆盖安装：先把旧的移走再放新的（直接往里拷会留下上一版的残留文件）。
if [ -d "$dest/$app_name.app" ]; then
  say "→ 替换已安装的版本"
  rm -rf "$tmp/old.app"
  mv "$dest/$app_name.app" "$tmp/old.app" || die "移走旧版本失败（可能需要 sudo，或 app 还开着）。"
fi
/usr/bin/ditto "$mnt/$app_name.app" "$dest/$app_name.app" || die "拷贝到 $dest 失败。"

# 去掉隔离属性：包本身已经过 Apple 公证并 staple，去掉它只是免掉那一次
# 「从互联网下载」的确认框，不降低任何安全检查。
/usr/bin/xattr -dr com.apple.quarantine "$dest/$app_name.app" 2>/dev/null || true

say ""
say "✓ 装好了：$dest/$app_name.app"
say ""
say "开之前先确认本机装了并且登录过 claude 或 codex 至少一个："
say "    claude --version    # 或 codex --version"
say ""
say "然后：open -a $app_name"
