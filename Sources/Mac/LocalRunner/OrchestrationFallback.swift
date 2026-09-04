#if os(macOS)
import Foundation

/// **翻默认之后「后台起不来」那一天的硬契约**
/// （设计 §9.2，2026-09-04 由父机长拍板）。
///
/// ## 为什么需要它
///
/// 总闸默认切成 daemon 之后，GUI 进程的身份是 viewer。viewer 连不上时有两种走法，
/// **两种都能翻车，而且翻法相反**：
///
/// - **只连不退**：用户双击图标，界面在、什么都不动、不报错 —— 这条线上**最贵**的
///   翻车形状，也正是这一整期在修的那种静默。
/// - **连不上就自己接管**：如果那边其实**有**一个 daemon 在编排（只是这次没连上），
///   就当场造出双头 —— 两个进程写同一批账、同一批唤醒发两遍，事后极难定位。
///
/// 所以不是二选一，是**按「能不能确定没有别人在编排」分岔**。判据用现成的
/// `SessionOrchestratorLock.Outcome`，不另造一套真值来源。
///
/// ## 表本身（每一行都有一条会红的测试盯着）
///
/// | 观测到的 | 允许回退本地编排？ |
/// |---|---|
/// | `acquired` 且拉 daemon **确实失败** | ✅ 允许（**唯一**允许的一支） |
/// | `heldBy(holder)` 且 `holder.kind == "daemon"` | ❌ 那边真的在，继续重连 |
/// | `heldBy(nil)` —— 锁被占着但读不出是谁 | ❌ **归属不明 = 不许接管** |
/// | `heldBy(holder)` 且 kind 不是 daemon | ❌ 冲突，摆到用户面前 |
/// | `unavailable(reason)` —— 锁文件打不开 | ❌ 拿不到锁就没资格当唯一所有者 |
/// | 连上了但**协议不兼容** | ❌ 可操作错误（能力集取交集是另一回事） |
/// | 连上了但 **attach / 握手失败** | ❌ 可操作错误 + 重试 |
///
/// **这条契约不许被「让用户少看见一个错误」的理由削弱** —— 静默本地接管带来的双头，
/// 症状是账被两个进程交替覆盖、唤醒发两遍，比一个说得清的错误横幅贵得多。
///
/// ## 纯判定，时间与 I/O 都在外面
///
/// 输入是三样已经观测到的事实，输出是一个动作。于是上表每一行都能当场断言，
/// 而且**把任何一行改成「允许」，对应那条测试立刻红**（`OrchestrationFallbackTests`
/// 里有一条专门的负向对照说明这件事）。
enum OrchestrationFallback {

    /// viewer 连不上后台的那一刻，允许做什么。
    enum Decision: Equatable {
        /// 继续退避重连。**界面上仍要看得出「还没连上」**，只是不构成接管的理由。
        case keepConnecting(String)
        /// 回退本地编排 —— 表里**唯一**允许的一支。
        case takeOverLocally(String)
        /// 既不接管也不假装正常：可操作的错误 + 重试 / 诊断入口。
        case refuse(String)

        var reason: String {
            switch self {
            case let .keepConnecting(r), let .takeOverLocally(r), let .refuse(r): return r
            }
        }
    }

    /// 拉起 daemon 这一步的结果。
    enum Spawn: Equatable {
        /// 还没试（比如锁上写着已经有 daemon 在跑）。
        case notAttempted
        /// 进程起来了。**起来了 ≠ 连得上** —— 那是下一步的事，不构成接管的理由。
        case launched
        /// **明确失败**：exec 没成，或者它在**完成握手之前**就自己退掉了。
        /// 这是允许接管那一支的必要条件之一。
        case failed(String)
        /// **不确定**：等到上限了，握手没来，而进程还活着。
        ///
        /// 这一态必须 **fail-closed**：既不许回退本地（我们并不知道那边有没有人在
        /// 编排），也不许继续无限「正在连接」——「无限的正在连接」是这一期要消灭的
        /// 那种静默的温和版本。所以它走可操作错误那条路。
        case uncertain(String)
    }

    /// 拉起这一步的**赛果** → `Spawn`。
    ///
    /// ## 为什么不能只看 `Process.run()` 抛没抛错（2026-09-04 真机逮到）
    ///
    /// daemon 在**拿不到编排锁**、或者**打不开锁文件**（数据目录不可写）时会
    /// `host.start()` 抛错、打一行原因、然后**当场自己退掉，而且两种都 exit 0**。
    /// 拉起方只看 `run()` 没抛错的话，这两种都会被记成「起来了」——于是
    /// `decide` 拿到 `.launched`、不去取锁、返回 `keepConnecting`，
    /// app 就永远挂在「正在连接后台进程…」上。
    ///
    /// **后果是契约有一半在空跑**：唯一允许接管的那一支要求 `spawn == .failed`，
    /// 而 `.failed` 那时只在 exec 本身失败（二进制没了 / 不可执行）时才成立 ——
    /// 那是罕见分支，「后台起不来」最常见的原因反而一条都到不了。症状恰恰是
    /// 这一期在消灭的那种「界面在、什么都不动」。
    ///
    /// ## 判据不是「睡够了没有」，是**哪件事先发生**
    ///
    /// 见 `DaemonLaunchRace`：并行等**首次协议握手**与**子进程终止**。
    /// 这里只把赛果翻译成契约的输入，**一个字都不去解析退出码** —— daemon 对
    /// 「别人占着锁」（正确结局）和「目录坏了」（真失败）都 exit 0，从退出码上本来
    /// 就分不出，那正是该由**锁的观测**去分辨的事（`decide` 那张表已经准备好了）。
    static func spawn(launchThrew: String?,
                      race: DaemonLaunchRace.Outcome,
                      limit: TimeInterval) -> Spawn {
        if let launchThrew { return .failed("拉不起后台进程：\(launchThrew)") }
        switch race {
        case let .exitedBeforeHandshake(code):
            let tail = code.map { "（退出码 \($0)）" } ?? ""
            return .failed(
                "后台进程起来了，但在握上手之前就自己退掉了\(tail) —— 它没起成。"
                + "常见原因是数据目录不可写、或者已经有别的进程占着编排锁。")
        case .timedOutStillAlive:
            return .uncertain(
                "拉起后台进程之后等了 \(Int(limit)) 秒仍然没握上手，而它还在运行 —— "
                + "起没起成说不准。用 `PendingCrew --daemon-status` 问一下它的实况；"
                + "或者把它停掉再重试。**本窗口不会自己接管编排**：那边可能真的有人在管账。")
        case .handshake, .pending:
            return .launched
        }
    }

    /// 同上，但握手成功时返回 `nil` —— 那一趟压根不该走降级判定，它已经连上了。
    static func spawnIfNotConnected(launchThrew: String?,
                                    race: DaemonLaunchRace.Outcome,
                                    limit: TimeInterval) -> Spawn? {
        if launchThrew == nil, case .handshake = race { return nil }
        return spawn(launchThrew: launchThrew, race: race, limit: limit)
    }

    /// 连上之后才可能出现的失败。它们**一律不构成接管的理由** ——
    /// 能回话的对端说明那边有东西在，接管就是双头。
    enum LinkFailure: Equatable {
        case protocolIncompatible(String)
        case handshakeFailed(String)
        case attachFailed(String)

        var detail: String {
            switch self {
            case let .protocolIncompatible(d):
                return "连上了后台，但协议对不上：\(d)\n"
                    + "多半是新旧版本混跑。请更新到同一版，或停掉旧的后台再重试。"
            case let .handshakeFailed(d):
                return "连上了后台的 socket，但握手没完成：\(d)\n"
                    + "后台可能卡住了。可以先 `PendingCrew --daemon-status` 问一下实况，再重试。"
            case let .attachFailed(d):
                return "连上了后台，但取不到 session 的画面：\(d)\n请重试；仍不行就把后台停掉重起。"
            }
        }
    }

    /// - Parameters:
    ///   - lock: 本进程**这一刻**试着取编排锁的结果。`nil` = 还没到取锁那一步
    ///     （比如 daemon 刚起来、还在等它监听）。**只在拉 daemon 明确失败之后才去
    ///     取锁** —— 抢在前面取会让我们自己拉起的那个 daemon 因为锁被占而退出。
    ///   - spawn: 拉起 daemon 的结果。
    ///   - linkFailure: 链路层已经发生的失败（nil = 没有）。
    /// - Parameters:
    ///   - stalledFor: 这一串重连已经连不上多久了（nil = 没在计）。
    ///   - stallLimit: 「有人在编排、我继续重连」这句话的**寿命**。见下面
    ///     `heldBy(daemon)` 那一支的注释。
    static func decide(lock: SessionOrchestratorLock.Outcome?,
                       spawn: Spawn,
                       linkFailure: LinkFailure?,
                       dataRoot: URL,
                       stalledFor: TimeInterval? = nil,
                       stallLimit: TimeInterval = 60) -> Decision {
        // 0) 不确定态 fail-closed：既不接管（可能真有人在管账），也不继续无限等。
        if case let .uncertain(reason) = spawn { return .refuse(reason) }
        // 1) **链路层的失败压过一切。** 对端刚才回过话 = 那边有东西在，
        //    不管这一刻锁在谁手上都不许接管。顺序不能挪到锁后面：锁**可能**恰好
        //    到手（daemon 崩在握手之后、锁刚被内核释放），那一瞬接管就是双头。
        if let linkFailure { return .refuse(linkFailure.detail) }

        // 2) 还没到取锁那一步 —— 比如 daemon 刚被拉起来、还在等它开始监听。
        //    「进程起来了」不是「连得上」，但也**不是**接管的理由，继续退避重连。
        guard let lock else { return .keepConnecting("正在连接后台进程…") }

        switch lock {
        case .acquired:
            // 锁到手 = 确定没有别人在编排。**但还不够** —— 必须同时确认
            // 拉 daemon 确实失败，否则我们会在它正要起来的那一瞬把位置占掉。
            guard case let .failed(reason) = spawn else {
                return .keepConnecting("正在连接后台进程…")
            }
            return .takeOverLocally(
                "后台起不来（\(reason)），已临时由本窗口接管编排。\n"
                + "本窗口现在持有 \(dataRoot.path) 的编排锁，所以不会有第二个进程同时管账；"
                + "**关掉这个窗口，正在跑的 session 也会跟着停**。"
                + "要回到后台模式：重开 PendingCrew。")

        case let .heldBy(holder?) where holder.kind == "daemon":
            // 那边确实有人在编排（它自称 daemon、锁也真在它手上）。接管 = 当场双头，
            // 所以**一律不接管**。但——
            //
            // **观测到有人在编排，只够否决「接管」，不够支撑「无限期沉默地等」。**
            //
            // 反例是现成的：**一个卡住的 daemon 照样持着锁**。锁在手、kind 写着
            // daemon，观测到的确实是「有人在编排」；但它不回握手。于是这句
            // 「后台进程正在运行，本窗口继续重连」会**永远转下去** —— 那是第 4 条
            // 要消灭的那种静默的另一种穿法，而且措辞更让人安心，所以更难被发现。
            //
            // 所以这句话有**寿命**：超过 `stallLimit` 之后状态不变（仍然不接管），
            // 变的只是「用户看不看得出这事不对劲」。
            if let stalledFor, stalledFor >= stallLimit {
                return .refuse(
                    "后台进程 pid \(holder.pid) 一直没有回应（已经 \(Int(stalledFor)) 秒）。"
                    + "本窗口**不会**接管编排 —— 那边锁在手，接管就是两个进程同时管账。\n"
                    + "用 `PendingCrew --daemon-status` 问一下它的实况；"
                    + "确认它卡住了就把它停掉，然后重试。")
            }
            return .keepConnecting(
                "后台进程正在运行（pid \(holder.pid)），本窗口继续重连。")

        case .heldBy(nil):
            // **整张表的重点。** 「读不出是谁」最像「那大概没人在管，我来吧」，
            // 而它恰恰最危险：读不出不等于没有。归属不明 = 一律不接管。
            return .refuse(
                "另一个进程正占着 \(dataRoot.path) 的编排锁，但锁文件里**读不出它是谁** —— "
                + "归属不明，本窗口不接管编排（接管就可能变成两个进程同时管账）。\n"
                + "用 `lsof \(dataRoot.appendingPathComponent(SessionOrchestratorLock.fileName).path)` "
                + "查出占着的 pid；确认它该退出就停掉它，然后重试。")

        case .heldBy:
            // 锁被一个**不听 socket** 的东西占着（另一个 inproc 窗口、崩到一半的进程）。
            return .refuse(
                SessionOrchestratorLock.describe(lock, dataRoot: dataRoot)
                + "\n本窗口既不接管编排也不连过去 —— 那边不听 socket，连过去只会得到一个"
                + "永远连不上的窗口。停掉它之后重试。")

        case let .unavailable(reason):
            // 拿不到锁就没资格当唯一所有者（锁自己的注释里就是这么写的）。
            return .refuse(
                "取不到 \(dataRoot.path) 的编排锁：\(reason)\n"
                + "拿不到锁就没资格当唯一所有者，所以本窗口不接管编排。"
                + "多半是数据目录的权限/磁盘问题，修好之后重试。")
        }
    }
}
#endif
