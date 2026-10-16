#!/bin/sh
# Embed the exact separately built Core in the ordinary Mac product. No Rust build,
# runtime launch, data access, signing identity or credentials are used here.
set -eu
[ "${PLATFORM_NAME:-}" = "macosx" ] || exit 0
case "${CONFIGURATION:-}" in Debug|Release) ;; *) exit 0 ;; esac
input=${PENDINGCREW_BACKEND_INPUT:-}
expected=${PENDINGCREW_BACKEND_SHA256:-}
case "$input" in /*) ;; *) echo 'error: Supply PENDINGCREW_BACKEND_INPUT with the coordinated normal crewd binary'; exit 1 ;; esac
case "$expected" in *[!0-9a-f]*|"") echo 'error: Supply PENDINGCREW_BACKEND_SHA256'; exit 1 ;; esac
[ "${#expected}" -eq 64 ] && [ -f "$input" ] && [ -x "$input" ] && [ ! -L "$input" ] || { echo 'error: Invalid Core input'; exit 1; }
actual=$(/usr/bin/shasum -a 256 "$input" | /usr/bin/awk '{print $1}')
[ "$actual" = "$expected" ] || { echo 'error: Core input hash differs'; exit 1; }
target="${TARGET_BUILD_DIR:?}/${CONTENTS_FOLDER_PATH:?}/Helpers"
mkdir -p "$target"
cp "$input" "$target/crewd"
chmod 755 "$target/crewd"
[ ! -e "$target/crewd-data-worker" ] || { echo 'error: Retired data worker present in unsigned target'; exit 1; }
echo 'note: Embedded coordinated normal crewd'
