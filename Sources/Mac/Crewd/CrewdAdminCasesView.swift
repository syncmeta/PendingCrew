import SwiftUI
struct CrewdAdminCasesView:View {
    @ObservedObject var store:CrewdWorkStore
    @Environment(\.dismiss) private var dismiss
    @State private var rows:[CrewdValue]=[]
    @State private var next:String?
    @State private var detail:CrewdValue?
    @State private var custody:CrewdValue?
    @State private var custodyHistory:[CrewdValue]=[]
    @State private var custodyReason=""
    @State private var lookupID=""
    @State private var custodyFresh=false
    @State private var loaded=false
    @State private var busy=false
    @State private var failure:String?
    @State private var token=UUID()
    var body:some View {
        VStack(alignment:.leading,spacing:12) {
            Text("行政处理记录").font(.headline)
            Text("记录跟进升级、介入判断与实际执行。启用判断且建议有效时，后台按权限、在线状态和容量安排受控启动；等待或未知都不代表已处理。").font(.caption).foregroundStyle(.secondary)
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
                        if detail["launchRecord"] != .null {
                            let launch=detail["launchRecord"]
                            Text("启动调度："+launchPhase(launch["phase"].string ?? ""))
                            Text("调度说明："+launchReason(launch["reason"].string ?? ""))
                            Text("调度更新时间："+time(launch["updatedAt"]))
                            Text("等待条件变化不会重新询问模型；绑定后的运行结果以执行记录为准，未知启动不自动重放。").font(.caption).foregroundStyle(.secondary)
                        } else {
                            Text(store.capabilities.contains("adminCase.automaticExecution") ? "尚无启动调度记录；未配置、未采纳或依据已失效时不会新建执行成员。" : "当前后台尚未提供自动行政启动调度记录。").font(.caption).foregroundStyle(.secondary)
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
                        if detail["handoffRecord"] == .null {Text("责任交接：暂无请求记录")}
                        else { handoffDetails(detail["handoffRecord"]) }
                        if let custody {custodyDetails(custody)}
                        else if !store.capabilities.contains("adminCase.custodyGet") {Text("当前后台尚未提供异常责任接管查询。").font(.caption).foregroundStyle(.secondary)}
                        Text("退场："+retirementPhase(detail["retirement"].string ?? ""))
                        if detail["retirementRecord"] != .null {
                            Text("退场说明："+retirementReason(detail["retirementRecord"]["reason"].string ?? ""))
                            Text(detail["retirementRecord"]["adopted"] == .bool(true) ? "此记录曾收到实际退出确认；结合当前阶段核对。":"此记录尚未确认实际退出。")
                            Text("退场执行代次："+(detail["retirementRecord"]["generation"].string ?? ""))
                        }
                        Text("业务核验解决或有效责任交接可以释放退场条件；业务仍未解决时也可交接后退场。退出请求不等于实际退场，未知结果不会自动重启。").font(.caption).foregroundStyle(.secondary)
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
    private func launchPhase(_ value:String)->String {
        ["waiting":"等待启动条件","dispatching":"正在原子领取与登记","bound":"已绑定唯一执行成员","unknown":"调度结果未知，不自动重放","stale":"调度依据已失效"][value] ?? "未知调度状态"
    }
    private func launchReason(_ value:String)->String {
        ["ready":"有效介入判断已就绪","source_not_running":"来源成员当前不在线","source_admission_missing":"来源成员未获本轮准入","capacity_unavailable":"等待执行容量","executor_unsupported":"执行器不支持受限行政策略","native_auth_unavailable":"原生执行认证不可用","runtime_unavailable":"运行时暂不可用","settings_disabled":"判断配置已停用","source_state_changed":"来源状态变化","workspace_unavailable":"工作目录不可用","shutting_down":"后台正在退出","claimed":"启动登记事务已领取","bound":"唯一执行身份已持久绑定","restart_unknown":"恢复时发现未知领取，不重放","state_changed":"当前判断或权限依据已改变"][value] ?? "未知调度原因"
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
    @ViewBuilder private func handoffDetails(_ handoff:CrewdValue)->some View {
        Text("责任交接："+handoffPhase(handoff["phase"].string ?? ""))
        Text("交接状态说明："+handoffReason(handoff["reason"].string ?? ""))
        Text((handoff["phase"] == .string("accepted") ? "已确认接手机长：":"指定接收机长（历史记录）：")+(handoff["captainSessionId"].string ?? "")).textSelection(.enabled)
        Text("请求行政成员："+(handoff["adminSessionId"].string ?? "")).textSelection(.enabled)
        Text("请求原因："+(handoff["requestReason"].string ?? ""))
        Text("请求时间："+time(handoff["createdAt"]))
        if let delivered=handoff["deliveredAt"].string {Text("通知送达时间："+delivered)}
        if handoff["acceptedAt"] != .null {Text("接受时间："+time(handoff["acceptedAt"]))}
        if let note=handoff["acceptanceNote"].string {Text("机长接受说明："+note)}
        ForEach(Array(handoff["targets"].array.enumerated()),id:\.offset) {_,target in
            Text("交接快照："+(target["id"].string ?? "")+" · "+(target["crewId"].string ?? "")+" · 版本 "+(target["revision"].string ?? "")).font(.caption)
        }
        Text("通知送达不代表机长接手；接受交接只确认后续责任，不代表业务已解决。交接依据失效不会抹去已接受的责任；须完成明确接管或核验解除，原责任才释放。").font(.caption).foregroundStyle(.secondary)
    }
    private func custodyPhase(_ value:String)->String {["active":"当前责任有效，尚未解除","stale":"责任依据已失效，原责任仍保留","discharged":"当前责任已凭证据解除"][value] ?? "未知责任状态"}
    private func custodyStatus(_ value:String)->String {["waiting":"等待通知送达","delivered":"通知已送达，等待新机长明确接受","accepted":"新机长已接受责任","stale":"提议依据已失效，保留历史"][value] ?? "未知提议状态"}
    private func custodyExplanation(_ value:String)->String {["original_custody":"原始已接受责任","proposed":"用户已提出接管","notification_delivered":"通知投递已确认","captain_accepted":"机长已明确接受","state_changed":"授权或事实已变化","notification_dropped":"通知未送达","superseded":"已有后续提议替代","targets_unresolved":"仍有目标未解决","out_of_scope":"含范围外目标，不能整体解除","target_missing":"原目标缺失","evidence_incomplete":"证据不足，不能解除","verified":"当前目标证据核验通过"][value] ?? "未知原因"}
    @ViewBuilder private func custodyTransferDetails(_ row:CrewdValue)->some View {
        Text("提议："+custodyStatus(row["phase"].string ?? ""))
        Text("原责任人："+(row["fromSessionId"].string ?? "")+" → 指定当前机长："+(row["toSessionId"].string ?? "")).textSelection(.enabled)
        Text("请求编号："+(row["requestId"].string ?? "")).font(.caption).textSelection(.enabled)
        Text("理由："+(row["requestReason"].string ?? ""))
        Text("提出时间："+time(row["createdAt"]))
        if let at=row["deliveredAt"].string {Text("送达："+at).font(.caption)}
        if row["acceptedAt"] != .null {Text("明确接受："+time(row["acceptedAt"]))}
        if let note=row["acceptanceNote"].string {Text("接受说明："+note)}
        if let next=row["supersededBy"].string {Text("后继提议："+next).font(.caption).textSelection(.enabled)}
    }
    @ViewBuilder private func custodyDetails(_ row:CrewdValue)->some View {
        Divider()
        Text("当前责任与异常接管").font(.headline)
        Text(custodyPhase(row["phase"].string ?? ""))
        Text("当前责任人："+(row["currentCustodianSessionId"].string ?? "")).textSelection(.enabled)
        Text("说明："+custodyExplanation(row["reason"].string ?? ""))
        if row["currentRecord"] != .null {custodyTransferDetails(row["currentRecord"])}
        if row["pending"] != .null {Text("待确认接管").font(.headline);custodyTransferDetails(row["pending"])}
        ForEach(Array(row["targetResults"].array.enumerated()),id:\.offset) {_,result in
            Text((result["id"].string ?? "")+" · "+targetReason(result["reason"].string ?? ""))
            Text("证据来源："+(result["resolvedBy"] == .string("case_admin") ? "原案行政成员":result["resolvedBy"] == .string("other") ? "其他操作者":"尚无已核验解决来源")).font(.caption)
        }
        if !row["outOfScope"].array.isEmpty {Text("范围外，不能整体解除："+row["outOfScope"].array.compactMap{$0.string}.joined(separator:"、")).foregroundStyle(.orange)}
        if row["discharge"] != .null {Text("责任解除核验时间："+time(row["discharge"]["verifiedAt"]))}
        Text("承接不代表业务解决；责任解除、原案件结果与实际退场分别记录。原行政成员的写权限不会恢复，原工作负责人不变。").font(.caption).foregroundStyle(.secondary)
        Button("重新读取责任") {readCustody()}.disabled(busy)
        if row["phase"] == .string("stale"),store.capabilities.contains("adminCase.custodyPropose") {
            Text("明确提出交由该机组当前机长接管。接收人由后台固定并核验，新机长仍须收到通知后亲自接受。").font(.caption)
            TextField("接管理由",text:$custodyReason,axis:.vertical).lineLimit(2...4)
            Button("向当前机长提出接管") {proposeCustody()}.disabled(busy || !custodyFresh || (row["pending"] != .null && row["pending"]["phase"] != .string("stale")) || custodyReason.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || store.custodyRequests(caseID:row["caseId"].string ?? "").contains{$0.status == .unknown || $0.status == .submitting})
        }
        ForEach(store.custodyRequests(caseID:row["caseId"].string ?? "")) {intent in
            Text("本机请求："+intent.id).font(.caption).textSelection(.enabled)
            Text("原操作理由："+intent.reason).font(.caption)
            Text(["submitting":"提交中","unknown":"结果未知；保留原编号，只做查证","recorded":"已查到原请求记录","refused":"已明确拒绝或尚未提交；重新读取后可由你发起新操作","notFound":"当前未查到原请求；不代表它永远不会提交"][intent.status.rawValue] ?? "待查证").font(.caption)
            if let message=intent.message {Text(message).font(.caption).foregroundStyle(.orange)}
            Button("只读查证此请求") {lookupCustody(intent.id)}.disabled(busy || intent.status == .submitting || !store.capabilities.contains("adminCase.custodyTransferGet"))
        }
        if store.capabilities.contains("adminCase.custodyTransferGet") {
            HStack {TextField("历史提议 UUID",text:$lookupID);Button("查询历史") {lookupCustody(lookupID.trimmingCharacters(in:.whitespacesAndNewlines))}.disabled(busy || UUID(uuidString:lookupID.trimmingCharacters(in:.whitespacesAndNewlines)) == nil)}
            if let previous=row["currentRecord"]["predecessorReceiptId"].string,previous != row["originalRequestId"].string {
                Button("查看上一代责任提议") {lookupCustody(previous)}.disabled(busy)
            }
        }
        ForEach(Array(custodyHistory.enumerated()),id:\.offset) {_,record in
            Divider();Text("历史查证记录").font(.headline);custodyTransferDetails(record)
            if let previous=record["predecessorReceiptId"].string,previous != row["originalRequestId"].string {
                Button("继续查看前一代") {lookupCustody(previous)}.disabled(busy)
            }
        }
    }
    private func readCustody() {
        guard let id=detail?["id"].string else{return}
        let request=UUID();token=request;busy=true;failure=nil;custodyFresh=false
        Task { @MainActor in
            defer {if token==request {busy=false}}
            do {let row=try await store.custody(caseID:id);guard token==request else{return};custody=row;custodyFresh=true}
            catch {if token==request {failure=error.localizedDescription}}
        }
    }
    private func proposeCustody() {
        guard let row=custody,let id=row["caseId"].string,let revision=row["revision"].string else{return}
        let request=UUID();token=request;busy=true;failure=nil;custodyFresh=false
        Task { @MainActor in
            defer {if token==request {busy=false}}
            do {let result=try await store.proposeCustody(caseID:id,revision:revision,reason:custodyReason);guard token==request else{return};custody=result;custodyReason=""}
            catch {if token==request {failure=error.localizedDescription}}
        }
    }
    private func lookupCustody(_ id:String) {
        guard let caseID=detail?["id"].string else{return}
        let request=UUID();token=request;busy=true;failure=nil
        Task { @MainActor in
            defer {if token==request {busy=false}}
            do {let row=try await store.custodyTransfer(requestID:id,caseID:caseID);guard token==request else{return};custodyHistory.removeAll{$0["requestId"] == .string(id)};custodyHistory.append(row)}
            catch {if token==request {failure=error.localizedDescription}}
        }
    }
    private func handoffPhase(_ value:String)->String {
        ["waiting":"等待通知送达与机长接受","delivered":"通知已送达，等待机长接受","accepted":"机长已明确接受后续责任","stale":"交接依据已失效"][value] ?? "未知交接状态"
    }
    private func handoffReason(_ value:String)->String {
        ["requested":"已提交交接请求","notification_delivered":"通知投递已确认","captain_accepted":"真实机长已确认接手","state_changed":"来源或工作依据已变化","stopped":"关联成员已停止","recipient_changed":"原指定接收机长已变化","notification_dropped":"通知未能送达"][value] ?? "未知交接原因"
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
        if reset {rows=[];next=nil;detail=nil;custody=nil;custodyHistory=[];custodyFresh=false;loaded=false}
        Task { @MainActor in
            defer {if token == request {busy=false}}
            do {let page=try await store.adminCases(after:cursor);guard token == request else{return};rows += page["items"].array;next=page["nextAfter"].string;loaded=true}
            catch {if token == request {failure=error.localizedDescription}}
        }
    }
    private func readDetail(_ id:String) {
        let request=UUID();token=request;busy=true;failure=nil;detail=nil;custody=nil;custodyHistory=[];custodyFresh=false
        Task { @MainActor in
            defer {if token == request {busy=false}}
            do {
                let row=try await store.adminCase(id:id);guard token == request else{return};detail=row
                if store.capabilities.contains("adminCase.custodyGet"),row["custodyRecord"] != .null {
                    let current=try await store.custody(caseID:id);guard token == request else{return};custody=current;custodyFresh=true
                }
            }
            catch {if token == request {failure=error.localizedDescription}}
        }
    }
}
