#if os(macOS)
import Foundation

/// 新建 session 的模型/effort 候选清单（人面 picker 与 prompt 文档共用一份，
/// 不在 UI 里散写字符串）。
///
/// **清单现在来自 `AgentModelCatalog`（Todo #37），不再手写在这个文件里**：
/// app 的 `ModelCatalogCenter` 定时实探两家 CLI 落成 models.json。手工兜底表仍可供
/// helper 的非否决性诊断使用，但**不能**成为 picker 的可选项；探测失败或过期时，
/// picker 留空并把不可验证原因交给 UI 显示。
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
    /// 运行中模型菜单传给编排层的内部值。它不是 Codex model slug；runner 收到后
    /// 会解析当前默认的真实 slug、切换 live thread，再清掉持久 model 覆盖。
    static let codexDefaultModelSelection = "__pendingcrew_codex_default__"

    /// 供 UI 呈现模型/effort picker 的异步快照结果。`values` 只有当前、无错误的
    /// app-server / CLI 现探表才会非空；其它情形一律带 `unavailableReason`，避免把
    /// 缓存、陈旧或手工表冒充当前可选值。
    struct PickerOptions: Equatable {
        let values: [String]
        let unavailableReason: String?

        var isAvailable: Bool { unavailableReason == nil }
    }

    /// `LocalCodingAgentKind` → 目录里两家表的键（"claude" / "codex"）。
    /// 表那一层是跨平台的（随 McpServer 上 iOS），不能用这个 macOS-only 的 enum。
    static func agentKey(for kind: LocalCodingAgentKind) -> String {
        switch kind {
        case .claudeCode: return "claude"
        case .codex:      return "codex"
        case .terminal:   return "terminal"
        }
    }

    /// 该 runner 当前可显式选择的模型（兼容只消费数组的旧调用方）。无法核实的
    /// 快照返回空；UI 若要显示原因，改用 `modelPickerOptions`。
    static func models(for kind: LocalCodingAgentKind,
                       catalog: AgentModelCatalogFile? = nil) -> [String] {
        modelPickerOptions(for: kind, catalog: catalog).values
    }

    /// 模型 picker 的值与状态。它只读 `ModelCatalogCenter` 已有的异步快照，绝不在
    /// 视图求值或主线程上自行探 CLI。
    static func modelPickerOptions(
        for kind: LocalCodingAgentKind,
        catalog: AgentModelCatalogFile? = nil,
        now: Date = Date()
    ) -> PickerOptions {
        let selection = livePickerTable(for: kind, catalog: catalog, now: now)
        guard let table = selection.table else {
            return PickerOptions(values: [], unavailableReason: selection.unavailableReason)
        }
        return PickerOptions(values: table.visibleModels.map(\.id), unavailableReason: nil)
    }

    /// 供只拿到了**已经原生解析过的**值的调用方显示。`model/list.isDefault` 是服务
    /// 目录的推荐值，不能代替某个 session cwd 的 config/profile 解析结果，更不能拿来
    /// 固定运行中 thread 的 slug。
    static func codexDefaultModel(
        configuredModel: String?, catalog: AgentModelCatalogFile?
    ) -> String? {
        if let configuredModel = configuredModel?.trimmingCharacters(in: .whitespacesAndNewlines),
           !configuredModel.isEmpty {
            return configuredModel
        }
        return nil
    }

    /// 模型菜单没有 session cwd，不能假装解析出 Codex 默认；运行中会单独向
    /// app-server 请求该 session 的原生解析值。这里留空比错标一个 slug 诚实。
    static func codexDefaultModel(catalog _: AgentModelCatalogFile?) -> String? {
        nil
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
        let selection = livePickerTable(for: kind, catalog: catalog, now: Date())
        return selection.table?.efforts ?? []
    }

    /// 某个当前/目标模型的 effort picker 值与状态。Codex 各模型支持的 effort 不同；
    /// 调用方必须传入将要显示或切换到的 model，不能拿全表并集给人选择。
    static func effortPickerOptions(
        for kind: LocalCodingAgentKind,
        model: String?,
        catalog: AgentModelCatalogFile? = nil,
        now: Date = Date()
    ) -> PickerOptions {
        let selection = livePickerTable(for: kind, catalog: catalog, now: now)
        guard let table = selection.table else {
            return PickerOptions(values: [], unavailableReason: selection.unavailableReason)
        }
        guard let model = model?.trimmingCharacters(in: .whitespacesAndNewlines), !model.isEmpty else {
            return PickerOptions(values: [], unavailableReason: "尚未取得当前有效模型，无法确认可选 effort")
        }
        guard let entry = table.models.first(where: { $0.id.caseInsensitiveCompare(model) == .orderedSame }) else {
            return PickerOptions(values: [],
                                 unavailableReason: "当前有效模型不在最新模型清单中，无法确认可选 effort")
        }
        return PickerOptions(values: entry.efforts, unavailableReason: nil)
    }

    /// Picker 专用的可信表：不是参数白名单，不能影响 helper 对显式旧别名的处理。
    /// 它只负责避免 UI 将不可靠的推荐清单说成「当前可选」。
    private static func livePickerTable(
        for kind: LocalCodingAgentKind,
        catalog: AgentModelCatalogFile?,
        now: Date
    ) -> (table: AgentModelTable?, unavailableReason: String?) {
        let key = agentKey(for: kind)
        guard key == "claude" || key == "codex" else {
            return (nil, "此 runner 没有模型清单")
        }
        guard let catalog else {
            return (nil, "尚无模型探测快照，无法确认当前可选模型")
        }
        if let error = catalog.error(agent: key)?.trimmingCharacters(in: .whitespacesAndNewlines),
           !error.isEmpty {
            return (nil, "模型清单暂不可验证：\(error)")
        }
        guard let table = catalog.table(agent: key) else {
            return (nil, "本轮 app-server 模型探测没有返回结果")
        }
        guard table.agent == key else {
            return (nil, "模型清单来源不匹配，无法确认当前可选模型")
        }
        guard AgentModelCatalog.isLiveProbe(table, now: now) else {
            let note = AgentModelCatalog.stalenessNote(for: table, now: now)
                ?? "模型清单不是当前 app-server 探测结果"
            return (nil, "模型清单暂不可验证：\(note)")
        }
        return (table, nil)
    }

    /// 当调用方**没显式选 model** 时，解析出一个具体别名显式落到启动配置里 ——
    /// 让 argv 永远带 `--model`、`run.model` 永不为 nil，UI 显示即真实跑的模型
    /// （用户定调：不许再糊「默认」，#489）。
    ///
    /// **只做主链，不复刻 claude 全部解析规则**（drift 风险已记 tech-debt 🟡）：
    /// - **两家先看 PendingCrew 编码工具设置**；未设置时沿用下面各自的规则。
    /// - **claude**：`ANTHROPIC_MODEL` env → 项目 `.claude/settings.local.json` →
    ///   项目 `.claude/settings.json` → 用户 `~/.claude/settings.json` 的 `model` 字段。
    ///   都没有 → 兜底当代默认 `sonnet` + fail-loud 日志（绝不返回 nil / 糊「默认」）。
    /// - **codex**：不在 PendingCrew 重算。新 session 不传 model，由 app-server 按 cwd
    ///   和完整原生配置优先级解析，再从回包回填显示。
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
        if let override = AgentLaunchPreferences.model(for: kind) {
            return DefaultModelResolution(value: override, source: "PendingCrew 编码工具设置的默认模型")
        }
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
            return DefaultModelResolution(
                value: nil, source: "由 Codex app-server 按 session 工作目录和原生配置优先级解析")
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

}
#endif
