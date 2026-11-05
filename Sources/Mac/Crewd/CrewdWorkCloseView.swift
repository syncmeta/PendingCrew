import SwiftUI
import Combine

struct CrewdWorkCloseView: View {
    @ObservedObject var store: CrewdWorkStore
    let id: CrewdWorkID
    let detail: CrewdValue
    var workspace: CrewdWorkspace? = nil
    var scopeIsCurrent: @MainActor () -> Bool = { true }
    @State private var originals: [CrewdPresentationJournal.Prepared] = []
    @State private var confirmed: CrewdPublicReceipt?
    @State private var expanded=false
    @State private var kind="done"
    @State private var evidenceKind="text"
    @State private var evidence=""
    @State private var note=""
    @State private var busy=false
    @State private var failure:String?
    @State private var token=UUID()
    @State private var history:[CrewdValue]=[]
    @State private var historyNext:String?
    @State private var historyLoaded=false
    @State private var historyBusy=false
    @State private var historyToken=UUID()
    var body: some View {
        let current = store.plans[id]?.value ?? detail
        let crew = current["crewId"].string ?? ""
        let routeCurrent = scopeIsCurrent()
        let scopedOriginals = originals.filter {
            CrewdFactualWorkCloseWire.matchesOriginal(method:$0.method,params:$0.params,journalCrew:$0.crew,id:id.raw,crew:crew)
        }
        VStack(alignment:.leading,spacing:8) {
            if current["closure"] != .null {
                Text("关闭类型：" + label(current["closure"]["kind"].string ?? ""))
                Text("关闭说明：" + (current["closure"]["note"].string ?? ""))
                Text("关闭凭据：" + (current["closure"]["evidence"]["value"].string ?? "")).textSelection(.enabled)
            }
            if let workspace,!crew.isEmpty {
                CrewdArtifactSubjectEntry(workspace:workspace,subject:.init(kind:.work,id:id.raw),crew:crew,
                    frozen:current["frozen"] == .bool(true),current:scopeIsCurrent)
            }
            recovery(scopedOriginals,crew:crew)
            if scopedOriginals.isEmpty, !["done","dropped"].contains(current["status"].string ?? ""),
               current["frozen"] != .bool(true) {

                DisclosureGroup("凭据性关闭计划",isExpanded:$expanded) {
                    Text(id.raw + " · 当前版本 " + (current["revision"].string ?? "")).foregroundStyle(.secondary)
                    Picker("关闭类型",selection:$kind) {Text("已完成").tag("done");Text("重复").tag("duplicate");Text("已被取代").tag("superseded")}
                    Picker("凭据类型",selection:$evidenceKind) {Text("文字").tag("text");Text("提交").tag("commit");Text("链接").tag("url")}
                    TextField("凭据（必填）",text:$evidence,axis:.vertical).lineLimit(2...5)
                    TextField("关闭说明（必填）",text:$note,axis:.vertical).lineLimit(2...5)
                    Text("重复或被取代时，请在凭据中明确指出对应成果或计划。关闭将更新此计划的终态。").foregroundStyle(.secondary)
                    Button(busy ? "正在提交…":"确认关闭计划") {submit(current)}.disabled(busy || evidence.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || note.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty)
                }.disabled(!routeCurrent || busy || (store.offersFactualWorkClose && (workspace == nil || !store.supportsFactualWorkClose)))
            }
            Button(historyBusy ? "正在读取关闭历史…":"读取关闭历史") {readHistory(reset:true)}.disabled(historyBusy)
            ForEach(Array(history.enumerated()),id:\.offset) {_,event in
                VStack(alignment:.leading) {
                    Text((event["at"].string ?? "") + " · " + label(event["closure"]["kind"].string ?? ""))
                    Text("操作者：" + actor(event["actor"]))
                    if let survivor = event["duplicateReview"]["survivorId"].string,
                       let revision = event["duplicateReview"]["survivorRevision"].string {
                        Text("复核保留项 " + survivor + " · 当时版本 " + revision).textSelection(.enabled)
                    }
                    if let note=event["closure"]["note"].string {Text(note)}
                    if let evidence=event["closure"]["evidence"]["value"].string {Text("凭据："+evidence).textSelection(.enabled)}
                }
            }
            if historyLoaded,history.isEmpty {Text("尚无关闭历史。")}
            if historyNext != nil {Button("更早的关闭记录") {readHistory(reset:false)}.disabled(historyBusy)}
            if let failure {Text(failure).foregroundStyle(.orange)}
        }.font(.caption)
        .onChange(of:id) {_,_ in reset()}
        .onChange(of:store.readScope) {_,_ in reset()}
        .task(id: RecoveryScope(id:id,crew:crew,store:store.readScope,routeCurrent:routeCurrent)) { reloadOriginals(crew:crew) }
        .onReceive(workspace?.$durableUnknown.eraseToAnyPublisher() ?? Empty().eraseToAnyPublisher()) { records in
            // @Published sends the new records before replacing the old value.
            originals = workspace?.originalWorkCloseRecords(id.raw,crew:crew,in:records) ?? []
        }
        .onDisappear { token=UUID();busy=false;historyToken=UUID();historyBusy=false }

    }
    private struct RecoveryScope: Hashable { let id: CrewdWorkID; let crew: String; let store: UUID; let routeCurrent: Bool }
    private func reset() {
        historyToken=UUID();history=[];historyNext=nil;historyLoaded=false;historyBusy=false
        token=UUID();busy=false;failure=nil;evidence="";note="";expanded=false;confirmed=nil;originals=[]
    }
    private func reloadOriginals(crew: String) {
        guard !crew.isEmpty, scopeIsCurrent(), let workspace else { return }
        do { originals = try workspace.refreshWorkCloseOriginals(id.raw,crew:crew) }
        catch { failure = error.localizedDescription }
    }

    @ViewBuilder private func recovery(_ originals: [CrewdPresentationJournal.Prepared], crew: String) -> some View {
        ForEach(originals) { original in
            if original.method == "work.close" {
                Text("旧版关闭提交待核对。当前终态无法证明这笔提交的结果。").foregroundStyle(.orange)
            } else {
                let params = original.params["command"]["params"]
                let request = original.params["requestId"].string ?? ""
                Text("原关闭请求待核对 · " + request).textSelection(.enabled)
                Text(label(params["kind"].string ?? "") + " · 原版本 " + (params["expectedRevision"].string ?? ""))
                Text("关闭说明：" + (params["note"].string ?? "")).textSelection(.enabled)
                Text("关闭凭据：" + (params["evidence"]["value"].string ?? "")).textSelection(.enabled)
                if params["crewId"] == .string(crew) {
                    Button(busy ? "正在核对…" : "查询原关闭回执") { lookup(original, crew: crew) }
                        .disabled(!scopeIsCurrent() || busy || !store.supportsFactualWorkClose)
                }
            }
        }
        if let confirmed, confirmed.value["result"]["id"] == .string(id.raw),
           confirmed.value["result"]["crewId"].string?.utf8.elementsEqual(crew.utf8) == true {
            let result = confirmed.value["result"]
            let title = confirmed.historical ? "已核实原关闭的历史回执" : "已核实原关闭回执"
            Text(title + " · " + label(result["kind"].string ?? ""))
            Text("版本 " + (result["originalRevision"].string ?? "") + " → " +
                 (result["resultRevision"].string ?? "") + " · 事件 " + (result["eventSeq"].string ?? ""))
                .textSelection(.enabled)
            Text("关闭说明：" + (result["note"].string ?? "")).textSelection(.enabled)
            Text("关闭凭据：" + (result["evidence"]["value"].string ?? "")).textSelection(.enabled)
        }
    }

    private func lookup(_ original: CrewdPresentationJournal.Prepared, crew: String) {
        guard !busy, scopeIsCurrent(), let workspace else { return }
        let request = UUID()
        token = request; busy = true; failure = nil; confirmed = nil
        Task { @MainActor in
            defer { if token == request { busy = false } }
            do {
                let receipt = try await workspace.recoverFactualWorkClose(original, id: id.raw, crew: crew) {
                    token == request && scopeIsCurrent()
                }
                guard token == request, scopeIsCurrent() else { return }
                if let receipt {
                    confirmed = receipt
                    try workspace.acknowledgeFactualWorkClose(receipt,lookedUp:original)
                } else { failure = "后台尚未找到原关闭回执。原请求仍保留。" }
            } catch is CancellationError {
                return
            } catch {
                if token == request, scopeIsCurrent() {
                    failure = confirmed == nil ? error.localizedDescription : "已核实回执，本机核对记录尚未保存。"
                }
            }
        }
    }
    private func label(_ kind:String)->String {["done":"已完成","duplicate":"重复","superseded":"已被取代"][kind] ?? kind}
    private func actor(_ value:CrewdValue)->String {
        switch value["kind"].string {case "human":return "人类";case "system":return "系统";default:return value["id"].string ?? "成员"}
    }
    private func readHistory(reset:Bool) {
        let request=UUID(),target=id,cursor=reset ? nil:historyNext
        let crew = detail["crewId"].string
        historyToken=request;historyBusy=true;failure=nil
        if reset {history=[];historyNext=nil;historyLoaded=false}
        Task { @MainActor in
            defer {if historyToken == request {historyBusy=false}}
            do {
                let page=try await store.closureHistory(target,beforeSeq:cursor,duplicateReviewCrew:crew)
                guard historyToken == request,id == target else{return}
                history += page["items"].array;historyNext=page["nextBeforeSeq"].string;historyLoaded=true
            } catch is CancellationError {return}
            catch {if historyToken == request {failure=error.localizedDescription}}
        }
    }
    private func submit(_ current: CrewdValue) {
        guard !busy, scopeIsCurrent(), current["id"] == .string(id.raw),
              let crew = current["crewId"].string, let revision = current["revision"].string else { return }
        let params: CrewdValue = .object([
            "crewId": .string(crew), "id": .string(id.raw), "expectedRevision": .string(revision),
            "kind": .string(kind), "evidence": .object(["kind": .string(evidenceKind), "value": .string(evidence)]),
            "note": .string(note)
        ])
        let request = UUID(), modern = store.offersFactualWorkClose
        token = request; busy = true; failure = nil; confirmed = nil
        Task { @MainActor in
            defer { if token == request { busy = false } }
            @MainActor func currentRoute() -> Bool { token == request && scopeIsCurrent() }
            guard currentRoute() else { return }
            do {
                if modern {
                    guard let workspace else { return }
                    let receipt = try await workspace.submitFactualWorkClose(params, stillCurrent: currentRoute)
                    guard currentRoute() else { return }
                    confirmed = receipt
                    if !receipt.historical { try workspace.acknowledgeFactualWorkClose(receipt) }
                } else {
                    _ = try await store.closePlan(id, expectedRevision: revision, kind: params["kind"].string ?? "",
                        evidenceKind: params["evidence"]["kind"].string ?? "", evidence: params["evidence"]["value"].string ?? "",
                        note: params["note"].string ?? "", scopeIsCurrent: currentRoute)
                }
                guard currentRoute() else { return }
                expanded = false
                _ = try? await store.detail(id)
            } catch is CancellationError {
                return
            } catch {
                guard currentRoute() else { return }
                reloadOriginals(crew:crew)
                if confirmed != nil { failure = "已核实回执，本机核对记录尚未保存。" }
                else { failure = originals.isEmpty ? error.localizedDescription : "原关闭结果尚待核对。" }
                _ = try? await store.detail(id)
            }
        }
    }
}
