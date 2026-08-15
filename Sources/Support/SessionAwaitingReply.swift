import Foundation

/// 「这个 session 正在等人回复」的**唯一**判定（人类 Todo #25 层 2）。
///
/// **要解决的病**：session 在 session 里问了句话就停住，界面上它跟正常空闲长得一模一样
/// （都是 🟡）。人不进右栏就永远不知道它在等，session 事实上死掉。层 1 补的是「回合结束
/// 群里必须留痕」，这一层补的是**界面上一眼看得出来** —— 判出「在等回复」就走已有的
/// 呼吸红点，不另起一套渲染。
///
/// **判据只认「有一个明确在等的对象」，不猜「它是不是在干活」**（活性判断另有其人，
/// 见 `SessionBackend.isWorking` / `SessionLaunchProbe`，两套口径不许分叉）：
///
/// 1. `.approval` —— `LocalApprovalStore` 里有本 session 的 pending 条目。`ask` 工具与
///    权限钩子都往这写，写完就 long-poll 阻塞在那儿。**零猜测**：进程此刻确实动不了。
/// 2. `.menu` —— 终端里认出了在等按键的选择菜单（`PendingTerminalDecision`，Todo #6）。
/// 3. `.question` —— 一轮说完，收尾**那句**是问句，且此刻不在吐字。这条是大白话提问的
///    唯一抓手（它既不调 `ask`、画面上也没有菜单，前两条一条都不占）。
///
/// **宁可少判几种也不要误报** —— 一个老是标红的界面等于没有标红。所以：
/// - 问句判定卡的是**最后一句**，不是「正文里出现过问号」。中途反问一句然后自己接着
///   干完的那种（「这样对吗？我先按 A 做了。」）不算在等。
/// - 还在吐字就不算在等：话没说完，谈不上等回复。用的是去抖后的显示态
///   （`SessionBackend.displayIsTyping`），不是生的 `isWorking` —— 后者被终端空闲心跳
///   重绘顶得一闪一闪（Todo #24），拿它当门会让红点跟着闪。
///
/// **进不去也出得来**：这是个每拍重算的纯函数，不是被点亮后等人来熄的驻留状态。
/// 拿到回复（approval 落 answered / 菜单消失）、被 nudge（开始吐字、下一轮结束覆盖问句）、
/// 被 stop、进程退出（`isRunning` 为假）——任何一条成立，下一拍自己就退出来了。
/// 「限额中」那个进得去出不来的坑（#545）正是因为它是驻留态且只有一条清除路径。
enum SessionAwaitingReply {
    enum Reason: Equatable {
        /// `ask` / 权限钩子挂着，进程阻塞在 long-poll 上。带的是问题摘要。
        case approval(String)
        /// 终端弹了选择菜单在等按键。带的是菜单问句。
        case menu(String)
        /// 一轮说完停在问句上。带的是那句问句。
        case question(String)

        /// 在等什么（点名快照 / inspect_session / tooltip 用）。
        var summary: String {
            switch self {
            case .approval(let s), .menu(let s), .question(let s): return s
            }
        }

        /// 中文短标签 —— 给人看的一眼判断，别在视图里各写各的。
        var label: String {
            switch self {
            case .approval: return "等答复"
            case .menu:     return "等拍板"
            case .question: return "等回复"
            }
        }
    }

    /// 判定输入。全部可注入 —— 时间、磁盘、终端画面都由调用方喂进来。
    struct Input: Equatable {
        /// 进程还活着（`run.status == .running`）。
        var isRunning: Bool
        /// `LocalApprovalStore` 里本 session 有 pending 条目（ask 或权限钩子）。
        var pendingApprovalSummary: String?
        /// 终端选择菜单的问句；非 nil = 有个菜单在等按键。
        var pendingMenuPrompt: String?
        /// 上一轮收尾那句问句（不是问句 / 没记到 = nil），由 `SessionTurnTrace.trailingQuestion`
        /// 在回合结束时算好落进 `SessionTurnMarker`，跨进程（claude 的 Stop hook 在
        /// helper 子进程里跑）也拿得到。
        var trailingQuestion: String?
        /// 此刻还在吐字（去抖后的显示态）。
        var isProducingOutput: Bool

        init(isRunning: Bool, pendingApprovalSummary: String? = nil,
             pendingMenuPrompt: String? = nil, trailingQuestion: String? = nil,
             isProducingOutput: Bool = false) {
            self.isRunning = isRunning
            self.pendingApprovalSummary = pendingApprovalSummary
            self.pendingMenuPrompt = pendingMenuPrompt
            self.trailingQuestion = trailingQuestion
            self.isProducingOutput = isProducingOutput
        }
    }

    /// 在等什么说不上来时的兜底文案 —— 只影响展示，不影响判定。
    private static let unnamed = "（没说在等什么）"

    /// nil = 没在等谁。优先级即列举顺序：阻塞态（approval）比画面态（menu）确凿，
    /// 画面态又比推断态（question）确凿 —— 同时成立时报最确凿的那条。
    static func reason(_ i: Input) -> Reason? {
        // 进程都没了，谈不上「在等」——「已退出」有它自己的灰点，别谎报成红。
        guard i.isRunning else { return nil }
        // approval 不看在不在吐字：ask 阻塞期间终端照样在转 spinner，那不是它在干活。
        // 摘要为空也照报 —— 「有条待办挂着」这件事本身才是判据，说明缺失不改变它在等。
        if let s = i.pendingApprovalSummary { return .approval(s.isEmpty ? unnamed : s) }
        if let p = i.pendingMenuPrompt { return .menu(p.isEmpty ? unnamed : p) }
        guard let q = i.trailingQuestion, !q.isEmpty, !i.isProducingOutput else { return nil }
        return .question(q)
    }
}
