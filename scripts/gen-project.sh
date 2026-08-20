#!/bin/sh
# 用法: scripts/gen-project.sh [--fetch]
#
# 生成 PendingCrew.xcodeproj。**改了 project.yml（新增 Swift 文件也算）就跑这个，
# 别直接跑 `xcodegen`。**
#
# 为什么要包一层：`.xcodeproj` 是生成物，但它被提交进仓库（好让 clone 下来的人
# 不装 XcodeGen 也能直接开工程）。既然产物进了版本库，**产生它的那个生成器的
# 版本就是仓库的一部分**，不能由每台机器各自的 brew 状态决定 —— 否则某天
# 上游一升级，所有人的 PR 会同时变红，包括每一个什么都没做错的人。一个出过
# 一次大规模假红的检查，之后就没人再看它了，那这道门就白建了。
#
# 所以版本的唯一真值是仓库根目录的 `.xcodegen-version`：
#   - CI 装的就是它（不是 brew 拿到的最新）
#   - 本机版本对不上时，**在生成任何东西之前**就停下并说清楚 —— 而不是让人
#     先看到一坨看不懂的 pbxproj diff 再去猜自己是不是忘了 regen
#   - 换版本 = 一次显式提交（改数字 + 补 checksum + 同笔重新生成 pbxproj），
#     于是历史上「这次 pbxproj 大改是因为换了生成器」是自解释的
#
# --fetch: 本机没有 / 版本不对时，把仓库声明的那一版下到 `.tools/`（gitignored）
#          里用，**不动你系统里已装的那个**。CI 走的就是这条。
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

want=$(tr -d ' \t\r\n' < .xcodegen-version)
[ -n "$want" ] || { echo "读不到 .xcodegen-version" >&2; exit 2; }

fetch=0
[ "${1:-}" = "--fetch" ] && fetch=1

cached="$root/.tools/xcodegen-$want/bin/xcodegen"

version_of() {
  # `xcodegen --version` 打的是 "Version: 2.46.0"
  "$1" --version 2>/dev/null | sed -n 's/^Version:[[:space:]]*//p' | tr -d ' \t\r\n'
}

pick() {
  # 依次试：调用者显式指定的 → 仓库缓存的 → PATH 上的。第一个版本对上的就用。
  for cand in ${XCODEGEN:-} "$cached" "$(command -v xcodegen 2>/dev/null || true)"; do
    [ -n "$cand" ] && [ -x "$cand" ] || continue
    if [ "$(version_of "$cand")" = "$want" ]; then printf '%s' "$cand"; return 0; fi
  done
  return 1
}

if ! bin=$(pick); then
  if [ "$fetch" = 1 ]; then
    expected=$(awk -v v="$want" '$1 == v { print $2 }' "$root/scripts/xcodegen-checksums.txt")
    [ -n "$expected" ] || {
      echo "scripts/xcodegen-checksums.txt 里没有 $want 的校验和 —— 换版本时这两个文件要一起改。" >&2
      exit 2
    }
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT HUP INT TERM
    url="https://github.com/yonaskolb/XcodeGen/releases/download/$want/xcodegen.zip"
    echo "note: 下载 XcodeGen $want"
    curl -fsSL -o "$tmp/xcodegen.zip" "$url"
    actual=$(shasum -a 256 "$tmp/xcodegen.zip" | awk '{print $1}')
    [ "$actual" = "$expected" ] || {
      echo "下载到的 xcodegen.zip 校验和对不上 —— 拒绝使用。" >&2
      echo "  期望 $expected" >&2
      echo "  实际 $actual" >&2
      exit 2
    }
    /usr/bin/unzip -q -o "$tmp/xcodegen.zip" -d "$tmp/unpacked"
    mkdir -p "$root/.tools"
    rm -rf "$root/.tools/xcodegen-$want"
    mv "$tmp/unpacked/xcodegen" "$root/.tools/xcodegen-$want"
    bin=$cached
    [ "$(version_of "$bin")" = "$want" ] || {
      echo "下下来的这份自称 $(version_of "$bin")，不是 $want" >&2; exit 2; }
  else
    have=$(command -v xcodegen >/dev/null 2>&1 && version_of "$(command -v xcodegen)" || echo "没装")
    echo "✗ XcodeGen 版本对不上，**什么都还没生成**。" >&2
    echo "" >&2
    echo "  本仓库要求：$want   （.xcodegen-version）" >&2
    echo "  你这台是：  $have" >&2
    echo "" >&2
    echo "  提前拦在这里，是为了不让你先看到一坨看不懂的 pbxproj diff、" >&2
    echo "  再花时间怀疑自己是不是忘了 regen —— 那是同一种症状。" >&2
    echo "" >&2
    echo "  两条路，选一条：" >&2
    echo "    scripts/gen-project.sh --fetch    # 把 $want 下到 .tools/ 里用，不动你系统里的" >&2
    echo "    XCODEGEN=/path/to/xcodegen scripts/gen-project.sh" >&2
    exit 2
  fi
fi

exec "$bin" generate
