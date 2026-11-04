import SwiftUI

struct CrewdPatrolView: View {
    @ObservedObject var store: CrewdWorkStore
    let workspace: CrewdWorkspace?
    @Environment(\.dismiss) private var dismiss
    @State private var backgroundStatus: CrewdValue?
    @State private var statusError: String?
    @State private var statusTask: Task<Void,Never>?
    @State private var statusRequest = UUID()
    @State private var scan = "work"
    @State private var archiveTarget:String?
    @State private var casesPresented=false
    @State private var similarPresented = false
    @State private var archived:[CrewdValue]=[]
    @State private var archivedLoaded=false
    @State private var rows: [CrewdValue] = []
    @State private var cursor: String?
    @State private var loaded = false
    @State private var error: String?
    @State private var task: Task<Void, Never>?
    @State private var request = UUID()
    private var field: String { scan == "work" ? "items" : scan }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("当前机组巡检").font(.headline)
            Text("手动巡检保存本轮观察。下方的即时读取可核对最新事实。巡检不会派活、关闭事项或归档群。")
                .font(.caption).foregroundStyle(.secondary)
            backgroundSection
            if let workspace, store.capabilities.contains("work.patrolStart") {
                manualSection(workspace)
            }
            Button("行政处理记录") {casesPresented=true}
            if store.capabilities.contains("adminCase.similarRequirements") {
                Button("审阅相近需求") { similarPresented = true }
            }
            Button("读取当前机组已归档群") {readArchived()}
            if archivedLoaded && archived.isEmpty {Text("当前机组没有已归档群。")}
            if !archived.isEmpty {
                ScrollView {
                    LazyVStack(alignment:.leading) {
                        ForEach(Array(archived.enumerated()),id:\.offset) {_,channel in
                            Button("核实并解档：" + (channel["name"].string ?? channel["id"].string ?? "群")) {archiveTarget=channel["id"].string}
                        }
                    }
                }.frame(maxHeight:120)
            }
            Picker("检查范围", selection: $scan) {
                Text("工作").tag("work")
                Text("需求").tag("entries")
                Text("群归档候选").tag("channels")
            }.pickerStyle(.segmented)
            HStack {
                Button(loaded ? "重新读取事实" : "读取当前事实") { reset(); load() }.disabled(task != nil)
                if task != nil { Button("取消读取") { cancel() } }
                if loaded && cursor != nil { Button("继续下一页") { load() }.disabled(task != nil) }
                Spacer()
                Button("关闭") { dismiss() }
            }
            if let error { Text(error).foregroundStyle(.orange) }
            if loaded { Text(cursor == nil ? "本类检查已读完，共 \(rows.count) 个对象。" : "已检查 \(rows.count) 个对象，尚有后续页。") }
            else { Text("本类尚未完成首轮读取。").foregroundStyle(.secondary) }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                        VStack(alignment: .leading) {
                            Text(row["id"].string ?? "未知对象").font(.headline).textSelection(.enabled)
                            Text(summary(row))
                            if scan == "channels" {Button("核实归档条件") {archiveTarget=row["id"].string}}
                        }.frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
            }
        }.padding().frame(minWidth: 540, minHeight: 400)
        .sheet(isPresented:Binding(get:{archiveTarget != nil},set:{if !$0 {archiveTarget=nil}})) {
            if let channel=archiveTarget {CrewdChannelArchiveView(store:store,channel:channel,crew:store.selectedCrew)}
        }
        .sheet(isPresented:$casesPresented) {CrewdAdminCasesView(store:store)}
        .sheet(isPresented: $similarPresented) { CrewdSimilarRequirementsView(store: store, workspace: workspace) }
        .onChange(of: scan) { _, _ in reset() }
        .onChange(of: store.readScope) { _, _ in reset(); clearStatus(); archiveTarget=nil; casesPresented=false; similarPresented=false; archived=[]; archivedLoaded=false; readStatus() }
        .onAppear { readStatus() }
        .onDisappear { cancel(); clearStatus() }
        .task(id: activeManualRun) {
            guard let run = activeManualRun else { return }
            while !Task.isCancelled && activeManualRun == run {
                do { try await Task.sleep(for: .seconds(1)) } catch { return }
                if !Task.isCancelled { readStatus() }
            }
        }
    }
    private func readArchived() {
        let scope=store.readScope
        Task { @MainActor in
            do {let channels=try await store.archivedChannels();guard scope == store.readScope else{return};archived=channels;archivedLoaded=true}
            catch {if scope == store.readScope {self.error=error.localizedDescription}}
        }
    }
    private func cancel() { request = UUID(); task?.cancel(); task = nil }
    private func reset() { cancel(); rows = []; cursor = nil; loaded = false; error = nil }
    private func load() {
        readStatus()
        let token = UUID(); request = token; error = nil
        let current = scan, after = cursor
        task = Task { @MainActor in
            defer { if request == token { task = nil } }
            do {
                let page = try await store.patrolPage(scan: current, after: after)
                guard request == token, current == scan else { return }
                rows += page[field].array; cursor = page["nextAfter"].string; loaded = true
            } catch {
                guard request == token else { return }
                self.error = "读取未完成：" + error.localizedDescription
            }
        }
    }
    @ViewBuilder private var backgroundSection: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button(statusTask == nil ? "读取后台巡检状态" : "正在读取后台状态…") { readStatus() }.disabled(statusTask != nil)
            if let statusError { Text(statusError).foregroundStyle(.orange) }
            if let status = backgroundStatus {
                if let failure = status["error"].string { Text("后台错误：" + failure).foregroundStyle(.orange) }
                if status["run"] == .null { Text("后台尚无巡检记录。") }
                else {
                    let run = status["run"]
                    Text("开始观察：" + time(run["startedAtMs"]))
                    Text("已扫描：" + counts(run["scannedCounts"]))
                    if run["completedAtMs"] == .null {
                        Text("后台巡检进行中。本轮结束后生成简报。")
                    } else {
                        Text("结束观察：" + time(run["completedAtMs"]))
                        if let rawPhase = status["captainBrief"]["phase"].string,
                           let phase = CrewdCaptainBriefPhase(rawValue: rawPhase) {
                            Text("机长简报投递 · " + phase.label)
                        }
                        if let brief = try? CrewdPatrolBrief(run: run) {
                            briefSection(brief)
                        } else {
                            Text("本轮简报无法读取。可使用下方的实时检查。").foregroundStyle(.orange)
                        }
                    }
                }
            }
        }.font(.caption)
    }
    private func briefSection(_ brief: CrewdPatrolBrief) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("本轮行政简报").font(.subheadline.weight(.semibold))
            Text("这是本轮保存的观察。同一事项可能出现在多类，处理前请检查最新状态。")
                .foregroundStyle(.secondary)
            if brief.hasPartialReferences { Text("部分发现只保存了编号样本，计数是本轮观察总数。") }
            if brief.hasIncompleteChannels { Text("仍有群因关联过多未完整检查。") }
            if brief.hasUnrecognizedFindings { Text("本轮还有当前版本未识别的发现类别。") }
            if brief.groups.isEmpty && !brief.hasUnrecognizedFindings {
                Text("本轮在已扫描范围内未发现这些问题。")
            }
            ForEach(brief.groups, id: \.kind) { group in
                DisclosureGroup("\(group.kind.title) · \(group.count)\(group.kind.unit)") {
                    if group.references.count < group.count {
                        Text("保留 \(group.references.count) 个编号")
                    }
                    ScrollView {
                        if group.kind == .idleArchiveCandidates {
                            VStack(alignment: .leading, spacing: 4) {
                                ForEach(group.references, id: \.self) { channel in
                                    Button("核对归档条件") { archiveTarget = channel }
                                    Text(verbatim: channel).textSelection(.enabled)
                                }
                            }.frame(maxWidth: .infinity, alignment: .leading)
                        } else {
                            Text(verbatim: group.references.joined(separator: "、"))
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }.frame(maxHeight: group.kind == .idleArchiveCandidates ? 120 : 74)
                }
            }
        }
    }
    private var activeManualRun: String? {
        guard let status = backgroundStatus, status["manualRunActive"] == .bool(true) else { return nil }
        let run = status["manualRun"]
        return run["runId"].string
    }
    private func manualSection(_ workspace: CrewdWorkspace) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            CrewdManualPatrolControls(workspace: workspace, crew: store.selectedCrew,
                running: activeManualRun != nil, refresh: readStatus).id(store.readScope)
            if let failure = backgroundStatus?["manualError"].string {
                Text(failure).foregroundStyle(.orange)
            }
            if let run = backgroundStatus?["manualRun"], run != .null {
                Text("手动巡检开始于 " + time(run["startedAtMs"]))
                Text("已扫描 " + counts(run["scannedCounts"]))
                if activeManualRun != nil {
                    Text("手动巡检进行中，可以关闭此页后回来查看。")
                } else if run["completedAtMs"] != .null {
                    Text("手动巡检结束于 " + time(run["completedAtMs"]))
                    if let brief = try? CrewdPatrolBrief(run: run) { briefSection(brief) }
                    else { Text("本轮手动简报无法读取。").foregroundStyle(.orange) }
                }
            }
        }.font(.caption)
    }
    private func time(_ value: CrewdValue) -> String {
        guard let ms = value.int else { return "未知" }
        return Date(timeIntervalSince1970: Double(ms) / 1000).formatted(date: .abbreviated, time: .standard)
    }
    private func counts(_ value: CrewdValue) -> String {
        guard case .object(let entries) = value else { return "未知" }
        let names = ["work":"工作", "entries":"需求", "channels":"群", "unowned":"无人负责", "lost":"失联", "stop_escalated":"停止升级", "long_waits":"长期等待", "unclaimed":"待认领需求", "unclaimed_entries":"待认领需求", "needs_ruling":"待裁定需求", "archivable_channels":"归档候选", "archive_candidates":"归档候选", "incomplete_channels":"尚未完整检查的群"]
        return entries.isEmpty ? "0" : entries.keys.sorted().map { (names[$0] ?? $0) + " " + String(entries[$0]?.int ?? 0) }.joined(separator:"；")
    }
    private func clearStatus() { statusRequest = UUID(); statusTask?.cancel(); statusTask = nil; backgroundStatus = nil; statusError = nil }
    private func readStatus() {
        guard statusTask == nil else { return }
        let token = UUID(); statusRequest = token; statusError = nil
        statusTask = Task { @MainActor in
            defer { if statusRequest == token { statusTask = nil } }
            do {
                let value = try await store.patrolStatus()
                guard statusRequest == token else { return }
                backgroundStatus = value
            } catch {
                guard statusRequest == token else { return }
                statusError = "后台状态读取失败：" + error.localizedDescription
            }
        }
    }
    private func summary(_ row: CrewdValue) -> String {
        if scan == "entries" {
            return row["unclaimed"] == .bool(true) ? "尚未认领" : row["needsRuling"] == .bool(true) ? "需要机长或人裁定" : "未发现待认领或待裁定情况"
        }
        if scan == "channels" {
            if row["archiveCandidate"] == .bool(true) { return "可能归档；仍需核实消息活动、权限和最新状态。" }
            let labels = ["no_linked_work":"没有关联工作", "live_work":"仍有未完成工作", "non_work_links":"还有其他类型关联", "source_removed":"来源机组已移除", "links_truncated":"关联过多，尚未完整检查"]
            return row["blockedBy"].array.compactMap { $0.string }.map { labels[$0] ?? "存在尚未识别的阻止原因" }.joined(separator: "；")
        }
        var facts: [String] = []
        if row["unowned"] == .bool(true) { facts.append("无人负责") }
        if row["lost"] != .null { facts.append("负责人失联，需要核实协调") }
        if row["stopEscalated"] == .bool(true) { facts.append("停止检查已升级") }
        if !row["longWaits"].array.isEmpty { facts.append("有人类决定等待较久") }
        if row["longWaitsTruncated"] == .bool(true) { facts.append("等待项仅显示部分") }
        return facts.isEmpty ? "本次未发现上述运转问题" : facts.joined(separator: "；")
    }
}

private struct CrewdManualPatrolControls: View {
    @ObservedObject var workspace: CrewdWorkspace
    let crew: String
    let running: Bool
    let refresh: () -> Void
    @State private var busy = false
    @State private var feedback: String?

    private var pending: [CrewdPresentationJournal.Prepared] {
        workspace.durableUnknown.filter {
            workspace.isCurrentOriginalObservation($0) && $0.method == "command.submit" &&
                $0.params["command"]["operation"] == .string(CrewdPublicOperation.patrolStart.rawValue) &&
                $0.params["command"]["params"]["crewId"] == .string(crew)
        }
    }

    var body: some View {
        Button(running ? "手动巡检进行中" : "启动手动巡检") { start() }
            .disabled(busy || running || !pending.isEmpty || !workspace.client.readyForWork)
        ForEach(pending) { original in
            Button("查询原巡检请求") { lookup(original) }.disabled(busy || !workspace.client.readyForWork)
        }
        if let feedback { Text(feedback).foregroundStyle(.secondary) }
    }

    private func start() {
        busy = true
        feedback = nil
        Task {
            defer { busy = false }
            do {
                let receipt = try await workspace.submitPublicCommand(.patrolStart,
                    params: .object(["crewId": .string(crew)]), crew: crew)
                observe(receipt)
            } catch { feedback = error.localizedDescription }
        }
    }

    private func lookup(_ original: CrewdPresentationJournal.Prepared) {
        busy = true
        feedback = nil
        Task {
            defer { busy = false }
            do {
                let receipt = try await workspace.lookupOriginalCommand(original)
                if let receipt, [.committed, .rejected, .suppressed].contains(receipt.phase) {
                    try workspace.acknowledgeOriginalCommand(original, receipt: receipt)
                }
                observe(receipt)
            } catch { feedback = error.localizedDescription }
        }
    }

    private func observe(_ receipt: CrewdPublicReceipt?) {
        guard workspace.crewID == crew else { return }
        if let receipt, receipt.historical, [.committed, .rejected, .suppressed].contains(receipt.phase) {
            feedback = "原历史回执已核对。这里显示当前可读的巡检轮次。"
        } else if let receipt, receipt.phase == .committed {
            feedback = "原请求已记录。这里显示最近一轮手动巡检。"
        } else if let receipt, [.rejected, .suppressed].contains(receipt.phase) {
            feedback = "后台未接受此巡检请求。"
        } else {
            feedback = "原请求结果未确认，可以继续查询原回执。"
        }
        refresh()
    }
}

private struct CrewdSimilarRequirementsView: View {
    enum Page: String, CaseIterable { case suggestions = "待审阅", reviews = "已审阅" }
    @ObservedObject var store: CrewdWorkStore
    let workspace: CrewdWorkspace?
    @Environment(\.dismiss) private var dismiss
    @State private var page: Page = .suggestions
    @State private var rows: [CrewdValue] = []
    @State private var next: String?
    @State private var enabled: Bool?
    @State private var loaded = false
    @State private var selected: CrewdValue?
    @State private var originals: [(String, String)] = []
    @State private var failure: String?
    @State private var actionFeedback: String?
    @State private var busy = false
    @State private var task: Task<Void, Never>?
    @State private var request = UUID()

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("相近原始需求").font(.headline)
                Spacer()
                Button("关闭") { dismiss() }
            }
            Text("后台对同一公开群内的近期候选作有限比较。建议供人工审阅，原需求和计划仍各自保留。")
                .font(.caption).foregroundStyle(.secondary)
            if store.capabilities.contains("adminCase.requirementReviews") {
                Picker("审阅范围", selection: $page) {
                    ForEach(Page.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }.pickerStyle(.segmented)
            }
            HStack {
                Button("重新读取") { load() }
                if next != nil { Button("下一页") { load(after: next) } }
                if busy { ProgressView().controlSize(.small) }
            }.disabled(busy)
            if let failure { Text(failure).foregroundStyle(.orange) }
            if let actionFeedback { Text(actionFeedback).font(.caption).foregroundStyle(.secondary) }
            if page == .suggestions, enabled == false {
                Text("当前后台未启用相近需求判断。")
            } else if loaded, rows.isEmpty {
                Text(page == .suggestions ? "目前没有可审阅的建议。" : "还没有人工审阅记录。")
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                        let pair = pair(in: row)
                        VStack(alignment: .leading, spacing: 4) {
                            Button("\(pair["firstId"].string ?? "") 与 \(pair["secondId"].string ?? "")") {
                                inspect(row)
                            }.disabled(busy)
                            if page == .reviews {
                                let decision = pair["disposition"].string ?? ""
                                Text(decisionLabel(decision, effective: row["effective"] == .bool(true)))
                                    .font(.caption).foregroundStyle(.secondary)
                                if let note = pair["note"].string {
                                    Text(verbatim: note).font(.caption).textSelection(.enabled)
                                }
                            }
                        }
                    }
                    if selected != nil { Divider() }
                    ForEach(Array(originals.enumerated()), id: \.offset) { _, original in
                        Text(original.0).font(.subheadline.weight(.semibold))
                        Text(verbatim: original.1).textSelection(.enabled)
                    }
                    if let workspace, store.capabilities.contains("adminCase.requirementReview") {
                        CrewdRequirementReviewControls(
                            workspace: workspace, crew: store.selectedCrew, scope: store.readScope,
                            isCurrent: { store.readScope == $0 }, source: selected.map { pair(in: $0) },
                            saved: page == .reviews, originalsRead: originals.count == 2 && !busy,
                            refresh: { message in actionFeedback = message; load() }
                        ).id(selectionKey)
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
        }.padding().modifier(CrewdAdaptiveFormFrame(width: 570, height: 520, scrolls: false))
        .task { load() }
        .onChange(of: page) { _, _ in load() }
        .onChange(of: store.readScope) { _, _ in dismiss() }
        .onDisappear { request = UUID(); task?.cancel() }
    }

    private func pair(in row: CrewdValue) -> CrewdValue { page == .reviews ? row["review"] : row }

    private var selectionKey: String {
        guard let selected else { return page.rawValue }
        let pair = pair(in: selected)
        return [page.rawValue, pair["firstId"].string, pair["secondId"].string,
                pair["revision"].string ?? pair["candidateRevision"].string].compactMap { $0 }.joined(separator: "/")
    }

    private func decisionLabel(_ decision: String, effective: Bool) -> String {
        switch decision {
        case "group": return effective ? "已归组" : "原分组已失效"
        case "dismiss": return effective ? "已排除" : "原排除已失效"
        default: return "已撤销审阅"
        }
    }

    private func read(_ operation: @escaping @MainActor (UUID, UUID) async throws -> Void) {
        task?.cancel()
        let token = UUID(), scope = store.readScope
        request = token
        busy = true
        failure = nil
        originals = []
        task = Task { @MainActor in
            defer { if request == token { busy = false; task = nil } }
            do { try await operation(token, scope) }
            catch {
                if request == token, store.readScope == scope, !(error is CancellationError) {
                    failure = error.localizedDescription
                }
            }
        }
    }

    private func load(after: String? = nil) {
        let requested = page
        selected = nil
        if after == nil { rows = []; next = nil; enabled = nil; loaded = false }
        read { token, scope in
            let result: CrewdValue
            if requested == .suggestions { result = try await store.similarRequirementPage(after: after) }
            else { result = try await store.requirementReviewPage(after: after) }
            guard request == token, store.readScope == scope, page == requested else { return }
            rows = result["items"].array
            next = result["nextAfter"].string
            enabled = requested == .suggestions ? result["enabled"] == .bool(true) : nil
            loaded = true
        }
    }

    private func inspect(_ row: CrewdValue) {
        selected = row
        let source = pair(in: row)
        let crew = store.selectedCrew
        read { token, scope in
            var pair: [(String, String)] = []
            for key in ["firstId", "secondId"] {
                guard let raw = source[key].string, let id = CrewdWorkID(raw, kind: .requirement) else { return }
                let value = try await store.requirementDetail(id, expectedCrew: crew)
                guard request == token, store.readScope == scope else { return }
                guard value["crewId"] == .string(crew), let text = value["text"].string else {
                    throw CrewdClientError(message: "原始需求已不可核对。请重新读取建议。")
                }
                pair.append((raw, text))
            }
            originals = pair
        }
    }
}

private struct CrewdRequirementReviewControls: View {
    @ObservedObject var workspace: CrewdWorkspace
    let crew: String
    let scope: UUID
    let isCurrent: (UUID) -> Bool
    let source: CrewdValue?
    let saved: Bool
    let originalsRead: Bool
    let refresh: (String) -> Void
    @State private var note = ""
    @State private var busy = false
    @State private var feedback: String?

    private var pending: [CrewdPresentationJournal.Prepared] {
        workspace.durableUnknown.filter { row in
            workspace.isCurrentOriginalObservation(row) && row.method == "command.submit" &&
                row.params["command"]["operation"] == .string(CrewdPublicOperation.requirementReview.rawValue) &&
                row.params["command"]["params"]["crewId"] == .string(crew)
        }
    }

    private var writing: Bool {
        workspace.publicCommands.observations.values.contains { row in
            workspace.publicCommands.pending.contains(row.id) && row.envelope.operation == .requirementReview &&
                row.envelope.params["crewId"] == .string(crew)
        }
    }

    private var ready: Bool {
        !busy && !writing && isCurrent(scope) && workspace.crewID == crew && workspace.client.readyForWork
    }

    var body: some View {
        if let feedback { Text(feedback).font(.caption).foregroundStyle(.secondary) }
        ForEach(pending) { row in
            let original = row.params["command"]["params"]
            Button("核对原审阅请求  " + (original["firstId"].string ?? "") + " / " + (original["secondId"].string ?? "")) {
                lookup(row)
            }.disabled(!ready)
        }
        if let source, saved ? source["disposition"] != .string("clear") : originalsRead {
            TextField("填写审阅理由", text: $note, axis: .vertical).lineLimit(2...4)
                .disabled(!ready || !pending.isEmpty)
            Text("两条原需求、关联计划与处理状态各自保留。")
                .font(.caption).foregroundStyle(.secondary)
            HStack {
                if saved {
                    Button("撤销原审阅") { submit("clear", source: source) }
                } else {
                    Button("归为一组") { submit("group", source: source) }
                    Button("排除建议") { submit("dismiss", source: source) }
                }
            }.disabled(!ready || !pending.isEmpty || note.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || note.utf8.count > 4_096)
        }
    }

    private func submit(_ disposition: String, source: CrewdValue) {
        guard ready, pending.isEmpty else { return }
        var params: [String: CrewdValue] = [
            "crewId": .string(crew), "firstId": source["firstId"], "secondId": source["secondId"],
            "expectedReviewRevision": source[saved ? "revision" : "reviewRevision"],
            "disposition": .string(disposition), "note": .string(note)
        ]
        if !saved { params["candidateRevision"] = source["candidateRevision"] }
        busy = true
        feedback = nil
        Task { @MainActor in
            defer { busy = false }
            guard isCurrent(scope), workspace.crewID == crew, pending.isEmpty, !writing else { return }
            do {
                let receipt = try await workspace.submitPublicCommand(.requirementReview, params: .object(params), crew: crew) { result in
                    if result.phase == .committed, result.value["result"]["channelId"] != source["channelId"] {
                        throw CrewdPublicWire.invalid()
                    }
                }
                observe(receipt)
            } catch {
                if isCurrent(scope) { feedback = error.localizedDescription }
            }
        }
    }

    private func lookup(_ original: CrewdPresentationJournal.Prepared) {
        guard ready else { return }
        busy = true
        feedback = nil
        Task { @MainActor in
            defer { busy = false }
            do {
                let receipt = try await workspace.lookupOriginalCommand(original)
                guard isCurrent(scope) else { return }
                if let receipt, [.committed, .rejected, .suppressed].contains(receipt.phase) {
                    try workspace.acknowledgeOriginalCommand(original, receipt: receipt)
                }
                observe(receipt)
            } catch {
                if isCurrent(scope) { feedback = error.localizedDescription }
            }
        }
    }

    private func observe(_ receipt: CrewdPublicReceipt?) {
        guard isCurrent(scope), workspace.crewID == crew else { return }
        switch receipt?.phase {
        case .committed:
            refresh(receipt?.historical == true ? "原历史回执已核对，当前审阅记录已另行读取。" : "审阅已记录，可在「已审阅」查看。")
        case .rejected, .suppressed:
            refresh("后台未接受此审阅，已重新读取当前建议。")
        default:
            feedback = "结果待确认。可用原编号查询回执。"
        }
    }
}
