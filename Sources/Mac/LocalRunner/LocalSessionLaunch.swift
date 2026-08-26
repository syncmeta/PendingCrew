#if os(macOS)
import Foundation

/// 本地起 session 的共享准备工序（#242 提取自 CrewSessionWindowView）。
/// 手动起（`CrewSessionWindowView`）与机长 `start_session` 排队起
/// （`CrewSessionRunner.startForBrief`）用同一份 —— comms 接线 / 世界观渲染不分叉。
/// （原来的第三条来路「relay task_request 远程自动起」随 #63 第二期删除跨端遥控
/// 整层一起去掉了。）
enum LocalSessionLaunch {
    /// Claude 的首次白板注入。后续轮次仍由 PostToolUse hook 续上；这里只补 hook 尚未
    /// 有机会触发的第一轮。读未读、首次 30 条上限、定向可见性与 fail-closed 都复用
    /// `HookEmitter`，不另造一套边界。
    static func initialPromptWithWhiteboard(
        _ prompt: String,
        crewId: String,
        sessionId: String,
        captain: Bool,
        directory: URL = LocalWhiteboardStore.defaultDirectory
    ) -> String {
        let context = HookEmitter(
            store: LocalWhiteboardStore(directory: directory),
            crewId: crewId,
            sessionId: sessionId,
            cursorDir: directory,
            isCaptain: captain
        ).emitContextAndAdvance()
        guard let context, !context.isEmpty else { return prompt }
        return """
        <external_crew_whiteboard>
        \(context)
        </external_crew_whiteboard>

        \(prompt)
        """
    }

    /// 生成 per-session 的 settings.json（PostToolUse hook = 自身二进制 `--mcp-hook`，
    /// 每轮注入未读白板）+ mcp-config.json（自身二进制 `--mcp-serve`，post_to_crew/read_whiteboard）。
    /// helper = `Bundle.main.executablePath`（re-exec self：app 二进制兼当 helper，最自包含、
    /// 免 embed）；白板 dir = `LocalWhiteboardStore.defaultDirectory`（与 app/store 同一份）。
    /// 接合 v2：comms 恒为本地，不再按 mode 跳过。任一步失败 → 对应 nil（session 仍启动，少 comms）。
    static func prepareLocalCommsConfig(
        crewId: String, sessionId: String, captain: Bool = false, label: String? = nil
    ) -> (settings: String?, mcp: String?) {
        guard let helper = Bundle.main.executablePath
        else { return (nil, nil) }
        let dir = LocalWhiteboardStore.defaultDirectory.path
        let tmp = FileManager.default.temporaryDirectory

        // mcp-config：command + args 是 exec 形式（claude 直接 spawn，不过 shell）→ 不引号。
        // captain → 多带 --captain，helper 端解锁机长专用 answer_decision（chunk2 T4）。
        // label → 多带 --label，post_to_crew 写白板时带发送者显示名（白板不再裸 uuid）。
        // --agent：本 session 跑在哪家 runner 上 —— set_session_profile 据此挑对照
        // 哪张可用模型表（Todo #37）。本函数只服务 claude 那条腿（codex 走
        // codexMcpServers），所以这里恒为 claude。
        var serveArgs = ["--mcp-serve", "--crew", crewId, "--dir", dir, "--session", sessionId,
                         "--agent", "claude"]
        if captain { serveArgs.append("--captain") }
        if let label, !label.isEmpty { serveArgs.append(contentsOf: ["--label", label]) }
        let mcp: [String: Any] = ["mcpServers": ["crew": [
            "command": helper,
            "args": serveArgs,
        ]]]
        // settings：两个 hook 的 command 都是 shell 字符串（claude 过 sh -c）→ 引号每段。
        // PostToolUse：每轮注入未读白板;机长多带 --captain → 注入含全机 crew 组织树概览（#24）。
        var hookArgs = [helper, "--mcp-hook", "--crew", crewId, "--dir", dir, "--session", sessionId]
        if captain { hookArgs.append("--captain") }
        let hookCmd = hookArgs.map(shellQuote).joined(separator: " ")
        // PreToolUse：权限审批 gate（spec ask-approval §3「权限类直达人类」）。v1 默认只 gate
        // computer-use（敏感的桌面控制）；**不** gate 自家 crew 工具（post_to_crew/read_whiteboard/
        // ask 都 mcp__crew__ 前缀，子串不含 "computer-use"）。未来可做成 UI 可配 gate 列表。
        let permGates = "computer-use"
        // --session 必带：权限待审批归档在**本地** sessionId 下，右栏内联卡片才过滤得到
        // （不能用 PreToolUse hook stdin 里 claude 自己的 session_id）。
        let permCmd = [helper, "--mcp-permission-hook", "--crew", crewId, "--dir", dir,
                       "--session", sessionId, "--gate", permGates]
            .map(shellQuote).joined(separator: " ")
        // Stop：agent 结束一轮时触发（#25 层 1）。这一轮它一次都没往群里说过话 → helper
        // 替它把收尾话头发进群、按规矩 @ 人。硬约束「一轮结束群里必须留下痕迹」的源头，
        // 补上自由文本提问那条漏 —— 那既不走 `ask`，画面上也没有可识别的选择菜单。
        // --label 带上：代发那条要署它的显示名，别在群里裸 uuid。
        var turnArgs = [helper, "--mcp-turn-hook", "--crew", crewId, "--dir", dir,
                        "--session", sessionId]
        if captain { turnArgs.append("--captain") }
        if let label, !label.isEmpty { turnArgs.append(contentsOf: ["--label", label]) }
        let turnCmd = turnArgs.map(shellQuote).joined(separator: " ")
        let settings: [String: Any] = ["hooks": [
            "PostToolUse": [["hooks": [["type": "command", "command": hookCmd]]]],
            "PreToolUse": [["hooks": [["type": "command", "command": permCmd]]]],
            "Stop": [["hooks": [["type": "command", "command": turnCmd]]]],
        ]]

        return (
            settings: writeJSON(settings, to: tmp.appendingPathComponent("pendingcrew-settings-\(sessionId).json")),
            mcp: writeJSON(mcp, to: tmp.appendingPathComponent("pendingcrew-mcp-\(sessionId).json"))
        )
    }

    /// 渲染本地世界观 system prompt → 临时 `.md` → 返回路径供
    /// `--append-system-prompt-file`。best-effort：任一步失败返 nil，session
    /// 仍照常启动（只是没世界观）。`appendPersona` 非 nil 时（captain session）
    /// 把 persona 拼到世界观后面。members 由 caller 拉好传入（view 用 backend、
    /// relay agent 同样走 backend）。
    static func renderWorldModelFile(
        detail: CrewDetail,
        members: [CrewMember],
        taskBrief: String,
        workdir: URL,
        sessionId: String,
        runnerKind: LocalCodingAgentKind,
        appendPersona: String? = nil
    ) -> String? {
        guard runnerKind.isAgent else { return nil }
        guard let md = renderWorldModelMarkdown(
            detail: detail, members: members, taskBrief: taskBrief, workdir: workdir,
            sessionId: sessionId, runnerKind: runnerKind, appendPersona: appendPersona)
        else { return nil }
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("pendingcrew-worldmodel-\(UUID().uuidString).md")
        do {
            try md.write(to: url, atomically: true, encoding: .utf8)
            return url.path
        } catch {
            return nil
        }
    }

    /// codex variant of world-model injection: render the SAME world-model markdown
    /// as `renderWorldModelFile` (runnerKind: codex) but return it as a STRING —
    /// codex feeds the world-model via `thread/start.developerInstructions`, not a
    /// `--append-system-prompt-file`. best-effort：渲染失败返 nil（session 仍照常
    /// 启动，只是没世界观）。`appendPersona` 非 nil（captain session）拼到世界观后面。
    static func renderWorldModelString(
        detail: CrewDetail,
        members: [CrewMember],
        taskBrief: String,
        workdir: URL,
        sessionId: String,
        appendPersona: String? = nil
    ) -> String? {
        renderWorldModelMarkdown(
            detail: detail, members: members, taskBrief: taskBrief, workdir: workdir,
            sessionId: sessionId, runnerKind: .codex, appendPersona: appendPersona)
    }

    /// codex 的 crew MCP 配置 dict → `thread/start.mcpServers`。与 claude 走同一个
    /// re-exec self helper（`--mcp-serve`）提供 post_to_crew/read_whiteboard；区别只是
    /// codex 经协议传 dict，claude 经 `--mcp-config <file>` 传文件。`captain` → 多带
    /// `--captain`，helper 端解锁机长专用 answer_decision（与 prepareLocalCommsConfig 一致）。
    static func codexMcpServers(crewId: String, sessionId: String, captain: Bool = false,
                                label: String? = nil) -> [String: Any]? {
        guard let helper = Bundle.main.executablePath else { return nil }
        let dir = LocalWhiteboardStore.defaultDirectory.path
        var args = ["--mcp-serve", "--crew", crewId, "--dir", dir, "--session", sessionId,
                    "--agent", "codex"]
        if captain { args.append("--captain") }
        if let label, !label.isEmpty { args.append(contentsOf: ["--label", label]) }
        return ["crew": ["command": helper, "args": args]]
    }

    /// 共享的世界观 markdown 渲染（claude 写临时文件、codex 取字符串都走这里，DRY）。
    /// 任一步失败返 nil（best-effort）。
    private static func renderWorldModelMarkdown(
        detail: CrewDetail,
        members: [CrewMember],
        taskBrief: String,
        workdir: URL,
        sessionId: String,
        runnerKind: LocalCodingAgentKind,
        appendPersona: String?
    ) -> String? {
        let humans = members
            .filter { $0.memberKind == "human" }
            .map { LocalSessionWorldModel.Human(
                displayName: $0.displayName ?? "(无名)", role: $0.role ?? "member", userId: $0.userId) }
        // 组织位置（本地 DAG）：caller 都在 MainActor（startCaptain/startForBrief/
        // 零配置启动）,直接读 store;非本地 crew（store 查无此 id）自然得空数组。
        let parents = MainActor.assumeIsolated {
            LocalCrewStore.shared.parentIds(of: detail.crew.id)
                .compactMap { LocalCrewStore.shared.title(of: $0) }
        }
        let children = MainActor.assumeIsolated {
            LocalCrewStore.shared.children(of: detail.crew.id).map(\.title)
        }
        let quotaFile = LocalWhiteboardStore.defaultDirectory
            .appendingPathComponent("quota.json")
        let quota = (try? Data(contentsOf: quotaFile)).flatMap {
            try? JSONDecoder().decode(AgentQuotaFile.self, from: $0)
        }
        func planDescription(snapshot: AgentQuotaSnapshot?, agent: String) -> String? {
            if let snapshot { return snapshot.subscriptionPlanDescription }
            return AgentSubscriptionPlanPreference.override(agent: agent).map { "\($0)（手动设置）" }
        }
        let plans = (planDescription(snapshot: quota?.claude, agent: "claude"),
                     planDescription(snapshot: quota?.codex, agent: "codex"))
        let ctx = LocalSessionWorldModel.Context(
            sessionTaskBrief: taskBrief,
            runnerKind: {
                switch runnerKind {
                case .claudeCode: return "claude_code"
                case .codex: return "codex"
                case .terminal: return "terminal"
                }
            }(),
            sessionId: sessionId,
            crewId: detail.crew.id,
            crewTitle: detail.crew.title,
            workingDirectory: workdir.path,
            humans: humans,
            captainName: detail.captain?.displayName,
            captainBotId: detail.captain?.botId,
            parentTitles: parents,
            childTitles: children,
            claudeSubscriptionPlan: plans.0,
            codexSubscriptionPlan: plans.1)
        guard var md = try? LocalSessionWorldModel().render(ctx) else { return nil }
        if let persona = appendPersona {
            md += "\n\n---\n\n" + persona
        }
        return md
    }

    private static func writeJSON(_ obj: [String: Any], to url: URL) -> String? {
        guard let data = try? JSONSerialization.data(withJSONObject: obj, options: [.prettyPrinted]),
              (try? data.write(to: url, options: .atomic)) != nil else { return nil }
        return url.path
    }

    private static func shellQuote(_ s: String) -> String {
        "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
}
#endif
