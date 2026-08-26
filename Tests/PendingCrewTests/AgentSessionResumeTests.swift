import XCTest

/// 人类 Todo #28 / #68：session 关掉再点恢复要真的接回原对话。
/// 这里钉判定层 —— 记没记会话号、agent 拒了怎么办、接不上时的降级与说明。
final class AgentSessionResumeTests: XCTestCase {

    // MARK: - decide：只看账本记没记（Todo #68：不许再预测 claude 能不能续）

    func testNoRecordFallsBackToFresh() {
        XCTAssertEqual(AgentSessionResume.decide(recordedId: nil), .fresh(reason: .noRecord))
    }

    func testBlankRecordCountsAsNoRecord() {
        XCTAssertEqual(AgentSessionResume.decide(recordedId: "   "), .fresh(reason: .noRecord))
    }

    /// Todo #68 的核心行为变化：**记了就带着 `--resume` 去起**，不再事先判断
    /// 「日志在不在我们以为的那个目录里」。那道门今天在本机把 69/339 条（20%）
    /// claude 本来续得回来的会话挡在了门外。
    func testRecordedIdAlwaysResumesWithoutPredicting() {
        XCTAssertEqual(AgentSessionResume.decide(recordedId: "abc"), .resume(id: "abc"))
        XCTAssertEqual(AgentSessionResume.decide(recordedId: "  abc  "), .resume(id: "abc"))
    }

    // MARK: - claude 拒绝续跑：吃它的原话

    func testRecognisesClaudeRefusal() {
        let screen = """
        $ claude --resume 7ab71389-c174-4efa-bf27-2b8dffb052aa
        No conversation found with session ID: 7ab71389-c174-4efa-bf27-2b8dffb052aa
        """
        XCTAssertEqual(
            AgentSessionResume.claudeResumeRejection(
                inScreenText: screen, resumedId: "7ab71389-c174-4efa-bf27-2b8dffb052aa"),
            "No conversation found with session ID: 7ab71389-c174-4efa-bf27-2b8dffb052aa")
    }

    /// 别的会话号被拒了，不算这一个被拒 —— 判据必须同时对上那句话**和**这个 id。
    func testRefusalForAnotherIdIsNotOurs() {
        let screen = "No conversation found with session ID: 别人的-id"
        XCTAssertNil(AgentSessionResume.claudeResumeRejection(
            inScreenText: screen, resumedId: "我们的-id"))
    }

    /// **真故障不许被吞成「会话没了」。** CLI 没装 / 参数写错 / 额度用尽同样是秒退，
    /// 但屏上没有那句话 —— 认不出就该如实报启动失败，而不是悄悄不带 `--resume` 重起
    /// （那才是把记忆真弄丢，且长得跟今天的病一模一样）。
    func testOtherStartupFailuresAreNotMistakenForRefusal() {
        for screen in ["zsh: command not found: claude",
                       "error: unknown option '--sessionid'",
                       "Claude usage limit reached. Your limit will reset at 3pm."] {
            XCTAssertNil(AgentSessionResume.claudeResumeRejection(
                inScreenText: screen, resumedId: "abc"), screen)
        }
    }

    // MARK: - claude 日志路径（只作诊断，不作决策）

    /// `projectSlug` 曾经只把 `/` 和 `.` 换成 `-`。**实际上 claude 把每一个不是
    /// ASCII 字母/数字/连字符的字符都换成 `-`**（2026-08-26 实测：`…/scratchpad/e1_a`
    /// 落进 `…-scratchpad-e1-a/`）。当时 21 个 worktree 里有 4 个名字带下划线，
    /// 按旧规则一个都对不上。
    func testProjectSlugReplacesSlashesAndDots() {
        XCTAssertEqual(
            AgentSessionResume.projectSlug(forWorkdir: "/Users/x/dev/.pendingcrew/wt"),
            "-Users-x-dev--pendingcrew-wt")
    }

    func testProjectSlugReplacesEveryNonAlphanumericCharacter() {
        XCTAssertEqual(
            AgentSessionResume.projectSlug(forWorkdir: "/tmp/wt/Todo-68-in_progress"),
            "-tmp-wt-Todo-68-in-progress")
        XCTAssertEqual(
            AgentSessionResume.projectSlug(forWorkdir: "/tmp/a b+c#d"),
            "-tmp-a-b-c-d")
    }

    func testClaudeTranscriptURL() {
        let url = AgentSessionResume.claudeTranscriptURL(
            sessionId: "1111-2222", workdir: "/tmp/x",
            home: URL(fileURLWithPath: "/Users/x"))
        XCTAssertEqual(url.path, "/Users/x/.claude/projects/-tmp-x/1111-2222.jsonl")
    }

    /// 按会话号在整个 projects 树下找 —— **跟工作目录无关**（这正是 claude 自己的做法：
    /// 把日志挪到一个不相干的目录，`--resume` 照样接得上）。
    func testLocateScansEveryProjectDirectory() {
        let root = URL(fileURLWithPath: "/h/.claude/projects")
        let dirs = ["-a-b", "-完全无关的-slug", "-c-d"].map {
            root.appendingPathComponent($0, isDirectory: true)
        }
        let target = dirs[1].appendingPathComponent("sess-1.jsonl")
        let lookup = AgentSessionResume.TranscriptLookup(
            projectDirectories: { dirs }, fileExists: { $0 == target })
        XCTAssertEqual(
            AgentSessionResume.locateClaudeTranscript(sessionId: "sess-1", lookup: lookup),
            target)
    }

    func testLocateReturnsNilWhenNowhereOnDisk() {
        let lookup = AgentSessionResume.TranscriptLookup(
            projectDirectories: { [URL(fileURLWithPath: "/h/.claude/projects/-a")] },
            fileExists: { _ in false })
        XCTAssertNil(
            AgentSessionResume.locateClaudeTranscript(sessionId: "sess-1", lookup: lookup))
    }

    /// 诊断只是旁证：盘上**有**也要照说 —— 那说明是别的原因，别把它藏起来。
    func testDiagnosisReportsBothDirections() {
        let dir = URL(fileURLWithPath: "/h/.claude/projects/-a")
        let present = AgentSessionResume.TranscriptLookup(
            projectDirectories: { [dir] }, fileExists: { _ in true })
        XCTAssertTrue(AgentSessionResume
            .resumeRejectionDiagnosis(sessionId: "s1", lookup: present)
            .contains("/h/.claude/projects/-a/s1.jsonl"))
        let absent = AgentSessionResume.TranscriptLookup(
            projectDirectories: { [dir] }, fileExists: { _ in false })
        XCTAssertTrue(AgentSessionResume
            .resumeRejectionDiagnosis(sessionId: "s1", lookup: absent)
            .contains("一个都找不到"))
    }

    func testNewClaudeSessionIdIsLowercasedUUID() {
        let id = AgentSessionResume.newClaudeSessionId(
            UUID(uuidString: "85CAE521-4146-481A-8AFC-F5C0CD6ED54C")!)
        XCTAssertEqual(id, "85cae521-4146-481a-8afc-f5c0cd6ed54c")
    }

    // MARK: - Todo #68：这一轮跑在哪个目录

    private func crewDir() -> URL { URL(fileURLWithPath: "/crew/shared") }

    /// 记着的目录还在 → 回它自己的 worktree 跑（旧行为是一律拉回 crew 共享目录）。
    func testRestartGoesBackToTheRecordedWorktree() {
        let dir = AgentSessionResume.restartDirectory(
            recorded: "/wt/todo-68", crewDirectory: crewDir(),
            isDirectory: { $0 == "/wt/todo-68" })
        XCTAssertEqual(dir.path, "/wt/todo-68")
    }

    /// **worktree 被删是常态**（本机 62 个记过的只剩 10 个）—— 必须回落到 crew 共享
    /// 目录，不能硬用：往一个不存在的目录里起进程是起不来的。
    func testRestartFallsBackWhenTheRecordedDirectoryIsGone() {
        let dir = AgentSessionResume.restartDirectory(
            recorded: "/wt/已经被删了", crewDirectory: crewDir(), isDirectory: { _ in false })
        XCTAssertEqual(dir.path, "/crew/shared")
    }

    /// 旧记录没有这个字段（nil）/ 空串 → 按今天的行为走，不许炸。
    func testRestartFallsBackForLegacyRecordsWithoutAWorkingDirectory() {
        for recorded in [nil, "", "   "] as [String?] {
            XCTAssertEqual(
                AgentSessionResume.restartDirectory(
                    recorded: recorded, crewDirectory: crewDir(), isDirectory: { _ in true }).path,
                "/crew/shared", String(describing: recorded))
        }
    }

    /// 记的是文件不是目录 → 同样回落（别把进程往一个文件里起）。
    func testRestartFallsBackWhenTheRecordedPathIsNotADirectory() {
        XCTAssertEqual(
            AgentSessionResume.restartDirectory(
                recorded: "/wt/a-file", crewDirectory: crewDir(),
                isDirectory: { _ in false }).path,
            "/crew/shared")
    }

    // MARK: - fail-loud（不装死）

    func testResumeAddsNoNoise() {
        let d = AgentSessionResume.Decision.resume(id: "abc")
        XCTAssertNil(AgentSessionResume.briefNotice(for: d))
        XCTAssertNil(AgentSessionResume.whiteboardNotice(memberName: "阿甲", decision: d))
    }

    func testFreshSaysSoInBriefAndGroup() {
        let rejected = AgentSessionResume.Decision.fresh(reason: .agentRejectedResume(
            id: "abc", agentSaid: "No conversation found with session ID: abc",
            diagnosis: "顺带：盘上确实没有。"))
        for d in [AgentSessionResume.Decision.fresh(reason: .noRecord), rejected] {
            let brief = AgentSessionResume.briefNotice(for: d)
            XCTAssertNotNil(brief)
            XCTAssertTrue(brief!.contains("新开的"), brief!)
            let group = AgentSessionResume.whiteboardNotice(memberName: "阿甲", decision: d)
            XCTAssertNotNil(group)
            XCTAssertTrue(group!.contains("阿甲"), group!)
            XCTAssertTrue(group!.contains("接不回来"), group!)
        }
        // 丢失的会话号要出现在首轮说明里（排查时人能对上号）。
        XCTAssertTrue(AgentSessionResume.briefNotice(for: rejected)!.contains("abc"))
    }

    /// **那句话的依据整个换掉了**：从「我们以为文件不在那儿」变成「claude 说没有这个会话」。
    /// 所以两条通知里都必须出现 claude 的原话，且**不许**再出现旧那句误导性的诊断。
    func testRefusalNoticesQuoteClaudeVerbatimAndKeepTheDiagnosisSeparate() {
        let said = "No conversation found with session ID: abc"
        let d = AgentSessionResume.Decision.fresh(reason: .agentRejectedResume(
            id: "abc", agentSaid: said, diagnosis: "顺带：盘上确实没有。"))
        let brief = AgentSessionResume.briefNotice(for: d)!
        let group = AgentSessionResume.whiteboardNotice(memberName: "阿甲", decision: d)!
        XCTAssertTrue(brief.contains(said), brief)
        XCTAssertTrue(group.contains(said), group)
        XCTAssertTrue(group.contains("顺带：盘上确实没有。"), group)
        XCTAssertFalse(group.contains("在本机已找不到"), group)
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
