import SwiftUI

struct CrewdCodexSettingsDraft {
    var modelEnabled=false,effortEnabled=false,fastEnabled=false,policyEnabled=false,reviewerEnabled=false,sandboxEnabled=false,modeEnabled=false,summaryEnabled=false
    var model="",effort="",fast=false,policy="onRequest",reviewer="user",sandbox="workspaceWrite",network=false,mode="default",summary="auto"
    var mcp=true,permissions=true,rules=true,sandboxApproval=true,skill=true
    func value()throws->CrewdValue?{
        var fields:[String:CrewdValue]=[:]
        if modelEnabled{fields["model"] = .string(model)}
        if effortEnabled{fields["effort"] = .string(effort)}
        if fastEnabled{fields["fast"] = .bool(fast)}
        if policyEnabled{fields["approvalPolicy"] = policy == "granular" ? .object(["granular":.object(["mcpElicitations":.bool(mcp),"requestPermissions":.bool(permissions),"rules":.bool(rules),"sandboxApproval":.bool(sandboxApproval),"skillApproval":.bool(skill)])]):.string(policy)}
        if reviewerEnabled{fields["approvalsReviewer"] = .string(reviewer)}
        if sandboxEnabled{fields["sandbox"] = sandbox == "dangerFullAccess" ? .object(["type":.string(sandbox)]):.object(["type":.string(sandbox),"networkAccess":.bool(network)])}
        if modeEnabled{fields["collaborationMode"] = .string(mode)}
        if summaryEnabled{fields["summary"] = .string(summary)}
        return fields.isEmpty ? nil:try CrewdCodexWire.settings(.object(fields))
    }
}

/// Shared first-turn and following-turn settings. Disabled fields are omitted;
/// UI defaults are never represented as observed provider values.
struct CrewdCodexSettingsEditor:View {
    @Binding var draft:CrewdCodexSettingsDraft
    var body:some View{
        VStack(alignment:.leading,spacing:16){
            CrewdFormSection(title:"模型与执行方式",detail:"未启用的项目沿用原设置，不会被界面默认值覆盖。") {
                Toggle("指定模型",isOn:$draft.modelEnabled)
                if draft.modelEnabled{TextField("模型目录中的实际 ID",text:$draft.model).textFieldStyle(.roundedBorder)}
                Toggle("指定推理强度",isOn:$draft.effortEnabled)
                if draft.effortEnabled{TextField("模型支持的 effort 值",text:$draft.effort).textFieldStyle(.roundedBorder)}
                Toggle("设置速度档位",isOn:$draft.fastEnabled)
                if draft.fastEnabled{Toggle("Fast",isOn:$draft.fast)}
                Toggle("设置协作模式",isOn:$draft.modeEnabled)
                if draft.modeEnabled{Picker("协作",selection:$draft.mode){Text("默认").tag("default");Text("计划").tag("plan")}}
            }
            DisclosureGroup("权限与高级设置"){
                CrewdFormSection(title:"审批与沙箱",detail:"只提交已勾选设置；不会替当前请求作答或提升机组权限。") {
                    Toggle("设置审批策略",isOn:$draft.policyEnabled)
                    if draft.policyEnabled{
                        Picker("审批",selection:$draft.policy){Text("按请求询问").tag("onRequest");Text("不可信操作询问").tag("untrusted");Text("不询问").tag("never");Text("逐类控制").tag("granular")}
                        if draft.policy == "granular"{Toggle("MCP 询问",isOn:$draft.mcp);Toggle("权限请求",isOn:$draft.permissions);Toggle("规则审批",isOn:$draft.rules);Toggle("沙箱审批",isOn:$draft.sandboxApproval);Toggle("Skill 审批",isOn:$draft.skill)}
                    }
                    Toggle("设置审批者",isOn:$draft.reviewerEnabled)
                    if draft.reviewerEnabled{Picker("审批者",selection:$draft.reviewer){Text("用户").tag("user");Text("自动审批审查").tag("autoReview")}}
                    Toggle("设置沙箱",isOn:$draft.sandboxEnabled)
                    if draft.sandboxEnabled{
                        Picker("沙箱",selection:$draft.sandbox){Text("只读").tag("readOnly");Text("工作区可写").tag("workspaceWrite");Text("完整访问").tag("dangerFullAccess")}
                        if draft.sandbox != "dangerFullAccess"{Toggle("允许网络",isOn:$draft.network)}
                    }
                    Toggle("设置推理摘要",isOn:$draft.summaryEnabled)
                    if draft.summaryEnabled{Picker("摘要",selection:$draft.summary){Text("自动").tag("auto");Text("简洁").tag("concise");Text("详细").tag("detailed");Text("无").tag("none")}}
                }.padding(.top,8)
            }.font(CrewdType.label)
        }.font(CrewdType.label).tint(Theme.Palette.accent)
    }
}

struct CrewdCodexSessionControls:View {
    @ObservedObject var workspace:CrewdWorkspace
    let sessionID:String
    @State private var showing=false
    var body:some View{
        if workspace.profileForSession(sessionID)?.backend == "codex"{
            HStack{
                Button("Codex 会话控制"){showing=true}.disabled(!workspace.client.readyForWork)
                if !workspace.client.capabilities.contains("session.codex"){Text("此后台版本尚未接入完整控制").font(.caption).foregroundStyle(.secondary)}
            }.sheet(isPresented:$showing){CrewdCodexControlPanel(workspace:workspace,sessionID:sessionID).id(workspace.client.connectionEpoch.uuidString+"/"+sessionID)}
        }
    }
}

private struct CrewdCodexControlPanel:View {
    @ObservedObject var workspace:CrewdWorkspace
    @StateObject private var store:CrewdCodexSessionStore
    let sessionID:String
    @Environment(\.dismiss) private var dismiss
    @State private var settings=CrewdCodexSettingsDraft()
    @State private var title=""
    @State private var bodyText=""
    @State private var reviewKind="uncommittedChanges"
    @State private var reviewValue=""
    @State private var forkTurn=""
    @State private var forkTitle=""
    @State private var localError:String?
    @State private var confirmationTarget:CrewdCommandTarget?
    @State private var confirmation:String?
    @State private var page="概览"
    init(workspace:CrewdWorkspace,sessionID:String){self.workspace=workspace;self.sessionID=sessionID;_store=StateObject(wrappedValue:.init(workspace:workspace,sessionID:sessionID))}
    private var info:CrewdValue{store.inspection?.value ?? .null}
    var body:some View{
        VStack(spacing:0){
            HStack(alignment:.top){
                CrewdPanelHeader(title:"Codex 会话",detail:"状态与记录来自当前后台；查看不会恢复会话。",symbol:"terminal",close:{dismiss()})
                Button{Task{await store.refresh()}}label:{Image(systemName:"arrow.clockwise")}.buttonStyle(CrewdToolButtonStyle()).help("刷新会话状态")
            }.padding(20)
            ScrollView(.horizontal,showsIndicators:false){
                Picker("会话控制",selection:$page){ForEach(["概览","执行","历史","设置"],id:\.self){Text($0).tag($0)}}.pickerStyle(.segmented).fixedSize().padding(.horizontal,20).padding(.bottom,12)
            }
            Divider()
            ScrollView{VStack(alignment:.leading,spacing:20){
                if !store.supported{CrewdInlineNotice(title:"当前后台未接入此控制协议",detail:"现有消息与明确恢复入口仍可使用。",symbol:"info.circle")}
                if let issue=localError ?? store.error{CrewdInlineNotice(title:"操作未完成",detail:issue,symbol:"exclamationmark.triangle",warning:true)}
                if workspace.sessionID == sessionID,workspace.inspection["runnerRequest"] != .null {
                    CrewdInlineNotice(title:"有一项请求等待你的答复",detail:"答复仍由原结构化表单提交，不会替你自动选择。",symbol:"person.crop.circle.badge.questionmark")
                    if page != "执行"{Button("查看待答复请求"){page="执行"}.buttonStyle(CrewdPrimaryButtonStyle())}
                }
                switch page {
                case "执行":CrewdFormSection(title:"当前执行与后续任务"){taskControls}
                case "历史":CrewdFormSection(title:"会话历史"){historySection}
                case "设置":settingsSection;CrewdFormSection(title:"能力与原请求核对"){capabilitySection}
                default:CrewdFormSection(title:"当前会话"){identity};CrewdFormSection(title:"上下文与工具"){metadataSection}
                }
            }.padding(20).frame(maxWidth:.infinity,alignment:.leading)}
                .background(Theme.Palette.surfaceMuted.opacity(0.22))
        }.background(Theme.Palette.canvas).tint(Theme.Palette.accent)
            .modifier(CrewdAdaptiveFormFrame(width:680,height:760,scrolls:false))
        .task{await store.refresh()}
        .onDisappear{store.retire()}
        .confirmationDialog("确认执行这个会话操作？",isPresented:Binding(get:{confirmation != nil},set:{if !$0{confirmation=nil}}),titleVisibility:.visible){
            if let action=confirmation{Button(action){executeConfirmed(action)}}
            Button("取消",role:.cancel){confirmation=nil}
        }message:{Text("副作用绑定当前已读取的原会话代际；未知结果只核对原请求。")}
    }
    private var identity:some View{
        VStack(alignment:.leading,spacing:6){
            DisclosureGroup("身份与原生记录"){
                Text("会话："+sessionID).font(.caption.monospaced()).textSelection(.enabled)
                Text("原生 thread："+(info["threadId"].string ?? "未观察到")).font(.caption.monospaced()).textSelection(.enabled)
            }.font(CrewdType.caption)
            Text(info["running"] == .null ? "状态尚未读取":info["running"] == .bool(true) ? (info["working"] == .bool(true) ? "正在执行":"已运行 · 空闲"):"已停止")
            Text("队列："+(info["queuedInputs"].int.map(String.init) ?? "未知")).font(.caption)
            if let row=workspace.sessions.first(where:{$0["id"] == .string(sessionID)}){Text("工作目录："+(row["worktree"].string ?? row["workingDirectory"].string ?? "由原机组配置")).font(.caption).textSelection(.enabled)}
            Text("期望标题："+(info["title"]["desired"].string ?? "未设置")).font(.caption)
            Text("原生标题："+(info["title"]["native"].string ?? "未确认")+" · "+(info["title"]["status"].string ?? "未知")).font(.caption)
            TextField("明确设置会话名",text:$title)
            actionButton("同步原生标题",capability:"title"){Task{await store.submit(.codexTitle,fields:["title":.string(title)])}}
            if store.lastResult != .null{DisclosureGroup("原操作回执"){Text(store.lastResult.pretty).font(.caption.monospaced()).textSelection(.enabled)}}
        }
    }
    private var settingsSection:some View{
        VStack(alignment:.leading,spacing:12){
            VStack(alignment:.leading,spacing:10){
                Text(info["historical"] == .bool(true) ? "以下观察来自历史进程，不证明当前进程已应用。":"以下为实际观察；未返回的值保持未知。").font(.caption).foregroundStyle(.secondary)
                CrewdDetailValue(title:"已观察模型",value:info["settings"]["model"].string ?? "未返回")
                CrewdDetailValue(title:"设置状态",value:info["settingsStatus"].string ?? "未提交")
                DisclosureGroup("完整观察与后续轮次期望"){
                    Text("实际观察："+info["settings"].pretty).font(.caption.monospaced()).textSelection(.enabled)
                    Text("后续轮次期望："+info["desiredSettings"].pretty).font(.caption.monospaced()).textSelection(.enabled)
                }
                actionButton("读取实际模型目录",capability:"models"){Task{await store.read("models")}}
                ForEach(store.models.indices,id:\.self){index in
                    let model=store.models[index]
                    Button(model["displayName"].string ?? model["model"].string ?? "模型") {settings.modelEnabled=true;settings.model=model["model"].string ?? ""}
                    ForEach(model["supportedReasoningEfforts"].array.indices,id:\.self){effortIndex in
                        let effort=model["supportedReasoningEfforts"].array[effortIndex]
                        Button("推理："+(effort["reasoningEffort"].string ?? "")){settings.effortEnabled=true;settings.effort=effort["reasoningEffort"].string ?? ""}.font(.caption)
                    }
                }
                CrewdCodexSettingsEditor(draft:$settings)
                actionButton("提交后续轮次设置",capability:"configure"){do{guard let patch=try settings.value() else{return};Task{await store.submit(.codexConfigure,fields:["patch":patch])}}catch{localError=error.localizedDescription}}
            }.padding(.vertical,8)
        }
    }
    private var taskControls:some View{
        VStack(alignment:.leading,spacing:12){
            VStack(alignment:.leading,spacing:10){
                TextField("给当前轮次或下一轮的原文",text:$bodyText,axis:.vertical).lineLimit(3...8)
                actionButton("Steer 当前轮次",capability:"steer"){Task{await store.submit(.codexSteer,fields:["text":.string(bodyText)])}}
                    .disabled(store.inspection?.target.turnID == nil)
                actionButton("明确排入下一轮",capability:"queue"){Task{await store.submit(.codexQueue,fields:["text":.string(bodyText)])}}
                Text("排队被接受不代表已投递或任务完成；停止会放弃余下队列。").font(.caption).foregroundStyle(.secondary)
                #if os(macOS)
                HStack{lifecycleActions}
                #else
                VStack(alignment:.leading,spacing:10){lifecycleActions}.frame(maxWidth:.infinity,alignment:.leading)
                #endif
                actionButton("压缩上下文",capability:"compact"){confirm("压缩上下文")}.disabled(info["working"] == .bool(true))
                Picker("审查目标",selection:$reviewKind){Text("未提交改动").tag("uncommittedChanges");Text("基准分支").tag("baseBranch");Text("提交").tag("commit");Text("自定义").tag("custom")}
                if reviewKind != "uncommittedChanges"{TextField("原分支 / SHA / 审查说明",text:$reviewValue,axis:.vertical)}
                actionButton("开始审查",capability:"review"){confirm("开始审查")}.disabled(info["working"] == .bool(true))
                Text("压缩与审查回执表示已开始；完成以真实事件为准。").font(.caption).foregroundStyle(.secondary)
                if workspace.sessionID == sessionID,workspace.inspection["runnerRequest"] != .null,workspace.inspection["live"]["pendingDecision"]["kind"] == .string("runner_request"){
                    CrewdHumanRequestView(workspace:workspace,request:workspace.inspection["runnerRequest"],decision:workspace.inspection["live"]["pendingDecision"])
                }
            }.padding(.vertical,8)
        }
    }
    @ViewBuilder private var lifecycleActions:some View {
        actionButton("打断当前轮次",capability:"interrupt"){existingControl("session.interrupt")}
        Button("停止整个会话"){confirm("停止会话")}
            .frame(minHeight:CrewdType.target)
            .disabled(store.busy || !store.supported || !workspace.client.readyForWork)
        actionButton("手动恢复原会话",capability:"resume"){confirm("恢复原会话")}
    }
    private var historySection:some View{
        VStack(alignment:.leading,spacing:12){
            VStack(alignment:.leading,spacing:10){
                Text("读取历史不恢复会话，查看旧轮次不回滚文件。").font(.caption).foregroundStyle(.secondary)
                actionButton("读取原生历史",capability:"history"){Task{await store.read("history")}}
                ForEach(store.history.indices,id:\.self){index in
                    let turn=store.history[index]
                    VStack(alignment:.leading,spacing:6){
                        Text((turn["id"].string ?? "未知轮次")+" · "+(turn["status"].string ?? "未知状态")).font(.caption.monospaced()).textSelection(.enabled)
                        ForEach(turn["items"].array.indices,id:\.self){itemIndex in CrewdRunnerItemView(item:turn["items"].array[itemIndex])}
                        if turn["status"] == .string("completed"),let id=turn["id"].string{Button("选此已完成轮次为分叉起点"){forkTurn=id}.font(.caption)}
                        DisclosureGroup("原始历史记录"){Text(turn.pretty).font(.caption.monospaced()).textSelection(.enabled)}
                    }.padding(.vertical,5)
                }
                if store.historyCursor != nil{Button("下一页"){Task{await store.read("history",more:true)}}.disabled(store.busy)}
                TextField("分叉截止的已完成 turn ID（可留空）",text:$forkTurn)
                TextField("新会话名称（可留空）",text:$forkTitle)
                actionButton("建立停止状态的分叉",capability:"fork"){confirm("建立分叉")}.disabled(info["working"] == .bool(true))
                Text("分叉不会启动源或新会话，需另行明确恢复。").font(.caption).foregroundStyle(.secondary)
            }.padding(.vertical,8)
        }
    }
    private var metadataSection:some View{
        DisclosureGroup("上下文用量、工具与 Skills"){
            VStack(alignment:.leading,spacing:10){
                Button("读取原始上下文 / Token 用量"){Task{await store.read("usage")}}.disabled(store.busy || store.inspection?.capability("history")?.available != true)
                Text("保留供方累计快照与缺失值；不相加成虚假的消耗或上下文余量。").font(.caption).foregroundStyle(.secondary)
                if let latest=store.usage.last,let bytes=latest["tokenUsageJson"].string?.data(using:.utf8),let observed=try? JSONDecoder().decode(CrewdValue.self,from:bytes){
                    Text("上下文窗口："+(observed["modelContextWindow"] == .null ? "未知":observed["modelContextWindow"].pretty)+" · 最近轮次 Token："+(observed["last"]["totalTokens"] == .null ? "未知":observed["last"]["totalTokens"].pretty)).font(.caption)
                }
                DisclosureGroup("原始用量快照"){
                    ForEach(store.usage.indices,id:\.self){index in
                        Text((store.usage[index]["observedAt"].string ?? "")+"\n"+(store.usage[index]["tokenUsageJson"].string ?? "未观察到用量")).font(.caption.monospaced()).textSelection(.enabled)
                    }
                }
                if store.usageCursor != nil{Button("读取后续用量记录"){Task{await store.read("usage",more:true)}}.disabled(store.busy)}
                actionButton("读取工具与 Skill 元数据",capability:"tools"){Task{await store.read("tools")}}
                if store.tools != .null{DisclosureGroup("工具与 Skill 原始清单"){Text(store.tools.pretty).font(.caption.monospaced()).textSelection(.enabled)}}
                Text("读取清单不调用工具或安装插件。原生附件与 Skill 输入的可信传输尚未接线。").font(.caption).foregroundStyle(.secondary)
            }.padding(.vertical,8)
        }
    }
    private var capabilitySection:some View{
        DisclosureGroup("实际能力状态"){
            ForEach(store.inspection?.capabilities ?? []){capability in
                VStack(alignment:.leading){Text(capability.id+" · "+capability.protocolKind);Text(capability.explanation).font(.caption).foregroundStyle(.secondary)}.padding(.vertical,3)
            }
            Text("归档、删除、历史回退和命名/全局权限配置：PendingCrew 尚未接线。").font(.caption).foregroundStyle(.secondary)
            DisclosureGroup("只读核对原控制请求"){CrewdPublicRecoveryView(workspace:workspace)}
        }
    }
    private func actionButton(_ title:String,capability:String,action:@escaping()->Void)->some View{
        Button(title,action:action).frame(minHeight:CrewdType.target).disabled(store.busy || store.inspection?.capability(capability)?.available != true || !workspace.client.readyForWork)
    }
    private func confirm(_ action:String){confirmationTarget=store.inspection?.target;confirmation=action}
    private func existingControl(_ method:String,target:CrewdCommandTarget?=nil){
        guard store.supported,workspace.client.readyForWork,let target=target ?? store.inspection?.target else{return}
        let params:CrewdValue=method == "session.stop" ? .object(["target":target.value,"reason":.string("人类在 Codex 控制页停止会话")]):target.value
        Task{do{_ = try await workspace.performValue(method,params);await store.refresh()}catch{localError=error.localizedDescription}}
    }
    private func executeConfirmed(_ action:String){
        let target=confirmationTarget
        confirmation=nil;confirmationTarget=nil
        guard let target else{return}
        switch action {
        case "停止会话":existingControl("session.stop",target:target)
        case "恢复原会话":existingControl("session.resume",target:target)
        case "压缩上下文":Task{await store.submit(.codexCompact,target:target)}
        case "建立分叉":
            var fields:[String:CrewdValue]=[:]
            if !forkTurn.isEmpty{fields["lastTurnId"] = .string(forkTurn)}
            if !forkTitle.isEmpty{fields["title"] = .string(forkTitle)}
            Task{await store.submit(.codexFork,fields:fields,target:target)}
        case "开始审查":
            var reviewTarget:[String:CrewdValue]=["type":.string(reviewKind)]
            if reviewKind != "uncommittedChanges"{reviewTarget[reviewKind == "baseBranch" ? "branch":reviewKind == "commit" ? "sha":"instructions"] = .string(reviewValue)}
            Task{await store.submit(.codexReview,fields:["reviewTarget":.object(reviewTarget)],target:target)}
        default:break
        }
    }
}

struct CrewdCodexStartOptions:View {
    @ObservedObject var workspace:CrewdWorkspace
    @Binding var title:String
    @Binding var settings:CrewdCodexSettingsDraft
    var body:some View {
        DisclosureGroup("Codex 首轮设置与名称"){
            VStack(alignment:.leading,spacing:8){
                CrewdFormField(title:"会话名称",detail:"留空时取首条任务摘要；名称不会替代首条任务。") {TextField("例如：整理发布资料",text:$title)}
                if workspace.client.capabilities.contains("session.codex"){
                    CrewdCodexSettingsEditor(draft:$settings)
                    Text("配置确认后才提交首轮原任务；标题不同步不会重发任务。").font(.caption).foregroundStyle(.secondary)
                }else{Text("当前后台尚未接入首轮 Codex 设置。").font(.caption).foregroundStyle(.secondary)}
            }
        }
    }
}
