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
    private enum TodoKind: String, CaseIterable { case ai = "AI Todo", human = "真人 Todo" }
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
        VStack(alignment:.leading,spacing:6) {
            if showTodoKinds {
                if humanOnly == nil {
                    ViewThatFits(in: .horizontal) {
                        HStack(spacing: 12) { kindPicker; searchField }
                        VStack(alignment: .leading, spacing: 8) { kindPicker; searchField }
                    }.padding(.bottom, 10)
                } else { searchField.padding(.bottom, 10) }
                ScrollView(.horizontal,showsIndicators:false) {
                    HStack(spacing:8) {
                        tagButton("",count:kindItems.count)
                        ForEach(tags,id:\.self){value in tagButton(value,count:kindItems.filter{$0["tags"].array.contains(.string(value))}.count)}
                    }
                }.padding(.bottom,8)
            }
            if !group.capabilities.contains("channelWork") {Text("群工作引用 · 配套后台待开放").font(.caption).foregroundStyle(.secondary)}
            else if channel.isEmpty {Text("请选择会话").font(.caption).foregroundStyle(.secondary)}
            else {
                HStack{Button("刷新",systemImage:"arrow.clockwise"){Task{do{try await group.refresh()}catch{failure=error.localizedDescription}}};Spacer();Button("关联已有工作",systemImage:"link"){association=true}.disabled(group.linkRevision == nil || busy)}.font(.caption)
                if let error=group.failure {Text(error.localizedDescription).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
                if let failure {Text(failure).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
                if group.loading {ProgressView().controlSize(.small)}
                ForEach(linkDrafts.uncertain(linkKey),id:\.requestID){old in
                    Text("此前"+(old.linked ? "关联":"解除关联")+(old.object["id"].string ?? "")+"的结果未确认，不会自动重发。").font(.caption2).foregroundStyle(.orange)
                    Button("核对当前关联"){selected = .object(["object":old.object]);association=true}.font(.caption)
                }
                if !showTodoKinds || selectedKind == .ai { composer }
                else if mutationDrafts != nil { Button("＋ 人待办") { creatingHuman=true }.buttonStyle(.bordered) }
                ScrollView {
                    VStack(alignment:.leading,spacing:8) {
                        if group.loaded,visibleItems.isEmpty {
                            Text(group.items.isEmpty ? "本会话尚无关联工作" : "没有符合筛选的事项").font(.caption).foregroundStyle(.secondary)
                        }
                        ForEach(visibleItems.indices,id:\.self){i in
                            let row=visibleItems[i]
                            if let id=CrewdChannelWorkStore.workID(row["object"]) {
                                VStack(alignment:.leading,spacing:4){
                                    Button{select(row)}label:{Text(id.raw+"  "+(row["label"].string ?? "")+(row["labelTruncated"] == .bool(true) ? "…":"")).font(.caption).multilineTextAlignment(.leading)}.buttonStyle(.plain)
                                    Text(statusLabel(row)).font(.caption2).foregroundStyle(.secondary)
                                    if id.kind != .plan {
                                        let source=group.todoAttributes[id.raw] ?? row["todo"]
                                        Text("负责人："+ownerLabel(source["owner"])).font(.caption).foregroundStyle(.secondary)
                                        if !row["tags"].array.isEmpty {
                                            ScrollView(.horizontal,showsIndicators:false){HStack{ForEach(row["tags"].array.compactMap(\.string),id:\.self){name in Button(name){tag.wrappedValue=name}.font(.caption)}}}
                                        }
                                        if mutationDrafts != nil {Button("负责人、状态与标签"){metadataTarget = .init(row:row)}.font(.caption)}
                                    }
                                    if selected?["object"] == row["object"] {selectedDetails(row,id)}
                                }.padding(12).frame(maxWidth:.infinity,alignment:.leading).background(Theme.Palette.canvas,in:RoundedRectangle(cornerRadius:10))
                            }
                        }
                    }.frame(maxWidth:.infinity,alignment:.leading)
                }
            }
        }.onAppear{group.setActive(true,owner:activeOwner)}.onDisappear{group.setActive(false,owner:activeOwner)}
        .onChange(of:channel){_,_ in detailToken=UUID();reading=false;selected=nil;detail = .null;association=false;planTarget=nil;humanTarget=nil;failure=nil}
        .onChange(of:crew){_,_ in detailToken=UUID();reading=false;selected=nil;detail = .null;association=false;planTarget=nil;humanTarget=nil;failure=nil}
        .onChange(of:group.items){_,rows in
            guard let old=selected else{return}
            if let current=rows.first(where:{$0["object"] == old["object"]}) {if current != old{readDetail(current)}}
            else {detailToken=UUID();reading=false;selected=nil;detail = .null}
        }
        .task(id:crew+channel+group.capabilities.sorted().joined(separator:",")+group.activationEpoch.uuidString){if group.capabilities.contains("channelWork"),!channel.isEmpty{do{try await group.refresh()}catch{failure=error.localizedDescription}}}
        .sheet(isPresented:$association){CrewdChannelAssociation(group:group,work:work,drafts:$linkDrafts,initial:selected?["object"])}
        .sheet(item:$planTarget){target in CrewdPlanActions(store:work,drafts:$planDrafts,target:target,members:members,crews:crews)}
        .sheet(item:$humanTarget){target in CrewdHumanActions(store:work,drafts:$humanDrafts,target:target)}
        .sheet(item:$metadataTarget){target in
            if let mutationDrafts {CrewdTodoMetadataEditor(group:group,drafts:mutationDrafts,target:target,members:members)}
        }
        .sheet(isPresented:$creatingHuman){
            if let mutationDrafts {CrewdHumanTodoCreation(group:group,drafts:mutationDrafts)}
        }
    }
    private func tagButton(_ value:String,count:Int)->some View {
        Button {tag.wrappedValue=value} label:{Text((value.isEmpty ? "所有待办":value)+" \(count)").font(.caption).padding(.horizontal,9).padding(.vertical,5)
                .background(tag.wrappedValue == value ? Theme.Palette.surfaceMuted:Color.clear,in:Capsule())}.buttonStyle(.plain)
    }
    private func ownerLabel(_ value:CrewdValue)->String {
        if value["kind"] == .string("human"){return "你"}
        guard let id=value["id"].string else{return "未分配"}
        return members.first(where:{$0["id"] == .string(id)})?["displayName"].string ?? id
    }
    private func statusLabel(_ row:CrewdValue)->String {
        let source=group.todoAttributes[row["object"]["id"].string ?? ""] ?? row["todo"]
        if source["manualState"] == .string("done"){return "人工完成"}
        if source["manualState"] == .string("open"){return "待处理"}
        return row["object"]["kind"] == .string("humanTask") && row["status"] == .string("open") ? "待答复":CrewdWorkStatusLabel.display(row["status"].string ?? "")
    }
    private var kindPicker: some View {
        Picker("Todo 类型", selection: $todoKind) {
            ForEach(TodoKind.allCases, id: \.self) { Text($0.rawValue).tag($0) }
        }.pickerStyle(.segmented).frame(maxWidth: 260)
    }
    private var searchField: some View {
        TextField("搜索编号或标题", text: query).textFieldStyle(.roundedBorder)
    }
    private var composer:some View {
        VStack(alignment:.leading,spacing:4) {
            ForEach(requirementDrafts.uncertain(crew,channel:channel),id:\.requestID){old in
                Text("可能已创建，确认原提交会复用原请求编号；已解除的关联不会重挂。").font(.caption2).foregroundStyle(.orange)
                Text(old.text).font(.caption).lineLimit(3).textSelection(.enabled)
                Button("确认这份原始提交"){if requirementDrafts.confirm(old){create(old)}}.font(.caption).disabled(busy)
            }
            TextField("写下本群原始需求",text:Binding(get:{requirementDrafts.text(crew,channel:channel)},set:{requirementDrafts.set(crew,channel:channel,text:$0)}),axis:.vertical).lineLimit(2...4).disabled(busy)
            Button(busy ? "提交中":"新增群需求"){if let snapshot=requirementDrafts.begin(crew,channel:channel){create(snapshot)}}.font(.caption).disabled(busy || requirementDrafts.text(crew,channel:channel).trimmingCharacters(in:.whitespacesAndNewlines).isEmpty)
        }
    }
    private func create(_ snapshot:CrewdRequirementDrafts.Snapshot){Task{
        guard crew == snapshot.crew,channel == snapshot.channel,work.selectedCrew == snapshot.crew else{requirementDrafts.finish(snapshot,accepted:false);return}
        do {
            let receipt=try await work.createRequirement(requestID:snapshot.requestID,text:snapshot.text,channel:snapshot.channel)
            requirementDrafts.finish(snapshot,accepted:true)
            guard crew == snapshot.crew,channel == snapshot.channel else{return}
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
            Text(detail["text"].string ?? detail["goal"].string ?? detail["question"].string ?? "").font(.caption).textSelection(.enabled)
            if let answer=detail["answer"].string {Text("答复："+answer).font(.caption).textSelection(.enabled)}
            if let reason=detail["withdrawReason"].string {Text("撤回理由："+reason).font(.caption).textSelection(.enabled)}
            if id.kind == .requirement {
                Button("从需求建本群计划"){planTarget=CrewdPlanTarget(key:.init(crew:crew,object:"",source:id.raw,channel:channel))}.font(.caption).disabled(busy || detail["status"] == .string("closed"))
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
            Text("本会话工作引用").font(.title2)
            Text("关联同一份公共需求、计划或人类事项；解除关联不删除工作。").font(.caption).foregroundStyle(.secondary)
            Picker("已加载工作",selection:$choice){Text("请选择").tag("");ForEach(options.indices,id:\.self){i in Text(options[i]["id"].string ?? "").tag(options[i]["id"].string ?? "")}}
            TextField("或填写公共编号（# / W / H）",text:$arbitraryID)
            Text("可见范围与跨机组权限由后台实时核验。").font(.caption).foregroundStyle(.secondary)
            if let failure {Text(failure).foregroundStyle(.orange).textSelection(.enabled)}
            if unknown {
                Text("提交结果未确认。请重新读取核对，程序不会自动重发；当前关联状态不能证明此前请求是否成功。").font(.caption).foregroundStyle(.orange)
                Button("我已核对当前关联，结束待确认记录"){drafts.acknowledge(key)}.font(.caption).disabled(!readVerified || submitting)
            }
            Button("读取当前关联"){Task{await read()}}.disabled(submitting)
            Text("已核对关联版本："+(revision ?? "尚未读取")).font(.caption)
            HStack{Button("关闭"){dismiss()};Spacer();Button(isLinked ? "解除关联":"关联"){send()}.disabled(selected == nil || revision == nil || submitting || group.channel != scope || unknown)}
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:480)).disabled(submitting).task{scope=group.channel;choice=initial?["id"].string ?? "";await read()}
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
                Text(target.row["label"].string ?? "待办").font(.headline).textSelection(.enabled)
                Text(target.id).font(.caption).foregroundStyle(.secondary)
                if reading{ProgressView()}
                if let failure{Text(failure).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
                Text("负责人和人工状态属于共享原事项；标签只属于本群引用。").font(.caption).foregroundStyle(.secondary)
                Picker("负责人",selection:Binding(get:{owner},set:{value in edit("todo.assign"){$0.owner=value.isEmpty ? .null:.object(value == "human" ? ["kind":.string("human")]:["kind":.string("session"),"id":.string(value)])}})) {
                    Text("未分配").tag("");Text("你").tag("human")
                    ForEach(members.compactMap{$0["id"].string},id:\.self){id in Text(members.first(where:{$0["id"] == .string(id)})?["displayName"].string ?? id).tag(id)}
                    if !owner.isEmpty,owner != "human",!members.contains(where:{$0["id"] == .string(owner)}){Text("原负责人（已不在本群）").tag(owner)}
                }
                revisionControl("todo.assign")
                submitButton("保存负责人","todo.assign")
                if source["origin"] != .string("agentAsk"),source != .null {
                    Picker("人工状态",selection:Binding(get:{fields("todo.setState").state},set:{value in edit("todo.setState"){$0.state=value}})) {Text("待处理").tag("open");Text("已完成").tag("done")}.pickerStyle(.segmented)
                    Text("人工完成不停止或结算既有计划；重新打开不会启动会话。").font(.caption).foregroundStyle(.secondary)
                    revisionControl("todo.setState")
                    submitButton("保存人工状态","todo.setState")
                } else {Text("这是 Agent 提问，请通过原来的答复或撤回入口处理。人工完成不会替代回答。").font(.caption).foregroundStyle(.secondary)}
                Text("本群标签").font(.subheadline)
                ForEach(fields("channel.workTagsSet").tags,id:\.self){tag in
                    HStack{Text(tag);Spacer();Button("移除"){edit("channel.workTagsSet"){$0.tags.removeAll{$0 == tag}}}.font(.caption)}
                }
                HStack{TextField("新标签",text:$tagInput);Button("添加"){let value=tagInput.trimmingCharacters(in:.whitespacesAndNewlines);if !value.isEmpty,!fields("channel.workTagsSet").tags.contains(value){edit("channel.workTagsSet"){$0.tags.append(value)}};tagInput=""}}
                revisionControl("channel.workTagsSet")
                submitButton("保存本群标签","channel.workTagsSet")
                ForEach(operations,id:\.self){op in
                    ForEach(drafts.uncertain(key(op)),id:\.requestID){snapshot in
                        Text("原提交结果未确认，输入保留；不会重新发送。").font(.caption).foregroundStyle(.orange)
                        Button("查询原请求结果"){lookup(snapshot)}.font(.caption).disabled(reading || !current)
                    }
                }
                Button("重新读取源版本与本群引用"){Task{await read()}}.disabled(reading || group.writing || !current)
                Text("源版本："+(source["revision"].string ?? "未读取")+"；本群引用版本："+(group.linkRevision ?? "未读取")).font(.caption).foregroundStyle(.secondary)
                Button("关闭"){dismiss()}
            }.padding(20)
        }.modifier(CrewdAdaptiveFormFrame(width:560,height:620))
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
        }}.disabled(reading || !current || group.writing || source == .null || !drafts.uncertain(key(operation)).isEmpty)
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
        VStack(alignment:.leading,spacing:14){
            Text("新建人待办").font(.headline)
            TextField("需要人处理的事项",text:Binding(get:{drafts.fields(key).text},set:{value in var f=drafts.fields(key);f.text=value;drafts.edit(key,f)}),axis:.vertical).lineLimit(3...8)
            Text("保存到当前群，不发送给 Agent，也不启动会话。").font(.caption).foregroundStyle(.secondary)
            if let failure{Text(failure).foregroundStyle(.orange).font(.caption)}
            ForEach(drafts.uncertain(key),id:\.requestID){snapshot in
                Text(snapshot.fields.text).textSelection(.enabled)
                Button("查询原创建结果"){Task{do{if try await group.lookup(snapshot) != nil{drafts.finish(snapshot,accepted:true);dismiss()}else{failure="原请求当前没有已提交回执；输入保留，不会重发。"}}catch{failure=error.localizedDescription}}}
            }
            HStack{Button("关闭"){dismiss()};Spacer();Button("保存"){guard let snapshot=drafts.begin(key) else{return};Task{do{_ = try await group.submit(snapshot);drafts.finish(snapshot,accepted:true);if current{dismiss()}}catch{drafts.finish(snapshot,accepted:false,uncertain:CrewdClientError.isUncertainSubmission(error));if current{failure=error.localizedDescription}}}}.disabled(!current || group.writing || drafts.fields(key).text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || !drafts.uncertain(key).isEmpty)}
        }.padding(20).modifier(CrewdAdaptiveFormFrame(width:520,height:360))
        .onAppear{crew=group.crew;channel=group.channel}
    }
}
