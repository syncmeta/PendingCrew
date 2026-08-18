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
                            Label(WorkdirMigrationExecutor.blockerText(b), systemImage: "exclamationmark.octagon.fill")
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
                    row("目录信任 / 工具权限", WorkdirMigrationExecutor.trustSummary(plan))
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

    private func row(_ title: String, _ value: String) -> some View {
        HStack(alignment: .top) {
            Text(title).foregroundStyle(.secondary)
            Spacer(minLength: 12)
            Text(value).multilineTextAlignment(.trailing)
        }
        .font(.callout)
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
        let rows = LocalCrewStore.shared.workdirCrewInputs()
        subtree = WorkdirMigrationPlan.subtree(rootId: crewId, crews: rows)
        selected = Set(subtree.map(\.id))
        recomputePlan()
    }

    private func recomputePlan() {
        guard !newDir.isEmpty else { plan = nil; return }
        plan = WorkdirMigrationPlan.make(makeInputs(),
                                         probe: WorkdirMigrationExecutor.probe(home: homeURL))
    }

    /// 与机长工具那条路**共用同一个构造**（`WorkdirChangeCommand.makeInputs`）——
    /// 免得两边对「谁算活着 / 谁算拦路」各有一套说法。人面走界面时没有「调用者」，
    /// 所以 callerSessionId 传 nil：任何在干活的 session 都拦路。
    private func makeInputs() -> WorkdirMigrationPlan.Inputs {
        WorkdirChangeCommand.makeInputs(
            crews: LocalCrewStore.shared.workdirCrewInputs(),
            rootCrewId: crewId, selected: selected, newPath: newDir,
            runs: sessionRunner.runs, callerSessionId: nil)
    }

    private var homeURL: URL { URL(fileURLWithPath: NSHomeDirectory()) }

    private func migrate() {
        // 按下按钮的这一刻重算一遍 —— 预览到确认之间可能有人起了 session / 删了目录。
        let fresh = WorkdirMigrationPlan.make(
            makeInputs(), probe: WorkdirMigrationExecutor.probe(home: homeURL))
        plan = fresh
        guard fresh.isExecutable else { return }

        executing = true
        // 文件动作都是同卷 rename / 小文件复制，量级在毫秒 —— 就地跑，
        // 免得把 `LocalCrewStore`（@MainActor）的写入甩到别的线程上。
        let result = WorkdirChangeCommand.execute(plan: fresh, newWorkdir: newDir)
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
