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
                      previous: String? = nil,
                      parents: [String] = []) -> WorkdirMigrationPlan.CrewInput {
        .init(id: id, title: title, workingDirectory: dir,
              previousWorkingDirectory: previous, parentCrewIds: parents)
    }

    /// `kind` 的默认值**故意写死字面量 `"claude_code"`** —— 账本里存的就是
    /// `LocalCodingAgentKind.rawValue`。这里原本默认 `"claude"`，于是整套测试全绿、
    /// 真迁移时一条会话都没搬（2026-08-19）。不要改成从 enum 取值：测试要钉的正是
    /// 「磁盘上那个字符串」，从同一个 enum 取值会让 rawValue 被改名时测试跟着漂。
    private func session(_ crewId: String, _ agentId: String, kind: String = "claude_code",
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
                        caller: String? = nil,
                        newWorkdir: String? = nil) -> WorkdirMigrationPlan.Inputs {
        .init(crews: crews, rootCrewId: root,
              selectedCrewIds: selected ?? Set(crews.map(\.id)),
              newWorkdir: newWorkdir ?? newDir,
              agentSessions: sessions, runningSessions: running,
              callerSessionId: caller, home: home)
    }

    private func run(_ crewId: String, _ sessionId: String, _ name: String,
                     working: Bool) -> WorkdirMigrationPlan.RunningSessionInput {
        .init(crewId: crewId, sessionId: sessionId, displayName: name, isWorking: working)
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
    /// 归一化没做对就会把上下文搬进一个谁也找不到的 slug。归一化后与当前目录相同 →
    /// 不是拦路条件（那正是清扫模式），而是「这个 crew 已经在新目录上了」。
    func testTrailingSlashNormalizesToSameDirectory() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)], newWorkdir: oldDir + "/"),
            probe: .init(pathExists: { _ in true }, isDirectory: { _ in true },
                         isWritable: { _ in true }))
        XCTAssertTrue(p.blockers.isEmpty)
        XCTAssertTrue(p.isSweep)
        XCTAssertTrue(p.skips.contains(.crewAlreadyAtNewWorkdir(crewId: "c1", title: "本群")))
        XCTAssertFalse(p.actions.contains {
            if case .setCrewWorkingDirectory = $0 { return true }
            return false
        })
    }

    // MARK: - 正在跑的 session

    /// **正在干活**的成员拦路，而且要点名（让人知道去停谁）。
    func testBusySessionsBlockAndAreNamed() {
        let busy = [run("c1", "s1", "打杂的", working: true),
                    run("c2", "s2", "子群的", working: true)]
        let crews = [crew("c1", "本群", dir: oldDir), crew("c2", "子群", dir: oldDir, parents: ["c1"])]
        let p = WorkdirMigrationPlan.make(inputs(crews: crews, running: busy), probe: probe())
        XCTAssertEqual(p.blockers, [.sessionsBusy(busy)])
        XCTAssertFalse(p.isExecutable)
    }

    /// **调用者自己不算拦路** —— 机长就是本 crew 里一个正在跑的 session，
    /// 沿用「有 session 在跑就拒绝」它永远调不动这个工具。
    func testCallerItselfDoesNotBlock() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)],
                   running: [run("c1", "captain", "机长", working: true)],
                   caller: "captain"),
            probe: probe())
        XCTAssertTrue(p.blockers.isEmpty, "机长自己不该拦住自己：\(p.blockers)")
        XCTAssertTrue(p.isExecutable)
    }

    /// 空闲的 worker 不拦路（它没在写东西，等它下次重启就换目录了）。
    func testIdleWorkerDoesNotBlock() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)],
                   running: [run("c1", "w1", "闲着的", working: false)],
                   caller: "captain"),
            probe: probe())
        XCTAssertTrue(p.blockers.isEmpty)
    }

    /// 没被勾上的 crew 里在跑的 session 不该拦路 —— 拦的是「要动的那些」。
    func testRunningSessionInUnselectedCrewDoesNotBlock() {
        let crews = [crew("c1", "本群", dir: oldDir), crew("c2", "别人家", dir: oldDir)]
        let p = WorkdirMigrationPlan.make(
            inputs(crews: crews, selected: ["c1"],
                   running: [run("c2", "s2", "别人家的成员", working: true)]),
            probe: probe())
        XCTAssertTrue(p.blockers.isEmpty)
    }

    // MARK: - 活着的成员：会话留待清扫

    /// 成员还活着 → 它正往那份 `.jsonl` 里写，现在搬会搬到半截。不搬，记进「留待清扫」。
    func testLiveMemberTranscriptIsHeldBackForSweep() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)],
                   sessions: [session("c1", "aaa", name: "还活着的")],
                   running: [run("c1", "local-aaa", "还活着的", working: false)],
                   caller: "captain"),
            probe: probe(existing: [oldProjectDir + "/aaa.jsonl"]))
        XCTAssertEqual(p.claudeTranscriptMoveCount, 0)
        XCTAssertTrue(p.skips.contains(.sessionStillLive(
            agentSessionId: "aaa", memberName: "还活着的")))
        XCTAssertEqual(p.pendingSweepMembers, ["还活着的"])
        // 字段照改 —— 迁移本身不该被「有人活着」挡住。
        XCTAssertTrue(p.actions.contains(.setCrewWorkingDirectory(
            crewId: "c1", title: "本群", from: oldDir, to: newDir)))
    }

    /// **调用者自己的**会话同样不搬（机长正写着自己那份日志）。
    func testCallerOwnTranscriptIsHeldBackToo() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)],
                   sessions: [session("c1", "cap", name: "机长")],
                   running: [run("c1", "local-cap", "机长", working: true)],
                   caller: "local-cap"),
            probe: probe(existing: [oldProjectDir + "/cap.jsonl"]))
        XCTAssertTrue(p.blockers.isEmpty)
        XCTAssertEqual(p.claudeTranscriptMoveCount, 0)
        XCTAssertEqual(p.pendingSweepMembers, ["机长"])
    }

    // MARK: - 清扫模式（幂等：同一个路径再调一次）

    /// 迁过之后再调一次：目录已经是新的，靠 `previousWorkingDirectory` 回旧目录
    /// 把此时已经停下的成员的会话补搬过去 —— 而不是报「已经迁过了」什么都不做。
    func testSweepModeMovesRemainingTranscriptsAfterMembersStopped() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: newDir, previous: oldDir)],
                   sessions: [session("c1", "aaa", name: "已经停了的")]),
            probe: probe(existing: [oldProjectDir + "/aaa.jsonl"]))
        XCTAssertTrue(p.isSweep)
        XCTAssertTrue(p.blockers.isEmpty)
        XCTAssertEqual(p.claudeTranscriptMoveCount, 1)
        XCTAssertTrue(p.isExecutable)
        // 字段已经是新的 → 不重复改。
        XCTAssertTrue(p.skips.contains(.crewAlreadyAtNewWorkdir(crewId: "c1", title: "本群")))
    }

    /// 清扫模式下成员还活着 → 还是不搬，也不该编出动作来（可以一直重复调）。
    func testSweepModeWithNothingLeftIsNotExecutable() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: newDir, previous: oldDir)],
                   sessions: [session("c1", "aaa", name: "还活着的")],
                   running: [run("c1", "local-aaa", "还活着的", working: false)],
                   caller: "captain"),
            probe: probe(existing: [oldProjectDir + "/aaa.jsonl"],
                         claudeSourceExists: false, codexOld: nil))
        XCTAssertTrue(p.isSweep)
        XCTAssertFalse(p.isExecutable, "没有可做的动作时不该让人点执行")
        XCTAssertEqual(p.pendingSweepMembers, ["还活着的"])
    }

    /// 没记过旧路径（从没迁过）+ 目录已经相同 → 找不到源，安静地什么都不做。
    func testSameDirectoryWithoutPreviousYieldsNothing() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: newDir)],
                   sessions: [session("c1", "aaa")]),
            probe: probe(existing: [oldProjectDir + "/aaa.jsonl"]))
        XCTAssertTrue(p.blockers.isEmpty)
        XCTAssertFalse(p.isExecutable)
    }

    // MARK: - 目标解析（机长 `crew` 参数）

    func testResolveTargetDefaultsToRootWhenHintEmpty() {
        let crews = [crew("c1", "根"), crew("c2", "子", parents: ["c1"])]
        XCTAssertEqual(WorkdirMigrationPlan.resolveTarget(hint: "", rootId: "c1", crews: crews), "c1")
    }

    func testResolveTargetMatchesTitleAndPrefixWithinSubtree() {
        let crews = [crew("c1", "根"), crew("c2", "驾驶舱改造", parents: ["c1"])]
        XCTAssertEqual(
            WorkdirMigrationPlan.resolveTarget(hint: "驾驶舱改造", rootId: "c1", crews: crews), "c2")
        XCTAssertEqual(
            WorkdirMigrationPlan.resolveTarget(hint: "驾驶舱", rootId: "c1", crews: crews), "c2")
        XCTAssertEqual(
            WorkdirMigrationPlan.resolveTarget(hint: "c2", rootId: "c1", crews: crews), "c2")
    }

    /// **不能拿这个工具去改别的部门** —— 子树外的一律解析不到。
    func testResolveTargetRefusesCrewsOutsideSubtree() {
        let crews = [crew("c1", "根"), crew("c9", "别的部门")]
        XCTAssertNil(WorkdirMigrationPlan.resolveTarget(hint: "别的部门", rootId: "c1", crews: crews))
        XCTAssertNil(WorkdirMigrationPlan.resolveTarget(hint: "c9", rootId: "c1", crews: crews))
    }

    func testResolveTargetRefusesAmbiguousTitles() {
        let crews = [crew("c1", "根"), crew("c2", "同名", parents: ["c1"]),
                     crew("c3", "同名", parents: ["c1"])]
        XCTAssertNil(WorkdirMigrationPlan.resolveTarget(hint: "同名", rootId: "c1", crews: crews))
    }

    // MARK: - 共用目录：只搬自己那份
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

    // MARK: - runner 名（2026-08-19 真迁移暴露的漏判）

    /// 账本里 claude 那条腿存的是 `claude_code`。判定必须认它，否则整批会话被
    /// 当成「不认识的 runner」跳过 —— 首次真迁移就是这样搬了 0 条。
    func testClaudeCodeRunnerNameIsRecognized() {
        XCTAssertEqual(LocalCodingAgentKind.claudeCode.rawValue, "claude_code",
                       "账本写的是 rawValue；改了它就要同步 WorkdirMigrationPlan 的判定")
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)],
                   sessions: [session("c1", "aaa", kind: "claude_code", name: "成员")]),
            probe: probe(existing: [oldProjectDir + "/aaa.jsonl"]))
        XCTAssertEqual(p.claudeTranscriptMoveCount, 1)
        XCTAssertFalse(p.skips.contains(.unknownAgentKind(kind: "claude_code", memberName: "成员")),
                       "claude_code 不该被当成不认识的 runner")
    }

    /// 旧账本里可能还留着 `claude`（写入方换成 rawValue 之前的行）——一并认，别漏搬。
    func testLegacyClaudeRunnerNameStillRecognized() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)],
                   sessions: [session("c1", "aaa", kind: "claude", name: "成员")]),
            probe: probe(existing: [oldProjectDir + "/aaa.jsonl"]))
        XCTAssertEqual(p.claudeTranscriptMoveCount, 1)
    }

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
