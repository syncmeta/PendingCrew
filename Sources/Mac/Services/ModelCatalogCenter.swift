#if os(macOS)
import Foundation

/// 可用模型表中心（Todo #37）：定时实探两家 CLI 的「有哪些模型可用」，落成
/// `models.json` 供 ① 新建 session 的 picker ② helper 侧 `start_session` /
/// `set_session_profile` 的清单注入与对照。
///
/// 数据源（2026-08-09 实测定档，见 docs/handbook 的 model-catalog 页）：
/// - **claude**：`claude -p "/model" --output-format json` + 同法的 `/effort`。
///   斜杠命令在 print 模式下由 CLI 本地处理，实测 `num_turns: 0` /
///   `total_cost_usd: 0` —— **不发 API 请求、不烧额度**（同 QuotaCenter 的 `/usage`）。
/// - **codex**：`codex app-server` 的 JSON-RPC `model/list`（`{includeHidden, limit}`
///   → `{data: [Model]}`）。同样是本地控制面查询，不开 thread、不发 turn。
///
/// 两家都探得到，所以 `models.json` 正常情况下两张表都是 `.probe`。
/// `AgentModelCatalog` 里那两张 `.manual` 兜底表只在**探不到时**兜底（CLI 没装、
/// 未登录、版本改了输出格式），并且注入时会明说「X 天没核实过，可能已过时」。
@MainActor
final class ModelCatalogCenter: ObservableObject {
    static let shared = ModelCatalogCenter()

    /// 当前这份表（UI 的 picker 直接读它）。启动首刷完成前是磁盘上的上一份 /
    /// nil —— 取用方一律经 `AgentModelCatalogFile.resolveTable` 回落兜底表，不会空。
    @Published private(set) var file: AgentModelCatalogFile?
    @Published private(set) var refreshing = false

    private var timer: Timer?
    private let directory: URL

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        self.file = AgentModelCatalogFile.load(from: self.directory)
    }

    /// 启动：立即探一轮，之后 6 小时一轮。模型清单是**天**级变化的东西，
    /// 探得再勤也没意义，两个短命子进程也不必反复起。
    func start() {
        guard timer == nil else { return }
        Task { await refresh() }
        timer = Timer.scheduledTimer(withTimeInterval: 6 * 3600, repeats: true) { [weak self] _ in
            Task { @MainActor in await self?.refresh() }
        }
    }

    func stop() { timer?.invalidate(); timer = nil }

    /// 探一轮两家（各自 best-effort：一家失败不影响另一家）。
    ///
    /// 探不到时**保留上一轮的表**继续用（整家消失更难懂），同时把原因记进
    /// `claudeError` / `codexError` —— 注入端会连同新鲜度一起说出来，不许静默。
    func refresh() async {
        guard !refreshing else { return }
        refreshing = true
        defer { refreshing = false }

        async let c = Self.probeClaude()
        async let x = Self.probeCodex()
        var next = file ?? AgentModelCatalogFile()

        if var table = await c {
            let resolution = SessionLaunchOptions.defaultModelResolution(for: .claudeCode,
                                                                        projectDir: nil)
            table.resolvedDefault = resolution.value
            table.resolvedDefaultSource = resolution.source
            next.claude = table
            next.claudeError = nil
        } else {
            next.claudeError = "读不到（claude -p \"/model\" 没跑起来或回显解析不出）"
        }
        if var table = await x {
            let resolution = SessionLaunchOptions.defaultModelResolution(for: .codex, projectDir: nil)
            table.resolvedDefault = resolution.value
            table.resolvedDefaultSource = resolution.source
            next.codex = table
            next.codexError = nil
        } else {
            next.codexError = "读不到（codex app-server 的 model/list 没答上）"
        }

        file = next
        persist(next)
    }

    private func persist(_ file: AgentModelCatalogFile) {
        guard let data = try? JSONEncoder().encode(file) else { return }
        try? data.write(to: directory.appendingPathComponent(AgentModelCatalog.fileName),
                        options: .atomic)
    }

    // MARK: - probes（nonisolated，跑在后台）

    /// claude：三条 `-p` 各起一个短命子进程 —— `/model`、`/effort`（运行时那套）、
    /// 外加一条**故意填错的 `--effort`** 把启动态那套逼出来。前两条任一解不出 → nil
    /// （半张表比没有表更危险，见 parser 注释）；第三条解不出只是少一层保护，不废表。
    nonisolated private static func probeClaude() async -> AgentModelTable? {
        guard let exe = LocalCodingAgentExecutable.resolve(.claudeCode) else { return nil }
        async let modelEcho = runClaudeSlash(exe: exe, command: "/model")
        async let effortEcho = runClaudeSlash(exe: exe, command: "/effort")
        async let launchWarning = probeClaudeLaunchEfforts(exe: exe)
        guard let m = await modelEcho, let e = await effortEcho else { return nil }
        return ClaudeModelProbeParser.table(modelEcho: m, effortEcho: e,
                                            launchEffortWarning: await launchWarning,
                                            probedAt: Date())
    }

    /// 启动参数 `--effort` 认哪些值：故意传一个不存在的值，CLI 会往 **stderr** 吐
    /// `Warning: Unknown --effort value 'X' — ignoring it and using the default effort.
    /// Valid values: low, medium, high, xhigh, max.` —— 那句就是启动态的权威清单。
    ///
    /// 用的哨兵值带 `pendingcrew` 前缀，万一将来某天它撞上一个真档位，行为也只是
    /// 「探不出来 → launchEfforts 留空 → 回落运行时那套」，不会给出错误清单。
    nonisolated private static func probeClaudeLaunchEfforts(exe: URL) async -> String? {
        await runClaudeSlash(exe: exe, command: "/model",
                             extraArgs: ["--effort", "__pendingcrew_probe__"],
                             wantStderr: true)
    }

    /// 跑一条 `claude [extra] -p "<slash>" --output-format json`。
    /// `wantStderr` = 要 stderr（启动参数警告写在那儿），否则要 stdout 的 `result` 字段。
    nonisolated private static func runClaudeSlash(
        exe: URL, command: String, extraArgs: [String] = [], wantStderr: Bool = false
    ) async -> String? {
        let captured: String? = await Task.detached(priority: .utility) {
            let p = Process()
            p.executableURL = exe
            p.arguments = extraArgs + ["-p", command, "--output-format", "json"]
            var env = ProcessInfo.processInfo.environment
            env["PATH"] = LocalCodingAgentExecutable.childProcessPath
            p.environment = env
            let out = Pipe(), err = Pipe()
            p.standardOutput = out
            p.standardError = err
            // stdin 明确给 /dev/null：`-p` 会**等 3 秒 stdin** 再放弃（实测警告
            // "no stdin data received in 3s"）。不给的话每条探测白等 3 秒。
            p.standardInput = FileHandle.nullDevice
            do { try p.run() } catch { return nil }
            // **先读干净管道再等退出**：两个 64KB 管道任一被写满，子进程会阻塞在
            // write 上，然后我们等它退出、它等我们读 —— 死等到超时（同
            // CodexAppServerConnection 的老坑）。
            let outData = out.fileHandleForReading.readDataToEndOfFile()
            let errData = err.fileHandleForReading.readDataToEndOfFile()
            // 兜底超时：斜杠命令是本地处理，实测毫秒级返回；卡住（首次 OAuth
            // refresh 挂起之类）就杀掉放弃这一轮，别把 Center 挂死。
            let deadline = Date().addingTimeInterval(30)
            while p.isRunning, Date() < deadline { Thread.sleep(forTimeInterval: 0.2) }
            if p.isRunning { p.terminate(); return nil }
            return String(data: wantStderr ? errData : outData, encoding: .utf8)
        }.value
        guard let captured else { return nil }
        return wantStderr ? captured : ClaudeModelProbeParser.resultField(fromJSON: captured)
    }

    /// codex：起一个短命 `codex app-server`，握手后问一句 `model/list` 就退出。
    /// 帧收发形状与 `QuotaCenter.fetchCodexLive` 同源（同一套 app-server 协议）。
    nonisolated private static func probeCodex() async -> AgentModelTable? {
        guard let exe = LocalCodingAgentExecutable.resolve(.codex) else { return nil }
        return await Task.detached(priority: .utility) { () -> AgentModelTable? in
            let p = Process()
            p.executableURL = exe
            p.arguments = ["app-server"]
            var env = ProcessInfo.processInfo.environment
            env["PATH"] = LocalCodingAgentExecutable.childProcessPath
            p.environment = env
            let stdin = Pipe(), stdout = Pipe()
            p.standardInput = stdin
            p.standardOutput = stdout
            // stderr 丢 /dev/null：codex 往 stderr 写 tracing，没人读会把 ~64KB
            // 管道塞满、进程卡死在 write 上（同 CodexAppServerConnection 的坑）。
            p.standardError = FileHandle.nullDevice
            do { try p.run() } catch { return nil }
            let watchdog = DispatchWorkItem { if p.isRunning { p.terminate() } }
            DispatchQueue.global().asyncAfter(deadline: .now() + 20, execute: watchdog)
            defer { watchdog.cancel(); if p.isRunning { p.terminate() } }

            func send(_ obj: [String: Any]) -> Bool {
                guard var line = try? JSONSerialization.data(withJSONObject: obj) else { return false }
                line.append(0x0a)
                do { try stdin.fileHandleForWriting.write(contentsOf: line); return true }
                catch { return false }
            }
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
                        // includeHidden: true —— hidden 的（如 codex-auto-review）
                        // 不进 picker 但仍是合法值，表里留着才对照得上。
                        guard send(["jsonrpc": "2.0", "method": "initialized", "params": [:]]),
                              send(["jsonrpc": "2.0", "id": requestId, "method": "model/list",
                                    "params": ["includeHidden": true, "limit": 100]])
                        else { return nil }
                    } else if msgId == requestId {
                        return CodexModelProbeParser.table(result: msg["result"], probedAt: Date())
                    }
                }
            }
        }.value
    }
}
#endif
