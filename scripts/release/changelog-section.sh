#!/bin/sh
# 用法: changelog-section.sh <version> [changelog路径]
#
# 从 CHANGELOG.md 里取出 `## <version>` 那一段的正文（不含标题行），原样输出 Markdown。
# GitHub Release 正文和 app 内更新弹窗都从这里取 —— **两处共用这一个取段实现**，
# 各写各的必然漂移。
#
# 取不到就非零退出，绝不回落到「那就拿 git log 自动生成」：那正是这个脚本要根治的
# 毛病 —— 用户会在更新弹窗里读到「docs: README 按作者手改的版本重写」这种提交标题。
# 静默兜底等于没改。
set -eu

version=${1:?usage: changelog-section.sh <version> [changelog]}
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
changelog=${2:-${PENDING_CHANGELOG:-$root/CHANGELOG.md}}

if [ ! -f "$changelog" ]; then
  echo "找不到更新日志：$changelog" >&2
  exit 1
fi

# 标题行按空白切分后的第一段等于版本号才算命中（允许 `## 0.1.13 (2026-08-23)` 这种写法）
if ! awk -v v="$version" '
  /^##[ \t]/ {
    line = $0
    sub(/^##[ \t]+/, "", line)
    split(line, a, /[ \t]+/)
    if (a[1] == v) { found = 1; exit }
  }
  END { exit(found ? 0 : 1) }
' "$changelog"; then
  echo "$changelog 里没有 '## $version' 这一段。" >&2
  echo "去 $changelog 补一段 '## $version'，写清这版对用的人有什么不同，再重跑。" >&2
  exit 1
fi

# 命中的标题之后、下一个 `## ` 之前的所有行，掐掉首尾只有空白的行
# （只用 sed '/./,$!d' 掐不掉「只有几个空格」的行，那种段会假装非空混过去）
section=$(awk -v v="$version" '
  /^##[ \t]/ {
    if (in_sec) exit
    line = $0
    sub(/^##[ \t]+/, "", line)
    split(line, a, /[ \t]+/)
    if (a[1] == v) in_sec = 1
    next
  }
  in_sec { print }
' "$changelog" | awk '
  { L[n++] = $0 }
  END {
    first = -1; last = -1
    for (i = 0; i < n; i++) if (L[i] ~ /[^ \t]/) { if (first < 0) first = i; last = i }
    if (first < 0) exit
    for (i = first; i <= last; i++) print L[i]
  }
')

if [ -z "$section" ]; then
  echo "$changelog 里 '## $version' 这一段是空的。" >&2
  echo "去 $changelog 的 '## $version' 下面把这版的更新日志写上，再重跑。" >&2
  exit 1
fi

printf '%s\n' "$section"
