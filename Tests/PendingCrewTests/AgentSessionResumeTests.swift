import XCTest

/// 人类 Todo #28：session 关掉再点恢复要真的接回原对话。
/// 这里钉判定层 —— 记没记会话号、会话还在不在、接不上时的降级与说明。
final class AgentSessionResumeTests: XCTestCase {
    func testNoRecordFallsBackToFresh() {
        let d = AgentSessionResume.decide(recordedId: nil, transcriptAvailable: { _ in true })
        XCTAssertEqual(d, .fresh(reason: .noRecord))
    }

    func testBlankRecordCountsAsNoRecord() {
        let d = AgentSessionResume.decide(recordedId: "   ", transcriptAvailable: { _ in true })
        XCTAssertEqual(d, .fresh(reason: .noRecord))
    }

    func testResumesWhenTranscriptStillThere() {
        var asked: [String] = []
        let d = AgentSessionResume.decide(recordedId: "abc") { id in
            asked.append(id); return true
        }
        XCTAssertEqual(d, .resume(id: "abc"))
        XCTAssertEqual(asked, ["abc"])
    }

    func testMissingTranscriptFallsBackAndKeepsTheLostId() {
        let d = AgentSessionResume.decide(recordedId: "abc", transcriptAvailable: { _ in false })
        XCTAssertEqual(d, .fresh(reason: .transcriptMissing(id: "abc")))
    }

    // MARK: - claude 日志路径

    /// `projectSlug` 只做一件事：把 `/` 和 `.` 换成 `-`（claude 的
    /// `~/.claude/projects/<slug>/` 就是这么拼的）。所以输入里的 `x` 出来还是 `x`。
    ///
    /// 期望值曾经写成 `-Users-hey-…` —— 输入是 `/Users/x/…`，看着像谁做过一次
    /// 「把用户名替换进来」的批量改，只改中了期望值那半边。是**期望值错**，不是
    /// 实现该变：隔壁 `testClaudeTranscriptURL` 用的也是同一个占位用户 `x`。
    func testProjectSlugReplacesSlashesAndDots() {
        XCTAssertEqual(
            AgentSessionResume.projectSlug(forWorkdir: "/Users/x/dev/.pendingcrew/wt"),
            "-Users-x-dev--pendingcrew-wt")
    }

    func testClaudeTranscriptURL() {
        let url = AgentSessionResume.claudeTranscriptURL(
            sessionId: "1111-2222", workdir: "/tmp/x",
            home: URL(fileURLWithPath: "/Users/x"))
        XCTAssertEqual(url.path, "/Users/x/.claude/projects/-tmp-x/1111-2222.jsonl")
    }

    func testNewClaudeSessionIdIsLowercasedUUID() {
        let id = AgentSessionResume.newClaudeSessionId(
            UUID(uuidString: "85CAE521-4146-481A-8AFC-F5C0CD6ED54C")!)
        XCTAssertEqual(id, "85cae521-4146-481a-8afc-f5c0cd6ed54c")
    }

    // MARK: - fail-loud（不装死）

    func testResumeAddsNoNoise() {
        let d = AgentSessionResume.Decision.resume(id: "abc")
        XCTAssertNil(AgentSessionResume.briefNotice(for: d))
        XCTAssertNil(AgentSessionResume.whiteboardNotice(memberName: "阿甲", decision: d))
    }

    func testFreshSaysSoInBriefAndGroup() {
        for d in [AgentSessionResume.Decision.fresh(reason: .noRecord),
                  .fresh(reason: .transcriptMissing(id: "abc"))] {
            let brief = AgentSessionResume.briefNotice(for: d)
            XCTAssertNotNil(brief)
            XCTAssertTrue(brief!.contains("新开的"), brief!)
            let group = AgentSessionResume.whiteboardNotice(memberName: "阿甲", decision: d)
            XCTAssertNotNil(group)
            XCTAssertTrue(group!.contains("阿甲"), group!)
            XCTAssertTrue(group!.contains("接不回来"), group!)
        }
        // 丢失的会话号要出现在首轮说明里（排查时人能对上号）。
        XCTAssertTrue(AgentSessionResume.briefNotice(
            for: .fresh(reason: .transcriptMissing(id: "abc")))!.contains("abc"))
    }

    // MARK: - argv 接线（claude 指定/续跑互斥）

    func testArgvUsesSessionIdForFreshAndResumeForRestart() {
        var fresh = SessionConfig(kind: .claudeCode)
        fresh.newSessionId = "uuid-1"
        XCTAssertEqual(Array(fresh.argv().prefix(2)), ["--session-id", "uuid-1"])

        var restart = SessionConfig(kind: .claudeCode, resumeSessionId: "uuid-1")
        restart.newSessionId = "uuid-2"   // 两者都在时只带 --resume（claude 不接受同时给）
        let a = restart.argv()
        XCTAssertEqual(Array(a.prefix(2)), ["--resume", "uuid-1"])
        XCTAssertFalse(a.contains("--session-id"))
    }
}
