#if os(macOS)
import Foundation

/// 机长 `change_workdir` 的落地编排（app 侧）。
///
/// helper 是离线子进程：读不到 `LocalCrewStore`，也看不到在跑的 run。所以工具只把
/// 一条命令写进控制通道，真正的解析 / 规划 / 执行在这里做，结果经
/// `LocalCrewControlStore.writeCommandResponse` 回给机长（它在 long-poll）。
///
/// 判定全在 `WorkdirMigrationPlan`（纯逻辑、可单测），落地在 `WorkdirMigrationExecutor`
/// （先备份、fail-loud）。这一层只负责：把 crew 账本与在跑的 run 两份现实装进 `Inputs`，
/// 以及决定「预览」还是「执行」。界面那条路（`ChangeWorkingDirectorySheet`）与这条路
/// **共用同一个 `makeInputs`**，免得两边对「谁算拦路」各有一套说法。
@MainActor
enum WorkdirChangeCommand {

    /// 执行一条机长命令，返回要回给机长的文本（预览 / 回执 / 错误）。
    static func run(_ req: WorkdirChangeRequest, runs: [CrewSessionRun]) -> String {
        let crews = LocalCrewStore.shared.workdirCrewInputs()
        guard crews.contains(where: { $0.id == req.crewId }) else {
            return "ERROR: 找不到发起的 crew（\(req.crewId)）。"
        }
        // 只允许动自己这棵子树 —— 别拿这个工具去改别的部门的目录。
        guard let targetId = WorkdirMigrationPlan.resolveTarget(
            hint: req.targetHint ?? "", rootId: req.crewId, crews: crews) else {
            let candidates = WorkdirMigrationPlan.subtree(rootId: req.crewId, crews: crews)
                .map { "「\($0.title)」" }.joined(separator: "、")
            return "ERROR: crew=\(req.targetHint ?? "") 对不上（歧义或不在你名下）。"
                + "可选：\(candidates)。只能改本 crew 及其子 crew。"
        }

        let selected = req.includeChildren
            ? Set(WorkdirMigrationPlan.subtree(rootId: targetId, crews: crews).map(\.id))
            : Set([targetId])
        let inputs = makeInputs(crews: crews, rootCrewId: targetId, selected: selected,
                                newPath: req.newPath, runs: runs,
                                callerSessionId: req.callerSessionId)
        let probe = WorkdirMigrationExecutor.probe(home: homeURL)
        let plan = WorkdirMigrationPlan.make(inputs, probe: probe)

        guard req.confirm else {
            return WorkdirMigrationExecutor.previewText(plan, newWorkdir: inputs.newWorkdir)
        }
        guard plan.isExecutable else {
            return "**没有执行**（带了 confirm，但这份计划现在跑不了）。\n\n"
                + WorkdirMigrationExecutor.previewText(plan, newWorkdir: inputs.newWorkdir)
        }

        let receipt = execute(plan: plan, newWorkdir: WorkdirMigrationPlan.normalize(req.newPath))
        let text = WorkdirMigrationExecutor.receiptText(
            receipt, newWorkdir: WorkdirMigrationPlan.normalize(req.newPath))
        // 回执进群 —— 机长自己看到的是 long-poll 的返回值，人类只看群聊。
        var boards = Set(receipt.crewsUpdated.map(\.id))
        boards.insert(req.crewId)
        boards.insert(targetId)
        for crewId in boards.sorted() {
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: crewId, sessionId: "system", text: text, senderName: "系统")
        }
        return text
    }

    /// 真正落地（备份 → 信任 → 记忆 → crew 字段）。界面与机长工具共用。
    static func execute(plan: WorkdirMigrationPlan.Plan,
                        newWorkdir: String) -> WorkdirMigrationExecutor.Receipt {
        let stamp = ISO8601DateFormatter().string(from: Date())
            .replacingOccurrences(of: ":", with: "-")
        let base = LocalWhiteboardStore.defaultDirectory.deletingLastPathComponent()
        return WorkdirMigrationExecutor.execute(
            plan: plan, home: homeURL,
            backupDirectory: base.appendingPathComponent(
                "backups/workdir-migration-\(stamp)", isDirectory: true),
            extraBackupFiles: [base.appendingPathComponent("local-crews.json")],
            applyCrewWorkingDirectory: { id, path in
                LocalCrewStore.shared.setWorkingDirectory(id, path)
            })
    }

    /// 把两份现实（crew 账本 / 在跑的 run）装进规划输入。
    ///
    /// `runs` 里**所有还活着的**都进 `runningSessions`（含空闲的、含调用者自己）——
    /// 拦不拦路由 `isWorking` + `callerSessionId` 在规划层判，别在这层提前过滤掉信息。
    ///
    /// **这里原本还读一遍 `LocalAgentSessionStore`**，为的是按会话号挑 claude 的
    /// `.jsonl` 搬走。会话不搬了（`--resume` 不按目录找），那次读取一起删了 ——
    /// 它传的是默认 `onIncident`（事故回调本来就被吞掉），账本损坏检测走的是
    /// `loadRowsLocked` 的归档，与这次读取无关，而且每起一个 session 就走一遍，
    /// 频率高几个数量级。**删掉它，损坏检测一点不少。**
    static func makeInputs(crews: [WorkdirMigrationPlan.CrewInput],
                           rootCrewId: String,
                           selected: Set<String>,
                           newPath: String,
                           runs: [CrewSessionRun],
                           callerSessionId: String?) -> WorkdirMigrationPlan.Inputs {
        let scope = Set(WorkdirMigrationPlan.subtree(rootId: rootCrewId, crews: crews).map(\.id))
        let live = runs
            .filter { $0.status == .running && scope.contains($0.crewId) }
            .map { WorkdirMigrationPlan.RunningSessionInput(
                crewId: $0.crewId, sessionId: $0.sessionId,
                displayName: $0.displayName, isWorking: $0.isWorking) }
        return .init(crews: crews, rootCrewId: rootCrewId, selectedCrewIds: selected,
                     newWorkdir: newPath, runningSessions: live,
                     callerSessionId: callerSessionId, home: homeURL)
    }

    static var homeURL: URL { URL(fileURLWithPath: NSHomeDirectory()) }
}
#endif
