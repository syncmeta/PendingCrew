import Foundation

/// **本机数据根目录的唯一真值**：`~/Library/Application Support/PendingCrew/`。
///
/// ## 为什么要有这个类型
///
/// 在它出现之前，七个地方各自算一遍 `Application Support` 再拼 `PendingCrew` ——
/// 白板、crew 账、机长模板、附件、崩溃日志、清除数据、daemon 的锁与 registry。
/// 各算各的时它们**碰巧**一致，而「碰巧一致」在需要把整个数据根挪走的时候会露馅：
/// 挪得动六个、剩一个还指着真目录，于是那一个进程一半读临时账、一半读真账，
/// **而且不会响**（CONTRIBUTING 第 4 条就是这种形状）。
///
/// ## 为什么需要挪得动
///
/// 2026-08-26 P4 落地时实测出来的：这台机器上**没有安全的办法冒烟测 `--daemon`** ——
/// app 正在跑时起一个 daemon 就是两个编排者写同一批账（真发生过 27 秒，改了
/// `quota.json` / `models.json` / `crew-sessions.json` 三个「单 writer」文件）。
/// 而 daemon 恰恰只有在这台机器上才测得出来。`PENDINGCREW_DATA_DIR` 让整个数据根
/// 挪到临时目录，daemon 连同它读写的全部账本一起挪走，跟真 app 彻底不相干。
///
/// **它不是「配置项」，是「能不能验」的前提。** 别把它当成用户可调的东西写文档。
///
/// ## 三条纪律
///
/// 1. **所有人从这里拿路径**，不许再有第二个 `getenv`。
/// 2. **进程启动时解析一次，之后只读**。中途可变 = 世界从中间劈开。
/// 3. **不设环境变量时逐字等于原来那条路径**，`PendingCrewDataRootTests` 钉着这条。
enum PendingCrewDataRoot {
    /// 覆盖数据根的环境变量。**不设 = 完全现状。**
    static let overrideEnvKey = "PENDINGCREW_DATA_DIR"

    /// 本进程的数据根。第一次取用时算一次，之后固定（同 `ProcessRole.requested`）。
    static let url: URL = resolve(
        environment: ProcessInfo.processInfo.environment,
        applicationSupport: FileManager.default.urls(
            for: .applicationSupportDirectory, in: .userDomainMask).first)

    /// 纯判定（可单测）。
    ///
    /// - `applicationSupport` 取不到时退到临时目录 —— 与被它取代的那七处的兜底
    ///   一致（那几处分别退到 `temporaryDirectory` 或 `NSTemporaryDirectory()`，
    ///   是同一个目录的两种写法）。
    static func resolve(environment: [String: String], applicationSupport: URL?) -> URL {
        let override = (environment[overrideEnvKey] ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if !override.isEmpty {
            return URL(fileURLWithPath: (override as NSString).expandingTildeInPath,
                       isDirectory: true)
        }
        let base = applicationSupport ?? FileManager.default.temporaryDirectory
        return base.appendingPathComponent("PendingCrew", isDirectory: true)
    }

    /// 数据根**是不是被环境变量挪走了**。`startupLine` 与 daemon 日志命名共用这一条
    /// 判定 —— 两处各算一遍就会有一天只改了其中一处。
    static let isOverridden: Bool = !(ProcessInfo.processInfo.environment[overrideEnvKey] ?? "")
        .trimmingCharacters(in: .whitespacesAndNewlines).isEmpty

    /// 数据根下的一个子目录（`whiteboards` / `attachments` / `crashes` …）。
    ///
    /// ⚠️ **运行时状态就住在 `whiteboards/` 里面**，不在数据根下：`quota.json` /
    /// `models.json` / `crew-sessions.json` / 控制通道 / todo / 审批 / 唤醒账
    /// 全部在那一层。所以「挪数据根」必须连它一起挪 —— 白板挪走了、状态还留在
    /// 老地方那种一半的隔离，比不隔离更难查。本文件把所有人收在同一个根下面，
    /// 就是为了让这件事不可能做到一半。
    static func subdirectory(_ name: String) -> URL {
        url.appendingPathComponent(name, isDirectory: true)
    }

    /// **新文件的出生地**，故意**不在数据根里**（`~/Library/Caches/PendingCrew/staging`）。
    ///
    /// 2026-09-12 量出来的：那个周期性 EPERM 故障拦的是「在 Application Support 底下
    /// 出生的文件」—— 标记在**创建时**按创建位置打上，之后跟着文件走（`clonefile`
    /// 都复制得过去），发作期间带标记的一律 `open()` 被拒。
    /// 而**在外面建好再 `rename` 进来的文件没有标记**，发作期间照样读得动、
    /// 之后原地改写也照样读得动（三个落脚点各验过一遍）。
    ///
    /// 所以整写一份账本时，临时文件建在这里再挪进去，那份账本就对这个故障免疫。
    /// Foundation 的 `.atomic` 把临时文件建在**目标目录里**，正是每份账本
    /// 从出生起就带标记的原因。
    ///
    /// ⚠️ **这是绕路，不是根治** —— 拒绝来自哪个系统策略仍然不知道
    /// （要发作当口的 `sudo` 抓取，见 `scripts/capture-eperm-fsusage.sh`）。
    /// 根治之后这条可以退役，但**退役前要先确认故障真的没了**，别看它安静就撤。
    /// 现场：`docs/internal/2026-09-12-eperm-marker-travels.md`
    ///
    /// 必须与数据根**同卷**，否则 `rename` 会 EXDEV —— 跨卷时调用方退回原子写。
    static let stagingDirectory: URL = {
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first
            ?? FileManager.default.temporaryDirectory
        return caches.appendingPathComponent("PendingCrew/staging", isDirectory: true)
    }()

    /// 启动时说一句「我到底在读写哪儿」。
    ///
    /// **这一行不是装饰。** 2026-08-26 那次事故是「daemon 悄悄跑在了真目录上」；
    /// 而这套隔离机制自己的失败形态是**方向相反的同一种静默**——「它悄悄跑在了
    /// 临时目录上」：人以为在动真数据，其实在动一个空壳，**而所有操作都会成功**。
    /// 两种都只有一个便宜的检测器，就是启动时把路径打出来。
    /// - `root` 只为单测：让「跑一遍启动那条路」不必碰真目录。生产上两个调用点
    ///   （daemon 的 `SessionDaemonHost.start`、GUI 的
    ///   `OrchestrationGate.installForGUIProcess`）都走默认值。
    static func startupLine(root: URL = url) -> String {
        return "数据根 = \(root.path)"
            + (isOverridden ? "（来自 \(overrideEnvKey)，**不是**默认目录）" : "（默认）")
    }
}
