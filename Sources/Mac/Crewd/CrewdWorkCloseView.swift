import SwiftUI

struct CrewdWorkCloseView: View {
    @ObservedObject var store: CrewdWorkStore
    let id: CrewdWorkID
    let detail: CrewdValue
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
        let current=store.plans[id]?.detail ?? detail
        VStack(alignment:.leading,spacing:8) {
            if current["closure"] != .null {
                Text("关闭类型：" + label(current["closure"]["kind"].string ?? ""))
                Text("关闭说明：" + (current["closure"]["note"].string ?? ""))
                Text("关闭凭据：" + (current["closure"]["evidence"]["value"].string ?? "")).textSelection(.enabled)
            }
            if !["done","dropped"].contains(current["status"].string ?? ""),current["frozen"] != .bool(true) {
                DisclosureGroup("凭据性关闭计划",isExpanded:$expanded) {
                    Picker("关闭类型",selection:$kind) {Text("已完成").tag("done");Text("重复").tag("duplicate");Text("已被取代").tag("superseded")}
                    Picker("凭据类型",selection:$evidenceKind) {Text("文字").tag("text");Text("提交").tag("commit");Text("链接").tag("url")}
                    TextField("凭据（必填）",text:$evidence,axis:.vertical).lineLimit(2...5)
                    TextField("关闭说明（必填）",text:$note,axis:.vertical).lineLimit(2...5)
                    Text("重复或被取代时，请在凭据中明确指出对应成果或计划。关闭将更新此计划的终态。").foregroundStyle(.secondary)
                    Button(busy ? "正在提交…":"确认关闭计划") {submit(current)}.disabled(busy || evidence.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || note.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty)
                }.disabled(busy)
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
        .onDisappear {token=UUID();historyToken=UUID();historyBusy=false}
    }
    private func reset() {historyToken=UUID();history=[];historyNext=nil;historyLoaded=false;historyBusy=false;token=UUID();busy=false;failure=nil;evidence="";note="";expanded=false}
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
    private func submit(_ current:CrewdValue) {
        let request=UUID(), target=id;token=request;busy=true;failure=nil
        Task { @MainActor in
            defer {if token == request {busy=false}}
            do {
                _ = try await store.closePlan(target,expectedRevision:current["revision"].string ?? "",kind:kind,evidenceKind:evidenceKind,evidence:evidence,note:note)
                guard token == request,id == target else{return};expanded=false
            } catch is CancellationError {
                return
            } catch {
                guard token == request,id == target else{return}
                failure="关闭未获确认："+error.localizedDescription+"；请核对当前详情，不会自动重发。"
                _ = try? await store.detail(target)
            }
        }
    }
}
