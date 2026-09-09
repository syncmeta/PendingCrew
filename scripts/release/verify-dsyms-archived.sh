#!/bin/sh
# 用法: verify-dsyms-archived.sh <App.app 路径> <可执行文件名> <符号归档目录>
#
# 断言：这次发出去的那个二进制，**每一个架构切片**的 UUID 都能在符号归档目录里
# 找到对应的 dSYM。找不到就退 2。
#
# 为什么需要这道门（2026-09-09 的血，人类 Todo #138）：
#   0.1.28 在用户机器上闪退，崩溃报告里我们自己那六帧全是裸地址。查符号表时发现
#   **三个地方都没有**：`~/Library/Developer/Xcode/Archives` 根本不存在（发版不走
#   Xcode Organizer）；DerivedData 里那份 dSYM 是另一次构建的，UUID 对不上；而发版
#   脚本的快照目录是 `mktemp -d /tmp/…`，跑完就被 trap 回收了 —— **每次发版都生成过
#   一份和线上逐字对得上的 dSYM，然后把它扔了。**
#   那一次只能靠「同 commit 同配置重编」这条近似路救回来（这次侥幸对上了：代码段
#   大小逐字节相同、24366 个函数入口只差一个常数）。但那是运气，不是流程 ——
#   依赖版本只要有一个动过，重编出来的布局就不再对得上，而且**它不会报错，只会
#   给出看起来很像真的错行号**。
#
# 这道门查的是「归档里那份是不是**这一次**的」，不是「有没有一个叫 dSYM 的东西」。
# 后者是今天真正咬人的形态：DerivedData 里那份确实存在、名字也对，就是不是它。
set -eu

app=${1:?usage: verify-dsyms-archived.sh <App.app> <exe-name> <symbols-dir>}
exe_name=${2:?usage: verify-dsyms-archived.sh <App.app> <exe-name> <symbols-dir>}
dir=${3:?usage: verify-dsyms-archived.sh <App.app> <exe-name> <symbols-dir>}

exe="$app/Contents/MacOS/$exe_name"
test -f "$exe" || { echo "找不到可执行文件: $exe" >&2; exit 2; }
test -d "$dir" || {
  echo "符号归档目录不存在: $dir" >&2
  echo "  发版链路没有把 dSYM 留下来 —— 这一版将来崩了只能靠重编近似。" >&2
  exit 2
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

# `dwarfdump --uuid` 每个切片一行："UUID: <uuid> (<arch>) <path>"。
# 一行一行读、不用 `for x in $var` —— 那个写法在 zsh 下不做词分裂，会让整道断言
# 退化成假阳性（同一个坑 verify-rpath-resolvable.sh 里踩过一次）。
dwarfdump --uuid "$exe" | awk '/^UUID: /{print $2 " " $3}' | sort -u > "$tmp/shipped"
[ -s "$tmp/shipped" ] || { echo "读不出 $exe 的 UUID —— dwarfdump 没有输出" >&2; exit 2; }

: > "$tmp/archived"
find "$dir" -maxdepth 2 -name '*.dSYM' -print | sort | while IFS= read -r d; do
  [ -n "$d" ] || continue
  dwarfdump --uuid "$d" 2>/dev/null | awk '/^UUID: /{print $2}'
done | sort -u > "$tmp/archived"

# 缺的写进文件、不塞进一个变量：`for m in $missing` 依赖词分裂，zsh 下不分裂，
# 同一个坑 verify-rpath-resolvable.sh 的注释里已经记过一次。
: > "$tmp/missing"
while IFS= read -r line; do
  [ -n "$line" ] || continue
  uuid=${line%% *}
  arch=${line#* }
  grep -qx "$uuid" "$tmp/archived" || printf '%s %s\n' "$arch" "$uuid" >> "$tmp/missing"
done < "$tmp/shipped"

if [ -s "$tmp/missing" ]; then
  echo "符号归档对不上这次的产物 —— 缺这几个切片的 dSYM：" >&2
  sed 's/^/  /' "$tmp/missing" >&2
  echo "" >&2
  echo "产物里的 UUID：" >&2
  sed 's/^/  /' "$tmp/shipped" >&2
  echo "归档目录 $dir 里的 UUID（$(wc -l < "$tmp/archived" | tr -d ' ') 个）：" >&2
  if [ -s "$tmp/archived" ]; then sed 's/^/  /' "$tmp/archived" >&2; else echo "  （一个都没有）" >&2; fi
  exit 2
fi

# `${dir}` 的花括号不是装饰：macOS /bin/sh 的多字节变量名解析会把紧邻的中文标点
# 吞进变量名，`set -u` 下就成了「unbound variable」。写这一行时当场踩了一次，
# 而它踩的正是本仓库 build-macos-update.sh 里已经写过一遍的同一个坑。
echo "note: dSYM 归档已核对 —— 产物 $(wc -l < "$tmp/shipped" | tr -d ' ') 个切片全部对得上（${dir}）"
