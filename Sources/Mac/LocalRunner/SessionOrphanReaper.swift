#if os(macOS)
import Darwin
import Foundation

/// daemon 崩了之后留下的 agent 子进程怎么办（spec §8.2）。
///
/// ## 这个文件存在的唯一理由：**pid 会复用**
///
/// 这台机器上常年十几个 agent 进程。daemon 记下 pid、崩掉、被拉起来，这中间那个
/// pid 完全可能已经属于别人了 —— 可能是用户的编辑器，可能是另一条线的 agent。
/// 只凭 pid（或 pgid）下手，迟早会误杀一个无辜进程，**而且是事后查不出来的那种
/// 事故**：被杀的那个进程只会看到自己莫名其妙没了，没有任何线索指向这里。
///
/// 所以判定必须双重核对：**registry 里记的启动时刻，与该 pid 当前的真实启动时刻
/// 一致，才动手**。`kinfo_proc` 的 `p_starttime` 精确到微秒，而 pid 复用要跨一整轮
/// pid 空间 —— 两者同时撞上的概率不是「小」，是「同一微秒里那个 pid 恰好又被
/// 分配给了一个 argv0 也一样的进程」。
///
/// **宁可留一个孤儿，也不能误杀。** 留下的孤儿写进日志与白板，由人决定怎么处理。
struct SessionProcessIdentity: Codable, Equatable {
    var pid: Int32
    var pgid: Int32
    /// `kinfo_proc.kp_proc.p_starttime` 的秒 + 微秒。**这两个数就是防误杀的那把锁。**
    var startTimeSeconds: Int64
    var startTimeMicroseconds: Int32
    /// 进程短名（`p_comm`，最多 16 字节）。只用于给人看的说明，**不参与判定** ——
    /// 它会被截断、也可能重名，当判据不够硬。
    var command: String

    var startTimeDescription: String {
        let date = Date(timeIntervalSince1970: Double(startTimeSeconds)
            + Double(startTimeMicroseconds) / 1_000_000)
        return ISO8601DateFormatter().string(from: date) + String(format: ".%06d", startTimeMicroseconds)
    }
}

/// 一条 registry 记录的处置。
enum SessionOrphanDecision: Equatable {
    /// pid 已经不在了 —— 记账，无事。
    case alreadyGone
    /// 启动时刻对得上，确认是我们的孤儿 → 杀进程组。
    case reap
    /// **pid 被复用了，绝不动手。** 记账 + 白板说明 + 日志留痕。
    case pidReused(current: SessionProcessIdentity)

    var killsSomething: Bool { self == .reap }
}

enum SessionOrphanReaper {

    // MARK: - 纯判定（这一半必须能单测，它是本文件的全部风险所在）

    static func decide(
        recorded: SessionProcessIdentity, current: SessionProcessIdentity?
    ) -> SessionOrphanDecision {
        guard let current else { return .alreadyGone }
        guard current.startTimeSeconds == recorded.startTimeSeconds,
              current.startTimeMicroseconds == recorded.startTimeMicroseconds else {
            return .pidReused(current: current)
        }
        return .reap
    }

    /// 给人看的一句话。**`pidReused` 那条必须说清「我没动手」** —— 静默留一个孤儿
    /// 和静默杀一个无辜进程一样查不出来。
    static func describe(
        sessionId: String, recorded: SessionProcessIdentity, decision: SessionOrphanDecision
    ) -> String {
        switch decision {
        case .alreadyGone:
            return "「\(sessionId)」的进程（pid \(recorded.pid)）已经不在了，无需处理。"
        case .reap:
            return "「\(sessionId)」的进程（pid \(recorded.pid)，启动于 "
                + "\(recorded.startTimeDescription)）启动时刻与记录一致，已回收其进程组。"
        case let .pidReused(current):
            return "「\(sessionId)」记的 pid \(recorded.pid) 现在属于**另一个进程**"
                + "（记录启动于 \(recorded.startTimeDescription)，实际启动于 "
                + "\(current.startTimeDescription)，命令 \(current.command)）—— "
                + "**pid 被复用了，我没有动它**。原来那个 session 的进程可能已经退出，"
                + "也可能还在别处跑着；要不要处理请人来定。"
        }
    }

    // MARK: - 系统调用那一半

    /// 读该 pid 当前的真实身份。进程不存在时返回 nil。
    static func probe(pid: Int32) -> SessionProcessIdentity? {
        guard pid > 0 else { return nil }
        var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]
        var info = kinfo_proc()
        var size = MemoryLayout<kinfo_proc>.stride
        let rc = sysctl(&mib, u_int(mib.count), &info, &size, nil, 0)
        // size == 0 是「调用成功但这个 pid 没有对应进程」——不是错误，别当错误处理。
        guard rc == 0, size > 0, info.kp_proc.p_pid == pid else { return nil }
        let comm = withUnsafeBytes(of: info.kp_proc.p_comm) { raw -> String in
            let bytes = raw.prefix(while: { $0 != 0 })
            return String(decoding: bytes, as: UTF8.self)
        }
        return SessionProcessIdentity(
            pid: pid,
            pgid: info.kp_eproc.e_pgid,
            startTimeSeconds: Int64(info.kp_proc.p_starttime.tv_sec),
            startTimeMicroseconds: Int32(info.kp_proc.p_starttime.tv_usec),
            command: comm)
    }

    /// 现在就为一个**活着的**进程拍一份身份，存进 registry。
    static func identity(forRunning pid: Int32) -> SessionProcessIdentity? { probe(pid: pid) }

    /// 执行处置。只有 `.reap` 会真的发信号；其余两种一个系统调用都不做。
    @discardableResult
    static func apply(_ decision: SessionOrphanDecision,
                      recorded: SessionProcessIdentity) -> Bool {
        guard decision == .reap else { return false }
        // 进程组：pty 子进程是 setsid 的会话首（组 id == pid），杀组能连带收掉它
        // 拉起的 bash/MCP 子进程。非组首则退回单杀。
        if killpg(recorded.pgid > 0 ? recorded.pgid : recorded.pid, SIGKILL) != 0 {
            kill(recorded.pid, SIGKILL)
        }
        return true
    }
}

/// 「这个 session 的子进程是谁」。daemon 的 registry 靠它填（§8.2）。
///
/// 它**不进 `SessionBackend`**：那个协议是控制面 + 状态面，是给编排和 UI 看的；
/// pid 只有常驻后台进程的善后逻辑要用，扩大生命周期协议只会让每个实现都被迫回答
/// 一个跟自己无关的问题。
@MainActor
protocol SessionProcessIdentifying: AnyObject {
    /// 直接子进程的 pid（PTY 的会话首 / codex app-server）。**0 = 还没起来**，
    /// 不是错误 —— 拉起是异步的，registry 会在下一次重建时补上。
    var agentProcessIdentifier: Int32 { get }
}

/// daemon 持续维护的「谁在跑」账本（§8.2）。它**不是** session 的业务状态，
/// 只回答一个问题：如果我下一秒崩了，回来之后该去看哪些 pid。
struct SessionProcessRegistry: Codable, Equatable {
    struct Entry: Codable, Equatable {
        var sessionId: String
        var crewId: String
        var identity: SessionProcessIdentity
    }

    var daemonPid: Int32 = 0
    var daemonStartedAt: Date = .distantPast
    var entries: [Entry] = []

    mutating func record(sessionId: String, crewId: String, identity: SessionProcessIdentity) {
        entries.removeAll { $0.sessionId == sessionId }
        entries.append(.init(sessionId: sessionId, crewId: crewId, identity: identity))
    }

    mutating func forget(sessionId: String) {
        entries.removeAll { $0.sessionId == sessionId }
    }
}
#endif
