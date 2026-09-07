#!/bin/sh
# 文档引用腐烂检测器：只判「一定烂了」的那半，不判「讲得对不对」。
#
#   用法： sh scripts/doc-ref-check.sh [仓库根]        # 默认 = 本脚本的上一级
#   退出： 0 = 没有违规；1 = 有违规（名单在 stdout）；2 = 用法错
#
# 判据只有两条，都不需要读懂那一行写了什么：
#   ① 行号越界： 引用 path:N，而 path 只有 M 行、N > M
#   ② 文件不存在：引用 path:N，而 path 不在那棵树里
# **刻意不做**「这一行是不是真的讲那件事」——那要语义，尺子一有语义就开始误报，
# 然后被人关掉。零误报是这把尺子唯一的卖点，宁可漏也不许错。
#
# 什么才算「一条引用」（这条界定就是零误报的全部所在）：
#   形如 `<路径>:<数字>`，且**路径的第一段是那棵树根下真实存在的条目**。
#   这一条同时把下面这些挡在门外，不需要为它们各写一条特例：
#     · 裸文件名 `AgentTerminalSession.swift:12`  —— 第一段不在根下 ⇒ 不是引用（那批的清理另有其事，不归这把尺子）
#     · 别的仓库  `apps/edge/src/routes/crew.ts:88` —— 本仓库根下没有 apps/ ⇒ 无从判断，不碰
#     · 半截路径  `Mac/Views/CrewDetailInspector.swift:20` —— 同上（它省掉了 Sources/）
#     · URL / 绝对路径 `http://127.0.0.1:10858`、`/tmp/pcw-x/a.log:3` —— 第一段为空或不在根下
#   代价说清楚：**某个根下目录整个被删掉的那天，指向它的引用会从「越界」静默降级成「不检查」**。
#   这是刻意换来的——宁可这里漏一次，也不要让它开始对别的仓库、对半截路径喊。
#
# ── 「哪棵树」：文档可以声明自己的基准提交 ────────────────────────────────
# 快照式文档（一次性清点、事后报告）描述的是**过去某一刻**的树。把它的行号改成
# 今天的树，等于把它改成假的——它就不再描述那一刻了。所以这类文档在头部声明：
#
#     <!-- doc-ref-base: 24a7893 -->
#
# 声明了就对 `git show <base>:<path>` 数行、`git cat-file -e <base>:<path>` 判在不在；
# 没声明就照旧对当前树。**判据一个字没变**（仍是越界 / 不存在），换的只是那棵树。
#
# **base 该填哪个提交**：先看文档自己有没有写（很多快照文档正文里就写了
# 「基线：main@xxxxxxx」「用同一 commit xxxxxxx 本地重编做符号化」——那比任何
# 启发式都硬）。没写就取**创建提交**（`git log --follow --format=%H -- <doc> | tail -1`）。
# **不要取「文档末次改动」**——下一个人一定会想到它，因为它听起来更新、更合理，
# 而且一行命令就能拿到。但它错得很安静：那次改动往往只是一次**无关的移动**
# （2f9edda 把 docs 挪进 docs/internal/），拿它当基准会把没烂的判成真可疑。
# 实测：用末次改动分组时「两棵树上都不成立」有 4 条，换成创建提交后归零。
# 根子跟 `git log` 要加 `--follow` 是同一个：**git 历史里混着「内容变了」和
# 「位置变了」，我们只要前者**。同一个坑这轮踩了两次。
#
# base 本身必须被验证，不能只被记录——否则它就退化成一行注释写的白名单，
# 任何人写个假 sha 就能豁免整份文档，而尺子一声不吭。两道，各自都会红：
#     · sha 解析不出提交              ⇒ 红
#     · 该提交不是 main 的祖先        ⇒ 红（包括 main 这个 ref 根本解析不出来的情况）
# 这两道挡得住**假**的 base，挡不住**很旧**的 base。实测过这一格，别靠猜：
# 把那份 41 条的文档的 base 换成仓库首个提交（真实、是祖先、但过早），
# **41 条里只有 5 条报红**。也就是说「声明一个老 base」躲得过大部分判据。
# 那一格唯一的照明是下面那份「落后 main 多少」的读数 —— 它**不是**「够用但保守」，
# 它是仅有的光。谁要是把那份读数删掉或做成有阈值的判据，这一格就全黑了。
# 注意 base 说的是「**被引用的那棵树**」，不是「这份文档自己的历史」——
# 文档后来被移动过、被补过一行，都不改 base。
#
# 扫描范围（可读可改，别塞特例）：docs/ 下的 *.md + 仓库根的 *.md。
set -e
ROOT=${1:-$(cd "$(dirname "$0")/.." && pwd)}
[ -d "$ROOT" ] || { echo "用法: sh doc-ref-check.sh [仓库根]（给的路径不是目录：${ROOT}）"; exit 2; }
cd "$ROOT"

SCAN_DIRS="docs"          # 递归扫这些目录下的 *.md
SCAN_ROOT_GLOB="*.md"     # 外加仓库根的 *.md（不递归）

# 名单先落到文件，计数再从这份名单里数出来 —— 数和名单结构上不可能对不上。
# （报「N 条」却另起一路去数，就是把名单和计数分家；分了家，错的通常是名单。）
OUT=$(mktemp)
# 第二本：声明了 base 的文档各一行「落后 main 多少」。**它是读数，不是判据** ——
# （实测：过早的 base 只点红 5/41，所以这份读数是「老 base 躲尺子」那一格仅有的光，
#  不是锦上添花。别给它加阈值，加了就又变成一个会误报、然后被人关掉的判据。）
# 不设阈值、不因此返回非零。判「这份文档该不该是快照」要语义，尺子一有语义就开始误报。
# 摆出来是因为护栏只挡得住**假的** base，挡不住「声明一个老 base 躲开尺子」：
# 快照文档落后是正常的、应该的；一份新文档声明了很旧的 base，这个数自己会刺眼。
BASEOUT=$(mktemp)
trap 'rm -f "$OUT" "$BASEOUT"' EXIT

# 行数一律用 awk 的 NR，不用 `wc -l`：末行没有换行符时 `wc -l` 会少数一行，
# 而那正好是「引用文件最后一行」时会误报的那一格。
count_lines_worktree() { awk 'END{print NR+0}' "$1"; }
count_lines_base()     { git show "$2:$1" | awk 'END{print NR+0}'; }

# shellcheck disable=SC2086
FILES=$(find $SCAN_DIRS -type f -name '*.md' 2>/dev/null; ls $SCAN_ROOT_GLOB 2>/dev/null)

for doc in $FILES; do
  # ① 这份文档声明基准提交了吗
  base=$(sed -n 's/.*<!-- *doc-ref-base: *\([0-9a-fA-F]\{7,40\}\) *-->.*/\1/p' "$doc" | head -1)
  base_ln=$(grep -n 'doc-ref-base:' "$doc" | head -1 | cut -d: -f1)
  if [ -n "$base" ]; then
    # ② base 自己先过两道，过不了就整份文档判红并跳过——红在 base 那一行，
    #    而不是把它的引用拿去跟错的树比、报出一堆看不懂的红。
    full=$(git rev-parse --verify --quiet "$base^{commit}" 2>/dev/null || true)
    if [ -z "$full" ]; then
      printf '%s:%s → doc-ref-base %s（解析不出这个提交）\n' "$doc" "$base_ln" "$base" >> "$OUT"
      continue
    fi
    if ! git merge-base --is-ancestor "$full" main 2>/dev/null; then
      printf '%s:%s → doc-ref-base %s（不是 main 的祖先，或这里根本没有 main）\n' "$doc" "$base_ln" "$base" >> "$OUT"
      continue
    fi
    printf '%s  base %s  落后 main %s 个提交 / %s 天\n' "$doc" "$(git rev-parse --short "$full")" \
      "$(git rev-list --count "$full..main" 2>/dev/null || echo '?')" \
      "$(( ( $(date +%s) - $(git log -1 --format=%ct "$full") ) / 86400 ))" >> "$BASEOUT"
  fi

  # 每个候选带上它在文档里的行号：grep -n 出行号，grep -o 出候选，用 awk 拼回一起。
  grep -nE '[A-Za-z0-9_./+-]+:[0-9]+' "$doc" 2>/dev/null | \
  awk -F: '{ ln=$1; sub(/^[0-9]+:/, "", $0); print ln "\t" $0 }' | \
  while IFS="$(printf '\t')" read -r ln text; do
    printf '%s\n' "$text" | grep -oE '[A-Za-z0-9_./+-]+\.[A-Za-z0-9]+:[0-9]+(-[0-9]+)?' | \
    while read -r ref; do
      path=${ref%:*}
      nums=${ref##*:}
      first=${path%%/*}
      # 门：第一段必须是那棵树根下真实存在的条目（见顶部「什么才算一条引用」）
      [ -n "$first" ] || continue
      case "$path" in */.../*|.../*|*/...) continue;; esac   # `Tests/.../X.swift` 这种省略号写法不是路径
      if [ -n "$base" ]; then
        git cat-file -e "$base:$first" 2>/dev/null || continue
        if ! git cat-file -e "$base:$path" 2>/dev/null; then
          printf '%s:%s → %s（基准 %s 上文件不存在）\n' "$doc" "$ln" "$ref" "$base" >> "$OUT"
          continue
        fi
        actual=$(count_lines_base "$path" "$base")
        label="基准 $base 上实际 $actual 行"
      else
        [ -e "./$first" ] || continue
        if [ ! -f "$path" ]; then
          printf '%s:%s → %s（文件不存在）\n' "$doc" "$ln" "$ref" >> "$OUT"
          continue
        fi
        actual=$(count_lines_worktree "$path")
        label="实际 $actual 行"
      fi
      # `12-30` 取区间上界；单个数就是它自己。上界不越界 ⇒ 下界也不会越界。
      hi=${nums##*-}
      if [ "$hi" -gt "$actual" ]; then
        printf '%s:%s → %s（%s）\n' "$doc" "$ln" "$ref" "$label" >> "$OUT"
      fi
    done
  done
done

if [ -s "$BASEOUT" ]; then
  echo "--- 声明了基准提交的文档（读数，不是判据；不设阈值、不影响退出码）---"
  sort "$BASEOUT"
  echo "  快照文档落后是正常的、应该的；一份新文档声明了很旧的 base，那个数自己会刺眼。"
fi

if [ -s "$OUT" ]; then
  echo "--- 腐烂的文档引用（逐条）---"
  sort "$OUT"
  printf '共 %s 条\n' "$(awk 'END{print NR+0}' "$OUT")"
  exit 1
fi
echo "--- 腐烂的文档引用：0 条 ---"
exit 0
