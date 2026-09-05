#if os(macOS)
import Foundation

/// 唤醒**投递回执**的纯判定（wake-resilience：修「假送达」）。
///
/// 本 enum 原来还装着 Phase 4b 事件驱动唤醒（edge mailbox → 注入）的决策核心
/// （`decide` / `renderInjection`）。那条路随 #63 第二期删除跨端遥控整层一起
/// 端掉了 —— 它的输入 `CrewMailboxItem` 来自 edge `getSessionInbox()`，本地
/// 白板永远产不出。留下来的这半跟 edge 无关：注入之后目标 run 到底有没有真的
/// 动起来，本地 @ 直投（`CrewLocalMentionDelivery`）与机长唤醒共用这一份判据。
///
/// 采样编排是 IO（`CrewSessionRunner.confirmWake`），判定在下面的纯函数。
enum CrewMailboxWakeLogic {

    // MARK: - 投递回执（wake-resilience：修「假送达」）

    /// 回执观察窗（秒）与采样间隔（秒）。注入后在窗内周期采样目标 run 的工作态；
    /// 采样编排是 IO（`CrewSessionRunner.confirmWake`），判定在下面的纯函数。
    static let receiptWindow: TimeInterval = 10
    static let receiptSampleInterval: TimeInterval = 1

    enum ReceiptVerdict: Equatable {
        /// 观察窗内目标转入过工作态 → 注入被吃进去了，可以消费（mark-delivered / 推游标）。
        case confirmed
        /// 整窗未见工作态 → 判定唤醒失败：不消费（留待重投）+ 白板告警 @captain。
        case failed
    }

    /// 注入回执的一拍证据。`activityRevision` 是 Codex transcript/turn/tool 事件的
    /// 单调序号；`latestPostId` 是目标 session 最近一次发群消息的 id。
    struct ReceiptEvidence: Equatable {
        let isWorking: Bool
        let activityRevision: UInt64
        let latestPostId: String?
        /// 最近一次收到子进程输出的时刻（`AgentSessionCore.lastOutputAt`）。**单调**，
        /// 因此跨得过采样间隙 —— `isWorking`（「最近 1s 内有输出」）跨不过：采样周期
        /// 1s、判据窗口 1s、claude 状态行也是 1s 一跳，三个 1 撞在一起相位就固定了，
        /// 整窗都采在跳字之前时它一路读到 false，而这个时刻已经前进了好几秒。
        /// codex 那条路早就有单调证据（`activityRevision`），claude 这条路缺的就是它。
        let lastOutputAt: Date
        /// 目标**此刻**是否还挂着「我在干活」的指示（claude 看终端状态行，codex 看
        /// 结构化 turn）。它不作为「到达」的证据 —— 只用来回答另一个问题：整窗安静
        /// 到底是「卡死了」还是「在做一件长的、不吐字的事」。见 `shouldKeepWaiting`。
        let isBusyNow: Bool

        init(isWorking: Bool, activityRevision: UInt64, latestPostId: String?,
             lastOutputAt: Date = .distantPast, isBusyNow: Bool = false) {
            self.isWorking = isWorking
            self.activityRevision = activityRevision
            self.latestPostId = latestPostId
            self.lastOutputAt = lastOutputAt
            self.isBusyNow = isBusyNow
        }
    }

    /// 判定一次唤醒注入是否真正到达。`workingSamples` = 注入后观察窗内周期采样的
    /// 目标工作态（`isBusy || isWorking` —— claude 的 `isBusy` 恒 false（PTY 无
    /// turn-state），真信号是输出活跃度 `isWorking`：注入被吃进去后 agent 起一轮
    /// turn，spinner/输出持续吐字；卡在模态菜单/进程假死时只有注入瞬间的一次
    /// 回显，之后整窗安静）。任一拍见工作态 → confirmed；全程安静（含空采样，
    /// 如 run 已退出）→ failed。
    static func receiptVerdict(workingSamples: [Bool]) -> ReceiptVerdict {
        workingSamples.contains(true) ? .confirmed : .failed
    }

    /// 带跨采样间隙证据的回执判定。瞬时 working 只覆盖“采样恰好撞见 turn”；
    /// revision / post id 是单调硬证据，覆盖 Codex 短 turn 在首拍之前已经结束的竞态。
    static func receiptVerdict(
        baseline: ReceiptEvidence, samples: [ReceiptEvidence]
    ) -> ReceiptVerdict {
        samples.contains { sample in
            sample.isWorking
                || sample.activityRevision != baseline.activityRevision
                || sample.latestPostId != baseline.latestPostId
                || sample.lastOutputAt != baseline.lastOutputAt
        } ? .confirmed : .failed
    }

    // MARK: - 长静默 ≠ 卡死

    /// 「它还挂着忙碌指示，我继续等」这句话的**寿命**。压缩上下文、长思考、长工具
    /// 等待都能安静好几分钟；挂着指示却一动不动超过这个数，才轮到告警说话。
    static let busyWaitLimit: TimeInterval = 300

    /// claude 干活时终端状态行上的标记。只用来**压掉误报**，绝不用来产生「到达」——
    /// 所以将来 claude 改了文案，退化回的是今天的行为（照旧告警），不是更糟。
    static let claudeBusyMarkers = ["esc to interrupt", "compacting"]

    /// 终端末几行里有没有 claude 的忙碌指示。
    static func claudeIsBusy(screenTail: String) -> Bool {
        let text = screenTail.lowercased()
        return claudeBusyMarkers.contains { text.contains($0) }
    }

    /// 观察窗到头、但还没有到达证据时：还要不要接着等？
    ///
    /// 老实现在这里没有分支 —— 10s 一到就判失败、喊「疑似卡死」、把消息退回去重投。
    /// 于是**同一句话对应了两种处境**：真卡死（模态菜单/假死）和「在做一件长的、
    /// 不吐字的事」（压缩上下文就是，人类 2026-09-05 实测撞到）。后者被误判的代价
    /// 不只是喊错，还会让那条消息被重投一遍 —— 重放旧指令那个病的来源之一。
    ///
    /// 所以窗口的寿命是**有条件的**：目标还挂着忙碌指示就续着等（有上限），
    /// 什么都没挂就按老口径立刻收摊。
    static func shouldKeepWaiting(
        latest: ReceiptEvidence?, elapsed: TimeInterval, limit: TimeInterval = busyWaitLimit
    ) -> Bool {
        if elapsed < receiptWindow { return true }
        guard let latest, latest.isBusyNow else { return false }
        return elapsed < limit
    }

    /// 唤醒失败的白板告警正文（调用方以 system 身份贴白板并 @captain —— system
    /// 条目免回执，不会因机长也唤不醒而告警成环）。
    static func wakeFailureAlert(targetLabel: String, window: TimeInterval = receiptWindow) -> String {
        "唤醒失败：「\(targetLabel)」注入 \(Int(window))s 后仍未转入工作态，疑似卡死。"
            + "消息留待重投；机长可 inspect_session / nudge_session 解卡。"
    }

    /// 挂着忙碌指示一路等到寿命上限的那条告警。**跟上面那条必须是两句话**：这一种
    /// 处境里目标看起来一直在干活，只是不吐字，所以要把「我在看什么、看了多久」和
    /// 「这可能是误报」一起说出来，别让人按「卡死」去 nudge 一个正在跑的 session。
    static func wakeBusyStallAlert(targetLabel: String, waited: TimeInterval) -> String {
        "「\(targetLabel)」注入后已经 \(Int(waited))s 一直挂着忙碌指示，但没有新输出、"
            + "也没有新发言。若它正在做一件长的不吐字的事（压缩上下文、长思考、长工具等待），"
            + "这条是误报，不用管；真要确认就 inspect_session 看一眼终端现场。消息留待重投。"
    }
}
#endif
