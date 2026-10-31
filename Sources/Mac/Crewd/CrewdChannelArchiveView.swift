import SwiftUI
struct CrewdChannelArchiveView:View {
    @ObservedObject var store:CrewdWorkStore
    let channel:String
    let crew:String
    @Environment(\.dismiss) private var dismiss
    @State private var value:CrewdValue?
    @State private var note=""
    @State private var evidence=""
    @State private var busy=false
    @State private var failure:String?
    @State private var token=UUID()
    var body:some View {
        VStack(alignment:.leading,spacing:12) {
            Text("群归档事实核查").font(.headline)
            ScrollView {
                VStack(alignment:.leading,spacing:8) {
                    if let value {
                        Text(value["channel"]["name"].string ?? channel).font(.headline)
                        Text(value["channel"]["archivedAt"] == .null ? "当前未归档":"当前已归档")
                        Text("最近消息：" + (value["facts"]["lastMessageAt"].string ?? "暂无消息记录"))
                        Text(value["facts"]["incomplete"] == .bool(true) ? "事实扫描不完整，不能确认可归档。":"本次事实扫描完整。")
                        ForEach(["linkedWork","liveWork","linkedRequirements","openRequirements","openAsks","pendingWakes","staleWakes","inflightWakes","activeDeliveries","pendingJudgments"],id:\.self) {key in
                            Text(factLabel(key)+"："+String(value["facts"][key].int ?? 0))
                        }
                        ForEach(value["blockers"].array.compactMap(\.string),id:\.self) {reason in Text("归档限制："+blocker(reason)).foregroundStyle(.orange)}
                        if let previous=value["lastAction"]["note"].string {Text("上次操作说明："+previous)}
                        TextField("核查说明（必填）",text:$note,axis:.vertical)
                        if value["channel"]["archivedAt"] == .null {TextField("归档凭据（必填）",text:$evidence,axis:.vertical)}
                        Text("仅在你确认后操作；归档保留群消息、成员和工作关联，解档恢复群使用。").font(.caption).foregroundStyle(.secondary)
                        Button(value["channel"]["archivedAt"] == .null ? "确认归档":"确认解档") {write()}.disabled(submissionDisabled(value))
                    }
                    if let failure {Text(failure).foregroundStyle(.orange)}
                }.frame(maxWidth:.infinity,alignment:.leading)
            }.frame(maxHeight:460)
            HStack {Button("重新核查") {read()}.disabled(busy);Spacer();Button("关闭") {dismiss()}}
        }.padding().frame(width:500)
        .task {read()}
        .onChange(of:store.readScope) {_,_ in token=UUID();value=nil;busy=false;dismiss()}
        .onDisappear {token=UUID()}
    }
    private func submissionDisabled(_ value:CrewdValue)->Bool {
        if busy || note.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty {return true}
        if value["channel"]["archivedAt"] == .null {
            return value["canArchive"] != .bool(true) || evidence.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty
        }
        return value["canRestore"] != .bool(true)
    }
    private func blocker(_ key:String)->String {
        ["no_linked_work":"没有关联工作，不能按工作完成核实归档","not_topic":"仅主题群可使用此操作","crew_archived":"所属机组已归档","not_authorized":"当前身份无管理权限","already_archived":"群已归档，可核实解档","live_work":"仍有未终结工作","open_requirement":"仍有未关闭需求","open_ask":"仍有待答问题","pending_wake":"仍有排队唤醒","inflight_wake":"仍有投递中唤醒","active_delivery":"仍有活动投递","pending_judgment":"仍有未结束判断","pending_owner_transfer":"群主交接尚未结束","incomplete":"事实未完整读取"][key] ?? key
    }
    private func factLabel(_ key:String)->String {["linkedWork":"关联工作","liveWork":"未终结工作","linkedRequirements":"关联需求","openRequirements":"未关闭需求","openAsks":"待答问题","pendingWakes":"排队唤醒","staleWakes":"已过期排队通知（不阻止归档）","inflightWakes":"投递中唤醒","activeDeliveries":"活动投递","pendingJudgments":"未结束判断"][key] ?? key}
    private func read() {
        let request=UUID();token=request;busy=true;failure=nil
        Task { @MainActor in
            defer {if token == request {busy=false}}
            do {let result=try await store.archiveCheck(channel:channel,crew:crew);guard token == request else{return};value=result}
            catch {if token == request {failure=error.localizedDescription}}
        }
    }
    private func write() {
        guard let value else{return}
        let request=UUID();token=request;busy=true;failure=nil
        Task { @MainActor in
            defer {if token == request {busy=false}}
            do {let result=try await store.verifyArchive(channel:channel,crew:crew,revision:value["revision"].string ?? "",restore:value["channel"]["archivedAt"] != .null,evidence:evidence,note:note);guard token == request else{return};self.value=result;note="";evidence=""}
            catch is CancellationError {return}
            catch {guard token == request else{return};failure="操作未获确认："+error.localizedDescription+"；不会自动重发。";if let result=try? await store.archiveCheck(channel:channel,crew:crew),token == request {self.value=result}}
        }
    }
}
