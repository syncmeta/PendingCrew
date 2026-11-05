import SwiftUI

/// The existing global search surface supplies its one query field.
struct CrewdOrganizationSearchView:View {
    typealias Wire=CrewdOrganizationSearchWire
    typealias Store=CrewdOrganizationSearchStore
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject private var client:CrewdClient
    let query:String
    let selected:()->Void
    @StateObject private var store:Store
    @State private var domain=Wire.Domain.all
    @State private var kind="all"
    @State private var area="all"
    @State private var tableOnly=false
    @State private var tableID=""
    @State private var includeDefaults=false
    @State private var hasStart=false
    @State private var hasEnd=false
    @State private var start=Date().addingTimeInterval(-30*86400)
    @State private var end=Date()
    @State private var visible=false
    @State private var appearance=UUID()
    @State private var refresh=UUID()
    @State private var navigation=UUID()
    @State private var opening=false
    @State private var openFailure:String?
    @State private var expanded:ExpansionTarget?
    @State private var needsContext:CrewdManagedTableV2Wire.Target?
    @State private var viewingCrew=""
    @State private var contextSource:Data?
    struct ExpansionTarget:Identifiable {let id=UUID();let hit:Wire.Hit}
    init(workspace:CrewdWorkspace,query:String,selected:@escaping ()->Void) {
        self.workspace=workspace;self.client=workspace.client;self.query=query;self.selected=selected
        _store=StateObject(wrappedValue:workspace.makeOrganizationSearchStore())
    }
    private func dateString(_ date:Date)->String {ISO8601DateFormatter().string(from:date)}
    private var requested:Wire.Query {
        let channel=area=="channel",singleTable=tableOnly && !channel
        let defaultsAllowed = !channel && !singleTable && domain != .messages && (kind=="all" || kind==Wire.Kind.decision.rawValue)
        return .init(query:query,domain:channel ? .messages:singleTable ? .objects:domain,
                     kinds:channel ? [.message]:singleTable ? [.row]:Wire.Kind(rawValue:kind).map{[$0]},
                     tableId:singleTable ? tableID:nil,crewId:area=="crew" ? workspace.crewID:nil,channelId:channel ? workspace.channelID:nil,
                     afterTime:hasStart ? dateString(start):nil,beforeTime:hasEnd ? dateString(end):nil,includeSystemDefaults:includeDefaults && defaultsAllowed)
    }
    private var requestKey:Data {var raw=requested.identity;raw.append(0);raw.append(store.sourceKey);raw.append(Data((appearance.uuidString+refresh.uuidString).utf8));return raw}
    private var allowed:Bool {visible && workspace.client.readyForWork && !workspace.isRetired && store.sourceCurrent}
    private var kinds:[Wire.Kind] {Wire.Kind.allCases.filter{domain == .all || (domain == .messages ? $0 == .message:$0 != .message)}}
    private struct Group:Identifiable {let id:String;var title:String;var items:[Wire.Hit]}
    private var groups:[Group] {
        var result:[Group]=[],indexes:[String:Int]=[:]
        for hit in store.items {
            if let index=indexes[hit.group] {result[index].items.append(hit);result[index].title=hit.groupTitle}
            else {indexes[hit.group]=result.count;result.append(.init(id:hit.group,title:hit.groupTitle,items:[hit]))}
        }
        return result
    }
    var body:some View {
        VStack(alignment:.leading,spacing:10) {
            DisclosureGroup("筛选搜索范围") {ScrollView{filters}.frame(maxHeight:180)}.font(.subheadline)
            HStack {Text("当前电脑的可见消息与组织对象").font(.caption).foregroundStyle(.secondary);Spacer();Button("重新搜索"){refresh=UUID()}.disabled(store.busy || !workspace.client.readyForWork)}
            if !store.sourceCurrent && store.phase != .idle && store.phase != .unavailable {Text("原搜索来源或权限已变化，请重新搜索。").foregroundStyle(.secondary)}
            else {
                if let snapshot=store.snapshot {snapshotLabel(snapshot)}
                phaseLabel(store.phase,failure:store.failure,busy:store.busy)
                if let openFailure {Text(openFailure).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
                if allowed,let needsContext {viewingContext(needsContext)}
                if allowed {ScrollView {LazyVStack(alignment:.leading,spacing:12) {
                    ForEach(groups){group in
                        Text(group.title).font(.headline).padding(.top,5)
                        ForEach(group.items){hit in result(hit)}
                    }
                    if store.phase == .ready && store.items.isEmpty {Text(store.nextCursor==nil ? "未找到当前可见的匹配结果。":"这一页没有当前可见结果，仍可继续读取。").foregroundStyle(.secondary)}
                    if store.nextCursor != nil {Button(store.busy ? "正在读取…":store.phase == .building ? "再检查原继续位置":"继续读取结果") {let key=requestKey;Task{await store.more(current:{visible && requestKey==key})}}.disabled(store.busy)}
                }.frame(maxWidth:.infinity,alignment:.leading)}}
            }
            if opening {ProgressView("正在核对并打开…").controlSize(.small)}
        }.padding(10)
        .onAppear{visible=true;appearance=UUID()}
        .onDisappear{visible=false;appearance=UUID();navigation=UUID();store.cancel()}
        .task(id:requestKey) {
            let key=requestKey;guard visible,!Task.isCancelled else{return}
            expanded=nil;needsContext=nil;navigation=UUID();opening=false;openFailure=nil
            if contextSource != store.sourceKey {contextSource=store.sourceKey;viewingCrew=""}
            store.cancel()
            if !query.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty {
                do{try await Task.sleep(nanoseconds:250_000_000)}catch{return}
            }
            guard visible,!Task.isCancelled,requestKey==key else{return}
            await store.start(requested,current:{visible && requestKey==key})
        }
        .onReceive(workspace.authorityEvents){event in store.handleEvent(event.0,connection:workspace.client.connectionEpoch)}
        .sheet(item:$expanded,onDismiss:{store.cancelExpansion()}) {target in
            CrewdOrganizationExpansionView(store:store,target:target.hit.target,title:target.hit.title,includeSystemDefaults:requested.includeSystemDefaults,
                current:{visible && expanded?.id==target.id && store.sourceCurrent},open:{neighbor,metadata in open(neighbor,metadataCrewId:metadata)})
        }
    }
    private var filters:some View {VStack(alignment:.leading,spacing:8) {
        Picker("内容",selection:$domain){ForEach(Wire.Domain.allCases){Text($0.title).tag($0)}}.disabled(tableOnly || area=="channel")
            .onChange(of:domain){_,_ in if let chosen=Wire.Kind(rawValue:kind),!kinds.contains(chosen){kind="all"}}
        if !tableOnly && area != "channel" {Picker("类型",selection:$kind){Text("全部类型").tag("all");ForEach(kinds){Text($0.title).tag($0.rawValue)}}}
        Picker("范围",selection:$area) {
            Text("当前电脑全部可见范围").tag("all")
            if !workspace.crewID.isEmpty {Text("当前机组").tag("crew")}
            if !workspace.channelID.isEmpty && !tableOnly {Text("当前会话（仅消息）").tag("channel")}
        }
        if area != "channel" {Toggle("仅搜索一张组织表的记录",isOn:$tableOnly);if tableOnly {TextField("组织表编号，例如 T12",text:$tableID).textFieldStyle(.roundedBorder)}}
        if area != "channel" && domain != .messages {Toggle("包含系统默认拍板的决定",isOn:$includeDefaults)}
        Toggle("限定起始时间",isOn:$hasStart);if hasStart {DatePicker("从",selection:$start,displayedComponents:[.date,.hourAndMinute])}
        Toggle("限定结束时间",isOn:$hasEnd);if hasEnd {DatePicker("到",selection:$end,displayedComponents:[.date,.hourAndMinute])}
        if hasStart || hasEnd {Text("按原消息时间或对象的实际变更时间筛选；不把查询时间当作变更时间。").font(.caption).foregroundStyle(.secondary)}
    }}
    private func result(_ hit:Wire.Hit)->some View {VStack(alignment:.leading,spacing:5) {
        Button {open(hit.target)} label:{VStack(alignment:.leading,spacing:4){
            Text(hit.title.isEmpty ? "（无标题）":hit.title).font(.headline)
            Text(hit.target.label).font(.caption).foregroundStyle(.secondary).lineLimit(2)
            if let excerpt=hit.excerpt,!excerpt.isEmpty {Text(excerpt).lineLimit(3)}
        }.frame(maxWidth:.infinity,alignment:.leading).contentShape(Rectangle())}.buttonStyle(.plain).disabled(opening)
        HStack {Text(hit.status.title);Text(hit.matchedOn.title);if let crew=hit.ownerCrewId {Text("机组 "+crew)}}.font(.caption).foregroundStyle(.secondary)
        if hit.kind == .artifact {Text("未核验"+(hit.target.reference?.namespace.kind=="unassigned" ? " · 仓库未归属":"")+" · 关联归属见详情").font(.caption).foregroundStyle(.secondary)}
        if let changed=hit.changedAt.flatMap(Wire.time) {Text("实际变更："+changed.formatted(date:.abbreviated,time:.shortened)).font(.caption).foregroundStyle(.secondary)}
        if hit.kind == .message {Text("查看消息所属的会话。").font(.caption).foregroundStyle(.secondary)}
        Button("展开一跳关联"){expanded = .init(hit:hit)}.font(.caption).disabled(opening)
    }.padding(9).frame(maxWidth:.infinity,alignment:.leading).background(.quaternary,in:RoundedRectangle(cornerRadius:8))}
    private func viewingContext(_ target:CrewdManagedTableV2Wire.Target)->some View {VStack(alignment:.leading,spacing:6) {
        Text("选择一个机组作为查看范围；这不会改变对象归属。").font(.caption).foregroundStyle(.secondary)
        Picker("查看范围",selection:$viewingCrew){Text("请选择机组").tag("");ForEach(workspace.crews.indices,id:\.self){index in
            let crew=workspace.crews[index]
            if let id=crew["id"].string {Text(crew["title"].string ?? id).tag(id)}
        }}
        Button("继续打开此结果"){self.needsContext=nil;open(target,presentationCrewId:viewingCrew)}.disabled(viewingCrew.isEmpty || opening)
        Button("取消"){needsContext=nil}.font(.caption)
    }}
    private func open(_ target:CrewdManagedTableV2Wire.Target,metadataCrewId:String?=nil,presentationCrewId:String?=nil) {
        guard allowed,!opening else{return}
        if workspace.crewID.isEmpty,metadataCrewId==nil,presentationCrewId==nil,[CrewdManagedTableV2Wire.TargetKind.artifact,.human].contains(target.kind) {
            expanded=nil;needsContext=target;openFailure=nil;return
        }
        needsContext=nil;let id=UUID();navigation=id;opening=true;openFailure=nil
        let source=workspace.authorityRecordID,connection=workspace.client.connectionEpoch,typedQuery=Data(query.utf8),filters=requested.identity
        @MainActor func current()->Bool {visible && navigation==id && Data(query.utf8)==typedQuery && requested.identity==filters && (presentationCrewId==nil || viewingCrew.utf8.elementsEqual(presentationCrewId!.utf8)) && workspace.authorityRecordID==source && workspace.client.connectionEpoch==connection && store.sourceCurrent}
        Task {defer{if navigation==id{opening=false}}
            do {
                try await workspace.openManagedBusinessTarget(target,metadataCrewId:metadataCrewId,presentationCrewId:presentationCrewId,current:current)
                // Selecting the verified destination may deliberately change crew/route.
                guard visible,navigation==id,Data(query.utf8)==typedQuery,workspace.authorityRecordID==source,workspace.client.connectionEpoch==connection else{return}
                expanded=nil;selected()
            }catch{if current(){
                if let code=(error as? CrewdClientError)?.code,[-32003,-32004].contains(code) {store.invalidateVisibleResults();openFailure=nil}
                else {openFailure=error.localizedDescription}
            }}
        }
    }
    @ViewBuilder private func snapshotLabel(_ snapshot:Wire.Snapshot)->some View {
        if let time=Wire.time(snapshot.sortedAt) {Text("排序范围固定于 "+time.formatted(date:.abbreviated,time:.shortened)+"；每页按当前权限核对内容。").font(.caption).foregroundStyle(.secondary)}
    }
    @ViewBuilder private func phaseLabel(_ phase:Store.Phase,failure:String?,busy:Bool)->some View {
        if phase == .idle {Text("输入文字或编号，查找消息、计划和组织对象。").foregroundStyle(.secondary)}
        if busy {ProgressView(phase == .building ? "正在整理可搜索内容…":"正在搜索…").controlSize(.small)}
        if phase == .building && !busy {
            if store.snapshot != nil {Text("后续内容仍在整理中；已读结果和原继续位置保留，排序范围与到期时间未延长。").foregroundStyle(.secondary)}
            else {Text("可搜索内容仍在整理中，本次已完成有限次检查。").foregroundStyle(.secondary);Button("再检查一次"){refresh=UUID()}}
        }
        if phase == .expired {Text("搜索已过期，请重新搜索。").foregroundStyle(.orange)}
        else if let failure {Text(failure).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
    }
}

private struct CrewdOrganizationExpansionView:View {
    typealias Store=CrewdOrganizationSearchStore
    typealias Wire=CrewdOrganizationSearchWire
    @ObservedObject var store:Store
    let target:CrewdManagedTableV2Wire.Target
    let title:String
    let includeSystemDefaults:Bool
    let current:@MainActor ()->Bool
    let open:(CrewdManagedTableV2Wire.Target,String?)->Void
    @Environment(\.dismiss) private var dismiss
    @State private var visible=false
    @State private var appearance=UUID()
    @State private var refresh=UUID()
    private var key:String {appearance.uuidString+refresh.uuidString}
    var body:some View {NavigationStack {
        VStack(alignment:.leading,spacing:12) {
            if current() {Text(title.isEmpty ? target.label:title).font(.headline).textSelection(.enabled)}
            Text("这里显示真实的正向与反向关系；同一对象可以因不同关系出现多次。").font(.caption).foregroundStyle(.secondary)
            if !current(){Text("原搜索来源已变化，请关闭后重新搜索。").foregroundStyle(.secondary)}
            else {
                if let failure=store.expansionFailure {Text(failure).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
                if store.expansionPhase == .building {Text(store.expansionSnapshot==nil ? "关联仍在整理中；这不是没有关联。":"后续关联仍在整理中；原结果与继续位置保留，到期时间不延长。").foregroundStyle(.secondary)}
                if store.expansionPhase == .expired {Text("关联查询已过期，请重新展开。").foregroundStyle(.orange)}
                if store.expansionBusy {ProgressView("正在核对关联…")}
                ScrollView {LazyVStack(alignment:.leading,spacing:12) {
                    ForEach(store.neighbors){item in
                        VStack(alignment:.leading,spacing:5) {
                            Text(item.direction.title+" · "+item.edge.origin.title).font(.subheadline)
                            Button(item.neighbor.summary.isEmpty ? item.neighbor.target.label:item.neighbor.summary){
                                guard visible,current() else{return}
                                open(item.neighbor.target,item.direction == .incoming && item.edge.source.kind=="artifact" ? item.edge.source.metadataCrewId:nil)
                            }.buttonStyle(.plain)
                            Text(item.neighbor.target.label).font(.caption).foregroundStyle(.secondary)
                            Text("关联归属机组 "+item.edge.ownerCrewId+" · "+item.status.title).font(.caption).foregroundStyle(.secondary)
                            if item.edge.origin.origin=="adapter" {Text("原业务只读关系；需修改时请进入原业务操作。").font(.caption).foregroundStyle(.secondary)}
                            if item.neighbor.target.kind == .message {Text("打开所在会话").font(.caption).foregroundStyle(.secondary)}
                        }.padding(10).frame(maxWidth:.infinity,alignment:.leading).background(.quaternary,in:RoundedRectangle(cornerRadius:8))
                    }
                    if store.expansionPhase == .ready && store.neighbors.isEmpty {Text(store.expansionNext==nil ? "本次未读到当前可见的一跳关系。":"本页没有当前可见关系，仍可继续读取。").foregroundStyle(.secondary)}
                    if store.expansionNext != nil {Button(store.expansionPhase == .building ? "再检查原关联页":"继续读取关联"){Task{await store.expandMore(current:{visible && current()})}}.disabled(store.expansionBusy)}
                }}
            }
        }.padding(16).navigationTitle("一跳关联")
        .toolbar{ToolbarItem(placement:.cancellationAction){Button("关闭"){dismiss()}};ToolbarItem(placement:.primaryAction){Button("重新展开"){refresh=UUID()}.disabled(store.expansionBusy || !current())}}
    }
    #if os(macOS)
    .frame(minWidth:440,minHeight:440)
    #endif
    .onAppear{visible=true;appearance=UUID()}
    .onDisappear{visible=false;appearance=UUID();store.cancelExpansion()}
    .task(id:key){let captured=key;await store.expand(target,includeSystemDefaults:includeSystemDefaults,current:{visible && key==captured && current()})}
    }
}


/// A navigation observation only; never an authority grant or a resume intent.
@MainActor struct CrewdObservedSessionRoute:Identifiable {
    let id=UUID()
    let sessionID:String
    let crewID:String
    private let source:CrewdSessionDisplayWitness
    init?(workspace:CrewdWorkspace) {
        guard let navigation=workspace.currentManagedBusinessNavigation,navigation.target.kind == .session,let session=navigation.target.idValue,
              workspace.sessionID.utf8.elementsEqual(session.utf8),navigation.crew.utf8.elementsEqual(workspace.crewID.utf8),
              let bytes=CrewdSessionDisplayWitness.scopeBytes(workspace.presentationScope) else{return nil}
        sessionID=session;crewID=workspace.crewID
        source = .init(authority:workspace.authorityRecordID,connection:workspace.client.connectionEpoch,observation:workspace.observationGeneration,scope:bytes)
    }
    func current(_ workspace:CrewdWorkspace)->Bool {
        !workspace.isRetired && workspace.client.readyForWork && source.matches(authority:workspace.authorityRecordID,connection:workspace.client.connectionEpoch,observation:workspace.observationGeneration,scope:CrewdSessionDisplayWitness.scopeBytes(workspace.presentationScope)) &&
        workspace.crewID.utf8.elementsEqual(crewID.utf8) && workspace.sessionID.utf8.elementsEqual(sessionID.utf8)
    }
}

/// Used only when there is no proven same-source local terminal projection.
/// Existing session APIs supply the facts; no controls or model launch are added.
struct CrewdManagedSessionReadback:View {
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject private var client:CrewdClient
    let route:CrewdObservedSessionRoute
    @Environment(\.dismiss) private var dismiss
    @State private var inspection:CrewdValue = .null
    @State private var session:CrewdValue = .null
    @State private var visible=false
    @State private var token=UUID()
    @State private var appearance=UUID()
    @State private var busy=false
    @State private var failure:String?
    init(workspace:CrewdWorkspace,route:CrewdObservedSessionRoute) {self.workspace=workspace;self.client=workspace.client;self.route=route}
    private var current:Bool {visible && route.current(workspace)}
    var body:some View {NavigationStack {
        VStack(alignment:.leading,spacing:12) {
            if !route.current(workspace) {Text("成员详情的来源已变化，请关闭后重新打开。").foregroundStyle(.secondary)}
            else {
                Text("后台返回的只读成员详情").font(.headline)
                if current {Text(session["displayName"].string ?? inspection["title"].string ?? route.sessionID).textSelection(.enabled)}
                if let failure {Text(failure).font(.caption).foregroundStyle(.orange)}
                if busy {ProgressView("正在读取成员…")}
                if current,inspection != .null {
                    if let running=running {Text(running ? "后台记录：正在运行":"后台记录：尚未运行").font(.caption).foregroundStyle(.secondary)}
                    ScrollView {
                        if let screen=inspection["screen"].string,!screen.isEmpty {Text(screen).font(.system(.footnote,design:.monospaced)).textSelection(.enabled).frame(maxWidth:.infinity,alignment:.leading)}
                        else {Text("当前只读快照没有附带输出正文。").foregroundStyle(.secondary)}
                    }
                }
            }
        }.padding(18).navigationTitle("成员详情")
        .toolbar{ToolbarItem(placement:.cancellationAction){Button("关闭"){dismiss()}};ToolbarItem(placement:.primaryAction){Button("刷新"){Task{await load()}}.disabled(busy || !route.current(workspace))}}
    }
    #if os(macOS)
    .frame(minWidth:480,minHeight:380)
    #endif
    .onAppear{visible=true;token=UUID();appearance=UUID()}.onDisappear{visible=false;token=UUID();appearance=UUID();inspection = .null;session = .null}
    .task(id:route.id.uuidString+appearance.uuidString){await load()}
    }
    private var running:Bool? {if case .bool(let value)=inspection["running"]{return value};if case .bool(let value)=inspection["live"]["running"]{return value};return nil}
    private func load()async {
        guard current,!busy else{return};let request=UUID();token=request;busy=true
        @MainActor func valid()->Bool {current && token==request && !Task.isCancelled}
        defer{if token==request{busy=false}}
        do {
            let rows=try await workspace.readValue("session.list",.object(["crewId":.string(route.crewID)]));guard valid() else{return}
            guard case .array(let rows)=rows else{throw CrewdOrganizationSearchWire.Problem.invalid}
            let matches=rows.filter{$0["id"].string?.utf8.elementsEqual(route.sessionID.utf8)==true && $0["crewId"].string?.utf8.elementsEqual(route.crewID.utf8)==true}
            guard matches.count==1 else{throw CrewdOrganizationSearchWire.Problem.sourceChanged}
            let value=try await workspace.readValue("session.inspect",.object(["sessionId":.string(route.sessionID),"maxLines":.integer(200)]));guard valid() else{return}
            let ids=[value["sessionId"].string,value["live"]["sessionId"].string].compactMap{$0}
            guard !ids.isEmpty,ids.allSatisfy({$0.utf8.elementsEqual(route.sessionID.utf8)}) else{throw CrewdOrganizationSearchWire.Problem.invalid}
            session=matches[0];inspection=value;failure=nil
        }catch{if valid(){inspection = .null;session = .null;failure=error.localizedDescription}}
    }
}

#if os(iOS)
/// The existing mobile detail is rendered only inside its exact originating workspace.
struct CrewdObservedIOSSessionDetail:View {
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject private var client:CrewdClient
    let route:CrewdObservedSessionRoute
    init(workspace:CrewdWorkspace,route:CrewdObservedSessionRoute) {self.workspace=workspace;self.client=workspace.client;self.route=route}
    var body:some View {Group {
        if route.current(workspace) {IOSRemoteSessionDetail(workspace:workspace,sessionID:route.sessionID)}
        else {ContentUnavailableView("成员来源已变化",systemImage:"person.crop.circle.badge.exclamationmark",description:Text("请关闭后重新打开成员详情。"))}
    }}
}
#endif
