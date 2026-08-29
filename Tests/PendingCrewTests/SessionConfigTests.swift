import XCTest


final class SessionConfigTests: XCTestCase {
    /// 2026-08-28 现场事故：另一个 session 为排查发布进程执行 `ps`，直接从
    /// Claude 的 argv 读到了本 session 的完整白板与开场任务，随后误当成自己的任务。
    /// 开场正文必须经 PTY stdin 送入，绝不能留在任何本机进程都能看到的命令行里。
    func testClaudeInitialPromptDoesNotLeakIntoProcessArguments() {
        let secret = "<external_crew_whiteboard>foreign crew private task</external_crew_whiteboard>"
        let cfg = SessionConfig(kind: .claudeCode, initialPrompt: secret)

        XCTAssertFalse(cfg.argv().contains(secret),
                       "Claude opening prompt must not be observable through ps argv")
    }

    func testClaudeArgvUsesAutoModeNotBypass() {
        let cfg = SessionConfig(kind: .claudeCode, initialPrompt: "do the thing")
        let argv = cfg.argv()
        XCTAssertFalse(argv.contains("--dangerously-skip-permissions"),
                       "claude must NOT bypass permissions (spec §9 auto mode)")
        XCTAssertEqual(argv, ["--permission-mode", "auto"])
    }

    func testClaudeArgvKeepsModelAndEffortButExcludesPrompt() {
        let cfg = SessionConfig(kind: .claudeCode, model: "opus", effort: "high",
                                initialPrompt: "fix bug")
        XCTAssertEqual(cfg.argv(),
                       ["--permission-mode", "auto", "--model", "opus", "--effort", "high"])
    }

    func testClaudeArgvKeepsMcpConfigButExcludesPrompt() {
        let cfg = SessionConfig(kind: .claudeCode, initialPrompt: "你是机长",
                                mcpConfigFile: "/tmp/m.json")
        let argv = cfg.argv()
        XCTAssertEqual(argv, ["--permission-mode", "auto", "--mcp-config", "/tmp/m.json"])
        XCTAssertFalse(argv.contains("你是机长"))
    }

    func testClaudeArgvOmitsEmptyOptionalsAndEmptyPrompt() {
        let cfg = SessionConfig(kind: .claudeCode, initialPrompt: "")
        XCTAssertEqual(cfg.argv(), ["--permission-mode", "auto"])
    }

    func testClaudeArgvResumePrependsResumeFlag() {
        let cfg = SessionConfig(kind: .claudeCode, initialPrompt: "go", resumeSessionId: "abc123")
        XCTAssertEqual(cfg.argv(),
                       ["--resume", "abc123", "--permission-mode", "auto"])
    }

    func testIsolationDoesNotLeakIntoArgv() {
        let isolated = SessionConfig(kind: .claudeCode, initialPrompt: "go", isolation: true)
        let shared = SessionConfig(kind: .claudeCode, initialPrompt: "go", isolation: false)
        XCTAssertEqual(isolated.argv(), shared.argv(), "isolation 是编排参数,不是 CLI flag")
        XCTAssertEqual(isolated.argv(), ["--permission-mode", "auto"])
    }

    func testCodexArgvKeepsPerSessionEffortOutOfProcessArguments() {
        let cfg = SessionConfig(kind: .codex, effort: "high", initialPrompt: "ship it")
        // Codex launches app-server; effort and prompt are sent over the protocol.
        XCTAssertEqual(cfg.argv(), ["app-server"])
    }

    func testCodexArgvAppServerNoEffort() {
        let cfg = SessionConfig(kind: .codex, initialPrompt: "go")
        XCTAssertEqual(cfg.argv(), ["app-server"])
    }

    func testTerminalArgvIsOnlyLoginShellAndIgnoresAgentConfiguration() {
        let cfg = SessionConfig(
            kind: .terminal,
            model: "must-not-leak",
            effort: "must-not-leak",
            initialPrompt: "must-not-run",
            appendSystemPromptFile: "/tmp/world.md",
            settingsFile: "/tmp/settings.json",
            mcpConfigFile: "/tmp/mcp.json")
        XCTAssertEqual(cfg.argv(), ["-l"])
    }

    func testTerminalHasNoAgentOrServerRunnerSemantics() {
        XCTAssertFalse(LocalCodingAgentKind.terminal.isAgent)
        XCTAssertNil(LocalCodingAgentKind.terminal.serverRunnerKind)
        XCTAssertNil(LocalCodingAgentKind.inferred(fromDisplayName: "终端 · abc123"))
        XCTAssertNil(LocalCodingAgentExecutable.resolve(.terminal))
    }

    func testPlainTerminalDefaultShellUsesValidUserShellThenZshFallback() {
        XCTAssertEqual(
            PlainTerminalSession.defaultShell(
                environment: ["SHELL": "/custom/fish"],
                isExecutable: { $0 == "/custom/fish" }),
            "/custom/fish")
        XCTAssertEqual(
            PlainTerminalSession.defaultShell(
                environment: ["SHELL": "relative-shell"],
                isExecutable: { $0 == "/bin/zsh" }),
            "/bin/zsh")
        XCTAssertNil(
            PlainTerminalSession.defaultShell(
                environment: ["SHELL": "/missing/shell"],
                isExecutable: { _ in false }))
    }

    func testPlainTerminalEnvironmentStripsPendingCrewSecretsOnly() {
        let env = PlainTerminalSession.shellEnvironment([
            "HOME": "/Users/me",
            "SHELL": "/bin/zsh",
            "MY_TOOL": "yes",
            "PENDINGCREW_DEVICE_TOKEN": "secret",
            "PENDINGBOT_DEVICE_GRANT": "secret",
            "SUPABASE_SECRET_KEY": "secret",
        ])
        XCTAssertEqual(env["MY_TOOL"], "yes")
        XCTAssertEqual(env["HOME"], "/Users/me")
        XCTAssertNil(env["PENDINGCREW_DEVICE_TOKEN"])
        XCTAssertNil(env["PENDINGBOT_DEVICE_GRANT"])
        XCTAssertNil(env["SUPABASE_SECRET_KEY"])
    }

    func testClaudeArgvAppendsSystemPromptFile() {
        // 世界观注入：--append-system-prompt-file（append，不 replace；flag 已核实）。
        let cfg = SessionConfig(kind: .claudeCode, initialPrompt: "go",
                                appendSystemPromptFile: "/tmp/world.md")
        XCTAssertEqual(cfg.argv(),
                       ["--permission-mode", "auto",
                        "--append-system-prompt-file", "/tmp/world.md"])
    }

    func testClaudeArgvOmitsEmptyAppendFile() {
        let cfg = SessionConfig(kind: .claudeCode, initialPrompt: "go", appendSystemPromptFile: "")
        XCTAssertEqual(cfg.argv(), ["--permission-mode", "auto"])
    }

    func testCodexArgvIgnoresClaudeOnlyFlags() {
        let cfg = SessionConfig(kind: .codex, initialPrompt: "go",
                                appendSystemPromptFile: "/t/w.md",
                                settingsFile: "/t/s.json", mcpConfigFile: "/t/m.json")
        XCTAssertEqual(cfg.argv(), ["app-server"])
    }

    func testClaudeArgvSettingsAndMcpConfig() {
        // 本地 comms：--settings(hook 注入未读白板) + --mcp-config(post_to_crew/read)。
        let cfg = SessionConfig(kind: .claudeCode, initialPrompt: "go",
                                settingsFile: "/t/s.json", mcpConfigFile: "/t/m.json")
        XCTAssertEqual(cfg.argv(),
                       ["--permission-mode", "auto",
                        "--settings", "/t/s.json", "--mcp-config", "/t/m.json"])
    }

    func testClaudeArgvAllLocalCommsFlagsOrder() {
        let cfg = SessionConfig(kind: .claudeCode, initialPrompt: "go",
                                appendSystemPromptFile: "/t/w.md",
                                settingsFile: "/t/s.json", mcpConfigFile: "/t/m.json")
        XCTAssertEqual(cfg.argv(),
                       ["--permission-mode", "auto",
                        "--append-system-prompt-file", "/t/w.md",
                        "--settings", "/t/s.json", "--mcp-config", "/t/m.json"])
    }

    func testSharedWorkspaceReturnsCrewDirectoryWithoutGit() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("session-workspace-\(UUID().uuidString)")
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        XCTAssertEqual(
            try SessionWorkspace.resolve(
                crewDirectory: directory, isolation: false, hint: "shared"),
            directory)
    }

    func testIsolatedWorkspaceDoesNotFallBackForNonGitDirectory() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("session-workspace-\(UUID().uuidString)")
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        XCTAssertThrowsError(
            try SessionWorkspace.resolve(
                crewDirectory: directory, isolation: true, hint: "isolated"))
    }

    func testCodexApprovalModesDefaultPersistAndStaySessionScoped() {
        let suite = "CodexApprovalModeStoreTests-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = CodexApprovalModeStore(defaults: defaults, defaultsKey: "modes")

        XCTAssertEqual(store.reviewer(crewId: "crew-a", scope: .captain), .autoReview)
        XCTAssertEqual(store.reviewer(crewId: "crew-a", scope: .session("one")), .autoReview)

        store.set(.user, crewId: "crew-a", scope: .captain)
        store.set(.user, crewId: "crew-a", scope: .session("one"))
        XCTAssertEqual(store.reviewer(crewId: "crew-a", scope: .captain), .user)
        XCTAssertEqual(store.reviewer(crewId: "crew-a", scope: .session("one")), .user)
        XCTAssertEqual(store.reviewer(crewId: "crew-a", scope: .session("two")), .autoReview)
        XCTAssertEqual(store.reviewer(crewId: "crew-b", scope: .captain), .autoReview)

        let restored = CodexApprovalModeStore(defaults: defaults, defaultsKey: "modes")
        XCTAssertEqual(restored.reviewer(crewId: "crew-a", scope: .captain), .user)
        XCTAssertEqual(restored.reviewer(crewId: "crew-a", scope: .session("one")), .user)
    }

}
