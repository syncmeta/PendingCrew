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
                        running: [WorkdirMigrationPlan.RunningSessionInput] = [],
                        caller: String? = nil,
                        newWorkdir: String? = nil) -> WorkdirMigrationPlan.Inputs {
        .init(crews: crews, rootCrewId: root,
              selectedCrewIds: selected ?? Set(crews.map(\.id)),
              newWorkdir: newWorkdir ?? newDir,
              runningSessions: running,
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
    /// 归一化没做对就会把记忆/信任补进一个谁也找不到的 slug。归一化后与当前目录相同 →
    /// 不是拦路条件，而是「这个 crew 已经在新目录上了」，安静地什么都不做。
    func testTrailingSlashNormalizesToSameDirectory() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)], newWorkdir: oldDir + "/"),
            probe: .init(pathExists: { _ in true }, isDirectory: { _ in true },
                         isWritable: { _ in true }))
        XCTAssertTrue(p.blockers.isEmpty)
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

    // MARK: - 已经在新目录上

    /// 目录已经是目标 → 没有源目录可取，安静地什么都不做（**不再有清扫模式**：
    /// 会话日志不搬了，也就没有「上一轮留下的尾巴」这回事；`previousWorkingDirectory`
    /// 从此不参与规划）。别让人点一个什么都不干的按钮。
    func testCrewAlreadyAtTargetYieldsNothing() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: newDir)]),
            probe: probe(existing: [oldProjectDir + "/aaa.jsonl"]))
        XCTAssertTrue(p.blockers.isEmpty)
        XCTAssertTrue(p.actions.isEmpty)
        XCTAssertFalse(p.isExecutable)
        XCTAssertTrue(p.skips.contains(.crewAlreadyAtNewWorkdir(crewId: "c1", title: "本群")))
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

    // MARK: - 共用目录：只复制不搬走

    /// 记忆是整个项目共享的（旧目录是多个 crew 共用的）→ 只能**复制**，
    /// 旧的原样留着给留守 crew 用。**规划层不许再产生任何「移动」类动作**。
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
    }

    // MARK: - 目标已存在：跳过不覆盖

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

    /// 旧目录不存在（整个项目目录都没了）→ 不该炸，也不该编出文件动作来。
    func testMissingOldProjectDirectoryYieldsNoFileActions() {
        let p = WorkdirMigrationPlan.make(
            inputs(crews: [crew("c1", "本群", dir: oldDir)]),
            probe: probe())
        XCTAssertTrue(p.blockers.isEmpty)
        XCTAssertEqual(p.memoryCopyCount, 0)
        XCTAssertTrue(p.skips.contains(.memoryDirectoryMissing(path: oldProjectDir + "/memory")))
        // 字段还是要改的 —— 目录没了不等于不搬家。
        XCTAssertTrue(p.actions.contains(.setCrewWorkingDirectory(
            crewId: "c1", title: "本群", from: oldDir, to: newDir)))
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

    /// 勾上子 crew → 它们各自的字段一起改。
    func testSelectedChildCrewsMigrateToo() {
        let crews = [crew("c1", "根", dir: oldDir), crew("c2", "子", dir: oldDir, parents: ["c1"])]
        let p = WorkdirMigrationPlan.make(inputs(crews: crews), probe: probe())
        XCTAssertEqual(p.crews.map(\.id), ["c1", "c2"])
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
            inputs(crews: [crew("c1", "本群", dir: oldDir)]),
            probe: probe(directories: [oldProjectDir + "/memory"],
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
