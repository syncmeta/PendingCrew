#if os(macOS)
import Darwin
import Foundation

/// 本机上一个 `--mcp-serve` helper 进程。
struct HelperProcessRecord: Equatable {
    let pid: Int32
    let crewId: String?
    let sessionId: String?
    /// 起它时给的可执行文件路径（`KERN_PROCARGS2` 的 exec path）。
    let launchPath: String
    /// 它**正在执行的那份文件**。nil = 读不出来。
    let running: HelperBuildStamp?
}

enum HelperProcessForensics {
    static func parseProcArgs(_ bytes: [UInt8]) -> (execPath: String, argv: [String])? {
        nil  // SKELETON
    }

    static func scanHelpers() -> [HelperProcessRecord] {
        []  // SKELETON
    }

    static func runningStamp(pid: Int32) -> HelperBuildStamp? {
        nil  // SKELETON
    }

    static func report(crewId: String, sessionId: String, helpers: [HelperProcessRecord],
                       onDisk: (String) -> HelperBuildStamp?) -> HelperBuildReport {
        HelperBuildReport(verdict: .unknown, helperCount: 0)  // SKELETON
    }

    static func reports(for keys: [SessionAwaitingReplyInputsCache.RunKey])
        -> [SessionAwaitingReplyInputsCache.RunKey: HelperBuildReport] {
        [:]  // SKELETON
    }
}
#endif
