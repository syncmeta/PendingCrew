import XCTest
// SessionOutputEvidence.swift + CrewSessionsSnapshot.swift 直接编进 PendingCrewTests target。

/// 点名的**产出证据**列（人类 Todo #107 第三件：判活不判状态）。
///
/// 病是这样的：`list_sessions` 只报状态，而状态会骗人 —— 2026-09-06 两个 session
/// 一个卡 90 分钟、一个卡 110 分钟，状态全显示「空闲」，其实任务书压根没提交。
/// 补的这一列不问它显示什么，只问它**最近真的写出过什么、什么时候**。
///
/// 这一组测试里**最重要的是第一条**：三态绝不许压成 Bool。把「看不出来」偷偷
/// 算成「没跑」是这类 bug 的经典丢弃点，而它一旦发生，机长看到的是一句
/// 言之凿凿的「确实没有产出」—— 比不报还坏。
@MainActor
final class SessionOutputEvidenceTests: XCTestCase {
    private func tempDir() -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("evidence-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private func write(_ path: URL, mtime: Date? = nil) {
        try? FileManager.default.createDirectory(
            at: path.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? Data("{}".utf8).write(to: path)
        if let mtime {
            try? FileManager.default.setAttributes([.modificationDate: mtime], ofItemAtPath: path.path)
        }
    }

    private let now = Date(timeIntervalSince1970: 1_788_700_000)

    // MARK: - ① 三态：看不出来 ≠ 确实没有产出（压成 Bool 就红）

    func test_看不出来与确实没有产出是两态_压成Bool会把看不出来算成没跑() throws {
        let home = tempDir()
        let claudeProjects = home.appendingPathComponent("claude/projects")
        let codexSessions = home.appendingPathComponent("codex/sessions")
        // 取证面在（claude 起过东西、目录读得出来），只是这个会话号一个字没写。
        try FileManager.default.createDirectory(
            at: claudeProjects.appendingPathComponent("-some-workdir"), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: codexSessions, withIntermediateDirectories: true)
        let probe = SessionOutputProbe(claudeProjectsDirectory: claudeProjects,
                                       codexSessionsDirectory: codexSessions)

        // A. 知道该去哪儿找、找了、确实没有 → 确实没有产出。
        XCTAssertEqual(probe.evidence(runnerKind: "claude_code", agentSessionId: "sid-never-wrote"),
                       .noOutput)

        // B. 连会话号都没有 → **看不出来**，不许当成「没跑」。
        //    （claude 的会话号我们起进程前就指定并记账，codex 的要等握手；
        //     所以「没有会话号」在 codex 上是常态，绝不是「它没干活」的证据。）
        switch probe.evidence(runnerKind: "codex", agentSessionId: nil) {
        case .unknown: break
        case let other: XCTFail("没有会话号必须是「看不出来」，实际是 \(other)")
        }

        // C. 两态渲染出来的字必须让机长一眼分得清；
        //    「看不出来」那句尤其不许说成「没有产出」。
        let noOutputText = SessionOutputEvidence.noOutput.rosterColumn(now: now)
        let unknownText = SessionOutputEvidence.unknown("没有会话号记录").rosterColumn(now: now)
        XCTAssertNotEqual(noOutputText, unknownText)
        XCTAssertFalse(unknownText.contains("没有产出"),
                       "「看不出来」被渲染成了「没有产出」：\(unknownText)")
        XCTAssertTrue(unknownText.contains("看不出来"), unknownText)
        XCTAssertTrue(noOutputText.contains("没有产出"), noOutputText)
        // 而且要说清楚是**为什么**看不出来 —— 机长得知道这不是「没干活」。
        XCTAssertTrue(unknownText.contains("没有会话号记录"), unknownText)
    }

    // MARK: - ② 有产出：找得到成绩单 + 最近写入时刻

    func test_claude_找到会话成绩单则报出最近产出时刻() throws {
        let home = tempDir()
        let claudeProjects = home.appendingPathComponent("claude/projects")
        let codexSessions = home.appendingPathComponent("codex/sessions")
        try FileManager.default.createDirectory(at: codexSessions, withIntermediateDirectories: true)
        let wrote = now.addingTimeInterval(-90 * 60)
        write(claudeProjects.appendingPathComponent("-Users-hey-x/abc-123.jsonl"), mtime: wrote)

        let probe = SessionOutputProbe(claudeProjectsDirectory: claudeProjects,
                                       codexSessionsDirectory: codexSessions)
        guard case let .produced(at) = probe.evidence(runnerKind: "claude_code", agentSessionId: "abc-123") else {
            return XCTFail("该找到成绩单")
        }
        XCTAssertEqual(at.timeIntervalSince1970, wrote.timeIntervalSince1970, accuracy: 2)
        let text = SessionOutputEvidence.produced(at: wrote).rosterColumn(now: now)
        XCTAssertTrue(text.contains("1 小时 30 分钟前"), text)
    }

    /// 会话成绩单挂在**哪个项目目录下我们不猜** —— 按会话号扫，不按 cwd 推 slug。
    /// （slug 的编码规则是上游的实现细节，猜错了会静默变成「确实没有产出」。）
    func test_claude_不按工作目录猜路径_换个项目目录照样找得到() throws {
        let home = tempDir()
        let claudeProjects = home.appendingPathComponent("claude/projects")
        try FileManager.default.createDirectory(
            at: home.appendingPathComponent("codex/sessions"), withIntermediateDirectories: true)
        write(claudeProjects.appendingPathComponent("-completely-unrelated-slug/zz-9.jsonl"),
              mtime: now.addingTimeInterval(-30))
        let probe = SessionOutputProbe(claudeProjectsDirectory: claudeProjects,
                                       codexSessionsDirectory: home.appendingPathComponent("codex/sessions"))
        guard case .produced = probe.evidence(runnerKind: "claude_code", agentSessionId: "zz-9") else {
            return XCTFail("按会话号扫应当找得到")
        }
    }

    func test_codex_按threadId在按日期分层的目录里找到rollout() throws {
        let home = tempDir()
        let claudeProjects = home.appendingPathComponent("claude/projects")
        try FileManager.default.createDirectory(at: claudeProjects, withIntermediateDirectories: true)
        let codexSessions = home.appendingPathComponent("codex/sessions")
        let wrote = now.addingTimeInterval(-5 * 60)
        write(codexSessions.appendingPathComponent(
            "2026/09/03/rollout-2026-09-03T22-25-49-01a067a9-620d-7a90-9c37-276d9166363c.jsonl"),
              mtime: wrote)
        let probe = SessionOutputProbe(claudeProjectsDirectory: claudeProjects,
                                       codexSessionsDirectory: codexSessions)
        guard case let .produced(at) = probe.evidence(
            runnerKind: "codex", agentSessionId: "01a067a9-620d-7a90-9c37-276d9166363c") else {
            return XCTFail("该按 threadId 找到 rollout")
        }
        XCTAssertEqual(at.timeIntervalSince1970, wrote.timeIntervalSince1970, accuracy: 2)
    }

    /// threadId 是 rollout 文件名的**尾段**，不许用「包含」去撞：
    /// 另一条 thread 的时间戳里碰巧有同样的字符片段不算命中。
    func test_codex_thread号必须整段匹配而不是子串() throws {
        let home = tempDir()
        let claudeProjects = home.appendingPathComponent("claude/projects")
        try FileManager.default.createDirectory(at: claudeProjects, withIntermediateDirectories: true)
        let codexSessions = home.appendingPathComponent("codex/sessions")
        write(codexSessions.appendingPathComponent("2026/09/03/rollout-2026-09-03T10-00-00-aaaa-bbbb.jsonl"))
        let probe = SessionOutputProbe(claudeProjectsDirectory: claudeProjects,
                                       codexSessionsDirectory: codexSessions)
        XCTAssertEqual(probe.evidence(runnerKind: "codex", agentSessionId: "bbb"), .noOutput)
        XCTAssertNotNil(probe.evidence(runnerKind: "codex", agentSessionId: "aaaa-bbbb").producedAt)
    }

    // MARK: - ③ 取证面自己不在场 → 看不出来（不是「没产出」）

    func test_取证面目录不存在时是看不出来而不是没有产出() throws {
        let home = tempDir()
        let probe = SessionOutputProbe(
            claudeProjectsDirectory: home.appendingPathComponent("claude/projects"),
            codexSessionsDirectory: home.appendingPathComponent("codex/sessions"))
        for (kind, sid) in [("claude_code", "s1"), ("codex", "t1")] {
            switch probe.evidence(runnerKind: kind, agentSessionId: sid) {
            case .unknown: break
            case let other: XCTFail("\(kind) 取证面不在场应当是「看不出来」，实际 \(other)")
            }
        }
    }

    func test_runner不认识时是看不出来() throws {
        let home = tempDir()
        try FileManager.default.createDirectory(
            at: home.appendingPathComponent("claude/projects"), withIntermediateDirectories: true)
        let probe = SessionOutputProbe(
            claudeProjectsDirectory: home.appendingPathComponent("claude/projects"),
            codexSessionsDirectory: home.appendingPathComponent("codex/sessions"))
        for kind in [nil, "", "gemini_cli"] as [String?] {
            switch probe.evidence(runnerKind: kind, agentSessionId: "s1") {
            case .unknown: break
            case let other: XCTFail("runner=\(kind ?? "nil") 应当是「看不出来」，实际 \(other)")
            }
        }
    }

    // MARK: - ④ 点名渲染：每一行都必须有这一列，没有静默省略

    func test_点名每一行都带产出证据列_没有会话的那行也要出现看不出来() throws {
        var snap = CrewSessionsSnapshot()
        snap.updatedAt = "2026-09-06T16:00:00Z"
        snap.crews["c1"] = [
            .init(sessionId: "w-1", name: "阿甲", role: "worker", brief: "改登录", state: "idle"),
            .init(sessionId: "w-2", name: "阿乙", role: "worker", brief: "清死码", state: "idle"),
            .init(sessionId: "w-3", name: "阿丙", role: "worker", brief: "写文档", state: "idle"),
        ]
        let evidence: [String: SessionOutputEvidence] = [
            "w-1": .produced(at: now.addingTimeInterval(-110 * 60)),
            "w-2": .noOutput,
            "w-3": .unknown("没有会话号记录"),
        ]
        let out = snap.renderRoster(crewId: "c1", now: now) { evidence[$0.sessionId]! }
        let lines = out.split(separator: "\n").filter { $0.hasPrefix("- ") }
        XCTAssertEqual(lines.count, 3)
        for line in lines {
            XCTAssertTrue(line.contains("产出"), "这一行没有产出证据列：\(line)")
        }
        XCTAssertTrue(out.contains("1 小时 50 分钟前"), out)
        XCTAssertTrue(out.contains("确实没有产出"), out)
        XCTAssertTrue(out.contains("看不出来"), out)
        // 三条显示状态一模一样（全是「空闲」），产出证据把它们区分开 —— 这一列
        // 存在的全部理由。
        XCTAssertNotEqual(lines[0], lines[1])
        XCTAssertNotEqual(lines[1], lines[2])
    }

    // MARK: - ⑤ list_sessions 端到端：会话号账本 → 取证面 → 点名那一行

    func test_list_sessions把三态真的渲染进点名输出() throws {
        let dir = tempDir()
        let home = tempDir()
        let claudeProjects = home.appendingPathComponent("claude/projects")
        let codexSessions = home.appendingPathComponent("codex/sessions")
        try FileManager.default.createDirectory(at: codexSessions, withIntermediateDirectories: true)

        // 甲：claude，写过东西 → 有产出。
        write(claudeProjects.appendingPathComponent("-slug-a/sid-jia.jsonl"),
              mtime: Date().addingTimeInterval(-42 * 60))
        // 乙：claude，会话号我们记着（起进程前就指定的），成绩单不存在 → 确实没有产出。
        //     这正是 #107 的现场：任务书从没提交过，而状态那一列写着「空闲」。
        // 丙：不在会话号账本里 → 看不出来。
        let ledger = LocalAgentSessionStore(directory: dir)
        ledger.record(crewId: "local-org", sessionId: "w-jia",
                      kind: "claude_code", agentSessionId: "sid-jia")
        ledger.record(crewId: "local-org", sessionId: "w-yi",
                      kind: "claude_code", agentSessionId: "sid-yi-从没写过")

        var snap = CrewSessionsSnapshot()
        snap.updatedAt = "2026-09-06T16:00:00Z"
        snap.crews["local-org"] = [
            .init(sessionId: "w-jia", name: "阿甲", role: "worker", brief: "改登录", state: "idle"),
            .init(sessionId: "w-yi", name: "阿乙", role: "worker", brief: "清死码", state: "idle"),
            .init(sessionId: "w-bing", name: "阿丙", role: "worker", brief: "写文档", state: "idle"),
        ]
        try JSONEncoder().encode(snap)
            .write(to: dir.appendingPathComponent(CrewSessionsSnapshot.fileName))

        let server = McpServer(
            store: LocalWhiteboardStore(directory: dir),
            approvals: LocalApprovalStore(directory: dir),
            control: LocalCrewControlStore(directory: dir),
            crewId: "local-org", sessionId: "cap-1", isCaptain: true,
            sessionLabel: "机长", quotaDirectory: dir,
            agentSessions: ledger,
            outputProbe: SessionOutputProbe(claudeProjectsDirectory: claudeProjects,
                                            codexSessionsDirectory: codexSessions))
        let obj: [String: Any] = ["jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                  "params": ["name": "list_sessions", "arguments": [String: Any]()]]
        let line = String(data: try JSONSerialization.data(withJSONObject: obj), encoding: .utf8)!
        let out = server.handleLine(line) ?? ""

        XCTAssertTrue(out.contains("42 分钟前"), out)
        XCTAssertTrue(out.contains("确实没有产出"), out)
        XCTAssertTrue(out.contains("看不出来"), out)
        // 三个人的状态一模一样（全「空闲」），产出证据把他们区分开。
        XCTAssertEqual(out.components(separatedBy: "空闲").count - 1, 3, out)
    }

    // MARK: - ⑥ 工具描述必须交代这一列什么时候说不出话

    func test_list_sessions工具描述交代了这一列以及它何时说不出话() throws {
        let desc = McpServer.listSessionsToolDescription
        XCTAssertTrue(desc.contains("产出"), desc)
        XCTAssertTrue(desc.contains("看不出来"), desc)
        // 「看不出来 ≠ 没干活」这句必须在描述里，否则机长会照着表象再判一次。
        XCTAssertTrue(desc.contains("不等于"), desc)
    }
}

private extension SessionOutputEvidence {
    var producedAt: Date? { if case let .produced(at) = self { return at }; return nil }
}
