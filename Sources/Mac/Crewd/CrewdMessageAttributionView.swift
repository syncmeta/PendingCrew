import SwiftUI

/// The exact message in the route where the user opened the inspector.
struct CrewdMessageInspectionTarget: Identifiable {
    let id = UUID()
    let source: CrewdMessageAttributionHistoryPage.Source
    let workScope: UUID
    let connection: UUID
    let authorityBinding: CrewdValue

    @MainActor static func canOpen(_ workspace: CrewdWorkspace, crew: String, channel: String) -> Bool {
        workspace.client.readyForWork && workspace.workStore.selectedCrew == crew &&
        workspace.crewID == crew && workspace.channelID == channel && !channel.isEmpty &&
        workspace.channels.contains { $0["id"] == .string(channel) && $0["crewId"] == .string(crew) }
    }

    @MainActor init?(workspace: CrewdWorkspace, crew: String, channel: String, message: String) {
        guard !message.isEmpty, Self.canOpen(workspace,crew:crew,channel:channel) else { return nil }
        source = .init(crew:crew,channel:channel,message:message)
        workScope = workspace.workStore.readScope
        connection = workspace.client.connectionEpoch
        authorityBinding = workspace.client.authorityBinding
    }

    @MainActor func isCurrent(_ workspace: CrewdWorkspace) -> Bool {
        Self.canOpen(workspace,crew:source.crew,channel:source.channel) &&
        workspace.workStore.readScope == workScope && workspace.client.connectionEpoch == connection &&
        workspace.client.authorityBinding == authorityBinding
    }
}

struct CrewdMessageInspectionSheet: View {
    @ObservedObject var workspace: CrewdWorkspace
    @ObservedObject private var store: CrewdWorkStore
    @Environment(\.dismiss) private var dismiss
    let target: CrewdMessageInspectionTarget
    let presentationCurrent: @MainActor () -> Bool
    @State private var visible = true
    @State private var historySourceEpoch = UUID()

    init(workspace: CrewdWorkspace, target: CrewdMessageInspectionTarget, presentationCurrent: @escaping @MainActor () -> Bool) {
        self.workspace = workspace
        store = workspace.workStore
        self.target = target
        self.presentationCurrent = presentationCurrent
    }

    private var current: Bool { visible && presentationCurrent() && target.isCurrent(workspace) }

    var body: some View {
        VStack(alignment:.leading,spacing:12) {
            HStack {
                Text("这条消息的判断记录").font(.headline)
                Spacer()
                Button("关闭") { visible = false; dismiss() }
            }
            if current {
                ScrollView {
                    VStack(alignment:.leading,spacing:16) {
                        Group {
                            Text("原始模型判断").font(.headline)
                            CrewdJudgmentInspectionView(store:store,crew:target.source.crew,kind:"message",
                                sourceID:target.source.message,members:Array(workspace.knownSessions.values),
                                automaticallyLoad:true,scopeCurrent:{ current })
                        }
                        Divider()
                        CrewdMessageAttributionView(store:store,source:target.source,
                            allowsCorrection:workspace.dataDirectory != nil,historySourceEpoch:historySourceEpoch,scopeCurrent:{ current })
                    }.frame(maxWidth:.infinity,alignment:.leading)
                }
                #if os(macOS)
                .frame(maxHeight:500)
                #endif
            } else {
                Text("当前消息、连接或授权已变化。请从消息重新打开。").foregroundStyle(.secondary)
            }
        }.padding()
        #if os(macOS)
        .frame(width:530)
        #endif
        .onReceive(workspace.authorityEvents) { event in
            guard current else { return }
            let (name, data) = event
            if name == "channel.purged", data["channelId"] == .string(target.source.channel) {
                visible = false
            } else if name == "events.lagged" ||
                (name == "channel.changed" && data["channel"]["id"] == .string(target.source.channel)) ||
                (name == "channel.workChanged" && data["channelId"] == .string(target.source.channel)) {
                historySourceEpoch = UUID()
            }
        }
        .onDisappear { visible = false }
    }
}

struct CrewdMessageAttributionView:View {
    @ObservedObject var store:CrewdWorkStore
    let source: CrewdMessageAttributionHistoryPage.Source
    let allowsCorrection: Bool
    let historySourceEpoch: UUID
    let scopeCurrent: @MainActor () -> Bool
    private var crew: String { source.crew }
    private var message: String { source.message }
    @State private var value:CrewdValue?
    @State private var selection=""
    @State private var targetKind="work"
    @State private var targetID=""
    @State private var failure:String?
    @State private var busy=false
    @State private var token=UUID()
    @State private var historyReload = UUID()
    private var identity:String {crew+"/"+source.channel+"/"+message}
    var body:some View {
        VStack(alignment:.leading,spacing:8) {
            Text("当前归属").font(.headline)
            if let value {
                Text(sourceLabel(value))
                if let id=value["link"]["id"].string {
                    Text("当前关联：" + (value["candidates"].array.first(where:{$0["link"] == value["link"]})?["summary"].string ?? (value["resolvedCandidate"]["link"] == value["link"] ? value["resolvedCandidate"]["summary"].string:nil) ?? "对象摘要暂不可读取") + "（" + id + "）")
                } else {Text(value["source"] == .string("human") ? "已明确设为无归属。":"当前没有有效归属。")}
                if allowsCorrection {
                    Picker("纠正为",selection:$selection) {
                        Text("请选择关联对象").tag("")
                        ForEach(Array(value["candidates"].array.enumerated()),id:\.offset) { index,candidate in
                            Text((candidate["summary"].string ?? "") + "（" + (candidate["link"]["id"].string ?? "") + "）").tag(String(index))
                        }
                    }
                    Picker("样本外对象类型",selection:$targetKind) {Text("计划").tag("work");Text("需求").tag("requirement");Text("前序消息").tag("message")}
                    TextField("对象编号",text:$targetID)
                    Button("查找并核对对象") {resolve()}.disabled(busy || targetID.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty)
                    if value["resolvedCandidate"] != .null {
                        Text("查找结果：" + (value["resolvedCandidate"]["summary"].string ?? ""))
                        Button("使用已核对对象纠正") {write("set",resolved:true)}.disabled(busy)
                    }
                    Text(value["candidatesTruncated"] == .bool(true) ? "候选为当前来源范围内的部分样本，并非全部对象。":"候选限当前来源范围；不会改变消息回复或工作负责人。").font(.caption).foregroundStyle(.secondary)
                    HStack {
                        Button("应用纠正") {write("set")}.disabled(selection.isEmpty)
                        Button("明确无归属") {write("clear")}
                        Button("撤销人工覆盖") {write("reset")}.disabled(value["override"] == .null)
                    }.disabled(busy)
                    Text("撤销后只恢复仍然有效的已有模型结果，不会重新请求模型。").font(.caption).foregroundStyle(.secondary)
                }
            }
            Button(busy ? "正在处理…" : allowsCorrection ? "刷新归属及候选" : "刷新当前归属") { read() }.disabled(busy)
            if let failure {Text(failure).foregroundStyle(.orange)}
            Divider()
            CrewdMessageAttributionHistoryView(store:store,source:source,reload:[historySourceEpoch,historyReload],scopeCurrent:scopeCurrent)
        }.font(.callout)
        .task(id:identity) {read()}
        .onChange(of:identity) {_,_ in invalidate()}
        .onChange(of:store.readScope) {_,_ in invalidate()}
        .onDisappear {invalidate()}
    }
    private func sourceLabel(_ v:CrewdValue)->String {["model":"当前采用模型归属","human":"当前采用人工纠正","none":"暂无归属记录","stale":"原归属依据已失效"][v["source"].string ?? ""] ?? "未知来源"}
    private func invalidate() {token=UUID();value=nil;failure=nil;selection="";busy=false}
    private func read() {
        guard scopeCurrent() else { return }
        let request=UUID(),original=identity;token=request;busy=true;failure=nil
        Task { @MainActor in
            defer {if token == request {busy=false}}
            guard token == request, identity == original, scopeCurrent() else { return }
            do {let result=try await store.messageAttributionWithSummary(crew:crew,message:message);guard token == request,identity == original,scopeCurrent() else{return};value=result;selection=""}
            catch {if token == request,scopeCurrent() {failure=error.localizedDescription}}
        }
    }
    private func resolve() {
        guard allowsCorrection, scopeCurrent() else { return }
        let request=UUID(),original=identity;token=request;busy=true;failure=nil
        let target:CrewdValue = .object(["kind":.string(targetKind),"id":.string(targetID.trimmingCharacters(in:.whitespacesAndNewlines))])
        Task { @MainActor in
            defer {if token == request {busy=false}}
            guard token == request, identity == original, scopeCurrent() else { return }
            do {let result=try await store.messageAttribution(crew:crew,message:message,target:target);guard token == request,identity == original,scopeCurrent() else{return};value=result;selection=""}
            catch {if token == request,scopeCurrent() {failure=error.localizedDescription}}
        }
    }
    private func write(_ action:String,resolved:Bool=false) {
        guard allowsCorrection, scopeCurrent(), let value else { return }
        let link:CrewdValue?
        if action == "set",resolved {link=value["resolvedCandidate"]["link"]}
        else if action == "set" {guard let index=Int(selection),value["candidates"].array.indices.contains(index) else{return};link=value["candidates"].array[index]["link"]} else {link=nil}
        let request=UUID(),original=identity;token=request;busy=true;failure=nil
        Task { @MainActor in
            defer {if token == request {busy=false}}
            guard token == request, identity == original, scopeCurrent() else { return }
            do {let result=try await store.setMessageAttribution(crew:crew,message:message,revision:value["revision"].string ?? "",action:action,link:link);guard token == request,identity == original,scopeCurrent() else{return};self.value=result;selection="";historyReload=UUID()
                if action == "set",let enriched=try? await store.messageAttributionWithSummary(crew:crew,message:message),token == request,identity == original,scopeCurrent() {self.value=enriched}
            }
            catch is CancellationError {return}
            catch {
                guard token == request,identity == original,scopeCurrent() else{return}
                failure="操作未获确认："+error.localizedDescription+"；只刷新记录，不会重发。"
                historyReload=UUID()
                if let result=try? await store.messageAttributionWithSummary(crew:crew,message:message),token == request,identity == original,scopeCurrent() {self.value=result;selection=""}
            }
        }
    }
}


private struct CrewdMessageAttributionHistoryView: View {
    typealias Page = CrewdMessageAttributionHistoryPage
    @ObservedObject var store: CrewdWorkStore
    let source: Page.Source
    let reload: [UUID]
    let scopeCurrent: @MainActor () -> Bool
    @State private var items: [Page.Item] = []
    @State private var cursor: String?
    @State private var seen: Set<String> = []
    @State private var olderUnavailable = false
    @State private var loaded = false
    @State private var error: String?
    @State private var request = UUID()
    @State private var reading: Task<Void,Never>?

    var body: some View {
        VStack(alignment:.leading,spacing:8) {
            Text("人工纠正历史").font(.headline)
            if !store.supportsMessageAttributionHistory {
                Text("当前后台未提供人工纠正历史。").foregroundStyle(.secondary)
            } else {
                if loaded && items.isEmpty { Text("未读取到人工纠正记录。").foregroundStyle(.secondary) }
                ForEach(Array(items.enumerated()),id:\.offset) { _, item in historyRow(item) }
                if olderUnavailable {
                    Text("更早的记录超出此次可查询范围。").font(.caption).foregroundStyle(.secondary)
                }
                if let cursor {
                    Button("读取更早记录") { read(after:cursor) }.disabled(reading != nil)
                }
                if let error { Text(error).foregroundStyle(.orange) }
                Button(reading == nil ? "刷新人工纠正历史" : "正在读取人工纠正历史…") { read() }.disabled(reading != nil)
            }
        }
        .task(id:reload) { read() }
        .onChange(of:store.readScope) { _,_ in clear() }
        .onChange(of:store.supportsMessageAttributionHistory) { _,supported in
            clear()
            if supported { read() }
        }
        .onDisappear { clear() }
    }

    private func clear() {
        request = UUID(); reading?.cancel(); reading = nil
        items = []; seen = []; cursor = nil; olderUnavailable = false; loaded = false; error = nil
    }

    private func read(after previous: String? = nil) {
        guard scopeCurrent(), store.supportsMessageAttributionHistory else { clear(); return }
        if previous == nil { clear() }
        else { guard reading == nil, cursor == previous else { return } }
        let token = UUID(), scope = store.readScope
        request = token
        reading = Task { @MainActor in
            defer { if request == token { reading = nil } }
            guard request == token, scopeCurrent(), !Task.isCancelled else { return }
            do {
                let page = try await store.messageAttributionHistory(source:source,cursor:previous)
                guard request == token, scope == store.readScope, scopeCurrent(), !Task.isCancelled else { return }
                let total = items.count + page.items.count
                guard total <= Page.limit, page.nextCursor == nil || total < Page.limit,
                      !page.olderUnavailable || total == Page.limit else {
                    throw CrewdClientError(message:"分页记录边界无效，请刷新。")
                }
                if let next = page.nextCursor, !seen.insert(next).inserted {
                    throw CrewdClientError(message:"分页记录重复，请刷新。")
                }
                items.append(contentsOf:page.items)
                cursor = page.nextCursor; olderUnavailable = page.olderUnavailable; loaded = true; error = nil
            } catch is CancellationError {
                return
            } catch {
                guard request == token, scope == store.readScope, scopeCurrent() else { return }
                clear()
                self.error = previous == nil ? "无法读取人工纠正历史。请刷新重读。" : "无法继续核对这一页。请从最新记录重新读取。"
            }
        }
    }

    private func historyRow(_ item: Page.Item) -> some View {
        VStack(alignment:.leading,spacing:4) {
            Text(actionLabel(item.action)).font(.callout.weight(.semibold))
            HStack(spacing:8) {
                Text(Date(timeIntervalSince1970:Double(item.atMs) / 1000),format:.dateTime.year().month().day().hour().minute().second())
                if item.historical { Text("历史范围") }
            }.font(.caption).foregroundStyle(.secondary)
            Text("此前：" + overrideLabel(item.before))
            Text("此后：" + overrideLabel(item.after))
        }.frame(maxWidth:.infinity,alignment:.leading)
    }

    private func actionLabel(_ action: Page.Action) -> String {
        switch action {
        case .set: return "设置关联"
        case .clear: return "明确设为无归属"
        case .reset: return "撤销人工覆盖"
        }
    }

    private func overrideLabel(_ state: Page.Override?) -> String {
        guard let state else { return "无人工覆盖" }
        if state.action == .clear { return "人工明确设为无归属" }
        if state.targetUnavailable { return "当时设置过关联，目标当前不可核对" }
        guard let target = state.target else { return "目标不可核对" }
        let label: String
        switch target.kind {
        case .message: label = "消息"
        case .work: label = "计划"
        case .requirement: label = "需求"
        }
        return label + " " + target.id
    }
}
