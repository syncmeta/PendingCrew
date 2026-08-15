import Foundation

/// 「新起的这个 run 该不该跳过去」的**唯一**判定（人类 Todo #42）——纯函数，单测钉住。
///
/// 病：`CrewSessionRunner.start()` 过去无条件 `selectedRunId = run.runID`。你正开着 A
/// 谈事，机长在同一个群里 `start_session` 起了 B，右栏当场被切走。跨 crew 那半 #481
/// 修过（别的 crew 起的 run 只记进它自己的 pane 态），漏的是**同 crew** 这半。
///
/// 分界不是「哪个 crew」而是「**这一下是不是人自己点的**」：
/// - 人点的（新建面板提交 / 驾驶舱派单）→ 跳过去，人就是要看它。
/// - 程序起的（机长 `start_session` / @ 唤醒拉起 / 远程排队认领）→ 不抢。
/// - 例外：目标 crew 一个 run 都没选中时选中它 —— 右栏本来空白，没有「正在看的东西」
///   可被打断，谈不上抢（与 #40 留的口子一致）。
enum SessionForegroundClaim {
    enum Outcome: Equatable {
        /// 抢当前 crew 的前台（顺带退出「新建」态）。
        case selectForeground
        /// 只写进目标 crew 记住的 pane 态，等用户自己切过去。
        case rememberInPane
        /// 什么都不动 —— 那边已经有选中的 run，别顶掉。
        case leaveAlone
    }

    /// - Parameters:
    ///   - isForegroundCrew: 新 run 所属 crew 就是右栏当前这个（含「还没定 crew」）。
    ///   - userInitiated: 这次启动是人自己点出来的（见 `CrewSessionRunner.start`）。
    ///   - foregroundSelection: 当前右栏选中的 run（nil = 右栏空白）。
    ///   - paneSelection: 目标 crew 记住的选中 run（后台 crew 用）。
    static func decide(
        isForegroundCrew: Bool, userInitiated: Bool,
        foregroundSelection: UUID?, paneSelection: UUID?
    ) -> Outcome {
        if isForegroundCrew {
            return (userInitiated || foregroundSelection == nil) ? .selectForeground : .leaveAlone
        }
        return (userInitiated || paneSelection == nil) ? .rememberInPane : .leaveAlone
    }
}
