#if os(macOS)
import Foundation
import XCTest

/// 恢复弹窗的承重件：**上一次是怎么结束的**。
///
/// 这里钉的不是「能读能写」，是**四档各自不许串档**。串档的代价不对称：
/// 把崩溃判成正常 → 该问的时候不问，人丢了活还不知道为什么；
/// 把正常判成崩溃 → 多问一次，还好。所以凡是拿不准的地方都必须倒向「问」。
final class ProcessExitMarkerTests: XCTestCase {

    private func store(_ role: ProcessExitMarkerRole = .daemon) throws -> ProcessExitMarkerStore {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("pc-exit-\(UUID().uuidString)")
        addTeardownBlock {
            chmod(dir.path, 0o755)
            try? FileManager.default.removeItem(at: dir)
        }
        return ProcessExitMarkerStore(directory: dir, role: role)
    }

    private func mark(_ s: ProcessExitMarkerStore,
                      _ phase: ProcessExitMarker.Phase,
                      build: String = "0.1.34(1)") throws {
        let started = Date(timeIntervalSince1970: 1_000)
        switch phase {
        case .running:  try s.markRunning(pid: 42, startedAt: started, build: build)
        case .draining: try s.markDraining(pid: 42, startedAt: started, build: build)
        case .clean:    try s.markClean(pid: 42, startedAt: started, build: build)
        }
    }

    // MARK: - 四档，逐档钉

    /// **全新安装不该弹窗。** 只写收尾那两次的话，「从没跑过」和「崩在收尾之前」
    /// 在盘上长得一模一样，第一次开 app 就会问他要不要恢复。
    func testFirstEverRunIsNotOfferedARestore() throws {
        let s = try store()
        XCTAssertEqual(s.classifyPreviousRun(), .noPriorRun)
        XCTAssertFalse(s.classifyPreviousRun().shouldOfferRestore,
                       "第一次运行就弹「要恢复吗」")
    }

    func testCleanExitIsNotOfferedARestore() throws {
        let s = try store()
        try mark(s, .running)
        try mark(s, .draining)
        try mark(s, .clean)
        XCTAssertEqual(s.classifyPreviousRun(), .clean)
        XCTAssertFalse(s.classifyPreviousRun().shouldOfferRestore)
    }

    /// 起来了、还没开始收尾就没了 = 崩溃 / SIGKILL / 断电。
    func testDiedWhileRunningIsUnexpected() throws {
        let s = try store()
        try mark(s, .running)
        XCTAssertEqual(s.classifyPreviousRun(), .unexpected)
        XCTAssertTrue(s.classifyPreviousRun().shouldOfferRestore)
    }

    /// **单独一档，不许并进任何一边。** 并进「正常」会漏掉真出事的那次；
    /// 并进「崩溃」会把人自己按的停说成崩溃。
    func testDiedWhileDrainingIsItsOwnAnswer() throws {
        let s = try store()
        try mark(s, .running)
        try mark(s, .draining)
        XCTAssertEqual(s.classifyPreviousRun(), .diedWhileDraining)
        XCTAssertTrue(s.classifyPreviousRun().shouldOfferRestore)
        XCTAssertNotEqual(s.classifyPreviousRun(), .unexpected,
                          "收尾中途被打断和从没开始收尾，对人的意义不一样")
    }

    /// 每一档都得能对人说清「发生了什么」，不是只给个状态名。
    func testEveryClassificationSaysWhatHappened() {
        for c: ProcessExitClassification in [.noPriorRun, .clean, .diedWhileDraining, .unexpected] {
            XCTAssertFalse(c.text.isEmpty, "\(c)")
            XCTAssertGreaterThan(c.text.count, 5, "\(c) 的说明太短，等于没说：\(c.text)")
        }
    }

    // MARK: - 读不动 ≠ 没跑过

    /// **这条是这个文件里最重要的一条。**
    ///
    /// 三态读被压成可选值的话，「我读不到」会落进 `.absent` → `.noPriorRun` →
    /// **不问**。而 `.noPriorRun` 恰好是唯一「不问」的一档 —— 于是一次读失败
    /// 就变成「崩了也不问」，人丢了活还查不出为什么。
    func testUnreadableMarkerFallsToAskingNotToSilence() throws {
        try XCTSkipIf(getuid() == 0, "root 绕过权限位")
        let s = try store()
        try mark(s, .clean)                    // 盘上是「正常退出」
        XCTAssertEqual(s.classifyPreviousRun(), .clean)

        XCTAssertEqual(chmod(s.directory.path, 0), 0)   // 现在读不进去了
        defer { chmod(s.directory.path, 0o755) }

        XCTAssertEqual(s.classifyPreviousRun(), .unexpected,
                       "读不动被当成了「没跑过」或「正常退出」—— 两个都会让人在崩溃后收不到提示")
        XCTAssertTrue(s.classifyPreviousRun().shouldOfferRestore)
    }

    /// 半个 JSON（写到一半被打死）同样倒向「问」，不许静默当成没跑过。
    func testCorruptMarkerFallsToAsking() throws {
        let s = try store()
        try mark(s, .clean)
        try Data("{ 这不是 JSON".utf8).write(to: s.url)
        guard case let .unreadable(detail) = s.readPrevious() else {
            return XCTFail("解不开却没答成 unreadable")
        }
        XCTAssertTrue(detail.contains("解不开"), detail)
        XCTAssertEqual(s.classifyPreviousRun(), .unexpected)
    }

    /// 「文件不在」「读不动」「读到了」必须是三个不同的答案。
    func testReadIsThreeStatesNotAnOptional() throws {
        let s = try store()
        XCTAssertEqual(s.readPrevious(), .absent)
        try mark(s, .running)
        guard case .found = s.readPrevious() else { return XCTFail("写完却读不到") }
    }

    // MARK: - 两个 role 互不干扰；写失败要抛

    /// daemon 会崩，**GUI 也会崩**（2026-09-09 真崩过一次）。两份印记各走各的文件，
    /// 一边的状态绝不能影响另一边的判定。
    func testTheTwoRolesDoNotOverwriteEachOther() throws {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("pc-exit-\(UUID().uuidString)")
        addTeardownBlock { try? FileManager.default.removeItem(at: dir) }
        let daemon = ProcessExitMarkerStore(directory: dir, role: .daemon)
        let app = ProcessExitMarkerStore(directory: dir, role: .app)

        try mark(daemon, .clean)
        try mark(app, .running)

        XCTAssertEqual(daemon.classifyPreviousRun(), .clean, "GUI 那份把 daemon 那份盖了")
        XCTAssertEqual(app.classifyPreviousRun(), .unexpected)
        XCTAssertNotEqual(daemon.url, app.url)
        XCTAssertEqual(Set(ProcessExitMarkerRole.allCases.map(\.fileName)).count,
                       ProcessExitMarkerRole.allCases.count, "两个 role 共用了一个文件名")
    }

    /// **写失败必须抛，不许 `try?` 吞掉。** 这枚印记是「要不要打扰人」的唯一依据，
    /// 写不进去而没人知道，下次就会判错。
    func testWriteFailureThrowsInsteadOfBeingSwallowed() throws {
        try XCTSkipIf(getuid() == 0, "root 绕过权限位")
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("pc-exit-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        addTeardownBlock {
            chmod(dir.path, 0o755)
            try? FileManager.default.removeItem(at: dir)
        }
        XCTAssertEqual(chmod(dir.path, 0o500), 0)      // 只读目录
        let s = ProcessExitMarkerStore(directory: dir, role: .daemon)
        XCTAssertThrowsError(try s.markRunning(startedAt: Date(), build: "x"),
                             "写不进去却没抛 —— 调用方永远不会知道印记没落下")
    }

    // MARK: - 「刚更新过」那一档不需要第二套机制

    func testPreviousBuildComesOutOfTheSameMarker() throws {
        let s = try store()
        try mark(s, .clean, build: "0.1.33(7)")
        XCTAssertEqual(s.previousBuild, "0.1.33(7)")
    }

    func testPreviousBuildIsNilWhenThereIsNoMarker() throws {
        XCTAssertNil(try store().previousBuild)
    }
}
#endif
