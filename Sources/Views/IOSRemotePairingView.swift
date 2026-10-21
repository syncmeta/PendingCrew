#if os(iOS)
import SwiftUI
import UIKit

/// The original pairing form uses the signed crewd invitation/response/receipt
/// exchange. Trust is established only after the computer's final confirmation.
struct IOSRemotePairingView: View {
    @EnvironmentObject private var appModel: AppModel
    @EnvironmentObject private var crewStore: CrewStore
    @Environment(\.dismiss) private var dismiss
    @State private var invitation = ""
    @State private var response = ""
    @State private var confirmation = ""
    @State private var deviceLabel = "我的设备"
    @State private var preview: CrewdRemoteInvitation?
    @State private var envelope: CrewdRemoteEnvelope?
    @State private var pendingID: UUID?
    @State private var status: String?
    @State private var busy = false
    @State private var removing: CrewdRemotePairSummary?

    var body: some View {
        NavigationStack {
            Form {
                if pendingID == nil {
                    Section("电脑的一次性邀请") {
                        TextEditor(text: $invitation).frame(minHeight: 150)
                            .textInputAutocapitalization(.never).autocorrectionDisabled()
                            .onChange(of: invitation) { _, _ in preview = nil; envelope = nil }
                        Button("核对电脑") { run {
                            let decoded = try CrewdRemoteText.decode(invitation)
                            preview = try appModel.crewdWindow.pairing.invitation(decoded)
                            envelope = decoded
                        } }.disabled(busy || invitation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                    if let preview, let envelope {
                        Section("确认电脑身份") {
                            Text(preview.computerLabel)
                            Text("\(preview.endpoint.host):\(preview.endpoint.port)")
                            Text(preview.server.spkiSha256).font(.caption.monospaced()).textSelection(.enabled)
                            TextField("设备名称", text: $deviceLabel)
                            Button("确认并生成回应") { run {
                                let id = try appModel.crewdWindow.pairing.respond(envelope, deviceLabel: deviceLabel)
                                pendingID = id
                                response = try appModel.crewdWindow.pairing.responseText(id)
                                status = "请将回应交给电脑，等待电脑批准后导入确认码。"
                            } }.disabled(busy || deviceLabel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        }
                    }
                }
                if let id = pendingID {
                    Section("发回电脑的回应") {
                        TextEditor(text: $response).frame(minHeight: 150).disabled(true)
                        Button("复制回应") {
                            UIPasteboard.general.string = response
                            status = "回应已复制；配对尚未完成。请在电脑上核对并批准。"
                        }
                    }
                    Section("电脑的正式确认码") {
                        TextEditor(text: $confirmation).frame(minHeight: 150)
                            .textInputAutocapitalization(.never).autocorrectionDisabled()
                        Button("确认并连接") {
                            run {
                                try appModel.crewdWindow.pairing.confirm(CrewdRemoteText.decode(confirmation), id: id)
                                status = "配对已确认，正在连接。"
                            }
                            guard (try? appModel.crewdWindow.pairing.paired(id)) != nil else { return }
                            Task {
                                await appModel.crewdWindow.select(id)
                                await appModel.connectCrewdAuthority()
                                await crewStore.refreshList()
                                status = appModel.crewdWindow.workspace?.client.connected == true ? "已安全连接。" : appModel.backendConfigurationError
                            }
                        }.disabled(busy || confirmation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
                Section("待确认的配对") {
                    ForEach(appModel.crewdWindow.computers.filter { !$0.paired }) { computer in
                        HStack {
                            Button(computer.name) { run {
                                pendingID = computer.id
                                response = try appModel.crewdWindow.pairing.responseText(computer.id)
                                confirmation = ""
                                status = "请核对这台电脑的回应和正式确认码。"
                            } }.buttonStyle(.borderless)
                            Spacer()
                            removeButton(computer)
                        }
                    }
                }
                Section("已配对的电脑") {
                    ForEach(appModel.crewdWindow.computers.filter(\.paired)) { computer in
                        HStack {
                            Button(computer.name) { Task {
                                await appModel.crewdWindow.select(computer.id)
                                await appModel.connectCrewdAuthority()
                                await crewStore.refreshList()
                            } }.buttonStyle(.borderless)
                            Spacer()
                            removeButton(computer)
                        }
                    }
                }
                if let status { Section { Text(status).foregroundStyle(.secondary) } }
            }
            .navigationTitle("连接电脑")
            .alert("从本设备移除已存电脑", isPresented: Binding(
                get: { removing != nil }, set: { if !$0 { removing = nil } }), presenting: removing) { computer in
                Button("移除配对凭据", role: .destructive) { removeComputer(computer) }
                Button("取消", role: .cancel) { removing = nil }
            } message: { computer in
                Text(computer.name + " · " + computer.endpoint.host + ":" + String(computer.endpoint.port) +
                     "\n本机配对记录：" + computer.id.uuidString.lowercased() +
                     "\n只移除本设备保存的配对凭据，不会删除电脑上的机组或数据，也不会撤销电脑端授权。")
            }
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("稍后继续") { dismiss() } } }
            .task { appModel.crewdWindow.refresh()
                if let pending = appModel.crewdWindow.computers.first(where: { !$0.paired }) {
                    run { pendingID = pending.id; response = try appModel.crewdWindow.pairing.responseText(pending.id) }
                }
            }
        }
    }
    private func removeButton(_ computer: CrewdRemotePairSummary) -> some View {
        Button("移除", role: .destructive) { removing = computer }
            .buttonStyle(.borderless).disabled(busy)
            .accessibilityLabel("从本设备移除 " + computer.name)
    }
    private func removeComputer(_ computer: CrewdRemotePairSummary) {
        guard !busy else { return }
        busy = true; defer { busy = false; removing = nil }
        // The original window verifies this exact displayed summary again;
        // a stale row cannot delete a changed record or redirect to another id.
        guard appModel.crewdWindow.forget(computer) else {
            status = appModel.crewdWindow.error ?? "移除结果尚未确认，请保留原记录待核对。"
            return
        }
        if pendingID == computer.id {
            pendingID = nil; response = ""; confirmation = ""
            preview = nil; envelope = nil; invitation = ""
        }
        status = "已从本设备移除这台电脑。电脑端的数据和授权未改变。"
    }
    private func run(_ operation: () throws -> Void) {
        guard !busy else { return }; busy = true; defer { busy = false }
        do { try operation() } catch { status = error.localizedDescription }
    }
}
#endif
