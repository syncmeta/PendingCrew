import XCTest

/// 侧栏「每个 crew 末条消息」指纹门控缓存（`CrewLastMessageCache`）的行为钉子
/// 与**前后对比基准**。
///
/// 病根复盘见 `CrewLastMessageCache` 的类型注释。这里回答两个问题：
/// 1. **行为有没有退化** —— 白板真变了（含另一个进程写的）必须立刻重新求值；
///    没变必须命中缓存、一个字节都不读。
/// 2. **到底省了多少** —— 拿一份贴近现场的白板目录（28 个 crew / 合计 ~2.7 MB）
///    量「一次目录 tick 要付的读取代价」：改前 = 28 份整板解码，改后 = 28 次 stat
///    （+ 真变了那一个 crew 的一次解码）。
final class CrewLastMessageCacheTests: XCTestCase {

    // MARK: - 夹具

    private func tempDir() -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("last-msg-cache-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    private static let iso = ISO8601DateFormatter()

    private func message(_ text: String) -> LocalWhiteboardMessage {
        LocalWhiteboardMessage(
            id: UUID().uuidString, senderKind: "session", senderUserId: nil,
            senderSessionId: "s", category: nil, text: text,
            createdAt: Self.iso.string(from: Date()))
    }

    /// 直接把整份白板写成磁盘上的最终形态（与 `LocalWhiteboardStore` 的落盘格式
    /// 一致）—— 造 2.7 MB 夹具时逐条 append 是 O(n²) 整文件重写，太慢。
    @discardableResult
    private func writeBoard(_ dir: URL, crewId: String, messages: [LocalWhiteboardMessage]) -> Int {
        let data = try! JSONEncoder().encode(messages)
        try! data.write(to: dir.appendingPathComponent("\(crewId).json"), options: .atomic)
        return data.count
    }

    // MARK: - 行为：变了要重新求值，没变要命中缓存

    func test_指纹没变时不读文件() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        for i in 0..<5 { writeBoard(dir, crewId: "c\(i)", messages: [message("hello \(i)")]) }
        let ids = (0..<5).map { "c\($0)" }

        var loads = 0
        let cache = CrewLastMessageCache(
            fingerprintOf: { store.fingerprint(crewId: $0) },
            loadLast: { loads += 1; return store.list(crewId: $0).last })

        let first = cache.refresh(crewIds: ids)
        XCTAssertEqual(first.count, 5)
        XCTAssertEqual(loads, 5, "首次求值该把 5 个都读一遍")
        XCTAssertEqual(cache.decodeCount, 5)

        // 之后无论 tick 多少次，文件没动就一次都不该再读。
        for _ in 0..<20 {
            let again = cache.refresh(crewIds: ids)
            // 恒等的完整快照正是 `CrewStore.publishLastWhiteboardMessages` 那道
            // 「相等就不赋值」的前提：白板没真变时连 objectWillChange 都不发，
            // 整个侧栏不会再按别人的写盘频率被拉起来重渲染。
            XCTAssertEqual(again, first, "缓存命中也必须返回完整快照，不能只返回变化项")
        }
        XCTAssertEqual(loads, 5, "指纹没变却又读了文件 —— 门控没生效")
        XCTAssertEqual(cache.decodeCount, 5)
    }

    func test_指纹变了就重新求值_且只重算变了的那个() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        for i in 0..<5 { writeBoard(dir, crewId: "c\(i)", messages: [message("旧 \(i)")]) }
        let ids = (0..<5).map { "c\($0)" }

        var loads = 0
        let cache = CrewLastMessageCache(
            fingerprintOf: { store.fingerprint(crewId: $0) },
            loadLast: { loads += 1; return store.list(crewId: $0).last })
        _ = cache.refresh(crewIds: ids)
        loads = 0

        // 走真正的写路径（flock + 重读-合并-整写），指纹自然变。
        store.appendUserMessage(crewId: "c3", text: "新消息")

        let snapshot = cache.refresh(crewIds: ids)
        XCTAssertEqual(snapshot["c3"]?.text, "新消息", "白板真变了，侧栏预览必须立刻跟上")
        XCTAssertEqual(loads, 1, "只该重算变了的那一个 crew")
        XCTAssertEqual(snapshot["c0"]?.text, "旧 0", "没变的 crew 仍要在快照里，且值不变")
    }

    /// 跨进程写（helper 子进程的 `post_to_crew`）在文件层面与本进程写没有区别 ——
    /// 另起一个 store 实例写同一个目录即等价：cache 只认磁盘指纹，不认进程内信号。
    func test_另一个进程写的白板也认得出() {
        let dir = tempDir()
        let appSide = LocalWhiteboardStore(directory: dir)
        let helperSide = LocalWhiteboardStore(directory: dir)
        writeBoard(dir, crewId: "c", messages: [message("旧")])

        let cache = CrewLastMessageCache(store: appSide)
        XCTAssertEqual(cache.refresh(crewIds: ["c"])["c"]?.text, "旧")

        helperSide.appendSessionMessage(crewId: "c", sessionId: "worker", text: "helper 写的")
        XCTAssertEqual(cache.refresh(crewIds: ["c"])["c"]?.text, "helper 写的")
    }

    func test_白板文件不存在时不开锁读() {
        var loads = 0
        let cache = CrewLastMessageCache(
            fingerprintOf: { _ in nil },   // 文件不存在
            loadLast: { _ in loads += 1; return nil })
        let snapshot = cache.refresh(crewIds: ["a", "b"])
        XCTAssertTrue(snapshot.isEmpty, "键缺失 = 白板是空的")
        XCTAssertEqual(loads, 0, "已知必然为空，不该为了拿一个 nil 去开 flock")
        XCTAssertEqual(cache.decodeCount, 0)
    }

    func test_列表里消失的crew被淘汰() {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)
        writeBoard(dir, crewId: "a", messages: [message("A")])
        writeBoard(dir, crewId: "b", messages: [message("B")])

        var loads = 0
        let cache = CrewLastMessageCache(
            fingerprintOf: { store.fingerprint(crewId: $0) },
            loadLast: { loads += 1; return store.list(crewId: $0).last })

        _ = cache.refresh(crewIds: ["a", "b"])
        XCTAssertEqual(loads, 2)
        _ = cache.refresh(crewIds: ["a"])          // b 离开列表
        XCTAssertEqual(loads, 2)
        // b 回来 → 必须重新求值（证明它的条目确实被淘汰了，缓存不会随开机时长长胖）
        XCTAssertEqual(cache.refresh(crewIds: ["a", "b"])["b"]?.text, "B")
        XCTAssertEqual(loads, 3)
    }

    // MARK: - 基准：一次目录 tick 的读取代价（改前 vs 改后）

    /// 现场量级：28 个 crew，白板 JSON 合计 ~2.7 MB（最大单个 ~516 KB）。
    /// 目录 tick 已合流到最多 4 次/秒，每次 tick 侧栏都要拿到「每个 crew 的末条消息」。
    ///
    /// - **改前**：每个 crew 各来一次 `list(crewId:).last` —— flock + 读整个文件 +
    ///   全量 JSON 解码，**在主线程的 SwiftUI body 里**。
    /// - **改后**：`cache.refresh(crewIds:)` —— 28 次 stat，只有指纹变了的那个才解码。
    ///
    /// 断言口径刻意宽松（新路径 < 旧路径的 20%），只为把「数量级差」钉住、不让
    /// 机器负载抖动把测试搞成 flaky；真实倍数由下面 print 出来的数据说话。
    func test_基准_一次tick的读取代价() throws {
        let dir = tempDir()
        let store = LocalWhiteboardStore(directory: dir)

        // 造夹具：28 个 crew，一个大的（~500 KB）+ 其余各 ~80 KB，合计 ~2.7 MB。
        let crewCount = 28
        let ids = (0..<crewCount).map { "local-bench-\($0)" }
        let body = String(repeating: "白板正文 whiteboard payload ", count: 24)  // ≈ 600 B/条
        var totalBytes = 0
        for (index, id) in ids.enumerated() {
            let count = index == 0 ? 850 : 130
            let rows = (0..<count).map { message("\(body) #\($0)") }
            totalBytes += writeBoard(dir, crewId: id, messages: rows)
        }

        // —— 改前：每 tick 把每个 crew 的白板整份重读重解 ——
        func legacyTick() {
            for id in ids {
                // 这一行就是被删掉的 `CrewSidebarCrewRow.lastMessage(crewId:revision:)`。
                _ = store.list(crewId: id).last
            }
        }
        legacyTick()                                   // 预热（文件缓存 / 惰性初始化）
        let ticks = 10
        let legacyStart = CFAbsoluteTimeGetCurrent()
        for _ in 0..<ticks { legacyTick() }
        let legacyMs = (CFAbsoluteTimeGetCurrent() - legacyStart) * 1000 / Double(ticks)

        // —— 改后：指纹门控 ——
        let cache = CrewLastMessageCache(store: store)
        _ = cache.refresh(crewIds: ids)                // 首次填充（等价于 app 启动那一下）
        let warmDecodes = cache.decodeCount
        let cachedStart = CFAbsoluteTimeGetCurrent()
        for _ in 0..<ticks { _ = cache.refresh(crewIds: ids) }
        let cachedMs = (CFAbsoluteTimeGetCurrent() - cachedStart) * 1000 / Double(ticks)

        XCTAssertEqual(warmDecodes, crewCount, "首次该把 28 个都读一遍")
        XCTAssertEqual(cache.decodeCount, warmDecodes,
                       "无关文件的写不改白板指纹 —— 这 10 次 tick 一次解码都不该有")

        // 现场最常见的一拍：某个 session 发了条消息，只有它那个 crew 的白板动了。
        store.appendSessionMessage(crewId: ids[3], sessionId: "s", text: "新进展")
        let oneChangeStart = CFAbsoluteTimeGetCurrent()
        let snapshot = cache.refresh(crewIds: ids)
        let oneChangeMs = (CFAbsoluteTimeGetCurrent() - oneChangeStart) * 1000
        XCTAssertEqual(snapshot[ids[3]]?.text, "新进展")
        XCTAssertEqual(cache.decodeCount, warmDecodes + 1, "只该多解码变了的那一个")

        print("""

        ── 侧栏「末条消息」一次 tick 的读取代价（\(crewCount) 个 crew / \
        \(String(format: "%.2f", Double(totalBytes) / 1_048_576)) MB 白板）──
          改前（每 crew 整板解码，主线程）      : \(String(format: "%7.3f", legacyMs)) ms/tick
          改后（全命中缓存，只 stat）           : \(String(format: "%7.3f", cachedMs)) ms/tick
          改后（一个 crew 真变了，重解它一份）  : \(String(format: "%7.3f", oneChangeMs)) ms/tick
          解码次数：改前 \(crewCount)/tick → 改后 0（无关写）/ 1（真有新消息）
          倍数：全命中快 \(String(format: "%.0f", legacyMs / max(cachedMs, 0.0001)))×，\
        单 crew 变化快 \(String(format: "%.1f", legacyMs / max(oneChangeMs, 0.0001)))×

        """)

        XCTAssertLessThan(cachedMs, legacyMs * 0.2,
                          "全命中的一次 tick 该比整表重读便宜一个数量级以上")
        XCTAssertLessThan(oneChangeMs, legacyMs * 0.5,
                          "只有一个 crew 变时，代价该接近『只解一份』而不是整表")
    }

    /// 同一把尺子量**现场那份真白板**。默认跳过 —— 真白板不入 git（同
    /// `Fixtures/`），也不该让测试依赖某台机器的数据。要量时把
    /// `~/Library/Application Support/PendingCrew/whiteboards` 拷一份到临时目录
    /// （别直接指线上目录，那儿有活着的 session 在写），然后：
    ///
    ///     TEST_RUNNER_PENDINGCREW_BENCH_WHITEBOARD_DIR=<拷贝出来的目录> xcodebuild test …
    ///
    /// （`TEST_RUNNER_` 前缀是必须的 —— xcodebuild 只把带这个前缀的环境变量转发给
    /// 测试进程并去掉前缀；直接设同名变量传不进去，测试会静默跳过。）
    ///
    /// 2026-08-17 在人类机器上实测（64 份白板 / 2.84 MB）：
    /// 改前 19.586 ms/tick → 改后（全命中）0.773 ms/tick，25×；按目录 tick 顶格
    /// 4 次/秒折算，主线程 78.3 ms/s → 3.1 ms/s，而且这 3.1 ms 已经不在主线程上。
    func test_基准_现场白板目录() throws {
        let path = ProcessInfo.processInfo
            .environment["PENDINGCREW_BENCH_WHITEBOARD_DIR"] ?? ""
        try XCTSkipIf(path.isEmpty, "未指定现场白板目录，跳过")
        let dir = URL(fileURLWithPath: path)
        let ids = try FileManager.default.contentsOfDirectory(atPath: path)
            .filter { $0.hasSuffix(".json") }
            .map { String($0.dropLast(".json".count)) }
            .sorted()
        let bytes = ids.reduce(0) { sum, id in
            sum + ((try? Data(contentsOf: dir.appendingPathComponent("\(id).json")))?.count ?? 0)
        }
        let store = LocalWhiteboardStore(directory: dir)

        func legacyTick() { for id in ids { _ = store.list(crewId: id).last } }
        legacyTick()
        let ticks = 10
        let legacyStart = CFAbsoluteTimeGetCurrent()
        for _ in 0..<ticks { legacyTick() }
        let legacyMs = (CFAbsoluteTimeGetCurrent() - legacyStart) * 1000 / Double(ticks)

        let cache = CrewLastMessageCache(store: store)
        _ = cache.refresh(crewIds: ids)
        let cachedStart = CFAbsoluteTimeGetCurrent()
        for _ in 0..<ticks { _ = cache.refresh(crewIds: ids) }
        let cachedMs = (CFAbsoluteTimeGetCurrent() - cachedStart) * 1000 / Double(ticks)
        XCTAssertEqual(cache.decodeCount, ids.count, "10 次无关 tick 一次都不该重解")

        print("""

        ── 现场白板（\(ids.count) 份 / \
        \(String(format: "%.2f", Double(bytes) / 1_048_576)) MB）一次 tick ──
          改前: \(String(format: "%7.3f", legacyMs)) ms   \
        改后(全命中): \(String(format: "%7.3f", cachedMs)) ms   \
        快 \(String(format: "%.0f", legacyMs / max(cachedMs, 0.0001)))×
          按目录 tick 顶格 4 次/秒折算：主线程 \(String(format: "%.1f", legacyMs * 4)) ms/s \
        → \(String(format: "%.1f", cachedMs * 4)) ms/s（且这部分已挪出主线程）

        """)
        XCTAssertLessThan(cachedMs, legacyMs * 0.2)
    }
}
