#if os(macOS)
import XCTest

/// **「期望状态没达成」不许被编码成成功**（2026-09-04 由一条写在前面的预测逮到）。
///
/// 病历：对着一个「持锁 + 只 accept 不回一个字节」的假监听跑 `--daemon-status`，
/// 实测
/// ```
/// PendingCrew 后台未运行或不可连接：后台已占用 socket，但状态握手超时
/// 退出码=0
/// ```
/// —— **它说了「不可连接」，却报成功。** 任何脚本化的用法都分不出「后台好着呢」
/// 和「后台连不上」；`if PendingCrew --daemon-status; then …` 会一路走进 then。
///
/// 同一个毛病在 daemon 那一侧也成立：拿不到锁与锁文件打不开**都** exit 0。
///
/// ## 判据只有一条
///
/// **0 = 期望状态成立**（拉起方要的东西已经在那儿了），**非 0 = 没成立**。
/// 按这条判，「锁被另一个 daemon 占着」该是 0（已经有一个 daemon 在跑，本进程
/// 安静退出是正确结局），而「锁被 app 窗口占着」不是 0（**一个 daemon 都没有**）。
final class DaemonExitCodeTests: XCTestCase {

    /// 锁被**另一个 daemon** 占着 —— 期望状态已经成立，安静退出是正确结局。
    func test_已经有另一个daemon在跑时退出码是0() {
        XCTAssertEqual(
            DaemonExitCode.forDaemonStart(
                .alreadyOrchestrated("另一个 daemon 在管这个数据根", holderIsDaemon: true)),
            0)
    }

    /// 锁被**app 窗口**占着（2026-08-26 那次事故的形状）—— 拉起方要一个 daemon，
    /// 而现在**一个都没有**。报成功等于骗它。
    func test_锁被app窗口占着时不是成功() {
        XCTAssertNotEqual(
            DaemonExitCode.forDaemonStart(
                .alreadyOrchestrated("一个 app 窗口占着", holderIsDaemon: false)),
            0,
            "拉起方要的是 daemon，而现在一个都没有 —— 报 0 就是谎报成功")
    }

    /// 锁文件根本打不开（数据根不可写）—— 真机上 `chmod 500` 复现到的那一条。
    func test_锁文件打不开时不是成功() {
        XCTAssertNotEqual(
            DaemonExitCode.forDaemonStart(.lockUnavailable("打不开 …/orchestrator.lock：Permission denied")),
            0)
    }

    /// socket 监听失败 —— 同样没达成。
    func test_监听失败时不是成功() {
        XCTAssertNotEqual(
            DaemonExitCode.forDaemonStart(.listen(CocoaError(.fileWriteNoPermission))), 0)
    }

    /// `--daemon-status` 问不出实况 —— **说了不可连接就不许报成功**。
    func test_状态问不出来时不是成功() {
        XCTAssertNotEqual(DaemonExitCode.statusProbeFailed, 0,
                          "说了「不可连接」却报成功，脚本一路走进 then")
    }

    /// 三个码互不相同，且 `badUsage` 与 `--daemon-attach` 已有的约定一致（2）。
    func test_码本身别互相打架() {
        XCTAssertEqual(DaemonExitCode.ok, 0)
        XCTAssertNotEqual(DaemonExitCode.failed, DaemonExitCode.ok)
        XCTAssertEqual(DaemonExitCode.badUsage, 2)
    }
}
#endif
