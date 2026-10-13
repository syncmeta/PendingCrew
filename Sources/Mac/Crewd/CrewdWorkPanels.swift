#if os(macOS)
import SwiftUI

/// Shared colors follow the original chat-pane's muted warm sidebar and mint selection.
enum CrewdChatStyle {
    // Same anchor values as original Theme.Palette; explicit SwiftUI colorScheme
    // keeps standalone NSHosting snapshots and the full application consistent.
    private static func color(_ light:UInt32,_ dark:UInt32,_ scheme:ColorScheme)->Color {
        let value=scheme == .dark ? dark:light
        return Color(red:Double((value>>16)&255)/255,green:Double((value>>8)&255)/255,blue:Double(value&255)/255)
    }
    static func sidebar(_ scheme:ColorScheme)->Color {color(0xEFEEE9,0x2A2823,scheme)}
    static func selection(_ scheme:ColorScheme)->Color {color(0xE3EEEA,0x1C3A2F,scheme)}
    static func bubble(_ scheme:ColorScheme)->Color {color(0xEFEEE9,0x2A2823,scheme)}
    static func canvas(_ scheme:ColorScheme)->Color {color(0xFFFFFF,0x161512,scheme)}
    static func ink(_ scheme:ColorScheme)->Color {color(0x1B1A14,0xECE9E0,scheme)}
    static func muted(_ scheme:ColorScheme)->Color {color(0x6E6A5C,0xA8A294,scheme)}
}

struct CrewdWorkPanels: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var store: CrewdWorkStore
    @Binding var drafts: CrewdRequirementDrafts
    @Binding var planDrafts: CrewdPlanDrafts
    var members:[CrewdValue]
    var progressOnly = false
    var compact = false
    @State private var collapsed:Set<String>=[]
    @State private var selected:String?
    @State private var showFolders=false
    @State private var failure:String?
    @State private var actionTarget: CrewdPlanTarget?
    @State private var selectedRequirement: CrewdWorkID?
    @State private var closing:Set<CrewdWorkID>=[]
    @State private var closeNotes:[CrewdWorkID:String]=[:]
    var body: some View {
        VStack(alignment:.leading,spacing:12) {
            if !compact {
                HStack {
                    Button("文件夹",systemImage:"folder") {showFolders.toggle()}.buttonStyle(.plain)
                    Spacer(); Button("刷新",systemImage:"arrow.clockwise") {Task {do {try await store.refresh()} catch {failure=error.localizedDescription}}}
                }
                if showFolders {Text("独立文件夹分组等待后台分类与视图接口接线；不会把文件夹写成计划父项。").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))}
            }
            if let error=store.lastFailure { Text(error.localizedDescription).font(.caption).foregroundStyle(.orange).textSelection(.enabled) }
            if let failure { Text(failure).foregroundStyle(.orange).textSelection(.enabled) }
            if store.loading {ProgressView().controlSize(.small)}
            ScrollView {
                VStack(alignment:.leading,spacing:10) {
                    if !progressOnly && !compact {
                        Text("原始需求").font(.headline)
                        requirementComposer
                        ForEach(store.requirementIDs,id:\.self) {id in
                            VStack(alignment:.leading,spacing:4) {
                                Button {
                                    selectedRequirement = selectedRequirement == id ? nil : id
                                    if selectedRequirement != nil {Task {do {_ = try await store.requirementDetail(id)} catch {failure = describe(error)}}}
                                } label: {Text("\(id.raw)  \(store.requirements[id]?["text"].string ?? "")").multilineTextAlignment(.leading)}.buttonStyle(.plain)
                                HStack {Button("从需求建计划") {actionTarget=CrewdPlanTarget(key:.init(crew:store.selectedCrew,object:"",source:id.raw))};Spacer()}
                                if selectedRequirement == id {requirementDetail(id)}
                                Text(CrewdWorkStatusLabel.display(store.requirements[id]?["status"].string ?? "")).font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                                Text(store.plans(from:id).map {$0.id.raw}.joined(separator:" · ")).font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                            }.padding(8).frame(maxWidth:.infinity,alignment:.leading).background(CrewdChatStyle.bubble(colorScheme),in:RoundedRectangle(cornerRadius:6))
                        }
                        Text("计划").font(.headline)
                    }
                    ForEach(CrewdNavigation.planTree(store.progress,collapsed:collapsed)) {row in
                        if let id=CrewdWorkID(row.object,kind:.plan),let plan=store.plans[id] {
                            VStack(alignment:.leading,spacing:4) {
                                HStack(alignment:.top) {
                                    if row.expandable {Button {if !collapsed.insert(row.id).inserted {collapsed.remove(row.id)}} label:{Image(systemName:collapsed.contains(row.id) ? "chevron.right":"chevron.down")}.buttonStyle(.plain)}
                                    Image(systemName:plan.status == "done" ? "checkmark.circle.fill":plan.status == "blocked" ? "questionmark.circle":"circle").foregroundStyle(CrewdChatStyle.muted(colorScheme))
                                    Button { selected = selected == id.raw ? nil : id.raw; if selected != nil && !compact {Task {do {_ = try await store.detail(id)} catch {failure=error.localizedDescription}}} } label:{Text("\(id.raw)  \(plan.value["title"].string ?? "计划")").multilineTextAlignment(.leading)}.buttonStyle(.plain)
                                    Spacer()
                                    Text(CrewdWorkStatusLabel.display(plan.status)).font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                                    if plan.value["pendingTransfer"] != .null {Text("交接中").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))}
                                }
                                if !compact {
                                    Text([plan.value["owner"].string,plan.value["note"].string,plan.value["nextCheckAt"].string].compactMap{$0}.joined(separator:" · ")).font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                                    if let parent=plan.value["parent"].string,row.depth == 0 {Text("上级：\(parent)").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))}
                                    if row.depth >= 3 {Text("后台返回超过三层的计划，请核对账本。").font(.caption).foregroundStyle(.orange)}
                                }
                                if selected == id.raw, !compact {
                                    if let detail=plan.detail {
                                        Text(detail["goal"].string ?? "").textSelection(.enabled)
                                        Text("来源：" + detail["sources"].array.compactMap(\.string).joined(separator:" · ")).font(.caption)
                                        Text("依赖：" + detail["dependsOn"].array.compactMap(\.string).joined(separator:" · ")).font(.caption)
                                        Text("卡点：" + detail["blockedOn"].array.compactMap(\.string).joined(separator:" · ")).font(.caption)
                                        if let evidence=detail["evidence"]["value"].string {Text(evidence).textSelection(.enabled)}
                                        CrewdHandoffView(store:store,id:id,members:members).id(store.selectedCrew + ":" + id.raw)
                                        Button("计划操作") {actionTarget=CrewdPlanTarget(key:.init(crew:store.selectedCrew,object:id.raw,source:""))}.disabled(plan.frozen || ["done","dropped"].contains(plan.status))
                                    } else {Button("读取当前详情") {Task {do {_ = try await store.detail(id)} catch {failure=describe(error)}}}}

                                }
                            }.padding(.leading,CGFloat(row.depth)*14).padding(.vertical,6)
                        }
                    }
                    if compact {
                        ForEach(store.humanTasks.keys.filter {store.humanTasks[$0]?["ask"]["crewId"].string == store.selectedCrew}.sorted(by:{$0.raw < $1.raw}),id:\.self) {id in
                            HStack(alignment:.top) {Image(systemName:"questionmark.circle"); Text("\(id.raw)  \(store.humanTasks[id]?["ask"]["question"].string ?? "")").font(.caption).lineLimit(2)}
                        }
                    }
                    if store.planIDs.isEmpty {Text("尚无计划").foregroundStyle(CrewdChatStyle.muted(colorScheme))}
                    if !progressOnly && !compact {
                        Divider(); Text("待人类处理 · 全组织收件箱").font(.headline)
                        ForEach(store.humanTasks.keys.sorted(by:{$0.raw < $1.raw}),id:\.self) {id in
                            let ask=store.humanTasks[id]?["ask"] ?? .null
                            Text("\(id.raw)  \(ask["question"].string ?? "")").textSelection(.enabled)
                            Text(ask["item"].string ?? ask["crewId"].string ?? "").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
                        }
                    }
                }.frame(maxWidth:.infinity,alignment:.leading)
            }
        }.padding(compact ? 8:16).foregroundStyle(CrewdChatStyle.ink(colorScheme))
        .sheet(item:$actionTarget) {target in CrewdPlanActions(store:store,drafts:$planDrafts,target:target,members:members)}
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
                    Text("可能已创建，使用原草稿再次确认会复用同一请求编号。").font(.caption).foregroundStyle(.orange)
                    Text(snapshot.text).font(.caption).lineLimit(3).textSelection(.enabled)
                    Button("确认这份原始提交") {
                        guard drafts.confirm(snapshot) else {return}
                        Task {
                            guard store.selectedCrew == snapshot.crew else {drafts.finish(snapshot,accepted:false);return}
                            do {
                                _ = try await store.createRequirement(requestID:snapshot.requestID,text:snapshot.text)
                                drafts.finish(snapshot,accepted:true)
                            } catch {
                                drafts.finish(snapshot,accepted:false,uncertain:(error as? CrewdClientError)?.code == nil)
                                if store.selectedCrew == snapshot.crew {failure=describe(error)}
                            }
                        }
                    }.disabled(drafts.busy(crew))
                }.padding(.vertical,5)
            }
            TextField("写下原始需求",text:Binding(get:{drafts.text(crew)},set:{drafts.set(crew,text:$0)}),axis:.vertical).lineLimit(2...5)
                .disabled(drafts.busy(crew))
            HStack {
                Text("保存原文，计划与处理由团队继续跟进").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
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
                            drafts.finish(snapshot,accepted:false,uncertain:(error as? CrewdClientError)?.code == nil)
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
        Text("全局编号：\(id.raw)").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme))
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

#endif
