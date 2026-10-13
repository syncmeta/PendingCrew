#if os(macOS)
import SwiftUI

/// Window-owned input snapshots only; ask state remains in the organization inbox.
struct CrewdHumanDrafts {
    struct Key:Hashable {let scope:String;let crew:String;let ask:String}
    enum Action:String,CaseIterable {case answer="答复",withdraw="撤回"}
    struct Fields:Equatable {var action=Action.answer;var text=""}
    struct Snapshot:Equatable {let key:Key;let fields:Fields;let version:UUID}
    private var entries:[Key:Snapshot]=[:]
    private var pending:[Key:UUID]=[:]
    private var unknown:[Key:[Snapshot]]=[:]
    func fields(_ key:Key)->Fields {entries[key]?.fields ?? Fields()}
    func busy(_ key:Key)->Bool {pending[key] != nil}
    func uncertain(_ key:Key)->[Snapshot] {unknown[key] ?? []}
    mutating func edit(_ key:Key,_ fields:Fields) {if self.fields(key) != fields {entries[key]=Snapshot(key:key,fields:fields,version:UUID())}}
    mutating func begin(_ key:Key)->Snapshot? {
        guard !busy(key),!fields(key).text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty else {return nil}
        if entries[key] == nil {return nil}
        let snapshot=entries[key]!;pending[key]=snapshot.version;return snapshot
    }
    mutating func finish(_ snapshot:Snapshot,accepted:Bool,uncertain:Bool=false) {
        if pending[snapshot.key] == snapshot.version {pending[snapshot.key]=nil}
        if uncertain && !(unknown[snapshot.key] ?? []).contains(snapshot) {unknown[snapshot.key,default:[]].append(snapshot)}
        if accepted {unknown[snapshot.key]?.removeAll {$0.version == snapshot.version}}
        if accepted && entries[snapshot.key] == snapshot {entries[snapshot.key]=nil}
    }
}
struct CrewdHumanTarget:Identifiable {
    let key:CrewdHumanDrafts.Key
    var crewTitle:String?=nil
    var id:String {key.scope+":"+key.crew+":"+key.ask}
}
struct CrewdHumanActions:View {
    @ObservedObject var store:CrewdWorkStore
    @Binding var drafts:CrewdHumanDrafts
    let target:CrewdHumanTarget
    @Environment(\.dismiss) private var dismiss
    @State private var failure:String?
    @State private var reading=false
    @State private var reviewedUnknown=false
    @State private var readUnknown=false
    private var key:CrewdHumanDrafts.Key {target.key}
    private var id:CrewdWorkID? {CrewdWorkID(key.ask,kind:.human)}
    private var ask:CrewdValue {id.flatMap{store.humanTasks[$0]}?["ask"] ?? .null}
    private var fields:CrewdHumanDrafts.Fields {drafts.fields(key)}
    private func binding<T>(_ path:WritableKeyPath<CrewdHumanDrafts.Fields,T>)->Binding<T> {Binding(get:{drafts.fields(key)[keyPath:path]},set:{var f=drafts.fields(key);f[keyPath:path]=$0;drafts.edit(key,f);reviewedUnknown=false;readUnknown=false})}
    var body:some View {
        VStack(alignment:.leading,spacing:12) {
            Text(key.ask+" · 待你处理").font(.title2)
            Text("所属机组："+(target.crewTitle ?? "机组资料尚未加载")).font(.caption).foregroundStyle(.secondary)
            Text(ask["question"].string ?? "此问题已不在当前待处理收件箱，请刷新核对。").textSelection(.enabled)
            if let item=ask["item"].string {Text("关联计划："+item).font(.caption)}
            if let original=ask["default"]["text"].string {Text("预设答复："+original).font(.caption).textSelection(.enabled)}
            ForEach(drafts.uncertain(key),id:\.version){snapshot in
                VStack(alignment:.leading,spacing:3){Text("此前提交的结果未确认，不会自动重发。").foregroundStyle(.orange);Text(snapshot.fields.action.rawValue+"："+snapshot.fields.text).textSelection(.enabled)}.font(.caption)
            }
            if !drafts.uncertain(key).isEmpty {
                Button("读取当前收件箱"){Task {await refresh()}}.disabled(reading || drafts.busy(key))
                Toggle("我已核对当前状态，确定要再次提交",isOn:$reviewedUnknown).disabled(!readUnknown || reading || drafts.busy(key))
            }
            Picker("操作",selection:binding(\.action)){ForEach(CrewdHumanDrafts.Action.allCases,id:\.self){Text($0.rawValue).tag($0)}}.pickerStyle(.segmented).disabled(drafts.busy(key))
            TextField(fields.action == .answer ? "你的答复":"撤回理由",text:binding(\.text),axis:.vertical).lineLimit(4...10).disabled(drafts.busy(key))
            if fields.action == .withdraw {Text("仍阻塞活跃计划的问题不能撤回；请先由负责人调整计划，后台会最终校验。").font(.caption).foregroundStyle(.secondary)}
            if let failure {Text(failure).foregroundStyle(.orange).textSelection(.enabled)}
            Spacer()
            HStack{Button("关闭"){dismiss()};Spacer();Button(drafts.busy(key) ? "提交中":"提交"){submit()}.disabled(drafts.busy(key) || reading || store.selectedCrew != key.scope || ask["state"] != .string("open") || fields.text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || (!drafts.uncertain(key).isEmpty && !reviewedUnknown))}
        }.padding(20).frame(width:540,height:480)
    }
    private func describe(_ error:Error)->String {
        guard let e=error as? CrewdClientError else {return error.localizedDescription}
        return [e.message,e.code.map{"错误 \($0)"},e.refusal].compactMap{$0}.joined(separator:" · ")
    }
    private func refresh() async {
        reading=true;defer{reading=false}
        do {try await store.refresh();if store.selectedCrew == key.scope {failure=nil;readUnknown=true;reviewedUnknown=false}}
        catch {if store.selectedCrew == key.scope {failure=describe(error)}}
    }
    private func submit() {
        guard let snapshot=drafts.begin(key),let id else {return}
        let original=ask
        Task {
            var attempted=false
            do {
                try await store.refresh()
                guard store.selectedCrew == key.scope else {throw CancellationError()}
                guard let current=store.humanTasks[id]?["ask"],current["crewId"] == .string(key.crew),current["question"] == original["question"],current["state"] == .string("open") else {throw CrewdClientError(message:"问题状态已变化，请核对收件箱后再决定。",code:-32009)}
                attempted=true
                _ = try await store.resolveHuman(id,crew:key.crew,text:snapshot.fields.text,withdraw:snapshot.fields.action == .withdraw)
                drafts.finish(snapshot,accepted:true)
                if store.selectedCrew == key.scope {dismiss()}
            } catch {
                drafts.finish(snapshot,accepted:false,uncertain:attempted && CrewdClientError.isUncertainSubmission(error))
                if store.selectedCrew == key.scope {reviewedUnknown=false;readUnknown=false;failure=describe(error)}
            }
        }
    }
}
#endif
