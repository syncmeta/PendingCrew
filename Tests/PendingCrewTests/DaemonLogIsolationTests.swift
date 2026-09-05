import XCTest

#if os(macOS)

/// `PENDINGCREW_DATA_DIR` 挪走数据根时，daemon 日志**不许再写默认那份**。
///
/// 两条彼此拉扯的约束，这几行是它们的交点：
/// - 日志**不能跟着数据根走**——临时根跑完就被删，而日志正是那种跑法唯一的观察窗
///   （原注释里已经写明这条理由，别把它当成疏忽去"修"掉）。
/// - 日志**也不能写进真人那份 `daemon.log`**——2026-09-04 的隔离冒烟就把
///   启动/连接/退出几十行混进了用户的真日志里。隔离做了一半比没做更难查：
///   人以为看的是自家后台，其实混着一次实验。
///
/// 所以解法是「留在 Logs 目录，但换一个由数据根决定的文件名」。
final class DaemonLogIsolationTests: XCTestCase {

    func test_默认数据根仍然写daemon点log() {
        XCTAssertEqual(
            PendingCrewDaemonPaths.logFileName(
                dataRoot: URL(fileURLWithPath: "/Users/x/Library/Application Support/PendingCrew"),
                dataRootIsOverridden: false),
            "daemon.log")
    }

    func test_覆盖数据根时不许写默认那份() {
        let name = PendingCrewDaemonPaths.logFileName(
            dataRoot: URL(fileURLWithPath: "/Users/x/Library/Caches/pcsmoke"),
            dataRootIsOverridden: true)
        XCTAssertNotEqual(name, "daemon.log", "覆盖数据根时还在写默认日志 = 污染真人那份")
        XCTAssertTrue(name.hasSuffix(".log"), "仍应当是一个 .log：\(name)")
        XCTAssertTrue(name.contains("pcsmoke"), "文件名要认得出是哪个数据根：\(name)")
    }

    func test_两个不同的覆盖根不共用一个文件() {
        let a = PendingCrewDaemonPaths.logFileName(
            dataRoot: URL(fileURLWithPath: "/tmp/rootA"), dataRootIsOverridden: true)
        let b = PendingCrewDaemonPaths.logFileName(
            dataRoot: URL(fileURLWithPath: "/tmp/rootB"), dataRootIsOverridden: true)
        XCTAssertNotEqual(a, b, "两次隔离跑写同一个文件，等于隔离只做了一半")
    }

    func test_日志仍然不落在数据根里面() {
        // 跟着数据根走 = 临时根删掉时把唯一的观察窗一起删掉。这条是反向护栏。
        let root = URL(fileURLWithPath: "/tmp/rootA")
        let paths = PendingCrewDaemonPaths.standard(
            dataRoot: root, logs: URL(fileURLWithPath: "/Users/x/Library/Logs"),
            dataRootIsOverridden: true)
        XCTAssertFalse(paths.log.path.hasPrefix(root.path),
                       "日志落进了数据根里：\(paths.log.path)")
    }
}

#endif
