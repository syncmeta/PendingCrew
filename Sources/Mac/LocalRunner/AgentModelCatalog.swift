// 纯 Foundation、不带平台门 —— McpServer（跨平台编译）的 start_session /
// set_session_profile 要用这里的表与判定；真正 macOS-only 的是 ModelCatalogCenter
// （起子进程实探），不在本文件。
import Foundation

/// 一张模型表是**探来的**还是**手抄的**。这个区分决定提示的措辞强度
/// （现探的表说得肯定些，手抄的表得先声明自己可能过时），**但两者都不构成否决权**
/// —— 见 `AgentModelCheck` 头部那条硬纪律。
enum AgentModelTableSource: String, Codable, Equatable {
    /// 从 CLI 现探（claude `-p "/model"` 回显 / codex app-server `model/list`）。
    case probe
    /// 仓库里手工维护的兜底表（探测通道失灵时用），带 `lastVerified` 日期。
    case manual
}

/// 表里的一个模型。`efforts` 为空表示「该模型没有自己的档位清单」，用表级 `efforts`。
struct AgentModel: Codable, Equatable {
    /// 传给 CLI / app-server 的**裸值**（claude 别名如 `opus`；codex slug 如 `gpt-5.6-sol`）。
    let id: String
    /// 人面显示名（codex 的 `displayName`；claude 的 `/model` 回显只给别名 → nil）。
    var displayName: String?
    /// 一句话说明（codex 的 `description`）。claude 侧探不到 → nil。
    var summary: String?
    /// 该模型支持的 thinking effort 档（codex 逐模型给；claude 全局一套 → 空）。
    var efforts: [String]
    /// 该 runner 不指定 model 时的默认（codex `isDefault`）。
    var isDefault: Bool
    /// 默认 picker 里藏起来的（codex `hidden`）—— 仍算合法值，只是不主动推荐。
    var hidden: Bool

    init(id: String, displayName: String? = nil, summary: String? = nil,
         efforts: [String] = [], isDefault: Bool = false, hidden: Bool = false) {
        self.id = id
        self.displayName = displayName
        self.summary = summary
        self.efforts = efforts
        self.isDefault = isDefault
        self.hidden = hidden
    }
}

/// 一家 agent 的可用模型表。
struct AgentModelTable: Codable, Equatable {
    /// "claude" | "codex"（与 `AgentQuotaFile` 的两家键一致；这里不能用
    /// `LocalCodingAgentKind`——那个是 macOS-only，而本文件要随 McpServer 上 iOS）。
    let agent: String
    let source: AgentModelTableSource
    /// 现探时刻（ISO8601）。`source == .probe` 才有。
    var probedAt: String?
    /// 手工表最后一次人工核实的日期（`yyyy-MM-dd`）。`source == .manual` 才有。
    var lastVerified: String?
    var models: [AgentModel]
    /// **运行时**可切的 effort 档（claude = `/effort` 斜杠命令认的那套；codex = 各
    /// 模型档位的并集）。切 session 配置（`set_session_profile`）按这套判。
    var efforts: [String]
    /// **启动参数**认的 effort 档（claude = `--effort` 认的那套）。空 = 与 `efforts` 同。
    ///
    /// 为什么必须分两套（2026-08-09 实测，血的教训）：claude 的 `/effort` 认
    /// `low|medium|high|xhigh|max|ultracode|auto`，而启动参数 `--effort` **只认前五个**
    /// —— 传 `auto` 会被 **静默降级**：`Warning: Unknown --effort value 'auto' —
    /// ignoring it and using the default effort.` 一份合并的清单会让机长照着运行时那套
    /// 去 `start_session`，结果配置根本没生效而没人知道。codex 两边同一套，留空即可。
    var launchEfforts: [String]
    /// 「实测收、但 CLI 帮助里没写」的启动 effort 值（如 claude 的 `ultracode`）。
    /// 未公开 = 随时可能变，注入时要标不确定，不能当稳定契约推荐。
    var undocumentedLaunchEfforts: [String]
    /// **不显式选 model 时实际会跑的那个值**。picker/清单只管「显式选」那条腿，
    /// 不选时走的是另一条腿（`SessionLaunchOptions.defaultModel`：codex 读
    /// `~/.codex/config.toml`，claude 读 env/settings）。只给清单不给这个，机长
    /// 照样不知道「我不填 model 会跑什么」。nil = 解析不出（照实留白，不猜）。
    var resolvedDefault: String?
    /// 上面那个值**从哪读出来的**（人话，如「~/.codex/config.toml 顶层 model」）。
    var resolvedDefaultSource: String?

    init(agent: String, source: AgentModelTableSource, probedAt: String? = nil,
         lastVerified: String? = nil, models: [AgentModel], efforts: [String],
         launchEfforts: [String] = [], undocumentedLaunchEfforts: [String] = [],
         resolvedDefault: String? = nil, resolvedDefaultSource: String? = nil) {
        self.agent = agent
        self.source = source
        self.probedAt = probedAt
        self.lastVerified = lastVerified
        self.models = models
        self.efforts = efforts
        self.launchEfforts = launchEfforts
        self.undocumentedLaunchEfforts = undocumentedLaunchEfforts
        self.resolvedDefault = resolvedDefault
        self.resolvedDefaultSource = resolvedDefaultSource
    }

    /// 旧 models.json 没有这两个字段 —— 解码时补空，别让整份表解不出来退回兜底。
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        agent = try c.decode(String.self, forKey: .agent)
        source = try c.decode(AgentModelTableSource.self, forKey: .source)
        probedAt = try c.decodeIfPresent(String.self, forKey: .probedAt)
        lastVerified = try c.decodeIfPresent(String.self, forKey: .lastVerified)
        models = try c.decode([AgentModel].self, forKey: .models)
        efforts = try c.decode([String].self, forKey: .efforts)
        launchEfforts = try c.decodeIfPresent([String].self, forKey: .launchEfforts) ?? []
        undocumentedLaunchEfforts =
            try c.decodeIfPresent([String].self, forKey: .undocumentedLaunchEfforts) ?? []
        resolvedDefault = try c.decodeIfPresent(String.self, forKey: .resolvedDefault)
        resolvedDefaultSource = try c.decodeIfPresent(String.self, forKey: .resolvedDefaultSource)
    }

    /// 启动参数那套（空 → 与运行时同一套）。**别直接读 `launchEfforts`**，
    /// 空数组会被误当成「一个都不认」。
    var effectiveLaunchEfforts: [String] {
        launchEfforts.isEmpty ? efforts : launchEfforts
    }

    /// 只能运行时切、**不能拿去起 session** 的档。
    ///
    /// = 运行时那套 − 启动那套 − 未公开但实测收的那些。最后一项必须减掉，否则
    /// `ultracode` 会同时出现在「只能运行时切」和「实测起 session 也收」两句里，
    /// 自相矛盾。
    var runtimeOnlyEfforts: [String] {
        guard !launchEfforts.isEmpty else { return [] }
        var accepted = Set(launchEfforts.map { $0.lowercased() })
        accepted.formUnion(undocumentedLaunchEfforts.map { $0.lowercased() })
        return efforts.filter { !accepted.contains($0.lowercased()) }
    }

    /// 默认 picker 里该露出来的（非 hidden）。
    var visibleModels: [AgentModel] { models.filter { !$0.hidden } }

    /// 这个值在表里吗（大小写不敏感 —— claude 别名与 codex slug 都是小写惯例）。
    func knowsModel(_ value: String) -> Bool {
        let v = value.lowercased()
        return models.contains { $0.id.lowercased() == v }
    }

    /// 某个 effort 档在表里吗。
    /// - `phase`：`.launch` 用启动参数那套（claude 的 `--effort`），`.runtime` 用
    ///   斜杠命令那套（claude 的 `/effort`）。两套在 claude 上**不一样**，混用会
    ///   让 `auto` 这类值被静默降级 —— 见 `launchEfforts` 的注释。
    /// - 给了 model 且该 model 有自己的档位清单时**按那个模型的清单判**
    ///   （codex 的 gpt-5.5 就没有 `max` / `ultra`）；否则退表级清单。
    func knowsEffort(_ value: String, forModel model: String? = nil,
                     phase: AgentModelEffortPhase = .runtime) -> Bool {
        let v = value.lowercased()
        if let model,
           let entry = models.first(where: { $0.id.lowercased() == model.lowercased() }),
           !entry.efforts.isEmpty {
            return entry.efforts.contains { $0.lowercased() == v }
        }
        let pool = phase == .launch
            ? effectiveLaunchEfforts + undocumentedLaunchEfforts
            : efforts
        return pool.contains { $0.lowercased() == v }
    }
}

/// 一个 effort 值是**起 session 时**传的还是**跑起来之后**切的。
/// claude 这两处认的集合不同（实测），所以对照必须分场合。
enum AgentModelEffortPhase: Equatable {
    /// `start_session` / composer 起 session → 落 claude 的 `--effort` / codex 的
    /// `thread/start.config.model_reasoning_effort`。
    case launch
    /// `set_session_profile` → 落 claude 终端的 `/effort` 斜杠命令。
    case runtime
}

// MARK: - 新鲜度

/// 一张表「有多可信」。**永远不许静默**：除了 `.freshProbe`，其余都必须在注入给
/// 机长的文案里说出来（Todo #37 的硬要求：过期的手工表不能当事实呈现）。
enum AgentModelTableFreshness: Equatable {
    /// 现探且还新鲜 —— 唯一有资格把未知值判成「不存在」的状态。
    case freshProbe
    /// 现探但探得有些日子了（app 长期没跑 / 探测一直失败）。
    case stale(days: Int)
    /// 手工兜底表，附最后核实距今天数。
    case manual(days: Int)
    /// 连日期都读不出来（旧文件 / 字段缺失）—— 最不该静默的一种。
    case undated
}

// MARK: - 目录（表的来源、兜底、判定、文案）

enum AgentModelCatalog {
    /// helper（离线子进程）读的快照文件名；与 quota.json 同目录（`--dir`）。
    static let fileName = "models.json"

    /// 现探的表放多久算陈旧。app 在跑时每 6 小时探一轮，3 天没探到 = 这台机器
    /// 上 PendingCrew 基本没开过、或探测通道已经坏了 —— 两种都该明说。
    static let probeStaleDays = 3
    /// 手工表多久没核实就明确标「可能已过时」。CLI 大版本节奏约两三周一动。
    static let manualStaleDays = 14

    // MARK: 新鲜度判定

    static func freshness(of table: AgentModelTable, now: Date = Date()) -> AgentModelTableFreshness {
        switch table.source {
        case .probe:
            guard let raw = table.probedAt, let at = parseISO(raw) else { return .undated }
            let days = dayGap(from: at, to: now)
            return days >= probeStaleDays ? .stale(days: days) : .freshProbe
        case .manual:
            guard let raw = table.lastVerified, let at = parseDay(raw) else { return .undated }
            return .manual(days: dayGap(from: at, to: now))
        }
    }

    /// 这张表是不是「刚探到的活表」。**只用来决定提示措辞的强度，不是否决权**
    /// —— 活表说「不在当前活表里」，旧表/手工表还得先声明自己可能过时。
    /// 两种情况都照常放行，见 `AgentModelCheck`。
    static func isLiveProbe(_ table: AgentModelTable, now: Date = Date()) -> Bool {
        freshness(of: table, now: now) == .freshProbe
    }

    /// 注入给机长/session 的新鲜度说明。nil = 现探且新鲜，不必啰嗦。
    /// 其余一律给一句人话，**必须**跟着表一起呈现。
    static func stalenessNote(for table: AgentModelTable, now: Date = Date()) -> String? {
        switch freshness(of: table, now: now) {
        case .freshProbe:
            return nil
        case let .stale(days):
            return "这张表是 \(days) 天前探到的，可能已过时"
        case let .manual(days):
            return days >= manualStaleDays
                ? "这张表是手工兜底表（探测通道没答上），已 \(days) 天没核实过，可能已过时"
                : "这张表是手工兜底表（探测通道没答上），\(days) 天前核实过，不是现探的"
        case .undated:
            return "这张表没有可读的核实日期，无法判断新鲜度，可能已过时"
        }
    }

    // MARK: 兜底表（探测通道失灵时用）

    /// claude 「运行时 `/effort` 认、`--effort` 帮助里没写、但实测起 session 也收」的档。
    /// 探测通道给不出这个信息（帮助里查无此值），只能人工逐个实测维护。
    /// **未公开 = 随时可能变**，注入时会明确标不确定，不当稳定契约推荐。
    static let claudeUndocumentedLaunchEfforts = ["ultracode"]

    /// 手工兜底表最后一次人工核实的日期。**改这张表就改这个日期**，否则新鲜度提示
    /// 会撒谎。核实办法见 docs/handbook 的 model-catalog 页（两条实探命令）。
    static let fallbackLastVerified = "2026-08-09"

    /// claude 兜底表 —— 2026-08-09 用 `claude -p "/model" --output-format json`
    /// （claude 2.1.226）实测回显：`Available: sonnet, opus, haiku, fable, best,
    /// sonnet[1m], opus[1m], fable[1m], opusplan, default, or a full model ID.`
    /// 运行时 effort 来自 `claude -p "/effort"`：
    /// `Usage: /effort <low|medium|high|xhigh|max|ultracode|auto>`；
    /// 启动 effort 来自 `claude --effort <乱填> …` 的 stderr 警告：
    /// `Valid values: low, medium, high, xhigh, max.`（`auto` 在启动时**会被静默降级**，
    /// `ultracode` 实测收但帮助里没写 → 归 undocumented）。
    static let claudeFallback = AgentModelTable(
        agent: "claude", source: .manual, lastVerified: fallbackLastVerified,
        models: [
            AgentModel(id: "fable", displayName: "Fable"),
            AgentModel(id: "opus", displayName: "Opus"),
            AgentModel(id: "sonnet", displayName: "Sonnet"),
            AgentModel(id: "haiku", displayName: "Haiku"),
            AgentModel(id: "best", displayName: "Best", summary: "交给 CLI 挑当代最强"),
            AgentModel(id: "opusplan", displayName: "Opus Plan", summary: "计划态 Opus、执行态降档"),
            AgentModel(id: "default", displayName: "Default", summary: "回落到工作区/用户默认"),
            AgentModel(id: "fable[1m]", displayName: "Fable 1M", hidden: true),
            AgentModel(id: "opus[1m]", displayName: "Opus 1M", hidden: true),
            AgentModel(id: "sonnet[1m]", displayName: "Sonnet 1M", hidden: true),
        ],
        efforts: ["low", "medium", "high", "xhigh", "max", "ultracode", "auto"],
        launchEfforts: ["low", "medium", "high", "xhigh", "max"],
        undocumentedLaunchEfforts: ["ultracode"])

    /// codex 兜底表 —— 2026-08-09 用 app-server JSON-RPC `model/list`
    /// （codex-cli 0.145.0，`includeHidden: true`）实测。
    ///
    /// 注意这张表推翻了旧硬编码：`gpt-5-codex` / `gpt-5` 在当代 codex 里**已经不存在**，
    /// 旧 picker 给出的两个选项都是死值。
    static let codexFallback = AgentModelTable(
        agent: "codex", source: .manual, lastVerified: fallbackLastVerified,
        models: [
            AgentModel(id: "gpt-5.6-sol", displayName: "GPT-5.6-Sol",
                       summary: "Latest frontier agentic coding model.",
                       efforts: ["low", "medium", "high", "xhigh", "max", "ultra"], isDefault: true),
            AgentModel(id: "gpt-5.6-terra", displayName: "GPT-5.6-Terra",
                       summary: "Balanced agentic coding model for everyday work.",
                       efforts: ["low", "medium", "high", "xhigh", "max", "ultra"]),
            AgentModel(id: "gpt-5.6-luna", displayName: "GPT-5.6-Luna",
                       summary: "Fast and affordable agentic coding model.",
                       efforts: ["low", "medium", "high", "xhigh", "max"]),
            AgentModel(id: "gpt-5.5", displayName: "GPT-5.5",
                       efforts: ["low", "medium", "high", "xhigh"]),
            AgentModel(id: "gpt-5.4", displayName: "GPT-5.4",
                       efforts: ["low", "medium", "high", "xhigh"]),
            AgentModel(id: "gpt-5.4-mini", displayName: "GPT-5.4-Mini",
                       efforts: ["low", "medium", "high", "xhigh"]),
        ],
        efforts: ["low", "medium", "high", "xhigh", "max", "ultra"])

    static func fallback(agent: String) -> AgentModelTable? {
        switch agent {
        case "claude": return claudeFallback
        case "codex":  return codexFallback
        default:       return nil
        }
    }

    // MARK: 文案

    /// 一行清单，给工具描述 / prompt 注入用。**新鲜度提示与默认腿直接串在后面** ——
    /// 分开返回迟早有调用方只取清单不取提示，那就等于静默了。
    static func summaryLine(for table: AgentModelTable, now: Date = Date()) -> String {
        let names = table.visibleModels.map(\.id).joined(separator: " / ")
        var line = "\(table.agent) 可用模型：\(names)"
        if !table.efforts.isEmpty {
            // 两套不同就必须分开说 —— 合并成一串正是让 `auto` 被静默降级的那个坑。
            if table.runtimeOnlyEfforts.isEmpty {
                line += "；effort：\(table.efforts.joined(separator: " / "))"
            } else {
                line += "；起 session 时的 effort（--effort）："
                    + table.effectiveLaunchEfforts.joined(separator: " / ")
                line += "；⚠️ 另有 \(table.runtimeOnlyEfforts.joined(separator: " / "))"
                    + " **只能跑起来之后用 set_session_profile 切**，拿去起 session 会被静默降级成默认档"
            }
            if !table.undocumentedLaunchEfforts.isEmpty {
                line += "；\(table.undocumentedLaunchEfforts.joined(separator: " / "))"
                    + " 实测起 session 也收，但 CLI 帮助里没写（未公开，可能随时变，别当稳定契约）"
            }
        }
        line += "；\(defaultLegPhrase(for: table))"
        if let note = stalenessNote(for: table, now: now) { line += "（⚠️ \(note)）" }
        line += "。清单是「有哪些**当代**档」，不是合法值白名单 —— 旧别名后端多半仍解析得了。"
        return line
    }

    /// 「你不选 model 时实际会跑什么」那条腿的人话。清单只管显式选那条腿，
    /// 不把这句一起说出来，机长仍然不知道默认指向哪（机长 2026-08-09 的实测修正）。
    static func defaultLegPhrase(for table: AgentModelTable) -> String {
        guard let value = table.resolvedDefault, !value.isEmpty else {
            return "不显式选 model 时跑什么解析不出（照实留白，别猜）"
        }
        let from = table.resolvedDefaultSource.map { "，来源 \($0)" } ?? ""
        return "不显式选 model 时实际跑 \(value)\(from)"
    }

    /// 探测失败、连兜底表都没有时的说明（例：agent 键不认识）。
    static func missingLine(for agent: String) -> String {
        "\(agent) 没有任何可用模型表（探测失败且无兜底表）—— 填的值无从对照，起不来只能等 CLI 报错。"
    }

    // MARK: 日期

    static func parseISO(_ raw: String) -> Date? {
        let withFraction = ISO8601DateFormatter()
        withFraction.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return ISO8601DateFormatter().date(from: raw) ?? withFraction.date(from: raw)
    }

    static func parseDay(_ raw: String) -> Date? {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(secondsFromGMT: 0)
        f.dateFormat = "yyyy-MM-dd"
        return f.date(from: raw)
    }

    /// 相隔几天（向下取整，负数夹到 0 —— 未来时刻当「刚刚」，不报负数天数）。
    static func dayGap(from: Date, to: Date) -> Int {
        max(0, Int(to.timeIntervalSince(from) / 86_400))
    }
}

// MARK: - 落盘文件

/// models.json 的文件形状 —— app 的 `ModelCatalogCenter` 定时写、helper 的
/// `start_session` / `set_session_profile` 读。形状对齐 `AgentQuotaFile`：
/// 两家各一张表 + 各自的「这轮没探到」原因（取不到时留旧表继续用，但把原因说出来）。
struct AgentModelCatalogFile: Codable, Equatable {
    var claude: AgentModelTable?
    var codex: AgentModelTable?
    var claudeError: String?
    var codexError: String?

    init(claude: AgentModelTable? = nil, codex: AgentModelTable? = nil,
         claudeError: String? = nil, codexError: String? = nil) {
        self.claude = claude
        self.codex = codex
        self.claudeError = claudeError
        self.codexError = codexError
    }

    func table(agent: String) -> AgentModelTable? {
        switch agent {
        case "claude": return claude
        case "codex":  return codex
        default:       return nil
        }
    }

    func error(agent: String) -> String? {
        switch agent {
        case "claude": return claudeError
        case "codex":  return codexError
        default:       return nil
        }
    }

    /// 从目录读一份；读不到/解不出 → nil（调用方回落 `AgentModelCatalog.fallback`）。
    static func load(from directory: URL) -> AgentModelCatalogFile? {
        let url = directory.appendingPathComponent(AgentModelCatalog.fileName)
        guard let data = try? Data(contentsOf: url),
              let file = try? JSONDecoder().decode(AgentModelCatalogFile.self, from: data)
        else { return nil }
        return file
    }

    /// 解析出这一家该用的表：现探的优先，没有就回落手工兜底表。
    /// 调用方拿到后**必须**把 `AgentModelCatalog.stalenessNote` 一起呈现。
    static func resolveTable(agent: String, file: AgentModelCatalogFile?) -> AgentModelTable? {
        file?.table(agent: agent) ?? AgentModelCatalog.fallback(agent: agent)
    }
}

// MARK: - 参数校验（fail-loud 的判定核心，Todo #36）

/// 一个 model / effort 值对着表的裁决。
///
/// # 硬纪律：这张表**不是白名单**，本类型永远不产生「拒绝」
///
/// 2026-08-09 实测钉死（机长的可用性核查）：有 session 显式传了 `gpt-5-codex`
/// —— 一个**已经不在 codex `model/list` 里**的旧别名 —— **没报错、正常跑完了**。
/// 后端仍解析得了旧名字，只是不再出现在活表里。所以：
///
/// - 「不在探测清单里」**≠**「非法」。
/// - 不许硬拒、不许自动改写成别的名字、不许静默降级到默认值。
/// - 唯一正确的动作是**说出来**：把「这个名字不在当前活表里，可能是旧别名或已下线」
///   摆到调用方和白板上，让人自己判断。
///
/// 后人若想把它收紧成白名单：先去把上面那条实测复现一遍，复现不了再谈。
enum AgentModelCheck: Equatable {
    /// 值在当前表里 —— 什么都不用说。
    case ok
    /// 表是刚探到的活表，而这个值不在里面。**提示，不是错误**：多半是旧别名 /
    /// 已下线档，也可能是刚发布还没进清单。带上候选便于调用方自查。
    case notInLiveTable(candidates: [String])
    /// 连表本身都不够新（手工兜底 / 陈旧 / 无表）→ 连「不在活表里」都说不出口，
    /// 只能声明自己没能力对照。**同样必须说出来**，不许静默装作校验过了。
    case unverifiable(reason: String, candidates: [String])
    /// 这个 effort 值**只能运行时切、不能拿去起 session** —— 起 session 时传它，
    /// CLI 会静默降级成默认档（claude 的 `auto` 就是）。这是确定性的坑，不是猜测。
    case runtimeOnlyEffort(launchCandidates: [String])

    /// 有没有话要说（非 ok 一律要进白板 —— 静默才是 bug）。
    var needsAttention: Bool { self != .ok }
}

enum AgentModelValidator {
    /// 对照 model 值。**只产出提示，不产出否决**（见 `AgentModelCheck`）。
    static func checkModel(_ value: String, table: AgentModelTable?, now: Date = Date()) -> AgentModelCheck {
        guard let table else {
            return .unverifiable(reason: "没有任何模型表可对照", candidates: [])
        }
        if table.knowsModel(value) { return .ok }
        let candidates = table.visibleModels.map(\.id)
        if AgentModelCatalog.isLiveProbe(table, now: now) {
            return .notInLiveTable(candidates: candidates)
        }
        return .unverifiable(reason: AgentModelCatalog.stalenessNote(for: table, now: now)
                             ?? "表不是现探的", candidates: candidates)
    }

    /// 对照 effort 值。
    /// - `phase` 决定拿哪一套清单比（claude 的 `--effort` 与 `/effort` 不是一套）。
    /// - `model` 给了就按该模型自己的档位清单看（codex 逐模型不同）。
    ///
    /// 特例：值在**运行时**那套里、却不在**启动**那套里 → `.runtimeOnlyEffort`，
    /// 这是最坑的一种（CLI 会静默降级），必须单独说清楚而不是笼统说「不在表里」。
    static func checkEffort(_ value: String, model: String?, table: AgentModelTable?,
                            phase: AgentModelEffortPhase = .runtime,
                            now: Date = Date()) -> AgentModelCheck {
        guard let table else {
            return .unverifiable(reason: "没有任何模型表可对照", candidates: [])
        }
        if table.knowsEffort(value, forModel: model, phase: phase) { return .ok }
        let entry = model.flatMap { m in
            table.models.first { $0.id.lowercased() == m.lowercased() }
        }
        let candidates: [String]
        if entry?.efforts.isEmpty == false {
            candidates = entry!.efforts
        } else {
            candidates = phase == .launch ? table.effectiveLaunchEfforts : table.efforts
        }
        // 「运行时有、启动没有」是确定性的坑，不受表新鲜度影响 —— 这个差集本身
        // 就是探来的事实，直接点名，别降级成含糊的「不在表里」。
        if phase == .launch,
           table.runtimeOnlyEfforts.contains(where: { $0.lowercased() == value.lowercased() }) {
            return .runtimeOnlyEffort(launchCandidates: candidates)
        }
        if AgentModelCatalog.isLiveProbe(table, now: now) {
            return .notInLiveTable(candidates: candidates)
        }
        return .unverifiable(reason: AgentModelCatalog.stalenessNote(for: table, now: now)
                             ?? "表不是现探的", candidates: candidates)
    }

    /// 裁决 → 给机长/人看的人话。`.ok` 返回 nil。两种非 ok 都是**提示**，
    /// 调用方照常执行、同时把这句摆进工具回执和白板（fail-loud 的落点，Todo #36）。
    static func message(_ check: AgentModelCheck, knob: String, value: String,
                        agent: String) -> String? {
        switch check {
        case .ok:
            return nil
        case let .notInLiveTable(candidates):
            let list = candidates.isEmpty ? "（表里没有候选）" : candidates.joined(separator: " / ")
            return "\(agent) 的 \(knob) =「\(value)」不在当前活表里 —— 可能是旧别名或已下线的档"
                + "（旧名字后端往往仍解析得了，所以**没有拦你**，照常执行了）。确认一下是不是你要的。"
                + "当代可选：\(list)"
        case let .unverifiable(reason, candidates):
            let list = candidates.isEmpty ? "（无候选）" : candidates.joined(separator: " / ")
            return "\(agent) 的 \(knob) =「\(value)」**没能对照**：\(reason)。已照常执行，"
                + "但它若真不存在，要等 CLI 回显才知道。手上这张表里有：\(list)"
        case let .runtimeOnlyEffort(launchCandidates):
            return "\(agent) 的 \(knob) =「\(value)」**只能运行时切，不能拿来起 session** —— "
                + "起 session 走的是启动参数，CLI 不认这个值会**静默降级成默认档**"
                + "（不报错、看不出来）。起 session 请从这些里选：\(launchCandidates.joined(separator: " / "))；"
                + "确实要这个档就先起再用 set_session_profile 切。"
        }
    }
}
