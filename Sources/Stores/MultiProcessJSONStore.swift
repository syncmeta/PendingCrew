import Foundation

/// 多进程 JSON 文件 store 基座（#528；把 #483 白板三件套抽出共用）。
///
/// app 与 `pendingcrew-mcp` helper 子进程并发读写同一批 `<dir>/*.json` 文件，
/// 四件套缺一不可：
/// ① **flock sidecar 互斥** —— read-modify-write 全程在锁内做，消除 last-write-wins
///    丢写（app 写完 helper 用旧快照覆盖）；
/// ② **逐条 lenient 解码** —— 数组元素坏一条丢一条，不连坐成整文件解码失败；
/// ③ **corrupt 归档 fail-loud** —— 外层 JSON **确认**解析不了（半截写入 / 乱码）时把
///    损坏字节归档成 `<file>.corrupt-<unix毫秒>`（人工可找回）并回调调用方 fail-loud，
///    绝不「(try? decode) ?? []」把文件静默当空 —— 那正是 2026-07-17 白板历史
///    被下一次写以空数组重写清掉的病根。
/// ④ **读失败 ≠ 内容损坏（2026-08-12 P0，本层最硬的一条不变式）** ——
///    「文件读不出来」永远不许归档、搬走、删除或重建原件。只有**两次独立读到的
///    字节都真解不开**才算损坏、才允许走 ③。
///
/// ④ 为什么是血写的：2026-08-12 晚上四轮误杀，19–24 份**完全合法**的 JSON 被归档
/// 并从空重建，约 2000+ 条群聊历史从 live 文件消失。病根不是解码，是 `open()`：
/// GUI app 从 launchd 继承的 `RLIMIT_NOFILE` 软上限只有 256（见
/// `FileDescriptorLimit`），`whiteboards/` 目录 900+ 文件 + 本机数十个 PendingCrew
/// 进程，定时唤醒那趟批量读一次顶穿 → `open()` 返回 EMFILE → **Foundation 把它
/// 包成 `NSFileReadNoPermissionError`，文案是「你没有权限查看此文件」** → 上层当成
/// 「读不出来 = 大概率坏了」→ 归档重建。fail-loud（喊出来）那半是对的，
/// **处置动作那半是破坏性的**，这一层把它改成：喊，但一个字节都不动。
///
/// 自包含 Foundation（编进 app / re-exec helper / PendingCrewTests bundle）。
enum MultiProcessJSONStore {

    /// 账本出事时，那条「白板上的系统警示」**到底有没有**。
    ///
    /// 报事故走 `LocalWhiteboardStore.appendSessionMessage`，而 append **要先把整份
    /// 白板读一遍**，读不了就整条拒写（2026-08-12 P0 的不变式：读不出来 ≠ 内容损坏，
    /// 一个字节都不许动）。所以：
    ///
    /// - 只有这一本账坏了（`.corrupt` 归档重建）而白板好着 → 警示**写得进去**；
    /// - **整个数据目录读不出来** → 账本和白板一起瞎，警示**必然不存在**。
    ///
    /// 而后者正是这类回执最常出现的场合（2026-09-12 实测：目录 EPERM 七小时，
    /// 提醒一直在响，它让人去找的那条警示一次都没写成）。
    ///
    /// 所以**所有回执都用这一句**，别再写「群聊白板上有一条系统警示」——
    /// 把人支去找一个不存在的东西，他会得出「那就不是这种事故」的反结论。
    static let whiteboardNoticeCaveat =
        "白板上**可能**有一条系统警示说明是哪种事故；但白板自己也读不出来时那条写不进去，所以没看到不等于没事"

    /// flock `lockURL` 执行 `body`。flock 跨进程互斥，对同进程内不同 fd 也互斥 ——
    /// 调用方只在 public 入口拿一次锁，锁内一律走 *Locked 变体，不嵌套同一把锁
    /// （嵌套**不同**文件的锁可以，前提是全仓无反向嵌套成环）。锁文件打不开
    /// （目录不可写等极端情况）退化为无锁执行。
    static func withFileLock<T>(_ lockURL: URL, _ body: () throws -> T) rethrows -> T {
        let fd = open(lockURL.path, O_CREAT | O_WRONLY, 0o644)
        guard fd >= 0 else { return try body() }
        defer { close(fd) }
        flock(fd, LOCK_EX)
        defer { flock(fd, LOCK_UN) }
        return try body()
    }

    // MARK: - 事故分类（④ 的对外说法）

    /// 账本这次出的事 —— **「读不出来」和「真的解不动」是两件事，两套文案。**
    ///
    /// 2026-08-12 之前两者共用一条 `onCorrupt(URL?)` 回调、一句「…文件损坏，已归档为 …」，
    /// 于是那晚 24 次**读失败**全被描述成「损坏」，十几个机长各自跑去翻归档、发现文件
    /// 好好的。**措辞在这种事故里不是修饰，是真实的成本项** —— 当晚一半的无效轮次是
    /// 这句假描述造成的。
    enum LedgerIncident {
        /// 文件在、读不出来（fd 打满 / 权限 / IO）。**原件一个字节没动**，本次写已拒。
        /// `url` = **读的人自己用的那个路径**。错误对象里通常带 `NSFilePath`，但不保证；
        /// 带上它，「读的进程用的数据根跟写的是不是同一个」才有得查。
        case unreadable(Error, url: URL?)
        /// 读到空表但磁盘文件非空 —— 漏读的另一种形态。同样原件不动、本次写已拒。
        case misread
        /// **两次独立读到的字节都真解不开**。已归档为 `.corrupt-<ts>`
        /// （nil = 归档也挪不动，原件留在原地）。
        case corrupt(archive: URL?)

        /// 说给人听的一句话；调用方在前面接主语（「白板」/「人类 Todo 列表」/…）。
        var summary: String {
            switch self {
            case .unreadable(let error, let url):
                // 括号里那句是 Foundation 的原文，**它对 EPERM / EACCES / EMFILE 一字不差、
                // 而且永远只有文件名**。后面那段方括号是把同一个错误对象里本来就装着、
                // 却一直被扔掉的两样东西取出来：底层 errno 和绝对路径。
                // 2026-09-01 的排查就是卡死在这里（Agent Todo #97 原话：「旧提示未保存
                // 底层 errno，无法事后证明具体是哪种系统错误」），然后被判成「没事」。
                return "这次读不出来（\(error.localizedDescription)）"
                    + "【\(diagnose(error, fallbackPath: url).line)】。"
                    + "**原件一个字节都没动**，本次写入已拒绝 —— 这不是文件损坏，不用去翻归档。"
            case .misread:
                return "读到的是空表、磁盘文件却非空（疑似漏读）。"
                    + "**原件一个字节都没动**，本次写入已拒绝 —— 这不是文件损坏，不用去翻归档。"
            case .corrupt(let archive):
                return "文件**确认解不开**（两次独立读都解不出来），已归档为 "
                    + "\(archive?.lastPathComponent ?? "（归档失败，原文件保留在原地）")"
                    + "（whiteboards 目录，可人工找回）。"
            }
        }

        /// 是不是「原件完好、只是这次没读到」—— 调用方据此决定要不要说「数据可能丢了」。
        var isDataIntact: Bool {
            switch self {
            case .unreadable, .misread: return true
            case .corrupt: return false
            }
        }
    }

    // MARK: - 读（④：读失败永不销毁原件）

    /// 瞬时读失败的退避序列（微秒）。fd 打满 / 忙 / 被信号打断都是**一阵子**的事，
    /// 三次退避（20/60/150ms）足够让 launchd 那趟批量读的峰值过去。
    static let readRetryBackoff: [useconds_t] = [20_000, 60_000, 150_000]

    /// POSIX 层的瞬时失败：fd 打满（EMFILE/ENFILE 就是 8-12 那次的真身）、
    /// 权限/忙/被信号打断/内存紧张。
    private static let transientPOSIXCodes: Set<Int32> = [
        EMFILE, ENFILE, EACCES, EPERM, EBUSY, EINTR, EAGAIN, ENOMEM,
    ]

    /// Cocoa 层的瞬时失败。`NSFileReadNoPermissionError` 名字写着「权限」，实际是
    /// Foundation 对 `open()` 一整类 errno（含 EMFILE）的**误导性映射** —— 8-12
    /// 事故里所有报错都长这个样子，而文件权限一直是 644。
    private static let transientCocoaCodes: Set<Int> = [
        NSFileReadNoPermissionError, NSFileReadUnknownError,
    ]

    /// 这次「读不出来」是不是瞬时的 —— **只用来决定要不要重试**，不用来决定要不要
    /// 归档（任何读失败都不归档，见 ④）。所以判错的代价至多是少重试一次，
    /// 永远不会变成毁数据。
    static func isTransientReadFailure(_ error: Error) -> Bool {
        forEachErrorInChain(error) { e in
            (e.domain == NSPOSIXErrorDomain && transientPOSIXCodes.contains(Int32(e.code)))
                || (e.domain == NSCocoaErrorDomain && transientCocoaCodes.contains(e.code))
        }
    }

    /// 「文件压根不存在」—— 合法空表，不是失败。
    static func isNotFound(_ error: Error) -> Bool {
        forEachErrorInChain(error) { e in
            (e.domain == NSCocoaErrorDomain && e.code == NSFileReadNoSuchFileError)
                || (e.domain == NSPOSIXErrorDomain && e.code == Int(ENOENT))
        }
    }

    /// 沿 `NSUnderlyingErrorKey` 链判定（Foundation 常把 errno 埋在下一层）。
    private static func forEachErrorInChain(
        _ error: Error, _ predicate: (NSError) -> Bool
    ) -> Bool {
        errorChain(error).contains(where: predicate)
    }

    /// 整条 `NSUnderlyingErrorKey` 链（最多四层，同上）。判真假用上面那个，
    /// **要把里面的值取出来**（errno / 路径）用这个。
    private static func errorChain(_ error: Error) -> [NSError] {
        var out: [NSError] = []
        var current: NSError? = error as NSError
        while let e = current, out.count < 4 {
            out.append(e)
            current = e.userInfo[NSUnderlyingErrorKey] as? NSError
        }
        return out
    }

    // MARK: - 读失败的可诊断读数（2026-09-08）

    /// 一次读失败里**本来就存在、却从没被说出口**的几样事实。
    ///
    /// 病根不在 Foundation：`Data(contentsOf:)` 抛的 `NSError` 里既有
    /// `NSUnderlyingError`（POSIX errno）也有 `NSFilePath`（绝对路径）。丢它们的是
    /// 我们自己 —— 每一处事故文案都只印 `localizedDescription`，而那句话
    /// **对下面三种完全不同的病一字不差**：
    ///
    /// | errno | 是什么病 | 该怎么办 |
    /// |---|---|---|
    /// | `EMFILE`/`ENFILE` | 句柄耗尽（2026-08-12 那次的真身） | 抬软上限、收敛文件数 |
    /// | `EACCES` | 文件权限位真的不给读 | 看 `ls -l` |
    /// | `EPERM` | **环境层**拒绝（沙盒 / TCC / 数据保护） | 跟文件本身无关，查授权 |
    ///
    /// 三者的区别就是「该找谁」的区别，而 2026-09-01 那次排查正是因为拿不到它
    /// 才只能写下「无法事后证明具体是哪种系统错误」并翻了 completed。
    struct ReadFailureDiagnosis {
        var cocoaCode: Int?
        var posixCode: Int32?
        var posixName: String?
        /// 绝对路径。**那句提示里永远没有它**（`localizedDescription` 只取文件名），
        /// 所以「读的人和写的人是不是同一个数据根」以前无从判断。
        var path: String?

        /// 说给排查的人听的一行。
        var line: String {
            var parts: [String] = []
            if let posixCode {
                parts.append("errno=\(posixCode) \(posixName ?? "（无名）")"
                             + "（\(String(cString: strerror(posixCode)))）")
            } else {
                parts.append("底层 errno 没带上")
            }
            if let cocoaCode { parts.append("Cocoa \(cocoaCode)") }
            parts.append("路径 " + (path ?? "（错误对象里没有，调用方也没给）"))
            parts.append("读的进程 pid=\(getpid())")
            return parts.joined(separator: "｜")
        }
    }

    /// errno 号 → 名字。只列这条路上真出得来的那些；查不到时只报号，**不猜**。
    private static let posixErrnoNames: [Int32: String] = [
        EPERM: "EPERM", ENOENT: "ENOENT", EINTR: "EINTR", EIO: "EIO",
        ENOMEM: "ENOMEM", EACCES: "EACCES", EBUSY: "EBUSY", ENODEV: "ENODEV",
        ENOTDIR: "ENOTDIR", EISDIR: "EISDIR", ENFILE: "ENFILE", EMFILE: "EMFILE",
        EDEADLK: "EDEADLK", EAGAIN: "EAGAIN", ELOOP: "ELOOP",
        ENAMETOOLONG: "ENAMETOOLONG", ESTALE: "ESTALE", EOVERFLOW: "EOVERFLOW",
    ]

    /// 从错误链里把 errno 与绝对路径取出来。`fallbackPath` = 读的人自己用的 URL ——
    /// 错误对象没带路径时用它兜底（**这一路才是回答「数据根对不对」的那条**）。
    static func diagnose(_ error: Error, fallbackPath: URL?) -> ReadFailureDiagnosis {
        var d = ReadFailureDiagnosis()
        for e in errorChain(error) {
            if e.domain == NSCocoaErrorDomain, d.cocoaCode == nil { d.cocoaCode = e.code }
            if e.domain == NSPOSIXErrorDomain, d.posixCode == nil {
                let code = Int32(truncatingIfNeeded: e.code)
                d.posixCode = code
                d.posixName = posixErrnoNames[code]
            }
            if d.path == nil {
                if let p = e.userInfo[NSFilePathErrorKey] as? String {
                    d.path = p
                } else if let u = e.userInfo[NSURLErrorKey] as? URL {
                    d.path = u.path
                }
            }
        }
        if d.path == nil { d.path = fallbackPath?.path }
        return d
    }

    // MARK: - 只写不读的持久痕迹

    /// 痕迹文件的上限。到顶**轮转**（`.1`），不是停止记录 —— 一个到点就悄悄不再
    /// 发声的检查，和一个从来没触发过的检查长得一模一样。
    static let readFailureLogMaxBytes = 512 * 1024

    /// 出事那个文件旁边的痕迹文件。
    ///
    /// 为什么必须另有一条**只写不读**的痕迹：读失败的播报是往白板 append，而
    /// append 自己要先把白板读出来。同一刻两个都读不出来时（真实形态往往正是
    /// 「整个目录这一刻都读不了」），那条播报会被 `_ = try?` 静默吞掉 ——
    /// **于是「这个错发作过多少次」这个问题永远查不清**，而它恰恰是排查里最便宜
    /// 的那份证据。
    ///
    /// 放进 `diagnostics/` 子目录是刻意的：白板目录本身挂着 `DispatchSource`，
    /// 直接写在那一层会把「读失败 → 写痕迹 → 目录事件 → 又一次读」接成自激。
    static func readFailureLogURL(besideFileAt url: URL) -> URL {
        url.deletingLastPathComponent()
            .appendingPathComponent("diagnostics", isDirectory: true)
            .appendingPathComponent("read-failures.log")
    }

    /// 记一条。`O_APPEND` 的小写入跨进程原子，不需要额外的锁（这条路径上**不能**
    /// 再去拿锁：它自己就跑在别人的锁里）。全程 best-effort —— 留痕失败绝不许
    /// 把原来那次读失败的处置变得更糟。
    static func recordReadFailure(_ error: Error, at url: URL) {
        let log = readFailureLogURL(besideFileAt: url)
        try? FileManager.default.createDirectory(
            at: log.deletingLastPathComponent(), withIntermediateDirectories: true)
        rotateReadFailureLogIfFull(log)
        // argv 而不是「角色」：`ProcessRole` 是 macOS-only，而这一层要同时编进 iOS；
        // 何况**原样记下 argv 比记一个我们自己算出来的结论硬** —— `--mcp-serve` /
        // `--daemon` / 什么都没有，读的人自己就能看出是 helper / 后台 / 界面。
        let argv = CommandLine.arguments.dropFirst().joined(separator: " ")
        let line = ISO8601DateFormatter().string(from: Date())
            + "｜" + diagnose(error, fallbackPath: url).line
            + "｜argv=" + (argv.isEmpty ? "（无）" : String(argv.prefix(240)))
            + "\n"
        let fd = open(log.path, O_WRONLY | O_APPEND | O_CREAT, 0o644)
        guard fd >= 0 else { return }
        defer { close(fd) }
        _ = Array(line.utf8).withUnsafeBufferPointer { write(fd, $0.baseAddress, $0.count) }
    }

    private static func rotateReadFailureLogIfFull(_ log: URL) {
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: log.path),
              let size = (attrs[.size] as? NSNumber)?.intValue,
              size >= readFailureLogMaxBytes else { return }
        let rotated = log.deletingLastPathComponent()
            .appendingPathComponent(log.lastPathComponent + ".1")
        try? FileManager.default.removeItem(at: rotated)
        try? FileManager.default.moveItem(at: log, to: rotated)
    }

    /// 读原始字节，瞬时失败退避重试。返回 nil = 文件不存在（合法空）；
    /// 重试用尽仍失败 → 抛（调用方一律走「保留原件 + 报警」，**绝不**归档）。
    static func readDataIfExists(at url: URL) throws -> Data? {
        var lastError: Error = CocoaError(.fileReadUnknown)
        for attempt in 0...readRetryBackoff.count {
            do {
                return try Data(contentsOf: url)
            } catch {
                if isNotFound(error) { return nil }
                lastError = error
                guard isTransientReadFailure(error), attempt < readRetryBackoff.count else { break }
                usleep(readRetryBackoff[attempt])
            }
        }
        // 全仓所有账本读失败都从这里出去 —— 留痕挂在这一个漏斗上，
        // 不指望每个调用点自己记得（那种规矩会漏，而漏掉的那次没人看得见）。
        recordReadFailure(lastError, at: url)
        throw lastError
    }

    /// 锁内读一个 `[Row]` JSON 文件。文件缺失 → `[]`；元素坏一条丢一条（②）；
    /// **确认**解析不了 → 归档损坏字节（③）并回调 `onCorrupt(归档URL)`（nil = 归档
    /// 挪不动，损坏文件原样留在原地），返回 `[]` 从头开始 —— 调用方在回调里
    /// fail-loud（往白板落警示 / 重建警示行）。
    ///
    /// 读不出来（IO / fd 打满 / 权限）→ 这个包装吞成 `[]`，**原件一个字节不动**，
    /// 并**照样把 `.unreadable` 报给 `onIncident`**（2026-08-12 前这条路径是彻底静默的，
    /// 读失败只能靠最后那道拒写闸间接暴露，且被描述成「损坏」）。要在类型上区分
    /// 「合法空」与「读不出来」的调用点用下面的严格版本。
    static func loadRowsLocked<Row: Decodable>(
        _ type: Row.Type, at url: URL,
        onIncident: (LedgerIncident) -> Void = { _ in }) -> [Row] {
        do {
            return try loadRowsLockedReportingFailure(type, at: url, onIncident: onIncident)
        } catch {
            onIncident(.unreadable(error, url: url))
            return []
        }
    }

    /// 会区分「文件不存在」与「文件存在但读不出来」的严格读版本。前者是合法空表；
    /// 后者把底层 IO 错误抛给调用方，绝不伪装成 `[]`、也绝不归档。高价值、整写型
    /// 调用点优先使用此版本，旧 store 可保留上面的包装并依靠拒写闸兜底。
    static func loadRowsLockedReportingFailure<Row: Decodable>(
        _ type: Row.Type, at url: URL, onIncident: (LedgerIncident) -> Void = { _ in }
    ) throws -> [Row] {
        guard let data = try readDataIfExists(at: url) else { return [] }
        if let rows = decodeRows(type, from: data) { return rows }
        // 归档前**复验**（④ 的执行面）：等一小下、重新读一次原字节、再解一次。
        // - 复验读不出来 → 抛，当「读不动」处理，不归档；
        // - 复验解得开 → 用复验的结果，说明上一次读到的是别人写入中途的快照；
        // - 两次都真解不开 → 才是内容损坏，走 ③。
        // 8-12 那批文件只要走到这一步就活了：它们从来没解码失败过，是 open() 就挂了。
        usleep(readRetryBackoff[0])
        guard let recheck = try readDataIfExists(at: url) else { return [] }
        if let rows = decodeRows(type, from: recheck) { return rows }
        onIncident(.corrupt(archive: quarantine(url)))
        return []
    }

    /// 逐条 lenient 解码（②）。返回 nil = 这份字节**确实**解不出来：外层 JSON 解析
    /// 失败，或非空数组里一条都活不下来。
    private static func decodeRows<Row: Decodable>(_ type: Row.Type, from data: Data) -> [Row]? {
        guard let rows = try? JSONDecoder().decode([FailableRow<Row>].self, from: data) else {
            return nil
        }
        let surviving = rows.compactMap(\.row)
        guard rows.isEmpty || !surviving.isEmpty else { return nil }
        return surviving
    }

    /// 最后一层拒写闸：调用方刚读到空表，但同一把文件锁内磁盘文件仍存在且非空，
    /// 说明「合法空」之外的读失败形态漏过了读侧。**拒绝本次整写、原件一个字节不动**，
    /// 绝不让 `[] + newRow` 把历史覆盖掉。文件大小查不到也按不安全处理。
    ///
    /// 2026-08-12 之前这里会先 `quarantine(url)` 再拒写 —— 那是把「我可能读漏了」
    /// 当成「文件坏了」在处置，同一个病。现在只拒写、只报警（`.misread`），
    /// 原件一个字节不动。
    @discardableResult
    static func refuseEmptyRewriteIfNonEmptyFile<Row>(
        _ rows: [Row], at url: URL, onRefusal: (LedgerIncident) -> Void = { _ in }
    ) -> Bool {
        guard rows.isEmpty, FileManager.default.fileExists(atPath: url.path) else { return false }
        let size: UInt64?
        do {
            let attrs = try FileManager.default.attributesOfItem(atPath: url.path)
            size = (attrs[.size] as? NSNumber)?.uint64Value
        } catch {
            size = nil
        }
        if let size, size == 0 { return false }
        if holdsEmptyJSONArray(url) { return false }
        onRefusal(.misread)
        return true
    }

    /// 文件内容就是一个**合法的空数组**（`[]`）—— 那是账本被正常清空后的样子
    /// （如最后一条 wakeup 触发后移除），不是「漏读」。不这样区分的话，空账本
    /// 每次写入都会被上面的拒写闸当成读失败拒掉，写不进去。读不出来（fd 打满 /
    /// 权限 / IO）时这里也读不出来 → 不放行，仍然拒写（宁可写不进，不可覆盖）。
    private static func holdsEmptyJSONArray(_ url: URL) -> Bool {
        guard let bytes = try? readDataIfExists(at: url),
              let array = try? JSONSerialization.jsonObject(with: bytes) as? [Any] else {
            return false
        }
        return array.isEmpty
    }

    /// 锁内整写（atomic：临时文件 + rename）。编码失败放弃本次写（行结构都是
    /// 简单 Codable，实际不会发生），宁可少写一笔也不落半截文件。
    ///
    /// **返回 nil = 这些行真的落到磁盘上了**；返回非 nil = 这次没写进去，错误原样带回。
    ///
    /// 2026-09-09 之前这里是个光秃秃的 `try?`，返回 `Void` —— 于是「写盘失败」
    /// 在**每一个**调用点都长得跟成功一模一样：账本 store 照常返回改好的那一行，
    /// MCP 工具照常回一句「已回应 / 已排上 / 已提交」，而磁盘上什么都没发生。
    /// 读那一侧早就是 fail-closed 的（`.unreadable` / 拒空写闸），**写这一侧
    /// 一直是敞开的** —— 这个不对称就是「回执说成功了、那件事其实没发生」
    /// 这一族缺陷在本仓库的总病根。
    ///
    /// 保留 `@discardableResult`：确实不在乎的调用点（纯缓存类）可以照旧忽略，
    /// 但**忽略必须是一次显式选择**，而不是这一层根本说不出话。
    @discardableResult
    static func saveRowsLocked<Row: Encodable>(_ rows: [Row], to url: URL) -> Error? {
        do {
            try saveRowsLockedReportingFailure(rows, to: url)
            return nil
        } catch {
            return error
        }
    }

    /// 需要确认投递成功的调用点使用这个版本。既有 store 多为 best-effort，继续走
    /// 上面的无返回包装；开场任务白板留痕不能静默丢，必须把编码/IO 错误抛回父 crew。
    static func saveRowsLockedReportingFailure<Row: Encodable>(
        _ rows: [Row], to url: URL
    ) throws {
        let data = try JSONEncoder().encode(rows)
        try writeStaged(data, to: url)
    }

    /// 整写一份字节：**临时文件在数据根之外出生**，再 `rename` 进来。
    ///
    /// 为什么不能用 `Data.write(options: .atomic)`：它把临时文件建在**目标目录里**，
    /// 于是新文件带上那个周期性 EPERM 故障的标记（标记按创建位置打、之后跟着文件走）。
    /// 从外面 `rename` 进来的文件没有标记，发作期间照样读得动。
    /// 判据与对照见 `PendingCrewDataRoot.stagingDirectory` 的注释。
    ///
    /// **原子性不降级**：POSIX `rename` 本来就是原子替换，跟 `.atomic` 给的是
    /// 同一条保证；差别只在临时文件建在哪儿。
    ///
    /// 落脚点建不了、或跟目标**跨卷**（`rename` EXDEV）时**退回 `.atomic`** ——
    /// 那种环境下这个故障本来也不成立（它只在 Application Support 底下发作）。
    ///
    /// - Returns: 实际用过的落脚路径；`nil` = 走了退路（原地原子写）。
    ///   生产调用点不看它，**测试靠它断言「这个文件不是在目标目录里出生的」** ——
    ///   出生地是这个函数的全部意义，而出生地事后在磁盘上看不出来。
    @discardableResult
    static func writeStaged(
        _ data: Data, to url: URL,
        staging: URL = PendingCrewDataRoot.stagingDirectory
    ) throws -> URL? {
        let fm = FileManager.default
        do {
            try fm.createDirectory(at: staging, withIntermediateDirectories: true)
            let staged = staging.appendingPathComponent(UUID().uuidString + ".staged")
            try data.write(to: staged)
            defer { try? fm.removeItem(at: staged) }
            try fm.createDirectory(
                at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            guard rename(staged.path, url.path) == 0 else {
                throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
            }
            return staged
        } catch {
            try data.write(to: url, options: .atomic)
            return nil
        }
    }

    /// 把**确认损坏**的文件挪到 `<file>.corrupt-<unix毫秒>`（同目录，人工可找回）。
    /// 挪不动（极端 IO 错误 / 文件已消失）→ nil，原文件不动。
    ///
    /// 调用前必须已经确认「两次独立读到的字节都真解不开」（见
    /// `loadRowsLockedReportingFailure` 的复验）。**读不出来不许调这个。**
    /// 反过来说：从 2026-08-12 起，目录里出现一个 `.corrupt-*` 就意味着那份字节
    /// 真的解不开 —— 归档的存在本身是「内容损坏」的判据，「读不动」不再产生归档。
    static func quarantine(_ url: URL) -> URL? {
        let ts = UInt64(Date().timeIntervalSince1970 * 1000)
        let archive = url.deletingLastPathComponent()
            .appendingPathComponent(url.lastPathComponent + ".corrupt-\(ts)")
        do {
            try FileManager.default.moveItem(at: url, to: archive)
            return archive
        } catch {
            return nil
        }
    }
}

/// 逐条 lenient 解码壳（②）：元素解码失败吞成 `nil`（丢那一条），不让单条坏行
/// 把整个数组解码失败连坐成空表。新旧 schema 混跑时只丢真坏的。
private struct FailableRow<Row: Decodable>: Decodable {
    let row: Row?
    init(from decoder: Decoder) throws {
        row = try? Row(from: decoder)
    }
}

/// 一个 key 最多存这么多条。**这不是容量估算，是一道跑飞保护**：故障窗口可能持续
/// 几小时，而这期间自动重试的东西（定时提醒、轮询、循环里的 agent）会一直往里存。
/// 没有上限的话，一个卡住的循环能把盘刷满，而且是在**系统已经出着毛病**的时候 ——
/// 那是最不该再补一刀的时刻。
///
/// 到顶之后**拒绝再存并如实说**（`spool` 返回 false，调用方的回执本来就分
/// 「存下来了 / 连存都没存下」两句话）。**不丢旧的换新的** —— 旧的那些是先发生的，
/// 丢它们等于按时间倒序丢数据，而且没有任何人看得见。
enum LedgerSpoolLimits {
    static let capacityPerKey = 500
}

/// 定序用的进程内计数器。**泛型类型不能有存储型 static**，所以单拎出来放这儿
/// （上面那个上限同理）。
private enum LedgerSpoolSequence {
    private static var value: UInt64 = 0
    static func next() -> UInt64 { value &+= 1; return value }
}

/// **落盘失败时把内容先存成一个新文件，等那本账重新读得动了再补回去。**
///
/// ## 为什么存得下来
///
/// 这台机器上那个周期性故障的形状是：**`open()` 一个已经存在的 inode 被拒，
/// `open(O_CREAT)` 建新文件照样成**（逐系统调用量过，见
/// `docs/internal/2026-09-12-eperm-cause-found.md`）。所以「存不下来」从来不是事实，
/// 只是以前没人去存 —— 每一窗里 agent 组织好的消息、提上来的 Todo 都当场蒸发，
/// 回执还明写着「没有留在任何地方」。
///
/// ## 两条不肯让步的规矩（都是被红测按着头学会的）
///
/// 1. **一条一个新文件，绝不追加进一个共用文件。** 追加要先读，而那条路正是断的。
/// 2. **文件名必须是真的单调键。** 第一版用秒级 ISO8601 时间戳，于是同一秒存下的两条
///    排序由后面那个 uuid 决定 = 随机，补回去是倒着的。**账上因果颠倒比丢一条更难
///    发现**。所以是「高精度定宽时间戳（字典序=时间序）+ 进程内序号兜同微秒」。
///
/// ## 还有一条是接线，不是算法
///
/// 补发**不能只挂在写路径上**。只在「下一次写成功」时补，意味着一本账只要之后没人再
/// 写，存下来的就永远补不回来、也永远看不见 —— 那是另一种形式的丢，而且更难发现，
/// 因为回执已经承诺过会自动补。所以读路径也要调 `drain`（先用 `hasPending` 便宜地
/// 探一眼，没积压就什么都不做）。
struct LedgerSpool<Payload: Codable> {

    /// 存哪儿。**不在被拒的那份账文件旁边另起炉灶** —— 用独立子目录，
    /// 免得它自己被当成账的一部分读进去。
    let directory: URL

    init(directory: URL) { self.directory = directory }

    /// 存一条。`key` 用来分账（通常是 crewId）：补发只认自己那一份。
    @discardableResult
    func spool(_ payload: Payload, key: String) -> Bool {
        guard fileNames(key: key).count < LedgerSpoolLimits.capacityPerKey else { return false }
        do {
            try FileManager.default.createDirectory(
                at: directory, withIntermediateDirectories: true)
            let name = key + "."
                + String(format: "%017.6f", Date().timeIntervalSince1970) + "."
                + String(format: "%06llu", LedgerSpoolSequence.next()) + "."
                + UUID().uuidString.lowercased() + ".json"
            // 同样走落脚点：待发件箱的文件建在数据根里就会带上标记，
            // 而它恰恰要在故障期间被写、故障过去后被读回来。
            try MultiProcessJSONStore.writeStaged(
                JSONEncoder().encode(payload), to: directory.appendingPathComponent(name))
            return true
        } catch {
            return false
        }
    }

    /// 有没有积压。**列目录在那个故障里是通的**，所以这一眼很便宜，
    /// 可以放在读路径上每次都问。
    func hasPending(key: String) -> Bool {
        !fileNames(key: key).isEmpty
    }

    /// 按当初的顺序交给 `consume`。`consume` 返回 true = 已经吃下，这条就删掉；
    /// 返回 false = 这次先不动它（下次再试）。返回吃下了几条。
    @discardableResult
    func drain(key: String, consume: (Payload) -> Bool) -> Int {
        var n = 0
        for name in fileNames(key: key) {
            let u = directory.appendingPathComponent(name)
            guard let data = try? Data(contentsOf: u),
                  let payload = try? JSONDecoder().decode(Payload.self, from: data)
            else { continue }   // 这一条读不出来：**原地留着**，别删 —— 删了就是丢
            guard consume(payload) else { continue }
            n += 1
            try? FileManager.default.removeItem(at: u)
        }
        return n
    }

    private func fileNames(key: String) -> [String] {
        let all = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
        return all.filter { $0.hasPrefix(key + ".") && $0.hasSuffix(".json") }.sorted()
    }
}

