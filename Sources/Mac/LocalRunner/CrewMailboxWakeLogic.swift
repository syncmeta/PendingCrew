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
        } ? .confirmed : .failed
    }

    /// 唤醒失败的白板告警正文（调用方以 system 身份贴白板并 @captain —— system
    /// 条目免回执，不会因机长也唤不醒而告警成环）。
    static func wakeFailureAlert(targetLabel: String, window: TimeInterval = receiptWindow) -> String {
        "唤醒失败：「\(targetLabel)」注入 \(Int(window))s 后仍未转入工作态，疑似卡死。"
            + "消息留待重投；机长可 inspect_session / nudge_session 解卡。"
    }
}
#endif
