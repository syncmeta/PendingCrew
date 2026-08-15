import Foundation

/// session 的**精简标题**收口（≤18 字、无项目名）——群聊气泡名 = 成员列表主名 =
/// 这个 title，单一真值（分诊「群聊名≠成员名 & 命名精简」）。
///
/// 两个纯函数，无副作用：
///   - `clamp`  把任意 title 收成单行、≤18 字（赋值处统一过一遍）。
///   - `derive` 没有显式 title 时，从 brief 兜底：剥【…】包裹 + 项目名前缀，取首行、clamp。
///
/// Foundation-only（编进 test bundle，跨 macOS/iOS）。
enum CrewSessionTitle {
    /// 精简标题上限（字数，按 Character 计——中文一字算一个）。
    static let maxLen = 18

    /// 已知项目代号前缀，兜底时从 brief 头部剥掉（让 title 聚焦「在干嘛」而非项目名）。
    private static let projectPrefixes = ["PendingCrew", "PendingBot", "pendingcrew", "pendingbot", "大绿豆"]

    /// 单行化 + trim + 截到 ≤`maxLen` 字。空白输入 → 空串（调用方自行兜底）。
    static func clamp(_ raw: String) -> String {
        var s = raw
        if let nl = s.firstIndex(where: \.isNewline) { s = String(s[..<nl]) }
        s = s.trimmingCharacters(in: .whitespacesAndNewlines)
        guard s.count > maxLen else { return s }
        return String(s.prefix(maxLen))
    }

    /// 从 brief 兜底出一个精简 title：取首行 → 剥一层【…】包裹（保留内文）→ 剥项目名
    /// 前缀 → clamp。用于「机长没在 start_session 传 title / 手动新建 session」两条来路。
    static func derive(fromBrief brief: String) -> String {
        var s = brief.trimmingCharacters(in: .whitespacesAndNewlines)
        if let nl = s.firstIndex(where: \.isNewline) { s = String(s[..<nl]) }
        // 剥一层【…】包裹：把内文抬出来当标题种子，其后的正文接在后面（clamp 会截断）。
        if s.hasPrefix("【"), let close = s.firstIndex(of: "】") {
            let inner = s[s.index(after: s.startIndex)..<close]
            let rest = s[s.index(after: close)...]
            s = (inner + " " + rest).trimmingCharacters(in: .whitespaces)
        }
        // 剥已知项目名前缀 + 其后的分隔标点。
        for p in projectPrefixes where s.hasPrefix(p) {
            s = String(s.dropFirst(p.count))
                .trimmingCharacters(in: CharacterSet(charactersIn: " ：:·-—、,，"))
            break
        }
        return clamp(s)
    }

    /// 统一解析：显式 title（clamp 后非空）优先，否则从 brief 兜底 derive。
    static func resolve(explicit: String?, brief: String) -> String {
        if let e = explicit {
            let c = clamp(e)
            if !c.isEmpty { return c }
        }
        return derive(fromBrief: brief)
    }
}
