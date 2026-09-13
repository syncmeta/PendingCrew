#if os(macOS)
import Darwin
import Foundation

/// 本机上一个 `--mcp-serve` helper 进程。
struct HelperProcessRecord: Equatable {
    let pid: Int32
    /// argv 里的 `--crew` / `--session`。解不出来就是 nil。
    let crewId: String?
    let sessionId: String?
    /// 起它时给的可执行文件路径（`KERN_PROCARGS2` 的 exec path）。
    ///
    /// ⚠️ **这个路径是会骗人的那一半**：Sparkle 把旧包整个挪进 Caches 之后，它还一字不差
    /// 地写着 `/Applications/...`。它在这里只有一个用途 —— 告诉我们「磁盘上现在那份」
    /// 该去哪儿读；**「正在跑的是哪份」绝不从它推**，那是 `running` 的事。
    let launchPath: String
    /// 它**正在执行的那份文件**（从进程攥着的 vnode 读，不经路径）。nil = 读不出来。
    let running: HelperBuildStamp?
}

/// 编排者一侧对 helper 进程取证：「这个成员的 helper 跑的是不是磁盘上现在那份」。
///
/// ## 为什么在 daemon 这一侧量，而不是让 helper 自报
///
/// 要抓的正是**今天已经在跑的旧 helper** —— 它们比这段代码早，没有自报的能力。
/// 从外面量不需要它们配合。两条路逐格对比见
/// `docs/internal/2026-09-13-helper-build-per-member.md`。
///
/// ## 判据
///
/// 正在执行的那份：`proc_pidinfo(PROC_PIDREGIONPATHINFO)` 找到映射了主可执行文件的那段
/// 内存，读它的 vnode stat（inode / 大小 / mtime）。**这是进程攥着的那个文件本身**，
/// 被挪走、被删掉都不影响它。磁盘上现在那份：`HelperBuildStamp.read(argv 里那个路径)`。
/// 比对：`HelperBuildVerdict.judge` —— 与拒绝话术同一把尺子。
enum HelperProcessForensics {

    // MARK: - 纯函数

    /// 解 `sysctl(KERN_PROCARGS2)` 的字节：`argc(int32)` + exec path + 若干 `\0` 填充 +
    /// `argc` 个 `\0` 结尾的 argv + 环境变量。解不出来（截断 / argc 对不上）→ nil。
    static func parseProcArgs(_ bytes: [UInt8]) -> (execPath: String, argv: [String])? {
        let intSize = MemoryLayout<Int32>.size
        guard bytes.count > intSize else { return nil }
        let argc = bytes.withUnsafeBytes { Int(Int32(littleEndian: $0.loadUnaligned(as: Int32.self))) }
        guard argc >= 0 else { return nil }
        var i = intSize
        func readCString() -> String? {
            guard let end = bytes[i...].firstIndex(of: 0) else { return nil }
            let s = String(decoding: bytes[i..<end], as: UTF8.self)
            i = end + 1
            return s
        }
        guard let execPath = readCString() else { return nil }
        while i < bytes.count, bytes[i] == 0 { i += 1 }
        var argv: [String] = []
        while argv.count < argc {
            guard i < bytes.count, let arg = readCString() else { return nil }
            argv.append(arg)
        }
        return (execPath, argv)
    }

    /// argv → (crew, session)。不是 `--mcp-serve` → nil。
    static func helperIdentity(argv: [String]) -> (crewId: String?, sessionId: String?)? {
        guard argv.contains("--mcp-serve") else { return nil }
        func value(_ flag: String) -> String? {
            guard let k = argv.firstIndex(of: flag), k + 1 < argv.count else { return nil }
            return argv[k + 1]
        }
        return (value("--crew"), value("--session"))
    }

    /// 一个成员名下的全部 helper → 一格。`onDisk` 注入是为了单测喂真文件走真逻辑。
    ///
    /// 聚合口径（都往保守方向）：
    /// - 一个都没找到 → 判不了；
    /// - 任一个旧 → 旧（旧的那个也在回这个 session 的工具调用）；
    /// - 否则有判不了的 → 判不了（**不许因为另一个是新的就说整格是新的**）；
    /// - 全部当前 → 当前。
    static func report(crewId: String, sessionId: String, helpers: [HelperProcessRecord],
                       onDisk: (String) -> HelperBuildStamp?) -> HelperBuildReport {
        let mine = helpers.filter { $0.sessionId == sessionId && ($0.crewId == nil || $0.crewId == crewId) }
        guard !mine.isEmpty else {
            return HelperBuildReport(
                verdict: .unknown, runningVersion: nil, diskVersion: nil,
                reason: "没找到它的 helper 进程（MCP 还没起来 / 已经退出 / 读不到进程参数）",
                helperCount: 0)
        }
        typealias Judged = (record: HelperProcessRecord, disk: HelperBuildStamp?, verdict: HelperBuildVerdict)
        let judged: [Judged] = mine.map { rec in
            let disk = onDisk(rec.launchPath)
            return (rec, disk, HelperBuildVerdict.judge(running: rec.running, onDisk: disk))
        }
        func versions(_ j: Judged) -> (String?, String?) {
            (j.record.running?.versionText, j.disk?.versionText)
        }
        if let stale = judged.first(where: { $0.verdict == .stale }) {
            let (run, disk) = versions(stale)
            return HelperBuildReport(verdict: .stale, runningVersion: run, diskVersion: disk,
                                     reason: "pid \(stale.record.pid)", helperCount: mine.count)
        }
        if let unknown = judged.first(where: { $0.verdict == .unknown }) {
            let why: String
            if unknown.record.running == nil {
                why = "读不到 pid \(unknown.record.pid) 正在执行的那个文件"
            } else if unknown.disk == nil {
                why = "磁盘上 \(unknown.record.launchPath) 读不出来（正在装新版 / 被挪走了）"
            } else {
                why = "文件身份信息不全"
            }
            let (run, disk) = versions(unknown)
            return HelperBuildReport(
                verdict: .unknown, runningVersion: run, diskVersion: disk,
                reason: mine.count > 1 ? "\(mine.count) 个 helper 里有判不了的：\(why)" : why,
                helperCount: mine.count)
        }
        let (run, disk) = versions(judged[0])
        return HelperBuildReport(verdict: .current, runningVersion: run, diskVersion: disk,
                                 reason: nil, helperCount: mine.count)
    }

    // MARK: - 系统调用那一半

    /// 可执行文件名。只对叫这个名字的进程读 argv —— 全机几百个进程逐个读参数没必要。
    static let executableName = "PendingCrew"

    /// 扫全机的 `--mcp-serve` helper。
    static func scanHelpers() -> [HelperProcessRecord] {
        var out: [HelperProcessRecord] = []
        for pid in allPids() where pid > 0 && processName(pid) == executableName {
            guard let bytes = procArgsBytes(pid),
                  let (execPath, argv) = parseProcArgs(bytes),
                  let ident = helperIdentity(argv: argv) else { continue }
            out.append(HelperProcessRecord(pid: pid, crewId: ident.crewId, sessionId: ident.sessionId,
                                           launchPath: execPath, running: runningStamp(pid: pid)))
        }
        return out
    }

    /// 编排者每拍用的入口：一次扫描，按成员出格。
    static func reports(for keys: [SessionAwaitingReplyInputsCache.RunKey])
        -> [SessionAwaitingReplyInputsCache.RunKey: HelperBuildReport] {
        guard !keys.isEmpty else { return [:] }
        let helpers = scanHelpers()
        // 同一拍里同一个路径只读一次盘。
        var diskCache: [String: HelperBuildStamp?] = [:]
        let onDisk: (String) -> HelperBuildStamp? = { path in
            if let hit = diskCache[path] { return hit }
            let s = HelperBuildStamp.read(executable: URL(fileURLWithPath: path))
            diskCache[path] = s
            return s
        }
        var out: [SessionAwaitingReplyInputsCache.RunKey: HelperBuildReport] = [:]
        for key in keys {
            out[key] = report(crewId: key.crewId, sessionId: key.sessionId, helpers: helpers, onDisk: onDisk)
        }
        return out
    }

    /// 该进程**正在执行的那个文件**的 stamp。
    ///
    /// 1. `proc_pidpath` 取 vnode 现在的路径（被挪走了就是挪去的那个路径）；
    /// 2. 走一遍内存区段，找映射着这个路径的那一段，读它的 vnode stat —— 身份三样从
    ///    **vnode 本身**来，不经路径查找；
    /// 3. 版本串：读那个路径旁边的 Info.plist，**但只有当那个路径上现在的文件就是这个
    ///    vnode 时才采信**。原地换过 / 已删掉时，旁边那份 plist 是别人的，宁可 nil。
    static func runningStamp(pid: Int32) -> HelperBuildStamp? {
        var pathBuf = [CChar](repeating: 0, count: Int(MAXPATHLEN) * 4)
        let n = proc_pidpath(pid, &pathBuf, UInt32(pathBuf.count))
        let currentPath = n > 0 ? String(cString: pathBuf) : nil

        var rpi = proc_regionwithpathinfo()
        let rpiSize = Int32(MemoryLayout<proc_regionwithpathinfo>.size)
        var address: UInt64 = 0
        var found: vinfo_stat?
        var firstExecutableLike: vinfo_stat?
        for _ in 0..<4096 {
            let r = proc_pidinfo(pid, PROC_PIDREGIONPATHINFO, address, &rpi, rpiSize)
            guard r == rpiSize else { break }
            let path = withUnsafeBytes(of: rpi.prp_vip.vip_path) {
                String(decoding: $0.prefix(while: { $0 != 0 }), as: UTF8.self)
            }
            if !path.isEmpty {
                if let currentPath, path == currentPath {
                    found = rpi.prp_vip.vip_vi.vi_stat
                    break
                }
                if firstExecutableLike == nil, path.hasSuffix("/Contents/MacOS/\(executableName)") {
                    firstExecutableLike = rpi.prp_vip.vip_vi.vi_stat
                }
            }
            let next = rpi.prp_prinfo.pri_address &+ rpi.prp_prinfo.pri_size
            guard next > address else { break }
            address = next
        }
        guard let st = found ?? firstExecutableLike else { return nil }

        var stamp = HelperBuildStamp()
        stamp.inode = st.vst_ino
        stamp.size = st.vst_size
        stamp.modified = Date(timeIntervalSince1970:
            TimeInterval(st.vst_mtime) + TimeInterval(st.vst_mtimensec) / 1_000_000_000)
        if let currentPath,
           let atPath = HelperBuildStamp.read(executable: URL(fileURLWithPath: currentPath)),
           HelperBuildVerdict.judge(running: stamp, onDisk: atPath) == .current {
            stamp.versionText = atPath.versionText
        }
        return stamp
    }

    private static func allPids() -> [Int32] {
        let count = proc_listallpids(nil, 0)
        guard count > 0 else { return [] }
        var pids = [Int32](repeating: 0, count: Int(count) + 64)
        let got = pids.withUnsafeMutableBytes {
            proc_listallpids($0.baseAddress, Int32($0.count))
        }
        guard got > 0 else { return [] }
        return Array(pids.prefix(Int(got)))
    }

    private static func processName(_ pid: Int32) -> String? {
        var buf = [CChar](repeating: 0, count: 256)
        guard proc_name(pid, &buf, UInt32(buf.count)) > 0 else { return nil }
        return String(cString: buf)
    }

    private static func procArgsBytes(_ pid: Int32) -> [UInt8]? {
        var mib: [Int32] = [CTL_KERN, KERN_PROCARGS2, pid]
        var size = 0
        guard sysctl(&mib, 3, nil, &size, nil, 0) == 0, size > 0 else { return nil }
        var buf = [UInt8](repeating: 0, count: size)
        guard sysctl(&mib, 3, &buf, &size, nil, 0) == 0 else { return nil }
        return Array(buf.prefix(size))
    }
}
#endif
