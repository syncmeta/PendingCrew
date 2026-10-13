#if os(macOS)
import SwiftUI

/// Shared colors follow the original chat-pane's muted warm sidebar and mint selection.
enum CrewdChatStyle {
    static let sidebar = Color(red:0.94,green:0.935,blue:0.916)
    static let selection = Color(red:0.885,green:0.935,blue:0.917)
    static let bubble = Color(red:0.943,green:0.939,blue:0.924)
}
struct CrewdWorkPanels: View {
    @ObservedObject var store: CrewdWorkStore
    var progressOnly = false
    var compact = false
    @State private var collapsed:Set<String>=[]
    @State private var selected:String?
    @State private var showFolders=false
    @State private var failure:String?
    var body: some View {
        VStack(alignment:.leading,spacing:12) {
            if !compact {
                HStack {
                    Button("文件夹",systemImage:"folder") {showFolders.toggle()}.buttonStyle(.plain)
                    Spacer(); Button("刷新",systemImage:"arrow.clockwise") {Task {do {try await store.refresh()} catch {failure=error.localizedDescription}}}
                }
                if showFolders {Text("独立文件夹分组等待后台分类与视图接口接线；不会把文件夹写成计划父项。").font(.caption).foregroundStyle(.secondary)}
            }
            if let error=store.lastFailure { Text(error.localizedDescription).font(.caption).foregroundStyle(.orange).textSelection(.enabled) }
            if let failure { Text(failure).foregroundStyle(.orange).textSelection(.enabled) }
            if store.loading {ProgressView().controlSize(.small)}
            ScrollView {
                VStack(alignment:.leading,spacing:10) {
                    if !progressOnly && !compact {
                        Text("原始需求").font(.headline)
                        ForEach(store.requirementIDs,id:\.self) {id in
                            VStack(alignment:.leading,spacing:4) {
                                Text("\(id.raw)  \(store.requirements[id]?["text"].string ?? "")").textSelection(.enabled)
                                Text(store.requirements[id]?["status"].string ?? "").font(.caption).foregroundStyle(.secondary)
                                Text(store.plans(from:id).map {$0.id.raw}.joined(separator:" · ")).font(.caption).foregroundStyle(.secondary)
                            }.padding(8).frame(maxWidth:.infinity,alignment:.leading).background(CrewdChatStyle.bubble,in:RoundedRectangle(cornerRadius:6))
                        }
                        Text("计划").font(.headline)
                    }
                    ForEach(CrewdNavigation.planTree(store.progress,collapsed:collapsed)) {row in
                        if let id=CrewdWorkID(row.object,kind:.plan),let plan=store.plans[id] {
                            VStack(alignment:.leading,spacing:4) {
                                HStack(alignment:.top) {
                                    if row.expandable {Button {if !collapsed.insert(row.id).inserted {collapsed.remove(row.id)}} label:{Image(systemName:collapsed.contains(row.id) ? "chevron.right":"chevron.down")}.buttonStyle(.plain)}
                                    Image(systemName:plan.status == "done" ? "checkmark.circle.fill":plan.status == "blocked" ? "questionmark.circle":"circle").foregroundStyle(.secondary)
                                    Button { selected = selected == id.raw ? nil : id.raw; if selected != nil && !compact {Task {do {_ = try await store.detail(id)} catch {failure=error.localizedDescription}}} } label:{Text("\(id.raw)  \(plan.value["title"].string ?? "计划")").multilineTextAlignment(.leading)}.buttonStyle(.plain)
                                    Spacer()
                                    Text(plan.status).font(.caption).foregroundStyle(.secondary)
                                }
                                if !compact {
                                    Text([plan.value["owner"].string,plan.value["note"].string,plan.value["nextCheckAt"].string].compactMap{$0}.joined(separator:" · ")).font(.caption).foregroundStyle(.secondary)
                                    if let parent=plan.value["parent"].string,row.depth == 0 {Text("上级：\(parent)").font(.caption).foregroundStyle(.secondary)}
                                    if row.depth >= 3 {Text("后台返回超过三层的计划，请核对账本。").font(.caption).foregroundStyle(.orange)}
                                }
                                if selected == id.raw, !compact {
                                    if let detail=plan.detail {
                                        Text(detail["goal"].string ?? "").textSelection(.enabled)
                                        Text("来源：" + detail["sources"].array.compactMap(\.string).joined(separator:" · ")).font(.caption)
                                        Text("依赖：" + detail["dependsOn"].array.compactMap(\.string).joined(separator:" · ")).font(.caption)
                                        Text("卡点：" + detail["blockedOn"].array.compactMap(\.string).joined(separator:" · ")).font(.caption)
                                        if let evidence=detail["evidence"]["value"].string {Text(evidence).textSelection(.enabled)}
                                    } else {Text("正在读取详情").font(.caption).foregroundStyle(.secondary)}
                                }
                            }.padding(.leading,CGFloat(row.depth)*14).padding(.vertical,6)
                        }
                    }
                    if compact {
                        ForEach(store.humanTasks.keys.filter {store.humanTasks[$0]?["ask"]["crewId"].string == store.selectedCrew}.sorted(by:{$0.raw < $1.raw}),id:\.self) {id in
                            HStack(alignment:.top) {Image(systemName:"questionmark.circle"); Text("\(id.raw)  \(store.humanTasks[id]?["ask"]["question"].string ?? "")").font(.caption).lineLimit(2)}
                        }
                    }
                    if store.planIDs.isEmpty {Text("尚无计划").foregroundStyle(.secondary)}
                    if !progressOnly && !compact {
                        Divider(); Text("待人类处理 · 全组织收件箱").font(.headline)
                        ForEach(store.humanTasks.keys.sorted(by:{$0.raw < $1.raw}),id:\.self) {id in
                            let ask=store.humanTasks[id]?["ask"] ?? .null
                            Text("\(id.raw)  \(ask["question"].string ?? "")").textSelection(.enabled)
                            Text(ask["item"].string ?? ask["crewId"].string ?? "").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }.frame(maxWidth:.infinity,alignment:.leading)
            }
        }.padding(compact ? 8:16)
    }
}
#endif
