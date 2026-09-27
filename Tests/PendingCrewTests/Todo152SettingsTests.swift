import XCTest

final class Todo152SettingsTests: XCTestCase {
    func testRunnerPreferenceModeDefaultsToCustomAndPreservesLegacyConditions() throws {
        let name = "captain-preference-mode-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }

        // Existing installations only have these two strings. Reading the new mode must
        // neither discard them nor alter the old inherited-runner behavior.
        CaptainRunnerPreferences.set("复杂重构", for: .claudeCode, defaults: defaults)
        CaptainRunnerPreferences.set("多文件测试", for: .codex, defaults: defaults)

        XCTAssertEqual(CaptainRunnerPreferences.preference(defaults: defaults), .custom)
        XCTAssertEqual(CaptainRunnerPreferences.condition(for: .claudeCode, defaults: defaults),
                       "复杂重构")
        XCTAssertEqual(CaptainRunnerPreferences.condition(for: .codex, defaults: defaults),
                       "多文件测试")
        XCTAssertNil(defaults.object(forKey: CaptainRunnerPreferences.preferenceKey),
                     "compatibility read must not eagerly rewrite user defaults")

        CaptainRunnerPreferences.setPreference(.codex, defaults: defaults)
        XCTAssertEqual(CaptainRunnerPreferences.preference(defaults: defaults), .codex)
        XCTAssertEqual(defaults.string(forKey: CaptainRunnerPreferences.preferenceKey), "codex")
        XCTAssertEqual(CaptainRunnerPreferences.condition(for: .claudeCode, defaults: defaults),
                       "复杂重构", "mode changes must not erase legacy conditions")

        defaults.set("retired_runner", forKey: CaptainRunnerPreferences.preferenceKey)
        XCTAssertEqual(CaptainRunnerPreferences.preference(defaults: defaults), .custom,
                       "unknown persisted values must preserve the safe old behavior")
    }

    func testRunnerPreferenceModeChangesAutoSelectionButCustomKeepsInheritedRunner() {
        let claude = CaptainRunnerCapability(kind: .claudeCode,
            executable: URL(fileURLWithPath: "/bin/claude"), authentication: .confirmed,
            health: .normal)
        let codex = CaptainRunnerCapability(kind: .codex,
            executable: URL(fileURLWithPath: "/bin/codex"), authentication: .confirmed,
            health: .normal)

        XCTAssertEqual(CaptainRunnerChoice.select(inherited: .claudeCode, preference: .custom,
                                                   claude: claude, codex: codex), .claudeCode)
        XCTAssertEqual(CaptainRunnerChoice.select(inherited: .claudeCode, preference: .codex,
                                                   claude: claude, codex: codex), .codex)
        XCTAssertEqual(CaptainRunnerChoice.select(inherited: .codex, preference: .claudeCode,
                                                   claude: claude, codex: codex), .claudeCode)
        XCTAssertEqual(CaptainRunnerChoice.select(inherited: .claudeCode, requested: .codex,
                                                   preference: .claudeCode,
                                                   claude: claude, codex: codex), .codex,
                       "an explicit tool request must override the global setting")
    }

    func testPreferencePersistenceAndClear() throws {
        let name = "captain-preference-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }
        CaptainRunnerPreferences.set("  复杂重构  ", for: .claudeCode, defaults: defaults)
        CaptainRunnerPreferences.set("多文件测试", for: .codex, defaults: defaults)
        XCTAssertEqual(CaptainRunnerPreferences.get(.claudeCode, defaults: defaults), "  复杂重构  ")
        XCTAssertEqual(CaptainRunnerPreferences.get(.codex, defaults: defaults), "多文件测试")
        CaptainRunnerPreferences.set(" \n ", for: .claudeCode, defaults: defaults)
        XCTAssertEqual(CaptainRunnerPreferences.get(.claudeCode, defaults: defaults), "")
        XCTAssertNil(defaults.object(forKey: CaptainRunnerPreferences.claudeKey))
        XCTAssertEqual(CaptainRunnerPreferences.get(.codex, defaults: defaults), "多文件测试")
    }

    func testUnavailableOrUnauthenticatedPreferenceNeverWins() {
        let codex = CaptainRunnerCapability(kind: .codex,
            executable: URL(fileURLWithPath: "/bin/codex"), authentication: .confirmed,
            health: .normal)
        let blocked: [(URL?, CaptainRunnerCapability.Authentication,
                       CaptainRunnerCapability.Health)] = [
            (nil, .confirmed, .normal),
            (URL(fileURLWithPath: "/bin/claude"), .required, .normal),
            (URL(fileURLWithPath: "/bin/claude"), .unknown, .normal),
            (URL(fileURLWithPath: "/bin/claude"), .confirmed, .unhealthy),
            (URL(fileURLWithPath: "/bin/claude"), .confirmed, .unknown),
        ]
        for (executable, auth, health) in blocked {
            let claude = CaptainRunnerCapability(kind: .claudeCode, executable: executable,
                                                 authentication: auth, health: health)
            XCTAssertEqual(CaptainRunnerChoice.select(inherited: .claudeCode,
                                                       claude: claude, codex: codex), .codex)
        }
        let unavailable = CaptainRunnerCapability(kind: .codex, executable: nil,
                                                  authentication: .unknown, health: .unknown)
        XCTAssertNil(CaptainRunnerChoice.select(inherited: .claudeCode,
                                                claude: unavailable, codex: unavailable))
    }

    func testPathFirstExecutableWins() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let first = root.appendingPathComponent("first")
        let second = root.appendingPathComponent("second")
        try FileManager.default.createDirectory(at: first, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: second, withIntermediateDirectories: true)
        let a = first.appendingPathComponent("codex")
        let b = second.appendingPathComponent("codex")
        try FileManager.default.copyItem(at: URL(fileURLWithPath: "/bin/sh"), to: a)
        try FileManager.default.copyItem(at: URL(fileURLWithPath: "/bin/sh"), to: b)
        XCTAssertEqual(LocalCodingAgentExecutable.resolve(.codex,
            path: "\(first.path):\(second.path)"), a)
        XCTAssertEqual(LocalCodingAgentExecutable.resolve(.codex,
            path: "\(second.path):\(first.path)"), b)
    }

    func testFreshRunnerHealthOverridesHealthyCLIProbe() throws {
        let now = Date()
        var snapshot = CrewSessionsSnapshot()
        snapshot.updatedAt = ISO8601DateFormatter().string(from: now)
        snapshot.crews = ["crew-a": [
            .init(sessionId: "one", name: "Codex", role: "worker", brief: "",
                  state: "error", healthDetail: "认证失效", runnerKind: "codex"),
        ]]
        XCTAssertEqual(CaptainRunnerProbe.observedHealthProblem(
            kind: .codex, snapshot: snapshot, now: now), "认证失效")
        XCTAssertNil(CaptainRunnerProbe.observedHealthProblem(
            kind: .claudeCode, snapshot: snapshot, now: now))
        snapshot.updatedAt = ISO8601DateFormatter().string(from: now.addingTimeInterval(-45))
        XCTAssertNil(CaptainRunnerProbe.observedHealthProblem(
            kind: .codex, snapshot: snapshot, now: now))
        let writer = try source("Sources/Mac/Services/CrewSessionRunner.swift")
        let probe = try source("Sources/Mac/LocalRunner/CaptainRunnerPreferences.swift")
        XCTAssertTrue(writer.contains("runnerKind: run.kind.rawValue"))
        XCTAssertTrue(probe.contains("snapshot: loadRuntimeSnapshot()"))
    }

    func testCodexNativeAccountReadRecoversOnlyConfirmedCLIProbeFailure() {
        let chatgpt: [String: Any] = ["account": ["type": "chatgpt"],
                                      "requiresOpenaiAuth": true]
        XCTAssertEqual(CaptainRunnerProbe.codexAuthentication(cliExit: nil, account: chatgpt),
                       .confirmed)
        XCTAssertEqual(CaptainRunnerProbe.codexAuthentication(cliExit: 1, account: chatgpt),
                       .confirmed)
        XCTAssertEqual(CaptainRunnerProbe.codexAuthentication(cliExit: 0, account: nil),
                       .confirmed)
        XCTAssertEqual(CaptainRunnerProbe.codexAuthentication(cliExit: 1, account: nil),
                       .unknown)
        XCTAssertEqual(CaptainRunnerProbe.codexAuthentication(cliExit: nil,
            account: ["account": NSNull(), "requiresOpenaiAuth": true]), .required)
        XCTAssertEqual(CaptainRunnerProbe.codexAuthentication(cliExit: nil,
            account: ["account": NSNull(), "requiresOpenaiAuth": false]), .unknown)
    }

    func testCodexNativeAccountReadUsesHandshakeAndDoesNotNeedATurn() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let executable = root.appendingPathComponent("fake-codex")
        let script = """
        #!/bin/sh
        IFS= read -r initialize || exit 1
        printf '%s\\n' '{"jsonrpc":"2.0","id":0,"result":{}}'
        IFS= read -r initialized || exit 1
        IFS= read -r account_read || exit 1
        case "$account_read" in
          *account*read*) ;;
          *) exit 2 ;;
        esac
        printf '%s\\n' '{"jsonrpc":"2.0","id":1,"result":{"account":{"type":"chatgpt"},"requiresOpenaiAuth":true}}'
        """
        try script.write(to: executable, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: executable.path)
        let account = try XCTUnwrap(CaptainRunnerProbe.readCodexAccount(executable))
        XCTAssertEqual(CaptainRunnerProbe.codexAuthentication(cliExit: nil, account: account),
                       .confirmed)
    }

    func testOldSessionSnapshotWithoutRunnerKindStillDecodes() throws {
        let json = """
        {"updatedAt":"2026-09-25T00:00:00Z","crews":{"crew-a":[{"sessionId":"one","name":"worker","role":"worker","brief":"","state":"idle"}]}}
        """
        let snapshot = try JSONDecoder().decode(CrewSessionsSnapshot.self, from: Data(json.utf8))
        XCTAssertNil(snapshot.crews["crew-a"]?.first?.runnerKind)
    }

    private func source(_ path: String) throws -> String {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        return try String(contentsOf: root.appendingPathComponent(path), encoding: .utf8)
    }

    func testPreferencesReachTheActualWorldModel() throws {
        let settings = try source("Sources/Mac/Views/CrewSettingsView.swift")
        let renderer = try source("Sources/Mac/LocalRunner/LocalSessionLaunch.swift")
        let world = try source("Sources/Stores/LocalSessionWorldModel.swift")
        let zh = try source("Resources/Prompts/session-world-model.zh.md")
        let en = try source("Resources/Prompts/session-world-model.en.md")
        XCTAssertTrue(settings.contains("CaptainRunnerPreferences"))
        XCTAssertTrue(renderer.contains("CaptainRunnerPreferences"))
        XCTAssertTrue(world.contains("captainRunnerBlock"))
        XCTAssertTrue(zh.contains("{{captainRunnerBlock}}"))
        XCTAssertTrue(en.contains("{{captainRunnerBlock}}"))
    }

    func testWorldModelRendererOnlyConsumesPreviouslyProbedCapabilities() throws {
        let renderer = try source("Sources/Mac/LocalRunner/LocalSessionLaunch.swift")
        let renderBody = try XCTUnwrap(renderer.components(separatedBy:
            "private static func renderWorldModelMarkdown(").last?.components(separatedBy:
            "private static func writeJSON(").first)
        XCTAssertFalse(renderBody.contains("CaptainRunnerProbe.inspect"))
        XCTAssertTrue(renderBody.contains("captainCapabilities.claude"))
        XCTAssertTrue(renderBody.contains("captainCapabilities.codex"))
        let runner = try source("Sources/Mac/Services/CrewSessionRunner.swift")
        let view = try source("Sources/Mac/Views/CrewSessionWindowView.swift")
        XCTAssertTrue(runner.contains("await CaptainRunnerCapabilities.capture()"))
        XCTAssertTrue(view.contains("await CaptainRunnerCapabilities.capture()"))
    }

    func testPathAndSettingsRemoveManualCLIOverride() throws {
        let executable = try source("Sources/Mac/LocalRunner/LocalCodingAgentExecutable.swift")
        let version = try source("Sources/Mac/Views/AgentCLIVersionView.swift")
        XCTAssertFalse(executable.contains("overrideDirectory(kind).map"))
        XCTAssertFalse(version.contains("NSOpenPanel"))
        XCTAssertFalse(version.contains("setOverrideDirectory"))
        XCTAssertFalse(version.contains("CLI 所在目录"))
    }

    func testBackendsKeepOperationsAndAccessibility() throws {
        let settings = try source("Sources/Mac/Views/CrewSettingsView.swift")
        for operation in ["BackendRegistry.load", "BackendRegistry.liveStatus",
                          "BackendRegistry.restartAction", "BackendRegistry.removePersisted",
                          "sessionHost.connectViewer", "ManualPairingCoordinator.production"] {
            XCTAssertTrue(settings.contains(operation), operation)
        }
        for identifier in ["backend.connect", "backend.refresh", "backend.restart",
                           "backend.remove", "backend.pair.create", "backend.pair.import"] {
            XCTAssertTrue(settings.contains(identifier), identifier)
        }
        XCTAssertFalse(settings.contains("GroupBox"))
    }
}
