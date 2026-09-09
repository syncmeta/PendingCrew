import Foundation

/// 一次性放行票（驾驶舱计划 #75 ②）。每 crew 一个 JSON：`<dir>/<crewId>.grants.json`。
///
/// # 为什么是一次性
///
/// 人在一条 Todo 上回了句「可以」，那是**对这一次**的同意。把它变成常设权限是另一个
/// 决定，不该由一句自由文本顺手完成 —— 所以 `consume` 拿走就作废，第二次要再问。
///
/// # 为什么按 (crew, 工具) 分
///
/// 给 A 工具的同意不能被 B 工具用掉，否则一次授权就成了通行证。
///
/// 跨进程：写在 app（人回应那条 Todo 时），读在 helper 子进程（PreToolUse hook），
/// 所以跟其余账本同一个 `MultiProcessJSONStore` 基座、同一把 flock。
final class PermissionGrantStore: @unchecked Sendable {
    static let shared = PermissionGrantStore()

    private struct Row: Codable, Equatable {
        var tool: String
        var grantedAt: String
    }

    private let directory: URL

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        try? FileManager.default.createDirectory(at: self.directory, withIntermediateDirectories: true)
    }

    /// 记一张票。同一个工具重复 grant 不叠加 —— 票是「有/没有」，不是余额。
    func grant(crewId: String, tool: String) {
        withFileLock(crewId) {
            var rows = load(crewId).filter { $0.tool != tool }
            rows.append(Row(tool: tool, grantedAt: ISO8601DateFormatter().string(from: Date())))
            MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL(crewId))
        }
    }

    /// 有票就放行并**当场作废**；没票返回 false。
    func consume(crewId: String, tool: String) -> Bool {
        withFileLock(crewId) {
            let rows = load(crewId)
            guard rows.contains(where: { $0.tool == tool }) else { return false }
            MultiProcessJSONStore.saveRowsLocked(
                rows.filter { $0.tool != tool }, to: fileURL(crewId))
            return true
        }
    }

    private func fileURL(_ crewId: String) -> URL {
        directory.appendingPathComponent("\(crewId).grants.json")
    }

    private func withFileLock<T>(_ crewId: String, _ body: () -> T) -> T {
        MultiProcessJSONStore.withFileLock(
            directory.appendingPathComponent("\(crewId).grants.lock"), body)
    }

    /// 读不出来 → 空表 = **没有票**。这个方向是安全的：最坏让 agent 再问一次人，
    /// 而不是凭一次读失败替人放行。
    private func load(_ crewId: String) -> [Row] {
        MultiProcessJSONStore.loadRowsLocked(Row.self, at: fileURL(crewId), onIncident: { _ in })
    }
}
