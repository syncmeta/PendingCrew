import Foundation

/// 进程启动时把打开文件数的**软上限**抬到硬上限（2026-08-12 P0 的触发闸）。
///
/// 病灶：macOS 上从 launchd 起的 GUI app 继承的 `RLIMIT_NOFILE` 软上限是 **256**
/// （`launchctl limit maxfiles` → `256 unlimited`）。shell 里看到的 1048576 是 shell
/// 自己的，所以「文件 644、终端里随手能读」和「app 读不动」能同时成立，误导了整整
/// 一天的排查方向。
///
/// 触发链：`whiteboards/` 目录 900+ 文件（每 session 一套 cursor/turn/lock）× 数十个
/// PendingCrew 进程，定时唤醒开火时那趟全机批量读一次顶穿 256 → `open()` 返 EMFILE
/// → Foundation 包成 `NSFileReadNoPermissionError`（「你没有权限查看此文件」）→
/// 上层当成文件损坏 → 归档重建。数据侧的根治在 `MultiProcessJSONStore` ④
/// （读失败永不销毁原件）；这里只是把墙推远，两件都要。
///
/// **这不是结构性修复**：fd 占用随 session 数线性涨，900+ 文件本身该收敛。
/// 结构问题记在 `docs/tech-debt.md`（2026-08-12 条目），不在这里连带重构。
enum FileDescriptorLimit {
    /// `RLIM_INFINITY` 是 C 宏（`sys/resource.h`：`(1<<63)-1`），Swift 导不进来，
    /// 在这里重述一次。`launchctl limit maxfiles` 第二列 `unlimited` 就是它。
    static let unlimited: rlim_t = rlim_t(Int64.max)

    /// 想要的软上限。硬上限是 `unlimited` 时用它封顶（无限软上限没意义，
    /// 也不礼貌）；硬上限是具体值时取 min。
    static let desiredSoftLimit: rlim_t = 65_536

    /// 抬软上限。幂等、失败即静默返回当前值 —— 抬不上去不该拦住 app 启动，
    /// 数据安全由 `MultiProcessJSONStore` 那道不变式独立保证。
    /// 返回抬完之后的软上限（诊断/测试用）。
    @discardableResult
    static func raiseSoftLimitToHardLimit() -> rlim_t {
        var limit = rlimit()
        guard getrlimit(RLIMIT_NOFILE, &limit) == 0 else { return 0 }
        let target = targetSoftLimit(soft: limit.rlim_cur, hard: limit.rlim_max)
        guard target > limit.rlim_cur else { return limit.rlim_cur }
        var raised = limit
        raised.rlim_cur = target
        guard setrlimit(RLIMIT_NOFILE, &raised) == 0 else { return limit.rlim_cur }
        return target
    }

    /// 纯判定（可单测）：给定当前软/硬上限，该把软上限抬到多少。
    /// 已经不低于目标 → 原样返回（不降级、不折腾）。
    static func targetSoftLimit(soft: rlim_t, hard: rlim_t) -> rlim_t {
        let ceiling = (hard >= unlimited) ? desiredSoftLimit : min(hard, desiredSoftLimit)
        return max(soft, ceiling)
    }
}
