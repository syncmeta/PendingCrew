import SwiftUI

private struct CrewdAdminAskGroupsNested: EnvironmentKey { static let defaultValue = false }
private extension EnvironmentValues {
    var adminAskGroupsNested: Bool {
        get { self[CrewdAdminAskGroupsNested.self] }
        set { self[CrewdAdminAskGroupsNested.self] = newValue }
    }
}

/// A normal Inbox/Ask entry. Both presentations reuse the window's original
/// drafts and the existing Human actions; no parallel answer editor is created.
struct CrewdAdminAskGroupsEntry: View {
    @ObservedObject var workspace: CrewdWorkspace
    @ObservedObject var workStore: CrewdWorkStore
    @Binding var drafts: CrewdHumanDrafts
    let crew: String
    var current: @MainActor () -> Bool = { true }
    @ObservedObject private var client: CrewdClient
    @Environment(\.adminAskGroupsNested) private var nested
    @State private var visible = false
    @State private var lease = UUID()
    @State private var presentation: Presentation?
    @State private var closing: UUID?
    @State private var pending: Selection?
    @State private var original: Original?
    @State private var failure: String?
    private struct Presentation: Identifiable {
        let id: UUID
        let crew: String
        let source: Data
        let readScope: UUID
        let store: CrewdAdminAskGroupsStore
    }
    private struct Selection { let presentation: Presentation; let question: String }
    private struct Original: Identifiable {
        let id = UUID()
        let presentation: Presentation
        let target: CrewdHumanTarget
    }
    private struct SourceKey: Hashable { let source: Data; let readScope: UUID; let crew: Data }
    init(workspace: CrewdWorkspace, workStore: CrewdWorkStore, drafts: Binding<CrewdHumanDrafts>, crew: String, current: @escaping @MainActor () -> Bool = { true }) {
        self.workspace = workspace; self.workStore = workStore; _drafts = drafts; self.crew = crew; self.current = current
        _client = ObservedObject(wrappedValue: workspace.client)
    }
    private var identity: Data {
        guard current(), client.readyForWork, let scope = workspace.presentationScope else { return Data() }
        return CrewdAdminAskGroupsStore.identity(.init(scope: scope, observation: workspace.observationGeneration, connection: client.connectionEpoch, crew: workspace.crewID))
    }
    private var sourceKey: SourceKey { .init(source: identity, readScope: workStore.readScope, crew: Data(crew.utf8)) }
    private func sourceCurrent(_ p: Presentation) -> Bool {
        visible && current() && !identity.isEmpty && identity == p.source && workStore.readScope == p.readScope &&
        CrewdAdminAskGroupsWire.bytes(crew, p.crew) && client.readyForWork
    }
    private func groupCurrent(_ p: Presentation) -> Bool {
        sourceCurrent(p) && lease == p.id && presentation?.id == p.id && client.capabilities.contains(CrewdAdminAskGroupsWire.capability)
    }
    private var binding: Binding<Presentation?> {
        Binding(get: { presentation }, set: { value in
            if value == nil { closing = presentation?.id ?? closing }
            presentation = value
        })
    }
    var body: some View {
        Group {
            if !nested, current(), client.readyForWork, client.capabilities.contains(CrewdAdminAskGroupsWire.capability), CrewdAdminAskGroupsWire.opaque(crew) {
                Button("同类待答问题整理") {
                    guard visible, current(), !identity.isEmpty, closing == nil, original == nil else { return }
                    lease = UUID(); pending = nil; failure = nil
                    presentation = .init(id: lease, crew: crew, source: identity, readScope: workStore.readScope, store: workspace.adminAskGroups.makePresentation())
                }.disabled(!visible || presentation != nil || closing != nil || original != nil)
                if let failure { Text(failure).font(.caption).foregroundStyle(.orange) }
            }
        }
        .task(id: sourceKey) {
            retire()
            visible = current() && client.readyForWork && !identity.isEmpty
        }
        .onDisappear { visible = false; retire() }
        .sheet(item: binding, onDismiss: finishGroup) { p in
            CrewdAdminAskGroupsView(workspace: workspace, store: p.store, crew: p.crew, current: { groupCurrent(p) }) { q in
                guard groupCurrent(p) else { return }
                pending = .init(presentation: p, question: q.id)
                closing = p.id; presentation = nil
            }
        }
        .sheet(item: $original) { selected in
            Group {
                if sourceCurrent(selected.presentation), original?.id == selected.id {
                    CrewdHumanActions(store: workStore, drafts: $drafts, target: selected.target, readExactly: true, workspace: workspace,
                        inboxIsCurrent: { sourceCurrent(selected.presentation) && original?.id == selected.id })
                        .environment(\.adminAskGroupsNested, true)
                } else { Text("原问题来源已变化，请从待办重新打开。").padding() }
            }
        }
    }
    private func retire() {
        lease = UUID(); pending = nil; original = nil; failure = nil
        if let p = presentation { closing = p.id; p.store.cancel() }
        presentation = nil
    }
    private func finishGroup() {
        let selection = pending; pending = nil
        let dismissed = closing; closing = nil
        guard let selection, dismissed == selection.presentation.id, lease == selection.presentation.id,
              sourceCurrent(selection.presentation), client.capabilities.contains(CrewdAdminAskGroupsWire.capability) else { return }
        lease = UUID()
        guard workStore.supportsAskResolution, workspace.originalJournalReady else {
            failure = "当前入口尚未具备原问题的提交与回执恢复，请从待办核对原问题。"
            return
        }
        original = .init(presentation: selection.presentation,
            target: .init(key: .init(scope: workStore.selectedCrew, crew: selection.presentation.crew, ask: selection.question)))
    }
}

/// Shared normal Inbox presentation. Opening a question returns to its existing actions.
struct CrewdAdminAskGroupsView: View {
    typealias Wire = CrewdAdminAskGroupsWire
    @ObservedObject var workspace: CrewdWorkspace
    @ObservedObject var store: CrewdAdminAskGroupsStore
    @ObservedObject private var client: CrewdClient
    @ObservedObject private var signals: CrewdAdminAskGroupsStore
    let crew: String
    let current: @MainActor () -> Bool
    let openQuestion: @MainActor (Wire.Question) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var visible = false
    @State private var appearance = UUID()
    @State private var showOther = false
    init(workspace: CrewdWorkspace, store: CrewdAdminAskGroupsStore, crew: String, current: @escaping @MainActor () -> Bool, openQuestion: @escaping @MainActor (Wire.Question) -> Void) {
        self.workspace = workspace; self.store = store; self.crew = crew; self.current = current; self.openQuestion = openQuestion
        _client = ObservedObject(wrappedValue: workspace.client)
        _signals = ObservedObject(wrappedValue: workspace.adminAskGroups)
    }
    private var live: Bool { visible && current() && client.readyForWork && client.capabilities.contains(Wire.capability) && store.owns(crew) }
    private var suggested: [Wire.Pair] { store.items.filter { $0.state == .suggested } }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("同类待答问题整理").font(.headline)
                Spacer()
                Button("关闭") { retire(); dismiss() }
            }
            Text("这里只提示可能需要一起对照的问题。每个原问题、预设答复和默认时限都独立保留，请分别核对处理。")
                .font(.caption).foregroundStyle(.secondary)
            Text("范围是成员提出的普通待答问题；人工待办和带选项的决策问题仍在各自入口处理。")
                .font(.caption).foregroundStyle(.secondary)
            if !current() || !client.readyForWork || !client.capabilities.contains(Wire.capability) {
                Text("来源连接已变化；原问题未作更改。").foregroundStyle(.secondary)
            } else {
                HStack {
                    Button(store.page == nil ? "读取当前整理" : "重新读取") { read() }.disabled(store.busy)
                    if store.busy { ProgressView().controlSize(.small); Button("取消读取") { store.cancel() } }
                    if live, store.page?.nextAfter != nil { Button("继续下一页") { more() }.disabled(store.busy) }
                }
                if let failure = store.failure { Text(failure).foregroundStyle(.orange) }
                if live, store.hasUpdates { Text("有新的整理结果，可重新读取。").font(.caption).foregroundStyle(.secondary) }
                if live, let page = store.page {
                    Text("本次读取：" + Date(timeIntervalSince1970: Double(page.observedAtMs) / 1000).formatted()).font(.caption).foregroundStyle(.secondary)
                    switch page.availability {
                    case .disabled: Text("尚未开启同类待答问题整理。可在判断模型设置中明确开启。")
                    case .unconfigured: Text("整理服务当前不可用。原问题仍可照常处理；读取不会调用模型。")
                    case .archived: Text("该机组已归档，暂不生成待答整理。")
                    case .ready:
                        Text(page.partialCoverage ? "本轮仍有未检查、不可用或结果未知的部分；未出现建议不表示没有同类。" : "当前可核对范围的本轮检查已完成，判断仍只是建议。")
                            .font(.caption).foregroundStyle(.secondary)
                        if page.historicalBaseline { Text("恢复前的问题不会自动重新检查。若要开始新的一轮，请在判断模型设置中明确保存当前选择。").font(.caption).foregroundStyle(.secondary) }
                        Text("相近估计来自模型原生概率，未经新的校准。").font(.caption).foregroundStyle(.secondary)
                        if suggested.isEmpty { Text("当前已读部分没有可展示的同类建议。").foregroundStyle(.secondary) }
                        ScrollView {
                            LazyVStack(alignment: .leading, spacing: 16) {
                                ForEach(suggested) { pair in pairRow(pair) }
                                let others = store.items.filter { $0.state != .suggested }
                                if !others.isEmpty {
                                    DisclosureGroup("其它检查状态", isExpanded: $showOther) {
                                        ForEach(others) { pair in pairRow(pair) }
                                    }
                                }
                            }.frame(maxWidth: .infinity, alignment: .leading)
                        }
                        if page.nextAfter != nil { Text("还有后续记录，可继续读取。").font(.caption).foregroundStyle(.secondary) }
                    }
                } else if !store.busy && store.failure == nil { Text("尚未读取当前整理。").foregroundStyle(.secondary) }
            }
        }
        .padding(16)
        #if os(macOS)
        .frame(minWidth: 500, idealWidth: 620, minHeight: 400, idealHeight: 600)
        #endif
        .task(id: store.sourceKey) {
            guard current() else { retire(); return }
            visible = true; appearance = UUID()
            let token = appearance
            await store.reload(crew: crew, current: { visible && appearance == token && current() })
        }
        .onDisappear { retire() }
        .onChange(of: signals.invalidation) { _, _ in
            if let hint = signals.lastInvalidation { store.handleEvent(hint.name, hint.data, connection: hint.connection) }
        }
    }
    private func stateLabel(_ state: Wire.State) -> String {
        switch state {
        case .prepared: "等待检查"
        case .dispatched: "正在检查"
        case .suggested: "疑似同类"
        case .different: "未建议一起处理"
        case .unavailable: "检查不可用"
        case .unknown: "原检查结果未知，不会自动重发"
        case .stale: "检查已失效，请分别核对原问题"
        }
    }
    @ViewBuilder private func pairRow(_ pair: Wire.Pair) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(pair.first.id + " · " + pair.second.id + "  " + (pair.state == .suggested ? "可能需要一起对照" : stateLabel(pair.state))).font(.subheadline.weight(.semibold))
            if pair.state != .suggested { Text("这不是同类结论，仍可分别打开原问题。").font(.caption).foregroundStyle(.secondary) }
            if pair.excerptTruncated { Text("以下是有限摘要，请打开各原问题核对完整内容及条件。").font(.caption).foregroundStyle(.orange) }
            questionRow(pair.first)
            Divider()
            questionRow(pair.second)
        }.padding(12).background(Color.secondary.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
    }
    @ViewBuilder private func questionRow(_ question: Wire.Question) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(question.id).font(.caption.monospacedDigit()).foregroundStyle(.secondary)
            Text(verbatim: question.question).textSelection(.enabled)
            if let preset = question.default { Text("预设答复：" + preset).font(.caption).textSelection(.enabled) }
            if let due = question.defaultDueAtMs { Text("默认时限：" + Date(timeIntervalSince1970: Double(due) / 1000).formatted()).font(.caption).foregroundStyle(.secondary) }
            Button("核对并处理原问题") {
                guard live else { return }
                openQuestion(question)
                retire(); dismiss()
            }.disabled(!live)
        }
    }
    private func retire() { visible = false; appearance = UUID(); store.cancel() }
    private func read() {
        let token = appearance
        Task { await store.reload(crew: crew, current: { visible && appearance == token && current() }) }
    }
    private func more() {
        let token = appearance
        Task { await store.more(current: { visible && appearance == token && current() }) }
    }
}
