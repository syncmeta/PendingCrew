#if os(macOS)
import XCTest

/// **daemon 形态的「零字节半死」端到端**（2026-09-06 之后补的窟窿）。
///
/// 为什么单开一组、而不是靠 `AgentTerminalLaunchFailureTests`：
///
/// 1. 那组测的是**门面**（`AgentTerminalSession`），daemon 里跑的是
///    `HeadlessSessionBackend`。「两条路共用同一个 `AgentSessionCore`」这句话是真的，
///    但**这正是今天那个 bug 的原话** —— 坏掉的不是内核，是观察者：门面那条路上的
///    观察者认得出后端，daemon 那条路上的认不出。所以「内核相同」推不出
///    「两条路行为相同」，同一个论证不能用来豁免它的兄弟机制。
/// 2. 那组只覆盖了 `spawnFailed`（可执行文件不存在，秒退）、健康常驻、主动停三档。
///    **`stalled`（进程活着、一个字节都不吐）一条都没有** —— 而这正是这条线上
///    人类最早那句「为什么 claude session 都卡住了」的那一档。它没被写，多半是因为
///    生产观察窗是 25 秒，没人愿意写一条要等 25 秒的测试；所以观察窗现在是构造参数。
@MainActor
final class HeadlessLaunchFailLoudTests: XCTestCase {

    /// 起得来、活着、但**一个字节都不吐**。`exec sleep` 而不是随手挑个系统命令：
    /// spawn 的 argv 是 claude 那套 flag，别的命令会把它们当参数报错、于是吐字。
    private func makeSilentButAlive() throws -> String {
        try makeExecutableScript("#!/bin/sh\nexec sleep 30\n")
    }

    /// daemon 形态：半死的 session 必须**喊出来**，而不是继续显示成「空闲」。
    func testStalledDaemonBackendShoutsInsteadOfLookingIdle() async throws {
        let backend = HeadlessSessionBackend(
            config: SessionConfig(kind: .claudeCode),
            mode: .agent,
            executable: try makeSilentButAlive(),
            workdir: NSTemporaryDirectory(),
            env: [:],
            launchDeadline: 1)
        defer { backend.stop() }

        try await waitUntil(timeout: 10) { backend.health != nil }

        XCTAssertEqual(backend.health?.kind, .launchFailed,
                       "daemon 里零字节的 session 必须翻 launchFailed —— "
                           + "看门狗在门面那条路上会喊，不代表它在这条路上也喊")
        let detail = backend.health?.detail ?? ""
        XCTAssertTrue(detail.contains("一个字都没输出"), "原因要说清是半死这一档：\(detail)")
        XCTAssertTrue(detail.contains("Claude Code"), "原因要说清是哪个工具：\(detail)")
        XCTAssertEqual(backend.status, .running,
                       "半死**故意不杀**：现场留给人看，靠 health 而不是靠杀进程来发声")
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: backend.status == .running,
                health: backend.health, isWorking: backend.isWorking),
            CrewSessionStateDerivation.launchFailed,
            "点名读到的必须是「拉起失败」，不是「空闲」—— 这条就是「不再静默」本身")
    }

    /// 门面形态同一档也要喊。**两个形态并排断言**，谁都不许被「内核相同」豁免。
    func testStalledFacadeBackendShoutsToo() async throws {
        let session = AgentTerminalSession(
            config: SessionConfig(kind: .claudeCode),
            executable: try makeSilentButAlive(),
            workdir: NSTemporaryDirectory(),
            env: [:],
            launchDeadline: 1)
        defer { session.stop() }

        try await waitUntil(timeout: 10) { session.health != nil }

        XCTAssertEqual(session.health?.kind, .launchFailed)
        XCTAssertTrue((session.health?.detail ?? "").contains("一个字都没输出"))
        XCTAssertEqual(
            CrewSessionStateDerivation.state(
                isRunning: session.status == .running,
                health: session.health, isWorking: session.isWorking),
            CrewSessionStateDerivation.launchFailed)
    }

    /// 把观察窗压到 1 秒之后，**正常吐字的 session 仍然不许被误报**。
    /// 没有这条，上面两条可能是因为「窗口太短、什么都会被判半死」而绿的。
    func testHealthyDaemonBackendIsNotFlaggedEvenWithATinyWindow() async throws {
        let backend = HeadlessSessionBackend(
            config: SessionConfig(kind: .claudeCode),
            mode: .agent,
            executable: try makeExecutableScript(
                "#!/bin/sh\necho pendingcrew-alive\nsleep 30\n"),
            workdir: NSTemporaryDirectory(),
            env: [:],
            launchDeadline: 1)
        defer { backend.stop() }

        try await waitUntil(timeout: 8) { backend.hasObservedLaunchSignal }
        // 给看门狗几轮机会误报。
        try await Task.sleep(nanoseconds: 2_500_000_000)

        XCTAssertNil(backend.health, "见过输出的 session 不该被判拉起失败")
        XCTAssertEqual(backend.status, .running)
    }

    // MARK: -

    private func makeExecutableScript(_ body: String) throws -> String {
        let url = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("pendingcrew-stalled-\(UUID().uuidString).sh")
        try body.write(to: url, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o755], ofItemAtPath: url.path)
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        return url.path
    }

    private func waitUntil(
        timeout: TimeInterval, _ condition: @MainActor () -> Bool
    ) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
    }
}
#endif
