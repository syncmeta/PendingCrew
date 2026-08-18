import XCTest

/// 「更改 crew 工作目录（含 agent 上下文迁移）」的规划层。
/// 这里钉的是**会不会把别人的东西搬走 / 会不会覆盖别人的文件 / 会不会在 session 还
/// 在跑的时候动手** —— 每一条都对应一次真会造成损失的误操作。
final class WorkdirMigrationPlanTests: XCTestCase {

    private let home = URL(fileURLWithPath: "/Users/x")
    private let oldDir = "/Users/x/mono/dev"
    private let newDir = "/Users/x/repo"

    private var oldSlug: String { WorkdirMigrationPlan.projectSlug(forWorkdir: oldDir) }
    private var newSlug: String { WorkdirMigrationPlan.projectSlug(forWorkdir: newDir) }
    private var oldProjectDir: String { "/Users/x/.claude/projects/" + oldSlug }
    private var newProjectDir: String { "/Users/x/.claude/projects/" + newSlug }

    // MARK: - 造输入

    private func crew(_ id: String, _ title: String, dir: String? = nil,
                      parents: [String] = []) -> WorkdirMigrationPlan.CrewInput {
        .init(id: id, title: title, workingDirectory: dir, parentCrewIds: parents)
    }

    private func session(_ crewId: String, _ agentId: String, kind: String = "claude",
                         name: String = "成员") -> WorkdirMigrationPlan.AgentSessionInput {
        .init(crewId: crewId, sessionId: "local-" + agentId, kind: kind,
              agentSessionId: agentId, memberName: name)
    }

    /// 默认探针：新目录存在可写，其余一律「不存在」。用 `existing` 补上要有的路径。
    private func probe(existing: Set<String> = [], directories: Set<String> = [],
                       memory: [String] = [],
                       claudeSource: Set<String> = [], claudeTarget: Set<String> = [],
                       claudeSourceExists: Bool = true, claudeTargetExists: Bool = true,
                       codexOld: String? = "trusted", codexNew: String? = nil)
        -> WorkdirMigrationPlan.Probe {
        let newDir = self.newDir
        let dirs = directories.union([newDir])
        let files = existing.union(dirs)
        return .init(
            pathExists: { files.contains($0) },
            isDirectory: { dirs.contains($0) },
            isWritable: { $0 == newDir },
            listFiles: { $0 == oldProjectDirMemory(self.oldProjectDir) ? memory : [] },
            claudeProjectSettings: { path in
                if path == self.oldDir {
                    return .init(exists: claudeSourceExists, meaningfulKeys: claudeSource)
                }
                if path == self.newDir {
                    return .init(exists: claudeTargetExists, meaningfulKeys: claudeTarget)
                }
                return .init()
            },
            codexTrustLevel: { $0 == self.oldDir ? codexOld : ($0 == self.newDir ? codexNew : nil) })
    }

    private func inputs(crews: [WorkdirMigrationPlan.CrewInput],
                        root: String = "c1",
                        selected: Set<String>? = nil,
                        sessions: [WorkdirMigrationPlan.AgentSessionInput] = [],
                        running: [WorkdirMigrationPlan.RunningSessionInput] = [],
                        newWorkdir: String? = nil) -> WorkdirMigrationPlan.Inputs {
        .init(crews: crews, rootCrewId: root,
              selectedCrewIds: selected ?? Set(crews.map(\.id)),
              newWorkdir: newWorkdir ?? newDir,
              agentSessions: sessions, runningSessions: running, home: home)
    }

    // MARK: - 目标目录校验

    /// 目录不存在就拒绝 —— **绝不偷偷创建**（人选错路径时该被拦住，不是被将错就错）。
    func testMissingNewDirectoryIsRefusedAndNeverCreated() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)], newWorkdir: "/Users/x/nope"),
            probe: probe())
        XCTAssertEqual(p.blockers, [.newWorkdirMissing("/Users/x/nope")])
        XCTAssertFalse(p.isExecutable)
    }

    func testFileInsteadOfDirectoryIsRefused() {
        let target = "/Users/x/afile"
        let pr = WorkdirMigrationPlan.Probe(
            pathExists: { $0 == target }, isDirectory: { _ in false }, isWritable: { _ in true })
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)], newWorkdir: target), probe: pr)
        XCTAssertEqual(p.blockers, [.newWorkdirNotADirectory(target)])
    }

    func testUnwritableDirectoryIsRefused() {
        let pr = WorkdirMigrationPlan.Probe(
            pathExists: { [newDir] in $0 == newDir },
            isDirectory: { [newDir] in $0 == newDir }, isWritable: { _ in false })
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)]), probe: pr)
        XCTAssertEqual(p.blockers, [.newWorkdirNotWritable(newDir)])
    }

    /// 尾部斜杠 / `~` 不该变成「另一个目录」—— slug 是从路径字面量算的，
    /// 归一化没做对就会把上下文搬进一个谁也找不到的 slug。
    func testTrailingSlashIsSameDirectory() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)], newWorkdir: oldDir + "/"),
            probe: .init(pathExists: { _ in true }, isDirectory: { _ in true },
                         isWritable: { _ in true }))
        XCTAssertEqual(p.blockers, [.newWorkdirSameAsCurrent(oldDir)])
    }

    // MARK: - 正在跑的 session

    /// 有 session 在跑就拒绝，而且要**点名**是哪几个（让人知道去停谁）。
    func testRunningSessionsBlockAndAreNamed() {
        let running = [
            WorkdirMigrationPlan.RunningSessionInput(crewId: "c1", sessionId: "s1", displayName: "机长"),
            WorkdirMigrationPlan.RunningSessionInput(crewId: "c2", sessionId: "s2", displayName: "打杂的"),
        ]
        let crews = [crew("c1", "本群", dir: oldDir), crew("c2", "子群", dir: oldDir, parents: ["c1"])]
        let p = WorkdirMigrationPlan.make(inputs(crews: crews, running: running), probe: probe())
        XCTAssertEqual(p.blockers, [.sessionsRunning(running)])
        XCTAssertFalse(p.isExecutable)
    }

    /// 没被勾上的 crew 里在跑的 session 不该拦路 —— 拦的是「要动的那些」。
    func testRunningSessionInUnselectedCrewDoesNotBlock() {
        let crews = [crew("c1", "本群", dir: oldDir), crew("c2", "别人家", dir: oldDir)]
        let running = [WorkdirMigrationPlan.RunningSessionInput(
            crewId: "c2", sessionId: "s2", displayName: "别人家的成员")]
        let p = WorkdirMigrationPlan.make(
            inputs(crews: crews, selected: ["c1"], running: running), probe: probe())
        XCTAssertTrue(p.blockers.isEmpty)
    }

    // MARK: - 共用目录：只搬自己那份

    /// 旧目录是多个 crew 共用的。只有**被迁 crew 的成员**的会话号才准搬走 ——
    /// 留守 crew 的会话必须原地不动，否则它们下次重启全变新脑子。
    func testSharedProjectDirectoryOnlyMovesOwnTranscripts() {
        let crews = [crew("c1", "要搬的", dir: oldDir), crew("c9", "留守的", dir: oldDir)]
        let sessions = [
            session("c1", "aaa", name: "我的成员"),
            session("c9", "zzz", name: "别人的成员"),
        ]
        let p = WorkdirMigrationPlan.make(
            inputs(crews: crews, selected: ["c1"], sessions: sessions),
            probe: probe(existing: [oldProjectDir + "/aaa.jsonl", oldProjectDir + "/zzz.jsonl"]))
        let moves = p.actions.compactMap { action -> String? in
            if case .moveClaudeTranscript(let id, _, _, _) = action { return id }
            return nil
        }
        XCTAssertEqual(moves, ["aaa"])
        XCTAssertEqual(p.affectedMembers, ["我的成员"])
    }

    /// 记忆是整个项目共享的 → 只能**复制**，旧的原样留着给留守 crew 用。
    func testMemoryIsCopiedNotMoved() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)]),
            probe: probe(directories: [oldProjectDir + "/memory"],
                         memory: ["MEMORY.md", "a.md"]))
        let copies = p.actions.compactMap { action -> String? in
            if case .copyClaudeMemoryFile(let rel, _, _) = action { return rel }
            return nil
        }
        XCTAssertEqual(copies, ["MEMORY.md", "a.md"])
        XCTAssertFalse(p.actions.contains {
            if case .moveClaudeTranscript = $0 { return true }
            return false
        })
    }

    // MARK: - 目标已存在：跳过不覆盖

    func testExistingTargetTranscriptIsSkippedNotOverwritten() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)],
                   sessions: [session("c1", "aaa", name: "小明")]),
            probe: probe(existing: [oldProjectDir + "/aaa.jsonl", newProjectDir + "/aaa.jsonl"]))
        XCTAssertEqual(p.claudeTranscriptMoveCount, 0)
        XCTAssertTrue(p.skips.contains(.transcriptTargetExists(
            agentSessionId: "aaa", memberName: "小明", path: newProjectDir + "/aaa.jsonl")))
    }

    func testExistingTargetMemoryFileIsSkippedNotOverwritten() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)]),
            probe: probe(existing: [newProjectDir + "/memory/a.md"],
                         directories: [oldProjectDir + "/memory"],
                         memory: ["a.md", "b.md"]))
        XCTAssertEqual(p.memoryCopyCount, 1)
        XCTAssertTrue(p.skips.contains(.memoryTargetExists(
            relativePath: "a.md", path: newProjectDir + "/memory/a.md")))
    }

    /// 源没了（日志被清 / 更早搬过一次）→ 记账说清楚，别假装搬成功了。
    func testMissingSourceTranscriptIsReported() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)],
                   sessions: [session("c1", "aaa", name: "小明")]),
            probe: probe())
        XCTAssertEqual(p.claudeTranscriptMoveCount, 0)
        XCTAssertTrue(p.skips.contains(.transcriptSourceMissing(
            agentSessionId: "aaa", memberName: "小明", path: oldProjectDir + "/aaa.jsonl")))
    }

    /// 旧目录不存在（整个项目目录都没了）→ 不该炸，也不该编出动作来。
    func testMissingOldProjectDirectoryYieldsNoFileActions() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)],
                   sessions: [session("c1", "aaa")]),
            probe: probe())
        XCTAssertTrue(p.blockers.isEmpty)
        XCTAssertEqual(p.claudeTranscriptMoveCount, 0)
        XCTAssertEqual(p.memoryCopyCount, 0)
        XCTAssertTrue(p.skips.contains(.memoryDirectoryMissing(path: oldProjectDir + "/memory")))
        // 字段还是要改的 —— 目录没了不等于不搬家。
        XCTAssertTrue(p.actions.contains(.setCrewWorkingDirectory(
            crewId: "c1", title: "本群", from: oldDir, to: newDir)))
    }

    /// 会话日志旁边的同名子目录跟着一起走。
    func testSidecarDirectoryMovesWithTranscript() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)],
                   sessions: [session("c1", "aaa", name: "小明")]),
            probe: probe(existing: [oldProjectDir + "/aaa.jsonl"],
                         directories: [oldProjectDir + "/aaa"]))
        XCTAssertTrue(p.actions.contains(.moveClaudeTranscriptSidecar(
            agentSessionId: "aaa", memberName: "小明",
            from: oldProjectDir + "/aaa", to: newProjectDir + "/aaa")))
    }

    // MARK: - 子 crew 连带

    func testSubtreeCollectsDescendants() {
        let crews = [
            crew("c1", "根"), crew("c2", "子A", parents: ["c1"]),
            crew("c3", "子B", parents: ["c1"]), crew("c4", "孙", parents: ["c2"]),
            crew("c9", "无关"),
        ]
        let ids = WorkdirMigrationPlan.subtree(rootId: "c1", crews: crews).map(\.id)
        XCTAssertEqual(Set(ids), ["c1", "c2", "c3", "c4"])
        XCTAssertEqual(ids.first, "c1")
    }

    /// 脏数据成环也要能返回（别把 UI 卡死）。
    func testSubtreeSurvivesCycles() {
        let crews = [crew("c1", "根", parents: ["c2"]), crew("c2", "子", parents: ["c1"])]
        XCTAssertEqual(Set(WorkdirMigrationPlan.subtree(rootId: "c1", crews: crews).map(\.id)),
                       ["c1", "c2"])
    }

    /// 勾上子 crew → 它们各自的字段一起改，各自的会话一起搬。
    func testSelectedChildCrewsMigrateToo() {
        let crews = [crew("c1", "根", dir: oldDir), crew("c2", "子", dir: oldDir, parents: ["c1"])]
        let p = WorkdirMigrationPlan.make(
            inputs(crews: crews, sessions: [session("c2", "bbb", name: "子群成员")]),
            probe: probe(existing: [oldProjectDir + "/bbb.jsonl"]))
        XCTAssertEqual(p.crews.map(\.id), ["c1", "c2"])
        XCTAssertEqual(p.claudeTranscriptMoveCount, 1)
    }

    /// 没勾的子 crew 一个字段都不许动。
    func testUnselectedChildCrewIsUntouched() {
        let crews = [crew("c1", "根", dir: oldDir), crew("c2", "子", dir: oldDir, parents: ["c1"])]
        let p = WorkdirMigrationPlan.make(inputs(crews: crews, selected: ["c1"]), probe: probe())
        XCTAssertEqual(p.crews.map(\.id), ["c1"])
    }

    // MARK: - 动作顺序

    /// crew 字段**最后**改：前面炸了 crew 还指着旧目录，成员照旧接得回上下文。
    func testCrewFieldUpdateComesLast() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)],
                   sessions: [session("c1", "aaa")]),
            probe: probe(existing: [oldProjectDir + "/aaa.jsonl"],
                         directories: [oldProjectDir + "/memory"],
                         memory: ["m.md"], claudeSource: ["hasTrustDialogAccepted"]))
        guard case .setCrewWorkingDirectory = p.actions.last else {
            return XCTFail("最后一个动作应该是改 crew 字段，实际是 \(String(describing: p.actions.last))")
        }
        guard case .copyClaudeProjectSettings = p.actions.first else {
            return XCTFail("第一个动作应该是补信任/权限")
        }
    }

    // MARK: - claude.json 的按键合并

    /// 新路径**已有条目但没接受过信任**是真实存在的状态（实测）。整条「已存在就跳过」
    /// 会把信任弹框留着卡人，所以按键补：缺的补、已有实质值的不动。
    func testClaudeSettingsMergePerKey() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)]),
            probe: probe(claudeSource: ["hasTrustDialogAccepted", "allowedTools"],
                         claudeTarget: ["allowedTools"]))
        XCTAssertTrue(p.actions.contains(.copyClaudeProjectSettings(
            fromPath: oldDir, toPath: newDir, keys: ["hasTrustDialogAccepted"])))
    }

    func testClaudeSettingsSkippedWhenTargetAlreadyComplete() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)]),
            probe: probe(claudeSource: ["hasTrustDialogAccepted"],
                         claudeTarget: ["hasTrustDialogAccepted"]))
        XCTAssertTrue(p.skips.contains(.claudeProjectSettingsAlreadyComplete(path: newDir)))
    }

    func testClaudeSettingsSkippedWhenSourceHasNothing() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)]),
            probe: probe(claudeSourceExists: false))
        XCTAssertTrue(p.skips.contains(.claudeProjectSettingsSourceEmpty(path: oldDir)))
    }

    func testIsMeaningfulTreatsFalseAndEmptyAsNothing() {
        XCTAssertFalse(WorkdirMigrationPlan.isMeaningful(nil))
        XCTAssertFalse(WorkdirMigrationPlan.isMeaningful(false))
        XCTAssertFalse(WorkdirMigrationPlan.isMeaningful([Any]()))
        XCTAssertFalse(WorkdirMigrationPlan.isMeaningful([String: Any]()))
        XCTAssertFalse(WorkdirMigrationPlan.isMeaningful(""))
        XCTAssertTrue(WorkdirMigrationPlan.isMeaningful(true))
        XCTAssertTrue(WorkdirMigrationPlan.isMeaningful(["a"]))
    }

    // MARK: - codex

    /// codex 的会话按日期+threadId 存、resume 显式带新 cwd → 不用搬，但要在预览里说出来。
    func testCodexSessionsAreNotMovedButReported() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)],
                   sessions: [session("c1", "th_1", kind: "codex", name: "codex 成员")]),
            probe: probe())
        XCTAssertEqual(p.claudeTranscriptMoveCount, 0)
        XCTAssertTrue(p.skips.contains(.codexSessionNeedsNoMove(
            agentSessionId: "th_1", memberName: "codex 成员")))
    }

    func testCodexTrustIsAddedForNewPath() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)]), probe: probe())
        XCTAssertTrue(p.actions.contains(
            .copyCodexTrust(fromPath: oldDir, toPath: newDir, trustLevel: "trusted")))
    }

    func testCodexTrustNotOverwrittenWhenNewPathAlreadyTrusted() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)]),
            probe: probe(codexNew: "trusted"))
        XCTAssertTrue(p.skips.contains(.codexTrustTargetExists(path: newDir)))
    }
}

/// 测试内部小工具：记忆目录路径（闭包里要用，避免重复拼串）。
private func oldProjectDirMemory(_ projectDir: String) -> String { projectDir + "/memory" }
