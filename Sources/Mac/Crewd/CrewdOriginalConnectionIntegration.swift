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
    @State private var reviewing = false
    @State private var recovering = false
    @State private var trusting = false
    @State private var pending: CrewdRemotePairSummary?
    @State private var removing: CrewdRemotePairSummary?
    func body(content: Content) -> some View {
        content
.toolbar {
            Menu("电脑") {
                Button("本机") { Task { await chooseLocal() } }
                #if PENDINGCREW_KNOWLEDGE_VALIDATION || PENDINGCREW_LOCAL_MAC
                Text("当前使用本机工作区")
                #else
                ForEach(window.computers) { computer in
                    Button(computer.name + (computer.paired ? "" : " · 等待确认")) {
                        if computer.paired { Task { await window.select(computer.id) } }
                        else { pending = computer }
                    }
                }
                Divider()
                Button("添加电脑") { adding = true }
                Menu("移除已存电脑") {
                    ForEach(window.computers) { computer in
                        Button(computer.name, role: .destructive) { removing = computer }
                    }
                }.disabled(window.computers.isEmpty)
                #endif
            }
            if !window.retainedWorkspaces.isEmpty || !previousAdmins.isEmpty {
                Button("原连接的输入") { reviewing = true }
            }
            if let current = window.workspace, current.client.capabilities.contains("command.receipts") {
                Button("原请求核对") { recovering = true }
                if current.dataDirectory != nil { Button("本机重连授权") { trusting = true } }
            }
            #if !PENDINGCREW_KNOWLEDGE_VALIDATION && !PENDINGCREW_LOCAL_MAC
            Button("设备连接") { managing = true }.disabled(window.workspace?.dataDirectory == nil || window.workspace?.client.connected != true || admin?.workspace !== window.workspace)
            #endif
        }.sheet(isPresented: $recovering) {
            if let current = window.workspace {
                ScrollView { CrewdPublicRecoveryView(workspace: current, openCrew: { crewStore.selectCrew($0) }).padding(20) }.frame(minWidth: 520, minHeight: 360)
            }
        }.sheet(isPresented: $trusting) {
            if let current = window.workspace { CrewdLocalTrustView(workspace: current).padding(20).frame(minWidth: 500) }
        }.sheet(isPresented: $reviewing) {
            VStack(alignment:.leading,spacing:12) {
                HStack {Text("原连接的输入").font(.title2);Spacer();Button("关闭"){reviewing=false}}
                Text("原连接已停止。以下只供核对，不会向当前电脑重新提交。").foregroundStyle(.secondary)
                ScrollView {
                    ForEach(Array(window.retainedWorkspaces.enumerated()),id:\.offset) {_, workspace in CrewdRemoteRetainedInputs(workspace:workspace)}
                    ForEach(Array(previousAdmins.enumerated()),id:\.offset) {_, old in CrewdRemoteAdminReadback(admin:old)}
                }
            }.padding(20).frame(minWidth:500,minHeight:360)
        }.sheet(isPresented: $managing) { if let admin { CrewdRemoteManagementView(admin: admin) } }
            .sheet(isPresented: $adding, onDismiss: { window.refresh() }) {
                CrewdRemotePairingView(pairing: window.pairing, pending: nil) { id in
                    adding = false; window.refresh(); Task { await window.select(id) }
                }
            }
            .sheet(item: $pending, onDismiss: { window.refresh() }) { pair in
                CrewdRemotePairingView(pairing: window.pairing, pending: pair.id) { id in
                    pending = nil; window.refresh(); Task { await window.select(id) }
                }
            }

            .alert("从本设备移除已存电脑", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), presenting: removing) { computer in
                Button("移除配对凭据", role: .destructive) { _ = window.forget(computer); removing = nil }
                Button("取消", role: .cancel) { removing = nil }
            } message: { computer in
                Text(computer.name + " · " + computer.endpoint.host + ":" + String(computer.endpoint.port) + "\n删除本设备保存的配对凭据。服务器授权和团队仍保留。")
            }
            .onAppear {
                #if !PENDINGCREW_LOCAL_MAC
                window.refresh()
                #endif
                synchronizeAdministration()
            }
            .onChange(of: window.workspace.map { ObjectIdentifier($0) }) { _, _ in synchronizeAdministration() }
    }
    private func chooseLocal() async {
        await window.selectLocal(directory: directory)
        synchronizeAdministration()
    }
    private func synchronizeAdministration() {
        #if !PENDINGCREW_LOCAL_MAC
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
    @State private var selectingRecovery = false
    @State private var askRecovery = false
    @State private var shownPrompt = ""
    @State private var selected = Set<String>()
    var body: some View {
        HStack {
            HStack(spacing: 5) {
                Circle().fill(workspace.client.readyForWork ? Theme.Palette.accent : Theme.Palette.inkMuted).frame(width: 5, height: 5)
                Text(workspace.status).font(.system(size: 11)).foregroundStyle(Theme.Palette.inkMuted)
            }
            if workspace.dataDirectory != nil, !(CrewdCodexConfiguration.program().map(CrewdCodexConfiguration.executable) ?? false) {
                Text("下次后台启动未配置可执行的 Codex；资料仍可使用，请在设置中准备后停工重开。").font(.caption).foregroundStyle(.secondary)
            }
            if let error = workspace.error {
                DisclosureGroup {
                    Text(error).font(.caption).textSelection(.enabled)
                } label: {
                    Label("当前操作需要核对", systemImage: "exclamationmark.triangle")
                        .font(.system(size: 11)).foregroundStyle(.orange)
                }.fixedSize(horizontal: false, vertical: true).frame(maxWidth: 380, alignment: .leading)
            }
            Spacer()
            if workspace.dataDirectory != nil {
                Text(workspace.managementRole == "owner" ? "当前管理窗口" : "观察窗口").font(.caption)
                if workspace.managementRole != "owner" {
                    Button("接管管理…") { Task { await workspace.readManagementStatus(); showingClaim = true } }
                }
            }
            Button("恢复会话…") { selectingRecovery = true }
                .disabled(!workspace.client.connected)
        }.controlSize(.small).padding(.horizontal, 12).padding(.vertical, 5)
            .background(Theme.Palette.surfaceMuted.opacity(0.35))
        .task {
            if workspace.previousExit == "unclean", workspace.restartPrompt != .null, shownPrompt.isEmpty {
                shownPrompt = workspace.restartPrompt["id"].pretty; askRecovery = true
            }
        }
        .onChange(of: workspace.restartPrompt) { _, prompt in
            let id = prompt["id"].pretty
            guard prompt != .null, !id.isEmpty, shownPrompt != id else { return }
            if workspace.previousExit == "unclean" { shownPrompt = id; askRecovery = true }
        }
        .onChange(of: workspace.previousExit) { _, value in
            if value == "unclean", workspace.restartPrompt != .null, shownPrompt.isEmpty {
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
    @State private var showingClaim = false
    private func declineRecovery() {
        guard workspace.restartPrompt != .null else { return }
        let id = workspace.restartPrompt["id"]
        Task { _ = await workspace.perform("restart.answer", .object(["id": id, "answer": .string("later")])) }
    }
}
#endif
