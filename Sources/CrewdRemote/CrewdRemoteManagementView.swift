#if os(macOS)
import SwiftUI

struct CrewdRemoteManagementView: View {
    @ObservedObject var admin: CrewdRemoteAdmin
    @Environment(\.dismiss) private var dismiss
    @State private var invitationText = ""
    @State private var receiptText = ""
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack { Text("设备连接").font(.title2); Spacer(); Button("关闭") { dismiss() }.disabled(admin.busy) }
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    if let error = admin.error { Text(error).foregroundStyle(.red).textSelection(.enabled) }
                    if let status = admin.status {
                        GroupBox("电脑连接设置") {
                            VStack(alignment: .leading, spacing: 10) {
                                Text(status.listening ? "正在接受已配对设备的连接" : "设备连接已关闭")
                                if let failure = status.failure { Text(failure).foregroundStyle(.orange) }
                                TextField("电脑的局域网地址", text: $admin.listenHost).disabled(admin.busy)
                                TextField("端口", text: $admin.port).disabled(admin.busy)
                                Text("只在你选择的地址上接收连接。手机要使用电脑实际的局域网地址。")
                                    .font(.caption).foregroundStyle(.secondary)
                                HStack {
                                    Button("允许设备连接") { Task { await admin.configure(enabled: true) } }
                                    Button("关闭设备连接") { Task { await admin.configure(enabled: false) } }
                                    Button("读取最新状态") { Task { await admin.refresh() } }
                                }.disabled(admin.busy)
                            }.padding(8)
                        }
                        GroupBox("单链接添加设备") {
                            VStack(alignment:.leading,spacing:10) {
                                TextField("本电脑显示名称",text:$admin.computerLabel)
                                TextField("其他设备可访问的主机地址",text:$admin.enrollmentHost)
                                Text("使用已启用的连接端口；每台设备确认后获得独立授权。链接五分钟有效，请只交给你的设备。")
                                    .font(.caption).foregroundStyle(.secondary)
                                if admin.mayIssueEnrollment {
                                    HStack {
                                        Button(admin.enrollment?.link == nil ? "生成配对链接":"为另一台设备生成新链接"){Task{await admin.createEnrollment()}}
                                            .buttonStyle(CrewdPrimaryButtonStyle())
                                            .disabled(admin.enrollment?.unknown == true || admin.enrollment?.prepared == true || !status.listening)
                                        Button("读取原链接请求"){Task{await admin.readEnrollment()}}
                                    }
                                    if let original=admin.enrollment {
                                        Text("原请求："+original.requestId).font(.caption.monospaced()).textSelection(.enabled)
                                        if let link=original.link {ShareLink("分享此配对链接",item:link)}
                                        if original.prepared {Button("提交已保存的原链接请求"){Task{await admin.lookupEnrollment(submitPrepared:true)}}}
                                        if original.unknown {Text("结果未确认，只查询原请求；不会重新生成token。")}
                                        Button("查询原链接结果"){Task{await admin.lookupEnrollment()}}
                                    }
                                }else{Text("当前连接未开放本机生成配对链接。需要本机 Human 管理连接与后台 enrollment 能力。")}
                            }.padding(8).disabled(admin.busy)
                        }
                        DisclosureGroup("保留的旧文件配对") {
                        GroupBox("添加手机或另一台电脑") {
                            VStack(alignment: .leading, spacing: 10) {
                                TextField("本电脑显示名称", text: $admin.computerLabel).disabled(admin.busy)
                                Button("生成新的邀请") { Task {
                                    await admin.newInvitation()
                                    if let envelope = admin.invitation { invitationText = (try? CrewdRemoteText.encode(envelope)) ?? "" }
                                } }.disabled(admin.busy || !status.listening)
                                if !invitationText.isEmpty {
                                    Text("把邀请连接码交给你的设备。邀请五分钟内有效。")
                                    ShareLink("分享邀请连接码", item: invitationText)
                                    CrewdRemoteCodeFileExport(text: invitationText, stage: .invitation, disabled: admin.busy)
                                    Text(invitationText).font(.caption.monospaced()).lineLimit(3).textSelection(.enabled)
                                }
                                Text("粘贴设备生成的回复码")
                                TextEditor(text: $admin.responseText).frame(minHeight: 80).disabled(admin.busy)
                                    .onChange(of: admin.responseText) { _, _ in admin.preview = nil }
                                CrewdRemoteCodeFileImport(text: $admin.responseText, stage: .response, disabled: admin.busy)
                                Button("核对设备") { admin.reviewResponse() }.disabled(admin.busy || admin.responseText.isEmpty)
                                if let preview = admin.preview {
                                    Text(preview.deviceLabel).font(.headline)
                                    Text("设备身份：\(preview.clientSpkiSha256)").font(.caption.monospaced()).textSelection(.enabled)
                                    Text("核对这是你的设备后批准；它可以与你协作管理这台电脑上的机组工作。")
                                        .font(.caption).foregroundStyle(.secondary)
                                    Button("批准此设备连接") { Task {
                                        await admin.approve()
                                        updateReceiptText()
                                    } }.disabled(admin.busy)
                                }
                                if let accepted = admin.accepted {
                                    if accepted.grant.revokedAt != nil { Text("此设备连接已撤销，不能用旧确认码重新授权。") }
                                    else {
                                        Text("已批准 \(accepted.grant.deviceLabel)，把确认码交回设备完成配对。")
                                        ShareLink("分享电脑确认码", item: receiptText)
                                        CrewdRemoteCodeFileExport(text: receiptText, stage: .receipt, disabled: admin.busy)
                                        Text(receiptText).font(.caption.monospaced()).lineLimit(3).textSelection(.enabled)
                                    }
                                }
                            }.padding(8)
                        }
                        }
                    } else { Button("读取连接设置") { Task { await admin.refresh() } }.disabled(admin.busy) }
                    if !admin.unknown.isEmpty {
                        GroupBox("结果尚未确认") {
                            VStack(alignment: .leading, spacing: 10) {
                                Text("操作可能已经完成。原内容和请求编号仍保留，不会自动重发。")
                                ForEach(admin.unknown) { draft in
                                    VStack(alignment: .leading) {
                                        Text(draft.label)
                                        if draft.method == "remote.revoke" {
                                            Text("请读取最新设备状态核对是否已撤销，不重新发送旧撤销操作。")
                                        } else { Button("使用原内容核对") { Task {
                                            await admin.confirm(draft)
                                            if draft.method == "remote.invite", let envelope = admin.invitation { invitationText = (try? CrewdRemoteText.encode(envelope)) ?? "" }
                                            updateReceiptText()
                                        } }.disabled(admin.busy) }
                                    }
                                }
                            }.padding(8)
                        }
                    }
                    if !admin.grants.isEmpty {
                        GroupBox("设备") {
                            VStack(alignment: .leading, spacing: 12) {
                                ForEach(admin.grants) { grant in
                                    VStack(alignment: .leading, spacing: 6) {
                                        Text(grant.deviceLabel).font(.headline)
                                        Text(grant.clientSpkiSha256).font(.caption.monospaced()).textSelection(.enabled)
                                        if grant.revokedAt != nil { Text("已撤销 · \(grant.revokeNote ?? "")").foregroundStyle(.secondary) }
                                        else {
                                            TextField("撤销理由", text: Binding(get: { admin.revokeNotes[grant.id] ?? "" }, set: { admin.revokeNotes[grant.id] = $0 })).disabled(admin.busy)
                                            Button("撤销连接", role: .destructive) { Task { await admin.revoke(grant); updateReceiptText() } }.disabled(admin.busy)
                                        }
                                    }
                                }
                            }.padding(8)
                        }
                    }
                }
            }
        }.padding(20).frame(minWidth: 600, minHeight: 560)
            .interactiveDismissDisabled(admin.busy)
            .task { await admin.refresh(); updateReceiptText() }
    }
    private func updateReceiptText() {
        if let accepted = admin.accepted, accepted.grant.revokedAt == nil { receiptText = (try? CrewdRemoteText.encode(accepted.receipt)) ?? "" }
        else { receiptText = "" }
    }
}
#endif

#if os(macOS)
/// Retired admin inputs remain inspectable; no write retry through the new scope.
struct CrewdRemoteAdminReadback: View {
    @ObservedObject var admin: CrewdRemoteAdmin
    var body: some View {
        if !admin.unknown.isEmpty {
            DisclosureGroup("电脑设备连接 · 原操作") {
                ForEach(admin.unknown) { draft in
                    VStack(alignment:.leading,spacing:6) {
                        Text(draft.label)
                        if draft.method == "remote.configure" {
                            Text("原设置：\(draft.params["listenHost"].string ?? ""):\(draft.params["port"].int ?? 0) · \(draft.params["enabled"] == .bool(true) ? "允许连接" : "关闭连接")")
                        } else if draft.method == "remote.invite" { Text("电脑名称：\(draft.params["deviceLabel"].string ?? "")") }
                        else if draft.method == "remote.revoke" { Text("撤销理由：\(draft.params["note"].string ?? "")") }
                        else if draft.method == "remote.accept", let code = responseCode(draft.params["responseEnvelope"]) {
                            ShareLink("原设备回复码",item:code)
                            Text(code).font(.caption.monospaced()).lineLimit(3).textSelection(.enabled)
                        }
                        DisclosureGroup("原请求编号") { Text(draft.id.uuidString.lowercased()).font(.caption.monospaced()).textSelection(.enabled) }
                    }
                }
            }
        }
    }
    private func responseCode(_ value:CrewdValue)->String? {
        guard let data=try? JSONEncoder().encode(value),let envelope=try? JSONDecoder().decode(CrewdRemoteEnvelope.self,from:data) else{return nil}
        return try? CrewdRemoteText.encode(envelope)
    }
}
#endif
