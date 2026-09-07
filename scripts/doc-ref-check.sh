#!/bin/sh
# 文档引用腐烂检测器：只判「一定烂了」的那半，不判「讲得对不对」。
#
#   用法： sh scripts/doc-ref-check.sh [仓库根]        # 默认 = 本脚本的上一级
#   退出： 0 = 没有违规；1 = 有违规（名单在 stdout）；2 = 用法错
#
# 判据只有两条，都不需要读懂那一行写了什么：
#   ① 行号越界： 引用 path:N，而 path 只有 M 行、N > M
#   ② 文件不存在：引用 path:N，而 path 不在树里
# **刻意不做**「这一行是不是真的讲那件事」——那要语义，尺子一有语义就开始误报，
# 然后被人关掉。零误报是这把尺子唯一的卖点，宁可漏也不许错。
#
# 什么才算「一条引用」（这条界定就是零误报的全部所在）：
#   形如 `<路径>:<数字>`，且**路径的第一段是仓库根下真实存在的条目**。
#   这一条同时把下面这些挡在门外，不需要为它们各写一条特例：
#     · 裸文件名 `AgentTerminalSession.swift:12`  —— 第一段不在根下 ⇒ 不是引用（那批的清理另有其事，不归这把尺子）
#     · 别的仓库  `apps/edge/src/routes/crew.ts:88` —— 本仓库根下没有 apps/ ⇒ 无从判断，不碰
#     · 半截路径  `Mac/Views/CrewDetailInspector.swift:20` —— 同上（它省掉了 Sources/）
#     · URL / 绝对路径 `http://127.0.0.1:10858`、`/tmp/pcw-x/a.log:3` —— 第一段为空或不在根下
#   代价说清楚：**某个根下目录整个被删掉的那天，指向它的引用会从「越界」静默降级成「不检查」**。
#   这是刻意换来的——宁可这里漏一次，也不要让它开始对别的仓库、对半截路径喊。
#
# 扫描范围（可读可改，别塞特例）：docs/ 下的 *.md + 仓库根的 *.md。
set -e
ROOT=${1:-$(cd "$(dirname "$0")/.." && pwd)}
[ -d "$ROOT" ] || { echo "用法: sh doc-ref-check.sh [仓库根]（给的路径不是目录：$ROOT）"; exit 2; }
cd "$ROOT"

SCAN_DIRS="docs"          # 递归扫这些目录下的 *.md
SCAN_ROOT_GLOB="*.md"     # 外加仓库根的 *.md（不递归）

# 名单先落到文件，计数再从这份名单里数出来 —— 数和名单结构上不可能对不上。
# （报「N 条」却另起一路去数，就是把名单和计数分家；分了家，错的通常是名单。）
OUT=$(mktemp)
trap 'rm -f "$OUT"' EXIT

# shellcheck disable=SC2086
FILES=$(find $SCAN_DIRS -type f -name '*.md' 2>/dev/null; ls $SCAN_ROOT_GLOB 2>/dev/null)

for doc in $FILES; do
  # 每个候选带上它在文档里的行号：grep -n 出行号，grep -o 出候选，用 awk 拼回一起。
  grep -nE '[A-Za-z0-9_./+-]+:[0-9]+' "$doc" 2>/dev/null | \
  awk -F: '{ ln=$1; sub(/^[0-9]+:/, "", $0); print ln "\t" $0 }' | \
  while IFS="$(printf '\t')" read -r ln text; do
    printf '%s\n' "$text" | grep -oE '[A-Za-z0-9_./+-]+\.[A-Za-z0-9]+:[0-9]+(-[0-9]+)?' | \
    while read -r ref; do
      path=${ref%:*}
      nums=${ref##*:}
      first=${path%%/*}
      # 门：第一段必须是仓库根下真实存在的条目（见顶部「什么才算一条引用」）
      [ -n "$first" ] || continue
      [ -e "./$first" ] || continue
      case "$path" in */.../*|.../*|*/...) continue;; esac   # `Tests/.../X.swift` 这种省略号写法不是路径
      if [ ! -f "$path" ]; then
        printf '%s:%s → %s（文件不存在）\n' "$doc" "$ln" "$ref" >> "$OUT"
        continue
      fi
      actual=$(wc -l < "$path" | tr -d ' ')
      # `12-30` 取区间上界；单个数就是它自己。上界不越界 ⇒ 下界也不会越界。
      hi=${nums##*-}
      if [ "$hi" -gt "$actual" ]; then
        printf '%s:%s → %s（实际 %s 行）\n' "$doc" "$ln" "$ref" "$actual" >> "$OUT"
      fi
    done
  done
done

if [ -s "$OUT" ]; then
  echo "--- 腐烂的文档引用（逐条）---"
  sort "$OUT"
  printf '共 %s 条\n' "$(wc -l < "$OUT" | tr -d ' ')"
  exit 1
fi
echo "--- 腐烂的文档引用：0 条 ---"
exit 0
