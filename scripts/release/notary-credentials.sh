#!/bin/sh
# 公证凭据的**唯一**解析处。点进来用（`. "$root/scripts/release/notary-credentials.sh"`），
# 它定义两个函数：
#   · notary_resolve   —— 挑一份凭据出来，并且**当场证明它真能认证**
#   · notary_submit <文件>  —— 用挑中的那份提交公证并等结果
#
# ## 为什么不再钉死在钥匙串 profile 上
#
# 原来两个脚本各写一行 `--keychain-profile "$PENDING_NOTARY_PROFILE"`，且用
# `:?` 把它当硬前提。2026-09-13 发 0.1.37 时它**凭空不见了**：
# `notarytool history --keychain-profile pendingcrew-notary` 回
# `No Keychain password item found`，而同一把 profile 前一天 15:18Z 刚公证过
# 0.1.36（`notarytool history` 里那条 Accepted 还在）。钥匙串条目是本机状态，
# 会没、而且没的时候不留痕迹。
#
# 而那把 profile 本来就只是 App Store Connect API key 的一层缓存 —— 本文件顶上
# 那段注释自己写着「用本机那把 App Store Connect API key 建的」。所以直接用 key
# 不是降级，是少绕一层。
#
# ## 挑选顺序（先显式，后约定）
#
#   ① `PENDING_NOTARY_KEY` / `PENDING_NOTARY_KEY_ID` / `PENDING_NOTARY_ISSUER`
#   ② `~/.appstoreconnect/pendingbot.env`（`ASC_KEY_PATH` / `ASC_KEY_ID` /
#      `ASC_ISSUER_ID`，600；iOS 防倒退闸用的也是这份）
#   ③ `PENDING_NOTARY_PROFILE` 钥匙串 profile（老路，仍然支持）
#
# ## 这里会先认证一次，而不是等构建完再发现
#
# 公证是整条链路的**倒数第二步**：凭据坏了的代价是白构建十几分钟。所以
# `notary_resolve` 会拿挑中的凭据跑一次 `notarytool history`（只读、几秒）。
# **这道闸此刻就有一份现成的红样本**：`--keychain-profile pendingcrew-notary`
# 今天就是不认证的 —— 不需要造实验去证明它会响。

notary_mode=
notary_key=
notary_key_id=
notary_issuer=
notary_profile=

notary_resolve() {
  notary_mode=
  if [ -n "${PENDING_NOTARY_KEY:-}" ]; then
    notary_mode=key
    notary_key=$PENDING_NOTARY_KEY
    notary_key_id=${PENDING_NOTARY_KEY_ID:?PENDING_NOTARY_KEY 给了就必须同时给 PENDING_NOTARY_KEY_ID}
    notary_issuer=${PENDING_NOTARY_ISSUER:?PENDING_NOTARY_KEY 给了就必须同时给 PENDING_NOTARY_ISSUER}
  elif [ -f "$HOME/.appstoreconnect/pendingbot.env" ]; then
    # 只取这三个键，不 source 整个文件（那等于让一份 600 的配置文件执行任意代码）。
    notary_key=$(sed -n 's/^ASC_KEY_PATH=//p' "$HOME/.appstoreconnect/pendingbot.env" | tail -1)
    notary_key_id=$(sed -n 's/^ASC_KEY_ID=//p' "$HOME/.appstoreconnect/pendingbot.env" | tail -1)
    notary_issuer=$(sed -n 's/^ASC_ISSUER_ID=//p' "$HOME/.appstoreconnect/pendingbot.env" | tail -1)
    # 路径里可能写着 `~`，sh 不会替你展开。
    case $notary_key in "~/"*) notary_key=$HOME/${notary_key#"~/"} ;; esac
    if [ -n "$notary_key" ] && [ -n "$notary_key_id" ] && [ -n "$notary_issuer" ] \
      && [ -f "$notary_key" ]; then
      notary_mode=key
    fi
  fi
  if [ -z "$notary_mode" ] && [ -n "${PENDING_NOTARY_PROFILE:-}" ]; then
    notary_mode=profile
    notary_profile=$PENDING_NOTARY_PROFILE
  fi
  [ -n "$notary_mode" ] || {
    echo "找不到任何公证凭据。三条路任选一条：" >&2
    echo "  · export PENDING_NOTARY_KEY=<.p8 路径> PENDING_NOTARY_KEY_ID=… PENDING_NOTARY_ISSUER=…" >&2
    echo "  · 写好 ~/.appstoreconnect/pendingbot.env（ASC_KEY_PATH/ASC_KEY_ID/ASC_ISSUER_ID）" >&2
    echo "  · export PENDING_NOTARY_PROFILE=<notarytool store-credentials 建的 profile 名>" >&2
    return 2
  }

  # 先认证一次再往下走 —— 便宜，而且它挡的是「构建十几分钟之后才发现凭据是坏的」。
  if notary_run history >/dev/null 2>&1; then
    if [ "$notary_mode" = key ]; then
      echo "note: 公证凭据 = App Store Connect API key（key-id ${notary_key_id}）"
    else
      echo "note: 公证凭据 = 钥匙串 profile ${notary_profile}"
    fi
    return 0
  fi

  echo "公证凭据认证不过（mode=${notary_mode}），拒绝开始构建。" >&2
  notary_run history >&2 2>&1 | head -5
  return 2
}

# 内部用：把挑中的凭据参数补上去跑 notarytool。
notary_run() {
  subcommand=$1
  shift
  if [ "$notary_mode" = key ]; then
    xcrun notarytool "$subcommand" "$@" \
      --key "$notary_key" --key-id "$notary_key_id" --issuer "$notary_issuer"
  else
    xcrun notarytool "$subcommand" "$@" --keychain-profile "$notary_profile"
  fi
}

notary_submit() {
  notary_run submit "$1" --wait
}
