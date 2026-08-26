import Foundation

/// `GET /v1/crews` 列表里每行 crew 的 metadata。
///
/// 字段对齐 `apps/edge/src/routes/crews.ts` 里 `GET /v1/crews` 的响应
/// shape；新增字段时 edge 那边也要一起加。
///
/// `runtimeLocation` 是字符串而不是 enum —— spec v2 §6.2 留了 v1.1
/// 后续新增 location kind 的余地，client 端遇到没见过的值应当回落
/// 显示原始字符串，不应 decode 失败。
struct CrewSummary: Decodable, Identifiable, Equatable, Hashable {
    let id: String
    let title: String
    let responsibleSubjectId: String
    let runtimeLocation: String
    let captainBotId: String?
    let status: String?
    let createdAt: String
    let updatedAt: String
    /// 本地 crew DAG 的父边(本 crew 挂在哪些父 crew 之下)。**纯本地概念** ——
    /// `LocalCrewStore` 填实值。decode 时缺键 → 空数组(根 crew)。
    let parentCrewIds: [String]
    /// 这个 crew 挂在哪些**根 crew** 之下（名字后面那行黄字标注）。
    ///
    /// 它原本是**服务端算好下发的**那一份（`GET /v1/crews` 的 `rootCrews`），给
    /// 看不到本地 DAG 的 iPad/iPhone 用；#63 第二期删掉云端整层后**恒空**，标注
    /// 一律由本地父边算。取用仍走 `CrewRootLineage.rootTitlesByCrew`（口径是
    /// 「本地血缘优先、算不出才用这份」），别在视图里各判各的。
    let rootCrewTitles: [String]

    /// `rootCrews` 的 wire 形状（id + 名字）。只取名字用，id 留着将来点标注跳转。
    struct RootCrewRef: Decodable, Equatable, Hashable {
        let crewId: String
        let title: String
    }

    /// 机长跑哪个本机 coding agent（"claude_code" / "codex"）。**本地概念** ——
    /// `LocalCrewStore` 填值;edge 不下发 → 缺键兜底 nil（下游回落 `.codex`）。
    let captainAgentKind: String?

    /// crew 所属机器（nil = 本机/本地）。**Mac 本地路径**由 `LocalCrewStore` 透传
    /// `LocalCrew.machineId`；edge `GET /v1/crews` 缺键时兜 nil（向后兼容）。
    /// 侧栏「按机器分组」的分组键。
    let machineId: String?

    /// 机长点亮的 attention 黄点文案（非 nil = 侧栏头像右上角显黄点，值为悬浮
    /// 提示）。**纯本地概念** —— `LocalCrewStore` 透传;edge 不下发 → 缺键兜 nil。
    let attentionReason: String?

    /// **人手动**把这个 crew 从侧栏藏起来的时刻（ISO8601；nil = 没藏）。**纯本地
    /// 概念** —— `LocalCrewStore` 透传;edge 不下发 → 缺键兜 nil。
    ///
    /// 名字里的「manually」不是修辞：它把「人自己藏的」和将来可能有的「自动判定该
    /// 藏的」从第一天起就分成两件事（判定与消费全在 `CrewHiding`）。
    let manuallyHiddenAt: String?

    /// memberwise init —— `LocalCrewStore.summary` 等本地构造点用。
    init(
        id: String,
        title: String,
        responsibleSubjectId: String,
        runtimeLocation: String,
        captainBotId: String?,
        status: String?,
        createdAt: String,
        updatedAt: String,
        parentCrewIds: [String] = [],
        rootCrewTitles: [String] = [],
        captainAgentKind: String? = nil,
        machineId: String? = nil,
        attentionReason: String? = nil,
        manuallyHiddenAt: String? = nil
    ) {
        self.id = id
        self.title = title
        self.responsibleSubjectId = responsibleSubjectId
        self.runtimeLocation = runtimeLocation
        self.captainBotId = captainBotId
        self.status = status
        self.createdAt = createdAt
        self.updatedAt = updatedAt
        self.parentCrewIds = parentCrewIds
        self.rootCrewTitles = rootCrewTitles
        self.captainAgentKind = captainAgentKind
        self.machineId = machineId
        self.attentionReason = attentionReason
        self.manuallyHiddenAt = manuallyHiddenAt
    }

    /// 显式 decoder —— edge `GET /v1/crews` 不下发 `parentCrewIds` / `captainAgentKind`,缺键兜底。
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        title = try c.decode(String.self, forKey: .title)
        responsibleSubjectId = try c.decode(String.self, forKey: .responsibleSubjectId)
        runtimeLocation = try c.decode(String.self, forKey: .runtimeLocation)
        captainBotId = try c.decodeIfPresent(String.self, forKey: .captainBotId)
        status = try c.decodeIfPresent(String.self, forKey: .status)
        createdAt = try c.decode(String.self, forKey: .createdAt)
        updatedAt = try c.decode(String.self, forKey: .updatedAt)
        parentCrewIds = try c.decodeIfPresent([String].self, forKey: .parentCrewIds) ?? []
        // 老 worker 不下发 `rootCrews` → 缺键兜空数组（列表照常，只是没标注）。
        rootCrewTitles = (try c.decodeIfPresent([RootCrewRef].self, forKey: .rootCrews) ?? [])
            .map(\.title)
        captainAgentKind = try c.decodeIfPresent(String.self, forKey: .captainAgentKind)
        machineId = try c.decodeIfPresent(String.self, forKey: .machineId)
        attentionReason = try c.decodeIfPresent(String.self, forKey: .attentionReason)
        manuallyHiddenAt = try c.decodeIfPresent(String.self, forKey: .manuallyHiddenAt)
    }

    private enum CodingKeys: String, CodingKey {
        case id, title, responsibleSubjectId, runtimeLocation
        case captainBotId, status, createdAt, updatedAt, parentCrewIds
        case rootCrews
        case captainAgentKind, machineId, attentionReason
        case manuallyHiddenAt
    }

    enum RuntimeLocation: String {
        case localHost = "local_host"
        case peerDevice = "peer_device"
        case flyMachine = "fly_machine"

        var displayIcon: String {
            switch self {
            case .localHost: return "desktopcomputer"
            case .peerDevice: return "personalhotspot"
            case .flyMachine: return "cloud"
            }
        }

        var shortLabel: String {
            switch self {
            case .localHost: return "本机"
            case .peerDevice: return "peer 设备"
            case .flyMachine: return "Fly machine"
            }
        }
    }

    /// 把 wire 字符串映射成 enum；遇到未知值返回 nil（UI 上展示
    /// 原始 `runtimeLocation` 作为兜底）。
    var runtimeLocationKind: RuntimeLocation? {
        RuntimeLocation(rawValue: runtimeLocation)
    }
}
