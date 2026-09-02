import Foundation

/// 把本地 crew + session 事实渲染成 session 的世界观 system prompt（spec
/// 2026-06-05-pendingcrew-local-first-crew-design §6 复用 + chunk 4 接线）。
///
/// 对齐 edge `renderSessionWorldModel` 的语义，但**全本地**：吃 primitives（调用方
/// 从 `CrewDetail`/`CrewMember` 抽出来传），不引 app module 的 model —— 这样能编进
/// PendingCrewTests bundle 单测（同 LocalWhiteboardStore 模式）。
///
/// 本地无 DAG / 无责任分账，所以 `lineageBlock`/`sharesBlock`/`tiebreakerBlock`
/// **不提供** → `PromptTemplate` 把那几个槽 strip 成空（§6/§7 留空，benign）。
struct LocalSessionWorldModel {
    /// 一个本地人类成员（喂花名册）。
    struct Human: Equatable {
        var displayName: String
        var role: String
        var userId: String?

        init(displayName: String, role: String = "member", userId: String? = nil) {
            self.displayName = displayName
            self.role = role
            self.userId = userId
        }
    }

    /// 渲染所需的本地事实。
    struct Context {
        var sessionTaskBrief: String
        var runnerKind: String              // "claude_code" | "codex"
        var sessionId: String = ""
        var crewId: String
        var crewTitle: String
        var workingDirectory: String = ""
        var humans: [Human] = []
        var captainName: String? = nil
        var captainBotId: String? = nil
        /// 本地 DAG 组织位置：直系父 crew 标签 / 直系子 crew 标签（#463 起本地
        /// 也真实填充,不再 strip）。空 = 根 crew 无子。
        var parentTitles: [String] = []
        var childTitles: [String] = []
        var claudeSubscriptionPlan: String? = nil
        var codexSubscriptionPlan: String? = nil
        var locale: String = "zh"
    }

    let loader: LocalPromptLoader

    init(loader: LocalPromptLoader = LocalPromptLoader()) {
        self.loader = loader
    }

    /// 渲染好的世界观 markdown，可作为 `--append-system-prompt-file` 喂给 claude。
    func render(_ ctx: Context) throws -> String {
        try loader.render(name: "session-world-model", locale: ctx.locale, vars: buildVars(ctx))
    }

    /// 暴露给单测：只测 var 映射不碰文件。
    func buildVars(_ ctx: Context) -> [String: String] {
        [
            "sessionTaskBrief": ctx.sessionTaskBrief.isEmpty ? "(无任务描述)" : ctx.sessionTaskBrief,
            "runnerKind": ctx.runnerKind,
            "sessionId": ctx.sessionId.isEmpty ? "(本地 session)" : ctx.sessionId,
            "crewId": ctx.crewId,
            "crewTitle": ctx.crewTitle.isEmpty ? "(未命名 crew)" : ctx.crewTitle,
            "runtimeLocation": "local_host",
            "workingDirectory": ctx.workingDirectory.isEmpty ? "(未设置)" : ctx.workingDirectory,
            "humanRoster": renderRoster(ctx.humans),
            "captainBlock": renderCaptain(name: ctx.captainName, botId: ctx.captainBotId),
            "lineageBlock": renderLineage(parents: ctx.parentTitles, children: ctx.childTitles),
            "quotaPlanBlock": renderQuotaPlans(
                claude: ctx.claudeSubscriptionPlan, codex: ctx.codexSubscriptionPlan,
                locale: ctx.locale),
            // sharesBlock / tiebreakerBlock 本地不提供（无责任分账）→ strip 成空。
        ]
    }

    private func renderQuotaPlans(claude: String?, codex: String?, locale: String) -> String {
        if locale.lowercased().hasPrefix("en") {
            return "Detected/configured subscription tiers: Claude Code: \(claude ?? "unknown"); Codex: \(codex ?? "unknown"). A tier label gives scale context, but neither provider exposes an absolute remaining token/request balance here; do not invent one."
        }
        return "当前自动探到的订阅档位：Claude Code：\(claude ?? "未知")；Codex：\(codex ?? "未知")。档位只能提供量级背景；两家在这里都不给剩余 token/request 绝对量，禁止据此编造绝对额度。"
    }

    /// 组织位置：上级部门（父 crew）/ 下辖部门（子 crew）。汇报线仍是组织纪律的
    /// 主干；2026-08-11 起另有通讯录（`directory` / `contact`）作补充通道 ——
    /// 「worker 不直接跨群喊话」那条已不再成立（见 §10）。
    private func renderLineage(parents: [String], children: [String]) -> String {
        var lines: [String] = []
        lines.append(parents.isEmpty
            ? "本 crew 是根 crew（没有上级部门）。"
            : "上级（父）crew：" + parents.map { "「\($0)」" }.joined(separator: "、"))
        lines.append(children.isEmpty
            ? "暂无子 crew（下属部门）。"
            : "下辖子 crew：" + children.map { "「\($0)」" }.joined(separator: "、"))
        lines.append("跨部门协调的主干仍是汇报线（机长工具 `report_to_parent` / `message_child_crew`）——"
            + "重要的事、要资源要拍板的事，走本群或 `ask` 机长。"
            + "要直接找某个部门 / 某个 session 说句话，用通讯录（`directory` 查号、`contact` 联系，全员可用，跨线联系全部留痕）；"
            + "别拿它绕过自己机长去替他做决定。")
        return lines.joined(separator: "\n")
    }

    private func renderRoster(_ humans: [Human]) -> String {
        guard !humans.isEmpty else { return "(暂无)" }
        return humans
            .map { "- **\($0.displayName)** — role: \($0.role), user_id: `\($0.userId ?? "?")`" }
            .joined(separator: "\n")
    }

    private func renderCaptain(name: String?, botId: String?) -> String {
        guard let botId, let name else { return "本 crew 暂无指定 captain。" }
        return "Captain: **\(name)** (bot_id: `\(botId)`)"
    }
}
