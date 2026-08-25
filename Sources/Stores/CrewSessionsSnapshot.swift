// 纯 Foundation、无平台门 —— app 侧 `CrewSessionRunner` 写、helper 侧
// `list_sessions` 工具读（离线 helper 唯一能碰的共享文件层）。
import Foundation

/// crew-sessions.json 的文件形状：按 crewId 分组的成员实时状态快照。
/// run 状态本体只在 app 内存（`sessionRunner.runs`），机长的 helper 是离线
/// 子进程看不到 —— 快照文件是给机长「点名」用的镜像（quota.json 同款模式）。
struct CrewSessionsSnapshot: Codable, Equatable {
    struct Entry: Codable, Equatable {
        let sessionId: String
        let name: String          // displayName（「机长」/「Claude Code · ab12cd」）
        let role: String          // "captain" | "worker"
        let brief: String         // 任务简述（captain 为空串）
        /// "working"(在跑回合) | "idle"(存活待命) | "awaitingDecision"(卡在待决策上
        /// 等人拍板; Todo #6) | "awaitingReply"(在等人回话: ask 挂着 / 说完停在一个
        /// 问句上; 人类 Todo #25 层 2) | "rateLimited"(撞限额等重置, 进程活着; Todo #10) |
        /// "error"(其他健康异常,进程活着) | "launchFailed"(压根没拉起来/起来即死/
        /// 起来了但零输出; #541) | "exited"(已退出, 仍在切换条上)
        ///
        /// **"idle" 的语义是「起来了、在等活」** —— 拉起失败绝不能落进这一档
        /// （#541 事故：进程从未跑起来却报空闲，机长照常派活，活石沉大海）。
        /// 同理「卡在菜单上等人选」「问完一句停住」也不能落进 idle：它们都不吐输出，
        /// 天然长得像空闲。
        let state: String
        /// state=="error"/"rateLimited"/"launchFailed" 时的人话说明；
        /// state=="awaitingDecision"/"awaitingReply" 时是「在等什么」。
        var healthDetail: String? = nil
    }

    /// crewId → 该 crew 的 session 条目（含 exited,直到被人从切换条移除）。
    var crews: [String: [Entry]] = [:]
    var updatedAt: String = ""

    static let fileName = "crew-sessions.json"

    /// 花名册：`sessionId → 显示名`。给注入面消歧标注
    /// （`CrewWhiteboardVisibility.directedNote`）解名字用 —— 「（发给 小王 的）」
    /// 比「（发给 session:9f2a1c 的）」有用得多。
    ///
    /// 跨进程从快照文件现读（helper 子进程也读得到），且**故意不做新鲜度门禁**：
    /// 名字过期一点无所谓，解不出名字才是真损失。读不到 → 空表，调用方自然退回
    /// 短 id 兜底。（对比 `HookEmitter.activeSessionCount`：那里的 15 秒门禁是因为
    /// 陈旧 roster 会制造「当前并行 N 个」的假信号，名字没有这个问题。）
    static func displayNames(ofCrew crewId: String, directory: URL) -> [String: String] {
        let url = directory.appendingPathComponent(fileName)
        guard let data = try? Data(contentsOf: url),
              let snapshot = try? JSONDecoder().decode(CrewSessionsSnapshot.self, from: data)
        else { return [:] }
        var out: [String: String] = [:]
        for e in snapshot.crews[crewId] ?? [] where !e.name.isEmpty {
            out[e.sessionId] = e.name
        }
        return out
    }

    /// 机长 `list_sessions` 的渲染：一行一个成员,直接可读。
    func renderRoster(crewId: String) -> String {
        guard let entries = crews[crewId], !entries.isEmpty else {
            return "本 crew 当前没有登记在案的 session（还没起过,或都已被移除）。"
        }
        let lines = entries.map { e -> String in
            let stateLabel: String
            switch e.state {
            case "working": stateLabel = "🟢 干活中"
            case "idle":    stateLabel = "🟡 空闲"
            case "awaitingDecision":
                stateLabel = "⌛ 等人拍板" + (e.healthDetail.map { "（\($0)）" } ?? "")
                    + " —— 它卡在这里动不了，你能拍就 inspect_session 看现场 + nudge_session 代答，拍不了就 @人"
            case "awaitingReply":
                stateLabel = "⌛ 等回复" + (e.healthDetail.map { "（\($0)）" } ?? "")
                    + " —— 它在等人回话，你答得了就 nudge_session 回它一句，答不了就 @人"
            case "rateLimited": stateLabel = "⏳ 限额中" + (e.healthDetail.map { "（\($0)）" } ?? "")
            case "error":   stateLabel = "🔴 异常" + (e.healthDetail.map { "（\($0)）" } ?? "")
            case "launchFailed":
                stateLabel = "💀 拉起失败" + (e.healthDetail.map { "（\($0)）" } ?? "")
                    + " —— 这个 session 没跑起来，派给它的活等于没派，请改派"
            default:        stateLabel = "⚪ 已退出"
            }
            let briefPart = e.brief.isEmpty ? "" : " — \(e.brief)"
            return "- \(e.name) [\(e.role == "captain" ? "机长" : "worker")] \(stateLabel)\(briefPart) (session_id: \(e.sessionId))"
        }
        return lines.joined(separator: "\n") + "\n（快照时间 \(updatedAt)）"
    }
}
