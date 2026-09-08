#!/bin/sh
# `$名字` 后面紧跟一个非 ASCII 字符 —— 在 UTF-8 环境下 bash/sh 会把那个字符的
# **首字节吞进变量名**（`$dpid` 后面紧跟一个中文逗号，就解析成变量 `dpid?`），配 `set -u` 当场炸成
# `unbound variable`，而且报的行号看着跟这件事毫无关系。
#
# 为什么它值得一道常设的尺子：
# - **只在中文环境下炸**。`LC_ALL=C` 或 LANG 没设时同一行完全正常，所以它躲得过
#   本地随手一跑，专挑用户的终端发作。
# - 我们的脚本里大量给人看的中文提示，正是它的高发地带。2026-09-07 一天里
#   `scripts/install.sh:83`（`$dest` 后面紧跟省略号）和一个安装器（`$dpid` 后面紧跟
#   中文逗号）各中一次，
#   后者直接让人类的安装在第一步就停住。
#
# **判据从危险本身推，不是列几个中文标点**：后面那个字节 >= 0x80 就算。
# 曾经用「常见中文标点白名单」自查过一次，报「全仓 0 命中」，而当时活着的那处
# 用的是 `…` —— 白名单里没有。字符集是尺子自己的盲区，会被自查原样继承。
#
# ⚠️ 上面那两个反面例子**故意不写成真的那个形状**（变量名后面紧挨着中文标点），
# 而是把标点挪出反引号、用文字说出来。原因：**这个文件自己也在被扫的名单里**，
# 反面例子长成被检测的形状，尺子就会咬自己 —— 2026-09-08 真发生过一次，
# 让 main 上的 `ReleaseScriptSourceContractTests` 红了 3 处。
# 别为了「写得更直观」改回去。
#
# 顺带记下它当初怎么躲过我的：加这道尺子那一趟我跑过它、报「命中 0 处」——
# 因为扫的是 `git ls-files`，**而这个文件那时还没提交，尺子扫不到自己**。
# 新加一道尺子时，先 `git add` 再让它扫一遍自己。
#
# 用法：sh scripts/shell-var-brace-check.sh [根目录]   （默认当前目录）
# 输出即名单，退出码非零表示有命中。
set -eu
root=${1:-.}

cd "$root"
files=$(git ls-files '*.sh' 2>/dev/null || find . -name '*.sh' -type f)
[ -n "$files" ] || { echo "没有 .sh 文件可扫"; exit 0; }

printf '%s\n' "$files" | /usr/bin/python3 -c '
import re, sys
pat = re.compile(rb"\$[A-Za-z_][A-Za-z0-9_]*[\x80-\xff]")
paths = [p for p in sys.stdin.read().split("\n") if p]
bad = 0
for path in paths:
    try:
        data = open(path, "rb").read()
    except OSError as e:
        print(f"{path}: 读不出来（{e}）—— 这是「没扫到」，不是「干净」"); bad += 1; continue
    for i, line in enumerate(data.split(b"\n"), 1):
        for m in pat.finditer(line):
            bad += 1
            hit = m.group().decode("utf-8", "replace")
            print(f"{path}:{i}: {hit}   ← 改成 ${{...}}")
print(f"扫了 {len(paths)} 个文件，命中 {bad} 处")
sys.exit(1 if bad else 0)
'
