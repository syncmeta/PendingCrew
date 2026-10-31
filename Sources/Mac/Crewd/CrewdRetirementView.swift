import SwiftUI

struct CrewdRetirementView: View {
    @ObservedObject var workspace: CrewdWorkspace
    let sessionID: String
    @State private var record: CrewdValue?
    @State private var failure: String?
    @State private var task: Task<Void,Never>?
    @State private var token = UUID()
    var body: some View {
        VStack(alignment:.leading,spacing:6) {
            Button(task == nil ? "读取自动退场记录" : "正在读取…") { read() }.disabled(task != nil)
            if let failure { Text(failure).foregroundStyle(.orange) }
            if let record {
                if record == .null { Text("当前成员暂无自动退场记录。") }
                else {
                    Text("退场状态：" + phase(record["phase"].string ?? ""))
                    Text("说明：" + reason(record["reason"].string ?? ""))
                    Text(record["adopted"] == .bool(true) ? "此记录曾收到实际退出确认；请结合当前状态核对。" : "此记录尚未确认实际退场。")
                    Text("自动退场保留原会话，可在有新工作时恢复；不是人类停止或删除会话。").foregroundStyle(.secondary)
                }
            }
        }.font(.caption)
        .onChange(of:sessionID) {_,_ in invalidate()}
        .onChange(of:workspace.observationGeneration) {_,_ in invalidate()}
        .onChange(of:workspace.observationSelection) {_,_ in invalidate()}
        .onChange(of:workspace.client.connectionEpoch) {_,_ in invalidate()}
        .onDisappear { invalidate() }
    }
    private func invalidate() { token=UUID(); task?.cancel(); task=nil; record=nil; failure=nil }
    private func read() {
        let request=UUID(), epoch=workspace.observationGeneration, route=workspace.observationSelection, connection=workspace.client.connectionEpoch
        let crew=workspace.crewID, session=sessionID
        token=request; record=nil; failure=nil
        task=Task { @MainActor in
            defer { if token == request {task=nil} }
            do {
                let value=try await workspace.readValue("session.retirementStatus",.object(["crewId":.string(crew),"sessionId":.string(session)]))
                guard token == request, workspace.acceptsObservation(epoch), workspace.observationSelection == route, workspace.client.connectionEpoch == connection, workspace.crewID == crew, sessionID == session else {return}
                record=try CrewdRetirementRecord.decode(value,crew:crew,session:session)
            } catch { guard token == request else {return}; failure="读取失败："+error.localizedDescription }
        }
    }
    private func phase(_ value:String)->String {
        ["prepared":"已准备判断","dispatched":"判断请求已发出","decision_ready":"模型建议已就绪，尚未请求退出","exit_requested":"已请求退出，等待实际确认","parked":"已确认退场","stale":"新活动或状态变化已使记录失效","unknown":"退出结果未知","suppressed":"本次退场已抑制"][value] ?? "未知状态"
    }
    private func reason(_ value:String)->String {
        ["prepared":"等待判断","answered":"模型已答复","low_confidence":"置信度不足","provider_unavailable":"模型服务不可用","unconfigured":"未配置判断模型","restart_unknown":"重启前结果未知","state_changed":"依据已变化","stopped":"已停止","exit_unknown":"未能确认执行器退出"][value] ?? "未知原因"
    }
}
