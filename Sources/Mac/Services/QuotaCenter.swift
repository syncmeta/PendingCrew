#if os(macOS)
import Foundation

/// 订阅额度中心（#455 / Todo #39）：聚合两家的订阅档位、限额窗口与使用画像，
/// 供 ① 侧栏额度显示 ② session 的 `get_quota` 工具（经 quota.json 快照文件）。
///
/// 数据源（2026-07-05 实测定档，见 handbook session-quota 页）：
/// - **claude**：`claude -p "/usage"` —— CLI 的本地 control-request（实测不烧额度、
///   自己处理 OAuth refresh，免 Keychain 工程尾巴），文本经 `ClaudeUsageTextParser`。
/// - **codex**：`codex app-server` 的 `account/rateLimits/read` —— **现查现得**，
///   经 `CodexAppServerQuotaParser`。问不到时才回落到
///   `~/.codex/sessions/**/rollout-*.jsonl` 最新文件（`CodexRolloutQuotaParser`）。
///
///   为什么主备是这个顺序（Todo #33 的真因）：rollout 文件是 codex **自己跑一轮**
///   才写的，我们只是捡。窗口在服务端重置之后，只要没人再跑 codex，那个文件就一直
///   是重置前的百分比 —— 界面于是永远停在旧数字上，看着就像「额度更新不了」。
///   实测 2026-08-08：盘上停在周窗 87%（12:17 落盘、自称 16:00 重置），同一时刻
///   app-server 答的是 0%。被动读盘拿不到「窗口翻篇」这件事，只能现问。
///
/// 两家都**没有剩余 token 绝对值**，只有百分比+重置——UI/工具照实呈现，不编造。
@MainActor
final class QuotaCenter: ObservableObject {
    static let shared = QuotaCenter()

    @Published private(set) var claude: AgentQuotaSnapshot?
    @Published private(set) var codex: AgentQuotaSnapshot?
    /// 上一轮**没取到**那一家时的人话原因（取到 → nil）。取不到时我们仍留着上一轮
    /// 的快照继续画（总比整行消失强），但必须同时把「这是旧值」说出来 —— 见
    /// `AgentQuotaFile.claudeError` 的说明。
    @Published private(set) var claudeError: String?
    @Published private(set) var codexError: String?
    @Published private(set) var refreshing = false

    /// helper（离线子进程）读的快照文件；与白板/控制通道同目录（--dir）。
    static let quotaFileName = "quota.json"

    private var timer: Timer?
    private let directory: URL
    private let codexSessionsDir: URL
    private let claudeConfigURL: URL

    init(directory: URL? = nil, codexSessionsDir: URL? = nil, claudeConfigURL: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        self.codexSessionsDir = codexSessionsDir
            ?? FileManager.default.homeDirectoryForCurrentUser
                .appendingPathComponent(".codex/sessions", isDirectory: true)
        self.claudeConfigURL = claudeConfigURL
            ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".claude.json")
    }

    /// 启动：立即刷一次，然后 10 分钟一轮。/usage 是本地 control-request 不烧额度，
    /// 但一次要跑几秒的子进程 —— 别高频轰。
    ///
    /// ⚠️ 只有编排者进程有资格起它（spec §6.2 闸门 1）。viewer 里误起 = 当场崩，
    /// 不是悄悄跑成双头 —— 双头会让同一批账被两个进程交替覆盖、唤醒发两遍，
    /// 而那种症状事后基本查不出来。
    func start() {
        precondition(
            ProcessRole.current == .orchestrator,
            "\(type(of: self)).start 只能在编排者进程里调用，当前角色=\(ProcessRole.current.rawValue)")
        guard timer == nil else { return }
        Task { await refresh() }
        timer = Timer.scheduledTimer(withTimeInterval: 600, repeats: true) { [weak self] _ in
            Task { @MainActor in await self?.refresh() }
        }
    }

    func stop() { timer?.invalidate(); timer = nil }

    /// 刷一轮两家额度（各自 best-effort：一家失败不影响另一家，保留旧值）。
    func refresh() async {
        guard !refreshing else { return }
        refreshing = true
        defer { refreshing = false }
        async let c = Self.fetchClaude(configURL: claudeConfigURL)
        async let x = Self.fetchCodex(sessionsDir: codexSessionsDir)
        // 取不到就**留着旧值继续画**（整行消失更难懂），但同时把原因记下来，
        // 由 UI / get_quota 明说「读不到、下面是旧值」—— 不许静默（Todo #33）。
        if let snap = await c {
            claude = snap
            claudeError = nil
        } else {
            claudeError = "读不到（claude -p /usage 没跑起来或输出解析不出）"
        }
        if let snap = await x {
            codex = snap
            codexError = nil
        } else {
            codexError = "读不到（codex app-server 没答上，rollout 记录也没有）"
        }
        persistSnapshotFile()
    }

    /// 落快照文件给 helper 的 get_quota 读（原子写；与控制通道同目录）。
    private func persistSnapshotFile() {
        let file = AgentQuotaFile(claude: claude, codex: codex,
                                  claudeError: claudeError, codexError: codexError)
        guard let data = try? JSONEncoder().encode(file) else { return }
        try? data.write(to: directory.appendingPathComponent(Self.quotaFileName), options: .atomic)
    }

    // MARK: - fetchers（nonisolated，跑在后台）

    /// `claude -p "/usage"`：resolve 不到 claude / 超时 / 输出解析不出 → nil。
    nonisolated private static func fetchClaude(configURL: URL) async -> AgentQuotaSnapshot? {
        guard let exe = LocalCodingAgentExecutable.resolve(.claudeCode) else { return nil }
        let text: String? = await Task.detached(priority: .utility) {
            let p = Process()
            p.executableURL = URL(fileURLWithPath: exe.path)
            p.arguments = ["-p", "/usage"]
            let out = Pipe()
            p.standardOutput = out
            p.standardError = Pipe()
            do { try p.run() } catch { return nil }
            // 兜底超时：/usage 实测几秒返回;卡住(如首次 OAuth refresh 挂起)则杀掉,
            // 这轮放弃,别把 QuotaCenter 挂死。
            let deadline = Date().addingTimeInterval(30)
            while p.isRunning, Date() < deadline {
                Thread.sleep(forTimeInterval: 0.2)
            }
            if p.isRunning { p.terminate(); return nil }
            let data = out.fileHandleForReading.readDataToEndOfFile()
            return String(data: data, encoding: .utf8)
        }.value
        guard let text else { return nil }
        // `/usage` 自己不写档位；本机实测 ~/.claude.json 的 oauthAccount
        // organizationRateLimitTier 会给到 Max 5x/20x。读不到就留 nil，不做人工覆盖。
        let detectedPlan = (try? Data(contentsOf: configURL)).flatMap(ClaudeAccountPlanParser.parse)
        return ClaudeUsageTextParser.parse(text, subscriptionPlan: detectedPlan)
    }

    /// codex 额度：**先现问 app-server**，问不到才回落读 rollout 文件。
    ///
    /// 顺序不能反（Todo #33）：rollout 是被动的，窗口在服务端翻篇它不会知道，
    /// 于是界面能停在一个早就过期的百分比上纹丝不动。详见类型头部注释。
    nonisolated private static func fetchCodex(sessionsDir: URL) async -> AgentQuotaSnapshot? {
        if let live = await fetchCodexLive() { return live }
        return await fetchCodexFromRollout(sessionsDir: sessionsDir)
    }

    /// `codex app-server` 起一个短命子进程，走 JSON-RPC 问一句
    /// `account/rateLimits/read` 就退出。resolve 不到 codex / 超时 / 答不出 → nil。
    ///
    /// 只读一句、不开 thread、不发 turn —— **不烧额度**（跟 claude 的 `/usage`
    /// 一样是本地控制面查询）。
    nonisolated private static func fetchCodexLive() async -> AgentQuotaSnapshot? {
        guard let exe = LocalCodingAgentExecutable.resolve(.codex) else { return nil }
        return await Task.detached(priority: .utility) { () -> AgentQuotaSnapshot? in
            let p = Process()
            p.executableURL = exe
            p.arguments = ["app-server"]
            var env = ProcessInfo.processInfo.environment
            env["PATH"] = LocalCodingAgentExecutable.childProcessPath
            p.environment = env
            let stdin = Pipe(), stdout = Pipe()
            p.standardInput = stdin
            p.standardOutput = stdout
            // stderr 直接丢给 /dev/null：codex 往 stderr 写 tracing，没人读会把
            // ~64KB 管道塞满、进程卡死在 write 上（同 CodexAppServerConnection 的坑）。
            p.standardError = FileHandle.nullDevice
            do { try p.run() } catch { return nil }
            // 看门狗：到点直接杀，阻塞中的 read 会因 EOF 返回，不会挂死这一轮刷新。
            let watchdog = DispatchWorkItem { if p.isRunning { p.terminate() } }
            DispatchQueue.global().asyncAfter(deadline: .now() + 20, execute: watchdog)
            defer { watchdog.cancel(); if p.isRunning { p.terminate() } }

            func send(_ obj: [String: Any]) -> Bool {
                guard var line = try? JSONSerialization.data(withJSONObject: obj) else { return false }
                line.append(0x0a)
                do { try stdin.fileHandleForWriting.write(contentsOf: line); return true }
                catch { return false }
            }
            // 握手 → initialized → 问额度。参数形状同 CodexProtocol.initializeParams。
            let requestId = 1
            guard send(["jsonrpc": "2.0", "id": 0, "method": "initialize",
                        "params": ["clientInfo": ["name": "PendingCrew",
                                                  "title": "PendingCrew", "version": "1.0"],
                                   "capabilities": ["experimentalApi": true,
                                                    "requestAttestation": false]]])
            else { return nil }

            var buffer = Data()
            var handshaken = false
            while true {
                let chunk = stdout.fileHandleForReading.availableData
                if chunk.isEmpty { return nil }              // EOF / 被看门狗杀掉
                buffer.append(chunk)
                while let nl = buffer.firstIndex(of: 0x0a) {
                    let lineData = buffer[buffer.startIndex..<nl]
                    buffer.removeSubrange(buffer.startIndex...nl)
                    guard let msg = (try? JSONSerialization.jsonObject(with: Data(lineData)))
                            as? [String: Any] else { continue }
                    let msgId = msg["id"] as? Int
                    if msgId == 0, !handshaken {
                        handshaken = true
                        guard send(["jsonrpc": "2.0", "method": "initialized", "params": [:]]),
                              send(["jsonrpc": "2.0", "id": requestId,
                                    "method": "account/rateLimits/read", "params": [:]])
                        else { return nil }
                    } else if msgId == requestId {
                        // error 响应没有 result → 解析不出 → nil，不猜。
                        return CodexAppServerQuotaParser.parse(msg["result"])
                    }
                }
            }
        }.value
    }

    /// 回落：最新 rollout jsonl 的 rate_limits。目录不存在（没装/没用过 codex）→ nil。
    ///
    /// **这条链路只有 codex 真跑过才会有新数**：rollout 文件是 codex 自己每轮落的盘，
    /// 我们只是读。所以「一个多月没跑 codex」＝「读到一个多月前的数」，不是刷新坏了。
    /// 把文件 mtime 当作数据的产生时刻带下去，UI / get_quota 才能如实标出陈旧。
    nonisolated private static func fetchCodexFromRollout(
        sessionsDir: URL
    ) async -> AgentQuotaSnapshot? {
        await Task.detached(priority: .utility) {
            guard let newest = newestRolloutFile(under: sessionsDir),
                  let data = try? Data(contentsOf: newest.url) else { return nil }
            // rollout 可能很大：只解析尾部 256KB（rate_limits 每轮都写,尾部必有最新）。
            // 字符边界对齐由 parseTail 负责（切在半个汉字上曾让整轮静默读空）。
            let tail = data.count > 262_144 ? Data(data.suffix(262_144)) : data
            return CodexRolloutQuotaParser.parseTail(tail, producedAt: newest.modifiedAt)
        }.value
    }

    /// `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` 里 mtime 最新的一个（连 mtime 一起给）。
    nonisolated private static func newestRolloutFile(
        under dir: URL
    ) -> (url: URL, modifiedAt: Date)? {
        let fm = FileManager.default
        guard let e = fm.enumerator(at: dir, includingPropertiesForKeys: [.contentModificationDateKey],
                                    options: [.skipsHiddenFiles]) else { return nil }
        var newest: (url: URL, modifiedAt: Date)? = nil
        for case let url as URL in e where url.lastPathComponent.hasPrefix("rollout-")
            && url.pathExtension == "jsonl" {
            let m = (try? url.resourceValues(forKeys: [.contentModificationDateKey]))?
                .contentModificationDate ?? .distantPast
            if newest == nil || m > newest!.modifiedAt { newest = (url: url, modifiedAt: m) }
        }
        return newest
    }
}
#endif
