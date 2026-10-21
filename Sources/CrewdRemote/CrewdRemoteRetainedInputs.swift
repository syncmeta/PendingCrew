import SwiftUI

struct CrewdRetainedInput: Identifiable {
    let id: String
    let title: String
    private let content: () -> String
    init(id:String,title:String,text:String) {self.id=id;self.title=title;content={text}}
    init(id:String,title:String,content:@escaping ()->String) {self.id=id;self.title=title;self.content=content}
    func text()->String {content()}
}
/// Read-only user input snapshots; never server status or an automatic retry queue.
struct CrewdRemoteRetainedInputs: View {
    @ObservedObject var workspace: CrewdWorkspace
    @State private var showing: CrewdRetainedInput?
    private var inputs: [CrewdRetainedInput] {
        var rows: [CrewdRetainedInput] = []
        for unknown in workspace.durableUnknown {
            rows.append(.init(id: "durable-" + unknown.id.uuidString,
                title: unknown.method + " · 原提交结果待确认",
                text: "这是本机保留的提交观察记录，不是后台成功回执。不会自动重放。\n原作用域：" + unknown.scope.namespace.uuidString + "\n原机组：" + unknown.crew + "\n原参数：\n" + unknown.params.pretty))
        }
        for submission in workspace.uncertainTextSubmissions {
            rows.append(.init(id: submission.id.uuidString, title: submission.method == "session.start" ? "启动成员 · 结果待确认" : "消息 · 结果待确认", text: submission.snapshot.text))
        }
        for (index, draft) in workspace.messageDrafts.allSnapshots.enumerated() where !workspace.uncertainTextSnapshots.contains(draft) {
            let kind = draft.context.kind == .initialTask ? "启动说明草稿" : "消息草稿"
            let destination = [draft.context.crew, draft.context.channel, draft.context.session].filter { !$0.isEmpty }.joined(separator: " · ")
            rows.append(.init(id: "text-\(index)", title: kind + " · " + destination,
                text: "原作用域：" + workspace.presentationNamespace + "\n原机组：" + draft.context.crew + "\n原群聊：" + draft.context.channel + "\n原会话：" + draft.context.session + "\n\n" + draft.text))
        }
        for (index, draft) in workspace.uncertainSessionStarts.enumerated() {
            rows.append(.init(id: "start-\(index)", title: "启动成员 · 结果待确认", text: draft.text))
        }
        for draft in workspace.crewCreationDrafts.unknown {
            rows.append(.init(id: draft.version.uuidString, title: "创建机组 · 结果待确认", text: "名称：\(draft.fields.title)\n工作目录：\(draft.fields.directory)\n上级机组：\(draft.fields.parent)"))
        }
        for draft in workspace.requirementDrafts.allUnknownSnapshots {
            rows.append(.init(id: draft.revision.uuidString, title: "原始需求 · 结果待确认", text: draft.text))
        }
        for draft in workspace.planDrafts.allUnknownSnapshots {
            let f = draft.fields
            let values = [("标题",f.title),("目标",f.goal),("验收",f.acceptance),("负责人",f.owner),("父计划",f.parent),("说明",f.note),("凭据",f.evidence),("问题",f.question),("可推进部分",f.splitTitle)]
            var text = values.filter { !$0.1.isEmpty }.map { "\($0.0)：\($0.1)" }.joined(separator: "\n")
            text += "\n原计划：\(draft.key.object)\n原需求：\(draft.key.source)\n操作：\(f.action.rawValue)"
            if f.action == .report { text += "\n状态：\(CrewdWorkStatusLabel.display(f.reportStatus))\n检查点：\(f.nextCheck.formatted())" }
            if f.action == .block { text += f.split ? "\n拆出独立推进部分" : "\n其余暂不能推进" }
            rows.append(.init(id: draft.version.uuidString, title: "计划 · 结果待确认", text: text))
        }
        for draft in workspace.humanDrafts.allUnknownSnapshots {
            rows.append(.init(id: draft.version.uuidString, title: "人类处理 · 结果待确认", text: "\(draft.key.ask) · \(draft.fields.action.rawValue)\n\(draft.fields.text)"))
        }
        for draft in workspace.resourceDrafts.allUnknownSnapshots {
            let f = draft.fields
            rows.append(.init(id: draft.version.uuidString, title: draft.key.domain == .document ? "文档 · 结果待确认" : "分类 · 结果待确认", content: { "标题：\(f.title)\n原文：\(f.body)\n父分类：\(f.parent)\n位置：\(f.position)\n分类：\(f.folder)\n原版本：\(f.revision)" }))
        }
        for draft in workspace.channelLinkDrafts.allUnknownSnapshots {
            rows.append(.init(id: draft.requestID.uuidString, title: "本群工作引用 · 结果待确认", text: "工作：\(draft.object["id"].string ?? "")\n操作：\(draft.linked ? "关联" : "解除关联")\n原版本：\(draft.expectedRevision)"))
        }
        return rows
    }
    var body: some View {
        DisclosureGroup("原连接 · \(workspace.authorityRecordID.uuidString.prefix(8))") {
            Text("原作用域：" + workspace.presentationNamespace).font(.caption).textSelection(.enabled)
            if let failure = workspace.composerDrafts.storageError { Text(failure).foregroundStyle(.red) }
            if inputs.isEmpty { Text("没有需要核对的输入。") }
            ForEach(inputs) { input in Button(input.title) { showing = input } }
        }.sheet(item: $showing) { input in
            NavigationStack {
                ScrollView { Text(input.text()).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading).padding() }
                    .navigationTitle(input.title)
                    .toolbar { ToolbarItem(placement: .cancellationAction) { Button("关闭") { showing = nil } } }
            }.frame(minWidth: 320, minHeight: 300)
        }
    }
}
