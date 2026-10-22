#!/bin/sh
# 把「这个产物是从哪个 commit 出的」写进产物 Info.plist。
# postBuildScripts 调 "$SRCROOT/Shared/scripts/stamp-build-info.sh"。
# 跑在 code signing 之前，改 Info.plist 不破签名。
# git 查不到就一个键都不写 —— 运行时 AppBuildStamp 如实报「无版本戳」。
set -eu

PLIST="${BUILT_PRODUCTS_DIR}/${INFOPLIST_PATH}"
if [ ! -f "$PLIST" ]; then
    if [ "${CONFIGURATION:-}" = "NativeCandidate" ] || [ "${CONFIGURATION:-}" = "DeliveryValidation" ] || [ "${CONFIGURATION:-}" = "KnowledgeValidation" ]; then
        echo "error: NativeCandidate product Info.plist is missing"
        exit 1
    fi
	echo "warning: stamp-build-info: 找不到产物 Info.plist（${PLIST}），跳过打戳"
	exit 0
fi

if [ "${CONFIGURATION:-}" = "NativeCandidate" ] || [ "${CONFIGURATION:-}" = "DeliveryValidation" ] || [ "${CONFIGURATION:-}" = "KnowledgeValidation" ]; then
    COMMIT=${PENDINGCREW_CANDIDATE_SOURCE_REF:-}
    if [ "${CONFIGURATION:-}" = "DeliveryValidation" ] || [ "${CONFIGURATION:-}" = "KnowledgeValidation" ]; then COMMIT=${PENDINGCREW_VALIDATION_SOURCE_REF:-}; fi
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
    /usr/libexec/PlistBuddy -c "Delete :PendingCrewKnowledgeValidation" "$PLIST" 2>/dev/null || true
    if [ "${CONFIGURATION:-}" = "KnowledgeValidation" ]; then
        /usr/libexec/PlistBuddy -c "Add :PendingCrewKnowledgeValidation bool true" "$PLIST"
    fi
    if [ "${CONFIGURATION:-}" = "DeliveryValidation" ] || [ "${CONFIGURATION:-}" = "KnowledgeValidation" ]; then
        /usr/libexec/PlistBuddy -c "Delete :PendingCrewDeliveryValidation" "$PLIST" 2>/dev/null || true
        /usr/libexec/PlistBuddy -c "Add :PendingCrewDeliveryValidation bool true" "$PLIST"
    else
        /usr/libexec/PlistBuddy -c "Add :PendingCrewNativeCandidate bool true" "$PLIST"
    fi
elif [ "${PENDINGCREW_PRODUCT_SOURCE_REF+x}" = x ]; then
    COMMIT=$PENDINGCREW_PRODUCT_SOURCE_REF
    case "$COMMIT" in
        *[!0-9a-f]*|"") echo "error: ordinary product requires a reviewed full source ref"; exit 1 ;;
    esac
    if [ "${#COMMIT}" -ne 40 ]; then
        echo "error: ordinary product source ref must be 40 lower-case hex characters"
        exit 1
    fi
else
    # An archive nested under another repository is not that repository's source.
    SOURCE_ROOT=$(cd "$SRCROOT" && pwd -P)
    GIT_ROOT=$(git -C "$SRCROOT" rev-parse --show-toplevel 2>/dev/null) || GIT_ROOT=
    if [ -z "$GIT_ROOT" ] || [ "$(cd "$GIT_ROOT" && pwd -P)" != "$SOURCE_ROOT" ] || ! COMMIT=$(git -C "$SRCROOT" rev-parse HEAD 2>/dev/null); then
        echo "warning: stamp-build-info: $SRCROOT 不是当前源码 git 根目录，本次构建不带版本戳"
        exit 0
    fi
fi
if [ "${PLATFORM_NAME:-}" = "macosx" ] && { [ "${CONFIGURATION:-}" = "Debug" ] || [ "${CONFIGURATION:-}" = "Release" ]; }; then
    /usr/libexec/PlistBuddy -c "Delete :PendingCrewSourceRef" "$PLIST" 2>/dev/null || true
    /usr/libexec/PlistBuddy -c "Add :PendingCrewSourceRef string $COMMIT" "$PLIST"
fi
DATE=$(date -u +%Y-%m-%dT%H:%M:%SZ)

/usr/libexec/PlistBuddy -c "Delete :BuildStampCommit" "$PLIST" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Delete :BuildStampDate" "$PLIST" 2>/dev/null || true
/usr/libexec/PlistBuddy -c "Add :BuildStampCommit string $COMMIT" "$PLIST"
/usr/libexec/PlistBuddy -c "Add :BuildStampDate string $DATE" "$PLIST"
echo "note: stamp-build-info: ${COMMIT} → $(basename "$PLIST")"
