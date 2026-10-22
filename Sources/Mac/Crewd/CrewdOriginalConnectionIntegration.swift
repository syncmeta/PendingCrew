#if os(macOS)
import SwiftUI

struct CrewdOriginalConnectionIntegration: ViewModifier {
    @EnvironmentObject private var model: AppModel
    func body(content: Content) -> some View {
        if model.usesCrewdAuthority, let directory = model.crewdDirectory {
            content.modifier(CrewdOriginalConnectionControls(window: model.crewdWindow, directory: directory))
        } else { content }
    }
}
private struct CrewdOriginalConnectionControls: ViewModifier {
    @ObservedObject var window: CrewdRemoteWindow
    let directory: URL
    @EnvironmentObject private var crewStore: CrewStore
    @State private var admin: CrewdRemoteAdmin?
    @State private var previousAdmins: [CrewdRemoteAdmin] = []
    @State private var managing = false
    @State private var adding = false
    @State private var personalDirectory=false
    @State private var reviewing = false
    @State private var recovering = false
    @State private var trusting = false
    @State private var pending: CrewdRemotePairSummary?
    @State private var removing: CrewdRemotePairSummary?
    func body(content: Content) -> some View {
        content
.toolbar {
            Menu {
            Button("个人主机目录与同步状态"){personalDirectory=true}
            Menu("电脑") {
                Button("本机") { Task { await chooseLocal() } }
                #if PENDINGCREW_KNOWLEDGE_VALIDATION || (PENDINGCREW_LOCAL_MAC && !PENDINGCREW_REMOTE_PRODUCT)
                Text("当前使用本机工作区")
                #else
                Button("管理远程电脑") { Task { await window.refresh() } }.disabled(window.credentialBusy)
                switch window.computerListState {
                case .notLoaded:
                    Text("已存电脑尚未读取")
                case .loaded:
                    if window.computers.isEmpty { Text("没有已存的远程电脑") }
                    ForEach(window.computers) { computer in
                        Button(computer.name + (computer.paired ? "" : " · 等待确认")) {
                            if computer.paired { Task { await window.select(computer.id) } }
                            else { pending = computer }
                        }
                    }
                case .failed(let message):
                    Text("已存电脑读取失败")
                    Text(message)
                }
                Divider()
                Button("添加电脑") {
                    Task { if await window.refresh() { adding = true } }
                }
                Menu("移除已存电脑") {
                    ForEach(window.computers) { computer in
                        Button(computer.name, role: .destructive) { removing = computer }
                    }
                }.disabled(window.computerListState != .loaded || window.computers.isEmpty)
                #endif
            }
            if !window.inputReviewWorkspaces.isEmpty || !previousAdmins.isEmpty {
                Button("原连接的输入") { reviewing = true }
            }
            if let current = window.workspace, current.client.capabilities.contains("command.receipts") {
                Button("原请求核对") { recovering = true }
                if current.dataDirectory != nil { Button("本机重连授权") { trusting = true } }
            }
            #if !PENDINGCREW_KNOWLEDGE_VALIDATION && (!PENDINGCREW_LOCAL_MAC || PENDINGCREW_REMOTE_PRODUCT)
            Button("设备连接") { managing = true }.disabled(window.workspace?.dataDirectory == nil || window.workspace?.client.connected != true || admin?.workspace !== window.workspace)
            #endif
            } label: { Label("连接与核对", systemImage: "network") }
                .labelStyle(.iconOnly).help("连接与核对")
        }.sheet(isPresented:$personalDirectory){VStack(alignment:.leading,spacing:12){HStack{Text("主机目录").font(.headline);Spacer();Button("关闭"){personalDirectory=false}};ScrollView{VStack(alignment:.leading,spacing:20){CrewPersonalHostDirectory();Divider();CrewPersonalDraftArchive()}}}.padding(20).modifier(CrewdAdaptiveFormFrame(width:580,height:440))}
        .sheet(isPresented: $recovering) {
            if let current = window.workspace {
                ScrollView { CrewdPublicRecoveryView(workspace: current, openCrew: { crewStore.selectCrew($0) }).padding(20) }.frame(minWidth: 520, minHeight: 360)
            }
        }.sheet(isPresented: $trusting) {
            if let current = window.workspace { CrewdLocalTrustView(workspace: current).padding(20).frame(minWidth: 500) }
        }.sheet(isPresented: $reviewing) {
            VStack(alignment:.leading,spacing:12) {
                HStack {Text("原连接的输入").font(.title2);Spacer();Button("关闭"){reviewing=false}}
                Text("以下保留原作用域的输入，只供核对，不会向当前电脑重新提交。").foregroundStyle(.secondary)
                ScrollView {
                    ForEach(Array(window.inputReviewWorkspaces.enumerated()),id:\.offset) {_, workspace in CrewdRemoteRetainedInputs(workspace:workspace)}
                    ForEach(Array(previousAdmins.enumerated()),id:\.offset) {_, old in CrewdRemoteAdminReadback(admin:old)}
                }
            }.padding(20).frame(minWidth:500,minHeight:360)
        }.sheet(isPresented: $managing) { if let admin { CrewdRemoteManagementView(admin: admin) } }
            .sheet(isPresented: $adding, onDismiss: { Task { await window.refresh() } }) {
                CrewdRemotePairingView(pairing: window.pairing, pending: nil) { id in
                    adding = false; Task { guard await window.refresh() else { return }; await window.select(id) }
                }
            }
            .sheet(item: $pending, onDismiss: { Task { await window.refresh() } }) { pair in
                CrewdRemotePairingView(pairing: window.pairing, pending: pair.id) { id in
                    pending = nil; Task { guard await window.refresh() else { return }; await window.select(id) }
                }
            }

            .alert("从本设备移除已存电脑", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), presenting: removing) { computer in
                Button("移除配对凭据", role: .destructive) { Task { await window.forget(computer) }; removing = nil }
                Button("取消", role: .cancel) { removing = nil }
            } message: { computer in
                Text(computer.name + " · " + computer.endpoint.host + ":" + String(computer.endpoint.port) + "\n删除本设备保存的配对凭据。服务器授权和团队仍保留。")
            }
            .onAppear { synchronizeAdministration() }
            .onChange(of: window.workspace.map { ObjectIdentifier($0) }) { _, _ in synchronizeAdministration() }
    }
    private func chooseLocal() async {
        await window.selectLocal(directory: directory)
        synchronizeAdministration()
    }
    private func synchronizeAdministration() {
        #if !PENDINGCREW_LOCAL_MAC || PENDINGCREW_REMOTE_PRODUCT
        let current = window.workspace
        if admin?.workspace === current { return }
        if let old = admin { old.retire(); previousAdmins.append(old) }
        managing = false
        if let current, current.dataDirectory != nil { admin = CrewdRemoteAdmin(workspace: current) }
        else { admin = nil }
        #endif
    }
}
struct CrewdOriginalConnectionStatus: View {
    @ObservedObject var workspace: CrewdWorkspace
    var compact=false
    @State private var checkingOriginal=false
    @State private var selectingRecovery = false
    @State private var askRecovery = false
    @State private var shownPrompt = ""
    @State private var selected = Set<String>()
    @State private var showingDetails = false
    var body:some View {
        VStack(alignment:.leading,spacing:6){
            if compact {
                Button{showingDetails=true}label:{
                    Label(workspace.error == nil ? "连接状态":"当前操作未完成",systemImage:workspace.error == nil ? "network":"exclamationmark.triangle")
                }.labelStyle(.iconOnly).foregroundStyle(workspace.error == nil ? Theme.Palette.inkMuted:Theme.Palette.amber).help("连接状态与原请求核对").accessibilityLabel(workspace.error == nil ? "查看连接状态":"查看当前未完成操作与原请求")
            }else {
                HStack(spacing:8){
                    Circle().fill(workspace.client.readyForWork ? Theme.Palette.accent:Theme.Palette.inkMuted).frame(width:6,height:6)
                    Text(workspace.client.readyForWork ? "已连接":workspace.status).lineLimit(1).font(CrewdType.caption.weight(.medium))
                    Spacer(minLength:2)
                    Button{showingDetails=true}label:{Image(systemName:"info.circle")}.buttonStyle(.plain).help("连接详情")
                    managementMenu
                }
                Text(workspace.dataDirectory == nil ? "远程工作区":workspace.managementRole == "owner" ? "本机 · 管理窗口":"本机 · 观察窗口").font(CrewdType.caption).foregroundStyle(.secondary)
                if workspace.error != nil {
                    Button{showingDetails=true}label:{Label("当前操作未完成 · 核对",systemImage:"exclamationmark.triangle")}
                        .buttonStyle(.plain).font(CrewdType.caption).foregroundStyle(.orange)
                }
            }
        }.controlSize(.small).padding(.horizontal,compact ? 0:12).padding(.vertical,compact ? 0:10)
        .popover(isPresented:$showingDetails){
            ScrollView {
                VStack(alignment:.leading,spacing:14){
                    CrewdPanelHeader(title:"连接状态",detail:workspace.status,symbol:"network")
                    if let error=workspace.error {
                        DisclosureGroup("当前操作未完成 · 原始说明"){
                            Text(error).font(CrewdType.label).textSelection(.enabled).fixedSize(horizontal:false,vertical:true).padding(.top,6)
                        }.foregroundStyle(.orange)
                        Text("结果未知时只核对原请求，不会自动重新提交。").font(CrewdType.caption).foregroundStyle(.secondary)
                    }
                    if workspace.client.capabilities.contains("command.receipts") {
                        Button("核对原请求"){showingDetails=false;checkingOriginal=true}
                    }
                    if workspace.dataDirectory != nil,!(CrewdCodexConfiguration.program().map(CrewdCodexConfiguration.executable) ?? false){
                        Text("下次后台启动尚未配置可执行的 Codex。资料仍可使用；可在设置中准备后停工重开。").font(CrewdType.caption).foregroundStyle(.secondary)
                    }
                    managementMenu
                }.padding(16)
            }.frame(width:360,height:workspace.error == nil ? 180:360)
        }
        .sheet(isPresented:$checkingOriginal){
            VStack(spacing:0){
                HStack{Text("核对原请求").font(.headline);Spacer();Button("关闭"){checkingOriginal=false}}.padding(16)
                ScrollView{CrewdPublicRecoveryView(workspace:workspace).padding(16)}
            }.frame(minWidth:520,minHeight:360)
        }
        .task {
            if !compact,workspace.previousExit == "unclean", workspace.restartPrompt != .null, shownPrompt.isEmpty {
                shownPrompt = workspace.restartPrompt["id"].pretty; askRecovery = true
            }
        }
        .onChange(of: workspace.restartPrompt) { _, prompt in
            let id = prompt["id"].pretty
            guard !compact,prompt != .null, !id.isEmpty, shownPrompt != id else { return }
            if workspace.previousExit == "unclean" { shownPrompt = id; askRecovery = true }
        }
        .onChange(of: workspace.previousExit) { _, value in
            if !compact,value == "unclean", workspace.restartPrompt != .null, shownPrompt.isEmpty {
                shownPrompt = workspace.restartPrompt["id"].pretty; askRecovery = true
            }
        }
        .alert("上次异常退出，要选择恢复哪些会话吗？", isPresented: $askRecovery) {
            Button("不恢复", role: .cancel) { declineRecovery() }
            Button("选择会话…") { selectingRecovery = true }
        } message: { Text("未选择或取消都不会恢复，也不会重放未确认的工作。") }
        .confirmationDialog("接管当前本机后台的管理权？不会因此恢复会话。", isPresented: $showingClaim) {
            Button("接管管理") { Task { await workspace.claimManagement() } }
            Button("取消", role: .cancel) {}
        }
        .sheet(isPresented: $selectingRecovery) {
            VStack(alignment: .leading, spacing: 12) {
                Text("手动恢复会话").font(.headline)
                Text("只恢复选中的会话。历史结果未知时仍需核对，不会自动重发旧操作。").font(.caption)
                ScrollView {
                    ForEach(Array(workspace.sessions.enumerated()), id: \.offset) { _, session in
                        if let id = session["id"].string {
                            Toggle(session["displayName"].string ?? session["title"].string ?? id,
                                   isOn: Binding(get: { selected.contains(id) }, set: { if $0 { selected.insert(id) } else { selected.remove(id) } }))
                        }
                    }
                }
                HStack {
                    Button("取消", role: .cancel) { selected.removeAll(); selectingRecovery = false; declineRecovery() }
                    Spacer()
                    Button("恢复所选会话") {
                        let ids = selected.sorted(); selected.removeAll(); selectingRecovery = false
                        Task { for id in ids { _ = await workspace.perform("session.resume", .object(["sessionId": .string(id)])) } }
                    }.disabled(selected.isEmpty || workspace.busy)
                }
            }.padding(20).frame(minWidth: 500, minHeight: 320)
        }
    }
    private var managementMenu:some View {
        Menu {
            if workspace.dataDirectory != nil {
                Text(workspace.managementRole == "owner" ? "当前为管理窗口":"当前为观察窗口")
                if workspace.managementRole != "owner"{Button("接管管理…"){Task{await workspace.readManagementStatus();showingClaim=true}}}
                Divider()
            }
            Button("手动恢复会话…"){selectingRecovery=true}.disabled(!workspace.client.connected)
        }label:{Label("后台管理",systemImage:"ellipsis.circle")}
            .menuStyle(.borderlessButton).labelStyle(.iconOnly).fixedSize().help("后台管理")
    }
    @State private var showingClaim = false
    private func declineRecovery() {
        guard workspace.restartPrompt != .null else { return }
        let id = workspace.restartPrompt["id"]
        Task { _ = await workspace.perform("restart.answer", .object(["id": id, "answer": .string("later")])) }
    }
}
#endif
