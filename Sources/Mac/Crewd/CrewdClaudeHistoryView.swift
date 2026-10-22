import SwiftUI

/// The same original Mac/phone/pad entry; no automatic recovery action.
struct CrewdClaudeSessionControls:View {
    @ObservedObject var workspace:CrewdWorkspace
    let sessionID:String
    var legacyResume:(()->Void)?=nil
    @State private var showing=false
    var body:some View{
        if workspace.profileForSession(sessionID)?.backend == "existingClaudePty"{
            Button("Claude 受管记录与原会话恢复"){showing=true}
                .disabled(!workspace.client.connected)
                .sheet(isPresented:$showing){CrewdClaudeHistoryPanel(workspace:workspace,session:sessionID,legacyResume:legacyResume).id(workspace.client.connectionEpoch.uuidString+"/"+sessionID)}
        }
    }
}

private struct CrewdClaudeHistoryPanel:View {
    @ObservedObject var workspace:CrewdWorkspace
    @StateObject private var store:CrewdClaudeHistoryStore
    @Environment(\.dismiss) private var dismiss
    let session:String,crew:String
    let legacyResume:(()->Void)?
    @State private var confirming=false
    @State private var confirmation:CrewdClaudeHistoryStore.ResumeConfirmation?
    @State private var legacyOrigin:CrewdClaudeHistoryStore.Origin?
    @State private var controlError:String?
    init(workspace:CrewdWorkspace,session:String,legacyResume:(()->Void)?){self.workspace=workspace;self.session=session;crew=workspace.crewID;self.legacyResume=legacyResume
        _store=StateObject(wrappedValue:CrewdClaudeHistoryStore(session:session,crew:workspace.crewID,origin:{
            guard !workspace.isRetired,workspace.client.connected else{return nil}
            return .init(generation:workspace.observationGeneration,connection:workspace.client.connectionEpoch,authority:workspace.client.authorityBinding,runtime:workspace.client.runtimeIdentity,crew:workspace.crewID)
        },read:{method,p in try await workspace.readValue(method,p)},perform:{method,p in try await workspace.performValue(method,p)},unknown:{workspace.hasOriginalUnknown(method:"session.resume",crew:workspace.crewID,session:session)}))
    }
    var body:some View{
        VStack(alignment:.leading,spacing:12){
            HStack{Text("Claude · PCW受管记录").font(.headline);Spacer();Button("关闭"){dismiss()}}
            ScrollView{VStack(alignment:.leading,spacing:16){
                Text("仅包含PCW拥有的任务与实际观察正文，不是Claude完整历史或原生ACP。查看、分页和重开不会启动或恢复会话。").font(.caption).foregroundStyle(.secondary)
                overview
                if let issue=controlError ?? store.failure{Text(issue).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
                taskList
                if store.detail != .null{taskDetail}
                DisclosureGroup("只读核对原恢复请求"){CrewdPublicRecoveryView(workspace:workspace)}
            }}
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:760,height:760))
        .task{await store.inspect();if store.inspection != .null{await store.page()}}
        .onDisappear{store.retire()}
        .onChange(of:workspace.crewID){_,next in if next != crew{store.invalidate();dismiss()}}
        .onChange(of:workspace.client.connectionEpoch){_,_ in store.invalidate();dismiss()}
        .onChange(of:workspace.observationGeneration){_,_ in store.invalidate();dismiss()}
        .onReceive(workspace.authorityEvents){event in if event.1["sessionId"] == .string(session){Task{await store.inspect()}}}
        .confirmationDialog(legacyOrigin == nil ? "只恢复这个原会话？":"尝试旧Core的恢复行为？",isPresented:$confirming,titleVisibility:.visible){
            if let confirmation{Button("明确恢复原会话"){Task{await store.resume(confirmation);self.confirmation=nil}}}
            if let legacyOrigin{Button("按旧Core语义尝试恢复"){guard store.unsupported,store.currentOrigin==legacyOrigin else{return};legacyResume?();self.legacyOrigin=nil}}
            Button("取消",role:.cancel){confirmation=nil;legacyOrigin=nil}
        }message:{Text(legacyOrigin == nil ? "恢复原记录的Claude会话；不创建替代对话，不发送首任务、恢复便条或旧Unknown。实际程序、目录与原进程终态仍会在启动时重新核验。":"此Core不提供新受管历史合同。沿原恢复入口尝试；旧Core在供应商拒绝时可能新建对话，请核对原记录。")}
    }
    private var overview:some View{
        VStack(alignment:.leading,spacing:8){
            HStack{Text("原会话").font(.subheadline.bold());Spacer();Button("核对当前资格"){Task{await store.inspect()}}.font(.caption)}
            Text(session).font(.caption.monospaced()).textSelection(.enabled)
            if store.inspection != .null{
                Text(resumeExplanation).font(.caption).foregroundStyle(.secondary)
                Text("原生ID："+(store.inspection["manualResume"]["originalAgentSessionId"].string ?? "缺失或不可验证")).font(.caption.monospaced()).textSelection(.enabled)
                Button("恢复原会话"){Task{do{legacyOrigin=nil;confirmation=try await store.prepareResume();confirming=true;controlError=nil}catch{controlError="原恢复资格未确认；未启动。请核对原请求与当前状态。"}}}
                    .buttonStyle(CrewdPrimaryButtonStyle()).disabled(!store.resumeEligible)
                Text("‘可恢复’只代表当前PCW准入，不承诺厂商完整历史或业务成功。").font(.caption).foregroundStyle(.secondary)
            }else if store.unsupported,legacyResume != nil{
                Button("尝试旧Core原恢复入口"){confirmation=nil;legacyOrigin=store.currentOrigin;confirming=true}
                    .disabled(workspace.profileForSession(session)?.canStart != true || workspace.hasOriginalUnknown(method:"session.resume",crew:crew,session:session))
            }
        }
    }
    private var resumeExplanation:String{
        switch store.inspection["manualResume"]["eligibility"].string{
        case "eligible":return "当前可明确尝试原会话恢复；实际启动仍重检程序和来源。"
        case "running":return "原会话仍在运行；不会重复启动。"
        case "notConfigured":return "原执行程序尚未配置或未合格。"
        case "missingOriginalId":return "缺少可验证的原生ID；不会生成替代ID。"
        case "previousBirthLiveOrUnknown":return "原进程仍活或终态未知；不能重复恢复。"
        case "removedCrew":return "原机组已移除，不能恢复；历史读取受当前权限控制。"
        case "missingWorkdir":return "原工作目录不可用；不会改用其它目录。"
        default:return "资格尚未核对。"
        }
    }
    private var taskList:some View{
        VStack(alignment:.leading,spacing:10){
            HStack{Text("受管任务").font(.subheadline.bold());Spacer();Button("刷新首页"){Task{await store.page()}}.font(.caption).disabled(store.reading || store.inspection == .null)}
            if let at=store.observedAt{Text("本页观察时间："+at+"；分页只固定成员范围，不冻结状态。").font(.caption).foregroundStyle(.secondary)}
            if store.reading{ProgressView("读取原任务")}
            if store.tasks.isEmpty,!store.reading,store.inspection != .null{Text("当前没有可读取的PCW受管任务记录。").font(.caption).foregroundStyle(.secondary)}
            ForEach(store.tasks.indices,id:\.self){i in let task=store.tasks[i]
                Button{if let request=task["requestId"].string{Task{await store.select(request)}}}label:{
                    VStack(alignment:.leading,spacing:5){
                        Text(task["requestId"].string ?? "").font(.caption.monospaced()).lineLimit(1)
                        Text(phaseLabel(task)+" · "+(task["currentRuntime"] == .bool(true) ? "当前受管实例":"历史实例")).font(.caption)
                        Text("创建："+(task["createdAt"].string ?? "")+" · 观察："+(task["observationCount"].string ?? "未知")+" · 未保留："+(task["omittedObservations"].string ?? "未知")).font(.caption).foregroundStyle(.secondary)
                    }.frame(maxWidth:.infinity,alignment:.leading).padding(.vertical,5)
                }.buttonStyle(.plain)
            }
            if store.cursor != nil{Button("下一页原任务"){Task{await store.page(more:true)}}.disabled(store.reading)}
        }
    }
    private func phaseLabel(_ task:CrewdValue)->String{
        if task["waitingObserved"] == .bool(true){return "实际观察到等待 · 非业务完成"}
        return ["prepared":"已准备","content_accepted":"原输入已接收","running":"执行中","cancel_requested":"已请求取消","raw_revoked":"手工控制已撤销","unknown":"原结果未知","completed":"原等待记录 · 非业务完成","aborted":"原执行已中止","failed":"原执行失败"][task["phase"].string ?? ""] ?? "未识别状态"
    }
    private var taskDetail:some View{
        VStack(alignment:.leading,spacing:10){
            Divider();Text("原任务与观察正文").font(.subheadline.bold())
            Text("读取时间："+(store.detail["observedAt"].string ?? "未知")).font(.caption).foregroundStyle(.secondary)
            let original=store.detail["originalTask"]
            if let text=original["text"].string{Text("已授权任务原文").font(.caption.bold());bodyText(text)}
            else if let envelope=original["managedEnvelope"].string{Text("历史原受管信封（没有猜测拆分为任务）").font(.caption.bold());bodyText(envelope)}
            else{Text("原任务正文未保留；没有补写或推测。").font(.caption).foregroundStyle(.secondary)}
            Text("原阶段："+phaseLabel(store.detail["task"])).font(.caption)
            ForEach(store.detail["observations"].array.indices,id:\.self){i in observation(store.detail["observations"].array[i])}
            if store.detail["observations"].array.isEmpty{Text("尚无保留的正文观察；不代表空回答或成功。").font(.caption).foregroundStyle(.secondary)}
            if store.detail["latestObservation"] != .null{Text("最新观察编号："+(store.detail["latestObservation"]["observationNo"].string ?? "未知")+" · "+availabilityLabel(store.detail["latestObservation"]["availability"].string)).font(.caption).foregroundStyle(.secondary)}
            Text("未保留观察数："+(store.detail["task"]["omittedObservations"].string ?? "未知")+"。后续活动可撤销当前等待资格，已保留正文不会变成执行授权。").font(.caption).foregroundStyle(.secondary)
        }
    }
    private func observation(_ value:CrewdValue)->some View{
        VStack(alignment:.leading,spacing:5){
            Text("观察 "+(value["observationNo"].string ?? "")+" · "+availabilityLabel(value["availability"].string)).font(.caption.bold())
            Text(value["observedAt"].string ?? "").font(.caption).foregroundStyle(.secondary)
            if let text=value["text"].string{bodyText(text.isEmpty ? "（实际收到空字符串）":text)}else{Text("此观察未保留正文，不补用旧回答。").font(.caption).foregroundStyle(.secondary)}
            Text("原观察字节："+(value["sourceBytes"].string ?? "缺失")+" · 保留："+(value["capturedBytes"].string ?? "未知")).font(.caption).foregroundStyle(.secondary)
        }.padding(.vertical,6)
    }
    private func availabilityLabel(_ value:String?)->String{["observed":"实际观察","missing":"正文缺失","truncated":"已截断UTF-8前缀","retentionLimit":"达到正文保留上限","observationLimit":"超过四条保留观察"][value ?? ""] ?? "未知"}
    private func bodyText(_ value:String)->some View{Text(value).font(.body).textSelection(.enabled).frame(maxWidth:.infinity,alignment:.leading)}
}
