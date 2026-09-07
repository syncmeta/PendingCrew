#if os(macOS)
import Combine
import Foundation

/// 菜单栏那个数字的取数层（P5b·B）。判定在 `HumanAttentionTally`（进得了单测），
/// 这里只负责把三样输入捞出来喂给它。
///
/// ## 为什么读 `crew-sessions.json` 而不是内存里的 `runs`
///
/// app 有两副面孔：自己当编排者，和退化成 viewer 连着 daemon。**只有点名快照这一份
/// 在两种模式下都是满的** —— 内存里的 `runs` 在 viewer 模式下是镜像来的，而快照文件
/// 无论谁在编排都由 `CrewSessionRunner` 写。挑内存那份的话，菜单栏会在"后台在跑、
/// 界面只是个 viewer"时莫名其妙地少数几件事。
///
/// ## 读不动的时候**不许显示 0**
///
/// 数据根整个读不出来是真实发生过的事（今天就发作过）。这种时候把数字刷成 0，
/// 等于告诉人「没事了」——而那正是最不该在这时候说的话。所以读失败就**保留上一次的
/// 数字并标记 stale**，面板上说明白「这个数是几点几分的，现在读不到账」。
@MainActor
final class MenuBarAttentionModel: ObservableObject {
    @Published private(set) var count = HumanAttentionCount()
    /// 上一次成功取数的时刻。读不动时数字不动，只有这个时刻会变旧。
    @Published private(set) var lastGoodAt: Date?
    /// 最近一次取数失败的原因（nil = 上一次是成功的）。
    @Published private(set) var staleReason: String?

    private var timer: Timer?
    private weak var crewStore: CrewStore?

    /// 2 秒一拍 —— 跟点名快照同一个节律，不另起一套。
    private static let interval: TimeInterval = 2

    func start(crewStore: CrewStore) {
        self.crewStore = crewStore
        timer?.invalidate()
        refresh()
        timer = Timer.scheduledTimer(withTimeInterval: Self.interval, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refresh() }
        }
    }

    func stop() {
        timer?.invalidate()
        timer = nil
    }

    private func refresh() {
        guard let crewStore else { return }
        let crewIds = crewStore.crews.map(\.id)
        let approvals = crewIds.flatMap { LocalApprovalStore.shared.pending(crewId: $0) }
            .map(\.sessionId)
        let todos = crewStore.humanTodoAttention.values.reduce(0) { $0 + $1.ownUnanswered }

        do {
            let states = try Self.rosterStates()
            count = HumanAttentionTally.tally(
                pendingApprovalSessionIds: approvals,
                sessionStates: states,
                unansweredTodos: todos)
            lastGoodAt = Date()
            staleReason = nil
        } catch {
            // **数字不动。** 见类型注释：读不动的时候刷成 0 就是在说「没事了」。
            staleReason = error.localizedDescription
        }
    }

    /// `sessionId → 点名状态`。**文件不在 = 真的还没有任何 session**（全新机器），
    /// 读不动 / 解不开 = 抛 —— 照 `CrewDirectory.load` 那个已有的正确形状写，
    /// 不再发明第二种。
    private static func rosterStates() throws -> [String: String] {
        let url = LocalWhiteboardStore.defaultDirectory
            .appendingPathComponent(CrewSessionsSnapshot.fileName)
        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch let error as NSError
            where error.domain == NSCocoaErrorDomain && error.code == NSFileNoSuchFileError {
            return [:]
        }
        let snapshot = try JSONDecoder().decode(CrewSessionsSnapshot.self, from: data)
        var out: [String: String] = [:]
        for entry in snapshot.crews.values.flatMap({ $0 }) {
            out[entry.sessionId] = entry.state
        }
        return out
    }
}
#endif
