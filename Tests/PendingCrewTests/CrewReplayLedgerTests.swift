import XCTest

/// 重放修复 第一批：**④ 四本账合一 + ① 游标跟对话身份走**（人类 Todo #105）。
///
/// 病根不是「多送了一遍」，是**「这条投过了吗」有四本账**：
/// 盘上 per-session 游标、唤醒队列的 `deliveredKeys`、唤醒器的 per-crew 扫描游标、
/// 以及 `listenCursors` —— 后三本都在内存里，互相看不见，也没有任何东西对账。
/// 一条消息可以同时在第一本里「已投」、在第二本里「未投」。
///
/// 这一批要的是：**盘上那一本成为唯一判据**，而且它的键**跟着对话走、不跟着进程走**
/// （机长每次重启都新造 `captain-<uuid8>`，而对话是 `--resume` 接回来的 ——
/// 对话记得，游标不记得）。
final class CrewReplayLedgerTests: XCTestCase {

    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("ledger-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func seed(_ store: LocalWhiteboardStore, _ n: Int, prefix: String = "第") {
        for i in 1...n { store.appendUserMessage(crewId: "c", text: "\(prefix)\(i)条") }
    }

    // MARK: - ① 游标跟对话身份走

    /// 每个 crew 同时只允许一个机长（`startCaptain` 里那道 guard + `runs.removeAll`），
    /// 所以「本 crew 的机长」就是一个无歧义的对话身份 —— 账本该按它记，
    /// 而不是按每次重启新造的 `captain-<uuid8>`。
    func test_两任机长共用同一本账() {
        XCTAssertEqual(CrewConversationKey.forSession("captain-17bf5b87"),
                       CrewConversationKey.forSession("captain-5e324881"),
                       "同一个 crew 的前后两任机长是同一个对话，必须落到同一本账上")
    }

    /// worker 走 `restartMember`，**复用原 sessionId**，本来就没有这个问题；
    /// 把它们也并成一本会让同 crew 的不同 worker 互相吃掉未读。
    func test_worker各记各的账() {
        XCTAssertNotEqual(CrewConversationKey.forSession("worker-aaaa1111"),
                          CrewConversationKey.forSession("worker-bbbb2222"))
        XCTAssertEqual(CrewConversationKey.forSession("worker-aaaa1111"), "worker-aaaa1111",
                       "worker 的键就是它自己的 sessionId，不做归并")
    }

    func test_换了sessionId之后不再重投已经消费过的历史() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        seed(store, 3)

        let old = HookEmitter(store: store, crewId: "c", sessionId: "captain-old11111", cursorDir: dir)
        XCTAssertNotNil(old.emitAndAdvance(), "前置条件：上一任真的消费掉了这三条")

        let fresh = HookEmitter(store: store, crewId: "c", sessionId: "captain-new22222", cursorDir: dir)
        XCTAssertNil(fresh.emitAndAdvance(),
                     "换了 sessionId 就把上一任消费过的那批重投了一遍 —— 这正是 #105 的主线")
    }

    /// 改键当天，磁盘上全是旧格式 `<crewId>.captain-<uuid8>.cursor`。
    /// **不继承 = 全机每个 crew 再各重放一次**，等于修复自己触发一次它要修的 bug。
    func test_旧的按sessionId命名的游标要被继承_不许因为改键再重放一次() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        seed(store, 3)

        // 旧世界的游标必须**手写出来** —— 新 API 已经不会再产生这个文件名了，
        // 拿它去造前置条件只会造出一个新格式文件（上一版这条就是这么假绿的，
        // 靠下面那句前置断言当场逮住）。
        let tail = store.list(crewId: "c").last!
        let legacyFile = dir.appendingPathComponent("c.captain-legacy1.cursor")
        try! "\(tail.id)\t\(tail.createdAt)".write(to: legacyFile, atomically: true, encoding: .utf8)
        XCTAssertTrue(FileManager.default.fileExists(atPath: legacyFile.path),
                      "前置条件：旧格式游标文件真的写出来了")
        XCTAssertFalse(
            FileManager.default.fileExists(atPath:
                dir.appendingPathComponent("c.captain.cursor").path),
            "前置条件：新键那份此刻还不存在，认领才有意义")

        // 新世界：新一任机长，新的键。
        let fresh = HookEmitter(store: store, crewId: "c", sessionId: "captain-fresh22", cursorDir: dir)
        XCTAssertNil(fresh.emitAndAdvance(),
                     "改键那一刻没继承旧游标 —— 升级本身又造了一次全机重放")
    }

    // MARK: - ① 的连带风险：接回游标之后必须自己带上限，而且不许静默截断

    /// 「新 sessionId ⇒ 游标 absent ⇒ 只投 30 条」这个行为**同时也在挡另一件事**：
    /// 机长隔几天醒来被灌几百条。① 把游标接回对话身份之后那道挡板就没了，
    /// 所以有锚点的那条路也必须有上限。
    func test_有锚点时也有上限() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "锚点")
        let cursor = WhiteboardCursor(directory: dir, crewId: "c", sessionId: "captain-aaaaaaaa")
        cursor.advance(to: store.list(crewId: "c").last!, in: store)
        seed(store, 100)

        let unread = cursor.unread(in: store)
        XCTAssertEqual(unread.messages.count, WhiteboardCursor.firstDeliveryLimit,
                       "睡了两天的机长醒来被灌了 \(unread.messages.count) 条")
        XCTAssertEqual(unread.omitted, 100 - WhiteboardCursor.firstDeliveryLimit)
    }

    /// **截断必须自报**。我们已经有两道方向相反的截断了；再加一道静默的，
    /// 收的人会以为自己看到的就是全部。
    func test_被上限截掉的部分必须明说省略了几条() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(crewId: "c", text: "锚点")
        let cursor = WhiteboardCursor(directory: dir, crewId: "c", sessionId: "captain-bbbbbbbb")
        cursor.advance(to: store.list(crewId: "c").last!, in: store)
        seed(store, 100)

        let out = HookEmitter(store: store, crewId: "c", sessionId: "captain-bbbbbbbb", cursorDir: dir)
            .emitAndAdvance()
        XCTAssertNotNil(out)
        XCTAssertTrue(out!.contains("70"), "没说省略了几条：\(out!.prefix(300))")
        XCTAssertTrue(out!.contains("省略"), "没说这是一次截断：\(out!.prefix(300))")
    }

    // MARK: - ④ 盘上那本是唯一判据

    /// 唤醒路此前只问自己内存里那本，问不到 hook 路投过什么。
    /// 合账之后，任何投递面都得能问这一句。
    func test_hook路投过之后这条就算已投递() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        seed(store, 2)
        let entries = store.list(crewId: "c")

        let cursor = WhiteboardCursor(directory: dir, crewId: "c", sessionId: "captain-cccccccc")
        XCTAssertFalse(cursor.hasDelivered(entries[0], in: store), "还没投过")

        _ = HookEmitter(store: store, crewId: "c", sessionId: "captain-cccccccc", cursorDir: dir)
            .emitAndAdvance()

        XCTAssertTrue(cursor.hasDelivered(entries[0], in: store),
                      "hook 路投过并推进了游标，唤醒路却仍然认为它没投过 —— 两本账互相看不见")
        XCTAssertTrue(cursor.hasDelivered(entries[1], in: store))
    }

    func test_游标之后的消息不算已投递() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        seed(store, 1)
        _ = HookEmitter(store: store, crewId: "c", sessionId: "captain-dddddddd", cursorDir: dir)
            .emitAndAdvance()
        store.appendUserMessage(crewId: "c", text: "游标之后的新消息")

        let cursor = WhiteboardCursor(directory: dir, crewId: "c", sessionId: "captain-dddddddd")
        XCTAssertFalse(cursor.hasDelivered(store.list(crewId: "c").last!, in: store),
                       "把没投过的判成已投 = 丢消息，比重复贵得多")
    }

    /// 游标一次都没写过时（真首次），不能把历史一律当「已投过」—— 那是丢消息。
    func test_没有游标时一律不算已投递() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        seed(store, 2)
        let cursor = WhiteboardCursor(directory: dir, crewId: "c", sessionId: "captain-eeeeeeee")
        for m in store.list(crewId: "c") {
            XCTAssertFalse(cursor.hasDelivered(m, in: store))
        }
    }
}
