#if os(macOS)
import Foundation

/// 「要不要问人恢复上次的 session」—— 弹窗那一下的判定层。
///
/// 人类给的规格（2026-09-11，他的原话是拿浏览器打的比方）：
///
/// > 什么情况下会恢复原来的标签页？比如更新、意外崩溃等等。这种情况下，弹出一个弹窗，
/// > 问是否恢复上一次的 session。否则，就不要自动恢复。
///
/// 落成两条：**只有「刚更新过」或「上次意外结束」才问**；问了他点头才恢复，
/// **一次都不自动**。
///
/// ## 第三条不在他嘴里，但不加就会变成骚扰
///
/// **没有可恢复的东西时，一次都不许问。** 崩溃时手上一个 session 都没有是常事
/// （刚开着窗什么也没干），那种时候弹一个「要恢复上次的 session 吗」，人点进去
/// 发现什么都没有 —— 弹两次他就再也不看这个窗了。
/// 一个会在没事时出现的提示，等于把有事时的那次也一起废掉。
///
/// ## 判定层为什么单独一个文件
///
/// 因为它必须进得了 test bundle。`SessionHost` / 视图都进不去，
/// 判定长在那里就只有「编得过」这一种证明 —— 今天全组抓到三处「建好了没接上」，
/// 都是这个形状。
enum SessionRestoreOffer {
    /// 上一轮在跑、这一轮可以拉回来的那些。来源是 daemon 的 registry
    /// （`SessionProcessRegistry.entries`）—— 它本来就在被收尸逻辑遍历。
    struct Candidate: Equatable {
        var sessionId: String
        var crewId: String
    }

    /// 为什么问。**必须能对人说清楚**，「要恢复吗」而不说为什么，人没法判断。
    enum Reason: Equatable {
        case unexpectedExit(ProcessExitClassification)
        case justUpdated(from: String, to: String)
    }

    struct Decision: Equatable {
        var reason: Reason?
        var candidates: [Candidate]
        /// 问还是不问。**nil reason 就是不问。**
        var shouldAsk: Bool { reason != nil && !candidates.isEmpty }
        /// 弹窗正文。不问的时候是空串。
        var message: String
    }

    /// - Parameters:
    ///   - exit: 上一轮是怎么结束的（`ProcessExitMarkerStore.classifyPreviousRun`）。
    ///   - previousBuild: 上一轮那枚印记里的版本；nil = 没有上一轮。
    ///   - currentBuild: 现在这个版本。
    ///   - candidates: 上一轮在跑的那些。**空 = 无论如何都不问。**
    static func decide(exit: ProcessExitClassification,
                       previousBuild: String?,
                       currentBuild: String,
                       candidates: [Candidate]) -> Decision {
        // 没东西可恢复 → 一次都不问。见类型注释第三条。
        guard !candidates.isEmpty else { return Decision(reason: nil, candidates: [], message: "") }
        // 从没跑过 → 不问。全新安装第一次开就弹窗是最糟的第一印象。
        guard exit != .noPriorRun else { return Decision(reason: nil, candidates: [], message: "") }

        // **意外结束优先于「刚更新」**：两者同时成立时（更新之后那一轮又崩了），
        // 该告诉人的是「崩了」，不是「更新了」—— 后者会让他以为是预期内的。
        let reason: Reason?
        if exit.shouldOfferRestore {
            reason = .unexpectedExit(exit)
        } else if let previousBuild, previousBuild != currentBuild {
            reason = .justUpdated(from: previousBuild, to: currentBuild)
        } else {
            reason = nil
        }
        guard let reason else { return Decision(reason: nil, candidates: [], message: "") }
        return Decision(reason: reason, candidates: candidates,
                        message: message(reason: reason, count: candidates.count))
    }

    /// 弹窗正文。**说清三件事**：发生了什么、有几个、点「不恢复」会怎样。
    static func message(reason: Reason, count: Int) -> String {
        let what: String
        switch reason {
        case let .unexpectedExit(classification):
            what = classification.text
        case let .justUpdated(from, to):
            what = "PendingCrew 刚从 \(from) 更新到 \(to)，后台随之重启了。"
        }
        return what
            + "\n\n上次有 \(count) 个 session 在运行。要把它们接回来吗？"
            + "\n（接回来会续上原来的对话；选「不恢复」的话它们就留在原地，"
            + "之后 @ 它们同样能接回来。）"
    }
}

/// 恢复动作跑完之后，对人说什么。
///
/// **这个类型存在的唯一理由是不让失败被包装成成功。** 今天全组抓到最多的一类 bug 就是
/// 「部分失败被汇总成一句好听的话」—— 人看到「已恢复」就走了，而三个 session 根本没回来。
///
/// 两条硬的：
/// - **有失败就必须把失败的那几个点名**，不许只给个数字。
/// - **全失败时一个字都不许出现「已接回」。**
///
/// 另外每个失败还要**各自落进它自己 crew 的白板、带 agent 的原话** —— 那一步在
/// 调用方（它要碰白板，进不了 test bundle），这里只管这句总结。
struct SessionRestoreOutcome: Equatable {
    struct Failure: Equatable {
        var sessionId: String
        var crewId: String
        /// agent / 拉起失败的**原话**，不要改写成「失败」。
        var reason: String
    }

    var restored: [String] = []
    var failures: [Failure] = []

    var allSucceeded: Bool { failures.isEmpty }

    /// 给人看的一句（弹窗回执 / 白板）。
    var summary: String {
        if restored.isEmpty && failures.isEmpty { return "没有需要接回的 session。" }
        if failures.isEmpty { return "已接回 \(restored.count) 个 session。" }
        let named = failures.map(\.sessionId).joined(separator: "、")
        if restored.isEmpty {
            return "\(failures.count) 个 session 都没接回来：\(named)。"
                + "失败原因已经写进各自 crew 的群里。"
        }
        return "接回了 \(restored.count) 个，**\(failures.count) 个没接回来**：\(named)。"
            + "失败原因已经写进各自 crew 的群里。"
    }
}
#endif
