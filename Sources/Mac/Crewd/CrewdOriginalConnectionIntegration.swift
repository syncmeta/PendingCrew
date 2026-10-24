#if os(macOS)
import SwiftUI
#if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
import Inject
#endif

struct CrewdOriginalConnectionIntegration: ViewModifier {
    @EnvironmentObject private var model: AppModel
    func body(content: Content) -> some View {
        if model.usesCrewdAuthority, let directory = model.crewdDirectory {
            content.modifier(CrewdOriginalConnectionControls(window: model.crewdWindow, directory: directory))
        } else { content }
    }
}
private struct CrewdOriginalConnectionControls: ViewModifier {
    @EnvironmentObject private var model: AppModel
    @ObservedObject var window: CrewdRemoteWindow
    let directory: URL
    @EnvironmentObject private var crewStore: CrewStore
    @State private var admin: CrewdRemoteAdmin?
    @State private var previousAdmins: [CrewdRemoteAdmin] = []
    @State private var managing = false
    @State private var adding = false
    @State private var personalDirectory=false
    @State private var statusDetails=false
    @State private var reviewing = false
    @State private var localCodexInputs=false
    @State private var recovering = false
    @State private var trusting = false
    @State private var pending: CrewdRemotePairSummary?
    @State private var removing: CrewdRemotePairSummary?
    @State private var settingsNext:(()->Void)?
    func body(content: Content) -> some View {
        content
.sheet(isPresented:$window.showingConnectionSettings,onDismiss:{let action=settingsNext;settingsNext=nil;action?()}){
            VStack(alignment:.leading,spacing:14){
                HStack{Text("电脑与连接设置").font(.headline);Spacer();Button("完成"){window.showingConnectionSettings=false}}
                Form { settingsItems }.formStyle(.grouped)
            }.padding(16).frame(width:560,height:560)
        }.sheet(isPresented:$localCodexInputs){CrewdCodexLocalInputs(directory:directory)}
        .sheet(isPresented:$statusDetails){
            if let current=window.workspace {
                VStack(spacing:0){
                    HStack{Text("当前电脑").font(.headline);Spacer();Button("关闭"){statusDetails=false}}.padding(16)
                    CrewdOriginalConnectionStatus(workspace:current,compact:true,detailOnly:true)
                }.frame(minWidth:360)
            }
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
                    adding = false; Task { await model.connectCrewdAuthority(computerID:id);if model.crewdBackend?.workspace.authorityRecordID==id{await crewStore.refreshList()} }
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
    @ViewBuilder private var settingsItems:some View {
            if model.connectingAuthority {ProgressView("正在连接电脑…")}
            if let failure=model.backendConfigurationError {Text(failure).foregroundStyle(.orange).textSelection(.enabled)}
            if window.workspace == nil {
                Text("后台尚未连接").foregroundStyle(.secondary)
                Button("重试连接"){Task{await model.connectCrewdAuthority()}}.disabled(model.connectingAuthority)
            }


            if window.workspace != nil { Button("当前电脑状态与原请求",systemImage:"info.circle"){fromSettings{statusDetails=true}} }
            Button("个人主机目录与同步状态"){fromSettings{personalDirectory=true}}
            Menu("电脑") {
                Button("本机") { Task { await chooseLocal() } }
                #if PENDINGCREW_KNOWLEDGE_VALIDATION || (PENDINGCREW_LOCAL_MAC && !PENDINGCREW_REMOTE_PRODUCT)
                Text("当前使用本机工作区")
                #else
                Button("读取已存电脑") { Task { await window.refresh() } }.disabled(window.credentialBusy)
                switch window.computerListState {
                case .notLoaded:
                    Text("已存电脑尚未读取")
                case .loaded:
                    if window.computers.isEmpty { Text("没有已存的远程电脑") }
                    ForEach(window.computers) { computer in
                        Button(computer.name + (computer.paired ? "" : " · 等待确认")) {
                            if computer.paired { Task { await window.select(computer.id) } }
                            else { fromSettings{pending = computer} }
                        }
                    }
                case .failed(let message):
                    Text("已存电脑读取失败")
                    Text(message)
                }
                Divider()
                Button("添加电脑") {
                    Task { if await window.refresh() { fromSettings{adding = true} } }
                }
                Menu("移除已存电脑") {
                    ForEach(window.computers) { computer in
                        Button(computer.name, role: .destructive) { removing = computer }
                    }
                }.disabled(window.computerListState != .loaded || window.computers.isEmpty)
                #endif
            }
            Button("本机Codex原输入"){fromSettings{localCodexInputs=true}}
            if !window.inputReviewWorkspaces.isEmpty || !previousAdmins.isEmpty {
                Button("原连接的输入") { fromSettings{reviewing = true} }
            }
            if let current = window.workspace, current.client.capabilities.contains("command.receipts") {
                Button("原请求核对") { fromSettings{recovering = true} }
                if current.dataDirectory != nil { Button("本机重连授权") { fromSettings{trusting = true} } }
            }
            #if !PENDINGCREW_KNOWLEDGE_VALIDATION && (!PENDINGCREW_LOCAL_MAC || PENDINGCREW_REMOTE_PRODUCT)
            Button("设备连接") { fromSettings{managing = true} }.disabled(window.workspace?.dataDirectory == nil || window.workspace?.client.connected != true || admin?.workspace !== window.workspace)
            #endif
    }
    private func fromSettings(_ action:@escaping ()->Void){
        settingsNext=action
        window.showingConnectionSettings=false
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
    var detailOnly=false
    var machineName:String?=nil
    @State private var checkingOriginal=false
    @State private var selectingRecovery = false
    @State private var askRecovery = false
    @State private var shownPrompt = ""
    @State private var selected = Set<String>()
    @State private var showingDetails = false
    var body:some View {
        Group {
        if detailOnly { connectionDetails } else {
        VStack(alignment:.leading,spacing:6){
            if let machineName {
                Button{showingDetails=true}label:{
                    CrewdHostConnectionLabel(client:workspace.client,name:machineName,connecting:workspace.status == "正在连接",hasUnfinishedOperation:workspace.error != nil || !workspace.durableUnknown.isEmpty)
                        .frame(maxWidth:.infinity,alignment:.leading).contentShape(Rectangle())
                }.buttonStyle(.plain).help("电脑连接详情与原请求核对")
            }else if compact {
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
        }
        }
        }.controlSize(.small).padding(.horizontal,compact ? 0:12).padding(.vertical,compact ? 0:10)
        .popover(isPresented:$showingDetails){connectionDetails}
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
    private var connectionDetails:some View {
            ScrollView {
                VStack(alignment:.leading,spacing:14){
                    CrewdPanelHeader(title:"连接状态",detail:workspace.status,symbol:"network")
                    if let error=workspace.error {
                        DisclosureGroup("当前操作未完成 · 原始说明"){
                            Text(error).font(CrewdType.label).textSelection(.enabled).fixedSize(horizontal:false,vertical:true).padding(.top,6)
                        }.foregroundStyle(.orange)
                        Text("结果未知时只核对原请求，不会自动重新提交。").font(CrewdType.caption).foregroundStyle(.secondary)
                    }
                    if workspace.error == nil,!workspace.durableUnknown.isEmpty {
                        Label("有原请求结果待核对",systemImage:"exclamationmark.triangle").font(CrewdType.caption).foregroundStyle(.orange)
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
/// One authority selector shared by the sidebar footer and new-chat controls.
/// Emoji use configured scope/pair identities, never process IDs or hardware data.
struct CrewdMachinePicker: View {
    #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
    @ObserveInjection private var injection
    #endif
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var crewStore: CrewStore
    @ObservedObject var window: CrewdRemoteWindow

    private var localSeed: String {
        let local = ([window.workspace].compactMap { $0 } + window.retainedWorkspaces)
            .first { $0.dataDirectory != nil }
        return "computer/" + (local?.authorityRecordID.uuidString.lowercased() ?? "local-unconnected")
    }
    private var localSelected: Bool { window.workspace?.dataDirectory != nil }
    private var currentName: String {
        if localSelected { return "本机" }
        if let id = window.selected { return window.computers.first { $0.id == id }?.name ?? "当前连接电脑" }
        return "尚未连接"
    }
    private var currentSeed: String { localSelected || window.selected == nil ? localSeed : "computer/" + window.selected!.uuidString.lowercased() }
    // Preserve the existing SwiftUI avatar as an original-colour native button image.
    private var machineAvatar: NSImage {
        let renderer = ImageRenderer(content: BotAvatar(seed: currentSeed, size: 26))
        renderer.scale = NSScreen.main?.backingScaleFactor ?? 2
        return renderer.nsImage ?? NSImage(size: NSSize(width: 26, height: 26))
    }
    var body: some View {
        CrewdMachineMenuButton(name: currentName, avatar: machineAvatar,
            ready: window.workspace?.client.readyForWork == true && !model.connectingAuthority,
            enabled: !model.connectingAuthority && !crewStore.pendingCrewSending,
            choices: [.init(name: "本机", enabled: true, action: { Task { await model.connectCrewdAuthority() } })]
                + window.computers.filter(\.paired).map { computer in
                    .init(name: computer.name, enabled: !window.credentialBusy,
                          action: { Task { await model.connectCrewdAuthority(computerID: computer.id) } })
                })
        .frame(maxWidth: .infinity, minHeight: 30, maxHeight: 30, alignment: .leading)
        .task { if window.computerListState == .notLoaded { await window.refresh() } }
        #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
        .enableInjection()
        #endif
    }
}

/// The visible native button owns the entire row and opens the native names-only
/// menu. AppKit's borderless Menu style otherwise centres and compresses its label.
private struct CrewdMachineMenuButton: NSViewRepresentable {
    struct Choice {
        let name: String
        let enabled: Bool
        let action: () -> Void
    }
    let name: String
    let avatar: NSImage
    let ready: Bool
    let enabled: Bool
    let choices: [Choice]

    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeNSView(context: Context) -> NSButton {
        let button = NSButton()
        button.setButtonType(.momentaryPushIn)
        button.isBordered = false
        button.imagePosition = .imageLeft
        button.imageHugsTitle = false
        button.imageScaling = .scaleNone
        button.alignment = .left
        button.target = context.coordinator
        button.action = #selector(Coordinator.openMenu(_:))
        button.setContentHuggingPriority(.defaultLow, for: .horizontal)
        button.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return button
    }
    func updateNSView(_ button: NSButton, context: Context) {
        context.coordinator.choices = choices
        button.image = avatar
        button.isEnabled = enabled
        let title = NSMutableAttributedString(string: "   " + name,
            attributes: [.font: NSFont.systemFont(ofSize: 13, weight: .semibold), .foregroundColor: NSColor.labelColor])
        title.append(NSAttributedString(string: "    "))
        if let chevron = NSImage(systemSymbolName: "chevron.up", accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: 16, weight: .medium)) {
            let attachment = NSTextAttachment()
            attachment.attachmentCell = NSTextAttachmentCell(imageCell: chevron)
            title.append(NSAttributedString(attachment: attachment))
        }
        title.append(NSAttributedString(string: "  "))
        title.append(NSAttributedString(string: "●",
            attributes: [.font: NSFont.systemFont(ofSize: 10), .foregroundColor: ready ? NSColor.systemGreen : NSColor.systemRed]))
        button.attributedTitle = title
        button.setAccessibilityLabel("机器：" + name)
        button.setAccessibilityValue(ready ? "已连接" : "未连接")
        button.toolTip = name + (ready ? " · 已连接" : " · 未连接")
    }
    final class Coordinator: NSObject {
        var choices: [Choice] = []
        private var menuChoices: [Choice] = []
        @objc func openMenu(_ button: NSButton) {
            let menu = NSMenu()
            menu.autoenablesItems = false
            menuChoices = choices
            for (index, choice) in menuChoices.enumerated() {
                let item = NSMenuItem(title: choice.name, action: #selector(selectMachine(_:)), keyEquivalent: "")
                item.target = self; item.tag = index; item.isEnabled = choice.enabled
                menu.addItem(item)
            }
            menu.popUp(positioning: nil, at: NSPoint(x: 0, y: button.bounds.maxY + 4), in: button)
        }
        @objc func selectMachine(_ item: NSMenuItem) {
            guard menuChoices.indices.contains(item.tag), menuChoices[item.tag].enabled else { return }
            menuChoices[item.tag].action()
        }
    }
}

#endif
