#if os(macOS)
import Darwin
import Foundation
import XCTest

final class CaptainAutomaticRecoveryClaimStoreTests: XCTestCase {
    func test_同一crew跨store重开及回滚新source也只能claim一次() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("captain-auto-recovery-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }
        let first = CaptainAutomaticRecoveryClaimStore(directory: directory)
        XCTAssertTrue(try first.claim(crewId: "crew", sourceSessionId: "captain-old"))

        let reopened = CaptainAutomaticRecoveryClaimStore(directory: directory)
        XCTAssertFalse(try reopened.claim(crewId: "crew", sourceSessionId: "captain-rollback"),
                       "重启或回滚产生新 session id 后也不得再次自动启动")
        XCTAssertTrue(try reopened.claim(crewId: "other-crew", sourceSessionId: "captain-old"),
                      "不同 crew 的独立硬失效不应被全局误挡")
        XCTAssertEqual(try reopened.records().count, 2)
    }

    func test_坏claim账本拒绝覆盖并保留原字节() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("captain-auto-recovery-corrupt-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = CaptainAutomaticRecoveryClaimStore(directory: directory)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let original = Data("[{\"crewId\":\"partial\"}]".utf8)
        try original.write(to: store.fileURL)

        XCTAssertThrowsError(try store.claim(crewId: "crew", sourceSessionId: "captain-old"))
        XCTAssertEqual(try Data(contentsOf: store.fileURL), original)
    }

    func test_锁文件打不开时必须拒绝claim且不落数据() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("captain-auto-recovery-lock-open-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        // 固定 lock 文件名被目录占用：open(O_CREAT|O_RDWR) 必须失败，但 claim ledger
        // 目录仍可写，正好复现旧公共 helper 会无锁执行 body 的危险形状。
        try FileManager.default.createDirectory(
            at: directory.appendingPathComponent("captain-auto-recovery-claims.lock"),
            withIntermediateDirectories: true)
        let store = CaptainAutomaticRecoveryClaimStore(directory: directory)

        XCTAssertThrowsError(try store.claim(crewId: "crew", sourceSessionId: "captain-old"))
        XCTAssertFalse(FileManager.default.fileExists(atPath: store.fileURL.path),
                       "拿不到锁时不许把 claim 当成功写入")
    }

    func test_flock失败时必须拒绝claim且不落数据() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("captain-auto-recovery-flock-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let operations = CaptainAutomaticRecoveryClaimLockOperations(
            open: { _ in .init(value: 17, errorNumber: 0) },
            flock: { _, _ in .init(value: -1, errorNumber: EACCES) },
            close: { _ in })
        let store = CaptainAutomaticRecoveryClaimStore(directory: directory, lockOperations: operations)

        XCTAssertThrowsError(try store.claim(crewId: "crew", sourceSessionId: "captain-old"))
        XCTAssertFalse(FileManager.default.fileExists(atPath: store.fileURL.path),
                       "拿不到 flock 时不许把 claim 当成功写入")
    }

    func test_跨进程并发claim最多一次且重开后不会重复() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("captain-auto-recovery-concurrent-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let startURL = directory.appendingPathComponent("start")
        let testBundleURL = Bundle(for: Self.self).bundleURL
        let xcrunURL = URL(fileURLWithPath: "/usr/bin/xcrun")

        func launchHarness(_ sourceSessionId: String) throws -> Process {
            let process = Process()
            process.executableURL = xcrunURL
            process.arguments = [
                "xctest",
                "-XCTest",
                "CaptainAutomaticRecoveryClaimStoreTests/testCrossProcessClaimHarness",
                testBundleURL.path
            ]
            var environment = ProcessInfo.processInfo.environment
            // 嵌套 xctest 不能继承父测试 runner 的注入配置，否则它会尝试复用父进程的
            // bundle/session 而不是执行这个明确指定的 harness。
            for key in environment.keys where key.hasPrefix("XCTest") {
                environment.removeValue(forKey: key)
            }
            environment.merge([
                "PENDINGCREW_CLAIM_HARNESS_DIRECTORY": directory.path,
                "PENDINGCREW_CLAIM_HARNESS_SOURCE": sourceSessionId
            ]) { _, new in new }
            process.environment = environment
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            try process.run()
            return process
        }

        let first = try launchHarness("captain-first")
        let second = try launchHarness("captain-second")
        defer {
            for process in [first, second] where process.isRunning {
                process.terminate()
                process.waitUntilExit()
            }
        }
        for _ in 0..<100 where !(
            FileManager.default.fileExists(atPath: directory.appendingPathComponent("ready-captain-first").path)
                && FileManager.default.fileExists(atPath: directory.appendingPathComponent("ready-captain-second").path)
        ) {
            Thread.sleep(forTimeInterval: 0.02)
        }
        XCTAssertTrue(FileManager.default.fileExists(
            atPath: directory.appendingPathComponent("ready-captain-first").path))
        XCTAssertTrue(FileManager.default.fileExists(
            atPath: directory.appendingPathComponent("ready-captain-second").path))
        guard FileManager.default.fileExists(
            atPath: directory.appendingPathComponent("ready-captain-first").path),
              FileManager.default.fileExists(
                atPath: directory.appendingPathComponent("ready-captain-second").path)
        else { return }
        try Data().write(to: startURL)
        first.waitUntilExit()
        second.waitUntilExit()

        XCTAssertEqual([first.terminationStatus, second.terminationStatus].filter { $0 == 0 }.count, 1,
                       "两个独立 xctest 进程同抢同一 crew 时最多一个可 claim")
        let outcomes = try ["captain-first", "captain-second"].map { sourceSessionId in
            try String(contentsOf: directory.appendingPathComponent("outcome-\(sourceSessionId)"))
        }
        XCTAssertEqual(outcomes.filter { $0 == "claimed" }.count, 1)
        XCTAssertEqual(outcomes.filter { $0 == "already-claimed" }.count, 1,
                       "失败进程也必须完成真实 claim 并得到已消费结果，不能以崩溃冒充互斥")
        let reopened = CaptainAutomaticRecoveryClaimStore(directory: directory)
        XCTAssertFalse(try reopened.claim(crewId: "crew", sourceSessionId: "captain-after-restart"),
                       "救援完成后重开 store 也不得再次自动接任")
    }

    /// 仅供上面的父测试作为独立 xctest 子进程调用；没有这两个环境变量时不做任何事。
    func testCrossProcessClaimHarness() throws {
        let environment = ProcessInfo.processInfo.environment
        guard let directoryPath = environment["PENDINGCREW_CLAIM_HARNESS_DIRECTORY"],
              let sourceSessionId = environment["PENDINGCREW_CLAIM_HARNESS_SOURCE"]
        else { return }
        let directory = URL(fileURLWithPath: directoryPath, isDirectory: true)
        let readyURL = directory.appendingPathComponent("ready-\(sourceSessionId)")
        let startURL = directory.appendingPathComponent("start")
        try Data().write(to: readyURL)
        for _ in 0..<500 where !FileManager.default.fileExists(atPath: startURL.path) {
            Thread.sleep(forTimeInterval: 0.01)
        }
        XCTAssertTrue(FileManager.default.fileExists(atPath: startURL.path), "父测试未发出并发起跑信号")
        guard FileManager.default.fileExists(atPath: startURL.path) else { return }
        let didClaim = try CaptainAutomaticRecoveryClaimStore(directory: directory)
            .claim(crewId: "crew", sourceSessionId: sourceSessionId)
        try Data((didClaim ? "claimed" : "already-claimed").utf8).write(
            to: directory.appendingPathComponent("outcome-\(sourceSessionId)"))
        XCTAssertTrue(didClaim,
                      "只有一个子进程应获得 durable claim；父测试由退出码核对至多一个成功")
    }
}
#endif
