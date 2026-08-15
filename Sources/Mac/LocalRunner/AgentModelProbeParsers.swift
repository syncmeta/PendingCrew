// 纯 Foundation、不带平台门 —— 只有解析（可单测）；起子进程的那半在
// `ModelCatalogCenter`（macOS-only）。
import Foundation

/// claude 侧的「列出可用模型」通道解析。
///
/// **实测通道（2026-08-09，claude 2.1.226）**：`claude -p "/model" --output-format json`
/// —— 斜杠命令在 print 模式下由 CLI 本地处理，`num_turns: 0` / `total_cost_usd: 0`，
/// **不发 API 请求、不烧额度**。返回 JSON 的 `result` 字段是：
/// ```
/// Current model: Opus 5 (effort: high)
/// Usage: /model <name>. Available: sonnet, opus, haiku, fable, best, sonnet[1m], opus[1m], fable[1m], opusplan, default, or a full model ID.
/// ```
/// effort 同法：`claude -p "/effort"` → `Usage: /effort <low|medium|high|xhigh|max|ultracode|auto>`。
///
/// 别的通道都试过、都不行（写下来省得后人重探）：
/// - `claude --help` 的子命令表里**没有** `models` / `config` 之类能列模型的命令；
///   `--model` 的帮助文本只举例 `fable/opus/sonnet`，不是完整清单。
/// - `claude --model <不存在的值> -p hi` 只报「不是本版本认识的模型」，**不列**可用值。
enum ClaudeModelProbeParser {
    /// 从 `--output-format json` 的整段 stdout 里取出 `result` 字段。
    static func resultField(fromJSON stdout: String) -> String? {
        guard let data = stdout.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let result = obj["result"] as? String
        else { return nil }
        return result
    }

    /// 从 `/model` 回显里抠出别名清单。抠不出 → nil（**不猜**，让调用方回落兜底表）。
    ///
    /// 清单尾巴是 `…, default, or a full model ID.` —— 「or a full model ID」不是一个
    /// 模型别名，靠「合法别名不含空格」这条把它连同任何未来的同类尾注一起滤掉。
    static func parseModels(_ text: String) -> [String]? {
        guard let range = text.range(of: "Available:") else { return nil }
        var tail = String(text[range.upperBound...])
        if let nl = tail.firstIndex(where: \.isNewline) { tail = String(tail[..<nl]) }
        let items = tail.split(separator: ",").compactMap { raw -> String? in
            var s = raw.trimmingCharacters(in: .whitespaces)
            while s.hasSuffix(".") { s.removeLast() }
            s = s.trimmingCharacters(in: .whitespaces)
            // 合法别名/slug 不含空格；「or a full model ID」这类尾注就是这么滤掉的。
            guard !s.isEmpty, !s.contains(" ") else { return nil }
            return s
        }
        return items.isEmpty ? nil : items
    }

    /// 从 `/effort` 回显 `Usage: /effort <a|b|c>` 里抠出档位。抠不出 → nil。
    static func parseEfforts(_ text: String) -> [String]? {
        guard let open = text.firstIndex(of: "<"),
              let close = text[open...].firstIndex(of: ">"), open < close
        else { return nil }
        let items = text[text.index(after: open)..<close]
            .split(separator: "|")
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty && !$0.contains(" ") }
        return items.isEmpty ? nil : items
    }

    /// **启动参数** `--effort` 认的那套 —— 与上面 `/effort` 那套**不是一回事**。
    ///
    /// 探法（2026-08-09 实测）：`claude --effort <随便一个不存在的值> -p "/model"
    /// --output-format json`，**stderr** 会吐
    /// `Warning: Unknown --effort value 'X' — ignoring it and using the default effort.
    /// Valid values: low, medium, high, xhigh, max.`
    /// 这句里的 `Valid values:` 就是启动态的权威清单。同样 `num_turns: 0` 不烧额度。
    ///
    /// 注意那句 warning 本身：CLI **忽略你的值、用默认档继续跑**，不报错不退出 ——
    /// 这正是人类在 Todo #36 里说的「静默降级」。
    static func parseLaunchEfforts(fromWarning text: String) -> [String]? {
        guard let range = text.range(of: "Valid values:") else { return nil }
        var tail = String(text[range.upperBound...])
        if let nl = tail.firstIndex(where: \.isNewline) { tail = String(tail[..<nl]) }
        let items = tail.split(separator: ",").compactMap { raw -> String? in
            var s = raw.trimmingCharacters(in: .whitespaces)
            while s.hasSuffix(".") { s.removeLast() }
            s = s.trimmingCharacters(in: .whitespaces)
            guard !s.isEmpty, !s.contains(" ") else { return nil }
            return s
        }
        return items.isEmpty ? nil : items
    }

    /// 三段回显 → 一张现探表。前两段任一解不出就返回 nil（**半张表比没有表更危险**：
    /// 一张只有模型没有 effort 的表会把所有 effort 值判成「不存在」）。
    ///
    /// `launchEffortWarning` 解不出**不致命**：`launchEfforts` 留空 → 对照回落到
    /// 运行时那套（等于恢复成本次改动之前的行为，只是少了那层保护），不至于把整张
    /// 表废掉。
    static func table(modelEcho: String, effortEcho: String,
                      launchEffortWarning: String? = nil,
                      probedAt: Date) -> AgentModelTable? {
        guard let ids = parseModels(modelEcho), let efforts = parseEfforts(effortEcho) else {
            return nil
        }
        // `x[1m]` 是长上下文变体，合法但不该占满 picker → 归到 hidden。
        let models = ids.map { AgentModel(id: $0, hidden: $0.hasSuffix("[1m]")) }
        let launch = launchEffortWarning.flatMap(parseLaunchEfforts(fromWarning:)) ?? []
        // 「运行时那套里有、启动帮助里没写、但实测启动也收」的值。目前只有
        // `ultracode` 一个，写死在 `AgentModelCatalog.claudeUndocumentedLaunchEfforts`
        // （逐个实测确认过；探测通道给不出这个信息，只能人工维护）。
        let undocumented = launch.isEmpty ? [] :
            AgentModelCatalog.claudeUndocumentedLaunchEfforts.filter { u in
                efforts.contains { $0.lowercased() == u.lowercased() }
                    && !launch.contains { $0.lowercased() == u.lowercased() }
            }
        return AgentModelTable(agent: "claude", source: .probe,
                               probedAt: ISO8601DateFormatter().string(from: probedAt),
                               models: models, efforts: efforts,
                               launchEfforts: launch, undocumentedLaunchEfforts: undocumented)
    }
}

/// codex 侧的「列出可用模型」通道解析。
///
/// **实测通道（2026-08-09，codex-cli 0.145.0）**：app-server JSON-RPC 的
/// `model/list`（在 `codex app-server generate-json-schema --experimental` 导出的
/// `ClientRequest.json` 里能查到；参数 `{cursor?, includeHidden?, limit?}`，
/// 响应 `{data: [Model], nextCursor?}`）。跟 `account/rateLimits/read` 一样是
/// 本地控制面查询，不开 thread、不发 turn，**不烧额度**。
///
/// 每个 Model 自带 `supportedReasoningEfforts` —— codex 的 effort 档**逐模型不同**
/// （gpt-5.5 没有 max/ultra），所以表里逐模型存一份，别只留并集。
enum CodexModelProbeParser {
    /// `model/list` 的 `result` → 一张现探表。形状对不上 / 空清单 → nil。
    static func table(result: Any?, probedAt: Date) -> AgentModelTable? {
        guard let obj = result as? [String: Any],
              let rows = obj["data"] as? [[String: Any]], !rows.isEmpty
        else { return nil }
        var models: [AgentModel] = []
        for row in rows {
            guard let id = row["id"] as? String ?? row["model"] as? String, !id.isEmpty else {
                continue
            }
            let efforts = (row["supportedReasoningEfforts"] as? [[String: Any]])?
                .compactMap { $0["reasoningEffort"] as? String } ?? []
            models.append(AgentModel(
                id: id,
                displayName: row["displayName"] as? String,
                summary: row["description"] as? String,
                efforts: efforts,
                isDefault: (row["isDefault"] as? Bool) ?? false,
                hidden: (row["hidden"] as? Bool) ?? false))
        }
        guard !models.isEmpty else { return nil }
        // 表级 efforts = 各模型档位的并集，保持模型表里出现的先后次序（低→高）。
        var union: [String] = []
        for m in models where !m.efforts.isEmpty {
            for e in m.efforts where !union.contains(e) { union.append(e) }
        }
        return AgentModelTable(agent: "codex", source: .probe,
                               probedAt: ISO8601DateFormatter().string(from: probedAt),
                               models: models, efforts: union)
    }
}
