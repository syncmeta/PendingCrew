#!/bin/sh
# 用法: scripts/release/build-ios-testflight.sh [--dry-run]
#
# 出一个能进 TestFlight 的 iOS 包：干净快照（钉 main HEAD）→ xcodegen →
# archive（generic/platform=iOS，自动签名 + ASC API key）→ exportArchive 出
# .ipa → 八道断言 →（默认）`xcrun altool --upload-package` 传 TestFlight。
#
#   --dry-run  走完 archive + export + 全部断言，**在上传那一步停下**，
#              把「将要上传什么、用哪条命令」原样打印出来。产物照样落盘。
#
# 与 `build-macos-update.sh` 的关系：那条是 Developer ID + 公证 + Sparkle
# 自动更新（自己分发）；这条是 App Store Connect（Apple 分发）。两条**共用
# 同一套 build 号编码**（纪元日「天.秒」），但各有各的防倒退闸 —— 那边比线上
# appcast，这边比 ASC 上已收到的最高 build。风格、断言习惯照抄那条，别另起一套。
#
# 前置见 docs/release-ios.md。这里只说一句最容易踩的：**ASC 凭据只用路径 + ID**，
# .p8 的内容从头到尾不会被这个脚本打印、复制或提交。
set -eu

app_name=PendingCrew
bundle_id=com.pendingname.pendingcrew

dry_run=0
for arg in "$@"; do
  case "$arg" in
    --dry-run) dry_run=1 ;;
    -h|--help)
      # 打印文件头整段注释（到第一行非注释为止），别写死行号 —— 写死过就会漂。
      awk 'NR>1 && !/^#/ { exit } NR>1 { sub(/^# ?/, ""); print }' "$0"
      exit 0
      ;;
    *) echo "未知参数: $arg（只认 --dry-run）" >&2; exit 2 ;;
  esac
done

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)

team_id="${PENDING_TEAM_ID:-M42BKJN82S}"
# ASC 上 PendingCrew 的数字 app id。**必须显式传给 altool**，理由见下面上传那一段。
asc_app_id="${PENDING_ASC_APP_ID:-6800257069}"

# --- 0. ASC 凭据（只读路径 + ID，绝不读内容）---------------------------------
# 优先环境变量，其次 ~/.appstoreconnect/pendingbot.env（600，KEY=value 一行一个）。
# 那个文件名带 "pendingbot" 是历史原因：**同一把 team key**（M42BKJN82S）既给
# PendingBot 传 TestFlight、也给 PendingCrew 公证 macOS 包，不是两把。
asc_env="${PENDING_ASC_ENV:-$HOME/.appstoreconnect/pendingbot.env}"
env_value() {
  [ -f "$asc_env" ] || return 0
  sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*//p" "$asc_env" | head -n 1 | tr -d "\"'"
}
asc_key_id="${ASC_KEY_ID:-$(env_value ASC_KEY_ID)}"
asc_issuer_id="${ASC_ISSUER_ID:-$(env_value ASC_ISSUER_ID)}"
asc_key_path="${ASC_KEY_PATH:-$(env_value ASC_KEY_PATH)}"

for pair in "ASC_KEY_ID=$asc_key_id" "ASC_ISSUER_ID=$asc_issuer_id" "ASC_KEY_PATH=$asc_key_path"; do
  case "$pair" in
    *=) echo "缺少 ${pair%=*} —— 设成环境变量，或写进 $asc_env。见 docs/release-ios.md。" >&2; exit 2 ;;
  esac
done
[ -f "$asc_key_path" ] || { echo "ASC_KEY_PATH 指向的 .p8 不存在: $asc_key_path" >&2; exit 2; }

# altool 的 `--api-key <ID>` **不吃路径**：它去几个固定目录里找名叫
# `AuthKey_<ID>.p8` 的文件（./private_keys、~/private_keys、~/.private_keys、
# ~/.appstoreconnect/private_keys，或 $API_PRIVATE_KEYS_DIR）。所以这里
# ① 把 key 所在目录喂给 API_PRIVATE_KEYS_DIR，② 提前断言文件名对得上 ——
# 否则要等 archive+export 十几分钟跑完，才在最后一步收到一句「找不到 key」。
asc_key_dir=$(CDPATH= cd -- "$(dirname -- "$asc_key_path")" && pwd)
if [ "$(basename "$asc_key_path")" != "AuthKey_$asc_key_id.p8" ]; then
  echo "ASC_KEY_PATH 的文件名必须是 AuthKey_$asc_key_id.p8（altool 按这个名字找 key）" >&2
  echo "  现在是: $(basename "$asc_key_path")" >&2
  exit 2
fi

# --- 1. 干净快照：钉 main HEAD -----------------------------------------------
# 这台机器的工作区常年是脏的（多 session 并行）。发出去的包必须来自 main，
# 不是来自「碰巧在树上的东西」——所见即所装。和 macOS 那条同一做法。
snap=$(mktemp -d "/tmp/pendingcrew-ios-release.XXXXXX")
git -C "$root" worktree add --detach "$snap/src" main
cleanup() {
  git -C "$root" worktree remove --force "$snap/src" 2>/dev/null || true
  git -C "$root" worktree prune
  rm -rf "$snap"
}
trap cleanup EXIT HUP INT TERM

# --- 2. build 号 = 纪元日时间戳「天.秒」（例 20683.07783）---------------------
# 为什么**不**从 git 历史算（`rev-list --count HEAD`）：2026-08 仓库重建，提交数
# 从三千多掉到个位数 —— ASC 会直接拒收倒退的 build 号，从新仓库一版都发不出去。
# 根因不是数字小，是 build 号建在「历史」这个仓库里最脆的东西上。改成从时钟派生：
# 天生单调，不依赖仓库状态，tarball / 浅克隆 / 重建过的仓库构建都对。
#
# 为什么是「天.秒」而不是别的时钟格式（这三条别删，否则以后有人嫌不好看又改回去；
# 用户 2026-08-18 明确拍板要这个**不可读**的格式）：
#   - 不用 `YYYYMMDDHHMM`：12 位单段 ≈ 2.03e11，**超 2^31**。Apple 只规定「不超过
#     18 个字符」，不管每段数值能有多大，ASC parser 宽度无从证实 —— 不赌。
#   - 不用 Unix 秒：10 位虽在 2^31 内，但 2038-01-19 会越线；且它是三个候选里
#     **最大**的，一旦发出去就再也换不回更小的格式（闸只放行递增）。
#   - 纪元日是三者里**最小**的：20683 → 20260818（YYYYMMDD）→ 1787018157（Unix 秒）
#     逐级都是增，所以以后想改主意，每条路都还走得通。
#   - 第二段**必须零填充**（`%05d`）：补零之后「按整数比」和「按字符串比」得出的
#     先后一致（07431 < 10000 两种解法都成立），不补零就只剩整数解一条路能对。
#
# 时钟**只读一次**、两段都从它派生 —— 分两次读会在跨日那一瞬撕成
# 「今天的日期 + 明天 00:00:00 的秒数」。
# 与 PendingBot、与本仓库 macOS 那条同一方案（2026-08-18 两边机长议定）。
_epoch=$(date -u +%s)
build_number=$(printf '%d.%05d' "$((_epoch / 86400))" "$((_epoch % 86400))")
build_human=$(date -u -r "$_epoch" +'%Y-%m-%d %H:%M:%SZ')

# 版本号必须从快照里读（构建全基于这份快照）——建快照之前的工作区可能是脏的，
# 读那份会所见≠所装。
version=$(sed -n 's/^[[:space:]]*MARKETING_VERSION:[[:space:]]*//p' "$snap/src/project.yml" | head -n 1 | tr -d '"')
test -n "$version"

# --- 3. 防倒退闸：跟 ASC 上已有的最高 build 号比 ------------------------------
# 放在 archive **之前**：拿不到就停，别白跑十几分钟构建。
# 闸本身是三态的（拉到有值 / 拉到但真没有 / 够不着 API 就 exit 1），
# 见 scripts/release/asc-highest-build.py 的注释。
guard="$root/scripts/release/asc-highest-build.py"
test -x "$guard" || { echo "找不到 $guard" >&2; exit 2; }
asc_highest=$(/usr/bin/python3 "$guard")
if ! /usr/bin/python3 "$guard" --greater "$build_number" "$asc_highest"; then
  cat >&2 <<EOF
build 号没有严格递增，拒绝发版。
  这次算出来 : $build_number
  ASC 上最高 : $asc_highest
时间戳理论上不该走到这儿。真走到了，说明要么系统时钟不对，要么有人手工传过一个
未来的 build 号 —— 两种都得先弄清楚，不要绕过这道闸。
EOF
  exit 2
fi
echo "note: build $build_number = $build_human（ASC 当前最高: $asc_highest）"
# 纪元日不可读是有意的，但「不可读」不等于「查不到」：上面这行抄进 TestFlight 的
# What to Test。事后反查任意一个 <天>.<秒>：date -u -r $(( 天 * 86400 + 秒 ))

# --- 4. 生成 xcodeproj -------------------------------------------------------
# 走 scripts/gen-project.sh 而不是裸 `xcodegen`：生成器版本的唯一真值是仓库根的
# `.xcodegen-version`，不能由这台机器的 brew 状态决定。--fetch 会在版本对不上时
# 把仓库声明的那一版下到 .tools/ 里用，不动系统里已装的那个。
# 快照是全新目录，把 $root 已经下好的那份接过去，省一次下载。
[ -d "$root/.tools" ] && ln -s "$root/.tools" "$snap/src/.tools"
"$snap/src/scripts/gen-project.sh" --fetch

# 之后所有 xcodebuild 都在快照里跑（相对路径的 -project / CODE_SIGN_ENTITLEMENTS
# 都按 cwd / SRCROOT 解析），和 macOS 那条一样。
cd "$snap/src"

# --- 5. 签名参数：写进快照里现生成的 Config/Local.xcconfig ---------------------
# 仓库里被跟踪的默认值（`Config/Signing.xcconfig`）是 **ad-hoc、不带 entitlements**，
# 好让外部贡献者 clone 下来就能编。开发机上那份身份写在 `Config/Local.xcconfig`
# 里，而它是 **gitignored** —— 上面那个干净快照里**根本没有这个文件**。所以发版
# 必须自己把签名参数补齐，不能指望开发机上碰巧有一份。
#
# **为什么不像 macOS 那条一样写在 xcodebuild 命令行上**（这段别删，删了一定有人
# 改回去，然后收到一屏看不懂的 SPM 报错）：命令行上的构建设置是**全局**的，连每个
# SPM 依赖的 target 都吃。`CODE_SIGN_ENTITLEMENTS` 是相对 SRCROOT 的路径，于是
# GoogleSignIn / SwiftTerm / swift-crypto…… 每个包都会拿
# `Resources/PendingCrew.entitlements` 去自己的 checkout 目录里找，然后齐刷刷报
# 「could not be opened」。2026-08-20 实测，十几个包一个不落。
# macOS 那条没炸只是因为它的 identity 停在 ad-hoc（`-`），Xcode 根本没走到给这些
# 包 target 签名那一步 —— 换成真身份就会炸，**不是 iOS 特有的坑**。
#
# 而 project 级 config file 只作用于**本工程的** target，SPM 包完全不受影响。
# `Config/Signing.xcconfig` 末尾那行 `#include? "Config/Local.xcconfig"` 是按
# SRCROOT 解析的（2026-08-20 用 -showBuildSettings 实测），所以这里写一份进快照就行。
#
# CODE_SIGN_IDENTITY 必须给一个真身份：iOS 上带 entitlements 的 app 不能 ad-hoc 签，
# Xcode 会直接拒（"has entitlements that require signing with a development
# certificate"）。archive 用**开发身份**，发布身份由下面 exportArchive 按
# `method: app-store-connect` 重新签 —— 这是 Apple 官方的分发路径，签名全权交给
# Xcode，脚本里不出现任何一行手写的 codesign（手工复刻 Xcode 的签名行为在 macOS
# 那条上出过两次血，见 build-macos-update.sh）。
cat > "$snap/src/Config/Local.xcconfig" <<XCCONFIG
CODE_SIGN_STYLE = Automatic
DEVELOPMENT_TEAM = $team_id
CODE_SIGN_IDENTITY = Apple Development
CODE_SIGN_ENTITLEMENTS = Resources/$app_name.entitlements
XCCONFIG

# --- 6. archive --------------------------------------------------------------
derived="$snap/derived"
xcarchive="$snap/$app_name.xcarchive"

# `-allowProvisioningUpdates` + 三个 -authenticationKey* 让 xcodebuild 拿 ASC API key
# 自己去开发者网站建/更新 App ID、profile 和证书 —— 这台机器的钥匙串里只有
# Apple Development 和 Developer ID，**没有 Apple Distribution**，全靠这条。
#
# `-destination 'generic/platform=iOS'` 不能省、也不能只写 `-sdk iphoneos`：
# PendingCrew 是**单 target 两平台**（supportedDestinations: [iOS, macOS]），
# 不指明 destination 会打出 macOS 产物 —— .ipa 的外壳看不出这个错，所以下面
# 那道 ③ 断言专门盯它。
#
# 版本号两个键留在命令行上：它们全局生效也无害（SPM 包的 target 拿到也不会怎样），
# 而且**必须**这样传 —— Info.plist 里写的是 $(MARKETING_VERSION) /
# $(CURRENT_PROJECT_VERSION) 变量替换，不传值就会原样留下 project.yml 里的占位符。
xcodebuild -project "$app_name.xcodeproj" -scheme "$app_name" -configuration Release \
  -destination 'generic/platform=iOS' -derivedDataPath "$derived" \
  -archivePath "$xcarchive" -allowProvisioningUpdates \
  -authenticationKeyPath "$asc_key_path" \
  -authenticationKeyID "$asc_key_id" \
  -authenticationKeyIssuerID "$asc_issuer_id" \
  MARKETING_VERSION="$version" CURRENT_PROJECT_VERSION="$build_number" archive

# --- 7. exportArchive 出 .ipa ------------------------------------------------
# method 用 `app-store-connect`：Xcode 15.3 起 `app-store` 已经是 deprecated 别名
# （`xcodebuild -help` 里写着 "app-store (deprecated: use app-store-connect)"）。
#
# destination 故意是 `export` 而不是 `upload`：`upload` 会让 exportArchive 一步
# 直传 ASC，**中间不落 .ipa**，于是「产物长什么样」这件事就没法在上传前验了。
# 我们要的顺序是「先出包 → 全部断言过了 → 再传」，所以出包和上传是两步。
#
# manageAppVersionAndBuildNumber 显式关掉：这个键默认 **YES**，意思是上传时让
# Xcode 自己改 build 号。第 3 步那道防倒退闸算出来的号是这次发版的唯一真值，
# 谁都不许在后面偷偷改它。
cat > "$snap/ExportOptions.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>method</key><string>app-store-connect</string>
	<key>teamID</key><string>$team_id</string>
	<key>signingStyle</key><string>automatic</string>
	<key>destination</key><string>export</string>
	<key>manageAppVersionAndBuildNumber</key><false/>
</dict>
</plist>
PLIST

xcodebuild -exportArchive -archivePath "$xcarchive" \
  -exportPath "$snap/export" -exportOptionsPlist "$snap/ExportOptions.plist" \
  -allowProvisioningUpdates \
  -authenticationKeyPath "$asc_key_path" \
  -authenticationKeyID "$asc_key_id" \
  -authenticationKeyIssuerID "$asc_issuer_id"

# --- 8. 八道断言 -------------------------------------------------------------
# 每一道都对应一种「包能出、能上传，然后在别处安静地坏掉」的失败。

# ① 产物在不在。
src_ipa="$snap/export/$app_name.ipa"
test -f "$src_ipa" || { echo "exportArchive 没产出 $src_ipa" >&2; exit 2; }

release_dir="$root/dist/testflight"
mkdir -p "$release_dir"
ipa="$release_dir/$app_name-$version-$build_number.ipa"
cp "$src_ipa" "$ipa"     # 快照退出时会被删掉，产物得先搬出来

work="$snap/verify"
mkdir -p "$work"
/usr/bin/unzip -q "$ipa" -d "$work"
app="$work/Payload/$app_name.app"
test -d "$app" || { echo ".ipa 里没有 Payload/$app_name.app" >&2; exit 2; }
plist="$app/Info.plist"

# ② 版本号必须真的落进产物。project.yml 里写的是 $(MARKETING_VERSION)/
#    $(CURRENT_PROJECT_VERSION) 变量替换，构建期不传值就会原样留下占位符 "1" ——
#    ASC 会拒收，但那要等上传往返才知道。
got_short=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist")
got_build=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$plist")
[ "$got_short" = "$version" ] || { echo "产物 CFBundleShortVersionString=$got_short，期望 $version" >&2; exit 2; }
[ "$got_build" = "$build_number" ] || { echo "产物 CFBundleVersion=$got_build，期望 $build_number" >&2; exit 2; }

# ③ 平台必须是 iPhoneOS。单 target 两平台，destination 写错就会打出 macOS 产物，
#    .ipa 外壳一模一样看不出来。
platform=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleSupportedPlatforms:0' "$plist")
[ "$platform" = "iPhoneOS" ] || { echo "产物平台是 $platform，不是 iPhoneOS —— archive 的 -destination 写错了" >&2; exit 2; }

# ④ 签的必须是团队身份，不是 ad-hoc。仓库默认 xcconfig 就是 ad-hoc（`-`），
#    命令行那几个键一旦漏传，构建照样成功、产物照样是 .ipa，但 ASC 会拒收，
#    而且从文件名上完全看不出来。
signing=$(codesign -dv "$app" 2>&1)
printf '%s\n' "$signing" | grep -q "TeamIdentifier=$team_id" \
  || { echo "产物不是 $team_id 签的：" >&2; printf '%s\n' "$signing" >&2; exit 2; }
if printf '%s\n' "$signing" | grep -q 'Signature=adhoc'; then
  echo "产物是 ad-hoc 签名 —— App Store 不收，签名参数没生效" >&2
  exit 2
fi

# ⑤ provisioning profile 必须嵌进去。app 的 entitlements 里有受限项
#    （application-identifier / keychain-access-groups），没有 profile 授权就会被
#    AMFI 在 exec 前拒 —— 表现是装上去点不开，而签名验证全过。
test -f "$app/embedded.mobileprovision" \
  || { echo "产物缺 embedded.mobileprovision —— 受限 entitlement 没有授权，app 会打不开" >&2; exit 2; }

built_ent=$(codesign -d --entitlements - --xml "$app" 2>/dev/null | plutil -convert xml1 -o - - 2>/dev/null)

# ⑥ entitlements 里不许残留未展开的构建变量，且 keychain 共享组必须真的带上
#    team 前缀（正面断言，不只是「没有 $(」这种反面判据）。
#    macOS 那边踩过：`$(AppIdentifierPrefix)` 没展开 → keychain access group 变成
#    无意义字面量 → app 读不到家族 SSO 凭据、登录态静默丢失，全链路零报错。
if printf '%s' "$built_ent" | grep -q '[$]('; then
  echo "签名后的 entitlements 仍含未展开的构建变量 —— 会让 keychain 组失效、登录态丢失" >&2
  printf '%s\n' "$built_ent" >&2
  exit 2
fi
for group in "$team_id.com.pendingname.pendingcrew" "$team_id.com.pendingname.shared"; do
  printf '%s' "$built_ent" | grep -q "$group" \
    || { echo "产物 entitlements 里没有 $group —— 钥匙串组会失效" >&2; exit 2; }
done

# ⑦ 源 entitlements 里声明的键，产物里必须真的还在（**逐项正面比对**）。
#    Xcode 签名时会把 profile 未授权的键**静默剥掉**：产物不报任何错，包能签能装
#    能跑，只是那个能力永远不工作。上面几道都是「查我们想到的那几个键」，查不出
#    「声明了但被吞了」这类问题，所以这里拿源文件当清单逐项核对。
#    2026-08-07 macOS 首发实测踩过：推送键写成了 iOS 的 `aps-environment`，
#    而 macOS 要 `com.apple.developer.aps-environment` —— 两个不同的键，被静默吞掉。
src_ent="$snap/src/Resources/$app_name-iOS.entitlements"
[ -f "$src_ent" ] || src_ent="$snap/src/Resources/$app_name.entitlements"
if [ -f "$src_ent" ]; then
  /usr/libexec/PlistBuddy -c 'Print' "$src_ent" 2>/dev/null \
    | sed -n 's/^ *\([A-Za-z][A-Za-z0-9._-]*\) = .*/\1/p; s/^ *\([A-Za-z][A-Za-z0-9._-]*\) = Array {/\1/p' \
    | sort -u \
    | while IFS= read -r k; do
        [ -n "$k" ] || continue
        if ! printf '%s' "$built_ent" | grep -q "<key>$k</key>"; then
          echo "源 entitlements 声明了 <$k>，但产物里没有 —— 被签名步骤静默剥掉了。" >&2
          echo "  多半是 provisioning profile 没授权这个能力，或键名用错了平台。" >&2
          exit 2
        fi
      done || exit 2
fi

# ⑧ 这份 entitlements 必须真的是**发布**用的，不是开发用的。两条一起看：
#    - `get-task-allow` 不能是 true。它是调试用的（让 debugger attach），
#      development 签名会带 true；带着 true 上传会被 ITMS 拒。
#      注意**不能只判「有没有这个键」** —— App Store 的正常产物里它是存在的、
#      值为 `<false/>`（2026-08-20 实测），按「存在即失败」写会把好包判死。
#    - `beta-reports-active` 必须是 true。这个键只有 **App Store 分发 profile**
#      才会给，是「这一步真的按 app-store-connect 重签过、不是拿 archive 里那份
#      开发签名蒙混过关」的正面证据。
#    这两条都是 iOS 特有的，Developer ID 那边没有对应项 —— 别照抄 macOS 的清单
#    就以为齐了。
ent_plist="$work/entitlements.plist"
printf '%s' "$built_ent" > "$ent_plist"
if [ "$(/usr/libexec/PlistBuddy -c 'Print :get-task-allow' "$ent_plist" 2>/dev/null)" = "true" ]; then
  echo "产物 entitlements 里 get-task-allow = true —— 这是 development 签名，App Store 会拒收" >&2
  exit 2
fi
if [ "$(/usr/libexec/PlistBuddy -c 'Print :beta-reports-active' "$ent_plist" 2>/dev/null)" != "true" ]; then
  echo "产物 entitlements 里没有 beta-reports-active —— 这不是 App Store 分发 profile 签的，TestFlight 收不了" >&2
  printf '%s\n' "$built_ent" >&2
  exit 2
fi

echo "note: 八道断言全过"
ls -lh "$ipa"

# --- 9. 上传 TestFlight ------------------------------------------------------
# 为什么必须显式传 `--apple-id $asc_app_id`：Xcode 26 的 altool 从 bundle id 反查
# app 时会挑到**相似**的那个（fastlane#29698 / #29743：日志照样打
# "Successfully uploaded"，实际传去了别的 app）。这个团队下正好有
# com.pendingname.pendingbot 和 com.pendingname.pendingcrew 两个相似 id，
# 这不是理论风险。数字 id 是唯一没有歧义的指认。
#
# 为什么是 altool 而不是别的：Xcode 26.5 的 altool 是**重写过的新版**（altool(1)，
# 走 App Store Connect API），没有被弃用 —— 被降级成 legacy 的是旧那版，
# 藏在 `--use-old-altool` / `man 7 altool` 后面。man 页里 upload 的首选写法是
# `--upload-package`，`--upload-app -f` 是同义的旧拼法。
# （notarytool 是公证专用，不传 App Store —— 那是 macOS 那条脚本的活。）
upload_cmd="API_PRIVATE_KEYS_DIR='$asc_key_dir' xcrun altool --upload-package '$ipa' \\
    --platform ios --apple-id $asc_app_id --bundle-id $bundle_id \\
    --bundle-version $build_number --bundle-short-version-string $version \\
    --api-key '<ASC_KEY_ID>' --api-issuer '<ASC_ISSUER_ID>' --show-progress"

if [ "$dry_run" = "1" ]; then
  cat <<EOF

—— --dry-run：到此为止，**没有上传任何东西** ——

  .ipa            : $ipa
  大小            : $(du -h "$ipa" | cut -f1)
  app id (ASC)    : $asc_app_id
  bundle id       : $bundle_id
  版本            : $version (CFBundleShortVersionString)
  build           : $build_number (CFBundleVersion) = $build_human
  ASC 当前最高    : $asc_highest
  签名            : $(printf '%s\n' "$signing" | grep -E 'TeamIdentifier|Authority' | head -n 2 | tr '\n' ' ')

真要上传就去掉 --dry-run 重跑（会重新算 build 号、重新构建）。等价的手工命令：

    $upload_cmd

  （<ASC_KEY_ID> / <ASC_ISSUER_ID> 在 $asc_env 里，脚本自己会读；
    这里不打印它们的值。）
EOF
  exit 0
fi

echo "note: 开始上传 $ipa → App Store Connect（app id $asc_app_id）"
API_PRIVATE_KEYS_DIR="$asc_key_dir" xcrun altool --upload-package "$ipa" \
  --platform ios --apple-id "$asc_app_id" --bundle-id "$bundle_id" \
  --bundle-version "$build_number" --bundle-short-version-string "$version" \
  --api-key "$asc_key_id" --api-issuer "$asc_issuer_id" --show-progress

cat <<EOF

上传完成。ASC 那边还要处理几分钟，查状态：

    API_PRIVATE_KEYS_DIR='$asc_key_dir' xcrun altool --build-status \\
      --apple-id $asc_app_id --bundle-id $bundle_id \\
      --bundle-version $build_number --bundle-short-version-string $version \\
      --platform ios --api-key '<ASC_KEY_ID>' --api-issuer '<ASC_ISSUER_ID>'

TestFlight 的 What to Test 里记一句可读时间：build $build_number = $build_human
EOF
