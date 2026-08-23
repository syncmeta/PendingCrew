#!/bin/sh
# 用法: generate-release-notes.sh <version>
# 从 CHANGELOG.md 里 `## <version>` 那一段生成一页 HTML，
# 输出 dist/updates/<product>/<AppName>-<version>.html —— generate_appcast
# 靠「与 zip 同名的 .html」约定把它接成完整 HTML 文档的 sparkle:releaseNotesLink
# （URL 相对 SUFeedURL 解析，发布脚本会把它跟 zip/appcast.xml 平铺上传到同一
# 目录），链接本身受 feed 签名保护，不是直接嵌进 XML。
# 该文件已存在（手写）则原样保留，不覆盖。
#
# 文案取自 CHANGELOG.md，不再拿提交标题充数 —— 用户在更新弹窗里读到的应该是
# 「这版对你有什么不同」，不是「docs: README 按作者手改的版本重写」。
# 那一段取不到，changelog-section.sh 会直接非零退出，这里跟着挂掉，不静默兜底。
set -eu

version=${1:?usage: generate-release-notes.sh <version>}
product=pendingcrew
app_name=PendingCrew

root=${PENDING_RELEASE_ROOT:-$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)}
release_dir="$root/dist/updates/$product"
out="$release_dir/$app_name-$version.html"
mkdir -p "$release_dir"

if [ -f "$out" ]; then
  echo "手写更新说明已存在，保留：$out"
  exit 0
fi

# CHANGELOG 跟脚本一起躺在仓库里，位置由 changelog-section.sh 自己解析 ——
# 不跟着 PENDING_RELEASE_ROOT 走，那个变量指的是产物输出的根。
section=$("$(dirname -- "$0")/changelog-section.sh" "$version")

escape() { sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }

# 先整段转义，再套标签 —— 标签是转义之后加的，不会被自己转义掉。
# Markdown 只认两种：`- ` 开头的列表项，和普通段落。空行断开列表。
body=$(printf '%s\n' "$section" | escape | awk '
  /^[ \t]*$/ { if (inlist) { print "</ul>"; inlist = 0 } ; next }
  /^- / {
    if (!inlist) { print "<ul>"; inlist = 1 }
    line = $0; sub(/^- /, "", line)
    print "<li>" line "</li>"
    next
  }
  {
    if (inlist) { print "</ul>"; inlist = 0 }
    print "<p>" $0 "</p>"
  }
  END { if (inlist) print "</ul>" }
')

{
  echo "<!doctype html><html><head><meta charset=\"utf-8\"><title>$app_name $version</title>"
  echo "<style>body{font:13px -apple-system,sans-serif;margin:1.2em}li{margin:.25em 0}</style></head><body>"
  echo "<h2>$app_name $version</h2>"
  printf '%s\n' "$body"
  echo "</body></html>"
} > "$out"
echo "生成更新说明：$out"
