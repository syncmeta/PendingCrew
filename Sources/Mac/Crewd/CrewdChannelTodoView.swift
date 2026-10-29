import SwiftUI

struct CrewdChannelTodoView:View {
    @ObservedObject var group:CrewdChannelWorkStore
    @ObservedObject var work:CrewdWorkStore
    @Binding var linkDrafts:CrewdChannelLinkDrafts
    @Binding var requirementDrafts:CrewdRequirementDrafts
    @Binding var planDrafts:CrewdPlanDrafts
    @Binding var humanDrafts:CrewdHumanDrafts
    let members:[CrewdValue]
    let crews:[CrewdValue]
    let openCrew:(String)->Void
    var showTodoKinds = false
    var humanOnly: Bool? = nil
    var externalQuery: Binding<String>? = nil
    var externalTag: Binding<String>? = nil
    var mutationDrafts: Binding<CrewdTodoMutationDrafts>? = nil
    @State private var selectedTags: [String:String] = [:]
    @State private var metadataTarget: CrewdTodoMetadataTarget?
    @State private var creatingHuman = false
    @State private var creatingRequirement=false
    private var tag: Binding<String> {
        externalTag ?? Binding(get:{selectedTags[queryKey] ?? ""},set:{selectedTags[queryKey]=$0})
    }
    private var kindItems: [CrewdValue] {
        group.items.filter { row in
            guard let id=CrewdChannelWorkStore.workID(row["object"]) else{return false}
            return !showTodoKinds || (selectedKind == .human ? id.kind == .human:id.kind == .requirement)
        }
    }
    private var tags: [String] {
        Array(Set(["紧急","小工程","大工程","前端","后端"] + kindItems.flatMap{$0["tags"].array.compactMap(\.string)})).sorted()
    }
    private enum TodoKind: String, CaseIterable { case ai = "AI待办", human = "人待办" }
    @State private var todoKind = TodoKind.ai
    @State private var queries: [String: String] = [:]
    private var selectedKind: TodoKind { humanOnly.map { $0 ? .human : .ai } ?? todoKind }
    private var queryKey: String { crew + "/" + channel + "/" + selectedKind.rawValue }
    private var query: Binding<String> {
        externalQuery ?? Binding(get: { queries[queryKey] ?? "" }, set: { queries[queryKey] = $0 })
    }
    private var visibleItems: [CrewdValue] {
        let needle=query.wrappedValue.trimmingCharacters(in:.whitespacesAndNewlines)
        return kindItems.filter { row in
            (tag.wrappedValue.isEmpty || row["tags"].array.contains(.string(tag.wrappedValue))) &&
            (needle.isEmpty || (row["object"]["id"].string ?? "").localizedCaseInsensitiveContains(needle)
                || (row["label"].string ?? "").localizedCaseInsensitiveContains(needle))
        }
    }
    @State private var activeOwner=UUID()
    @State private var selected:CrewdValue?
    @State private var detail:CrewdValue = .null
    @State private var reading=false
    @State private var detailToken=UUID()
    @State private var failure:String?
    @State private var planTarget:CrewdPlanTarget?
    @State private var humanTarget:CrewdHumanTarget?
    @State private var association=false
    private var crew:String{group.crew}
    private var channel:String{group.channel}
    private var linkKey:CrewdChannelLinkDrafts.Key{.init(crew:crew,channel:channel)}
    private var busy:Bool{linkDrafts.busy(linkKey) || group.writing || requirementDrafts.busy(crew,channel:channel)}
    var body:some View {
        VStack(alignment:.leading,spacing:12) {
            if showTodoKinds {
                if humanOnly == nil {
                    ViewThatFits(in: .horizontal) {
                        HStack(spacing: 12) { kindPicker; searchField }
                        VStack(alignment: .leading, spacing: 8) { kindPicker; searchField }
                    }.padding(.bottom, 10)
                } else {
                    HStack { searchField.frame(maxWidth:300); Spacer(minLength:0) }
                }
                if kindItems.contains(where:{!$0["tags"].array.isEmpty}) || !tag.wrappedValue.isEmpty {
                    ScrollView(.horizontal,showsIndicators:false){
                        HStack(spacing:8){
                            tagButton("",count:kindItems.count)
                            ForEach(tags.filter{value in value == tag.wrappedValue || kindItems.contains(where:{$0["tags"].array.contains(.string(value))})},id:\.self){value in
                                tagButton(value,count:kindItems.filter{$0["tags"].array.contains(.string(value))}.count)
                            }
                        }
                    }
                }
            }
            if !group.capabilities.contains("channelWork") {Text("当前连接暂不支持待办").font(.caption).foregroundStyle(.secondary)}
            else if channel.isEmpty {Text("请选择会话").font(.caption).foregroundStyle(.secondary)}
            else {
                HStack(spacing:10){
                    if !showTodoKinds || selectedKind == .ai {
                        Button("新建 AI 待办",systemImage:"plus"){failure=nil;creatingRequirement=true}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(busy)
                    }else if mutationDrafts != nil {
                        Button("新建人待办",systemImage:"plus"){creatingHuman=true}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(busy)
                    }
                    Button("关联已有",systemImage:"link"){association=true}.disabled(group.linkRevision == nil || busy)
                    Spacer(minLength:4)
                    Button{Task{await refreshReferences()}}label:{Image(systemName:"arrow.clockwise")}
                        .buttonStyle(CrewdToolButtonStyle()).help("刷新本群待办工作").accessibilityLabel("刷新本群待办工作")
                }.font(CrewdType.label).controlSize(.small)

                if let error=group.failure {Text(error.localizedDescription).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
                if let failure {Text(failure).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
                if group.loading {ProgressView().controlSize(.small)}
                ForEach(linkDrafts.uncertain(linkKey),id:\.requestID){old in
                    Text("此前"+(old.linked ? "关联":"解除关联")+(old.object["id"].string ?? "")+"的结果未确认，不会自动重发。").font(.caption2).foregroundStyle(.orange)
                    Button("核对当前关联"){selected = .object(["object":old.object]);association=true}.font(.caption)
                }
                ScrollView {
                    LazyVStack(alignment:.leading,spacing:10) {
                        if group.loaded,!group.loading,group.failure == nil,visibleItems.isEmpty {
                            if !query.wrappedValue.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || !tag.wrappedValue.isEmpty {
                                HStack {
                                    Text("没有符合筛选的待办").foregroundStyle(.secondary)
                                    Button("清除筛选") { query.wrappedValue=""; tag.wrappedValue="" }
                                }.font(CrewdType.label)
                            }else {
                                Text(selectedKind == .human ? "暂无待处理的人待办" : "暂无 AI 待办")
                                    .font(CrewdType.label).foregroundStyle(.secondary).padding(.top, 8)
                            }
                        }
                        ForEach(visibleItems.indices,id: \.self) { index in
                            let row = visibleItems[index]
                            if let id = CrewdChannelWorkStore.workID(row["object"]) {
                                todoRow(row, id)
                            }
                        }
                    }.padding(.vertical,4).frame(maxWidth:.infinity,alignment:.leading)
                }
            }
        }.padding(16).background(todoPageBackground).onAppear{group.setActive(true,owner:activeOwner)}.onDisappear{group.setActive(false,owner:activeOwner)}
        .onChange(of:channel){_,_ in detailToken=UUID();reading=false;selected=nil;detail = .null;association=false;planTarget=nil;humanTarget=nil;failure=nil;creatingRequirement=false}
        .onChange(of:crew){_,_ in detailToken=UUID();reading=false;selected=nil;detail = .null;association=false;planTarget=nil;humanTarget=nil;failure=nil;creatingRequirement=false}
        .onChange(of:group.items){_,rows in
            guard let old=selected else{return}
            if let current=rows.first(where:{$0["object"] == old["object"]}) {if current != old{readDetail(current)}}
            else {detailToken=UUID();reading=false;selected=nil;detail = .null}
        }
        .task(id:crew+channel+group.capabilities.sorted().joined(separator:",")+group.activationEpoch.uuidString){if group.capabilities.contains("channelWork"),!channel.isEmpty{await refreshReferences()}}
        .sheet(isPresented:$association){CrewdChannelAssociation(group:group,work:work,drafts:$linkDrafts,initial:selected?["object"])}
        .sheet(item:$planTarget){target in CrewdPlanActions(store:work,drafts:$planDrafts,target:target,members:members,crews:crews)}
        .sheet(item:$humanTarget){target in CrewdHumanActions(store:work,drafts:$humanDrafts,target:target)}
        .sheet(item:$metadataTarget){target in
            if let mutationDrafts {CrewdTodoMetadataEditor(group:group,drafts:mutationDrafts,target:target,members:members)}
        }
        .sheet(isPresented:$creatingRequirement){
            VStack(alignment:.leading,spacing:16){
                CrewdPanelHeader(title:"新建 AI 待办",symbol:"checklist")
                composer
            }.padding(20).modifier(CrewdAdaptiveFormFrame(width:540,height:360))
        }
        .sheet(isPresented:$creatingHuman){
            if let mutationDrafts {CrewdHumanTodoCreation(group:group,drafts:mutationDrafts)}
        }
    }
    private func refreshReferences() async {
        // The store publishes only current-scope read failures and excludes
        // cancellation. Copying errors into view state revives cancelled or
        // superseded reads after navigation and leaves them after a good refresh.
        do { try await group.refresh() } catch { /* Presented through group.failure. */ }
    }
    private func tagButton(_ value:String,count:Int)->some View {
        Button {tag.wrappedValue=value} label:{Text((value.isEmpty ? "所有待办":value)+(group.loaded && !group.loading ? " \(count)":"")).font(CrewdType.caption).padding(.horizontal,9).padding(.vertical,6)
                .background(tag.wrappedValue == value ? Theme.Palette.surfaceMuted:Color.clear,in:Capsule())}.buttonStyle(.plain)
    }
    private func ownerLabel(_ value:CrewdValue)->String {
        if value["kind"] == .string("human"){return "你"}
        guard let id=value["id"].string else{return "未分配"}
        return members.first(where:{$0["id"] == .string(id)})?["displayName"].string ?? id
    }
    private var todoPageBackground: Color {
        #if os(macOS)
        Theme.Palette.canvas
        #else
        Theme.Palette.canvas
        #endif
    }
    private func rowState(_ row: CrewdValue, _ id: CrewdWorkID) -> String {
        let source = row["status"].string ?? ""
        if ["withdrawn", "dropped"].contains(source) { return source }
        return (group.todoAttributes[id.raw] ?? row["todo"])["manualState"].string ?? source
    }
    private func bodyText(_ row: CrewdValue, _ id: CrewdWorkID) -> String {
        let preview = (row["label"].string ?? "") + (row["labelTruncated"] == .bool(true) ? "…" : "")
        if selected?["object"] == row["object"], detail != .null {
            return detail["text"].string ?? detail["goal"].string ?? detail["question"].string ?? preview
        }
        switch id.kind {
        case .requirement: return work.requirements[id]?["text"].string ?? preview
        case .human: return work.humanTasks[id]?["ask"]["question"].string ?? preview
        case .plan: return work.plans[id]?.value["title"].string ?? preview
        }
    }
    private func todoRow(_ row: CrewdValue, _ id: CrewdWorkID) -> some View {
        let state = rowState(row, id)
        let withdrawn = ["withdrawn", "dropped"].contains(state)
        let finished = ["done", "closed", "completed", "answered"].contains(state)
        return VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Image(systemName: withdrawn ? "minus.circle" : finished ? "checkmark.circle.fill" : id.kind == .human ? "questionmark.circle" : "circle")
                    .font(.system(size: 14)).frame(width: 14, height: 14)
                    .foregroundStyle(finished || withdrawn ? Theme.Palette.inkMuted : id.kind == .human ? Theme.Palette.amber : Theme.Palette.accent)
                Text(id.raw).font(CrewdType.label.weight(.semibold).monospacedDigit()).foregroundStyle(.secondary)
                Spacer(minLength: 8)
                Button(selected?["object"] == row["object"] ? "收起" : "详情") { select(row) }
                if id.kind != .plan, mutationDrafts != nil {
                    Button { metadataTarget = .init(row: row) } label: { Image(systemName: "pencil") }
                        .help("编辑负责人、状态与标签").accessibilityLabel("编辑待办")
                }
            }.font(CrewdType.caption).buttonStyle(.plain)
            MarkdownText(text: bodyText(row, id), variant: .todo, dimmed: finished || withdrawn)
                .textSelection(.enabled).padding(.leading, 22)
            HStack(spacing: 8) {
                Text(statusLabel(row))
                if id.kind != .plan {
                    let source = group.todoAttributes[id.raw] ?? row["todo"]
                    Text("·"); Label(ownerLabel(source["owner"]), systemImage: "person")
                }
            }.font(CrewdType.caption).foregroundStyle(.secondary).padding(.leading, 22)
            if id.kind != .plan, !row["tags"].array.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(row["tags"].array.compactMap(\.string), id: \.self) { name in
                            Button(name) { tag.wrappedValue = name }.buttonStyle(.plain).font(CrewdType.caption)
                        }
                    }
                }.padding(.leading, 22)
            }
            if selected?["object"] == row["object"] {
                VStack(alignment: .leading, spacing: 8) { selectedDetails(row, id) }.padding(.leading, 22)
            }
        }.padding(10).frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.Palette.surface, in: RoundedRectangle(cornerRadius: 8))
    }
    private func statusLabel(_ row:CrewdValue)->String {
        if let raw = row["status"].string, ["withdrawn", "dropped"].contains(raw) { return CrewdWorkStatusLabel.display(raw) }
        let source=group.todoAttributes[row["object"]["id"].string ?? ""] ?? row["todo"]
        if source["manualState"] == .string("done"){return "人工完成"}
        if source["manualState"] == .string("open"){return "待处理"}
        return row["object"]["kind"] == .string("humanTask") && row["status"] == .string("open") ? "待答复":CrewdWorkStatusLabel.display(row["status"].string ?? "")
    }
    private var kindPicker: some View {
        Picker("待办归属", selection: $todoKind) {
            ForEach(TodoKind.allCases, id: \.self) { Text($0.rawValue).tag($0) }
        }.pickerStyle(.segmented).frame(maxWidth: 260)
    }
    private var searchField: some View {
        TextField("搜索编号或标题", text: query).textFieldStyle(.roundedBorder)
    }
    private var composer:some View {
        VStack(alignment:.leading,spacing:12) {
            if let failure {Text(failure).font(CrewdType.caption).foregroundStyle(.orange).textSelection(.enabled)}
            ForEach(requirementDrafts.uncertain(crew,channel:channel),id:\.requestID){old in
                Text("创建结果未确认，请先核对原提交。").font(.caption2).foregroundStyle(.orange)
                Text(old.text).font(.caption).lineLimit(3).textSelection(.enabled)
                Button("确认这份原始提交"){if requirementDrafts.confirm(old){create(old)}}.font(.caption).disabled(busy)
            }
            CrewdFormField(title:"待办内容") {TextField("写下希望完成的事情",text:Binding(get:{requirementDrafts.text(crew,channel:channel)},set:{requirementDrafts.set(crew,channel:channel,text:$0)}),axis:.vertical).lineLimit(4...8).disabled(busy)}
            HStack {Button("取消"){creatingRequirement=false};Spacer()
            Button(busy ? "提交中":"新建"){if let snapshot=requirementDrafts.begin(crew,channel:channel){create(snapshot)}}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(busy || requirementDrafts.text(crew,channel:channel).trimmingCharacters(in:.whitespacesAndNewlines).isEmpty)
            }
        }
    }
    private func create(_ snapshot:CrewdRequirementDrafts.Snapshot){Task{
        guard crew == snapshot.crew,channel == snapshot.channel,work.selectedCrew == snapshot.crew else{requirementDrafts.finish(snapshot,accepted:false);return}
        do {
            let receipt=try await work.createRequirement(requestID:snapshot.requestID,text:snapshot.text,channel:snapshot.channel)
            requirementDrafts.finish(snapshot,accepted:true)
            guard crew == snapshot.crew,channel == snapshot.channel else{return}
            creatingRequirement=false
            if receipt["channelAssociation"]["linked"] == .bool(false){failure="原需求已确认，但此前会话关联已解除；不会重新关联。"}
            do{try await group.refresh()}catch{}
        }catch{requirementDrafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error));if crew == snapshot.crew,channel == snapshot.channel{failure=error.localizedDescription}}
    }}
    private func select(_ row:CrewdValue){
        if selected?["object"] == row["object"] {detailToken=UUID();reading=false;selected=nil;detail = .null;return}
        readDetail(row)
    }
    private func readDetail(_ row:CrewdValue){
        let token=UUID();detailToken=token;selected=row;detail = .null
        guard let id=CrewdChannelWorkStore.workID(row["object"]) else{return}
        let scope=channel;reading=true
        Task{defer{if detailToken == token,channel == scope,selected?["object"] == row["object"]{reading=false}};do{
            let value:CrewdValue
            switch id.kind {case .requirement:value=try await work.requirementDetail(id);case .plan:value=(try await work.detail(id)).value;case .human:value=try await work.humanDetail(id,crew:row["crewId"].string ?? "")}
            guard detailToken == token,channel == scope,selected?["object"] == row["object"] else{return};detail=value
        }catch{if detailToken == token,channel == scope,selected?["object"] == row["object"]{failure=error.localizedDescription}}}
    }
    private func openBlocker(_ id:CrewdWorkID){
        let token=detailToken,scope=channel,scopeCrew=crew
        Task{do{
            try await work.refresh()
            guard detailToken == token,channel == scope,crew == scopeCrew else{return}
            guard let ask=work.humanTasks[id]?["ask"],ask["state"] == .string("open"),let owner=ask["crewId"].string else{failure="卡点已不在当前待答复收件箱，请刷新核对计划。";return}
            humanTarget=CrewdHumanTarget(key:.init(scope:work.selectedCrew,crew:owner,ask:id.raw),crewTitle:crews.first(where:{$0["id"] == .string(owner)})?["title"].string)
        }catch{if detailToken == token,channel == scope,crew == scopeCrew{failure=error.localizedDescription}}}
    }
    @ViewBuilder private func selectedDetails(_ row:CrewdValue,_ id:CrewdWorkID)->some View {
        if reading {ProgressView().controlSize(.small)}
        if detail != .null {
            if let answer=detail["answer"].string { MarkdownText(text: "答复：" + answer, variant: .todoNote).textSelection(.enabled) }
            if let reason=detail["withdrawReason"].string { MarkdownText(text: "撤回理由：" + reason, variant: .todoNote).textSelection(.enabled) }
            if id.kind == .requirement {
                Button("新建计划"){planTarget=CrewdPlanTarget(key:.init(crew:crew,object:"",source:id.raw,channel:channel))}.font(.caption).disabled(busy || detail["status"] == .string("closed"))
            }else if id.kind == .plan,row["crewId"] == .string(crew) {
                Button("计划操作"){planTarget=CrewdPlanTarget(key:.init(crew:crew,object:id.raw,source:""))}.font(.caption).disabled(detail["frozen"] == .bool(true) || ["done","dropped"].contains(detail["status"].string ?? ""))
                ForEach(detail["blockedOn"].array.compactMap{$0.string.flatMap{CrewdWorkID($0,kind:.human)}},id:\.self){wait in
                    Button("答复卡点 "+wait.raw){openBlocker(wait)}.font(.caption).disabled(busy)
                }
            }else if id.kind == .human,(group.todoAttributes[id.raw] ?? row["todo"])["origin"] != .string("humanTask"),detail["state"] == .string("open"),let owner=row["crewId"].string {
                Button("答复 / 撤回"){humanTarget=CrewdHumanTarget(key:.init(scope:work.selectedCrew,crew:owner,ask:id.raw),crewTitle:crews.first(where:{$0["id"] == .string(owner)})?["title"].string)}.font(.caption)
            }
        }
        if let owner=row["crewId"].string,owner != crew {Button("打开所属机组处理"){openCrew(owner)}.font(.caption)}
        Button("解除本会话关联"){association=true}.font(.caption).disabled(busy)
    }
}

private struct CrewdChannelAssociation:View {
    @ObservedObject var group:CrewdChannelWorkStore
    @ObservedObject var work:CrewdWorkStore
    @Binding var drafts:CrewdChannelLinkDrafts
    let initial:CrewdValue?
    @Environment(\.dismiss) private var dismiss
    @State private var choice=""
    @State private var arbitraryID=""
    @State private var revision:String?
    @State private var failure:String?
    @State private var submitting=false
    @State private var readVerified=false
    private var key:CrewdChannelLinkDrafts.Key{.init(crew:group.crew,channel:scope)}
    private var unknown:Bool{!drafts.uncertain(key).isEmpty}
    @State private var scope=""
    private var options:[CrewdValue] {
        let linked=group.items.map{$0["object"]}
        let plans=work.planIDs.map{CrewdFolderFilter.object("work",$0.raw)}
        let requirements=work.requirementIDs.map{CrewdFolderFilter.object("requirement",$0.raw)}
        let humans=work.humanTasks.keys.map{CrewdFolderFilter.object("humanTask",$0.raw)}
        return Array(Set((linked+plans+requirements+humans).compactMap{CrewdChannelWorkStore.workID($0)?.raw})).sorted().compactMap{raw in object(raw)}
    }
    private func object(_ raw:String)->CrewdValue?{for (kind,type) in [("requirement",CrewdWorkID.Kind.requirement),("work",.plan),("humanTask",.human)]{if CrewdWorkID(raw,kind:type) != nil{return CrewdFolderFilter.object(kind,raw)}};return nil}
    private var selected:CrewdValue? {object(arbitraryID.isEmpty ? choice:arbitraryID.trimmingCharacters(in:.whitespacesAndNewlines))}
    private var isLinked:Bool {selected.map{value in group.items.contains{$0["object"] == value}} ?? false}
    var body:some View {
        VStack(alignment:.leading,spacing:12){
            CrewdPanelHeader(title:"关联已有事项",symbol:"link")
            Picker("已加载工作",selection:$choice){Text("请选择").tag("");ForEach(options.indices,id:\.self){i in Text(options[i]["id"].string ?? "").tag(options[i]["id"].string ?? "")}}
            TextField("或填写公共编号（# / W / H）",text:$arbitraryID)
            if let failure {Text(failure).foregroundStyle(.orange).textSelection(.enabled)}
            if unknown {
                Text("提交结果未确认。请重新读取核对，程序不会自动重发；当前关联状态不能证明此前请求是否成功。").font(.caption).foregroundStyle(.orange)
                Button("我已核对当前关联，结束待确认记录"){drafts.acknowledge(key)}.font(.caption).disabled(!readVerified || submitting)
            }
            Button("读取当前关联"){Task{await read()}}.disabled(submitting)
            if revision == nil {Text("请先读取当前关联。").font(.caption).foregroundStyle(.secondary)}
            HStack{Button("关闭"){dismiss()};Spacer();Button(isLinked ? "解除关联":"关联"){send()}.disabled(selected == nil || revision == nil || submitting || group.channel != scope || unknown)}
        }.padding(20).textFieldStyle(.roundedBorder).modifier(CrewdAdaptiveFormFrame(width:480)).disabled(submitting).task{scope=group.channel;choice=initial?["id"].string ?? "";await read()}
    }
    private func read()async{readVerified=false;do{try await group.refresh();guard group.channel == scope else{return};revision=group.linkRevision;readVerified=true;failure=nil}catch{failure=error.localizedDescription}}
    private func send(){
        guard let selected,let revision,let snapshot=drafts.begin(key,object:selected,expectedRevision:revision,linked:!isLinked) else{return}
        submitting=true;readVerified=false
        Task{defer{submitting=false};do{
            _ = try await group.changeLink(object:snapshot.object,expectedRevision:snapshot.expectedRevision,linked:snapshot.linked)
            drafts.finish(snapshot,accepted:true)
            if group.channel == snapshot.key.channel,group.crew == snapshot.key.crew{dismiss()}
        }catch{
            drafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error))
            if group.channel == snapshot.key.channel,group.crew == snapshot.key.crew{failure=error.localizedDescription;self.revision=nil}
        }}
    }
}

struct CrewdTodoMetadataTarget:Identifiable {
    let row:CrewdValue
    var id:String{row["object"]["id"].string ?? ""}
}

/// Global owner/state and channel-local tags have different CAS tokens. The
/// editor keeps those operations distinct and never converts one into another.
private struct CrewdTodoMetadataEditor:View {
    @ObservedObject var group:CrewdChannelWorkStore
    @Binding var drafts:CrewdTodoMutationDrafts
    let target:CrewdTodoMetadataTarget
    let members:[CrewdValue]
    @Environment(\.dismiss) private var dismiss
    @State private var source:CrewdValue = .null
    @State private var tagInput=""
    @State private var failure:String?
    @State private var reading=false
    @State private var originalCrew=""
    @State private var originalChannel=""
    private var current:Bool{group.crew == originalCrew && group.channel == originalChannel}
    private func key(_ op:String)->CrewdTodoMutationDrafts.Key{.init(crew:originalCrew,channel:originalChannel,object:target.id,operation:op)}
    private func fields(_ op:String)->CrewdTodoMutationDrafts.Fields{drafts.fields(key(op))}
    private func edit(_ op:String,_ mutate:(inout CrewdTodoMutationDrafts.Fields)->Void){var value=fields(op);mutate(&value);drafts.edit(key(op),value)}
    private var owner:String {let owner=fields("todo.assign").owner;return owner["kind"] == .string("human") ? "human":owner["id"].string ?? ""}
    private var operations:[String]{["todo.assign","todo.setState","channel.workTagsSet"]}
    var body:some View {
        ScrollView {
            VStack(alignment:.leading,spacing:14) {
                CrewdPanelHeader(title:target.row["label"].string ?? "编辑待办",symbol:"checklist")
                if reading{ProgressView()}
                if let failure{Text(failure).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
                CrewdFormSection(title:"负责人") {
                Picker("负责人",selection:Binding(get:{owner},set:{value in edit("todo.assign"){$0.owner=value.isEmpty ? .null:.object(value == "human" ? ["kind":.string("human")]:["kind":.string("session"),"id":.string(value)])}})) {
                    Text("未分配").tag("");Text("你").tag("human")
                    ForEach(members.compactMap{$0["id"].string},id:\.self){id in Text(members.first(where:{$0["id"] == .string(id)})?["displayName"].string ?? id).tag(id)}
                    if !owner.isEmpty,owner != "human",!members.contains(where:{$0["id"] == .string(owner)}){Text("原负责人（已不在本群）").tag(owner)}
                }
                revisionControl("todo.assign")
                submitButton("保存负责人","todo.assign")
                }
                CrewdFormSection(title:"处理状态") {
                if source["origin"] != .string("agentAsk"),source != .null {
                    Picker("人工状态",selection:Binding(get:{fields("todo.setState").state},set:{value in edit("todo.setState"){$0.state=value}})) {Text("待处理").tag("open");Text("已完成").tag("done")}.pickerStyle(.segmented)
                    Text("只改变待办状态，不影响关联计划。").font(.caption).foregroundStyle(.secondary)
                    revisionControl("todo.setState")
                    submitButton("保存人工状态","todo.setState")
                } else {Text("请打开事项，通过「答复 / 撤回」处理这个问题。").font(.caption).foregroundStyle(.secondary)}
                }
                CrewdFormSection(title:"本群标签") {
                ForEach(fields("channel.workTagsSet").tags,id:\.self){tag in
                    HStack{Text(tag);Spacer();Button("移除"){edit("channel.workTagsSet"){$0.tags.removeAll{$0 == tag}}}.font(.caption)}
                }
                HStack{TextField("新标签",text:$tagInput);Button("添加"){let value=tagInput.trimmingCharacters(in:.whitespacesAndNewlines);if !value.isEmpty,!fields("channel.workTagsSet").tags.contains(value){edit("channel.workTagsSet"){$0.tags.append(value)}};tagInput=""}}
                revisionControl("channel.workTagsSet")
                submitButton("保存本群标签","channel.workTagsSet")
                }
                ForEach(operations,id:\.self){op in
                    ForEach(drafts.uncertain(key(op)),id:\.requestID){snapshot in
                        Text("原提交结果未确认，输入保留；不会重新发送。").font(.caption).foregroundStyle(.orange)
                        Button("查询原请求结果"){lookup(snapshot)}.font(.caption).disabled(reading || !current)
                    }
                }
                Button("刷新"){Task{await read()}}.disabled(reading || group.writing || !current)
                Button("关闭"){dismiss()}
            }.padding(20)
        }.modifier(CrewdAdaptiveFormFrame(width:560,height:680,scrolls:false))
        .task{originalCrew=group.crew;originalChannel=group.channel;await read()}
    }
    @ViewBuilder private func revisionControl(_ operation:String)->some View {
        let revision=operation == "channel.workTagsSet" ? group.linkRevision:source["revision"].string
        if let revision,fields(operation).revision != revision {
            Text("后台版本已变化，请比较当前负责人、状态或标签后再决定。").font(.caption).foregroundStyle(.orange)
            Button("我已比较，保留编辑并采用当前版本") {edit(operation){$0.revision=revision}}.font(.caption)
        }
    }
    private func submitButton(_ label:String,_ operation:String)->some View {
        Button(label){guard let snapshot=drafts.begin(key(operation)) else{return};Task{
            do{_ = try await group.submit(snapshot);drafts.finish(snapshot,accepted:true);await read()}
            catch{drafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error));if current{failure=error.localizedDescription}}
        }}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(reading || !current || group.writing || source == .null || !drafts.uncertain(key(operation)).isEmpty)
    }
    private func read()async {
        guard current else{return};reading=true;defer{reading=false}
        do {
            let value=try await group.attributes(target.row["object"]);try await group.refresh();guard current else{return};source=value;failure=nil
            for op in operations where !drafts.exists(key(op)) {
                var f=CrewdTodoMutationDrafts.Fields();f.revision=op == "channel.workTagsSet" ? group.linkRevision ?? "":value["revision"].string ?? ""
                f.owner=value["owner"];f.state=value["manualState"].string ?? "open"
                f.tags=group.items.first(where:{$0["object"] == target.row["object"]})?["tags"].array.compactMap(\.string) ?? []
                drafts.edit(key(op),f)
            }
        }catch{if current{failure=error.localizedDescription}}
    }
    private func lookup(_ snapshot:CrewdTodoMutationDrafts.Snapshot){reading=true;Task{defer{reading=false};do{
        if try await group.lookup(snapshot) != nil{drafts.finish(snapshot,accepted:true);failure=nil}
        else{failure="原请求当前没有已提交回执；原输入保留，不会自动重发。"}
    }catch{if current{failure=error.localizedDescription}}}}
}

private struct CrewdHumanTodoCreation:View {
    @ObservedObject var group:CrewdChannelWorkStore
    @Binding var drafts:CrewdTodoMutationDrafts
    @Environment(\.dismiss) private var dismiss
    @State private var crew=""
    @State private var channel=""
    @State private var failure:String?
    private var key:CrewdTodoMutationDrafts.Key{.init(crew:crew,channel:channel,object:"",operation:"humanTask.create")}
    private var current:Bool{group.crew == crew && group.channel == channel}
    var body:some View {
        VStack(alignment:.leading,spacing:16){
            CrewdPanelHeader(title:"新建人待办",symbol:"person.crop.circle.badge.checkmark")
            CrewdFormSection(title:"待办内容") {
                TextField("需要人处理的事项",text:Binding(get:{drafts.fields(key).text},set:{value in var f=drafts.fields(key);f.text=value;drafts.edit(key,f)}),axis:.vertical).lineLimit(3...8).font(CrewdType.body).textFieldStyle(.roundedBorder)
            }
            if let failure{CrewdInlineNotice(title:"尚未保存",detail:failure,warning:true)}
            ForEach(drafts.uncertain(key),id:\.requestID){snapshot in
                Text(snapshot.fields.text).textSelection(.enabled)
                Button("查询原创建结果"){Task{do{if try await group.lookup(snapshot) != nil{drafts.finish(snapshot,accepted:true);dismiss()}else{failure="原请求当前没有已提交回执；输入保留，不会重发。"}}catch{failure=error.localizedDescription}}}
            }
            HStack{Button("关闭"){dismiss()};Spacer();Button("保存"){guard let snapshot=drafts.begin(key) else{return};Task{do{_ = try await group.submit(snapshot);drafts.finish(snapshot,accepted:true);if current{dismiss()}}catch{drafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error));if current{failure=error.localizedDescription}}}}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(!current || group.writing || drafts.fields(key).text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || !drafts.uncertain(key).isEmpty)}
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:520,height:460))
        .onAppear{crew=group.crew;channel=group.channel}
    }
}
