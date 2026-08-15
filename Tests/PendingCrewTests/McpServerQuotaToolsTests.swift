import XCTest
// McpServer + LocalCrewControlStore + AgentQuota 直接编进 PendingCrewTests target。

/// session 面三工具单测（#455）：get_quota 读快照、schedule_wakeup 入队、
/// set_session_profile 入队与卫生。
final class McpServerQuotaToolsTests: XCTestCase {
    private func tmp() -> URL {
        let d = FileManager.default.temporaryDirectory.appendingPathComponent("mcpquota-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true); return d
    }
    private func makeServer(dir: URL) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "local-q", sessionId: "worker-abc", isCaptain: false,
                  sessionLabel: "测试", quotaDirectory: dir)
    }
    private func call(_ s: McpServer, _ name: String, _ args: [String: Any]) -> String {
        let obj: [String: Any] = ["jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                  "params": ["name": name, "arguments": args]]
        let line = String(data: try! JSONSerialization.data(withJSONObject: obj), encoding: .utf8)!
        return s.handleLine(line) ?? ""
    }

    // MARK: - get_quota

    func testGetQuotaReadsSnapshotFile() throws {
        let dir = tmp()
        let file = AgentQuotaFile(
            claude: AgentQuotaSnapshot(
                agent: "claude",
                windows: [AgentQuotaWindow(label: "session", usedPercent: 80, resetsAt: "4:39am"),
                          AgentQuotaWindow(label: "周(全模型)", usedPercent: 23, resetsAt: nil)],
                fetchedAt: "2026-07-05T01:00:00Z", subscriptionPlan: "Max 5x",
                subscriptionPlanSource: "claude_config",
                activities: [
                    AgentQuotaActivity(periodLabel: "Last 24h", requests: 758, sessions: 4),
                    AgentQuotaActivity(periodLabel: "Last 7d", requests: 1500, sessions: 9),
                ]),
            codex: AgentQuotaSnapshot(
                agent: "codex",
                windows: [AgentQuotaWindow(label: "周窗", usedPercent: 9, resetsAt: nil)],
                fetchedAt: "2026-07-05T01:00:00Z", subscriptionPlan: "Plus",
                subscriptionPlanSource: "codex_rate_limits"))
        try JSONEncoder().encode(file).write(to: dir.appendingPathComponent("quota.json"))
        let out = call(makeServer(dir: dir), "get_quota", [:])
        XCTAssertTrue(out.contains("session 已用 80%"))
        XCTAssertTrue(out.contains("4:39am"))
        XCTAssertTrue(out.contains("Codex"))
        XCTAssertTrue(out.contains("9%"))
        XCTAssertTrue(out.contains("订阅档位 Max 5x"), out)
        XCTAssertTrue(out.contains("订阅档位 Plus"), out)
        XCTAssertTrue(out.contains("758 requests · 4 sessions"), out)
        XCTAssertTrue(out.contains("1500 requests · 9 sessions"), out)
        XCTAssertTrue(out.contains("说不出绝对剩余"), out)
    }

    func testGetQuotaMissingFileSaysNoData() {
        let out = call(makeServer(dir: tmp()), "get_quota", [:])
        XCTAssertTrue(out.contains("暂无额度数据"))
    }

    func testGetQuotaMarksExpiredAndStaleSnapshotsInsteadOfPresentingThemAsCurrent() throws {
        let dir = tmp()
        let stale = AgentQuotaSnapshot(
            agent: "codex",
            windows: [.init(label: "周窗", usedPercent: 87,
                            resetsAt: "2020-01-01T00:00:00Z")],
            fetchedAt: "2020-01-01T00:00:00Z", producedAt: "2020-01-01T00:00:00Z")
        try JSONEncoder().encode(AgentQuotaFile(codex: stale))
            .write(to: dir.appendingPathComponent("quota.json"))
        let out = call(makeServer(dir: dir), "get_quota", [:])
        XCTAssertTrue(out.contains("档位未知"), out)
        XCTAssertTrue(out.contains("重置时刻都已过去"), out)
        XCTAssertTrue(out.contains("不是当前值"), out)
    }

    func testGetQuotaKeepsAgentWhoseSnapshotIsMissingButFailureExists() throws {
        let dir = tmp()
        try JSONEncoder().encode(AgentQuotaFile(claudeError: "读不到（真实样本解析失败）"))
            .write(to: dir.appendingPathComponent("quota.json"))
        let out = call(makeServer(dir: dir), "get_quota", [:])
        XCTAssertTrue(out.contains("Claude Code：读不到"), out)
    }

    // MARK: - schedule_wakeup

    func testScheduleWakeupAfterMinutesEnqueues() {
        let dir = tmp()
        let before = Date()
        _ = call(makeServer(dir: dir), "schedule_wakeup",
                 ["after_minutes": 240, "note": "额度重置后继续 refactor-auth"])
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].kind, "schedule_wakeup")
        XCTAssertEqual(cmds[0].sessionId, "worker-abc")
        XCTAssertEqual(cmds[0].note, "额度重置后继续 refactor-auth")
        let fireAt = McpServer.parseISO(cmds[0].fireAt ?? "")
        XCTAssertNotNil(fireAt)
        // 240 分钟后（±2 分钟容差）。
        let delta = fireAt!.timeIntervalSince(before) - 240 * 60
        XCTAssertLessThan(abs(delta), 120)
    }

    func testScheduleWakeupRejectsMissingNoteAndBadTime() {
        let dir = tmp()
        let s = makeServer(dir: dir)
        XCTAssertTrue(call(s, "schedule_wakeup", ["after_minutes": 30]).contains("ERROR"))
        XCTAssertTrue(call(s, "schedule_wakeup", ["note": "x"]).contains("ERROR"))            // 无时间
        XCTAssertTrue(call(s, "schedule_wakeup", ["note": "x", "after_minutes": 0]).contains("ERROR"))
        XCTAssertTrue(call(s, "schedule_wakeup", ["note": "x", "at": "2020-01-01T00:00:00Z"]).contains("ERROR")) // 过去
        XCTAssertTrue(LocalCrewControlStore(directory: dir).drainCommands().isEmpty)
    }

    func testScheduleWakeupAcceptsFutureISO() {
        let dir = tmp()
        let future = ISO8601DateFormatter().string(from: Date().addingTimeInterval(3600))
        let out = call(makeServer(dir: dir), "schedule_wakeup", ["at": future, "note": "醒来跑测试"])
        XCTAssertFalse(out.contains("ERROR"))
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].fireAt.flatMap(McpServer.parseISO)?.timeIntervalSince1970 ?? 0,
                       McpServer.parseISO(future)!.timeIntervalSince1970, accuracy: 1)
    }

    // MARK: - set_session_profile

    func testSetProfileEnqueuesWithOwnSessionId() {
        let dir = tmp()
        let out = call(makeServer(dir: dir), "set_session_profile", ["model": "haiku", "effort": "LOW"])
        // 回执必须如实说「还没生效」——老回执谎称「claude 立即生效」，机长信了
        // 继续用旧模型跑到撞额度上限（#544）。
        XCTAssertTrue(out.contains("还没生效"), out)
        XCTAssertFalse(out.contains("立即生效"), out)
        let cmds = LocalCrewControlStore(directory: dir).drainCommands()
        XCTAssertEqual(cmds.count, 1)
        XCTAssertEqual(cmds[0].kind, "set_profile")
        XCTAssertEqual(cmds[0].sessionId, "worker-abc")
        XCTAssertEqual(cmds[0].model, "haiku")
        XCTAssertEqual(cmds[0].effort, "low")
    }

    func testSetProfileRejectsEmptyAndSentences() {
        let dir = tmp()
        let s = makeServer(dir: dir)
        XCTAssertTrue(call(s, "set_session_profile", [:]).contains("ERROR"))
        XCTAssertTrue(call(s, "set_session_profile", ["model": "the best model please"]).contains("ERROR"))
        XCTAssertTrue(LocalCrewControlStore(directory: dir).drainCommands().isEmpty)
    }
}
