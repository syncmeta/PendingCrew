import SwiftUI

enum CrewdWorkPanelDomain:Equatable { case all, requirements, human }

private struct CrewdHumanBriefView: View {
    @ObservedObject var store: CrewdWorkStore
    @Binding var drafts: CrewdHumanDrafts
    let crewTitle: String?
    @Environment(\.dismiss) private var dismiss
    @State private var brief: CrewdHumanBrief?
    @State private var failure: String?
    @State private var loading = false
    @State private var reload = UUID()
    @State private var humanTarget: CrewdHumanTarget?
    @State private var caseID: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("行政简报").font(.headline)
                Spacer()
                Button("刷新") { reload = UUID() }.disabled(loading)
                Button("关闭") { dismiss() }
            }
            Text(crewTitle ?? "当前机组").font(.subheadline)
            if let failure { Text(failure).foregroundStyle(.orange) }
            if loading { ProgressView("正在核对当前事项…").controlSize(.small) }
            if let brief {
                Text("事项核对时间  " + timestamp(brief.observedAt)).font(.caption).foregroundStyle(.secondary)
                ScrollView {
                    VStack(alignment: .leading, spacing: 14) {
                        waits(brief)
                        Divider()
                        cases(brief)
                        Divider()
                        patrol(brief.patrol)
                    }.frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }.padding(18).modifier(CrewdAdaptiveFormFrame(width: 560, height: 570, scrolls: false))
            .task(id: reload) { await read(reload) }
            .onChange(of: store.readScope) { _, _ in brief = nil; dismiss() }
            .sheet(item: $humanTarget, onDismiss: { reload = UUID() }) {
                CrewdHumanActions(store: store, drafts: $drafts, target: $0, readExactly: true)
            }
            .sheet(isPresented: Binding(get: { caseID != nil }, set: { if !$0 { caseID = nil } })) {
                if let caseID { CrewdAdminCasesView(store: store, initialCaseID: caseID) }
            }
    }

    private func waits(_ brief: CrewdHumanBrief) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("等你处理").font(.subheadline.weight(.semibold))
            if brief.waits.isEmpty { Text("当前机组没有符合简报条件的待处理事项。") }
            if brief.waitsTruncated { Text("这里只显示前 \(brief.waits.count) 项，其余事项可在待办查看。") }
            ForEach(Array(brief.waits.enumerated()), id: \.offset) { _, wait in
                let id = wait["askId"].string ?? ""
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text(verbatim: id).font(.caption.weight(.semibold)).textSelection(.enabled)
                        if wait["kind"] == .string("human_task") {
                            Text("人工待办").foregroundStyle(.secondary)
                        } else if let summary = CrewdHumanWaitPresentation.summary(wait) {
                            Text(summary).foregroundStyle(.secondary)
                        }
                        Spacer()
                        if store.capabilities.contains("channelWork") {
                            Button("打开原事项") {
                                let crew = store.selectedCrew
                                humanTarget = CrewdHumanTarget(key: .init(scope: crew, crew: crew, ask: id), crewTitle: crewTitle)
                            }
                        }
                    }.font(.caption)
                    Text(verbatim: (wait["question"].string ?? "") + (wait["questionTruncated"] == .bool(true) ? "…" : ""))
                        .textSelection(.enabled)
                    if let deadline = wait["defaultDueAtMs"].int {
                        Text("预设答复截止  " + timestamp(deadline)).font(.caption).foregroundStyle(.secondary)
                    }
                    let blockers = wait["blockingWorkIds"].array.compactMap(\.string)
                    if !blockers.isEmpty {
                        Text("阻塞工作  " + blockers.joined(separator: "、")
                             + (wait["blockersTruncated"] == .bool(true) ? " 等" : ""))
                            .font(.caption).textSelection(.enabled)
                    }
                }
            }
        }
    }

    private func cases(_ brief: CrewdHumanBrief) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("待核实行政案件").font(.subheadline.weight(.semibold))
            if brief.caseCoverage == "unavailable" { Text("当前无法读取案件记录。").foregroundStyle(.orange) }
            if brief.caseCoverage == "recent_sample" { Text("这里只核对最近一批案件。").foregroundStyle(.secondary) }
            if brief.caseCoverage != "unavailable" && brief.cases.isEmpty {
                Text("已读取的案件中没有待核实项。")
            }
            ForEach(Array(brief.cases.enumerated()), id: \.offset) { _, row in
                HStack {
                    let id = row["caseId"].string ?? ""
                    VStack(alignment: .leading, spacing: 2) {
                        Text(verbatim: id).textSelection(.enabled)
                        let status = row["custodyPhase"] == .string("stale") ? "责任待核对"
                            : row["outcomePhase"] == .string("stale") ? "核验依据已变化"
                            : row["outcomePhase"] == .string("unresolved") ? "仍待解决" : "尚无核验结果"
                        Text(status + " · " + timestamp(row["createdAtMs"].int ?? 0)).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button("查看案件") { caseID = id }
                }.font(.caption)
            }
        }
    }

    private func patrol(_ value: CrewdValue) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text("后台巡检记录").font(.subheadline.weight(.semibold))
            switch value["state"].string {
            case "complete":
                if let at = value["completedAtMs"].int { Text("最近一次结束  " + timestamp(at)) }
                let observations = CrewdPatrolBrief.Kind.allCases.compactMap { kind in
                    value["findingCounts"][kind.rawValue].int.map { "\(kind.title) \($0)\(kind.unit)" }
                }
                if !observations.isEmpty { Text(observations.joined(separator: " · ")) }
                if value["findingsTruncated"] == .bool(true) { Text("该轮只保存了部分发现的编号。") }
                Text("这是已保存的观察，处理前请在巡检页核对当前情况。").foregroundStyle(.secondary)
            case "in_progress":
                if let at = value["startedAtMs"].int { Text("本轮进行中 · " + timestamp(at)) }
            case "not_recorded": Text("尚无后台巡检记录。")
            default: Text("后台巡检记录暂不可读取。").foregroundStyle(.orange)
            }
        }.font(.caption)
    }

    private func timestamp(_ ms: Int) -> String {
        Date(timeIntervalSince1970: Double(ms) / 1_000).formatted(date: .abbreviated, time: .shortened)
    }

    private func read(_ request: UUID) async {
        let scope = store.readScope
        loading = true
        failure = nil
        brief = nil
        defer { if reload == request { loading = false } }
        do {
            let result = try await store.humanBrief()
            guard !Task.isCancelled, reload == request, store.readScope == scope else { return }
            brief = result
        } catch {
            if !Task.isCancelled, reload == request, store.readScope == scope {
                failure = error.localizedDescription
            }
        }
    }
}

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
    var patrolWorkspace: CrewdWorkspace? = nil
    private var visibleProgress: [CrewdPlan] {
        guard let visiblePlanIDs else { return store.progress }
        return visiblePlanIDs.sorted().compactMap { raw in
            guard let id = CrewdWorkID(raw, kind: .plan) else { return nil }
            return store.plans[id]
        }
    }
    @State private var patrolPresented = false
    @State private var humanBriefPresented = false
    @State private var collapsed:Set<String>=[]
    @State private var selected:String?
    @State private var failure:String?
    @State private var actionTarget: CrewdPlanTarget?
    @State private var humanTarget:CrewdHumanTarget?
    @State private var selectedRequirement: CrewdWorkID?
    @State private var openingPlan = false
    @State private var closing:Set<CrewdWorkID>=[]
    @State private var closeNotes:[CrewdWorkID:String]=[:]
    private var folderView:CrewdResourceView{progressOnly ? .progress:.todo}
    private var folderSelection:Binding<String>{Binding(get:{resourceDrafts.selection(resources.crew,folderView,channel:resources.channel)},set:{resourceDrafts.select(resources.crew,folderView,$0,channel:resources.channel)})}
    private func includes(_ kind:String,_ id:String)->Bool {compact || CrewdFolderFilter.includes(CrewdFolderFilter.object(kind,id),selection:folderSelection.wrappedValue,snapshot:resources.folders[folderView])}
    var body: some View {
        VStack(alignment:.leading,spacing:12) {
            if !compact {
                HStack(spacing:10) {
                    CrewdFolderControls(store:resources,drafts:$resourceDrafts,selection:folderSelection,view:folderView,compact:true).fixedSize(horizontal:true,vertical:false)
                    Button("巡检") { patrolPresented = true }.disabled(store.selectedCrew.isEmpty)
                    if !progressOnly && store.capabilities.contains("adminCase.humanBrief") {
                        Button("行政简报") { humanBriefPresented = true }.disabled(store.selectedCrew.isEmpty)
                    }
                    Spacer(minLength:8)
                    Button {Task {do {try await store.refresh()} catch {failure=error.localizedDescription}}} label: {Image(systemName:"arrow.clockwise")}
                        .buttonStyle(CrewdToolButtonStyle()).help("刷新").accessibilityLabel("刷新")
                }.font(CrewdType.label).controlSize(.small)
            }
            if let error=store.lastFailure { Text(error.localizedDescription).font(.caption).foregroundStyle(.orange).textSelection(.enabled) }
            if let error=resources.failure {Text(error.localizedDescription).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
            if let failure { Text(failure).foregroundStyle(.orange).textSelection(.enabled) }
            if store.loading {ProgressView().controlSize(.small)}
            if !store.hasLoaded && !store.loading && store.lastFailure == nil {
                Text("尚未读取当前机组事项").font(CrewdType.label).foregroundStyle(.secondary)
            }
            if store.hasLoaded {
            ScrollView {
                LazyVStack(alignment:.leading,spacing:10) {
                    if progressOnly && !compact && !visibleProgress.isEmpty {
                        HStack {
                            Spacer()
                            Button("全部展开") { collapsed.removeAll() }
                            Button("收起层级") { collapsed = Set(visibleProgress.map { $0.id.raw }) }
                        }.font(.caption).padding(.bottom, 4)
                    }
                    if progressOnly && visibleProgress.isEmpty && !store.loading && store.lastFailure == nil {
                        VStack(spacing:10) {
                            Text("暂无计划").font(CrewdType.label).foregroundStyle(.secondary)
                            if let openTodo { Button("从待办新建计划",action:openTodo).font(CrewdType.label) }
                        }.frame(maxWidth:.infinity,minHeight:120)
                    }
                    if !progressOnly && !compact && domain != .human {
                        requirementComposer
                        ForEach(store.requirementIDs.filter{includes("requirement",$0.raw)},id:\.self) { id in
                            requirementRow(id)
                        }
                        if store.requirementIDs.filter({includes("requirement",$0.raw)}).isEmpty && !store.loading {
                            Text("暂无待办").font(CrewdType.label).foregroundStyle(.secondary).padding(.vertical,12)
                        }
                        if domain == .all{Text("计划").font(.headline)}
                    }
                    if domain == .all {
                    ForEach(CrewdNavigation.planTree(visibleProgress.filter{includes("work",$0.id.raw)},collapsed:collapsed)) { row in
                        if let id=CrewdWorkID(row.object,kind:.plan),let plan=store.plans[id] {
                            planRow(plan,row:row)
                        }
                    }
                    }
                    if compact {
                        ForEach(store.humanTaskIDs.filter {store.humanTasks[$0]?["ask"]["crewId"].string == store.selectedCrew},id:\.self) { id in
                            let row = store.humanTasks[id] ?? .null
                            HStack(alignment:.top) {
                                Image(systemName:"questionmark.circle")
                                VStack(alignment:.leading,spacing:4) {
                                    Text("\(id.raw)  \(row["ask"]["question"].string ?? "")").lineLimit(2)
                                    if let summary = CrewdHumanWaitPresentation.summary(row) {
                                        Text(summary).foregroundStyle(.secondary)
                                    }
                                }.font(.caption)
                            }
                        }
                    }
                    if domain == .all && visibleProgress.isEmpty && !progressOnly {Text("尚无计划").foregroundStyle(CrewdChatStyle.muted(colorScheme))}
                    if !progressOnly && !compact && domain != .requirements {
                        if domain == .all { Divider() }
                        ForEach(visibleHumanIDs,id:\.self) { id in humanRow(id) }
                        if visibleHumanIDs.isEmpty && !store.loading {
                            Text("暂无待人处理事项").font(CrewdType.label).foregroundStyle(.secondary).padding(.vertical,12)
                        }
                    }
                }.frame(maxWidth:.infinity,alignment:.leading)
            }
            }
        }.padding(compact ? 8:16).foregroundStyle(CrewdChatStyle.ink(colorScheme)).background(pageBackground)
        .task(id:store.readScope.uuidString+store.capabilities.sorted().joined(separator:",")) { await store.ensureLoaded() }
        .sheet(isPresented: $patrolPresented) { CrewdPatrolView(store: store, workspace: patrolWorkspace) }
        .sheet(isPresented: $humanBriefPresented) {
            CrewdHumanBriefView(store: store, drafts: $humanDrafts,
                crewTitle: crews.first { $0["id"] == .string(store.selectedCrew) }?["title"].string)
        }
        .sheet(item:$humanTarget){target in CrewdHumanActions(store:store,drafts:$humanDrafts,target:target)}
        .sheet(item:$actionTarget) {target in CrewdPlanActions(store:store,drafts:$planDrafts,target:target,members:members,crews:crews)}
    }
    private var pageBackground:Color {
        #if os(macOS)
        return Theme.Palette.canvas
        #else
        return Theme.Palette.canvas
        #endif
    }
    private var visibleHumanIDs:[CrewdWorkID] {
        store.humanTaskIDs.filter { id in
            (!humanCrewOnly || store.humanTasks[id]?["ask"]["crewId"].string == store.selectedCrew) &&
            (store.humanTasks[id]?["ask"]["crewId"].string != resources.crew || includes("humanTask",id.raw))
        }
    }
    private func statusMark(_ status:String,human:Bool=false)->some View {
        let finished=["done","closed","answered"].contains(status)
        let withdrawn=["withdrawn","dropped"].contains(status)
        let needsAnswer=human && status == "open"
        return Image(systemName:withdrawn ? "minus.circle":finished ? "checkmark.circle.fill":needsAnswer || status == "blocked" || status == "needs_ruling" ? "questionmark.circle":"circle")
            .font(.system(size:14)).foregroundStyle(finished || withdrawn ? Theme.Palette.inkMuted:needsAnswer || status == "blocked" || status == "needs_ruling" ? Theme.Palette.amber:Theme.Palette.accent)
            .frame(width:14,height:14).accessibilityLabel(needsAnswer ? "待答复":CrewdWorkStatusLabel.display(status))
    }
    private func requirementRow(_ id:CrewdWorkID)->some View {
        let value=store.requirements[id] ?? .null
        let status=value["status"].string ?? ""
        return VStack(alignment:.leading,spacing:6) {
            HStack(spacing:8) {
                statusMark(status)
                Text(id.raw).font(CrewdType.label.weight(.semibold).monospacedDigit()).foregroundStyle(.secondary)
                Spacer(minLength:8)
                Button(selectedRequirement == id ? "收起":"详情") {
                    selectedRequirement = selectedRequirement == id ? nil:id
                    if selectedRequirement != nil {Task {do {_ = try await store.requirementDetail(id)} catch {failure=describe(error)}}}
                }
                Button("建计划") {openPlan(id)}
                    .disabled(openingPlan || CrewdWorkStore.requirementBlocksPlan(value))
                CrewdFolderAssignmentButton(store:resources,drafts:$resourceDrafts,view:.todo,object:CrewdFolderFilter.object("requirement",id.raw))
            }.font(CrewdType.caption).buttonStyle(.plain)
            MarkdownText(text:value["text"].string ?? "需求",variant:.todo,dimmed:status == "closed" || status == "done")
                .textSelection(.enabled).padding(.leading,22)
            Text(CrewdWorkStatusLabel.display(status)).font(CrewdType.caption).foregroundStyle(.secondary).padding(.leading,22)
            if selectedRequirement == id { VStack(alignment:.leading,spacing:8) {requirementDetail(id)}.padding(.leading,22) }
        }.padding(10).frame(maxWidth:.infinity,alignment:.leading)
            .background(Theme.Palette.surface,in:RoundedRectangle(cornerRadius:8))
    }
    private func planScope() -> @MainActor () -> Bool {
        let store = store
        let scope = store.readScope
        return { store.readScope == scope }
    }
    private func openPlan(_ id: CrewdWorkID) {
        let crew = store.selectedCrew
        let target = CrewdPlanTarget(key: .init(crew: crew, object: "", source: id.raw), scopeIsCurrent: planScope())
        openingPlan = true
        failure = nil
        Task {
            defer { openingPlan = false }
            do {
                try await store.requirePlanSource(id, sourceCrew: crew, in: crew)
                guard target.scopeIsCurrent() else { return }
                actionTarget = target
            } catch {
                if target.scopeIsCurrent() { failure = describe(error) }
            }
        }
    }
    private func humanRow(_ id:CrewdWorkID)->some View {
        let row=store.humanTasks[id] ?? .null
        let ask=row["ask"]
        let state=ask["state"].string ?? ""
        return VStack(alignment:.leading,spacing:6) {
            HStack(spacing:8) {
                statusMark(state,human:true)
                Text(id.raw).font(CrewdType.label.weight(.semibold).monospacedDigit()).foregroundStyle(.secondary)
                Spacer(minLength:8)
                if let crew=ask["crewId"].string {
                    Button("答复 / 撤回") {humanTarget=CrewdHumanTarget(key:.init(scope:store.selectedCrew,crew:crew,ask:id.raw),crewTitle:crews.first(where:{$0["id"].string == crew})?["title"].string)}
                    if crew == resources.crew {CrewdFolderAssignmentButton(store:resources,drafts:$resourceDrafts,view:.todo,object:CrewdFolderFilter.object("humanTask",id.raw))}
                }
            }.font(CrewdType.caption).buttonStyle(.plain)
            if let summary = CrewdHumanWaitPresentation.summary(row) {
                Text(summary).font(CrewdType.caption).foregroundStyle(.secondary).padding(.leading,22)
            }
            MarkdownText(text:ask["question"].string ?? "",variant:.todo,dimmed:state == "answered" || state == "withdrawn")
                .textSelection(.enabled).padding(.leading,22)
            Text(ask["item"].string.map{"关联计划："+$0} ?? (crews.first(where:{$0["id"] == ask["crewId"]})?["title"].string ?? "机组资料尚未加载"))
                .font(CrewdType.caption).foregroundStyle(.secondary).padding(.leading,22)
        }.padding(10).frame(maxWidth:.infinity,alignment:.leading)
            .background(Theme.Palette.surface,in:RoundedRectangle(cornerRadius:8))
    }
    private func planRow(_ plan:CrewdPlan,row:CrewdNavigation.Row)->some View {
        let id=plan.id
        return VStack(alignment:.leading,spacing:8) {
            HStack(alignment:.firstTextBaseline,spacing:8) {
                if row.expandable {
                    Button {if !collapsed.insert(row.id).inserted {collapsed.remove(row.id)}} label: {
                        Image(systemName:collapsed.contains(row.id) ? "chevron.right":"chevron.down").font(CrewdType.caption)
                    }.buttonStyle(.plain).accessibilityLabel(collapsed.contains(row.id) ? "展开子计划":"收起子计划")
                }
                statusMark(plan.status)
                Button {
                    selected = selected == id.raw ? nil:id.raw
                    if selected != nil && !compact {Task {do {_ = try await store.detail(id)} catch {failure=error.localizedDescription}}}
                } label: {
                    Text(plan.value["title"].string ?? "计划").font(CrewdType.body.weight(.medium)).multilineTextAlignment(.leading)
                }.buttonStyle(.plain)
                Spacer(minLength:4)
                Text(CrewdWorkStatusLabel.display(plan.status)).font(CrewdType.caption).foregroundStyle(.secondary)
                if plan.value["pendingTransfer"] != .null {Text("交接中").font(CrewdType.caption).foregroundStyle(.secondary)}
            }
            if !compact {
                ViewThatFits(in:.horizontal) {
                    HStack(spacing:12) {planMetadata(plan)}
                    VStack(alignment:.leading,spacing:4) {planMetadata(plan)}
                }.font(CrewdType.caption).foregroundStyle(.secondary).padding(.leading,22)
                if selected != id.raw,let note=plan.value["note"].string,!note.isEmpty {
                    Text(note).font(CrewdType.label).foregroundStyle(.secondary).lineLimit(2).padding(.leading,22)
                }
                if let parent=plan.value["parent"].string,row.depth == 0 {Text("上级："+parent).font(CrewdType.caption).foregroundStyle(.secondary).padding(.leading,22)}
            }
            if selected == id.raw,!compact {
                VStack(alignment:.leading,spacing:12) {
                    if let detail=plan.detail {
                        if let goal=detail["goal"].string,!goal.isEmpty {MarkdownText(text:goal,variant:.todo).textSelection(.enabled)}
                        if let acceptance=detail["acceptance"].string,!acceptance.isEmpty {planDetail("验收",acceptance)}
                        if let note=detail["note"].string,!note.isEmpty {planDetail("进展",note)}
                        if !detail["dependsOn"].array.isEmpty {planDetail("依赖",detail["dependsOn"].array.compactMap(\.string).joined(separator:" · "))}
                        if !detail["blockedOn"].array.isEmpty {planDetail("待答复",detail["blockedOn"].array.compactMap(\.string).joined(separator:" · "))}
                        if let evidence=detail["evidence"]["value"].string {planDetail("凭据",evidence)}
                        CrewdCoordinationView(store:store,crew:plan.value["crewId"].string ?? store.selectedCrew,id:id.raw,revision:detail["revision"].string ?? "",members:members)
                        CrewdReassignmentView(store:store,crew:plan.value["crewId"].string ?? store.selectedCrew,id:id.raw,revision:detail["revision"].string ?? "",members:members)
                        CrewdWorkCloseView(store:store,id:id,detail:detail)
                        CrewdHandoffView(store:store,id:id,members:members).id(store.selectedCrew+":"+id.raw)
                        HStack {
                            Button("更新计划") {
                                actionTarget = CrewdPlanTarget(key: .init(crew: store.selectedCrew, object: id.raw, source: ""),
                                                               scopeIsCurrent: planScope())
                            }.disabled(plan.frozen || ["done","dropped"].contains(plan.status))
                            Spacer()
                            CrewdFolderAssignmentButton(store:resources,drafts:$resourceDrafts,view:folderView,object:CrewdFolderFilter.object("work",id.raw))
                        }.font(CrewdType.label)
                    } else {Button("读取当前详情") {Task {do {_ = try await store.detail(id)} catch {failure=describe(error)}}}}
                }.padding(.leading,22).padding(.top,4)
            }
        }.padding(10).frame(maxWidth:.infinity,alignment:.leading)
            .background(selected == id.raw ? Theme.Palette.surfaceMuted:Theme.Palette.surface,in:RoundedRectangle(cornerRadius:8))
            .padding(.leading,CGFloat(min(row.depth,3))*12)
    }
    @ViewBuilder private func planMetadata(_ plan:CrewdPlan)->some View {
        if let owner=plan.value["owner"].string {Label(members.first(where:{$0["id"] == .string(owner)})?["displayName"].string ?? owner,systemImage:"person")}
        if let date=plan.value["nextCheckAt"].string {Label(checkpointLabel(date),systemImage:"clock")}
    }
    private func planDetail(_ title:String,_ value:String)->some View {
        VStack(alignment:.leading,spacing:4) {
            Text(title).font(CrewdType.caption).foregroundStyle(.secondary)
            MarkdownText(text:value,variant:.todoNote).textSelection(.enabled)
        }
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
        CrewdJudgmentInspectionView(store: store, crew: store.requirements[id]?["crewId"].string ?? store.selectedCrew, kind: "requirement", sourceID: id.raw, members: members)
        let raw=store.requirements[id] ?? .null
        let status=raw["status"].string ?? "", note=closeNotes[id] ?? ""
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
