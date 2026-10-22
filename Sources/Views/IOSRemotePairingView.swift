#if os(iOS)
import SwiftUI
import UIKit

/// The original pairing form uses the signed crewd invitation/response/receipt
/// exchange. Trust is established only after the computer's final confirmation.
struct IOSLegacyRemotePairingView: View {
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
    @StateObject private var operation = CrewdRemoteOperation()
    private var busy: Bool { operation.busy }
    @State private var epoch = UUID()
    @State private var removing: CrewdRemotePairSummary?

    var body: some View {
        NavigationStack {
            Form {
                Section {CrewdPairingStepHeader(awaitingReceipt:pendingID != nil,hasPreview:preview != nil)}
                if pendingID == nil {
                    Section("电脑的一次性邀请") {
                        TextEditor(text: $invitation).frame(height: 150)
                            .textInputAutocapitalization(.never).autocorrectionDisabled().disabled(busy)
                            .onChange(of: invitation) { _, _ in preview = nil; envelope = nil; status = nil }
                        CrewdRemoteCodeFileImport(text: $invitation, stage: .invitation, disabled: busy)
                        Button("核对电脑") { run {
                            let decoded = try CrewdRemoteText.decode(invitation)
                            return (try await appModel.crewdWindow.pairing.invitation(decoded), decoded)
                        } apply: { value in preview = value.0; envelope = value.1 } }.disabled(busy || invitation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                    if let preview, let envelope {
                        Section("确认电脑身份") {
                            Label(preview.computerLabel,systemImage:"desktopcomputer").font(.headline)
                            Text("\(preview.endpoint.host):\(preview.endpoint.port)").foregroundStyle(.secondary)
                            Text(preview.server.spkiSha256).font(.caption.monospaced()).textSelection(.enabled)
                            TextField("设备名称", text: $deviceLabel).disabled(busy)
                            Button("确认并生成回应") { run {
                                try await appModel.crewdWindow.pairing.respondWithText(envelope, deviceLabel: deviceLabel)
                            } apply: { result in
                                pendingID = result.id; response = result.text
                                status = "请将回应交给电脑，等待电脑批准后导入确认码。"
                            } }.disabled(busy || deviceLabel.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        }
                    }
                }
                if let id = pendingID {
                    Section("发回电脑的回应") {
                        CrewdRemoteCodeReadback(text: response, height: 150)
                        CrewdRemoteCodeFileExport(text: response, stage: .response, disabled: busy)
                        Button("复制回应") {
                            UIPasteboard.general.string = response
                            status = "回应已复制；配对尚未完成。请在电脑上核对并批准。"
                        }
                    }
                    Section("电脑的正式确认码") {
                        TextEditor(text: $confirmation).frame(height: 150)
                            .textInputAutocapitalization(.never).autocorrectionDisabled().disabled(busy)
                        CrewdRemoteCodeFileImport(text: $confirmation, stage: .receipt, disabled: busy)
                        Button("确认并连接") {
                            run {
                                try await appModel.crewdWindow.pairing.confirmReceipt(CrewdRemoteText.decode(confirmation), id: id)
                            } apply: { _ in
                                status = "配对已确认，正在连接。"
                                let token = epoch
                                Task {
                                    guard epoch == token else { return }
                                    await appModel.crewdWindow.select(id)
                                    guard epoch == token else { return }
                                    await appModel.connectCrewdAuthority()
                                    guard epoch == token else { return }
                                    await crewStore.refreshList()
                                    guard epoch == token else { return }
                                    status = appModel.crewdWindow.workspace?.client.connected == true ? "已安全连接。" : appModel.backendConfigurationError
                                }
                            }
                        }.disabled(busy || confirmation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
                Section("待确认的配对") {
                    ForEach(appModel.crewdWindow.computers.filter { !$0.paired }) { computer in
                        HStack {
                            Button(computer.name) { run {
                                try await appModel.crewdWindow.pairing.responseText(computer.id)
                            } apply: { text in
                                pendingID = computer.id; response = text; confirmation = ""
                                status = "请核对这台电脑的回应和正式确认码。"
                            } }.buttonStyle(.borderless).disabled(busy)
                            Spacer()
                            removeButton(computer)
                        }
                    }
                }
                Section{CrewPersonalHostDirectory()}
                Section{CrewPersonalDraftArchive()}
                Section("已配对的电脑") {
                    ForEach(appModel.crewdWindow.computers.filter(\.paired)) { computer in
                        HStack {
                            Button(computer.name) { Task {
                                await appModel.crewdWindow.select(computer.id)
                                await appModel.connectCrewdAuthority()
                                await crewStore.refreshList()
                            } }.buttonStyle(.borderless).disabled(busy)
                            Spacer()
                            removeButton(computer)
                        }
                    }
                }
                if let status { Section {CrewdInlineNotice(title:"当前进度",detail:status)} }
            }
            .tint(Theme.Palette.accent)
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
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("稍后继续") { epoch = UUID(); operation.invalidate(); dismiss() } } }
            .onDisappear { epoch = UUID(); operation.invalidate() }
            .task {
                let token = epoch
                await appModel.crewdWindow.refresh()
                guard epoch == token, !Task.isCancelled else { return }
                if let pending = appModel.crewdWindow.computers.first(where: { !$0.paired }) {
                    run { try await appModel.crewdWindow.pairing.responseText(pending.id) } apply: { text in pendingID = pending.id; response = text }
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
        run { await appModel.crewdWindow.forget(computer) } apply: { removed in
            removing = nil
            guard removed else {
                status = appModel.crewdWindow.error ?? "移除结果尚未确认，请保留原记录待核对。"; return
            }
            if pendingID == computer.id {
                pendingID = nil; response = ""; confirmation = ""
                preview = nil; envelope = nil; invitation = ""
            }
            status = "已从本设备移除这台电脑。电脑端的数据和授权未改变。"
        }
    }
    @MainActor private func run<Value: Sendable>(_ action: @escaping @MainActor () async throws -> Value, apply: @escaping @MainActor (Value) -> Void) {
        operation.run(action, apply: apply, failed: { error in status = error.localizedDescription })
    }
}
#endif

#if os(iOS)
struct IOSRemotePairingView:View {
    @EnvironmentObject private var appModel:AppModel
    @EnvironmentObject private var crewStore:CrewStore
    @Environment(\.dismiss) private var dismiss
    @State private var computers=false
    @State private var pending:CrewdRemotePairSummary?
    @State private var removing:CrewdRemotePairSummary?
    var body:some View {
        CrewdEnrollmentView(completed:choose)
            .safeAreaInset(edge:.bottom){Button("已存电脑与草稿"){computers=true}.padding(8)}
            .sheet(isPresented:$computers){computerList}
            .sheet(item:$pending){computer in
                CrewdRemotePairingView(pairing:appModel.crewdWindow.pairing,pending:computer.id,completed:choose)
            }
    }
    private var computerList:some View {
        NavigationStack {
            List {
                Section("已存电脑") {
                    if let error=appModel.crewdWindow.error{Text(error).foregroundStyle(.red)}
                    if appModel.crewdWindow.computers.isEmpty{Text("尚无已存电脑。返回添加电脑，粘贴一次性配对链接。")}
                    ForEach(appModel.crewdWindow.computers){computer in
                        HStack {
                            Button {
                                computers=false
                                if computer.paired{choose(computer.id)}else{pending=computer}
                            }label:{VStack(alignment:.leading){Text(computer.name);Text(computer.paired ? "\(computer.endpoint.host):\(computer.endpoint.port)":"原文件配对等待确认").font(.caption).foregroundStyle(.secondary)}}
                            .buttonStyle(.borderless)
                            Spacer()
                            Button("移除",role:.destructive){removing=computer}.buttonStyle(.borderless)
                        }.disabled(appModel.crewdWindow.credentialBusy)
                    }
                }
                Section{CrewPersonalHostDirectory()}
                Section{CrewPersonalDraftArchive()}
            }
            .navigationTitle("已存电脑")
            .toolbar{ToolbarItem(placement:.cancellationAction){Button("返回添加电脑"){computers=false}}}
            .task{await appModel.crewdWindow.refresh()}
            .alert("从本设备移除已存电脑",isPresented:Binding(get:{removing != nil},set:{if !$0{removing=nil}}),presenting:removing){computer in
                Button("移除配对凭据",role:.destructive){Task{_ = await appModel.crewdWindow.forget(computer)};removing=nil}
                Button("取消",role:.cancel){removing=nil}
            }message:{computer in Text(computer.name+" · "+computer.endpoint.host+":"+String(computer.endpoint.port)+"\n只移除本设备保存的配对凭据与其私钥恢复副本，不会删除电脑数据或撤销服务器授权。")}
        }
    }
    private func choose(_ id:UUID){Task{await appModel.crewdWindow.select(id);await appModel.connectCrewdAuthority();await crewStore.refreshList();dismiss()}}
}
#endif
