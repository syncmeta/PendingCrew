#if os(macOS)
import XCTest
import Foundation

/// **真 · TUI 语料的录制器**（前后端分离 P3 / 设计 §7.1 第二组语料）。
///
/// 随机合成语料测不出真实 TUI 的怪癖，而我们要还原的**恰恰就是它**。所以这里
/// 真的在 PTY 里把 agent 拉起来，把它吐的原始字节原样存盘，供
/// `TerminalSnapshotFixtureTests` 拿去跑往返。
///
/// **这不是一条会跑的测试** —— 它默认 skip，只有显式带上环境变量才动：
/// ```sh
/// TEST_RUNNER_PENDINGCREW_RECORD_TUI=claude xcodebuild -project PendingCrew.xcodeproj \
///   -scheme PendingCrew -destination 'platform=macOS' \
///   -only-testing:PendingCrewTests/AgentTuiFixtureRecorder test
/// ```
/// 理由：它要花真钱（订阅额度）、要联网、而且结果每次都不一样 —— 那三条里的任何
/// 一条都足以让它不配当 CI 里的一条测试。它是**工具**，产物才是测试。
///
/// **全程无画面**：走的是 `AgentSessionCore`（Foundation only，P1 劈出来的那半），
/// 一行 AppKit 都没有，所以不碰图形界面、不会弹任何系统权限框。
///
/// ## 只录 claude，不录 codex —— 后者不存在
///
/// `CrewSessionRunner` 里 claude 走 PTY + 真终端，**codex 走的是 app-server
/// （JSON-RPC），根本没有 PTY、没有终端缓冲区**。codex 的输出一个字节都不会经过
/// 快照编码器要序列化的那个 `Terminal`。设计 §7.1 里「claude 与 codex 各一段」
/// 是照着更早的假设写的（那时 codex 还打算也塞进 PTY），已在文档里更正。
@MainActor
final class AgentTuiFixtureRecorder: XCTestCase {

    /// fixture 存这儿。**故意不在 `Tests/PendingCrewTests/Fixtures/`** —— 那个目录
    /// 是 gitignored 的（CrewChatOpenCostTests 用的真实群聊内容），东西放进去会
    /// 变成「本机绿、别人 skip」。这里的 fixture 要入库。
    static var fixtureDirectory: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()      // PendingCrewTests
            .deletingLastPathComponent()      // Tests
            .appendingPathComponent("Fixtures", isDirectory: true)
    }

    /// 单段录像的上限。超过就停 —— fixture 是要入库的，几十 MB 不合适。
    private static let byteCap = 3 * 1024 * 1024

    func testRecord() async throws {
        // 两个键都认：`xcodebuild test` 不把任意环境变量透进 xctest 进程，官方通道是
        // 加 `TEST_RUNNER_` 前缀（由 runner 剥掉再转交）。直接跑 xctest 时前者才有效。
        let env = ProcessInfo.processInfo.environment
        let what = env["PENDINGCREW_RECORD_TUI"] ?? env["TEST_RUNNER_PENDINGCREW_RECORD_TUI"]
        guard let what, !what.isEmpty else {
            throw XCTSkip("""
            录制器默认不跑（要花订阅额度、要联网、每次结果都不同）。要录：
              TEST_RUNNER_PENDINGCREW_RECORD_TUI=claude  …只录 claude 的 TUI
              TEST_RUNNER_PENDINGCREW_RECORD_TUI=shell   …只录人直接用的那个终端 session
              TEST_RUNNER_PENDINGCREW_RECORD_TUI=all     …两段都录
            产物落在 \(Self.fixtureDirectory.path)
            """)
        }
        if what == "shell" || what == "all" { try await recordShell() }
        if what == "claude" || what == "all" { try await recordClaude() }
    }

    // MARK: - 人直接用的那个终端（`.terminal`）

    /// 不花额度、不联网，但它同样是**真的**会经过快照编码器的语料：`ls` 的颜色、
    /// 真实的滚动历史、提示符重绘。跟 claude 的整屏重绘不是一类，两类都要有。
    private func recordShell() async throws {
        let core = AgentSessionCore(
            config: SessionConfig(kind: .terminal),
            mode: .plainShell,
            executable: "/bin/sh",
            workdir: NSTemporaryDirectory(),
            env: ["TERM": "xterm-256color", "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
                  "CLICOLOR": "1", "CLICOLOR_FORCE": "1"])
        defer { core.stop() }

        let sink = ByteSink(cap: Self.byteCap)
        core.onOutput = { sink.append($0) }

        for command in [
            "printf '\\033[1;31mred\\033[0m \\033[4:3munder\\033[0m \\033[48;5;27mbg\\033[0m\\n'",
            "ls -laG /usr/bin | head -40",
            "printf '中文宽字符 \\u00e9\\u0301 组合\\n'",
            "for i in $(seq 1 300); do echo \"scrollback line $i\"; done",
            "printf '\\033[2J\\033[HAFTER CLEAR\\n'",
        ] {
            core.write(Array((command + "\n").utf8))
            try await settle(sink, quietFor: 0.6, upTo: 15)
        }
        // 改一次宽度 —— reflow 是 §7.2 要验的那条。
        core.resize(cols: 60, rows: 20)
        try await settle(sink, quietFor: 0.6, upTo: 5)
        core.write(Array("echo AFTER_RESIZE\n".utf8))
        try await settle(sink, quietFor: 0.6, upTo: 10)

        try write(sink.bytes, to: "tui-shell.bin")
    }

    // MARK: - claude 的 TUI

    private func recordClaude() async throws {
        guard let executable = LocalCodingAgentExecutable.resolve(.claudeCode) else {
            throw XCTSkip("本机找不到 claude 可执行文件")
        }
        let workdir = FileManager.default.temporaryDirectory
            .appendingPathComponent("pendingcrew-tui-record", isDirectory: true)
        try? FileManager.default.createDirectory(at: workdir, withIntermediateDirectories: true)

        // **故意不接白板 / hook / MCP**：录的是画面，不是编排。少一条外部依赖就少
        // 一种「录到一半 fixture 里混进了本机白板内容」的可能。
        let config = SessionConfig(
            kind: .claudeCode,
            initialPrompt: "只回答这一句，不要用任何工具：说出 1 到 5 这五个数字。")
        let core = AgentSessionCore(
            config: config,
            mode: .agent,
            executable: executable.path,
            workdir: workdir.path,
            env: ProcessInfo.processInfo.environment)
        defer { core.stop() }

        let sink = ByteSink(cap: Self.byteCap)
        core.onOutput = { sink.append($0) }

        // 第一轮：启动 + 首屏 + 回答。
        try await settle(sink, quietFor: 2.0, upTo: 90)
        // 改一次宽度（§7.2 的 reflow，也是真 TUI 最爱出怪事的地方）。
        core.resize(cols: 100, rows: 30)
        try await settle(sink, quietFor: 1.5, upTo: 20)
        // 第二轮：再问一句，拿到一段新的整屏重绘。
        // `AgentSessionCore.send` 自己会隔一拍单独补回车 —— claude 的 TUI 按字节
        // 到达时序区分「粘贴 vs 敲键」，正文+回车一笔写入会被整段判成粘贴、不提交。
        // 所以这里**不能**再补一个 0x0d。
        core.send("再说出 6 到 10。")
        try await settle(sink, quietFor: 2.0, upTo: 90)

        try write(sink.bytes, to: "tui-claude.bin")
    }

    // MARK: -

    /// 等到「输出真的来过、而且安静了 `quietFor` 秒」，或超时。
    ///
    /// **不能只看「安静了多久」**：刚写完命令时子进程还没来得及回一个字节，那一刻
    /// 它「已经安静很久了」，判据当场成立、直接返回 —— 第一版就是这么写的，录出来
    /// 是个 0 字节的空文件。所以必须先等到**这一轮真的有字节进来**，再等它安静。
    private func settle(_ sink: ByteSink, quietFor: TimeInterval,
                        upTo timeout: TimeInterval) async throws {
        let startCount = sink.count
        let deadline = Date().addingTimeInterval(timeout)
        var lastCount = startCount
        var lastChange = Date()
        while Date() < deadline {
            try await Task.sleep(nanoseconds: 200_000_000)
            let now = sink.count
            if now != lastCount {
                lastCount = now
                lastChange = Date()
                continue
            }
            // 这一轮有过输出，且已经安静够久了。
            if now > startCount, Date().timeIntervalSince(lastChange) >= quietFor { return }
        }
    }

    private func write(_ bytes: [UInt8], to name: String) throws {
        let dir = Self.fixtureDirectory
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent(name)
        try Data(bytes).write(to: url)
        print("[recorder] \(url.path) ← \(bytes.count) 字节")
        XCTAssertGreaterThan(bytes.count, 1024, "录了个空的就别存了：\(name)")
    }

    /// `onOutput` 在主队列上被调，但它是逃逸闭包 —— 用一个 class 把字节攒起来。
    private final class ByteSink {
        private(set) var bytes: [UInt8] = []
        var count: Int { bytes.count }
        private let cap: Int
        init(cap: Int) { self.cap = cap }
        func append(_ slice: ArraySlice<UInt8>) {
            guard bytes.count < cap else { return }
            bytes.append(contentsOf: slice)
        }
    }
}
#endif
