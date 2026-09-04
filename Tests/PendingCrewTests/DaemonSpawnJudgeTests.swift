#if os(macOS)
import XCTest

/// **「后台到底起成了没有」**（2026-09-04 真机逮到的洞 + 父机长定下的六条判据）。
///
/// 病历：把隔离数据根 `chmod 500` 之后起 daemon，实测
/// ```
/// PendingCrew daemon：拿不到编排锁，本进程不接管编排：打不开 …/orchestrator.lock：Permission denied
/// 本进程退出。
/// 退出码=0
/// ```
/// —— 它**起得来、然后立刻自己退掉、而且退出码是 0**。拉起方那时只看
/// `Process.run()` 抛没抛错，于是把这种情况记成「起来了」，`decide` 便不去取锁、
/// 返回 `keepConnecting`，app 永远挂在「正在连接后台进程…」上。
///
/// **代价不是少报一个错，是契约有一半在空跑**：唯一允许接管的那一支要求
/// `spawn == .failed`，而 `.failed` 那时只在 exec 本身失败（二进制没了 / 不可执行）
/// 时才成立 —— 「后台起不来」最常见的原因一条都到不了。而症状恰恰是这一整期在
/// 消灭的那种「界面在、什么都不动、不报错」。
///
/// ## 判据不是「睡够了没有」，是「哪件事先发生」
///
/// 并行等两件事：**首次协议握手**与**子进程终止**，谁先到算谁。这一组用**可控
/// 假进程**（`ChildState` + 握手布尔值就是那个假进程）覆盖四种赛果。
final class DaemonSpawnJudgeTests: XCTestCase {

    private func step(link: DaemonLaunchRace.LinkState = .none,
                      child: DaemonLaunchRace.ChildState = .alive,
                      elapsed: TimeInterval = 0,
                      limit: TimeInterval = 10) -> DaemonLaunchRace.Outcome {
        DaemonLaunchRace.step(link: link, child: child, elapsed: elapsed, limit: limit)
    }

    // MARK: - 四种赛果

    /// ① **握手先到** = 真起成了。这是唯一一种「起成了」的证据 ——
    /// 「进程还在」只证明它没死，不证明它在服务。
    func test_握手先到就是起成了() {
        XCTAssertEqual(step(link: .handshaken), .handshake)
        // 就算同一拍里进程也退了，握手赢 —— 它已经服务过我们了。
        XCTAssertEqual(step(link: .handshaken, child: .exited(0)), .handshake)
    }

    /// ② **握手之前退出、退出码 0** —— 真机上那一条。**exit 0 也是失败。**
    ///
    /// 判据**不许去解析退出码**：daemon 对「别人占着锁」（正确结局）和
    /// 「锁文件打不开」（真失败）**都 exit 0**，从退出码上本来就分不出。
    /// 这里只回答「起成了没有」，是哪种原因交给锁的观测去分辨。
    func test_握手前退出即使退出码为0也算启动失败() {
        XCTAssertEqual(step(child: .exited(0)), .exitedBeforeHandshake(0))
    }

    /// ③ 握手之前退出、非 0 退出码 —— 同样是失败，走同一条路。
    /// （将来 daemon 若把「数据目录打不开」改成非 0，这里一个字都不用改。）
    func test_握手前退出非零退出码同样算启动失败() {
        XCTAssertEqual(step(child: .exited(3)), .exitedBeforeHandshake(3))
    }

    /// ④ **到上限了、进程还活着** = 不确定态。既没有握手（不能说它起成了），
    /// 也没有退出（不能说它起不来）。
    func test_超时但进程还活着是不确定态() {
        XCTAssertEqual(step(elapsed: 10, limit: 10), .timedOutStillAlive)
        XCTAssertEqual(step(elapsed: 9.9, limit: 10), .pending, "没到上限就别下结论")
    }

    // MARK: - 赛果 → 契约输入

    /// 不确定态**必须 fail-closed**：不许回退本地（我们并不知道那边有没有人在编排），
    /// 也不许继续无限「正在连接」。
    func test_不确定态既不许回退也不许继续无限等() {
        let spawn = OrchestrationFallback.spawn(
            launchThrew: nil, race: .timedOutStillAlive, limit: 10)
        guard case let .uncertain(reason) = spawn else {
            return XCTFail("超时且进程还活着必须是不确定态，实际：\(spawn)")
        }
        XCTAssertTrue(reason.contains("10"), "原因要带上等了多久：\(reason)")

        // 而且它压过锁的观测：哪怕这一刻锁恰好到手，也不许接管。
        let root = URL(fileURLWithPath: "/tmp/pcrew-uncertain-\(UUID().uuidString.prefix(8))")
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let lock = SessionOrchestratorLock.acquire(dataRoot: root, kind: "app")
        let decision = OrchestrationFallback.decide(
            lock: lock, spawn: spawn, linkFailure: nil, dataRoot: root)
        guard case .refuse = decision else {
            return XCTFail("不确定态下接管 = 在「可能有人在编排」时造双头，实际：\(decision)")
        }
    }

    /// 握手前退出 → `.failed`，于是**唯一允许接管的那一支到得了**（由锁的观测
    /// 去分辨到底该接管、该重连、还是该报错）。这正是真机那条洞被堵上的地方。
    func test_握手前退出会让契约那一支真的到得了() {
        let spawn = OrchestrationFallback.spawn(
            launchThrew: nil, race: .exitedBeforeHandshake(0), limit: 10)
        guard case let .failed(reason) = spawn else { return XCTFail("实际：\(spawn)") }
        XCTAssertTrue(reason.contains("退"), reason)

        let root = URL(fileURLWithPath: "/tmp/pcrew-exited-\(UUID().uuidString.prefix(8))")
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let lock = SessionOrchestratorLock.acquire(dataRoot: root, kind: "app")
        let decision = OrchestrationFallback.decide(
            lock: lock, spawn: spawn, linkFailure: nil, dataRoot: root)
        guard case .takeOverLocally = decision else {
            return XCTFail("锁到手 + 后台确实起不来 = 唯一允许的那一支，实际：\(decision)")
        }
    }

    /// exec 本身就失败（二进制没了 / 不可执行）—— 原来就对的那一条，别改坏。
    func test_起都起不来算失败() {
        let spawn = OrchestrationFallback.spawn(
            launchThrew: "launch path not accessible", race: .pending, limit: 10)
        guard case let .failed(reason) = spawn else { return XCTFail("实际：\(spawn)") }
        XCTAssertTrue(reason.contains("launch path"), reason)
    }

    /// **一个卡住的 daemon 照样持着锁。** 锁在手、kind 写着 daemon，所以观测到的
    /// 确实是「有人在编排」；但它不回握手。这时不许接管（接管就是双头），
    /// **但也不许一直用同一句话转下去** —— 那是「无限的正在连接」换了个更让人
    /// 安心的措辞，因此更难被发现。
    ///
    /// 规矩一句话：**观测到有人在编排，只够否决「接管」，不够支撑「无限期沉默地等」。**
    func test_卡住的daemon占着锁时文案必须升级但仍然不接管() {
        let root = URL(fileURLWithPath: "/tmp/pcrew-stalled-\(UUID().uuidString.prefix(8))")
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let holder = SessionOrchestratorLock.Holder(
            kind: "daemon", pid: 4242, startTimeSeconds: 1, startTimeMicroseconds: 2,
            dataRoot: root.path, acquiredAt: Date())

        func decide(stalledFor: TimeInterval) -> OrchestrationFallback.Decision {
            OrchestrationFallback.decide(
                lock: .heldBy(holder), spawn: .notAttempted, linkFailure: nil,
                dataRoot: root, stalledFor: stalledFor, stallLimit: 60)
        }

        // 还没到上限：照常「继续重连」。
        guard case let .keepConnecting(early) = decide(stalledFor: 59) else {
            return XCTFail("实际：\(decide(stalledFor: 59))")
        }
        XCTAssertTrue(early.contains("4242"), early)

        // 过了上限：**同一句话不许再转下去**。
        let late = decide(stalledFor: 60)
        guard case let .refuse(detail) = late else {
            return XCTFail("卡住的 daemon 会让这句话永远转下去 —— 那是静默换了个措辞。实际：\(late)")
        }
        XCTAssertTrue(detail.contains("4242"), "要指名道姓说是谁没回应：\(detail)")
        XCTAssertTrue(detail.contains("daemon-status"), "要给可操作的下一步：\(detail)")
        XCTAssertTrue(detail.contains("不会") || detail.contains("不接管"),
                      "状态没变：仍然不接管。\(detail)")
        if case .takeOverLocally = late { XCTFail("这一支永远不许接管") }
    }

    /// **真目录、真权限**跑一遍那条链：数据根不可写 → 锁真的取不到 →
    /// 契约给出「不接管 + 可操作错误 + 原因」。
    ///
    /// 这一条盯的是**判据的输入是真的**：上面几条喂的是手写的 `.unavailable("…")`，
    /// 而 `chmod 500` 之后 `open(…, O_CREAT)` 到底返回什么、`acquire` 会不会真的
    /// 走到那一支，只有真跑一次才知道。2026-09-04 真机上 daemon 侧的原始输出是
    /// `打不开 …/orchestrator.lock：Permission denied` + `退出码=0`，与这里一致。
    func test_数据根不可写时真的取不到锁而且给出可操作错误() throws {
        let root = URL(fileURLWithPath: "/tmp/pcrew-ro-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.setAttributes([.posixPermissions: 0o700],
                                                   ofItemAtPath: root.path)
            try? FileManager.default.removeItem(at: root)
        }
        try FileManager.default.setAttributes([.posixPermissions: 0o500],
                                              ofItemAtPath: root.path)

        let outcome = SessionOrchestratorLock.acquire(dataRoot: root, kind: "app")
        guard case let .unavailable(reason) = outcome else {
            throw XCTSkip("这台机器上 chmod 500 之后仍然建得出锁文件（root？），前提不成立：\(outcome)")
        }
        XCTAssertTrue(reason.contains("orchestrator.lock"), reason)

        // 真机上 daemon 就是在这一步打完原因 exit(0) 的 —— 于是拉起方看到
        // 「握手前退出」，契约这才走得到锁的观测。
        let spawn = OrchestrationFallback.spawn(
            launchThrew: nil, race: .exitedBeforeHandshake(0), limit: 15)
        let decision = OrchestrationFallback.decide(
            lock: outcome, spawn: spawn, linkFailure: nil, dataRoot: root)
        guard case let .refuse(detail) = decision else {
            return XCTFail("拿不到锁就没资格当唯一所有者，必须拒绝，实际：\(decision)")
        }
        XCTAssertTrue(detail.contains("Permission denied") || detail.contains("权限"),
                      "原因要原样带出来给人看：\(detail)")
    }

    /// 握手成功那一趟压根不该走降级判定 —— 它已经连上了。
    func test_握手成功不产生任何降级输入() {
        XCTAssertNil(OrchestrationFallback.spawnIfNotConnected(
            launchThrew: nil, race: .handshake, limit: 10))
    }
}


/// **「socket 连上了」不是「握上手了」**（2026-09-04 读代码逮到的第四种穿法）。
///
/// 判据第 1 条要的是等**首次协议握手**。而实现里拿的是
/// `UnixSocketTransport.connect` 成功 —— 这两件事差着一整个握手：
/// `SessionProtocolClient.isConnected` 只在真收到 `daemonHello`、
/// 且协议/能力协商**兼容**时才翻。
///
/// 差这一截的后果：一个**接受连接但不回话**的 daemon（卡死在 listen 之后、
/// 或半开链路）会被判成「起成了」——赛跑当场收工、横幅被清，之后一直在
/// 「连上 → 心跳超时 → 重连 → 又连上」之间打转，降级判定一次都不会被调用，
/// 「超限升级成可操作错误」**永远不触发**。界面上确实有字在变、也确实说了
/// 「没有回应」，**只是永远不会升级成一件人能做的事**。
final class ViewerHandshakeTests: XCTestCase {

    private func step(_ link: DaemonLaunchRace.LinkState,
                      child: DaemonLaunchRace.ChildState = .alive,
                      elapsed: TimeInterval = 0,
                      limit: TimeInterval = 15) -> DaemonLaunchRace.Outcome {
        DaemonLaunchRace.step(link: link, child: child, elapsed: elapsed, limit: limit)
    }

    /// **socket 开着、hello 没回来 —— 不算赢，接着等。**
    func test_socket连上但没收到hello不算握手() {
        XCTAssertEqual(step(.socketOpen), .pending,
                       "「socket 连上了」被当成「握上手了」——那是最会骗人的那种静默")
    }

    /// 一个**接受连接但不回话**的 daemon：socket 一直开着、hello 一直不来。
    /// 到上限之后必须落进 fail-closed 的不确定态，**不许一直转下去**。
    func test_一直不回hello的daemon到上限落进不确定态() {
        XCTAssertEqual(step(.socketOpen, elapsed: 15, limit: 15), .timedOutStillAlive)
        let spawn = OrchestrationFallback.spawn(
            launchThrew: nil, race: .timedOutStillAlive, limit: 15)
        guard case .uncertain = spawn else {
            return XCTFail("接受连接但不回话 = 说不准，必须 fail-closed，实际：\(spawn)")
        }
    }

    /// 收到 hello 才算握上手。
    func test_收到hello才算握上手() {
        XCTAssertEqual(step(.handshaken), .handshake)
    }

    /// **这一轮没拉过**（锁上写着有 daemon 在跑）而 hello 迟迟不来：到上限之后
    /// 结论不是「说不准」，而是**去问锁** —— 那边确实有人占着，该由 §9.2 那张表
    /// 决定继续重连还是升级文案，不该被「说不准」抢先短路掉。
    func test_没拉过的那条路等够了要去问锁而不是下说不准的结论() {
        XCTAssertEqual(step(.socketOpen, child: .notSpawned, elapsed: 15, limit: 15),
                       .timedOutWithoutSpawn)
        XCTAssertEqual(OrchestrationFallback.spawn(
            launchThrew: nil, race: .timedOutWithoutSpawn, limit: 15), .notAttempted)
    }
}
#endif
