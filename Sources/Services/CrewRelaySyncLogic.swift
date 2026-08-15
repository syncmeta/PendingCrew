import Foundation

/// CrewRelayAgent 的纯同步逻辑（接合 v2 block 3）。
///
/// **自包含**：只依赖 Foundation、只吃轻量值类型 —— 不引 `CrewWhiteboardEntry`
/// / `LocalWhiteboardMessage`，方便用 swiftc 单独编译做单测（与 LocalRunner
/// 系列测试同款打法）。CrewRelayAgent 负责把 wire/store 模型映射成这里的
/// `PullItem` / `LocalItem`。
enum CrewRelaySyncLogic {
    /// 一条从 edge 拉下来的白板条目（拉取侧视角）。
    struct PullItem: Equatable {
        /// edge message id（幂等去重 key）。
        let remoteId: String
        /// `relay.origin == "mac_relay"` —— Mac 自己推上去的，绝不能再写回
        /// 本地白板（回环防护）。
        let isMacRelayOrigin: Bool
    }

    /// 一条本地白板消息（推送侧视角）。
    struct LocalItem: Equatable {
        let id: String
        /// 非 nil `relayRemoteId` —— relay 搬进来的，不上行（防回环）。
        let isRelayWritten: Bool
    }

    /// 拉取合并：过滤掉 Mac 自己推上去的（`origin == mac_relay`）和已同步过的
    /// （`known` = 本地白板已存的 relayRemoteId 集合 —— edge `?since=` 是
    /// **闭区间**，游标边界条目每次都会重复出现）。返回应写入本地白板的
    /// remoteId，保持 edge 返回顺序（created_at 升序）。
    static func importableRemoteIds(pulled: [PullItem], known: Set<String>) -> [String] {
        var seen = known
        var out: [String] = []
        for item in pulled {
            guard !item.isMacRelayOrigin, !seen.contains(item.remoteId) else { continue }
            seen.insert(item.remoteId)   // 同批内重复也去掉
            out.append(item.remoteId)
        }
        return out
    }

    // MARK: - task_request 指令提取（#242 遥控 v1）

    /// 一条拉取条目在 task_request 识别视角下的轻量投影。
    struct TaskRequestItem: Equatable {
        let remoteId: String
        /// Mac 自己推上去的 —— 永远不当指令处理（防回环）。
        let isMacRelayOrigin: Bool
        /// `entry.messageKind == "task_request"`。
        let isTaskRequest: Bool
        /// payload.action —— 当前只认 `"run_session"`（nil 视作 run_session，
        /// 容忍老/省字段客户端）；其他 action 留给未来 kind，本版静默跳过。
        let action: String?
        let taskBrief: String?
        let runnerKind: String?
    }

    /// 提取出的「该在本机起 session」的指令。
    struct RelayTaskRequest: Equatable {
        /// edge message id —— 处理后持久化进 processedTaskRequestIds 防重跑。
        let remoteId: String
        let taskBrief: String
        /// payload.runner_kind 原样透传（nil → 调用方默认 claude_code）。
        let runnerKind: String?
    }

    /// 从一批拉取条目中提取新 task_request 指令：
    /// - 只认 messageKind == task_request 且 action ∈ {nil, "run_session"}
    /// - task_brief 缺失/空白 → 跳过（指令无效，不标记、不报错）
    /// - 排除 mac_relay 来源（自己发的）与 processed（重启后按持久化集合幂等）
    /// - 同批内按 remoteId 去重，保持 edge 返回顺序
    static func newTaskRequests(
        pulled: [TaskRequestItem], processed: Set<String>
    ) -> [RelayTaskRequest] {
        var seen = processed
        var out: [RelayTaskRequest] = []
        for item in pulled {
            guard item.isTaskRequest, !item.isMacRelayOrigin, !seen.contains(item.remoteId)
            else { continue }
            guard item.action == nil || item.action == "run_session" else { continue }
            guard let brief = item.taskBrief?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !brief.isEmpty
            else { continue }
            seen.insert(item.remoteId)
            out.append(RelayTaskRequest(
                remoteId: item.remoteId, taskBrief: brief, runnerKind: item.runnerKind))
        }
        return out
    }

    /// 推送筛选：水位（最后一条已推送的本地消息 id）**之后**的、本地产生的
    /// （非 relay 写入的）消息 id，保持本地顺序。
    /// - 水位 nil 或在白板里找不到（消息被清/文件重建）→ 从头算起。
    static func pendingPushIds(local: [LocalItem], pushedThroughId: String?) -> [String] {
        var tail = local[...]
        if let pushedThroughId, let idx = local.firstIndex(where: { $0.id == pushedThroughId }) {
            tail = local[local.index(after: idx)...]
        }
        return tail.filter { !$0.isRelayWritten }.map(\.id)
    }
}
