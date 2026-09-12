#if os(macOS)
import Foundation

/// 「上一次是**怎么**结束的」—— 恢复弹窗的承重件。
///
/// ## 为什么非要有这么一枚印记
///
/// 人类要的规格是浏览器那一套：**只有「刚更新过」或「上次意外结束」才问要不要恢复**，
/// 其余一律不问、更不自动恢复。整条规格压在一个判断上 —— **分得出「正常退出」和
/// 「意外结束」**。分不出来的两种下场都很糟：天天弹窗骚扰人，或者真崩了也不问。
///
/// 而 2026-09-11 查下来**现有的东西不够**：
/// - `SessionDaemonHost.stop()` 只关 listener、写一行日志、放锁，**不清 registry**。
/// - registry（`SessionProcessRegistry`）存 pid / 启动时刻 / 进程条目，
///   **没有任何一处记「上次是怎么结束的」**。
/// - 实测：正常停之后盘上那本 registry 的条目照样全在，与崩溃后一模一样。
///
/// 唯一看起来能用的弱信号是「收尸时判 `.alreadyGone` 还是 `.reap`」——**它两个方向
/// 都会错**（正常停时有子进程赖着不死 → `.reap`；崩溃时子进程恰好也没了 →
/// `.alreadyGone`），拿它当判据又是一次「拿便宜的代理量替代那件事」。
///
/// 所以缺的不是判定逻辑，是**一条上次退出留下的痕迹**。
///
/// ## 四态，不是两态
///
/// 印记在一个进程的一生里写**三次**（起来时 / 收尾一开始 / 真的要退之前），
/// 于是下次开机读到的状态有四种，每一种对人的意义都不同：
///
/// | 盘上是什么 | 判定 | 要不要问 |
/// |---|---|---|
/// | 文件不在 | `.noPriorRun` —— 这台机器上从没跑过 | **不问**（全新安装不该弹窗） |
/// | `phase == .running` | `.unexpected` —— 连收尾都没开始就没了（崩溃 / 被 SIGKILL / 断电） | 问 |
/// | `phase == .draining` | `.diedWhileDraining` —— 收尾走到一半没了 | 问 |
/// | `phase == .clean` | `.clean` —— 正常退出 | **不问** |
///
/// **为什么「收尾走到一半」必须单独一档**：并进「正常」会漏掉真出事的那次；
/// 并进「崩溃」会把人自己按的停说成崩溃。两边都是在说谎。
/// （同一族的前例：`SessionOrchestratorLock.Presence` 也是被压成两态之后才出的事。）
///
/// **为什么要有起来时那一次写**：只写收尾的话，「从没跑过」和「崩在收尾之前」
/// 在盘上长得一模一样 —— 全新安装第一次开就会弹一个「要恢复吗」。
///
/// ## 一套代码两个 role
///
/// daemon 会崩，**GUI 也会崩**（2026-09-09 就崩过一次，SIGABRT）。只盯 daemon 的话，
/// 人类真正撞到过的那个场景反而不弹窗。所以同一个结构、同一份代码，按 `role` 分文件写。
///
/// **按 role 分成两个文件、而不是一个文件两个 key**：那两个 role 是**两个进程**，
/// 同时写一个文件就要处理跨进程写竞争 —— 而这枚印记的全部价值在于它写得下去。
/// 各写各的文件，没有竞争，也不需要锁。
enum ProcessExitMarkerRole: String, Codable, CaseIterable {
    case daemon
    case app

    /// 文件名。**数据根下**，跟锁 / registry 一起走 `PENDINGCREW_DATA_DIR`。
    var fileName: String { "\(rawValue).lastexit.json" }
}

struct ProcessExitMarker: Codable, Equatable {
    enum Phase: String, Codable {
        /// 起来了，还在跑。
        case running
        /// 收到停止信号，正在停 session / 放锁。
        case draining
        /// 收尾做完了，马上 `exit`。
        case clean
    }

    var role: ProcessExitMarkerRole
    var phase: Phase
    /// 写下这枚印记的时刻。
    var at: Date
    /// 哪个版本写的 —— 「刚更新过」那一档直接比它，不需要第二套机制。
    var build: String
    var pid: Int32
    /// 进程启动时刻。跟 pid 一起比，防 pid 复用。
    var startedAt: Date
}

/// 上一轮是怎么结束的。
enum ProcessExitClassification: Equatable {
    /// 这台机器上从没跑过（或数据根是新的）。**不问。**
    case noPriorRun
    /// 正常退出。**不问。**
    case clean
    /// 收尾走到一半没了。问。
    case diedWhileDraining
    /// 连收尾都没开始就没了。问。
    case unexpected

    /// 该不该提「要不要恢复上次的 session」。
    var shouldOfferRestore: Bool {
        switch self {
        case .noPriorRun, .clean: return false
        case .diedWhileDraining, .unexpected: return true
        }
    }

    /// 给人看的一句话。**每一档都说清「发生了什么」**，不是只给个状态名。
    var text: String {
        switch self {
        case .noPriorRun: return "这是第一次运行。"
        case .clean: return "上次是正常退出的。"
        case .diedWhileDraining: return "上次在收尾的过程中被中断了。"
        case .unexpected: return "上次没有正常退出（崩溃、被强制结束或断电）。"
        }
    }
}

/// 读写那枚印记。
///
/// **写失败一律抛，不许 `try?` 吞掉。** 这枚印记是「要不要打扰人」的唯一依据：
/// 写不进去而没人知道的话，下次开机会把一次正常退出判成崩溃（多问一次，还好），
/// 或者把一次崩溃判成正常（该问的时候不问，人丢了活还不知道为什么）。
/// **调用方必须把错误落进日志。**
struct ProcessExitMarkerStore {
    let directory: URL
    let role: ProcessExitMarkerRole

    init(directory: URL = PendingCrewDataRoot.url, role: ProcessExitMarkerRole) {
        self.directory = directory
        self.role = role
    }

    var url: URL { directory.appendingPathComponent(role.fileName) }

    // MARK: - 写

    /// 起来了。**进程启动时调一次。**
    @discardableResult
    func markRunning(pid: Int32 = ProcessInfo.processInfo.processIdentifier,
                     startedAt: Date,
                     build: String,
                     now: Date = Date()) throws -> ProcessExitMarker {
        try write(.init(role: role, phase: .running, at: now,
                        build: build, pid: pid, startedAt: startedAt))
    }

    /// 开始收尾。**收尾的第一件事就调它** —— 放在最后调的话，收尾卡死被强杀时
    /// 盘上留的还是 `.running`，那一次就跟真崩溃分不开了。
    @discardableResult
    func markDraining(pid: Int32 = ProcessInfo.processInfo.processIdentifier,
                      startedAt: Date,
                      build: String,
                      now: Date = Date()) throws -> ProcessExitMarker {
        try write(.init(role: role, phase: .draining, at: now,
                        build: build, pid: pid, startedAt: startedAt))
    }

    /// 收尾做完了，马上退。
    @discardableResult
    func markClean(pid: Int32 = ProcessInfo.processInfo.processIdentifier,
                   startedAt: Date,
                   build: String,
                   now: Date = Date()) throws -> ProcessExitMarker {
        try write(.init(role: role, phase: .clean, at: now,
                        build: build, pid: pid, startedAt: startedAt))
    }

    @discardableResult
    private func write(_ marker: ProcessExitMarker) throws -> ProcessExitMarker {
        try FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        // 原子写：写到一半被打死时，盘上要么是上一版要么是这一版，
        // 绝不会是半个 JSON（半个 JSON 会被读成「解不开」→ 判成崩溃，白问一次）。
        try MultiProcessJSONStore.writeStaged(encoder.encode(marker), to: url)
        return marker
    }

    // MARK: - 读

    /// 上一轮是怎么结束的。**读不动 ≠ 没跑过** —— 见 `Read`。
    func classifyPreviousRun() -> ProcessExitClassification {
        switch readPrevious() {
        case .absent: return .noPriorRun
        case .unreadable: return .unexpected
        case let .found(marker):
            switch marker.phase {
            case .clean: return .clean
            case .draining: return .diedWhileDraining
            case .running: return .unexpected
            }
        }
    }

    /// 三态读。**「文件不在」「读不动」「读到了」是三件事**，压成可选值就会把
    /// 「我读不到」说成「没跑过」——而那一档恰好是唯一「不问」的一档，
    /// 于是一次读失败会变成「崩了也不问」。
    enum Read: Equatable {
        case absent
        case unreadable(String)
        case found(ProcessExitMarker)
    }

    func readPrevious() -> Read {
        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch let error as NSError
            where error.domain == NSCocoaErrorDomain
                && error.code == NSFileReadNoSuchFileError {
            return .absent
        } catch {
            return .unreadable("读不了 \(url.lastPathComponent)：\(error.localizedDescription)")
        }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        guard let marker = try? decoder.decode(ProcessExitMarker.self, from: data) else {
            return .unreadable("\(url.lastPathComponent) 解不开")
        }
        return .found(marker)
    }

    /// 上一轮是哪个版本写的（用来判「刚更新过」）。读不到就是 nil。
    var previousBuild: String? {
        if case let .found(marker) = readPrevious() { return marker.build }
        return nil
    }
}

/// 一个进程从起来到退出，替它记那三笔。
///
/// 存在的理由是**让调用点只剩一行，同时不让写失败被吞掉** —— 三个 mark 方法都会抛，
/// 而两个调用点（daemon / GUI）各自有自己的日志通道，所以错误经 `onWriteFailure`
/// 交回去，由调用方落进它自己的日志。**这里不 `try?`，也不打印到黑洞里。**
final class ProcessLifecycleMarker {
    private let store: ProcessExitMarkerStore
    private let build: String
    private let pid: Int32
    private let startedAt: Date
    /// 写失败时叫谁。**必须有人接** —— 印记写不下去而没人知道，下次开机就会判错。
    private let onWriteFailure: (String) -> Void

    init(role: ProcessExitMarkerRole,
         directory: URL = PendingCrewDataRoot.url,
         build: String,
         pid: Int32 = ProcessInfo.processInfo.processIdentifier,
         startedAt: Date = Date(),
         onWriteFailure: @escaping (String) -> Void) {
        store = ProcessExitMarkerStore(directory: directory, role: role)
        self.build = build
        self.pid = pid
        self.startedAt = startedAt
        self.onWriteFailure = onWriteFailure
    }

    /// 上一轮是怎么结束的。**要在 `markRunning()` 之前读** —— 先写就把上一轮盖掉了。
    func classifyPreviousRun() -> ProcessExitClassification { store.classifyPreviousRun() }
    var previousBuild: String? { store.previousBuild }

    func markRunning()  { record { try store.markRunning(pid: pid, startedAt: startedAt, build: build) } }
    func markDraining() { record { try store.markDraining(pid: pid, startedAt: startedAt, build: build) } }
    func markClean()    { record { try store.markClean(pid: pid, startedAt: startedAt, build: build) } }

    private func record(_ body: () throws -> ProcessExitMarker) {
        do { _ = try body() } catch {
            onWriteFailure("退出印记写不进去（\(store.url.lastPathComponent)）：\(error.localizedDescription)"
                + "。下次启动会把这次判成「没有正常退出」。")
        }
    }
}
#endif
