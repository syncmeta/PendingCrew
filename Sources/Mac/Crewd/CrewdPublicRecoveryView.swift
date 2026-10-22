import SwiftUI

/// Original-scope recovery reads the actual server receipt. Neither not-found,
/// historical results nor an external Unknown authorizes replay.
struct CrewdPublicRecoveryView: View {
    @ObservedObject var workspace: CrewdWorkspace
    var openCrew: ((String) -> Void)? = nil
    @State private var found: [UUID: CrewdPublicReceipt] = [:]
    @State private var missing = Set<UUID>()
    @State private var checking = Set<UUID>()
    @State private var failure: String?
    private var records: [CrewdPresentationJournal.Prepared] {
        var seen = Set<UUID>()
        return (workspace.durableUnknown.filter { $0.method == "command.submit" } + workspace.originalFirstMessageHistory)
            .filter { seen.insert($0.id).inserted }
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("原请求核对").font(.headline)
            Text("保留原数据库、请求身份与内容。此处仅查找；不会重新发送、补写终端字节或启动成员。")
                .font(.caption).foregroundStyle(.secondary)
            CrewdResourceRecoveryView(workspace:workspace)
            if let failure { Text(failure).foregroundStyle(.orange).textSelection(.enabled) }
            ForEach(records, id: \.id) { record in
                let id = record.params["requestId"].string.flatMap(UUID.init(uuidString:))
                VStack(alignment: .leading, spacing: 4) {
                    Text(record.params["command"]["operation"].string ?? "原命令").font(.subheadline.bold())
                    Text(record.params["requestId"].string ?? "缺少请求身份").font(.caption).textSelection(.enabled)
                    DisclosureGroup("原作用域与内容") {
                        Text(record.params.pretty).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                    }
                    if let id, let receipt = found[id] {
                        Text("后台阶段：\(receipt.phase.rawValue)\(receipt.historical ? " · 历史实例" : "")")
                        DisclosureGroup("后台原回执") { Text(receipt.value.pretty).font(.caption).textSelection(.enabled) }
                        if record.params["command"]["operation"].string == "crew.createFirstMessage", receipt.phase == .committed, openCrew != nil {
                            Button("查看原首条消息（不重新发送）") { open(record, receipt) }
                                .disabled(checking.contains(receipt.requestID) || !workspace.client.connected || workspace.isRetired)
                        }
                        if workspace.durableUnknown.contains(where: { $0.id == record.id }), !receipt.historical && [.committed, .launched, .written, .resized, .confirmed, .suppressed, .rejected].contains(receipt.phase) {
                            Button("已核对原回执，结束此项待确认") { acknowledge(record, receipt) }
                        }
                    } else if let id, missing.contains(id) {
                        Text("后台没有返回该原请求；结果仍未确认，不能据此再次发送。")
                            .font(.caption).foregroundStyle(.orange)
                    }
                    Button("只读查找原请求") { inspect(record) }
                        .disabled(id == nil || id.map { checking.contains($0) } == true || !workspace.client.connected || workspace.isRetired)
                }.padding(8).background(.quaternary.opacity(0.2), in: RoundedRectangle(cornerRadius: 8))
            }
        }
        .onChange(of: workspace.observationGeneration) { _, _ in found.removeAll(); missing.removeAll(); failure = nil }
    }
    private func inspect(_ record: CrewdPresentationJournal.Prepared) {
        guard let id = record.params["requestId"].string.flatMap(UUID.init(uuidString:)), !checking.contains(id) else { return }
        let epoch = workspace.observationGeneration
        checking.insert(id)
        Task {
            defer { checking.remove(id) }
            do {
                let receipt = try await workspace.lookupOriginalCommand(record)
                guard workspace.acceptsObservation(epoch) else { return }
                if let receipt { found[id] = receipt; missing.remove(id) }
                else { missing.insert(id) }
                failure = nil
            } catch { if workspace.acceptsObservation(epoch) { failure = error.localizedDescription } }
        }
    }
    private func open(_ record: CrewdPresentationJournal.Prepared, _ receipt: CrewdPublicReceipt) {
        let epoch = workspace.observationGeneration
        checking.insert(receipt.requestID)
        Task {
            defer { checking.remove(receipt.requestID) }
            do {
                let crew = try await workspace.openOriginalFirstMessage(record, receipt: receipt)
                guard workspace.acceptsObservation(epoch) else { return }
                openCrew?(crew); failure = nil
            } catch { if workspace.acceptsObservation(epoch) { failure = error.localizedDescription } }
        }
    }
    private func acknowledge(_ record: CrewdPresentationJournal.Prepared, _ receipt: CrewdPublicReceipt) {
        do { try workspace.acknowledgeOriginalCommand(record, receipt: receipt); found.removeValue(forKey: receipt.requestID) }
        catch { failure = error.localizedDescription }
    }
}

private struct CrewdResourceRecoveryView:View {
    @ObservedObject var workspace:CrewdWorkspace
    @State private var receipts:[UUID:CrewdValue]=[:]
    @State private var checking:Set<UUID>=[]
    @State private var failure:String?
    var body:some View {
        VStack(alignment:.leading,spacing:12){
            ForEach(workspace.durableUnknown.filter{CrewdWorkspace.resourceMutationMethods.contains($0.method)},id:\.id){record in
                VStack(alignment:.leading,spacing:8){
                    Text(record.method).font(.headline)
                    Text(record.params["requestId"].string ?? "缺少原请求身份").font(.caption).textSelection(.enabled)
                    DisclosureGroup("原输入与作用域"){Text(record.params.pretty).font(.caption).textSelection(.enabled)}
                    if let value=receipts[record.id] {
                        if value["status"] == .string("committed") {
                            DisclosureGroup("原提交回执"){Text(value["result"].pretty).font(.caption).textSelection(.enabled)}
                            Button("已核对原回执，结束待确认"){do{try workspace.acknowledgeOriginalResource(record,receipt:value);receipts[record.id]=nil}catch{failure=error.localizedDescription}}
                        }else{Text("当前未找到已提交回执；原输入保留，不会自动重新发送。").font(.caption).foregroundStyle(.orange)}
                    }
                    Button("只读查询原请求"){inspect(record)}.disabled(checking.contains(record.id) || !workspace.client.connected || workspace.isRetired)
                }.padding(10).background(.quaternary.opacity(0.2),in:RoundedRectangle(cornerRadius:8))
            }
            if let failure{Text(failure).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
        }.onChange(of:workspace.observationGeneration){_,_ in receipts=[:];failure=nil}
    }
    private func inspect(_ record:CrewdPresentationJournal.Prepared){
        guard checking.insert(record.id).inserted else{return}
        let epoch=workspace.observationGeneration
        Task{defer{checking.remove(record.id)};do{let receipt=try await workspace.lookupOriginalResource(record);guard workspace.acceptsObservation(epoch) else{return};receipts[record.id]=receipt;failure=nil}catch{if workspace.acceptsObservation(epoch){failure=error.localizedDescription}}}
    }
}
