import Foundation
import SwiftUI

/// session 头像右下角那**唯一**一颗状态点的语义（人类定调 2026-08-08）。
///
/// 此前头像上有两颗点：右上「注意红点」（未读 ∨ 异常 ∨ 进程死）＋ 右下「运行状态点」。
/// 三种点亮红点的情形里有两种右下角本来就已经变红 —— 重复且互相打架，于是合成一颗：
/// **右下角这颗**。左上角机长橙星不受影响。
///
/// 四态，红只留给「真需要人出手」：
/// - `working` 🟢 正在干活（跑回合）
/// - `idle`    🟡 空闲 / 排队待命
/// - `attention` 🔴 需要人出手：异常（未登录 / 额度到顶 / 拉起失败）**或**卡住等人回话
///   （`ask` 挂着、终端弹了选择菜单、说完停在一个问句上 —— 见 `SessionAwaitingReply`）。
///   只有这一态做呼吸动画。
/// - `exited`  ⚪ 已退出 / 停了
///
/// **未读消息不再点亮任何点**（人类明确要求）——未读只走切换条上的数字角标。
///
/// ⚠️ 与 crew 级状态点（`CrewStatusAggregation.dot`，侧栏 crew 色条右上角）是**两套**：
/// 那边红 = 任一 session 健康异常、黄 = 机长 `raise_attention` 点亮的 attention、
/// 绿 = 有人在干活、灰 = 有记录但全闲。**黄的含义完全不同**（那边是"机长要人看"，
/// 这边是"空闲"），改一边时别顺手改另一边。
enum SessionStatusDot: String, Equatable, CaseIterable {
    case working
    case idle
    case attention
    case exited

    /// 状态点颜色。切换条/成员列表/气泡头像共用这一份，别在视图里各写各的。
    var color: Color {
        switch self {
        case .working:   return .green
        case .idle:      return .yellow
        case .attention: return .red
        case .exited:    return .gray
        }
    }

    /// 只有「需要人出手」才呼吸 —— 动画是稀缺信号，滥用就不再是信号。
    var breathes: Bool { self == .attention }
}

/// session 状态字符串 → 状态点的**唯一**推导。视图一律走这里，不要现拼三元表达式。
///
/// 输入的状态字符串有两个来源，词表相同：
/// - 本机 run：`CrewSessionStateDerivation.state(isRunning:health:isWorking:awaitingDecision:)`
///   —— 它才是「这个 session 处于什么状态」的事实源（点名快照 / inspect_session 同一份）。
/// - 远端成员：`CrewMember.sessionStatus`（server 下发，用 running/queued 那套词）。
///
/// 两套词表在这里合流。新增状态词时**必须**同时补 `SessionStatusDotTests`
/// 里那条「CrewSessionStateDerivation 的每个输出都有归宿」的守卫测试。
enum SessionStatusDotDerivation {
    /// - Returns: `nil` = 不画点（无状态信息的成员，如 bot / 人类）。
    ///   未知状态词按「已退出」处理（灰）——宁可画得保守，也不要谎报成绿或红。
    static func dot(state: String?) -> SessionStatusDot? {
        guard let state, !state.isEmpty else { return nil }
        switch state {
        // 干活中。"running" 是远端成员的说法，等同本机的 "working"。
        case "working", "running":
            return .working
        // 存活待命 / 排队。
        case "idle", "queued":
            return .idle
        // 需要人出手的五类：
        // - error         健康异常（未登录 / key 失效）—— 进程活着但干不了活
        // - rateLimited   撞限额卡在等重置 —— 同样干不了活，人得决定换模型还是等
        // - launchFailed  压根没拉起来 —— 派给它的活等于没派出去，必须立刻改派
        // - awaitingDecision 卡在待决策/终端菜单上等人答 —— 不答就永远停在那
        // - awaitingReply 在等人回话（ask 挂着 / 说完停在一个问句上，Todo #25 层 2）
        //   —— 这类此前跟「空闲」长得一模一样，人不进右栏就永远不知道它在等
        case "error", "rateLimited", "launchFailed", "awaitingDecision", "awaitingReply":
            return .attention
        // 正常退出 = 灰，不是红：它不是在求助，只是干完/停了。
        // （「进程死」里真需要人管的那种是 launchFailed —— 从来没活过，已归红。）
        case "exited":
            return .exited
        default:
            return .exited
        }
    }
}
