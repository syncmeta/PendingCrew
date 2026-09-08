#!/bin/sh
# 跑 macOS 全量测试，并把 **xcresult 与完整日志归档到不会被轮转的位置**。
#
# 为什么有这个脚本（2026-09-07）：一次接线后的全量跑出 `2 failures`，跑的人只
# grep 了汇总行、没留全日志，等回头去找时 Xcode 已经把 DerivedData 里的 xcresult
# 轮转掉了 —— **失败用例名永远拿不到了**。之后同一份代码连跑 4 趟全绿。
#
# 那条红的状态因此只能写成：**一次未复现的红，不是「飘」，也不是「已修」。**
# 「飘」是一个已经观察到规律的结论；那条只有一个样本，而且样本内容丢了。
#
# 所以这里落的是机制不是规矩：
#   **「下次记得留日志」是一条不会触发的规矩；「日志自动留下来」是一个机制。**
#
# 用法:
#   sh scripts/test-mac.sh [仓库根]
# 环境变量:
#   TEST_ARCHIVE  归档根目录（默认 <仓库根>/.test-archive，已在 .gitignore 里）
#   KEEP          保留最近几趟（默认 10）
#
# 退出码 = xcodebuild 的退出码，所以它可以直接当闸用。
set -e

ROOT=${1:-$(cd "$(dirname "$0")/.." && pwd)}
[ -d "${ROOT}" ] || { echo "用法: sh scripts/test-mac.sh [仓库根]（给的路径不是目录：${ROOT}）"; exit 2; }
cd "${ROOT}"

ARCHIVE=${TEST_ARCHIVE:-${ROOT}/.test-archive}
KEEP=${KEEP:-10}
DD=${ARCHIVE}/dd
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
SHA=$(git -C "${ROOT}" rev-parse --short HEAD 2>/dev/null || echo nogit)
DIRTY=$(git -C "${ROOT}" status --porcelain 2>/dev/null | head -1)
[ -n "${DIRTY}" ] && SHA="${SHA}-dirty"
NAME="${STAMP}-${SHA}"
mkdir -p "${ARCHIVE}"

LOG="${ARCHIVE}/${NAME}.log"
echo "跑全量 → ${LOG}"

# **先单独 build 一次再 test。** 冷 derivedDataPath 上直接 `test` 会挂在
#   `AgentCLIMaintenanceTests.swift:3: unable to resolve module dependency: 'PendingCrew'`
# —— 有 5 个测试文件用 `@testable import PendingCrew`，而测试 target 的
# `dependencies:` 里**只有 package、没有 `- target: PendingCrew`**，于是模块要靠
# 「app 恰好已经在这份 derivedData 里编好了」才解析得到。暖的 derivedData 上看不见，
# 冷的第一趟必挂。
#
# 这里 build 一次是**兜住它**，不是修它 —— 根因在 project.yml 的测试 target 配置，
# 归那条线修。写在这儿是为了：下一个看到这行的人知道它为什么在。
rc=0
xcodebuild -project "${ROOT}/PendingCrew.xcodeproj" -scheme PendingCrew \
  -destination 'platform=macOS' -derivedDataPath "${DD}" build \
  > "${LOG}" 2>&1 || rc=$?
if [ "${rc}" -ne 0 ]; then
  echo "构建就没过，测试没跑。看 ${LOG}"
  grep -E "error:" "${LOG}" | head -5
  exit "${rc}"
fi

xcodebuild -project "${ROOT}/PendingCrew.xcodeproj" -scheme PendingCrew \
  -destination 'platform=macOS' -derivedDataPath "${DD}" test \
  >> "${LOG}" 2>&1 || rc=$?

# 归档 xcresult。**必须在剪枝之前做**，否则这一趟自己可能先被剪掉。
newest=$(ls -td "${DD}"/Logs/Test/*.xcresult 2>/dev/null | head -1)
if [ -n "${newest}" ]; then
  cp -R "${newest}" "${ARCHIVE}/${NAME}.xcresult"
  echo "xcresult 已归档 → ${ARCHIVE}/${NAME}.xcresult"
else
  echo "⚠️ 这一趟没找到 xcresult —— 归档是空的，别把它当成「查得到」"
fi

# 只剪 **归档目录里** 的历史，不碰 DerivedData 自己的轮转。
for ext in log xcresult; do
  ls -td "${ARCHIVE}"/*."${ext}" 2>/dev/null | tail -n "+$((KEEP + 1))" | while read -r old; do
    rm -rf "${old}"
  done
done

echo "--- 汇总 ---"
grep -E "Executed [0-9]+ tests, with|TEST SUCCEEDED|TEST FAILED" "${LOG}" | tail -3
echo "--- 具名失败（空 = 零条）---"
grep -E "' failed \(" "${LOG}" || true
# `-d` 不能少：xcresult 是**目录**，没有 -d 的 ls 会去列它里面的东西，
# 数出来的是内容条数不是趟数（第一版就这么多报了一趟）。
echo "--- 归档里现有的趟数 ---"
ls -1d "${ARCHIVE}"/*.xcresult 2>/dev/null | wc -l | tr -d ' '

exit "${rc}"
