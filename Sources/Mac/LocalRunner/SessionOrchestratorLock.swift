#if os(macOS)
import Darwin
import Foundation

/// **同一个数据根下只能有一个编排者** —— 不管它是 GUI 还是 `--daemon`（spec §6.2 闸门 2）。
///
/// ## 这个文件是被一次真事故逼出来的
///
/// 2026-08-26：app 正常跑着，同时起了一个 `--daemon`，**它照样起来了**，两个进程
/// 同时写 `whiteboards/quota.json` / `models.json` / `crew-sessions.json` 二十七秒 ——
/// 正是 §6.1 点名「单 writer + 原子整写，两个进程同时写就是无声的互相覆盖」的那三个。
/// 没出事只是因为 app 每两秒覆盖一次。**没有任何报错。**
///
/// 原因：那时的单实例锁只在 `--daemon` 那条路上取，**只排除 daemon-vs-daemon，
/// 不排除 daemon-vs-app**。而 §6.2 那道闸门要守的不变量是「**只有一个长期编排者**」，
/// 它跟你是哪副身份无关。
///
/// 所以现在：**凡是 `ProcessRole.requested == .orchestrator` 的进程都要来取这把锁**，
/// 取不到就当场拒绝接管编排，并且说清是**谁**占着。
///
/// ## 「谁占着」必须回答三样
///
/// pid、启动时刻、数据根路径。**缺一样人就得再查一轮**：只给 pid 认不出是不是复用的；
/// 只给启动时刻找不到人；不给数据根就答不出「我隔离设了没有生效」这个最常见的疑问。
///
/// ## 为什么是 flock 而不是「写个 pid 文件再看那个进程在不在」
///
/// flock 随进程消失由内核释放 —— **崩溃不会留下一把没人持有的锁**。pid 文件会，
/// 而且那种残留会把「谁也起不来」变成一个要人手删文件才能解的死结。
///
/// ## ⚠️ 上面那句保证要成立，`O_CLOEXEC` 是必要条件（2026-09-07 在真机上实测到）
///
/// flock 挂在**打开文件描述**上，而 fd 默认**跨 `exec` 继承**。daemon 拿着这把锁去
/// spawn agent 子进程，那把锁就同时被每一个子进程拿着 —— 实测当时：
/// ```
/// lsof orchestrator.lock → daemon 77461 + 它的 11 个 claude 子进程，全都 fd 3u
/// ```
/// 于是「进程一死锁就没了」**只对 daemon 自己成立**：daemon 崩了而 session 还活着时，
/// 锁被那些孤儿子进程继续held 着。下一个 daemon 来取锁会读到一个**已死 pid 的持有者**，
/// 判成「已经有一个 daemon 在跑」→ 安静退出 0 → **谁也起不来，而且没有任何报错**，
/// 直到最后一个子进程死掉为止。**这正是上面那段说 pid 文件才会有的死结。**
///
/// 换句话说：这个类型选 flock 的**全部理由**，是靠「没人给这些 fd 加 `O_CLOEXEC`」
/// 这件事在背后无声地否定的。所以两处 `open` 都带 `O_CLOEXEC`，并有测试钉着
/// （`testTheLockIsNotInheritedByChildProcesses`）。
enum SessionOrchestratorLock {
    /// 锁文件名。**它落在数据根下**（不是 Application Support 下）——
    /// `PENDINGCREW_DATA_DIR` 挪走数据根之后锁必须跟着挪，否则临时根里的进程会和
    /// 真 app 抢同一把锁，那就白挪了，而且症状是「隔离明明设了却还是打架」。
    static let fileName = "orchestrator.lock"

    /// 谁占着。写进锁文件的 JSON。
    struct Holder: Codable, Equatable {
        /// "app" / "daemon"。
        var kind: String
        var pid: Int32
        /// `kinfo_proc` 的启动时刻 —— 光有 pid 认不出复用（同 `SessionOrphanReaper`）。
        var startTimeSeconds: Int64
        var startTimeMicroseconds: Int32
        var dataRoot: String
        var acquiredAt: Date
    }

    enum Outcome {
        case acquired(Handle)
        /// 拿不到 —— 有人占着。读得出是谁就带上（读不出时为 nil，仍然要拒绝）。
        case heldBy(Holder?)
        /// 锁文件建不出来（目录不可写之类）。**这也要拒绝**：拿不到锁就没资格当
        /// 唯一所有者，而「打不开锁文件」和「锁被别人拿着」在后果上没有区别。
        case unavailable(String)
    }

    /// 持有句柄。**必须一直被持有** —— 释放（deinit）即解锁。
    final class Handle {
        private var fd: Int32
        let holder: Holder
        init(fd: Int32, holder: Holder) {
            self.fd = fd
            self.holder = holder
        }
        /// 幂等。正常路径靠 `deinit`；测试要**确定性地**在某一行放锁时用
        /// `releaseForTesting`（ARC 什么时候回收对象不是判据）。
        fileprivate func release() {
            guard fd >= 0 else { return }
            flock(fd, LOCK_UN)
            close(fd)
            fd = -1
        }
        deinit { release() }

        var isCloseOnExec: Bool {
            guard fd >= 0 else { return true }
            let flags = fcntl(fd, F_GETFD)
            return flags >= 0 && (flags & FD_CLOEXEC) != 0
        }
    }

    /// 显式放锁。**只给测试用** —— 生产代码一律靠持有 `Handle`、让它随作用域结束
    /// 自己放，那样「谁持有编排权」和「对象活着」是同一件事，少一处能忘的地方。
    static func releaseForTesting(_ handle: Handle) { handle.release() }

    /// 这把锁的 fd 会不会跨 `exec` 传给子进程。**测试用**（见类型注释里
    /// `O_CLOEXEC` 那一段）。
    ///
    /// 为什么钉的是这个标志位、而不是"起个子进程看它拿没拿到锁"：agent session
    /// 走的是 **SwiftTerm 的 `forkpty`**（裸 fork+exec，不关 fd），而
    /// Foundation 的 `Process` 在 Darwin 上默认就 CLOEXEC-all —— **拿 `Process`
    /// 起子进程去验，无论有没有 `O_CLOEXEC` 都是绿的**（2026-09-07 实测，我第一版
    /// 测试就是这么写的，变异之后照样绿）。判据要对着真正会继承的那条路。
    static func isCloseOnExecForTesting(_ handle: Handle) -> Bool { handle.isCloseOnExec }

    static func acquire(dataRoot: URL, kind: String) -> Outcome {
        let url = dataRoot.appendingPathComponent(fileName)
        do {
            try FileManager.default.createDirectory(at: dataRoot, withIntermediateDirectories: true)
        } catch {
            return .unavailable("建不出数据根 \(dataRoot.path)：\(error.localizedDescription)")
        }
        // O_CLOEXEC 见类型注释：不带它，这把锁会被每个 agent 子进程一起拿着，
        // 而「崩溃不留残锁」正是选 flock 的全部理由。
        let fd = open(url.path, O_CREAT | O_RDWR | O_CLOEXEC, 0o600)
        guard fd >= 0 else {
            return .unavailable("打不开 \(url.path)：\(String(cString: strerror(errno)))")
        }
        guard flock(fd, LOCK_EX | LOCK_NB) == 0 else {
            let holder = readHolder(fd: fd)
            close(fd)
            return .heldBy(holder)
        }
        let me = ProcessInfo.processInfo.processIdentifier
        let identity = SessionOrphanReaper.probe(pid: me)
        let holder = Holder(
            kind: kind, pid: me,
            startTimeSeconds: identity?.startTimeSeconds ?? 0,
            startTimeMicroseconds: identity?.startTimeMicroseconds ?? 0,
            dataRoot: dataRoot.path, acquiredAt: Date())
        ftruncate(fd, 0)
        lseek(fd, 0, SEEK_SET)
        if let data = try? JSONEncoder().encode(holder) {
            _ = data.withUnsafeBytes { write(fd, $0.baseAddress, data.count) }
            fsync(fd)
        }
        return .acquired(Handle(fd: fd, holder: holder))
    }

    /// 给人看的一句话。**三样必须都在**（pid / 启动时刻 / 数据根）。
    static func describe(_ outcome: Outcome, dataRoot: URL) -> String {
        switch outcome {
        case .acquired:
            return "本进程是这个数据根的编排者：\(dataRoot.path)"
        case let .heldBy(holder):
            guard let holder else {
                return "另一个 PendingCrew 进程正在管理这个数据根（\(dataRoot.path)），"
                    + "但锁文件里读不出它是谁 —— 本进程不接管编排。"
                    + "用 `lsof \(dataRoot.appendingPathComponent(fileName).path)` 能查出占着的 pid。"
            }
            let live = SessionOrphanReaper.probe(pid: holder.pid)
            let stillTheSame = live?.startTimeSeconds == holder.startTimeSeconds
                && live?.startTimeMicroseconds == holder.startTimeMicroseconds
            return "另一个 PendingCrew 进程正在管理这个数据根，本进程不接管编排。\n"
                + "- 谁：\(holder.kind == "daemon" ? "常驻后台进程（--daemon）" : "app 窗口")"
                + " pid \(holder.pid)"
                + (stillTheSame ? "" : "（注意：该 pid 现在的进程与锁里记的启动时刻对不上）")
                + "\n- 启动于：\(holder.startTimeDescription)"
                + "\n- 数据根：\(holder.dataRoot)"
        case let .unavailable(reason):
            return "拿不到编排锁，本进程不接管编排：\(reason)"
        }
    }

    /// 这个数据根上**现在有没有编排者、是谁** —— 三态，不是可选值。
    ///
    /// `currentHolder` 返回 `Holder?`，于是「没人占着」「占着但读不出是谁」「连锁文件
    /// 都问不出来」三件事被压成同一个 `nil`。对老调用点无所谓（它们只想知道「有没有
    /// daemon 在」），但对**停用命令**是致命的：把「我读不到」答成「没人在跑」，
    /// `--daemon-stop` 会报成功退 0，而后台还活着 —— 紧跟着的「清除本机所有数据」
    /// 就删在一个正在写的目录上。这跟 `CrewDirectory` 把「读不动」说成「查无此号」
    /// 是同一族 bug；仓库里已有的正确范式是 `WhiteboardCursor.read` 的三态，照它写。
    ///
    /// **不用 `FileManager.fileExists` 判「有没有锁文件」**：数据根本身不可读时
    /// （真机上 `chmod 500` 复现过）它同样返回 false，于是「我进不去这个目录」会被
    /// 说成「这里没有编排者」—— 元数据尺子恰好在你要诊断的那种故障下说谎。
    /// 这里只认 `open` 的 errno：`ENOENT` 才是「没有」。
    enum Presence: Equatable {
        /// 锁没人占着。
        case none
        case held(Holder)
        /// 有人占着（flock 拿不到），但读不出是谁。
        case heldByUnknown(String)
        /// 连问都问不出来。**不是「没人在」。**
        case undecidable(String)
    }

    static func presence(dataRoot: URL) -> Presence {
        let url = dataRoot.appendingPathComponent(fileName)
        let fd = open(url.path, O_RDONLY | O_CLOEXEC)
        guard fd >= 0 else {
            let code = errno
            if code == ENOENT { return .none }
            return .undecidable("打不开 \(url.path)：\(String(cString: strerror(code)))")
        }
        defer { close(fd) }
        if flock(fd, LOCK_EX | LOCK_NB) == 0 {
            flock(fd, LOCK_UN)          // 没人占着
            return .none
        }
        guard let holder = readHolder(fd: fd) else {
            return .heldByUnknown("锁被占着，但 \(url.path) 里读不出持有者；"
                + "`lsof \(url.path)` 能查出占着的 pid")
        }
        return .held(holder)
    }

    /// **只看，不写。** `acquire` 成功时会把自己的身份写进锁文件；探测不该有那个副作用
    /// （否则「问一句谁占着」会把锁文件改成自己的名字，下一个进程读到的就是假的）。
    ///
    /// 保留可选值形状是给老调用点用的（它们只问「有没有 daemon」，三态对它们是噪音）。
    /// **要区分「没人在」和「我读不到」的地方一律用 `presence`。**
    static func currentHolder(dataRoot: URL) -> Holder? {
        if case let .held(holder) = presence(dataRoot: dataRoot) { return holder }
        return nil
    }

    /// 从已经打开的 fd 读 —— 不走 `Data(contentsOf:)`，免得为了读一份已经在手上的
    /// 文件再开一次（那一次可能失败于完全不同的原因，而我们会把它记在同一笔账上）。
    private static func readHolder(fd: Int32) -> Holder? {
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while true {
            let n = read(fd, &buffer, buffer.count)
            if n > 0 { data.append(contentsOf: buffer[0..<n]); continue }
            if n == 0 { break }
            if errno == EINTR { continue }
            return nil
        }
        return try? JSONDecoder().decode(Holder.self, from: data)
    }
}

extension SessionOrchestratorLock.Holder {
    var startTimeDescription: String {
        guard startTimeSeconds > 0 else { return "未知" }
        let date = Date(timeIntervalSince1970: Double(startTimeSeconds)
            + Double(startTimeMicroseconds) / 1_000_000)
        return ISO8601DateFormatter().string(from: date)
            + String(format: ".%06d", startTimeMicroseconds)
    }
}
#endif
