#!/bin/sh
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
product=${1:-}
release_dir=${2:-}

if [ -z "$product" ] || [ -z "$release_dir" ]; then
  echo "usage: $0 <pendingnet|pendingcrew|pendingbot> <release-directory>" >&2
  exit 2
fi

case "$product" in
  pendingnet|pendingcrew|pendingbot) ;;
  *) echo "unsupported product: $product" >&2; exit 2 ;;
esac

test -d "$release_dir"
test -f "$release_dir/appcast.xml"

bucket=${PENDING_UPDATES_R2_BUCKET:-pending-updates-prod}
public_base=${PENDING_UPDATES_PUBLIC_BASE_URL:-https://updates.pendingname.com}
wrangler=${PENDING_UPDATES_WRANGLER:-}

case "$public_base" in
  https://*) ;;
  *) echo "public update URL must use HTTPS" >&2; exit 2 ;;
esac

if [ -z "$wrangler" ]; then
  wrangler=$(command -v wrangler || true)
fi
if [ -z "$wrangler" ] || [ ! -x "$wrangler" ]; then
  echo "wrangler 4.x is required; set PENDING_UPDATES_WRANGLER if it is not on PATH" >&2
  exit 2
fi

upload() {
  file=$1
  content_type=$2
  cache_control=$3
  name=$(basename "$file")
  "$wrangler" r2 object put "$bucket/$product/$name" \
    --remote \
    --file "$file" \
    --content-type "$content_type" \
    --cache-control "$cache_control" \
    --force
}

# Publish immutable payloads first. The signed feed is the release commit point
# and is intentionally uploaded last, so it never references a missing archive.
for file in "$release_dir"/*; do
  test -f "$file" || continue
  name=$(basename "$file")
  case "$name" in
    appcast.xml) continue ;;
    *.zip)
      # **上传前逐个验公证**：这个循环把目录里的**每一个**文件都推上公开 CDN，
      # 包括某次公证失败留下的半成品（0.1.35 那个躺了一上午）。判据与
      # `build-macos-update.sh` / `publish-github-release.sh` 共用同一份脚本。
      "$here/verify-zip-notarized.sh" "$file" || {
        echo "拒绝上传：$file 没公证 —— 它会挂在公开 CDN 上。" >&2; exit 2; }
      upload "$file" "application/zip" "public, max-age=31536000, immutable" ;;
    *.html) upload "$file" "text/html; charset=utf-8" "public, max-age=300" ;;
    *.json) upload "$file" "application/json" "public, max-age=300" ;;
    *) upload "$file" "application/octet-stream" "public, max-age=300" ;;
  esac
done

upload "$release_dir/appcast.xml" "application/xml; charset=utf-8" "no-cache, no-store, max-age=0"

feed_url="$public_base/$product/appcast.xml"
curl --fail --silent --show-error --location --max-time 30 "$feed_url" >/dev/null
echo "Published $product update feed: $feed_url"
