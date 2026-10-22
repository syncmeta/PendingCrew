import SwiftUI
#if os(macOS)
import AppKit
#endif

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
    static func bubble(_ scheme:ColorScheme)->Color {color(0xFFFFFF,0x201E1A,scheme)}
    static func accent(_ scheme:ColorScheme)->Color {color(0x044735,0x57B690,scheme)}
    static func line(_ scheme:ColorScheme)->Color {color(0xE2E0D7,0x383530,scheme)}
    static func listSelection(_ scheme:ColorScheme)->Color {color(0xEFEEE9,0x33312D,scheme)}
    static func canvas(_ scheme:ColorScheme)->Color {
        // Inherit the shared window material; never add a chat-only plate.
        Color.clear
    }
    // Hierarchical styles preserve Material vibrancy. The old fixed warm
    // colors assumed an opaque canvas and were not readable over root glass.
    static func ink(_ scheme:ColorScheme)->HierarchicalShapeStyle {.primary}
    static func muted(_ scheme:ColorScheme)->HierarchicalShapeStyle {.secondary}
}

/// Shared native chrome, using the product tour's warm neutral surfaces.
/// These components only present caller-owned data and explicit actions.
struct CrewdConversationRow<Avatar: View>: View {
    @Environment(\.colorScheme) private var scheme
    let title: String
    let subtitle: String
    let selected: Bool
    let action: () -> Void
    @ViewBuilder let avatar: () -> Avatar
    var body: some View {
        Button(action: action) {
            HStack(spacing: 9) {
                avatar()
                VStack(alignment: .leading, spacing: 4) {
                    Text(title).font(CrewdType.body.weight(selected ? .semibold:.medium)).lineLimit(1)
                        .foregroundStyle(CrewdChatStyle.ink(scheme))
                    Text(subtitle).font(CrewdType.caption).lineLimit(1)
                        .foregroundStyle(CrewdChatStyle.muted(scheme))
                }
                Spacer(minLength: 0)
            }.padding(.horizontal, 10).padding(.vertical, 11)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(selected ? CrewdChatStyle.listSelection(scheme) : .clear, in: RoundedRectangle(cornerRadius: 7))
                .contentShape(Rectangle())
        }.buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }
}
struct CrewdEmptySurface: View {
    @Environment(\.colorScheme) private var scheme
    let title: String
    let detail: String
    let symbol: String
    var actionTitle: String? = nil
    var action: (() -> Void)? = nil
    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: symbol).font(.system(size: 27, weight: .light))
                .foregroundStyle(CrewdChatStyle.accent(scheme))
            Text(title).font(CrewdType.title).foregroundStyle(CrewdChatStyle.ink(scheme))
            Text(detail).font(CrewdType.label).foregroundStyle(CrewdChatStyle.muted(scheme))
                .multilineTextAlignment(.center).fixedSize(horizontal: false, vertical: true).frame(maxWidth: 320)
            if let actionTitle, let action {
                Button(actionTitle, action: action).buttonStyle(.borderedProminent)
                    .tint(CrewdChatStyle.accent(scheme)).padding(.top, 4)
            }
        }.padding(28).frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(CrewdChatStyle.canvas(scheme))
    }
}

/// Window-owned input snapshots only; ask state remains in the organization inbox.
struct CrewdHumanDrafts {
    struct Key:Hashable {let scope:String;let crew:String;let ask:String}
    enum Action:String,CaseIterable {case answer="答复",withdraw="撤回"}
    struct Fields:Equatable {var action=Action.answer;var text=""}
    struct Snapshot:Equatable {let key:Key;let fields:Fields;let version:UUID}
    private var entries:[Key:Snapshot]=[:]
    private var pending:[Key:UUID]=[:]
    private var unknown:[Key:[Snapshot]]=[:]
    func fields(_ key:Key)->Fields {entries[key]?.fields ?? Fields()}
    func busy(_ key:Key)->Bool {pending[key] != nil}
    func uncertain(_ key:Key)->[Snapshot] {unknown[key] ?? []}
    var allUnknownSnapshots:[Snapshot] {unknown.values.flatMap{$0}.sorted{$0.version.uuidString < $1.version.uuidString}}
    mutating func edit(_ key:Key,_ fields:Fields) {if self.fields(key) != fields {entries[key]=Snapshot(key:key,fields:fields,version:UUID())}}
    mutating func begin(_ key:Key)->Snapshot? {
        guard !busy(key),!fields(key).text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty else {return nil}
        if entries[key] == nil {return nil}
        let snapshot=entries[key]!;pending[key]=snapshot.version;return snapshot
    }
    mutating func finish(_ snapshot:Snapshot,accepted:Bool,uncertain:Bool=false) {
        if pending[snapshot.key] == snapshot.version {pending[snapshot.key]=nil}
        if uncertain && !(unknown[snapshot.key] ?? []).contains(snapshot) {unknown[snapshot.key,default:[]].append(snapshot)}
        if accepted {unknown[snapshot.key]?.removeAll {$0.version == snapshot.version}}
        if accepted && entries[snapshot.key] == snapshot {entries[snapshot.key]=nil}
    }
}
struct CrewdHumanTarget:Identifiable {
    let key:CrewdHumanDrafts.Key
    var crewTitle:String?=nil
    var id:String {key.scope+":"+key.crew+":"+key.ask}
}
struct CrewdHumanActions:View {
    @ObservedObject var store:CrewdWorkStore
    @Binding var drafts:CrewdHumanDrafts
    let target:CrewdHumanTarget
    @Environment(\.dismiss) private var dismiss
    @State private var failure:String?
    @State private var humanOrigin=false
    @State private var reading=false
    @State private var reviewedUnknown=false
    @State private var readUnknown=false
    private var key:CrewdHumanDrafts.Key {target.key}
    private var id:CrewdWorkID? {CrewdWorkID(key.ask,kind:.human)}
    private var ask:CrewdValue {id.flatMap{store.humanTasks[$0]}?["ask"] ?? .null}
    private var fields:CrewdHumanDrafts.Fields {drafts.fields(key)}
    private func binding<T>(_ path:WritableKeyPath<CrewdHumanDrafts.Fields,T>)->Binding<T> {Binding(get:{drafts.fields(key)[keyPath:path]},set:{var f=drafts.fields(key);f[keyPath:path]=$0;drafts.edit(key,f);reviewedUnknown=false;readUnknown=false})}
    var body:some View {
        VStack(alignment:.leading,spacing:12) {
            CrewdPanelHeader(title:"处理人工待办",detail:"答复或撤回 Agent 的原问题。",symbol:"person.crop.circle.badge.questionmark")
            DisclosureGroup("原事项编号"){Text(key.ask).textSelection(.enabled)}.font(CrewdType.caption).foregroundStyle(.secondary)
            Text("所属机组："+(target.crewTitle ?? "机组资料尚未加载")).font(.caption).foregroundStyle(.secondary)
            Text(ask["question"].string ?? "此问题已不在当前待处理收件箱，请刷新核对。").textSelection(.enabled)
            if let item=ask["item"].string {Text("关联计划："+item).font(.caption)}
            if let original=ask["default"]["text"].string {Text("预设答复："+original).font(.caption).textSelection(.enabled)}
            ForEach(drafts.uncertain(key),id:\.version){snapshot in
                VStack(alignment:.leading,spacing:3){Text("此前提交的结果未确认，不会自动重发。").foregroundStyle(.orange);Text(snapshot.fields.action.rawValue+"："+snapshot.fields.text).textSelection(.enabled)}.font(.caption)
            }
            if !drafts.uncertain(key).isEmpty {
                Button("读取当前收件箱"){Task {await refresh()}}.disabled(reading || drafts.busy(key))
                Toggle("我已核对当前状态，确定要再次提交",isOn:$reviewedUnknown).disabled(!readUnknown || reading || drafts.busy(key))
            }
            if humanOrigin{Text("这是人创建的待办，请从关联群的人待办入口更新人工状态；不会把它当作 Agent 提问作答。").font(.caption).foregroundStyle(.secondary)}
            Picker("操作",selection:binding(\.action)){ForEach(CrewdHumanDrafts.Action.allCases,id:\.self){Text($0.rawValue).tag($0)}}.pickerStyle(.segmented).disabled(drafts.busy(key))
            CrewdFormField(title:fields.action == .answer ? "你的答复":"撤回理由"){
                TextField(fields.action == .answer ? "写下处理意见":"说明撤回的原因",text:binding(\.text),axis:.vertical).lineLimit(4...10).disabled(drafts.busy(key))
            }
            if fields.action == .withdraw {Text("仍阻塞活跃计划的问题不能撤回；请先由负责人调整计划，后台会最终校验。").font(.caption).foregroundStyle(.secondary)}
            if let failure {Text(failure).foregroundStyle(.orange).textSelection(.enabled)}
            Spacer()
            HStack{Button("关闭"){dismiss()};Spacer();Button(drafts.busy(key) ? "提交中":"提交"){submit()}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(humanOrigin || drafts.busy(key) || reading || store.selectedCrew != key.scope || ask["state"] != .string("open") || fields.text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || (!drafts.uncertain(key).isEmpty && !reviewedUnknown))}
        }.padding(20).font(CrewdType.body).modifier(CrewdAdaptiveFormFrame(width:540,height:580))
        .task(id:key){guard let id else{return};do{let value=try await store.humanTodoAttributes(id);humanOrigin=value["origin"] == .string("humanTask")}catch{failure=error.localizedDescription}}
    }
    private func describe(_ error:Error)->String {
        guard let e=error as? CrewdClientError else {return error.localizedDescription}
        return [e.message,e.code.map{"错误 \($0)"},e.refusal].compactMap{$0}.joined(separator:" · ")
    }
    private func refresh() async {
        reading=true;defer{reading=false}
        do {try await store.refresh();if store.selectedCrew == key.scope {failure=nil;readUnknown=true;reviewedUnknown=false}}
        catch {if store.selectedCrew == key.scope {failure=describe(error)}}
    }
    private func submit() {
        guard let snapshot=drafts.begin(key),let id else {return}
        let original=ask
        Task {
            var attempted=false
            do {
                try await store.refresh()
                guard store.selectedCrew == key.scope else {throw CancellationError()}
                guard let current=store.humanTasks[id]?["ask"],current["crewId"] == .string(key.crew),current["question"] == original["question"],current["state"] == .string("open") else {throw CrewdClientError(message:"问题状态已变化，请核对收件箱后再决定。",code:-32009)}
                let source=try await store.humanTodoAttributes(id)
                guard source["origin"] != .string("humanTask") else{humanOrigin=true;throw CrewdClientError(message:"人创建的待办须从关联群更新人工状态。",kind:.notSubmitted)}
                attempted=true
                _ = try await store.resolveHuman(id,crew:key.crew,text:snapshot.fields.text,withdraw:snapshot.fields.action == .withdraw)
                drafts.finish(snapshot,accepted:true)
                if store.selectedCrew == key.scope {dismiss()}
            } catch {
                drafts.finish(snapshot,accepted:false,uncertain:attempted && CrewdClientError.isUncertainSubmission(error))
                if store.selectedCrew == key.scope {reviewedUnknown=false;readUnknown=false;failure=describe(error)}
            }
        }
    }
}

struct CrewdHumanRequestView: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var workspace: CrewdWorkspace
    let request: CrewdValue
    let decision: CrewdValue
    @State private var answers: [String: String] = [:]
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("成员等待你的答复", systemImage: "person.crop.circle.badge.questionmark").font(.headline)
            if request["method"].string == "item/tool/requestUserInput" {
                ForEach(request["params"]["questions"].array.indices, id: \.self) { index in
                    let q = request["params"]["questions"].array[index]
                    let id = q["id"].string ?? ""
                    Text(q["question"].string ?? "")
                    if !q["options"].array.isEmpty {
                        ForEach(q["options"].array.indices, id: \.self) { option in
                            let row = q["options"].array[option]
                            Button(row["label"].string ?? "") { answers[id] = row["label"].string }
                            if let detail = row["description"].string { Text(detail).font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme)) }
                        }
                    }
                    if q["options"].array.isEmpty || q["isOther"] == .bool(true) {
                        if q["isSecret"] == .bool(true) { SecureField("你的答复", text: binding(id)) }
                        else { TextField("你的答复", text: binding(id)) }
                    }
                    if q["isSecret"] != .bool(true), let value = answers[id], !value.isEmpty { Text("已选：\(value)").font(.caption).foregroundStyle(CrewdChatStyle.muted(colorScheme)) }
                }
                Button("提交答复") {
                    let response = CrewdValue.object(["answers": .object(answers.mapValues { .object(["answers": .array([.string($0)])]) })])
                    send(method: "session.respondRunnerRequest", extra: ["response": response])
                }.disabled(!canSubmit || request["params"]["questions"].array.contains(where: { (answers[$0["id"].string ?? ""] ?? "").isEmpty }))
            } else {
                Text(decision["summary"].string ?? "请确认当前请求").textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                ForEach(decision["options"].array.indices, id: \.self) { i in
                    Button(decision["options"].array[i].string ?? "选项") { send(method: "session.answer", extra: ["choice": .number(Double(i))]) }.disabled(!canSubmit)
                }
            }
        }.onChange(of: request["runnerRequestId"]) { _, _ in answers.removeAll() }
        .onChange(of: request["fingerprint"]) { _, _ in answers.removeAll() }
        .padding(12).frame(maxWidth: .infinity, alignment: .leading).background(.blue.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
    }
    private func binding(_ id: String) -> Binding<String> { Binding(get: { answers[id] ?? "" }, set: { answers[id] = $0 }) }
    private var canSubmit: Bool {
        guard !workspace.busy, workspace.client.connected, !workspace.isRetired, workspace.profileForSession(workspace.sessionID)?.structuredDecisions == true else { return false }
        if !workspace.client.capabilities.contains("command.receipts") { return true }
        let alreadySubmitted = workspace.publicCommands.observations.values.contains { row in
            guard workspace.isCurrentOriginalObservation(row.prepared),
                  [.sessionAnswer, .sessionRespond].contains(row.envelope.operation),
                  row.envelope.params["runnerRequestId"] == request["runnerRequestId"],
                  row.envelope.params["fingerprint"] == request["fingerprint"] else { return false }
            if let phase = row.receipt?.phase { return ![.suppressed, .rejected].contains(phase) }
            return true
        }
        guard !alreadySubmitted else { return false }
        return request["runnerRequestId"].string != nil && request["fingerprint"].string != nil
            && workspace.controlTargets[workspace.sessionID] != nil
    }
    private func send(method: String, extra: [String: CrewdValue]) {
        guard canSubmit else { return }
        let epoch = workspace.observationGeneration
        let session = workspace.sessionID
        let originalRequest = request
        var params = extra
        params["fingerprint"] = originalRequest["fingerprint"]
        if workspace.client.capabilities.contains("command.receipts") {
            guard let target = workspace.controlTargets[session],
                  let id = originalRequest["runnerRequestId"].string else { return }
            params["target"] = target.value
            params["runnerRequestId"] = .string(id)
            if method == "session.answer" {
                params["response"] = .object(["choice": extra["choice"] ?? .null])
                params.removeValue(forKey: "choice")
            }
        } else { params["sessionId"] = .string(session) }
        Task {
            guard workspace.acceptsObservation(epoch), workspace.sessionID == session else { return }
            do {
                let current = try await workspace.readValue("session.inspect", .object(["sessionId": .string(session)]))
                guard workspace.acceptsObservation(epoch), workspace.sessionID == session,
                      current["runnerRequest"]["fingerprint"] == originalRequest["fingerprint"],
                      current["runnerRequest"]["runnerRequestId"] == originalRequest["runnerRequestId"] else {
                    workspace.error = "原审批请求已改变；草稿未转交新请求，未提交。"; return
                }
                _ = await workspace.perform(method, .object(params))
            } catch { if workspace.acceptsObservation(epoch) { workspace.error = error.localizedDescription } }
        }
    }

}

struct CrewdRunnerItemView: View {
    @Environment(\.colorScheme) private var colorScheme
    let item: CrewdValue
    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            switch item["type"].string {
            case "agentMessage": Text(item["text"].string ?? "").textSelection(.enabled)
            case "userMessage": Text(item["content"].array.compactMap {$0["text"].string}.joined(separator: "\n")).foregroundStyle(CrewdChatStyle.muted(colorScheme)).textSelection(.enabled)
            case "commandExecution":
                Text(item["command"].string ?? "命令").font(.system(.body, design: .monospaced)).textSelection(.enabled)
                DisclosureGroup("命令输出 · \(item["status"].string ?? "")") { Text(item["aggregatedOutput"].string ?? "").font(.system(.caption, design: .monospaced)).textSelection(.enabled) }
            case "mcpToolCall": Text("工具：\(item["server"].string ?? "") / \(item["tool"].string ?? "") · \(item["status"].string ?? "")")
            case "fileChange": DisclosureGroup("文件改动") { Text(item["changes"].pretty).font(.system(.caption, design: .monospaced)).textSelection(.enabled) }
            default: DisclosureGroup(item["type"].string ?? "会话事件") { Text(item.pretty).font(.system(.caption, design: .monospaced)).textSelection(.enabled) }
            }
        }.padding(10).frame(maxWidth: .infinity, alignment: .leading).background(.quaternary.opacity(0.3), in: RoundedRectangle(cornerRadius: 8))
    }
}

/// Shared-platform Codex start form; the host supplies its configured
/// model and account. This view never starts a runner merely by appearing.
struct CrewdExecutorChoiceView: View {
    @ObservedObject var workspace: CrewdWorkspace
    @Binding var executor: String
    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Picker("执行器", selection: $executor) {
                if workspace.executorProfiles.isEmpty { Text("尚未读取").tag(executor) }
                ForEach(workspace.executorProfiles.filter(\.offeredInOriginalUI)) { profile in
                    Text(profile.title).tag(profile.executor)
                }
            }
            if let profile = workspace.executorProfile(executor) {
                Text(profile.canStart ? "后台已配置此执行器；登录、额度与实际启动仍以原请求及会话状态为准。" : profile.unavailableDescription)
                    .font(.caption).foregroundStyle(profile.canStart ? Color.secondary : Color.orange)
                DisclosureGroup("执行能力详情") {
                Text("额外任务上下文：\(profile.initialContext ? "支持" : "未开放")").font(.caption).foregroundStyle(.secondary)
                Text("手工终端：\(profile.manualTerminal ? "支持" : "未开放") · 结构化审批：\(profile.structuredDecisions ? "支持" : "未开放")")
                    .font(.caption).foregroundStyle(.secondary)
                if !profile.fullHistoryLoad {
                    Text(profile.backend == "codex" && workspace.client.capabilities.contains("session.codex") ? "原历史可在会话控制页只读读取；手动恢复被拒绝时不会创建替代 thread。":profile.recoveryDescription)
                        .font(.caption).foregroundStyle(.secondary)
                }
                }.font(CrewdType.caption)

            } else {
                Text(workspace.executorProfilesFailure ?? "尚未读取当前后台的执行能力，不能启动。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Button("重新读取执行能力") { Task { await workspace.refreshExecutorProfiles() } }
                .disabled(!workspace.client.readyForWork || workspace.crewID.isEmpty)
        }.task(id: workspace.crewID + "/" + workspace.client.connectionEpoch.uuidString) {
            await workspace.refreshExecutorProfiles()
        }
    }
}

struct CrewdSessionStartView: View {
    @State private var isolation = false
    @ObservedObject private var personalService=CrewPersonalDraftService.shared
    @State private var personalScope:CrewPersonalDraftService.EditingScope?
    @State private var codexSettings=CrewdCodexSettingsDraft()
    @State private var explicitTitle=""
    @ObservedObject var workspace: CrewdWorkspace
    @Binding var drafts: CrewdDrafts
    @State private var failure: String?
    @State private var reviewedUnknown = false
    @State private var readUnknown = false
    @State private var reading = false
    private var unknown: [CrewdDrafts.Snapshot] { workspace.uncertainSessionStarts.filter { $0.context.crew == workspace.crewID } }
    private var context: CrewdDrafts.Context { .initialTask(crew: workspace.crewID, session: workspace.sessionID) }
    private var input: Binding<String> { Binding(get: { drafts.text(for: context) }, set: { drafts.set($0, for: context); workspace.setPersonalInput($0,context:context,scope:personalScope); reviewedUnknown = false }) }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            CrewdPanelHeader(title:"新成员会话",detail:"发送第一件具体任务后才会启动。",symbol:"person.badge.plus")
            CrewdFormSection(title:"执行成员") {CrewdExecutorChoiceView(workspace: workspace, executor: $workspace.selectedExecutor)}
            CrewdFormField(title:"第一件事",detail:"这段任务原文会作为新会话的首条消息。") {
                TextField("给新成员交代任务", text: input, axis: .vertical).lineLimit(3...8)
            }
            CrewPersonalDraftControls(workspace:workspace,context:context,payload:Binding(get:{.init(text:input.wrappedValue)},set:{input.wrappedValue=$0.text}))
            if workspace.selectedExecutor == "codex"{CrewdCodexStartOptions(workspace:workspace,title:$explicitTitle,settings:$codexSettings)}
            Toggle("使用独立工作树", isOn: $isolation)
            Text(isolation ? "需要已提交的 Git 仓库；创建失败不会改用共享目录。" : "使用机组的共享工作目录，适合知识整理及普通文件。").font(.caption).foregroundStyle(.secondary)
            if let failure { Text(failure).font(.caption).foregroundStyle(.orange) }
            ForEach(unknown.indices, id: \.self) { index in
                Text("此前启动结果未确认，可能已创建成员；原任务：" + unknown[index].text).font(.caption).foregroundStyle(.orange).textSelection(.enabled)
            }
            if !unknown.isEmpty {
                Button("读取成员列表核对") { let crew = workspace.crewID; Task {
                    reading = true; defer { reading = false }
                    do { try await workspace.refreshSessionDirectory(crew); if workspace.crewID == crew { readUnknown = true; reviewedUnknown = false } }
                    catch { if workspace.crewID == crew { failure = error.localizedDescription } }
                }}.disabled(reading || workspace.busy)
                Toggle("我已核对，仍要启动这个任务", isOn: $reviewedUnknown).disabled(!readUnknown || reading)
            }
            Button("启动所选成员") {
                let snapshot = drafts.snapshot(for: context), owner = workspace
                let personal=owner.personalInputSnapshot(snapshot)
                let role = owner.sessions.contains(where: { $0["role"].string == "captain" }) ? "worker" : "captain"
                var fields:[String:CrewdValue] = ["crewId": .string(snapshot.context.crew), "role": .string(role), "executor": .string(owner.selectedExecutor), "brief": .string(snapshot.text), "isolation": .bool(isolation)]
                if !explicitTitle.isEmpty{fields["title"] = .string(explicitTitle)}
                do{if owner.selectedExecutor == "codex",let options=try codexSettings.value(){guard owner.client.capabilities.contains("session.codex") else{throw CrewdClientError(message:"当前后台未接入所选 Codex 首轮设置。",kind:.notSubmitted)};fields["codexOptions"]=options}}
                catch{failure=error.localizedDescription;return}
                let submission=CrewdValue.object(fields)
                failure = nil; readUnknown = false; reviewedUnknown = false
                Task {
                    do { _ = try await owner.performValue("session.start", submission); owner.clearPersonalInput(snapshot,personal:personal) }
                    catch {
                        owner.recordUncertainSessionStart(snapshot, error: error)
                        if owner.crewID == snapshot.context.crew { failure = error.localizedDescription }
                    }
                }
            }.disabled(personalScope.map{!personalService.isCurrent($0)} == true || workspace.executorProfile(workspace.selectedExecutor)?.canStart != true || reading || workspace.busy || !workspace.client.connected || !workspace.client.supportsSessionStart || workspace.crewID.isEmpty || input.wrappedValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || (!unknown.isEmpty && !reviewedUnknown))
            Text(workspace.status).font(CrewdType.caption).foregroundStyle(.secondary).textSelection(.enabled)
        }.onAppear{workspace.bindPersonalProjection();personalScope=personalService.editingScope()}
        .onChange(of:personalService.projectionID){_,_ in personalScope=personalService.editingScope()}
        .onChange(of: workspace.crewID) { _, _ in
            failure = nil; readUnknown = false; reviewedUnknown = false
        }
    }
}

struct CrewdCrewCreationView: View {
    @ObservedObject var workspace: CrewdWorkspace
    @Environment(\.dismiss) private var dismiss
    @State private var failure: String?
    @State private var reading = false
    @State private var readAfterUnknown = false
    @State private var reviewedUnknown = false
    private var fields: CrewdCrewCreationDrafts.Fields { workspace.crewCreationDrafts.fields }
    private func input(_ key: WritableKeyPath<CrewdCrewCreationDrafts.Fields, String>) -> Binding<String> {
        Binding(get: { workspace.crewCreationDrafts.fields[keyPath: key] }, set: { var fields = workspace.crewCreationDrafts.fields; fields[keyPath: key] = $0; workspace.crewCreationDrafts.edit(fields); reviewedUnknown = false })
    }
    var body: some View {
        VStack(spacing:0){
            CrewdPanelHeader(title:"新建机组",detail:"把同一件事情的人、消息和工作放在一起。",symbol:"person.3",close:{dismiss()}).padding(20)
            Divider()
            ScrollView {
                VStack(alignment:.leading,spacing:20){
                    CrewdFormSection(title:"机组信息") {
                        CrewdFormField(title:"名称") {TextField("例如：产品发布",text:input(\.title))}
                        CrewdFormField(title:"所属机组",detail:"独立工作可保持独立机组；组织关系不代替访问权限。") {
                            Picker("上级机组",selection:input(\.parent)) {
                                Text("独立机组").tag("")
                                ForEach(workspace.crews.filter{$0["management"] != .bool(true)}.indices,id:\.self){index in
                                    let crew=workspace.crews.filter{$0["management"] != .bool(true)}[index]
                                    Text(crew["title"].string ?? "机组").tag(crew["id"].string ?? "")
                                }
                            }.labelsHidden()
                        }
                    }
                    CrewdFormSection(title:"电脑上的工作目录",detail:"使用当前连接电脑的路径；此操作不会启动成员或模型。") {
                        CrewdFormField(title:"目录路径"){directoryInput}
                        Menu("沿用已有机组的目录") {
                            ForEach(workspace.crews.indices,id:\.self){index in
                                let crew=workspace.crews[index]
                                if let directory=crew["workingDirectory"].string{Button(crew["title"].string ?? "工作目录"){input(\.directory).wrappedValue=directory}}
                            }
                        }.font(CrewdType.caption)
                    }
                    if !workspace.crewCreationDrafts.unknown.isEmpty {
                        CrewdFormSection(title:"此前创建结果未确认",detail:"原输入仍保留；先核对列表，避免重复创建。") {
                            ForEach(workspace.crewCreationDrafts.unknown,id:\.version){snapshot in
                                VStack(alignment:.leading,spacing:4){Text(snapshot.fields.title).font(CrewdType.label.weight(.medium));Text(snapshot.fields.directory).font(CrewdType.caption).foregroundStyle(.secondary).textSelection(.enabled)}
                            }
                            Button("读取当前机组列表核对"){Task{reading=true;defer{reading=false};do{try await workspace.refreshCrewDirectory();readAfterUnknown=true}catch{failure=error.localizedDescription}}}.disabled(reading || workspace.crewCreationDrafts.busy)
                            Toggle("已核对列表，仍要创建这份输入",isOn:$reviewedUnknown).disabled(!readAfterUnknown || reading)
                        }
                    }
                    if let failure{CrewdInlineNotice(title:"机组尚未创建",detail:failure,symbol:"exclamationmark.triangle",warning:true)}
                }.padding(20)
            }.background(Theme.Palette.surfaceMuted.opacity(0.22))
            Divider()
            VStack(alignment:.leading,spacing:8){
                Text(workspace.crewCreationDrafts.busy ? "正在提交原请求…":"创建后再为成员交代具体任务").font(CrewdType.caption).foregroundStyle(.secondary)
                HStack{
                Spacer()
                Button("取消"){dismiss()}.keyboardShortcut(.cancelAction)
                Button("创建机组"){create()}.buttonStyle(CrewdPrimaryButtonStyle())
                    .disabled(reading || workspace.busy || workspace.crewCreationDrafts.busy || !workspace.client.connected || fields.title.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || fields.directory.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || (!workspace.crewCreationDrafts.unknown.isEmpty && !reviewedUnknown))
                }
            }.padding(16)
        }.background(Theme.Palette.canvas).tint(Theme.Palette.accent)
        .modifier(CrewdAdaptiveFormFrame(width:560,height:640,scrolls:false))
    }
    private var directoryInput:some View {
        HStack{
            #if os(iOS)
            TextField("连接电脑的完整路径",text:input(\.directory)).textInputAutocapitalization(.never).autocorrectionDisabled()
            #else
            TextField("连接电脑的完整路径",text:input(\.directory))
            if workspace.dataDirectory != nil {
                Button("选择…") {
                    let panel=NSOpenPanel();panel.canChooseFiles=false;panel.canChooseDirectories=true
                    do{panel.directoryURL=try workspace.localDirectoryPickerRoot()}catch{workspace.error=error.localizedDescription;return}
                    if panel.runModal() == .OK,!workspace.isRetired{input(\.directory).wrappedValue=panel.url?.path ?? ""}
                }
            }
            #endif
        }
    }

    private func create() {
        guard let snapshot = workspace.crewCreationDrafts.begin() else { return }
        let owner = workspace
        var fields: [String: CrewdValue] = ["title": .string(snapshot.fields.title), "workingDirectory": .string(snapshot.fields.directory)]
        if !snapshot.fields.parent.isEmpty { fields["parentId"] = .string(snapshot.fields.parent) }
        failure = nil; readAfterUnknown = false; reviewedUnknown = false
        Task {
            do {
                _ = try await owner.performValue("crew.create", .object(fields))
                let wasCurrent=owner.crewCreationDrafts.isCurrent(snapshot)
                owner.crewCreationDrafts.finish(snapshot, accepted: true)
                if wasCurrent {dismiss()} else {failure="原创建已确认，当前编辑草稿仍保留。"}
            }
            catch { owner.crewCreationDrafts.finish(snapshot, accepted: false, uncertain: CrewdClientError.isUncertainSubmission(error)); failure = error.localizedDescription }
        }
    }
}
