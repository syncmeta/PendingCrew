#if os(macOS)
import SwiftUI

/// Human can inspect and cancel an offer; only the real target agent can accept it.
struct CrewdHandoffView:View {
    @ObservedObject var store:CrewdWorkStore
    let id:CrewdWorkID
    let members:[CrewdValue]
    @Environment(\.colorScheme) private var colorScheme
    @State private var historyExpanded=false
    @State private var history:[CrewdValue]=[]
    @State private var nextBefore:String?
    @State private var reading=false
    @State private var cancelling=false
    @State private var query=UUID()
    @State private var failure:String?
    @State private var uncertain=false
    private var plan:CrewdPlan? {store.plans[id]}
    private func name(_ raw:String?)->String {
        guard let raw else {return "未指派"}
        return members.first(where:{$0["id"].string == raw})?["displayName"].string ?? raw
    }
    var body:some View {
        VStack(alignment:.leading,spacing:10) {
            if let plan,plan.value["pendingTransfer"] != .null {
                let offer=plan.value["pendingTransfer"]
                VStack(alignment:.leading,spacing:5) {
                    Text("待 \(name(offer["to"].string)) 接手").font(.headline)
                    Text("当前负责人仍是 \(name(plan.value["owner"].string))").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                    if let note=offer["note"].string {Text(note).textSelection(.enabled)}
                    else {Text("请选择详情读取完整交接说明").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))}
                    if offer["noteTruncated"] == .bool(true) {Text("交接说明较长，当前展示已截短。").font(.caption).foregroundStyle(.orange)}
                    if let by=offer["requestedBy"].string {Text("提出：\(name(by))").font(.caption)}
                    if let at=offer["at"].string {Text(at).font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))}
                    Text("接手由目标成员自己确认。").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                    Button(cancelling ? "取消中":"取消交接请求") {withdraw(plan,offer)}.disabled(cancelling || plan.frozen)
                }.padding(10).background(CrewdChatStyle.bubble(colorScheme),in:RoundedRectangle(cornerRadius:6))
            }
            if let failure {Text(failure).foregroundStyle(.orange).textSelection(.enabled)}
            if uncertain {Text("取消结果未确认，请读取当前详情核对。不会自动重复取消。").font(.caption).foregroundStyle(.orange);Button("读取当前详情"){Task {do {_ = try await store.detail(id)} catch {failure=describe(error)}}}}
            DisclosureGroup("负责人交接记录",isExpanded:Binding(get:{historyExpanded},set:{historyExpanded=$0;if $0 && history.isEmpty {Task {await load(reset:true)}}})) {
                VStack(alignment:.leading,spacing:8) {
                    ForEach(history.indices,id:\.self){i in
                        let record=history[i]
                        VStack(alignment:.leading,spacing:3) {
                            Text("\(label(record["how"].string))：\(name(record["from"].string)) → \(name(record["to"].string))").font(.caption)
                            Text(record["note"].string ?? "").textSelection(.enabled)
                            Text("确认人：" + actor(record["by"])).font(.caption)
                            Text(record["at"].string ?? "").font(.caption2).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                        }
                    }
                    if reading {ProgressView().controlSize(.small)}
                    if nextBefore != nil {Button("更早的交接记录"){Task {await load(reset:false)}}.disabled(reading)}
                    Button("刷新交接记录"){Task {await load(reset:true)}}.disabled(reading)
                    if history.isEmpty && !reading {Text("尚无负责人变更记录").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))}
                }
            }
        }
    }
    private func label(_ value:String?)->String {
        switch value {case "claim":return "认领";case "assign":return "指派";case "transfer":return "接受交接";case "lost":return "失联后重派";default:return "负责人变更"}
    }
    private func actor(_ value:CrewdValue)->String {
        switch value["kind"].string {case "human":return "你";case "system":return "系统";case "session":return name(value["id"].string);default:return "未记录"}
    }
    private func describe(_ error:Error)->String {
        guard let e=error as? CrewdClientError else {return error.localizedDescription}
        return [e.message,e.code.map{"错误 \($0)"},e.refusal].compactMap{$0}.joined(separator:" · ")
    }
    private func load(reset:Bool) async {
        guard !reading else {return}
        reading=true;defer{reading=false}
        let request=UUID(),crew=store.selectedCrew;query=request
        do {
            let page=try await store.ownerHistory(id,before:reset ? nil:nextBefore)
            guard request == query,crew == store.selectedCrew,!Task.isCancelled else {return}
            if reset {history=page["items"].array} else {
                let known=Set(history.compactMap{$0["seq"].string})
                history += page["items"].array.filter {item in item["seq"].string.map{!known.contains($0)} ?? false}
            }
            nextBefore=page["nextBefore"].string;failure=nil
        } catch {if request == query,crew == store.selectedCrew,!(error is CancellationError) {failure=describe(error)}}
    }
    private func withdraw(_ original:CrewdPlan,_ offer:CrewdValue) {
        guard !cancelling else {return};cancelling=true;failure=nil
        let crew=store.selectedCrew
        Task {
            defer{cancelling=false}
            var attempted=false
            do {
                let latest=try await store.detail(id)
                let current=latest.value["pendingTransfer"]
                guard store.selectedCrew == crew else {throw CancellationError()}
                guard latest.revision == original.revision,current != .null,current["to"] == offer["to"],current["requestSeq"] == offer["requestSeq"] else {throw CrewdClientError(message:"交接已变化，请核对当前请求后再决定。",code:-32009,data:.object(["refusal":.string("work.revision_conflict")]))}
                attempted=true
                _ = try await store.withdrawTransfer(id,expectedRevision:latest.revision)
                uncertain=false
                if historyExpanded {await load(reset:true)}
            } catch {
                if store.selectedCrew == crew {failure=describe(error);uncertain=attempted && (error as? CrewdClientError)?.code == nil}
            }
        }
    }
}
#endif
