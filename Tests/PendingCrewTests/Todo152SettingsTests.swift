import XCTest

final class Todo152SettingsTests: XCTestCase {
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
