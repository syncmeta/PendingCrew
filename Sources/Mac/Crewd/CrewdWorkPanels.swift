import SwiftUI

enum CrewdWorkPanelDomain:Equatable { case all, requirements, human }

struct CrewdWorkPanels: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var store: CrewdWorkStore
    @ObservedObject var resources:CrewdResources
    @Binding var resourceDrafts:CrewdResourceDrafts
    @Binding var drafts: CrewdRequirementDrafts
    @Binding var planDrafts: CrewdPlanDrafts
    @Binding var humanDrafts: CrewdHumanDrafts
    var members:[CrewdValue]
    var crews:[CrewdValue]=[]
    var domain = CrewdWorkPanelDomain.all
    var humanCrewOnly = false
    var progressOnly = false
    var compact = false
    var openTodo: (() -> Void)? = nil
    var visiblePlanIDs: Set<String>? = nil
    private var visibleProgress: [CrewdPlan] {
        guard let visiblePlanIDs else { return store.progress }
        return visiblePlanIDs.sorted().compactMap { raw in
            guard let id = CrewdWorkID(raw, kind: .plan) else { return nil }
            return store.plans[id]
        }
    }
    @State private var collapsed:Set<String>=[]
    @State private var selected:String?
    @State private var failure:String?
    @State private var actionTarget: CrewdPlanTarget?
    @State private var humanTarget:CrewdHumanTarget?
    @State private var selectedRequirement: CrewdWorkID?
    @State private var closing:Set<CrewdWorkID>=[]
    @State private var closeNotes:[CrewdWorkID:String]=[:]
    private var folderView:CrewdResourceView{progressOnly ? .progress:.todo}
    private var folderSelection:Binding<String>{Binding(get:{resourceDrafts.selection(resources.crew,folderView,channel:resources.channel)},set:{resourceDrafts.select(resources.crew,folderView,$0,channel:resources.channel)})}
    private func includes(_ kind:String,_ id:String)->Bool {compact || CrewdFolderFilter.includes(CrewdFolderFilter.object(kind,id),selection:folderSelection.wrappedValue,snapshot:resources.folders[folderView])}
    var body: some View {
        VStack(alignment:.leading,spacing:12) {
            if !compact {
                HStack {
                    CrewdFolderControls(store:resources,drafts:$resourceDrafts,selection:folderSelection,view:folderView)
                    Spacer(); Button("刷新",systemImage:"arrow.clockwise") {Task {do {try await store.refresh()} catch {failure=error.localizedDescription}}}
                }
            }
            if let error=store.lastFailure { Text(error.localizedDescription).font(.caption).foregroundStyle(.orange).textSelection(.enabled) }
            if let error=resources.failure {Text(error.localizedDescription).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
            if let failure { Text(failure).foregroundStyle(.orange).textSelection(.enabled) }
            if store.loading {ProgressView().controlSize(.small)}
            ScrollView {
                VStack(alignment:.leading,spacing:10) {
                    if progressOnly && !compact && !visibleProgress.isEmpty {
                        HStack {
                            Text("工作计划").font(.system(size: 14, weight: .semibold))
                            Spacer()
                            Button("全部展开") { collapsed.removeAll() }
                            Button("收起层级") { collapsed = Set(visibleProgress.map { $0.id.raw }) }
                        }.font(.caption).padding(.bottom, 4)
                    }
                    if progressOnly && visibleProgress.isEmpty && !store.loading && store.lastFailure == nil {
                        CrewdEmptySurface(title: "还没有计划", detail: "从 AI 待办新建计划，在这里跟进进展。",
                                          symbol: "list.bullet.indent", actionTitle: openTodo == nil ? nil : "打开 AI 待办", action: openTodo)
                            .frame(minHeight: 200)
                    }
                    if !progressOnly && !compact && domain != .human {
                        Text("原始需求").font(.system(size: 14, weight: .semibold))
                        requirementComposer
                        ForEach(store.requirementIDs.filter{includes("requirement",$0.raw)},id:\.self) {id in
                            VStack(alignment:.leading,spacing:4) {
                                Button {
                                    selectedRequirement = selectedRequirement == id ? nil : id
                                    if selectedRequirement != nil {Task {do {_ = try await store.requirementDetail(id)} catch {failure = describe(error)}}}
                                } label: {
                                    VStack(alignment: .leading, spacing: 5) {
                                        Text(store.requirements[id]?["text"].string ?? "需求").font(CrewdType.body.weight(.medium)).multilineTextAlignment(.leading)
                                    }
                                }.buttonStyle(.plain)
                                HStack {CrewdFolderAssignmentButton(store:resources,drafts:$resourceDrafts,view:.todo,object:CrewdFolderFilter.object("requirement",id.raw));Button("从需求建计划") {actionTarget=CrewdPlanTarget(key:.init(crew:store.selectedCrew,object:"",source:id.raw))};Spacer()}
                                if selectedRequirement == id {requirementDetail(id)}
                                Text(CrewdWorkStatusLabel.display(store.requirements[id]?["status"].string ?? "")).font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                            }.padding(12).frame(maxWidth:.infinity,alignment:.leading)
                                .background(CrewdChatStyle.bubble(colorScheme),in:RoundedRectangle(cornerRadius:10))
                                .overlay(RoundedRectangle(cornerRadius:10).stroke(CrewdChatStyle.line(colorScheme),lineWidth:0.6))
                        }
                        if domain == .all{Text("计划").font(.headline)}
                    }
                    if domain == .all {
                    ForEach(CrewdNavigation.planTree(visibleProgress.filter{includes("work",$0.id.raw)},collapsed:collapsed)) {row in
                        if let id=CrewdWorkID(row.object,kind:.plan),let plan=store.plans[id] {
                            VStack(alignment:.leading,spacing:10) {
                                HStack(alignment:.top) {
                                    if row.expandable {Button {if !collapsed.insert(row.id).inserted {collapsed.remove(row.id)}} label:{Image(systemName:collapsed.contains(row.id) ? "chevron.right":"chevron.down")}.buttonStyle(.plain)}
                                    Image(systemName:plan.status == "done" ? "checkmark.circle.fill":plan.status == "blocked" ? "questionmark.circle":"circle").foregroundStyle(CrewdChatStyle.muted(colorScheme))
                                    Button { selected = selected == id.raw ? nil : id.raw; if selected != nil && !compact {Task {do {_ = try await store.detail(id)} catch {failure=error.localizedDescription}}} } label: {
                                        VStack(alignment: .leading, spacing: 4) {
                                            Text(plan.value["title"].string ?? "计划").font(CrewdType.body.weight(.medium)).multilineTextAlignment(.leading)
                                        }
                                    }.buttonStyle(.plain)
                                    Spacer()
                                    Text(CrewdWorkStatusLabel.display(plan.status)).font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                                    if plan.value["pendingTransfer"] != .null {Text("交接中").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))}
                                }
                                if !compact {
                                    VStack(alignment:.leading,spacing:5){
                                        if let owner=plan.value["owner"].string {
                                            Label(members.first(where:{$0["id"] == .string(owner)})?["displayName"].string ?? owner,systemImage:"person").font(CrewdType.caption)
                                        }
                                        if selected != id.raw,let note=plan.value["note"].string,!note.isEmpty{Text(note).font(CrewdType.label).lineLimit(2)}
                                        if let date=plan.value["nextCheckAt"].string{Label("下次检查 · "+checkpointLabel(date),systemImage:"clock").font(CrewdType.caption)}
                                    }.foregroundStyle(CrewdChatStyle.muted(colorScheme))
                                    if let parent=plan.value["parent"].string,row.depth == 0 {Text("上级：\(parent)").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))}
                                }
                                if selected == id.raw, !compact {
                                    if let detail=plan.detail {
                                        Divider()
                                        if let goal=detail["goal"].string,!goal.isEmpty {Text(goal).font(CrewdType.body).textSelection(.enabled)}
                                        if let acceptance=detail["acceptance"].string,!acceptance.isEmpty {Text("验收："+acceptance).font(CrewdType.label).textSelection(.enabled)}
                                        if let note=detail["note"].string,!note.isEmpty {Text(note).font(CrewdType.label).textSelection(.enabled)}
                                        if !detail["dependsOn"].array.isEmpty {Text("依赖：" + detail["dependsOn"].array.compactMap(\.string).joined(separator:" · ")).font(.caption)}
                                        if !detail["blockedOn"].array.isEmpty {Text("待答复：" + detail["blockedOn"].array.compactMap(\.string).joined(separator:" · ")).font(.caption)}
                                        if let evidence=detail["evidence"]["value"].string {Text(evidence).textSelection(.enabled)}
                                        CrewdFolderAssignmentButton(store:resources,drafts:$resourceDrafts,view:folderView,object:CrewdFolderFilter.object("work",id.raw))
                                        CrewdHandoffView(store:store,id:id,members:members).id(store.selectedCrew + ":" + id.raw)
                                        Button("计划操作") {actionTarget=CrewdPlanTarget(key:.init(crew:store.selectedCrew,object:id.raw,source:""))}.disabled(plan.frozen || ["done","dropped"].contains(plan.status))
                                    } else {Button("读取当前详情") {Task {do {_ = try await store.detail(id)} catch {failure=describe(error)}}}}

                                }
                            }.padding(16).frame(maxWidth: .infinity, alignment: .leading)
                                .background(CrewdChatStyle.bubble(colorScheme),in:RoundedRectangle(cornerRadius:10))
                                .overlay(RoundedRectangle(cornerRadius:10).stroke(CrewdChatStyle.line(colorScheme),lineWidth:0.6))
                                .padding(.leading,CGFloat(min(row.depth,3))*12)
                        }
                    }
                    }
                    if compact {
                        ForEach(store.humanTasks.keys.filter {store.humanTasks[$0]?["ask"]["crewId"].string == store.selectedCrew}.sorted(by:{$0.raw < $1.raw}),id:\.self) {id in
                            HStack(alignment:.top) {Image(systemName:"questionmark.circle"); Text("\(id.raw)  \(store.humanTasks[id]?["ask"]["question"].string ?? "")").font(.caption).lineLimit(2)}
                        }
                    }
                    if domain == .all && visibleProgress.isEmpty && !progressOnly {Text("尚无计划").foregroundStyle(CrewdChatStyle.muted(colorScheme))}
                    if !progressOnly && !compact && domain != .requirements {
                        Divider(); Text(humanCrewOnly ? "本机组待人处理":"所有可访问机组 · 待人处理").font(.headline)
                        ForEach(store.humanTasks.keys.filter{(!humanCrewOnly || store.humanTasks[$0]?["ask"]["crewId"].string == store.selectedCrew) && (store.humanTasks[$0]?["ask"]["crewId"].string != resources.crew || includes("humanTask",$0.raw))}.sorted(by:{$0.raw < $1.raw}),id:\.self) {id in
                            let ask=store.humanTasks[id]?["ask"] ?? .null
                            Text("\(id.raw)  \(ask["question"].string ?? "")").textSelection(.enabled)
                            Text(ask["item"].string.map{"关联计划："+$0} ?? ("所属机组："+(crews.first(where:{$0["id"] == ask["crewId"]})?["title"].string ?? "机组资料尚未加载"))).font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                            if let crew=ask["crewId"].string {if crew == resources.crew{CrewdFolderAssignmentButton(store:resources,drafts:$resourceDrafts,view:.todo,object:CrewdFolderFilter.object("humanTask",id.raw))};Button("答复 / 撤回"){humanTarget=CrewdHumanTarget(key:.init(scope:store.selectedCrew,crew:crew,ask:id.raw),crewTitle:crews.first(where:{$0["id"].string == crew})?["title"].string)}}
                        }
                    }
                }.frame(maxWidth:.infinity,alignment:.leading)
            }
        }.padding(compact ? 8:16).foregroundStyle(CrewdChatStyle.ink(colorScheme))
        .sheet(item:$humanTarget){target in CrewdHumanActions(store:store,drafts:$humanDrafts,target:target)}
        .sheet(item:$actionTarget) {target in CrewdPlanActions(store:store,drafts:$planDrafts,target:target,members:members,crews:crews)}
    }
    private func checkpointLabel(_ raw:String)->String {
        let parser=ISO8601DateFormatter()
        parser.formatOptions=[.withInternetDateTime,.withFractionalSeconds]
        let precise=parser.date(from:raw)
        parser.formatOptions=[.withInternetDateTime]
        guard let date=precise ?? parser.date(from:raw) else{return raw}
        return date.formatted(date:.abbreviated,time:.shortened)
    }
    private func describe(_ error:Error)->String {
        guard let e=error as? CrewdClientError else {return error.localizedDescription}
        return [e.message,e.code.map {"错误 \($0)"},e.refusal].compactMap{$0}.joined(separator:" · ")
    }
    private var requirementComposer: some View {
        let crew=store.selectedCrew
        return VStack(alignment:.leading,spacing:6) {
            ForEach(drafts.uncertain(crew),id:\.requestID) {snapshot in
                VStack(alignment:.leading,spacing:4) {
                    Text("创建结果未确认，请核对原提交后再继续。").font(.caption).foregroundStyle(.orange)
                    Text(snapshot.text).font(.caption).lineLimit(3).textSelection(.enabled)
                    Button("确认这份原始提交") {
                        guard drafts.confirm(snapshot) else {return}
                        Task {
                            guard store.selectedCrew == snapshot.crew else {drafts.finish(snapshot,accepted:false);return}
                            do {
                                _ = try await store.createRequirement(requestID:snapshot.requestID,text:snapshot.text)
                                drafts.finish(snapshot,accepted:true)
                            } catch {
                                drafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error))
                                if store.selectedCrew == snapshot.crew {failure=describe(error)}
                            }
                        }
                    }.disabled(drafts.busy(crew))
                }.padding(.vertical,5)
            }
            TextField("写下原始需求",text:Binding(get:{drafts.text(crew)},set:{drafts.set(crew,text:$0)}),axis:.vertical).lineLimit(2...5)
                .disabled(drafts.busy(crew))
            HStack {
                Spacer()
                Button(drafts.busy(crew) ? "提交中":"新增需求") {
                    guard let snapshot=drafts.begin(crew) else {return}
                    failure=nil
                    Task {
                        guard store.selectedCrew == snapshot.crew else {drafts.finish(snapshot,accepted:false);return}
                        do {
                            _ = try await store.createRequirement(requestID:snapshot.requestID,text:snapshot.text)
                            drafts.finish(snapshot,accepted:true)
                        } catch {
                            drafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error))
                            if store.selectedCrew == snapshot.crew {failure=describe(error)}
                        }
                    }
                }.disabled(crew.isEmpty || drafts.busy(crew) || drafts.text(crew).trimmingCharacters(in:.whitespacesAndNewlines).isEmpty)
            }
        }.padding(8).background(CrewdChatStyle.bubble(colorScheme),in:RoundedRectangle(cornerRadius:6))
    }
    @ViewBuilder private func requirementDetail(_ id:CrewdWorkID)->some View {
        let raw=store.requirements[id] ?? .null
        let status=raw["status"].string ?? "", note=closeNotes[id] ?? ""
        Text(raw["text"].string ?? "").textSelection(.enabled)
        if raw["closure"] != .null {
            Text("关闭理由：" + (raw["closure"]["note"].string ?? "")).textSelection(.enabled)
        }
        if status == "unclaimed" || status == "needs_ruling" {
            TextField("如果不再继续，请写明关闭理由",text:Binding(get:{closeNotes[id] ?? ""},set:{closeNotes[id]=$0}),axis:.vertical).lineLimit(2...4).disabled(closing.contains(id))
            Button(closing.contains(id) ? "提交中":"有理由关闭需求") {
                let original=note, crew=store.selectedCrew
                guard closing.insert(id).inserted else {return}
                failure=nil
                Task {
                    defer {closing.remove(id)}
                    do {
                        _ = try await store.closeRequirement(id,expectedStatus:status,note:original)
                        if closeNotes[id] == original {closeNotes[id]=nil}
                    } catch {if store.selectedCrew == crew {failure=describe(error)}}
                }
            }.disabled(closing.contains(id) || note.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty)
        }
    }

}
struct CrewdMemberAvatar:View {
    let seed:String
    let colorSeed:String
    var size:CGFloat=30
    var body:some View {
        Text(CrewColorHash.emoji(for:seed)).font(.system(size:size*0.55))
            .frame(width:size,height:size).background(CrewColorHash.softBackground(for:colorSeed),in:Circle())
            .overlay(Circle().stroke(.secondary.opacity(0.15),lineWidth:0.5))
    }
}
struct CrewdGroupAvatar:View {
    @Environment(\.colorScheme) private var colorScheme
    let members:[CrewdValue]
    let channel:String
    let name:String
    var body:some View {
        let seeds=CrewdAvatarIdentity.group(members)
        ZStack {
            Circle().fill(CrewColorHash.softBackground(for:channel))
            if seeds.isEmpty {Text(CrewColorHash.initial(for:name)).font(.caption).foregroundStyle(Color.black.opacity(0.65))}
            else {HStack(spacing:-9) {ForEach(seeds,id:\.self) {seed in Text(CrewColorHash.emoji(for:seed)).font(.system(size:18)).offset(y:seed == seeds.first ? -4:4)}}}
        }.frame(width:36,height:36).overlay(Circle().stroke(.secondary.opacity(0.15),lineWidth:0.5))
    }
}
