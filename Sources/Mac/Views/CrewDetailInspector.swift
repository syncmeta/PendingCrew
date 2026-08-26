#if os(macOS)
import SwiftUI
import AppKit

/// crew 详情 / 设置面板（spec §9 把它从中栏挪出来，中栏改放群聊）。
/// 显示 crew 元数据 + 父/子 crew(DAG) + 「谁说了算 谁负责」+ 工作目录。
/// 机长那一栏已按用户定调去掉 —— 机长在群聊/成员列表里本来就看得见,
/// 信息页再列一遍是重复。
/// 以 inspector sheet 形式从 `CrewCenterView` 顶栏 ⓘ 打开。
struct CrewDetailInspector: View {
    let detail: CrewDetail
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var crewStore: CrewStore
    /// 「更改工作目录」要看本 crew 下有没有 session 在跑（在跑就拒绝迁）。
    @EnvironmentObject private var sessionRunner: CrewSessionRunner

    /// 「挂到父 crew」操作的进行中 / 失败态。
    @State private var dagBusy = false
    @State private var dagError: String?

    /// 「新建子 crew」sheet 显隐 —— 建完自动 attachParent 到本 crew 之下。
    @State private var showingChildCrewSheet = false
    /// 「更改工作目录…」sheet（含 agent 上下文迁移，见 `ChangeWorkingDirectorySheet`）。
    @State private var showingWorkdirSheet = false
    @State private var displayedTitle = ""
    @State private var titleDraft = ""
    @State private var editingTitle = false

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    header
                    parentDAGSection
                    sharesSection
                }
                .padding(20)
            }
        }
        .onAppear {
            displayedTitle = detail.crew.title
            titleDraft = detail.crew.title
        }
        .frame(minWidth: 420, minHeight: 440)
        .toolbar {
            ToolbarItem(placement: .principal) { Text("crew 详情").font(.headline) }
            ToolbarItem(placement: .automatic) {
                Button {
                    showingChildCrewSheet = true
                } label: {
                    Label("新建子 crew", systemImage: "plus")
                }
                .help("在当前 crew 之下新建一个子 crew")
            }
            ToolbarItem(placement: .confirmationAction) { Button("完成") { dismiss() } }
        }
        .sheet(isPresented: $showingChildCrewSheet) {
            CreateCrewSheet(parentCrewId: detail.crew.id)
                .environmentObject(crewStore)
        }
        .sheet(isPresented: $showingWorkdirSheet) {
            ChangeWorkingDirectorySheet(crewId: detail.crew.id, crewTitle: detail.crew.title)
                .environmentObject(crewStore)
                .environmentObject(sessionRunner)
        }
    }

    @ViewBuilder
    private var header: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                if editingTitle {
                    TextField("crew 名称", text: $titleDraft)
                        .textFieldStyle(.roundedBorder)
                    Button("保存") { saveTitle() }
                        .disabled(titleDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    Button("取消") {
                        titleDraft = displayedTitle
                        editingTitle = false
                    }
                } else {
                    Text(displayedTitle.isEmpty ? detail.crew.title : displayedTitle)
                        .font(.title2.weight(.semibold))
                    Button { editingTitle = true } label: {
                        Image(systemName: "pencil")
                    }
                    .buttonStyle(.borderless)
                    .help("手动改名")
                }
            }
            HStack(spacing: 12) {
                Label(detail.crew.runtimeLocationKind?.shortLabel ?? detail.crew.runtimeLocation,
                      systemImage: detail.crew.runtimeLocationKind?.displayIcon ?? "questionmark.circle")
            }
            .font(.callout)
            .foregroundStyle(.secondary)
            HStack(spacing: 6) {
                Image(systemName: "folder").foregroundStyle(.secondary)
                let wd = detail.crew.workingDirectory ?? ""
                Text(wd.isEmpty ? "（未设置工作目录）" : wd)
                    .font(.system(.callout, design: .monospaced))
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if !wd.isEmpty {
                    Button { revealInFinder(wd) } label: {
                        Image(systemName: "arrow.up.right.square")
                    }
                    .buttonStyle(.borderless)
                    .help("在 Finder 中打开")
                }
                // 仓库搬家用的：改目录 + 把 agent 侧上下文（会话/记忆/目录信任）一起迁过去。
                Button("更改工作目录…") { showingWorkdirSheet = true }
                    .buttonStyle(.link)
                    .help("换一个工作目录，并把成员的 agent 会话、项目记忆、目录信任一起迁过去")
            }
        }
    }

    private func saveTitle() {
        let title = titleDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty else { return }
        displayedTitle = title
        editingTitle = false
        Task { await crewStore.renameCrewFromUI(detail.crew.id, title: title) }
    }

    // MARK: - 本地 DAG 父边（「挂到父 crew」）

    /// 本 crew 在 `crewStore` 里的实时 summary —— 父边的事实源是本地
    /// `parentCrewIds`(edge `detail.parents` 对本地 crew 恒空)。
    private var liveSummary: CrewSummary? {
        crewStore.crews.first(where: { $0.id == detail.crew.id })
    }

    /// 当前已挂的父 crew(实时)。
    private var currentParents: [CrewSummary] {
        let ids = liveSummary?.parentCrewIds ?? []
        return ids.compactMap { pid in crewStore.crews.first(where: { $0.id == pid }) }
    }

    /// 可挂的候选父 crew:排除自己、已挂的父、以及会成环的(本 crew 的后代)。
    private var attachCandidates: [CrewSummary] {
        let selfId = detail.crew.id
        let already = Set(liveSummary?.parentCrewIds ?? [])
        let descendants = descendantIds(of: selfId)
        return crewStore.crews.filter { c in
            c.id != selfId && !already.contains(c.id) && !descendants.contains(c.id)
        }
    }

    /// 从 `crewStore.crews` 反推某 crew 的全部后代(child = 其 parentCrewIds 含
    /// 该 crew)。带 visited 自保防脏数据成环。挂到后代会成环,故从候选里剔除。
    private func descendantIds(of crewId: String) -> Set<String> {
        var result: Set<String> = []
        var queue: [String] = [crewId]
        while let current = queue.popLast() {
            for c in crewStore.crews where c.parentCrewIds.contains(current) {
                if result.insert(c.id).inserted { queue.append(c.id) }
            }
        }
        return result
    }

    @ViewBuilder
    private var parentDAGSection: some View {
        HStack {
            sectionHeader("父 crew")
            Spacer()
            Menu {
                if attachCandidates.isEmpty {
                    Text("没有可挂的 crew").foregroundStyle(.secondary)
                } else {
                    ForEach(attachCandidates) { cand in
                        Button(cand.title) { attach(parentId: cand.id) }
                    }
                }
            } label: {
                Label("挂到父 crew", systemImage: "arrow.up.to.line")
                    .font(.caption)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
            .disabled(dagBusy || attachCandidates.isEmpty)
        }
        if currentParents.isEmpty {
            Text("(根 crew，未挂到任何父)").foregroundStyle(.secondary)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(currentParents) { parent in
                    HStack(spacing: 8) {
                        Image(systemName: "arrow.up.forward.square").foregroundStyle(.secondary)
                        Text(parent.title).lineLimit(1)
                        Spacer()
                        Button {
                            detach(parentId: parent.id)
                        } label: {
                            Image(systemName: "minus.circle")
                        }
                        .buttonStyle(.borderless)
                        .disabled(dagBusy)
                        .help("解绑这个父 crew")
                    }
                }
            }
        }
        if let dagError {
            Text(dagError)
                .font(.caption)
                .foregroundStyle(.red)
                .textSelection(.enabled)
        }
    }

    private func attach(parentId: String) {
        dagBusy = true
        dagError = nil
        let crewId = detail.crew.id
        Task {
            defer { dagBusy = false }
            do {
                try await crewStore.attachParent(crewId: crewId, parentCrewId: parentId)
            } catch {
                dagError = error.localizedDescription
            }
        }
    }

    private func detach(parentId: String) {
        dagBusy = true
        dagError = nil
        let crewId = detail.crew.id
        Task {
            defer { dagBusy = false }
            do {
                try await crewStore.detachParent(crewId: crewId, parentCrewId: parentId)
            } catch {
                dagError = error.localizedDescription
            }
        }
    }

    @ViewBuilder
    private var sharesSection: some View {
        sectionHeader("本 Crew 谁说了算 谁负责")
        if detail.shares.isEmpty {
            Text("(尚未结算)").foregroundStyle(.secondary)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(detail.shares, id: \.subjectId) { share in
                    HStack(spacing: 8) {
                        Image(systemName: share.kind == "group_account" ? "person.3" : "person.crop.circle")
                            .foregroundStyle(.secondary)
                        Text(share.displayName.isEmpty ? share.subjectId : share.displayName)
                            .lineLimit(1)
                        Spacer()
                        Text("\(sharePercent(share.shareBps))%")
                            .font(.callout.monospacedDigit())
                    }
                }
            }
        }
    }

    private func sectionHeader(_ text: String) -> some View {
        Text(text)
            .font(.subheadline.weight(.semibold))
            .foregroundStyle(.secondary)
            .padding(.top, 4)
    }

    private func sharePercent(_ bps: Int) -> String {
        String(format: "%.2f", Double(bps) / 100.0)
    }

    private func revealInFinder(_ path: String) {
        let expanded = (path as NSString).expandingTildeInPath
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: expanded)])
    }
}
#endif
