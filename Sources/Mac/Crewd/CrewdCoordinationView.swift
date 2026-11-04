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
    @State private var historicalAssignment: CrewdCoordinationStatus.Assignment?
    @State private var historyFailure: String?
    @State private var historyTask: Task<Void,Never>?
    @State private var historyToken = UUID()
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
                Text("最近一次实际分派").fontWeight(.semibold)
                assignment(status.lastAssignment)
            }
            DisclosureGroup("按判断编号核对原分派") {
                TextField("判断编号",text:$decisionInput)
                    .textFieldStyle(.roundedBorder)
                    .disabled(historyTask != nil)
                HStack {
                    Button(historyTask == nil ? "查询原分派" : "正在查询…") { readHistory() }
                        .disabled(queryDecision == nil || historyTask != nil)
                    if historyTask != nil { Button("取消") { clearHistory() } }
                }
                if let historyFailure { Text(historyFailure).foregroundStyle(.orange) }
                if let historicalDecision {
                    Text("原判断 " + historicalDecision).textSelection(.enabled)
                    assignment(historicalAssignment)
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
    }
    @ViewBuilder private func assignment(_ receipt: CrewdCoordinationStatus.Assignment?) -> some View {
        if let receipt {
            Text("机长 " + member(receipt.captain,includeID:true) + " 已分派").textSelection(.enabled)
            Text(member(receipt.previousOwner,includeID:true) + " → " + member(receipt.owner,includeID:true)).textSelection(.enabled)
            Text("计划版本 " + receipt.previousRevision + " → " + receipt.revision + " · 事件 " + receipt.eventSequence)
                .textSelection(.enabled)
            Text("分派时间 " + Date(timeIntervalSince1970:Double(receipt.assignedAtMs) / 1_000).formatted(date:.abbreviated,time:.shortened))
            Text("对应判断编号 " + receipt.decisionID).textSelection(.enabled)
            Text("回执保留当时的分派事实。当前负责人见计划详情。").foregroundStyle(.secondary)
        } else {
            Text("当前后台未返回可核对的分派回执。").foregroundStyle(.secondary)
        }
    }
    private func member(_ id: String, includeID: Bool = false) -> String {
        guard let name = members.first(where:{$0["id"] == .string(id)})?["displayName"].string, !name.isEmpty else { return id }
        return includeID && name != id ? name + " (" + id + ")" : name
    }
    private func reset() { invalidate(); clearHistory(); decisionInput = "" }
    private func clearHistory() {
        historyToken = UUID(); historyTask?.cancel(); historyTask = nil
        historicalDecision = nil; historicalAssignment = nil; historyFailure = nil
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
                historicalDecision = decision; historicalAssignment = value.requestedAssignment
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
