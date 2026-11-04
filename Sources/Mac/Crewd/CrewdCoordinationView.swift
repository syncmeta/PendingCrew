import SwiftUI

struct CrewdCoordinationView: View {
    @ObservedObject var store: CrewdWorkStore
    let crew: String
    let id: String
    let revision: String
    var members: [CrewdValue] = []
    @State private var status: CrewdCoordinationStatus?
    @State private var failure: String?
    @State private var task: Task<Void,Never>?
    @State private var token = UUID()
    @State private var decisionInput = ""
    @State private var historicalDecision: String?
    @State private var historicalOutcome: CrewdCoordinationStatus.Outcome?
    @State private var historyFailure: String?
    @State private var historyTask: Task<Void,Never>?
    @State private var historyToken = UUID()
    @State private var childTarget: ChildTarget?
    private struct ChildTarget: Identifiable {
        let child: CrewdCoordinationStatus.Split.Child
        let crew: String
        let parent: String
        let scope: UUID
        var id: CrewdWorkID { child.id }
    }
    private var identity: String { crew + "/" + id }
    private var queryDecision: String? {
        UUID(uuidString:decisionInput.trimmingCharacters(in:.whitespacesAndNewlines))?.uuidString.lowercased()
    }
    var body: some View {
        VStack(alignment:.leading,spacing:6) {
            Text("进展判断与协调").font(.headline)
            HStack {
                Button(task == nil ? "读取协调记录" : "正在读取…") { read() }.disabled(task != nil)
                if task != nil { Button("取消") { invalidate() } }
            }
            if let failure { Text(failure).foregroundStyle(.orange) }
            if let status {
                let record = status.record
                if record == .null { Text("该计划暂无协调判断记录。") }
                else {
                    Text("判断").fontWeight(.semibold)
                    Text("判断状态：" + phase(record["phase"].string ?? ""))
                    Text("判断说明：" + reason(record["reason"].string ?? ""))
                    Text(record["adopted"] == .bool(true) ? "已采纳协调请求。" : "本条记录未采纳协调请求。")
                    if let decision = record["decisionId"].string {
                        Text("判断编号 " + decision).foregroundStyle(.secondary).textSelection(.enabled)
                    }
                    Text("通知").fontWeight(.semibold)
                    if let recipient=record["recipient"].string {
                        Text("通知对象 " + member(recipient))
                    }
                    Text("通知状态 " + wake(record["wakeState"].string))
                }
                if let receipt = status.lastAssignment {
                    Text("最近一次实际分派").fontWeight(.semibold)
                    assignment(receipt)
                }
                if let receipt = status.lastSplit {
                    Text("最近一次实际拆分").fontWeight(.semibold)
                    split(receipt)
                }
                if status.lastAssignment == nil && status.lastSplit == nil { unavailableOutcome }
            }
            DisclosureGroup("按判断编号核对原处理结果") {
                TextField("判断编号",text:$decisionInput)
                    .textFieldStyle(.roundedBorder)
                    .disabled(historyTask != nil)
                HStack {
                    Button(historyTask == nil ? "查询原回执" : "正在查询…") { readHistory() }
                        .disabled(queryDecision == nil || historyTask != nil)
                    if historyTask != nil { Button("取消") { clearHistory() } }
                }
                if let historyFailure { Text(historyFailure).foregroundStyle(.orange) }
                if let historicalDecision {
                    Text("原判断 " + historicalDecision).textSelection(.enabled)
                    switch historicalOutcome {
                    case .assignment(let receipt): assignment(receipt)
                    case .split(let receipt): split(receipt)
                    case nil: unavailableOutcome
                    }
                }
            }
            .onChange(of:decisionInput) {_,_ in clearHistory()}
        }.font(.caption)
        .onChange(of:identity) {_,_ in reset()}
        .onChange(of:revision) {_,_ in
            let inspected = status != nil || task != nil
            invalidate()
            if inspected { read() }
        }
        .onChange(of:store.readScope) {_,_ in reset()}
        .onDisappear {reset()}
        .sheet(item:$childTarget) { target in
            childDetails(target)
        }
    }
    private var unavailableOutcome: some View {
        Text("当前后台未返回可核对的处理回执。").foregroundStyle(.secondary)
    }
    @ViewBuilder private func assignment(_ receipt: CrewdCoordinationStatus.Assignment) -> some View {
        Text("机长 " + member(receipt.captain,includeID:true) + " 已分派").textSelection(.enabled)
        Text(member(receipt.previousOwner,includeID:true) + " → " + member(receipt.owner,includeID:true)).textSelection(.enabled)
        Text("计划版本 " + receipt.previousRevision + " → " + receipt.revision + " · 事件 " + receipt.eventSequence)
            .textSelection(.enabled)
        Text("分派时间 " + timestamp(receipt.assignedAtMs))
        Text("对应判断编号 " + receipt.decisionID).textSelection(.enabled)
        Text("回执保留当时的分派事实。当前负责人见计划详情。").foregroundStyle(.secondary)
    }
    @ViewBuilder private func split(_ receipt: CrewdCoordinationStatus.Split) -> some View {
        Text("机长 " + member(receipt.captain,includeID:true) + " 创建了 \(receipt.children.count) 项子工作")
            .textSelection(.enabled)
        Text("上级计划 " + id + " · 当时版本 " + receipt.parentRevision).textSelection(.enabled)
        Text("拆分时间 " + timestamp(receipt.splitAtMs))
        Text("对应判断编号 " + receipt.decisionID).textSelection(.enabled)
        ForEach(receipt.children) { child in
            VStack(alignment:.leading,spacing:3) {
                Button("查看 " + child.id.raw) {
                    childTarget = .init(child:child,crew:crew,parent:id,scope:store.readScope)
                }
                Text("创建版本 1 · 事件 " + child.eventSequence).textSelection(.enabled)
                if let owner = child.owner { Text("创建时负责人 " + member(owner,includeID:true)).textSelection(.enabled) }
                else { Text("创建时待分派").foregroundStyle(.secondary) }
            }
        }
    }
    private func timestamp(_ milliseconds: Int64) -> String {
        Date(timeIntervalSince1970:Double(milliseconds) / 1_000).formatted(date:.abbreviated,time:.shortened)
    }
    private func childDetails(_ target: ChildTarget) -> some View {
        CrewdCoordinationChildDetail(store:store,id:target.id,crew:target.crew,parent:target.parent,scope:target.scope)
    }
    private func member(_ id: String, includeID: Bool = false) -> String {
        guard let name = members.first(where:{$0["id"] == .string(id)})?["displayName"].string, !name.isEmpty else { return id }
        return includeID && name != id ? name + " (" + id + ")" : name
    }
    private func reset() { invalidate(); clearHistory(); decisionInput = ""; childTarget = nil }
    private func clearHistory() {
        historyToken = UUID(); historyTask?.cancel(); historyTask = nil
        historicalDecision = nil; historicalOutcome = nil; historyFailure = nil
    }
    private func readHistory() {
        guard let decision = queryDecision, historyTask == nil else { return }
        clearHistory()
        let request = historyToken
        historyTask = Task { @MainActor in
            defer { if historyToken == request { historyTask = nil } }
            do {
                let value = try await store.coordinationStatus(crew:crew,id:id,decisionID:decision)
                guard historyToken == request else { return }
                historicalDecision = decision; historicalOutcome = value.requestedOutcome
            } catch {
                guard historyToken == request else { return }
                if let failure = error as? CrewdClientError, failure.code == -32602 {
                    historyFailure = "当前后台未接受按判断编号查询。"
                } else { historyFailure = "查询失败：" + error.localizedDescription }
            }
        }
    }
    private func invalidate() {token=UUID();task?.cancel();task=nil;status=nil;failure=nil}
    private func read() {
        let request=UUID();token=request;status=nil;failure=nil
        task=Task { @MainActor in
            defer {if token == request {task=nil}}
            do {
                let value=try await store.coordinationStatus(crew:crew,id:id)
                guard token == request else{return};status=value
            }catch {guard token == request else{return};failure="读取失败："+error.localizedDescription}
        }
    }
    private func phase(_ value:String)->String {
        ["prepared":"已准备", "dispatched":"判断请求已发出", "settled":"本次判断已结束", "stale":"依据已变化，记录已失效", "unknown":"结果未知", "suppressed":"本次处理已抑制"][value] ?? "未知状态"
    }
    private func reason(_ value:String)->String {
        ["insufficient_reports":"回报不足", "unconfigured":"尚未配置判断模型", "prepared":"已准备判断", "answered":"模型已答复", "low_confidence":"置信度不足", "provider_unavailable":"模型服务不可用", "restart_unknown":"重启前结果未知", "state_changed":"工作状态已改变", "stopped":"已停止"][value] ?? "未知原因"
    }
    private func wake(_ value:String?)->String {
        guard let value else{return "未记录通知投递"}
        return ["pending":"排队中", "inflight":"投递中", "delivered":"已送达", "dropped":"投递未完成"][value] ?? "未知投递状态"
    }
}

private struct CrewdCoordinationChildDetail: View {
    @ObservedObject var store: CrewdWorkStore
    let id: CrewdWorkID
    let crew: String
    let parent: String
    let scope: UUID
    @Environment(\.dismiss) private var dismiss
    @State private var plan: CrewdPlan?
    @State private var failure: String?

    var body: some View {
        VStack(alignment:.leading,spacing:12) {
            HStack {
                Text(id.raw + " · 子工作详情").font(.headline)
                Spacer()
                Button("关闭") { dismiss() }
            }
            if let failure { Text(failure).foregroundStyle(.orange) }
            if let plan, store.readScope == scope {
                ScrollView {
                    VStack(alignment:.leading,spacing:8) {
                        Text(verbatim:plan.value["title"].string ?? "").font(.headline).textSelection(.enabled)
                        Text("上级计划 " + parent + " · 当前版本 " + plan.revision).textSelection(.enabled)
                        Text(CrewdWorkStatusLabel.display(plan.status)).foregroundStyle(.secondary)
                        if let owner = plan.value["owner"].string { Text("当前负责人 " + owner).textSelection(.enabled) }
                        if let goal = plan.value["goal"].string, !goal.isEmpty {
                            Text("目标").font(.subheadline)
                            Text(verbatim:goal).textSelection(.enabled)
                        }
                        if let acceptance = plan.value["acceptance"].string, !acceptance.isEmpty {
                            Text("验收条件").font(.subheadline)
                            Text(verbatim:acceptance).textSelection(.enabled)
                        }
                    }.frame(maxWidth:.infinity,alignment:.leading)
                }
            } else if failure == nil { ProgressView("正在读取子工作…") }
        }
        .padding().modifier(CrewdAdaptiveFormFrame(width:520,height:420,scrolls:false))
        .task {
            guard store.readScope == scope else { return }
            do {
                let value = try await store.coordinationChildDetail(id,crew:crew,parent:parent)
                guard store.readScope == scope, !Task.isCancelled else { return }
                plan = value
            } catch is CancellationError {
                return
            } catch {
                if store.readScope == scope && !Task.isCancelled { failure = error.localizedDescription }
            }
        }
        .onChange(of:store.readScope) {_,_ in dismiss()}
    }
}
