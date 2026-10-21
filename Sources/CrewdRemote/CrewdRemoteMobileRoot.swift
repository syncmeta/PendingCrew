import SwiftUI
#if os(iOS)

/// No legacy PSK backend enters the new mobile workbench.
struct CrewdRemoteMobileRoot: View {
    @StateObject private var window = CrewdRemoteWindow()
    @State private var pairingSheet = false
    @State private var pendingPair: CrewdRemotePairSummary?
    @State private var computersSheet = false
    @Environment(\.scenePhase) private var phase
    var body: some View {
        Group {
            if let workspace = window.workspace {
                CrewdMobileWorkspaceView(workspace: workspace).id(ObjectIdentifier(workspace))
                    .overlay(alignment: .topTrailing) {
                        Button { computersSheet = true } label: { Label("电脑", systemImage: "desktopcomputer") }
                            .buttonStyle(.bordered).padding(8).accessibilityLabel("切换连接电脑")
                    }
            } else {
                NavigationStack {
                    computerList.navigationTitle("连接你的电脑")
                        .toolbar { Button("添加电脑", systemImage: "plus") { pairingSheet = true } }
                }
            }
        }
        .task { await window.refresh() }
        .onDisappear {window.close()}
        .onChange(of: phase) { _, value in if value == .active { Task { await window.refresh() } } }
        .sheet(isPresented: $computersSheet) {
            NavigationStack {
                computerList.navigationTitle("电脑")
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) { Button("返回") { computersSheet = false } }
                        ToolbarItem(placement: .primaryAction) { Button("添加") { computersSheet = false; pairingSheet = true } }
                    }
            }
        }
        .sheet(isPresented: $pairingSheet, onDismiss: { Task { await window.refresh() } }) {
            CrewdRemotePairingView(pairing: window.pairing, pending: nil) { id in
                pairingSheet = false; Task { guard await window.refresh() else { return }; await window.select(id) }
            }
        }
        .sheet(item: $pendingPair, onDismiss: { Task { await window.refresh() } }) { pending in
            CrewdRemotePairingView(pairing: window.pairing, pending: pending.id) { id in
                pendingPair = nil; Task { guard await window.refresh() else { return }; await window.select(id) }
            }
        }
    }
    private var computerList: some View {
        List {
            if let error = window.error { Text(error).foregroundStyle(.red) }
            if window.computers.isEmpty {
                Text("在新版 PendingCrew 的电脑端生成邀请，再在这里添加。旧版连接需要重新配对。")
                    .foregroundStyle(.secondary)
            }
            ForEach(window.computers) { computer in
                Button {
                    if computer.paired { computersSheet = false; Task { await window.select(computer.id) } }
                    else { computersSheet = false; pendingPair = computer }
                } label: {
                    HStack {
                        Image(systemName: "desktopcomputer")
                        VStack(alignment: .leading) {
                            Text(computer.name)
                            Text(computer.paired ? "\(computer.endpoint.host):\(computer.endpoint.port)" : "等待电脑确认")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        if computer.id == window.selected { Image(systemName: "checkmark") }
                    }
                }.contextMenu { Button("仅从本设备移除", role: .destructive) { Task { await window.forget(computer) } } }
            }
            if !window.retainedWorkspaces.isEmpty {
                Section("切换前的未确认操作") {
                    Text("旧连接已停止。原草稿保留在本窗口，切换不会重新提交。")
                        .font(.caption).foregroundStyle(.secondary)
                    ForEach(Array(window.retainedWorkspaces.enumerated()), id: \.offset) { _, old in
                        CrewdRemoteRetainedInputs(workspace: old)
                    }
                }
            }
        }
    }
}


#endif

struct CrewdRemotePairingView: View {
    let pairing: CrewdRemotePairing
    let pending: UUID?
    let completed: (UUID) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var inviteText = ""
    @State private var receiptText = ""
    @State private var deviceLabel = "我的设备"
    @State private var preview: CrewdRemoteInvitation?
    @State private var envelope: CrewdRemoteEnvelope?
    @State private var pendingID: UUID?
    @State private var responseText = ""
    @StateObject private var operation = CrewdRemoteOperation()
    private var busy: Bool { operation.busy }
    @State private var error: String?
    var body: some View {
        NavigationStack {
            Form {
                if let error { Text(error).foregroundStyle(.red) }
                if let id = pendingID {
                    Section("把回复码交给电脑") {
                        Text("请在电脑端核对设备并批准连接，再复制电脑给出的确认码。")
                        ShareLink("分享回复码", item: responseText)
                        Text(responseText).font(.caption.monospaced()).textSelection(.enabled).lineLimit(4)
                    }
                    Section("电脑确认码") {
                        TextEditor(text: $receiptText).frame(minHeight: 100).crewdRemoteCodeInput().disabled(busy)
                        Button("完成配对") { run { try await pairing.confirmReceipt(CrewdRemoteText.decode(receiptText), id: id) } apply: { _ in completed(id) } }
                            .disabled(busy || receiptText.isEmpty)
                    }
                } else {
                    Section("电脑邀请连接码") {
                        TextEditor(text: $inviteText).frame(minHeight: 110).crewdRemoteCodeInput().disabled(busy)
                            .onChange(of: inviteText) { _, _ in preview = nil; envelope = nil; error = nil }
                        Button("核对电脑") { run {
                            let value = try CrewdRemoteText.decode(inviteText)
                            return (try await pairing.invitation(value), value)
                        } apply: { value in preview = value.0; envelope = value.1 } }.disabled(busy || inviteText.isEmpty)
                    }
                    if let preview, let envelope {
                        Section("确认这台电脑") {
                            Text(preview.computerLabel)
                            Text("\(preview.endpoint.host):\(preview.endpoint.port)").foregroundStyle(.secondary)
                            Text("电脑身份：\(preview.server.spkiSha256)").font(.caption.monospaced()).textSelection(.enabled)
                            TextField("设备名称", text: $deviceLabel).disabled(busy)
                            Button("确认是这台电脑，生成回复") { run {
                                try await pairing.respondWithText(envelope, deviceLabel: deviceLabel)
                            } apply: { result in pendingID = result.id; responseText = result.text } }.disabled(busy || deviceLabel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        }
                    }
                }
            }.navigationTitle("添加电脑")
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("稍后继续") { operation.invalidate(); dismiss() } } }
                .task { if let pending { run { try await pairing.responseText(pending) } apply: { text in pendingID = pending; responseText = text } } }
                .onDisappear { operation.invalidate() }
        }
    }
    @MainActor private func run<Value: Sendable>(_ action: @escaping @MainActor () async throws -> Value, apply: @escaping @MainActor (Value) -> Void) {
        operation.run(action, apply: apply, failed: { error in self.error = error.localizedDescription })
    }

}

private extension View {
    @ViewBuilder func crewdRemoteCodeInput() -> some View {
        #if os(iOS)
        self.textInputAutocapitalization(.never).autocorrectionDisabled()
        #else
        self
        #endif
    }
}
