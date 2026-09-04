#if os(macOS)
import Foundation
import SwiftTerm
import XCTest

/// P5a 的证据工具：`PendingCrew --daemon-attach <sessionId>` 这条**无界面** viewer 路。
///
/// 这一组钉的是三件事，一件都不许靠时序赌：
/// 1. 快照字节 → 文本的渲染是**纯函数**，不需要 daemon、不需要窗口就能量。
/// 2. 「daemon 里没有这个 session id」必须给**人能看懂的错误**，不能是一片空白 ——
///    空白正是这条路最容易翻的车（attach 一个不存在的 session，服务端那句
///    `guard let record = records[...] else { return }` 是**静默返回**，
///    客户端会一直等一份永远不来的快照）。
/// 3. 真 socket 上 attach 拿到快照、断开、重连再拿一份，两份画面一致 ——
///    这就是「viewer 断开→重连→恢复同一 session 的画面」这条链的证据本身。
@MainActor
final class SessionDaemonAttachTests: XCTestCase {
    private let capabilities = SessionDaemonHost.defaultCapabilities

    // MARK: - ① 快照字节 → 文本（纯函数）

    func test_快照字节渲染成屏幕文本() {
        let source = HeadlessTerminalHarness(cols: 40, rows: 6)
        source.feed("第一行\r\n第二行 hello\r\n")
        let snapshot = TerminalSnapshotEncoder.encode(source.terminal, probe: source.probe)

        let text = SessionSnapshotTextRenderer.render(
            snapshotBytes: snapshot.bytes, cols: snapshot.cols, rows: snapshot.rows,
            maxLines: 100)

        XCTAssertEqual(text, "第一行\n第二行 hello",
                       "快照喂进无画面终端后取出的文本必须与源终端的画面一致")
    }

    /// 渲染宽度是**调用方给的**（探针不许为了看一眼就把在跑的 TUI 重排）。
    /// 所以这个函数必须认自己收到的 cols/rows，不能偷用 80×25 默认值。
    func test_渲染按给定宽度而不是默认80列() {
        let source = HeadlessTerminalHarness(cols: 120, rows: 4)
        source.feed(String(repeating: "x", count: 100))
        let snapshot = TerminalSnapshotEncoder.encode(source.terminal, probe: source.probe)

        let text = SessionSnapshotTextRenderer.render(
            snapshotBytes: snapshot.bytes, cols: 120, rows: 4, maxLines: 100)

        XCTAssertEqual(text, String(repeating: "x", count: 100),
                       "120 列的画面在 120 列上渲染不该被折行")
    }

    func test_空快照渲染成空串而不是崩() {
        XCTAssertEqual(
            SessionSnapshotTextRenderer.render(snapshotBytes: [], cols: 80, rows: 25,
                                               maxLines: 100),
            "")
    }

    // MARK: - ② 没有这个 session id：明确错误，不是空白

    func test_daemon里没有这个session时报明确错误而不是空白() {
        let sessions = [Self.summary(id: "worker-aaaaaaa1", kind: .claudeCode)]

        XCTAssertThrowsError(
            try SessionDaemonAttachProbe.resolveTarget(sessionId: "worker-zzzzzzz9",
                                                       in: sessions)
        ) { error in
            let message = String(describing: error)
            XCTAssertFalse(
                message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                "错误信息不能是空白 —— 空白正是这条路最容易翻的车")
            XCTAssertTrue(message.contains("worker-zzzzzzz9"),
                          "错误里必须点名要找的那个 session id：\(message)")
            XCTAssertTrue(message.contains("worker-aaaaaaa1"),
                          "错误里必须列出 daemon 里真有的 session：\(message)")
        }
    }

    /// codex session 走的是 transcript 事件，服务端**根本不发终端快照**
    /// （`SessionProtocolServer.attach` 里 `wantsSnapshot` 那一行）。
    /// 所以这条要在 attach 之前就说清楚，而不是让人对着一个永远不来的快照等超时。
    func test_codex_session直接说没有终端快照而不是干等() {
        let sessions = [Self.summary(id: "worker-codex01", kind: .codex)]

        XCTAssertThrowsError(
            try SessionDaemonAttachProbe.resolveTarget(sessionId: "worker-codex01",
                                                       in: sessions)
        ) { error in
            let message = String(describing: error)
            XCTAssertTrue(message.contains("codex"),
                          "错误里要说清是 codex session 没有终端快照：\(message)")
        }
    }

    func test_有这个session时解析出它的kind() throws {
        let sessions = [Self.summary(id: "worker-aaaaaaa1", kind: .claudeCode),
                        Self.summary(id: "shell-1", kind: .terminal)]
        XCTAssertEqual(
            try SessionDaemonAttachProbe.resolveTarget(sessionId: "shell-1", in: sessions),
            .terminal)
    }

    // MARK: - ③ 真 socket：attach → 快照 → 断开重连 → 第二份快照

    func test_真socket上attach拿到快照并渲染成文本() throws {
        let harness = try ProbeHarness(capabilities: capabilities)
        harness.registerSession(id: "worker-bf39513e", screen: "从 daemon 拿到的画面")

        let report = try SessionDaemonAttachProbe.run(
            options: .init(sessionId: "worker-bf39513e", reconnect: false,
                           cols: 80, rows: 25, maxLines: 200),
            timeout: 5,
            connect: harness.connect)

        XCTAssertEqual(report.screens.count, 1)
        XCTAssertEqual(report.screens.first?.text, "从 daemon 拿到的画面")
        XCTAssertGreaterThan(report.screens.first?.byteCount ?? 0, 0,
                             "快照字节数得如实报出来，0 字节等于没证据")
        XCTAssertTrue(report.text.contains("从 daemon 拿到的画面"),
                      "给人看的那份输出里必须有画面本身：\(report.text)")
    }

    /// **这条是整个工具存在的理由**：断开、重连、第二份快照与第一份一致。
    func test_断开重连后拿到第二份快照且画面恢复() throws {
        let harness = try ProbeHarness(capabilities: capabilities)
        harness.registerSession(id: "worker-bf39513e", screen: "断开前就在屏幕上的字")

        let report = try SessionDaemonAttachProbe.run(
            options: .init(sessionId: "worker-bf39513e", reconnect: true,
                           cols: 80, rows: 25, maxLines: 200),
            timeout: 5,
            connect: harness.connect)

        XCTAssertEqual(report.screens.count, 2, "开了重连开关就必须打两份快照")
        XCTAssertEqual(report.screens.first?.text, "断开前就在屏幕上的字")
        XCTAssertEqual(report.screens.last?.text, report.screens.first?.text,
                       "重连后的画面必须与断开前一致 —— 这就是 P5a 要的那条证据")
        XCTAssertEqual(harness.connectCount, 2, "重连必须是真的新连接，不是同一条链路装装样子")
        XCTAssertTrue(report.text.contains("两份快照文本一致：是"), report.text)
        XCTAssertEqual(harness.backendStopCount, 0, "探针绝不能碰 session")
    }

    /// 探针默认不报视口尺寸（`announceViewport: false`）：报一个尺寸就等于把在跑的
    /// TUI 按那个宽度重排一次 + 给 agent 发一次 SIGWINCH。看一眼画面不该有副作用。
    func test_探针不改动在跑session的终端尺寸() throws {
        let harness = try ProbeHarness(capabilities: capabilities)
        harness.registerSession(id: "s1", screen: "hi")

        _ = try SessionDaemonAttachProbe.run(
            options: .init(sessionId: "s1", reconnect: true, cols: 80, rows: 25,
                           maxLines: 200),
            timeout: 5,
            connect: harness.connect)

        XCTAssertEqual(harness.backendResizes, [], "探针一次 resize 都不该发")
    }

    func test_连不上时给出人能看懂的原因而不是静默() {
        struct Boom: Error {}
        XCTAssertThrowsError(
            try SessionDaemonAttachProbe.run(
                options: .init(sessionId: "s1", reconnect: false, cols: 80, rows: 25,
                               maxLines: 200),
                timeout: 1,
                connect: { throw Boom() })
        ) { error in
            let message = String(describing: error)
            XCTAssertFalse(message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            XCTAssertTrue(message.contains("连不上") || message.contains("后台"),
                          "连不上后台要说人话：\(message)")
        }
    }

    /// daemon 在，但这个 id 不在它的名单里 —— 整条路要在 attach 之前就停住并报错，
    /// 不许挂在那儿等一份永远不会来的快照。
    func test_真socket上attach不存在的session不静默挂住() throws {
        let harness = try ProbeHarness(capabilities: capabilities)
        harness.registerSession(id: "worker-real", screen: "x")

        XCTAssertThrowsError(
            try SessionDaemonAttachProbe.run(
                options: .init(sessionId: "worker-fake", reconnect: false, cols: 80,
                               rows: 25, maxLines: 200),
                timeout: 5,
                connect: harness.connect)
        ) { error in
            let message = String(describing: error)
            XCTAssertTrue(message.contains("worker-fake"), message)
            XCTAssertTrue(message.contains("worker-real"), message)
        }
    }

    // MARK: - argv

    func test_argv解析() {
        XCTAssertEqual(SessionDaemonAttachOptions.parse(["PendingCrew"]), .notRequested)

        XCTAssertEqual(
            SessionDaemonAttachOptions.parse(
                ["PendingCrew", "--daemon-attach", "worker-bf39513e"]),
            .options(.init(sessionId: "worker-bf39513e", reconnect: false,
                           cols: SessionDaemonAttachOptions.defaultCols,
                           rows: SessionDaemonAttachOptions.defaultRows,
                           maxLines: SessionDaemonAttachOptions.defaultMaxLines)))

        XCTAssertEqual(
            SessionDaemonAttachOptions.parse(
                ["PendingCrew", "--daemon-attach", "s1", "--attach-reconnect",
                 "--attach-cols", "120", "--attach-rows", "40", "--attach-max-lines", "7"]),
            .options(.init(sessionId: "s1", reconnect: true, cols: 120, rows: 40,
                           maxLines: 7)))

        guard case let .invalid(message) =
                SessionDaemonAttachOptions.parse(["PendingCrew", "--daemon-attach"]) else {
            return XCTFail("缺 session id 必须是 invalid，不能当没请求过")
        }
        XCTAssertTrue(message.contains("--daemon-attach"), message)
    }

    // MARK: - 器材

    private static func summary(id: String, kind: LocalCodingAgentKind) -> SessionSummary {
        .init(sessionId: id, stateSeq: 1,
              state: .init(status: .running, isWorking: false, displayIsTyping: false,
                           health: nil, pendingDecision: nil, kind: kind.rawValue,
                           launchParameterProblem: nil, scrollState: nil))
    }
}

/// 一台**真跑在 unix socket 上**的假 daemon：每次 `connect()` 造一对新 socket 并
/// 接进同一个 server —— 于是「断开重连」在测试里也是真的换了一条链路。
@MainActor
private final class ProbeHarness {
    let server: SessionProtocolServer
    private(set) var connectCount = 0
    private var pairs: [(app: UnixSocketTransport, daemon: UnixSocketTransport)] = []
    private var backends: [ProtocolTestBackend] = []

    init(capabilities: [String]) throws {
        server = SessionProtocolServer(capabilities: capabilities, daemonBuild: "test-daemon")
    }

    deinit {
        MainActor.assumeIsolated {
            for pair in pairs { pair.app.close(); pair.daemon.close() }
        }
    }

    var backendResizes: [TerminalSize] { backends.flatMap(\.resizes) }
    var backendStopCount: Int { backends.reduce(0) { $0 + $1.stopCount } }

    /// 用一台无画面终端造出**真的**快照字节（不是手搓的假字节流）——
    /// 这样这条测试量的是整条路，而不是「我发了什么就收到什么」。
    func registerSession(id: String, screen: String, cols: Int = 80, rows: Int = 25) {
        let terminal = HeadlessTerminalHarness(cols: cols, rows: rows)
        terminal.feed(screen)
        let backend = ProtocolTestBackend(kind: .claudeCode)
        backend.terminalSnapshot = TerminalSnapshotEncoder.encode(terminal.terminal,
                                                                  probe: terminal.probe)
        backends.append(backend)
        server.register(sessionId: id, backend: backend)
    }

    func connect() throws -> any SessionMessageLink {
        let pair = try UnixSocketTransport.makePair()
        pairs.append(pair)
        connectCount += 1
        server.accept(link: pair.daemon)
        return pair.app
    }
}
#endif
