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
        private let fd: Int32
        let holder: Holder
        init(fd: Int32, holder: Holder) {
            self.fd = fd
            self.holder = holder
        }
        deinit { flock(fd, LOCK_UN); close(fd) }
    }

    static func acquire(dataRoot: URL, kind: String) -> Outcome {
        let url = dataRoot.appendingPathComponent(fileName)
        do {
            try FileManager.default.createDirectory(at: dataRoot, withIntermediateDirectories: true)
        } catch {
            return .unavailable("建不出数据根 \(dataRoot.path)：\(error.localizedDescription)")
        }
        let fd = open(url.path, O_CREAT | O_RDWR, 0o600)
        guard fd >= 0 else {
            return .unavailable("打不开 \(url.path)：\(String(cString: strerror(errno)))")
        }
        guard flock(fd, LOCK_EX | LOCK_NB) == 0 else {
            let holder = readHolder(at: url)
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

    /// **只看，不写。** `acquire` 成功时会把自己的身份写进锁文件；探测不该有那个副作用
    /// （否则「问一句谁占着」会把锁文件改成自己的名字，下一个进程读到的就是假的）。
    static func currentHolder(dataRoot: URL) -> Holder? {
        let url = dataRoot.appendingPathComponent(fileName)
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        let fd = open(url.path, O_RDWR)
        guard fd >= 0 else { return nil }
        defer { close(fd) }
        if flock(fd, LOCK_EX | LOCK_NB) == 0 {
            flock(fd, LOCK_UN)          // 没人占着
            return nil
        }
        return readHolder(at: url)
    }

    private static func readHolder(at url: URL) -> Holder? {
        guard let data = try? Data(contentsOf: url) else { return nil }
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
