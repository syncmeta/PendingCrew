import Foundation

/// 「我这个 helper 进程跑的是哪一份二进制」的一次快照。
///
/// 为什么要有这个东西：**MCP 的工具表在 helper 启动那一刻就定死了**。
/// `--mcp-serve` 是个长命进程（跟着 session 活，可以活好几天），装新版只换掉
/// 磁盘上的文件，**活着的进程还是旧的**。于是新版加的工具 / 新加的参数档位，
/// 这个 session 一个都拿不到 —— 而它收到的拒绝话术跟「这个能力根本不存在」
/// 一模一样。**两件完全不同的事说同一句话**，agent 只会得出「没做」然后绕路走。
///
/// 对照：`--mcp-hook` 是每次调用现 spawn 的，换文件立刻生效。同一个 app 的
/// 两条腿，换代码的时机不同 —— 这条差别是这个类型存在的全部理由。
struct HelperBuildStamp: Equatable {
    /// 人读的版本串。**走 `AppBuildStamp.versionDisplay`，不自己拼** —— 仓库里
    /// 已经有一份「这一份构建是谁」的口径（版本 + build 号 + 构建戳 commit），
    /// 再发明第二种只会让两处慢慢说不一样的话。读不到 plist → nil。
    var versionText: String?
    /// 可执行文件的最后修改时刻。
    var modified: Date?
    /// 可执行文件字节数。
    var size: Int64?
    /// inode。**整份替换**（装新版、cp 一个新文件过来）会换 inode，
    /// 原地覆写则只动 mtime/size —— 三样一起看，两种换法都盖得住。
    var inode: UInt64?

    /// 说给人 / agent 看的那一句。**读不到就明说读不到**，不拿占位串假装能比对
    /// （跟 `AppBuildStamp` 里「不写占位 SHA」是同一条）。
    var displayText: String { versionText ?? "版本读不出来" }

    /// 读一份快照。`executable` 是 `<x>.app/Contents/MacOS/<x>`，
    /// Info.plist 按 bundle 惯例取 `<x>.app/Contents/Info.plist`。
    ///
    /// **Info.plist 每次都从磁盘现读，绝不走 `Bundle.main.infoDictionary`** ——
    /// 那份是进程启动时载进内存的，app 被换掉之后它还是旧的，拿它当「磁盘上
    /// 现在是什么版本」会稳定地报「一样」，正好把要抓的那件事抓漏。
    static func read(executable: URL) -> HelperBuildStamp? {
        let fm = FileManager.default
        guard let attrs = try? fm.attributesOfItem(atPath: executable.path) else { return nil }
        var stamp = HelperBuildStamp()
        stamp.modified = attrs[.modificationDate] as? Date
        stamp.size = (attrs[.size] as? NSNumber)?.int64Value
        stamp.inode = (attrs[.systemFileNumber] as? NSNumber)?.uint64Value

        let plist = executable
            .deletingLastPathComponent()          // …/Contents/MacOS
            .deletingLastPathComponent()          // …/Contents
            .appendingPathComponent("Info.plist")
        if let data = try? Data(contentsOf: plist),
           let info = (try? PropertyListSerialization.propertyList(
               from: data, options: [], format: nil)) as? [String: Any] {
            stamp.versionText = AppBuildStamp.versionDisplay(info: info)
        }
        return stamp
    }
}

/// 「这份在跑的」和「磁盘上那份」是不是同一个文件。**三态，不许压成 Bool** ——
/// 读不出来和一样是两件事（`.unknown` 被算成「是新的」正是这个类型要挡的那一刀）。
enum HelperBuildVerdict: String, Codable, Equatable, Sendable {
    case current
    case stale
    case unknown

    /// mtime 的比对容差。两侧的时刻来自不同的取法（`FileManager` 的属性 / 进程 vnode 的
    /// `vst_mtime` + `vst_mtimensec`），都换算成 `Double` 秒 —— 纳秒位上的舍入不该被
    /// 读成「换了文件」。一毫秒之内的两次构建不存在。
    static let mtimeTolerance: TimeInterval = 0.001

    /// 纯判定。**文件身份（inode / 大小 / mtime）三样缺一样就判不了** ——
    /// 一个「拿不准就当一样」的尺子会把读不出来显示成「是新的」。
    ///
    /// 版本串**只在两侧都读得出来时**参与比对：进程正在跑的那份，它的包可能已经被
    /// Sparkle 挪走删掉了，版本串读不出来，但身份三样仍然量得到 —— 不能因为少了一个
    /// 给人读的字段就放弃判定。
    static func judge(running: HelperBuildStamp?, onDisk: HelperBuildStamp?) -> HelperBuildVerdict {
        guard let running, let onDisk,
              let runningInode = running.inode, let diskInode = onDisk.inode,
              let runningSize = running.size, let diskSize = onDisk.size,
              let runningModified = running.modified, let diskModified = onDisk.modified
        else { return .unknown }
        if runningInode != diskInode || runningSize != diskSize
            || abs(runningModified.timeIntervalSince(diskModified)) > mtimeTolerance {
            return .stale
        }
        if let rv = running.versionText, let dv = onDisk.versionText, rv != dv { return .stale }
        return .current
    }
}

/// 点名快照里「这个成员的 helper 跑在哪一版」那一格。app 侧（编排者）写，
/// helper 的 `list_sessions` 和界面读。
struct HelperBuildReport: Codable, Equatable, Sendable {
    var verdict: HelperBuildVerdict
    /// 正在执行的那份的版本串（`HelperBuildStamp.displayText` 口径）。nil = 读不出来。
    var runningVersion: String?
    /// 磁盘上那份的版本串。nil = 读不出来。
    var diskVersion: String?
    /// `.unknown` 时为什么判不了；其余情况的补充说明。
    var reason: String?
    /// 找到了几个属于这个 session 的 helper 进程。
    var helperCount: Int
}

/// 界面 / 点名去查「某个成员的 helper 版本」时拿到的东西。**四种来源各说各的话**，
/// 其中三种都是「判不了」，但判不了的原因不同，人要据此做的事也不同。
enum HelperBuildLookup: Equatable {
    case report(HelperBuildReport)
    /// 快照里有这个人，但没有这一格 —— 写快照的那个进程比这个功能旧。
    case writerTooOld
    /// 快照里没有这个人（刚起来，下一拍快照还没写到）。
    case notInSnapshot
    /// 快照文件本身读不出来。
    case unreadable(String)
}

/// 界面上那枚小标。纯值，判定不长在 View 里。
struct HelperBuildBadge: Equatable {
    enum Tone: Equatable { case neutral, warning, unknown }
    var text: String
    var tone: Tone
    /// 悬停说明：完整那句话。
    var help: String

    /// 已退出的成员没有 helper → nil（不挂）。**判不了的四种来源一律是「版本?」**，
    /// 标面上绝不出现版本号 —— 一个数字摆在那儿，人就会读成「它跑的是这一版」。
    static func make(_ lookup: HelperBuildLookup, isRunning: Bool) -> HelperBuildBadge? {
        guard isRunning else { return nil }
        let column = HelperBuildReport.rosterColumn(lookup)
        guard case let .report(report) = lookup else {
            return HelperBuildBadge(text: "版本?", tone: .unknown, help: column)
        }
        switch report.verdict {
        case .current:
            return HelperBuildBadge(text: HelperBuildReport.shortVersion(report.runningVersion) ?? "当前版",
                                    tone: .neutral, help: column)
        case .stale:
            return HelperBuildBadge(text: "旧版", tone: .warning, help: column)
        case .unknown:
            return HelperBuildBadge(text: "版本?", tone: .unknown, help: column)
        }
    }
}

extension HelperBuildReport {
    /// `list_sessions` 每行那一列（界面那枚标的悬停说明也是这一句，同一份话）。
    static func rosterColumn(_ lookup: HelperBuildLookup) -> String {
        let notNew = " —— 不等于是新的"
        switch lookup {
        case let .report(r):
            switch r.verdict {
            case .current:
                return "🧩 helper \(r.runningVersion ?? "版本读不出来") · 与磁盘上那份一致"
            case .stale:
                return "⚠️ helper 是旧的：跑的 \(r.runningVersion ?? "版本读不出来（它的安装包已被挪走 / 删掉）")，"
                    + "磁盘上已是 \(r.diskVersion ?? "版本读不出来") —— 工具表是旧的，要新工具只能重开这个 session"
            case .unknown:
                return "❔ helper 版本判不了（\(r.reason ?? "原因没记下")）" + notNew
            }
        case .writerTooOld:
            return "❔ helper 版本判不了（写这份快照的后台比这个功能早，快照里没有这一格；后台换成新版之后才有）" + notNew
        case .notInSnapshot:
            return "❔ helper 版本判不了（快照里还没有这个成员，刚起来的话几秒后再看）" + notNew
        case let .unreadable(why):
            return "❔ helper 版本判不了（快照读不出来：\(why)）" + notNew
        }
    }

    /// `"0.1.32 (20709.1) · bbbbbbb"` → `"0.1.32"`。读不出来 → nil。
    static func shortVersion(_ display: String?) -> String? {
        guard let display, !display.isEmpty else { return nil }
        return display.components(separatedBy: " (").first
    }
}

/// 「本进程的二进制」与「磁盘上现在那份」的对照。
///
/// 只做一件事：**在 agent 看得见的地方**说清「你这个 session 的工具表是旧的」。
/// 只写日志不算 —— agent 不读日志，它只读回执。
final class HelperBuildWatch {
    /// 进程启动那一刻的快照。**必须在启动时取**：懒到第一次用才取的话，
    /// app 早换掉了，读到的是新版，于是永远判「一样」——这个检测器会以
    /// 一种最安静的方式失效（它从不发声，跟「没这个 bug」长得一模一样）。
    let running: HelperBuildStamp?
    /// 取上面那份快照的时刻，约等于本 helper 进程的启动时刻。
    let launchedAt: Date
    /// 现读磁盘。注入是为了单测能拿真文件走真逻辑，不是为了造替身。
    private let probe: () -> HelperBuildStamp?

    init(running: HelperBuildStamp?, launchedAt: Date = Date(),
         probe: @escaping () -> HelperBuildStamp?) {
        self.running = running
        self.launchedAt = launchedAt
        self.probe = probe
    }

    /// 生产入口：盯住本进程自己的可执行文件。
    static func captureAtLaunch(executable: URL? = Bundle.main.executableURL) -> HelperBuildWatch {
        guard let executable else {
            return HelperBuildWatch(running: nil, probe: { nil })
        }
        return HelperBuildWatch(running: HelperBuildStamp.read(executable: executable),
                                probe: { HelperBuildStamp.read(executable: executable) })
    }

    /// 磁盘上那份已经不是我跑的这份 → 一段给 agent 看的话；一样 / 看不出来 → nil。
    func staleNotice() -> String? {
        Self.notice(running: running, onDisk: probe(), launchedAt: launchedAt)
    }

    /// 纯判定。
    ///
    /// **两侧任一读不出来就闭嘴**：一个「拿不准就喊」的检测器在正常情况下也会喊，
    /// 看起来跟真检测一模一样，而它会训练人忽略它。宁可漏报。
    static func notice(running: HelperBuildStamp?, onDisk: HelperBuildStamp?,
                       launchedAt: Date) -> String? {
        // 比对走 `HelperBuildVerdict.judge` —— 跟点名 / 成员列表那一格同一把尺子。
        // 版本号一样但文件被覆写过也算「对不上」—— 工具表是跟着**那份二进制**走的，
        // 不是跟着版本号走的；开发机上同一版重新构建就是这个形状。
        guard HelperBuildVerdict.judge(running: running, onDisk: onDisk) == .stale,
              let running, let onDisk else { return nil }
        let since = launchedFormatter.string(from: launchedAt)
        return """
            ⚠️ 本 session 的工具表可能是旧的：这个 helper 进程跑的是 \(running.displayText)（起于 \(since)），\
            磁盘上的 PendingCrew 已经是 \(onDisk.displayText)。**工具表在 helper 启动那一刻就定死了**，\
            装新版只换磁盘上的文件，换不掉活着的进程。
            所以上面那句拒绝分不出两件事：这个能力这一版真的没有，还是这一版有、只是你这个进程太老。\
            要拿到新工具只能重开这个 session。
            """
    }

    static let launchedFormatter: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "MM-dd HH:mm"
        return f
    }()
}
