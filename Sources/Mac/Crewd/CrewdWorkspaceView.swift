#if os(macOS)
import SwiftUI
import AppKit

struct CrewdWorkspaceView: View {
    @ObservedObject var workspace: CrewdWorkspace
    var autoConnect = true
    @State private var drafts = CrewdDrafts()
    @State private var creatingCrew = false
    @State private var crewTitle = ""
    @State private var workingDirectory = ""
    private var messageContext: CrewdDrafts.Context {
        .message(crew: workspace.crewID, channel: workspace.channelID, session: workspace.sessionID)
    }
    private var taskContext: CrewdDrafts.Context {
        .initialTask(crew: workspace.crewID, session: workspace.sessionID)
    }
    private var compose: String { drafts.text(for: messageContext) }
    private var task: String { drafts.text(for: taskContext) }
    private func draftBinding(_ context: CrewdDrafts.Context) -> Binding<String> {
        Binding(get: { drafts.text(for: context) }, set: { drafts.set($0, for: context) })
    }
    var body: some View {
        NavigationSplitView {
            List {
                ForEach(workspace.crews.indices, id: \.self) { index in
                    let crew = workspace.crews[index]
                    Button { workspace.selectCrew(crew["id"].string ?? "") } label: {
                        HStack { Text(crew["title"].string ?? "机组"); Spacer(); if crew["id"].string == workspace.crewID { Image(systemName: "checkmark") } }
                    }.buttonStyle(.plain)
                }
            }
            .navigationTitle("PendingCrew")
            .safeAreaInset(edge: .bottom) {
                VStack(alignment: .leading, spacing: 8) {
                    Button("新建机组", systemImage: "plus") { creatingCrew = true }
                    Text("Codex 首版 · 独立数据空间").font(.caption).foregroundStyle(.secondary)
                    Text(workspace.status).font(.caption).foregroundStyle(.secondary)
                    Button("重新连接") { Task { await workspace.connect() } }
                }.padding()
            }
        } detail: {
            VStack(spacing: 0) {
                if workspace.restartPrompt != .null { restartQuestion }
                if let error = workspace.error {
                    HStack { Image(systemName: "exclamationmark.triangle"); Text(error).textSelection(.enabled); Spacer(); Button("关闭") { workspace.error = nil } }
                        .padding(10).background(.orange.opacity(0.12))
                }
                if workspace.crewID.isEmpty {
                    ContentUnavailableView("建立你的第一个机组", systemImage: "person.3", description: Text("选择工作目录，与 Codex 一起开始工作。"))
                } else {
                    HSplitView { conversation.frame(minWidth: 330); session.frame(minWidth: 420) }
                }
            }
        }
        .frame(minWidth: 1050, minHeight: 680)
        .sheet(isPresented: $creatingCrew) { crewForm }
        .task { if autoConnect { await workspace.connect() } }
    }

    private var restartQuestion: some View {
        let prompt = workspace.restartPrompt
        let count = prompt["sessionIds"].array.count
        return VStack(alignment: .leading, spacing: 8) {
            Label(prompt["reason"].string == "hibernate" ? "后台已从休眠启动" : "后台已重新启动", systemImage: "arrow.clockwise")
                .font(.headline)
            Text("上次运行的 \(count) 个会话正在等待恢复；尚未自动启动。")
            if !prompt["interrupted"].array.isEmpty {
                Text("其中 \(prompt["interrupted"].array.count) 个会话的工作曾被打断，恢复后请核对结果。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            HStack {
                Button("恢复这些会话") { answerRestart(prompt, "resume") }
                Button("暂不恢复") { answerRestart(prompt, "later") }
                    .disabled(prompt["answer"].string == "later")
                if prompt["answer"].string == "later" {
                    Text("已暂缓，你可以稍后恢复。") .font(.caption).foregroundStyle(.secondary)
                }
            }.disabled(workspace.busy || !workspace.client.connected)
        }.frame(maxWidth: .infinity, alignment: .leading).padding(12).background(.blue.opacity(0.08))
    }

    private func answerRestart(_ prompt: CrewdValue, _ answer: String) {
        Task { _ = await workspace.perform("restart.answer", .object(["id": prompt["id"], "answer": .string(answer)])) }
    }

    private var conversation: some View {
        VStack(alignment: .leading, spacing: 12) {
            Picker("群", selection: Binding(get: { workspace.channelID }, set: { workspace.selectChannel($0) })) {
                ForEach(workspace.channels.indices, id: \.self) { i in
                    let channel = workspace.channels[i]
                    Text(channelName(channel)).tag(channel["id"].string ?? "")
                }
            }
            if !workspace.pendingAsks.isEmpty {
                DisclosureGroup("机组等待你的答复（\(workspace.pendingAsks.count)）") {
                    ScrollView {
                        ForEach(workspace.pendingAsks.indices, id: \.self) { i in
                            let ask = workspace.pendingAsks[i]["ask"]
                            CrewdCrewQuestionView(workspace: workspace, ask: ask).id((ask["crewId"].string ?? "") + "/" + (ask["ask"].string ?? ""))
                        }
                    }.frame(maxHeight: 180)
                }
            }
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 14) {
                        ForEach(workspace.messages.indices, id: \.self) { i in
                            let message = workspace.messages[i]
                            VStack(alignment: .leading, spacing: 4) {
                                Text(message["senderName"].string ?? message["senderKind"].string ?? "成员").font(.caption).foregroundStyle(.secondary)
                                Text(message["text"].string ?? "").textSelection(.enabled)
                            }.id(message["id"].string ?? "\(i)")
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 8)
                }
                .onChange(of: workspace.messages.count) { _, _ in
                    if let last = workspace.messages.last?["id"].string { proxy.scrollTo(last, anchor: .bottom) }
                }
            }
            TextField("发给当前 Codex 会话", text: draftBinding(messageContext), axis: .vertical).lineLimit(2...6)
            HStack {
                Text("发送会通知选中的会话").font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("发送") {
                    let sent = drafts.snapshot(for: messageContext)
                    let text = sent.text, channel = sent.context.channel, target = sent.context.session
                    Task {
                        let mentions: [CrewdValue] = target.isEmpty ? [] : [.object(["kind": .string("session"), "targetId": .string(target)])]
                        if await workspace.perform("message.post", .object(["channelId": .string(channel), "text": .string(text), "mentions": .array(mentions)])) { drafts.clearIfUnchanged(sent) }
                    }
                }.disabled(compose.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || workspace.busy || workspace.channelID.isEmpty || workspace.sessionID.isEmpty)
            }
        }.padding()
    }

    private var session: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Picker("会话", selection: Binding(get: { workspace.sessionID }, set: { workspace.selectSession($0) })) {
                    ForEach(workspace.sessions.indices, id: \.self) { i in
                        let row = workspace.sessions[i]
                        Text(row["displayName"].string ?? row["title"].string ?? row["role"].string ?? "Codex").tag(row["id"].string ?? "")
                    }
                }
                if !workspace.sessionID.isEmpty {
                    Button("打断") { action("session.interrupt") }
                    Button("停止") { action("session.stop", extra: ["reason": .string("人类在 Mac 客户端停止会话")]) }
                    Button("恢复") { action("session.resume") }
                }
            }.disabled(workspace.busy)
            if !workspace.sessionID.isEmpty {
                Label(workspace.inspection["runnerRequest"] != .null ? "等待你的答复" : workspace.inspection["live"]["working"] == .bool(true) ? "正在工作" : workspace.inspection["live"]["running"] == .bool(true) ? "等待下一步" : "会话尚未运行", systemImage: workspace.inspection["live"]["working"] == .bool(true) ? "gearshape" : "circle")
                    .font(.caption).foregroundStyle(.secondary)
            }
            if let row = workspace.sessions.first(where: {$0["id"].string == workspace.sessionID}) {
                Text([row["model"].string, row["effort"].string].compactMap {$0}.joined(separator: " · ")).font(.caption).foregroundStyle(.secondary)
            }
            if workspace.inspection["live"]["health"] != .null {
                Text(workspace.inspection["live"]["health"]["detail"].string ?? "会话状态异常").foregroundStyle(.orange).textSelection(.enabled)
            }
            if workspace.inspection["runnerRequest"] != .null {
                CrewdHumanRequestView(workspace: workspace, request: workspace.inspection["runnerRequest"], decision: workspace.inspection["live"]["pendingDecision"])
                    .id(workspace.inspection["runnerRequest"]["fingerprint"].string ?? "")
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    if !workspace.history.isEmpty {
                        DisclosureGroup("已恢复的会话记录") { Text(workspace.history).font(.system(.body, design: .monospaced)).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading) }
                    }
                    ForEach(workspace.runnerItems.indices, id: \.self) { i in CrewdRunnerItemView(item: workspace.runnerItems[i]) }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
            Divider()
            TextField("给新的 Codex 会话交代任务", text: draftBinding(taskContext), axis: .vertical).lineLimit(2...6)
            HStack {
                Text("沿用 Codex 的默认模型和设置").font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("启动 Codex") {
                    let sent = drafts.snapshot(for: taskContext)
                    let brief = sent.text, crew = sent.context.crew
                    let role = workspace.sessions.contains(where: {$0["role"].string == "captain"}) ? "worker" : "captain"
                    Task {
                        if await workspace.perform("session.start", .object(["crewId": .string(crew), "role": .string(role), "executor": .string("codex"), "brief": .string(brief), "isolation": .bool(true)])) { drafts.clearIfUnchanged(sent) }
                    }
                }.disabled(task.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || workspace.busy)
            }
        }.padding()
    }
    private func action(_ method: String, extra: [String: CrewdValue] = [:]) {
        var p = extra; p["sessionId"] = .string(workspace.sessionID)
        Task { _ = await workspace.perform(method, .object(p)) }
    }
    private func channelName(_ row: CrewdValue) -> String {
        switch row["kind"].string { case "human": return "与我沟通"; case "lobby": return "大厅"; default: return row["name"].string ?? "群" }
    }
    private var crewForm: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("新建机组").font(.title2)
            TextField("机组名称", text: $crewTitle)
            HStack { Text(workingDirectory.isEmpty ? "选择工作目录" : workingDirectory).lineLimit(2).textSelection(.enabled); Spacer(); Button("选择…") {
                let panel = NSOpenPanel(); panel.canChooseFiles = false; panel.canChooseDirectories = true
                if panel.runModal() == .OK { workingDirectory = panel.url?.path ?? "" }
            } }
            HStack { Button("取消") { creatingCrew = false }; Spacer(); Button("创建") {
                let title = crewTitle, path = workingDirectory
                Task { if await workspace.perform("crew.create", .object(["title": .string(title), "workingDirectory": .string(path)])) { creatingCrew = false; crewTitle = ""; workingDirectory = "" } }
            }.disabled(crewTitle.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || workingDirectory.isEmpty || workspace.busy) }
        }.padding(24).frame(width: 500)
    }
}

private struct CrewdCrewQuestionView: View {
    @ObservedObject var workspace: CrewdWorkspace
    let ask: CrewdValue
    @State private var answer = ""
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(ask["question"].string ?? "机组的问题").textSelection(.enabled)
            HStack {
                TextField("你的答复", text: $answer)
                Button("答复") {
                    let text = answer
                    Task {
                        if await workspace.perform("ask.answer", .object(["crewId": ask["crewId"], "ask": ask["ask"], "text": .string(text)])), answer == text { answer = "" }
                    }
                }.disabled(answer.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || workspace.busy)
            }
        }.padding(.vertical, 6)
    }
}

private struct CrewdHumanRequestView: View {
    @ObservedObject var workspace: CrewdWorkspace
    let request: CrewdValue
    let decision: CrewdValue
    @State private var answers: [String: String] = [:]
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("Codex 等待你的答复", systemImage: "person.crop.circle.badge.questionmark").font(.headline)
            if request["method"].string == "item/tool/requestUserInput" {
                ForEach(request["params"]["questions"].array.indices, id: \.self) { index in
                    let q = request["params"]["questions"].array[index]
                    let id = q["id"].string ?? ""
                    Text(q["question"].string ?? "")
                    if !q["options"].array.isEmpty {
                        ForEach(q["options"].array.indices, id: \.self) { option in
                            let row = q["options"].array[option]
                            Button(row["label"].string ?? "") { answers[id] = row["label"].string }
                            if let detail = row["description"].string { Text(detail).font(.caption).foregroundStyle(.secondary) }
                        }
                    }
                    if q["options"].array.isEmpty || q["isOther"] == .bool(true) {
                        if q["isSecret"] == .bool(true) { SecureField("你的答复", text: binding(id)) }
                        else { TextField("你的答复", text: binding(id)) }
                    }
                    if q["isSecret"] != .bool(true), let value = answers[id], !value.isEmpty { Text("已选：\(value)").font(.caption).foregroundStyle(.secondary) }
                }
                Button("提交答复") {
                    let response = CrewdValue.object(["answers": .object(answers.mapValues { .object(["answers": .array([.string($0)])]) })])
                    send(method: "session.respondRunnerRequest", extra: ["response": response])
                }.disabled(workspace.busy || request["params"]["questions"].array.contains(where: { (answers[$0["id"].string ?? ""] ?? "").isEmpty }))
            } else {
                Text(decision["summary"].string ?? "请确认当前请求").textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                ForEach(decision["options"].array.indices, id: \.self) { i in
                    Button(decision["options"].array[i].string ?? "选项") { send(method: "session.answer", extra: ["choice": .number(Double(i))]) }.disabled(workspace.busy)
                }
            }
        }.padding(12).frame(maxWidth: .infinity, alignment: .leading).background(.blue.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
    }
    private func binding(_ id: String) -> Binding<String> { Binding(get: { answers[id] ?? "" }, set: { answers[id] = $0 }) }
    private func send(method: String, extra: [String: CrewdValue]) {
        var params = extra; params["sessionId"] = .string(workspace.sessionID); params["fingerprint"] = request["fingerprint"]
        Task { _ = await workspace.perform(method, .object(params)) }
    }
}

private struct CrewdRunnerItemView: View {
    let item: CrewdValue
    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            switch item["type"].string {
            case "agentMessage": Text(item["text"].string ?? "").textSelection(.enabled)
            case "userMessage": Text(item["content"].array.compactMap {$0["text"].string}.joined(separator: "\n")).foregroundStyle(.secondary).textSelection(.enabled)
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

struct PendingCrewNextApp: App {
    @StateObject private var workspace: CrewdWorkspace
    init() {
        let explicit = ProcessInfo.processInfo.environment["PENDINGCREW_CREWD_DATA_DIR"]
        let directory = explicit.map { URL(fileURLWithPath: $0) } ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/PendingCrewNext")
        _workspace = StateObject(wrappedValue: CrewdWorkspace(dataDirectory: directory))
    }
    var body: some Scene {
        WindowGroup("PendingCrew · Codex 首版") { CrewdWorkspaceView(workspace: workspace) }
        .commands { CommandGroup(replacing: .help) { Text("Codex 首版使用独立数据目录，旧版数据保留。") } }
    }
}
#endif
