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
/// ## 远程这一档：**只出现在类型里，不实现**
///
/// `.remote` 是给以后留的。现在连它一律返回「未实现」并说清楚 ——
/// **绝不许静默降级成本机**。静默降级的症状是：人填了一个远程地址，界面显示「已连接」，
/// 而他看到的其实是自己这台机器上的 session。那种错人是查不出来的。
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
        /// 远程后端。**占位，现在连不上** —— 见类型注释。
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
        try encoder.encode(refs.filter { !$0.isBuiltIn }).write(to: url, options: .atomic)
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

    /// 能不能连它。**远程那一档现在一律拒，且绝不降级成本机。**
    enum Connectivity: Equatable {
        case supported
        case unsupported(String)
    }

    static func connectivity(of ref: BackendRef) -> Connectivity {
        switch ref.transport {
        case .localSocket:
            return .supported
        case let .remote(url):
            return .unsupported("远程后端还没做（\(url)）。**没有退回本机** ——"
                + "退回去的话你会看到自己这台机器上的 session，而界面显示的是远程那台。")
        }
    }
}
#endif
