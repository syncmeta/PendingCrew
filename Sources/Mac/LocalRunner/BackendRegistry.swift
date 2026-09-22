#if os(macOS)
import Foundation

/// 「我认识哪些后端」—— 管理入口的模型层（人类 Todo #121：这套前端可以直接连接其它后端）。
///
/// 人类原话：「我希望 pendingcrew 要有管理后端的能力 **本机的后端也是一个** 要能管理
/// 这些的更新」。那个「也」字是这一层的全部设计约束：
///
/// - **本机那条不是特例，是列表里的第 0 条**。它走同一套状态查询、同一个更新流程、
///   同一个停用入口。给它开特例的那一刻，「管理后端」就退化成「管理本机后端」，
///   而以后加远程就得把整层重写。
/// - **它也不许被删掉**：删了之后这个 app 就没有任何后端可连，界面上却看不出为什么。
///
/// ## 远程这一档：安全地址与配对信任缺一不可
///
/// `.remote` 只接受 `pendingcrew+tls://host:port`，并且必须有同 backend id 的对端信任
/// 记录。任何解析、信任账本或握手失败都原样失败，**绝不静默降级成本机**。静默降级的
/// 症状是：人填了一个远程地址，界面显示「已连接」，而他看到的其实是自己这台机器上的
/// session。那种错人是查不出来的。
///
/// ## 读不出来 ≠ 空列表
///
/// 存盘的那份读不动时（权限、半个文件），**不许当成「一个后端都没登记过」**——
/// 那会让界面显示一个干净的空列表，而人刚刚明明加过三个。三态：
/// 文件不在（真的没加过）/ 读不动（说出来）/ 读到了。
struct BackendRef: Codable, Equatable, Identifiable {
    enum Transport: Equatable {
        /// 本机后端：Unix socket。
        case localSocket(path: String)
        /// 远程后端。地址必须是 `pendingcrew+tls://host:port`。
        case remote(url: String)
    }

    var id: String
    var displayName: String
    var transport: Transport
    /// 内置那条（本机）。**不可删、永远排第一。**
    var isBuiltIn: Bool = false

    var isRemote: Bool { if case .remote = transport { return true }; return false }

    // MARK: - Codable（Transport 是带负载的 enum，手写以免对存盘格式失控）

    private enum CodingKeys: String, CodingKey {
        case id, displayName, kind, address, isBuiltIn
    }

    init(id: String, displayName: String, transport: Transport, isBuiltIn: Bool = false) {
        self.id = id
        self.displayName = displayName
        self.transport = transport
        self.isBuiltIn = isBuiltIn
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        displayName = try c.decode(String.self, forKey: .displayName)
        isBuiltIn = try c.decodeIfPresent(Bool.self, forKey: .isBuiltIn) ?? false
        let address = try c.decode(String.self, forKey: .address)
        switch try c.decode(String.self, forKey: .kind) {
        case "localSocket": transport = .localSocket(path: address)
        case "remote": transport = .remote(url: address)
        case let other:
            throw DecodingError.dataCorruptedError(
                forKey: .kind, in: c, debugDescription: "不认识的后端类型：\(other)")
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(displayName, forKey: .displayName)
        try c.encode(isBuiltIn, forKey: .isBuiltIn)
        switch transport {
        case let .localSocket(path):
            try c.encode("localSocket", forKey: .kind)
            try c.encode(path, forKey: .address)
        case let .remote(url):
            try c.encode("remote", forKey: .kind)
            try c.encode(url, forKey: .address)
        }
    }
}

enum BackendRegistry {
    /// 本机那条的固定 id。
    static let localId = "local"

    static var registryFile: URL {
        PendingCrewDataRoot.subdirectory("backends").appendingPathComponent("registry.json")
    }

    static var selectionFile: URL {
        PendingCrewDataRoot.subdirectory("backends").appendingPathComponent("selection.json")
    }

    private struct SelectionRecord: Codable, Equatable { var backendID: String }

    enum Selection: Equatable {
        case selected(BackendRef)
        /// A previously explicit choice cannot be resolved.  This is intentionally not local.
        case unavailable(String)

        var backendID: String? {
            if case let .selected(ref) = self { return ref.id }
            return nil
        }
    }

    /// Resolve the persisted viewer target.  No selection file means the installation has never
    /// selected a backend and therefore starts on the built-in local entry.  Once a selection was
    /// written, unreadable/missing data fails closed instead of silently changing machines.
    static func selectedBackend(
        registryFile: URL = BackendRegistry.registryFile,
        selectionFile: URL = BackendRegistry.selectionFile,
        paths: PendingCrewDaemonPaths = .standard()
    ) -> Selection {
        let data: Data
        do {
            data = try Data(contentsOf: selectionFile)
        } catch let error as NSError
            where error.domain == NSCocoaErrorDomain && error.code == NSFileReadNoSuchFileError {
            return .selected(builtInLocal(paths: paths))
        } catch {
            return .unavailable("后端选择读不出来：\(error.localizedDescription)。没有退回本机。")
        }
        guard let record = try? JSONDecoder().decode(SelectionRecord.self, from: data),
              !record.backendID.isEmpty else {
            return .unavailable("后端选择已损坏。没有退回本机。")
        }
        let loaded = load(from: registryFile, paths: paths)
        if let problem = loaded.problem {
            return .unavailable("\(problem) 当前选择是 \(record.backendID)，没有退回本机。")
        }
        guard let ref = loaded.refs.first(where: { $0.id == record.backendID }) else {
            return .unavailable("之前选择的后端 \(record.backendID) 已不在登记表中。没有退回本机。")
        }
        return .selected(ref)
    }

    static func select(_ ref: BackendRef,
                       selectionFile: URL = BackendRegistry.selectionFile) throws {
        try MultiProcessJSONStore.writeStaged(
            JSONEncoder().encode(SelectionRecord(backendID: ref.id)), to: selectionFile)
    }

    /// 本机那条。**每次现算**（socket 路径跟着数据根走，写死会在
    /// `PENDINGCREW_DATA_DIR` 挪走之后指向错的地方）。
    static func builtInLocal(paths: PendingCrewDaemonPaths = .standard()) -> BackendRef {
        BackendRef(id: localId, displayName: "本机",
                   transport: .localSocket(path: paths.socket), isBuiltIn: true)
    }

    /// 读出来的东西 —— **三态**，别压成一个数组。
    enum Load: Equatable {
        /// 没存过（全新机器）。列表 = 只有本机那条。
        case fresh([BackendRef])
        case loaded([BackendRef])
        /// 存盘那份读不动 / 解不开。**列表仍然给本机那条**（否则界面会显示
        /// 「一个后端都没有」），但必须把原因带出去让人看见。
        case degraded([BackendRef], reason: String)

        var refs: [BackendRef] {
            switch self {
            case let .fresh(r), let .loaded(r), let .degraded(r, _): return r
            }
        }
        var problem: String? {
            if case let .degraded(_, reason) = self { return reason }
            return nil
        }
    }

    /// 归一化：**本机那条永远在、永远第一、永远是内置的**，其余按登记顺序，
    /// 重复 id 只留第一个。
    static func normalize(_ stored: [BackendRef],
                          paths: PendingCrewDaemonPaths = .standard()) -> [BackendRef] {
        var seen: Set<String> = [localId]
        var out = [builtInLocal(paths: paths)]
        for ref in stored where seen.insert(ref.id).inserted {
            // 存盘里混进来的 isBuiltIn 不作数 —— 内置只有一条，由我们说了算。
            var ref = ref
            ref.isBuiltIn = false
            out.append(ref)
        }
        return out
    }

    static func load(from url: URL, paths: PendingCrewDaemonPaths = .standard()) -> Load {
        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch let error as NSError
            where error.domain == NSCocoaErrorDomain
                && error.code == NSFileReadNoSuchFileError {
            return .fresh(normalize([], paths: paths))
        } catch {
            return .degraded(normalize([], paths: paths),
                             reason: "后端列表读不出来（\(error.localizedDescription)）——"
                                 + "下面只列出本机那条，你之前加过的还在盘上，没有被删。")
        }
        guard let stored = try? JSONDecoder().decode([BackendRef].self, from: data) else {
            return .degraded(normalize([], paths: paths),
                             reason: "后端列表解不开 —— 下面只列出本机那条，原文件没有被改动。")
        }
        return .loaded(normalize(stored, paths: paths))
    }

    /// 存盘。**内置那条不写进去** —— 它每次现算，写进去只会在数据根挪走之后变成
    /// 一条指向旧路径的假记录。
    static func save(_ refs: [BackendRef], to url: URL) throws {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try MultiProcessJSONStore.writeStaged(
            encoder.encode(refs.filter { !$0.isBuiltIn }), to: url)
    }

    /// 删一条。**内置那条删不得**，而且要说清为什么，不是静默忽略。
    enum RemoveResult: Equatable {
        case removed([BackendRef])
        case refused(String)
    }

    static func removing(_ id: String, from refs: [BackendRef]) -> RemoveResult {
        guard let ref = refs.first(where: { $0.id == id }) else {
            return .refused("列表里没有这个后端：\(id)")
        }
        guard !ref.isBuiltIn else {
            return .refused("「\(ref.displayName)」是内置的本机后端，删不掉 ——"
                + "删了之后这个界面就没有任何后端可连了。")
        }
        return .removed(refs.filter { $0.id != id })
    }

    enum RemoteConnectionError: Error, Equatable, CustomStringConvertible {
        case insecureScheme(String?)
        case invalidAddress(String)
        case trustStoreUnreadable(String)
        case notPaired(String)
        case invalidTrust(String)
        case localSocketIsNotRemote

        var description: String {
            switch self {
            case let .insecureScheme(scheme):
                return "远程地址必须使用 pendingcrew+tls 安全通道，不能用 \(scheme ?? "空")"
            case let .invalidAddress(address):
                return "远程地址无效（必须带主机和端口）：\(address)"
            case let .trustStoreUnreadable(reason):
                return "对端信任账本读不出来：\(reason)"
            case let .notPaired(id):
                return "远程后端 \(id) 还没有配对信任记录"
            case let .invalidTrust(id):
                return "远程后端 \(id) 的配对信任记录无效"
            case .localSocketIsNotRemote:
                return "本机 socket 不是远程连接"
            }
        }
    }

    /// 能不能连它。远程必须同时通过地址解析和信任记录验证；失败绝不降级成本机。
    enum Connectivity: Equatable {
        case supported
        case unsupported(String)
    }

    static func connectivity(of ref: BackendRef,
                             trustedPeers suppliedPeers: [PeerTrustRecord]? = nil) -> Connectivity {
        switch ref.transport {
        case .localSocket:
            return .supported
        case .remote:
            do {
                let peers: [PeerTrustRecord]
                if let suppliedPeers {
                    peers = suppliedPeers
                } else {
                    do { peers = try PeerTrustStore.load(from: DevicePairingPaths.trustedPeers) }
                    catch { throw RemoteConnectionError.trustStoreUnreadable(String(describing: error)) }
                }
                _ = try remoteConnectionParameters(
                    of: ref, localIdentity: PairingDeviceIdentity.generate(), trustedPeers: peers)
                return .supported
            } catch {
                return .unsupported("\(error)。**没有退回本机** ——"
                    + "退回去的话你会看到自己这台机器上的 session，而界面显示的是远程那台。")
            }
        }
    }

    /// Parse and bind an endpoint to one exact paired peer.  This is also the single source of
    /// truth used by `connectivity`; the UI cannot report supported parameters that the connector
    /// later interprets differently.
    static func remoteConnectionParameters(
        of ref: BackendRef,
        localIdentity: PairingDeviceIdentity,
        trustedPeers: [PeerTrustRecord]
    ) throws -> SecureConnectionParameters {
        guard case let .remote(rawAddress) = ref.transport else {
            throw RemoteConnectionError.localSocketIsNotRemote
        }
        guard let components = URLComponents(string: rawAddress) else {
            throw RemoteConnectionError.invalidAddress(rawAddress)
        }
        guard components.scheme?.lowercased() == "pendingcrew+tls" else {
            throw RemoteConnectionError.insecureScheme(components.scheme)
        }
        guard let host = components.host, !host.isEmpty,
              let integerPort = components.port,
              let port = UInt16(exactly: integerPort), port != 0,
              components.user == nil, components.password == nil,
              components.path.isEmpty || components.path == "/",
              components.query == nil, components.fragment == nil else {
            throw RemoteConnectionError.invalidAddress(rawAddress)
        }
        guard let peer = trustedPeers.first(where: { $0.backendID == ref.id }) else {
            throw RemoteConnectionError.notPaired(ref.id)
        }
        guard peer.isValid else { throw RemoteConnectionError.invalidTrust(ref.id) }
        return .init(host: host, port: port, localIdentity: localIdentity, peer: peer)
    }

    /// Production remote connector.  Every failure is thrown; there is deliberately no local
    /// branch in this function, so a remote `BackendRef` can never become a Unix socket by accident.
    @MainActor
    static func connectRemote(
        to ref: BackendRef,
        identityFile: URL? = nil,
        trustFile: URL = DevicePairingPaths.trustedPeers
    ) throws -> SecureTCPTransport {
        let identity = try identityFile.map(DeviceIdentityStore.loadOrCreate(at:))
            ?? DeviceIdentityStore.loadOrCreate()
        let peers: [PeerTrustRecord]
        do { peers = try PeerTrustStore.load(from: trustFile) }
        catch { throw RemoteConnectionError.trustStoreUnreadable(String(describing: error)) }
        let parameters = try remoteConnectionParameters(
            of: ref, localIdentity: identity, trustedPeers: peers)
        return try SecureTCPTransport.connect(parameters: parameters)
    }
}

// MARK: - 实况与重启入口（设置「后端」页用）

/// 一条后端此刻的实况。**四态**，别压成「在 / 不在」：
/// 「管不了这条」「没在跑」「在跑但问不出」「在跑」对界面该说什么完全不同。
enum BackendLiveStatus: Equatable {
    /// 这个 app 探不了它（远程 / 不是本数据根下那个 socket）。**不探本机冒充它。**
    case unsupported(String)
    case notRunning
    /// 有后台占着锁，但握手问不出实况。
    case undecidable(String)
    /// `runningSessions` 只数真在跑的；`retainedSessions` 是已退出、画面还留在后台里的。
    case running(build: String, pid: Int32, runningSessions: Int, retainedSessions: Int)
}

/// 「重启后台」按钮能不能按、按之前要让人确认什么。
enum BackendRestartAction: Equatable {
    case available(title: String, confirmation: String)
    case unavailable(String)
}

extension BackendRegistry {
    /// 本机那一个后台的探针。注入以便测试不碰真 socket。
    /// **主线程上调**：真的握手靠主队列收回调（`SessionDaemonStatusProbe.query` 是 MainActor 隔离的）。
    struct LocalProbe {
        var daemonIsRunning: @MainActor () -> Bool
        var query: @MainActor () throws -> SessionDaemonStatusSnapshot

        @MainActor
        static func standard(paths: PendingCrewDaemonPaths) -> LocalProbe {
            LocalProbe(
                daemonIsRunning: { SessionDaemonControl.runningDaemonPid(paths: paths) != nil },
                query: { try SessionDaemonStatusProbe.query(paths: paths) })
        }
    }

    /// **只数真在跑的。** 后台的 records 在 session 退出后照旧留着（右栏还要看画面），
    /// `hello.sessionCount` 把它们也算进去 —— 拿它说「会打断 N 个」就把数说多了。
    /// 同一个毛病 `--daemon-status` 在 2026-09-04 撞过一次（`SessionDaemonStatusTextTests`）。
    static func runningSessionCount(in snapshot: SessionDaemonStatusSnapshot) -> Int {
        snapshot.sessions.filter { $0.state.status == .running }.count
    }

    /// 一条后端的实况。
    ///
    /// **远程、以及 socket 不是本数据根那一个的，一律不探** —— 探针只认得本数据根下
    /// 那一把锁、那一个 socket；拿它的读数填到别的条目上，就是「界面说连着 A，
    /// 看到的是本机」那种查不出来的错。本机那条走的是同一个函数，不开特例。
    ///
    /// ⚠️ 真探针要在主线程上调：socket 的回调投主队列（`UnixSocketTransport`），
    /// 握手最多等 2 秒。
    @MainActor
    static func liveStatus(of ref: BackendRef,
                           paths: PendingCrewDaemonPaths = .standard(),
                           probe: LocalProbe? = nil) -> BackendLiveStatus {
        if case let .unsupported(why) = connectivity(of: ref) { return .unsupported(why) }
        if case .remote = ref.transport {
            // Connectivity is now real for paired remotes, but this synchronous settings probe
            // cannot block the main actor on a TCP/TLS handshake.  Most importantly, it returns
            // before touching LocalProbe: no local reading can be mislabeled as the remote one.
            return .undecidable("远程后端已配对；建立安全连接后才能读取实况")
        }
        if case let .localSocket(path) = ref.transport, path != paths.socket {
            return .unsupported("这条指向 \(path)，不是这个 app 数据根下的后台（\(paths.socket)）。"
                + "现在只探得了后者，**也不会拿后者的实况冒充它**。")
        }
        let probe = probe ?? .standard(paths: paths)
        guard probe.daemonIsRunning() else { return .notRunning }
        do {
            let snapshot = try probe.query()
            let running = runningSessionCount(in: snapshot)
            return .running(build: snapshot.hello.daemonBuild, pid: snapshot.hello.pid,
                            runningSessions: running,
                            retainedSessions: snapshot.sessions.count - running)
        } catch {
            return .undecidable(String(describing: error))
        }
    }

    /// 重启入口。
    ///
    /// - Parameter interfaceRelaunchesBackend: 本界面是不是 viewer。**是的话「停」之后
    ///   界面会自己马上拉起一个新的**（`DaemonLaunchPlan`：锁空了就拉）—— 所以按钮叫
    ///   「重启」不叫「停用」；叫「停用」就是在骗人。
    static func restartAction(for status: BackendLiveStatus, appBuild: String,
                              interfaceRelaunchesBackend: Bool) -> BackendRestartAction {
        let after = interfaceRelaunchesBackend
            ? "停掉之后界面会马上拉起 \(appBuild) 版本的后台。"
            : "停掉之后不会自动再起。"
        switch status {
        case let .unsupported(why):
            return .unavailable(why)
        case .notRunning:
            return .unavailable(interfaceRelaunchesBackend
                ? "没有后台进程在跑，界面会自己拉起一个。"
                : "没有后台进程在跑，没有可停的。")
        case let .undecidable(why):
            // 卡死、握手超时的后台正是最需要重启的那种，所以仍然给按；
            // 但**说不出会打断几个**，就照实说说不出。
            return .available(
                title: interfaceRelaunchesBackend ? "重启后台" : "停止后台",
                confirmation: "问不出后台现在的情况（\(why)），也就说不清会打断几个 session。"
                    + after)
        case let .running(build, _, running, _):
            let replacing = build != appBuild
            let title = !interfaceRelaunchesBackend ? "停止后台"
                : replacing ? "换成 \(appBuild)" : "重启后台"
            var text = running > 0
                ? "会打断正在跑的 \(running) 个 session。"
                : "当前没有在跑的 session，不会打断任何东西。"
            text += after
            if running > 0 {
                text += (interfaceRelaunchesBackend && replacing)
                    ? "换完会问你要不要把它们接回来。"
                    : "被打断的不会自动接回，之后 @ 它们能接回。"
            }
            return .available(title: title, confirmation: text)
        }
    }
}
#endif
