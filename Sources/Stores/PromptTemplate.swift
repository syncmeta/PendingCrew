import Foundation

/// 极简 `{{var}}` 模板替换（对齐 edge `applySubstitutions`：逐键字面替换）。
///
/// 纯函数、无 IO —— 可单独编进 PendingCrewTests bundle 单测（脚手架期 test 不链
/// app module，见 project.yml；同 LocalWhiteboardStore 模式）。文件加载在
/// `LocalPromptLoader`。
enum PromptTemplate {
    /// 把 `template` 里的 `{{key}}` 全部替换成 `vars[key]`。
    /// 未提供值的残留 `{{...}}` 槽 → 清成空串（绝不把模板语法泄漏给 agent，
    /// 对齐 edge renderer "绝不输出 {{var}} 字面值" 的约束）。
    static func render(_ template: String, vars: [String: String]) -> String {
        var out = template
        for (k, v) in vars {
            out = out.replacingOccurrences(of: "{{\(k)}}", with: v)
        }
        return stripUnfilledPlaceholders(out)
    }

    /// 清掉残留 `{{ 任意非括号字符 }}` 槽。
    static func stripUnfilledPlaceholders(_ s: String) -> String {
        guard let re = try? NSRegularExpression(pattern: "\\{\\{[^{}]*\\}\\}") else { return s }
        let range = NSRange(s.startIndex..., in: s)
        return re.stringByReplacingMatches(in: s, range: range, withTemplate: "")
    }
}
