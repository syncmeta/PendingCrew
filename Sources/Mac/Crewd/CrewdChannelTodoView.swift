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
                composer
                ScrollView {
                    VStack(alignment:.leading,spacing:8) {
                        if group.loaded,group.items.isEmpty {Text("本会话尚无关联工作").font(.caption).foregroundStyle(.secondary)}
                        ForEach(group.items.indices,id:\.self){i in
                            let row=group.items[i]
                            if let id=CrewdChannelWorkStore.workID(row["object"]) {
                                VStack(alignment:.leading,spacing:4){
                                    Button{select(row)}label:{Text(id.raw+"  "+(row["label"].string ?? "")+(row["labelTruncated"] == .bool(true) ? "…":"")).font(.caption).multilineTextAlignment(.leading)}.buttonStyle(.plain)
                                    Text(id.kind == .human && row["status"] == .string("open") ? "待答复":CrewdWorkStatusLabel.display(row["status"].string ?? "")).font(.caption2).foregroundStyle(.secondary)
                                    if selected?["object"] == row["object"] {selectedDetails(row,id)}
                                }.padding(6).frame(maxWidth:.infinity,alignment:.leading).background(.quaternary.opacity(0.3),in:RoundedRectangle(cornerRadius:5))
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
            }else if id.kind == .human,detail["state"] == .string("open"),let owner=row["crewId"].string {
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
