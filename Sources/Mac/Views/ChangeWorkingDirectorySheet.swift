#if os(macOS)
import SwiftUI
import AppKit

/// 「更改 crew 工作目录（含 agent 上下文迁移）」的界面：选目录 → **迁移预览**（dry-run）
/// → 确认执行 → 群里发一条如实回执。
///
/// 为什么要有它：crew 的工作目录此前只在建 crew 那一刻定下，仓库一搬家就只能去手改
/// `local-crews.json` —— 而那份账 app 启动时读一次、之后整份覆写，运行中手改会被吞掉。
/// 更要命的是 agent 侧上下文按路径分家（见 `WorkdirMigrationPlan` 的文件头），
/// 光改字段等于把所有成员的记忆和会话丢在旧路径上。
///
/// 判定全在 `WorkdirMigrationPlan`（纯逻辑、可单测），落地在 `WorkdirMigrationExecutor`
/// （先备份、fail-loud）。这一层只负责编排和展示。
struct ChangeWorkingDirectorySheet: View {
    let crewId: String
    let crewTitle: String

    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var crewStore: CrewStore
    @EnvironmentObject private var sessionRunner: CrewSessionRunner

    /// 本 crew 自己 + 全部后代（勾选框的数据源）。
    @State private var subtree: [WorkdirMigrationPlan.CrewInput] = []
    /// 勾了哪些一起迁（默认全勾；本 crew 恒勾且不可取消）。
    @State private var selected: Set<String> = []
    @State private var newDir: String = ""
    @State private var plan: WorkdirMigrationPlan.Plan?
    @State private var executing = false
    @State private var receipt: WorkdirMigrationExecutor.Receipt?

    private var currentDir: String {
        subtree.first { $0.id == crewId }?.workingDirectory ?? ""
    }

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    picker
                    if !subtree.isEmpty && subtree.count > 1 { crewChecklist }
                    if let receipt { receiptSection(receipt) }
                    else if let plan, !newDir.isEmpty { previewSection(plan) }
                }
                .padding(20)
            }
            Divider()
            footer
        }
        .frame(minWidth: 560, minHeight: 480)
        .onAppear(perform: reload)
    }

    // MARK: - 选目录

    @ViewBuilder
    private var picker: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("更改工作目录").font(.title3.weight(.semibold))
            LabeledContent("当前") {
                Text(currentDir.isEmpty ? "（未设置）" : currentDir)
                    .font(.system(.callout, design: .monospaced))
                    .textSelection(.enabled)
            }
            HStack(spacing: 8) {
                Text("新目录").foregroundStyle(.secondary)
                Text(newDir.isEmpty ? "（还没选）" : newDir)
                    .font(.system(.callout, design: .monospaced))
                    .textSelection(.enabled)
                Spacer()
                Button("选择…") { chooseDirectory() }
                    .disabled(executing)
            }
            Text("只认已经存在的目录 —— 选错了会被拦住，这里不会替你创建。")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    private func chooseDirectory() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = false
        panel.prompt = "选择"
        panel.message = "选一个已经存在的目录作为这个 crew 的新工作目录"
        if !currentDir.isEmpty {
            panel.directoryURL = URL(fileURLWithPath: currentDir).deletingLastPathComponent()
        }
        guard panel.runModal() == .OK, let url = panel.url else { return }
        newDir = WorkdirMigrationPlan.normalize(url.path)
        receipt = nil
        recomputePlan()
    }

    // MARK: - 子 crew 勾选

    @ViewBuilder
    private var crewChecklist: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("连同子 crew 一起迁").font(.headline)
            ForEach(subtree, id: \.id) { crew in
                Toggle(isOn: Binding(
                    get: { selected.contains(crew.id) },
                    set: { on in
                        if crew.id == crewId { return } // 本 crew 恒迁
                        if on { selected.insert(crew.id) } else { selected.remove(crew.id) }
                        receipt = nil
                        recomputePlan()
                    })) {
                    HStack(spacing: 6) {
                        Text(crew.title + (crew.id == crewId ? "（本 crew）" : ""))
                        if let dir = crew.workingDirectory, dir != currentDir {
                            Text(dir).font(.caption).foregroundStyle(.secondary)
                                .lineLimit(1).truncationMode(.middle)
                        }
                    }
                }
                .disabled(crew.id == crewId || executing)
            }
        }
    }

    // MARK: - 预览（dry-run）

    @ViewBuilder
    private func previewSection(_ plan: WorkdirMigrationPlan.Plan) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            if !plan.blockers.isEmpty {
                GroupBox {
                    VStack(alignment: .leading, spacing: 4) {
                        ForEach(Array(plan.blockers.enumerated()), id: \.offset) { _, b in
                            Label(blockerText(b), systemImage: "exclamationmark.octagon.fill")
                                .foregroundStyle(.red)
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                } label: { Text("这些先解决，才能迁").font(.headline) }
            }

            GroupBox {
                VStack(alignment: .leading, spacing: 4) {
                    row("要改工作目录的 crew", "\(plan.crews.count) 个")
                    row("claude 会话搬过去", "\(plan.claudeTranscriptMoveCount) 个")
                    row("claude 项目记忆复制", "\(plan.memoryCopyCount) 个文件（旧目录原样留着）")
                    row("目录信任 / 工具权限", trustSummary(plan))
                    if !plan.affectedMembers.isEmpty {
                        row("影响的成员", plan.affectedMembers.joined(separator: "、"))
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            } label: { Text("会做这些").font(.headline) }

            let notable = plan.skips.compactMap(WorkdirMigrationExecutor.skipLine)
            if !notable.isEmpty {
                GroupBox {
                    VStack(alignment: .leading, spacing: 4) {
                        ForEach(Array(notable.enumerated()), id: \.offset) { _, line in
                            Text("• " + line).font(.callout).foregroundStyle(.secondary)
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                } label: { Text("搬不了 / 不搬的").font(.headline) }
            }
        }
    }

    private func trustSummary(_ plan: WorkdirMigrationPlan.Plan) -> String {
        var parts: [String] = []
        for action in plan.actions {
            switch action {
            case .copyClaudeProjectSettings(_, _, let keys):
                parts.append("claude 补 " + keys.joined(separator: "、"))
            case .copyCodexTrust(_, _, let level):
                parts.append("codex 补 trust_level=\(level)")
            default: break
            }
        }
        return parts.isEmpty ? "无需改动（源没有，或新路径已经有了）" : parts.joined(separator: "；")
    }

    private func row(_ title: String, _ value: String) -> some View {
        HStack(alignment: .top) {
            Text(title).foregroundStyle(.secondary)
            Spacer(minLength: 12)
            Text(value).multilineTextAlignment(.trailing)
        }
        .font(.callout)
    }

    private func blockerText(_ b: WorkdirMigrationPlan.Blocker) -> String {
        switch b {
        case .emptyNewWorkdir: return "还没选新目录。"
        case .newWorkdirMissing(let p): return "目录不存在：\(p)"
        case .newWorkdirNotADirectory(let p): return "这不是一个目录：\(p)"
        case .newWorkdirNotWritable(let p): return "目录不可写：\(p)"
        case .newWorkdirSameAsCurrent(let p): return "新目录和当前目录是同一个：\(p)"
        case .rootCrewNotFound(let id): return "找不到这个 crew：\(id)"
        case .noCrewSelected: return "一个 crew 都没勾。"
        case .sessionsRunning(let running):
            let names = running.map { "「\($0.displayName)」" }.joined(separator: "、")
            return "还有 session 在跑，先停了再迁：\(names)"
        }
    }

    // MARK: - 回执

    @ViewBuilder
    private func receiptSection(_ receipt: WorkdirMigrationExecutor.Receipt) -> some View {
        GroupBox {
            Text(WorkdirMigrationExecutor.receiptText(receipt, newWorkdir: newDir))
                .font(.callout)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        } label: {
            Label(receipt.succeeded ? "迁完了" : "中途停了",
                  systemImage: receipt.succeeded ? "checkmark.circle.fill" : "exclamationmark.triangle.fill")
                .foregroundStyle(receipt.succeeded ? .green : .orange)
                .font(.headline)
        }
    }

    // MARK: - 底部按钮

    @ViewBuilder
    private var footer: some View {
        HStack {
            if receipt == nil {
                Text("动手前会把 `~/.claude.json`、`~/.codex/config.toml`、crew 账本各备份一份。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Button(receipt == nil ? "取消" : "完成") { dismiss() }
                .keyboardShortcut(.cancelAction)
            if receipt == nil {
                Button(executing ? "迁移中…" : "确认迁移") { migrate() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(executing || plan?.isExecutable != true)
            }
        }
        .padding(16)
    }

    // MARK: - 编排

    private func reload() {
        let rows = LocalCrewStore.shared.workdirRows().map {
            WorkdirMigrationPlan.CrewInput(id: $0.id, title: $0.title,
                                           workingDirectory: $0.workingDirectory,
                                           parentCrewIds: $0.parentCrewIds)
        }
        subtree = WorkdirMigrationPlan.subtree(rootId: crewId, crews: rows)
        selected = Set(subtree.map(\.id))
        recomputePlan()
    }

    private func recomputePlan() {
        guard !newDir.isEmpty else { plan = nil; return }
        plan = WorkdirMigrationPlan.make(makeInputs(),
                                         probe: WorkdirMigrationExecutor.probe(home: homeURL))
    }

    private func makeInputs() -> WorkdirMigrationPlan.Inputs {
        let allRows = LocalCrewStore.shared.workdirRows().map {
            WorkdirMigrationPlan.CrewInput(id: $0.id, title: $0.title,
                                           workingDirectory: $0.workingDirectory,
                                           parentCrewIds: $0.parentCrewIds)
        }
        let ids = Set(subtree.map(\.id))
        let sessions = LocalAgentSessionStore.shared.list()
            .filter { ids.contains($0.crewId) }
            .map { record in
                WorkdirMigrationPlan.AgentSessionInput(
                    crewId: record.crewId, sessionId: record.sessionId, kind: record.kind,
                    agentSessionId: record.agentSessionId,
                    memberName: memberName(crewId: record.crewId, sessionId: record.sessionId))
            }
        let running = sessionRunner.runs
            .filter { $0.status == .running && ids.contains($0.crewId) }
            .map { WorkdirMigrationPlan.RunningSessionInput(
                crewId: $0.crewId, sessionId: $0.sessionId, displayName: $0.displayName) }
        return .init(crews: allRows, rootCrewId: crewId, selectedCrewIds: selected,
                     newWorkdir: newDir, agentSessions: sessions,
                     runningSessions: running, home: homeURL)
    }

    /// 成员显示名：优先 crew 账本里登记的持久成员名，没有就退回会话号前缀。
    private func memberName(crewId: String, sessionId: String) -> String {
        if let m = LocalCrewStore.shared.sessionMembers(crewId: crewId)
            .first(where: { $0.sessionId == sessionId }), !m.displayName.isEmpty {
            return m.displayName
        }
        if let run = sessionRunner.runs.first(where: { $0.sessionId == sessionId }) {
            return run.displayName
        }
        return "session " + String(sessionId.prefix(6))
    }

    private var homeURL: URL { URL(fileURLWithPath: NSHomeDirectory()) }

    private func migrate() {
        // 按下按钮的这一刻重算一遍 —— 预览到确认之间可能有人起了 session / 删了目录。
        let fresh = WorkdirMigrationPlan.make(
            makeInputs(), probe: WorkdirMigrationExecutor.probe(home: homeURL))
        plan = fresh
        guard fresh.isExecutable else { return }

        executing = true
        let stamp = ISO8601DateFormatter().string(from: Date())
            .replacingOccurrences(of: ":", with: "-")
        let backup = LocalWhiteboardStore.defaultDirectory
            .deletingLastPathComponent()
            .appendingPathComponent("backups/workdir-migration-\(stamp)", isDirectory: true)
        let crewLedger = LocalWhiteboardStore.defaultDirectory
            .deletingLastPathComponent()
            .appendingPathComponent("local-crews.json")

        // 文件动作都是同卷 rename / 小文件复制，量级在毫秒 —— 就地跑，
        // 免得把 `LocalCrewStore`（@MainActor）的写入甩到别的线程上。
        let result = WorkdirMigrationExecutor.execute(
            plan: fresh, home: homeURL, backupDirectory: backup,
            extraBackupFiles: [crewLedger],
            applyCrewWorkingDirectory: { id, path in
                LocalCrewStore.shared.setWorkingDirectory(id, path)
            })
        executing = false
        receipt = result

        let text = WorkdirMigrationExecutor.receiptText(result, newWorkdir: newDir)
        for crew in Set(fresh.crews.map(\.id)).union([crewId]) {
            crewStore.postSystemNotice(crewId: crew, text: text)
        }
        Task { await crewStore.refreshDetail(crewId) }
    }
}
#endif
