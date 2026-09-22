#if os(iOS)
import SwiftUI
import UIKit

/// Minimal manual bridge for the first iOS data-plane batch: paste a one-use Mac invitation,
/// persist the iOS Keychain identity/trust/backend records, then copy the signed response back.
struct IOSRemotePairingView: View {
    @EnvironmentObject private var appModel: AppModel
    @EnvironmentObject private var crewStore: CrewStore
    @Environment(\.dismiss) private var dismiss
    @State private var invitation = ""
    @State private var response = ""
    @State private var status: String?

    var body: some View {
        NavigationStack {
            Form {
                Section("Mac 的一次性邀请") {
                    TextEditor(text: $invitation).frame(minHeight: 150)
                    Button("导入并生成回应") { importInvitation() }
                        .disabled(invitation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                if !response.isEmpty {
                    Section("发回 Mac 的回应") {
                        TextEditor(text: $response).frame(minHeight: 150)
                        Button("复制回应") {
                            UIPasteboard.general.string = response
                            status = "回应已复制。请在 Mac 设置中导入，然后重启后台。"
                        }
                    }
                }
                if let status { Section { Text(status).foregroundStyle(.secondary) } }
            }
            .navigationTitle("连接 Mac")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("完成") { dismiss() } }
            }
        }
    }

    private func importInvitation() {
        do {
            let coordinator = try ManualPairingCoordinator.production()
            guard case let .invitationAccepted(text, _) = try coordinator.importText(invitation)
            else { throw ManualPairingError.invalidInvitation }
            response = text
            appModel.reloadRemoteBackend()
            Task { await crewStore.refreshList() }
            status = "邀请已保存；设备私钥留在本机 Keychain。复制回应到 Mac，Mac 导入并重启后台后即可连接。"
        } catch {
            response = ""
            status = "导入失败：\(error.localizedDescription)"
        }
    }
}
#endif
