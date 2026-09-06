import XCTest

/// Todo #101：「切换机长」在界面上必然失败，报「机长启动槽持续被其它唤醒占用」。
///
/// 这里钉的是那句错误信息**在指认错误的对象**：占着槽的不是别的唤醒，是它自己
/// 上一轮的尝试。前后端分离之后 GUI 进程里的 `runs` 是 daemon 那份 roster 的镜像，
/// 而 `launchCaptainForHandoff` 每一轮开头都无条件停掉本 crew 全部 captain run ——
/// 于是每一轮杀掉的正是上一轮刚请求起好的那个。
final class CaptainHandoffSiteTests: XCTestCase {

    // MARK: - 最小世界模型

    /// 「持有 run 的那个进程」与「本进程看到的镜像」之间差了几拍。
    ///
    /// 只建模一件事：**做**（停 / 起）作用在持有者那边，而**看**（roster 里有没有一个
    /// 在跑的机长）读到的是滞后 `mirrorLag` 拍的镜像。`mirrorLag == 0` = 本进程就是
    /// 持有者。一拍 = 有界重试循环的一轮（真实实现里是 200ms）。
    private struct HandoffWorld {
        let mirrorLag: Int
        /// 持有者那边当前这位机长是第几拍起来的；nil = 现在没有机长在跑。
        /// 循环开始前旧机长已被事务的 stopOld 停掉，所以初值是 nil ——
        /// 也就是说下面数到的每一次 kill 都是「自己杀自己」。
        private var runningSinceTick: Int?
        private(set) var startRequests = 0
        private(set) var selfKills = 0

        init(mirrorLag: Int) { self.mirrorLag = mirrorLag }

        /// 本进程**看得到**的 roster 里有没有一个在跑的机长。
        func rosterShowsRunningCaptain(at tick: Int) -> Bool {
            guard let since = runningSinceTick else { return false }
            return tick >= since + mirrorLag
        }

        /// `stopAndRemoveForCaptainHandoff(collisions)`：collisions 来自
        /// `runs.filter { ... }`，所以**看不到的就停不掉，看得到的就一定被停**。
        mutating func stopVisibleCaptains(at tick: Int) {
            guard rosterShowsRunningCaptain(at: tick) else { return }
            runningSinceTick = nil
            selfKills += 1
        }

        /// `startCaptain(...)`：持有者那边真的起了一个（viewer 侧是转发，daemon 侧是直起）。
        mutating func requestStart(at tick: Int) {
            startRequests += 1
            runningSinceTick = tick
        }
    }

    private enum Outcome: Equatable {
        case confirmed(attempt: Int)
        case exhausted
    }

    /// `launchCaptainForHandoff` 那个 `for attempt in 0..<30` 的逐句转写。
    private static func runBoundedLaunchLoop(_ world: inout HandoffWorld) -> Outcome {
        for attempt in 0..<CaptainHandoffSite.launchAttempts {
            world.stopVisibleCaptains(at: attempt)
            world.requestStart(at: attempt)
            switch CaptainHandoffSite.step(
                attempt: attempt,
                maxAttempts: CaptainHandoffSite.launchAttempts,
                rosterShowsRunningCaptain: world.rosterShowsRunningCaptain(at: attempt)
            ) {
            case .confirmed: return .confirmed(attempt: attempt)
            case .exhausted: return .exhausted
            case .retry: continue
            }
        }
        return .exhausted
    }

    /// 从 `site` 发起的交接，**最终执行它的那个进程**看到的 roster 滞后几拍。
    ///
    /// 这是路由决定的：就地执行 → viewer 只有滞后镜像；转交持有者 → 它看到的就是事实。
    private static func effectiveMirrorLag(isViewer: Bool) -> Int {
        switch CaptainHandoffSite.decide(isViewer: isViewer) {
        case .executeHere: return isViewer ? 1 : 0
        case .forwardToOwner: return 0
        }
    }

    // MARK: - 病灶

    /// 人在界面上点「切换机长」，界面在 daemon 模式下是 viewer。这必须成功。
    func testHandoffStartedFromViewerMustSucceed() {
        var world = HandoffWorld(mirrorLag: Self.effectiveMirrorLag(isViewer: true))
        let outcome = Self.runBoundedLaunchLoop(&world)
        XCTAssertEqual(outcome, .confirmed(attempt: 0), """
            从 viewer 发起的机长交接必须成功。实际结果=\(outcome)；\
            期间请求起新 \(world.startRequests) 次、\
            把自己上一轮刚起好的机长杀掉 \(world.selfKills) 次。\
            报给人的却是「启动槽持续被其它唤醒占用」——占着槽的是它自己。
            """)
    }

    /// 对照组：持有者进程里这条路今天就是好的（机长实测从 daemon 侧走没报过这个错）。
    /// 它保证上面那条红不是「模型对谁都红」。
    func testHandoffStartedFromOwnerSucceedsOnFirstAttempt() {
        var world = HandoffWorld(mirrorLag: Self.effectiveMirrorLag(isViewer: false))
        XCTAssertEqual(Self.runBoundedLaunchLoop(&world), .confirmed(attempt: 0))
        XCTAssertEqual(world.startRequests, 1)
        XCTAssertEqual(world.selfKills, 0)
    }

    // MARK: - 归属票

    /// 「记得先问一句归属」这条规矩不能靠人记住 —— 同一个文件里五处编排动作记住了、
    /// 交接的两个 GUI 入口没记住，这就是 Todo #101。票是把那句问话搬进
    /// `executeCaptainHandoff` 的参数表：拿不到就调不动，少一个参数编不过。
    func testOwnershipTicketIsOnlyIssuedWhereTheRunsActuallyLive() {
        XCTAssertNil(CaptainHandoffOwnership.claim(isViewer: true),
                     "viewer 只看得到镜像，不该拿到就地执行交接的票")
        XCTAssertNotNil(CaptainHandoffOwnership.claim(isViewer: false),
                        "持有 run 的进程必须拿得到票，否则交接谁也做不了")
    }

    /// 机制本身的反例，与路由无关：只要让这个循环跑在一份滞后的镜像上，它就必然
    /// 饿死，而且每一轮都杀掉自己上一轮的成果。**修法只能是别让它跑在镜像上**——
    /// 把 30 调大只是让饿死变慢，这条测试会一直在这里证明这一点。
    func testBoundedLoopOnALaggingMirrorStarvesAndKillsItsOwnWork() {
        var world = HandoffWorld(mirrorLag: 1)
        XCTAssertEqual(Self.runBoundedLaunchLoop(&world), .exhausted)
        XCTAssertEqual(world.startRequests, CaptainHandoffSite.launchAttempts)
        XCTAssertEqual(world.selfKills, CaptainHandoffSite.launchAttempts - 1)
    }
}
