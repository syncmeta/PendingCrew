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
        guard let running, let onDisk else { return nil }
        // 全字段比。版本号一样但文件被覆写过也算「对不上」—— 工具表是跟着**那份
        // 二进制**走的，不是跟着版本号走的；开发机上同一版重新构建就是这个形状。
        guard running != onDisk else { return nil }
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
