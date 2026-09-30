#if os(macOS)
import Darwin
import XCTest

final class AgentCLIMaintenanceTests: XCTestCase {
    /// #188：缺少 Codex CLI 不能只落成一条通用错误；必须有可见状态，并且
    /// 安装能力只能接收我们定义的固定计划，绝不能把消息正文当命令执行。
    ///
    /// 这是源码接线回归：实现仍在同一个 LocalRunner 源文件中，以免新增 pbx
    /// 文件时先把测试工程漂移问题混进功能红证。
    func testCodexProvisioningHasVisibleStateAndTypedOfficialAction() throws {
        let testURL = URL(fileURLWithPath: #filePath)
        let root = testURL.deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let source = try String(contentsOf: root.appendingPathComponent(
            "Sources/Mac/LocalRunner/AgentCLIMaintenance.swift"))

        XCTAssertTrue(source.contains("enum CodexCLIProvisioningState"),
                      "缺 CLI 仍只有通用错误，UI 没有可靠的可见状态")
        XCTAssertTrue(source.contains("struct CodexCLIInstallAction"),
                      "安装动作没有类型边界，容易把消息正文误当命令")
        XCTAssertTrue(source.contains("officialRegistry"),
                      "安装来源不是固定的官方 registry")
        XCTAssertTrue(source.contains("--ignore-scripts"),
                      "npm 生命周期脚本没有被明确禁止")
        XCTAssertTrue(source.contains("requiresSecondConfirmation"),
                      "一键安装没有二次确认边界")
    }

    func testCodexProvisioningStateIsConnectedToSettingsUI() throws {
        let testURL = URL(fileURLWithPath: #filePath)
        let root = testURL.deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let center = try String(contentsOf: root.appendingPathComponent(
            "Sources/Mac/Services/AgentCLIVersionCenter.swift"))
        let view = try String(contentsOf: root.appendingPathComponent(
            "Sources/Mac/Views/AgentCLIVersionView.swift"))

        XCTAssertTrue(center.contains("codexProvisioningState"),
                      "缺失状态没有发布给设置界面")
        XCTAssertTrue(center.contains("prepareCodexInstall"),
                      "首次点击没有独立的零执行准备边界")
        XCTAssertTrue(center.contains("installCodex"),
                      "二次确认没有独立的执行入口")
        XCTAssertTrue(view.contains("查看官方安装指引"),
                      "CLI 缺失时设置界面没有官方指引")
        XCTAssertTrue(view.contains("确认安装官方 Codex CLI？"),
                      "安装前没有第二次确认对话框")
    }

    func testManagedCLIRefreshKeepsAuthenticationUnknownState() throws {
        let testURL = URL(fileURLWithPath: #filePath)
        let root = testURL.deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let center = try String(contentsOf: root.appendingPathComponent(
            "Sources/Mac/Services/AgentCLIVersionCenter.swift"))

        XCTAssertTrue(center.contains("managedInstallation(for: value)"),
                      "受管 Codex CLI 的普通 refresh 会擦掉认证未知状态")
        XCTAssertTrue(center.contains(".installedNeedsManualSignIn"),
                      "受管 Codex CLI 刷新后没有保留认证未知的显式状态")
    }

    func testProvisioningPrepareIsOfflineAndReturnsOnlyFixedOfficialAction() throws {
        try provisioningFixture { service, fakeNpm, _, _ in
            var processCalls = 0
            var installCalls = 0
            var prepared = service
            prepared.processExecute = { _, _, _ in
                processCalls += 1
                return .init(status: 0, output: "1 /sbin/launchd\n", logURL: nil)
            }
            prepared.execute = { _, _, _, _ in
                installCalls += 1
                return .init(status: 0, output: "", logURL: nil)
            }

            XCTAssertEqual(prepared.currentState(), .missing(.init()))
            let action = try prepared.prepareOfficialInstall()
            XCTAssertEqual(action.packageManager, fakeNpm)
            XCTAssertEqual(action.registry, "https://registry.npmjs.org/")
            XCTAssertEqual(action.package, "@openai/codex@0.159.1")
            XCTAssertEqual(action.target.standardizedFileURL.path,
                           prepared.managedRoot.appendingPathComponent("0.159.1", isDirectory: true).standardizedFileURL.path)
            XCTAssertEqual(action.cache.standardizedFileURL.path,
                           prepared.managedRoot.appendingPathComponent("npm-cache", isDirectory: true).standardizedFileURL.path)
            XCTAssertEqual(action.arguments, [
                "install", "--prefix", action.target.path,
                "--cache", action.cache.path,
                "--userconfig", "/dev/null", "--globalconfig", "/dev/null",
                "--registry", "https://registry.npmjs.org/",
                "--ignore-scripts", "--no-audit", "--no-fund", "@openai/codex@0.159.1",
            ])
            XCTAssertTrue(action.requiresSecondConfirmation)
            XCTAssertEqual(processCalls, 0, "首次点击不得扫描/启动安装")
            XCTAssertEqual(installCalls, 0, "首次点击不得联网或运行 npm")
        }
    }

    func testProvisioningConfirmedActionUsesFixedNpmThenVerifiesWithoutAuth() throws {
        try provisioningFixture { service, _, root, _ in
            var prepared = service
            let action = try prepared.prepareOfficialInstall()
            var calls: [(URL, [String], [String: String])] = []
            prepared.processExecute = { _, _, _ in
                .init(status: 0, output: "1 /sbin/launchd\n", logURL: nil)
            }
            prepared.execute = { executable, arguments, _, environment in
                calls.append((executable, arguments, environment))
                if arguments.first == "install" {
                    let binary = action.target.appendingPathComponent("node_modules/@openai/codex/bin/codex")
                    try FileManager.default.createDirectory(at: binary.deletingLastPathComponent(), withIntermediateDirectories: true)
                    try Data("fixture".utf8).write(to: binary)
                    try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: binary.path)
                    let link = action.target.appendingPathComponent("node_modules/.bin/codex")
                    try FileManager.default.createDirectory(at: link.deletingLastPathComponent(), withIntermediateDirectories: true)
                    try FileManager.default.createSymbolicLink(atPath: link.path,
                                                                withDestinationPath: "../@openai/codex/bin/codex")
                    return .init(status: 0, output: "installed", logURL: root.appendingPathComponent("install.log"))
                }
                XCTAssertEqual(executable.path, action.target.appendingPathComponent("node_modules/.bin/codex").path)
                XCTAssertEqual(arguments, ["--version"])
                return .init(status: 0, output: "codex-cli 0.159.1", logURL: root.appendingPathComponent("verify.log"))
            }

            let installed = try prepared.installConfirmed(action)
            XCTAssertEqual(installed.version, "0.159.1")
            XCTAssertEqual(installed.executable, action.target.appendingPathComponent("node_modules/.bin/codex"))
            XCTAssertEqual(calls.count, 2)
            XCTAssertEqual(calls[0].0, action.packageManager)
            XCTAssertEqual(calls[0].1, action.arguments)
            XCTAssertEqual(calls[1].1, ["--version"])
            XCTAssertEqual(calls[0].2["npm_config_registry"], "https://registry.npmjs.org/")
            XCTAssertEqual(calls[0].2["npm_config_cache"], action.cache.path)
            XCTAssertEqual(calls[0].2["npm_config_ignore_scripts"], "true")
            XCTAssertEqual(calls[0].2["HOME"], prepared.home.path)
            XCTAssertNil(calls[0].2["OPENAI_API_KEY"])
            XCTAssertFalse(calls.flatMap { $0.1 }.contains("login"), "安装链不得自动登录")

            let inspected = AgentCLIInstallation(
                kind: .codex,
                executable: installed.executable,
                resolved: installed.executable.resolvingSymlinksInPath(),
                version: installed.version,
                native: false,
                rollbackReleases: [],
                checkedAt: .now)
            XCTAssertEqual(
                CodexCLIProvisioningService.managedInstallation(for: inspected, root: prepared.managedRoot),
                installed,
                "后续普通 refresh 也必须保留受管 CLI 的认证未知状态")
        }
    }

    func testProvisioningRefusesLiveCodexBeforeNpm() throws {
        try provisioningFixture { service, _, _, _ in
            var prepared = service
            let action = try prepared.prepareOfficialInstall()
            var npmRan = false
            prepared.processExecute = { _, _, _ in
                .init(status: 0, output: "24 codex app-server\n", logURL: nil)
            }
            prepared.execute = { _, _, _, _ in
                npmRan = true
                return .init(status: 0, output: "", logURL: nil)
            }

            XCTAssertThrowsError(try prepared.installConfirmed(action)) {
                XCTAssertTrue($0.localizedDescription.contains("24"))
            }
            XCTAssertFalse(npmRan, "有存活 Codex 时不得启动 npm")
        }
    }

    func testProvisioningFailedNpmNeverClaimsInstalledOrRetries() throws {
        try provisioningFixture { service, _, _, _ in
            var prepared = service
            let action = try prepared.prepareOfficialInstall()
            var calls = 0
            prepared.processExecute = { _, _, _ in
                .init(status: 0, output: "1 /sbin/launchd\n", logURL: nil)
            }
            prepared.execute = { _, _, _, _ in
                calls += 1
                return .init(status: 17, output: "fake package manager failed", logURL: nil)
            }

            XCTAssertThrowsError(try prepared.installConfirmed(action)) {
                XCTAssertTrue($0.localizedDescription.contains("17"))
                XCTAssertTrue($0.localizedDescription.contains("fake package manager failed"))
            }
            XCTAssertEqual(calls, 1, "失败后不自动重试或继续版本探测")
            XCTAssertNil(CodexCLIProvisioningService.managedExecutable(root: prepared.managedRoot))
        }
    }

    /// #188 permission boundary: before the package manager can download or run
    /// anything, the managed root, version target and npm cache must all be
    /// private to this user. A compromised or merely group-writable directory is
    /// not a safe place to later execute the downloaded CLI from.
    func testProvisioningCreatesPrivateRootTargetAndDedicatedCacheBeforeNpm() throws {
        try provisioningFixture { service, _, _, _ in
            var prepared = service
            let action = try prepared.prepareOfficialInstall()
            let cache = prepared.managedRoot.appendingPathComponent("npm-cache", isDirectory: true)
            prepared.processExecute = { _, _, _ in
                .init(status: 0, output: "1 /sbin/launchd\n", logURL: nil)
            }
            prepared.execute = { _, arguments, _, environment in
                guard arguments.first == "install" else {
                    return .init(status: 0, output: "codex-cli 0.159.1", logURL: nil)
                }
                for directory in [prepared.managedRoot, action.target, cache] {
                    let attributes = try FileManager.default.attributesOfItem(atPath: directory.path)
                    let mode = try XCTUnwrap(attributes[.posixPermissions] as? NSNumber).intValue & 0o777
                    XCTAssertEqual(mode, 0o700, "受管目录必须是当前用户私有的 0700：\(directory.path)")
                    XCTAssertEqual((try XCTUnwrap(attributes[.ownerAccountID] as? NSNumber)).uint32Value,
                                   UInt32(geteuid()),
                                   "受管目录不能由别的本机用户拥有：\(directory.path)")
                }
                XCTAssertEqual(environment["npm_config_cache"], cache.path,
                               "npm 不能落回共享的 HOME cache")
                return .init(status: 17, output: "injected npm failure", logURL: nil)
            }

            XCTAssertThrowsError(try prepared.installConfirmed(action))
            XCTAssertTrue(FileManager.default.fileExists(atPath: action.target.path),
                          "失败产物必须留在固定 target，供人工核对；不能悄悄清理或重试")
            XCTAssertTrue(FileManager.default.fileExists(atPath: cache.path),
                          "失败后的私有 npm cache 必须保留，不能自动改用 HOME cache")
        }
    }

    func testProvisioningRejectsSymlinkedOrGroupWritableManagedRootBeforeNpm() throws {
        try provisioningFixture { service, _, root, _ in
            for insecureRoot in ["symlink", "group-writable"] {
                var prepared = service
                let action = try prepared.prepareOfficialInstall()
                if insecureRoot == "symlink" {
                    let destination = root.appendingPathComponent("outside")
                    try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
                    try FileManager.default.createSymbolicLink(at: prepared.managedRoot,
                                                                withDestinationURL: destination)
                } else {
                    try FileManager.default.createDirectory(at: prepared.managedRoot, withIntermediateDirectories: true)
                    try FileManager.default.setAttributes([.posixPermissions: 0o770],
                                                          ofItemAtPath: prepared.managedRoot.path)
                }
                var npmRan = false
                prepared.processExecute = { _, _, _ in
                    .init(status: 0, output: "1 /sbin/launchd\n", logURL: nil)
                }
                prepared.execute = { _, _, _, _ in
                    npmRan = true
                    return .init(status: 0, output: "unexpected", logURL: nil)
                }

                XCTAssertThrowsError(try prepared.installConfirmed(action),
                                     "\(insecureRoot) root 必须在 npm 之前被拒绝")
                XCTAssertFalse(npmRan, "不安全 root 下不得调用 npm：\(insecureRoot)")
                try? FileManager.default.removeItem(at: prepared.managedRoot)
            }
        }
    }

    func testProvisioningRejectsDanglingTargetOrCacheSymlinkBeforeNpm() throws {
        try provisioningFixture { service, _, root, _ in
            for unsafeName in ["target", "cache"] {
                var prepared = service
                let action = try prepared.prepareOfficialInstall()
                try FileManager.default.createDirectory(at: prepared.managedRoot,
                                                        withIntermediateDirectories: true,
                                                        attributes: [.posixPermissions: 0o700])
                let unsafe = unsafeName == "target" ? action.target : action.cache
                try FileManager.default.createSymbolicLink(atPath: unsafe.path,
                                                            withDestinationPath: root.appendingPathComponent("missing").path)
                var npmRan = false
                prepared.processExecute = { _, _, _ in
                    .init(status: 0, output: "1 /sbin/launchd\n", logURL: nil)
                }
                prepared.execute = { _, _, _, _ in
                    npmRan = true
                    return .init(status: 0, output: "unexpected", logURL: nil)
                }

                XCTAssertThrowsError(try prepared.installConfirmed(action),
                                     "\(unsafeName) symlink 必须在 npm 之前被拒绝")
                XCTAssertFalse(npmRan, "受管 \(unsafeName) 不得跟随 symlink 后再运行 npm")
                try? FileManager.default.removeItem(at: prepared.managedRoot)
            }
        }
    }

    func testManagedExecutableRejectsPostInstallWritableRootOrTarget() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let target = root.appendingPathComponent("0.159.1", isDirectory: true)
        let binary = target.appendingPathComponent("node_modules/@openai/codex/bin/codex")
        try FileManager.default.createDirectory(at: binary.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("fixture".utf8).write(to: binary)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: binary.path)
        let link = target.appendingPathComponent("node_modules/.bin/codex")
        try FileManager.default.createDirectory(at: link.deletingLastPathComponent(), withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(atPath: link.path,
                                                    withDestinationPath: "../@openai/codex/bin/codex")
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: root.path)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: target.path)
        XCTAssertNotNil(CodexCLIProvisioningService.managedExecutable(root: root))

        try FileManager.default.setAttributes([.posixPermissions: 0o770], ofItemAtPath: target.path)
        XCTAssertNil(CodexCLIProvisioningService.managedExecutable(root: root),
                     "后来变成 group-writable 的 version target 不能再被当作可执行 CLI")
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: target.path)
        try FileManager.default.setAttributes([.posixPermissions: 0o707], ofItemAtPath: root.path)
        XCTAssertNil(CodexCLIProvisioningService.managedExecutable(root: root),
                     "后来变成 others-writable 的 managed root 不能再被当作可执行 CLI")
    }

    func testManagedProvisioningRejectsSymlinkEscapingItsFixedTarget() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let link = root.appendingPathComponent("0.159.1/node_modules/.bin/codex")
        try FileManager.default.createDirectory(at: link.deletingLastPathComponent(), withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(atPath: link.path, withDestinationPath: "/bin/sh")
        XCTAssertNil(CodexCLIProvisioningService.managedExecutable(root: root))
    }

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

    private func provisioningFixture(
        _ body: (CodexCLIProvisioningService, URL, URL, URL) throws -> Void
    ) throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let fakeNpm = root.appendingPathComponent("tools/npm")
        try FileManager.default.createDirectory(at: fakeNpm.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("fixture".utf8).write(to: fakeNpm)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: fakeNpm.path)
        var service = CodexCLIProvisioningService()
        service.home = root.appendingPathComponent("home")
        service.managedRoot = root.appendingPathComponent("managed")
        service.lockDirectory = root.appendingPathComponent("locks")
        service.resolveCodex = { nil }
        service.locateNpm = { fakeNpm }
        try body(service, fakeNpm, root, service.managedRoot)
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
