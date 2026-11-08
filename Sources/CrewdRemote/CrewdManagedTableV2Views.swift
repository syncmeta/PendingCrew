import SwiftUI

/// Shared organization table UI; private SQLite tables retain their existing separate entry.
struct CrewdManagedTableV2View:View {
    typealias Wire=CrewdManagedTableV2Wire
    typealias Store=CrewdManagedTableV2Store
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var store:Store
    var initialID:String?=nil
    @State private var scope:Store.Scope?
    @State private var tables:[Wire.Table]=[]
    @State private var selected:Wire.Table?
    @State private var rows:[Wire.Row]=[]
    @State private var directRow:Wire.Row?
    @State private var openedInitial=false
    @State private var visible=true
    @State private var next:String?
    @State private var busy=false
    @State private var loaded=false
    @State private var token=UUID()
    @State private var refreshQueued=false
    @State private var pendingDestination:String?
    @State private var failure:String?
    @State private var notice:String?
    @State private var pending:[Store.Pending]=[]
    @State private var editor:CrewdManagedV2EditorTarget?
    private var route:String {[workspace.authorityRecordID.uuidString,workspace.observationGeneration.uuidString,workspace.client.connectionEpoch.uuidString,workspace.crewID,String(workspace.originalJournalReady),String(workspace.client.readyForWork)].joined(separator:"/")}
    private var allowed:Bool {visible && store.supportsTables && workspace.client.readyForWork && workspace.originalJournalReady && !workspace.isRetired}
    private var current:Bool {allowed && scope.map(store.owns)==true}
    var body:some View {
        VStack(alignment:.leading,spacing:12) {
            HStack {
                Text("组织表").font(.headline);Spacer()
                Button("刷新",systemImage:"arrow.clockwise") {Task{await load(selected?.id)}}.disabled(busy)
                Button("新建组织表",systemImage:"plus") {if let scope {editor = .init(mode:.create,scope:scope,route:route)}}.disabled(!current || busy)
            }
            Text("关联列使用对象的稳定编号；修改记录不会复制或改写原业务状态。").font(.caption).foregroundStyle(.secondary)
            if let notice {Text(notice).font(.caption).textSelection(.enabled)}
            if let failure {Text(failure).font(.caption).foregroundStyle(.orange).textSelection(.enabled)}
            if let scope,current {recovery(scope)}
            if let table=selected,current {
                HStack {Button("返回组织表目录") {Task{await load(nil)}};Spacer();if !table.frozen {Button("管理列与表定义") {if let scope {editor = .init(mode:.schema(table),scope:scope,route:route)}}}}
                Text(table.title+" · "+table.id).font(.title3)
                Text("所属机组 "+table.crewId+" · 定义版本 "+table.schemaRevision+(table.frozen ? " · 已归档，只读":"")).font(.caption).foregroundStyle(.secondary)
                if let purpose=table.purpose,!purpose.isEmpty {Text(purpose).textSelection(.enabled)}
                Text("写权限："+table.rowWritePolicy.title+" · "+table.author.title).font(.caption)
                CrewdManagedLinksEntry(workspace:workspace,source:.init(kind:"table",id:table.id),current:{current && selected?.id==table.id})
                CrewdArtifactSubjectEntry(workspace:workspace,subject:.init(kind:.table,id:table.id),crew:table.crewId,frozen:table.frozen,current:{current && selected?.id==table.id})
                if !table.frozen {Button("新增记录",systemImage:"plus") {if let scope {editor = .init(mode:.add(table),scope:scope,route:route)}}.disabled(busy)}
                ScrollView {LazyVStack(alignment:.leading,spacing:12) {if let directRow,!rows.contains(where:{$0.id==directRow.id}) {Text("通过关联打开的记录；下面列表仍按实际分页读取。").font(.caption);rowView(directRow,table:table)};ForEach(rows){row in rowView(row,table:table)}
                    if let next {Button("更多记录") {Task{await more(next,table:table)}}.disabled(busy)}
                    if rows.isEmpty && !busy {Text("本次读取没有记录。").foregroundStyle(.secondary)}
                }}
            } else if current {
                ScrollView {LazyVStack(alignment:.leading,spacing:10) {ForEach(tables){table in
                    Button {Task{await load(table.id)}} label:{VStack(alignment:.leading){Text(table.title);Text(table.id+" · "+table.crewId+(table.frozen ? " · 已归档":"")).font(.caption).foregroundStyle(.secondary)}.frame(maxWidth:.infinity,alignment:.leading)}.buttonStyle(.plain)
                };if let next {Button("更多组织表") {Task{await more(next,table:nil)}}.disabled(busy)}
                    if loaded && tables.isEmpty && !busy {Text("本次读取没有组织表。").foregroundStyle(.secondary)}
                }}
            } else if !busy {Text("当前表格范围尚未核对，或原连接已经改变。").foregroundStyle(.secondary)}
            if busy {ProgressView().controlSize(.small)}
        }.padding(16)
        .task(id:route) {let wanted=directRow?.id ?? selected?.id ?? (openedInitial ? nil:initialID);openedInitial=true;token=UUID();busy=false;scope=nil;selected=nil;tables=[];rows=[];directRow=nil;next=nil;loaded=false;pendingDestination=nil;await load(wanted)}
        .onChange(of:store.invalidation) {_,_ in refreshQueued=true;refreshIfReady()}
        .onChange(of:editor?.id) {_,_ in refreshIfReady()}
        .onChange(of:busy) {_,_ in refreshIfReady()}
        .onAppear{visible=true}
        .onDisappear {visible=false;token=UUID();busy=false}
        .sheet(item:$editor) {target in
            CrewdManagedTableV2Editor(workspace:workspace,store:store,target:target,current:{editor?.id==target.id && route==target.route && store.owns(target.scope)},saved:{observation in
                guard editor?.id==target.id,route==target.route else {return}
                notice="原请求已确认 · "+observation.receipt.effect.tableId+" · "+observation.receipt.requestId
                pendingDestination=observation.receipt.effect.tableId;refreshQueued=true;editor=nil;refreshIfReady()
            },attempted:{if let scope {refreshPending(scope)}})
        }
    }
    private func rowView(_ row:Wire.Row,table:Wire.Table)->some View {
        VStack(alignment:.leading,spacing:8) {
            HStack {Text(row.scalar(table.titleFieldId) == .null ? row.id:row.scalar(table.titleFieldId).display).font(.headline);Text(row.id).font(.caption).foregroundStyle(.secondary);Spacer()
                if !row.frozen {Button("编辑记录") {if let scope {editor = .init(mode:.patch(table,row),scope:scope,route:route)}}.disabled(busy)}
            }
            CrewdManagedLinksEntry(workspace:workspace,source:.init(kind:"row",id:row.id),current:{current && selected?.id==table.id})
            CrewdArtifactSubjectEntry(workspace:workspace,subject:.init(kind:.row,id:row.id),crew:row.crewId,frozen:row.frozen,current:{current && selected?.id==table.id})
            ForEach(table.active){field in
                VStack(alignment:.leading,spacing:4) {
                    Text(field.name).font(.subheadline)
                    if field.kind.link {
                        if let cell=row.links(field.id) {
                            ForEach(cell.visible){link in CrewdManagedTargetButton(workspace:workspace,target:link.target,summary:link.summary,current:{current})}
                            if cell.hiddenSet != nil {Text("部分关联当前不可见；其原关系仍保留。").font(.caption).foregroundStyle(.secondary)}
                            if cell.incompleteRequiredLink {Text("原必填关联目标已删除，需要重新补充；其他字段仍可编辑。").font(.caption).foregroundStyle(.orange)}
                            if cell.visible.isEmpty && cell.hiddenSet==nil && !cell.incompleteRequiredLink {Text("空值").foregroundStyle(.secondary)}
                        } else {Text("空值").foregroundStyle(.secondary)}
                    } else {Text(row.scalar(field.id).display).textSelection(.enabled)}
                }
            }
            Text("记录版本 "+row.revision+" · "+row.author.title).font(.caption).foregroundStyle(.secondary)
        }.padding(10).frame(maxWidth:.infinity,alignment:.leading).background(.quaternary,in:RoundedRectangle(cornerRadius:8))
    }
    private func refreshPending(_ scope:Store.Scope) {do {pending=try store.pending(scope)}catch{failure=error.localizedDescription}}
    private func recovery(_ scope:Store.Scope)->some View {VStack(alignment:.leading,spacing:6) {ForEach(pending.filter{$0.witness.table != nil}){original in
        Text("表格原请求待核对 · "+original.witness.requestId).font(.caption).textSelection(.enabled)
        Button("只读查询原回执") {busy=true;let captured=token;Task{defer{if token==captured{busy=false}};do {let observed=try await store.lookup(original,scope:scope,current:{current && token==captured});if current && token==captured {notice=observed.map{($0.historical ? "历史原请求已核对 · ":"原请求已核对 · ")+$0.receipt.requestId} ?? "原结果尚未知，未重新提交。";refreshPending(scope)}}catch{if current{failure=error.localizedDescription}}}}.disabled(busy)
    }}}
    private func refreshIfReady() {guard refreshQueued,!busy,editor==nil,allowed else {return};refreshQueued=false;let destination=pendingDestination ?? selected?.id;Task{await load(destination)}}
    private func load(_ id:String?) async {
        guard allowed,!busy else {return};busy=true;let captured=route,request=UUID();token=request
        @MainActor func valid()->Bool {route==captured && token==request && allowed}
        defer {if token==request {busy=false}}
        do {
            let s=try await store.scope(stillCurrent:valid)
            guard valid() else{return};scope=s;refreshPending(s);loaded=false
            if let id,id.hasPrefix("O") {
                let (table,row)=try await store.row(id,scope:s,current:valid)
                let page=try await store.rows(table.id,scope:s,after:nil,current:valid)
                guard valid(),page.table.schemaRevision==table.schemaRevision else {throw Store.Problem.stale}
                selected=table;directRow=row;rows=page.items;next=page.nextAfter
            } else if let id {
                let page=try await store.rows(id,scope:s,after:nil,current:valid);guard valid() else {return}
                selected=page.table;directRow=nil;rows=page.items;next=page.nextAfter
            } else {
                let page=try await store.tables(s,after:nil,current:valid);guard valid() else {return}
                tables=page.items;selected=nil;rows=[];directRow=nil;next=page.nextAfter
            }
            guard valid() else {return};scope=s;loaded=true;failure=nil;if id==pendingDestination{pendingDestination=nil};refreshPending(s)
        } catch {if valid(){failure=error.localizedDescription;selected=nil;rows=[];tables=[];next=nil}}
    }
    private func more(_ cursor:String,table:Wire.Table?) async {
        guard let scope,current,!busy else {return};busy=true;let captured=route,request=UUID();token=request
        @MainActor func valid()->Bool {current && route==captured && token==request}
        defer {if token==request {busy=false}}
        do {
            if let table {
                let page=try await store.rows(table.id,scope:scope,after:cursor,current:valid)
                guard page.table.schemaRevision==table.schemaRevision,Set(rows.map(\.id)).isDisjoint(with:page.items.map(\.id)) else {throw Store.Problem.stale}
                if valid(){rows+=page.items;next=page.nextAfter}
            } else {
                let page=try await store.tables(scope,after:cursor,current:valid)
                guard Set(tables.map(\.id)).isDisjoint(with:page.items.map(\.id)) else {throw Store.Problem.invalid}
                if valid(){tables+=page.items;next=page.nextAfter}
            }
        } catch {if valid(){failure="后续页未读取；现有列表仍不完整。 "+error.localizedDescription}}
    }
}

struct CrewdManagedV2EditorTarget:Identifiable {
    enum Mode {case create,schema(CrewdManagedTableV2Wire.Table),add(CrewdManagedTableV2Wire.Table),patch(CrewdManagedTableV2Wire.Table,CrewdManagedTableV2Wire.Row)}
    let id=UUID()
    let mode:Mode
    let scope:CrewdManagedTableV2Store.Scope
    let route:String
}

private struct CrewdManagedTableV2Editor:View {
    typealias Wire=CrewdManagedTableV2Wire
    typealias Store=CrewdManagedTableV2Store
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var store:Store
    let target:CrewdManagedV2EditorTarget
    let current:@MainActor ()->Bool
    let saved:@MainActor (Store.Observation)->Void
    let attempted:@MainActor ()->Void
    @Environment(\.dismiss) private var dismiss
    struct LocalField:Identifiable {let id=UUID();var draft:Wire.FieldDraft}
    @State private var title=""
    @State private var purpose=""
    @State private var policy=Wire.Policy.authorCaptainHuman
    @State private var draftFields=[LocalField(draft:.init(name:"名称",kind:.init(kind:"text"),nullable:false))]
    @State private var titleIndex=0
    @State private var schemaAction="addField"
    @State private var fieldId=""
    @State private var label=""
    @State private var newField=Wire.FieldDraft(name:"",kind:.init(kind:"text"),nullable:true)
    @State private var text:[String:String]=[:]
    @State private var nulls:Set<String>=[]
    @State private var linkValues:[String:[Wire.LinkWrite]]=[:]
    @State private var edited:Set<String>=[]
    @State private var selectingLink:Wire.Field?
    @State private var confirming=false
    @State private var busy=false
    @State private var stopped=false
    @State private var problem:String?
    @State private var original:Store.Pending?
    @State private var initialized=false
    @State private var adoptedTable:Wire.Table?
    @State private var adoptedRow:Wire.Row?
    @State private var comparedTable:Wire.Table?
    @State private var comparedRow:Wire.Row?
    private var table:Wire.Table? {if let adoptedTable {return adoptedTable};switch target.mode {case .create:return nil;case .schema(let t),.add(let t),.patch(let t,_):return t}}
    private var row:Wire.Row? {if let adoptedRow {return adoptedRow};if case .patch(_,let row)=target.mode {return row};return nil}
    private var schema:Bool {if case .schema=target.mode {return true};return false}
    private var creating:Bool {if case .create=target.mode {return true};return false}
    private var crew:String {table?.crewId ?? target.scope.origin.crew}
    private var heading:String {creating ? "新建组织表":schema ? "管理表定义":row==nil ? "新增记录":"编辑 "+row!.id}
    private var availableFields:[Wire.Field] {
        guard let table else {return []}
        if schemaAction=="restoreField" {return table.fields.filter(\.hidden)}
        if schemaAction=="setTitleField" {return table.active.filter{$0.kind.scalar != nil}}
        if schemaAction=="hideField" {return table.active.filter{$0.id != table.titleFieldId && $0.nullable}}
        if schemaAction=="widenSingleLink" {return table.active.filter{$0.kind.link && $0.kind.many==false}}
        return table.active
    }
    var body:some View {
        NavigationStack {
            ScrollView {VStack(alignment:.leading,spacing:14) {
                if creating {creation}
                else if schema {schemaForm}
                else if let table {
                    rowForm(table)
                    if let issue=rowDraftIssue {Text(issue).font(.caption).foregroundStyle(.orange)}
                }
                if let problem {Text(problem).foregroundStyle(.orange).textSelection(.enabled)}
                if !creating {Button("读取最新版本用于对照"){readLatest()}.disabled(busy || !current())}
                if let comparedTable {
                    DisclosureGroup("当前实际定义版本 "+comparedTable.schemaRevision+(comparedRow.map{" · 记录版本 "+$0.revision} ?? "")) {
                        ForEach(comparedTable.active){field in
                            Text(field.name+" · "+field.id+" · "+field.kind.title).font(.caption)
                            if let comparedRow,field.kind.scalar != nil {Text(comparedRow.scalar(field.id).display).textSelection(.enabled)}
                        }
                        ForEach(edited.sorted(),id:\.self){id in Button("放弃字段 "+id+" 的本地修改，采用当前值",role:.destructive){discardField(id,table:comparedTable,row:comparedRow)}}
                        Button("以新版本重新核对这份编辑"){adoptCompared()}.disabled(original != nil || busy || !current())
                        Text("保留你实际改过的兼容字段，未编辑的列采用当前值。无法安全保留的关联更改会明确阻止采用，不会清空隐藏关系。").font(.caption)
                    }
                }
                if let original {
                    Text("原请求 "+original.witness.requestId+" 尚待核对；不会重新提交。").font(.caption).textSelection(.enabled)
                    Button("只读查询原回执") {lookup(original)}.disabled(busy || !current())
                }
                if busy {ProgressView().controlSize(.small)}
            }.padding(20)}
            .navigationTitle(heading)
            .toolbar {
                ToolbarItem(placement:.cancellationAction){Button("关闭"){dismiss()}.disabled(busy)}
                ToolbarItem(placement:.confirmationAction){Button("核对并保存…"){confirming=true}.disabled(!current() || busy || stopped || operation==nil)}
            }
        }
        #if os(macOS)
        .frame(minWidth:560,minHeight:520)
        #endif
        .interactiveDismissDisabled(busy)
        .onAppear {initialize()}
        .confirmationDialog("保存这次明确的表格变更？",isPresented:$confirming) {
            Button("确认保存"){submit()};Button("取消",role:.cancel){}
        } message: {
            let tableDescription: String = table.map { $0.id + " · 定义版本 " + $0.schemaRevision } ?? "新组织表"
            let rowDescription: String = row.map { " · 记录版本 " + $0.revision } ?? ""
            let message = tableDescription + rowDescription + "。仅修改本次填写的稳定字段，其他列与原业务状态保留。"
            Text(message)
        }
        .sheet(item:$selectingLink){field in
            CrewdManagedV2TargetPicker(workspace:workspace,store:store,scope:target.scope,crew:crew,constraint:field.kind.target,current:{current() && selectingLink?.id==field.id},choose:{value in
                guard current(),selectingLink?.id==field.id else {return}
                var values=linkValues[field.id] ?? []
                guard canAdd(field,values:values) else {return}
                values.append(value);linkValues[field.id]=values;edited.insert(field.id);selectingLink=nil
            })
        }
    }
    private var creation:some View {VStack(alignment:.leading,spacing:12) {
        Text("组织表与记录可被当前组织权限允许的成员读取；不复制私人资料正文。").font(.caption).foregroundStyle(.secondary)
        TextField("表名",text:$title);TextField("用途（必填）",text:$purpose,axis:.vertical)
        if purpose.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty {Text("请填写这张表的用途后再保存。").font(.caption).foregroundStyle(.secondary)}
        Picker("记录写权限",selection:$policy){Text(Wire.Policy.authorCaptainHuman.title).tag(Wire.Policy.authorCaptainHuman);Text(Wire.Policy.ownerCrewWriters.title).tag(Wire.Policy.ownerCrewWriters)}
        ForEach(draftFields.indices,id:\.self){index in VStack(alignment:.leading){
            fieldEditor($draftFields[index].draft)
            HStack {if draftFields[index].draft.kind.scalar != nil {Button(titleIndex==index ? "当前标题列":"设为标题列"){titleIndex=index}};Spacer();if draftFields.count>1 {Button("移除此未保存列",role:.destructive){draftFields.remove(at:index);titleIndex=min(titleIndex,draftFields.count-1)}}}
        }.padding(10).background(.quaternary,in:RoundedRectangle(cornerRadius:8))}
        Button("添加列"){draftFields.append(.init(draft:.init(name:"",kind:.init(kind:"text"),nullable:true)))}.disabled(draftFields.count>=64)
        Text("标题列须为文字、整数或小数。字段保存后会获得稳定编号，修改名称不会变成另一列。").font(.caption).foregroundStyle(.secondary)
    }}
    private var schemaForm:some View {VStack(alignment:.leading,spacing:12) {
        if let table {Text(table.id+" · 原定义版本 "+table.schemaRevision).font(.caption);Text("列名不决定字段身份；本次只执行一个明确操作。").font(.caption)}
        Picker("操作",selection:$schemaAction) {
            Text("新增列").tag("addField");Text("修改列名").tag("renameField");Text("修改表名").tag("renameTable");Text("修改用途").tag("setPurpose")
            Text("更换标题列").tag("setTitleField");Text("隐藏列").tag("hideField");Text("恢复列").tag("restoreField");Text("单关联改为多关联").tag("widenSingleLink")
        }.onChange(of:schemaAction){_,_ in fieldId=availableFields.first?.id ?? "";label=""}
        if schemaAction=="addField" {fieldEditor($newField);Text("已有记录的表只能新增可为空列；后台会核对实际数据。").font(.caption);if table?.fields.count==64 {Text("此表已达到64列（含隐藏列），暂不能继续新增。").foregroundStyle(.orange)}}
        else if ["renameTable","setPurpose"].contains(schemaAction) {
            TextField(schemaAction=="renameTable" ? "新表名":"新用途（必填）",text:$label,axis:.vertical)
            if schemaAction=="setPurpose",label.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty {Text("用途不能为空；原表内容保持原样。").font(.caption).foregroundStyle(.secondary)}
        }
        else {
            Picker("字段",selection:$fieldId){ForEach(availableFields){Text($0.name+" · "+$0.id).tag($0.id)}}
            if schemaAction=="renameField" {TextField("新列名",text:$label)}
            if schemaAction=="hideField" {Text("这里只能隐藏可为空且不是当前标题的列。必填列暂不能隐藏；标题列须先明确更换。原值与关联保留。").font(.caption)}
            if schemaAction=="widenSingleLink" {Text("保留所有现有关联；不能在这里改回单关联或改变目标类型。").font(.caption)}
        }
    }}
    private func fieldEditor(_ field:Binding<Wire.FieldDraft>)->some View {VStack(alignment:.leading,spacing:8) {
        TextField("列名",text:field.name)
        Picker("类型",selection:Binding(get:{field.wrappedValue.kind.kind},set:{value in field.wrappedValue.kind = value=="link" ? .init(kind:value,target:.init(kind:"work"),many:false):.init(kind:value)})) {
            Text("文字").tag("text");Text("整数").tag("integer");Text("小数").tag("real");Text("关联对象").tag("link")
        }
        Toggle("允许空值",isOn:field.nullable)
        if field.wrappedValue.kind.link {
            Picker("关联目标",selection:Binding(get:{field.wrappedValue.kind.target?.kind ?? "work"},set:{value in field.wrappedValue.kind.target = .init(kind:value)})) {
                ForEach(targetConstraintKinds,id:\.self){kind in Text(Wire.Constraint(kind:kind).title).tag(kind)}
            }
            if field.wrappedValue.kind.target?.kind=="row" {
                TextField("限定组织表 T编号（可留空）",text:Binding(get:{field.wrappedValue.kind.target?.tableId ?? ""},set:{field.wrappedValue.kind.target?.tableId=$0.isEmpty ? nil:$0}))
            }
            Toggle("允许多个关联",isOn:Binding(get:{field.wrappedValue.kind.many==true},set:{field.wrappedValue.kind.many=$0}))
        }
    }}
    private var targetConstraintKinds:[String] {
        Wire.TargetKind.allCases.filter{workspace.client.capabilities.contains(Wire.knowledgeCapability) || ![.document,.knowledge].contains($0)}.map{$0 == .knowledge ? "knowledgeView":$0.rawValue}+["anyReadable"]
    }
    private func rowForm(_ table:Wire.Table)->some View {VStack(alignment:.leading,spacing:14) {
        Text(table.id+" · 定义版本 "+table.schemaRevision+(row.map{" · 记录版本 "+$0.revision} ?? "")).font(.caption)
        Text("未编辑的列保持原值。连接、定义或记录变化时，本表单不会自动覆盖新版本。").font(.caption).foregroundStyle(.secondary)
        ForEach(table.active){field in VStack(alignment:.leading,spacing:8) {
            Text(field.name+" · "+field.id+" · "+field.kind.title).font(.subheadline)
            if field.kind.link {
                let values=linkValues[field.id] ?? []
                ForEach(values.indices,id:\.self){index in HStack {Text(linkLabel(values[index],field:field));Spacer();Button("移除这项可见关联",role:.destructive){linkValues[field.id]?.remove(at:index);edited.insert(field.id)}}}
                if row?.links(field.id)?.hiddenSet != nil {Text("部分原关联当前不可见；保存时整组保留，可见项不能净增。此提示不表示隐藏数量。").font(.caption).foregroundStyle(.secondary)}
                if row?.links(field.id)?.incompleteRequiredLink==true {Text("此必填关联已不完整；不编辑它可继续保存其他列，编辑它时须补充目标。").font(.caption).foregroundStyle(.orange)}
                Button("选择关联目标…"){selectingLink=field}.disabled(!canAdd(field,values:values))
            } else {
                if field.nullable {Toggle("空值",isOn:Binding(get:{nulls.contains(field.id)},set:{if $0 {nulls.insert(field.id)}else{nulls.remove(field.id)};edited.insert(field.id)}))}
                TextField(field.kind.scalar == .text ? "填写文字":"填写数值",text:Binding(get:{text[field.id] ?? ""},set:{text[field.id]=$0;edited.insert(field.id)}),axis:.vertical).disabled(nulls.contains(field.id))
            }
        }.padding(10).background(.quaternary,in:RoundedRectangle(cornerRadius:8))}
    }}
    private func canAdd(_ field:Wire.Field,values:[Wire.LinkWrite])->Bool {
        if row?.links(field.id)?.hiddenSet != nil {return values.count<(row?.links(field.id)?.visible.count ?? 0)}
        return values.count<(field.kind.many==true ? 64:1)
    }
    private func linkLabel(_ value:Wire.LinkWrite,field:Wire.Field)->String {
        if let target=value.target {return row?.links(field.id)?.visible.first(where:{$0.target==target})?.summary ?? target.label}
        return "新填写的产物指针："+(value.input?.value ?? "")+"（未核验）"
    }
    private func initialize() {
        guard !initialized else {return};initialized=true
        guard let table else {return};title=table.title;purpose=table.purpose ?? "";fieldId=availableFields.first?.id ?? ""
        for field in table.active {
            if field.kind.link {linkValues[field.id]=row?.links(field.id)?.visible.map{.init(kind:"reference",target:$0.target)} ?? []}
            else {let value=row?.scalar(field.id) ?? (field.nullable ? .null:.text(""));if value == .null {nulls.insert(field.id)};text[field.id]=value == .null ? "":value.display}
        }
    }
    private var rowDraftIssue:String? {
        guard let table,!schema,!creating else{return nil}
        do{_=try Wire.draftPatches(table:table,row:row,text:text,nulls:nulls,links:linkValues,edited:edited);return nil}catch{return error.localizedDescription}
    }
    private var operation:Wire.Operation? {
        let result:Wire.Operation
        if creating {result = .init(kind:"createTable",title:title,purpose:purpose,fields:draftFields.map(\.draft),titleFieldIndex:titleIndex,rowWritePolicy:policy)}
        else if let table,schema {
            let change:Wire.SchemaChange
            switch schemaAction {
            case "addField":guard table.fields.count<64 else{return nil};change = .init(kind:schemaAction,field:newField)
            case "renameField":change = .init(kind:schemaAction,fieldId:fieldId,name:label)
            case "renameTable":change = .init(kind:schemaAction,title:label)
            case "setPurpose":change = .init(kind:schemaAction,purpose:label)
            default:change = .init(kind:schemaAction,fieldId:fieldId)
            }
            if !["addField","renameTable","setPurpose"].contains(schemaAction),!availableFields.contains(where:{$0.id==fieldId}) {return nil}
            result = .init(kind:"alterTable",tableId:table.id,expectedSchemaRevision:table.schemaRevision,change:change)
        } else if let table {
            guard let patches=try? Wire.draftPatches(table:table,row:row,text:text,nulls:nulls,links:linkValues,edited:edited) else{return nil}
            result = row.map{.init(kind:"patchRow",rowId:$0.id,expectedSchemaRevision:table.schemaRevision,expectedRowRevision:$0.revision,patches:patches)} ?? .init(kind:"addRow",tableId:table.id,expectedSchemaRevision:table.schemaRevision,patches:patches)
        } else {return nil}
        return result.valid ? result:nil
    }
    private func readLatest() {
        guard current(),!busy,let table else {return};busy=true
        Task {defer{busy=false}
            do {
                if let row {let (t,r)=try await store.row(row.id,scope:target.scope,current:current);guard current() else{return};comparedTable=t;comparedRow=r}
                else {let t=try await store.table(table.id,scope:target.scope,current:current);guard current() else{return};comparedTable=t;comparedRow=nil}
            }catch{if current(){problem=error.localizedDescription}}
        }
    }
    private func applyCurrentField(_ field:Wire.Field,row:Wire.Row?) {
        if field.kind.link {linkValues[field.id]=row?.links(field.id)?.visible.map{.init(kind:"reference",target:$0.target)} ?? []}
        else {let value=row?.scalar(field.id) ?? (field.nullable ? .null:.text(""));if value == .null {nulls.insert(field.id)}else{nulls.remove(field.id)};text[field.id]=value == .null ? "":value.display}
    }
    private func discardField(_ id:String,table:Wire.Table,row:Wire.Row?) {
        edited.remove(id)
        if let field=table.active.first(where:{$0.id==id}) {applyCurrentField(field,row:row)}
    }
    private func adoptCompared() {
        guard current(),!busy,original==nil,let next=comparedTable,let old=table,!next.frozen else{return}
        do {
            let outstanding=try store.pending(target.scope).contains{$0.witness.table?.operation.target==operation?.target && Wire.exact($0.witness.managedScope,target.scope.authority)}
            guard !outstanding else {throw Store.Problem.stale}
            if !schema {
                for id in edited {
                    guard let previous=old.active.first(where:{$0.id==id}),let currentField=next.active.first(where:{$0.id==id}),previous.kind==currentField.kind,previous.nullable==currentField.nullable else {
                        throw CrewdClientError(message:"字段 "+id+" 已隐藏或定义变化。原填写保留，请先明确放弃该列本地修改。",kind:.notSubmitted)
                    }
                    if previous.kind.link,let oldRow=row,let fresh=comparedRow {
                        let a=oldRow.links(id),b=fresh.links(id)
                        guard a?.hiddenSet==nil,b?.hiddenSet==nil,Set(a?.visible.map{$0.target.id} ?? [])==Set(b?.visible.map{$0.target.id} ?? []) else {
                            throw CrewdClientError(message:"关联列 "+id+" 的原关系已变化或当前不可见。原草稿保留，须先采用该列当前值再重新选择。",kind:.notSubmitted)
                        }
                    }
                }
                for field in next.active where !edited.contains(field.id) {applyCurrentField(field,row:comparedRow)}
            }
            adoptedTable=next;adoptedRow=comparedRow;comparedTable=nil;comparedRow=nil;stopped=false;problem="已采用新版本作为明确核对基线；尚未保存。"
        }catch{problem=error.localizedDescription}
    }
    private func submit() {
        guard current(),!busy,!stopped,let operation else {return};busy=true;problem=nil
        Task {defer {busy=false;attempted()}
            do {let result=try await store.submit(operation,crew:crew,scope:target.scope,tableId:table?.id,current:current);guard current() else {return};stopped=true;saved(result);if current(){dismiss()}}
            catch {if current(){problem=error.localizedDescription
                let known=error is Store.DefinitiveRefusal || (error as? CrewdClientError)?.kind == .notSubmitted
                stopped = !known
                original=(try? store.pending(target.scope))?.first{$0.witness.table?.operation.target==operation.target && $0.witness.crewId==crew && Wire.exact($0.witness.managedScope,target.scope.authority)}
            }}
        }
    }
    private func lookup(_ original:Store.Pending) {
        guard current(),!busy else {return};busy=true
        Task {defer{busy=false;attempted()}
            do {let result=try await store.lookup(original,scope:target.scope,current:current);guard current() else {return};if let result {saved(result);if current(){dismiss()}}else{problem="原结果尚未知，原请求仍保留。"}}
            catch {if current(){problem=error.localizedDescription}}
        }
    }
}
