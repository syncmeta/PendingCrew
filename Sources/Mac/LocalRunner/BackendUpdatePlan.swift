#if os(macOS)
import Foundation

/// 「前端更新了，本机后端也跟着换代」—— 判定层。
///
/// 人类的原话（2026-09-11）：
///
/// > 我希望 pendingcrew 要有管理后端的能力 本机的后端也是一个 要能管理这些的更新
/// > 如果前端更新了 本机有后端 也一起更新
///
/// 病灶是实测出来的：**后台正是最不换代的那个**。它由 app 拉起来之后就一直跑，
/// app 更新过好几轮它还是老的（本机量到过连续跑几天、名下十几个 session，
/// 而同一时刻有四个不同版本的 claude CLI 在跑）。
///
/// ## 这一层只回答一个问题：该不该把本机后端换掉
///
/// 换掉的代价是**那些 session 会断**。在人类 9-11 拿掉「session 不能断」这条约束
/// 之前，这件事做不了；现在可以了 —— 断了用 resume 接回来（弹窗问他）。
///
/// ## 「拿不准」倒向哪一侧：这里和退出印记**相反**，要说清为什么
///
/// 退出印记那边，拿不准时倒向「问一次」——多问一次只是打扰。
/// **这里拿不准时倒向「不换」**——换错一次会把正在干活的 session 全打断。
/// 同一个「三态」形状，安全的那一侧由**误判的代价**决定，不是由习惯决定。
///
/// 所以 `.undecidable` 不换，但**必须大声说**（调用方落日志）：一个永远换不了代的
/// 后台和一个每次启动都被打断的后台一样糟，只是它安静。
enum BackendUpdatePlan {
    /// 本机后端现在是什么情况。
    enum BackendState: Equatable {
        /// 没有独立后端进程（app 自己在编排，或者压根没起过）。
        case none
        /// 有，而且问出了它的版本。
        case running(build: String, pid: Int32, sessionCount: Int)
        /// 有个后端在，但问不出版本（握手超时 / 协议对不上 / 读不到）。
        case undecidable(String)
    }

    enum Decision: Equatable {
        /// 什么都不做。`why` 说清为什么 —— 这条**不是**沉默。
        case leaveAlone(why: String)
        /// 换代：停掉旧的，起新的。
        case replace(oldBuild: String, newBuild: String, sessionCount: Int)

        var shouldReplace: Bool { if case .replace = self { return true }; return false }
    }

    /// - Parameters:
    ///   - backend: 本机后端的实况。
    ///   - appBuild: 当前这个 app 的版本（`SessionDaemonHost.currentBuild`，
    ///     **别新造第二种版本表示**）。
    static func decide(backend: BackendState, appBuild: String) -> Decision {
        switch backend {
        case .none:
            return .leaveAlone(why: "本机没有独立后端进程，没有要换的东西。")
        case let .undecidable(detail):
            // 见类型注释：拿不准时不换，但不许静默。
            return .leaveAlone(why: "有后端在，但问不出它的版本，**不动它**（换错会打断正在跑的 session）：\(detail)")
        case let .running(build, _, sessionCount):
            guard build != appBuild else {
                return .leaveAlone(why: "本机后端已经是 \(build)，跟界面同版。")
            }
            return .replace(oldBuild: build, newBuild: appBuild, sessionCount: sessionCount)
        }
    }

    /// 换代之前要在群里说的那句。**不是装饰**：人必须分得清「我的 session 断了」
    /// 和「出事了」——同样是断，一个是预期内的，一个不是。
    static func announcement(oldBuild: String, newBuild: String, sessionCount: Int) -> String {
        var text = "界面已经更新到 \(newBuild)，本机后台还是 \(oldBuild)，正在把它一起换掉。"
        if sessionCount > 0 {
            text += "\n这会中断当前在跑的 \(sessionCount) 个 session；换完之后会问你要不要把它们接回来。"
        } else {
            text += "\n当前没有在跑的 session，不会打断任何东西。"
        }
        return text
    }

    /// 第一次装上带这个能力的版本时，**这段逻辑自己还住在老后端里**，所以它不可能
    /// 自己生效 —— 必须人手重起一次后台。这句话是给发版说明用的，
    /// 写成代码里的常量是为了让它跟实现待在一起、不会各自漂。
    static let chickenAndEggNote =
        "首次升级到带「后端跟随更新」的版本时，这段逻辑本身还在旧后台里，"
        + "所以那一次需要手动重启一次后台（或退出并重开 PendingCrew）；此后自动。"
}
#endif
