import Foundation

/// 点名的**产出证据**（人类 Todo #107 第三件：判活不判状态）。
///
/// 病：机长的 `list_sessions` 只报状态，而状态会骗人 —— 2026-09-06 两个 session
/// 一个卡 90 分钟、一个卡 110 分钟，状态**全显示「空闲」**，其实任务书压根没提交。
/// 机长照着「空闲」判断，等于被骗。同一天的另一次同类错误是反向的：有人从
/// 「这条被反复催」推出「它一定还没做完」，实际早做完了。两者是同一种病 ——
/// **拿表象当状态**。
///
/// 药：不问它显示什么，问它**最近真的写出过什么、什么时候** —— 读 runner 自己
/// 留下的会话成绩单的最近写入时刻。
///
/// # 为什么必须是三态
///
/// 曾有人提议判据用「`~/.claude/projects/<slug>/<会话号>.jsonl` 存不存在」。
/// mtime 可以用，但**「文件不存在」证明不了「没跑」** —— 那份成绩单要有第一条
/// 用户消息之后才生成，而且 codex 侧的 threadId 要握手成功才拿得到。把这种
/// 「问不出答案」压成 Bool（或 `Optional<Date>` 再 `??` 掉）就等于告诉机长
/// 「它确实一个字没产出」—— 一句言之凿凿的假话，比不报还坏。
/// （`try?` / `as?` / 三态压 Bool 是账上记过的常见真相丢弃点。）
///
/// 所以这里是三态，`rosterColumn` 也把三态渲染成机长分得清的三句话。
/// 守卫这条的是 `test_看不出来与确实没有产出是两态_压成Bool会把看不出来算成没跑`：
/// 那条测试是先写的，也先在一版故意压成 `Optional`-then-`??` 的实现上跑红过 ——
/// 尺子自己先证明会红，才轮到信它的绿。
enum SessionOutputEvidence: Equatable {
    /// 找到了这个会话的成绩单，最近一次写入是 `at`。
    case produced(at: Date)
    /// 该找的地方找过了，这个会话号一个字都没写过。**这是断言，不是回落值** ——
    /// 只有在取证面确实读得到、只是里面没有这一条时才允许用。
    case noOutput
    /// **取证面自己不在场**，这条路问不出答案。`reason` 是人话原因，要跟着渲染
    /// 出去 —— 机长得知道这不是「没干活」。
    case unknown(String)

    /// 点名那一行右边挂的一列。三态必须长得不一样，且「看不出来」那句
    /// **绝不能出现「没有产出」四个字**（单测钉着）。
    func rosterColumn(now: Date) -> String {
        switch self {
        case let .produced(at):
            return "📝 最近产出 \(Self.ago(from: at, to: now))（\(Self.clock.string(from: at))）"
        case .noOutput:
            return "🈳 确实没有产出（会话号记着，成绩单一个字都没有）"
        case let .unknown(reason):
            return "❔ 产出看不出来（\(reason)）—— 这不等于它没干活"
        }
    }

    /// 「多久以前」。故意只给到分钟：这一列是拿来看数量级的（刚刚 / 十几分钟 /
    /// 一个多小时），秒级精度对机长没有用。
    static func ago(from: Date, to: Date) -> String {
        let seconds = Int(to.timeIntervalSince(from))
        if seconds < 0 { return "就在刚才（时钟比文件还早）" }
        if seconds < 60 { return "刚刚" }
        let minutes = seconds / 60
        if minutes < 60 { return "\(minutes) 分钟前" }
        let hours = minutes / 60
        if hours < 24 {
            return minutes % 60 == 0 ? "\(hours) 小时前" : "\(hours) 小时 \(minutes % 60) 分钟前"
        }
        return "\(hours / 24) 天前"
    }

    /// 绝对时刻（本机时区）。相对时间给数量级，绝对时刻给对账用 —— 机长要把它
    /// 跟白板上某条消息的时间对上时，只有相对时间是不够的。
    private static let clock: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "MM-dd HH:mm"
        return f
    }()
}

/// 取证面：runner 自己留下的会话成绩单落在哪儿、最近什么时候被写过。
///
/// # 两家的取证面（2026-09-06 在本机实测，别凭印象改）
///
/// - **claude**：`~/.claude/projects/<项目 slug>/<会话号>.jsonl`。会话号是
///   PendingCrew 起进程前用 `--session-id` 指定的（`CrewSessionRunner` 立刻记进
///   `LocalAgentSessionStore`），所以**一个卡在信任提示上、任务书从没提交过的
///   claude session，账本里有会话号、磁盘上没有那份 .jsonl** —— 正是 #107 那个
///   现场，这一列会明说「确实没有产出」。
/// - **codex**：`~/.codex/sessions/<年>/<月>/<日>/rollout-<ISO 时间戳>-<threadId>.jsonl`。
///   threadId 要 app-server 握手回来才有，所以 codex 成员刚起来的头几秒天然是
///   「看不出来」，那**不是**「没干活」。
///
/// # 一条被证伪的建议
///
/// 有人建议改读 `~/.claude.json` 的 `projects[<cwd>]`（`lastDuration` / 帧数 /
/// `lastGracefulShutdown`）。**在本机实测过，对我们没用**：那份 telemetry 只有
/// 565 个项目条目里的 36 个有，而 **PendingCrew 成员跑的 507 个 worktree 目录里
/// 命中数是 0**；而且它是会话**收尾**时才写的，正在跑的 session（实测：写这段
/// 代码的这一个）在那份文件里压根没有条目。它证明不了「现在还活着」。
/// 完整实测记录在 `docs/session-output-forensics.md`。
///
/// # 为什么按会话号扫，不按 cwd 推 slug
///
/// 那个项目 slug 的编码规则是上游的实现细节（路径里的 `/` `.` 怎么换、大小写怎么
/// 处理），我们猜错一次的代价是**静默变成「确实没有产出」** —— 又一句言之凿凿的
/// 假话。所以这里只按会话号在 `projects/` 底下逐目录问一句「有没有这个文件」，
/// 编码规则怎么变都不影响。
struct SessionOutputProbe {
    let claudeProjectsDirectory: URL
    let codexSessionsDirectory: URL

    static func onThisMachine(home: URL = FileManager.default.homeDirectoryForCurrentUser) -> SessionOutputProbe {
        SessionOutputProbe(
            claudeProjectsDirectory: home.appendingPathComponent(".claude/projects"),
            codexSessionsDirectory: home.appendingPathComponent(".codex/sessions"))
    }

    /// `runnerKind` 传 `LocalCodingAgentKind.rawValue`（`claude_code` / `codex`）。
    /// 任何一个入参「不知道」都必须落进 `.unknown` —— 这个函数里没有一处
    /// `?? .noOutput`，也不许有。
    func evidence(runnerKind: String?, agentSessionId: String?) -> SessionOutputEvidence {
        let sessionId = (agentSessionId ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        guard !sessionId.isEmpty else { return .unknown("还没记下它的 agent 会话号") }
        switch runnerKind {
        case "claude_code":
            return newest(in: claudeProjectsDirectory, recursive: false,
                          matching: { $0 == "\(sessionId).jsonl" },
                          absentSurface: "~/.claude/projects 读不出来")
        case "codex":
            // rollout 文件名是 `rollout-<时间戳>-<threadId>.jsonl`，threadId 在
            // **尾段**。用后缀整段匹配，不用 `contains` —— 后者会被别的 thread
            // 名字里碰巧同形的片段撞上。
            return newest(in: codexSessionsDirectory, recursive: true,
                          matching: { $0.hasSuffix("-\(sessionId).jsonl") },
                          absentSurface: "~/.codex/sessions 读不出来")
        default:
            let named = (runnerKind?.isEmpty == false) ? "「\(runnerKind!)」" : "（没记 runner）"
            return .unknown("runner \(named) 没有我们认得的取证面")
        }
    }

    /// 在 `root` 底下找符合 `matching` 的文件，返回其中**最近一次写入**的时刻。
    ///
    /// 三态在这一层就分好：root 本身进不去 → `.unknown(absentSurface)`；
    /// 进得去但一个都没有 → `.noOutput`。**这两条分支绝不许合并**。
    private func newest(in root: URL, recursive: Bool,
                        matching: (String) -> Bool,
                        absentSurface: String) -> SessionOutputEvidence {
        let fm = FileManager.default
        var isDirectory: ObjCBool = false
        guard fm.fileExists(atPath: root.path, isDirectory: &isDirectory), isDirectory.boolValue else {
            return .unknown(absentSurface)
        }
        var newest: Date?
        if recursive {
            guard let walker = fm.enumerator(at: root, includingPropertiesForKeys: [.contentModificationDateKey])
            else { return .unknown(absentSurface) }
            for case let file as URL in walker where matching(file.lastPathComponent) {
                if let m = Self.modified(file) { newest = max(newest ?? m, m) }
            }
        } else {
            guard let children = try? fm.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)
            else { return .unknown(absentSurface) }
            // 只下探一层：成绩单就挂在 `projects/<slug>/` 里，再深的目录不是它。
            for dir in children {
                guard let files = try? fm.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil)
                else { continue }
                for file in files where matching(file.lastPathComponent) {
                    if let m = Self.modified(file) { newest = max(newest ?? m, m) }
                }
            }
        }
        guard let newest else { return .noOutput }
        return .produced(at: newest)
    }

    private static func modified(_ file: URL) -> Date? {
        (try? file.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate
    }
}
