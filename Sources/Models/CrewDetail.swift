import Foundation

/// `GET /v1/crews/:crewId` 详情响应。
///
/// 一个 nested struct 包住 crew + parents/children/shares/captain；
/// 一坨字段全平铺到 store 会再走回老 PendingCrew 那种 "10+ @Published
/// 字典爆炸" 的老路（见 AppModel 注释 + spec v2 §11）。Keep it
/// structured —— store 把 detail 按 crewId 存一份完整 `CrewDetail`。
struct CrewDetail: Decodable, Equatable, Hashable {
    let crew: CrewBody
    let parents: [CrewLink]
    let children: [CrewLink]
    let shares: [ResponsibilityShare]
    let captain: Captain?

    struct CrewBody: Decodable, Equatable, Hashable {
        let id: String
        let title: String
        let responsibleSubjectId: String
        let runtimeLocation: String
        let workingDirectory: String?
        let captainBotId: String?
        let status: String?
        let createdAt: String
        let updatedAt: String
        /// 机长跑哪个本机 coding agent（"claude_code" / "codex"）。**本地概念** ——
        /// `LocalCrewStore` 填值;edge 不下发 → 缺键兜底 nil（下游回落 `.codex`）。
        let captainAgentKind: String?

        var runtimeLocationKind: CrewSummary.RuntimeLocation? {
            CrewSummary.RuntimeLocation(rawValue: runtimeLocation)
        }

        /// memberwise init —— `LocalCrewStore.detail` 等本地构造点用。
        init(
            id: String,
            title: String,
            responsibleSubjectId: String,
            runtimeLocation: String,
            workingDirectory: String?,
            captainBotId: String?,
            status: String?,
            createdAt: String,
            updatedAt: String,
            captainAgentKind: String? = nil
        ) {
            self.id = id
            self.title = title
            self.responsibleSubjectId = responsibleSubjectId
            self.runtimeLocation = runtimeLocation
            self.workingDirectory = workingDirectory
            self.captainBotId = captainBotId
            self.status = status
            self.createdAt = createdAt
            self.updatedAt = updatedAt
            self.captainAgentKind = captainAgentKind
        }

        /// 显式 decoder —— edge `GET /v1/crews/:id` 不下发 `captainAgentKind`,缺键兜底 nil。
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            id = try c.decode(String.self, forKey: .id)
            title = try c.decode(String.self, forKey: .title)
            responsibleSubjectId = try c.decode(String.self, forKey: .responsibleSubjectId)
            runtimeLocation = try c.decode(String.self, forKey: .runtimeLocation)
            workingDirectory = try c.decodeIfPresent(String.self, forKey: .workingDirectory)
            captainBotId = try c.decodeIfPresent(String.self, forKey: .captainBotId)
            status = try c.decodeIfPresent(String.self, forKey: .status)
            createdAt = try c.decode(String.self, forKey: .createdAt)
            updatedAt = try c.decode(String.self, forKey: .updatedAt)
            captainAgentKind = try c.decodeIfPresent(String.self, forKey: .captainAgentKind)
        }

        private enum CodingKeys: String, CodingKey {
            case id, title, responsibleSubjectId, runtimeLocation
            case workingDirectory, captainBotId, status, createdAt, updatedAt
            case captainAgentKind
        }
    }

    struct CrewLink: Decodable, Equatable, Hashable {
        let crewId: String
        let title: String
        /// child 在这条 parent-child 边上保留的责任份额（bps，0-10000）。
        let childShareBps: Int
    }

    struct ResponsibilityShare: Decodable, Equatable, Hashable {
        let subjectId: String
        let shareBps: Int
        let isTiebreaker: Bool
        let displayName: String
        /// `subjects.subject_type` —— 一般是 `user_account` / `group_account`。
        let kind: String
    }

    struct Captain: Decodable, Equatable, Hashable {
        let botId: String
        let displayName: String
    }
}
