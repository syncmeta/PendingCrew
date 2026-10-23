import SwiftUI

/// The normal Mac/iOS root owns this entry. A notification never chooses a host.
struct CrewNotificationEntry:ViewModifier {
    @EnvironmentObject private var model:AppModel
    @EnvironmentObject private var crewStore:CrewStore
    @ObservedObject private var native=CrewNativeNotifications.shared
    @State private var unavailable=false
    func body(content:Content)->some View {
        content
        .onChange(of:native.hint){_,hint in
            guard let hint,native.hint==hint else{return};native.consume(hint)
            guard let workspace=model.crewdBackend?.workspace else{unavailable=true;return}
            Task{do{_ = try await workspace.notifications.open(hint)}catch{unavailable=true}}
        }
        .alert("此通知的原内容暂不可用",isPresented:$unavailable){Button("好",role:.cancel){}}message:{Text("需当前已验证的原电脑连接、原设备注册与现有访问权限。没有自动连接、恢复或重新执行任务。")}
        .modifier(CrewNotificationOpened(workspace:model.crewdBackend?.workspace))
        .modifier(CrewNotificationLifecycle(workspace:model.crewdBackend?.workspace))
    }
}
/// Observe the actual handshake and journal readiness, not just the root's
/// workspace pointer. Startup never chooses or connects a computer.
private struct CrewNotificationLifecycle:ViewModifier {
    let workspace:CrewdWorkspace?
    func body(content:Content)->some View{
        if let workspace{content.modifier(CrewNotificationLifecycleScope(workspace:workspace,client:workspace.client))}else{content}
    }
}
private struct CrewNotificationLifecycleScope:ViewModifier {
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var client:CrewdClient
    @Environment(\.scenePhase) private var scenePhase
    private var identity:String{(workspace.notifications.currentOrigin.map{($0.scope.grantID ?? "")+"/"+($0.scope.grantRevision ?? "")+"/"+$0.connection.uuidString} ?? "offline")+"/"+String(describing:scenePhase)}
    func body(content:Content)->some View{
        content.task(id:identity){
            guard CrewNativeNotifications.qualification != nil,scenePhase == .active,let origin=workspace.notifications.currentOrigin,workspace.notifications.optedIn else{return}
            let store=workspace.notifications;await store.refresh()
            guard store.currentOrigin==origin,let choices=store.subscriptions(for:origin),!Task.isCancelled else{return}
            await CrewNativeNotifications.shared.enable(store:store,crews:choices.0,kinds:choices.1,ask:false)
        }
    }
}
private struct CrewNotificationOpened:ViewModifier {
    let workspace:CrewdWorkspace?
    func body(content:Content)->some View {
        if let workspace{content.modifier(CrewNotificationOpenedScope(workspace:workspace,store:workspace.notifications))}else{content}
    }
}
private struct CrewNotificationOpenedScope:ViewModifier {
    @EnvironmentObject private var crewStore:CrewStore
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var store:CrewdNotificationStore
    func body(content:Content)->some View {
        content.sheet(item:$store.opened){value in
            VStack(alignment:.leading,spacing:14){
                HStack{Text(value.hint.context.kind.label).font(.headline);Spacer();Button("关闭"){store.opened=nil}}
                Text("当前授权内容 · 只读").font(.caption).foregroundStyle(.secondary)
                ScrollView{
                    Text(value.current["text"].string ?? value.current["goal"].string ?? value.current["question"].string ?? value.current["screen"].string ?? "当前对象状态已核对").frame(maxWidth:.infinity,alignment:.leading).textSelection(.enabled)
                    if let state=value.current["status"].string ?? value.current["state"].string{Text(state).font(.caption).foregroundStyle(.secondary)}
                    DisclosureGroup("原通知来源"){Text((try? JSONEncoder().encode(value.hint)).flatMap{String(data:$0,encoding:.utf8)} ?? "").font(.caption.monospaced()).textSelection(.enabled)}
                }
                Button("在工作区查看"){
                    guard store.canNavigate(value) else{store.opened=nil;return}
                    let context=value.hint.context,source=context.source
                    crewStore.selectCrew(context.crewId);workspace.selectCrew(context.crewId)
                    switch source["type"].string {
                    case "message":if let channel=source["channelId"].string,let id=source["messageId"].string{workspace.selectChannel(channel);workspace.originalSection="消息";crewStore.jumpToMessage(crewId:context.crewId,messageId:id,from:nil)}
                    case "workEvent":workspace.originalSection=context.kind == .askOpened ? "真人Todo":"进展"
                    default:if let id=source["sessionId"].string{workspace.selectSession(id)}
                    }
                    store.opened=nil
                }.buttonStyle(CrewdPrimaryButtonStyle()).disabled(workspace.isRetired || !workspace.client.readyForWork || !store.canNavigate(value))
            }.padding(20).modifier(CrewdAdaptiveFormFrame(width:620,height:480))
        }.onChange(of:workspace.client.connectionEpoch){_,_ in store.opened=nil}
    }
}
struct CrewNotificationSettings:View {
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject private var store:CrewdNotificationStore
    @ObservedObject private var native=CrewNativeNotifications.shared
    @Environment(\.dismiss) private var dismiss
    var embedded=false
    @State private var crews=Set<String>()
    @State private var kinds=Set(CrewdNotificationWire.Kind.allCases)
    @State private var failure:String?
    @State private var confirming=false
    init(workspace:CrewdWorkspace,embedded:Bool=false){self.workspace=workspace;self.embedded=embedded;store=workspace.notifications}
    var body:some View {
        VStack(alignment:.leading,spacing:12){
            HStack{Text("任务与工作通知").font(.headline);Spacer();if !embedded{Button("关闭"){dismiss()}}}
            ScrollView{VStack(alignment:.leading,spacing:14){
                Text("只通知你明确选择的机组。通知不包含任务正文；点击仅查看原授权内容，不会连接电脑、启动或恢复会话。").font(.caption).foregroundStyle(.secondary)
                if !workspace.client.capabilities.contains("notification.nativeApns"){Text("当前后台尚未接入原生通知。").font(.callout)}
                else if let status=store.status{
                    Text(status.enabled ? (status.available ? "后台通知已配置":"后台通知暂不可用"):"后台通知未启用")
                    if let reason=status.reason{Text(reason).font(.caption).foregroundStyle(.secondary)}
                }
                if !store.remoteEligible{Text("接收通知需要本设备独立配对后的已验证连接。本机管理连接不能当作设备接收授权。").font(.caption).foregroundStyle(.secondary)}
                if CrewNativeNotifications.qualification == nil{Text("此版本尚未启用系统通知，未申请系统权限或注册设备。").font(.caption).foregroundStyle(.secondary)}
                if store.installationIDs.count>1{
                    DisclosureGroup("选择原设备注册"){
                        Text("不会按时间或列表顺序猜注册身份。选择后只读核对原注册。").font(.caption).foregroundStyle(.secondary)
                        ForEach(store.installationIDs,id:\.self){id in Button(id){run{try await store.selectInstallation(id)}}.font(.caption.monospaced()).disabled(store.busy)}
                    }
                }
                if let install=store.installation{Text(install.revoked ? "原设备通知注册已撤销":"原设备通知注册已核对 · 第 \(install.generation) 代").font(.caption)}
                VStack(alignment:.leading,spacing:8){
                    Text("接收哪些机组").font(.subheadline)
                    ForEach(Array(workspace.crews.enumerated()),id:\.offset){_,crew in if let id=crew["id"].string{Toggle(crew["title"].string ?? id,isOn:Binding(get:{crews.contains(id)},set:{if $0{crews.insert(id)}else{crews.remove(id)}}))}}
                    Text("提醒类型").font(.subheadline)
                    ForEach(CrewdNotificationWire.Kind.allCases,id:\.rawValue){kind in Toggle(kind.label,isOn:Binding(get:{kinds.contains(kind)},set:{if $0{kinds.insert(kind)}else{kinds.remove(kind)}}))}
                }
                Button(store.optedIn ? "更新所选设备通知":"启用设备通知"){confirming=true}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(CrewNativeNotifications.qualification == nil || !store.remoteEligible || !store.selectionReady || store.status?.available != true || store.busy || native.pending || crews.isEmpty || kinds.isEmpty || !store.unknown.isEmpty)
                if native.pending{Button("取消本次系统注册等待"){native.cancelRegistration()}}
                if store.optedIn || store.installation?.revoked==false{Button("停用并撤销本设备通知",role:.destructive){native.cancelRegistration();run{try await store.revoke()}}.disabled(store.busy)}
                Text("撤销不能收回可能已交给 Apple 的通知；不会撤销独立的电脑配对授权。").font(.caption).foregroundStyle(.secondary)
                if let message=failure ?? store.failure ?? native.notice{Text(message).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
                Button("只读刷新状态"){Task{await store.refresh()}}.disabled(store.busy)
                if !store.unknown.isEmpty{
                    Divider();Text("原请求尚未确认").font(.subheadline)
                    ForEach(store.unknown){record in VStack(alignment:.leading){Text(record.params["requestId"].string ?? "").font(.caption.monospaced()).textSelection(.enabled);Button("只读核对原请求"){run{try await store.lookup(record)}}.disabled(store.busy)}}
                }
                if store.installation?.revoked==false{
                    Divider();Button("读取本设备的通知记录"){run{try await store.readDeliveries()}}.disabled(store.busy)
                    ForEach(store.deliveries){row in HStack{VStack(alignment:.leading){Text(row.context.kind.label);Text(row.label).font(.caption).foregroundStyle(.secondary)};Spacer();Button("查看原内容"){guard let install=store.installation,let hint=try? CrewdNotificationWire.Hint(value:.object(["installationId":.string(install.id),"deliveryId":.string(row.id),"context":(try? JSONDecoder().decode(CrewdValue.self,from:JSONEncoder().encode(row.context))) ?? .null])) else{return};run{_ = try await store.open(hint)}}.disabled(store.busy)}}
                    if store.nextDelivery != nil{Button("更多通知记录"){run{try await store.readDeliveries(more:true)}}.disabled(store.busy)}
                }
            }}
        }.padding(20).modifier(CrewdNotificationSettingsFrame(embedded:embedded))
        .task{await store.refresh();if let origin=store.currentOrigin,let value=store.subscriptions(for:origin){crews=Set(value.0);kinds=Set(value.1)}else if !workspace.crewID.isEmpty{crews=[workspace.crewID]}}
        .confirmationDialog("允许这台设备接收所选机组的系统通知？",isPresented:$confirming,titleVisibility:.visible){Button("继续核对系统通知权限"){Task{await native.enable(store:store,crews:crews.sorted(),kinds:CrewdNotificationWire.Kind.allCases.filter{kinds.contains($0)},ask:true)}};Button("取消",role:.cancel){}}message:{Text("系统授权后，仅向当前已验证电脑提交本设备 token。不会上传到个人云目录，也不会启动任务。")}
    }
    private func run(_ action:@escaping @MainActor ()async throws->Void){Task{do{try await action();failure=nil}catch{failure="通知操作未完成；保留原请求和当前输入，请核对。"}}}
}

private struct CrewdNotificationSettingsFrame:ViewModifier {
    let embedded:Bool
    @ViewBuilder func body(content:Content)->some View{if embedded{content}else{content.modifier(CrewdAdaptiveFormFrame(width:600,height:650))}}
}
