#!/bin/sh
# 用法: PENDING_NOTARY_PROFILE=pendingcrew-notary \
#       scripts/release/make-dmg.sh <PendingCrew.app 或 .zip> [输出目录]
#
# 把一个**已经签名+公证+staple 好**的 .app 装进 .dmg，再把 dmg 自己也签名 +
# 公证 + staple。
#
# 为什么要 dmg（Release 页面已经有 .zip 了）：.zip 下载解压之后，用户多半直接
# 在「下载」文件夹里双击 —— macOS 会把从下载目录直接运行的 app 挪到一个随机
# 只读路径跑（App Translocation），自动更新写不回去、本地数据目录也会出怪事。
# dmg 里放一个「应用程序」快捷方式，是唯一能把人**引导到正确安装位置**的标准
# 姿势。.zip 保留给 Sparkle 自动更新用（Sparkle 只吃 zip）。
#
# 为什么 dmg 自己也要签名+公证（里面的 app 已经 staple 过了，光这样也能开）：
# 没签的 dmg 挂载那一下 Gatekeeper 仍会去联网查一次；断网或 Apple 那边慢的
# 时候用户会盯着一个转圈的框。签+staple 之后那次往返变成本地校验。
#
# **这个脚本不构建任何东西。** 挂 GitHub Release 用的是手边那份已公证的产物，
# 不为了发 Release 而重新构建一版 —— 理由见 docs/release-macos.md。
set -eu

app_name=PendingCrew
: "${PENDING_NOTARY_PROFILE:?set the notarytool Keychain profile}"
src=${1:?usage: make-dmg.sh <app-or-zip> [outdir]}
[ -e "$src" ] || { echo "找不到 $src" >&2; exit 2; }

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)

# 默认**不**输出到输入文件旁边（那是 dist/updates/pendingcrew/ —— Sparkle
# generate_appcast 的扫描目录）。dmg 只给 GitHub Release 用，Sparkle 只吃 zip；
# 两者挤在同一个目录里，Sparkle 会看见同一个 bundle version 出现两次，直接拒：
#   "Duplicate updates are not supported. Found archives 'X.zip' and 'X.dmg'"
# 2026-08-24 出 0.1.14 时就是这么断在最后一步的（0.1.13 那次只是因为 dmg 是在
# appcast 之后才造的，顺序碰巧躲过去了）。
# 所以：feed 目录只装 feed 该有的东西（zip + appcast.xml + .html），dmg 单独放。
outdir=${2:-$root/dist/releases/pendingcrew}
mkdir -p "$outdir"
outdir=$(CDPATH= cd -- "$outdir" && pwd)

work=$(mktemp -d "/tmp/$app_name-dmg.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM

# 输入可以是 .app 也可以是发版脚本产出的 .zip
case "$src" in
  *.zip) /usr/bin/ditto -x -k "$src" "$work/unzip"; app="$work/unzip/$app_name.app" ;;
  *)     app=$(CDPATH= cd -- "$src" && pwd) ;;
esac
test -d "$app" || { echo "解不出 $app_name.app（$src）" >&2; exit 2; }

# —— 进 dmg 之前先确认这份 app 本身是合格的 ——
# 这三道都是**输入检查**，不是我们自己造出来的性质。dmg 只是个容器：装进去一个
# 没公证的 app，dmg 签得再好用户照样被拦。宁可在这儿停，别做出一个装不上的包。
codesign --verify --deep --strict "$app" \
  || { echo "输入的 app 签名不过 —— 别往 dmg 里装" >&2; exit 2; }
spctl -a -vvv -t install "$app" 2>&1 | grep -q 'source=Notarized Developer ID' \
  || { echo "输入的 app 不是「已公证的 Developer ID」—— 用户双击会被 Gatekeeper 拦成「无法验证开发者」" >&2
       spctl -a -vvv -t install "$app" >&2 2>&1 || true; exit 2; }
xcrun stapler validate "$app" >/dev/null 2>&1 \
  || { echo "输入的 app 没有 staple 上公证票 —— 用户离线首次打开会被拦" >&2; exit 2; }

version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")
build=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$app/Contents/Info.plist")
echo "note: 打包 $app_name $version ($build)"

stage="$work/stage"
mkdir -p "$stage"
/usr/bin/ditto "$app" "$stage/$app_name.app"
ln -s /Applications "$stage/应用程序"

dmg="$outdir/$app_name-$version.dmg"
rm -f "$dmg"
hdiutil create -volname "$app_name $version" -srcfolder "$stage" \
  -fs HFS+ -format UDZO -ov -quiet "$dmg"

identity=${PENDING_SIGN_IDENTITY:-Developer ID Application: Yanze Tan (M42BKJN82S)}
codesign --sign "$identity" --timestamp --force "$dmg"
xcrun notarytool submit "$dmg" --keychain-profile "$PENDING_NOTARY_PROFILE" --wait
xcrun stapler staple "$dmg"

# —— 出门前自查。`-t open --context context:primary-signature` 是 Gatekeeper
# 判一个**下载来的磁盘映像**时走的那条路径，跟判 app 的 `-t install` 不是一回事，
# 必须按 dmg 的口径验一遍。
codesign --verify --strict --verbose=2 "$dmg"
spctl -a -t open --context context:primary-signature -vvv "$dmg"
xcrun stapler validate "$dmg"
echo "DMG ready: $dmg"
