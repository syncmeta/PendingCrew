#if os(macOS)
import XCTest

final class AgentCLIMaintenanceTests: XCTestCase {
    func testBothCLIFormatsUseTheExistingNumericVersionParser() {
        XCTAssertEqual(LocalCodingAgentExecutable.cliVersion("codex-cli 0.153.4\n"), "0.153.4")
        XCTAssertEqual(LocalCodingAgentExecutable.cliVersion("2.1.263 (Claude Code)\n"), "2.1.263")
        for bad in ["", "error 400", "codex-cli banana", "2.1.x (Claude Code)", "codex-cli 0.153.4-beta", "0..4", "warning\n2.1.263 (Claude Code)"] {
            XCTAssertNil(LocalCodingAgentExecutable.cliVersion(bad), bad)
        }
    }

    func testIdleLiveProcessesBlockAndOrdinaryCommandArgumentsDoNot() throws {
        let output = "101 /Users/a/.local/bin/codex app-server\n102 claude\n103 /bin/echo codex\n104 /bin/zsh -c claude --version\n105 /usr/bin/node /tmp/node_modules/@openai/codex/bin/codex.js\n"
        XCTAssertEqual(try AgentCLIProcessScan.blockers(output, kind: .codex), [101, 105])
        XCTAssertEqual(try AgentCLIProcessScan.blockers(output, kind: .claudeCode), [102])
        XCTAssertThrowsError(try AgentCLIProcessScan.blockers("unparseable", kind: .codex))
        XCTAssertThrowsError(try AgentCLIProcessScan.blockers("", kind: .codex))
    }

    func testMaintenanceLeaseBlocksLaunchAndConcurrentMaintenance() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: dir) }
        var session: AgentCLIMaintenanceLease? = try .acquire(.codex, exclusive: false, directory: dir)
        XCTAssertThrowsError(try AgentCLIMaintenanceLease.acquire(.codex, exclusive: true, directory: dir))
        session = nil
        XCTAssertNil(session)
        let upgrade = try AgentCLIMaintenanceLease.acquire(.codex, exclusive: true, directory: dir)
        XCTAssertThrowsError(try AgentCLIMaintenanceLease.acquire(.codex, exclusive: false, directory: dir))
        XCTAssertThrowsError(try AgentCLIMaintenanceLease.acquire(.codex, exclusive: true, directory: dir))
        let otherRunner = try AgentCLIMaintenanceLease.acquire(.claudeCode, exclusive: false, directory: dir)
        withExtendedLifetime((upgrade, otherRunner)) {}
    }
    private func fixture(_ body: (URL, URL, AgentCLIMaintenanceService) throws -> Void) throws {
        let home = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: home) }
        let release = home.appendingPathComponent(".codex/packages/standalone/releases/0.153.4-aarch64-apple-darwin")
        try FileManager.default.createDirectory(at: release.appendingPathComponent("bin"), withIntermediateDirectories: true)
        let binary = release.appendingPathComponent("bin/codex")
        try Data("fixture".utf8).write(to: binary)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: binary.path)
        let current = home.appendingPathComponent(".codex/packages/standalone/current")
        try FileManager.default.createSymbolicLink(at: current, withDestinationURL: release)
        var service = AgentCLIMaintenanceService()
        service.home = home
        service.lockDirectory = home.appendingPathComponent("locks")
        service.resolve = { _ in current.appendingPathComponent("bin/codex") }
        service.execute = { _, args, _ in
            .init(status: 0, output: args == ["--version"] ? "codex-cli 0.153.4" : "10 /sbin/launchd", logURL: nil)
        }
        try body(home, current, service)
    }

    func testNativeUpdateOnlyCallsUpdateAfterNoProcessesAndHoldsLaunchLock() throws {
        try fixture { _, _, base in
            var service = base
            var calls: [[String]] = []
            service.execute = { _, args, _ in
                calls.append(args)
                if args == ["update"] {
                    XCTAssertThrowsError(try AgentCLIMaintenanceLease.acquire(.codex, exclusive: false, directory: base.lockDirectory))
                }
                return .init(status: 0, output: args == ["--version"] ? "codex-cli 0.153.4" : "10 /sbin/launchd", logURL: nil)
            }
            let installation = try service.inspect(.codex)
            XCTAssertTrue(installation.native)
            XCTAssertEqual(calls, [["--version"]], "Detection must never update")
            let result = try service.update(installation)
            XCTAssertEqual(calls.filter { $0 == ["update"] }.count, 1)
            XCTAssertTrue(result.contains("版本未变化"))
            XCTAssertTrue(result.contains("不证明已是最新"))
        }
    }

    func testLiveExternalProcessBlocksUpdateEvenWhenNoCrewLeaseExists() throws {
        try fixture { _, _, base in
            let installation = try base.inspect(.codex)
            var service = base
            var updated = false
            service.execute = { exe, args, _ in
                if args == ["update"] { updated = true }
                return .init(status: 0, output: exe.path == "/bin/ps" ? "24 codex app-server" : "codex-cli 0.153.4", logURL: nil)
            }
            XCTAssertThrowsError(try service.update(installation)) { XCTAssertTrue($0.localizedDescription.contains("24")) }
            XCTAssertFalse(updated)
        }
    }

    func testScanFailureAndUpdateFailureAreReportedWithOutput() throws {
        try fixture { _, _, base in
            let installation = try base.inspect(.codex)
            for failedCommand in ["ps", "update"] {
                var service = base
                var updated = false
                service.execute = { exe, args, _ in
                    if args == ["update"] { updated = true }
                    let fail = failedCommand == "ps" ? exe.lastPathComponent == "ps" : args == ["update"]
                    return .init(status: fail ? 7 : 0, output: fail ? "injected permission failure" : (args == ["--version"] ? "codex-cli 0.153.4" : "10 /sbin/launchd"), logURL: nil)
                }
                XCTAssertThrowsError(try service.update(installation)) {
                    XCTAssertTrue($0.localizedDescription.contains("injected permission failure"))
                    XCTAssertTrue($0.localizedDescription.contains("7"))
                }
                XCTAssertEqual(updated, failedCommand == "update")
            }
        }
    }

    func testUnknownInstallAndChangedVersionNeverUpdate() throws {
        try fixture { home, _, base in
            let installation = try base.inspect(.codex)
            for unknown in [true, false] {
                var service = base
                if unknown { service.resolve = { _ in home.appendingPathComponent("brew/bin/codex") } }
                var updated = false
                service.execute = { _, args, _ in
                    if args == ["update"] { updated = true }
                    return .init(status: 0, output: args == ["--version"] ? "codex-cli 0.999.0" : "10 /sbin/launchd", logURL: nil)
                }
                XCTAssertThrowsError(try service.update(installation))
                XCTAssertFalse(updated)
            }
        }
    }

    func testExitZeroWithUnparseablePostUpdateVersionIsNotSuccess() throws {
        try fixture { _, _, base in
            let installation = try base.inspect(.codex)
            var service = base
            var updated = false
            service.execute = { _, args, _ in
                if args == ["update"] { updated = true }
                return .init(status: 0, output: args == ["--version"] ? (updated ? "broken" : "codex-cli 0.153.4") : "10 /sbin/launchd", logURL: nil)
            }
            XCTAssertThrowsError(try service.update(installation)) { XCTAssertTrue($0.localizedDescription.contains("无法解析")) }
            XCTAssertTrue(updated)
        }
    }

    func testRollbackUsesOnlyRetainedMatchingPlatformAndVerifiesBinary() throws {
        try fixture { home, current, base in
            let releases = home.appendingPathComponent(".codex/packages/standalone/releases")
            for name in ["0.149.1-aarch64-apple-darwin", "0.137.0-x86_64-apple-darwin"] {
                let binary = releases.appendingPathComponent(name + "/bin/codex")
                try FileManager.default.createDirectory(at: binary.deletingLastPathComponent(), withIntermediateDirectories: true)
                try Data("fixture".utf8).write(to: binary)
                try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: binary.path)
            }
            var service = base
            service.execute = { exe, args, _ in
                let old = exe.resolvingSymlinksInPath().path.contains("0.149.1")
                return .init(status: 0, output: args == ["--version"] ? "codex-cli " + (old ? "0.149.1" : "0.153.4") : "10 /sbin/launchd", logURL: nil)
            }
            let installation = try service.inspect(.codex)
            XCTAssertEqual(installation.rollbackReleases, ["0.149.1-aarch64-apple-darwin"])
            XCTAssertThrowsError(try service.rollback(installation, release: "../../elsewhere"))
            XCTAssertThrowsError(try service.rollback(installation, release: "0.137.0-x86_64-apple-darwin"))
            let result = try service.rollback(installation, release: "0.149.1-aarch64-apple-darwin")
            XCTAssertTrue(result.contains("0.153.4 → 0.149.1"))
            XCTAssertTrue(current.resolvingSymlinksInPath().path.hasSuffix("0.149.1-aarch64-apple-darwin"))
        }
    }

    func testClaudeNativeTargetAndDoctorUseExactArgumentArrays() throws {
        try fixture { home, _, base in
            var service = base
            let exe = home.appendingPathComponent(".local/share/claude/versions/2.1.263")
            try FileManager.default.createDirectory(at: exe.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Data("fixture".utf8).write(to: exe)
            service.resolve = { _ in exe }
            var calls: [[String]] = []
            service.execute = { _, args, _ in
                calls.append(args)
                return .init(status: 0, output: args == ["--version"] ? "2.1.263 (Claude Code)" : "10 /sbin/launchd", logURL: nil)
            }
            let installation = try service.inspect(.claudeCode)
            XCTAssertTrue(installation.native)
            XCTAssertThrowsError(try service.update(installation, target: "latest; touch nope"))
            _ = try service.update(installation, target: "stable")
            _ = try service.doctor(.claudeCode)
            XCTAssertTrue(calls.contains(["update", "stable"]))
            XCTAssertTrue(calls.contains(["doctor"]))
        }
    }

    func testCommandDrainsLargeOutputAndRetainsNonzeroDiagnostics() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let result = try AgentCLICommand.run(URL(fileURLWithPath: "/bin/sh"),
            ["-c", "i=0; while [ $i -lt 10000 ]; do printf 'fixture output\\n'; i=$((i+1)); done; printf 'injected stderr' >&2; exit 7"], 10, directory: directory)
        XCTAssertEqual(result.status, 7)
        XCTAssertTrue(result.output.contains("injected stderr"))
        XCTAssertGreaterThan(try Data(contentsOf: XCTUnwrap(result.logURL)).count, 32_768)
    }

    func testCommandTimeoutAlsoStopsChildAfterWrapperExits() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let start = Date()
        XCTAssertThrowsError(try AgentCLICommand.run(URL(fileURLWithPath: "/bin/sh"),
            ["-c", "sleep 30 & echo $!; exit 0"], 0.3, directory: directory)) {
            XCTAssertTrue($0.localizedDescription.contains("超过"))
            XCTAssertTrue($0.localizedDescription.contains("日志"))
        }
        XCTAssertLessThan(Date().timeIntervalSince(start), 5)
        let log = try XCTUnwrap(FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil).first)
        let pid = try XCTUnwrap(Int32(String(contentsOf: log).trimmingCharacters(in: .whitespacesAndNewlines)))
        // Reparented children can briefly remain zombies; neither state can keep installing.
        let probe = Process()
        probe.executableURL = URL(fileURLWithPath: "/bin/ps")
        probe.arguments = ["-p", String(pid), "-o", "stat="]
        let pipe = Pipe(); probe.standardOutput = pipe
        try probe.run(); probe.waitUntilExit()
        let state = String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        XCTAssertTrue(state.isEmpty || state.hasPrefix("Z"), "child still alive: \(state)")
    }

    func testLoginShellDiscoveryHasABoundedFailureInsteadOfHangingDetection() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let shell = directory.appendingPathComponent("slow-shell")
        try Data("#!/bin/sh\nsleep 30\n".utf8).write(to: shell)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: shell.path)
        let start = Date()
        XCTAssertNil(LocalCodingAgentExecutable.loginShellPath(shell: shell.path, timeout: 0.2))
        XCTAssertLessThan(Date().timeIntervalSince(start), 3)
    }

}
#endif
