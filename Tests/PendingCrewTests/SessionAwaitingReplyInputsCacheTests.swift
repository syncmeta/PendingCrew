import XCTest

/// 点名快照那 2 秒一拍的两样磁盘输入（审批账本 + 回合 marker）指纹门控缓存的
/// 行为钉子与**前后对比基准**（2026-08-18：开久了卡第三条）。
///
/// 病根复盘见 `SessionAwaitingReplyInputsCache` 的类型注释。这里回答三个问题：
/// 1. **判定有没有变** —— 缓存给出的 pending 摘要 / 收尾问句必须与直接读 store
///    一模一样，**包括状态翻转（pending → answered）之后**（这是审批账本与白板
///    最不一样的地方，指纹语义能不能用全看它）。
/// 2. **门控灵不灵** —— 没变的一拍必须 0 次解码；真变了的那个 crew 必须立刻跟上。
/// 3. **到底省了多少** —— 拿贴近现场的规模（28 个 crew / 60 个 run）量一拍的代价。
final class SessionAwaitingReplyInputsCacheTests: XCTestCase {

    private func tempDir() -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("awaiting-inputs-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    private func key(_ crew: String, _ session: String) -> SessionAwaitingReplyInputsCache.RunKey {
        .init(crewId: crew, sessionId: session)
    }

    // MARK: - 判定不许变

    /// 审批账本的整条生命周期：没有 → raise（pending）→ answer（翻 answered）。
    /// 每一步缓存的结果都必须与直接问 store 一致 —— 尤其**翻转那一步**：
    /// 指纹门控靠的是「任何一次写都必然改变字节数」，翻转如果漏判，红点就会
    /// 一直亮着下不来（「进得去出不来」那类病）。
    func test_状态翻转必须被指纹认出来() {
        let dir = tempDir()
        let store = LocalApprovalStore(directory: dir)
        let cache = SessionAwaitingReplyInputsCache(directory: dir, store: store)
        let k = key("crew-1", "worker-a")

        XCTAssertNil(cache.refresh(runs: [k])[k]?.pendingApprovalSummary, "还没挂待办")

        let id = store.raise(crewId: "crew-1", kind: "decision",
                             sessionId: "worker-a", summary: "要不要合到 main？")
        XCTAssertNotNil(id)
        XCTAssertEqual(cache.refresh(runs: [k])[k]?.pendingApprovalSummary, "要不要合到 main？",
                       "raise 之后必须立刻算作「在等人回话」")

        store.answer(crewId: "crew-1", id: id!, reply: "合")
        XCTAssertNil(cache.refresh(runs: [k])[k]?.pendingApprovalSummary,
                     "答完了红点必须下得来 —— 状态翻转没被指纹认出来")
    }

    /// 同一个 crew 挂了别的 session 的待办，不能算到我头上。
    func test_只认本session的待办() {
        let dir = tempDir()
        let store = LocalApprovalStore(directory: dir)
        let cache = SessionAwaitingReplyInputsCache(directory: dir, store: store)
        store.raise(crewId: "c", kind: "permission", sessionId: "other", summary: "rm -rf?")

        let mine = key("c", "me"), theirs = key("c", "other")
        let out = cache.refresh(runs: [mine, theirs])
        XCTAssertNil(out[mine]?.pendingApprovalSummary)
        XCTAssertEqual(out[theirs]?.pendingApprovalSummary, "rm -rf?")
    }

    /// 收尾问句走另一份文件（`<crewId>.<sessionId>.turn`），由 helper 子进程在回合
    /// 结束时写 —— 跨进程写在文件层面与本进程写没区别，缓存只认磁盘指纹。
    func test_跨进程写的回合marker认得出() {
        let dir = tempDir()
        let cache = SessionAwaitingReplyInputsCache(directory: dir, store: LocalApprovalStore(directory: dir))
        let k = key("c", "s")
        XCTAssertNil(cache.refresh(runs: [k])[k]?.trailingQuestion)

        let marker = SessionTurnMarker(directory: dir, crewId: "c", sessionId: "s")
        marker.write(.init(lastMessageId: "m1", lastTurnId: "t1", awaitingQuestion: "A 还是 B？"))
        XCTAssertEqual(cache.refresh(runs: [k])[k]?.trailingQuestion, "A 还是 B？")

        marker.clearAwaitingQuestion()
        XCTAssertNil(cache.refresh(runs: [k])[k]?.trailingQuestion, "熄灭也要认得出")
    }

    // MARK: - 门控

    func test_没变的一拍一次都不读() {
        let dir = tempDir()
        let store = LocalApprovalStore(directory: dir)
        let crews = (0..<8).map { "c\($0)" }
        for c in crews {
            store.raise(crewId: c, kind: "decision", sessionId: "s", summary: "q")
            SessionTurnMarker(directory: dir, crewId: c, sessionId: "s")
                .write(.init(lastMessageId: nil, lastTurnId: nil, awaitingQuestion: "问句"))
        }
        let runs = crews.map { key($0, "s") }
        let cache = SessionAwaitingReplyInputsCache(directory: dir, store: store)

        _ = cache.refresh(runs: runs)
        XCTAssertEqual(cache.approvalDecodeCount, crews.count, "首拍该把 8 份审批各读一遍")
        XCTAssertEqual(cache.markerReadCount, runs.count)

        for _ in 0..<20 { _ = cache.refresh(runs: runs) }
        XCTAssertEqual(cache.approvalDecodeCount, crews.count, "文件没动却又加锁读了 —— 门控没生效")
        XCTAssertEqual(cache.markerReadCount, runs.count)

        // 只有一个 crew 真变了 → 只该重读那一份。
        store.raise(crewId: crews[3], kind: "decision", sessionId: "s2", summary: "新的")
        let out = cache.refresh(runs: runs + [key(crews[3], "s2")])
        XCTAssertEqual(out[key(crews[3], "s2")]?.pendingApprovalSummary, "新的")
        XCTAssertEqual(cache.approvalDecodeCount, crews.count + 1, "只该重读变了的那一份")
    }

    func test_离开列表的run被淘汰() {
        let dir = tempDir()
        let store = LocalApprovalStore(directory: dir)
        store.raise(crewId: "a", kind: "decision", sessionId: "s", summary: "q")
        store.raise(crewId: "b", kind: "decision", sessionId: "s", summary: "q")
        let cache = SessionAwaitingReplyInputsCache(directory: dir, store: store)
        let a = key("a", "s"), b = key("b", "s")

        _ = cache.refresh(runs: [a, b])
        XCTAssertEqual(cache.approvalDecodeCount, 2)
        _ = cache.refresh(runs: [a])                 // b 的 run 被移除
        XCTAssertEqual(cache.approvalDecodeCount, 2)
        _ = cache.refresh(runs: [a, b])              // b 回来 → 必须重新求值
        XCTAssertEqual(cache.approvalDecodeCount, 3, "条目没被淘汰的话缓存会随开机时长长胖")
    }

    // MARK: - 基准：一拍的代价（改前 vs 改后）

    /// 夹具按**现场实测的形状**造，不是随手编的（2026-08-18 量的那份目录）：
    /// 28 个 crew，其中只有 10 个真有审批账本（1–23 条 / 2.6–19 KB），其余 18 个连
    /// 文件都没有 —— 但改前那条路照样要为它们各开一次 flock（`withFileLock` 用
    /// `O_CREAT` 开锁文件，所以「没有审批」并不等于「不花钱」，那 398 个 `.lock`
    /// 残骸就是这么来的）。turn marker 每个约 100 B。
    private func makeFieldShapedFixture(dir: URL, store: LocalApprovalStore,
                                        crewCount: Int, runsPerCrew: Int) {
        let itemCounts = [23, 6, 5, 4, 4, 3, 2, 2, 2, 1]     // 现场那 10 份的条目数
        for (index, count) in itemCounts.enumerated() where index < crewCount {
            let crew = "local-bench-\(index)"
            for i in 0..<count {
                let id = store.raise(
                    crewId: crew, kind: "permission", sessionId: "s\(i % runsPerCrew)",
                    summary: "命令审批：git push --force origin main（第 \(i) 条，"
                        + "现场单条摘要就有这么长，含工作目录与完整命令行）")
                // 现场绝大多数是已答复的历史条目，只有最后一条还挂着。
                if i < count - 1 { store.decide(crewId: crew, id: id!, decision: "allow") }
            }
        }
        for index in 0..<crewCount {
            for r in 0..<runsPerCrew {
                SessionTurnMarker(directory: dir, crewId: "local-bench-\(index)",
                                  sessionId: "s\(r)")
                    .write(.init(lastMessageId: UUID().uuidString,
                                 lastTurnId: UUID().uuidString, awaitingQuestion: nil))
            }
        }
    }

    /// 现场量级：28 个 crew、60 个 agent run（run 只增不减，开一天就是这个数），2 秒一拍。
    /// - **改前**：每个 crew 一次 `pending(crewId:)`（**开锁文件 + flock + 读整份 JSON +
    ///   全量解码**）+ 每个 run 一次 turn marker 读，全部在 MainActor 上。
    /// - **改后**：28 + 60 次 `stat(2)`，只有指纹变了的才真读。
    func test_基准_一拍的代价() {
        let dir = tempDir()
        let store = LocalApprovalStore(directory: dir)
        let crewCount = 28, runsPerCrew = 2
        makeFieldShapedFixture(dir: dir, store: store,
                               crewCount: crewCount, runsPerCrew: runsPerCrew)
        let crews = (0..<crewCount).map { "local-bench-\($0)" }
        let runs = crews.flatMap { c in (0..<runsPerCrew).map { key(c, "s\($0)") } }

        func legacyTick() {
            for c in crews {
                var bySession: [String: String] = [:]
                for item in store.pending(crewId: c) where bySession[item.sessionId] == nil {
                    bySession[item.sessionId] = item.summary
                }
            }
            for r in runs {
                _ = SessionTurnMarker(directory: dir, crewId: r.crewId,
                                      sessionId: r.sessionId).read().awaitingQuestion
            }
        }
        legacyTick()                                    // 预热
        let ticks = 20
        let legacyStart = CFAbsoluteTimeGetCurrent()
        for _ in 0..<ticks { legacyTick() }
        let legacyMs = (CFAbsoluteTimeGetCurrent() - legacyStart) * 1000 / Double(ticks)

        let cache = SessionAwaitingReplyInputsCache(directory: dir, store: store)
        _ = cache.refresh(runs: runs)                   // 首拍填充
        let warmDecodes = cache.approvalDecodeCount
        let cachedStart = CFAbsoluteTimeGetCurrent()
        for _ in 0..<ticks { _ = cache.refresh(runs: runs) }
        let cachedMs = (CFAbsoluteTimeGetCurrent() - cachedStart) * 1000 / Double(ticks)

        XCTAssertEqual(cache.approvalDecodeCount, warmDecodes, "这 20 拍一次解码都不该有")
        XCTAssertEqual(cache.markerReadCount, runs.count)

        // 现场最常见的一拍：某个 session 挂了条待审批，只有它那个 crew 的账本动了。
        store.raise(crewId: crews[3], kind: "permission", sessionId: "s0", summary: "新的")
        let oneChangeStart = CFAbsoluteTimeGetCurrent()
        let out = cache.refresh(runs: runs)
        let oneChangeMs = (CFAbsoluteTimeGetCurrent() - oneChangeStart) * 1000
        XCTAssertEqual(out[key(crews[3], "s0")]?.pendingApprovalSummary, "新的")
        XCTAssertEqual(cache.approvalDecodeCount, warmDecodes + 1, "只该重读变了的那一份")

        print("""

        ── 点名快照一拍的代价（\(crewCount) 个 crew / \(runs.count) 个 run，按现场形状造）──
          改前（每 crew 开锁+加锁+整份解码，每 run 读 marker，全在主线程）: \
        \(String(format: "%7.3f", legacyMs)) ms/拍
          改后（全命中，只 stat）                                        : \
        \(String(format: "%7.3f", cachedMs)) ms/拍
          改后（一个 crew 真变了，重读它一份）                           : \
        \(String(format: "%7.3f", oneChangeMs)) ms/拍
          加锁读盘次数：改前 \(crewCount)+\(runs.count) 次/拍 → 改后 0（无变化）/ 1（真有新待办）
          倍数：全命中快 \(String(format: "%.1f", legacyMs / max(cachedMs, 0.0001)))×；\
        按 2 秒一拍折算，主线程 \(String(format: "%.2f", legacyMs / 2)) ms/s → 0（整段已挪出主线程）

        """)

        XCTAssertLessThan(cachedMs, legacyMs * 0.5,
                          "全命中的一拍该明显比整表重读便宜")
        XCTAssertLessThan(oneChangeMs, legacyMs,
                          "只有一个 crew 变时也不该比整表重读贵")
    }

    /// 同一把尺子量**现场那份真目录**。默认跳过 —— 真数据不入 git，也不该让测试依赖
    /// 某台机器。要量时把 `~/Library/Application Support/PendingCrew/whiteboards`
    /// **拷一份**到临时目录（别直接指线上目录，那儿有活着的 session 在写），然后：
    ///
    ///     TEST_RUNNER_PENDINGCREW_BENCH_WHITEBOARD_DIR=<拷贝出来的目录> xcodebuild test …
    ///
    /// （`TEST_RUNNER_` 前缀是必须的，见 `CrewLastMessageCacheTests` 同名说明。）
    func test_基准_现场目录() throws {
        let path = ProcessInfo.processInfo
            .environment["PENDINGCREW_BENCH_WHITEBOARD_DIR"] ?? ""
        try XCTSkipIf(path.isEmpty, "未指定现场白板目录，跳过")
        let dir = URL(fileURLWithPath: path)
        let names = try FileManager.default.contentsOfDirectory(atPath: path)
        let crews = names.filter { $0.hasPrefix("local-") && $0.hasSuffix(".json")
                                   && !$0.contains(".approvals.") && !$0.contains(".todos.")
                                   && !$0.contains(".captain-awareness.") }
            .map { String($0.dropLast(".json".count)) }.sorted()
        let runs: [SessionAwaitingReplyInputsCache.RunKey] = names
            .filter { $0.hasSuffix(".turn") }
            .compactMap { name in
                let base = String(name.dropLast(".turn".count))
                guard let dot = base.firstIndex(of: ".") else { return nil }
                return key(String(base[base.startIndex..<dot]),
                           String(base[base.index(after: dot)...]))
            }
        let store = LocalApprovalStore(directory: dir)

        func legacyTick() {
            for c in crews { _ = store.pending(crewId: c) }
            for r in runs {
                _ = SessionTurnMarker(directory: dir, crewId: r.crewId,
                                      sessionId: r.sessionId).read().awaitingQuestion
            }
        }
        legacyTick()
        let ticks = 20
        let legacyStart = CFAbsoluteTimeGetCurrent()
        for _ in 0..<ticks { legacyTick() }
        let legacyMs = (CFAbsoluteTimeGetCurrent() - legacyStart) * 1000 / Double(ticks)

        let cache = SessionAwaitingReplyInputsCache(directory: dir, store: store)
        _ = cache.refresh(runs: runs)
        let cachedStart = CFAbsoluteTimeGetCurrent()
        for _ in 0..<ticks { _ = cache.refresh(runs: runs) }
        let cachedMs = (CFAbsoluteTimeGetCurrent() - cachedStart) * 1000 / Double(ticks)

        print("""

        ── 现场目录一拍（\(crews.count) 个 crew / \(runs.count) 个 run）──
          改前: \(String(format: "%7.3f", legacyMs)) ms   改后(全命中): \
        \(String(format: "%7.3f", cachedMs)) ms   快 \
        \(String(format: "%.1f", legacyMs / max(cachedMs, 0.0001)))×
          按 2 秒一拍折算：主线程 \(String(format: "%.2f", legacyMs / 2)) ms/s → 0（已挪出主线程）

        """)
        XCTAssertLessThan(cachedMs, legacyMs * 0.5)
    }
}
