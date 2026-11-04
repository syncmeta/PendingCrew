import SwiftUI

/// An inspection reads an existing record; it never asks the model again.
struct CrewdJudgmentInspectionView: View {
    @ObservedObject var store: CrewdWorkStore
    let crew: String
    let kind: String
    let sourceID: String
    var members: [CrewdValue] = []
    var automaticallyLoad = false
    var scopeCurrent: @MainActor () -> Bool = { true }
    @State private var record: CrewdValue?
    @State private var error: String?
    @State private var busy = false
    @State private var request = UUID()
    private var identity: String { crew + "/" + kind + "/" + sourceID }
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button(busy ? "正在读取判断记录…" : "读取判断记录") { read() }.disabled(busy)
            if let error { Text("读取失败：" + error).foregroundStyle(.orange) }
            if let record {
                if record == .null {
                    Text("没有这条内容的判断记录；不能据此认定模型已经处理。")
                } else {
                    Text(phase(record["phase"].string ?? ""))
                    Text(reason(record["reason"].string ?? ""))
                    let recipients = record["recipients"].array.compactMap(\.string)
                    Text(recipients.isEmpty ? "记录中没有接收成员。" : "记录的接收成员：" + recipients.map(memberName).joined(separator: "、"))
                    if let id = record["attribution"]["id"].string {
                        Text("原判断中的关联对象：" + id)
                    } else { Text("记录中没有关联对象。") }
                    Text("接收名单和关联是记录结果，不代表成员已经执行或任务已经完成。").font(.caption).foregroundStyle(.secondary)
                }
            }
        }.font(.callout)
        .task(id: identity) { if automaticallyLoad { read() } }
        .onChange(of: identity) { _, _ in invalidate() }
        .onChange(of: store.readScope) { _, _ in invalidate() }
        .onDisappear { invalidate() }
    }
    private func invalidate() { request = UUID(); record = nil; error = nil; busy = false }
    private func read() {
        guard scopeCurrent() else { return }
        let token = UUID(); request = token; busy = true; record = nil; error = nil
        let original = identity
        Task { @MainActor in
            defer { if request == token { busy = false } }
            guard request == token, identity == original, scopeCurrent() else { return }
            do {
                let value = try await store.inspectJudgment(crew: crew, kind: kind, id: sourceID)
                guard request == token, identity == original, scopeCurrent() else { return }
                record = value
            } catch {
                guard request == token, identity == original, scopeCurrent() else { return }
                self.error = error.localizedDescription
            }
        }
    }
    private func memberName(_ id: String) -> String {
        guard let member = members.first(where: { $0["id"].string == id }),
              let name = member["displayName"].string ?? member["title"].string ?? member["role"].string,
              !name.isEmpty else { return id }
        let duplicates = members.filter { ($0["displayName"].string ?? $0["title"].string ?? $0["role"].string) == name }.count
        return duplicates > 1 ? name + "（" + String(id.suffix(8)) + "）" : name
    }
    private func phase(_ value: String) -> String {
        switch value {
        case "pending": return "等待判断"
        case "dispatched": return "判断请求已发出，结果尚未确定"
        case "settled": return "本次处理已结束"
        case "suppressed": return "情况已变化，本次判断未应用"
        case "historical": return "历史记录，不代表当前状态"
        default: return "判断状态未知"
        }
    }
    private func reason(_ value: String) -> String {
        switch value {
        case "answered": return "模型已返回答案；不代表答案已被采纳。"
        case "prepared": return "已准备，尚未取得答案。"
        case "unconfigured": return "未配置判断模型，使用既有处理规则。"
        case "deadline": return "判断超时，使用既有处理规则。"
        case "provider_error": return "模型服务出错，使用既有处理规则。"
        case "invalid_response": return "模型答案无效，使用既有处理规则。"
        case "restart_unknown": return "重启前的判断结果未知，没有重新请求模型。"
        case "stale": return "判断依据已过期。"
        case "busy": return "判断服务繁忙，使用既有处理规则。"
        case "configuration_changed": return "判断配置已改变。"
        default: return "处理原因未知。"
        }
    }
}
