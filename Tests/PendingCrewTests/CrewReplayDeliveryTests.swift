#if os(macOS)
import XCTest

/// 群聊消息**重放**（人类 Todo #105）—— 判据：**同一条消息对同一个对话只投递一次**。
///
/// 出处是 2026-09-06 的实测（`docs/internal/2026-09-06-whiteboard-replay-delivery-audit.md`）：
/// 一条已经被回过、并且当面纠正过的消息被原封不动再送一遍，收的人不核账本就会
/// **按一条已经作废的事实行动**。所以这不只是噪音 —— 重放送的是消息的「最初形态」，
/// 不是最新形态。
///
/// 这里钉的是**产品行为**（一份 prompt / 一次注入里同一条消息出现几次），不是内部
/// 返回值 —— 因为收的人看到的就是那份 prompt。
///
/// ## 为什么两条都包在 `XCTExpectFailure` 里
///
/// R1 已于 2026-09-07 修好、包装已拆；R2 还包着（它排在第三批）。原来的理由：
/// 包起来不是把红藏掉：`XCTExpectFailure` 是严格的 —— 一旦这个行为被修好、断言不再
/// 失败，**这条用例就会转红**，逼下一个人把包装拆掉。所以它同时是「判据已记录」和
/// 「修完必须回来改这里」。
/// 两条都在此前裸跑过并确认会红（见提交说明里的 `Executed`/`failures` 数）。
final class CrewReplayDeliveryTests: XCTestCase {

    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("replay-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func occurrences(of needle: String, in haystack: String) -> Int {
        guard !needle.isEmpty else { return 0 }
        var n = 0
        var idx = haystack.startIndex
        while let r = haystack.range(of: needle, range: idx..<haystack.endIndex) {
            n += 1
            idx = r.upperBound
        }
        return n
    }

    // MARK: - R2：@ 一个睡着的目标 → 同一份开场里出现两遍

    /// `wakeText` 那条路把消息正文**烤进开场 prompt**，而它**一个游标都不碰**
    /// （`CrewSessionRunner.startCaptain(wakeText:)` / `restartMember`）；紧接着
    /// `LocalSessionLaunch.initialPromptWithWhiteboard` 又把同一条当「未读」渲染一遍。
    ///
    /// **这不是竞态，是必然**：每一次「@ 一个没在跑的目标」都产生两份。
    ///
    /// 两种改法都能让它转绿（① 开场带正文就把该条在游标上标成已投；② 开场不带正文、
    /// 只说「有人 @ 你，见未读」），所以这条断言**不预设**选哪一种。
    func test_叫醒一个睡着的目标时开场里不许出现两遍同一条消息() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        let body = "去把 #105 的账对一遍"
        store.appendUserMessage(crewId: "c", text: body)

        // 生产里的形状：wakeText =「发送者：正文」，被拼进开场 brief。
        let brief = "有人在群里 @ 你：「机长：\(body)」。接着处理这条。"
        // 拉起缺席目标那条路现在会把这条 @ 的白板 id 一起带下来（#105 ②）。
        let entryId = LocalWhiteboardStore(directory: dir).list(crewId: "c").last!.id
        let prompt = LocalSessionLaunch.initialPromptWithWhiteboard(
            brief, crewId: "c", sessionId: "captain-new", captain: true,
            excludingEntryId: entryId, directory: dir)

        // ✅ 2026-09-07 修好了（#105 ②），包装已拆。
        XCTAssertEqual(
            occurrences(of: body, in: prompt), 1,
            "同一条消息在一份开场 prompt 里出现了两遍：一遍是烤进 brief 的 wakeText，"
                + "一遍是白板未读注入。收的人无从判断这是一条还是两条。")
    }

    /// 反面：**不是被 @ 醒**的普通启动（没有 `excludingEntryId`）不许因为这条改动
    /// 少掉任何一条未读 —— 排除是「这条已经由别的通道送到了」，不是「少送一条」。
    func test_不是被叫醒的普通启动一条未读都不许少() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        let body = "这条必须出现在未读块里"
        store.appendUserMessage(crewId: "c", text: body)
        let prompt = LocalSessionLaunch.initialPromptWithWhiteboard(
            "开工吧", crewId: "c", sessionId: "captain-plain", captain: true, directory: dir)
        XCTAssertEqual(occurrences(of: body, in: prompt), 1)
    }

    // MARK: - R1：换了 sessionId 就等于「从没投递过」

    /// 机长每次重启都新造 `captain-<uuid8>`（`CrewSessionRunner.swift:2113`），
    /// 而**对话是 `--resume` 接回来的**。游标文件名带 sessionId ⇒ 新 id ⇒ 文件不存在
    /// ⇒ `WhiteboardCursor.read() == .absent` ⇒ 按「真首次」重投最近
    /// `firstDeliveryLimit` 条。**对话记得，游标不记得。**
    ///
    /// 实测规模：本机 408 个机长游标 / 47 个 crew = 361 次「非首任」，每次上限 30 条。
    func test_同一个对话换了sessionId之后不许重投它已经消费过的历史() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        for i in 1...3 { store.appendUserMessage(crewId: "c", text: "历史第 \(i) 条") }

        let old = HookEmitter(store: store, crewId: "c", sessionId: "captain-old", cursorDir: dir)
        XCTAssertNotNil(old.emitAndAdvance(), "前置条件：旧任真的消费掉了这三条")
        XCTAssertNil(old.emitAndAdvance(), "前置条件：旧任确实已读完")

        // 重启：对话被 --resume 接回来，sessionId 换了。
        let fresh = HookEmitter(store: store, crewId: "c", sessionId: "captain-new", cursorDir: dir)

        // ✅ 2026-09-07 修好了（#105 ①）：游标键改成跟对话身份走，包装已拆。
        // 拆包装这件事是 `XCTExpectFailure` 自己逼出来的 —— 行为一修好，它就转红。
        XCTAssertNil(
            fresh.emitAndAdvance(),
            "同一个对话换了 sessionId 之后，旧任已经消费过的那批被原封不动重投了一遍。")
    }

    /// `.absent` 一次重投的上限 = 重放一次的规模。这条是**规模的锚**，不是判据：
    /// 有人调这个数时应当知道它同时是「一次重放灌多少条」。
    func test_absent首投上限就是重放一次的规模() {
        XCTAssertEqual(WhiteboardCursor.firstDeliveryLimit, 30)
    }
}
#endif
