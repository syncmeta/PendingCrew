#if os(macOS)
import Foundation

/// 事件驱动唤醒的 **纯决策核心**（Phase 4b）。
///
/// 输入：本 session 的 inbox mailbox 项（`getSessionInbox().mailbox`，server 端
/// 已只回未 delivered/processed 的行）+ 本 run 是否 busy（`SessionBackend.isBusy`）。
/// 输出：一个 `Decision` —— 要么「啥也不做」，要么「把这段文本注入唤醒、然后
/// 把这些 item id mark-delivered」。
///
/// 抽成纯函数让「@我了 + 空闲 → 注入什么、mark 哪些」可在不开 WS/HTTP 的前提下
/// 单测；活体 hub 帧 → 拉 inbox → 注入这圈 IO 编排留在 `CrewMailboxWaker` + E2E。
///
/// 语义要点：
///   * **busy → 不打断**。返回 `.noop`，**不** mark-delivered —— 让这些项留在
///     mailbox，下一次空闲（或 codex turn/completed 后的下一个事件）再带。
///   * **空闲 + 有待处理项 → inject**。文本用微信式短标头「有人@你：」+ 发送者
///     正文行（#484；「注入合法可信」的教学在 world-model 系统提示，注入面不重复），
///     并附上要 mark 的 item id。
///   * 防御性再过滤：server 已只回未投递行，但仍剔掉 status 落在
///     `delivered`/`processed` 的项（双 vocab 过渡期兜底，见 crew-comms.ts）。
enum CrewMailboxWakeLogic {

    /// 决策结果。
    enum Decision: Equatable {
        /// 不唤醒：没有待处理项，或本 run 正忙（busy 时不动 mailbox，留给下一轮）。
        case noop
        /// 唤醒：把 `injectText` 作为下一轮输入注入，注入后把 `deliveredIds`
        /// mark-delivered（避免下次重复注入）。
        case inject(text: String, deliveredIds: [String])
    }

    /// 已投递/已处理的 status —— 这些不再是「待处理」。其余（unread / 老 vocab /
    /// 未知）一律当待处理（fail-open：宁可多注入一次，也别漏掉@我的消息）。
    private static let terminalStatuses: Set<String> = ["delivered", "processed"]

    /// 核心决策。`isBusy == true` 时恒 `.noop`（不打断、不消费）。`recent` =
    /// 近期群聊上下文（项8：被 @ 唤醒的 claude 第一拍前置一块群聊历史；仅 claude
    /// 后端传，codex 传空——判定在调用方 `CrewMailboxWaker.wake`）。
    static func decide(
        mailbox: [CrewMailboxItem], isBusy: Bool,
        recent: [LocalWhiteboardMessage] = []
    ) -> Decision {
        guard !isBusy else { return .noop }
        let pending = mailbox.filter { !terminalStatuses.contains($0.status) }
        guard !pending.isEmpty else { return .noop }
        return .inject(text: renderInjection(pending, recent: recent), deliveredIds: pending.map(\.id))
    }

    /// 把待处理的定向 mailbox 项渲染成注入文本。单行前导「有人@你：」+ 每条保留
    /// 发送者身份（CC-P3）的正文行，点明**谁@你说了啥**。`recent` 非空 → 在
    /// 「有人@你：」**之前**前置一块「近期群聊」上下文（项8）。
    static func renderInjection(
        _ items: [CrewMailboxItem], recent: [LocalWhiteboardMessage] = []
    ) -> String {
        var lines = ["有人@你："]
        for it in items {
            lines.append("- \(senderLabel(it)): \(it.displayText)")
        }
        // #530:与本地直投路同款「先吱一声」提醒 —— 两条 @ 唤醒路注入口径一致。
        lines.append("（先 post_to_crew 吱一声「收到/我看看」再干活）")
        let directed = lines.joined(separator: "\n")
        guard let ctx = CrewRecentContextRender.block(recent) else { return directed }
        return ctx + "\n\n" + directed
    }

    /// 发送者标注。有 session 来源 → `session:<前6>`，否则按 kind 兜底
    /// （human → 人类，captain → 机长，其它 → 原 kind）。
    private static func senderLabel(_ it: CrewMailboxItem) -> String {
        if let sid = it.senderSessionId, !sid.isEmpty {
            return "session:\(sid.prefix(6))"
        }
        switch it.senderKind {
        case "user", "human": return "人类"
        case "captain": return "机长"
        default: return it.senderKind
        }
    }

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

    /// 判定一次唤醒注入是否真正到达。`workingSamples` = 注入后观察窗内周期采样的
    /// 目标工作态（`isBusy || isWorking` —— claude 的 `isBusy` 恒 false（PTY 无
    /// turn-state），真信号是输出活跃度 `isWorking`：注入被吃进去后 agent 起一轮
    /// turn，spinner/输出持续吐字；卡在模态菜单/进程假死时只有注入瞬间的一次
    /// 回显，之后整窗安静）。任一拍见工作态 → confirmed；全程安静（含空采样，
    /// 如 run 已退出）→ failed。
    static func receiptVerdict(workingSamples: [Bool]) -> ReceiptVerdict {
        workingSamples.contains(true) ? .confirmed : .failed
    }

    /// 唤醒失败的白板告警正文（调用方以 system 身份贴白板并 @captain —— system
    /// 条目免回执，不会因机长也唤不醒而告警成环）。
    static func wakeFailureAlert(targetLabel: String, window: TimeInterval = receiptWindow) -> String {
        "唤醒失败：「\(targetLabel)」注入 \(Int(window))s 后仍未转入工作态，疑似卡死。"
            + "消息留待重投；机长可 inspect_session / nudge_session 解卡。"
    }
}
#endif
