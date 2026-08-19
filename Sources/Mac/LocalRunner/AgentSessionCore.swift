#if os(macOS)
import Foundation
import Combine
import SwiftTerm

/// **无画面的终端内核**（spec `docs/2026-08-19-backend-split-design.md` §5.2）。
///
/// 在这个类型出现之前，PTY、屏幕缓冲区、渲染、以及「它在忙 / 撞额度了 / 卡在
/// 选择菜单等人按」那套从画面上认状态的逻辑，全长在**同一个 AppKit 视图对象**上、
/// 全跑主线程。那既是「关掉 app 就全停」的一半，也是 tech-debt 里
/// 「PTY 每批输出都要过主线程、代价随 session 数线性涨」那条的结构成因。
///
/// 现在它们在这里，而这里**没有一行 AppKit**：`Terminal` 与 `LocalProcess` 都是
/// 纯 Foundation（SwiftTerm 自带的 `HeadlessTerminal` 就是这个组合）。
/// 所以 P4 时这个类可以原样搬进没有画面的后台进程。
///
/// ⚠️ **不许在本文件 import AppKit / SwiftUI。** 有一条测试盯着这件事。
@MainActor
final class AgentSessionCore: NSObject, TerminalDelegate, LocalProcessDelegate {

    /// 内核的两种用法。**不是配置项，是两套本来就不同的行为**，劈分前分别长在
    /// `AgentTerminalSession` 与 `PlainTerminalSession` 上，这里逐字保留：
    /// - `.agent`：全套状态扫描器 + 拉起自检看门狗；退出码原样交出（不解 wait status）。
    /// - `.plainShell`：人直接操作的 shell —— 什么都不扫（普通 shell 的输出里出现
    ///   「usage limit」之类字样是它自己的事，不该翻成 session 健康异常），
    ///   退出码按 `wait(2)` 位布局解码，spawn 失败当场落 `.exited(127)`。
    enum Mode {
        case agent
        case plainShell
    }

    /// 回滚历史行数上限。理由与代价见 `TerminalMirrorView` 上同名常量的长注释
    /// （两侧必须一致，否则 mirror 能滚到的历史比 core 记得的多/少）。
    static let scrollbackLines = 10_000

    let kind: LocalCodingAgentKind
    private let mode: Mode

    private(set) var terminal: Terminal!
    private(set) var process: LocalProcess!

    /// mirror 挂这里拿字节。**core 自己不知道有没有人在看** —— 没人挂就没人收，
    /// 那正是 P4 之后「没人看的 session 不占主线程」的形状。
    var onOutput: ((ArraySlice<UInt8>) -> Void)?

    /// `status` 翻成 `.exited` 的那一拍（门面据此收窄两份回滚缓冲）。
    var onExited: (() -> Void)?

    /// 最近一次收到子进程输出的时刻；busy 判定 = now - lastOutputAt < 阈值。
    private(set) var lastOutputAt = Date.distantPast
    /// 当前视口尺寸（没有 mirror 在看时保持最后一次的值，**绝不 resize 成 0**）。
    /// 初值对齐 `TerminalOptions.default` 的 80×25 —— 劈分前视图以 `.zero` frame
    /// 构造、零尺寸守卫让它停在同一个默认值上，PTY 拿到的初始 winsize 因此不变。
    private(set) var cols = TerminalOptions.default.cols
    private(set) var rows = TerminalOptions.default.rows

    var isProcessRunning: Bool { process?.running == true }

    /// 进程终止的那一拍顺手把回滚缓冲收窄（2026-08-18 第二条）——
    /// 挂在 `didSet` 上而不是各个退出路径里：正常退出 / 用户停 / 拉起失败自检
    /// 三条路都各自翻 `status`，逐个去加容易漏，且漏掉的那条就是内存不放的那条。
    @Published private(set) var status: SessionStatus = .running {
        didSet {
            if case .exited = status { onExited?() }
        }
    }

    /// `executable` = 已 resolve 的 claude/codex 绝对路径（`.plainShell` 时是用户的
    /// 登录 shell）；`workdir` = 工作目录。argv 由 `config.argv()` 构建。
    init(config: SessionConfig,
         mode: Mode = .agent,
         executable: String,
         workdir: String,
         env: [String: String]) {
        self.kind = config.kind
        self.mode = mode
        super.init()

        var opts = TerminalOptions.default
        opts.scrollback = Self.scrollbackLines
        terminal = Terminal(delegate: self, options: opts)
        // dispatchQueue 留空 = `DispatchQueue.main`，与劈分前 `LocalProcessTerminalView`
        // 的默认完全一致。P1 仍是单进程、字节仍走主线程；把它挪到 per-session 队列
        // 是 P4 的事（那时候没有主线程可挪回去了）。
        process = LocalProcess(delegate: self)

        var envArr = env.map { "\($0.key)=\($0.value)" }
        if !envArr.contains(where: { $0.hasPrefix("TERM=") }) { envArr.append("TERM=xterm-256color") }
        process.startProcess(
            executable: executable,
            args: config.argv(),
            environment: envArr,
            currentDirectory: workdir)

        if mode == .plainShell, !isProcessRunning || process.shellPid <= 0 {
            status = .exited(127)
        }
    }

    // MARK: - LocalProcessDelegate

    func processTerminated(_ source: LocalProcess, exitCode: Int32?) {
        handleProcessExit(rawExitCode: exitCode)
    }

    /// PTY 有新字节：**先喂自己那份权威缓冲区，再转给看的人**。顺序不能反 ——
    /// mirror 可能不存在，权威那份必须无条件收到。
    func dataReceived(slice: ArraySlice<UInt8>) {
        terminal.feed(buffer: slice)
        lastOutputAt = Date()
        scanOutput(slice)
        onOutput?(slice)
    }

    /// 报给 PTY 的窗口尺寸。像素维度报 16×16 的常量（SwiftTerm 自带的
    /// `HeadlessTerminal` 也是这么报的）—— 真实 cell 尺寸只有视图知道，而
    /// SwiftTerm 的 `cellDimension` 是 internal，模块外拿不到。行列数是准的，
    /// TUI 只吃这两个；像素维度仅图形协议（sixel / kitty）会看，两个 agent 都不用。
    func getWindowSize() -> winsize {
        winsize(ws_row: UInt16(rows), ws_col: UInt16(cols), ws_xpixel: 16, ws_ypixel: 16)
    }

    // MARK: - TerminalDelegate

    /// 终端要往「主机」写字节（回复设备查询之类）——原路送回子进程。
    /// **这是权威那份 Terminal 的回复，唯一一份**：mirror 那份的同名回复被它自己
    /// 拦掉了（见 `TerminalMirrorView.send(source:data:)` 上的注释），否则同一个
    /// 设备查询会被答两次。
    func send(source: Terminal, data: ArraySlice<UInt8>) {
        process.send(data: data)
    }

    // MARK: - 控制面

    /// 写字节进 PTY（键盘、程序化 send、菜单按键都走这里）。
    func write(_ bytes: [UInt8]) {
        guard isProcessRunning else { return }
        process.send(data: bytes[...])
    }

    /// 视口尺寸变了：同步自己那份 `Terminal`，并把 winsize 推给 PTY。
    /// 零/退化尺寸一律忽略 —— 2 列下 reflow 会把历史按 2 字宽重排、顶部被永久裁掉
    /// （Todo #34 实测：100 列 400 行历史过一次退化尺寸，10000 行上限下只剩 223 行，
    /// 且首行断在半截）。守卫在 mirror 侧也有一道，这里是第二道。
    func resize(cols newCols: Int, rows newRows: Int) {
        guard newCols >= 2, newRows >= 1 else { return }
        guard newCols != cols || newRows != rows else { return }
        cols = newCols
        rows = newRows
        terminal.resize(cols: newCols, rows: newRows)
        guard isProcessRunning, process.childfd >= 0 else { return }
        var size = getWindowSize()
        _ = PseudoTerminalHelpers.setWinSize(masterPtyDescriptor: process.childfd, windowSize: &size)
    }

    /// 收窄权威那份的回滚缓冲（进程终止后，门面用 mirror 算出的保留行数调）。
    func changeScrollback(_ lines: Int) {
        terminal.changeScrollback(lines)
    }

    /// **权威画面**（`inspect_session` 用）：当前屏幕内容，与任何窗口滚到哪无关。
    func screenText(maxLines: Int) -> String {
        var lines: [String] = []
        for row in 0..<terminal.rows {
            guard let line = terminal.getLine(row: row) else { continue }
            lines.append(line.translateToString(trimRight: true))
        }
        while let last = lines.last, last.trimmingCharacters(in: .whitespaces).isEmpty {
            lines.removeLast()
        }
        return lines.suffix(maxLines).joined(separator: "\n")
    }

    func stop() {
        guard status == .running else { return }
        let pid = process.shellPid
        process.terminate()             // SIGTERM + close PTY（但不回调 processTerminated）
        status = .exited(nil)           // ← 自己翻状态，否则 UI 永远 running
        Task { await terminateTree(pid: pid, graceSeconds: 2.0) }
    }

    // MARK: - Task 2 会填进来的钩子

    /// PTY 输出旁路（六个状态扫描器）。Task 2 之前是空的。
    private func scanOutput(_ slice: ArraySlice<UInt8>) {}

    /// 子进程退出的统一入口。
    private func handleProcessExit(rawExitCode: Int32?) {
        guard status == .running else { return }
        status = .exited(rawExitCode.map(Self.decodeWaitStatus))
    }

    /// SwiftTerm 的 `processTerminated` 把 **`waitpid` 的原始 status** 原样当
    /// exitCode 交出来（没走 WEXITSTATUS）—— 于是 exit 2 会显示成 512、exit 127
    /// 显示成 32512。给人看的原因里必须是真的退出码，这里解一次。
    static func decodeWaitStatus(_ st: Int32) -> Int32 {
        (st & 0x7f) == 0 ? (st >> 8) & 0xff : st & 0x7f
    }
}
#endif
