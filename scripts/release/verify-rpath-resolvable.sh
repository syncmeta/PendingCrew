#!/bin/sh
# 用法: verify-rpath-resolvable.sh <App.app 路径> <可执行文件名>
#
# 断言：主可执行文件的每个 @rpath 依赖，都能沿它自己的 LC_RPATH 在包内解析出
# 真实文件。解析不到就退 2 并打印诊断。
#
# 为什么需要这道门（2026-08-06 的血）：XcodeGen 对 supportedDestinations 多端
# target 只生成 iOS 约定的 rpath（@executable_path/Frameworks），而 macOS 的嵌入式
# 框架在 Contents/Frameworks。结果是**编译、链接、签名、公证、Gatekeeper、stapler
# 六道全绿**，用户一双击就 dyld "Library not loaded" SIGABRT —— 发布脚本原有的
# 五道断言一道都拦不住，因为它们查的都是签名和 entitlement，没人查 dyld 能不能
# 把库找着。
#
# 只查主可执行文件：这是踩过的那一类。嵌入框架自身的依赖不在范围内。
set -eu

app=${1:?usage: verify-rpath-resolvable.sh <App.app> <exe-name>}
exe_name=${2:?usage: verify-rpath-resolvable.sh <App.app> <exe-name>}
exe="$app/Contents/MacOS/$exe_name"
test -f "$exe" || { echo "找不到可执行文件: $exe" >&2; exit 2; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

# 逐行读文件，不用 `for x in $var` —— 那个写法依赖词分裂，在 zsh 下不分裂，
# 会让整道断言退化成"永远全都解析不到"的假阳性。这个坑现场踩过一次。
otool -l "$exe" | awk '/LC_RPATH/{f=1} f&&/^ *path /{print $2; f=0}' | sort -u > "$tmp/rpaths"
otool -L "$exe" | awk '/^\t@rpath\//{print $1}' | sort -u > "$tmp/deps"

missing=''
while IFS= read -r dep; do
  [ -n "$dep" ] || continue
  suffix=${dep#@rpath/}
  found=''
  while IFS= read -r rp; do
    [ -n "$rp" ] || continue
    # dyld 对主可执行文件而言 @loader_path 等同 @executable_path
    resolved=$(printf '%s' "$rp" | sed -e "s|@executable_path|$app/Contents/MacOS|" \
                                       -e "s|@loader_path|$app/Contents/MacOS|")
    if [ -e "$resolved/$suffix" ]; then found=1; break; fi
  done < "$tmp/rpaths"
  [ -n "$found" ] || missing="$missing  $dep
"
done < "$tmp/deps"

if [ -n "$missing" ]; then
  echo "以下 @rpath 依赖在包内解析不到 —— app 一启动就会 dyld 崩溃：" >&2
  printf '%s' "$missing" >&2
  echo "可执行文件的 LC_RPATH：" >&2
  sed 's/^/  /' "$tmp/rpaths" >&2
  echo "（macOS 的嵌入式框架在 Contents/Frameworks，rpath 需要 @executable_path/../Frameworks；" >&2
  echo "  在 project.yml 里设 \"LD_RUNPATH_SEARCH_PATHS[sdk=macosx*]\"）" >&2
  exit 2
fi
