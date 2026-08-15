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
        case unreadable(Error)
        /// 读到空表但磁盘文件非空 —— 漏读的另一种形态。同样原件不动、本次写已拒。
        case misread
        /// **两次独立读到的字节都真解不开**。已归档为 `.corrupt-<ts>`
        /// （nil = 归档也挪不动，原件留在原地）。
        case corrupt(archive: URL?)

        /// 说给人听的一句话；调用方在前面接主语（「白板」/「人类 Todo 列表」/…）。
        var summary: String {
            switch self {
            case .unreadable(let error):
                return "这次读不出来（\(error.localizedDescription)）。"
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
        var current: NSError? = error as NSError
        var depth = 0
        while let e = current, depth < 4 {
            if predicate(e) { return true }
            current = e.userInfo[NSUnderlyingErrorKey] as? NSError
            depth += 1
        }
        return false
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
            onIncident(.unreadable(error))
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
    static func saveRowsLocked<Row: Encodable>(_ rows: [Row], to url: URL) {
        try? saveRowsLockedReportingFailure(rows, to: url)
    }

    /// 需要确认投递成功的调用点使用这个版本。既有 store 多为 best-effort，继续走
    /// 上面的无返回包装；开场任务白板留痕不能静默丢，必须把编码/IO 错误抛回父 crew。
    static func saveRowsLockedReportingFailure<Row: Encodable>(
        _ rows: [Row], to url: URL
    ) throws {
        let data = try JSONEncoder().encode(rows)
        try data.write(to: url, options: .atomic)
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
