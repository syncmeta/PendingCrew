import XCTest

/// 机长空闲时被提醒回头看 Todo 账，**直到它给出一份对得上的账才停**（驾驶舱计划 #71）。
///
/// ## 人类的原话与它推翻的东西
///
/// > 不要新给 todo 的时候看。如果机长休眠不干活，pendingcrew 就提醒一次看 todo，
/// > 直到机长确认 都做完了 或者卡在人类这边 明确输出确认应该停止 再停
///
/// 被推翻的是「新 Todo 进来时顺手回头看老账」：**新活进来是最差的触发时刻**，那时机长
/// 最忙，会敷衍地扫一眼就过。而「空闲」是对的时刻 —— 没有别的事跟它抢，而且
/// **空闲本身就意味着机长以为没事干了，那正是该被质问的一刻**。
///
/// ## 为什么确认必须带账，不能是一句「都做完了」
///
/// 现场（2026-09-08，机长自述）：它一整天以为自己在推进，直到人类问、把账拉出来，
/// 才发现 **23 条未完成里有 9 条是「做完了没翻牌」**，其中两条早在 0.1.26 就发出去了。
/// 它自己的结论是：**「如果确认只需要说一句『都做完了』，我今天会毫不犹豫地说出口。」**
///
/// 所以确认这一步要求把**每一条未完成的 #N 都归进一个桶**（在跑 / 卡人类 / 排队），
/// 而且并集必须跟真账本逐个对上。**让敷衍的成本高于真查** —— 这是整个机制唯一的
/// 承重点。归不进桶的那条，正是「其实早就做完了、只是没翻牌」的那条。
///
/// ## 量得到什么、量不到什么
///
/// **量得到**：什么时候提醒、什么时候闭嘴、确认在什么条件下被拒。全是纯判定，真跑。
///
/// **量不到**：机长会不会**认真**去查。这机制只保证「不查就答不上来」，
/// 不保证它答得对 —— 一个决心糊弄的机长仍可以先 `list` 一遍再照抄。
/// 那不是这一层能解决的，别声称它解决了。
final class CaptainTodoSweepTests: XCTestCase {

    private let now = Date(timeIntervalSince1970: 1_757_000_000)
    private let floor: TimeInterval = 10 * 60

    // MARK: - ① 空闲时提醒真的出现

    func testRemindsWhenIdleWithOpenItemsAndNoConfirmation() {
        let decision = CaptainTodoSweep.decide(
            open: .read([3, 7, 12]), confirmation: nil, lastRemindedAt: nil,
            now: now, minimumInterval: floor)
        guard case let .remind(text) = decision else {
            return XCTFail("有 3 条未完成、从没确认过，机长却没被提醒 —— 这就是 #71 要治的那一刻")
        }
        XCTAssertTrue(text.contains("confirm_todo_sweep"),
                      "提醒里没说该怎么让它停 —— 那它就只是噪音")
        XCTAssertTrue(text.contains("3"), "提醒里没有未完成条数，机长看不出规模")
    }

    // MARK: - ② 确认之后不再提醒

    func testSilentAfterAConfirmationThatCoversExactlyTheOpenSet() {
        let confirmation = CaptainTodoSweep.Confirmation(
            confirmedAt: Self.iso(now), openNumbers: [3, 7, 12])
        let decision = CaptainTodoSweep.decide(
            open: .read([3, 7, 12]), confirmation: confirmation, lastRemindedAt: nil,
            now: now.addingTimeInterval(3600), minimumInterval: floor)
        XCTAssertEqual(decision.isSilent, true,
                       "确认过、而且这批一个字没变，还在提醒 —— 那就是一条永远在响的提醒")
    }

    /// 「确认之后多久不再提醒」的答案**不是一个时长，是一个集合**。
    /// 确认覆盖的是**那一批具体条目**；只要那批没变，隔多久都不该再响。
    func testConfirmationDoesNotExpireByTimeAlone() {
        let confirmation = CaptainTodoSweep.Confirmation(
            confirmedAt: Self.iso(now), openNumbers: [3])
        let decision = CaptainTodoSweep.decide(
            open: .read([3]), confirmation: confirmation, lastRemindedAt: nil,
            now: now.addingTimeInterval(30 * 86400), minimumInterval: floor)
        XCTAssertEqual(decision.isSilent, true,
                       "光靠时间就把确认作废了 —— 那机长每隔一阵就要重报一次一模一样的账，正是「永远在响」")
    }

    /// 新 Todo 进来**算重置** —— 但只因为「确认没覆盖到它」，不是因为「来了新活」。
    func testANewTodoReArmsTheReminder() {
        let confirmation = CaptainTodoSweep.Confirmation(
            confirmedAt: Self.iso(now), openNumbers: [3, 7])
        let decision = CaptainTodoSweep.decide(
            open: .read([3, 7, 99]), confirmation: confirmation, lastRemindedAt: nil,
            now: now.addingTimeInterval(60), minimumInterval: floor)
        guard case let .remind(text) = decision else {
            return XCTFail("多了一条 #99 没被任何确认覆盖过，却不提醒了")
        }
        XCTAssertTrue(text.contains("99"), "没点名是哪条没被确认覆盖，机长得自己再扫一遍全账")
    }

    /// 反面：条目**变少**（做完翻牌了）不该重新提醒 —— 那是好事，不是新情况。
    func testFinishingItemsDoesNotReArm() {
        let confirmation = CaptainTodoSweep.Confirmation(
            confirmedAt: Self.iso(now), openNumbers: [3, 7, 12])
        let decision = CaptainTodoSweep.decide(
            open: .read([3]), confirmation: confirmation, lastRemindedAt: nil,
            now: now.addingTimeInterval(60), minimumInterval: floor)
        XCTAssertEqual(decision.isSilent, true,
                       "机长把两条做完翻了牌，反而被提醒了 —— 这会训练它别去翻牌")
    }

    // MARK: - ③ 不许变成背景噪音

    func testNothingOpenMeansNothingToNag() {
        let decision = CaptainTodoSweep.decide(
            open: .read([]), confirmation: nil, lastRemindedAt: nil,
            now: now, minimumInterval: floor)
        XCTAssertEqual(decision.isSilent, true,
                       "一条未完成都没有还要提醒 —— 一条永远报「已知没事」的提醒会训练人忽略整个通道")
    }

    func testDoesNotRepeatWithinTheFloorInterval() {
        let decision = CaptainTodoSweep.decide(
            open: .read([3]), confirmation: nil,
            lastRemindedAt: now.addingTimeInterval(-60),
            now: now, minimumInterval: floor)
        XCTAssertEqual(decision.isSilent, true,
                       "一分钟前刚提醒过又提醒 —— 机长在空闲/忙之间抖一下就会被刷屏")
    }

    func testRepeatsAfterTheFloorInterval() {
        let decision = CaptainTodoSweep.decide(
            open: .read([3]), confirmation: nil,
            lastRemindedAt: now.addingTimeInterval(-(floor + 1)),
            now: now, minimumInterval: floor)
        XCTAssertEqual(decision.isSilent, false,
                       "过了间隔仍然不提醒 —— 人类要的是「直到确认为止」，不是提醒一次就算")
    }

    // MARK: - ④ 确认那一步：承重点

    func testConfirmationMustAccountForEveryOpenItem() {
        let result = CaptainTodoSweep.validate(
            running: [3], blockedOnHuman: [], queued: [], open: [3, 7, 12])
        XCTAssertEqual(result.refusal, .missing([7, 12]),
                       """
                       只交代了 3 条里的 1 条就放行了 —— 剩下那两条正是「早就做完了没翻牌」\
                       最可能藏身的地方（机长自己那 9 条就是这么攒出来的）。
                       """)
    }

    /// **空确认必须被拒**。这条是整个机制的承重点：允许空确认 = 允许一句「都做完了」。
    func testEmptyConfirmationIsRefusedWhenItemsAreStillOpen() {
        let result = CaptainTodoSweep.validate(
            running: [], blockedOnHuman: [], queued: [], open: [3])
        XCTAssertEqual(result.refusal, .missing([3]),
                       "空确认被放行了 —— 那这个机制就退化成一颗可以随口按的确认按钮")
    }

    func testConfirmationWithNothingOpenIsAccepted() {
        let result = CaptainTodoSweep.validate(
            running: [], blockedOnHuman: [], queued: [], open: [])
        XCTAssertNil(result.refusal, "账上真的一条未完成都没有时，空确认是唯一正确的答案")
    }

    func testUnknownNumbersAreRefused() {
        let result = CaptainTodoSweep.validate(
            running: [3, 404], blockedOnHuman: [], queued: [], open: [3])
        XCTAssertEqual(result.refusal, .unknown([404]),
                       "报了一个账上没有（或已完成）的 #N 却放行了 —— 那份账对不上真账本")
    }

    func testOverlappingBucketsAreRefused() {
        let result = CaptainTodoSweep.validate(
            running: [3], blockedOnHuman: [3], queued: [], open: [3])
        XCTAssertEqual(result.refusal, .overlapping([3]),
                       "同一条被同时说成在跑和卡人类 —— 两个桶的数加起来就不再等于未完成数")
    }

    func testAcceptedConfirmationRecordsExactlyWhatWasCovered() throws {
        let result = CaptainTodoSweep.validate(
            running: [7], blockedOnHuman: [3], queued: [12], open: [3, 7, 12])
        let confirmation = try XCTUnwrap(result.confirmation)
        XCTAssertEqual(Set(confirmation.openNumbers), [3, 7, 12],
                       "确认没记下它覆盖了哪些条目 —— 那下次就没法判断「这批变没变」")
    }

    // MARK: - ⑤ 别造第二套督办：这套跟 supervise_after_minutes 的边界

    /// `SupervisionLease` 那套是**按单条计划、按超时**触发的；这一条是**按整本 Todo 账、
    /// 按空闲**触发的。两者共享的是纪律不是代码：**都没有「我知道了 / 顺延」出口**。
    /// 这条断言钉住那个纪律不被偷偷加回来。
    ///
    /// ⚠️ 扫的是**标识符**，注释和字符串字面量都剥掉。第一版只剥了注释，结果被
    /// 提醒正文里那句「这里没有『我知道了』『顺延』这种动作」咬红 —— 那句话正是在
    /// **否认**有这种出口。**一把会咬到「说明自己没有 X」的尺子，量错了面**：
    /// 风险面是 API 上真有这么个参数/分支，不是文案里提到过这个词。
    func testThereIsNoAcknowledgeOrSnoozeWayOut() throws {
        let source = try Self.text(of: "CaptainTodoSweep.swift")
        for forbidden in ["snooze", "acknowledge", "顺延", "已查看", "我知道了"] {
            XCTAssertFalse(
                Self.identifiersOnly(source).lowercased().contains(forbidden.lowercased()),
                """
                判定里出现了「\(forbidden)」这种出口。一旦给了它，这个机制必然退化成\
                「看一眼就算办完」—— 那正是 SupervisionLease 里点名要避免的东西。
                """)
        }
    }

    // MARK: - ⑤b 账本读不出来时**必须提醒**，不许沉默

    /// 机长裁定：**读不到 → 提醒，不要静默。** 理由是这条通道的存在意义就是不让沉默
    /// 发生，它自己却在读失败时沉默 —— 那是自我否定。静默的代价是「账上可能挂着一堆
    /// 而没人知道」，提醒的代价只是多问一句。
    ///
    /// 这个缺口 2026-09-08 第一次报出来时我把成因写错了 —— 写的是「要改 store 的
    /// 返回形状」，而 `MultiProcessJSONStore.LedgerIncident.unreadable` **早就存在**，
    /// 是 `LocalTodoStore.list(crewId:)` 把它压成了 `[]`。**三态压成一个值**，不是
    /// 缺少形状。归错的方向正好会让这条一直排不上（听起来是大改）。
    func testUnreadableLedgerRemindsInsteadOfGoingSilent() {
        let decision = CaptainTodoSweep.decide(
            open: .unreadable, confirmation: nil, lastRemindedAt: nil,
            now: now, minimumInterval: floor)
        XCTAssertEqual(decision.isSilent, false,
                       "账本读不出来却闭嘴了 —— 账上可能挂着一堆，而没有任何人会知道")
    }

    /// 有过确认也不行：确认覆盖的是**某一批具体条目**，而现在根本不知道有哪些条目。
    func testUnreadableRemindsEvenWithAnExistingConfirmation() {
        let confirmation = CaptainTodoSweep.Confirmation(
            confirmedAt: Self.iso(now), openNumbers: [3, 7, 12])
        let decision = CaptainTodoSweep.decide(
            open: .unreadable, confirmation: confirmation, lastRemindedAt: nil,
            now: now.addingTimeInterval(60), minimumInterval: floor)
        XCTAssertEqual(decision.isSilent, false,
                       "拿一份旧确认去盖住一次读失败 —— 那份确认覆盖的是当时那批，现在有哪些条目根本不知道")
    }

    /// 但**地板间隔仍然管用** —— 一本一直读不出来的账不该在每次空闲抖动时都刷屏。
    func testUnreadableStillRespectsTheFloorInterval() {
        let decision = CaptainTodoSweep.decide(
            open: .unreadable, confirmation: nil,
            lastRemindedAt: now.addingTimeInterval(-60),
            now: now, minimumInterval: floor)
        XCTAssertEqual(decision.isSilent, true,
                       "读失败绕过了地板间隔 —— 账一直坏着就会每次空闲都刷一遍")
    }

    func testUnreadableReminderSaysWhatIsWrong() {
        guard case let .remind(text) = CaptainTodoSweep.decide(
            open: .unreadable, confirmation: nil, lastRemindedAt: nil,
            now: now, minimumInterval: floor)
        else { return XCTFail("读不出来时没提醒") }
        XCTAssertTrue(text.contains("读不出来"),
                      "提醒没说清是「读不到」而不是「有 N 条没做」—— 机长会去找一批根本查不到的条目")
        XCTAssertFalse(text.contains("confirm_todo_sweep"),
                       "读不出来时还让机长去 confirm —— 它交不出账，那条建议只会让它撞墙")
    }

    /// 读不出来那段话要**教得动**：它每隔一分钟响一次，而收到它的人这时候
    /// 什么也交不出来。2026-09-12 那次断了 8.5 小时，机长是自己摸出「git 还通、
    /// 先定性、架哨别空等」这条路的 —— 那条路本该写在提醒里。
    func test_读不出来那段话要告诉人这时候能干什么() {
        guard case let .remind(text) = CaptainTodoSweep.decide(
            open: .unreadable, confirmation: nil, lastRemindedAt: nil,
            now: Date(), minimumInterval: 60)
        else { return XCTFail("读不出来时没提醒") }
        XCTAssertTrue(text.contains("git"),
                      "没说 git 还通 —— 收到这条的人会以为整个人被卡住了：\(text)")
        XCTAssertTrue(text.contains("diagnose-data-dir.sh"),
                      "没给定性那一步，人只能凭「界面看起来正常」判，而那恰恰判不了：\(text)")
        XCTAssertTrue(text.contains("别逐分钟重试") || text.contains("哨"),
                      "没说别空等 —— 这条提醒每分钟响一次，不说就是在催人空转：\(text)")
    }

    /// 那段话**不许保证白板上有警示**。
    ///
    /// 原文写的是「群聊白板上**应该**有一条系统警示说明是哪种事故」，而事实相反：
    /// 报事故走 `LocalWhiteboardStore.appendSessionMessage`，append 要先把整份白板
    /// 读一遍，读不了就整条拒写。**整个数据目录读不出来的那种事故里——也就是这条
    /// 提醒最常出现的那种——那条警示必然不存在**，而原文正把人支去找它。
    ///
    /// 2026-09-12 实测撞到：数据目录 EPERM 连续 7 小时，这条提醒按最短间隔一直在响，
    /// 而它让人去看的那条警示一次都没能写进去。
    func test_读不出来那段话不许保证白板上有警示() {
        guard case let .remind(text) = CaptainTodoSweep.decide(
            open: .unreadable, confirmation: nil, lastRemindedAt: nil,
            now: Date(), minimumInterval: 60)
        else { return XCTFail("读不出来时没提醒") }
        XCTAssertFalse(text.contains("白板上应该有"),
                       "又把「白板上应该有一条警示」写死了 —— 数据目录整个读不出来时它必然没有：\(text)")
        XCTAssertTrue(text.contains("写不进去") || text.contains("可能"),
                      "没说清那条警示可能根本不存在：\(text)")
        XCTAssertTrue(text.contains("不等于账本没事"),
                      "没说「白板上没警示 ≠ 账本没事」—— 少了这句，人会把「找不到警示」当成没事：\(text)")
    }

    // MARK: - ⑤c 读失败真的能被这条路看见（不是只在纯逻辑里成立）

    /// **信号一直在，只是被 `list()` 扔了。** 这条钉住新读法真的把它接住了。
    func testStoreReportsUnreadableInsteadOfPretendingEmpty() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("sweep-unreadable-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let store = LocalTodoStore(directory: dir, ledger: .agent)
        let crewId = "crew-unreadable"
        // 造一次**真的打不开**：文件在、内容非空、但没有读权限（EACCES）。
        //
        // 第一版这里用的是「把文件位置占成一个目录」，实测**不红** —— 那种失败在
        // `readDataIfExists` 里被归到「文件不存在」，直接当合法空表返回，一个事故都不报。
        // 取红样本自己不对，而它长得跟「修好了」一模一样：纯逻辑那几条照样绿。
        // ⚠️ 文件名走 `TodoLedger.fileSuffix`，**别自己拼 `.json`** —— 第二版取红样本
        // 就栽在这儿：`<crewId>.json` 是**白板**，agent 那本是 `<crewId>.todos.json`。
        // 结果我造的是一个跟这本账无关的文件，真账本压根不存在 → 合法空表 → 不红。
        // 一个造错了的取红样本，跟「已经修好了」长得一模一样。
        let file = dir.appendingPathComponent("\(crewId)\(TodoLedger.agent.fileSuffix)")
        try Data("[]".utf8).write(to: file)
        try FileManager.default.setAttributes([.posixPermissions: 0], ofItemAtPath: file.path)
        defer { try? FileManager.default.setAttributes(
            [.posixPermissions: 0o644], ofItemAtPath: file.path) }

        XCTAssertEqual(store.read(crewId: crewId), .unreadable,
                       """
                       读失败被压成了「读到了，是空的」。这正是 2026-09-08 那个缺口：\
                       `MultiProcessJSONStore.LedgerIncident.unreadable` 一直在，\
                       只有写路径在用它，读路径把它扔了。
                       """)
    }

    /// 解不开的字节（`.corrupt`）也算「这次读不到可信内容」—— 调用方要的就是这一位，
    /// 不该让它去分辨是哪一种事故。
    func testCorruptBytesAlsoCountAsUnreadable() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("sweep-corrupt-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let store = LocalTodoStore(directory: dir, ledger: .agent)
        let crewId = "crew-corrupt"
        try Data([0xFF, 0xFE, 0x00, 0x01]).write(
            to: dir.appendingPathComponent("\(crewId)\(TodoLedger.agent.fileSuffix)"))
        XCTAssertEqual(store.read(crewId: crewId), .unreadable,
                       "解不开的字节被当成了「读到了，是空的」")
    }

    func testStoreStillReportsRowsWhenReadable() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("sweep-readable-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let store = LocalTodoStore(directory: dir, ledger: .agent)
        let crewId = "crew-readable"
        XCTAssertEqual(store.read(crewId: crewId), .rows([]),
                       "空账本被说成读不出来 —— 那机制会对一本干净的账一直提醒")
        XCTAssertNotNil(store.add(crewId: crewId, text: "一件事"))
        guard case let .rows(rows) = store.read(crewId: crewId) else {
            return XCTFail("加了一条却读不出来")
        }
        XCTAssertEqual(rows.map(\.number), [1])
    }

    // MARK: - ⑤c 报事故那条路自己不许静默失败（三本账同一个形状）

    /// 账本出事时往白板报一行，是这三本账（Todo / 机长任务列表 / codex 审批）
    /// 唯一会留在盘上的痕迹。而**它经常写不进去**：append 要先把整份白板读一遍，
    /// 读不了就整条拒写（2026-08-12 P0 的不变式）。整个数据目录读不出来时账本和
    /// 白板一起瞎，这条警示必然落不了盘。
    ///
    /// 用吞错的 `appendSessionMessage` 的话，它连「没写成」都不说一声 ——
    /// **一条专门用来留痕的东西自己静默失败**，那是这个仓库反复被咬的那种病。
    ///
    /// 名单是**扫出来的不是写死的**：将来多一本账，它自动进这道闸，
    /// 而不是「那张名单上没有它，所以没人管」。
    func test_每一个reportIncident都不许吞掉写失败() throws {
        let found = try Self.sourcesContaining("func reportIncident")
        XCTAssertGreaterThanOrEqual(
            found.count, 3,
            "只扫到 \(found.count) 处 reportIncident —— 三本账至少各有一处，"
            + "少了说明这把尺子自己瞎了（被改名 / 扫不到源码目录），别当成「都合规」")
        for (name, text) in found {
            let body = try XCTUnwrap(Self.bodyOfFunc("reportIncident", in: text),
                                     "\(name)：切不出 reportIncident 的函数体")
            XCTAssertTrue(
                body.contains("appendSessionMessageReportingFailure("),
                "\(name) 的 reportIncident 没用会报错的那一支")
            XCTAssertFalse(
                body.contains("appendSessionMessage("),
                "\(name) 的 reportIncident 还在用吞错的 appendSessionMessage —— "
                + "白板写不进去时它一声不吭，而那正是它唯一该说话的时刻")
        }
    }

    /// 扫 Sources 下所有 .swift，返回含 `needle` 的 (文件名, 全文)。
    ///
    /// `inCodeOnly` 决定**拿什么去筛**：
    /// - `true`（默认）：拿剥掉注释和字符串之后的文本筛 —— 找符号名该这样。
    /// - `false`：拿原文筛 —— **判字符串内容时必须这样**。
    ///
    /// ⚠️ 这个参数是被一次变异测试逼出来的：第一版把要判字符串的那条也走了 `true`，
    /// 于是「白板」只出现在字符串里的文件**一个都没被选中**，那条断言从此永远绿。
    /// **自查照着尺子自己的口径写，就会继承它的盲区。**
    private static func sourcesContaining(
        _ needle: String, inCodeOnly: Bool = true
    ) throws -> [(String, String)] {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources", isDirectory: true)
        guard let walker = FileManager.default.enumerator(
            at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
        else { throw XCTSkip("读不到源码目录") }
        var out: [(String, String)] = []
        for case let url as URL in walker where url.pathExtension == "swift" {
            guard let text = try? String(contentsOf: url, encoding: .utf8),
                  (inCodeOnly ? identifiersOnly(text) : text).contains(needle) else { continue }
            out.append((url.lastPathComponent, text))
        }
        return out.sorted { $0.0 < $1.0 }
    }

    /// 切出一个函数体：从 `func <name>` 起到下一个顶格四空格的 `}` 为止。
    /// 够用就行 —— 切不出来时返回 nil，调用方当场红，不会静默放过。
    private static func bodyOfFunc(_ name: String, in text: String) -> String? {
        guard let start = text.range(of: "func \(name)") else { return nil }
        let rest = text[start.lowerBound...]
        guard let end = rest.range(of: "\n    }") else { return nil }
        return String(rest[..<end.upperBound])
    }

    // MARK: - ⑤d 别再有回执**保证**白板上有那条警示

    /// 报事故那一行**经常写不进去**（append 要先读整份白板，读不了就整条拒写）。
    /// 所以「群聊白板上有一条系统警示」这句话在**整个数据目录读不出来**时是假的 ——
    /// 而那正是这类回执最常出现的场合。把人支去找一个不存在的东西，
    /// 他会得出「那就不是这种事故」的反结论。
    ///
    /// 2026-09-12 一次把 7 处改成共用 `MultiProcessJSONStore.whiteboardNoticeCaveat`。
    /// 这条钉住别再长回来：**扫的是代码，不是注释**（`identifiersOnly` 已经把注释和
    /// 字符串里的引号剥掉；这里改扫原文但排除注释行，因为要判的正是字符串内容）。
    func test_没有回执再保证白板上一定有警示() throws {
        let caveat = MultiProcessJSONStore.whiteboardNoticeCaveat
        XCTAssertTrue(caveat.contains("可能"), "这句话自己就把「一定有」写死了：\(caveat)")
        XCTAssertTrue(caveat.contains("写不进去"), "没说清它可能根本不存在：\(caveat)")
        XCTAssertTrue(caveat.contains("不等于没事"),
                      "没说「没看到 ≠ 没事」—— 少了这句，人会把找不到当成没事：\(caveat)")

        let banned = ["群聊白板上有系统警示", "群聊白板上有一条系统警示",
                      "白板上有一条系统警示", "白板上会有一条系统警示",
                      "群聊白板上应该有一条系统警示"]
        for (name, text) in try Self.sourcesContaining("白板", inCodeOnly: false) {
            let code = Self.linesWithoutComments(text)
            for phrase in banned where code.contains(phrase) {
                XCTFail("\(name) 里还写着「\(phrase)」—— 改用 "
                        + "MultiProcessJSONStore.whiteboardNoticeCaveat，"
                        + "那句话在整个数据目录读不出来时是假的")
            }
        }
    }

    /// 只去掉整行注释与行尾 `//` 之后的部分；字符串字面量要留着（判的就是它）。
    private static func linesWithoutComments(_ text: String) -> String {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .map { line -> Substring in
                guard let slash = line.range(of: "//") else { return line }
                return line[..<slash.lowerBound]
            }
            .joined(separator: "\n")
    }

    // MARK: - ⑥ 装到车上了没有（判定造好了没人调 = 等于不存在）

    func testIdleHookActuallyAsksTheCaptain() throws {
        let runner = Self.identifiersOnly(try Self.text(of: "CrewSessionRunner.swift"))
        XCTAssertTrue(
            runner.contains("remindCaptainToSweepTodos("),
            "空闲钩子里没人调这套判定 —— 零件造好了没装到车上，机长永远不会被问账")
        XCTAssertTrue(
            runner.contains("run.role == .captain"),
            "没限定只问机长 —— 这条是给机长的，不该去打扰 worker")
        XCTAssertTrue(
            runner.contains(".read(crewId:"),
            """
            空闲这条路还在用会把读失败压成空表的读法。信号一直在（LedgerIncident.unreadable），\
            被 list() 扔掉了 —— 这条路必须用能看见它的那个读法。
            """)
    }

    /// **顺序也是需求的一部分**：补投的唤醒和 continue_work 的续跑都是真活，
    /// 它们认领了这个空闲窗口就说明机长并不是「以为没事干了」。核账必须排在最后。
    func testTheSweepIsTheLastThingTriedOnIdle() throws {
        let runner = Self.identifiersOnly(try Self.text(of: "CrewSessionRunner.swift"))
        guard let idle = runner.range(of: "func runBecameIdle"),
              let sweep = runner.range(of: "remindCaptainToSweepTodos(", range: idle.upperBound..<runner.endIndex),
              let continuation = runner.range(of: "continuationStore.takeReady", range: idle.upperBound..<runner.endIndex)
        else { return XCTFail("runBecameIdle 里的锚点找不齐 —— 先修测试") }
        XCTAssertLessThan(
            continuation.lowerBound, sweep.lowerBound,
            "核账排在了 continue_work 续跑之前 —— 那会打断一个正要接着干活的机长")
    }

    func testTheConfirmToolIsCaptainOnlyAndReachable() throws {
        let mcp = try Self.text(of: "McpServer.swift")
        XCTAssertTrue(mcp.contains("\"confirm_todo_sweep\""),
                      "MCP 没暴露 confirm_todo_sweep —— 机长没有任何办法让提醒停下来")
        guard let handler = mcp.range(of: "case \"confirm_todo_sweep\":") else {
            return XCTFail("找不到 confirm_todo_sweep 的 handler")
        }
        let body = String(mcp[handler.upperBound...].prefix(400))
        XCTAssertTrue(body.contains("guard isCaptain"),
                      "confirm_todo_sweep 没有机长门禁 —— 这本账是机长的责任，worker 不该替它销账")
        XCTAssertTrue(body.contains("CaptainTodoSweep.validate"),
                      "handler 没走那套校验 —— 那它就是一颗可以随口按的确认按钮")
    }

    /// 工具说明里那段「为什么不能只说一句都做完了」**必须留着**。
    /// 机长读到的只有这段文字；把理由删成一句「请逐条归桶」，它就只剩一条没来由的
    /// 形式要求 —— 而没来由的形式要求正是最先被绕过的东西。
    func testTheToolStillCarriesTheReasonNotJustTheRule() throws {
        let mcp = try Self.text(of: "McpServer.swift")
        XCTAssertTrue(mcp.contains("confirmSweepDescription"),
                      "工具说明被内联回去了，改一次就容易把理由一起删掉")
        XCTAssertTrue(mcp.contains("做完了没翻牌"),
                      "工具说明里那段「23 条里 9 条其实做完了没翻牌」的由来被删了")
    }

    // MARK: - 小工具

    private static func iso(_ date: Date) -> String {
        ISO8601DateFormatter().string(from: date)
    }

    /// 只留标识符那一层：先剥多行字符串、再剥普通字符串、最后剥注释。
    private static func identifiersOnly(_ text: String) -> String {
        var out = text
        while let open = out.range(of: "\"\"\""),
              let close = out.range(of: "\"\"\"", range: open.upperBound..<out.endIndex) {
            out.replaceSubrange(open.lowerBound..<close.upperBound, with: "\"\"")
        }
        out = out.split(separator: "\n", omittingEmptySubsequences: false)
            .map { line -> String in
                let noComment = line.range(of: "//").map { String(line[..<$0.lowerBound]) }
                    ?? String(line)
                var kept = ""
                var inString = false
                for ch in noComment {
                    if ch == "\"" { inString.toggle(); continue }
                    if !inString { kept.append(ch) }
                }
                return kept
            }
            .joined(separator: "\n")
        return out
    }

    private static func text(of fileName: String) throws -> String {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources", isDirectory: true)
        guard let walker = FileManager.default.enumerator(
            at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
        else { throw XCTSkip("读不到源码目录") }
        for case let url as URL in walker where url.lastPathComponent == fileName {
            return try String(contentsOf: url, encoding: .utf8)
        }
        throw XCTSkip("找不到源码文件 \(fileName)")
    }
}
