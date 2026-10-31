import SwiftUI

struct CrewdReassignmentView: View {
    @ObservedObject var store: CrewdWorkStore
    let crew: String
    let id: String
    let revision: String
    var members: [CrewdValue] = []
    @State private var record: CrewdValue?
    @State private var failure: String?
    @State private var task: Task<Void,Never>?
    @State private var token = UUID()
    private var identity: String { crew + "/" + id + "/" + revision }
    var body: some View {
        VStack(alignment:.leading,spacing:6) {
            Text("失联重派").font(.headline)
            HStack {
                Button(task == nil ? "读取重派记录" : "正在读取…") { read() }.disabled(task != nil)
                if task != nil { Button("取消") { invalidate() } }
            }
            if let failure { Text(failure).foregroundStyle(.orange) }
            if let record {
                if record == .null { Text("该计划暂无失联重派记录。") }
                else {
                    Text("判断状态：" + phase(record["phase"].string ?? ""))
                    Text("判断说明：" + reason(record["reason"].string ?? ""))
                    Text(record["adopted"] == .bool(true) ? "原计划负责人已转移；尚不代表工作完成。" : "本条记录未执行负责人转移。")
                    if let recipient=record["newOwner"].string {
                        Text("接替负责人：" + (members.first(where:{$0["id"] == .string(recipient)})?["displayName"].string ?? recipient))
                    }
                    Text("接替者通知：" + wake(record["ownerWakeState"].string))
                    Text("机长通知：" + wake(record["captainWakeState"].string))
                    Text("原负责人：" + (record["oldOwner"].string ?? "未知"))
                    Text("实际进展、完成情况与成果以本计划的状态、回报和证据为准。重派依据和原工作证据应一起核对。").foregroundStyle(.secondary)
                }
            }
        }.font(.caption)
        .onChange(of:identity) {_,_ in invalidate()}
        .onChange(of:store.readScope) {_,_ in invalidate()}
        .onDisappear {invalidate()}
    }
    private func invalidate() {token=UUID();task?.cancel();task=nil;record=nil;failure=nil}
    private func read() {
        let request=UUID();token=request;record=nil;failure=nil
        task=Task { @MainActor in
            defer {if token == request {task=nil}}
            do {
                let value=try await store.reassignmentStatus(crew:crew,id:id)
                guard token == request else{return};record=value
            }catch {guard token == request else{return};failure="读取失败："+error.localizedDescription}
        }
    }
    private func phase(_ value:String)->String {
        ["prepared":"已准备", "dispatched":"判断请求已发出", "settled":"本次判断已结束", "stale":"依据已变化，记录已失效", "unknown":"结果未知", "suppressed":"本次处理已抑制"][value] ?? "未知状态"
    }
    private func reason(_ value:String)->String {
        ["no_candidates":"当前没有合格接替者", "unconfigured":"尚未配置判断模型", "prepared":"已准备判断", "answered":"模型已答复", "low_confidence":"置信度不足", "provider_unavailable":"模型服务不可用", "restart_unknown":"重启前结果未知", "state_changed":"工作状态已改变", "stopped":"已停止"][value] ?? "未知原因"
    }
    private func wake(_ value:String?)->String {
        guard let value else{return "未记录通知投递"}
        return ["pending":"排队中", "inflight":"投递中", "delivered":"已投递，不代表工作已完成", "dropped":"未投递或已丢弃"][value] ?? "未知投递状态"
    }
}
