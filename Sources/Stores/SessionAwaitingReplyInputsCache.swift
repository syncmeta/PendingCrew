import Foundation

/// 「在等谁回话」那两样**磁盘输入**的指纹门控缓存（2026-08-18：开久了卡第三条）。
///
/// ## 病根
///
/// `CrewSessionRunner` 的点名快照定时器每 **2 秒**在 MainActor 上跑一拍，一拍里：
/// 1. 对**每个有 agent run 的 crew** 各来一次 `LocalApprovalStore.pending(crewId:)`
///    —— 而 `pending` → `list` 是 **flock + 读整份 JSON + 全量解码**；
/// 2. 再对**每个 agent run** 各读一次它的 `<crewId>.<sessionId>.turn` marker
///    （`Data(contentsOf:)` + 解码），拿上一轮的收尾问句。
///
/// 两样都在主线程，而 crew / run 的数量随着「跑完不移除」单向增长（第二条那个病的
/// 同一个根）。开一天下来就是每 2 秒几十次加锁读盘挂在主线程上。
///
/// ## 这里做的事
///
/// 两样各挂一个 `FileFingerprintCache`：先 stat 指纹，没变直接用上次的结果，
/// **一个字节都不读、一把锁都不拿**。整体由 `CrewSessionRunner` 放到后台队列上调，
/// 主线程只剩「把算好的值写回 run 的 @Published」这一步内存活。
///
/// 判定口径一个字没改 —— 仍然是 `LocalApprovalStore.pending` 与 `SessionTurnMarker.read`
/// 的原样结果喂给 `SessionAwaitingReply.reason`，只是不再每拍重读。
///
/// ## mtime+size 指纹在 approvals 上成不成立（**与白板不是同一套论证**）
///
/// 白板那条能用指纹，靠的是「只追加、尺寸单调增」。审批账本**不是纯追加**：它有
/// 状态翻转（`pending` → `answered`）。所以要单独论证：
///
/// - 写路径只有一条漏斗 `saveLocked`（raise / answer / decide 都经它），整份重写。
///   `raise` 追加一条 → 尺寸必增。
/// - `answer` 把 `"status":"pending"` 改成 `"status":"answered"`（+1 字节）并把
///   `"reply":null` 换成非空字符串；`decide` 同理换 `"decision":null` → `"allow"`/`"deny"`。
///   **两种翻转都只会让 JSON 变长**，不存在「翻转后尺寸不变」。
/// - 没有删除路径（条目只增不减），所以文件不会缩短；唯一会缩短的是损坏重建，
///   与原文件同尺寸的概率可忽略。
/// - mtime 取自 `stat(2)` 的 `st_mtimespec`（**纳秒**精度），而每次落盘是 `.atomic`
///   替换、取的是 rename 时刻 —— 同一纳秒两次 rename 不成立。
///
/// 也就是说：**这里不依赖「只追加」，依赖的是「任何一次写都必然改变字节数」**，
/// 而这一条在审批账本的四条写路径上逐条成立。
///
/// turn marker 那侧更弱一点（`awaitingQuestion` 换成另一句**等长**的问句、且与上次
/// 写在同一纳秒，才会漏判），实际撞不上：marker 每轮结束才写一次，两轮之间隔着一整轮
/// 对话；且漏判的后果是红点晚一拍（下一次真变化即纠正），不是判错。
///
/// ## 线程
///
/// 非 `@MainActor`，**要在后台队列上调**。内部状态由两个 `FileFingerprintCache` 自己
/// 的锁保护。
final class SessionAwaitingReplyInputsCache: @unchecked Sendable {

    /// 一个 agent run 的身份（审批按 crew 存、marker 按 crew+session 存）。
    struct RunKey: Hashable, Sendable {
        let crewId: String
        let sessionId: String
    }

    /// 一个 run 这一拍的两样磁盘输入，原样喂给 `SessionAwaitingReply.Input`。
    struct Inputs: Equatable, Sendable {
        var pendingApprovalSummary: String?
        var trailingQuestion: String?
    }

    /// crewId → (sessionId → 该 session 最早那条 pending 的摘要)。
    private let approvals: FileFingerprintCache<String, [String: String]>
    /// run → 上一轮的收尾问句。
    private let markers: FileFingerprintCache<RunKey, String>

    /// 生产用：审批走 `LocalApprovalStore.shared`，marker 走白板目录。
    ///
    /// 两条指纹都按「目录路径 + 字符串拼接」造路径（`FileChangeGate.fingerprint(atPath:)`）
    /// —— 一拍要 stat 上百个文件，为每次 stat 建一个 `URL` 比 stat 本身还贵。
    convenience init(directory: URL, store: LocalApprovalStore = .shared) {
        let dirPath = directory.path
        self.init(
            approvalsFingerprint: { store.fingerprint(crewId: $0) },
            pendingBySession: { crewId in
                var bySession: [String: String] = [:]
                // 一个 session 同时挂多条时取**最早**那条 —— 与改前
                // `refreshAwaitingReplies` 的 `where bySession[...] == nil` 同口径。
                for item in store.pending(crewId: crewId) where bySession[item.sessionId] == nil {
                    bySession[item.sessionId] = item.summary
                }
                return bySession.isEmpty ? nil : bySession
            },
            markerFingerprint: { key in
                FileChangeGate.fingerprint(
                    atPath: dirPath + "/" + key.crewId + "." + key.sessionId + ".turn")
            },
            trailingQuestion: { key in
                SessionTurnMarker(directory: directory, crewId: key.crewId,
                                  sessionId: key.sessionId).read().awaitingQuestion
            })
    }

    /// 单测 / 基准用：四条 IO 全可注入，好数「一拍里到底真读了几次」。
    init(approvalsFingerprint: @escaping (String) -> FileChangeGate.Fingerprint?,
         pendingBySession: @escaping (String) -> [String: String]?,
         markerFingerprint: @escaping (RunKey) -> FileChangeGate.Fingerprint?,
         trailingQuestion: @escaping (RunKey) -> String?) {
        approvals = FileFingerprintCache(fingerprintOf: approvalsFingerprint,
                                         load: pendingBySession)
        markers = FileFingerprintCache(fingerprintOf: markerFingerprint,
                                       load: trailingQuestion)
    }

    /// 这一拍真正做过多少次「读文件 + 解码」（审批账本 / turn marker 分开数）。
    /// 验收口径：改前恒等于 `crew 数 + run 数`，改后在无变化的一拍应为 0。
    var approvalDecodeCount: Int { approvals.loadCount }
    var markerReadCount: Int { markers.loadCount }

    /// 刷新这批 run 的两样输入。**在后台队列上调。**
    ///
    /// - Returns: 每个 run 一条（恒是完整快照；没有待审批 / 没有收尾问句就是两个 nil）。
    func refresh(runs: [RunKey]) -> [RunKey: Inputs] {
        let crewIds = Array(Set(runs.map(\.crewId)))
        let pendingByCrew = approvals.refresh(keys: crewIds)
        let questions = markers.refresh(keys: runs)
        var result: [RunKey: Inputs] = [:]
        result.reserveCapacity(runs.count)
        for run in runs {
            result[run] = Inputs(
                pendingApprovalSummary: pendingByCrew[run.crewId]?[run.sessionId],
                trailingQuestion: questions[run])
        }
        return result
    }

    func clear() {
        approvals.clear()
        markers.clear()
    }
}
