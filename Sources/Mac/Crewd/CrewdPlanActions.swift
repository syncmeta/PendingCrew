#if os(macOS)
import Foundation
import SwiftUI

/// UI command drafts, never a work ledger or a copied status authority.
struct CrewdPlanDrafts {
    struct Key: Hashable {let crew:String;let object:String;let source:String}
    enum Action:String,CaseIterable {case report="回报",assign="指派",complete="完成",drop="放弃",block="阻塞"}
    struct Fields: Equatable {
        var action=Action.report
        var title="",goal="",acceptance="",owner="",parent="",note="",evidence="",question="",splitTitle=""
        var reportStatus="active",evidenceKind="text"
        var checkpoint=true,split=false
        var nextCheck=Date().addingTimeInterval(15*60)
    }
    struct Snapshot {let key:Key;let fields:Fields;let version:UUID}
    private struct Entry {var fields:Fields;var version:UUID}
    private var entries:[Key:Entry]=[:]
    private var pending:[Key:UUID]=[:]
    private var unknown:[Key:Set<UUID>]=[:]
    func fields(_ key:Key)->Fields {entries[key]?.fields ?? Fields()}
    func busy(_ key:Key)->Bool {pending[key] != nil}
    func uncertain(_ key:Key)->Bool {!(unknown[key] ?? []).isEmpty}
    mutating func edit(_ key:Key,_ fields:Fields) {if entries[key]?.fields != fields {entries[key]=Entry(fields:fields,version:UUID())}}
    mutating func begin(_ key:Key)->Snapshot? {
        guard !busy(key) else {return nil}
        if entries[key] == nil {entries[key]=Entry(fields:Fields(),version:UUID())}
        let entry=entries[key]!;pending[key]=entry.version
        return Snapshot(key:key,fields:entry.fields,version:entry.version)
    }
    mutating func finish(_ snapshot:Snapshot,accepted:Bool,uncertain:Bool=false) {
        if pending[snapshot.key] == snapshot.version {pending[snapshot.key]=nil}
        if uncertain {unknown[snapshot.key,default:[]].insert(snapshot.version)} else if accepted {unknown[snapshot.key]?.remove(snapshot.version)}
        if accepted && entries[snapshot.key]?.version == snapshot.version {entries[snapshot.key]=nil}
    }
}
struct CrewdPlanTarget:Identifiable {
    let key:CrewdPlanDrafts.Key
    var id:String {"\(key.crew)/\(key.object)/\(key.source)"}
}
/// Typed forms become ordinary human RPC commands. No raw JSON editor or inferred IDs.
enum CrewdPlanCommand {
    static func build(_ key:CrewdPlanDrafts.Key,_ f:CrewdPlanDrafts.Fields,revision:String?) throws -> (String,CrewdValue) {
        func need(_ text:String,_ name:String)throws {if text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty {throw CrewdClientError(message:"请填写"+name)}}
        if key.object.isEmpty {
            try need(f.title,"计划标题")
            guard CrewdWorkID(key.source,kind:.requirement) != nil else {throw CrewdClientError(message:"计划必须关联公共需求编号。")}
            var p:[String:CrewdValue]=["crewId":.string(key.crew),"title":.string(f.title),"goal":.string(f.goal),"acceptance":.string(f.acceptance),"sources":.array([.string(key.source)])]
            if !f.owner.isEmpty {p["owner"] = .string(f.owner)}
            if !f.parent.isEmpty {p["parent"] = .string(f.parent)}
            return ("work.create",.object(p))
        }
        guard CrewdWorkID(key.object,kind:.plan) != nil,let revision,!revision.isEmpty else {throw CrewdClientError(message:"请先读取当前计划详情。")}
        var p:[String:CrewdValue]=["id":.string(key.object),"expectedRevision":.string(revision)]
        switch f.action {
        case .assign:
            try need(f.owner,"真实负责人");try need(f.note,"交接说明")
            p["owner"] = .string(f.owner);p["note"] = .string(f.note);return ("work.assign",.object(p))
        case .report:
            try need(f.note,"当前回报")
            guard ["open","active"].contains(f.reportStatus) else {throw CrewdClientError(message:"回报状态无效。")}
            guard f.reportStatus != "active" || f.checkpoint else {throw CrewdClientError(message:"推进中的计划必须设置下一检查点。")}
            p["status"] = .string(f.reportStatus);p["note"] = .string(f.note)
            if f.checkpoint {p["nextCheckAt"] = .string(ISO8601DateFormatter().string(from:f.nextCheck))}
        case .complete:
            p["status"] = .string("done")
            if !f.note.isEmpty {p["note"] = .string(f.note)}
            if !f.evidence.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty {p["evidence"] = .object(["kind":.string(f.evidenceKind),"value":.string(f.evidence)])}
        case .drop:
            try need(f.note,"放弃理由");p["status"] = .string("dropped");p["note"] = .string(f.note)
        case .block:
            try need(f.question,"需要人类处理的问题")
            p["on"] = .array([.object(["type":.string("new_human"),"question":.string(f.question)])])
            if f.split {
                try need(f.splitTitle,"仍可推进的部分")
                p["canContinue"] = .object(["type":.string("split"),"items":.array([.object(["crewId":.string(key.crew),"title":.string(f.splitTitle)])])])
            } else {p["canContinue"] = .object(["type":.string("nothing")])}
            return ("work.block",.object(p))
        }
        return ("work.update",.object(p))
    }
}
struct CrewdPlanActions:View {
    @ObservedObject var store:CrewdWorkStore
    @Binding var drafts:CrewdPlanDrafts
    let target:CrewdPlanTarget
    let members:[CrewdValue]
    @Environment(\.dismiss) private var dismiss
    @State private var revision:String?
    @State private var failure:String?
    @State private var reading=false
    @State private var reviewedUnknown=false
    private var key:CrewdPlanDrafts.Key {target.key}
    private var fields:CrewdPlanDrafts.Fields {drafts.fields(key)}
    private func binding<T>(_ path:WritableKeyPath<CrewdPlanDrafts.Fields,T>)->Binding<T> {Binding(get:{drafts.fields(key)[keyPath:path]},set:{var f=drafts.fields(key);f[keyPath:path]=$0;drafts.edit(key,f)})}
    var body:some View {
        VStack(alignment:.leading,spacing:12) {
            Text(key.object.isEmpty ? "从 \(key.source) 建计划":"\(key.object) · 计划操作").font(.title2)
            Text("机组：\(key.crew)").font(.caption).foregroundStyle(.secondary)
            if !key.object.isEmpty {
                Picker("操作",selection:binding(\.action)){ForEach(CrewdPlanDrafts.Action.allCases,id:\.self){Text($0.rawValue).tag($0)}}.pickerStyle(.segmented)
                HStack {Text("已读取版本："+(revision ?? "尚未读取")).font(.caption);Spacer();Button("核对当前详情"){Task {await load()}}.disabled(reading || drafts.busy(key))}
                if let id=CrewdWorkID(key.object,kind:.plan),let current=store.plans[id] {
                    Text(current.value["title"].string ?? "").font(.headline)
                    Text("当前："+CrewdWorkStatusLabel.display(current.status)+" · "+(current.value["owner"].string ?? "未指派")).font(.caption).foregroundStyle(.secondary)
                    Text("卡点："+current.value["blockedOn"].array.compactMap(\.string).joined(separator:" · ")).font(.caption).foregroundStyle(.secondary)
                    if let note=current.value["note"].string {Text(note).font(.caption).textSelection(.enabled)}
                }
            }
            if let failure {Text(failure).foregroundStyle(.orange).textSelection(.enabled)}
            if drafts.uncertain(key) {
                Text("提交结果未确认。请先核对当前计划与关联事项；不会自动重复提交。原草稿仍保留。").font(.caption).foregroundStyle(.orange)
                Button("读取当前状态") {Task {do {try await store.refresh();if !key.object.isEmpty {await load()}} catch {failure=describe(error)}}}
                Toggle("我已核对当前计划与关联事项，再继续提交",isOn:$reviewedUnknown).font(.caption)
            }
            if store.selectedCrew != key.crew {Text("当前已切换机组，请切回原机组后再提交。").foregroundStyle(.orange)}
            ScrollView {
                VStack(alignment:.leading,spacing:12) {
                    if key.object.isEmpty {
                        TextField("计划标题",text:binding(\.title));TextField("目标",text:binding(\.goal),axis:.vertical).lineLimit(2...5)
                        TextField("验收描述",text:binding(\.acceptance),axis:.vertical).lineLimit(2...5)
                        ownerPicker
                        Picker("父计划",selection:binding(\.parent)) {
                            Text("无父计划").tag("")
                            ForEach(store.plans.values.filter{!$0.frozen && !["done","dropped"].contains($0.status)}.sorted(by:{$0.id.raw < $1.id.raw}),id:\.id){plan in Text("\(plan.id.raw) · \(plan.value["title"].string ?? "")").tag(plan.id.raw)}
                        }
                        Text("父计划选项来自已读取的实际计划；层级与跨机组权限由后台校验。").font(.caption).foregroundStyle(.secondary)
                    } else {
                        switch fields.action {
                        case .assign:ownerPicker;TextField("交接说明",text:binding(\.note),axis:.vertical).lineLimit(2...5)
                        case .report:
                            Picker("状态",selection:binding(\.reportStatus)){Text("待推进").tag("open");Text("推进中").tag("active")}
                            TextField("当前进展与下一步",text:binding(\.note),axis:.vertical).lineLimit(2...5)
                            Toggle("设置下一检查点",isOn:binding(\.checkpoint))
                            if fields.checkpoint {DatePicker("下次检查",selection:binding(\.nextCheck),displayedComponents:[.date,.hourAndMinute])}
                        case .complete:
                            TextField("完成说明（可选）",text:binding(\.note),axis:.vertical).lineLimit(2...5)
                            Picker("凭据类型（可选）",selection:binding(\.evidenceKind)){Text("文字").tag("text");Text("提交").tag("commit");Text("链接").tag("url")}
                            TextField("凭据（可选）",text:binding(\.evidence),axis:.vertical).lineLimit(2...5)
                        case .drop:TextField("为什么放弃这项计划",text:binding(\.note),axis:.vertical).lineLimit(2...5)
                        case .block:
                            TextField("需要人类处理什么？",text:binding(\.question),axis:.vertical).lineLimit(2...5)
                            Picker("其余工作能否继续",selection:binding(\.split)){Text("其余不能继续").tag(false);Text("拆出仍可推进的部分").tag(true)}
                            if fields.split {TextField("可独立推进的部分",text:binding(\.splitTitle),axis:.vertical).lineLimit(2...5)}
                        }
                    }
                }.disabled(drafts.busy(key))
            }
            HStack {Button("关闭"){dismiss()};Spacer();Button(drafts.busy(key) ? "提交中":"提交"){submit()}.disabled(drafts.busy(key) || reading || store.selectedCrew != key.crew || (!key.object.isEmpty && revision == nil) || (drafts.uncertain(key) && !reviewedUnknown))}
        }.padding(20).frame(width:560,height:560).task {if !key.object.isEmpty {await load()}}
    }
    private var ownerPicker:some View {
        VStack(alignment:.leading,spacing:4) {
            Picker("负责人",selection:binding(\.owner)) {
                Text("暂不指派").tag("")
                ForEach(members.indices,id:\.self){i in let m=members[i];Text((m["displayName"].string ?? m["id"].string ?? "成员")+" · "+(m["crewId"].string ?? "")).tag(m["id"].string ?? "")}
            }
            Text("已加载的真实成员，不代表完整组织名册。").font(.caption).foregroundStyle(.secondary)
        }
    }
    private func load() async {
        guard let id=CrewdWorkID(key.object,kind:.plan) else {return}
        reading=true;defer{reading=false}
        do {let detail=try await store.detail(id);revision=detail.revision;failure=nil}
        catch {failure=describe(error)}
    }
    private func describe(_ error:Error)->String {
        guard let e=error as? CrewdClientError else {return error.localizedDescription}
        let current=e.currentWork["revision"].string.map {"后台当前版本："+$0}
        return [e.message,e.code.map{"错误 \($0)"},e.refusal,current].compactMap{$0}.joined(separator:" · ")
    }
    private func submit() {
        guard let snapshot=drafts.begin(key) else {return}
        let expected=revision
        Task {
            guard store.selectedCrew == key.crew else {drafts.finish(snapshot,accepted:false);return}
            var attempted = false
            do {
                if !key.object.isEmpty,let id=CrewdWorkID(key.object,kind:.plan) {
                    let latest=try await store.detail(id)
                    guard latest.revision == expected else {throw CrewdClientError(message:"计划已变化，请核对当前详情后再次决定。",code:-32009,data:.object(["refusal":.string("work.revision_conflict")]))}
                    guard !latest.frozen else {throw CrewdClientError(message:"归档计划只读。",code:-32009)}
                }
                let (method,params)=try CrewdPlanCommand.build(key,snapshot.fields,revision:expected)
                guard store.selectedCrew == key.crew else {throw CancellationError()}
                attempted = true
                _ = try await store.mutate(method,params)
                drafts.finish(snapshot,accepted:true);if store.selectedCrew == key.crew {dismiss()}
            } catch {
                drafts.finish(snapshot,accepted:false,uncertain:attempted && CrewdClientError.isUncertainSubmission(error))
                if attempted && CrewdClientError.isUncertainSubmission(error) {reviewedUnknown=false}
                failure=describe(error)
            }
        }
    }
}
#endif
