#!/bin/sh
# 发版闸门 / 合前基线：在一棵**钉死的** worktree 上跑一趟真口径全量，
# 并顺带证明「跑的就是那棵树」。
#
# 两个入口，同一份脚本 —— **别分叉成两份**：两份会各自漂，而漂了的那天
# 没有任何读数会报警（发版那趟和日常那趟给出不同结论时，没人知道该信哪个）。
#   · 发版闸门：     sh scripts/release-gate.sh <要发的 commit>
#   · 日常合前基线： sh scripts/release-gate.sh $(git -C <仓库> rev-parse HEAD)
#
# 判读（跑完读这四样，不需要任何事前判据）：
#   ① skip **不比数字，比构成** —— 逐条看这三条各自还在不在、成立条件还成不成立：
#        · CrewLastMessageCacheTests.test_基准_现场白板目录         —— 未指定现场白板目录 → skip
#        · SessionAwaitingReplyInputsCacheTests.test_基准_现场目录  —— 同上
#        · AgentTuiFixtureRecorder.testRecord                       —— 要花订阅额度、要联网，刻意 skip
#      条目还在、条件还成立 ⇒ 没有回归，**不管总数是几**。
#      为什么这里不写死一个数：那个数是**这台机器、这一刻**的属性，不是仓库的属性。
#      **在这台机器上、这一刻是 3；换台没 git 的机器最多 41 —— 跑出 41 不是回归**
#      （另外 38 条是 `XCTSkip("git not on PATH")`，2026-08-26 在本仓库数得；
#       它同样会随用例增删漂，所以它是佐证，不是判据）。
#      读到不符：先看那三条各自还在不在，别默认是回归，
#      更别为了「凑回某个数」去拆掉一条刻意的 skip。
#   ② CrewChatOpenCostTests 那 8 条应为 Executed —— 若整套 skipped 说明 fixture 没拷进去
#   ③ 具名失败必须为空 —— 汇总行只说红了几条，不说是哪一条
#   ④ HEAD / TREE 前后逐字相同 —— 否则你测的不是你以为的那棵树
set -e
REPO=/Users/hey/Untitled/Pendingname/PendingCrew
COMMIT="$1"; [ -n "$COMMIT" ] || { echo "用法: sh release-gate.sh <commit>"; exit 2; }
# 开跑前先断言：这个 commit 真的在 main 上。
# 现场：2026-08-26 有人在共享目录 `git checkout -b` 开了分支，`git log -1` 读到的
# 是分支 head，差一点被当成发版提交。**闸门那四条读数一条都不会报警** —— 它们量的是
# 「这棵树跑得对不对」，不是「这棵树是不是那棵树」。
git -C "$REPO" merge-base --is-ancestor "$COMMIT" main || {
  echo "✋ $COMMIT 不在 main 上（共享目录当前分支：$(git -C "$REPO" rev-parse --abbrev-ref HEAD)）"; exit 3; }
WT=/tmp/pcw-$COMMIT
LOG=/tmp/pcw-$COMMIT-log   # 日志带 commit：两个人同时跑不会互相冲掉读数
mkdir -p "$LOG"
# 幂等：worktree 已在就复用（我们不删 worktree），否则同一 commit 跑第二趟会因为
# `add` 报错 + set -e 当场早退，而那个报错跟测试毫无关系
[ -d "$WT" ] || git -C "$REPO" worktree add --detach "$WT" "$COMMIT"
# 本脚本**不收尾**：每跑一次留下 /tmp/pcw-<commit>/ 和 /tmp/pcw-<commit>-log/，
# 并在共享仓库里注册一条 worktree。跑 N 次就有 N 份，**不会自己回收**。
# 清理是仓库主人的事，脚本不代劳（跑完的现场是可复查的资产，删了就查不了）；
# 也别顺手 remove 掉别人那条 —— 你不知道谁还在读它的日志。
# 当前有多少、占多少，**自己查**（这里不写死数字：写死的那一刻起它就在过期）：
#   git worktree list | grep /tmp/pcw
#   du -sh /tmp/pcw-* 2>/dev/null      # 要 -h：macOS 的 `du -s` 默认是 512 字节块，当 KB 读会翻一倍
# fixture 在哪、叫什么，**这里一个字都不写死** —— 仓库里唯一知道它的是
# scripts/make-chat-fixtures.sh 那行 `DEST=`，从那儿接，路径知识就只有一份。
# 写死第二份的代价不是「重复」，是**两份会各自漂，而漂掉的那天没有任何读数会报警**：
# 拷贝会静默失败，然后那 8 条变成 skip，看起来只是「少了几条」。
# 所以接不出来就当场停（exit 4），宁可报「它的 DEST= 变了」，也不要拿陈旧路径去拷。
FIX_REL=$(sed -n 's|^DEST="$HERE/\.\./\(.*\)"$|\1|p' "$REPO/scripts/make-chat-fixtures.sh" | head -1)
[ -n "$FIX_REL" ] || { echo "✋ 接不出 fixture 路径：scripts/make-chat-fixtures.sh 的 DEST= 那行变了，先去看它"; exit 4; }
FIX_DIR=${FIX_REL%/*}   # 那个 crew 目录的上一级 = Fixtures 本身（整个拷，将来多一份 fixture 也带上）
# fixture 不入 git（.gitignore），不拷则 CrewChatOpenCost 那 8 条全 skip、基线就变成两个数。
# `|| true` 是刻意的，不是疏忽：拷贝失败不早退，它会在下游以「那 8 条变成 skip」显形，
# 那是一个可识别的读数。改成让 set -e 生效，等于把「看得出哪儿错了」换成一次早退。
cp -R "$REPO/$FIX_DIR" "$WT/${FIX_DIR%/*}/" 2>/dev/null || true
before_head=$(git -C "$WT" rev-parse HEAD)
# 基线用 `status --porcelain --ignored`，三条都别改：
#   * 不能用 `git diff` —— 它定义上看不见未跟踪/被忽略的文件。
#   * 也不能用 `--untracked-files=all` —— 那份 fixture 不是「未跟踪」，是 **.gitignore:27 里被忽略的**，
#     而 `-uall` 不列 ignored。实测：`-uall` 2 行、Fixtures 命中 0；`--ignored` 10 行、命中 1。
#   * 不要再加 `-uall` 展开 —— 那会变成 89 行、随构建和别人的 worktree 抖动，指纹永远不相等。
#     折叠正是我们要的：要判的是「那个 fixture 目录在不在」，不是里面有几个文件。
# 为什么非要盖住它：那份 fixture 决定 CrewChatOpenCost 那 8 条跑还是 skip —— 尺子瞎的地方
# 恰好是它唯一被指望看清的地方。（`cp -R` 必须在取基线之前，否则前后两次不相等。）
before_diff=$(git -C "$WT" status --porcelain --ignored | shasum | cut -c1-12)
xcodebuild -project "$WT/PendingCrew.xcodeproj" -scheme PendingCrew -destination 'platform=macOS' test      > "$LOG"/t-mac.log 2>&1 || true
xcodebuild -project "$WT/PendingCrew.xcodeproj" -scheme PendingCrew -destination 'platform=macOS' build     > "$LOG"/b-mac.log 2>&1 || true
xcodebuild -project "$WT/PendingCrew.xcodeproj" -scheme PendingCrew -destination 'generic/platform=iOS Simulator' build > "$LOG"/b-ios.log 2>&1 || true
after_head=$(git -C "$WT" rev-parse HEAD)
after_diff=$(git -C "$WT" status --porcelain --ignored | shasum | cut -c1-12)
echo "HEAD $before_head -> $after_head   (必须逐字相同)"
echo "TREE $before_diff -> $after_diff   (必须逐字相同；含被忽略的 fixture)"
echo "--- 汇总 / 结论 ---"; grep -E "Executed [0-9]{3,} tests, with|TEST SUCCEEDED|TEST FAILED" "$LOG"/t-mac.log | tail -3
echo "--- 具名失败（空=零条）---"; grep -E "' failed \(" "$LOG"/t-mac.log || true
echo "--- 那 8 条跑了没（最要紧）---"
grep -A1 "Test Suite 'CrewChatOpenCostTests' started" "$LOG"/t-mac.log | head -2
# 下面这行不是「记得去看什么」，是**你看到什么就说明什么**：
#   看到 `Test Case ... started` = 那 8 条真跑了；
#   看不到 / 整套 skipped        = 上面那次 `cp -R` 没生效，那 8 条退化成了 skip。
# 差别就在这两行输出的字面上，不需要谁事先记住一个期望值再回来比对。
echo "# 看到 Test Case ... started = 真跑了；看不到/整套 skipped = cp -R 没生效，那 8 条会变成 skip"
echo "--- 两端 build ---"; grep -E "BUILD SUCCEEDED|BUILD FAILED" "$LOG"/b-mac.log "$LOG"/b-ios.log
true  # 末行 grep 若两个词都没命中会返回 1，让脚本退出码非零、误导看 $? 的人
