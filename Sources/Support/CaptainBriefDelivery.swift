import Foundation

/// 建 crew 后自动起机长的一次性请求。
///
/// 用数组队列承载（见 `CrewStore.captainAutostartRequests`），避免同一轮创建多个
/// crew 时 SwiftUI 合并 `@Published` 变更、只消费最后一个。`sourceCrewId` 非 nil
/// 表示这是父机长创建子 crew 的开场任务；启动失败时必须回执到这个父 crew。
struct CaptainAutostartRequest: Equatable, Identifiable {
    let id: UUID
    let crewId: String
    let brief: String?
    let sourceCrewId: String?
    let childTitle: String

    init(id: UUID = UUID(), crewId: String, brief: String? = nil,
         sourceCrewId: String? = nil, childTitle: String) {
        self.id = id
        self.crewId = crewId
        self.brief = brief
        self.sourceCrewId = sourceCrewId
        self.childTitle = childTitle
    }

    /// 只有 `create_child_crew` 的非空 brief 才需要回父 crew fail-loud；普通手建
    /// crew 的自动报到失败继续走 runner 自己的 UI/子白板错误通道。
    func deliveryFailureReceipt(reason: String) -> (crewId: String, text: String)? {
        guard let sourceCrewId,
              brief?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == false else {
            return nil
        }
        return (
            sourceCrewId,
            "子 crew「\(childTitle)」已建出来，但开场任务没送到（\(reason)）。请手动转达。")
    }
}

/// brief / @ 唤醒文本到机长首轮 prompt 的单一拼装口，供两种 runner 共用。
enum CaptainBriefDelivery {
    static func openingPrompt(brief: String?, wakeText: String?) -> String {
        let task = brief?.trimmingCharacters(in: .whitespacesAndNewlines)
        let wake = wakeText?.trimmingCharacters(in: .whitespacesAndNewlines)

        if let task, !task.isEmpty {
            var prompt = "你是本 crew 的机长。父机长交给你的开场任务如下：\n\(task)"
                + "\n先用 post_to_crew 简短确认已收到，然后立即推进这项任务。"
            if let wake, !wake.isEmpty {
                prompt += "\n同时有人在群里 @ 你：「\(wake)」，一并处理。"
            }
            return prompt
        }
        if let wake, !wake.isEmpty {
            return "你是本 crew 的机长。刚有人在群里 @ 你：「\(wake)」"
                + "——用 post_to_crew 报到一句，然后处理这条消息。"
        }
        return "你是本 crew 的机长，用 post_to_crew 报到一句即可。"
    }
}
