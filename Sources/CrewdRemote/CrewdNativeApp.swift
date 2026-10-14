import SwiftUI

#if os(iOS)
/// New mobile entry never constructs the legacy PSK AppModel/CrewStore.
struct CrewdRemoteMobileApp: App {
    var body: some Scene { WindowGroup { CrewdRemoteMobileRoot() } }
}
#endif

#if os(macOS)
/// Explicit --crewd-ui entry keeps the established isolated data-directory rule.
struct PendingCrewNativeNextApp: App {
    private let directory: URL?
    init() {
        if let explicit = ProcessInfo.processInfo.environment["PENDINGCREW_CREWD_DATA_DIR"], !explicit.isEmpty {
            directory = URL(fileURLWithPath: explicit)
        } else {
            #if PENDINGCREW_NATIVE_CANDIDATE
            if let ref = Bundle.main.object(forInfoDictionaryKey: "PendingCrewSourceRef") as? String,
               ref.count == 40, ref.allSatisfy({ "0123456789abcdef".contains($0) }) {
                directory = FileManager.default.homeDirectoryForCurrentUser
                    .appendingPathComponent("Library/Application Support/PendingCrewNativeCandidates").appendingPathComponent(ref)
            } else { directory = nil }
            #else
            directory = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/PendingCrewNext")
            #endif
        }
    }
    var body: some Scene {
        WindowGroup("PendingCrew · 开发候选") {
            if let directory { CrewdNativeMacRoot(directory: directory).frame(minWidth: 1040, minHeight: 680) }
            else { Text("这个候选版本缺少版本信息，暂时无法打开。") }
        }
    }
}
private struct CrewdNativeMacRoot: View {
    let directory: URL
    @StateObject private var window = CrewdRemoteWindow()
    @State private var admin: CrewdRemoteAdmin?
    @State private var previousAdmins: [CrewdRemoteAdmin] = []
    @State private var managing = false
    @State private var adding = false
    @State private var reviewing = false
    @State private var pending: CrewdRemotePairSummary?
    var body: some View {
        Group {
            if let workspace = window.workspace {
                CrewdWorkspaceView(workspace: workspace, autoConnect: false, machineLabel: window.selected.flatMap { id in window.computers.first(where: { $0.id == id })?.name } ?? "本机")
                    .id(ObjectIdentifier(workspace))
            } else { Text("正在连接本机") }
        }.toolbar {
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
            }
            if !window.retainedWorkspaces.isEmpty || !previousAdmins.isEmpty {
                Button("原连接的输入") { reviewing = true }
            }
            Button("设备连接") { managing = true }.disabled(window.workspace?.dataDirectory == nil || window.workspace?.client.connected != true || admin?.workspace !== window.workspace)
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
            .task { window.refresh(); await chooseLocal() }
            .onChange(of: window.workspace.map {ObjectIdentifier($0)}) {_,_ in synchronizeAdministration()}
            .onDisappear {window.close();synchronizeAdministration()}
    }
    private func chooseLocal() async {
        await window.selectLocal(directory: directory)
        synchronizeAdministration()
    }
    private func synchronizeAdministration() {
        let current=window.workspace
        if admin?.workspace === current {return}
        if let old=admin {old.retire();previousAdmins.append(old)}
        managing=false
        if let current,current.dataDirectory != nil {admin=CrewdRemoteAdmin(workspace:current)}
        else {admin=nil}
    }
}
#endif
