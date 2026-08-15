#if os(macOS)
import Foundation

/// 新建 session 的模型/effort 候选清单（人面 picker 与 prompt 文档共用一份，
/// 不在 UI 里散写字符串）。
///
/// **清单现在来自 `AgentModelCatalog`（Todo #37），不再手写在这个文件里**：
/// app 的 `ModelCatalogCenter` 定时实探两家 CLI 落成 models.json，这里只是取用；
/// 探不到才回落 `AgentModelCatalog` 里带 `lastVerified` 的手工兜底表。
///
/// 为什么非改不可：旧版把 codex 候选写死成 `gpt-5-codex` / `gpt-5`，而
/// 2026-08-09 实测 codex-cli 0.145.0 的 `model/list` 里这两个**已经不在了**
/// （当代是 gpt-5.6-sol / terra / luna、gpt-5.5、gpt-5.4(-mini)）—— picker 把人
/// 一直导向已下线的档。**注意**：不在活表里 ≠ 非法（旧别名后端往往仍解析得了），
/// 所以这里只管「该推荐哪些」，校验一律走 `AgentModelValidator` 的提示口径。
///
/// **两条腿要分清**：picker/清单只作用于「显式选 model」那条腿；不选时走的是
/// 下面 `defaultModelResolution` 那条（claude 读 env/settings，codex 读
/// config.toml）。给清单不给默认腿，机长照样不知道不填会跑什么。
enum SessionLaunchOptions {
    /// `LocalCodingAgentKind` → 目录里两家表的键（"claude" / "codex"）。
    /// 表那一层是跨平台的（随 McpServer 上 iOS），不能用这个 macOS-only 的 enum。
    static func agentKey(for kind: LocalCodingAgentKind) -> String {
        switch kind {
        case .claudeCode: return "claude"
        case .codex:      return "codex"
        case .terminal:   return "terminal"
        }
    }

    /// 该 runner 当前该推荐的模型（picker 用）。`catalog` 传 app 现探的那份；
    /// 传 nil 或那一家没探到 → 回落手工兜底表。
    static func models(for kind: LocalCodingAgentKind,
                       catalog: AgentModelCatalogFile? = nil) -> [String] {
        let key = agentKey(for: kind)
        guard let table = AgentModelCatalogFile.resolveTable(agent: key, file: catalog) else {
            return []
        }
        return table.visibleModels.map(\.id)
    }

    /// 别名 → UI 友好显示名（**只标系列、不标版本号**）。传给 CLI/MCP 的仍是裸
    /// 别名，只有人面展示走这里。刻意不带版本号：别名本就交给 CLI 解析到当代最新
    /// （`/model opus` 同款语义），UI 硬编「4.8」这类数字会过时，系列名则永不漂移。
    /// 表里探到 displayName 的（codex 侧有）优先用表里的；都没有则原样返回。
    static func displayName(for model: String, catalog: AgentModelCatalogFile? = nil) -> String {
        switch model {
        case "fable":  return "Fable"
        case "opus":   return "Opus"
        case "sonnet": return "Sonnet"
        case "haiku":  return "Haiku"
        default: break
        }
        for key in ["claude", "codex"] {
            if let table = AgentModelCatalogFile.resolveTable(agent: key, file: catalog),
               let entry = table.models.first(where: { $0.id.lowercased() == model.lowercased() }),
               let name = entry.displayName, !name.isEmpty {
                return name
            }
        }
        return model
    }

    /// 该 runner 的 effort 档。同样来自表 —— 旧版把 codex 写死成
    /// `minimal/low/medium/high`，而实测各模型支持到 xhigh/max/ultra，且**逐模型不同**
    /// （gpt-5.5 就没有 max/ultra）。要逐模型精确判定请用
    /// `AgentModelTable.knowsEffort(_:forModel:)`，这里给的是该家的并集。
    static func efforts(for kind: LocalCodingAgentKind,
                        catalog: AgentModelCatalogFile? = nil) -> [String] {
        let key = agentKey(for: kind)
        guard let table = AgentModelCatalogFile.resolveTable(agent: key, file: catalog) else {
            return []
        }
        return table.efforts
    }

    /// 当调用方**没显式选 model** 时，解析出一个具体别名显式落到启动配置里 ——
    /// 让 argv 永远带 `--model`、`run.model` 永不为 nil，UI 显示即真实跑的模型
    /// （用户定调：不许再糊「默认」，#489）。
    ///
    /// **只做主链，不复刻 claude 全部解析规则**（drift 风险已记 tech-debt 🟡）：
    /// - **claude**：`ANTHROPIC_MODEL` env → 项目 `.claude/settings.local.json` →
    ///   项目 `.claude/settings.json` → 用户 `~/.claude/settings.json` 的 `model` 字段。
    ///   都没有 → 兜底当代默认 `sonnet` + fail-loud 日志（绝不返回 nil / 糊「默认」）。
    /// - **codex**：`~/.codex/config.toml` 顶层 `model = "..."` 一眼可读则用；读不到就
    ///   返回 nil（保持 Codex app-server 自身默认；显示端会退「默认」，复杂 TOML 解析留 tech-debt 尾巴）。
    ///
    /// - Parameter projectDir: session 的工作目录（claude 会在此找项目级 settings）。
    static func defaultModel(for kind: LocalCodingAgentKind, projectDir: URL?) -> String? {
        defaultModelResolution(for: kind, projectDir: projectDir).value
    }

    /// 默认那条腿的解析结果 —— **值 + 它是从哪读出来的**。
    ///
    /// 光有值不够：机长看到「默认跑 gpt-5.6-sol」也不知道该去哪儿改、该不该信。
    /// 把来源一并带出来，注入模型表时才能说清「你不选 model 时会跑什么、凭什么」。
    struct DefaultModelResolution: Equatable {
        /// 解析出的模型值；nil = 真没解析出（照实留白，别猜）。
        let value: String?
        /// 人话来源，如「~/.codex/config.toml 顶层 model」。
        let source: String
    }

    static func defaultModelResolution(for kind: LocalCodingAgentKind,
                                       projectDir: URL?) -> DefaultModelResolution {
        switch kind {
        case .claudeCode:
            if let env = ProcessInfo.processInfo.environment["ANTHROPIC_MODEL"],
               !env.isEmpty {
                return DefaultModelResolution(value: env, source: "ANTHROPIC_MODEL 环境变量")
            }
            let home = FileManager.default.homeDirectoryForCurrentUser
            var candidates: [(URL, String)] = []
            if let dir = projectDir {
                candidates.append((dir.appendingPathComponent(".claude/settings.local.json"),
                                   "项目 .claude/settings.local.json 的 model"))
                candidates.append((dir.appendingPathComponent(".claude/settings.json"),
                                   "项目 .claude/settings.json 的 model"))
            }
            candidates.append((home.appendingPathComponent(".claude/settings.json"),
                               "~/.claude/settings.json 的 model"))
            for (url, label) in candidates {
                if let m = jsonStringField("model", at: url), !m.isEmpty {
                    return DefaultModelResolution(value: m, source: label)
                }
            }
            // fail-loud：真没解析出用户默认 —— 落一个当代默认，别静默糊「默认」。
            FileHandle.standardError.write(Data(
                "⚠️ [SessionLaunch] claude 默认模型未从 env/settings.json 解析出，兜底 sonnet（显示=实际）。\n".utf8))
            return DefaultModelResolution(
                value: "sonnet", source: "env/settings 都没写，PendingCrew 兜底成 sonnet")
        case .codex:
            let home = FileManager.default.homeDirectoryForCurrentUser
            let toml = home.appendingPathComponent(".codex/config.toml")
            if let m = tomlTopLevelString("model", at: toml), !m.isEmpty {
                return DefaultModelResolution(value: m, source: "~/.codex/config.toml 顶层 model")
            }
            // 解析不出就照实留白 —— codex app-server 自己会挑默认（`model/list` 里
            // `isDefault` 那个），我们不冒充知道是哪一个。
            return DefaultModelResolution(
                value: nil, source: "~/.codex/config.toml 没写顶层 model，交给 codex app-server 自己的默认")
        case .terminal:
            return DefaultModelResolution(value: nil, source: "纯终端没有模型")
        }
    }

    /// 读 JSON 文件顶层某个字符串字段（best-effort，任何失败返回 nil）。
    private static func jsonStringField(_ key: String, at url: URL) -> String? {
        guard let data = try? Data(contentsOf: url),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        return obj[key] as? String
    }

    /// 从 TOML 里读**顶层**（第一个 `[section]` 之前）的 `key = "..."`。只认一眼可读的
    /// 简单形式，不做完整 TOML 解析（codex 复杂配置留 tech-debt 尾巴）。
    private static func tomlTopLevelString(_ key: String, at url: URL) -> String? {
        guard let text = try? String(contentsOf: url, encoding: .utf8) else { return nil }
        for rawLine in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("#") { continue }
            if line.hasPrefix("[") { break } // 进入某个 section，顶层结束
            guard let eq = line.firstIndex(of: "=") else { continue }
            let name = line[line.startIndex..<eq].trimmingCharacters(in: .whitespaces)
            guard name == key else { continue }
            var value = line[line.index(after: eq)...].trimmingCharacters(in: .whitespaces)
            if let hash = value.firstIndex(of: "#") { // 行尾注释
                value = String(value[value.startIndex..<hash]).trimmingCharacters(in: .whitespaces)
            }
            value = value.trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))
            return value.isEmpty ? nil : value
        }
        return nil
    }
}
#endif
