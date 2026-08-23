#!/bin/sh
# 用法: PENDING_NOTARY_PROFILE=pendingcrew-notary [PENDING_PUBLISH_R2=1] \
#       scripts/release/build-macos-update.sh
#
# 公证 profile 叫 `pendingcrew-notary`，在登录钥匙串里，用本机那把 App Store
# Connect API key 建的（团队 M42BKJN82S，与 PendingBot 传 TestFlight 同一把）。
# 没有的话按 `docs/release-macos.md` 里的 `notarytool store-credentials` 重建 ——
# **别在这儿翻半天然后把公证关掉**：2026-08-19 查出线上装着的包正是这么来的，
# 签名对、hardened runtime 对，就是没公证票，换台机器直接被 Gatekeeper 拦。
#
# 干净快照（钉 main HEAD）里构建 Release → Developer ID 签名（含 Sparkle
# 内嵌件）→ 公证 → staple → 生成更新说明 → generate_appcast 签 feed
# → 打 tag → （可选）发 R2。工作区脏不脏都不影响所见即所装。
set -eu

product=pendingcrew
app_name=PendingCrew
: "${PENDING_NOTARY_PROFILE:?set the notarytool Keychain profile}"

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)

# 干净快照：钉 main HEAD（#556 的教训 —— 这台机器工作区常年是脏的）
snap=$(mktemp -d "/tmp/$product-release.XXXXXX")
git -C "$root" worktree add --detach "$snap/src" main
cleanup() {
  git -C "$root" worktree remove --force "$snap/src" 2>/dev/null || true
  git -C "$root" worktree prune
  rm -rf "$snap"
}
trap cleanup EXIT HUP INT TERM
# build 号 = 纪元日时间戳「天.秒」（例 20683.07783）。
#
# 为什么**不**再从 git 历史算（原来是 `rev-list --count HEAD`）：2026-08 仓库重建，
# 提交数从三千多掉到个位数，而线上已发布 3703 —— 从新仓库一版都发不出去（下面那道
# 闸会正确地拒）。历史是这个仓库里最脆的东西，build 号不该建在它上面。改成从时钟
# 派生：天生单调，不依赖仓库状态，tarball / 浅克隆 / 重建过的仓库构建都对。
#
# 为什么是「天.秒」而不是别的时钟格式（这三条别删，否则以后有人嫌不好看又改回去）：
#   - 不用 `YYYYMMDDHHMM`：12 位单段 ≈ 2.03e11，**超 2^31**。Apple 只规定「不超过
#     18 个字符」，不管每段数值能有多大，ASC parser 宽度无从证实 —— 不赌。
#   - 不用 Unix 秒：10 位虽在 2^31 内，但 2038-01-19 会越线；且它是三个候选里**最大**
#     的，一旦发出去就再也换不回更小的格式（闸只放行递增）。
#   - 纪元日是三者里**最小**的：20683 → 20260818（YYYYMMDD）→ 1787018157（Unix 秒）
#     逐级都是增，所以以后想改主意，每条路都还走得通。
#   - 第二段**必须零填充**（`%05d`）：补零之后「按整数比」和「按字符串比」得出的先后
#     一致（07431 < 10000 两种解法都成立），不补零就只剩整数解一条路能对。
#
# 时钟**只读一次**、两段都从它派生 —— 分两次读会在跨日那一瞬撕成
# 「今天的日期 + 明天 00:00:00 的秒数」。
# 与 PendingBot 同一方案（2026-08-18 两边机长议定）。
_epoch=$(date -u +%s)
build_number=$(printf '%d.%05d' "$((_epoch / 86400))" "$((_epoch % 86400))")

# 版本号必须从快照里读（构建/tag 全基于这份快照）——建快照之前的工作区
# 可能是脏的，读那份会所见≠所装。
version=$(sed -n 's/^[[:space:]]*MARKETING_VERSION:[[:space:]]*//p' "$snap/src/project.yml" | head -n 1 | tr -d '"')
test -n "$version"

# 更新说明在第 ⑦ 步才真正生成，但那时公证已经走完了 —— CHANGELOG 里少一段就得
# 从头再来一遍（含一次 Apple 公证往返）。这里提前把同一个取段脚本跑一次，缺了
# 立刻停，别让人白等。
echo "note: 预检 CHANGELOG.md 里 $version 那一段"
"$root/scripts/release/changelog-section.sh" "$version" >/dev/null

# 逐段比较两个 1~3 段版本号。退出码 0 = $1 严格大于 $2。
#
# 不用 `[ -le ]`：那是 shell **整数**比较，喂带句点的版本号会直接
# `integer expression expected` 报错。也不用 `sort -V`：BSD/GNU 行为不完全一致。
# 段数不同的按缺位补 0 比（`20683` vs `20683.00001` → 后者大），与 Sparkle 的
# `SUStandardVersionComparator` 同一口径。`+0` 让 `07431` 这类零填充段按整数解。
version_gt() {
  awk -v a="$1" -v b="$2" '
    BEGIN {
      n = split(a, xa, "."); m = split(b, ya, ".")
      k = (n > m ? n : m)
      for (i = 1; i <= k; i++) {
        xv = (i <= n ? xa[i] + 0 : 0)
        yv = (i <= m ? ya[i] + 0 : 0)
        if (xv > yv) exit 0
        if (xv < yv) exit 1
      }
      exit 1          # 完全相等也不算「大于」
    }'
}

# 不许倒退：build 号必须严格大于线上 feed 里的最大值，否则已装机的用户永远
# 收不到这次更新（Sparkle 比的是 CFBundleVersion）。
#
# **三态**，缺一不可（2026-08-18）：以前这里是 `curl ... || true`，于是
# 「网络失败」和「feed 里确实没有任何版本（首发）」长得一模一样 —— 网络抖一下，
# 这道防倒退的闸就**静默失效**。拿不到数据时默认放行，正是这类闸最典型的死法。
#   ① 拉到了、且有版本  → 正常比大小
#   ② 拉到了、但确实为空 → 这才是合法的首发，放行
#   ③ 拉取失败          → 退出，**不放行**
#
# 版本号正则要吃带句点的（`[0-9][0-9.]*`）—— build 号已改成「天.秒」两段式，
# 旧那个只认纯数字的 `[0-9][0-9]*` 会一条都匹配不上，然后一路走进 ②「首发」。
feed_url="https://updates.pendingname.com/$product/appcast.xml"
if feed=$(curl -fsS --max-time 20 "$feed_url"); then
  live_versions=$(printf '%s\n' "$feed" \
    | sed -n 's/.*<sparkle:version>\([0-9][0-9.]*\)<\/sparkle:version>.*/\1/p')
  if [ -z "$live_versions" ]; then
    echo "note: 线上 feed 拉到了，但里面没有任何已发布版本（首发），跳过不许倒退的检查"
  else
    live_max=""
    for v in $live_versions; do
      if [ -z "$live_max" ] || version_gt "$v" "$live_max"; then live_max=$v; fi
    done
    if ! version_gt "$build_number" "$live_max"; then
      echo "build 号 $build_number 不大于线上已发布的 $live_max —— 已装机的用户收不到这次更新" >&2
      exit 2
    fi
    echo "note: build 号 $build_number > 线上最大 $live_max，不倒退"
  fi
else
  echo "拉不到线上 feed（$feed_url）—— 无法确认这次不会版本倒退，拒绝构建。" >&2
  echo "这是 fail-closed：网络失败绝不能和「首发」共用一条放行路径。" >&2
  exit 2
fi

release_dir="$root/dist/updates/$product"
mkdir -p "$release_dir"
derived="$snap/derived"

team_id="${PENDING_TEAM_ID:-M42BKJN82S}"

cd "$snap/src"
xcodegen generate

# 为什么这里要显式传 CODE_SIGN_STYLE / DEVELOPMENT_TEAM / CODE_SIGN_ENTITLEMENTS：
# 仓库里被跟踪的默认值（`Signing.xcconfig`）是 **ad-hoc、不带 entitlements**，
# 好让外部贡献者 clone 下来就能编。开发机上那份 Developer ID 身份写在
# `Local.xcconfig` 里，而它是 **gitignored** —— 上面那个干净快照
# （`git worktree add`）里**根本没有这个文件**。所以发版必须在命令行上把三个
# 键都补齐（命令行优先级最高），不能指望 xcconfig。
# 漏掉 CODE_SIGN_ENTITLEMENTS 的话下面第 ④ 道断言会拦住（产物里没有
# `$team_id.com.pendingname.shared`），但那要等到构建完才炸，不如在这儿说清。

# 签名**全权交给 Xcode**（archive + exportArchive，Apple 官方的 Developer ID 分发路径）。
#
# 2026-08-06 的教训，两次出血同源：此前是 `CODE_SIGNING_ALLOWED=NO` 构建再手工
# codesign，等于手工复刻 Xcode 的签名行为，而两次都只复刻了看得见的那部分——
#   ① 漏了 `$(AppIdentifierPrefix)` 展开 → keychain access group 变成无意义字面量，
#      app 读不到家族 SSO 凭据、登录态静默丢失（签名有效/公证通过/Gatekeeper 放行，
#      全链路零报错）
#   ② 补上 application-identifier 后，又漏了**授权这个受限 entitlement 的
#      provisioning profile** → AMFI 在 exec 前拒绝，app 根本打不开
# Xcode 自己签会把这些一并做对：申请并嵌入 Developer ID profile、展开 entitlement
# 变量、按 inside-out 顺序签 Sparkle 等内嵌框架、开 hardened runtime、加时间戳。
# 这里不再出现任何一行手写的 codesign。
xcarchive="$snap/$app_name.xcarchive"
xcodebuild -project "$app_name.xcodeproj" -scheme "$app_name" -configuration Release \
  -destination 'generic/platform=macOS' -derivedDataPath "$derived" \
  -archivePath "$xcarchive" -allowProvisioningUpdates \
  CODE_SIGN_STYLE=Automatic DEVELOPMENT_TEAM="$team_id" \
  CODE_SIGN_ENTITLEMENTS="Resources/$app_name.entitlements" \
  ENABLE_HARDENED_RUNTIME=YES \
  MARKETING_VERSION="$version" CURRENT_PROJECT_VERSION="$build_number" archive
# 注意：archive 阶段**不要**指定 CODE_SIGN_IDENTITY。自动签名 + 手工指定发布身份
# 会被 Xcode 判为 "conflicting provisioning settings" 直接失败（含每个 SPM 依赖）。
# 正确分工是：archive 用自动签名（开发身份），发布身份由下面 exportArchive 按
# `method: developer-id` 重新签，并在那一步嵌入 Developer ID profile。

# 签名风格：PendingCrew 只声明 keychain-access-groups —— provisioning profile
# 自动允许 TEAMID.* 下所有组，不需要在 portal 上配任何东西，automatic 一路畅通。
signing_block="	<key>signingStyle</key><string>automatic</string>"

cat > "$snap/ExportOptions.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>method</key><string>developer-id</string>
	<key>teamID</key><string>$team_id</string>
$signing_block
	<key>destination</key><string>export</string>
</dict>
</plist>
PLIST

xcodebuild -exportArchive -archivePath "$xcarchive" \
  -exportPath "$snap/export" -exportOptionsPlist "$snap/ExportOptions.plist" \
  -allowProvisioningUpdates

app="$snap/export/$app_name.app"
test -d "$app" || { echo "exportArchive 没产出 $app" >&2; exit 2; }

# —— 以下四道断言，每一道都对应一次真实踩过的坑 ——

# ① 无公钥的包 = 永远收不到更新的死包。generate_appcast 对缺钥静默跳过签名，
#    客户端对缺钥静默不启动 —— 两端都安静，只能在这里拦。
/usr/libexec/PlistBuddy -c 'Print :SUPublicEDKey' "$app/Contents/Info.plist" >/dev/null \
  || { echo "产物 Info.plist 缺 SUPublicEDKey —— 先用 Sparkle 的 generate_keys 生成密钥、把公钥回填进 Info.plist，再发布" >&2; exit 2; }

# ② provisioning profile 必须嵌进去。app 的 entitlements 里有受限项
#    （application-identifier / keychain-access-groups），没有 profile 授权就会被
#    AMFI 在 exec 前拒绝 —— 表现是「无法打开」，而签名验证、公证、Gatekeeper 全过。
test -f "$app/Contents/embedded.provisionprofile" \
  || { echo "产物缺 embedded.provisionprofile —— 受限 entitlement 没有授权，app 会打不开" >&2; exit 2; }

# ③ 签完的 entitlements 里不许残留未展开的构建变量。
if codesign -d --entitlements - --xml "$app" 2>/dev/null | grep -q '[$](' ; then
  echo "签名后的 entitlements 仍含未展开的构建变量 —— 会让 keychain 组失效、登录态丢失" >&2
  codesign -d --entitlements - --xml "$app" 2>/dev/null | plutil -convert xml1 -o - - >&2
  exit 2
fi

# ④ keychain 共享组必须真的带上 team 前缀（正面断言，不只是"没有 $(" 这种反面判据）。
if ! codesign -d --entitlements - --xml "$app" 2>/dev/null | grep -q "$team_id\.com\.pendingname\.shared"; then
  echo "产物 entitlements 里没有 $team_id.com.pendingname.shared —— 家族 SSO 共享组会失效" >&2
  exit 2
fi

# ⑤ 源 entitlements 里声明的键，产物里必须真的还在（**逐项正面比对**）。
#    2026-08-07 首发实测：macOS 那份把推送键写成了 iOS 的 `aps-environment`，
#    而 macOS 要的是 `com.apple.developer.aps-environment` —— **两个不同的键**。
#    Xcode 签名时会把 profile 未授权的键**静默剥掉**：产物既没有推送 entitlement
#    也不报任何错，包能签能公证能装能跑，只是 Mac 端永远收不到推送。
#    上面 ①—④ 那几道都是「查我们想到的那几个键」，查不出「声明了但被吞了」这类问题，
#    所以这里改成拿源文件当清单逐项核对。
src_ent="$snap/src/Resources/$app_name-macOS.entitlements"
[ -f "$src_ent" ] || src_ent="$snap/src/Resources/$app_name.entitlements"
if [ -f "$src_ent" ]; then
  built_ent=$(codesign -d --entitlements - --xml "$app" 2>/dev/null | plutil -convert xml1 -o - - 2>/dev/null)
  /usr/libexec/PlistBuddy -c 'Print' "$src_ent" 2>/dev/null \
    | sed -n 's/^ *\([A-Za-z][A-Za-z0-9._-]*\) = .*/\1/p; s/^ *\([A-Za-z][A-Za-z0-9._-]*\) = Array {/\1/p' \
    | sort -u \
    | while IFS= read -r k; do
        [ -n "$k" ] || continue
        if ! printf '%s' "$built_ent" | grep -q "<key>$k</key>"; then
          echo "源 entitlements 声明了 <$k>，但产物里没有 —— 被签名步骤静默剥掉了。" >&2
          echo "  多半是键名用错平台（如 macOS 推送要 com.apple.developer.aps-environment，" >&2
          echo "  不是 iOS 的 aps-environment），或 provisioning profile 没授权这个能力。" >&2
          exit 2
        fi
      done || exit 2
fi

# ⑥ hardened runtime 必须开（公证的硬性前提）。漏了的话要等三分钟公证往返
#    才被 Apple 拒，不如在这里立刻拦。flags 里带 `runtime` 才算数。
if ! codesign -dvv "$app" 2>&1 | grep -qE '^CodeDirectory.*flags=.*runtime'; then
  echo "产物没开 hardened runtime —— 公证会被 Apple 拒绝" >&2
  codesign -dvv "$app" 2>&1 | grep -E '^CodeDirectory' >&2
  exit 2
fi

# ⑥ 每个 @rpath 依赖都必须能在包内解析出真实文件 —— 也就是把 dyld 的解析规则
#    在发布前跑一遍。上面五道查的全是签名和 entitlement，没人查 dyld 能不能把库
#    找着；2026-08-06 就是这么让一个**六道全绿、一双击就 SIGABRT** 的包上了线。
"$root/scripts/release/verify-rpath-resolvable.sh" "$app" "$app_name"

archive="$release_dir/$app_name-$version.zip"
/usr/bin/ditto -c -k --keepParent "$app" "$archive"
xcrun notarytool submit "$archive" --keychain-profile "$PENDING_NOTARY_PROFILE" --wait
xcrun stapler staple "$app"
/usr/bin/ditto -c -k --keepParent "$app" "$archive"

# 更新说明要在 generate_appcast 之前生成（同名 .html 会被嵌进 feed item）
"$root/scripts/release/generate-release-notes.sh" "$version"

gen="$derived/SourcePackages/artifacts/sparkle/Sparkle/bin/generate_appcast"
test -x "$gen"
"$gen" --account "com.pendingname.$product" \
  --download-url-prefix "https://updates.pendingname.com/$product/" \
  "$release_dir"

codesign --verify --deep --strict --verbose=2 "$app"
spctl -a -vvv -t install "$app"
xcrun stapler validate "$app"
git -C "$root" tag "v$version" main 2>/dev/null \
  || echo "tag v$version 已存在，沿用"
echo "Update ready: $archive"

if [ "${PENDING_PUBLISH_R2:-0}" = "1" ]; then
  # 发布到 R2 需要 PATH 上有 wrangler 4.x（或用 PENDING_UPDATES_WRANGLER 指路）。
  "$root/scripts/release/publish-macos-update-r2.sh" "$product" "$release_dir"
fi
