#!/bin/sh
# 发版闸门 / 合前基线：在一棵**钉死的** worktree 上跑一趟真口径全量，
# 并顺带证明「跑的就是那棵树」。
#
# 两个入口，同一份脚本 —— **别分叉成两份**：两份会各自漂，而漂了的那天
# 没有任何读数会报警（发版那趟和日常那趟给出不同结论时，没人知道该信哪个）。
#   · 发版闸门：     sh scripts/release-gate.sh <要发的 commit>
#   · 日常合前基线： sh scripts/release-gate.sh $(git -C <仓库> rev-parse HEAD)
#
# 跑完的现场全在 /tmp/pcw-<commit>-log/：三份 .log ＋ **一份 .xcresult**（失败用例名在里面）。
# xcresult 是 2026-09-12 才补上的 —— 在那之前这道闸门只留 .log，而
# **日常跑红了可以再跑一次；发版闸门红了、日志又丢了，你面对的是一个已经开始的
# 发布流程和一条查不出来的红。**
#
# 判读（跑完读这六样，不需要任何事前判据）：
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
#      **已知的第四种成因（2026-09-12 实测到）**：人的数据目录读不出来时，
#      `CrewMentionFilterRealWhiteboardTests` 那 3 条也会 skip（它们读真白板）。
#      那**不是回归，也不是通过** —— 那几条自己的 skip 文案就写着「别把这次 skip
#      当成通过」。它是一条免费的横向对照：跟着这一趟量到了机器当下的故障。
#   ② CrewChatOpenCostTests 那 8 条应为 Executed —— 若整套 skipped 说明 fixture 没拷进去
#   ③ 具名失败必须为空 —— 汇总行只说红了几条，不说是哪一条
#   ④ HEAD / TREE 前后逐字相同 —— 否则你测的不是你以为的那棵树
#   ⑤ 文档引用腐烂名单应为空 —— 逐条列「文档:行 → 被引用的 path:N」，
#      判据只有「行号越界」和「文件不存在」两条，都不需要读懂那一行写了什么。
#      **红了看名单，不是看那个数**：数只说明红了几条，说不出是哪一条烂的。
#      同一段还会先列一份**读数**：声明了 `doc-ref-base` 的文档各落后 main 多少。
#      那是读数不是判据 —— 不设阈值、不影响退出码。快照文档落后是正常的、应该的；
#      **一份新文档声明了很旧的 base，那个数自己会刺眼** —— 它挡的是「声明个老 base 躲开尺子」。
#   ⑥ 工程漂移应为「无漂移」——**这一条是唯一会改退出码的**，理由见文件末尾那段。
#      它问的是：提交进来的 pbxproj，是不是「`.xcodegen-version` 钉住的那版生成器
#      + 这棵树的源码」生成出来的那一份。加了 Swift 文件没 regen、裸跑了别的版本的
#      xcodegen、手改了 pbxproj —— 都在这里红。
#      **它有三种结果，不是两种**：无漂移 / 有漂移 / **判不了**（生成器没跑起来，
#      比如离线）。判不了**不算红** —— 一把在离线时报红的尺子会被人关掉，
#      那就等于没有。
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
# 这一趟自己的 DerivedData。**给它显式路径不是为了隔离，是为了「知道 xcresult 在哪」**。
# 不指定时产物落进共享 DerivedData，多条线并跑时「最近那个 xcresult」是谁的**并不确定**；
# 在那儿猜一个比不归档更坏 —— **它会让人拿着别人的日志去查自己的红**。
# 指定之后这个猜测整个消失：这个目录底下的 xcresult 只可能是这个 commit 这一趟的。
#
# 位置选在 $LOG 底下（不是 $WT 底下）是**刻意的**：worktree 里多一个未跟踪目录会被
# `status --porcelain -uall` 数进去，读数 ④ 的前后指纹就必不相等 ——
# `.test-data-root` 2026-09-09 就是这么把 ④ 弄成过报的，别再踩第二次。
DD="$LOG"/dd
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
# 基线口径（2026-09-09 收窄过一次，理由在下面「为什么不再 --ignored 全收」）：
#   * 不能用 `git diff` —— 它定义上看不见未跟踪/被忽略的文件。
#   * 也不能用 `--untracked-files=all` —— 那份 fixture 不是「未跟踪」，是 **.gitignore:27 里被忽略的**，
#     而 `-uall` 不列 ignored。实测：`-uall` 2 行、Fixtures 命中 0；`--ignored` 10 行、命中 1。
#   * 不要再加 `-uall` 展开 —— 那会变成 89 行、随构建和别人的 worktree 抖动，指纹永远不相等。
#     折叠正是我们要的：要判的是「那个 fixture 目录在不在」，不是里面有几个文件。
# 为什么非要盖住它：那份 fixture 决定 CrewChatOpenCost 那 8 条跑还是 skip —— 尺子瞎的地方
# 恰好是它唯一被指望看清的地方。（`cp -R` 必须在取基线之前，否则前后两次不相等。）
#
# ⚠️ **为什么不再 `--ignored` 全收**（2026-09-09，这道读数当天就被自己人弄成过报的）：
# 我那笔进程级数据隔离引入了 `.test-data-root/` —— gitignored、**测试跑完才被创建**。
# 于是全新 worktree 上：before 没有它、after 有它 ⇒ 读数 ④ **必不相等**；
# 而同一个 worktree 复跑反而相等。**它只在「第一次跑某个 commit」时报错，
# 而那正是闸门唯一被用到的场合。**
#
# 修法选「口径」而不是「再删一次」：后者依赖「记得在正确的位置删」这种顺序假设，
# 而 `.test-data-root` 不会是最后一个测试副产物（`--fetch` 的 `.tools/` 已经是第二个，
# 只是它跑在 after 之后、一趟内影响不到 ④ —— 那是运气，不是设计）。
# **排一个名字治一次，换口径治一类。**
#
# 现在的口径 = **tracked 改动 + 未跟踪文件**（`-uall` 展开到文件级），
# 不含 ignored。它仍然回答读数 ④ 要问的那件事：「你测的还是你以为的那棵树吗」——
# 构建产物、隔离数据根、工具缓存都是 ignored，本来就不属于「那棵树」。
#
# **代价说清楚**：那份被 .gitignore 挡住的 chat fixture（决定 CrewChatOpenCost 那 8 条
# 跑还是 skip）**不再进指纹**。所以下面单列一行显式报它在不在 —— 把「盖住它」
# 从指纹里挪成一条明写的读数，而不是悄悄丢掉。
fixture_before=$([ -d "$WT/Tests/PendingCrewTests/Fixtures" ] && echo present || echo absent)
before_diff=$(git -C "$WT" status --porcelain -uall | shasum | cut -c1-12)
# ── 进程级数据隔离（2026-09-09）────────────────────────────────────────────
# 整趟测试的**数据根**挪出人的 `~/Library/Application Support/PendingCrew`，
# 让测试进程根本够不着它。
#
# **隔离本身不在这个脚本里**，在 `project.yml` 的 scheme 环境变量
# （`PENDINGCREW_DATA_DIR: $(SRCROOT)/.test-data-root`）。
#
# ⚠️ **别把它挪回脚本里 export，那样是无效的** —— 实测过：
#   `PENDINGCREW_DATA_DIR=/tmp/x xcodebuild test ...`  → 隔离断言照样红
#   `TEST_RUNNER_PENDINGCREW_DATA_DIR=/tmp/x ...`      → 同样红
# xcodebuild 不把任意 shell 环境变量转给 xctest 进程（这个 bundle 是 standalone、
# 没有 test host，`TEST_RUNNER_` 那条是给有 host 的 runner 的）。能到达它的只有
# scheme：生成的 TestAction 带 `shouldUseLaunchSchemeArgsEnv = YES`。
#
# 好处是连**在 Xcode 里点运行**也隔离，不只是这个入口。
# `TestProcessDataRootIsolationTests` 会断言它真的生效 —— 没生效就红。
#
# 这跟「测试里别忘了给 store 注入 temp dir」那把尺子是两层：
# 那层是**别写错**，这层是**就算写错了也伤不到**。
# 清掉上一趟留下的隔离数据根（纯清理 —— 它已经**不进指纹**了，见上面的口径说明）。
rm -rf "$WT/.test-data-root"
# ─────────────────────────────────────────────────────────────────────────

xcodebuild -project "$WT/PendingCrew.xcodeproj" -scheme PendingCrew -destination 'platform=macOS' -derivedDataPath "$DD" test  > "$LOG"/t-mac.log 2>&1 || true
# 归档 xcresult。**必须紧跟在 test 后面**：下面两趟 build 要是挂住或被人打断，
# 这一趟的失败用例名也已经落盘了。照 scripts/test-mac.sh 的形状（带时间戳的名字），
# 不发明第二种 —— 同一个 commit 跑第二趟不会把第一趟的现场盖掉。
xcresult=$(ls -td "$DD"/Logs/Test/*.xcresult 2>/dev/null | head -1)
if [ -n "$xcresult" ]; then
  # 报出去的必须是**真写下去的那个路径本身**，不是照规则再拼一次 ——
  # 拼的那份平时都对，只在你真要照着去找的时候错。
  archived="$LOG/$(date -u +%Y%m%dT%H%M%SZ).xcresult"
  cp -R "$xcresult" "$archived"
  echo "xcresult 已归档 → $archived"
else
  echo "⚠️ 这一趟没找到 xcresult —— **归档是空的，别把它当成「查得到」**（$DD/Logs/Test 下没有）"
fi
xcodebuild -project "$WT/PendingCrew.xcodeproj" -scheme PendingCrew -destination 'platform=macOS' -derivedDataPath "$DD" build > "$LOG"/b-mac.log 2>&1 || true
xcodebuild -project "$WT/PendingCrew.xcodeproj" -scheme PendingCrew -destination 'generic/platform=iOS Simulator' -derivedDataPath "$DD" build > "$LOG"/b-ios.log 2>&1 || true
# 三趟都跑完、xcresult 也归档了 ⇒ **DerivedData 可以扔了**。
#
# 改这个决定是因为量到了数（2026-09-12 08:45）：**每趟 2.1 GB，而真正要留的
# xcresult 只有 133 MB —— 16 倍**。第一版为了「同一个 commit 复跑能热启动」留着它，
# 那个好处一年用不上几次，而这道闸门每次发版、每次合前基线都跑一趟，
# 两趟就 4.3 GB。**留下的是失败用例名，不是编译中间产物。**
#
# 三份 .log ＋ drift.log 已经把 xcodebuild 的全部输出接住了，所以扔掉它不会让
# 任何一条读数变得查不了。
rm -rf "$DD"
after_head=$(git -C "$WT" rev-parse HEAD)
after_diff=$(git -C "$WT" status --porcelain -uall | shasum | cut -c1-12)
fixture_after=$([ -d "$WT/Tests/PendingCrewTests/Fixtures" ] && echo present || echo absent)
echo "HEAD $before_head -> $after_head   (必须逐字相同)"
echo "TREE $before_diff -> $after_diff   (必须逐字相同；tracked+untracked，不含 ignored)"
echo "FIXTURE $fixture_before -> $fixture_after   (present=CrewChatOpenCost 那 8 条真跑；absent=它们 skip)"
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
# 文档引用腐烂：对**这棵钉死的树**跑（"$WT"），不是对共享目录跑 ——
# 共享目录随时在动，在那儿量出来的读数说不清是哪个 commit 的。
# `|| true`：闸门只报读数、不代人做判断（它自己也从不因为任何一条红而早退）。
echo "--- 文档引用腐烂（名单即计数；空=零条）---"
sh "$WT/scripts/doc-ref-check.sh" "$WT" || true
# ⑥ 工程漂移。**位置很讲究：必须在上面 after_diff 取完之后** —— 它会重新生成
# pbxproj，在取指纹之前跑就会把读数 ④ 弄脏（而 ④ 正是用来证明「你测的就是这棵树」的）。
#
# 为什么本机要有这条：完整的漂移判据本来只在 CI 的 `project-drift` job 上跑，
# 而它挂在 push / PR 上 —— 这台机器上 main 常年领先 origin（2026-09-08 实测过一次
# 是 27 笔未推），**那道闸平时根本没看过我们的代码**。2026-09-08 有人连续四次裸跑
# `xcodegen generate` 绕过 scripts/gen-project.sh，本地没有任何检查会为它红。
#
# 三种结果，别压成两种：**「判不了」不算「有漂移」**。生成器要联网下载
# （`--fetch` 按 scripts/xcodegen-checksums.txt 校验），离线是常事；
# 把离线报成红，这把尺子会在两周内被人注释掉，那就等于没有。
echo "--- 工程漂移（pbxproj == 钉住的生成器产物？）---"
drift_rc=0
if sh "$WT/scripts/gen-project.sh" --fetch > "$LOG"/drift.log 2>&1; then
  if git -C "$WT" diff --quiet -- PendingCrew.xcodeproj; then
    echo "  ✅ 无漂移"
  else
    echo "  ❌ 有漂移：pbxproj 和这棵树的 project.yml/源码对不上"
    git -C "$WT" diff --numstat -- PendingCrew.xcodeproj | sed 's|^|    |'
    echo "    修法：跑 scripts/gen-project.sh（**不是裸 xcodegen generate**），把 pbxproj 一起提交。"
    drift_rc=1
  fi
else
  echo "  ⚠️ 判不了：生成器没跑起来（离线 / 校验和对不上都算），日志 $LOG/drift.log"
  echo "     **这不算漂移**，退出码不变 —— 别把「没量成」读成「量到了没事」，也别读成红。"
fi
# 还原：$WT 是闸门自己建的一次性钉死 worktree，不是共享工作树，这里还原是安全的
# （共享树里还原 pbxproj 会拆掉别人的桥 —— 那条禁令针对的是共享树）。
# 不还原的话，同一个 commit 跑第二趟时 before_diff 会从一棵脏树上取，读数 ④ 就变味了。
git -C "$WT" checkout -- PendingCrew.xcodeproj 2>/dev/null || true
echo "--- 闸门自己留下的（不自动回收）---"
echo "本趟：$WT 和 $LOG"
# 清单和计数出自同一次 `ls` —— 数是从名单里数出来的，两者结构上不可能对不上。
# （报「N 份」却另起一路去数，正是把名单和计数分家；分了家，错的通常是名单。）
ls -d /tmp/pcw-* 2>/dev/null | grep -v -- '-log$' | sed 's|^|  |'
printf '共 %s 趟，合计 %s（含各自的 -log 目录）\n' \
  "$(ls -d /tmp/pcw-* 2>/dev/null | grep -v -- '-log$' | wc -l | tr -d ' ')" \
  "$(du -shc /tmp/pcw-* 2>/dev/null | tail -1 | awk '{print $1}')"
echo "  合计里的大头是每趟归档的 xcresult（-log/*.xcresult，约 130 MB/趟）——"
echo "  DerivedData 跑完就扔了（它是那 130 MB 的 16 倍，留着不值）。"
echo "  只报不删：清不清、什么时候清是仓库主人的事。"
echo "  也只说闸门自己这一堆 —— 本机别处还有 worktree，不在此列。"
echo "  另有一类更该管的：注册比目录活得久 —— worktree 建在会被回收的临时目录里"
echo "  （比如某个 session 的 scratchpad），目录没了、git worktree list 里那条还挂着。"
echo "  那不是占地方，是一条会骗人的登记。清它：git worktree prune（本脚本不替你跑）"
# 退出码 = 只有「有漂移」会让它非零。
#
# 为什么这一条能改退出码，而上面五条都不能：**前五条要人看**（skip 比的是构成不是
# 数字、腐烂名单要逐条读、那 8 条跑没跑要看字面），机器判不了，所以闸门只报读数、
# 从不早退。**⑥ 不一样：它是 `git diff` 的二值结果，没有需要人权衡的余地**，
# 而且它红的时候，上面那趟测试其实是在一个跟源码对不上的工程上跑的。
#
# （原本这里是一句裸 `true`，理由是末行 grep 两个词都没命中会返回 1、让退出码
# 非零误导人。那个理由仍然成立，所以这里显式 exit 一个自己算出来的值，
# 而不是让它裸奔到末尾去捡上一条命令的退出码。）
exit "$drift_rc"
