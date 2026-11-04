import Foundation
import Darwin
#if os(macOS)
import CoreFoundation
import SystemConfiguration
#endif

/// Production storage is stable across source versions. Candidate storage and
/// credentials stay isolated; selecting a directory never imports legacy data.
enum CrewdFrontendConfiguration {
    #if os(macOS) && PENDINGCREW_LOCAL_DEMO
    /// A locally installed native demo never adopts environment-selected roots.
    /// The one-time typed seed writes its marker last; ordinary reopen only reads it.
    static var demoDirectory: URL? {
        guard Bundle.main.bundleIdentifier == "com.pendingname.pendingcrew.demo" else { return nil }
        let root = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/PendingCrewDemo", isDirectory: true)
        let authority = root.appendingPathComponent("authority", isDirectory: true)
        for leaf in [root, authority] {
            var part = leaf
            while true {
                var info = stat()
                guard lstat(part.path, &info) == 0, (info.st_mode & S_IFMT) == S_IFDIR else { return nil }
                if part == leaf {
                    guard info.st_uid == getuid(), info.st_mode & 0o077 == 0 else { return nil }
                }
                if part.path == "/" { break }
                part.deleteLastPathComponent()
            }
        }
        let marker = root.appendingPathComponent("demo-ready.json")
        var info = stat()
        guard lstat(marker.path, &info) == 0, (info.st_mode & S_IFMT) == S_IFREG,
              info.st_uid == getuid(), info.st_nlink == 1, info.st_size > 0, info.st_size < 8192,
              let data = try? Data(contentsOf: marker),
              let record = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              record["kind"] as? String == "pendingcrew.native-html-demo.v1",
              record["bundleIdentifier"] as? String == "com.pendingname.pendingcrew.demo",
              record["ready"] as? Bool == true else { return nil }
        // A moved or linked database must not turn a demo into another authority.
        let database = authority.appendingPathComponent("crewd.db")
        guard lstat(database.path, &info) == 0, (info.st_mode & S_IFMT) == S_IFREG,
              info.st_uid == getuid(), info.st_nlink == 1 else { return nil }
        return authority
    }
    #endif
    /// Stable across candidate rebuilds. Explicit validation roots stay outside daily storage.
    private static var candidateDirectory: URL? {
        let env = ProcessInfo.processInfo.environment
        if let path = env["PENDINGCREW_CREWD_DATA_DIR"], path.hasPrefix("/") {
            return candidateDirectory(path: path)
        }
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            .flatMap { candidateDirectory(path: $0.appendingPathComponent("PendingCrewCandidates/SQLite-Mac").path) }
    }
    /// Resolve only an existing ancestor; missing leaves remain subject to the
    /// journal's owner/mode checks. Foundation standardization rewrites Darwin's
    /// /private/tmp to /tmp, which is unsuitable for the journal's exact identity.
    static func candidateDirectory(path: String) -> URL? {
        guard path.hasPrefix("/"), !path.contains("pcw-delivery-validation.f401"),
              let canonical = canonicalCandidatePath(path),
              !canonical.contains("pcw-delivery-validation.f401") else { return nil }
        #if os(macOS)
        let daily = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/PendingCrew").path
        #else
        let daily = URL(fileURLWithPath: NSHomeDirectory(), isDirectory: true).appendingPathComponent("Library/Application Support/PendingCrew").path
        #endif
        guard let canonicalDaily = canonicalCandidatePath(daily),
              canonical != canonicalDaily, !canonical.hasPrefix(canonicalDaily + "/") else { return nil }
        return URL(fileURLWithPath: canonical, isDirectory: true)
    }
    private static func canonicalCandidatePath(_ path: String) -> String? {
        var components = [String]()
        for part in path.split(separator: "/") {
            if part == "." { continue }
            if part == ".." { if !components.isEmpty { components.removeLast() }; continue }
            components.append(String(part))
        }
        var existing = "/" + components.joined(separator: "/"), missing = [String]()
        var info = stat()
        while lstat(existing, &info) != 0 {
            guard errno == ENOENT, existing != "/" else { return nil }
            let url = URL(fileURLWithPath: existing)
            missing.insert(url.lastPathComponent, at: 0)
            existing = url.deletingLastPathComponent().path
        }
        // A selected existing leaf must be an entity directory, never a symlink.
        guard !missing.isEmpty || (info.st_mode & S_IFMT) == S_IFDIR,
              let pointer = realpath(existing, nil) else { return nil }
        defer { free(pointer) }
        let parent = String(cString: pointer)
        guard stat(parent, &info) == 0, (info.st_mode & S_IFMT) == S_IFDIR else { return nil }
        return missing.reduce(parent) { $0 == "/" ? "/" + $1 : $0 + "/" + $1 }
    }
    static var directory: URL? {
        #if os(macOS) && PENDINGCREW_LOCAL_DEMO
        return demoDirectory
        #elseif PENDINGCREW_KNOWLEDGE_VALIDATION
        return candidateDirectory
        #elseif PENDINGCREW_DELIVERY_VALIDATION
        return try? CrewdDeliveryValidation.qualified().data
        #else
        #if PENDINGCREW_NATIVE_CANDIDATE
        guard let ref = Bundle.main.object(forInfoDictionaryKey: "PendingCrewSourceRef") as? String,
              ref.count == 40, ref.allSatisfy({ "0123456789abcdef".contains($0) }) else { return nil }
        #endif
        if let explicit = ProcessInfo.processInfo.environment["PENDINGCREW_CREWD_DATA_DIR"], !explicit.isEmpty {
            #if os(macOS)
            return candidateDirectory(path: explicit)
            #else
            guard explicit.hasPrefix("/") else { return nil }
            return URL(fileURLWithPath: explicit).standardizedFileURL
            #endif
        }
        #if PENDINGCREW_NATIVE_CANDIDATE
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent("PendingCrewNativeCandidates").appendingPathComponent(ref)
        #else
        #if os(macOS)
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            .flatMap { candidateDirectory(path: $0.appendingPathComponent("PendingCrewLocal").path) }
        #else
        return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent("PendingCrewCore").standardizedFileURL
        #endif
        #endif
        #endif
    }
}

#if os(macOS)
/// Local file selection only. Never probes a CLI, login state or credentials.
enum CrewdClaudeConfiguration {
    static let key = "pendingcrew.crewd.claudeProgram.v1"
    static func program(environment: [String: String] = ProcessInfo.processInfo.environment,
                        defaults: UserDefaults = .standard) -> String? {
        #if PENDINGCREW_LOCAL_DEMO
        return "/usr/bin/false"
        #else
        if let explicit = environment["PENDINGCREW_CLAUDE_PROGRAM"], !explicit.isEmpty { return explicit }
        return defaults.string(forKey: key).flatMap { $0.isEmpty ? nil : $0 }
        #endif
    }
}

enum CrewdCodexConfiguration {
    static let key = "pendingcrew.crewd.codexProgram.v1"
    static func program(environment: [String: String] = ProcessInfo.processInfo.environment,
                        defaults: UserDefaults = .standard, home: URL = FileManager.default.homeDirectoryForCurrentUser) -> String? {
        #if PENDINGCREW_LOCAL_DEMO
        return "/usr/bin/false"
        #else
        if let explicit = environment["PENDINGCREW_CODEX_PROGRAM"], !explicit.isEmpty { return explicit }
        if let selected = defaults.string(forKey: key), !selected.isEmpty { return selected }
        let paths = [home.appendingPathComponent(".local/bin/codex").path, "/opt/homebrew/bin/codex", "/usr/local/bin/codex"] +
            (environment["PATH"] ?? "").split(separator: ":").filter { $0.hasPrefix("/") }.map { String($0) + "/codex" }
        return paths.first { executable($0) }
        #endif
    }
    static func launchArguments(directory: URL, program: String?, claudeProgram: String? = CrewdClaudeConfiguration.program(),
                                networkRoute: NetworkRoute = .init(status:.inherited)) -> [String] {
        #if PENDINGCREW_LOCAL_DEMO
        guard let fixed = CrewdFrontendConfiguration.demoDirectory, directory.path == fixed.path else {
            fputs("PendingCrew Demo: isolated data is unavailable; refusing startup.\n", stderr)
            exit(78)
        }
        return ["--data-dir", fixed.path, "serve", "--managed-parent", "--disable-claude"]
        #else
        var arguments = ["--data-dir", directory.path, "serve"]
        if let claude = enabledProgram(claudeProgram) { arguments += ["--claude", claude] }
        else { arguments.append("--disable-claude") }
        #if PENDINGCREW_LOCAL_MAC
        arguments.append("--managed-parent")
        #endif
        guard networkRoute.status.allowsCodexLaunch, let codex = enabledProgram(program) else { return arguments }
        return arguments + ["--codex", codex] + networkRoute.arguments
        #endif
    }
    /// The App selects executors through its own configuration and argv. CLI
    /// environment aliases must not conflict with or re-enable that selection.
    static func childEnvironment(_ environment: [String: String]) -> [String: String] {
        var child = environment
        child.removeValue(forKey: "CREWD_CLAUDE")
        child.removeValue(forKey: "CREWD_CODEX")
        return child
    }
    /// Retain the existing explicit isolation sentinels, but express them to
    /// Core as Disabled rather than advertising the false executable as ready.
    private static func enabledProgram(_ path: String?) -> String? {
        path.flatMap { value in
            guard value != "/usr/bin/false", value != "/bin/false", executable(value) else { return nil }
            return value
        }
    }
    static func executable(_ path: String) -> Bool {
        var directory: ObjCBool = false
        return path.hasPrefix("/") && FileManager.default.fileExists(atPath: path, isDirectory: &directory) &&
            !directory.boolValue && FileManager.default.isExecutableFile(atPath: path)
    }

    static let systemProxyKey = "pendingcrew.crewd.codexUseSystemHTTPSProxy.v1"

    enum NetworkRouteStatus: Equatable {
        case inherited, explicitEnvironment, noCodex, systemHTTPS
        case settingsUnavailable, noStaticHTTPS, unsupportedPolicy, unsupportedEndpoint

        var allowsCodexLaunch: Bool {
            switch self {
            case .inherited, .explicitEnvironment, .systemHTTPS: return true
            case .noCodex, .settingsUnavailable, .noStaticHTTPS, .unsupportedPolicy, .unsupportedEndpoint: return false
            }
        }

        var label: String {
            switch self {
            case .inherited: return "沿用启动环境。"
            case .explicitEnvironment: return "已有显式代理或绕过设置，沿用启动环境。"
            case .noCodex: return "Codex 尚未启用。"
            case .systemHTTPS: return "使用 macOS 本机 HTTPS 代理；尚未验证连通。"
            case .settingsUnavailable: return "无法读取 macOS 代理设置，Codex 停用。"
            case .noStaticHTTPS: return "macOS 未启用静态 HTTPS 代理，Codex 停用。"
            case .unsupportedPolicy: return "当前 macOS 代理包含自动、分接口或认证规则，Codex 停用。"
            case .unsupportedEndpoint: return "只支持无凭据的本机回环 HTTPS 代理，Codex 停用。"
            }
        }
    }

    struct NetworkRoute {
        let status: NetworkRouteStatus
        fileprivate var arguments: [String] = []
    }

    /// This is a local configuration read. No probe, provider change or proxy
    /// value enters App preferences, UI diagnostics, or the Core environment.
    static func networkRoute(program: String?, environment: [String: String] = ProcessInfo.processInfo.environment,
                             useSystemProxy: Bool = UserDefaults.standard.bool(forKey:systemProxyKey)) -> NetworkRoute {
        #if PENDINGCREW_LOCAL_DEMO
        return .init(status:.noCodex)
        #else
        return resolveNetworkRoute(enabled:useSystemProxy,
            codexEnabled:enabledProgram(program) != nil,environment:environment) {
            SCDynamicStoreCopyProxies(nil) as? [String:Any]
        }
        #endif
    }

    static func resolveNetworkRoute(enabled: Bool, codexEnabled: Bool, environment: [String: String],
                                    readSystem: () -> [String: Any]?) -> NetworkRoute {
        guard codexEnabled else { return .init(status:.noCodex) }
        guard enabled else { return .init(status:.inherited) }
        // The Rust per-Codex overrides win over identically named inherited
        // entries. Defer to the whole explicit policy, including empty values.
        let explicit = ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY", "WS_PROXY", "WSS_PROXY"]
        guard !environment.keys.contains(where:{ explicit.contains($0.uppercased()) }) else {
            return .init(status:.explicitEnvironment)
        }
        guard let system = readSystem() else { return .init(status:.settingsUnavailable) }
        guard staticProxyPolicy(system) else { return .init(status:.unsupportedPolicy) }
        guard exactInteger(system["HTTPSEnable"]) == 1 else { return .init(status:.noStaticHTTPS) }
        guard let host = system["HTTPSProxy"] as? String, let literal = loopbackAuthority(host),
              let port = exactInteger(system["HTTPSPort"]), (1...65_535).contains(port) else {
            return .init(status:.unsupportedEndpoint)
        }
        if let scoped = system["__SCOPED__"] {
            // SystemConfiguration can repeat the same static route for its
            // interfaces. Accept exact mirrors, never a competing route.
            guard let interfaces = scoped as? [String:Any], interfaces.values.allSatisfy({ value in
                guard let settings = value as? [String:Any], staticProxyPolicy(settings), settings["__SCOPED__"] == nil,
                      exactInteger(settings["HTTPSEnable"]) == 1, exactInteger(settings["HTTPSPort"]) == port,
                      let scopedHost = settings["HTTPSProxy"] as? String else { return false }
                return loopbackAuthority(scopedHost) == literal
            }) else { return .init(status:.unsupportedPolicy) }
        }
        // This explicit mode borrows the address only. macOS and NO_PROXY
        // have different domain matching rules. Use the disclosed local list.
        let bypass = "localhost,127.0.0.1,::1"
        let url = "http://\(literal):\(port)"
        let values = ["HTTPS_PROXY=\(url)", "https_proxy=\(url)", "NO_PROXY=\(bypass)", "no_proxy=\(bypass)"]
        return .init(status:.systemHTTPS,arguments:values.flatMap { ["--codex-env",$0] })
    }

    private static func staticProxyPolicy(_ settings: [String:Any]) -> Bool {
        let automatic = ["ProxyAutoConfigEnable","ProxyAutoDiscoveryEnable"]
        return automatic.allSatisfy { settings[$0] == nil || exactInteger(settings[$0]) == 0 } &&
            settings["__SUPPLEMENTAL__"] == nil &&
            (settings["HTTPSUser"] == nil || (settings["HTTPSUser"] as? String) == "")
    }

    private static func exactInteger(_ value: Any?) -> Int? {
        guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID(),
              let integer = Int(number.stringValue) else { return nil }
        return integer
    }

    private static func ipv4(_ raw: String) -> [UInt8]? {
        let parts = raw.split(separator:".",omittingEmptySubsequences:false)
        let bytes = parts.compactMap { part in UInt8(part).flatMap { String($0) == part ? $0 : nil } }
        return parts.count == 4 && bytes.count == 4 ? bytes : nil
    }

    private static func ipv6(_ raw: String) -> in6_addr? {
        var address = in6_addr()
        return raw.withCString { inet_pton(AF_INET6,$0,&address) } == 1 ? address : nil
    }

    private static func loopbackAuthority(_ host: String) -> String? {
        guard !host.utf8.contains(0) else { return nil }
        if let bytes = ipv4(host), bytes[0] == 127 { return host }
        let raw = host.hasPrefix("[") && host.hasSuffix("]") ? String(host.dropFirst().dropLast()) : host
        guard raw.utf8.allSatisfy({ (48...58).contains($0) || (65...70).contains($0) || (97...102).contains($0) }),
              var address = ipv6(raw), withUnsafeBytes(of:&address,{ Array($0) }) == Array(repeating:UInt8(0),count:15) + [1] else { return nil }
        return "[::1]"
    }

}
#endif
