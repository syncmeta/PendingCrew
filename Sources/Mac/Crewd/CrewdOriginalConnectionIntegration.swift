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
            }
            if !window.retainedWorkspaces.isEmpty || !previousAdmins.isEmpty {
                Button("原连接的输入") { reviewing = true }
            }
            if let current = window.workspace, current.client.capabilities.contains("command.receipts") {
                Button("原请求核对") { recovering = true }
                if current.dataDirectory != nil { Button("本机重连授权") { trusting = true } }
            }
            Button("设备连接") { managing = true }.disabled(window.workspace?.dataDirectory == nil || window.workspace?.client.connected != true || admin?.workspace !== window.workspace)
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
            .onAppear { window.refresh(); synchronizeAdministration() }
            .onChange(of: window.workspace.map { ObjectIdentifier($0) }) { _, _ in synchronizeAdministration() }
    }
    private func chooseLocal() async {
        await window.selectLocal(directory: directory)
        synchronizeAdministration()
    }
    private func synchronizeAdministration() {
        let current = window.workspace
        if admin?.workspace === current { return }
        if let old = admin { old.retire(); previousAdmins.append(old) }
        managing = false
        if let current, current.dataDirectory != nil { admin = CrewdRemoteAdmin(workspace: current) }
        else { admin = nil }
    }
}
struct CrewdOriginalConnectionStatus: View {
    @ObservedObject var workspace: CrewdWorkspace
    var body: some View {
        HStack {
            Text(workspace.status).font(.caption)
            if let error = workspace.error { Text(error).font(.caption).foregroundStyle(.orange).textSelection(.enabled) }
            Spacer()
        }.padding(8)
    }
}
#endif
