import SwiftUI
struct CrewdAdminCasesView:View {
    @ObservedObject var store:CrewdWorkStore
    @Environment(\.dismiss) private var dismiss
    @State private var rows:[CrewdValue]=[]
    @State private var next:String?
    @State private var detail:CrewdValue?
    @State private var loaded=false
    @State private var busy=false
    @State private var failure:String?
    @State private var token=UUID()
    var body:some View {
        VStack(alignment:.leading,spacing:12) {
            Text("行政处理记录").font(.headline)
            Text("记录工作跟进升级来源及介入判断；判断被采纳不代表已启动成员或已解决工作。").font(.caption).foregroundStyle(.secondary)
            HStack {
                Button("重新读取") {load(reset:true)}.disabled(busy)
                if next != nil {Button("下一页") {load(reset:false)}.disabled(busy)}
                Spacer();Button("关闭") {dismiss()}
            }
            if let failure {Text(failure).foregroundStyle(.orange)}
            if loaded && rows.isEmpty {Text("当前机组暂无行政处理记录。")}
            ScrollView {
                LazyVStack(alignment:.leading,spacing:12) {
                    ForEach(Array(rows.enumerated()),id:\.offset) {_,row in
                        Button {readDetail(row["id"].string ?? "")} label: {
                            VStack(alignment:.leading) {
                                Text("工作跟进升级 · " + (row["source"]["sessionId"].string ?? "成员"))
                                Text(decisionPhase(row["decision"].string ?? "")+"，尚未执行 · \(row["targets"].array.count) 项工作 · " + time(row["createdAt"]))
                            }
                        }.disabled(busy)
                    }
                    if let detail {
                        Divider()
                        Text("来源成员："+(detail["source"]["sessionId"].string ?? "")).textSelection(.enabled)
                        Text("来源公告："+(detail["source"]["announcementId"].string ?? "")).textSelection(.enabled)
                        Text("观察时间："+time(detail["createdAt"]))
                        Text("判断："+decisionPhase(detail["decision"].string ?? ""))
                        if detail["decisionRecord"] != .null {
                            Text("原因："+decisionReason(detail["decisionRecord"]["reason"].string ?? ""))
                            if detail["decisionRecord"]["probability"] != .null {Text("介入概率："+detail["decisionRecord"]["probability"].pretty)}
                            Text(detail["decisionRecord"]["adopted"] == .bool(true) ? "介入建议已采纳；尚未启动执行，也不代表工作解决。":"本次判断未采纳介入建议。")
                        }
                        Text("执行：尚未启动临时行政成员")
                        Text("结果：尚未核验解决")
                        Text("退场：尚未开始；会话退出不等于业务解决")
                        ForEach(Array(detail["targets"].array.enumerated()),id:\.offset) {_,target in
                            Text((target["id"].string ?? "")+" · "+kind(target["kind"].string ?? "")+" · "+(target["scope"] == .string("in_scope") ? "当前范围":"范围外，仅保留来源事实"))
                            Text("来源快照机组："+(target["crewId"].string ?? "")+" · 工作版本："+(target["revision"].string ?? "")).font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }.frame(maxWidth:.infinity,alignment:.leading)
            }.frame(maxHeight:460)
        }.padding().frame(width:560)
        .task {load(reset:true)}
        .onChange(of:store.readScope) {_,_ in token=UUID();rows=[];detail=nil;next=nil;dismiss()}
        .onDisappear {token=UUID()}
    }
    private func decisionPhase(_ value:String)->String {
        ["observed":"已观察来源，尚未判断","prepared":"判断已准备","dispatched":"判断请求已发出","answered":"模型已答复","unknown":"判断结果未知","unconfigured":"未配置判断模型","waiting_admission":"等待当前运行准入，尚未请求模型","stale":"判断依据已失效"][value] ?? "未知判断状态"
    }
    private func decisionReason(_ value:String)->String {
        ["prepared":"等待模型判断","native_answered":"模型原生概率答复","generated":"生成式答复，仅作建议","provider_unavailable":"模型服务不可用","restart_unknown":"重启前结果未知，不重复请求","unconfigured":"尚未配置判断模型","no_admission":"缺少当前运行准入","state_changed":"来源或工作状态已变化","stopped":"已停止","no_in_scope_targets":"当前范围没有可处理目标"][value] ?? "未知原因"
    }
    private func time(_ value:CrewdValue)->String {guard let ms=value.int else{return "未知"};return Date(timeIntervalSince1970:Double(ms)/1000).formatted(date:.abbreviated,time:.standard)}
    private func kind(_ value:String)->String {["unanswered":"缺少回报","overdue":"检查逾期","ready_unscheduled":"可推进但未安排检查"][value] ?? value}
    private func load(reset:Bool) {
        let request=UUID(),cursor=reset ? nil:next;token=request;busy=true;failure=nil
        if reset {rows=[];next=nil;detail=nil;loaded=false}
        Task { @MainActor in
            defer {if token == request {busy=false}}
            do {let page=try await store.adminCases(after:cursor);guard token == request else{return};rows += page["items"].array;next=page["nextAfter"].string;loaded=true}
            catch {if token == request {failure=error.localizedDescription}}
        }
    }
    private func readDetail(_ id:String) {
        let request=UUID();token=request;busy=true;failure=nil;detail=nil
        Task { @MainActor in
            defer {if token == request {busy=false}}
            do {let row=try await store.adminCase(id:id);guard token == request else{return};detail=row}
            catch {if token == request {failure=error.localizedDescription}}
        }
    }
}
