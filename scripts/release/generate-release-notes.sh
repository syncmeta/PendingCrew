#!/bin/sh
# 用法: generate-release-notes.sh <version>
# 从上一个 v* tag 到 main 的提交标题生成一页 HTML，
# 输出 dist/updates/<product>/<AppName>-<version>.html —— generate_appcast
# 靠「与 zip 同名的 .html」约定把它接成完整 HTML 文档的 sparkle:releaseNotesLink
# （URL 相对 SUFeedURL 解析，发布脚本会把它跟 zip/appcast.xml 平铺上传到同一
# 目录），链接本身受 feed 签名保护，不是直接嵌进 XML。
# 该文件已存在（手写）则原样保留，不覆盖。
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

prev=$(git -C "$root" tag --list "v*" --sort=-creatordate | head -n 1)
escape() { sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }

{
  echo "<!doctype html><html><head><meta charset=\"utf-8\"><title>$app_name $version</title>"
  echo "<style>body{font:13px -apple-system,sans-serif;margin:1.2em}li{margin:.25em 0}</style></head><body>"
  echo "<h2>$app_name $version</h2><ul>"
  if [ -n "$prev" ]; then
    git -C "$root" log "$prev..main" --format=%s | escape | sed 's/^/<li>/'
  else
    echo "<li>（首个发布，仅列最近 30 条）</li>"
    git -C "$root" log main -n 30 --format=%s | escape | sed 's/^/<li>/'
  fi
  echo "</ul></body></html>"
} > "$out"
echo "生成更新说明：$out"
