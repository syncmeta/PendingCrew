#if os(macOS)
import Foundation
import SwiftTerm

/// **无界面 viewer 探针的内核**（P5a 的证据工具，CLI 入口在
/// `SessionDaemonAttachMain`）。
///
/// 要证的那条链是：**viewer 断开 → 重连 → 同一个 session 的画面恢复**。
/// 在这之前它只在带窗口的 app 上成立过，而「开窗口才验得了」等于没有可复跑的证据。
/// 所以这里把 viewer 那条路上除画面之外的每一步原样跑一遍 ——
/// 连 socket、握手、`attach`、收快照分片、把快照还原成画面 —— 只是最后一步不画到
/// 屏幕上，而是喂进一台**无画面 `Terminal`** 再把文本取出来。
///
/// 三条纪律钉在这里，不是可选项：
/// 1. **不报视口尺寸**（`announceViewport: false`）。报一个尺寸 = 把在跑的 TUI 按那个
///    宽度重排一次 + 给 agent 发一次 SIGWINCH。看一眼画面不该有副作用。
/// 2. **不拉起 daemon**。连不上就说连不上；探针把后台拉起来，「后台在不在」这个
///    问题就永远问不出真话了。
/// 3. **绝不静默**。连不上 / 没有这个 session / 等不到快照，各有各的人话，
///    并且都在 attach **之前**能判的就在之前判 —— 服务端 attach 一个不存在的
///    session 是 `guard ... else { return }`，静默返回，客户端只会等到天荒地老。

// MARK: - 快照字节 → 屏幕文本

/// 快照字节流 → 文本。**纯函数**：不要 daemon、不要窗口、不碰 AppKit。
///
/// 取文本用的是 `TerminalScreenText` —— **与权威那份 `AgentSessionCore.screenText`
/// 同一个函数**。两边不是「同一套写法」而是同一段代码：不然探针打出来的画面就不是
/// daemon 里那份画面（2026-09-04 真机上就是这么翻的车，见那个类型的注释）。
enum SessionSnapshotTextRenderer {

    /// - Parameters:
    ///   - cols/rows: **本地渲染尺寸**，不是 session 的真实尺寸。协议里 `attached`
    ///     不带尺寸，而探针又刻意不报视口（见类型注释第 1 条），所以真实尺寸是问不到
    ///     的；由调用方明确给一个，并在输出里写清这是本地尺寸。
    static func render(snapshotBytes: [UInt8], cols: Int, rows: Int, maxLines: Int) -> String {
        guard !snapshotBytes.isEmpty else { return "" }
        // delegate 是 weak（`Terminal.tdel` 没有 public setter），必须本地持有到用完。
        let sink = SnapshotRenderSink()
        var options = TerminalOptions.default
        options.cols = max(2, cols)
        options.rows = max(1, rows)
        options.scrollback = AgentSessionCore.scrollbackLines
        let terminal = Terminal(delegate: sink, options: options)
        terminal.feed(byteArray: snapshotBytes)

        return TerminalScreenText.screen(of: terminal, maxLines: max(1, maxLines))
    }
}

/// 快照里可能带 DECRQM 之类要回话的序列；没有 delegate 时 SwiftTerm 无处送回程字节。
/// 探针不往 daemon 回写任何东西，所以这里把回程原地丢掉 —— 但**必须有人接**。
private final class SnapshotRenderSink: TerminalDelegate {
    func send(source: Terminal, data: ArraySlice<UInt8>) {}
}

// MARK: - argv

struct SessionDaemonAttachOptions: Equatable {
    static let flag = "--daemon-attach"
    static let reconnectFlag = "--attach-reconnect"
    static let colsFlag = "--attach-cols"
    static let rowsFlag = "--attach-rows"
    static let maxLinesFlag = "--attach-max-lines"

    /// 本地渲染尺寸的默认值。**故意给得比常见窗口宽**：源画面比渲染终端窄时只是
    /// 右边留白，反过来（渲染终端更窄）会把长行折断，那才是把证据弄脏。
    static let defaultCols = 200
    static let defaultRows = 50
    static let defaultMaxLines = 200

    var sessionId: String
    var reconnect: Bool
    var cols: Int
    var rows: Int
    var maxLines: Int

    enum ParseResult: Equatable {
        /// argv 里没有 `--daemon-attach`：这一趟不归探针管。
        case notRequested
        /// 是冲着探针来的，但参数不对 —— **必须与 `notRequested` 分开**，
        /// 否则「敲错一个参数」会静默掉进 GUI 分支，人看到的是「怎么开了个窗口」。
        case invalid(String)
        case options(SessionDaemonAttachOptions)
    }

    static let usage = """
        用法：PendingCrew \(flag) <sessionId> [\(reconnectFlag)] \
        [\(colsFlag) N] [\(rowsFlag) N] [\(maxLinesFlag) N]
          \(reconnectFlag)  attach 拿到画面后断开、重连、再拿一份，两份都打
          \(colsFlag)/\(rowsFlag)      本地渲染尺寸（默认 \(defaultCols)×\(defaultRows)）；\
        探针不会改动在跑 session 的终端尺寸
          \(maxLinesFlag)   每份画面最多打多少行（默认 \(defaultMaxLines)）
        """

    static func parse(_ argv: [String]) -> ParseResult {
        guard let index = argv.firstIndex(of: flag) else { return .notRequested }
        guard index + 1 < argv.count, !argv[index + 1].hasPrefix("-") else {
            return .invalid("\(flag) 后面要跟 session id（形如 worker-bf39513e）。\n\(usage)")
        }
        var options = Self(sessionId: argv[index + 1], reconnect: argv.contains(reconnectFlag),
                           cols: defaultCols, rows: defaultRows, maxLines: defaultMaxLines)
        for (name, target) in [(colsFlag, \Self.cols), (rowsFlag, \Self.rows),
                               (maxLinesFlag, \Self.maxLines)] {
            guard let at = argv.firstIndex(of: name) else { continue }
            guard at + 1 < argv.count, let value = Int(argv[at + 1]), value > 0 else {
                return .invalid("\(name) 后面要跟一个正整数。\n\(usage)")
            }
            options[keyPath: target] = value
        }
        return .options(options)
    }
}

// MARK: - 结果

struct SessionDaemonAttachReport {
    struct Screen {
        var label: String
        var cols: Int
        var rows: Int
        /// 这一份快照的原始字节数。**0 字节等于没证据**，所以如实打出来。
        var byteCount: Int
        /// 这一份是**对话记录**（codex）还是**终端画面**（claude / 纯终端）。
        var isTranscript = false
        /// transcript 的条数（终端那份恒为 0）。
        var itemCount = 0
        var text: String
    }

    var sessionId: String
    var kind: LocalCodingAgentKind
    var hello: SessionDaemonHello?
    var screens: [Screen]

    /// 两份的文本是否一致 —— 只有开了重连开关时才有意义。
    var screensMatch: Bool? {
        guard screens.count == 2 else { return nil }
        return screens[0].text == screens[1].text
    }

    /// codex 那份是**对话记录**，不是画面。措辞上一个字都不能含糊 —— 不然读的人会
    /// 以为 codex 也有终端快照，而它根本没有终端。
    private var matchPhrase: String {
        screens.first?.isTranscript == true ? "两份 transcript 文本一致" : "两份快照文本一致"
    }

    var text: String {
        var lines: [String] = ["session：\(sessionId)（\(kind.rawValue)）"]
        if let hello {
            lines.append("后台：PID \(hello.pid) · \(hello.daemonBuild)"
                + "（协议 \(hello.protocolVersion)）")
        }
        for (index, screen) in screens.enumerated() {
            let measure = screen.isTranscript
                ? "\(screen.itemCount) 条 · 这是**对话记录**，不是终端画面（codex 没有终端）"
                : "\(screen.byteCount) 字节 · 本地渲染 \(screen.cols)×\(screen.rows)"
            lines.append("")
            lines.append("── 第 \(index + 1) 份\(screen.isTranscript ? "transcript" : "快照")"
                + "（\(screen.label)）· \(measure) ──")
            lines.append(screen.text)
        }
        if let screensMatch {
            lines.append("")
            lines.append("\(matchPhrase)：\(screensMatch ? "是" : "否")")
        }
        return lines.joined(separator: "\n")
    }
}

// MARK: - 探针

@MainActor
enum SessionDaemonAttachProbe {
    /// 握手时报给 daemon 的 app build 名。日志里一眼看得出这条连接是探针，不是真 app。
    static let appBuild = "daemon-attach"

    enum AttachError: Error, CustomStringConvertible {
        case cannotConnect(socket: String?, reason: String)
        case handshakeTimeout(TimeInterval)
        case missingTerminalBytes([String])
        case unknownSession(requested: String, available: [String])
        case missingTranscriptEvents([String])
        case snapshotTimeout(sessionId: String, seconds: TimeInterval)
        case transcriptTimeout(sessionId: String, seconds: TimeInterval)

        var description: String {
            switch self {
            case let .cannotConnect(socket, reason):
                let where_ = socket.map { "（socket：\($0)）" } ?? ""
                return "连不上 PendingCrew 后台\(where_)：\(reason)\n"
                    + "后台没在跑的话先起它；探针**故意不替你拉起** —— "
                    + "那样「后台在不在」就再也问不出真话了。"
            case let .handshakeTimeout(seconds):
                return "连上了 socket，但 \(Int(seconds)) 秒内没等到后台的握手回应。"
                    + "后台可能卡住了，也可能协议版本对不上（新 app 连旧后台）。"
            case let .missingTerminalBytes(negotiated):
                return "后台不支持 terminal-bytes 能力，拿不到终端快照。"
                    + "协商到的能力：\(negotiated.isEmpty ? "（空）" : negotiated.joined(separator: "、"))"
            case let .unknownSession(requested, available):
                let list = available.isEmpty
                    ? "（后台里一个 session 都没有）"
                    : available.joined(separator: "\n  - ")
                return "后台里没有这个 session：\(requested)\n"
                    + "后台当前有：\n  - \(list)"
            case let .missingTranscriptEvents(negotiated):
                return "后台不支持 transcript-events 能力，拿不到 codex 的对话记录。"
                    + "协商到的能力：\(negotiated.isEmpty ? "（空）" : negotiated.joined(separator: "、"))"
            case let .snapshotTimeout(sessionId, seconds):
                return "attach 上了 \(sessionId)，但 \(Int(seconds)) 秒内没收到完整快照。"
                    + "后台可能正在背压重同步，也可能这个 session 的权威终端还没建起来。"
            case let .transcriptTimeout(sessionId, seconds):
                return "attach 上了 \(sessionId)（codex），但 \(Int(seconds)) 秒内"
                    + "没等到后台把这条连接的 session 名单发回来 —— transcript 重放就排在它前面，"
                    + "所以这时候手上那份对话记录**可能是不全的**，不能当证据用。"
            }
        }
    }

    /// **attach 之前**就把「这个 id 到底在不在、是什么 kind」判掉。
    ///
    /// 服务端 `attach` 对不存在的 session 是静默 `return`：不回 `attached`、不回错误。
    /// 所以这一步不做的话，症状就是探针挂在那儿等一份永远不会来的快照 ——
    /// 那正是「绝不静默」要挡的形状。
    static func resolveTarget(sessionId: String,
                             in sessions: [SessionSummary]) throws -> LocalCodingAgentKind {
        guard let summary = sessions.first(where: { $0.sessionId == sessionId }) else {
            throw AttachError.unknownSession(requested: sessionId,
                                             available: sessions.map(\.sessionId).sorted())
        }
        return LocalCodingAgentKind(rawValue: summary.state.kind) ?? .claudeCode
    }

    /// 走真 socket 的那条便利入口（CLI 用）。**不拉起 daemon。**
    static func run(options: SessionDaemonAttachOptions,
                    paths: PendingCrewDaemonPaths = .standard(),
                    timeout: TimeInterval = 5) throws -> SessionDaemonAttachReport {
        try run(options: options, timeout: timeout, socketPath: paths.socket) {
            try UnixSocketTransport.connect(toPath: paths.socket)
        }
    }

    /// - Parameter connect: 每调用一次给**一条新链路**。重连那一趟会再调一次 ——
    ///   这就是「断开重连」在这里是真的换了条连接、而不是同一条链路上再握一次手。
    ///
    ///   （`SessionProtocolClient.reconnect()` 走不了这条路：那个 client 的 `link`
    ///   是 `let`，它服务的是「同一条链路断了又回来」的同进程桥。socket 上真断开
    ///   就是 fd 没了，只能新开一条 —— 换新连接是更硬的证据，不是绕过。）
    static func run(options: SessionDaemonAttachOptions,
                    timeout: TimeInterval = 5,
                    socketPath: String? = nil,
                    connect: () throws -> any SessionMessageLink)
        throws -> SessionDaemonAttachReport {
        var links: [any SessionMessageLink] = []
        defer { links.forEach { $0.close() } }
        var hello: SessionDaemonHello?
        var kind: LocalCodingAgentKind = .claudeCode

        func capture(label: String) throws -> SessionDaemonAttachReport.Screen {
            let link: any SessionMessageLink
            do {
                link = try connect()
            } catch {
                throw AttachError.cannotConnect(socket: socketPath,
                                                reason: String(describing: error))
            }
            links.append(link)
            let client = SessionProtocolClient(
                link: link, capabilities: SessionDaemonHost.defaultCapabilities,
                appBuild: appBuild)
            var list: SessionList?
            var listCount = 0
            client.onDaemonHello = { hello = $0 }
            client.onSessionList = { list = $0; listCount += 1 }
            client.connect()
            client.requestSessionList()
            guard wait(timeout, until: { client.isConnected && list != nil }) else {
                throw AttachError.handshakeTimeout(timeout)
            }
            kind = try resolveTarget(sessionId: options.sessionId,
                                     in: list?.sessions ?? [])
            let needed = kind == .codex ? "transcript-events" : "terminal-bytes"
            guard client.negotiatedCapabilities.contains(needed) else {
                throw kind == .codex
                    ? AttachError.missingTranscriptEvents(client.negotiatedCapabilities)
                    : AttachError.missingTerminalBytes(client.negotiatedCapabilities)
            }

            // announceViewport: false —— 见类型注释第 1 条。rendersLocally: false ——
            // 这个进程里一个 NSView 都不该有（mirror 是 AppKit 视图）。
            let listsBeforeAttach = listCount
            let remote = client.attach(sessionId: options.sessionId, kind: kind,
                                       announceViewport: false, rendersLocally: false)
            let screen: SessionDaemonAttachReport.Screen
            if kind == .codex {
                // codex 没有终端 —— 服务端在 attach 里把整份 reduced transcript 当
                // `codexNotification` 重放过来，**然后**才发这条连接的 session 名单
                // （`SessionProtocolServer.attach` 的末尾）。同一条有序字节流上，
                // 「名单到了」就等于「重放发完了」—— 这是个顺序判据，不是等一会儿
                // 看看够不够的时序赌。
                guard wait(timeout, until: { listCount > listsBeforeAttach }) else {
                    throw AttachError.transcriptTimeout(sessionId: options.sessionId,
                                                        seconds: timeout)
                }
                let items = remote.transcript?.items ?? []
                screen = .init(label: label, cols: options.cols, rows: options.rows,
                               byteCount: 0, isTranscript: true, itemCount: items.count,
                               text: CodexTranscriptText.render(items: items,
                                                                maxLines: options.maxLines))
            } else {
                guard wait(timeout, until: { remote.completedSnapshotCount > 0 }) else {
                    throw AttachError.snapshotTimeout(sessionId: options.sessionId,
                                                      seconds: timeout)
                }
                let bytes = remote.lastCompletedSnapshotBytes
                screen = .init(label: label, cols: options.cols, rows: options.rows,
                               byteCount: bytes.count,
                               text: SessionSnapshotTextRenderer.render(
                                snapshotBytes: bytes, cols: options.cols, rows: options.rows,
                                maxLines: options.maxLines))
            }
            // detach = 本端把链路关掉：服务端 `dropConnection` 会把这条连接上的
            // handle 全作废，并在日志里记「viewer 断开；session 不受影响」。
            link.close()
            return screen
        }

        var screens = [try capture(label: "首次 attach")]
        if options.reconnect { screens.append(try capture(label: "断开后重连")) }
        return .init(sessionId: options.sessionId, kind: kind, hello: hello, screens: screens)
    }

    /// 转 runloop 等条件成立。socket 的读回调走 DispatchIO → 主队列，所以必须真转。
    private static func wait(_ timeout: TimeInterval, until condition: () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() {
            if Date() >= deadline { return false }
            _ = RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.01))
        }
        return true
    }
}
#endif
