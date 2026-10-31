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
            Text("记录工作跟进升级来源、判断与执行事实。自动行政启动尚未启用；内部执行验证记录不代表日用自动处理。").font(.caption).foregroundStyle(.secondary)
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
                                Text(decisionPhase(row["decision"].string ?? "")+" · "+executionPhase(row["execution"].string ?? "")+" · \(row["targets"].array.count) 项工作 · " + time(row["createdAt"]))
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
                            Text(detail["decisionRecord"]["adopted"] == .bool(true) ? "介入建议已采纳；是否启动须看独立执行记录，不代表工作解决。":"本次判断未采纳介入建议。")
                        }
                        Text("执行："+executionPhase(detail["execution"].string ?? ""))
                        if detail["executionRecord"] != .null {
                            Text("执行成员："+(detail["executionRecord"]["sessionId"].string ?? "")).textSelection(.enabled)
                            Text("执行原因："+executionReason(detail["executionRecord"]["reason"].string ?? ""))
                            Text("已核实协作效果："+String(detail["executionRecord"]["effectsCount"].int ?? 0)+"；效果数不代表案件已解决。")
                            if let generation=detail["executionRecord"]["generation"].string {Text("确认执行代次："+generation)}
                        }
                        if detail["outcomeRecord"] == .null {Text("结果：尚无核验记录")}
                        else {
                            let outcome=detail["outcomeRecord"]
                            Text("结果："+outcomePhase(outcome["phase"].string ?? ""))
                            Text("核验说明："+outcomeReason(outcome["reason"].string ?? ""))
                            Text("核验时间："+time(outcome["verifiedAt"]))
                            ForEach(Array(outcome["targets"].array.enumerated()),id:\.offset) {_,result in
                                Text((result["id"].string ?? "")+" · "+targetReason(result["reason"].string ?? ""))
                                Text("结果归因："+(result["resolvedBy"] == .string("case_admin") ? "本案行政成员":result["resolvedBy"] == .string("other") ? "其他操作者":"尚未确认解决来源"))
                                if let actor=result["actorSessionId"].string {Text("操作者："+actor).font(.caption)}
                                if let seq=result["eventSeq"].string {Text("核验事件序号："+seq).font(.caption)}
                                if let wake=result["wakeId"].int {Text("通知回执编号："+String(wake)).font(.caption)}
                            }
                            Text("核验解决与执行器退场独立；其他操作者解决的目标不算本案行政成果。").font(.caption).foregroundStyle(.secondary)
                        }
                        Text("退场："+retirementPhase(detail["retirement"].string ?? ""))
                        if detail["retirementRecord"] != .null {
                            Text("退场说明："+retirementReason(detail["retirementRecord"]["reason"].string ?? ""))
                            Text(detail["retirementRecord"]["adopted"] == .bool(true) ? "此记录曾收到实际退出确认；结合当前阶段核对。":"此记录尚未确认实际退出。")
                            Text("退场执行代次："+(detail["retirementRecord"]["generation"].string ?? ""))
                        }
                        Text("案件解决只代表业务核验通过；退出请求不等于实际退场，未知结果不会自动重启。").font(.caption).foregroundStyle(.secondary)
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
    private func executionPhase(_ value:String)->String {
        ["not_started":"尚未开始","registered":"已登记唯一成员，尚未请求启动","requested":"已请求启动，尚未确认实际运行","running":"已收到实际启动确认","unknown":"启动结果未知，不自动重试","stale":"执行依据已失效"][value] ?? "未知执行状态"
    }
    private func executionReason(_ value:String)->String {
        ["registered":"案件与成员已绑定","launch_requested":"启动请求已登记","started":"已收到实际 Spawned 确认","launch_unknown":"启动结果未确认","restart_unknown":"重启前执行结果未知","state_changed":"执行所依赖的状态已变化"][value] ?? "未知原因"
    }
    private func outcomePhase(_ value:String)->String {["unresolved":"尚未解决","resolved":"目标已核验解决，尚不代表成员退场","stale":"核验依据已失效"][value] ?? "未知结果"}
    private func outcomeReason(_ value:String)->String {["no_execution":"尚无执行记录","no_verified_admin_effect":"尚无已核实行政效果","targets_unresolved":"仍有目标未解决","out_of_scope":"含范围外目标，不能确认全案解决","verified":"目标核验通过","authority_changed":"来源或权限依据已变化"][value] ?? "未知原因"}
    private func targetReason(_ value:String)->String {
        ["closed_with_evidence":"已凭证据关闭","reassigned_and_acknowledged":"已重派并确认接收","no_closure_evidence":"关闭缺少证据","no_matching_event":"尚无匹配真实事件","original_problem_remains":"原问题仍存在","notification_pending":"等待通知投递确认","notification_dropped":"通知未送达","owner_unavailable":"负责人不可用","target_missing":"目标缺失","no_verified_admin_receipt":"尚无已核实行政回执"][value] ?? "未知核验状态"
    }
    private func retirementPhase(_ value:String)->String {
        ["not_started":"尚未开始","prepared":"已准备判断","dispatched":"退场判断请求已发出","decision_ready":"退场判断就绪，尚未请求退出","exit_requested":"已请求退出，等待实际确认","parked":"已确认实际退出并保留会话","unknown":"退出结果未知","stale":"退场依据已失效","suppressed":"本次退场已抑制"][value] ?? "未知退场阶段"
    }
    private func retirementReason(_ value:String)->String {
        ["prepared":"等待判断","answered":"模型已答复","low_confidence":"置信度不足","provider_unavailable":"模型服务不可用","unconfigured":"未配置判断模型","restart_unknown":"重启前结果未知","state_changed":"依据已变化","stopped":"已停止","exit_unknown":"未能确认执行器退出"][value] ?? "未知退场原因"
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
