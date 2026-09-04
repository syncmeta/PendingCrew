#if os(macOS)
import Foundation

/// **这一轮该不该再拉一个 daemon**（从 `ViewerSessionClient.connect` 里挪下来的判断）。
///
/// 挪下来的理由只有一条：**逻辑一律不许留在接线里。** 那个文件在
/// `Sources/Mac/Services`、进不了 test bundle，判断留在那儿就只有「编译过」，
/// 没有任何人能证明它对 —— P4 那次「闸门挂错对象 → 在 app 侧无法被证明」是同一个形状。
///
/// ## ⚠️ 这一层防的是「白拉一次进程」，**不是防双头**
///
/// 别把它当成安全闸门 —— **防双头是锁的事**（daemon 自己的单实例锁 + §9.2 那条
/// 「拿到独占锁才许接管」）。这里问 `daemonHoldsLock` 不是为了拦住第二个编排者，
/// 而是因为那种情况下**再拉一个纯属白拉**：它起来就会撞上单实例锁、当场自己退掉，
/// 白造一次进程还把日志弄脏。
///
/// 两件事混在同一个判定里写，以后就分不清哪条约束在生效 —— 而分不清的那一天，
/// 删掉这里任何一条都会看起来「没影响」，直到某一次真的双头。
///
/// 两条规则的性质**不一样，要分清**：
/// - 「锁上写着有 daemon 在跑就不拉」是**契约推出来的**（第二个 daemon 会撞上单实例锁
///   当场退掉，白造一次进程还弄脏日志）。
/// - 「上次拉起来的那个还活着就不再拉」是**实现时自己加的**：超时判成「说不准」之后
///   照常重连，而重连又走到拉起这一步，于是每一轮都再造一个不回话的 daemon。
///   自己发明的规则更需要有人盯着 —— 没有第二份文档能对照它。
enum DaemonLaunchPlan {

    enum Step: Equatable {
        /// 别拉，直接连。字符串是「为什么不拉」，进日志。
        case connectOnly(String)
        case launch
    }

    /// - Parameters:
    ///   - daemonHoldsLock: 编排锁此刻被一个自称 daemon 的进程占着吗
    ///     （`SessionDaemonControl.runningDaemonPid` 的判据，不是「pid 文件里那个在不在」）。
    ///   - lastSpawnedChild: 本轮重连里上一次我们拉起来的那个子进程现在的样子；
    ///     nil = 这一轮还没拉过。
    static func next(daemonHoldsLock: Bool,
                     lastSpawnedChild: DaemonLaunchRace.ChildState?) -> Step {
        if daemonHoldsLock {
            return .connectOnly("编排锁被一个 daemon 占着，它已经在跑了")
        }
        if lastSpawnedChild == .alive {
            return .connectOnly("上一次拉起来的后台进程还活着，不再拉第二个")
        }
        return .launch
    }
}
#endif
