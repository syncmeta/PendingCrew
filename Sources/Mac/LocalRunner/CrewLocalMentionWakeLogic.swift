#if os(macOS)
import Foundation

/// 后台 mention 唤醒器的**纯扫描核心**（wake-resilience 根因修复）。
///
/// 背景（2026-07-19 事故复盘）：session/机长经 `post_to_crew` 发的定向 @ 只落
/// 白板 JSON —— app 侧从来没有观察者把它转成对 idle run 的注入，idle worker
/// 因此永远收不到机长派工。撞限额前没暴露，纯因 worker 一直 busy、PostToolUse
/// hook 每轮把白板未读带进上下文兜了底；全员 idle 后这条从未存在的路径就成了
/// 「断链」。`CrewLocalMentionWaker` 监听白板变更补上活体路径；本类型是它的
/// 纯决策半边：从一批**新增**白板条目里滤出「该触发唤醒的定向 @」，转成
/// `CrewMention` 交给既有 `CrewLocalMentionInjectLogic.decide`（busy 不打断 /
/// 目标缺席拉起的语义原样复用，与人类 composer 路同一套核心）。
///
/// 过滤语义：
///   * 收 session / captain 作者的条目 —— 人类（user）在 composer 发送时
///     `CrewChatView.send()` 已直投，这里再投会重复注入。
///     （#554 断链修复的规则 3 —— 「远端人类经 relay 落进本地白板的 @ 也收」——
///     随 #63 第二期删除跨端遥控整层一起移除：它的判据是 `relayRemoteId != nil`，
///     relay 一走这个条件恒 false。**前后端解耦重建 relay 时要一起重建**，
///     否则远端人类的 @ 会重新变成没有投递者的断链。）
///   * 只收带 `@session` / `@captain` 的条目（broadcast / human 不唤醒具体
///     run，与 decide 一致）。
///   * **通讯录 `contact` 的跨 crew 来电例外（2026-08-11）**：`externalContactFrom`
///     非 nil 且没有定向 @ 的条目 = 外面按 crew 号打进来的广播，按 `@机长` 唤醒 ——
///     与「人类在群里无 @ 发言默认当 @机长」（`CrewLocalMentionDelivery`）同语义。
///     本群成员之间的普通广播仍然不唤醒任何人，语义不变。
///   * 系统条目（sessionId == "system"，如审批通告/唤醒失败告警）参与唤醒但
///     `trackReceipt == false` —— 回执失败的告警本身就是 system @captain 条目，
///     对它再做回执判定会告警成环。
enum CrewLocalMentionWakeLogic {

    /// 一条待唤醒投递（一条白板条目 → 至多一条）。
    struct PendingDelivery: Equatable {
        let entryId: String
        /// 只含 session / captain 两种（其余 kind 已滤掉）。
        let mentions: [CrewMention]
        /// 注入正文 = `agentText`（含附件路径提示行，与其它白板→agent 渲染口一致）。
        let messageText: String
        let senderName: String
        /// 作者 session id —— 唤醒器据此排除「自己 @ 自己」的注入；system/relay 为 nil。
        let senderSessionId: String?
        /// false = 系统条目，注入后不做回执判定（防告警环）。
        let trackReceipt: Bool
    }

    /// 陈旧 @ 不唤醒的年龄阈值（#595 最后一道防线）。
    ///
    /// 2026-08-12：白板游标集体悬空，几周前（7/25）的派工文案被当成新增逐条重放，
    /// 把全机 session 拉起来照过期指令返工 —— 代价是两位数的无效轮次。游标层已经
    /// fail-closed（`WhiteboardCursor` / `LocalWhiteboardStore.entries`），但那是可以
    /// 再被别的路径破掉的；这道闸独立于游标，破了游标也还有它兜着。
    static let maxWakeAge: TimeInterval = 6 * 60 * 60

    /// 扫描一批新增白板条目 → 待唤醒投递集（保持输入序）。
    /// 明显陈旧的条目（`maxWakeAge` 之前写的）直接丢，不拉起也不注入。
    static func pending(entries: [LocalWhiteboardMessage], now: Date = Date()) -> [PendingDelivery] {
        entries.compactMap { e in
            guard !isStale(e, now: now) else { return nil }
            guard e.senderKind == "session" || e.senderKind == "captain" else { return nil }
            var wakeMentions: [CrewMention] = (e.mentions ?? []).compactMap { m in
                switch m.kind {
                case "session":
                    guard let t = m.targetId, !t.isEmpty else { return nil }
                    return .session(t)
                case "captain":
                    return .captain
                default:
                    return nil   // broadcast / human：不唤醒具体 run
                }
            }
            // 跨 crew 来电的广播 → 当 @机长（外线打进来不能只躺在白板上）。
            if wakeMentions.isEmpty, e.externalContactFrom != nil { wakeMentions = [.captain] }
            guard !wakeMentions.isEmpty else { return nil }
            let isSystem = e.senderSessionId == "system"
            return PendingDelivery(
                entryId: e.id,
                mentions: wakeMentions,
                messageText: e.agentText,
                senderName: senderLabel(e),
                senderSessionId: isSystem ? nil : e.senderSessionId,
                trackReceipt: !isSystem)
        }
    }

    /// 这条 @ 是不是「明显陈旧」。时间戳解析不出来 → **不算**陈旧：这道闸是兜底，
    /// 不是主防线，宁可多投一条也不该因为脏时间戳静默吃掉一条真派工。
    ///（主防线是游标 fail-closed，那边反过来 —— 解析不了就按「不是新的」处理。）
    static func isStale(_ e: LocalWhiteboardMessage, now: Date) -> Bool {
        guard let at = CrewTimestamp.parse(e.createdAt) else { return false }
        return now.timeIntervalSince(at) > maxWakeAge
    }

    /// 扫描游标该钉在哪（`CrewLocalMentionWaker.pin` 的纯判定半边）。
    enum PinDecision: Equatable {
        /// 钉在这个位置。`nil` = 白板为空，没有锚点 —— 那是「钉之后的一切」，
        /// 语义正确，与「锚点悬空」是两回事。
        case pin(WhiteboardCursorPosition?)
        /// 这次不钉，等下一次白板事件再来（末条是读失败的合成行）。
        case retryLater
    }

    /// 白板当前内容 → 钉游标的决定。
    ///
    /// 末条是 `LocalWhiteboardStore.readFailureRowId` 时**不许钉**（#595）：那条警示行
    /// 只存在于内存，磁盘上根本没有，钉下去就是当场钉一个表里不存在的 id → 下一次扫描
    /// 把整部历史当新增。也不能退化成「不钉游标」之外的 nil —— nil 在
    /// `entries(in:after:)` 里同样等于全量。所以这次干脆不钉，留着下次事件重试。
    static func pinPosition(rows: [LocalWhiteboardMessage]) -> PinDecision {
        guard let last = rows.last else { return .pin(nil) }
        guard last.id != LocalWhiteboardStore.readFailureRowId else { return .retryLater }
        return .pin(WhiteboardCursorPosition(id: last.id, createdAt: last.createdAt))
    }

    /// 发送者标注（与 `HookEmitter.render` 的取名次序一致）：显示名优先，
    /// 机长兜底「机长」，再兜 `session:<前6>`，最后退回原 kind。
    static func senderLabel(_ e: LocalWhiteboardMessage) -> String {
        if let n = e.senderName, !n.isEmpty { return n }
        if e.senderKind == "captain" { return "机长" }
        if let sid = e.senderSessionId, !sid.isEmpty { return "session:\(sid.prefix(6))" }
        return e.senderKind
    }
}
#endif
