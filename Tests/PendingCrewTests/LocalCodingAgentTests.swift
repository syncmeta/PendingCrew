#if os(macOS)
import XCTest
// 注：LocalRunner 源码直接通过 project.yml 编进 PendingCrewTests target
// （见 project.yml 的注释），所以不需要 @testable import —— 类型已经在
// 当前 module 内可见，internal 访问级也直接可用。

final class LocalCodingAgentTests: XCTestCase {

    // MARK: - env whitelist (LocalCodingAgentEnv)

    func testEnvWhitelistDropsForbiddenKeys() {
        let env = LocalCodingAgentEnv.build(additionalEnv: [
            "ANTHROPIC_API_KEY": "sk-test",
            // 防御：caller 误传 PendingCrew 自己的 secret 也应被丢弃。
            // SUPABASE_ 是前缀封禁——新旧 key 命名都不许过。
            "PENDINGBOT_DEVICE_GRANT": "leaked",
            "PENDINGCREW_KEYCHAIN_TOKEN": "leaked",
            "SUPABASE_SERVICE_ROLE_KEY": "leaked",
            "SUPABASE_SECRET_KEY": "leaked",
            "SUPABASE_PUBLISHABLE_KEY": "leaked",
        ])
        XCTAssertEqual(env["ANTHROPIC_API_KEY"], "sk-test")
        XCTAssertNil(env["PENDINGBOT_DEVICE_GRANT"])
        XCTAssertNil(env["PENDINGCREW_KEYCHAIN_TOKEN"])
        XCTAssertNil(env["SUPABASE_SERVICE_ROLE_KEY"])
        XCTAssertNil(env["SUPABASE_SECRET_KEY"])
        XCTAssertNil(env["SUPABASE_PUBLISHABLE_KEY"])
        // 白名单基础项必在
        XCTAssertNotNil(env["PATH"], "PATH must always be set for child process")
    }

    func testEnvWhitelistOnlyPassesThroughKnownKeys() {
        // 验：PendingCrew 父进程 env 里非白名单的 key 不会泄给子进程。
        let parent = ProcessInfo.processInfo.environment
        let env = LocalCodingAgentEnv.build(additionalEnv: [:])
        let nonWhitelistedParent = parent.keys.first { key in
            !LocalCodingAgentEnv.passthroughKeys.contains(key)
                && !key.hasPrefix("LC_")
                && !key.isEmpty
        }
        if let leak = nonWhitelistedParent {
            XCTAssertNil(env[leak],
                         "non-whitelisted parent env var '\(leak)' leaked into child env")
        }
    }

    // MARK: - billing guard (codex strips OPENAI_API_KEY)

    func testCodexEnvOmitsOpenAIApiKeyForSubscriptionBilling() {
        // An exported OPENAI_API_KEY would flip app-server to pay-as-you-go despite a
        // chatgpt auth.json. Guard: codex env must never carry it.
        let env = LocalCodingAgentEnv.build(additionalEnv: ["OPENAI_API_KEY": "sk-xxx"], kind: .codex)
        XCTAssertNil(env["OPENAI_API_KEY"])
    }

    func testClaudeEnvStillAllowsCallerKeys() {
        let env = LocalCodingAgentEnv.build(additionalEnv: ["ANTHROPIC_API_KEY": "x"], kind: .claudeCode)
        XCTAssertEqual(env["ANTHROPIC_API_KEY"], "x")
    }

    // MARK: - executable resolution

    func testResolveAgainstDiscoverAvailableIsConsistent() {
        let available = LocalCodingAgentExecutable.discoverAvailable()
        for kind in available {
            XCTAssertNotNil(LocalCodingAgentExecutable.resolve(kind),
                            "discovered kind \(kind) must be re-resolvable")
        }
        for kind in LocalCodingAgentKind.allCases where !available.contains(kind) {
            XCTAssertNil(LocalCodingAgentExecutable.resolve(kind),
                         "kind \(kind) was filtered out of discoverAvailable but resolve still returned non-nil")
        }
    }

    // MARK: - 子进程 PATH：定位与运行必须同一条

    /// 定位可执行文件用的目录，一个不落地进子进程 PATH。这就是「找得到 codex，
    /// 但 codex 找不到 node」那类不对称的根：npm/nvm 装的 CLI 是
    /// `#!/usr/bin/env node` 脚本，运行时 PATH 里没 node 就直接 shebang 失败秒退。
    func testChildPathContainsEverySearchDirectory() {
        let search = ["/opt/nvm/v22/bin", "/Users/x/.local/bin"]
        let path = LocalCodingAgentExecutable.composeChildPath(
            searchDirs: search, parentPath: "/usr/bin:/bin")
        let entries = path.split(separator: ":").map(String.init)
        for dir in search {
            XCTAssertTrue(entries.contains(dir), "搜索目录 \(dir) 没进子进程 PATH")
        }
    }

    /// 只增不减：父进程 PATH 里原有的目录一个都不许丢（避免这次改动把本来能用的
    /// 环境弄坏），且搜索目录排在前面（CLI 在哪找到的，它的运行时多半也在那）。
    func testChildPathKeepsParentEntriesAndPrefersSearchDirs() {
        let path = LocalCodingAgentExecutable.composeChildPath(
            searchDirs: ["/opt/rich/bin"], parentPath: "/only/in/parent:/usr/bin")
        let entries = path.split(separator: ":").map(String.init)
        XCTAssertTrue(entries.contains("/only/in/parent"))
        XCTAssertEqual(entries.first, "/opt/rich/bin")
        XCTAssertEqual(Set(entries).count, entries.count, "PATH 不该有重复项")
    }

    /// 登录 shell 抓失败（searchDirs 只剩用户级 bin）时，系统目录必须兜住 ——
    /// 没有 `/usr/bin` 的 PATH 会让子进程连 `env` / `git` 都跑不了。
    func testChildPathAlwaysCarriesSystemDirectories() {
        let path = LocalCodingAgentExecutable.composeChildPath(
            searchDirs: ["/Users/x/.local/bin"], parentPath: nil)
        let entries = path.split(separator: ":").map(String.init)
        for dir in ["/usr/bin", "/bin"] {
            XCTAssertTrue(entries.contains(dir), "系统目录 \(dir) 必须兜底")
        }
    }

    /// 实际构造的子进程 env 走的就是这条 —— 钉住接线，别哪天又退回只读父 PATH。
    func testBuiltEnvUsesTheResolvedChildPath() {
        let env = LocalCodingAgentEnv.build(additionalEnv: [:], kind: .codex)
        XCTAssertEqual(env["PATH"], LocalCodingAgentExecutable.childProcessPath)
    }

    // MARK: - node 工具链版本目录排序

    /// nvm/fnm 的版本目录必须按**数值**倒序，不能按字符串序 —— 字符串序会把
    /// "v9" 排在 "v22" 前面，于是老 node 抢在新 node 前面进 PATH。
    func testVersionedNodeBinDirsSortNumericallyNewestFirst() {
        let dirs = LocalCodingAgentExecutable.versionedNodeBinDirs(
            root: "/nvm", versions: ["v9.11.2", "v22.14.0", "v24.18.0", "v24.4.0"],
            binSubpath: "bin")
        XCTAssertEqual(dirs, [
            "/nvm/v24.18.0/bin",
            "/nvm/v24.4.0/bin",
            "/nvm/v22.14.0/bin",
            "/nvm/v9.11.2/bin",
        ])
    }

    /// fnm 的布局多一层 `installation`，同一套排序逻辑要能拼对路径。
    func testVersionedNodeBinDirsHonorsNestedBinSubpath() {
        let dirs = LocalCodingAgentExecutable.versionedNodeBinDirs(
            root: "/fnm", versions: ["v20.1.0"], binSubpath: "installation/bin")
        XCTAssertEqual(dirs, ["/fnm/v20.1.0/installation/bin"])
    }
}
#endif
