import Foundation
import SwiftUI

/// UI command drafts, never a work ledger or a copied status authority.
struct CrewdPlanDrafts {
    struct Key: Hashable {let crew:String;let object:String;let source:String;let channel:String
        init(crew:String,object:String,source:String,channel:String=""){self.crew=crew;self.object=object;self.source=source;self.channel=channel}
    }
    enum Action:String,CaseIterable {case report="回报",assign="指派",reassignLost="失联重派",complete="完成",drop="放弃",block="阻塞"}
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
    private var unknown:[Key:[UUID:Snapshot]]=[:]
    func fields(_ key:Key)->Fields {entries[key]?.fields ?? Fields()}
    func hasDraft(_ key:Key)->Bool {entries[key] != nil}
    func busy(_ key:Key)->Bool {pending[key] != nil}
    func uncertain(_ key:Key)->Bool {!(unknown[key] ?? [:]).isEmpty}
    func isCurrent(_ snapshot:Snapshot)->Bool {entries[snapshot.key]?.version == snapshot.version}
    mutating func confirm(_ snapshot:Snapshot)->Bool {
        guard !busy(snapshot.key),unknown[snapshot.key]?[snapshot.version] != nil else{return false}
        pending[snapshot.key]=snapshot.version;return true
    }
    func unknownSnapshots(_ key:Key)->[Snapshot] {(unknown[key] ?? [:]).values.sorted{$0.version.uuidString < $1.version.uuidString}}
    var allUnknownSnapshots:[Snapshot] {unknown.values.flatMap{$0.values}.sorted{$0.version.uuidString < $1.version.uuidString}}
    mutating func edit(_ key:Key,_ fields:Fields) {if entries[key]?.fields != fields {entries[key]=Entry(fields:fields,version:UUID())}}
    mutating func begin(_ key:Key)->Snapshot? {
        guard !busy(key) else {return nil}
        if entries[key] == nil {entries[key]=Entry(fields:Fields(),version:UUID())}
        let entry=entries[key]!;pending[key]=entry.version
        return Snapshot(key:key,fields:entry.fields,version:entry.version)
    }
    mutating func finish(_ snapshot:Snapshot,accepted:Bool,uncertain:Bool=false) {
        if pending[snapshot.key] == snapshot.version {pending[snapshot.key]=nil}
        if uncertain {unknown[snapshot.key,default:[:]][snapshot.version]=snapshot} else if accepted {unknown[snapshot.key]?[snapshot.version]=nil}
        if accepted && entries[snapshot.key]?.version == snapshot.version {entries[snapshot.key]=nil}
    }
}
struct CrewdPlanTarget:Identifiable {
    let key:CrewdPlanDrafts.Key
    var id:String {"\(key.crew)/\(key.object)/\(key.source)/\(key.channel)"}
}
/// Typed forms become ordinary human RPC commands. No raw JSON editor or inferred IDs.
enum CrewdPlanCommand {
    static func validateOwnership(_ fields:CrewdPlanDrafts.Fields,owner:String?) throws {
        guard fields.action == .block || (fields.action == .report && (fields.reportStatus == "active" || fields.checkpoint)) else {return}
        guard let owner,!owner.isEmpty else {
            throw CrewdClientError(message:"请先指派负责人，再设置检查点、推进中或阻塞状态。",kind:.notSubmitted)
        }
    }
    static func build(_ key:CrewdPlanDrafts.Key,_ f:CrewdPlanDrafts.Fields,revision:String?,requestID:UUID?=nil,current:CrewdValue?=nil) throws -> (String,CrewdValue) {
        func need(_ text:String,_ name:String)throws {if text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty {throw CrewdClientError(message:"请填写"+name)}}
        if key.object.isEmpty {
            try need(f.title,"计划标题")
            guard CrewdWorkID(key.source,kind:.requirement) != nil else {throw CrewdClientError(message:"计划必须关联公共需求编号。")}
            var p:[String:CrewdValue]=["crewId":.string(key.crew),"title":.string(f.title),"goal":.string(f.goal),"acceptance":.string(f.acceptance),"sources":.array([.string(key.source)])]
            if !key.channel.isEmpty {
                guard let requestID else{throw CrewdClientError(message:"群内创建必须保留原始请求编号。",kind:.notSubmitted)}
                p["channelId"] = .string(key.channel);p["requestId"] = .string(requestID.uuidString.lowercased())
            }
            if !f.owner.isEmpty {p["owner"] = .string(f.owner)}
            if !f.parent.isEmpty {p["parent"] = .string(f.parent)}
            return ("work.create",.object(p))
        }
        guard CrewdWorkID(key.object,kind:.plan) != nil,let revision,!revision.isEmpty else {throw CrewdClientError(message:"请先读取当前计划详情。")}
        var p:[String:CrewdValue]=["id":.string(key.object),"expectedRevision":.string(revision)]
        switch f.action {
        case .assign,.reassignLost:
            try need(f.owner,"真实负责人");try need(f.note,"交接说明")
            p["owner"] = .string(f.owner);p["note"] = .string(f.note);return (f.action == .reassignLost ? "work.reassignLost":"work.assign",.object(p))
        case .report:
            try need(f.note,"当前回报")
            guard ["open","active"].contains(f.reportStatus) else {throw CrewdClientError(message:"回报状态无效。")}
            guard f.reportStatus != "active" || f.checkpoint || current?["nextCheckAt"].string != nil else {throw CrewdClientError(message:"推进中的计划必须有下一检查点。",kind:.notSubmitted)}
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
    var crews:[CrewdValue]=[]
    @Environment(\.dismiss) private var dismiss
    @State private var revision:String?
    @State private var failure:String?
    @State private var reading=false
    @State private var reviewedUnknown=false
    private var key:CrewdPlanDrafts.Key {target.key}
    private var fields:CrewdPlanDrafts.Fields {drafts.fields(key)}
    private var currentHasOwner:Bool {
        guard let id=CrewdWorkID(key.object,kind:.plan),let owner=store.plans[id]?.value["owner"].string else{return false}
        return !owner.isEmpty
    }
    private var ownerRequired:Bool { !key.object.isEmpty && !currentHasOwner && (fields.action == .block || (fields.action == .report && (fields.reportStatus == "active" || fields.checkpoint))) }
    private var checkpointRequired:Bool {
        guard !key.object.isEmpty,fields.action == .report,fields.reportStatus == "active",!fields.checkpoint else{return false}
        guard let id=CrewdWorkID(key.object,kind:.plan) else{return true}
        return store.plans[id]?.value["nextCheckAt"].string == nil
    }
    private func binding<T>(_ path:WritableKeyPath<CrewdPlanDrafts.Fields,T>)->Binding<T> {Binding(get:{drafts.fields(key)[keyPath:path]},set:{var f=drafts.fields(key);f[keyPath:path]=$0;drafts.edit(key,f)})}
    var body:some View {
        VStack(alignment:.leading,spacing:12) {
            CrewdPanelHeader(title:key.object.isEmpty ? "新建计划":"更新计划")
            if !key.object.isEmpty {
                HStack {
                    Picker("操作",selection:binding(\.action)){ForEach(CrewdPlanDrafts.Action.allCases,id:\.self){Text($0.rawValue).tag($0)}}.pickerStyle(.menu).fixedSize()
                    Spacer()
                    if reading {ProgressView().controlSize(.small)}
                    Button {Task {await load()}} label:{Image(systemName:"arrow.clockwise")}
                        .buttonStyle(CrewdToolButtonStyle()).help("刷新计划").accessibilityLabel("刷新计划").disabled(reading || drafts.busy(key))
                }
                if let id=CrewdWorkID(key.object,kind:.plan),let current=store.plans[id] {
                    Text(current.value["title"].string ?? "").font(CrewdType.body.weight(.semibold)).textSelection(.enabled)
                    Text(CrewdWorkStatusLabel.display(current.status)+" · "+ownerName(current.value["owner"].string)).font(.caption).foregroundStyle(.secondary)
                    if !current.value["blockedOn"].array.isEmpty {Text("待答复："+current.value["blockedOn"].array.compactMap(\.string).joined(separator:" · ")).font(.caption).foregroundStyle(.secondary)}
                    if let note=current.value["note"].string {Text(note).font(.caption).textSelection(.enabled)}
                }
            }
            if let failure {Text(failure).foregroundStyle(.orange).textSelection(.enabled)}
            if drafts.uncertain(key) {
                Text("提交结果未确认。请先核对当前计划与关联事项；不会自动重复提交。以下原始提交内容保留供核对，当前编辑不会覆盖它。").font(.caption).foregroundStyle(.orange)
                DisclosureGroup("查看此前未确认的提交") {
                    ScrollView {
                        VStack(alignment:.leading,spacing:8) {
                ForEach(drafts.unknownSnapshots(key),id:\.version){snapshot in
                    VStack(alignment:.leading,spacing:3){
                        Text(snapshot.key.object.isEmpty ? "此前创建计划："+snapshot.fields.title:"此前操作："+snapshot.fields.action.rawValue).font(.caption).bold()
                        Text(unknownPayload(snapshot.fields)).font(.caption).textSelection(.enabled)
                        if snapshot.key.object.isEmpty,!snapshot.key.channel.isEmpty {
                            Button("确认这份原始群内创建请求") {if drafts.confirm(snapshot){submit(snapshot)}}
                                .disabled(drafts.busy(key) || !store.capabilities.contains("channelWork") || store.selectedCrew != key.crew)
                        }
                    }
                }
                        }.frame(maxWidth:.infinity,alignment:.leading)
                    }.frame(maxHeight:120)
                }.font(.caption)
                Button("读取当前状态") {Task {do {try await store.refresh();if !key.object.isEmpty {await load()}} catch {failure=describe(error)}}}
                Toggle("我已核对当前计划与关联事项，再继续提交",isOn:$reviewedUnknown).font(.caption)
            }
            if store.selectedCrew != key.crew {Text("当前已切换机组，请切回原机组后再提交。").foregroundStyle(.orange)}
            Divider()
            ScrollView {
                VStack(alignment:.leading,spacing:16) {
                    if key.object.isEmpty {
                        CrewdFormField(title:"计划标题"){TextField("概括这项工作",text:binding(\.title))}
                        CrewdFormField(title:"目标"){TextField("希望达成什么",text:binding(\.goal),axis:.vertical).lineLimit(2...5)}
                        CrewdFormField(title:"验收描述"){TextField("如何确认已经完成",text:binding(\.acceptance),axis:.vertical).lineLimit(2...5)}
                        ownerPicker
                        Picker("父计划",selection:binding(\.parent)) {
                            Text("无父计划").tag("")
                            ForEach(store.plans.values.filter{!$0.frozen && !["done","dropped"].contains($0.status)}.sorted(by:{$0.id.raw < $1.id.raw}),id:\.id){plan in Text("\(plan.id.raw) · \(plan.value["title"].string ?? "")").tag(plan.id.raw)}
                        }
                    } else {
                        switch fields.action {
                        case .assign,.reassignLost:
                            if fields.action == .reassignLost {
                                Text("仅用于负责人失联；正常调整请选择「指派」。").font(.caption).foregroundStyle(.secondary)
                                if !store.capabilities.contains("work.reassignLost") {Text("配套后台尚未开放此操作，当前不能提交。").font(.caption).foregroundStyle(.orange)}
                            }
                            ownerPicker;TextField("交接说明",text:binding(\.note),axis:.vertical).lineLimit(2...5)
                        case .report:
                            Picker("状态",selection:binding(\.reportStatus)){Text("待推进").tag("open");Text("推进中").tag("active").disabled(!currentHasOwner)}
                            if !currentHasOwner {Text("先指派负责人，才能推进或设置检查点。").font(.caption).foregroundStyle(.secondary)}
                            TextField("当前进展与下一步",text:binding(\.note),axis:.vertical).lineLimit(2...5)
                            Toggle("设置下一检查点",isOn:binding(\.checkpoint)).disabled(!currentHasOwner && !fields.checkpoint)
                            if checkpointRequired {Text("推进中需要下一检查点，请设置检查时间。").font(.caption).foregroundStyle(.orange)}
                            if fields.checkpoint {DatePicker("下次检查",selection:binding(\.nextCheck),displayedComponents:[.date,.hourAndMinute])}
                        case .complete:
                            TextField("完成说明（可选）",text:binding(\.note),axis:.vertical).lineLimit(2...5)
                            Picker("凭据类型（可选）",selection:binding(\.evidenceKind)){Text("文字").tag("text");Text("提交").tag("commit");Text("链接").tag("url")}
                            TextField("凭据（可选）",text:binding(\.evidence),axis:.vertical).lineLimit(2...5)
                        case .drop:TextField("为什么放弃这项计划",text:binding(\.note),axis:.vertical).lineLimit(2...5)
                        case .block:
                            if !currentHasOwner {Text("请先指派负责人，再将计划设为阻塞。").font(.caption).foregroundStyle(.orange)}
                            TextField("需要人类处理什么？",text:binding(\.question),axis:.vertical).lineLimit(2...5)
                            Picker("其余工作能否继续",selection:binding(\.split)){Text("其余不能继续").tag(false);Text("拆出仍可推进的部分").tag(true)}
                            if fields.split {TextField("可独立推进的部分",text:binding(\.splitTitle),axis:.vertical).lineLimit(2...5)}
                        }
                    }
                }.disabled(drafts.busy(key)).padding(.vertical,12)
            }
            Divider()
            HStack {Button("关闭"){dismiss()};Spacer();Button(drafts.busy(key) ? "提交中":key.object.isEmpty ? "创建计划":fields.action.rawValue){submit()}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(drafts.busy(key) || reading || ownerRequired || checkpointRequired || (fields.action == .reassignLost && !store.capabilities.contains("work.reassignLost")) || store.selectedCrew != key.crew || (!key.object.isEmpty && revision == nil) || (drafts.uncertain(key) && !reviewedUnknown))}
        }.padding(20).textFieldStyle(.roundedBorder).font(CrewdType.body).modifier(CrewdAdaptiveFormFrame(width:560,height:640)).task {if !key.object.isEmpty {await load()}}
    }
    private var ownerPicker:some View {
        VStack(alignment:.leading,spacing:4) {
            Picker("负责人",selection:binding(\.owner)) {
                Text("暂不指派").tag("")
                ForEach(members.indices,id:\.self){i in let m=members[i];Text(m["displayName"].string ?? m["id"].string ?? "成员").tag(m["id"].string ?? "")}
            }
        }
    }
    private func ownerName(_ id:String?)->String {
        guard let id,!id.isEmpty else{return "未指派"}
        return members.first(where:{$0["id"] == .string(id)})?["displayName"].string ?? id
    }
    private func unknownPayload(_ f:CrewdPlanDrafts.Fields)->String {
        var lines:[String]=[]
        for (label,value) in [("目标",f.goal),("验收",f.acceptance),("负责人",f.owner),("父计划",f.parent),("说明",f.note),("凭据",f.evidence),("问题",f.question),("可推进部分",f.splitTitle)] {
            if !value.isEmpty {lines.append(label+"："+value)}
        }
        if !key.object.isEmpty,f.action == .report {lines.append("状态："+CrewdWorkStatusLabel.display(f.reportStatus));if f.checkpoint {lines.append("检查点："+f.nextCheck.formatted())}}
        if !key.object.isEmpty,f.action == .block {lines.append(f.split ? "拆出独立推进部分":"其余暂不能推进")}
        return lines.joined(separator:"\n")
    }
    private func load() async {
        guard let id=CrewdWorkID(key.object,kind:.plan) else {return}
        reading=true;defer{reading=false}
        do {
            let detail=try await store.detail(id)
            if !drafts.hasDraft(key),detail.value["owner"].string == nil {
                var initial=fields;initial.reportStatus="open";initial.checkpoint=false;drafts.edit(key,initial)
            }
            revision=detail.revision;failure=nil
        }
        catch {failure=describe(error)}
    }
    private func describe(_ error:Error)->String {
        guard let e=error as? CrewdClientError else {return error.localizedDescription}
        let current=e.currentWork["revision"].string.map {"后台当前版本："+$0}
        return [e.message,e.code.map{"错误 \($0)"},e.refusal,current].compactMap{$0}.joined(separator:" · ")
    }
    private func submit(_ original:CrewdPlanDrafts.Snapshot?=nil) {
        guard let snapshot=original ?? drafts.begin(key) else {return}
        let expected=revision
        Task {
            guard store.selectedCrew == key.crew else {drafts.finish(snapshot,accepted:false);return}
            var attempted = false
            var verifiedCurrent:CrewdValue?
            do {
                if !key.object.isEmpty,let id=CrewdWorkID(key.object,kind:.plan) {
                    let latest=try await store.detail(id)
                    guard latest.revision == expected else {throw CrewdClientError(message:"计划已变化，请核对当前详情后再次决定。",code:-32009,data:.object(["refusal":.string("work.revision_conflict")]))}
                    guard !latest.frozen else {throw CrewdClientError(message:"归档计划只读。",code:-32009)}
                    try CrewdPlanCommand.validateOwnership(snapshot.fields,owner:latest.value["owner"].string)
                    verifiedCurrent=latest.value
                }
                let (method,params)=try CrewdPlanCommand.build(key,snapshot.fields,revision:expected,requestID:snapshot.version,current:verifiedCurrent)
                guard store.selectedCrew == key.crew else {throw CancellationError()}
                attempted = true
                let receipt = try await store.mutate(method,params)
                let wasCurrent=drafts.isCurrent(snapshot)
                drafts.finish(snapshot,accepted:true)
                if store.selectedCrew == key.crew {
                    if receipt["channelAssociation"]["linked"] == .bool(false) {failure="原计划已确认，但此前群关联已解除；确认原请求不会重新关联。"}
                    else if wasCurrent {dismiss()} else {failure="原提交已确认，当前编辑草稿仍保留。"}
                }
            } catch {
                drafts.finish(snapshot,accepted:false,uncertain:attempted && CrewdClientError.isUncertainSubmission(error))
                if attempted && CrewdClientError.isUncertainSubmission(error) {reviewedUnknown=false}
                failure=describe(error)
            }
        }
    }
}

/// Mac keeps the original form sizes; mobile follows the available sheet width.
/// This changes layout only, not command fields or draft ownership.
struct CrewdAdaptiveFormFrame: ViewModifier {
    let width: CGFloat
    var height: CGFloat? = nil
    var scrolls=true
    func body(content: Content) -> some View {
        #if os(macOS)
        content.frame(minWidth:min(width,440),idealWidth:width,maxWidth:width,
                      minHeight:height.map{min($0,360)},idealHeight:height,maxHeight:height)
        #else
        if scrolls {
            ScrollView { content.frame(maxWidth:.infinity,alignment:.topLeading) }
                .frame(maxWidth:.infinity,maxHeight:.infinity)
        }else {content.frame(maxWidth:.infinity,maxHeight:.infinity,alignment:.topLeading)}
        #endif
    }
}
