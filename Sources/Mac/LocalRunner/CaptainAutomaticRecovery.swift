#if os(macOS)
import Foundation

/// P2 自动接任只接受可枚举的硬失效证据；静默、首输出迟、断链和旧账本本身都不是
/// 进程死亡的证明。这个纯判定不读磁盘、不探 CLI，也不启动任何 session。
enum CaptainAutomaticRecoveryTrigger: Equatable {
    case authenticationRequired
    case launchFailed
    case endedFailed
    case expectedLiveMissing(
        recordIsCaptain: Bool,
        recordBelongsToOldDaemonEpoch: Bool,
        probe: SessionProcessProbeResult
    )
    case unresponsive
}

struct CaptainAutomaticRecoveryContext: Equatable {
    let isCaptain: Bool
    let sourceKind: LocalCodingAgentKind
    /// 当前 crew 中除失效源之外已有 live captain 时绝不再起第二位。
    let hasOtherLiveCaptain: Bool
    let trigger: CaptainAutomaticRecoveryTrigger
}

enum CaptainAutomaticRecoveryDecision: Equatable {
    case recover
    case alertOnly
}

/// 一个 terminal event 不能同时拥有两条启动责任。Claude 明确拒绝 `--resume` 时，
/// 无 resume 降级已经是这一次结束的恢复决策；只有它没有取得所有权，且 captain
/// 真正以 `.failed` 结束时，P2 才可继续硬失效接任。
enum CaptainEndedRecoveryRoute: Equatable {
    case none
    case retryClaudeWithoutResume
    case automaticCaptainRecovery
}

enum CaptainEndedRecoveryOwnership {
    static func decide(
        resumeFallbackScheduled: Bool,
        isCaptain: Bool,
        exitReason: SessionExitReason?
    ) -> CaptainEndedRecoveryRoute {
        if resumeFallbackScheduled { return .retryClaudeWithoutResume }
        guard isCaptain, exitReason == .failed else { return .none }
        return .automaticCaptainRecovery
    }
}

enum CaptainAutomaticRecoveryPolicy {
    /// 自动替代的唯一资格口。它特意不接受 timeout、idle、断链或旧记录——那些最多
    /// 触发人工可见提示；只有这里的硬证据才会继续进入 durable claim。
    static func decide(_ context: CaptainAutomaticRecoveryContext) -> CaptainAutomaticRecoveryDecision {
        guard context.isCaptain,
              context.sourceKind == .claudeCode,
              !context.hasOtherLiveCaptain else { return .alertOnly }
        switch context.trigger {
        case .authenticationRequired, .launchFailed, .endedFailed:
            return .recover
        case let .expectedLiveMissing(recordIsCaptain, oldEpoch, probe):
            guard recordIsCaptain, oldEpoch, case .missing = probe else {
                return .alertOnly
            }
            return .recover
        case .unresponsive:
            return .alertOnly
        }
    }
}
#endif
