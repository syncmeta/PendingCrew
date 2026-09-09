import Foundation

/// 定时唤醒账本（schedule_wakeup；#455 额度重置自唤醒）。从 `CrewSessionRunner`
/// 的私有 loadWakeups/saveWakeups 抽出成独立 store（#528）：`wakeups.json` 承载
/// 「有约必赴」承诺，此前 `(try? decode) ?? []` 损坏即当空、下一次写以空数组
/// 重写落盘 —— 全部在途约定静默失约。现在走 `MultiProcessJSONStore` 基座三件套
/// （`wakeups.lock` flock + 逐条 lenient + corrupt 归档 fail-loud）。
///
/// 单文件跨 crew 共用（行内带 crewId）。当前唯一读写方是 app 进程的 runner，
/// 但文件与白板同目录，上锁防未来 helper/多窗并发，也让 register 的
/// 「查重-追加」原子化。**自包含 Foundation**（编进 PendingCrewTests bundle 单测）。
final class LocalWakeupStore: @unchecked Sendable {
    /// 一条待触发的定时唤醒。`fireAt` = ISO8601 触发时刻。
    struct PendingWakeup: Codable, Equatable {
        let id: String
        let crewId: String
        let sessionId: String
        var fireAt: String
        let note: String
        /// **督办租约**（人类 Todo #107，见 `SupervisionLease`）：这条唤醒是替哪条
        /// 计划盯着的。nil = 普通 `schedule_wakeup` 定时唤醒，两者走同一条账本 /
        /// 定时器 / 重挂路径 —— 那是仓库里已有的正确孪生，别为督办另起一套。
        ///
        /// 以下四个字段全部可选，磁盘上还躺着一批只有前五个字段的老约定，
        /// 它们必须照常解得开（否则这次加字段会把全部在途约定一次抹掉，
        /// 正是 #528 修的那一族事故）。
        var planNumber: Int? = nil
        /// 挂上督办的那一刻（ISO8601）。到期文案里「已 X 没有结果」从它算起 ——
        /// **不是**从上一次响铃算，所以退避重排时它不动。
        var leaseSince: String? = nil
        /// 基础间隔（秒）。退避在它上面翻倍。
        var leaseBaseSeconds: Double? = nil
        /// 退避档位。0 = 还没叫过；**每真的叫到人一次** +1（没叫到不推进）。
        var leaseStep: Int? = nil
    }

    private let directory: URL

    init(directory: URL? = nil) {
        self.directory = directory ?? LocalWhiteboardStore.defaultDirectory
        try? FileManager.default.createDirectory(at: self.directory, withIntermediateDirectories: true)
    }

    /// 全部待触发唤醒。出事 → `onIncident`（读不出来 / 漏读 = 原件完好、本次写已拒；
    /// 确认解不开 = 已归档、人工可找回），调用方负责 fail-loud（白板警示）。
    /// 一次读的结果 —— **三态里的后两态（真空 / 读不到）不许压成同一个空数组**。
    ///
    /// 这是 `LocalTodoStore.LedgerRead` 的**孪生**（`c12c80c` 做的），形状照抄，不是
    /// 第二种设计。同一个病：`list()` 把 `loadLocked` 的失败压成 `[]`，于是
    /// 「一条待唤醒都没有」和「这本账这次读不出来」在调用方眼里长得一模一样。
    ///
    /// **这里的后果比 Todo 那本更重**：`CrewSessionRunner.rearmWakeups()` 在 app 启动时
    /// 读它来重挂全部定时唤醒。读失败当成空表 = **所有在途的唤醒（含督办租约）静默
    /// 全部消失**，而且没有任何人会发现 —— 那正是 #107「别人停了我不知道」的形状。
    ///
    /// 注意写路径**早就**在用这个信号（`register` 里的 `refuseEmptyRewriteIfNonEmptyFile`），
    /// 只有读路径把它扔了。**「三态压成一个值」在这个仓库里是个反复出现的形状**，
    /// 不是某一个 store 的疏漏。
    enum LedgerRead: Equatable {
        case rows([PendingWakeup])
        /// 这次没读到可信内容。**任何一种事故都算**（打不开 / 读到空但文件非空 / 解不开）。
        case unreadable
    }

    /// 跟 `list` 是**同一条读**（同一把锁、同一个解码、同一份事故上报），区别只在于
    /// 把「读不出来」交还给调用方。
    func read(onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void = { _ in }) -> LedgerRead {
        withFileLock {
            var hadIncident = false
            let rows = MultiProcessJSONStore.loadRowsLocked(
                PendingWakeup.self, at: fileURL,
                onIncident: { incident in hadIncident = true; onIncident(incident) })
            return hadIncident ? .unreadable : .rows(rows)
        }
    }

    /// 待唤醒清单。**读不出来时返回空表** —— 历史行为，既有调用点按它写的，这一笔
    /// 不动它。要区分「真的没有」和「读不到」的调用方走 `read(onIncident:)`
    /// （判「有没有事要做」的那类路径**必须**走那条，理由见 `LedgerRead`）。
    func list(onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void = { _ in }) -> [PendingWakeup] {
        if case let .rows(rows) = read(onIncident: onIncident) { return rows }
        return []
    }

    /// 登记一条（同 id 已存在 → no-op，drain 重放安全）。返回是否真的新登记。
    @discardableResult
    func register(_ w: PendingWakeup, onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void = { _ in }) -> Bool {
        withFileLock {
            var rows = loadLocked(onIncident: onIncident)
            guard !MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(
                rows, at: fileURL) else { return false }
            guard !rows.contains(where: { $0.id == w.id }) else { return false }
            rows.append(w)
            MultiProcessJSONStore.saveRowsLocked(rows, to: fileURL)
            return true
        }
    }

    /// 移除一条（触发后清账）。
    ///
    /// 这里也必须过拒写闸（#577）：读不出来时 `loadLocked` 给的是空表，直接
    /// `filter + save` 就是拿空数组整写覆盖 —— 全部在途约定一次抹光，正是 #576
    /// 那道闸要拦的形态，而这条路径当初漏装了闸。
    func remove(id: String, onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void = { _ in }) {
        withFileLock {
            let rows = loadLocked(onIncident: onIncident)
            guard !MultiProcessJSONStore.refuseEmptyRewriteIfNonEmptyFile(
                rows, at: fileURL) else { return }
            MultiProcessJSONStore.saveRowsLocked(rows.filter { $0.id != id }, to: fileURL)
        }
    }

    // MARK: - Persistence（基座三件套）

    private var fileURL: URL { directory.appendingPathComponent("wakeups.json") }

    private func withFileLock<T>(_ body: () -> T) -> T {
        MultiProcessJSONStore.withFileLock(
            directory.appendingPathComponent("wakeups.lock"), body)
    }

    private func loadLocked(onIncident: (MultiProcessJSONStore.LedgerIncident) -> Void) -> [PendingWakeup] {
        MultiProcessJSONStore.loadRowsLocked(PendingWakeup.self, at: fileURL, onIncident: onIncident)
    }
}
