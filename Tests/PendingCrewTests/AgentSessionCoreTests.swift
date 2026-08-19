#if os(macOS)
import XCTest

/// `AgentSessionCore` 的 headless 集成测试（前后端分离 P1）。
///
/// 这些测试**真的起子进程、真的走 PTY**，但全程无画面 —— 这正是要证明的事：
/// 终端内核不需要 AppKit 也能跑，所以它可以整个搬进后台进程。
@MainActor
final class AgentSessionCoreTests: XCTestCase {

    /// 等某个条件成立，最多 `timeout` 秒。core 的输出是异步到达的。
    @discardableResult
    private func waitUntil(
        _ timeout: TimeInterval = 8, _ cond: @MainActor () -> Bool
    ) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if cond() { return true }
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
        return cond()
    }

    private func makeShellCore() -> AgentSessionCore {
        AgentSessionCore(
            config: SessionConfig(kind: .terminal),
            mode: .plainShell,
            executable: "/bin/sh",
            workdir: NSTemporaryDirectory(),
            env: ["TERM": "xterm-256color"])
    }

    func testRunsProcessAndCapturesScreenWithoutAnyView() async throws {
        let core = makeShellCore()
        defer { core.stop() }
        core.write(Array("echo PENDINGCREW_MARKER\n".utf8))

        let ok = await waitUntil { core.screenText(maxLines: 40).contains("PENDINGCREW_MARKER") }
        XCTAssertTrue(ok, "无画面 core 应当能读到子进程输出的画面")
    }

    func testOnOutputDeliversRawBytes() async throws {
        let core = makeShellCore()
        defer { core.stop() }
        let box = ByteBox()
        core.onOutput = { box.append($0) }
        core.write(Array("echo BYTES_HOOK\n".utf8))

        let ok = await waitUntil { box.text.contains("BYTES_HOOK") }
        XCTAssertTrue(ok, "onOutput 应当把原始 PTY 字节交出来（mirror 靠它画）")
    }

    func testStopEndsTheProcess() async throws {
        let core = makeShellCore()
        await waitUntil { core.isProcessRunning }
        XCTAssertTrue(core.isProcessRunning, "前置条件：子进程该先跑起来")
        core.stop()
        let stopped = await waitUntil { !core.isProcessRunning }
        XCTAssertTrue(stopped, "stop() 之后子进程不该还活着")
    }

    /// 视口尺寸是内核的状态，不是视图的 —— 没有 mirror 在看时也要能改，
    /// 且退化尺寸一律拦掉（2 列下 reflow 会把历史按 2 字宽重排、顶部被永久裁掉）。
    func testResizeIsGuardedAgainstDegenerateSizes() async throws {
        let core = makeShellCore()
        defer { core.stop() }

        core.resize(cols: 120, rows: 40)
        XCTAssertEqual(core.cols, 120)
        XCTAssertEqual(core.rows, 40)
        XCTAssertEqual(core.terminal.cols, 120, "内核那份 Terminal 要跟着改")
        XCTAssertEqual(core.terminal.rows, 40)

        core.resize(cols: 0, rows: 0)
        XCTAssertEqual(core.cols, 120, "零尺寸不许下传")
        XCTAssertEqual(core.rows, 40)
    }
}

/// 线程安全的字节累加器（`onOutput` 从 PTY 队列回调）。
private final class ByteBox: @unchecked Sendable {
    private let lock = NSLock()
    private var bytes: [UInt8] = []
    func append(_ slice: ArraySlice<UInt8>) {
        lock.lock(); bytes.append(contentsOf: slice); lock.unlock()
    }
    var text: String {
        lock.lock(); defer { lock.unlock() }
        return String(decoding: bytes, as: UTF8.self)
    }
}
#endif
