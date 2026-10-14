#!/bin/sh
# 把「这个产物是从哪个 commit 出的」写进产物 Info.plist。
# postBuildScripts 调 "$SRCROOT/Shared/scripts/stamp-build-info.sh"。
# 跑在 code signing 之前，改 Info.plist 不破签名。
# git 查不到就一个键都不写 —— 运行时 AppBuildStamp 如实报「无版本戳」。
set -eu

PLIST="${BUILT_PRODUCTS_DIR}/${INFOPLIST_PATH}"
if [ ! -f "$PLIST" ]; then
    if [ "${CONFIGURATION:-}" = "NativeCandidate" ]; then
        echo "error: NativeCandidate product Info.plist is missing"
        exit 1
    fi
	echo "warning: stamp-build-info: 找不到产物 Info.plist（${PLIST}），跳过打戳"
	exit 0
fi

if [ "${CONFIGURATION:-}" = "NativeCandidate" ]; then
    COMMIT=${PENDINGCREW_CANDIDATE_SOURCE_REF:-}
    case "$COMMIT" in
        *[!0-9a-f]*|"") echo "error: NativeCandidate requires a reviewed full source ref"; exit 1 ;;
    esac
    if [ "${#COMMIT}" -ne 40 ]; then
        echo "error: NativeCandidate source ref must be 40 lower-case hex characters"
        exit 1
    fi
    /usr/libexec/PlistBuddy -c "Delete :PendingCrewSourceRef" "$PLIST" 2>/dev/null || true
    /usr/libexec/PlistBuddy -c "Delete :PendingCrewNativeCandidate" "$PLIST" 2>/dev/null || true
    /usr/libexec/PlistBuddy -c "Add :PendingCrewSourceRef string $COMMIT" "$PLIST"
    /usr/libexec/PlistBuddy -c "Add :PendingCrewNativeCandidate bool true" "$PLIST"
elif ! COMMIT=$(git -C "$SRCROOT" rev-parse HEAD 2>/dev/null); then
	echo "warning: stamp-build-info: $SRCROOT 不是 git 仓库（或没装 git），本次构建不带版本戳"
	exit 0
fi
DATE=$(date -u +%Y-%m-%dT%H:%M:%SZ)

/usr/libexec/PlistBuddy -c "Delete :BuildStampCommit" "$PLIST" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Delete :BuildStampDate" "$PLIST" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :BuildStampCommit string $COMMIT" "$PLIST"
/usr/libexec/PlistBuddy -c "Add :BuildStampDate string $DATE" "$PLIST"
echo "note: stamp-build-info: ${COMMIT} → $(basename "$PLIST")"
